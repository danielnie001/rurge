# Surge 兼容性清单

> 本文档逐项列出 Surge（macOS 6.9 / iOS 5.22 时期的官方手册）的配置项、规则、策略、参数、脚本 API、HTTP API、CLI 与 URL Scheme，并标注 rurge 的兼容计划与实现阶段。
> 它是 [需求文档](requirements.md) 的附录：需求文档描述"做什么、为什么、做到什么程度"，本清单回答"每一个 Surge 配置项在 rurge 里会怎样"。
> 所有条目目前均处于**计划**状态，尚无实现。清单会随各阶段实现进度更新。

## 图例

| 标记 | 含义 |
| --- | --- |
| ✅ | 计划完全支持，语义与 Surge 一致 |
| 🟡 | 计划支持，但语义有差异或只支持一部分（备注列说明） |
| 🔁 | 解析并忽略：保持配置文件可加载，记录一条日志，不产生行为 |
| ⛔ | 不支持（平台限制、专有协议或明确的非目标） |
| ❓ | 待评估，需要在对应阶段的设计文档中决定 |

阶段编号与需求文档一致：1 核心骨架 · 2 出站协议与策略组 · 3 增强模式（TUN） · 4 HTTP 引擎 · 5 脚本与模块 · 6 API/Dashboard/工具 · 7 网关与服务端能力 · 8 桌面 GUI · 远期 = 不在当前路线图内。

平台缩写：Win = Windows，Lin = Linux，mac = macOS。Surge 手册中的 "Mac only" / "iOS only" 标注保留在"Surge 平台"列。

---

## 1. 配置文件格式与指令

### 1.1 基本语法

| 项 | Surge 行为 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| INI 风格：`[Section]` + `key = value` 或逐行条目 | 键值节顺序无关；有序节顺序即行为 | ✅ | 1 | |
| 有序节 | `[Rule]` `[Host]` `[URL Rewrite]` `[Header Rewrite]` `[Body Rewrite]` `[Map Local]` `[Panel]` `[Port Forwarding]` `[Script]` `[SSID Setting]` `[Ruleset <name>]` | ✅ | 1 | 解析器阶段 1 完成，各节语义按所属阶段实现 |
| 未识别的节 | 原样保留，不报错 | ✅ | 1 | rurge 不改写用户配置文件，只在内存中保留 |
| 注释 | 行首 `#` `;` `//`；行内注释前需至少一个空格 | ✅ | 1 | |
| 引号值 | 值可用 `"` 包裹，`\"` 与 `\\` 转义（iOS 5.21 / Mac 6.8+） | ✅ | 1 | |
| 值含逗号 | 规则值含逗号时用单/双引号包裹 | ✅ | 1 | |

### 1.2 节清单（23 个）

| 节 | 用途 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| `[General]` | 全局设置 | ✅ | 1 | 逐键见第 2 节 |
| `[Proxy]` | 代理策略 | ✅ | 1 / 2 | 阶段 1 仅内置策略别名，阶段 2 全协议 |
| `[Proxy Group]` | 策略组 | ✅ | 2 | |
| `[Rule]` | 规则 | ✅ | 1 | |
| `[Host]` | 本地 DNS 映射 | ✅ | 1 | `script:` 值依赖阶段 5 |
| `[URL Rewrite]` | URL 重写 | ✅ | 4 | |
| `[Header Rewrite]` | 头部重写 | ✅ | 4 | |
| `[Body Rewrite]` | 正文重写 | ✅ | 4 | |
| `[Map Local]` | 本地映射（Mock） | ✅ | 4 | |
| `[MITM]` | HTTPS 解密 | ✅ | 4 | |
| `[Keystore]` | 证书与私钥 | ✅ | 2 | |
| `[SSID Setting]` | 子网设置 | ✅ | 3 | 需要网络环境探测（SSID/BSSID/网关） |
| `[Script]` | 脚本 | ✅ | 5 | |
| `[Panel]` | 信息面板 | ✅ | 6 | 在 Dashboard / API 中呈现 |
| `[Ponte]` | Surge Ponte | ⛔ | 远期 | 依赖 iCloud 账号体系 |
| `[Port Forwarding]` | 端口转发 | ✅ | 7 | |
| `[Testing]` | 吞吐测试 | ✅ | 6 | 参数见 9.9 |
| `[DHCP]` | DHCP 服务器（Mac 网关模式） | ✅ | 7 | |
| `[Snell Server]` | 内置 Snell 服务器 | 🟡 | 7 | 支持的协议版本待阶段 2 评估后确定 |
| `[MTProto]` | 内置 MTProto 服务器 | ✅ | 7 | |
| `[WireGuard <name>]` | WireGuard 策略配置 | ✅ | 2 | |
| `[Tailscale <name>]` | Tailscale 策略配置 | ❓ | 远期 | 需要嵌入 Tailscale 客户端实现 |
| `[Ruleset <name>]` | 内联规则集 | ✅ | 1 | |

### 1.3 Profile 类型与拆分

| 项 | Surge 行为 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| 普通 Profile | 手工创建 | ✅ | 1 | |
| 托管 Profile `#!MANAGED-CONFIG <URL> interval=<秒> strict=<bool>` | 首行声明；定期从 URL 更新；`interval` 默认 86400；`strict=true` 时到期必须更新成功 | ✅ | 1 | 守护进程常驻，更新不受"主程序是否运行"限制；strict 失败时保留旧配置并告警 |
| 企业 Profile | 不可查看/修改/复制 | ⛔ | — | 非目标 |
| `#!include <file>` 单文件 | 节内容来自另一个文件；UI 可编辑并写回 | 🟡 | 1 | 读取语义完全支持；"写回"在无 UI 的核心中不适用 |
| `#!include A.dconf, B.dconf` 多文件 | 节为只读 | ✅ | 1 | |
| 混合内容与 include（iOS 5.22 / Mac 6.9+） | include 在语句位置展开，有序节中位置决定优先级 | ✅ | 1 | |
| 通配命名节 `[Ruleset *]` `[WireGuard *]` `[Tailscale *]` + `#!include` | 加载另一配置中全部同类命名节 | ✅ | 1 | |
| 远程 `#!include <URL>`（Linked Profiles，Mac 6.0+） | 引用远程托管配置并自动更新；本地只存覆盖层 | ✅ | 1 | 缓存与更新周期沿用托管配置逻辑 |
| 模块（`.sgmodule`） | 见 1.5 | ✅ | 5 | |

### 1.4 行级 Requirement 表达式

| 项 | Surge 行为 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| `#!REQUIREMENT <expr> <line>`（行首形式） | 条件不满足则该行视为禁用/注释 | ✅ | 1 | |
| `<line> #!REQUIREMENT <expr>` / `<line> //!REQUIREMENT <expr>`（行尾形式） | 同上 | ✅ | 1 | |
| `#!IOS-ONLY` / `#!MACOS-ONLY` / `#!TVOS-ONLY` | 简写 | 🟡 | 1 | `#!MACOS-ONLY` 仅在 macOS 上生效；`#!IOS-ONLY` `#!TVOS-ONLY` 永不生效 |
| 变量 `CORE_VERSION`（整数） | 新编码 `major*1000000+minor*1000+patch`；旧值 20/22 | ✅ | 1 | 按需求文档 FR-CFG-08 的递增方案报告：阶段 1 为 20，阶段 2 完成 smart 组后为 22，阶段 5 后切换到新编码并对齐已实现的 Surge Mac 版本 |
| 变量 `SYSTEM` | `iOS` / `macOS` / `tvOS` | 🟡 | 1 | rurge 报告 `macOS` / `Windows` / `Linux` |
| 变量 `SYSTEM_VERSION` `DEVICE_MODEL` `LANGUAGE` `DEVICE_NAME` | 字符串 | 🟡 | 1 | `DEVICE_MODEL` 在桌面平台返回硬件型号字符串（尽力而为） |
| 比较运算符 `=` `==` `>=` `=>` `<=` `=<` `>` `<` `!=` `<>` | | ✅ | 1 | |
| 逻辑运算符 `AND` `&&` `OR` `\|\|` `NOT` `!` | | ✅ | 1 | |
| 字符串运算符 `BEGINSWITH` `CONTAINS` `ENDSWITH` `LIKE` `MATCHES` | | ✅ | 1 | |
| 字符串用 `'` 包裹；含空格的整个表达式用 `"` 包裹 | | ✅ | 1 | |
| `#!FORBIDDEN-AUTO-UPGRADE smart-group` | 禁止把 url-test/load-balance 自动升级为 smart | 🔁 | 1 | rurge 不做自动升级，解析后忽略 |

### 1.5 模块（`.sgmodule`）

| 项 | Surge 行为 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| 模块来源：内部 / 本地文件 / URL 安装 | | 🟡 | 5 | rurge 无"内部模块"；本地与 URL 安装支持 |
| 优先级 | 模块设置高于 Profile | ✅ | 5 | |
| 可覆盖节 | `[General]` `[MITM]` `[WireGuard *]` `[Ruleset *]` `[Rule]` `[Script]` `[URL Rewrite]` `[Header Rewrite]` `[Body Rewrite]` `[Map Local]` `[Host]` | ✅ | 5 | 行式节的新行插入到原内容顶部 |
| `key = value` 覆盖 / `%APPEND%` 追加 / `%INSERT%` 前插 | | ✅ | 5 | |
| `[MITM]` 限制 | 只能改 `hostname` 与 `skip-server-cert-verify`，不能改 CA | ✅ | 5 | |
| `[Rule]` 限制 | 插入顶部，策略只能是 `DIRECT` `REJECT` `REJECT-TINYGIF` | ✅ | 5 | |
| 不能修改 `[Proxy]` `[Proxy Group]` | | ✅ | 5 | |
| `#!name=` `#!desc=` | 元数据 | ✅ | 5 | |
| `#!system=mac\|iOS\|tvOS` | 平台限制 | 🟡 | 5 | `mac` 视为适用于所有桌面平台；`iOS`/`tvOS` 模块被禁用并告警 |
| `#!arguments=` `#!arguments-desc=` 与 `{{{name}}}` 占位符 | 用户参数表 | ✅ | 5 | 参数值通过 API / Dashboard / CLI 设置 |
| `#!requirement=<expr>` | 复杂条件 | ✅ | 5 | 变量语义同 1.4 |
| 模块启用/禁用/更新 | UI 操作 | ✅ | 5 / 6 | API 与 CLI 提供等价操作 |

### 1.6 Keystore 与 Host List 参数类型

| 项 | Surge 行为 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| `[Keystore]` 条目 `name = type=, base64=, password=` | `type` 为 `p12` 或 `openssh-private-key`，可省略推断 | ✅ | 2 | |
| 引用点：`client-cert` / `ca-keystore-name` / `private-key` | | ✅ | 2 / 4 | |
| Host List：`-` 前缀排除、`*` `?` 通配、顺序优先 | | ✅ | 1 | |
| Host List：`host:port` `host:0` 与各参数默认端口 | `force-http-engine-hosts` 默认 80，MITM `hostname` 默认 443 | ✅ | 1 | |
| Host List 特殊记号 `<ip-address>` `<ipv4-address>` `<ipv6-address>` `<simple-hostname>` `*:0` | | ✅ | 1 | |

---

## 2. `[General]` 选项

### 2.1 通用参数

