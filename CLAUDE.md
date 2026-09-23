# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 项目是什么

rurge 是用 Rust 复刻 Surge（macOS / iOS 网络代理工具）全部功能的跨平台核心，原生兼容 Surge 的 `.conf` / `.sgmodule` 配置格式、JavaScript 脚本 API 与 HTTP API。目标平台 Windows / Linux / macOS；产品形态是单二进制守护进程 `rurge` + CLI + Surge 兼容 HTTP API，Web Dashboard 与桌面 GUI 放在后期阶段。

## 当前状态（2026-09）

阶段 1 功能已齐备（M1 ～ M4b）；阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`。M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。

## 先读这些文档

- `docs/requirements.md`：需求文档（PRD）。定位与非目标（第 2 节）、架构与连接流水线（第 3 节）、按模块编号的功能需求 `FR-<模块>-<序号>`（带优先级 P0/P1/P2 与所属阶段，第 4 节）、非功能需求、平台差异矩阵、8 个阶段的路线图与验收标准（第 7 节）、风险与开放问题。
- `docs/surge-compatibility-matrix.md`：逐项兼容清单（约 530 条）。每个 Surge 配置键、规则类型、策略参数、脚本 API、HTTP API 端点、CLI 命令在 rurge 中的计划状态（✅ 完全支持 / 🟡 有差异 / 🔁 解析并忽略 / ⛔ 不支持 / ❓ 待评估）与实现阶段。实现任何 Surge 特性前先查这里；与 Surge 的任何行为差异必须登记在这里。
- `docs/superpowers/specs/2026-09-03-phase1-core-skeleton-design.md`：阶段 1 设计文档。crate 边界与依赖方向、配置数据模型、规则引擎索引、DNS 流程、连接流水线、REJECT 行为、阶段 1 的 API / CLI、里程碑 M1 ～ M4。
- `docs/superpowers/specs/2026-09-04-phase1-m2-rules-dns-design.md`：M2 设计文档。`rurge-net`（连接器 / HTTP 客户端 / 外部资源管理器）、`rurge-rules`（索引结构、规则集、匹配器、GeoIP / ASN、规则引擎）、`rurge-dns`（M2b）的接口与语义；第 15 节列出需登记进兼容性清单的行为差异。
- `docs/superpowers/plans/2026-09-03-phase1-m1-config-parser.md`：M1 实施计划（14 个任务，含完整代码与测试）。执行时按任务顺序推进，每个任务结束跑 `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace`。
- `docs/superpowers/plans/2026-09-04-phase1-m2a-rules-plan.md`：M2a 实施计划（16 个任务）。末尾「执行期修正记录」表记录了与计划的偏差及基准数字。
- `docs/superpowers/plans/2026-09-04-phase1-m2b-dns-plan.md`：M2b 实施计划（14 个任务）。末尾「执行期修正记录」与「延后事项」同 M2a。
- `docs/superpowers/specs/2026-09-05-phase1-m3-pipeline-design.md`：M3 设计文档。四个 crate（`rurge-proto` / `rurge-policy` / `rurge-inbound` / `rurge-engine`）的接口、dial 流水线、REJECT 语义、`state.json`、`rurge run`。
- `docs/superpowers/plans/2026-09-05-phase1-m3a-pipeline-plan.md`：M3a 实施计划。末尾「执行期修正记录」与「延后事项」同 M2a。
- `docs/superpowers/plans/2026-09-06-phase1-m3b-operability-plan.md`：M3b 实施计划。末尾「执行期修正记录」与「延后事项」同 M2a。
- `docs/superpowers/specs/2026-09-07-phase1-m4-control-plane-design.md`：M4 设计文档。控制面（M4a：`rurge-api`、`StateStore`、`Control` trait、`rurge reload/stop/status`）与平台集成（M4b：系统代理、服务安装）的接口与语义；第 14 节记录 M4a、第 15 节记录 M4b 实施期与设计的偏差。
- `docs/superpowers/plans/2026-09-07-phase1-m4a-control-plane-plan.md`：M4a 实施计划（9 个任务）。末尾「执行期修正记录」与「延后事项」同 M2a。
- `docs/superpowers/plans/2026-09-18-phase1-m4b-platform-plan.md`：M4b 实施计划（10 个任务）。开头「计划期决定」表（P1–P11）记录与设计文档文字的出入；末尾「执行期修正记录」与「延后事项」同 M2a。
- `docs/acceptance/phase1-manual.md`：阶段 1 系统代理（三平台）与 `rurge service install/uninstall` 的手工验收清单，需要真实桌面环境，不能被自动化测试覆盖。
- `docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`：阶段 2 总设计文档（出站协议与策略组）。范围与八个里程碑（M1 出站地基与 HTTP / SOCKS5 上游 → M2 TLS 族 → M3 策略组与订阅 → M4 WireGuard / SSH / external → M5 UDP → M6 Shadowsocks / Snell / HTTP/2 族 → M7 QUIC 族 → M8 收尾）、技术选型、按传输族拆分的协议 crate、`Outbound` / `Dialer` / `Datagram` 抽象、`PolicySpec`、策略运行时、三层测试策略（不使用 Docker）、已决事项 D1–D11 与开放问题；每个里程碑开工前另写细化设计与实施计划。
- `docs/superpowers/specs/2026-09-19-phase2-m1-outbound-foundation-design.md`：阶段 2 / M1 设计文档（出站地基与 HTTP / SOCKS5 上游）。`rurge-config::spec`（`PolicySpec`、`ParamReader`、诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、竞速的 `DirectConnector`）、TLS 层与 p12、`http(s)` / `socks5(-tls)` 出站与明文 HTTP 的绝对 URI 转发、`OutboundFactory`、`RegistryCell` / `ChainConnector`、`SelectionTable`、干构建、四个策略 / 组 API 端点、三层测试；拆成 M1a（库）与 M1b（装配与控制面）两份计划；第 11 节登记了对阶段 2 总设计的八处订正。
- `docs/superpowers/specs/2026-09-20-phase2-m2-tls-family-design.md`：阶段 2 / M2 设计文档（TLS 族）。三份计划的拆分（M2a Trojan 优先 → M2b VMess / AnyTLS → M2c Shadow TLS）、`rurge-config::spec` 的 `WsOpts` / `TrojanSpec` / `VmessSpec` / `AnyTlsSpec` / `ShadowTlsOpts`、传输阶梯 `transport::Stack`（connect → shadow-tls → tls → ws）、WebSocket 层、惰性请求头 `LazyHead`、三种协议的线上格式与语义、重载时按指纹复用出站与 `ResolverCell`、M1b 承接事项的处理、三层测试（xray 只用于 vmess）；附录 A 记录 Shadow TLS v3 在 stock rustls 上签名 ClientHello 的做法与 spike 结论；第 15 节列出写各份计划时必须核对的事项。
- `docs/superpowers/plans/2026-09-19-phase2-m1a-outbound-library-plan.md`：M1a 实施计划（13 个任务）。开头「计划期决定」表（P1–P10）记录写计划时核对源码得出的结论（`socket2` 无 TFO 封装、`p12-keystore` 用 0.2 等）；末尾「执行期修正记录」与「延后事项」同 M2a。
- `docs/superpowers/plans/2026-09-19-phase2-m1b-assembly-control-plane-plan.md`：M1b 实施计划（12 个任务）。开头「计划期决定」表（P1–P12）与「承接事项」（接手 M1a 计划「延后事项」里标给 M1b 的条目）；末尾「执行期修正记录」与「延后事项」两张表。
- `docs/superpowers/plans/2026-09-20-phase2-m2a-trojan-plan.md`：阶段 2 / M2a（Trojan 优先）实施计划（8 个任务）。开头「计划期决定」表（P1–P14）；末尾「执行期修正记录」与「延后事项」两张表。
- `docs/superpowers/plans/2026-09-20-phase2-m2b-vmess-anytls-plan.md`：阶段 2 / M2b（VMess / AnyTLS）实施计划（12 个任务）。开头「计划期决定」表（P1–P18）与「承接事项」（接手 M2a 计划「延后事项」里标给 M2b 的四条）；末尾「执行期修正记录」与「延后事项」两张表。
- `docs/superpowers/plans/2026-09-21-phase2-m2c-shadow-tls-plan.md`：阶段 2 / M2c（Shadow TLS v2 / v3）实施计划（9 个任务）。开头「计划期决定」表（P1–P20）记录对照参考实现与手册核对出的逐字节细节，以及与设计文字不同的决定（不写 `shadow-tls-sni` 时不发 SNI、alert 记录跳过、读取不设 16 KiB 上限、v2 握手后仍在转发的记录先交给伪装会话等）；末尾「执行期修正记录」与「延后事项」两张表。
- `docs/acceptance/phase2-manual.md`：阶段 2 手工验收清单（M2a 起新建），需要真实公网节点的项目，自动化测试（只用回环）覆盖不了，由项目所有者用自己的节点验收。
- `docs/api/phase1.md`：阶段 1 HTTP API 参考——端点、JSON 形状、鉴权与封禁、系统代理的地址 / `skip-proxy` 转换 / 生命周期、`rurge reload/stop/status` 客户端。
- `docs/api/phase2.md`：阶段 2 HTTP API 参考——M1 新增的四个策略 / 策略组端点（`policies/detail`、`policy_groups`、`policy_groups/select`）的响应形状、`lineHash` 的定义、选择的生效时机与持久化位置。
- `README.md`（中文）与 `README_en.md`（英文）：对外的状态、特性表与路线图，两份内容保持一致，并与 PRD 保持一致；文件顶部互相链接。

## 工作流约定

- 每个阶段：先写设计文档（`docs/superpowers/specs/YYYY-MM-DD-<topic>-design.md`），再写实施计划（`docs/superpowers/plans/`），再实现。新工作必须能对应到 PRD 第 7 节的某个阶段和第 4 节的 FR 编号。
- 兼容性原则：Surge 语法就是 rurge 语法。不认识或平台不适用的配置项要"解析并忽略 + 记录日志"，不能报错；rurge 不改写用户的配置文件；rurge 专有的运行时选项只通过命令行参数和环境变量提供，不扩展 Surge 配置格式（FR-CFG-17）。
- 事实来源：Surge 官方手册 <https://manual.nssurge.com/>（本项目基于 2026-09 版本，对应 Surge Mac 6.9 / iOS 5.22）。手册改版频繁，有疑问查手册，不凭记忆。已知的常见误区：`doh-server` 已改名 `encrypted-dns-server`；URL Rewrite 只有 `header` / `302` / `307` / `reject` 四种模式；`[MITM]` 没有 `tcp-connection` 键；url-test 组的测试 URL来自策略的 `test-url` 或全局 `proxy-test-url`。
- 语言：文档与对话用中文；README 分中文（`README.md`）与英文（`README_en.md`）两份；日志与 CLI 输出以英文为主（NFR-10）。
- 提交：用户自行决定何时 commit；未被要求时不要提交。

## 计划中的架构（阶段 1 建立后生效）

- Rust stable，Cargo workspace，按职责拆 crate：`rurge-config`（解析 / 校验 / include / 模块叠加 / 托管配置）、`rurge-rules`、`rurge-dns`（客户端 / 加密 DNS / `[Host]` / fake-IP）、`rurge-policy`（策略组 / 测试 / 订阅）、`rurge-proto`（出站协议）、`rurge-net`（内部 HTTP 客户端 / 外部资源管理）、`rurge-inbound`、`rurge-engine`（会话流水线 / 请求记录 / 运行时状态）、`rurge-tun`（虚拟网卡 / 协议栈 / 网关 / DHCP）、`rurge-http`（HTTP 引擎 / MITM / 重写 / 抓包）、`rurge-script`、`rurge-api`、`rurge-platform`、`rurge`（bin）。职责与依赖见 PRD 3.2；平台特定代码只允许出现在 `rurge-platform` 与 `rurge-tun`（AR-02）。
- 依赖方向（M2 设计文档确认）：`rurge-dns → rurge-rules → rurge-net → rurge-config`；`[Host]` 集合键与 `LazyResolver` 都由 `rurge-dns` 依赖 `rurge-rules` 提供，而非并列关系；`rurge (bin) → rurge-engine → { rurge-inbound → rurge-proto, rurge-policy → rurge-proto, rurge-dns }`。M4 设计文档确认：`rurge (bin) → rurge-api → rurge-engine`；`rurge-api` 依赖 `rurge-engine` / `rurge-config`（`rurge-dns` 的类型经 `rurge-engine` 间接可达，不需要直接依赖），不依赖 `rurge-platform`，也不认识 bin。`tests/interop`（`rurge-interop`，`publish = false`）是仅测试用的工作区成员。
- 连接处理流水线（PRD 3.3）：入站 → 协议嗅探（SNI / Host / QUIC / STUN）→ 预匹配 → 出站模式判断 → 规则匹配（域名规则不触发 DNS，IP 规则按需解析）→ 策略解析（组 / 链式 / 别名）→ 出站建立 → HTTP 引擎（MITM → Header Rewrite → URL Rewrite → Body Rewrite → 脚本 → Map Local 短路）→ 观测。
- 配置对象不可变，重载时原子切换（AR-04）；每个连接是独立 tokio 任务（AR-03）。
- `unsafe_code`：全工作区 `forbid`；只有 `rurge-platform` 用 crate 自己的 `[lints] unsafe_code = "deny"` 放宽，且仅 `sysproxy::windows` 里调用 `InternetSetOptionW` 通知 WinINet 的那一个函数标 `#[allow(unsafe_code)]`（M4b）。

