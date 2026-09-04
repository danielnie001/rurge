# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 项目是什么

rurge 是用 Rust 复刻 Surge（macOS / iOS 网络代理工具）全部功能的跨平台核心，原生兼容 Surge 的 `.conf` / `.sgmodule` 配置格式、JavaScript 脚本 API 与 HTTP API。目标平台 Windows / Linux / macOS；产品形态是单二进制守护进程 `rurge` + CLI + Surge 兼容 HTTP API，Web Dashboard 与桌面 GUI 放在后期阶段。

## 当前状态（2026-09）

阶段 1 进行中。M1、M2a 已完成：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）。M2b（DNS）未开始；M3（连接流水线）、M4（控制面与平台）未开始，`rurge run` 尚不存在。

## 先读这些文档

- `docs/requirements.md`：需求文档（PRD）。定位与非目标（第 2 节）、架构与连接流水线（第 3 节）、按模块编号的功能需求 `FR-<模块>-<序号>`（带优先级 P0/P1/P2 与所属阶段，第 4 节）、非功能需求、平台差异矩阵、8 个阶段的路线图与验收标准（第 7 节）、风险与开放问题。
- `docs/surge-compatibility-matrix.md`：逐项兼容清单（约 530 条）。每个 Surge 配置键、规则类型、策略参数、脚本 API、HTTP API 端点、CLI 命令在 rurge 中的计划状态（✅ 完全支持 / 🟡 有差异 / 🔁 解析并忽略 / ⛔ 不支持 / ❓ 待评估）与实现阶段。实现任何 Surge 特性前先查这里；与 Surge 的任何行为差异必须登记在这里。
- `docs/superpowers/specs/2026-09-03-phase1-core-skeleton-design.md`：阶段 1 设计文档。crate 边界与依赖方向、配置数据模型、规则引擎索引、DNS 流程、连接流水线、REJECT 行为、阶段 1 的 API / CLI、里程碑 M1 ～ M4。
- `docs/superpowers/specs/2026-09-04-phase1-m2-rules-dns-design.md`：M2 设计文档。`rurge-net`（连接器 / HTTP 客户端 / 外部资源管理器）、`rurge-rules`（索引结构、规则集、匹配器、GeoIP / ASN、规则引擎）、`rurge-dns`（M2b）的接口与语义；第 15 节列出需登记进兼容性清单的行为差异。
- `docs/superpowers/plans/2026-09-03-phase1-m1-config-parser.md`：M1 实施计划（14 个任务，含完整代码与测试）。执行时按任务顺序推进，每个任务结束跑 `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace`。
- `docs/superpowers/plans/2026-09-04-phase1-m2a-rules-plan.md`：M2a 实施计划（16 个任务）。末尾「执行期修正记录」表记录了与计划的偏差及基准数字。
- `README.md`：中英双语，对外的状态、特性表与路线图，必须与 PRD 保持一致。

## 工作流约定

- 每个阶段：先写设计文档（`docs/superpowers/specs/YYYY-MM-DD-<topic>-design.md`），再写实施计划（`docs/superpowers/plans/`），再实现。新工作必须能对应到 PRD 第 7 节的某个阶段和第 4 节的 FR 编号。
- 兼容性原则：Surge 语法就是 rurge 语法。不认识或平台不适用的配置项要"解析并忽略 + 记录日志"，不能报错；rurge 不改写用户的配置文件；rurge 专有的运行时选项只通过命令行参数和环境变量提供，不扩展 Surge 配置格式（FR-CFG-17）。
- 事实来源：Surge 官方手册 <https://manual.nssurge.com/>（本项目基于 2026-09 版本，对应 Surge Mac 6.9 / iOS 5.22）。手册改版频繁，有疑问查手册，不凭记忆。已知的常见误区：`doh-server` 已改名 `encrypted-dns-server`；URL Rewrite 只有 `header` / `302` / `307` / `reject` 四种模式；`[MITM]` 没有 `tcp-connection` 键；url-test 组的测试 URL来自策略的 `test-url` 或全局 `proxy-test-url`。
- 语言：文档与对话用中文；README 中英双语；日志与 CLI 输出以英文为主（NFR-10）。
- 提交：用户自行决定何时 commit；未被要求时不要提交。

## 计划中的架构（阶段 1 建立后生效）

- Rust stable，Cargo workspace，按职责拆 crate：`rurge-config`（解析 / 校验 / include / 模块叠加 / 托管配置）、`rurge-rules`、`rurge-dns`（客户端 / 加密 DNS / `[Host]` / fake-IP）、`rurge-policy`（策略组 / 测试 / 订阅）、`rurge-proto`（出站协议）、`rurge-net`（内部 HTTP 客户端 / 外部资源管理）、`rurge-inbound`、`rurge-engine`（会话流水线 / 请求记录 / 运行时状态）、`rurge-tun`（虚拟网卡 / 协议栈 / 网关 / DHCP）、`rurge-http`（HTTP 引擎 / MITM / 重写 / 抓包）、`rurge-script`、`rurge-api`、`rurge-platform`、`rurge`（bin）。职责与依赖见 PRD 3.2；平台特定代码只允许出现在 `rurge-platform` 与 `rurge-tun`（AR-02）。
- 依赖方向（M2 设计文档确认，`rurge-dns` 尚未实现）：`rurge-dns → rurge-rules → rurge-net → rurge-config`；`[Host]` 集合键与 `LazyResolver` 都由 `rurge-dns` 依赖 `rurge-rules` 提供，而非并列关系。
- 连接处理流水线（PRD 3.3）：入站 → 协议嗅探（SNI / Host / QUIC / STUN）→ 预匹配 → 出站模式判断 → 规则匹配（域名规则不触发 DNS，IP 规则按需解析）→ 策略解析（组 / 链式 / 别名）→ 出站建立 → HTTP 引擎（MITM → Header Rewrite → URL Rewrite → Body Rewrite → 脚本 → Map Local 短路）→ 观测。
- 配置对象不可变，重载时原子切换（AR-04）；每个连接是独立 tokio 任务（AR-03）。

## 常用命令

```bash
cargo test --workspace                          # 全部测试
cargo test -p rurge-config <name>               # 单个 crate / 用例
cargo insta test -p rurge-config --review       # 语料库快照有变化时审阅
cargo clippy --all-targets -- -D warnings       # 零警告
cargo fmt --all
cargo run -p rurge -- check -c config.conf      # 校验 Surge 配置（--json / --strict / --platform）
cargo run -p rurge -- rule match -c config.conf example.com --explain   # 离线测试规则匹配
cargo bench -p rurge-rules                      # criterion 基准（域名 / IP 索引、规则引擎）
```

`Cargo.lock` 需要提交（`.gitignore` 已注明）。