| 键 | 取值 / 默认 | Surge 平台 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- | --- |
| `loglevel` | `verbose` `info` `notify` `warning`；默认 `notify` | 全部 | ✅ | 1 | `--log-file <path>` 按天滚动保留 7 个（rurge 专有运行时选项，M3b，经 `tracing-appender`） |
| `debug-cpu-usage` | 布尔；默认 false | 全部 | 🔁 | 1 | rurge 用自己的 profiling 开关 |
| `debug-memory-usage` | 布尔；默认 false | 全部 | 🔁 | 1 | |
| `dns-server` | IP[:port] 列表或 `system`；含加密 URL 时自动迁移到 `encrypted-dns-server` | 全部 | ✅ | 1 | |
| `encrypted-dns-server` | URL 列表：`https://` `h3://` `quic://` `tls://` `tcp://` | 全部 | 🟡 | 1 / 2 | `https` `tls` `tcp` 阶段 1；`h3` `quic` 依赖 QUIC 栈，阶段 2；阶段 1 对 `h3` / `quic` 条目告警 W0026 并忽略 |
| `encrypted-dns-follow-outbound-mode` | 布尔；默认 false | 全部 | 🟡 | 1 | 含"代理服务器为域名时回退 DIRECT 并告警"的防环逻辑；M3b：TCP/DoT/DoH 上游连接走流水线（成 Internal 会话，`PROTOCOL,DOH/DOT/DNS` 可匹配）；上游主机名由 Bootstrap 解析，流水线只见 IP 目标，故域名规则不匹配上游主机名；协议标签按端口启发（853→DoT，443→DoH，其余→DNS）；被规则 REJECT 时告警并直连以保 DNS；UDP 上游不经连接器；这类内部会话的 `SRC-IP` 恒为 `127.0.0.1`、`IN-PORT` 恒为 `0`，`SRC-IP,127.0.0.1/32` / `IN-PORT,0` 规则可能意外匹配到它们，且它们的 `kill` 是空操作（DNS 路径不监听取消令牌） |
| `encrypted-dns-skip-cert-verification` | 布尔；默认 false | 全部 | ✅ | 1 | |
| `allow-dns-svcb` | 布尔；默认 false | 全部 | ✅ | 3 | fake-IP 应答器拒绝 type 65 查询 |
| `use-local-host-item-for-proxy` | 布尔；默认 false | 全部 | ✅ | 2 | |
| `hijack-dns` | `ip[:port]` 或 `*[:port]` 列表；默认端口 53 | 全部 | ✅ | 3 | |
| `always-real-ip` | Host List | 全部 | ✅ | 3 | |
| `geoip-maxmind-url` | URL；默认 `https://nssurge.com/resource/geoip-database.tar.gz` | 全部 | 🟡 | 1 | 接受 `.tar.gz`（含 `GeoLite2-Country.mmdb`）或裸 `.mmdb`；rurge 默认镜像为 `https://github.com/P3TERX/GeoLite.mmdb` 发布件（`GeoLite2-Country.mmdb` / `GeoLite2-ASN.mmdb`），而非 `nssurge.com`；可用 `--geoip-url` / `RURGE_GEOIP_URL` 覆盖 |
| `disable-geoip-db-auto-update` | 布尔；默认 false | 全部 | 🟡 | 1 | rurge 默认每 7 天检查一次更新（Surge 未说明周期） |
| `ipv6` | 布尔；默认 false | 全部 | ✅ | 1 | |
| `ipv6-vif` | `disabled` `auto` `always`；默认 `disabled`；旧值 `off` | 全部 | ✅ | 3 | |
| `tun-excluded-routes` | CIDR 列表 | 全部 | ✅ | 3 | |
| `tun-included-routes` | CIDR 列表 | 全部 | ✅ | 3 | |
| `icmp-forwarding` | 布尔；默认 true | 全部 | ✅ | 3 | |
| `skip-proxy` | Host List | 全部（平台语义不同） | ✅ | 1 | 采用 macOS 语义：写入系统代理的绕过列表（Win: `ProxyOverride`；Lin: `no_proxy` / GNOME ignore-hosts） |
| `exclude-simple-hostnames` | 布尔；默认 false | 全部 | ✅ | 1 | Windows 对应 `<local>` |
| `proxy-restricted-to-lan` | 布尔；默认 true | 全部 | 🟡 | 1 | rurge 按回环 / 私有 / 链路本地 / ULA 判定来源；手册为『当前子网』 |
| `gateway-restricted-to-lan` | 布尔；默认 true | 全部 | ✅ | 7 | |
| `external-controller-access` | `key@ip:port` | 全部 | 🔁 | 1 | Surge Dashboard 原生协议为专有协议；远程控制统一走 `http-api` |
| `http-api` | `key@ip:port` | 全部 | ✅ | 1 | |
| `http-api-tls` | 布尔；默认 false；需先配置 MITM CA | 全部 | ✅ | 4 | |
| `http-api-web-dashboard` | 布尔；默认 false | 全部 | ✅ | 6 | |
| `internet-test-url` | URL；默认 `http://bing.com/` | 全部 | ✅ | 2 | |
| `proxy-test-url` | URL；默认 `http://bing.com/` | 全部 | ✅ | 2 | |
| `test-timeout` | 秒；默认 5（DIRECT 为 10） | 全部 | ✅ | 2 | |
| `proxy-test-udp` | `hostname@ipv4` | 全部 | ✅ | 2 | |
| `force-http-engine-hosts` | Host List；默认端口 80 | 全部 | ✅ | 4 | |
| `always-raw-tcp-hosts` | Host List | 全部 | ✅ | 4 | |
| `always-raw-tcp-keywords` | 关键字列表 | Mac 5.5+ | ✅ | 4 | |
| `udp-policy-not-supported-behaviour` | `REJECT` `DIRECT`；默认 `REJECT`（Mac 6.0 起） | 全部 | ✅ | 2 | |
| `udp-priority` | 布尔；默认 true | 全部 | 🟡 | 3 | 高负载下优先处理 UDP，尽力而为 |
| `block-quic` | `per-policy` `all-proxy` `all` `always-allow`；默认 `per-policy` | 全部 | ✅ | 2 | |
| `show-error-page` | 布尔；默认 true | Mac 5.8+ | 🟡 | 1 | 错误页为 rurge 自己的 HTML（写明规则、策略链、会话 id）；对连接失败的 502 页在 M3a 只覆盖明文请求，CONNECT 连接失败的 502 已实现（M3b） |
| `show-error-page-for-reject` | 布尔；默认 false | 全部 | 🟡 | 1 | 错误页为 rurge 自己的 HTML（写明规则、策略链、会话 id） |
| 空闲超时（`--idle-timeout`） | Surge 未公开默认值 | 全部 | 🟡 | 1 | rurge 默认 600 s，`--idle-timeout` 覆盖（M3b，专有运行时选项，不是 Surge 配置键）；只作用于 CONNECT / SOCKS5 的中继会话，**明文 HTTP 转发不受其约束**（该路径不经中继泵，保活等待由 hyper 的 `header_read_timeout` 覆盖，单次交换由上游连接寿命约束；阶段 4 自有 HTTP 引擎后统一） |
| 请求记录（RequestLog） | rurge 专有能力，无对应 Surge 配置键 | 全部 | ✅ | 1（基础） | 内存环形缓冲（`--request-log-size`，默认 1000）+ 活动索引 + `kill`（M3b）；`kill` 对 CONNECT / SOCKS5 与明文 HTTP 转发会话均有效（明文转发在 M3b 修复波补齐），对内部 DNS 会话仍是空操作；完整 HTTP API 在 M4 / 阶段 6 |
| 流量统计（TrafficStats） | rurge 专有能力，无对应 Surge 配置键 | 全部 | ✅ | 1（基础） | 总计 / 按策略 / 按监听器原子计数 + 每秒采样速率（M3b）；完整 HTTP API 在 M4 / 阶段 6 |

### 2.2 iOS 专属参数

| 键 | 取值 / 默认 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| `compatibility-mode` | 0–5 | 🔁 | 1 | 桌面平台无此概念 |
| `auto-suspend` | 布尔 | 🔁 | 1 | |
| `allow-wifi-access` | 布尔 | 🟡 | 1 | 未配置 `http-listen`/`socks5-listen` 时，若为 true 则按 `wifi-access-*` 端口在 `0.0.0.0` 监听，便于直接复用 iOS 配置 |
| `allow-hotspot-access` | 布尔 | 🔁 | 1 | |
| `wifi-access-http-port` | 端口；默认 6152 | 🟡 | 1 | 同上 |
| `wifi-access-socks5-port` | 端口；默认 6153 | 🟡 | 1 | 同上 |
| `wifi-access-http-auth` | `username:password` | 🟡 | 1 | 同上 |
| `wifi-assist` `all-hybrid` `hide-vpn-icon` | 布尔 | 🔁 | 1 | |
| `include-all-networks` `include-local-networks` `include-apns` `include-cellular-services` | 布尔 | 🔁 | 1 | |

### 2.3 macOS 专属参数

| 键 | 取值 / 默认 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| `http-listen` | `[password@]address[:port]` 列表；默认端口 6152；多监听器 | 🟡 | 1 | 全平台可用；地址必须是 IP 字面量，IPv6 用 `[...]`；Basic 认证只比较密码，用户名任意（手册只给出 `[password@]address[:port]`）；两者都缺省时 rurge 仍在 `127.0.0.1:6152` / `6153` 监听，手册则是 `http-listen` 与 `socks5-listen` 都缺省则代理服务关闭；只配其一时与 Surge 一致：只开那一种 |
| `socks5-listen` | `address[:port]` 列表；默认端口 6153；不支持密码 | 🟡 | 1 | 全平台可用；REJECT 时回 `0x02`（手册未说明） |
| `set-system-socks-proxy` | 布尔；默认 true | ✅ | 1 | 随"设为系统代理"功能生效 |
| `read-etc-hosts` | 布尔；默认 true | 🟡 | 1 | 手册标注 Mac only；rurge 三平台生效（Win: `System32\drivers\etc\hosts`） |
| `subnet-exp-wifi-always-match` | 布尔；默认 true | ✅ | 3 | |

### 2.4 旧键自动迁移

| 旧键 / 旧值 | 迁移目标 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| `doh-server` | `encrypted-dns-server` | ✅ | 1 | rurge 只在内存中迁移，不回写文件 |
| `doh-follow-outbound-mode` | `encrypted-dns-follow-outbound-mode` | ✅ | 1 | |
| `doh-skip-cert-verification` | `encrypted-dns-skip-cert-verification` | ✅ | 1 | |
| `ipv6-vif = off` | `disabled` | ✅ | 3 | |
| `interface` + `port` | `http-listen` | ✅ | 1 | |
| `socks-interface` + `socks-port` | `socks5-listen` | ✅ | 1 | |
| `use-default-policy-if-wifi-not-primary` | `subnet-exp-wifi-always-match`（含义取反） | ✅ | 3 | |
| 已从手册消失的键：`vif-mode` `tls-provider` `network-framework` `bypass-system` `bypass-tun` `enhanced-mode-by-rule` `allow-udp-proxy` | 无 | 🔁 | 1 | 解析后忽略并告警，避免旧配置加载失败 |

---

## 3. 规则系统

### 3.1 规则类型（30 种）