## 常用命令

```bash
cargo test --workspace                          # 全部测试
cargo test -p rurge-config <name>               # 单个 crate / 用例
cargo test -p rurge-api                         # HTTP API 集成测试（鉴权、封禁、阶段 1 端点）
cargo test -p rurge-proto                       # 出站协议库：TLS 层、p12、http / socks5 出站对回环假上游（rurge_proto::testing）、trojan 出站与 WebSocket 层
cargo test -p rurge-proto vmess                 # vmess：KDF / 头密封 / 分块的向量，出站对回环假服务端（FakeVmess）
cargo test -p rurge-proto anytls                # anytls：padding 方案解析、会话层，出站对回环假服务端（FakeAnyTls）
cargo test -p rurge-engine --test outbounds     # 经真实 http / socks5 出站的端到端用例（回环假上游）
cargo test -p rurge-engine --test outbounds_shadow_tls   # 经 Shadow TLS 的端到端用例（回环假服务端 + 夹具自己的伪装站点）
cargo test -p rurge-interop                     # 对 sing-box（全部协议）与 xray（只测 vmess）的互操作测试；没装就跳过（RURGE_TEST_SING_BOX / RURGE_TEST_XRAY / RURGE_INTEROP_REQUIRED=1）
cargo insta test -p rurge-config --review       # 语料库快照有变化时审阅
cargo clippy --all-targets -- -D warnings       # 零警告
cargo fmt --all
cargo run -p rurge -- check -c config.conf      # 校验 Surge 配置（--json / --strict / --platform）
cargo run -p rurge -- rule match -c config.conf example.com --explain   # 离线测试规则匹配
cargo bench -p rurge-rules                      # criterion 基准（域名 / IP 索引、规则引擎）
cargo run -p rurge -- dns lookup -c config.conf example.com --trace    # 离线 DNS 解析（--server 覆盖上游）
cargo bench -p rurge-dns                        # criterion 基准（DNS 缓存命中）
cargo run -p rurge -- run -c config.conf --log-level info   # 前台运行 HTTP / SOCKS5 代理（Ctrl-C 退出）
cargo run -p rurge -- run -c config.conf --watch --log-file rurge.log   # 加配置热重载（SIGHUP / --watch）与滚动日志文件
cargo run -p rurge -- run -c config.conf --system-proxy   # 前台运行并把系统代理指向 rurge，退出时恢复；崩溃后在下次启动时恢复
cargo run -p rurge -- status -c config.conf     # 查看运行中实例的状态（reload / stop 同样支持 -c 或 --remote/--key）
cargo run -p rurge -- service install -c config.conf --user --dry-run   # 打印开机自启的安装计划（去掉 --dry-run 才真正写入 / 执行）
```

`RURGE_SYSTEM_PROXY_BACKEND=file:<path>` 把系统代理的读写重定向到一个 JSON 文件，是仅供自动化测试使用的后端（未知取值会报错退出），不要在正常使用中设置它。

`Cargo.lock` 需要提交（`.gitignore` 已注明）。