| 类型 | 匹配对象 | Surge 平台 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- | --- |
| `DOMAIN` | 主机名精确匹配 | 全部 | ✅ | 1 | 大小写不敏感，忽略尾部 `.` |
| `DOMAIN-SUFFIX` | 域名及其子域 | 全部 | ✅ | 1 | |
| `DOMAIN-KEYWORD` | 主机名包含子串（字面量） | 全部 | ✅ | 1 | |
| `DOMAIN-WILDCARD` | `*` `?` `[...]` 通配，`*` 可跨点 | 全部 | ✅ | 1 | |
| `DOMAIN-SET` | 外部域名列表（URL / 本地文件） | 全部 | 🟡 | 1 | 上限 1,000,000 条，超出部分截断并告警（Surge 未说明超限行为）；本地文件监视自动重载 |
| `IP-CIDR` | IPv4 段；单地址视为 `/32`（Mac 6.0+） | 全部 | ✅ | 1 | |
| `IP-CIDR6` | IPv6 段；单地址视为 `/128` | 全部 | ✅ | 1 | |
| `GEOIP` | 目标 IP 所属国家（ISO 码，不区分大小写） | 全部 | ✅ | 1 | MaxMind GeoLite2 Country |
| `IP-ASN` | 目标 IP 所属 ASN；接受 `AS` 前缀 | 全部 | 🟡 | 1 | GeoLite2 ASN 库经外部资源管理器下载并热更新（而非随 rurge 版本发布捆绑），来源与更新周期见 `geoip-maxmind-url` / `disable-geoip-db-auto-update` |
| `USER-AGENT` | HTTP User-Agent，`*` `?` 通配，区分大小写 | 全部 | ✅ | 1 / 4 | 阶段 1 对显式 HTTP 代理请求可用；HTTPS 需 MITM（阶段 4） |
| `URL-REGEX` | 完整 URL 正则，区分大小写 | 全部 | ✅ | 1 / 4 | 同上 |
| `PROCESS-NAME` | 发起进程：文件名模式 / 以 `/` 开头的全路径模式 / 以 `/` 结尾的前缀模式（Mac 6.0+）；区分大小写 | Mac only | 🟡 | 3 | rurge 在 Win/Lin/mac 三平台都实现进程识别；Windows 路径分隔符为 `\`，规则中的 `/` 前缀模式按路径归一化处理 |
| `DEST-PORT` | 目标端口：单值 / 区间 `a-b` / `>` `<` `>=` `<=` | 全部 | ✅ | 1 | |
| `SRC-PORT` | 客户端源端口，同上语法 | iOS 5.8.4 / Mac 5.4.4+ | ✅ | 1 | |
| `IN-PORT` | 接受请求的 rurge 监听端口 | 全部 | ✅ | 1 | |
| `SRC-IP` | 客户端 IP（单地址或 CIDR，v4/v6） | 全部 | ✅ | 1 | |
| `DEVICE-NAME` | 客户端设备名，`*` `?` 通配，区分大小写 | 全部 | ✅ | 7 | 设备名来自 DHCP / 网关模式设备表；M2a 起解析通过，匹配前始终不匹配（并记一次告警），等待阶段 7 网关模式实现 |
| `MAC-ADDRESS` | 同一局域网客户端 MAC | Mac 6.1+ | ✅ | 7 | 经网关转发的流量无法取得 MAC，与 Surge 一致；M2a 起解析通过，匹配前始终不匹配（并记一次告警），等待阶段 7 网关模式实现 |
| `PROTOCOL` | `HTTP` `HTTPS` `TCP` `UDP` `QUIC` `STUN` `MTProto` `DOH` `DOH3` `DOQ` `DOT` `DNS`；区分大小写；`TCP` 覆盖 HTTP/HTTPS/MTProto，`UDP` 覆盖 QUIC/STUN | 全部 | ✅ | 1 / 3 | `DOH*` `DOQ` `DOT` `DNS` 只匹配 rurge 自身发出的 DNS 请求且需 `encrypted-dns-follow-outbound-mode=true`；`MTProto` 依赖阶段 7；M3b：DoT/DoH/DNS 标签按上游端口启发（853/443/其余）；基于 SNI 的路由与 `PROTOCOL,HTTPS` 的 dial 前匹配随阶段 4 |
| `HOSTNAME-TYPE` | `IPv4` `IPv6` `DOMAIN` `SIMPLE`；关键字区分大小写，未知值使规则无效 | Mac 5.7.3+ | ✅ | 1 | |
| `SUBNET` | 子网表达式（见 3.4） | 全部 | 🟡 | 3 | `TYPE:CELLULAR` `MCCMNC:` 在桌面平台永不匹配；M2a 起解析通过，匹配前始终不匹配（并记一次告警），等待阶段 3 增强模式实现 |
| `CELLULAR-RADIO` | 蜂窝网络制式 | iOS only | 🔁 | 1 | 解析通过，永不匹配 |
| `CELLULAR-CARRIER` | 运营商 MCC+MNC | iOS only | 🔁 | 1 | 同上 |
| `AND` / `OR` / `NOT` | 逻辑组合，子规则加括号且不带策略；最多嵌套 10 层；`NOT` 只接受一个子规则；`FINAL` 不能作子规则 | 全部 | ✅ | 1 | |
| `SCRIPT` | 由 `type=rule` 脚本决定 | 全部 | ✅ | 5 | 阶段 1 解析通过，脚本引擎就绪前视为不匹配 |
| `RULE-SET` | 内部集（`SYSTEM` `LAN`）/ 内联 `[Ruleset <name>]` / 外部 URL 或文件 | 全部 | ✅ | 1 | |
| `FINAL` | 兜底策略；必须以启用的 FINAL 结尾；多条 FINAL 以最后一条为准 | 全部 | ✅ | 1 | |

### 3.2 规则参数（10 个）

| 参数 | 适用规则 | 效果 | rurge | 阶段 |
| --- | --- | --- | --- | --- |
| `no-resolve` | IP-CIDR, IP-CIDR6, GEOIP, IP-ASN, RULE-SET, DOMAIN-SET | 未解析的域名请求跳过该规则，不触发 DNS；RULE-SET 上作用于全部子规则 | ✅ | 1 |
| `dns-failed` | FINAL | 规则评估中 DNS 失败时使用 FINAL 策略而非报错 | ✅ | 1 |
| `extended-matching` | DOMAIN, DOMAIN-SUFFIX, DOMAIN-KEYWORD, DOMAIN-WILDCARD, URL-REGEX, RULE-SET, DOMAIN-SET | 同时匹配 TLS SNI 与 HTTP Host / `:authority`；M3b：SNI 记录进请求记录用于观测（只解析首个客户端 chunk，≤8 KiB，ClientHello 跨段不拼接）；基于 SNI 的匹配随阶段 4 的 HTTP 引擎生效 | ✅ | 1 |
| `pre-matching` | 域名类、IP 类、SRC-IP、DEST-PORT、SRC-PORT、SUBNET、CELLULAR-*、逻辑规则、RULE-SET、DOMAIN-SET；仅顶层规则；策略必须是 REJECT 系 | 在 DNS 查询与 TCP 握手阶段提前拒绝 | ✅ | 1 / 3 | 阶段 1 实现"优先匹配"语义；DNS/SYN 层拦截依赖阶段 3 |
| `notification-text=<text>` | 任意规则含 FINAL | 命中时发系统通知 | ✅ | 6 |
| `notification-interval=<秒>` | 任意规则含 FINAL | 同一规则通知最小间隔；默认 300 | ✅ | 6 |
| `update-interval=<秒>` | RULE-SET, DOMAIN-SET | 外部资源重新下载间隔；默认 86400；负数禁用 | ✅ | 1 |
| `requires-resolve` | SCRIPT | 运行脚本前先做 DNS | ✅ | 5 |
| `always-capture=<session>` | 任意规则含 FINAL | 强制对命中连接开启 HTTP 抓包并记入指定会话 | ✅ | 4 |
| 未知参数 | 任意 | 静默忽略 | ✅ | 1 |

### 3.3 匹配语义

| 项 | Surge 行为 | rurge | 阶段 |
| --- | --- | --- | --- |
| 自上而下、首个命中即停止；`pre-matching` 规则最先评估 | | ✅ | 1 |
| 出站模式 Direct / Global 时完全跳过规则表 | | ✅ | 1 |
| 域名类规则不触发 DNS；IP 类规则遇域名请求时暂停评估、解析后继续；本次评估内解析结果缓存 | | ✅ | 1 |
| IP-CIDR / GEOIP / IP-ASN 用第一条 IPv4 记录（GEOIP/IP-ASN 回退 IPv6）；IP-CIDR6 用第一条 IPv6 记录 | | ✅ | 1 |
| DNS 失败：终止评估并报 DNS 错误，除非 FINAL 带 `dns-failed` | | ✅ | 1 |
| HTTP 类规则仅对 HTTP 引擎处理的请求生效；未解密 HTTPS 无 URL；USER-AGENT 可能来自 CONNECT 请求头 | | ✅ | 1 / 4 |
| SCRIPT 结果在同一请求评估内缓存 | | ✅ | 5 |
| 策略可写 `DEVICE:<name>`（Ponte 设备） | | ⛔ | — |
| 日志记录子规则命中：`Sub-rule matched: <rule> (in <set>)` | | ✅ | 1 |

### 3.4 子网表达式（SUBNET 规则、subnet 策略组、`[SSID Setting]` 共用）

| 形式 | 含义 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| `SSID:<value>` | Wi-Fi 名称，`*` `?` 通配，区分大小写 | ✅ | 3 | Win: WLAN API；Lin: nl80211 / `iw`；mac: CoreWLAN |
| `BSSID:<value>` | AP MAC，通配，不区分大小写 | ✅ | 3 | |
| `ROUTER:<ip>` | 默认网关 IP 精确匹配 | ✅ | 3 | |
| `TYPE:WIFI` / `TYPE:WIRED` / `TYPE:CELLULAR` | 网络类型，不区分大小写 | 🟡 | 3 | `CELLULAR` 永不匹配 |
| `MCCMNC:<digits>` | 运营商 | 🔁 | 3 | 永不匹配 |
| 裸值（legacy） | 含通配符按 SSID 通配，否则依次精确匹配 SSID / BSSID（不区分大小写）/ 网关 IP | ✅ | 3 | |

### 3.5 规则集与域名集

| 项 | Surge 行为 | rurge | 阶段 |
| --- | --- | --- | --- |
| RULE-SET 值解析顺序：内部名 → 内联节名 → URL / 文件路径 | | ✅ | 1 |
| 内部集 `SYSTEM`（Apple 系统域名 + `PROCESS-NAME,trustd` `netbiosd`）与 `LAN`（私有与特殊地址段 + `.local`） | 内容随版本变化 | 🟡 | 1 | rurge 内置内容与手册当前清单一致；`SYSTEM` 在非 Apple 平台可能匹配不到任何流量 |
| 外部规则集文件格式：每行一条不带策略的规则；允许 `no-resolve` `extended-matching`；不允许 FINAL 与 `pre-matching`；注释 `#` `//` `;`；非法行跳过并告警；上限 1,000,000 | | 🟡 | 1 | 超出上限的行截断并告警（Surge 未说明超限行为） |
| DOMAIN-SET 文件格式：每行一个域名；`.` 前缀表示后缀匹配；注释与空行忽略 | | ✅ | 1 |
| 同一 URL / 文件不能同时作为 RULE-SET 与 DOMAIN-SET | | ✅ | 1 |
| 外部资源下载缓存；本地文件监视自动重载 | | 🟡 | 1 | 下载使用 HTTP 条件请求（`If-None-Match` / ETag）避免重复传输，属 rurge 内部优化，Surge 未说明其下载策略细节 |
| 内联规则集可嵌套引用 RULE-SET / DOMAIN-SET（iOS 5.22 / Mac 6.9+）；循环引用在加载时拒绝；最多 8 层，超出视为不匹配 | | 🟡 | 1 | 手册只对内联 `[Ruleset <name>]` 说明嵌套；rurge 对外部规则集 / 域名集文件内的嵌套引用同样支持，深度限制同为 8 层 |
| 预处理：域名反转标签排序数组（后缀 / 精确二分查找）+ IP 前缀树（最长前缀匹配）+ 其余线性 | 性能特性 | 🟡 | 1 | rurge 全内存实现（不用磁盘库）；100,000 条域名集单次查询、`evaluate`（1,000 条顶层规则 + 3 个 10 万条集）基准数字见 `docs/superpowers/plans/2026-09-04-phase1-m2a-rules-plan.md` 执行期修正记录 |
| RULE-SET 行上的 `no-resolve` / `extended-matching` / `update-interval` / `pre-matching` 作用于整集 | | ✅ | 1 |

---

## 4. 出站策略

### 4.1 内置策略与别名

| 项 | Surge 行为 | Surge 平台 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- | --- |
| `DIRECT` | 直连 | 全部 | ✅ | 1 | |
| `REJECT` | 拒绝；HTTP 请求返回错误页（受 `show-error-page-for-reject` 控制）；同一主机 30 秒内触发 50 次自动升级为 `REJECT-DROP` | 全部 | ✅ | 1 | 同主机 30 s 内 50 次可升级拒绝后自动升级 REJECT-DROP（M3b）；按 `count >= 50` 判定，即**第 50 次拒绝本身就已按 DROP 处理**（不是第 51 次才升级）；计数表上限 4096 个主机，表满且无空闲条目可清时新主机不被记录、因而不会升级 |
| `REJECT-TINYGIF` | 拒绝；HTTP 请求返回 1px 透明 GIF | 全部 | ✅ | 1 | |
| `REJECT-DROP` | 静默丢弃连接 | 全部 | 🟡 | 1 | rurge 最多保持 30 s（M3b 可调）；Surge 直到客户端超时；SOCKS5 侧客户端先关闭则提前结束，HTTP 侧固定保持到超时（hyper 服务内观察不到客户端关闭，阶段 4 自有 HTTP 引擎后统一） |
| `REJECT-NO-DROP` | 拒绝且永不升级为 DROP | 全部 | ✅ | 1 | |
| `CELLULAR` `CELLULAR-ONLY` `HYBRID` `NO-HYBRID` | 蜂窝/混合网络策略 | iOS only | 🟡 | 1 | 桌面平台视为 `DIRECT` 并告警一次 |
| 别名类型 `direct` `reject` `reject-drop` `reject-no-drop` `reject-tinygif` | `[Proxy]` 中定义别名，可带通用参数（如 `interface`） | 全部 | ✅ | 1 / 2 | 参数生效依赖阶段 2 |
| 重定义 `DIRECT` | 静默忽略 | 全部 | ✅ | 1 | |
| 重定义其他内置名（`REJECT` `CELLULAR` 等） | 配置错误 | 全部 | ✅ | 1 | |
| pre-matching 下的 REJECT 行为：DNS 阶段 `REJECT` 返回无记录（限频后不响应）、`REJECT-DROP` 不响应、`REJECT-NO-DROP` 返回 `198.18.0.244` 并对其所有 TCP 连接回 RST | | 全部 | ✅ | 3 | |
| pre-matching 下的 TCP 行为：`REJECT` 回 RST（限频后丢 SYN）、`REJECT-DROP` 丢 SYN、`REJECT-NO-DROP` 回 RST；3 秒内 100 次 RST 后暂停回 RST 改为丢包 | | 全部 | ✅ | 3 | |
| UDP 行为：`REJECT` / `REJECT-NO-DROP` 回 ICMP Administratively Prohibited（限频后丢包），`REJECT-DROP` 直接丢包 | UDP 无预匹配阶段 | 全部 | ✅ | 3 | |
| 被 pre-matching 拒绝的规则 5 分钟内只在最近请求列表出现一次 | 防刷屏 | 全部 | ✅ | 4 | |

### 4.2 代理协议类型（16 种）

| 类型关键字 | 协议 | Surge 版本 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- | --- |
| `http` / `https` | HTTP 代理 / HTTP over TLS | 全部 | ✅ | 2 | |
| `h2-connect` | HTTP/2 CONNECT 多路复用 | Mac 6.6+ | ✅ | 2 | |
| `socks5` / `socks5-tls` | SOCKS5 / SOCKS5 over TLS | 全部 | ✅ | 2 | |
| `ss` | Shadowsocks | 全部 | ✅ | 2 | 加密方法见 4.6 |
| `snell` | Snell v1–v6 | v6 需 iOS 5.20 / Mac 6.7+ | 🟡 | 2 | v1–v4 计划支持；v5（QUIC Proxy Mode）与 v6（PSK 派生协议画像、流量整形，beta）协议细节未公开，❓ 待评估 |
| `vmess` | VMess（AEAD / 旧握手、TLS、WebSocket） | 全部 | ✅ | 2 | 旧式非 AEAD 握手 🟡 低优先级 |
| `trojan` | Trojan（TLS、WebSocket） | 全部 | ✅ | 2 | |
| `tuic` / `tuic-v5` | TUIC v4（token）/ v5（uuid + password） | 全部 | ✅ | 2 | |
| `hysteria2` | Hysteria 2 | iOS 5.8 / Mac 5.4+ | ✅ | 2 | Salamander 混淆 ✅；Gecko 混淆 ❓ |
| `masque` | MASQUE（HTTP/3 CONNECT + CONNECT-UDP，RFC 9298 / 9297） | iOS 5.22 / Mac 6.9+ | ✅ | 2 | |
| `anytls` | AnyTLS v2 | iOS 5.17 / Mac 6.4.3+ | ✅ | 2 | |
| `trust-tunnel` | Trust Tunnel（AdGuard，HTTP/2 或 HTTP/3） | Mac 6.4.4+ | ✅ | 2 | |
| `ssh` | SSH 动态转发 | 全部 | ✅ | 2 | |
| `wireguard` | WireGuard L3 隧道作为策略 | 全部 | ✅ | 2 | |
| `tailscale` | Tailscale 节点作为策略 | iOS 5.20 / Mac 6.7+ | ❓ | 远期 | 需嵌入 Tailscale 客户端（控制面协议、DERP、MagicDNS）；单独评估 |
| `external` | 外部代理程序（本地 SOCKS5） | Mac only（iOS 视为 REJECT） | ✅ | 2 | rurge 在 Win/Lin/mac 均支持 |
| 策略指向 rurge 尚未实现的协议类型 | Surge 原生支持全部协议 | 全部 | 🟡 | 1 / 2 | 加载时告警 `W0007`；运行时该策略按 `REJECT` 处理，会话日志 `error = policy protocol not implemented: <type>`；阶段 2 逐协议实现后移除 |

### 4.3 通用策略参数（14 个）

| 参数 | 取值 / 默认 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| `interface` | 出口网卡名；默认自动 | ✅ | 2 | Win 用 `IP_UNICAST_IF` / 绑定网卡地址；Lin 用 `SO_BINDTODEVICE`；mac 用 `IP_BOUND_IF`。WireGuard / Tailscale 不支持，与 Surge 一致 |
| `allow-other-interface` | 布尔；默认 false | ✅ | 2 | |
| `dns-follow-interface` | 布尔；默认 false | ✅ | 2 | |
| `no-error-alert` | 布尔；默认 false | ✅ | 6 | 抑制该策略的错误通知 |
| `ip-version` | `dual` `v4-only` `v6-only` `prefer-v4` `prefer-v6`；默认 `dual`；prefer 模式 3 秒后尝试另一族 | ✅ | 2 | 有 `underlying-proxy` 时无效 |
| `hybrid` | `auto` `on` `off` | 🔁 | 2 | iOS 专属 |
| `tfo` | 布尔；默认 false | 🟡 | 2 | Lin / mac 支持；Windows 的 `TCP_FASTOPEN` 支持情况阶段 2 验证 |
| `tos` | 0–255 或 `0x` 十六进制；默认 0 | ✅ | 2 | |
| `ecn` | `auto` `on` `off`；QUIC 类协议默认开启，WireGuard/Tailscale 默认关闭 | 🟡 | 2 | 取决于所选 QUIC 库对 ECN 的支持 |
| `block-quic` | `auto` `on` `off`；默认 `auto`（代理策略默认阻断，DIRECT 不阻断） | ✅ | 2 | 与 `[General] block-quic` 全局覆盖联动 |
| `test-url` | HTTP(S) URL；默认全局设置 | ✅ | 2 | |
| `test-timeout` | 秒；默认全局设置 | ✅ | 2 | |
| `test-udp` | `hostname@ipv4` | ✅ | 2 | |
| `underlying-proxy` | 另一策略或策略组名；仅代理策略；不能与 `port-hopping` 同用 | ✅ | 2 | 目标代理主机名在上游远程解析 |

### 4.4 TLS 与 Shadow TLS 参数

适用范围：`https` `socks5-tls` `h2-connect` `trust-tunnel` `trojan` `tuic` `hysteria2` `anytls` `masque` 以及 `vmess` + `tls=true`。

| 参数 | 取值 / 默认 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| `skip-cert-verify` | 布尔；默认 false | ✅ | 2 | |
| `sni` | 主机名或 `off`；默认代理主机名 | ✅ | 2 | |
| `server-cert-verify-name` | 主机名（与 SNI 独立） | ✅ | 2 | |
| `server-cert-fingerprint-sha256` | 64 位十六进制 | ✅ | 2 | |
| `alpn` | 协议列表；TUIC / Hysteria 2 / MASQUE 默认 `h3` | ✅ | 2 | |
| `client-cert` | `[Keystore]` 条目名（p12） | ✅ | 2 | 双向 TLS |
| `shadow-tls-password` | 字符串；设置即启用 Shadow TLS | ✅ | 2 | |
| `shadow-tls-sni` | 主机名；v3 必填 | ✅ | 2 | |
| `shadow-tls-version` | `2` / `3`；默认 2 | ✅ | 2 | |
| 约束：Shadow TLS 不能与 TUIC / WireGuard / Tailscale / 其他 QUIC 类协议组合 | 配置错误 | ✅ | 2 | |

### 4.5 UDP 中继

| 项 | Surge 行为 | rurge | 阶段 |
| --- | --- | --- | --- |
| `udp-relay`（布尔；默认 false） | 适用 SOCKS5 / SOCKS5-TLS / Shadowsocks / External / HTTP/2 CONNECT（RFC 9298） | ✅ | 2 |
| `udp-port`（端口；默认主端口） | 适用 Shadowsocks / Snell | ✅ | 2 |
| 自动支持 UDP 的协议：Snell v3+、VMess、Trojan、TUIC、Hysteria 2、MASQUE、AnyTLS（UDP over TCP）、WireGuard、Tailscale | | ✅ | 2 |
| 不支持 UDP 的协议：HTTP / HTTPS、Trust Tunnel、SSH | 受 `udp-policy-not-supported-behaviour` 控制 | ✅ | 2 |
| DIRECT / REJECT 系始终处理 UDP | | ✅ | 1 |
| UDP 测试：通过中继向 `hostname@ipv4` 做 DNS 查询 | `proxy-test-udp` / `test-udp` | ✅ | 2 |

### 4.6 各协议专属参数

| 协议 | 参数 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| `http` `https` `h2-connect` | `username` `password`（位置或命名） | ✅ | 2 | |
| `http` `https` | `always-use-connect`（默认 false） | ✅ | 2 | |
| `http` `https` `h2-connect` | `headers`（分号分隔；`<random-string(n)>` / `<random-string(min-max)>` 占位） | ✅ | 2 | |
| `h2-connect` | `max-streams`（默认 3） | ✅ | 2 | |
| `h2-connect` | `udp-relay`（CONNECT-UDP，RFC 9298） | ✅ | 2 | |
| `socks5` `socks5-tls` | `username` `password` `udp-relay`（UDP ASSOCIATE） | ✅ | 2 | |
| `ss` | `encrypt-method`：AEAD 2022 `2022-blake3-aes-128-gcm` `2022-blake3-aes-256-gcm`；AEAD `aes-128-gcm` `aes-192-gcm` `aes-256-gcm` `chacha20-ietf-poly1305` `xchacha20-ietf-poly1305`；`none` | ✅ | 2 | 2022 方法密码为 Base64 密钥（16/32 字节），支持 `serverKey:userKey` |
| `ss` | 流式旧方法 `rc4` `rc4-md5` `aes-128/192/256-cfb` `aes-128/192/256-ctr` `salsa20` `chacha20` `chacha20-ietf` | 🟡 | 2 | 低优先级，加载时告警"不推荐" |
| `ss` | `password` `udp-relay` `udp-port` `obfs`（`http` / `tls`）`obfs-host` `obfs-uri` | ✅ | 2 | |
| `snell` | `psk` `version`（1–6，默认 1）`reuse`（v4+）`obfs`（v1–3 `http`/`tls`；v4–5 `http`；v6 无）`obfs-host` `obfs-uri` `udp-port` `mode`（v6：`default` `unshaped` `unsafe-raw`） | 🟡 | 2 | 版本支持范围见 4.2；v3+ 自动 UDP |
| `vmess` | `username`（UUID）`encrypt-method`（`aes-128-gcm` / `chacha20-ietf-poly1305`，默认前者）`vmess-aead`（默认 false）`tls` `ws` `ws-path`（默认 `/`）`ws-headers`（`\|` 分隔） | ✅ | 2 | 自动 UDP |
| `trojan` | `password` `ws` `ws-path` `ws-headers` | ✅ | 2 | 自动 UDP |
| `tuic` / `tuic-v5` | `token`（v4）/ `uuid` + `password`（v5）`alpn`（默认 h3）`port-hopping`（`1234;5000-6000`）`port-hopping-interval`（默认 30） | ✅ | 2 | 自动 UDP；默认 ECN |
| `hysteria2` | `password` `download-bandwidth`（Mbps）`port-hopping` `port-hopping-interval` `salamander-password` | ✅ | 2 | |
| `hysteria2` | `gecko-password` | ❓ | 2 | 混淆算法细节待确认 |
| `masque` | `username` `password`（HTTP Basic）`port-hopping` `port-hopping-interval` | ✅ | 2 | 服务器须声明 extended CONNECT 与 HTTP Datagram，连接时校验 |
| `anytls` | `password` `reuse`（默认 true） | ✅ | 2 | |
| `trust-tunnel` | `username` `password` `headers` `max-streams`（默认 3）`h3`（默认 false） | ✅ | 2 | 无 UDP |
| `ssh` | `username` `password` \| `private-key`（Keystore 名）`idle-timeout`（默认 180）`server-fingerprint`（多指纹逗号分隔） | ✅ | 2 | Surge 仅 `curve25519-sha256` + `aes128-gcm`；rurge 至少支持这两者（可为超集）；未配置指纹时一次性安全告警 |
| `wireguard` 策略行 | `section-name`（必填）`underlying-proxy`（默认 DIRECT）`test-url`（仅 http）`test-timeout`（另加 10 秒 L3 初始化）`ecn` | ✅ | 2 | |
| `[WireGuard <name>]` | `private-key`（Base64 或 64 位十六进制）`self-ip` / `self-ip-v6`（至少一个）`dns-server` `prefer-ipv6` `mtu`（576–1420，默认 1280）`peer`（可多个，多行累加） | ✅ | 2 | |
| `peer` 字段 | `public-key` `allowed-ips`（最长前缀匹配，v4/v6 分表）`endpoint` `preshared-key` `keepalive`（0–65535）`client-id`（`83/12/235` / 3 字节十六进制 / 4 字符 Base64，WARP 保留字节） | ✅ | 2 | |
| WireGuard 生命周期 | 加载时准备、按需握手；网络变化或底层策略变化时重建；分片重组；仅回应发往本地隧道地址的 ICMP echo；握手包 DSCP 0x88 | ✅ | 2 | |
| WireGuard 测试 | 无 `dns-server` 且无 `test-url` → 原生 RTT 探测；否则标准 URL 测试 | ✅ | 2 | |
| `tailscale` 策略行与 `[Tailscale <name>]`（`auth-key` `interactive-login` `control-url` `hostname` `derp-only` `auto-add-magic-dns-rule` `exit-node` `idle-keepalive` `prefer-ipv6` `dns-server` `mtu`） | | ❓ | 远期 | 解析通过，策略视为不可用 |
| `external` | `exec` `local-port` `args`（可重复）`addresses`（可重复）`udp-relay` | ✅ | 2 | 进程退出自动重启；`addresses` 从 VIF 路由排除（阶段 3）；外部进程流量走 DIRECT；退出时清理；日志写入 rurge 数据目录 |

---

## 5. 策略组

### 5.1 组类型

| 类型 | Surge 行为 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| `select` | 手动选择；选择按 Profile 持久化；无记录或成员消失时用第一个成员 | ✅ | 2 | 通过 API / CLI / Dashboard 切换 |
| `url-test` | 选延迟最低者；HEAD 两次（第二次计分）；用时且过期或网络变化才重测；变更时通知（除非 `no-alert`） | ✅ | 2 | |
| `fallback` | 按声明顺序取第一个可用；全部不可用则用第一个 | ✅ | 2 | |
| `load-balance` | 可用集合内随机；`persistent` 时按目标主机名哈希；从不通知；嵌套时分数取均值 | ✅ | 2 | |
| `smart` | 按真实连接质量动态选择：首响应延迟时间加权均值 + 重传惩罚（约每 1% 丢包 50 ms）× `policy-priority`；接近最优者构成优选集，其余为重试列表；按站点记忆约 1 小时；固定 5 分钟重测，`interval` 无效；>12 成员只测子集；忽略嵌套组与内置策略 | 🟡 | 2 | 算法细节非公开，rurge 按手册描述近似实现 |
| `subnet`（旧名 `ssid`） | 按当前网络选择；条件按声明顺序首个命中；网络变化重算；无命中用 `default` | 🟡 | 3 | `TYPE:CELLULAR` / `MCCMNC:` 永不匹配 |
| 嵌套与循环 | 组可嵌套；循环引用告警且该组临时表现为 REJECT；无可用成员回退 DIRECT | ✅ | 2 | |
| 临时覆盖 | 自动类型组可手动指定成员，期间停止自动测试 | ✅ | 2 | API / CLI 提供 |

### 5.2 组参数

| 参数 | 适用 | 取值 / 默认 | rurge | 阶段 |
| --- | --- | --- | --- | --- |
| `interval` | url-test / fallback / load-balance（smart 忽略） | 秒；默认 600 | ✅ | 2 |
| `tolerance` | url-test | 毫秒；默认 100 | ✅ | 2 |
| `timeout` | url-test / fallback / load-balance | 秒；无默认；延迟低于此值才算可用 | ✅ | 2 |
| `evaluate-before-use` | 自动类型组 | 布尔；默认 false | ✅ | 2 |
| `persistent` | load-balance | 布尔；默认 false | ✅ | 2 |
| `policy-priority` | smart | `"regex:factor;regex:factor"`；必须为正数 | ✅ | 2 |
| `default` | subnet | 策略名；必填 | ✅ | 3 |
| `cellular` | subnet | 策略名；已弃用（用 `TYPE:CELLULAR`） | 🔁 | 3 |
| `no-alert` | url-test / fallback | 布尔；默认 false | ✅ | 6 |
| `hidden` | 全部 | 布尔；默认 false | ✅ | 2 |
| `icon-url` | 全部（Mac 6.5+） | URL | ✅ | 6 |
| `underlying-proxy` | 全部（iOS 5.22 / Mac 6.9+） | 策略名；整组链式代理，派生策略名 `Name (via Relay)` | ✅ | 2 |
| `policy-path` | 除 subnet 外 | 文件路径或 URL；内容为策略行列表或含 `[Proxy]` 的完整配置；远程缓存并定期更新 | ✅ | 2 |
| `update-interval` | 同上 | 秒；默认 86400 | ✅ | 2 |
| `policy-regex-filter` | 同上 | 正则；作用于导入成员，不作用于显式成员 | ✅ | 2 |
| `external-policy-modifier` | 同上 | 引号包裹的 `key=value` 列表；覆盖导入策略参数 | ✅ | 2 |
| `external-policy-name-prefix` | 同上 | 前缀（不能含 `=`） | ✅ | 2 |
| `include-all-proxies` | 同上（iOS 4.12 / Mac 4.5+） | 布尔；含 `[Proxy]` 全部代理策略，不含内置与组 | ✅ | 2 |
| `include-other-group` | 同上 | `"g1,g2"`；递归展开 | ✅ | 2 |
| 成员装配顺序 | 显式成员 → `include-other-group` → `include-all-proxies` → `policy-path`；重名保留首个；导入项按 过滤 → 前缀 → 修饰 处理 | | ✅ | 2 |
| 测试 URL / 超时解析顺序 | 策略自身 `test-url` → 全局 `proxy-test-url` / `internet-test-url`；策略 `test-timeout` → 全局 `test-timeout`（默认 5，直连类 10） | | ✅ | 2 |

---

## 6. DNS 与 `[Host]`

### 6.1 内部 DNS 客户端

| 项 | Surge 行为 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| 自有 DNS 客户端，不用系统解析器 | 用于自身建立的连接与需要 IP 的规则 | ✅ | 1 | |
| 并发查询全部服务器，首个有效应答胜出 | 类似 dnsmasq `--all-servers` | ✅ | 1 | |
| 重试：1 秒无应答重发；5 次（约 5 秒）后失败 | | ✅ | 1 | |
| `ipv6=true` 且网络有 IPv6 时并行 A + AAAA；重试定时触发时接受部分结果 | | ✅ | 1 | |
| 连续 5 次 AAAA 超时后停止发送 AAAA，直到网络变化或缓存刷新 | | ✅ | 1 | |
| 空应答语义：所有服务器明确返回空，或部分返回空其余超时，才报空应答错误 | | ✅ | 1 | |
| 缓存：按记录集最小 TTL；乐观缓存（过期先返回旧值并后台刷新）；LRU 上限 2000（macOS）；网络切换时清空 | | ✅ | 1 | 容量可配置，默认 2000 |
| 简单主机名（无点）：追加系统首个搜索域后交系统 DNS | | ✅ | 1 | Windows 使用连接专用 DNS 后缀 |
| `.local` 默认走系统解析库（mDNS / Bonjour） | | 🟡 | 1 | Linux 依赖 nss-mdns / systemd-resolved；Windows 10+ 系统自带 mDNS |
| 尾部 `.` 剥离并禁止搜索域改写；IP 字面量原样返回 | | ✅ | 1 | |
| `dns-server` 条目：IPv4/IPv6[:port]（默认 53）、`system`；不允许主机名；`ipv6=false` 时丢弃配置中的 IPv6 服务器（系统展开出的服务器不过滤，M3 统一）；未设置则用系统 DNS | | 🟡 | 1 | |
| `tcp://host[:port]`：持久 TCP 连接的明文 DNS；配置后同列的 UDP 服务器只用于解析该主机名 | iOS 5.21 / Mac 6.8+ | ✅ | 1 | |
| `[SSID Setting]` 中的 `dns-server` / `encrypted-dns-server` 按网络覆盖 | | ✅ | 3 | |
| `localhost` / `*.localhost` 直接返回回环地址，不查询上游 | | 🟡 | 1 | 手册未说明 |
| 空应答（NOERROR 无记录 / NXDOMAIN）负缓存 30 s；错误不缓存 | | 🟡 | 1 | 手册未说明 |

### 6.2 加密 DNS

| 项 | Surge 行为 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| `https://`（DoH，443）`tls://`（DoT，853）`tcp://`（明文 TCP，53） | | ✅ | 1 | |
| `h3://`（DoH3，443）`quic://`（DoQ，853） | | ✅ | 2 | 依赖 QUIC 栈 |
| 多服务器并发，首个应答胜出 | | ✅ | 1 | |
| 引导豁免：配置加密 DNS 后，传统 DNS 只用于连通性测试与解析加密 DNS URL 中的主机名（含 `[Host]` `server:` 项中的 URL） | | ✅ | 1 | |
| 特殊值 `off`（主要用于 `[SSID Setting]` 覆盖） | | ✅ | 1 / 3 | |
| `encrypted-dns-skip-cert-verification` | 默认 false | ✅ | 1 | |
| `encrypted-dns-follow-outbound-mode`：DNS 连接走规则；`PROTOCOL,DOH/DOH3/DOQ/DOT/DNS` 可匹配；命中的代理若以域名配置则告警并回退 DIRECT | 默认 false | 🟡 | 1 | M3b：TCP/DoT/DoH 上游走流水线（Internal 会话，`PROTOCOL` 可匹配）；上游主机名先由 Bootstrap 解析，流水线只见 IP 目标，域名规则不匹配上游主机名；协议标签按端口启发（853→DoT，443→DoH，其余→DNS）；被 REJECT 时告警并直连保底；UDP 上游不经连接器；这类会话的 `SRC-IP`/`IN-PORT` 为占位值（`127.0.0.1:0`/`0`），`kill` 对其无效 |
| `[Host]` 中 `server:<加密 URL>` 按域名指定加密 DNS | iOS 5.21 / Mac 6.8+ | ✅ | 1 | |

### 6.3 `[Host]` 本地 DNS 映射

| 条目形式 | Surge 行为 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| 自上而下首个命中；作用于内部 DNS 客户端与 fake-IP 应答器；代理服务器主机名永不匹配（防环） | | ✅ | 1 | |
| `<host> = <ip>[, <ip>...]` | 权威应答，v4/v6 可混合 | ✅ | 1 | |
| 键中的 `*` `?` 通配（整串匹配：`*google.com` 匹配 `bargoogle.com`，`*.google.com` 不匹配 `google.com`） | | ✅ | 1 | |
| `<host> = <other host>`（别名，CNAME 语义） | 以新名字重新查找 | ✅ | 1 | |
| `<host> = server:<ip[:port] \| 加密 URL>[, ...]` | 指定上游 | ✅ | 1 | |
| `server:system` / `server:syslib` | 普通模式交系统解析库；增强模式在 rurge 内转发到系统当前配置的 DNS 服务器 | ✅ | 1 / 3 | |
| `server:force-syslib` | 始终用系统解析库（mDNS 等特殊域名） | 🟡 | 3 | 阶段 1 等同 `syslib`；M3 起区分（Mac 6.4.3+） |
| `<host> = script:<name>` | 由 `type=dns` 脚本解析 | ✅ | 5 | 阶段 1 构建时告警 W0027 并跳过该条目 |
| `DOMAIN-SET:<url\|path> = ...` / `RULE-SET:<url\|path> = ...` | 整集绑定映射；规则集中只有域名类条目生效 | ✅ | 1 | Mac 5.10+ |
| `read-etc-hosts`（macOS，默认 true） | 追加系统 hosts 项于 `[Host]` 之后并监视变化 | 🟡 | 1 | 手册标注 Mac only；rurge 三平台生效（Win: `System32\drivers\etc\hosts`） |
| `use-local-host-item-for-proxy` | 有本地 IP 映射时用 IP 发起代理请求；多地址随机取一；不影响 `server:` / `script:` | ✅ | 2 | |

### 6.4 VIF（增强模式）下的 DNS 应答器与 fake-IP

| 项 | Surge 行为 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| 应答器监听地址：macOS `198.18.0.2`，iOS `198.18.0.4`，IPv6 `fd00:6152::2`；`198.18.0.2`–`198.18.0.9` 均视为发往应答器 | | ✅ | 3 | rurge 使用 `198.18.0.2` 与 `fd00:6152::2`；网关模式经 DHCP 通告 |
| A / AAAA 查询立即返回 `198.18.0.0/15` 内的 fake IP（v4 池 `198.18.1.1`–`198.19.255.254`；v6 用 `fd00:6152::` 下专用前缀）；映射持久保存；连接到达时反查域名 | | ✅ | 3 | |
| fake 应答 TTL：macOS 30 秒；只返回与查询所用地址族一致的 fake 地址 | | ✅ | 3 | rurge 默认 30 秒 |
| 非 A/AAAA 查询（TXT、MX 等）转发上游，遵守 `[Host]` `server:` | | ✅ | 3 | |
| `hijack-dns`：劫持发往指定 IP[:port] 或 `*[:port]` 的合法 DNS 查询 | | ✅ | 3 | |
| `always-real-ip`：豁免域名返回真实 IP；豁免域名的 `[Host]` IP 映射由应答器权威应答 | | ✅ | 3 | |
| `allow-dns-svcb`：默认拒绝 type 65 查询（返回 Not Implemented） | | ✅ | 3 | |
| pre-matching REJECT 在应答器层执行 | 见 4.1 | ✅ | 3 | |
| Firefox canary 域 `use-application-dns.net` 返回 NXDOMAIN | | ✅ | 3 | |

---

## 7. HTTP 处理

### 7.1 HTTP 引擎

| 项 | Surge 行为 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| 新 TCP 连接嗅探首字节，像 HTTP 则交给 HTTP 引擎 | | ✅ | 4 | 阶段 1 的 HTTP 代理端口本身即解析 HTTP，但捕获 / 重写 / 脚本等引擎功能在阶段 4 |
| 80 / 443 端口自动协议嗅探；`always-raw-tcp-hosts` / `always-raw-tcp-keywords` 关闭嗅探 | | ✅ | 4 | |
| `force-http-engine-hosts` 强制明文连接进入引擎（不解密 TLS） | | ✅ | 4 | |
| HTTPS 默认不透明，需 MITM 命中主机名 | | ✅ | 4 | |
| 流水线顺序：Header Rewrite → URL Rewrite → Body Rewrite → 脚本 | | ✅ | 4 / 5 | |
| 每个请求 / 响应最多一个脚本；重写规则可串联 | | ✅ | 4 / 5 | |
| Map Local 命中则直接返回本地响应，不发上游 | | ✅ | 4 | |
| 引擎处理的请求在抓包视图中逐条展示，含重写与脚本备注 | | ✅ | 4 / 6 | |
| 明文 HTTP 代理转发 | | 🟡 | 1 | 每个请求独立分流；出站连接每请求一条（阶段 4 引入连接池）；按 RFC 7230 剥离逐跳头并以请求目标覆盖 `Host`；不转发 `Upgrade`（WebSocket 经明文代理属阶段 4）；只接受 `http://` 绝对 URI |

### 7.2 `[MITM]`（9 个键）

| 键 | 取值 / 默认 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| `hostname` | Host List；默认端口 443；`:0` 全端口；`-` 排除；SNI 命中时忽略端口；非法项只告警 | ✅ | 4 | |
| `hostname-disabled` | Host List；从生效列表移除 | ✅ | 4 | |
| `ca-p12` | Base64 PKCS#12 | ✅ | 4 | |
| `ca-passphrase` | 非空字符串 | ✅ | 4 | |
| `ca-keystore-name` | Keystore p12 项名；优先于 `ca-p12` | ✅ | 4 | |
| `skip-server-cert-verify` | 布尔；默认 false | ✅ | 4 | |
| `h2` | 布尔；默认 false；MITM over HTTP/2 | ✅ | 4 | |
| `client-source-address` | IP / CIDR 列表，`-` 排除；默认全部；Mac 6.1+ 支持 MAC 地址 | ✅ | 4 / 7 | MAC 地址形式依赖阶段 7 |
| `auto-quic-block` | 布尔；默认 true；命中 MITM 列表的 QUIC 连接自动阻断以回落 H2/H1 | 🟡 | 3 / 4 | 需解析 QUIC Initial 中的 SNI；仅在 VIF 或 UDP 可见时生效 |
| CA 生成：Surge 在 Dashboard / 配置编辑器内生成并加入系统信任 | | 🟡 | 4 | rurge 提供 `rurge mitm ca generate` 与各平台安装指引；自动写入系统信任库为 P2 |
| 使用现有 CA：导出 p12（口令非空）→ base64 → `ca-p12` + `ca-passphrase` | | ✅ | 4 | |
| 证书固定提示：握手完成但无请求即断开时记录提示日志 | | ✅ | 4 | |
| 手册未定义的键（如 `tcp-connection`、客户端证书） | 不存在 | 🔁 | 4 | 未知键忽略并告警 |

### 7.3 `[URL Rewrite]`

| 项 | Surge 行为 | rurge | 阶段 |
| --- | --- | --- | --- |
| 语法 `<regex> <replacement> <type>`；`type` 省略即 `header`；支持 `$1` 捕获组 | | ✅ | 4 |
| `header`：原地改写并按需改 Host；同一请求只应用首个命中的 header 规则；结果必须是合法 `http` / `https` / `ws` URL | | ✅ | 4 |
| `302` / `307`：返回重定向，替换值作为 `Location`；可用 `{{{GATEWAY_ADDRESS}}}` 占位 | | ✅ | 4 |
| `reject`：拒绝请求，替换值用 `_` 占位 | | ✅ | 4 |
| 匹配时同时尝试用 Host 头与底层连接主机名重建的 URL | | ✅ | 4 |
| 其他工具的 `reject-200` `reject-img` `reject-dict` 等模式 | Surge 不支持 | ⛔ | — |

### 7.4 `[Header Rewrite]`

| 项 | Surge 行为 | rurge | 阶段 |
| --- | --- | --- | --- |
| 语法 `[http-request\|http-response] <url regex> <action> <field> [value] [template]`；方向省略即 `http-request` | | ✅ | 4 |
| `header-add`（即使已存在也追加）`header-del` `header-replace`（不存在则不动）`header-replace-regex <field> <regex> <template>` | | ✅ | 4 |
| 多条命中按顺序生效 | | ✅ | 4 |
| 改动 `Content-Length` / `Transfer-Encoding` 的重写被拒绝且请求失败 | | ✅ | 4 |

### 7.5 `[Body Rewrite]`

| 项 | Surge 行为 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| 正则形式 `http-request\|http-response <url regex> <find> <replace> [<find> <replace>...]`；`$1`；`^` `$` 按行锚定；多条顺序执行；`^$` 可凭空生成正文 | iOS 5.10 / Mac 5.6+ | ✅ | 4 | |
| 正文必须是合法 UTF-8，否则跳过并在请求记录加备注 | | ✅ | 4 | |
| jq 形式 `http-request-jq` / `http-response-jq <url regex> <jq 表达式>`；非法 JSON 跳过并备注；空输出保留原文；非法表达式告警 | iOS 5.14 / Mac 5.9+ | ✅ | 4 | 使用 Rust 实现的 jq 子集，兼容性在阶段 4 设计文档中界定 |
| 先于脚本执行，脚本看到改写后的正文 | | ✅ | 5 | |
| 请求体缓冲上限 32 MB（超出断开）；响应体可改上限 10 MB（macOS，超出直通） | | ✅ | 4 | |
| `Transfer-Encoding: chunked` 或 `Expect: 100-continue` 的请求体不改写并告警 | | ✅ | 4 | |
| 改写后自动解压并重算 `Content-Length` | | ✅ | 4 | |

### 7.6 `[Map Local]`

| 项 | Surge 行为 | rurge | 阶段 |
| --- | --- | --- | --- |
| 语法 `<url regex> key=value ...`；需启用 Rewrite 功能；HTTPS 需 MITM | | ✅ | 4 |
| `data-type`：`file`（默认）`text` `tiny-gif` `base64` | | ✅ | 4 |
| `data`：文件路径（相对配置目录 / 绝对路径 / URL 下载缓存）、文本、Base64 | | ✅ | 4 |
| `data-type=text data=""` 返回空响应 | | ✅ | 4 |
| `header`：`a:b\|c:d`；无 `:` 时视为 Base64 编码的多行头 | | ✅ | 4 |
| `status-code`：200–999，默认 200 | | ✅ | 4 |
| `Content-Type` 自动补全：按扩展名 / `text/plain` / `image/gif` / `application/octet-stream` | | ✅ | 4 |

---

## 8. 脚本

### 8.1 `[Script]` 声明与参数（15 个）

| 参数 | 取值 / 默认 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| 现代形式 `name = key=value, ...`；`script-path` 必填 | | ✅ | 5 | |
| 旧形式 `<type> <value> <parameters>`（value 映射到 pattern / cronexp / event-name / 名字） | 仍解析 | ✅ | 5 | |
| `type` | `http-request` `http-response` `rule` `dns` `event` `cron` `generic`；默认 generic；未知值拒绝该行 | ✅ | 5 | |
| `script-path` | 相对 / 绝对路径或 HTTP(S) URL（下载缓存） | ✅ | 5 | |
| `script-update-interval` | 秒；默认 86400 | ✅ | 5 | |
| `timeout` | 秒；默认 5；超时终止会话并告警，可中断同步代码 | ✅ | 5 | |
| `argument` | 任意字符串 → `$argument` | ✅ | 5 | |
| `engine` | `auto` `jsc` `webview`；默认 auto | 🔁 | 5 | rurge 只有一个内嵌引擎，接受该参数 |
| `debug` | 布尔；每次从磁盘重载 + `console.log` 进请求备注 | ✅ | 5 | |
| `pattern` | 正则；http-request / http-response 必填；按 Profile 顺序首个命中 | ✅ | 5 | |
| `requires-body` | 布尔；默认 false | ✅ | 5 | |
| `max-size` | 字节；默认 10 MB（macOS）；`-1` 不限（请求体硬上限 32 MB） | ✅ | 5 | |
| `binary-body-mode` | 布尔；正文以 `Uint8Array` 传递 | ✅ | 5 | |
| `full-header-mode` | 布尔；头部以 `{field, value}` 数组传递 | ✅ | 5 | |
| `cronexp` | 5 或 6 段（含秒）；需引号 | ✅ | 5 | |
| `event-name` | `network-changed` `notification` `engine-started` `profile-reloaded` | ✅ | 5 | |
| `wake-system` | 布尔；iOS 专属 | 🔁 | 5 | |
| 引擎：Surge 有 JSC（进程内、无 JIT、并发 2）与 WebView（独立进程、JIT、WebAPI、并发 3） | | 🟡 | 5 | rurge 单引擎（选型在阶段 5 设计文档确定），并发上限可配置，默认 3 |
| WebAPI（`fetch` `TextDecoder` `crypto` 等） | WebView 提供 | 🟡 | 5 | rurge 提供常用子集 polyfill |
| 调试：每脚本日志文件；编辑器 mock 执行；HTTP API 评估接口 | | 🟡 | 5 / 6 | 无内置编辑器；mock 执行通过 API / CLI |

### 8.2 全局 API

| 对象 / 函数 | Surge 语义 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| `$environment.system` `["surge-build"]` `["surge-version"]` `language` `["device-model"]` | 运行环境 | 🟡 | 5 | `system` 报告 `macOS` / `Windows` / `Linux`；`surge-version` 报告兼容的 Surge 版本字符串 |
| `$script.name` `type` `startTime` `sessionID` `binaryBodyMode` | | ✅ | 5 | |
| `$network.wifi{ssid,bssid}` `v4{primaryAddress,primaryInterface,primaryRouter}` `v6{...}` `dns[]` `cellular-data` | | 🟡 | 5 | 无 `cellular-data` |
| `$argument` | | ✅ | 5 | |
| `$trigger`：`editor` `http-api` `intent` `button` `auto-interval` | | 🟡 | 5 / 6 | `editor` `intent` 不会出现 |
| `$intent.parameter` | 快捷指令 | ⛔ | — | |
| `$input {purpose:"panel", position, panelName}` | 面板调用 | ✅ | 6 | |
| `$done([result])` | 必须恰好调用一次；多余忽略；未调用则超时；未捕获异常使脚本无效 | ✅ | 5 | |
| `$httpClient.get/post/put/delete/head/options/patch(options, callback)` | 选项：`url` `headers` `body`（字符串 / 对象→JSON / TypedArray）`timeout`（5）`policy` `policy-descriptor` `insecure` `auto-redirect`（默认开）`auto-cookie`（默认开）`binary-mode` `full-header-mode`；回调 `(error, {status, headers}, data)`；每次运行最多 20 并发；正文上限 256 MB（macOS）；请求出现在抓包视图 | ✅ | 5 | |
| `$httpAPI(method, path, body, callback)` | 免鉴权调用自身 HTTP API | ✅ | 6 | |
| `$persistentStore.write(data, [key])` / `read([key])` | 同 `script-path` 共享；`null` 删除；单值 32 MB（macOS）；磁盘目录可直接编辑 | ✅ | 5 | 存于 rurge 数据目录 |
| `$notification.post(title, subtitle, body, [options])` | 选项 `action`（`open-url` / `clipboard`）`url` `text` `media-url` `media-base64` `media-base64-mime` `auto-dismiss` `sound` | 🟡 | 6 | 桌面通知（Win Toast / Lin D-Bus / mac 通知中心）；媒体附件与点击动作尽力实现 |
| `notification` 事件脚本内禁止 `$notification.post` | 抛异常 | ✅ | 5 | |
| `$utils.geoip(ip)` `ipasn(ip)` `ipaso(ip)` `ungzip(bytes)`（输出上限 128 MB） | | ✅ | 5 | |
| `$surge.setSelectGroupPolicy(group, policy)` `selectGroupDetails()` `retestGroup(group, cb)` `setOutboundMode(mode)` `setHTTPCaptureEnabled` `setRewriteEnabled` `logbook(text)` | | ✅ | 5 / 6 | |
| `$surge.setEnhancedModeEnabled(bool)` | Mac only | ✅ | 5 | 切换 TUN |
| `$surge.setCellularModeEnabled(bool)` | Mac only | 🔁 | 5 | 返回 false |
| `console.log(msg)` | 对象 JSON 化；单行截断 512 KB | ✅ | 5 | |
| `setTimeout(fn, delay)` / `clearTimeout` | 最长 24 小时、最多 64 个；运行结束取消全部 | ✅ | 5 | `clearTimeout` 始终可用 |
| `setInterval` 等未文档化的函数 | 不存在 | 🔁 | 5 | 可作为扩展提供，但不承诺 |

### 8.3 各脚本类型

| 类型 | 输入 | 结果 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- | --- |
| `http-request` | `$request.url` `method` `headers` `body`（需 `requires-body`）`id`；`pattern` 同时对 Host 与 SNI 替换后的 URL 匹配；首个命中 | `url`（不改 Host）`headers` `body` `response{status,headers,body}`（直接应答）`abort` | ✅ | 5 | chunked / `Expect: 100-continue` 不改写正文 |
| `http-response` | `$request.url` `method` `headers` `id`；`$response.status` `headers` `body` | `status` `headers` `body`（无 `requires-body` 时返回 body 会中止连接）`abort` | ✅ | 5 | |
| `rule` | `$request.hostname` `destPort` `sourcePort` `protocol`（HTTP/HTTPS/TCP/UDP/QUIC/STUN）`processPath` `userAgent` `url` `sourceIP` `listenPort` `dnsResult{v4Addresses,v6Addresses}`（需 `requires-resolve`）；缺失字段为 `null` | `matched: true/false` | ✅ | 5 | `processPath` 在三平台均提供 |
| `dns` | `$domain` | `address` / `addresses` / `server` / `servers` 之一，可附 `ttl`；`{}` 回退标准解析 | ✅ | 5 | |
| `event` | `$event.name` `$event.data`（`notification` 事件含 `title` `subtitle` `body` `identifier` `script-options`）；手动触发时 `name = "manually"` | 忽略 | ✅ | 5 | 手动触发通过 API / CLI |
| `cron` | `$cronexp`；5 / 6 段表达式，支持 `sun` 等名字与 `*/n` 步进 | 忽略 | ✅ | 5 | 每小时 >10 次只记录告警 |
| `generic` | `$trigger` `$intent` `$input` | 手动运行忽略；面板运行返回 `title`（必填）`content` `style` `icon` `icon-color` | ✅ | 5 / 6 | |

---

## 9. 高级网络功能

### 9.1 增强模式（VIF / TUN）

| 项 | Surge 行为 | Surge 平台 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- | --- |
| 创建虚拟网卡并注册为默认路由，接管不遵守系统代理的应用 | Mac 手动开启；iOS 默认 | 全部 | ✅ | 3 | Win: Wintun；Lin: `/dev/net/tun`；mac: utun |
| 只处理 TCP / UDP / ICMP | | 全部 | ✅ | 3 | |
| ICMP 由 VIF 本地应答（ping 可用）；`icmp-forwarding=false` 关闭 | | 全部 | ✅ | 3 | |
| fake IP `198.18.0.0/15`（见 6.4） | | 全部 | ✅ | 3 | |
| `tun-excluded-routes` / `tun-included-routes` / `ipv6-vif` | | 全部 | ✅ | 3 | |
| `vif-mode` | Mac 5.8 起无操作 | Mac | 🔁 | 3 | |
| 开关不写在 Profile 中，由 UI / API / CLI 控制 | | Mac | ✅ | 3 / 6 | 另提供命令行参数在启动时开启 |
| 实现基础：Surge Mac 5.8+ 基于 Network Extension | | Mac | 🟡 | 3 | rurge 直接操作 utun，需要 root；Windows 需管理员；Linux 需 `CAP_NET_ADMIN` |

### 9.2 网关模式

| 项 | Surge 行为 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| 作为 L3 网关处理局域网其他设备流量，走同一规则 / 策略 / DNS 流水线 | Mac only；UI 开启 | 🟡 | 7 | Lin / mac 计划支持；Windows 需 WinDivert 或等价机制，❓ 待评估 |
| 内置 DHCP 服务器通告自身为网关与 DNS | | ✅ | 7 | |
| 设备列表：查看流量、自定义设备名；Dashboard / CLI `device` | | ✅ | 7 | |
| 按设备策略：`SRC-IP` `DEVICE-NAME` `MAC-ADDRESS` | | ✅ | 7 | |
| `gateway-restricted-to-lan`（默认 true） | | ✅ | 7 | |
| UDP Fast Path（Mac 6.4+）：客户端 1 秒 10 连接或 10 秒 30 连接时降级为 L3 直转，绕过规则与 MITM；<1024 端口不降级；可按设备开关 | Gateway VM 模式 | 🟡 | 7 | 作为可选优化实现，阈值可配置 |
| Gateway VM 模式 / `vmnet` 虚拟接口诊断 | Mac 专属实现 | 🟡 | 7 | 各平台实现方式不同，`vmnet` 命令语义按平台调整 |

### 9.3 `[DHCP]`（Mac 6.5+）

| 键 | 取值 | rurge | 阶段 |
| --- | --- | --- | --- |
| `max-lease-time` `default-lease-time` `min-lease-time` | 秒 | ✅ | 7 |
| `one-lease-per-client` | 布尔 | ✅ | 7 |
| `ping-check` | 布尔；分配前探测地址是否占用 | ✅ | 7 |
| 静态分配地址自动从动态池排除 | | ✅ | 7 |

### 9.4 `[SSID Setting]` 子网设置

| 项 | Surge 行为 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| 行格式 `<子网表达式> key=value, key=value`；表达式见 3.4 | | ✅ | 3 | |
| `suspend` | 匹配网络下暂停 | ✅ | 3 | 暂停 = 停止接管并释放系统代理 / TUN，保留 API |
| `cellular-fallback`（`default` `off` `wifi-assist` `hybrid`） | iOS 专属 | 🔁 | 3 | |
| `cellular-mode` | Mac：视为计费网络（Metered Network Mode） | ❓ | 远期 | 计费网络模式（应用白名单）整体列为远期评估 |
| `tfo-behaviour`（`auto` `force-enabled` `force-disabled`） | | 🟡 | 3 | 依平台 TFO 支持情况 |
| `dns-server` / `encrypted-dns-server`（含 `off`） | 按网络覆盖 | ✅ | 3 | |

### 9.5 `[Port Forwarding]`（iOS 5.14.3 / Mac 5.10+）

| 项 | Surge 行为 | rurge | 阶段 |
| --- | --- | --- | --- |
| `<listen-address:port> <target-host:port> policy=<name>`；仅 TCP；不依赖系统代理 / 增强模式 | | ✅ | 7 |
| `policy` 省略时走标准规则匹配 | | ✅ | 7 |

### 9.6 Surge Ponte

| 项 | Surge 行为 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| 设备间网络：iCloud 注册、`<name>.sgponte` 主机名、`DEVICE:<name>` 策略、多种通道（直连 / NAT 穿透 / 代理辅助穿透 / 仅局域网 / IPv6）；服务器仅 Mac | | ⛔ | 远期 | 依赖 Apple 账号体系与专有协议。替代方案：WireGuard 策略 + 自建节点，在文档中给出示例 |
| `[Ponte] client-proxy-name` `server-proxy-name` | | 🔁 | 1 | 解析后忽略 |
| Dashboard / 远程控制器经 `.sgponte` 连接 | | ⛔ | — | |

### 9.7 `[Snell Server]` 内置 Snell 服务器（Mac only）

| 键 | 取值 / 默认 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| `interface` `port` `psk` | 监听地址、端口、预共享密钥 | 🟡 | 7 | 支持范围与客户端实现的 Snell 版本一致 |
| `version` | `1` / `6`；默认 1 | 🟡 | 7 | v1 计划支持；v6 ❓ |
| `mode` | v6：`default` `unshaped` `unsafe-raw` | ❓ | 7 | |

### 9.8 `[MTProto]` 内置 MTProto 代理服务器（iOS 5.21 / Mac 6.8+）

| 项 | Surge 行为 | rurge | 阶段 |
| --- | --- | --- | --- |
| 每 Profile 一个 `[MTProto]` 节、一个监听器 | | ✅ | 7 |
| `interface`（必填）`port`（必填）`secret`（32 位十六进制，可带 `dd` 前缀）`ipv6`（默认 false，只影响出站 DC 地址族）`dc-config-url`（默认官方 JSON） | | ✅ | 7 |
| 连接按普通 TCP 请求进入规则系统，目标为映射后的 DC IP；`PROTOCOL,MTProto` 可匹配 | | ✅ | 7 |
| 带符号 DC ID（正数 general，负数 media）；8 步端点选择；失败标记与轮换；全部失败后清零 | | ✅ | 7 |
| 内置 DC 快照；持久映射 30 天过期触发一次非阻塞更新；失败保留旧映射 | | ✅ | 7 |
| 自定义 DC 配置：HTTP 200、合法 JSON、≤256 KiB、`version=1`、`options[]`（`id` `ip` `port` `flags` 位集 `secret`） | | ✅ | 7 |
| 请求记录标注 `(Telegram DC 2 Static/Media/IPv6)`；流量方向按客户端视角 | | ✅ | 7 |
| 一进一出、不复用后端连接、AES-CTR 流式处理 | | ✅ | 7 |

### 9.9 其他

| 项 | Surge 行为 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| Metered Network Mode（Mac）：限制可联网的应用 / 进程 | UI 功能 + `cellular-mode` | ❓ | 远期 | |
| `[Testing]`（Mac 6.4.4+）：`download-url` `upload-url` `download-url-proxy` `upload-url-proxy` `download-concurrency`（4）`upload-concurrency`（4）`download-duration-limit`（10s）`upload-size-limit`（1GB）`upload-duration-limit`（10s） | 吞吐测试参数 | ✅ | 6 | 配合 CLI `test-policy-bandwidth` |

---

## 10. 工具与可观测性

### 10.1 Dashboard 与远程访问

| 项 | Surge 行为 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| Surge Dashboard 原生应用（Mac）：请求查看、DNS 缓存、设备管理 | | ⛔ | — | 专有控制协议（`external-controller-access`）不复刻 |
| Web Dashboard：由 HTTP API 端口提供，`http-api-web-dashboard=true` | | ✅ | 6 | rurge 自带 Web Dashboard，覆盖请求 / 策略 / 规则 / DNS / 设备 / 模块 / 脚本 / 日志 |
| 远程访问：Wi-Fi / USB 连接 iOS 实例 | | ⛔ | — | |
| 远程读取 Logbook | | ✅ | 6 | 通过 HTTP API |

### 10.2 Logbook（Mac 6.6+）

| 项 | Surge 行为 | rurge | 阶段 |
| --- | --- | --- | --- |
| 持久记录事件：配置重载、网络切换、崩溃恢复、更新、DHCP 变化、脚本运行（含输入 / 输出 / 日志） | | ✅ | 6 |
| 默认保留 7 天 | | ✅ | 6 |
| CLI `logbook` 读取；Dashboard 远程查看 | | ✅ | 6 |

### 10.3 CLI

Surge 的 `surge-cli` 是随 Mac 版附带的控制工具。rurge 的 `rurge` 二进制同时是守护进程与客户端：`run` / `check` 等命令本地执行，其余命令通过 HTTP API 操作运行中的实例。

| Surge 命令 | 用途 | rurge 对应 | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| 无参数进入交互模式（补全、历史） | | `rurge shell` | 6 | |
| `--raw` | 输出原始 JSON | `--json` | 6 | |
| `--remote / -r <host:port>` `--password-stdin` `SURGE_CLI_PASSWORD` | 远程实例 | `--remote` `--password-stdin` `RURGE_CLI_PASSWORD` | 6 | 远程走 HTTP API |
| `--check / -c <path>` | 校验配置 | `rurge check -c <path>` | 1 | |
| `--help` / `help <command>` | | 同 | 1 | |
| `status` `summary` `version` | 状态 | 同名 | 6 | |
| `mode` `global-policy` `policy-group` | 出站模式 / 全局策略 / 组选择与清除覆盖 | 同名 | 6 | |
| `rule match` `rule explain` `rule temp`（Mac 6.9+） | 规则测试 / 解释 / 临时规则 | 同名 | 6 | 临时规则位于全部规则之前，停止时丢弃 |
| `profile`（inspect / validate / list / switch / diff） | 配置管理 | 同名 | 6 | `profile diff` 显示模块叠加后的有效配置 |
| `module` `feature` `managed-profile update` `external-resource` | 模块 / 功能开关 / 托管配置更新 / 外部资源 | 同名 | 6 | `feature` 覆盖 MITM、Rewrite、Scripting、HTTP Capture、Packet Capture、System Proxy、Enhanced Mode；Cellular Mode 🔁 |
| `dns lookup` `dns trace` `flush dns` `geoip` `http probe` `test` `diagnostics` | 网络诊断 | 同名 | 6 | |
| `dump summary/performance/rule-usage/virtual-ip` `watch speed` `log` `log watch` `logbook` `proxy-runtime-status` | 检视 | 同名 | 6 | |
| `script list/run` `script-log` `benchmark encryption/rule-matching` `test-policy-bandwidth` | 自动化与基准 | 同名 | 6 | |
| `device` `reconnect-device` `vmnet` `security ban` | 网关 | `device` `security ban` 同名；`reconnect-device` 🔁；`vmnet` 🟡 按平台 | 7 | |
| `reload` `switch-profile` `kill` `stop` `unattended-upgrade` | 控制 | `reload` `switch-profile` `kill` `stop` 同名；`unattended-upgrade` 🔁 | 1 / 6 | rurge 自身更新由包管理器负责；SIGHUP / `--watch` 的配置热重载已实现（M3b，重建整个 Stack，DNS 缓存随之清空）；重建监听器的判据是**监听器配置面**（监听地址集合、`password@` / wifi 认证、`proxy-restricted-to-lan`、两个错误页开关）任一变化——只比地址会让密码轮换与来源限制静默不生效（M3b 修复波订正）；重绑失败会留下「零监听器」的退化态，此时下一次重载无条件重试绑定；`--watch` 的监视列表在启动时固定，重载新增的 `#!include` 需重启才会被监视；`rurge reload` 命令与 API 触发仍在 M4 |
| `environment` `set` `set-log-level` | 环境 | 同名 | 6 | |
| Agent Skill（Mac 6.5+） | 面向 AI 代理的技能文档 | 🟡 | 6 | rurge 仓库可提供等价 skill 文档 |
| rurge 专有 | 守护进程 | `rurge run -c <path> [--tun] [--system-proxy]`、`rurge service install/uninstall`、`rurge mitm ca generate/export` | 1 / 3 / 4 | |
| rurge 专有开发命令 | 离线（不启动守护进程）在当前进程内构建配置并评估一次会话，用于调试规则与规则集 | `rurge rule match -c <conf> <host[:port]> [--explain] [--json] [--resolve <ip,...>\|--no-dns] ...` | 1 | 见 M2 设计文档 §10.1；阶段 6 的 `rule match`/`rule explain` 经 HTTP API 查询运行中的守护进程，语义一致但走线上实例 |
| rurge 专有开发命令 | 离线按配置的 DNS 设置解析域名，`--server` 覆盖上游，`--trace` 打印每次尝试；`dns cache` 打印本进程缓存快照 | `rurge dns lookup -c <conf> <name> [--type a\|aaaa\|both] [--server <spec>...] [--no-cache] [--trace] [--json]`；`rurge dns cache -c <conf> [name...]` | 1 | 见 M2 设计文档 §10.2；阶段 6 的 `dns lookup` 经 HTTP API 查询守护进程 |
| rurge 专有命令 | 前台运行 HTTP / SOCKS5 代理；出站模式初值来自 `--outbound-mode`（M4 起 `state.json` 优先）；`--log-level` 覆盖 `loglevel` | `rurge run -c <conf> [--outbound-mode direct\|proxy=<p>\|rule] [--log-level <l>] [--idle-timeout <secs>] [--request-log-size <n>] [--watch] [--log-file <path>]` | 1 | 见 M3 设计文档 §9.3；`--idle-timeout` / `--request-log-size` / `--watch` / `--log-file` 为 rurge 专有运行时选项（M3b，只经 CLI 参数 / 环境变量提供，不写入 Surge 配置）；`reload` / `stop` 命令仍依赖 M4 的控制通道，但 SIGHUP / `--watch` 的热重载已可用 |

### 10.4 HTTP API

鉴权：请求头 `X-Key: <key>` 或查询参数 `?x-key=<key>`。rurge 保持路径、方法与 JSON 字段名一致，以便现有 Dashboard 与脚本直接对接。

| 端点 | 用途 | Surge 平台 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- | --- |
| `GET/POST /v1/features/mitm` `capture` `rewrite` `scripting` | 功能开关 `{"enabled":bool}` | 全部 | ✅ | 1 / 4 / 5 | 阶段 1 提供开关状态骨架 |
| `GET/POST /v1/features/system_proxy` | 系统代理开关 | Mac only | ✅ | 1 | |
| `GET/POST /v1/features/enhanced_mode` | 增强模式开关 | Mac only | ✅ | 3 | |
| `GET/POST /v1/outbound` | `{"mode":"direct"\|"proxy"\|"rule"}` | 全部 | ✅ | 1 | |
| `GET/POST /v1/outbound/global` | 全局模式策略 | 全部 | ✅ | 1 | |
| `GET /v1/policies` | 列出策略 | 全部 | ✅ | 1 | |
| `GET /v1/policies/detail?policy_name=` | 策略详情 | 全部 | ✅ | 2 | |
| `POST /v1/policies/test` | `{"policy_names":[...],"url":...}` | 全部 | ✅ | 2 | |
| `GET /v1/policy_groups` | 列出组与选项 | 全部 | ✅ | 2 | |
| `GET /v1/policy_groups/test_results` | 自动组测试结果 | 全部 | ✅ | 2 | |
| `GET/POST /v1/policy_groups/select` | 读 / 改 select 组选择 | 全部 | ✅ | 2 | |
| `POST /v1/policy_groups/test` | 立即测试 → `{"available":[...]}` | 全部 | ✅ | 2 | |
| `GET /v1/requests/recent` `GET /v1/requests/active` `POST /v1/requests/kill` | 请求列表与终止 | 全部 | 🟡 | 1 / 4 | 响应结构手册未定义，以 Surge 实际输出为准做兼容测试 |
| `GET /v1/profiles/current?sensitive=0` | 当前配置文本（可脱敏） | 全部 | ✅ | 1 | |
| `POST /v1/profiles/reload` | 重载 | 全部 | ✅ | 1 | 底层热重载能力（SIGHUP / `--watch`）已在 M3b 就位，API 触发在 M4 暴露 |
| `POST /v1/profiles/switch` `GET /v1/profiles` `POST /v1/profiles/check` | 多配置管理 | Mac only | ✅ | 1 / 6 | rurge 以配置目录管理多个 Profile |
| `POST /v1/dns/flush` `GET /v1/dns` `POST /v1/test/dns_delay` | DNS | 全部 | ✅ | 1 | |
| `GET/POST /v1/modules` | 模块列表与开关 | 全部 | ✅ | 5 | |
| `GET /v1/scripting` `POST /v1/scripting/evaluate` `POST /v1/scripting/cron/evaluate` | 脚本列表 / mock 执行 / 运行 cron | 全部 | ✅ | 5 | |
| `GET /v1/devices` `GET /v1/resources/devices-icon?id=` `POST /v1/devices` | 设备管理（`physicalAddress` 必填；`name` `address` `shouldHandledBySurge`） | Mac only | ✅ | 7 | |
| `POST /v1/stop` | 关闭引擎 | 全部 | 🟡 | 1 | rurge：停止引擎并退出进程；由服务管理器决定是否重启 |
| `GET /v1/events` | 事件中心 | 全部 | ✅ | 6 | 与 Logbook 共用存储 |
| `GET /v1/rules` | 规则列表 | 全部 | ✅ | 1 | |
| `GET /v1/traffic` | 流量信息 | 全部 | ✅ | 1 | 底层按策略 / 监听器的流量统计能力已在 M3b 就位（`TrafficStats`），API 读取在 M4 暴露 |
| `POST /v1/log/level` | `{"level":"verbose"\|"debug"\|"info"\|"warning"\|"error"}` | 全部 | ✅ | 1 | |
| `GET /v1/mitm/ca` | DER 格式 CA 证书 | 全部 | ✅ | 4 | |
| `GET /v1/metrics?x-key=` | Prometheus 文本格式（iOS 5.22 / Mac 6.9+） | 全部 | ✅ | 6 | 指标名保持 `surge_*` 前缀以兼容现有 Grafana 面板：`surge_build_info` `surge_uptime_seconds` `surge_memory_bytes` `surge_active_requests` `surge_dns_cache_entries` `surge_active_bans` `surge_interface_in/out_bytes_total{interface}` `surge_policy_in/out_bytes_total{policy}` |
| 未授权访问封禁（`security ban`、`surge_active_bans`） | 反复错误鉴权后封禁来源 | 全部 | ✅ | 6 | |
| `http-api-tls` | HTTPS 服务 API，使用 MITM CA | 全部 | ✅ | 4 | |

### 10.5 URL Scheme

| 项 | Surge 行为 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| `surge:///start` `stop` `toggle`（`autoclose=true`） | iOS 专属 | 🟡 | 8 | 桌面 GUI 阶段以 `rurge://` 提供等价动作 |
| `surge:///install-config?url=` `surge:///install-module?url=`（`surgeconfig://` 别名） | 全平台 | 🟡 | 8 | rurge 注册 `rurge://`，不占用 `surge://`；CLI 提供 `rurge profile install <url>` / `module install <url>` |
| `email-license` `enterprise-license` `team-license` | 许可证 | ⛔ | — | |
| x-callback-url | | 🔁 | 8 | |

### 10.6 `[Panel]` 信息面板（iOS 4.9.3 / Mac 5.7.5+）

| 项 | Surge 行为 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| 行格式 `<name> = title=, content=（支持 \n）, style=good\|info\|alert\|error, script-name=, update-interval=, icon=<SF Symbol>, icon-color=<hex>` | | ✅ | 6 | 在 Web Dashboard 与 API 中呈现 |
| 静态模式（固定内容，随托管配置更新） | | ✅ | 6 | |
| 动态模式：`script-name` 指向 generic 脚本；`$input{purpose,position,panelName}`；`$trigger` 为 `button` / `auto-interval`；返回 `title` `content` `style` `icon` `icon-color` | | ✅ | 6 | |
| `update-interval`：用户打开面板时判断是否到期 | | ✅ | 6 | |
| `icon`（SF Symbol 名） | Apple 图标体系 | 🟡 | 6 | Dashboard 做名称到通用图标的映射，未知名称忽略 |
| rurge 扩展 | | `GET /v1/panels` 读取面板内容 | ❓ | 6 | Surge 无对应 API，是否新增在阶段 6 决定 |

---


---

## 附录：统计

按各节表格行的首个状态标记统计（由脚本生成，随清单更新）。

| 章节 | 条目 | ✅ | 🟡 | 🔁 | ⛔ | ❓ |
| --- | --- | --- | --- | --- | --- | --- |
| 1. 配置文件格式与指令 | 66 | 55 | 7 | 1 | 2 | 1 |
| 2. `[General]` 选项 | 60 | 43 | 8 | 9 | 0 | 0 |
| 3. 规则系统 | 61 | 47 | 10 | 3 | 1 | 0 |
| 4. 出站策略 | 85 | 75 | 6 | 1 | 0 | 3 |
| 5. 策略组 | 29 | 26 | 2 | 1 | 0 | 0 |
| 6. DNS 与 `[Host]` | 41 | 40 | 1 | 0 | 0 | 0 |
| 7. HTTP 处理 | 45 | 41 | 2 | 1 | 1 | 0 |
| 8. 脚本 | 47 | 35 | 7 | 4 | 1 | 0 |
| 9. 高级网络功能 | 43 | 28 | 7 | 3 | 2 | 3 |
| 10. 工具与可观测性 | 50 | 36 | 6 | 4 | 3 | 1 |
| **合计** | **527** | **426** | **56** | **27** | **10** | **8** |
