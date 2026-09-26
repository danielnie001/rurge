# 阶段 2 / M3c「smart 组」Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 `smart` 组按真实连接的质量自动选成员，并在选中的成员连不上时换成员：`SmartBook` 按策略记首字节耗时的时间加权分数与失败罚分、健康 / 未知 / 失败三种状态、站点记忆与使用计数；注册表为 `smart` 组排序，给出选中的成员与重试列表；引擎拨号时按重试列表换成员，每条会话回报它看到的首字节、3 秒无响应或上游先断；测速固定 5 分钟一轮、成员多时抽样；请求记录增加 `connectMs` / `firstByteMs`；能力表翻转 `smart`。先处理 M3b 留下的每次拨号开销（#14）。

**Architecture:** `rurge-inbound` 的 `SessionHandle` 记"出站就绪"与"首字节"两个时刻，结束钩子改为可挂多个，另有首字节钩子与"上游先断"的标记——它仍不认识 `smart`。`rurge-policy` 新模块 `smart`：`SmartBook`（按策略的记录、站点记忆、使用计数）与纯函数 `rank`（排序、优选集、重试列表）/ `sample`（大组抽样）；`AutoGroups` 持有 `SmartBook`，并让 `TestBook` 把保存下来的结果推送给它。注册表为 `smart` 组只留代理成员、按 `policy-priority` 预算因子，`choose` 的 `smart` 分支给出选中者与重试列表（`Resolution.smart`），`test_round` 做常规轮（大组抽样）。`rurge-engine` 新模块 `smart`：`attempts` 把重试列表展开成带链的尝试序列，`watch` 把质量探针挂到会话上；拨号循环按重试列表换成员（单次时限"剩余时间 ÷ 剩余尝试次数"）；`pump` 与明文 HTTP 转发标记首字节与"上游先断"。

**Tech Stack:** Rust 1.89 / edition 2024；只用工作区已有的依赖（`tokio`、`tracing`，`policy-priority` 的正则经 `rurge_config::rule::Pattern` 用 `fancy-regex`），**不新增任何第三方 crate，`Cargo.lock` 不变**。

**Spec:** `docs/superpowers/specs/2026-09-26-phase2-m3c-smart-design.md`（第 2 节 M3c-D1 ～ D10、第 4 ～ 9 节、第 11 节、第 15 节 V1–V11、第 16 节草图）；M3 设计 `docs/superpowers/specs/2026-09-23-phase2-m3-groups-subscriptions-design.md` 的 M3-D11 / D13；M3b 计划 `docs/superpowers/plans/2026-09-25-phase2-m3b-testing-auto-groups-plan.md` 末尾「延后事项」#6、#14。与本计划「计划期决定」表不一致处，以该表为准，并由 Task 8 写回设计文档新增的第 17 节。

## Global Constraints

- MSRV **1.89**，edition 2024。`unsafe_code`：全工作区 `forbid`；只有 `rurge-platform` 是 `deny` + 唯一一个 `#[allow(unsafe_code)]` 函数。**本计划不新增任何 unsafe。**
- 依赖方向不变：`rurge-policy` 不依赖 `rurge-engine`，也不依赖任何具体协议实现；`rurge-inbound` 不依赖 `rurge-policy`（`SessionHandle` 只提供通用的钩子与标记，`smart` 的回报由引擎挂上去）；平台代码只在 `rurge-platform`（AR-02）。**不新增任何第三方依赖**，`Cargo.lock` 不变，不下载任何东西。
- **测试绝不碰公网**：只用回环 + 端口 0 + 有界等待；不用固定 sleep 当同步手段（轮询 + 截止时间）。**任何带 `url-test` / `fallback` / `load-balance` / `smart` 组的测试配置，`proxy-test-url` 与 `internet-test-url` 都必须指向回环**——引擎用例的 `Profile::text` 与 API 用例的夹具已默认指向 `http://127.0.0.1:9/`（M3b），不要删掉；注册表单元用例自己写 `[General]` 两行。"黑洞"上游只能用回环：接受 TCP 却不回代理握手（`HttpProxyScript { delay, .. }`），绝不用不可路由的地址。
- **测试与任何命令都不得修改本机的系统代理、注册表、网络设置，不得注册真实服务 / 计划任务**：不在 CLI 测试夹具之外运行 `rurge run --system-proxy`；不运行不带 `--dry-run` 的 `rurge service install | uninstall`。
- **不在本机下载或安装任何东西**（不装 sing-box、不装 xray、不 `rustup target add`、不 `cargo install`，也不装 `cargo-insta`）。
- **凭据及其派生物永不外泄**（M3-D7）：`smart` 的回报、站点记忆、会话备注与日志里只有组名、策略名与目标主机名——没有 URL、没有凭据；测试 URL 照 M3b 的规矩（永不进日志、错误文本与请求记录）。
- rurge 专有的运行时选项只经命令行与环境变量提供，不扩展 Surge 配置格式（FR-CFG-17）。与 Surge 的每一处行为差异都登记进 `docs/surge-compatibility-matrix.md`。`smart` 的常数集中在 `rurge_policy::smart` 与 `rurge_engine::smart` 的常量里（M3-D13）。
- 日志、错误文本、CLI 输出、代码注释用英文；文档与提交标题用中文。注释密度、命名、惯用法与相邻代码保持一致；注释里不写评审轮次的标签。
- 提交信息结尾两行：`Co-Authored-By: <执行者自己的署名>` 与 `Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th`。**不 push、不 merge**，不 amend。
- 每个任务结束的门禁（全部通过才算完成）：

  ```bash
  RUSTFMT="C:\Users\SZV01065\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe" cargo fmt --all --check \
    && cargo clippy --all-targets -- -D warnings \
    && timeout 1500 cargo test --workspace --no-fail-fast
  ```

  `timeout` 不能省：`rurge-dns` 的一个用例曾让测试进程以 100% CPU 空转数小时（M3a「延后事项」#20）。测试二进制异常退出而没有失败用例时（`STATUS_ACCESS_VIOLATION`、`STATUS_HEAP_CORRUPTION` / `0xc0000374`、段错误——本机已知的既有问题，M3b 计划 P21），或整轮被 `timeout` 杀掉时，重跑一次并保留两次的日志，**不要在任务里去修它**。已知偶发失败的计时类用例（`rurge-dns` 的 `a_partial_result_completes_aaaa_in_the_background`、`rurge` 的 `run::watch_reloads_rules_on_change`）同样重跑。
- 第三方 crate 的 API 以本机源码为准：`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`。
- 本机的 bash 处理不了超过约 8 KB 或引号复杂的 heredoc：新文件一律用写文件的工具落盘，不用 heredoc。
- Windows 上连接回环里一个没人监听的端口，要一两秒才报"拒绝连接"：用到它的端到端用例因此各要几秒，这是正常的。

## Review Focus

设计没有逐条写到、而最可能伤到使用者的五类输入或失败方式；每一条都在负责它的任务里配了用例。

1. **连上之后很久才有第一个字节的会话**（长轮询、推送、服务端先不说话的协议）：按手册，出站就绪 3 秒内没有第一个字节就计一次失败（罚分），但连接照常，不换成员、不打断。用例：Task 5 `three_seconds_without_an_answer_count_against_the_member`（会话仍在活动表里）、`three_silent_seconds_are_a_failure`。
2. **客户端自己先断**（浏览器取消请求、页面跳走）不能算成员的错，`kill`、空闲超时、优雅退出也不算；上游在回过数据之后才断也不算。用例：Task 5 `the_client_leaving_first_or_an_answer_is_no_upstream_failure`、`upstream_ending_first_is_a_failure_and_a_kill_is_not`。
3. **节点被墙**（握手挂住直到超时）是最常见的失败：换成员的重试不能被第一次超时吃光；全部连不上时 10 秒内失败，并在请求记录里点名试过的成员。用例：Task 5 `a_member_that_never_answers_has_its_share_of_the_time`、`when_no_member_connects_the_session_fails_naming_them`。
4. **订阅节点很多**（几十上百个）：拨号只查表与做算术，常规轮只测 12 个，手动测才测全部。用例：Task 2 `the_outcome_is_read_for_the_current_definition`；Task 6 `a_regular_round_of_a_big_smart_group_tests_a_sample`、`a_dial_asks_for_a_sampled_round_of_a_big_smart_group`。
5. **订阅更新或重载删掉、改掉了节点**：删掉的策略不留分数、站点记忆与使用计数；改了定义的从头积累，不沿用旧服务器的数据。用例：Task 3 `another_outbound_under_the_same_name_starts_over`、`retain_forgets_the_policies_that_are_gone`；Task 6 `a_reload_forgets_what_the_book_knew_of_the_policies_it_drops`。

## 计划期决定

写计划时对照设计、Surge 手册（`policy-groups/smart.html`，2026-09-26 读取）与本仓库源码核对后定下的事；与设计文档文字不同的，由 Task 8 写回设计文档第 17 节。

**本计划里的代码不是凭空写的。** 全部 8 个任务的改动在仓库的一份副本上按任务顺序真实做了一遍（副本用自己的构建目录，不与本仓库的 `target/` 混用），每个任务之后跑一次全工作区门禁：最后一次是 **41 个测试二进制，986 通过 / 0 失败 / 1 忽略**（本计划开工前的 main 是 40 个二进制、931 通过）。计划里新文件的全文取自副本上该任务的提交，修改处的"把 … 换成 …"由脚本从相邻两个任务提交的差异生成，并在拼好之后按计划的顺序套到开工前的源码上逐字核对过——计划文本与验证过的代码一字不差。每个任务 Step 2 的"预期失败"是只把该任务的用例块套到上一个任务的状态上、真实跑出来的。

| # | 事项 | 决定与依据 |
| - | ---- | ---------- |
| P1 | 设计 4.2：`rurge-policy` 定义 `SessionReporter` trait，`SmartBook` 实现它 | 不另设 trait：引擎直接调用 `SmartBook::report_success` / `report_failure`。只有一个实现，trait 什么也不解耦；依赖方向不变（引擎 → `rurge-policy`） |
| P2 | 设计 5.1 / V4："同一个定义"的标识；只改测试 URL 或超时不清零 | 按**出站对象**判断：M2b 在指纹不变时沿用同一个出站对象（同一个 `Arc`），记录存它的 `Weak`（持有 `Weak` 使那块内存不会被别的出站复用，地址比较因此可靠）。M2b 的指纹含 `test-url` / `test-timeout`，所以改它们也会清零——与设计"不清零"不同。另做一套不含测试参数的标识要改注册表的复用逻辑；改测试参数少见，清零后几分钟内就会重新积累 |
| P3 | 设计 4.3 / M3c-D4：DNS 会话也回报 | DNS 会话只回报拨号失败、首字节与 3 秒无响应。"上游先断"在 `pump` 与明文 HTTP 转发（`forward`）里判断，DNS 会话的流（`wrap_internal`）不经过这两处；它的首字节照样由 `Counting` 记下（V1） |
| P4 | 设计 §10：成员在"健康 ⇄ 失败"之间切换时记 INFO（组名、成员名） | 只在会话回报（`report_success` / `report_failure`）引起切换时记，只写策略名（`smart: the policy counts as failed` / `smart: the policy works again`）。`SmartBook` 按策略记、不知道组；测速推送覆盖所有被测的策略（多数不是 `smart` 成员），由它触发会刷屏 |
| P5 | 设计 6.1：被忽略的成员 | 每一代构建注册表时，`smart` 组的成员表就只剩代理（`Entry::Outbound { proxy: true }` 与未实现的协议）；视图、`select`、`test_results`、测速都用这张表。被忽略的名字每次构建记一行 INFO。过滤后为空 → 空组兜底 |
| P6 | 设计 §7 / V7：单次时限 | 有两个以上候选时，每次尝试另由引擎用 `tokio::time::timeout` 计时（同时把 `ConnectOpts.timeout` 设为同一个值）：个别出站若不严格遵守 `ConnectOpts.timeout`，也不会超出它的那一份；只有一个候选时保持 M1 起的原样（只靠 `ConnectOpts.timeout = 10 秒`） |
| P7 | 设计 6.2：重试列表 | 引擎展开重试列表时跳过解析结果不是代理的成员（尚未实现的协议）：试它只会把一次连接失败变成 REJECT |
| P8 | 设计 §7："连接层面的错误"才重试 | 除 REJECT 与"协议尚未实现"之外的连接错误都重试（I/O、超时、代理握手、TLS、代理服务器名在本机解析失败、不可用）；每个失败的成员都回报（并记进站点记忆） |
| P9 | 设计 §7：会话记录 | 换过成员并成功：备注 ``smart group `G`: `A`, `B` failed to connect, used `C` ``；全部失败：备注 ``smart group `G`: tried `A`, `B`, `C` ``，M1 的 `fail` 再接上 `; <最后一次的错误>`；只试了一个（没有可换的）时不写备注，与之前一样。策略链的末项是实际用上 / 最后试的成员 |
| P10 | 设计 8.1 / V6：常规轮与手动轮 | 新增 `PolicyRegistry::test_round(group)`：拨号请求的常规轮，成员超过 `ROUND_SAMPLE = 12` 的 `smart` 组只测 `sample` 选出的 12 个；`test_group` 仍测全部（API 用它）。引擎的调度任务改调 `test_round`；`round_timeout` 按常规轮实际要测的成员算（`evaluate-before-use` 的等待上限随之） |
| P11 | 设计 8.1："有成员处于未知状态"时也请求一轮 | "未知"是 `SmartBook` 的状态：有真实会话样本的成员不算未测。M3b 按"有没有测速结果"判断，对 `smart` 不合适 |
| P12 | 设计 8.3：视图里的"当前选择" | `current_member`（`live = false`）：最近 10 分钟用得最多的成员；最近没用过时是 `rank` 在"没有站点记忆、随机数固定取 0"下选出的成员（优选集里分数最低的）。读取不回报、不计数、不触发测速 |
| P13 | 设计 8.2：使用计数 | 在连接成功（`mark_connected`）时记一次，会话拨号与 DNS 会话都记；按分钟分桶、只留 10 分钟 |
| P14 | 设计 6.3：站点记忆 | 按"主机 + 策略"只存最近一次的结果与时间，新结果覆盖旧的；写入一个站点时顺手清掉它超过 1 小时的条目；读取一个站点也算"使用"（LRU）。`retain` 同时清掉消失策略的站点条目与消失组的使用计数 |
| P15 | 设计 5.1：记录的清理 | 每次发布注册表时（重载的 `publish_generation`、订阅重建的发布）调用 `SmartBook::retain(|name| registry.contains(name))`；只在内存里操作，可以在代际锁内做 |
| P16 | 设计 6.4 / V8：`Resolution.smart` | `SmartPick { group, member, retry }`，由 `named()` 在 `smart` 组做出选择时填上；覆盖期间不填（没有重试列表、不回报）。链的中间跳（`resolve_relay`）也会得到它，但 `ChainConnector` 不看（不重试、不挂探针，M3c-D5）；`resolve_member(name)` 单独解析一个成员给引擎重试用 |
| P17 | V1：首字节的记点 | `pump` 下行方向的首块钩子（读到时，写给客户端之前）；明文 HTTP 转发与 DNS 会话经 `Counting::poll_read`（hyper 经它读响应头）。都是"只记一次"的 `OnceLock`，每次读只多一次原子读 |
| P18 | V2 / V3：`SessionHandle` 的钩子与"上游先断" | `on_finish` 改为追加（按添加顺序全部运行，引擎的会话日志先装）；新增 `on_first_byte`（首字节已到时立即运行）、`first_byte_seen`、`mark_upstream_failed` / `upstream_failed`。`pump` 给 `copy_half` 加一个"读端结束"（EOF 或读错误，不含 `stop`）的回调：客户端方向结束时记下"客户端已走"，上游方向结束时若客户端还在、又没收到首字节，就标记"上游先断"。`forward` 在请求发出后拿不到响应头时标记 |
| P19 | 既有用例 `only_a_member_can_be_selected`（M1b） | 它拿 `smart` 当"不接受选择"的组；M3c 起改用 `subnet`（阶段 3 之前仍不接受） |
| P20 | M3a 的组环检测 | 仍把 `smart` 组写进去的嵌套组当作边：加载时 `W0030`、运行期 REJECT，虽然 `smart` 实际忽略它。这种写法少见，改组环检测要动 `rurge-config` 与 `assemble` 两处；记入「延后事项」 |
| P21 | 任务的切分 | 与设计第 16 节草图相同的 8 个任务；三条验证 Task 5 行为的端到端用例（黑洞成员的单次时限、3 秒无响应、明文请求的上游先断）放在 Task 5 里先写（TDD），Task 7 只翻转能力表 |

## 承接事项

之前计划「延后事项」表里标给 M3c 的条目。

| # | 来源 | 事项 | 处理 | 任务 |
| - | ---- | ---- | ---- | ---- |
| C1 | M3b #6 | `smart` 组仍 `W0008`、取第一个成员、不在 `test_results` 里、`select` 不接受；请求记录的两列耗时 | 两列耗时（1）；选择（4）；`automatic()` 纳入 `smart`（6）；能力表（7） | 1、4、6、7 |
| C2 | M3b #14 | 自动组每次拨号都为每个成员构造 `TestCase` | 见 Task 2（`TestBook::outcome` 与 `test_slot`） | 2 |
| C3 | M3b #21 | `CommonOpts` 的 `Debug` 打印 `test_url` | 不在本计划：`smart` 不打印 `PolicySpec`，仍是潜在问题，留在原处（单独小改动） | — |
| C4 | M3b #7（P21） | 测试二进制偶发崩溃 | 照旧：门禁遇到就重跑 | — |

## File Structure

新建：

| 文件 | 职责 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-policy/src/smart.rs` | `SmartBook`（记录、分数、状态；站点记忆；使用计数；回报）、`Health`、`rank` / `Candidate` / `Ranking` / `SiteMemory`、`sample`，与用例 | 3、4、6 |
| `crates/rurge-engine/src/smart.rs` | 引擎一侧：`attempts`、`retryable`、`quoted`、`watch`（质量探针），与用例 | 5 |
| `crates/rurge-engine/tests/smart.rs` | `smart` 组经整个引擎的用例 | 5、6 |

修改：

| 文件 | 改动 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-inbound/src/session.rs` | 两个时刻、`Counting` 记首字节（1）；多个结束钩子、首字节钩子、"上游先断"（5） | 1、5 |
| `crates/rurge-inbound/src/http.rs` | 转发拿不到响应头时标记"上游先断" | 5 |
| `crates/rurge-engine/src/{relay.rs, dns_pipeline.rs, engine.rs, observe.rs}` | 首字节、出站就绪、请求记录两列（1）；"上游先断"、拨号循环与 DNS 会话的回报（5）；发布时清理（6） | 1、5、6 |
| `crates/rurge-engine/src/{lib.rs, auto.rs, subscriptions.rs}` | `mod smart`（5）；`automatic()` 纳入 `smart`、调度改调 `test_round`、订阅重建时清理（6） | 5、6 |
| `crates/rurge-engine/tests/{pipeline.rs, outbounds.rs}` | 请求记录两列（1）；不接受选择的组改用 `subnet`（6） | 1、6 |
| `crates/rurge-api/src/routes/requests.rs` | 请求 JSON 两个字段与用例 | 1 |
| `crates/rurge-policy/src/{testbook.rs, registry.rs}` | `TestBook::outcome` 与 `test_slot`（2）；`TestSink` 推送（3）；`smart` 成员与因子、`choose` 的 `smart` 分支、`Resolution.smart`、`resolve_member`、`available`（4）；`test_round` / `sample_of`（6） | 2、3、4、6 |
| `crates/rurge-policy/src/{auto.rs, lib.rs}` | `AutoGroups.smart`（3）；`pub mod smart`、导出 `SmartPick`（3、4） | 3、4 |
| `crates/rurge/src/capabilities.rs`、`crates/rurge/tests/cli.rs` | 能力表翻转与用例 | 7 |
| 文档（清单、两份 API 参考、手工验收、M3c 设计第 17 节、两份 README、CLAUDE.md） | 见 Task 8 | 8 |

## 任务一览

| 任务 | 交付物 | 依赖 |
| ---- | ------ | ---- |
| 1 | 请求记录两列耗时：`SessionHandle` 的两个时刻、首字节的两个记点、`RequestRecord` 与 API | — |
| 2 | 拨号读成员的测试结果不再构造 `TestCase`（M3b #14） | — |
| 3 | `SmartBook` 的打分与状态；`TestBook` 推送测速结果 | 2 |
| 4 | `smart` 组的选择：排序与重试列表、站点记忆、使用计数；接入注册表 | 3 |
| 5 | 引擎：拨号重试与质量回报 | 1、4 |
| 6 | 测速节奏与抽样；`automatic()` 纳入 `smart`；发布注册表时清理 | 5 |
| 7 | 能力表翻转 `smart` | 6 |
| 8 | 文档 | 7 |

---

### Task 1: 请求记录两列耗时（承接 C1 的一半）

所有会话的请求记录都多两个字段（设计 4.1）：`connectMs`——会话开始到出站就绪（含规则匹配、DNS、`evaluate-before-use` 的等待；Task 5 起还含换成员的重试）；`firstByteMs`——出站就绪到第一个上游字节被**读到**（不是写给客户端之后）。没有对应时刻（被拒绝、拨号失败、还没收到数据）时为 `null`。`smart` 的打分在 Task 5 用的就是 `firstByteMs`。

**Files:**
- Modify: `crates/rurge-inbound/src/session.rs`（`SessionHandle` 两个 `OnceLock` 时刻与四个方法；`Counting::poll_read` 记首字节）
- Modify: `crates/rurge-engine/src/relay.rs`（`pump` 下行方向的首块钩子）
- Modify: `crates/rurge-engine/src/engine.rs`（会话拨号成功时 `mark_connected`）、`crates/rurge-engine/src/dns_pipeline.rs`（`wrap_internal` 里 `mark_connected`：DNS 会话连上的所有路径都经过它，含两种旁路）
- Modify: `crates/rurge-engine/src/observe.rs`（`RequestRecord` 两个字段）
- Modify: `crates/rurge-api/src/routes/requests.rs`（`RequestJson` 的 `connectMs` / `firstByteMs`）
- Test: 上述文件的单元用例；`crates/rurge-engine/tests/pipeline.rs`

**Interfaces:**
- Produces:
  - `SessionHandle::mark_connected(&self)`、`SessionHandle::mark_first_byte(&self)`（都只认第一次）
  - `SessionHandle::connect_time(&self) -> Option<Duration>`（会话开始 → 出站就绪）、`SessionHandle::first_byte_time(&self) -> Option<Duration>`（出站就绪 → 首字节；出站还没就绪时为 `None`）
  - `RequestRecord { .., connect_ms: Option<u64>, first_byte_ms: Option<u64>, .. }`（毫秒），API JSON 的 `connectMs` / `firstByteMs`

- [ ] **Step 1: 先写用例**

`crates/rurge-inbound/src/session.rs`——把

```rust
        assert_eq!(h.bytes(), (5, 2));
    }

    #[test]
    fn kill_cancels_the_token_and_marks_the_handle() {
```

换成

```rust
        assert_eq!(h.bytes(), (5, 2));
    }

    /// The two moments of the request log (M3c design 4.1): each is set by
    /// its first mark only, and the first byte is measured from the moment
    /// the outbound was ready.
    #[test]
    fn the_outbound_and_its_first_byte_are_marked_once() {
        let h = SessionHandle::new(3, SessionInfo::tcp(HostName::parse("a.test"), 443));
        assert_eq!((h.connect_time(), h.first_byte_time()), (None, None));
        h.mark_connected();
        let connected = h.connect_time().expect("marked");
        assert_eq!(h.first_byte_time(), None, "nothing came back yet");
        h.mark_first_byte();
        let first = h.first_byte_time().expect("marked");
        std::thread::sleep(Duration::from_millis(5));
        h.mark_connected();
        h.mark_first_byte();
        assert_eq!(h.connect_time(), Some(connected), "the first mark stands");
        assert_eq!(h.first_byte_time(), Some(first), "the first mark stands");
    }

    /// Bytes read from upstream through `Counting` mark the first byte; bytes
    /// written to it, and the end of the stream, do not.
    #[tokio::test]
    async fn counting_marks_the_first_byte_read_from_upstream() {
        let (mut a, b) = tokio::io::duplex(64);
        let h = SessionHandle::new(2, SessionInfo::tcp(HostName::parse("a.test"), 80));
        h.mark_connected();
        let mut counted = Counting::new(b, h.clone());
        counted.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 5];
        a.read_exact(&mut buf).await.unwrap();
        assert_eq!(h.first_byte_time(), None, "a write is not a response");
        a.write_all(b"ok").await.unwrap();
        let mut buf2 = [0u8; 2];
        counted.read_exact(&mut buf2).await.unwrap();
        assert!(h.first_byte_time().is_some());
    }

    #[tokio::test]
    async fn the_end_of_the_stream_is_no_first_byte() {
        let (a, b) = tokio::io::duplex(64);
        let h = SessionHandle::new(4, SessionInfo::tcp(HostName::parse("a.test"), 80));
        h.mark_connected();
        let mut counted = Counting::new(b, h.clone());
        drop(a);
        let mut buf = Vec::new();
        counted.read_to_end(&mut buf).await.unwrap();
        assert_eq!(h.first_byte_time(), None);
    }

    #[test]
    fn kill_cancels_the_token_and_marks_the_handle() {
```

`crates/rurge-engine/src/relay.rs`——把

```rust
        assert_eq!(h.bytes(), (4, 2));
        assert_eq!(h.outcome(), Some(SessionOutcome::Completed));
    }
```

换成

```rust
        assert_eq!(h.bytes(), (4, 2));
        assert_eq!(h.outcome(), Some(SessionOutcome::Completed));
    }

    /// The first byte is what upstream sends, not what the client does (M3c
    /// design 4.1).
    #[tokio::test]
    async fn the_first_byte_is_the_first_one_upstream_sends() {
        let (mut ca, client_b) = tokio::io::duplex(1024);
        let (mut ua, upstream_b) = tokio::io::duplex(1024);
        let h = handle();
        h.mark_connected();
        let task = tokio::spawn(pump(
            Box::new(client_b),
            Box::new(upstream_b),
            h.clone(),
            Duration::from_secs(30),
        ));
        ca.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        ua.read_exact(&mut buf).await.unwrap();
        assert_eq!(
            h.first_byte_time(),
            None,
            "the client's bytes are no response"
        );
        ua.write_all(b"po").await.unwrap();
        let mut buf2 = [0u8; 2];
        ca.read_exact(&mut buf2).await.unwrap();
        assert!(h.first_byte_time().is_some());
        drop(ca);
        drop(ua);
        task.await.unwrap();
    }
```

`crates/rurge-engine/tests/pipeline.rs`——把

```rust
    assert!(h.engine.traffic().totals().down > 0);
```

换成

```rust
    assert!(h.engine.traffic().totals().down > 0);
}

/// Both ways a session reaches its target carry the two timings (M3c design
/// 4.1) — a plain request the HTTP listener forwards, and a CONNECT tunnel —
/// and a rejected session has neither.
#[tokio::test]
async fn the_request_log_times_the_outbound_and_the_first_byte() {
    let h = harness("", "DOMAIN,ads.test,REJECT", OutboundMode::Rule).await;
    let port = h.target_port();
    let (head, _) = get_via_proxy(h.http(), &format!("http://target.test:{port}/hello")).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let plain = wait_for_record(&h.engine, Duration::from_secs(3), |r| {
        r.dst.starts_with("target.test:")
            && matches!(r.status, rurge_engine::RecordStatus::Completed)
    })
    .await
    .expect("the plain request's record");
    assert!(
        plain.connect_ms.is_some() && plain.first_byte_ms.is_some(),
        "{plain:?}"
    );

    let mut s = TcpStream::connect(h.http()).await.unwrap();
    let (head, _) = http_exchange(
        &mut s,
        &format!("CONNECT target.test:{port} HTTP/1.1\r\nHost: target.test:{port}\r\n\r\n"),
    )
    .await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    s.write_all(b"GET /hello HTTP/1.1\r\nHost: target.test\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(3), s.read_to_end(&mut out)).await;
    drop(s);
    let tunnel = wait_for_record(&h.engine, Duration::from_secs(3), |r| {
        r.id != plain.id
            && r.dst.starts_with("target.test:")
            && matches!(r.status, rurge_engine::RecordStatus::Completed)
    })
    .await
    .expect("the tunnel's record");
    assert!(
        tunnel.connect_ms.is_some() && tunnel.first_byte_ms.is_some(),
        "{tunnel:?}"
    );

    let (head, _) = get_via_proxy(h.http(), "http://ads.test/").await;
    assert!(head.is_empty(), "{head}");
    let rejected = wait_for_record(&h.engine, Duration::from_secs(3), |r| {
        r.dst.starts_with("ads.test:")
    })
    .await
    .expect("the rejected session's record");
    assert_eq!(
        (rejected.connect_ms, rejected.first_byte_ms),
        (None, None),
        "{rejected:?}"
    );
```

`crates/rurge-api/src/routes/requests.rs`——把

```rust
            elapsed_ms: 0,
```

换成

```rust
            elapsed_ms: 0,
            connect_ms: None,
            first_byte_ms: None,
```

`crates/rurge-api/src/routes/requests.rs`——把

```rust
        assert_eq!(j.protocol, Some("mtproto"));
    }
}
```

换成

```rust
        assert_eq!(j.protocol, Some("mtproto"));
    }

    /// The two timing fields (M3c design 4.1): `null` until the moment has
    /// come, milliseconds after.
    #[test]
    fn the_timings_are_null_until_known() {
        let mut r = record(1, ListenerKind::Http);
        let v = serde_json::to_value(RequestJson::from(&r)).unwrap();
        assert_eq!(v["connectMs"], Value::Null);
        assert_eq!(v["firstByteMs"], Value::Null);
        r.connect_ms = Some(12);
        r.first_byte_ms = Some(34);
        let v = serde_json::to_value(RequestJson::from(&r)).unwrap();
        assert_eq!(v["connectMs"], 12);
        assert_eq!(v["firstByteMs"], 34);
    }
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-inbound session`
Expected: FAIL，编译错误——

```text
error[E0599]: no method named `mark_connected` found for struct `std::sync::Arc<session::SessionHandle>` in the current scope
error[E0599]: no method named `first_byte_time` found for struct `std::sync::Arc<session::SessionHandle>` in the current scope
error[E0599]: no method named `mark_first_byte` found for struct `std::sync::Arc<session::SessionHandle>` in the current scope
error[E0599]: no method named `connect_time` found for struct `std::sync::Arc<session::SessionHandle>` in the current scope
error: could not compile `rurge-inbound` (lib test) due to 16 previous errors
```

- [ ] **Step 3: 实现**

`crates/rurge-inbound/src/session.rs`——把

```rust
use std::sync::{Arc, Mutex};
```

换成

```rust
use std::sync::{Arc, Mutex, OnceLock};
```

`crates/rurge-inbound/src/session.rs`——把

```rust
    down: AtomicU64,
```

换成

```rust
    down: AtomicU64,
    /// Since `started`: when the outbound was ready (`mark_connected`).
    connected: OnceLock<Duration>,
    /// Since `started`: when the first byte came back from upstream.
    first_byte: OnceLock<Duration>,
```

`crates/rurge-inbound/src/session.rs`——把

```rust
            down: AtomicU64::new(0),
```

换成

```rust
            down: AtomicU64::new(0),
            connected: OnceLock::new(),
            first_byte: OnceLock::new(),
```

`crates/rurge-inbound/src/session.rs`——把

```rust
            self.down.load(Ordering::Relaxed),
        )
    }

    /// Installs the hook `finish` runs once (the engine's session log).
```

换成

```rust
            self.down.load(Ordering::Relaxed),
        )
    }

    /// The outbound connection is ready. Only the first call counts.
    pub fn mark_connected(&self) {
        let _ = self.connected.set(self.started.elapsed());
    }

    /// The first byte came back from upstream — as it is read, before it
    /// goes on to the client. Only the first call counts.
    pub fn mark_first_byte(&self) {
        let _ = self.first_byte.set(self.started.elapsed());
    }

    /// From the session's start until its outbound was ready (rule
    /// matching, name resolution and any wait included).
    pub fn connect_time(&self) -> Option<Duration> {
        self.connected.get().copied()
    }

    /// From the outbound being ready until the first byte came back.
    pub fn first_byte_time(&self) -> Option<Duration> {
        let connected = self.connected.get()?;
        let first = self.first_byte.get()?;
        Some(first.saturating_sub(*connected))
    }

    /// Installs the hook `finish` runs once (the engine's session log).
```

`crates/rurge-inbound/src/session.rs`——把

```rust
            self.handle.add_down(n as u64);
```

换成

```rust
            self.handle.add_down(n as u64);
            if n > 0 {
                self.handle.mark_first_byte();
            }
```

`crates/rurge-engine/src/relay.rs`——把

```rust
/// sniffing; see `pump`).
```

换成

```rust
/// sniffing, and the first byte from upstream; see `pump`).
```

`crates/rurge-engine/src/relay.rs`——把

```rust
        }));
```

换成

```rust
        }));
        // Upstream → client: the first chunk marks the first byte as it is
        // read, before it goes on to the client (phase 2 M3c design 4.1).
        let h_first = handle.clone();
        let first_byte: Option<FirstChunkHook> = Some(Box::new(move |_: &[u8]| {
            h_first.mark_first_byte();
        }));
```

`crates/rurge-engine/src/relay.rs`——把

```rust
                    move |n| h_down.add_down(n),
                    None,
```

换成

```rust
                    move |n| h_down.add_down(n),
                    first_byte,
```

`crates/rurge-engine/src/dns_pipeline.rs`——把

```rust
/// Wraps a dialed stream so bytes are counted (`Counting`) and the handle is
/// finished on drop (`FinishOnDrop`).
pub(crate) fn wrap_internal(stream: BoxedStream, handle: Arc<SessionHandle>) -> BoxedStream {
```

换成

```rust
/// Wraps a freshly dialed stream so bytes are counted (`Counting`) and the
/// handle is finished on drop (`FinishOnDrop`); the outbound is ready as of
/// now.
pub(crate) fn wrap_internal(stream: BoxedStream, handle: Arc<SessionHandle>) -> BoxedStream {
    handle.mark_connected();
```

`crates/rurge-engine/src/engine.rs`——把

```rust
                Ok((stream, forward)) => Ok(Dialed {
                    stream,
                    handle,
                    forward,
                }),
```

换成

```rust
                Ok((stream, forward)) => {
                    handle.mark_connected();
                    Ok(Dialed {
                        stream,
                        handle,
                        forward,
                    })
                }
```

`crates/rurge-engine/src/observe.rs`——把

```rust
    pub elapsed_ms: u64,
```

换成

```rust
    pub elapsed_ms: u64,
    /// From the session's start until its outbound was ready; `None` when
    /// it never was (rejected, failed, still dialling).
    pub connect_ms: Option<u64>,
    /// From the outbound being ready until the first byte came back from
    /// upstream; `None` until one does.
    pub first_byte_ms: Option<u64>,
```

`crates/rurge-engine/src/observe.rs`——把

```rust
        elapsed_ms,
```

换成

```rust
        elapsed_ms,
        connect_ms: h.connect_time().map(|d| d.as_millis() as u64),
        first_byte_ms: h.first_byte_time().map(|d| d.as_millis() as u64),
```

`crates/rurge-api/src/routes/requests.rs`——把

```rust
    pub elapsed_ms: u64,
```

换成

```rust
    pub elapsed_ms: u64,
    pub connect_ms: Option<u64>,
    pub first_byte_ms: Option<u64>,
```

`crates/rurge-api/src/routes/requests.rs`——把

```rust
            elapsed_ms: r.elapsed_ms,
```

换成

```rust
            elapsed_ms: r.elapsed_ms,
            connect_ms: r.connect_ms,
            first_byte_ms: r.first_byte_ms,
```

要点：
- 两个时刻都存"自会话开始的时长"（`OnceLock<Duration>`），`first_byte_time` 是两者之差；`mark_*` 用 `OnceLock::set`，第二次起什么也不做——重试时（Task 5）只有最终连上的那次调用 `mark_connected`。
- 首字节在**读到**时记：`pump` 下行方向用 `copy_half` 已有的首块钩子（上行方向的同一个钩子做 SNI 嗅探），在写给客户端之前；`Counting::poll_read` 读到非空数据时记（明文 HTTP 转发与 DNS 会话都经过它）。每次读只多一次原子读，没有每字节开销（设计 V10）。
- 被拒绝与拨号失败的会话从不调用 `mark_connected`，两列就是 `null`。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-inbound session` → 9 passed（新增 `the_outbound_and_its_first_byte_are_marked_once`、`counting_marks_the_first_byte_read_from_upstream`、`the_end_of_the_stream_is_no_first_byte`）。
Run: `cargo test -p rurge-engine --lib relay` → 10 passed; 1 ignored（新增 `the_first_byte_is_the_first_one_upstream_sends`）。
Run: `cargo test -p rurge-engine --test pipeline the_request_log` → 1 passed。
Run: `cargo test -p rurge-api requests` → 单元 3 passed（新增 `the_timings_are_null_until_known`）。

- [ ] **Step 5: 门禁与提交**

跑门禁（全工作区 40 个测试二进制，937 通过 / 1 忽略）。

```bash
git add crates/rurge-inbound/src/session.rs crates/rurge-engine/src/relay.rs crates/rurge-engine/src/engine.rs crates/rurge-engine/src/dns_pipeline.rs crates/rurge-engine/src/observe.rs crates/rurge-api/src/routes/requests.rs crates/rurge-engine/tests/pipeline.rs
git commit -m "feat(engine): 请求记录增加 connectMs / firstByteMs——会话记下出站就绪与首字节两个时刻"
```

### Task 2: 拨号读成员的测试结果不再构造 `TestCase`（承接 C2）

M3b 的 `standing` 每次拨号都为组里的每个成员调用 `test_case`，构造一个完整的 `TestCase`（克隆 URL、出站、根证书与名字）只为了读它的结果（M3b「延后事项」#14，设计 6.5）。`smart` 的打分要走同一条路径，先把它改成只查表：每一代构建时已经存好了"策略 → `TestSpec`（URL、超时、key）"，拨号直接按 `(策略, key)` 读 `TestBook` 的结果；只有真正发起测试时才构造 `TestCase`。行为不变，由现有的注册表用例守护。

**Files:**
- Modify: `crates/rurge-policy/src/testbook.rs`（`TestBook::outcome`）
- Modify: `crates/rurge-policy/src/registry.rs`（私有 `test_slot`；`standing`、`test_case`、`test_result`、`round_timeout` 改用它）
- Test: `crates/rurge-policy/src/testbook.rs`

**Interfaces:**
- Produces: `TestBook::outcome(&self, policy: &str, key: u64) -> Option<Result<Duration, ()>>`——`result` 的结论部分，不克隆原因文本。
- 注册表的公开方法签名不变（`test_case`、`test_result`、`round_timeout`）。

- [ ] **Step 1: 先写用例**

`crates/rurge-policy/src/testbook.rs`——把

```rust
            "superseded result should not exist"
        );
    }
}

```

换成

```rust
            "superseded result should not exist"
        );
    }

    /// A dial reads the outcome of a member's result for the definition it
    /// has now (M3c design 6.5).
    #[test]
    fn the_outcome_is_read_for_the_current_definition() {
        let book = TestBook::new();
        assert_eq!(book.outcome("P", 1), None);
        book.record("P", 1, Ok(Duration::from_millis(40)));
        assert_eq!(book.outcome("P", 1), Some(Ok(Duration::from_millis(40))));
        assert_eq!(book.outcome("P", 2), None, "another definition");
        book.record("P", 1, Err("timed out".to_string()));
        assert_eq!(book.outcome("P", 1), Some(Err(())));
    }
}

```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-policy testbook`
Expected: FAIL，编译错误——

```text
error[E0599]: no method named `outcome` found for struct `testbook::TestBook` in the current scope
error: could not compile `rurge-policy` (lib test) due to 4 previous errors
```

- [ ] **Step 3: 实现**

`crates/rurge-policy/src/testbook.rs`——把

```rust
            .map(|(_, r)| r.clone())
```

换成

```rust
            .map(|(_, r)| r.clone())
    }

    /// `result`'s outcome without its reason: what a dial reads of every
    /// member of a group, with nothing cloned (M3c design 6.5).
    pub fn outcome(&self, policy: &str, key: u64) -> Option<Result<Duration, ()>> {
        self.results
            .read()
            .expect("test results")
            .get(policy)
            .filter(|(k, _)| *k == key)
            .map(|(_, r)| r.outcome.as_ref().copied().map_err(|_| ()))
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        match self.test_case(name) {
            Some(case) => match self.auto.tests.result(&case.policy, case.key) {
                Some(result) => match result.outcome {
                    Ok(score) => Standing::Passed(score),
                    Err(_) => Standing::Failed,
                },
```

换成

```rust
        match self.test_slot(name) {
            Some((policy, test)) => match self.auto.tests.outcome(policy, test.key) {
                Some(Ok(score)) => Standing::Passed(score),
                Some(Err(())) => Standing::Failed,
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            None => Standing::Failed,
        }
    }

    /// How to test `name` now (M3 design 6.1): through its outbound, at its
```

换成

```rust
            None => Standing::Failed,
        }
    }

    /// Where `name`'s test results are kept — under the policy, `DIRECT` for
    /// the built-in and its desktop stand-ins, and its definition's key —
    /// when it can be tested at all. Nothing is built or cloned: a dial reads
    /// every member's result through this (M3c design 6.5).
    fn test_slot<'a>(&'a self, name: &'a str) -> Option<(&'a str, &'a TestSpec)> {
        let policy = match self.entries.get(name) {
            Some(Entry::Alias(Terminal::Direct) | Entry::Outbound { .. }) => name,
            Some(_) => return None,
            None => match PolicyRef::parse(name) {
                PolicyRef::Builtin(b) if RejectKind::from_builtin(b).is_none() => "DIRECT",
                _ => return None,
            },
        };
        let test = self.tests.get(policy)?;
        test.url.as_ref()?;
        Some((policy, test))
    }

    /// How to test `name` now (M3 design 6.1): through its outbound, at its
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        let (policy, outbound) = match PolicyRef::parse(name) {
            // DIRECT, and on the desktop the iOS-only built-ins that stand
            // in for it
            PolicyRef::Builtin(b) if RejectKind::from_builtin(b).is_none() => {
                ("DIRECT".to_string(), self.direct())
            }
            PolicyRef::Builtin(_) | PolicyRef::Device(_) => return None,
            PolicyRef::Named(n) => match self.entries.get(&n)? {
                Entry::Alias(Terminal::Direct) => (n, self.direct()),
                Entry::Outbound { outbound, .. } => {
                    let outbound = outbound.clone();
                    (n, outbound)
                }
                _ => return None,
            },
        };
        let test = self.tests.get(&policy)?;
```

换成

```rust
        let (policy, test) = self.test_slot(name)?;
        let outbound = match self.entries.get(policy) {
            Some(Entry::Outbound { outbound, .. }) => outbound.clone(),
            // DIRECT — on the desktop also the iOS-only built-ins that stand
            // in for it — and a `direct` alias without options of its own
            _ => self.direct(),
        };
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            policy,
```

换成

```rust
            policy: policy.to_string(),
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        let case = self.test_case(name)?;
        self.auto.tests.result(&case.policy, case.key)
```

换成

```rust
        let (policy, test) = self.test_slot(name)?;
        self.auto.tests.result(policy, test.key)
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            .filter_map(|p| self.test_case(p))
            .map(|case| case.timeout)
```

换成

```rust
            .filter_map(|p| self.test_slot(p))
            .map(|(_, test)| test.timeout)
```

要点：
- `test_slot(name)` 先按名字查条目（`Entry::Alias(Terminal::Direct)` 与 `Entry::Outbound` 可测，其余不可测），查不到才当内置策略解析（DIRECT 与它在桌面上的 iOS 替身都按 `DIRECT` 测）；再要求这一代的 `TestSpec` 里有一个能解析的 URL。与原来 `test_case` 的判断逐条相同，只是不分配。
- `test_case` 与 `test_result` 都改为从 `test_slot` 出发；`round_timeout` 只取 `TestSpec` 里的超时。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-policy` → 89 passed（新增 `the_outcome_is_read_for_the_current_definition`；M3b 的注册表用例全部照旧通过）。

- [ ] **Step 5: 门禁与提交**

跑门禁（40 个测试二进制，938 通过 / 1 忽略）。

```bash
git add crates/rurge-policy/src/testbook.rs crates/rurge-policy/src/registry.rs
git commit -m "perf(policy): 拨号读成员的测试结果不再构造 TestCase（M3b 延后事项 #14）"
```

### Task 3: `SmartBook` 的打分与状态；`TestBook` 推送测速结果

`smart` 组对成员的了解（设计第 5 节）：每个策略一条记录——首字节耗时的时间加权移动平均（半衰期 5 分钟）、失败罚分（每次 800 毫秒，同样衰减）、连续失败次数——得出健康 / 未知 / 失败三种状态。记录按策略名存，同时记下它是哪个出站对象的（P2）：定义变了（出站换了）从头积累。随引擎存续，不持久化。测速结果由 `TestBook` 在保存时推送过来（M3c-D7）：通过的分数是样本，失败是一次失败；`test_once` 不推送。本任务只做打分；排序、站点记忆与使用计数在 Task 4。

**Files:**
- Create: `crates/rurge-policy/src/smart.rs`（常数、`Health`、`SmartBook::{new, sample, failure, health, retain}`、`impl TestSink for SmartBook`，与用例）
- Modify: `crates/rurge-policy/src/testbook.rs`（`TestSink` trait、`TestBook::sink`、`run` 在保存结果后推送）
- Modify: `crates/rurge-policy/src/auto.rs`（`AutoGroups.smart`，`new` 里把它设为 `TestBook` 的 sink）
- Modify: `crates/rurge-policy/src/lib.rs`（`pub mod smart;`、crate 说明）

**Interfaces:**
- Consumes: Task 2 的 `TestBook`。
- Produces:
  - `pub const HALF_LIFE: Duration`（5 分钟）、`PENALTY`（800 毫秒）、`FAILED_SCORE`（3000 毫秒）、`FAILED_IN_A_ROW: u32`（3）
  - `pub enum Health { Healthy(Duration), Unknown { failures: u32 }, Failed(Option<Duration>) }`（`Clone + Copy + Debug + PartialEq`）
  - `SmartBook::new()`（及 `Default`）、`sample(&self, policy: &str, outbound: &OutboundRef, latency: Duration, now: Instant)`、`failure(&self, policy: &str, outbound: &OutboundRef, now: Instant)`、`health(&self, policy: &str, outbound: &OutboundRef, now: Instant) -> Health`、`retain(&self, keep: impl Fn(&str) -> bool)`
  - `pub trait TestSink: Send + Sync { fn tested(&self, case: &TestCase, result: &TestResult); }`、`TestBook::sink(&self, Arc<dyn TestSink>)`（只认第一次）
  - `AutoGroups { pub tests: Arc<TestBook>, pub smart: Arc<SmartBook>, .. }`

- [ ] **Step 1: 先写用例**

`crates/rurge-policy/src/testbook.rs`——把

```rust
        assert_eq!(book.outcome("P", 1), Some(Err(())));
    }
}
```

换成

```rust
        assert_eq!(book.outcome("P", 1), Some(Err(())));
    }

    #[derive(Default)]
    struct Heard(Mutex<Vec<(String, bool)>>);

    impl TestSink for Heard {
        fn tested(&self, case: &TestCase, result: &TestResult) {
            self.0
                .lock()
                .unwrap()
                .push((case.policy.clone(), result.outcome.is_ok()));
        }
    }

    /// The results the book keeps go to its sink; a one-off test is kept
    /// nowhere and goes nowhere (M3c-D7).
    #[tokio::test]
    async fn a_kept_result_goes_to_the_sink_and_a_one_off_does_not() {
        let gate = Arc::new(Gate::default());
        let heard = Arc::new(Heard::default());
        let book = book();
        book.sink(heard.clone());
        book.test(case("P", gate.clone(), 1)).await;
        book.test_once(&case("Q", gate.clone(), 1)).await;
        assert_eq!(*heard.0.lock().unwrap(), [("P".to_string(), false)]);
    }
}
```

`crates/rurge-policy/src/auto.rs`——把

```rust
        assert_eq!(rx.recv().await.as_deref(), Some("U"));
    }
}
```

换成

```rust
        assert_eq!(rx.recv().await.as_deref(), Some("U"));
    }

    /// The `smart` groups hear of every result the tests keep (M3c-D7).
    #[tokio::test]
    async fn the_smart_book_hears_of_every_kept_test() {
        let auto = auto();
        let outbound: rurge_proto::OutboundRef =
            Arc::new(rurge_proto::Reject::new(rurge_proto::RejectKind::Reject));
        let case = crate::testbook::TestCase {
            policy: "A".to_string(),
            outbound: outbound.clone(),
            url: url::Url::parse("http://127.0.0.1:9/").unwrap(),
            timeout: Duration::from_secs(5),
            key: 1,
            roots: Arc::new(rustls::RootCertStore::empty()),
        };
        auto.tests.test(case).await;
        assert_eq!(
            auto.smart.health("A", &outbound, Instant::now()),
            crate::smart::Health::Unknown { failures: 1 }
        );
    }
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-policy`
Expected: FAIL，编译错误——

```text
error[E0433]: failed to resolve: could not find `smart` in the crate root
error[E0405]: cannot find trait `TestSink` in this scope
error: could not compile `rurge-policy` (lib test) due to 2 previous errors
```

- [ ] **Step 3: 实现（新模块自带用例）**

新建 `crates/rurge-policy/src/smart.rs`：

```rust
//! What the `smart` groups know of their members (phase 2 M3c design §5): a
//! score per policy from the first-byte times of real sessions and from the
//! connectivity tests, a penalty for every failure, and whether a member is
//! healthy, failed or not known yet. It is kept per policy, so groups that
//! share a member share what is known of it (M3c-D2), and it outlives config
//! generations; a policy whose definition changed starts over.

use crate::testbook::{TestCase, TestResult, TestSink};
use rurge_proto::{Outbound, OutboundRef};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

/// How fast what is known fades: a sample, or a penalty, counts half as much
/// after this long (M3c design §9).
pub const HALF_LIFE: Duration = Duration::from_secs(5 * 60);
/// What each failure adds to the score.
pub const PENALTY: Duration = Duration::from_millis(800);
/// The score (before the group's factor) from which a member counts as
/// failed.
pub const FAILED_SCORE: Duration = Duration::from_millis(3000);
/// Failures in a row that make a member failed.
pub const FAILED_IN_A_ROW: u32 = 3;

/// What is known of a member (M3c design 5.4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Health {
    /// It has samples and has not failed: its score, before the group's
    /// factor.
    Healthy(Duration),
    /// No sample yet, and fewer than `FAILED_IN_A_ROW` failures in a row.
    Unknown { failures: u32 },
    /// `FAILED_IN_A_ROW` failures in a row, or a score of at least
    /// `FAILED_SCORE`; the score, when it has samples.
    Failed(Option<Duration>),
}

/// How much of what was known at `since` still counts at `now`.
fn fade(since: Instant, now: Instant) -> f64 {
    let age = now.saturating_duration_since(since).as_secs_f64();
    (-age / HALF_LIFE.as_secs_f64()).exp2()
}

fn millis(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

struct Record {
    /// What the numbers are about: another outbound under the same name —
    /// the definition changed, and with it the outbound (M2 design 7.1) —
    /// starts over. Holding a `Weak` keeps the allocation, so its address
    /// cannot come back as another outbound.
    outbound: Weak<dyn Outbound>,
    /// The time-weighted sum of the samples (ms) and of their weights, both
    /// as of `sampled`: their ratio does not move with time alone.
    sum: f64,
    weight: f64,
    sampled: Option<Instant>,
    /// The penalty (ms) as of `penalized`.
    penalty: f64,
    penalized: Option<Instant>,
    failures: u32,
}

impl Record {
    fn new(outbound: &OutboundRef) -> Record {
        Record {
            outbound: Arc::downgrade(outbound),
            sum: 0.0,
            weight: 0.0,
            sampled: None,
            penalty: 0.0,
            penalized: None,
            failures: 0,
        }
    }

    fn is_of(&self, outbound: &OutboundRef) -> bool {
        std::ptr::addr_eq(self.outbound.as_ptr(), Arc::as_ptr(outbound))
    }

    fn sample(&mut self, ms: f64, now: Instant) {
        if let Some(at) = self.sampled {
            let f = fade(at, now);
            self.sum *= f;
            self.weight *= f;
        }
        self.sum += ms;
        self.weight += 1.0;
        self.sampled = Some(now);
        self.failures = 0;
    }

    fn failure(&mut self, now: Instant) {
        self.penalty = self.penalty_at(now) + millis(PENALTY);
        self.penalized = Some(now);
        self.failures = self.failures.saturating_add(1);
    }

    fn penalty_at(&self, now: Instant) -> f64 {
        self.penalized
            .map_or(0.0, |at| self.penalty * fade(at, now))
    }

    fn health(&self, now: Instant) -> Health {
        let score = (self.weight > 0.0).then(|| {
            Duration::from_secs_f64((self.sum / self.weight + self.penalty_at(now)) / 1000.0)
        });
        if self.failures >= FAILED_IN_A_ROW || score.is_some_and(|s| s >= FAILED_SCORE) {
            return Health::Failed(score);
        }
        match score {
            Some(score) => Health::Healthy(score),
            None => Health::Unknown {
                failures: self.failures,
            },
        }
    }
}

/// What the `smart` groups know of their members; one per engine, like the
/// test results (M3c design 5.1). Time is always the caller's.
#[derive(Default)]
pub struct SmartBook {
    records: Mutex<HashMap<String, Record>>,
}

/// The record of `policy`, a fresh one when there is none for `outbound`.
fn record_of<'a>(
    records: &'a mut HashMap<String, Record>,
    policy: &str,
    outbound: &OutboundRef,
) -> &'a mut Record {
    if records.get(policy).is_none_or(|r| !r.is_of(outbound)) {
        records.insert(policy.to_string(), Record::new(outbound));
    }
    records.get_mut(policy).expect("just inserted")
}

impl SmartBook {
    pub fn new() -> SmartBook {
        SmartBook::default()
    }

    /// A sample of `policy` through `outbound`: the first byte came back
    /// `latency` after the outbound was ready, or a test passed with that
    /// score (M3c design 5.3).
    pub fn sample(&self, policy: &str, outbound: &OutboundRef, latency: Duration, now: Instant) {
        let mut records = self.records.lock().expect("smart records");
        record_of(&mut records, policy, outbound).sample(millis(latency), now);
    }

    /// A failure of `policy` through `outbound`: a dial, a session that got
    /// no answer, a test.
    pub fn failure(&self, policy: &str, outbound: &OutboundRef, now: Instant) {
        let mut records = self.records.lock().expect("smart records");
        record_of(&mut records, policy, outbound).failure(now);
    }

    /// What is known of `policy` through `outbound` at `now`.
    pub fn health(&self, policy: &str, outbound: &OutboundRef, now: Instant) -> Health {
        self.records
            .lock()
            .expect("smart records")
            .get(policy)
            .filter(|r| r.is_of(outbound))
            .map_or(Health::Unknown { failures: 0 }, |r| r.health(now))
    }

    /// A new generation: what is kept of the policies it no longer has goes.
    pub fn retain(&self, keep: impl Fn(&str) -> bool) {
        self.records
            .lock()
            .expect("smart records")
            .retain(|policy, _| keep(policy));
    }
}

/// The tests feed the scores too (M3c-D7): a pass is a sample, a failure a
/// failure.
impl TestSink for SmartBook {
    fn tested(&self, case: &TestCase, result: &TestResult) {
        match result.outcome {
            Ok(score) => self.sample(&case.policy, &case.outbound, score, result.at),
            Err(_) => self.failure(&case.policy, &case.outbound, result.at),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_proto::{Reject, RejectKind};

    fn outbound() -> OutboundRef {
        Arc::new(Reject::new(RejectKind::Reject))
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Within a millisecond: the scores are computed in floating point.
    fn healthy_near(health: Health, expected: Duration) {
        match health {
            Health::Healthy(score) => assert!(
                score.abs_diff(expected) < ms(1),
                "{score:?} is not {expected:?}"
            ),
            other => panic!("{other:?} is not healthy"),
        }
    }

    /// A sample counts half as much five minutes on (M3c design 5.2): 100
    /// then, 300 now, is (100 × 0.5 + 300) ÷ 1.5.
    #[test]
    fn a_sample_weighs_half_as_much_five_minutes_on() {
        let (book, a, t0) = (SmartBook::new(), outbound(), Instant::now());
        book.sample("A", &a, ms(100), t0);
        book.sample("A", &a, ms(300), t0 + HALF_LIFE);
        healthy_near(
            book.health("A", &a, t0 + HALF_LIFE),
            Duration::from_micros(233_333),
        );
    }

    #[test]
    fn the_score_does_not_move_with_time_alone() {
        let (book, a, t0) = (SmartBook::new(), outbound(), Instant::now());
        book.sample("A", &a, ms(100), t0);
        book.sample("A", &a, ms(300), t0 + HALF_LIFE);
        let later = t0 + Duration::from_secs(3600);
        assert_eq!(
            book.health("A", &a, t0 + HALF_LIFE),
            book.health("A", &a, later)
        );
    }

    /// Each failure adds 800 ms, which fades like the samples do.
    #[test]
    fn a_failure_adds_a_penalty_that_halves_every_five_minutes() {
        let (book, a, t0) = (SmartBook::new(), outbound(), Instant::now());
        book.sample("A", &a, ms(100), t0);
        book.failure("A", &a, t0);
        healthy_near(book.health("A", &a, t0), ms(900));
        healthy_near(book.health("A", &a, t0 + HALF_LIFE), ms(500));
    }

    #[test]
    fn three_failures_in_a_row_fail_a_member_and_a_success_restores_it() {
        let (book, a, t0) = (SmartBook::new(), outbound(), Instant::now());
        book.sample("A", &a, ms(100), t0);
        for _ in 0..3 {
            book.failure("A", &a, t0);
        }
        assert!(matches!(book.health("A", &a, t0), Health::Failed(Some(_))));
        book.sample("A", &a, ms(100), t0);
        healthy_near(book.health("A", &a, t0), ms(2500));
    }

    #[test]
    fn a_score_of_three_seconds_fails_a_member() {
        let (book, a, b, t0) = (SmartBook::new(), outbound(), outbound(), Instant::now());
        book.sample("A", &a, FAILED_SCORE, t0);
        assert_eq!(book.health("A", &a, t0), Health::Failed(Some(FAILED_SCORE)));
        book.sample("B", &b, FAILED_SCORE - ms(1), t0);
        healthy_near(book.health("B", &b, t0), FAILED_SCORE - ms(1));
    }

    #[test]
    fn without_a_sample_a_member_is_unknown_until_it_fails_three_times() {
        let (book, a, t0) = (SmartBook::new(), outbound(), Instant::now());
        assert_eq!(book.health("A", &a, t0), Health::Unknown { failures: 0 });
        book.failure("A", &a, t0);
        book.failure("A", &a, t0);
        assert_eq!(book.health("A", &a, t0), Health::Unknown { failures: 2 });
        book.failure("A", &a, t0);
        assert_eq!(book.health("A", &a, t0), Health::Failed(None));
    }

    /// A policy whose definition changed has another outbound: what was known
    /// of the old one does not count (M3c design 5.1).
    #[test]
    fn another_outbound_under_the_same_name_starts_over() {
        let (book, old, new, t0) = (SmartBook::new(), outbound(), outbound(), Instant::now());
        book.sample("A", &old, ms(100), t0);
        assert_eq!(book.health("A", &new, t0), Health::Unknown { failures: 0 });
        book.failure("A", &new, t0);
        assert_eq!(book.health("A", &new, t0), Health::Unknown { failures: 1 });
        assert_eq!(book.health("A", &old, t0), Health::Unknown { failures: 0 });
    }

    #[test]
    fn retain_forgets_the_policies_that_are_gone() {
        let (book, a, b, t0) = (SmartBook::new(), outbound(), outbound(), Instant::now());
        book.sample("A", &a, ms(100), t0);
        book.sample("B", &b, ms(100), t0);
        book.retain(|p| p == "B");
        assert_eq!(book.health("A", &a, t0), Health::Unknown { failures: 0 });
        healthy_near(book.health("B", &b, t0), ms(100));
    }
}
```

`crates/rurge-policy/src/lib.rs`——把

```rust
//! M3 design §6).
```

换成

```rust
//! M3 design §6), the `smart` groups by what real sessions show too
//! (`smart`; phase 2 M3c design).
```

`crates/rurge-policy/src/lib.rs`——把

```rust
pub mod selections;
```

换成

```rust
pub mod selections;
pub mod smart;
```

`crates/rurge-policy/src/testbook.rs`——把

```rust
    fn end(self: Box<Self>, outcome: &Result<Duration, String>);
}

type Running = (u64, watch::Receiver<Option<TestResult>>);
```

换成

```rust
    fn end(self: Box<Self>, outcome: &Result<Duration, String>);
}

/// Told of every result a test keeps: the `smart` groups score by the tests
/// too (phase 2 M3c design 5.3). Called with no lock of the book held.
pub trait TestSink: Send + Sync {
    fn tested(&self, case: &TestCase, result: &TestResult);
}

type Running = (u64, watch::Receiver<Option<TestResult>>);
```

`crates/rurge-policy/src/testbook.rs`——把

```rust
    observer: OnceLock<Arc<dyn TestObserver>>,
```

换成

```rust
    observer: OnceLock<Arc<dyn TestObserver>>,
    sink: OnceLock<Arc<dyn TestSink>>,
```

`crates/rurge-policy/src/testbook.rs`——把

```rust
            observer: OnceLock::new(),
```

换成

```rust
            observer: OnceLock::new(),
            sink: OnceLock::new(),
```

`crates/rurge-policy/src/testbook.rs`——把

```rust
        let _ = self.observer.set(observer);
```

换成

```rust
        let _ = self.observer.set(observer);
    }

    /// Where the results the book keeps go as well; set once, later calls
    /// are ignored. The one-off tests of `test_once` are not kept, so they
    /// do not go there either.
    pub fn sink(&self, sink: Arc<dyn TestSink>) {
        let _ = self.sink.set(sink);
```

`crates/rurge-policy/src/testbook.rs`——把

```rust
        // result, whichever ends last.
```

换成

```rust
        // result, whichever ends last.
        let mut kept = false;
```

`crates/rurge-policy/src/testbook.rs`——把

```rust
                        .insert(case.policy.clone(), (case.key, result.clone()));
                }
            }
```

换成

```rust
                        .insert(case.policy.clone(), (case.key, result.clone()));
                    kept = true;
                }
            }
        }
        if kept && let Some(sink) = self.sink.get() {
            sink.tested(&case, &result);
```

`crates/rurge-policy/src/auto.rs`——把

```rust
//! the way the registry asks the engine for a new round of tests.

use crate::testbook::TestBook;
```

换成

```rust
//! the way the registry asks the engine for a new round of tests.

use crate::smart::SmartBook;
use crate::testbook::TestBook;
```

`crates/rurge-policy/src/auto.rs`——把

```rust
    pub tests: Arc<TestBook>,
```

换成

```rust
    pub tests: Arc<TestBook>,
    /// What the `smart` groups know of their members; it hears of every
    /// result `tests` keeps (phase 2 M3c design 5.3).
    pub smart: Arc<SmartBook>,
```

`crates/rurge-policy/src/auto.rs`——把

```rust
        AutoGroups {
            tests,
```

换成

```rust
        let smart = Arc::new(SmartBook::new());
        tests.sink(smart.clone());
        AutoGroups {
            tests,
            smart,
```

要点：
- 移动平均用 `(S, W, t₀)` 增量维护：来一个样本先把 `S`、`W` 都乘以 `2^(−Δt/H)` 再加上新样本与权重 1；基础分 `S ÷ W` 与读取的时刻无关。罚分另存 `(P, tₚ)`，读时按同样的半衰期衰减。
- 失败：连续失败 ≥ 3 次，或有样本且"基础分 + 当前罚分" ≥ 3000 毫秒（因子之前）；一次成功清掉连续失败次数。没有样本、连续失败 < 3 次为未知。
- 同一个出站：记录存 `Weak<dyn Outbound>`，用 `std::ptr::addr_eq` 与当前出站比地址；持有 `Weak` 使那块内存不会被别的出站复用（P2）。
- `TestBook::run` 只在结果真正保存（仍是当前登记的那个测试）时推送，推送时不持有簿子的锁；`test_once` 不经过 `run`，所以不推送。
- 时间一律由调用方传入，单元用例因此不用真的等。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-policy` → 99 passed（新增 `smart::tests` 8 条、`a_kept_result_goes_to_the_sink_and_a_one_off_does_not`、`the_smart_book_hears_of_every_kept_test`）。

- [ ] **Step 5: 门禁与提交**

跑门禁（40 个测试二进制，948 通过 / 1 忽略）。

```bash
git add crates/rurge-policy/src/smart.rs crates/rurge-policy/src/lib.rs crates/rurge-policy/src/testbook.rs crates/rurge-policy/src/auto.rs
git commit -m "feat(policy): SmartBook——按策略的时间加权首字节分数、失败罚分与三种状态，测速结果由 TestBook 推送"
```

### Task 4: `smart` 组的选择——排序与重试列表、站点记忆、使用计数；接入注册表

`smart` 组怎样选成员（设计第 6 节）：健康的按分数（乘上 `policy-priority` 因子）升序，然后未知的（连续失败少的在前，其次按成员顺序），最后失败的；最近在这个目标主机上失败过的挪到最末。某个健康成员最近在这个主机上成功过、且分数不超过最优者 2 倍，直接选它；否则在最优者 1.2 倍以内的健康成员里随机选一个；没有健康成员时选排在第一的。其余按这个顺序成为重试列表（引擎在 Task 5 用它）。站点记忆按"主机 + 策略"记最近一次的结果，1 小时有效，最多 4096 个主机、每个主机 16 个策略；使用计数按分钟分桶、只留 10 分钟。

注册表这边：每一代构建时 `smart` 组只留代理成员（P5）、按 `policy-priority` 预算每个成员的因子；`choose` 的 `smart` 分支——覆盖期间只用覆盖的成员（没有重试列表），拨号时（`live`）按上面的规则选并在有成员"未知"或上一轮已过 5 分钟时请求一轮测试（P11），控制面的读取显示最近 10 分钟用得最多的成员（P12）；解析结果多一个 `smart: Option<SmartPick>`（P16）。

**Files:**
- Modify: `crates/rurge-policy/src/smart.rs`（常数、`SiteMemory`、`Candidate` / `Ranking` / `rank`；`SmartBook` 的站点记忆与使用计数、`report_success` / `report_failure` / `site` / `used` / `most_used`，`retain` 一并清理它们）
- Modify: `crates/rurge-policy/src/registry.rs`（`Resolution.smart`、`SmartPick`、组条目的 `factors`、`smart_members`、`choose` 返回 `Choice`、`smart_health`、`available` 对 `smart` 组、`resolve_member`）
- Modify: `crates/rurge-policy/src/lib.rs`（导出 `SmartPick`）

**Interfaces:**
- Consumes: Task 3 的 `SmartBook`、`Health`。
- Produces:
  - `pub const PREFERRED: f64`（1.2）、`SITE_PREFERRED: f64`（2.0）、`SITE_MEMORY: Duration`（1 小时）、`SITES: usize`（4096）、`POLICIES_PER_SITE: usize`（16）、`USAGE_WINDOW: Duration`（10 分钟）、`ROUND_INTERVAL: Duration`（5 分钟）
  - `pub struct SiteMemory { pub worked: Vec<String>, pub failed: Vec<String> }`（`Default`）
  - `pub struct Candidate<'a> { pub name: &'a str, pub health: Health, pub factor: f64 }`、`pub struct Ranking { pub pick: String, pub retry: Vec<String> }`
  - `pub fn rank(candidates: &[Candidate<'_>], site: &SiteMemory, pick_at: impl FnOnce(usize) -> usize) -> Option<Ranking>`
  - `SmartBook::report_success(&self, policy: &str, outbound: &OutboundRef, host: Option<&str>, latency: Duration, now: Instant)`、`report_failure(&self, policy: &str, outbound: &OutboundRef, host: Option<&str>, now: Instant)`、`site(&self, host: &str, now: Instant) -> SiteMemory`、`used(&self, group: &str, member: &str, now: Instant)`、`most_used(&self, group: &str, members: &[String], now: Instant) -> Vec<String>`
  - `pub struct SmartPick { pub group: String, pub member: String, pub retry: Vec<String> }`（`Clone + Debug + PartialEq + Eq`），`Resolution { .., pub smart: Option<SmartPick> }`
  - `PolicyRegistry::resolve_member(&self, member: &str) -> Resolution`（链只有成员自己）

- [ ] **Step 1: 先写用例**

`crates/rurge-policy/src/smart.rs`——把

```rust
        healthy_near(book.health("B", &b, t0), ms(100));
    }
}
```

换成

```rust
        healthy_near(book.health("B", &b, t0), ms(100));
    }

    fn healthy(name: &str, ms: u64) -> Candidate<'_> {
        Candidate {
            name,
            health: Health::Healthy(Duration::from_millis(ms)),
            factor: 1.0,
        }
    }

    fn with(name: &str, health: Health) -> Candidate<'_> {
        Candidate {
            name,
            health,
            factor: 1.0,
        }
    }

    fn nowhere() -> SiteMemory {
        SiteMemory::default()
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// The preferred set is the healthy members within a fifth of the best
    /// one's score; a dial picks from it at random (M3c design 6.2).
    #[test]
    fn a_dial_picks_at_random_among_those_close_to_the_best() {
        let c = [healthy("A", 100), healthy("B", 110), healthy("C", 200)];
        let mut seen = 0;
        let r = rank(&c, &nowhere(), |n| {
            seen = n;
            1
        })
        .unwrap();
        assert_eq!(seen, 2, "A and B are within 1.2 × 100 ms");
        assert_eq!(r.pick, "B");
        assert_eq!(r.retry, names(&["A", "C"]));
        let r = rank(&c, &nowhere(), |_| 0).unwrap();
        assert_eq!((r.pick.as_str(), r.retry), ("A", names(&["B", "C"])));
    }

    #[test]
    fn a_priority_factor_scales_the_score() {
        let mut slow = healthy("A", 100);
        slow.factor = 2.0;
        let c = [slow, healthy("B", 150)];
        let r = rank(&c, &nowhere(), |n| n - 1).unwrap();
        assert_eq!(r.pick, "B", "200 ms is beyond 1.2 × 150 ms");
        assert_eq!(r.retry, names(&["A"]));
    }

    /// Healthy ones first, then those not known yet — fewer failures first —
    /// then the failed ones, those with a score first.
    #[test]
    fn the_unknown_come_after_the_healthy_and_the_failed_last() {
        let c = [
            with("A", Health::Failed(None)),
            with("B", Health::Unknown { failures: 1 }),
            with("C", Health::Failed(Some(ms(4000)))),
            with("D", Health::Unknown { failures: 0 }),
            healthy("E", 300),
        ];
        let r = rank(&c, &nowhere(), |_| 0).unwrap();
        assert_eq!(r.pick, "E");
        assert_eq!(r.retry, names(&["D", "B", "C", "A"]));
    }

    #[test]
    fn without_a_healthy_member_the_first_in_line_is_picked() {
        let c = [
            with("A", Health::Failed(Some(ms(4000)))),
            with("B", Health::Unknown { failures: 0 }),
        ];
        let r = rank(&c, &nowhere(), |_| unreachable!("nothing to draw from")).unwrap();
        assert_eq!((r.pick.as_str(), r.retry), ("B", names(&["A"])));
        assert_eq!(rank(&[], &nowhere(), |_| 0), None);
    }

    /// A member that worked at the site lately is picked while it is within
    /// twice the best score; one that failed there goes last (M3c design
    /// 6.2).
    #[test]
    fn what_happened_at_the_site_comes_first() {
        let c = [healthy("A", 100), healthy("B", 180), healthy("C", 250)];
        let site = SiteMemory {
            worked: names(&["B", "C"]),
            failed: Vec::new(),
        };
        let r = rank(&c, &site, |_| unreachable!("the site decides")).unwrap();
        assert_eq!((r.pick.as_str(), r.retry), ("B", names(&["A", "C"])));
        let site = SiteMemory {
            worked: names(&["C"]),
            failed: names(&["A"]),
        };
        let r = rank(&c, &site, |n| {
            assert_eq!(n, 1, "only B is left within 1.2 × 180 ms");
            0
        })
        .unwrap();
        assert_eq!((r.pick.as_str(), r.retry), ("B", names(&["C", "A"])));
    }

    #[test]
    fn a_site_is_remembered_for_an_hour() {
        let (book, a, b, t0) = (SmartBook::new(), outbound(), outbound(), Instant::now());
        book.report_success("A", &a, Some("x.test"), ms(50), t0);
        book.report_failure("B", &b, Some("x.test"), t0);
        let memory = book.site("x.test", t0 + Duration::from_secs(60));
        assert_eq!(
            memory,
            SiteMemory {
                worked: names(&["A"]),
                failed: names(&["B"]),
            }
        );
        assert_eq!(book.site("x.test", t0 + SITE_MEMORY), SiteMemory::default());
        assert_eq!(book.site("y.test", t0), SiteMemory::default());
        // the latest outcome of a policy stands
        book.report_failure("A", &a, Some("x.test"), t0);
        assert_eq!(book.site("x.test", t0).failed, names(&["B", "A"]));
    }

    #[test]
    fn the_site_used_least_recently_goes_first() {
        let (book, a, t0) = (SmartBook::new(), outbound(), Instant::now());
        for i in 0..SITES {
            book.report_success("A", &a, Some(&format!("{i}.test")), ms(50), t0);
        }
        // reading a site uses it
        assert_eq!(book.site("0.test", t0).worked, names(&["A"]));
        book.report_success("A", &a, Some("new.test"), ms(50), t0);
        assert_eq!(book.site("0.test", t0).worked, names(&["A"]), "used lately");
        assert_eq!(
            book.site("1.test", t0),
            SiteMemory::default(),
            "the least recently used"
        );
        assert_eq!(book.site("new.test", t0).worked, names(&["A"]));
    }

    #[test]
    fn a_site_keeps_its_latest_sixteen_policies() {
        let (book, t0) = (SmartBook::new(), Instant::now());
        let outbounds: Vec<OutboundRef> = (0..=POLICIES_PER_SITE).map(|_| outbound()).collect();
        for (i, o) in outbounds.iter().enumerate() {
            book.report_success(&format!("P{i}"), o, Some("x.test"), ms(50), t0);
        }
        let worked = book.site("x.test", t0).worked;
        assert_eq!(worked.len(), POLICIES_PER_SITE);
        assert!(!worked.contains(&"P0".to_string()), "the oldest went");
    }

    /// The usage counts: the last ten minutes, most used first, a tie in the
    /// group's order (M3c design 8.2).
    #[test]
    fn the_most_used_members_of_the_last_ten_minutes() {
        let (book, t0) = (SmartBook::new(), Instant::now());
        let members = names(&["A", "B", "C"]);
        book.used("G", "C", t0);
        book.used("G", "B", t0 + Duration::from_secs(120));
        book.used("G", "C", t0 + Duration::from_secs(180));
        book.used("G", "A", t0 + Duration::from_secs(240));
        let at = t0 + Duration::from_secs(300);
        assert_eq!(book.most_used("G", &members, at), names(&["C", "A", "B"]));
        assert_eq!(book.most_used("H", &members, at), Vec::<String>::new());
        let later = t0 + USAGE_WINDOW + Duration::from_secs(1);
        assert_eq!(
            book.most_used("G", &members, later),
            names(&["A", "B", "C"])
        );
    }

    #[test]
    fn a_report_scores_the_member_as_well() {
        let (book, a, t0) = (SmartBook::new(), outbound(), Instant::now());
        book.report_success("A", &a, None, ms(100), t0);
        book.report_failure("A", &a, None, t0);
        healthy_near(book.health("A", &a, t0), ms(900));
    }
}
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        assert!(reg.auto().requested().is_empty());
    }

    #[test]
    fn selections_api() {
```

换成

```rust
        assert!(reg.auto().requested().is_empty());
    }

    const SMART: &str = "[General]\nproxy-test-url = http://127.0.0.1:9/\ninternet-test-url = http://127.0.0.1:9/\n\
[Proxy]\nA = http, a.example, 80\nB = http, b.example, 80\nC = http, c.example, 80\nD = direct\n\
[Proxy Group]\nSel = select, A, B\nS = smart, A, B, C, Sel, DIRECT, D, policy-priority=\"C:0.5\"\n\
Only = smart, Sel, DIRECT, D\nE = smart, A, B, evaluate-before-use=true\nOuter = select, S\n\
[Rule]\nFINAL,S\n";

    /// As if a session through `name` had its first byte back in `ms`.
    fn smart_seed(reg: &PolicyRegistry, name: &str, ms: u64) {
        let outbound = outbound_of(reg, name);
        reg.auto()
            .smart
            .sample(name, &outbound, Duration::from_millis(ms), Instant::now());
    }

    fn smart_fail(reg: &PolicyRegistry, name: &str) {
        let outbound = outbound_of(reg, name);
        for _ in 0..crate::smart::FAILED_IN_A_ROW {
            reg.auto().smart.failure(name, &outbound, Instant::now());
        }
    }

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// A `smart` group takes proxies only: a nested group, a built-in and a
    /// `direct` alias are left out; with none left it is an empty group
    /// (M3c design 6.1).
    #[test]
    fn a_smart_group_takes_proxies_only() {
        let reg = generation(SMART, &FakeFactory::new(), None);
        assert_eq!(reg.members("S").unwrap(), strings(&["A", "B", "C"]));
        assert!(reg.members("Only").unwrap().is_empty());
        let empty = reg.resolve(&PolicyRef::parse("Only"));
        assert_eq!(empty.chain, ["Only", "DIRECT"]);
        assert_eq!(empty.smart, None);
    }

    /// A dial picks by what the book knows — `policy-priority` scales C's
    /// 150 ms to 75 ms — and brings the members to try next along, through a
    /// group that holds the `smart` one too (M3c design 6.2, 6.4).
    #[test]
    fn a_dial_picks_by_the_smart_book_and_brings_the_others_along() {
        let reg = generation(SMART, &FakeFactory::new(), None);
        smart_seed(&reg, "A", 300);
        smart_seed(&reg, "B", 100);
        smart_seed(&reg, "C", 150);
        let r = reg.resolve(&PolicyRef::parse("S"));
        assert_eq!(r.chain, ["S", "C"]);
        assert_eq!(r.terminal, TerminalKind::Proxy);
        assert_eq!(
            r.smart,
            Some(SmartPick {
                group: "S".to_string(),
                member: "C".to_string(),
                retry: strings(&["B", "A"]),
            })
        );
        let outer = reg.resolve(&PolicyRef::parse("Outer"));
        assert_eq!(outer.chain, ["Outer", "S", "C"]);
        assert_eq!(outer.smart.map(|p| p.group).as_deref(), Some("S"));
    }

    #[test]
    fn a_member_on_its_own_resolves_to_itself() {
        let reg = generation(SMART, &FakeFactory::new(), None);
        let r = reg.resolve_member("B");
        assert_eq!(r.chain, ["B"]);
        assert_eq!(r.terminal, TerminalKind::Proxy);
        assert_eq!(r.smart, None);
    }

    /// An override stands: no ranking, nobody to try next, no test asked for.
    #[test]
    fn an_override_of_a_smart_group_stands_alone() {
        let reg = generation(SMART, &FakeFactory::new(), None);
        reg.auto().set_override(reg.group_spec("S").unwrap(), "A");
        let r = reg.resolve(&PolicyRef::parse("S"));
        assert_eq!(r.chain, ["S", "A"]);
        assert_eq!(r.smart, None);
        assert!(reg.auto().requested().is_empty());
    }

    /// A dial asks for a round while a member is not known yet; the control
    /// plane's view does not (M3c design 8.1).
    #[test]
    fn a_dial_asks_for_a_round_while_a_member_is_unknown() {
        let reg = generation(SMART, &FakeFactory::new(), None);
        let _ = reg.current_member("S");
        assert!(reg.auto().requested().is_empty(), "the view asks nothing");
        let _ = reg.resolve(&PolicyRef::parse("S"));
        assert_eq!(reg.auto().requested(), ["S"]);
        reg.auto().round_done(&["S".to_string()]);
        for name in ["A", "B", "C"] {
            smart_seed(&reg, name, 100);
        }
        let _ = reg.resolve(&PolicyRef::parse("S"));
        assert!(
            reg.auto().requested().is_empty(),
            "all known, the round fresh"
        );
    }

    /// The view shows the member used most lately, else the first in line
    /// (M3c design 8.3).
    #[test]
    fn the_view_of_a_smart_group_is_its_most_used_member() {
        let reg = generation(SMART, &FakeFactory::new(), None);
        smart_seed(&reg, "A", 50);
        smart_seed(&reg, "B", 100);
        assert_eq!(reg.current_member("S").as_deref(), Some("A"));
        let now = Instant::now();
        reg.auto().smart.used("S", "B", now);
        reg.auto().smart.used("S", "B", now);
        reg.auto().smart.used("S", "A", now);
        assert_eq!(reg.current_member("S").as_deref(), Some("B"));
    }

    /// Of a `smart` group, the healthy members are available; the first
    /// dial of an `evaluate-before-use` one waits for a round (M3c design
    /// 8.1).
    #[test]
    fn a_smart_group_is_available_by_its_health() {
        let reg = generation(SMART, &FakeFactory::new(), None);
        smart_seed(&reg, "A", 100);
        smart_fail(&reg, "B");
        assert_eq!(reg.available("S"), ["A"]);
        let r = reg.resolve(&PolicyRef::parse("E"));
        assert_eq!(r.pending.as_deref(), Some("E"));
        reg.auto().round_done(&["E".to_string()]);
        assert_eq!(reg.resolve(&PolicyRef::parse("E")).pending, None);
    }

    #[test]
    fn selections_api() {
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-policy`
Expected: FAIL，编译错误——

```text
error[E0433]: failed to resolve: use of undeclared type `SiteMemory`
error[E0425]: cannot find function `rank` in this scope
```

（另有 `SmartPick`、`resolve_member`、`report_success` 等未定义的错误。）

- [ ] **Step 3: 实现**

`crates/rurge-policy/src/smart.rs`——把

```rust
//! generations; a policy whose definition changed starts over.
```

换成

```rust
//! generations; a policy whose definition changed starts over. Beside it,
//! what happened at each site lately and how often each group used each
//! member — and how a dial ranks a group's members by all that (M3c design
//! §6).
```

`crates/rurge-policy/src/smart.rs`——把

```rust
use std::collections::HashMap;
```

换成

```rust
use std::collections::{HashMap, VecDeque};
```

`crates/rurge-policy/src/smart.rs`——把

```rust
pub const FAILED_IN_A_ROW: u32 = 3;
```

换成

```rust
pub const FAILED_IN_A_ROW: u32 = 3;
/// Members whose score is within this factor of the best one's form the
/// preferred set, which a dial picks from at random (M3c design 6.2).
pub const PREFERRED: f64 = 1.2;
/// A member that worked at the session's site lately is picked while its
/// score is within this factor of the best one's.
pub const SITE_PREFERRED: f64 = 2.0;
/// How long what happened at a site is remembered (M3c design 6.3).
pub const SITE_MEMORY: Duration = Duration::from_secs(60 * 60);
/// Sites remembered at most; the one used least recently goes first.
pub const SITES: usize = 4096;
/// Policies remembered per site at most; the oldest goes first.
pub const POLICIES_PER_SITE: usize = 16;
/// The window of the usage counts (M3c design 8.2), kept a minute a bucket.
pub const USAGE_WINDOW: Duration = Duration::from_secs(10 * 60);
const USAGE_BUCKET: Duration = Duration::from_secs(60);
/// A `smart` group's round of tests is due this long after its last one;
/// `interval` has no effect on it (M3c design 8.1).
pub const ROUND_INTERVAL: Duration = Duration::from_secs(5 * 60);
```

`crates/rurge-policy/src/smart.rs`——把

```rust
        }
    }
}

/// What the `smart` groups know of their members; one per engine, like the
```

换成

```rust
        }
    }
}

/// What happened lately at one site, as a dial through a `smart` group
/// reads it (M3c design 6.3).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SiteMemory {
    /// The policies whose last session at the site worked.
    pub worked: Vec<String>,
    /// The policies whose last session at the site failed.
    pub failed: Vec<String>,
}

#[derive(Default)]
struct Sites {
    hosts: HashMap<String, Site>,
    /// Orders the uses of the sites: the least recently used one goes when
    /// there are too many.
    clock: u64,
}

struct Site {
    /// `Sites::clock` when the site was last read or written.
    used: u64,
    /// The last outcome of each policy at the site, oldest first.
    policies: Vec<(String, bool, Instant)>,
}

impl Sites {
    fn remember(&mut self, host: &str, policy: &str, worked: bool, now: Instant) {
        self.clock += 1;
        let clock = self.clock;
        if !self.hosts.contains_key(host) && self.hosts.len() >= SITES {
            let oldest = self
                .hosts
                .iter()
                .min_by_key(|(_, site)| site.used)
                .map(|(host, _)| host.clone());
            if let Some(oldest) = oldest {
                self.hosts.remove(&oldest);
            }
        }
        let site = self.hosts.entry(host.to_string()).or_insert_with(|| Site {
            used: clock,
            policies: Vec::new(),
        });
        site.used = clock;
        site.policies
            .retain(|(p, _, at)| p != policy && now.saturating_duration_since(*at) < SITE_MEMORY);
        site.policies.push((policy.to_string(), worked, now));
        if site.policies.len() > POLICIES_PER_SITE {
            site.policies.remove(0);
        }
    }

    fn read(&mut self, host: &str, now: Instant) -> SiteMemory {
        self.clock += 1;
        let clock = self.clock;
        let mut memory = SiteMemory::default();
        if let Some(site) = self.hosts.get_mut(host) {
            site.used = clock;
            for (policy, worked, at) in &site.policies {
                if now.saturating_duration_since(*at) < SITE_MEMORY {
                    let list = if *worked {
                        &mut memory.worked
                    } else {
                        &mut memory.failed
                    };
                    list.push(policy.clone());
                }
            }
        }
        memory
    }
}

/// A group's uses of its members, a bucket a minute over `USAGE_WINDOW`.
#[derive(Default)]
struct Usage {
    buckets: VecDeque<(Instant, HashMap<String, u32>)>,
}

impl Usage {
    fn add(&mut self, member: &str, now: Instant) {
        while self
            .buckets
            .front()
            .is_some_and(|(start, _)| now.saturating_duration_since(*start) >= USAGE_WINDOW)
        {
            self.buckets.pop_front();
        }
        if self
            .buckets
            .back()
            .is_none_or(|(start, _)| now.saturating_duration_since(*start) >= USAGE_BUCKET)
        {
            self.buckets.push_back((now, HashMap::new()));
        }
        let (_, bucket) = self.buckets.back_mut().expect("just pushed");
        *bucket.entry(member.to_string()).or_default() += 1;
    }

    fn count(&self, member: &str, now: Instant) -> u32 {
        self.buckets
            .iter()
            .filter(|(start, _)| now.saturating_duration_since(*start) < USAGE_WINDOW)
            .filter_map(|(_, bucket)| bucket.get(member))
            .sum()
    }
}

/// A member of a `smart` group as a dial ranks it.
#[derive(Clone, Copy, Debug)]
pub struct Candidate<'a> {
    pub name: &'a str,
    pub health: Health,
    /// The group's `policy-priority` factor for it.
    pub factor: f64,
}

/// The member a dial through a `smart` group uses, and the others in the
/// order a dial tries them when it does not connect (M3c design 6.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ranking {
    pub pick: String,
    pub retry: Vec<String>,
}

/// Ranks the members of a `smart` group (M3c design 6.2): the healthy ones by
/// score (their factor applied), then those not known yet, then the failed
/// ones; those that failed at the site lately go last of all. A healthy one
/// that worked at the site lately is picked while its score is within
/// `SITE_PREFERRED` of the best; otherwise `pick_at(n)` picks one of the `n`
/// healthy members within `PREFERRED` of the best; with none healthy, the
/// first in line is.
pub fn rank(
    candidates: &[Candidate<'_>],
    site: &SiteMemory,
    pick_at: impl FnOnce(usize) -> usize,
) -> Option<Ranking> {
    let mut healthy: Vec<(usize, f64)> = Vec::new();
    let mut unknown: Vec<(usize, u32)> = Vec::new();
    let mut failed: Vec<(usize, Option<Duration>)> = Vec::new();
    for (i, c) in candidates.iter().enumerate() {
        match c.health {
            Health::Healthy(score) => healthy.push((i, score.as_secs_f64() * c.factor)),
            Health::Unknown { failures } => unknown.push((i, failures)),
            Health::Failed(score) => failed.push((i, score)),
        }
    }
    healthy.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    unknown.sort_by_key(|&(i, failures)| (failures, i));
    // those with a score first, lowest first; then the others, in order
    failed.sort_by_key(|&(i, score)| (score.is_none(), score, i));
    let failed_here = |i: usize| site.failed.iter().any(|p| p == candidates[i].name);
    let worked_here = |i: usize| site.worked.iter().any(|p| p == candidates[i].name);
    let (mut order, last): (Vec<usize>, Vec<usize>) = healthy
        .iter()
        .map(|&(i, _)| i)
        .chain(unknown.iter().map(|&(i, _)| i))
        .chain(failed.iter().map(|&(i, _)| i))
        .partition(|&i| !failed_here(i));
    order.extend(last);
    let usable: Vec<(usize, f64)> = healthy
        .iter()
        .copied()
        .filter(|&(i, _)| !failed_here(i))
        .collect();
    let known = healthy.first().map(|&(_, best)| {
        usable
            .iter()
            .find(|&&(i, score)| worked_here(i) && score <= best * SITE_PREFERRED)
            .map(|&(i, _)| i)
    });
    let pick = match (known.flatten(), usable.first()) {
        (Some(i), _) => i,
        (None, Some(&(_, best))) => {
            let preferred: Vec<usize> = usable
                .iter()
                .take_while(|&&(_, score)| score <= best * PREFERRED)
                .map(|&(i, _)| i)
                .collect();
            preferred[pick_at(preferred.len()).min(preferred.len() - 1)]
        }
        (None, None) => *order.first()?,
    };
    Some(Ranking {
        pick: candidates[pick].name.to_string(),
        retry: order
            .iter()
            .filter(|&&i| i != pick)
            .map(|&i| candidates[i].name.to_string())
            .collect(),
    })
}

/// What the `smart` groups know of their members; one per engine, like the
```

`crates/rurge-policy/src/smart.rs`——把

```rust
    records: Mutex<HashMap<String, Record>>,
```

换成

```rust
    records: Mutex<HashMap<String, Record>>,
    sites: Mutex<Sites>,
    /// By group.
    usage: Mutex<HashMap<String, Usage>>,
```

`crates/rurge-policy/src/smart.rs`——把

```rust
    /// A new generation: what is kept of the policies it no longer has goes.
```

换成

```rust
    /// The first byte of a session through `policy` came back `latency`
    /// after the outbound was ready (M3c design 4.3): a sample, and `policy`
    /// worked at `host`.
    pub fn report_success(
        &self,
        policy: &str,
        outbound: &OutboundRef,
        host: Option<&str>,
        latency: Duration,
        now: Instant,
    ) {
        self.change(policy, outbound, now, |r| r.sample(millis(latency), now));
        if let Some(host) = host {
            self.sites
                .lock()
                .expect("smart sites")
                .remember(host, policy, true, now);
        }
    }

    /// A dial through `policy` did not connect, or its session got no answer
    /// (M3c design 4.3): a failure, and `policy` failed at `host`.
    pub fn report_failure(
        &self,
        policy: &str,
        outbound: &OutboundRef,
        host: Option<&str>,
        now: Instant,
    ) {
        self.change(policy, outbound, now, |r| r.failure(now));
        if let Some(host) = host {
            self.sites
                .lock()
                .expect("smart sites")
                .remember(host, policy, false, now);
        }
    }

    /// Applies `change` to the record of `policy`; says so when the member
    /// comes to count as failed by it, or stops to (M3c design §10).
    fn change(
        &self,
        policy: &str,
        outbound: &OutboundRef,
        now: Instant,
        change: impl FnOnce(&mut Record),
    ) {
        let (was, is) = {
            let mut records = self.records.lock().expect("smart records");
            let record = record_of(&mut records, policy, outbound);
            let was = matches!(record.health(now), Health::Failed(_));
            change(record);
            (was, matches!(record.health(now), Health::Failed(_)))
        };
        if was != is {
            if is {
                tracing::info!(policy, "smart: the policy counts as failed");
            } else {
                tracing::info!(policy, "smart: the policy works again");
            }
        }
    }

    /// What happened at `host` lately (M3c design 6.3).
    pub fn site(&self, host: &str, now: Instant) -> SiteMemory {
        self.sites.lock().expect("smart sites").read(host, now)
    }

    /// A session of `group` went through `member` (M3c design 8.2).
    pub fn used(&self, group: &str, member: &str, now: Instant) {
        self.usage
            .lock()
            .expect("smart usage")
            .entry(group.to_string())
            .or_default()
            .add(member, now);
    }

    /// `members` that `group` used within `USAGE_WINDOW`, most used first;
    /// a tie keeps the order of `members`.
    pub fn most_used(&self, group: &str, members: &[String], now: Instant) -> Vec<String> {
        let usage = self.usage.lock().expect("smart usage");
        let Some(usage) = usage.get(group) else {
            return Vec::new();
        };
        let mut used: Vec<(u32, usize)> = members
            .iter()
            .enumerate()
            .map(|(i, m)| (usage.count(m, now), i))
            .filter(|&(n, _)| n > 0)
            .collect();
        used.sort_by_key(|&(n, i)| (std::cmp::Reverse(n), i));
        used.into_iter().map(|(_, i)| members[i].clone()).collect()
    }

    /// A new generation: what is kept of the policies and groups it no longer
    /// has goes.
```

`crates/rurge-policy/src/smart.rs`——把

```rust
            .retain(|policy, _| keep(policy));
```

换成

```rust
            .retain(|policy, _| keep(policy));
        for site in self.sites.lock().expect("smart sites").hosts.values_mut() {
            site.policies.retain(|(policy, _, _)| keep(policy));
        }
        self.usage
            .lock()
            .expect("smart usage")
            .retain(|group, _| keep(group));
```

`crates/rurge-policy/src/registry.rs`——把

```rust
use crate::selections::SelectionTable;
```

换成

```rust
use crate::selections::SelectionTable;
use crate::smart::{Candidate, Health, ROUND_INTERVAL, SiteMemory, rank};
```

`crates/rurge-policy/src/registry.rs`——把

```rust
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
```

换成

```rust
use std::collections::hash_map::RandomState;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::{BuildHasher, DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
```

`crates/rurge-policy/src/registry.rs`——把

```rust
    pub pending: Option<String>,
```

换成

```rust
    pub pending: Option<String>,
    /// The `smart` group on the way, when a dial went through one: what it
    /// picked and whom to try next (phase 2 M3c design 6.4). There is one at
    /// most: a `smart` group takes no groups.
    pub smart: Option<SmartPick>,
}

/// A dial through a `smart` group (M3c design 6.4): the group, the member it
/// picked, and the members to try, in order, when that one does not connect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SmartPick {
    pub group: String,
    pub member: String,
    pub retry: Vec<String>,
}

/// What `choose` found for a group.
struct Choice {
    member: String,
    /// The group wants its first round of tests before it is used
    /// (`evaluate-before-use`).
    pending: bool,
    /// A `smart` group's dial: the members to try after `member`, in order.
    retry: Option<Vec<String>>,
}

impl Choice {
    fn plain(member: String) -> Choice {
        Choice {
            member,
            pending: false,
            retry: None,
        }
    }
}

/// A number below `n`, drawn the way `load-balance` draws.
fn random_below(n: usize) -> usize {
    RandomState::new().hash_one(Instant::now()) as usize % n.max(1)
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        cycle: Option<String>,
```

换成

```rust
        cycle: Option<String>,
        /// A `smart` group's `policy-priority` factor of each member, in
        /// member order; empty for the other kinds.
        factors: Vec<f64>,
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        .map_err(|e| BuildError::new(format!("policy `{}`: {}", spec.name, e.message)))
```

换成

```rust
        .map_err(|e| BuildError::new(format!("policy `{}`: {}", spec.name, e.message)))
}

/// A `smart` group's members: the proxies among `members` — the manual has
/// it ignore a nested group, a built-in policy and a `direct` / `reject*`
/// alias, which is said once per build — each with its `policy-priority`
/// factor, the first pattern that matches deciding (M3c design 6.1).
fn smart_members(
    spec: &GroupSpec,
    members: Vec<String>,
    entries: &HashMap<String, Entry>,
) -> (Vec<String>, Vec<f64>) {
    let (kept, ignored): (Vec<String>, Vec<String>) = members.into_iter().partition(|m| {
        matches!(
            entries.get(m.as_str()),
            Some(Entry::Outbound { proxy: true, .. } | Entry::Unsupported { .. })
        )
    });
    if !ignored.is_empty() {
        tracing::info!(
            group = %spec.name,
            ignored = %ignored.join(", "),
            "a smart group takes proxy policies only; the others are ignored"
        );
    }
    let factors = kept
        .iter()
        .map(|m| {
            spec.priority
                .iter()
                .find(|p| p.pattern.regex.is_match(m).unwrap_or(false))
                .map_or(1.0, |p| p.factor)
        })
        .collect();
    (kept, factors)
```

`crates/rurge-policy/src/registry.rs`——把

```rust
                .collect();
            let cycle = on_cycle.get(g.name.as_str()).cloned();
```

换成

```rust
                .collect();
            let (members, factors) = if g.kind == GroupKind::Smart {
                smart_members(g, members, &table.entries)
            } else {
                (members, Vec::new())
            };
            let cycle = on_cycle.get(g.name.as_str()).cloned();
```

`crates/rurge-policy/src/registry.rs`——把

```rust
                cycle,
            };
```

换成

```rust
                cycle,
                factors,
            };
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            .map(|(member, _)| member)
```

换成

```rust
            .map(|choice| choice.member)
```

`crates/rurge-policy/src/registry.rs`——把

```rust
    /// is asked for. `live` is a dial: only then may `url-test` move the
    /// member it holds and a round be asked for; the second value then says
    /// whether the group wants its first round before it is used
    /// (`evaluate-before-use`).
    fn choose(
        &self,
        group: &str,
        ctx: &SelectCtx,
        live: bool,
        depth: usize,
    ) -> Option<(String, bool)> {
        let Some(Entry::Group { spec, members, .. }) = self.entries.get(group) else {
```

换成

```rust
    /// is asked for. `smart`: by what its book knows (M3c design 6.2), with
    /// the members to try next. `live` is a dial: only then may `url-test`
    /// move the member it holds and a round be asked for, and only then is
    /// it said whether the group wants its first round before it is used
    /// (`evaluate-before-use`).
    fn choose(&self, group: &str, ctx: &SelectCtx, live: bool, depth: usize) -> Option<Choice> {
        let Some(Entry::Group {
            spec,
            members,
            factors,
            ..
        }) = self.entries.get(group)
        else {
```

`crates/rurge-policy/src/registry.rs`——把

```rust
                    .map(|m| (m, false))
```

换成

```rust
                    .map(Choice::plain)
```

`crates/rurge-policy/src/registry.rs`——把

```rust
                    return Some((member, false));
```

换成

```rust
                    return Some(Choice::plain(member));
```

`crates/rurge-policy/src/registry.rs`——把

```rust
                Some((member, pending))
            }
            // `smart` (M3c) and `subnet` (phase 3): the first member
            GroupKind::Smart | GroupKind::Subnet => members.first().map(|m| (m.clone(), false)),
```

换成

```rust
                Some(Choice {
                    member,
                    pending,
                    retry: None,
                })
            }
            GroupKind::Smart => {
                // an override stands, asks for no test and has nobody to try
                // after it (M3c design 6.2)
                if let Some(member) = self.auto.override_of(group, members) {
                    return Some(Choice::plain(member));
                }
                let now = Instant::now();
                let candidates: Vec<Candidate<'_>> = members
                    .iter()
                    .zip(factors)
                    .map(|(m, &factor)| Candidate {
                        name: m,
                        health: self.smart_health(m, now),
                        factor,
                    })
                    .collect();
                if !live {
                    // the control plane's view: the most used lately, else the
                    // first in line (M3c design 8.3)
                    let member = self
                        .auto
                        .smart
                        .most_used(group, members, now)
                        .into_iter()
                        .next()
                        .or_else(|| {
                            rank(&candidates, &SiteMemory::default(), |_| 0).map(|r| r.pick)
                        })?;
                    return Some(Choice::plain(member));
                }
                let last = self.auto.last_round(group);
                let unknown = candidates
                    .iter()
                    .any(|c| matches!(c.health, Health::Unknown { .. }));
                if unknown || last.is_none_or(|t| t.elapsed() >= ROUND_INTERVAL) {
                    self.auto.wake(group);
                }
                let site = ctx
                    .host
                    .as_deref()
                    .map_or_else(SiteMemory::default, |host| self.auto.smart.site(host, now));
                let ranking = rank(&candidates, &site, random_below)?;
                Some(Choice {
                    member: ranking.pick,
                    pending: spec.test.evaluate_before_use && last.is_none(),
                    retry: Some(ranking.retry),
                })
            }
            // `subnet` (phase 3): the first member
            GroupKind::Subnet => members.first().cloned().map(Choice::plain),
        }
    }

    /// What the `smart` groups know of `member` now; a protocol not
    /// implemented yet never works (M3c design 6.1).
    fn smart_health(&self, member: &str, now: Instant) -> Health {
        match self.entries.get(member) {
            Some(Entry::Outbound { outbound, .. }) => self.auto.smart.health(member, outbound, now),
            _ => Health::Failed(None),
```

`crates/rurge-policy/src/registry.rs`——把

```rust
                cycle,
            }) = self.entries.get(&n)
```

换成

```rust
                cycle,
                ..
            }) = self.entries.get(&n)
```

`crates/rurge-policy/src/registry.rs`——把

```rust
                Some((member, _)) => self.standing(&member, depth + 1),
```

换成

```rust
                Some(choice) => self.standing(&choice.member, depth + 1),
```

`crates/rurge-policy/src/registry.rs`——把

```rust
    /// The members of `group` that pass their tests now.
```

换成

```rust
    /// The members of `group` that pass their tests now; of a `smart` group,
    /// those that are healthy (M3c design 5.4).
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        };
        members
```

换成

```rust
        };
        if spec.kind == GroupKind::Smart {
            let now = Instant::now();
            return members
                .iter()
                .filter(|m| matches!(self.smart_health(m, now), Health::Healthy(_)))
                .cloned()
                .collect();
        }
        members
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            &SelectCtx::default(),
        )
    }

    fn device(&self, name: &str, chain: &mut Vec<String>) -> Resolution {
```

换成

```rust
            &SelectCtx::default(),
        )
    }

    /// A member of a `smart` group on its own: what a dial tries after the
    /// member the group picked did not connect (M3c design §7). Its chain is
    /// the member alone.
    pub fn resolve_member(&self, member: &str) -> Resolution {
        self.named(
            member,
            &mut Vec::new(),
            0,
            self.empty_group,
            &SelectCtx::default(),
        )
    }

    fn device(&self, name: &str, chain: &mut Vec<String>) -> Resolution {
```

`crates/rurge-policy/src/registry.rs`——把

```rust
                Some((member, pending)) => {
                    let mut resolution = match PolicyRef::parse(&member) {
```

换成

```rust
                Some(choice) => {
                    let mut resolution = match PolicyRef::parse(&choice.member) {
```

`crates/rurge-policy/src/registry.rs`——把

```rust
                    if pending {
                        resolution.pending = Some(name.to_string());
```

换成

```rust
                    if choice.pending {
                        resolution.pending = Some(name.to_string());
                    }
                    if let Some(retry) = choice.retry {
                        resolution.smart = Some(SmartPick {
                            group: name.to_string(),
                            member: choice.member,
                            retry,
                        });
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            pending: None,
```

换成

```rust
            pending: None,
            smart: None,
```

`crates/rurge-policy/src/lib.rs`——把

```rust
pub use registry::{EmptyGroup, GroupInfo, Line, Note, PolicyRegistry, Resolution, TerminalKind};
```

换成

```rust
pub use registry::{
    EmptyGroup, GroupInfo, Line, Note, PolicyRegistry, Resolution, SmartPick, TerminalKind,
};
```

要点：
- `rank` 是纯函数：随机数由调用方给（`pick_at(n)` 返回 `n` 以内的下标）；注册表用 M3b `load-balance` 同样的取法（`RandomState::new().hash_one(Instant::now())`），不新增依赖。控制面的视图传 `|_| 0`、不带站点记忆。
- "最优者"：2 倍直选比的是全部健康成员里的最优者；1.2 倍的优选集比的是去掉"在这个主机上失败过"之后的最优者（至少包含它）。
- `report_success` / `report_failure` 在改记录前后比较状态，成员因此"变为失败"或"恢复"时记一行 INFO，只写策略名（P4）；`sample` / `failure`（测速推送走它们）不记。
- 站点：写入时先删掉同一策略的旧条目与超过 1 小时的条目再追加，超过 16 条删最旧的；满 4096 个主机再来新主机时删最久没用的（读也算用）。
- 注册表构建：`smart` 组的成员里只留 `Entry::Outbound { proxy: true }` 与 `Entry::Unsupported`（未实现的协议，恒为失败），被忽略的名字记一行 INFO；因子取 `spec.priority` 里第一个匹配成员名的（`fancy-regex` 匹配出错按不匹配），没有匹配为 1.0。这一切在空组判断之前做：过滤后为空就是空组。
- `choose` 改为返回私有的 `Choice { member, pending, retry }`，其它组的行为不变；`named()` 在 `retry` 有值时填 `Resolution.smart`。`standing`（`smart` 组作为别的组的成员时）按它在视图里的成员计分。
- 本任务之后，经 `smart` 组的拨号已按 `SmartBook` 选成员，但还不换成员、不回报（Task 5），能力表也还没翻转（Task 7）。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-policy` → 116 passed（`smart::tests` 新增 10 条：排序、优选集、因子、没有健康成员时、站点记忆的直选与挪末尾、1 小时、4096 个主机、每主机 16 个策略、使用计数、回报；`registry::tests` 新增 7 条：`a_smart_group_takes_proxies_only`、`a_dial_picks_by_the_smart_book_and_brings_the_others_along`、`a_member_on_its_own_resolves_to_itself`、`an_override_of_a_smart_group_stands_alone`、`a_dial_asks_for_a_round_while_a_member_is_unknown`、`the_view_of_a_smart_group_is_its_most_used_member`、`a_smart_group_is_available_by_its_health`）。

- [ ] **Step 5: 门禁与提交**

跑门禁（40 个测试二进制，965 通过 / 1 忽略）。

```bash
git add crates/rurge-policy/src/smart.rs crates/rurge-policy/src/registry.rs crates/rurge-policy/src/lib.rs
git commit -m "feat(policy): smart 组的选择——排序与重试列表、站点记忆、使用计数，注册表只留代理成员并预算 policy-priority 因子"
```

### Task 5: 引擎——拨号重试与质量回报

会话拨号经过 `smart` 组时（设计第 4、7 节）：选中的成员连不上，就按重试列表依次再试最多 2 个（`RETRIES`），整条会话的连接超时仍是 10 秒，每次尝试最多用"剩余时间 ÷ 剩余尝试次数"（M3c-D6，P6）；REJECT 与"协议尚未实现"不重试（P8）；每个失败的成员都回报给 `SmartBook`。连上以后，给会话挂一个质量探针（`watch`）：第一个上游字节是一个样本；出站就绪 3 秒后还没有首字节，或者上游在发出任何数据之前、客户端还在时就结束，是一次失败——三者取先到的，只报一次；`kill`、空闲超时、优雅退出与客户端先走都不算。DNS 会话只用选中的成员，照样回报（没有"上游先断"那一种，P3）；链的中间跳既不重试也不回报（M3c-D5）。

`SessionHandle`（`rurge-inbound`）为此提供通用的钩子与标记，它仍不认识 `smart`：结束钩子可挂多个，另有首字节钩子；`pump` 与明文 HTTP 转发在"上游先断"时标记（P18）。

**Files:**
- Modify: `crates/rurge-inbound/src/session.rs`（多个结束钩子、`on_first_byte`、`first_byte_seen`、`mark_upstream_failed` / `upstream_failed`）
- Modify: `crates/rurge-inbound/src/http.rs`（`forward` 拿不到响应头时标记"上游先断"）
- Modify: `crates/rurge-engine/src/relay.rs`（`copy_half` 的 `reader_done` 回调；`pump` 判断谁先结束）
- Create: `crates/rurge-engine/src/smart.rs`（`RETRIES`、`NO_RESPONSE`、`attempts`、`retryable`、`quoted`、`watch`，与用例）
- Modify: `crates/rurge-engine/src/lib.rs`（`mod smart;`）
- Modify: `crates/rurge-engine/src/engine.rs`（`Dialer::dial` 的尝试循环、`connect_through`；`dial_internal` 的回报）
- Test: `crates/rurge-engine/tests/smart.rs`（新建）

**Interfaces:**
- Consumes: Task 1 的 `mark_connected` / `mark_first_byte` / `first_byte_time`；Task 4 的 `Resolution.smart`、`SmartPick`、`resolve_member`、`SmartBook::{report_success, report_failure, used}`。
- Produces:
  - `SessionHandle::on_finish` 改为追加（全部按添加顺序运行一次）；`on_first_byte(&self, f: impl FnOnce(&SessionHandle) + Send + 'static)`（首字节已到时立即运行）、`first_byte_seen(&self) -> bool`、`mark_upstream_failed(&self)`、`upstream_failed(&self) -> bool`
  - `rurge_engine::smart`（crate 内）：`RETRIES: usize = 2`、`NO_RESPONSE: Duration = 3 秒`、`attempts(&PolicyRegistry, Resolution) -> Vec<Resolution>`、`retryable(&OutboundError) -> bool`、`quoted(&[String]) -> String`、`watch(&Arc<SessionHandle>, Arc<SmartBook>, policy: &str, outbound: &OutboundRef, host: &str)`

- [ ] **Step 1: 先写用例**

`crates/rurge-inbound/src/session.rs`——把

```rust
        assert!(h.first_byte_time().is_some());
    }

    #[tokio::test]
    async fn the_end_of_the_stream_is_no_first_byte() {
```

换成

```rust
        assert!(h.first_byte_time().is_some());
    }

    /// Every hook runs, once, in the order they came (M3c design 4.3: the
    /// engine hangs a `smart` group's report on the session after its log).
    #[test]
    fn every_finish_hook_runs_once_in_order() {
        let h = SessionHandle::new(5, SessionInfo::tcp(HostName::parse("a.test"), 443));
        let order = Arc::new(Mutex::new(Vec::new()));
        for n in 1..=2 {
            let order = order.clone();
            h.on_finish(move |_, _| order.lock().unwrap().push(n));
        }
        h.finish(SessionOutcome::Completed);
        h.finish(SessionOutcome::Completed);
        assert_eq!(*order.lock().unwrap(), [1, 2]);
    }

    /// A first-byte hook runs when the first byte comes back — right away
    /// when it already has — and once.
    #[test]
    fn a_first_byte_hook_runs_once_when_the_byte_is_back() {
        let h = SessionHandle::new(6, SessionInfo::tcp(HostName::parse("a.test"), 443));
        let runs = Arc::new(AtomicUsize::new(0));
        let r = runs.clone();
        h.on_first_byte(move |_| {
            r.fetch_add(1, Ordering::SeqCst);
        });
        assert!(!h.first_byte_seen());
        assert_eq!(runs.load(Ordering::SeqCst), 0);
        h.mark_first_byte();
        h.mark_first_byte();
        assert!(h.first_byte_seen());
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        let r = runs.clone();
        h.on_first_byte(move |_| {
            r.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(runs.load(Ordering::SeqCst), 2, "already back: runs at once");
    }

    #[test]
    fn upstream_failing_is_marked() {
        let h = SessionHandle::new(7, SessionInfo::tcp(HostName::parse("a.test"), 443));
        assert!(!h.upstream_failed());
        h.mark_upstream_failed();
        assert!(h.upstream_failed());
    }

    #[tokio::test]
    async fn the_end_of_the_stream_is_no_first_byte() {
```

`crates/rurge-engine/src/relay.rs`——把

```rust
        task.await.unwrap();
    }
```

换成

```rust
        task.await.unwrap();
    }

    /// Upstream ending before it answered, while the client waits, is
    /// upstream's failure (M3c design 4.3).
    #[tokio::test]
    async fn upstream_ending_silently_while_the_client_waits_is_marked() {
        let (ca, client_b) = tokio::io::duplex(1024);
        let (ua, upstream_b) = tokio::io::duplex(1024);
        let h = handle();
        let task = tokio::spawn(pump(
            Box::new(client_b),
            Box::new(upstream_b),
            h.clone(),
            Duration::from_secs(30),
        ));
        drop(ua);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !h.upstream_failed() {
            assert!(Instant::now() < deadline, "not marked");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        drop(ca);
        task.await.unwrap();
    }

    /// The client leaving first is no failure of upstream's, and neither is
    /// upstream ending after it answered.
    #[tokio::test]
    async fn the_client_leaving_first_or_an_answer_is_no_upstream_failure() {
        let (ca, client_b) = tokio::io::duplex(1024);
        let (mut ua, upstream_b) = tokio::io::duplex(1024);
        let h = handle();
        let task = tokio::spawn(pump(
            Box::new(client_b),
            Box::new(upstream_b),
            h.clone(),
            Duration::from_secs(30),
        ));
        drop(ca);
        let mut buf = [0u8; 1];
        assert_eq!(
            ua.read(&mut buf).await.unwrap(),
            0,
            "the half-close came through"
        );
        drop(ua);
        task.await.unwrap();
        assert!(!h.upstream_failed());

        let (mut ca, client_b) = tokio::io::duplex(1024);
        let (mut ua, upstream_b) = tokio::io::duplex(1024);
        let h = handle();
        let task = tokio::spawn(pump(
            Box::new(client_b),
            Box::new(upstream_b),
            h.clone(),
            Duration::from_secs(30),
        ));
        ua.write_all(b"hi").await.unwrap();
        let mut buf = [0u8; 2];
        ca.read_exact(&mut buf).await.unwrap();
        drop(ua);
        drop(ca);
        task.await.unwrap();
        assert!(!h.upstream_failed());
    }
```

`crates/rurge-engine/src/relay.rs`——把

```rust
                    None,
```

换成

```rust
                    None,
                    || {},
```

新建 `crates/rurge-engine/tests/smart.rs`：

```rust
//! `smart` groups through the whole engine (phase 2 M3c design §4, §7, §11):
//! a dial that does not connect through the member the group picked goes on
//! to the next ones in line, and every session tells the group's book how the
//! member it used did.

mod common;
use common::*;
use rurge_config::HostName;
use rurge_config::session::SessionInfo;
use rurge_inbound::{DialError, Dialer};
use std::time::Instant;

/// A loopback port nothing listens on.
async fn closed_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap().port()
}

fn session(host: &str) -> SessionInfo {
    SessionInfo::tcp(HostName::parse(host), 80)
}

/// The chain of a dial of `host` that connects.
async fn chain_of(h: &Harness, host: &str) -> Vec<String> {
    match h.engine.dial(session(host)).await {
        Ok(dialed) => dialed.handle.policy_chain(),
        Err(DialError::Failed { message, .. }) => panic!("{host}: {message}"),
        Err(DialError::Reject { kind, .. }) => panic!("{host}: rejected by {}", kind.name()),
    }
}

/// A SOCKS5 upstream that reaches `origin` whatever it is asked for.
async fn upstream(origin: &TestServer) -> FakeSocks5 {
    FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(origin)),
        ..Socks5Script::default()
    })
    .await
}

/// A member that does not connect hands the session over to the next one in
/// line: the session goes through that one, and the note says who failed
/// (M3c design §7).
#[tokio::test]
async fn a_member_that_does_not_connect_hands_over_to_the_next() {
    let origin = TestServer::spawn().await;
    let good = upstream(&origin).await;
    let proxies = format!(
        "Dead = socks5, 127.0.0.1, {}\nGood = socks5, 127.0.0.1, {}",
        closed_port().await,
        good.addr().port()
    );
    let h = harness(Profile {
        proxies: &proxies,
        groups: "S = smart, Dead, Good",
        rules: "DOMAIN,target.test,S",
        ..Profile::default()
    })
    .await;
    // nothing known yet: Dead comes first, in member order
    let dialed = match h.engine.dial(session("target.test")).await {
        Ok(dialed) => dialed,
        Err(_) => panic!("the dial goes through Good"),
    };
    assert_eq!(dialed.handle.policy_chain(), ["S", "Good"]);
    assert_eq!(
        dialed.handle.error().as_deref(),
        Some("smart group `S`: `Dead` failed to connect, used `Good`")
    );
    let site = h
        .engine
        .registry()
        .auto()
        .smart
        .site("target.test", Instant::now());
    assert_eq!(site.failed, ["Dead"]);
}

/// With no member connecting, the session fails once the one picked and the
/// next two have been tried — the fourth is not — and the note names them.
#[tokio::test]
async fn when_no_member_connects_the_session_fails_naming_them() {
    let mut proxies = String::new();
    for name in ["A", "B", "C", "D"] {
        proxies += &format!("{name} = socks5, 127.0.0.1, {}\n", closed_port().await);
    }
    let h = harness(Profile {
        proxies: &proxies,
        groups: "S = smart, A, B, C, D",
        rules: "DOMAIN,target.test,S",
        ..Profile::default()
    })
    .await;
    let begun = Instant::now();
    let handle = match h.engine.dial(session("target.test")).await {
        Err(DialError::Failed { handle, .. }) => handle,
        Err(DialError::Reject { .. }) => panic!("rejected"),
        Ok(_) => panic!("nothing listens"),
    };
    assert!(
        begun.elapsed() < Duration::from_secs(11),
        "{:?}",
        begun.elapsed()
    );
    let note = handle.error().unwrap_or_default();
    assert!(
        note.starts_with("smart group `S`: tried `A`, `B`, `C`; "),
        "{note}"
    );
    assert_eq!(handle.policy_chain(), ["S", "C"]);
}

/// A session that gets its first byte back is a sample of the member it
/// used, and the member worked at the site (M3c design 4.3).
#[tokio::test]
async fn the_first_byte_back_is_reported() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi");
    let good = upstream(&origin).await;
    let proxies = format!("Good = socks5, 127.0.0.1, {}", good.addr().port());
    let h = harness(Profile {
        proxies: &proxies,
        groups: "S = smart, Good",
        rules: "DOMAIN,target.test,S",
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:80").await;
    let body = get(&mut tunnel, "target.test", "/hello").await;
    assert!(body.ends_with("hi"), "{body}");
    let registry = h.engine.registry();
    let smart = &registry.auto().smart;
    wait_until("the report of the first byte", || {
        smart.site("target.test", Instant::now()).worked == ["Good"]
    })
    .await;
    let good = outbound_now(&h, "Good");
    assert!(matches!(
        smart.health("Good", &good, Instant::now()),
        rurge_policy::smart::Health::Healthy(_)
    ));
}

/// A member that never answers the handshake has its share of the ten
/// seconds, and no more: two members, five seconds each (M3c design §7).
#[tokio::test]
async fn a_member_that_never_answers_has_its_share_of_the_time() {
    let origin = TestServer::spawn().await;
    let good = upstream(&origin).await;
    // takes the connection, and answers the CONNECT a minute later
    let hole = FakeHttpProxy::spawn(HttpProxyScript {
        delay: Duration::from_secs(60),
        ..HttpProxyScript::default()
    })
    .await;
    let proxies = format!(
        "Hole = http, 127.0.0.1, {}\nGood = socks5, 127.0.0.1, {}",
        hole.addr().port(),
        good.addr().port()
    );
    let h = harness(Profile {
        proxies: &proxies,
        groups: "S = smart, Hole, Good",
        rules: "DOMAIN,target.test,S",
        ..Profile::default()
    })
    .await;
    let begun = Instant::now();
    assert_eq!(chain_of(&h, "target.test").await, ["S", "Good"]);
    let took = begun.elapsed();
    assert!(
        took >= Duration::from_secs(4) && took < Duration::from_secs(8),
        "{took:?}"
    );
}

/// A session whose first byte does not come back within three seconds of
/// its outbound being ready counts against the member; the session itself
/// goes on (M3c design 4.3).
#[tokio::test]
async fn three_seconds_without_an_answer_count_against_the_member() {
    // takes the connection and never says a word
    let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let silent_addr = silent.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((s, _)) = silent.accept().await {
            held.push(s);
        }
    });
    let quiet = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(silent_addr),
        ..Socks5Script::default()
    })
    .await;
    let proxies = format!("Quiet = socks5, 127.0.0.1, {}", quiet.addr().port());
    let h = harness(Profile {
        proxies: &proxies,
        groups: "S = smart, Quiet",
        rules: "DOMAIN,target.test,S",
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:80").await;
    tunnel
        .write_all(b"GET / HTTP/1.1\r\nHost: target.test\r\n\r\n")
        .await
        .unwrap();
    let registry = h.engine.registry();
    let smart = &registry.auto().smart;
    wait_until("the member counted as not answering", || {
        smart.site("target.test", Instant::now()).failed == ["Quiet"]
    })
    .await;
    assert!(
        h.engine
            .request_log()
            .active()
            .iter()
            .any(|r| r.dst == "target.test:80"),
        "the session goes on"
    );
}

/// A plain request whose upstream hangs up before answering counts against
/// the member (M3c design 4.3).
#[tokio::test]
async fn a_plain_request_whose_upstream_hangs_up_counts_against_the_member() {
    // an HTTP proxy that reads the request and hangs up
    let rude = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = rude.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = rude.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf).await;
            });
        }
    });
    let proxies = format!("Rude = http, 127.0.0.1, {port}");
    let h = harness(Profile {
        proxies: &proxies,
        groups: "S = smart, Rude",
        rules: "DOMAIN,target.test,S",
        ..Profile::default()
    })
    .await;
    let _ = plain_get(h.http(), "http://target.test/", "target.test").await;
    let registry = h.engine.registry();
    let smart = &registry.auto().smart;
    wait_until("the member counted as failing", || {
        smart.site("target.test", Instant::now()).failed == ["Rude"]
    })
    .await;
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-engine --test smart`
Expected: FAIL，6 条全部失败——

```text
test a_member_that_does_not_connect_hands_over_to_the_next ... FAILED
test when_no_member_connects_the_session_fails_naming_them ... FAILED
test a_plain_request_whose_upstream_hangs_up_counts_against_the_member ... FAILED
test the_first_byte_back_is_reported ... FAILED
test three_seconds_without_an_answer_count_against_the_member ... FAILED
test a_member_that_never_answers_has_its_share_of_the_time ... FAILED
thread 'a_member_that_does_not_connect_hands_over_to_the_next' panicked at crates\rurge-engine\tests\smart.rs:63:19:
thread 'when_no_member_connects_the_session_fails_naming_them' panicked at crates\rurge-engine\tests\smart.rs:106:5:
thread 'a_plain_request_whose_upstream_hangs_up_counts_against_the_member' panicked at crates\rurge-engine\tests\common\mod.rs:190:9:
timed out waiting for the member counted as failing
thread 'the_first_byte_back_is_reported' panicked at crates\rurge-engine\tests\common\mod.rs:190:9:
timed out waiting for the report of the first byte
thread 'three_seconds_without_an_answer_count_against_the_member' panicked at crates\rurge-engine\tests\common\mod.rs:190:9:
timed out waiting for the member counted as not answering
thread 'a_member_that_never_answers_has_its_share_of_the_time' panicked at crates\rurge-engine\tests\smart.rs:27:51:
target.test: connect timed out
test result: FAILED. 0 passed; 6 failed; 0 ignored; 0 measured; 0 filtered out
```

（`rurge-inbound` 与 `rurge-engine` 的单元用例此时编译不过：`on_first_byte`、`mark_upstream_failed` 等尚未定义。）

- [ ] **Step 3: 实现**

`crates/rurge-inbound/src/session.rs`——把

```rust
type FinishHook = Box<dyn FnOnce(&SessionHandle, &SessionOutcome) + Send>;
```

换成

```rust
type FinishHook = Box<dyn FnOnce(&SessionHandle, &SessionOutcome) + Send>;
type FirstByteHook = Box<dyn FnOnce(&SessionHandle) + Send>;
```

`crates/rurge-inbound/src/session.rs`——把

```rust
    finished: AtomicBool,
    outcome: Mutex<Option<SessionOutcome>>,
    on_finish: Mutex<Option<FinishHook>>,
```

换成

```rust
    on_first_byte: Mutex<Vec<FirstByteHook>>,
    /// Upstream ended, before sending anything, while the client was still
    /// there (`mark_upstream_failed`).
    upstream_failed: AtomicBool,
    finished: AtomicBool,
    outcome: Mutex<Option<SessionOutcome>>,
    on_finish: Mutex<Vec<FinishHook>>,
```

`crates/rurge-inbound/src/session.rs`——把

```rust
            finished: AtomicBool::new(false),
            outcome: Mutex::new(None),
            on_finish: Mutex::new(None),
```

换成

```rust
            on_first_byte: Mutex::new(Vec::new()),
            upstream_failed: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            outcome: Mutex::new(None),
            on_finish: Mutex::new(Vec::new()),
```

`crates/rurge-inbound/src/session.rs`——把

```rust
    /// goes on to the client. Only the first call counts.
    pub fn mark_first_byte(&self) {
        let _ = self.first_byte.set(self.started.elapsed());
```

换成

```rust
    /// goes on to the client. Only the first call counts; it runs the hooks
    /// of `on_first_byte`.
    pub fn mark_first_byte(&self) {
        if self.first_byte.set(self.started.elapsed()).is_ok() {
            let hooks = std::mem::take(&mut *self.on_first_byte.lock().expect("first byte hooks"));
            for hook in hooks {
                hook(self);
            }
        }
    }

    /// Whether a byte came back from upstream yet.
    pub fn first_byte_seen(&self) -> bool {
        self.first_byte.get().is_some()
    }

    /// Runs `f` once the first byte is back — right away when it is already.
    pub fn on_first_byte(&self, f: impl FnOnce(&SessionHandle) + Send + 'static) {
        let mut hooks = self.on_first_byte.lock().expect("first byte hooks");
        if self.first_byte_seen() {
            drop(hooks);
            f(self);
        } else {
            hooks.push(Box::new(f));
        }
    }

    /// Upstream closed or failed before sending anything back, while the
    /// client was still there: the member of a `smart` group that carried
    /// the session is to blame (phase 2 M3c design 4.3).
    pub fn mark_upstream_failed(&self) {
        self.upstream_failed.store(true, Ordering::Release);
    }

    pub fn upstream_failed(&self) -> bool {
        self.upstream_failed.load(Ordering::Acquire)
```

`crates/rurge-inbound/src/session.rs`——把

```rust
    /// Installs the hook `finish` runs once (the engine's session log).
    pub fn on_finish(&self, f: impl FnOnce(&SessionHandle, &SessionOutcome) + Send + 'static) {
        *self.on_finish.lock().expect("finish hook") = Some(Box::new(f));
```

换成

```rust
    /// Adds a hook `finish` runs, once, after the ones added before it (the
    /// engine's session log first).
    pub fn on_finish(&self, f: impl FnOnce(&SessionHandle, &SessionOutcome) + Send + 'static) {
        self.on_finish
            .lock()
            .expect("finish hooks")
            .push(Box::new(f));
```

`crates/rurge-inbound/src/session.rs`——把

```rust
        let hook = self.on_finish.lock().expect("finish hook").take();
        if let Some(hook) = hook {
```

换成

```rust
        let hooks = std::mem::take(&mut *self.on_finish.lock().expect("finish hooks"));
        for hook in hooks {
```

`crates/rurge-inbound/src/http.rs`——把

```rust
            Ok(resp.map(|body| body.boxed()))
        }
        Err(e) => {
            handle.finish(SessionOutcome::Failed(format!(
```

换成

```rust
            Ok(resp.map(|body| body.boxed()))
        }
        Err(e) => {
            // no response head: upstream gave up while the client waits
            handle.mark_upstream_failed();
            handle.finish(SessionOutcome::Failed(format!(
```

`crates/rurge-engine/src/relay.rs`——把

```rust
use std::sync::atomic::{AtomicU64, Ordering};
```

换成

```rust
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
```

`crates/rurge-engine/src/relay.rs`——把

```rust
        }));
        async move {
```

换成

```rust
        }));
        // Upstream ending before it sent anything, while the client is still
        // there, is upstream's failure (M3c design 4.3); once the client's
        // side has ended, it is not.
        let client_done = Arc::new(AtomicBool::new(false));
        let c_up = client_done.clone();
        let h_end = handle.clone();
        async move {
```

`crates/rurge-engine/src/relay.rs`——把

```rust
                    sniff_first,
```

换成

```rust
                    sniff_first,
                    move || c_up.store(true, Ordering::Release),
```

`crates/rurge-engine/src/relay.rs`——把

```rust
                    first_byte,
```

换成

```rust
                    first_byte,
                    move || {
                        if !client_done.load(Ordering::Acquire) && !h_end.first_byte_seen() {
                            h_end.mark_upstream_failed();
                        }
                    },
```

`crates/rurge-engine/src/relay.rs`——把

```rust
/// onward) and is then consumed.
```

换成

```rust
/// onward) and is then consumed; `reader_done` runs when the reader ends —
/// at EOF or on a read error, not when `stop` ends the direction.
#[allow(clippy::too_many_arguments)]
```

`crates/rurge-engine/src/relay.rs`——把

```rust
    mut first: Option<FirstChunkHook>,
```

换成

```rust
    mut first: Option<FirstChunkHook>,
    reader_done: impl Fn(),
```

`crates/rurge-engine/src/relay.rs`——把

```rust
            r = reader.read(&mut buf) => r?,
        };
        if n == 0 {
```

换成

```rust
            r = reader.read(&mut buf) => match r {
                Ok(n) => n,
                Err(e) => {
                    reader_done();
                    return Err(e);
                }
            },
        };
        if n == 0 {
            reader_done();
```

新建 `crates/rurge-engine/src/smart.rs`：

```rust
//! The engine's side of the `smart` groups (phase 2 M3c design §4, §7): a
//! dial that does not connect through the member a group picked tries the
//! next ones in line, and every session through such a group reports what it
//! saw of the member it went through.

use rurge_inbound::SessionHandle;
use rurge_policy::smart::SmartBook;
use rurge_policy::{PolicyRegistry, Resolution, SmartPick, TerminalKind};
use rurge_proto::{OutboundError, OutboundRef};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Members a dial tries after the one a `smart` group picked (M3c design §9).
pub(crate) const RETRIES: usize = 2;
/// A session through a `smart` group without a byte back this long after its
/// outbound was ready counts against the member (M3c design 4.3).
pub(crate) const NO_RESPONSE: Duration = Duration::from_secs(3);

/// The dials of a session, in order (M3c design §7): through what `first`
/// picked and — when a `smart` group picked it — through the next members in
/// line, at most `RETRIES` of them, each with the chain it would have had. A
/// member that is no proxy (its protocol is not implemented yet) is not
/// tried.
pub(crate) fn attempts(registry: &PolicyRegistry, first: Resolution) -> Vec<Resolution> {
    let Some(pick) = first.smart.clone() else {
        return vec![first];
    };
    let prefix: Vec<String> = first
        .chain
        .iter()
        .take_while(|name| **name != pick.group)
        .cloned()
        .chain([pick.group.clone()])
        .collect();
    let mut out = vec![first];
    for member in &pick.retry {
        if out.len() > RETRIES {
            break;
        }
        let mut next = registry.resolve_member(member);
        if next.terminal != TerminalKind::Proxy {
            continue;
        }
        let mut chain = prefix.clone();
        chain.append(&mut next.chain);
        next.chain = chain;
        next.smart = Some(SmartPick {
            group: pick.group.clone(),
            member: member.clone(),
            retry: Vec::new(),
        });
        out.push(next);
    }
    out
}

/// Whether another member may do better: the connection failed — unlike a
/// REJECT, or a protocol not implemented yet.
pub(crate) fn retryable(e: &OutboundError) -> bool {
    !matches!(e, OutboundError::Reject(_) | OutboundError::Unsupported(_))
}

/// `members`, quoted the way the session log's notes quote names.
pub(crate) fn quoted(members: &[String]) -> String {
    members
        .iter()
        .map(|m| format!("`{m}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// What a session tells the book of the member a `smart` group gave it:
/// once, whichever comes first.
struct Watch {
    book: Arc<SmartBook>,
    policy: String,
    outbound: OutboundRef,
    host: String,
    told: AtomicBool,
}

impl Watch {
    fn success(&self, latency: Duration) {
        if !self.told.swap(true, Ordering::AcqRel) {
            let host = Some(self.host.as_str());
            self.book
                .report_success(&self.policy, &self.outbound, host, latency, Instant::now());
        }
    }

    fn failure(&self) {
        if !self.told.swap(true, Ordering::AcqRel) {
            let host = Some(self.host.as_str());
            self.book
                .report_failure(&self.policy, &self.outbound, host, Instant::now());
        }
    }
}

/// Hangs the report of `policy` on `handle` (M3c design 4.3): the first byte
/// back is a sample; `NO_RESPONSE` without one, or upstream ending before it
/// sent anything while the client was still there, is a failure — whichever
/// comes first, once. A `kill` and a shutdown say nothing of the member.
pub(crate) fn watch(
    handle: &Arc<SessionHandle>,
    book: Arc<SmartBook>,
    policy: &str,
    outbound: &OutboundRef,
    host: &str,
) {
    let watch = Arc::new(Watch {
        book,
        policy: policy.to_string(),
        outbound: outbound.clone(),
        host: host.to_string(),
        told: AtomicBool::new(false),
    });
    let on_byte = watch.clone();
    handle.on_first_byte(move |h| {
        if let Some(latency) = h.first_byte_time() {
            on_byte.success(latency);
        }
    });
    let on_end = watch.clone();
    handle.on_finish(move |h, _| {
        if h.upstream_failed() && !h.first_byte_seen() && !h.was_killed() {
            on_end.failure();
        }
    });
    let session = Arc::downgrade(handle);
    tokio::spawn(async move {
        tokio::time::sleep(NO_RESPONSE).await;
        if let Some(h) = session.upgrade()
            && !h.first_byte_seen()
            && !h.is_finished()
        {
            watch.failure();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::HostName;
    use rurge_config::session::SessionInfo;
    use rurge_inbound::SessionOutcome;
    use rurge_policy::smart::{Health, SiteMemory};
    use rurge_proto::{Reject, RejectKind};

    fn outbound() -> OutboundRef {
        Arc::new(Reject::new(RejectKind::Reject))
    }

    fn session() -> Arc<SessionHandle> {
        let h = SessionHandle::new(1, SessionInfo::tcp(HostName::parse("a.test"), 443));
        h.mark_connected();
        h
    }

    /// The first byte back is a sample of the member, and the member worked
    /// at the site; nothing after it counts (M3c design 4.3).
    #[tokio::test]
    async fn the_first_byte_is_the_one_report() {
        let (book, a, h) = (Arc::new(SmartBook::new()), outbound(), session());
        watch(&h, book.clone(), "A", &a, "a.test");
        h.mark_first_byte();
        let now = Instant::now();
        assert!(matches!(book.health("A", &a, now), Health::Healthy(_)));
        assert_eq!(book.site("a.test", now).worked, ["A"]);
        h.mark_upstream_failed();
        h.finish(SessionOutcome::Completed);
        assert!(matches!(book.health("A", &a, now), Health::Healthy(_)));
    }

    /// Three seconds without a byte back is a failure; the session goes on.
    #[tokio::test(start_paused = true)]
    async fn three_silent_seconds_are_a_failure() {
        let (book, a, h) = (Arc::new(SmartBook::new()), outbound(), session());
        watch(&h, book.clone(), "A", &a, "a.test");
        tokio::time::sleep(NO_RESPONSE + Duration::from_millis(10)).await;
        let now = Instant::now();
        assert_eq!(book.health("A", &a, now), Health::Unknown { failures: 1 });
        assert_eq!(book.site("a.test", now).failed, ["A"]);
        assert!(!h.is_finished());
    }

    /// Upstream ending before it answered is a failure; a session that was
    /// killed, or that just ended, says nothing.
    #[tokio::test]
    async fn upstream_ending_first_is_a_failure_and_a_kill_is_not() {
        let book = Arc::new(SmartBook::new());
        let (a, b, c) = (outbound(), outbound(), outbound());
        let failed = session();
        watch(&failed, book.clone(), "A", &a, "a.test");
        failed.mark_upstream_failed();
        failed.finish(SessionOutcome::Failed("upstream closed".into()));
        let killed = session();
        watch(&killed, book.clone(), "B", &b, "a.test");
        killed.kill();
        killed.mark_upstream_failed();
        killed.finish(SessionOutcome::Failed("killed".into()));
        let ended = session();
        watch(&ended, book.clone(), "C", &c, "a.test");
        ended.finish(SessionOutcome::Completed);
        let now = Instant::now();
        assert_eq!(book.health("A", &a, now), Health::Unknown { failures: 1 });
        assert_eq!(book.health("B", &b, now), Health::Unknown { failures: 0 });
        assert_eq!(book.health("C", &c, now), Health::Unknown { failures: 0 });
        assert_eq!(
            book.site("a.test", now),
            SiteMemory {
                worked: Vec::new(),
                failed: vec!["A".to_string()],
            }
        );
    }

    #[test]
    fn a_connect_failure_may_go_to_the_next_member_and_a_reject_may_not() {
        assert!(retryable(&OutboundError::Timeout));
        assert!(retryable(&OutboundError::Proxy("refused".into())));
        assert!(!retryable(&OutboundError::Reject(RejectKind::Reject)));
        assert!(!retryable(&OutboundError::Unsupported("hysteria2".into())));
        assert_eq!(quoted(&["A".into(), "B".into()]), "`A`, `B`");
    }
}
```

`crates/rurge-engine/src/lib.rs`——把

```rust
pub mod shared;
```

换成

```rust
pub mod shared;
mod smart;
```

`crates/rurge-engine/src/engine.rs`——把

```rust
use rurge_proto::OutboundError;
```

换成

```rust
use rurge_proto::{OutboundError, OutboundRef};
```

`crates/rurge-engine/src/engine.rs`——把

```rust
        match resolution.outbound.connect_tcp(&target, &opts).await {
```

换成

```rust
        let connected = resolution.outbound.connect_tcp(&target, &opts).await;
        // the member of a `smart` group reports how it went (M3c design 4.3);
        // a DNS session tries no other member (M3c-D5)
        if let Some(pick) = &resolution.smart {
            let (book, now) = (&registry.auto().smart, Instant::now());
            let host = handle.session().dst_host.to_string();
            match &connected {
                Ok(_) => {
                    book.used(&pick.group, &pick.member, now);
                    let outbound = &resolution.outbound;
                    crate::smart::watch(&handle, book.clone(), &pick.member, outbound, &host);
                }
                Err(e) if crate::smart::retryable(e) => {
                    book.report_failure(&pick.member, &resolution.outbound, Some(&host), now);
                }
                Err(_) => {}
            }
        }
        match connected {
```

`crates/rurge-engine/src/engine.rs`——把

```rust
    })
}

fn reject(handle: Arc<SessionHandle>, kind: rurge_proto::RejectKind) -> Result<Dialed, DialError> {
```

换成

```rust
    })
}

/// Connects `outbound` to `target`: a plain request goes in absolute form to
/// an HTTP proxy when `may_forward` (M1 design 6.5), anything else through a
/// tunnel.
async fn connect_through(
    outbound: &OutboundRef,
    target: &Target,
    may_forward: bool,
    opts: &ConnectOpts,
) -> Result<(BoxedStream, Option<Vec<(String, String)>>), OutboundError> {
    match outbound.http_forward().filter(|_| may_forward) {
        Some(proxy) if rurge_proto::http::valid_target(target) => proxy
            .connect(opts)
            .await
            .map(|stream| (stream, Some(proxy.request_headers()))),
        Some(_) => Err(OutboundError::Proxy(
            "the target host name is not valid for an HTTP proxy request".to_string(),
        )),
        None => outbound
            .connect_tcp(target, opts)
            .await
            .map(|stream| (stream, None)),
    }
}

fn reject(handle: Arc<SessionHandle>, kind: rurge_proto::RejectKind) -> Result<Dialed, DialError> {
```

`crates/rurge-engine/src/engine.rs`——把

```rust
                Target::new(handle.session().dst_host.clone(), handle.session().dst_port);
            let opts = ConnectOpts {
                timeout: CONNECT_TIMEOUT,
            };
```

换成

```rust
                Target::new(handle.session().dst_host.clone(), handle.session().dst_port);
```

`crates/rurge-engine/src/engine.rs`——把

```rust
            let forward = if plain_http && !pinned {
                resolution.outbound.http_forward()
            } else {
                None
            };
            let connected = match forward {
                Some(proxy) if rurge_proto::http::valid_target(&target) => proxy
                    .connect(&opts)
                    .await
                    .map(|stream| (stream, Some(proxy.request_headers()))),
                Some(_) => Err(OutboundError::Proxy(
                    "the target host name is not valid for an HTTP proxy request".to_string(),
                )),
                None => resolution
                    .outbound
                    .connect_tcp(&target, &opts)
                    .await
                    .map(|stream| (stream, None)),
            };
            match connected {
                Ok((stream, forward)) => {
                    handle.mark_connected();
                    Ok(Dialed {
                        stream,
                        handle,
                        forward,
                    })
                }
                Err(OutboundError::Reject(kind)) => {
```

换成

```rust
            let may_forward = plain_http && !pinned;
            let host = handle.session().dst_host.to_string();
            let book = registry.auto().smart.clone();
            // the member a `smart` group picked, then the next ones in line
            // while they do not connect (phase 2 M3c design §7)
            let attempts = crate::smart::attempts(&registry, resolution);
            let begun = Instant::now();
            let mut tried: Vec<String> = Vec::new();
            let mut failure = None;
            for (k, attempt) in attempts.iter().enumerate() {
                if k > 0 {
                    handle.set_policy_chain(attempt.chain.clone());
                }
                // every try gets its share of the time that is left
                let timeout = match attempts.len() {
                    1 => CONNECT_TIMEOUT,
                    n => CONNECT_TIMEOUT.saturating_sub(begun.elapsed()) / (n - k) as u32,
                };
                let opts = ConnectOpts { timeout };
                let connecting = connect_through(&attempt.outbound, &target, may_forward, &opts);
                let connected = if attempts.len() == 1 {
                    connecting.await
                } else {
                    tokio::time::timeout(timeout, connecting)
                        .await
                        .unwrap_or(Err(OutboundError::Timeout))
                };
                let e = match connected {
                    Ok((stream, forward)) => {
                        handle.mark_connected();
                        if let Some(pick) = &attempt.smart {
                            book.used(&pick.group, &pick.member, Instant::now());
                            let outbound = &attempt.outbound;
                            crate::smart::watch(
                                &handle,
                                book.clone(),
                                &pick.member,
                                outbound,
                                &host,
                            );
                            if !tried.is_empty() {
                                handle.set_error(format!(
                                    "smart group `{}`: {} failed to connect, used `{}`",
                                    pick.group,
                                    crate::smart::quoted(&tried),
                                    pick.member
                                ));
                            }
                        }
                        return Ok(Dialed {
                            stream,
                            handle,
                            forward,
                        });
                    }
                    Err(e) => e,
                };
                let retry = crate::smart::retryable(&e);
                if let Some(pick) = &attempt.smart
                    && retry
                {
                    let (outbound, now) = (&attempt.outbound, Instant::now());
                    book.report_failure(&pick.member, outbound, Some(&host), now);
                    tried.push(pick.member.clone());
                }
                failure = Some(e);
                if !retry {
                    break;
                }
            }
            if tried.len() > 1
                && let Some(pick) = attempts.first().and_then(|a| a.smart.as_ref())
            {
                handle.set_error(format!(
                    "smart group `{}`: tried {}",
                    pick.group,
                    crate::smart::quoted(&tried)
                ));
            }
            match failure.expect("a dial was tried") {
                OutboundError::Reject(kind) => {
```

`crates/rurge-engine/src/engine.rs`——把

```rust
                Err(OutboundError::Unsupported(_)) => {
                    reject(handle, rurge_proto::RejectKind::Reject)
                }
                Err(OutboundError::Dns(m)) => fail(handle, FailKind::Dns, m),
                Err(OutboundError::Io(e)) => fail(handle, FailKind::Connect, e.to_string()),
                Err(OutboundError::Timeout) => fail(handle, FailKind::Timeout, "connect timed out"),
                Err(
                    e @ (OutboundError::Proxy(_)
                    | OutboundError::Tls(_)
                    | OutboundError::Unavailable(_)),
                ) => fail(handle, FailKind::Connect, e.to_string()),
```

换成

```rust
                OutboundError::Unsupported(_) => reject(handle, rurge_proto::RejectKind::Reject),
                OutboundError::Dns(m) => fail(handle, FailKind::Dns, m),
                OutboundError::Io(e) => fail(handle, FailKind::Connect, e.to_string()),
                OutboundError::Timeout => fail(handle, FailKind::Timeout, "connect timed out"),
                e @ (OutboundError::Proxy(_)
                | OutboundError::Tls(_)
                | OutboundError::Unavailable(_)) => fail(handle, FailKind::Connect, e.to_string()),
```

要点：
- **钩子的次序**：引擎在 `new_handle` 里装的请求记录钩子先装，质量探针在拨号选定之后才挂，所以记录先写、回报后到。`on_first_byte` 在锁内判断"首字节是否已到"再决定立即运行还是排队，`mark_first_byte` 先设值再取走排队的钩子，两者并发也不会漏掉钩子。
- **谁先结束**：`copy_half` 的 `reader_done` 只在读端结束（EOF 或读错误）时调用，`stop` 结束的不算。上行方向（读客户端）结束时记下"客户端已走"；下行方向（读上游）结束时，客户端还在、又没收到首字节，才 `mark_upstream_failed`。`forward` 里 `send_request` 出错（拿不到响应头）即标记——客户端断开会让整个处理函数被丢弃，走不到这里。
- **尝试循环**：`attempts` 把第一次的解析结果与重试列表里的成员（`resolve_member`，只取解析为代理的，最多 `RETRIES` 个）展开成带完整链的尝试序列：链的前缀截到 `smart` 组为止，再接上成员。循环里每次尝试前 `set_policy_chain`；有两个以上候选时每次用 `tokio::time::timeout` 包住（P6），`ConnectOpts.timeout` 设成同一个值；只有一个候选时与之前完全一样。连接方式的选择（明文 HTTP 请求能否以绝对形式交给 HTTP 代理）抽成 `connect_through`，每个候选各算一次。
- **成功**：`mark_connected`，经 `smart` 组的再记一次使用（`used`）并挂上 `watch`；换过成员时写备注（P9）。**失败**：可重试的错误先回报再试下一个；试过两个以上都失败时写"tried"备注，然后照 M1 的映射返回最后一次的错误（`fail` 会把原因接在备注后面）。
- **DNS 会话**：`dial_internal` 只用选中的成员，连上时同样记使用、挂 `watch`，可重试的连接失败回报一次。
- `watch` 的 3 秒定时是一个只持 `Weak<SessionHandle>` 的任务：到点时会话还在、没有首字节才报失败；会话已结束就什么也不做。
- 端到端用例的"黑洞"成员是 `HttpProxyScript { delay: 60 秒 }`：接受 TCP，一分钟后才回 CONNECT——只用回环。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-inbound` → 36 passed（新增 `every_finish_hook_runs_once_in_order`、`a_first_byte_hook_runs_once_when_the_byte_is_back`、`upstream_failing_is_marked`）。
Run: `cargo test -p rurge-engine --lib` → 56 passed; 1 ignored（新增 `relay` 2 条、`smart` 4 条）。
Run: `cargo test -p rurge-engine --test smart` → 6 passed（约 6 秒：Windows 上连回环里没人监听的端口要一两秒才被拒绝，黑洞成员要等满它的 5 秒）。

- [ ] **Step 5: 门禁与提交**

跑门禁（41 个测试二进制，980 通过 / 1 忽略）。

```bash
git add crates/rurge-inbound/src/session.rs crates/rurge-inbound/src/http.rs crates/rurge-engine/src/relay.rs crates/rurge-engine/src/smart.rs crates/rurge-engine/src/lib.rs crates/rurge-engine/src/engine.rs crates/rurge-engine/tests/smart.rs
git commit -m "feat(engine): smart 组的拨号重试与质量回报——按重试列表换成员（单次时限）、首字节 / 3 秒无响应 / 上游先断"
```

### Task 6: 测速节奏与抽样；`automatic()` 纳入 `smart`；发布注册表时清理

`smart` 组的测速沿用 M3b"拨号触发一轮"的机制，过期固定 5 分钟（Task 4 已按此请求一轮）；本任务补上抽样（设计 8.1）：成员超过 `ROUND_SAMPLE = 12` 的组，拨号请求的常规轮只测 12 个——最近 10 分钟用得最多的 6 个，加上最久没测过的（没测过的优先）补足 12 个；手动触发（`POST /v1/policy_groups/test`）测全部（P10）。控制面把 `smart` 当自动组对待：`select` 即临时覆盖、`test_results` 列出它（设计 8.3）。每次发布注册表时，`SmartBook` 丢掉已不存在的策略与组（P15）。

**Files:**
- Modify: `crates/rurge-policy/src/smart.rs`（`ROUND_SAMPLE`、`sample`）
- Modify: `crates/rurge-policy/src/registry.rs`（`test_round`、`sample_of`、`tested_at`、`round`；`round_timeout` 按常规轮实际要测的成员算）
- Modify: `crates/rurge-engine/src/auto.rs`（`automatic()` 纳入 `Smart`；调度任务改调 `test_round`）
- Modify: `crates/rurge-engine/src/engine.rs`（`publish_generation` 里 `SmartBook::retain`）、`crates/rurge-engine/src/subscriptions.rs`（订阅重建发布时同样）
- Test: 上述两个 `rurge-policy` 文件的单元用例；`crates/rurge-engine/tests/smart.rs`；`crates/rurge-engine/tests/outbounds.rs`（`only_a_member_can_be_selected` 改用 `subnet` 当"不接受选择"的组，P19）

**Interfaces:**
- Consumes: Task 4 的 `SmartBook::most_used`、`rank`；Task 5 的引擎 `smart` 模块。
- Produces:
  - `pub const ROUND_SAMPLE: usize = 12`、`pub fn sample(members: &[String], most_used: &[String], tested_at: impl Fn(&str) -> Option<Instant>) -> Vec<String>`
  - `PolicyRegistry::test_round(&self, group: &str) -> Vec<String>`（`async`；常规轮，大 `smart` 组抽样）；`test_group` 不变（测全部）

- [ ] **Step 1: 先写用例**

`crates/rurge-policy/src/smart.rs`——把

```rust
            names(&["A", "B", "C"])
        );
    }

    #[test]
    fn a_report_scores_the_member_as_well() {
```

换成

```rust
            names(&["A", "B", "C"])
        );
    }

    fn members(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("M{i}")).collect()
    }

    #[test]
    fn a_round_of_a_small_group_tests_every_member() {
        let all = members(ROUND_SAMPLE);
        assert_eq!(sample(&all, &[], |_| None), all);
    }

    /// Six of the most used, then those tested longest ago — never tested
    /// first — up to twelve (M3c design 8.1).
    #[test]
    fn a_round_of_a_big_group_tests_the_most_used_and_the_longest_untested() {
        let all = members(20);
        let t0 = Instant::now();
        let tested_at = |m: &str| {
            let i: u64 = m[1..].parse().unwrap();
            (i > 0).then(|| t0 + Duration::from_secs(i))
        };
        let used = names(&["M5", "M6", "M7", "M8", "M9", "M10", "M11", "M12"]);
        assert_eq!(
            sample(&all, &used, tested_at),
            names(&[
                "M5", "M6", "M7", "M8", "M9", "M10", "M0", "M1", "M2", "M3", "M4", "M11"
            ])
        );
        // fewer used ones: the untested fill the round
        let used = names(&["M19"]);
        let round = sample(&all, &used, tested_at);
        assert_eq!(round.len(), ROUND_SAMPLE);
        assert_eq!(round[..2], names(&["M19", "M0"]));
    }

    #[test]
    fn a_report_scores_the_member_as_well() {
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        assert_eq!(reg.resolve(&PolicyRef::parse("E")).pending, None);
    }

    #[test]
    fn selections_api() {
```

换成

```rust
        assert_eq!(reg.resolve(&PolicyRef::parse("E")).pending, None);
    }

    /// A round a dial asks for tests twelve members of a big `smart` group;
    /// one asked for by hand, all of them (M3c design 8.1).
    #[tokio::test]
    async fn a_regular_round_of_a_big_smart_group_tests_a_sample() {
        let mut profile = String::from(
            "[General]\nproxy-test-url = http://127.0.0.1:9/\ninternet-test-url = http://127.0.0.1:9/\n[Proxy]\n",
        );
        let names: Vec<String> = (0..ROUND_SAMPLE + 2).map(|i| format!("P{i}")).collect();
        for name in &names {
            profile += &format!("{name} = http, {name}.example, 80\n");
        }
        profile += &format!(
            "[Proxy Group]\nBig = smart, {}\n[Rule]\nFINAL,Big\n",
            names.join(", ")
        );
        let reg = generation(&profile, &FakeFactory::new(), None);
        let tested = |reg: &PolicyRegistry| {
            names
                .iter()
                .filter(|n| reg.test_result(n).is_some())
                .count()
        };
        reg.test_round("Big").await;
        assert_eq!(tested(&reg), ROUND_SAMPLE);
        assert!(reg.auto().last_round("Big").is_some());
        let timeout = reg.test_case("P0").unwrap().timeout;
        assert_eq!(
            reg.round_timeout("Big"),
            timeout * 2,
            "12 tests, 8 at a time"
        );
        reg.test_group("Big").await;
        assert_eq!(tested(&reg), names.len());
    }

    #[test]
    fn selections_api() {
```

`crates/rurge-engine/tests/smart.rs`——把

```rust
    .await;
}

```

换成

```rust
    .await;
}

/// A `smart` group is an automatic group to the control plane (M3c design
/// 8.3): `select` sets and clears an override, and the test endpoints take
/// it.
#[tokio::test]
async fn a_smart_group_takes_an_override_and_the_test_endpoints() {
    let origin = TestServer::spawn().await;
    let (a, b) = (upstream(&origin).await, upstream(&origin).await);
    let proxies = format!(
        "A = socks5, 127.0.0.1, {}\nB = socks5, 127.0.0.1, {}",
        a.addr().port(),
        b.addr().port()
    );
    let h = harness(Profile {
        proxies: &proxies,
        groups: "S = smart, A, B",
        rules: "DOMAIN,target.test,S",
        ..Profile::default()
    })
    .await;
    h.engine.select_group("S", "B").await.unwrap();
    assert_eq!(h.engine.group_selection("S").unwrap(), "B");
    assert_eq!(chain_of(&h, "target.test").await, ["S", "B"]);
    h.engine.select_group("S", "").await.unwrap();
    // both upstreams reach the origin: both pass, and both are healthy
    assert_eq!(h.engine.test_group("S").await.unwrap(), ["A", "B"]);
    let results = h.engine.test_results();
    let (_, members) = results
        .iter()
        .find(|(group, _)| group == "S")
        .expect("a smart group has test results");
    assert!(
        members
            .iter()
            .all(|(_, r)| r.as_ref().is_some_and(|r| r.outcome.is_ok())),
        "{members:?}"
    );
}

/// A round a dial asks for tests twelve members of a big `smart` group
/// (M3c design 8.1).
#[tokio::test]
async fn a_dial_asks_for_a_sampled_round_of_a_big_smart_group() {
    let origin = TestServer::spawn().await;
    let up = upstream(&origin).await;
    let names: Vec<String> = (0..13).map(|i| format!("P{i}")).collect();
    let proxies: String = names
        .iter()
        .map(|n| format!("{n} = socks5, 127.0.0.1, {}\n", up.addr().port()))
        .collect();
    let groups = format!("Big = smart, {}", names.join(", "));
    let h = harness(Profile {
        proxies: &proxies,
        groups: &groups,
        rules: "DOMAIN,target.test,Big",
        ..Profile::default()
    })
    .await;
    let _ = chain_of(&h, "target.test").await;
    wait_until("a round of Big", || {
        h.engine.registry().auto().last_round("Big").is_some()
    })
    .await;
    let registry = h.engine.registry();
    let tested = names
        .iter()
        .filter(|n| registry.test_result(n).is_some())
        .count();
    assert_eq!(tested, 12);
}

/// A reload that drops a policy drops what the book knew of it (M3c design
/// 5.1).
#[tokio::test]
async fn a_reload_forgets_what_the_book_knew_of_the_policies_it_drops() {
    let origin = TestServer::spawn().await;
    let (a, b) = (upstream(&origin).await, upstream(&origin).await);
    let proxies = format!(
        "A = socks5, 127.0.0.1, {}\nB = socks5, 127.0.0.1, {}",
        a.addr().port(),
        b.addr().port()
    );
    let h = harness(Profile {
        proxies: &proxies,
        groups: "S = smart, A, B",
        ..Profile::default()
    })
    .await;
    let registry = h.engine.registry();
    let smart = &registry.auto().smart;
    let b_now = outbound_now(&h, "B");
    let now = Instant::now();
    smart.sample("B", &b_now, Duration::from_millis(50), now);
    let text = std::fs::read_to_string(h.dir.path().join("t.conf")).unwrap();
    let fewer = text
        .replace(&format!("B = socks5, 127.0.0.1, {}\n", b.addr().port()), "")
        .replace("S = smart, A, B", "S = smart, A");
    h.engine
        .swap_runtime(runtime(h.dir.path(), &fewer, h.engine.shared()).await);
    assert_eq!(
        smart.health("B", &b_now, now),
        rurge_policy::smart::Health::Unknown { failures: 0 }
    );
}

```

`crates/rurge-engine/tests/outbounds.rs`——把

```rust
        groups: &format!("{PICK}\nSmart = smart, A, B"),
```

换成

```rust
        groups: &format!("{PICK}\nSub = subnet, default=A"),
```

`crates/rurge-engine/tests/outbounds.rs`——把

```rust
        h.engine.select_group("Smart", "A").await,
        Err(SelectError::NotSelectable("Smart".into()))
```

换成

```rust
        h.engine.select_group("Sub", "A").await,
        Err(SelectError::NotSelectable("Sub".into()))
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-engine --test smart`
Expected: FAIL，新增的 3 条失败——

```text
test a_reload_forgets_what_the_book_knew_of_the_policies_it_drops ... FAILED
test a_smart_group_takes_an_override_and_the_test_endpoints ... FAILED
test a_dial_asks_for_a_sampled_round_of_a_big_smart_group ... FAILED
thread 'a_reload_forgets_what_the_book_knew_of_the_policies_it_drops' panicked at crates\rurge-engine\tests\smart.rs:355:5:
assertion `left == right` failed
  left: Healthy(50ms)
 right: Unknown { failures: 0 }
thread 'a_smart_group_takes_an_override_and_the_test_endpoints' panicked at crates\rurge-engine\tests\smart.rs:276:43:
thread 'a_dial_asks_for_a_sampled_round_of_a_big_smart_group' panicked at crates\rurge-engine\tests\smart.rs:324:5:
assertion `left == right` failed
  left: 13
 right: 12
test result: FAILED. 6 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out
```

（`rurge-policy` 的单元用例此时编译不过：`sample`、`ROUND_SAMPLE`、`test_round` 尚未定义。`only_a_member_can_be_selected` 改用 `subnet` 后在旧代码上也通过——不改的话，下面 `automatic()` 纳入 `smart` 之后它就会失败。）

- [ ] **Step 3: 实现**

`crates/rurge-policy/src/smart.rs`——把

```rust
pub const ROUND_INTERVAL: Duration = Duration::from_secs(5 * 60);
```

换成

```rust
pub const ROUND_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// A regular round of a `smart` group with more members than this tests
/// this many of them (M3c design 8.1).
pub const ROUND_SAMPLE: usize = 12;
```

`crates/rurge-policy/src/smart.rs`——把

```rust
    })
}
```

换成

```rust
    })
}

/// The members a regular round of a big `smart` group tests (M3c design
/// 8.1): half of `ROUND_SAMPLE` from `most_used` (most first), the rest from
/// those tested longest ago — never tested first.
pub fn sample(
    members: &[String],
    most_used: &[String],
    tested_at: impl Fn(&str) -> Option<Instant>,
) -> Vec<String> {
    if members.len() <= ROUND_SAMPLE {
        return members.to_vec();
    }
    let mut out: Vec<String> = most_used
        .iter()
        .filter(|m| members.contains(m))
        .take(ROUND_SAMPLE / 2)
        .cloned()
        .collect();
    let mut rest: Vec<(Option<Instant>, usize)> = members
        .iter()
        .enumerate()
        .filter(|(_, m)| !out.contains(m))
        .map(|(i, m)| (tested_at(m), i))
        .collect();
    rest.sort();
    let room = ROUND_SAMPLE - out.len();
    out.extend(rest.into_iter().take(room).map(|(_, i)| members[i].clone()));
    out
}
```

`crates/rurge-policy/src/registry.rs`——把

```rust
use crate::smart::{Candidate, Health, ROUND_INTERVAL, SiteMemory, rank};
```

换成

```rust
use crate::smart::{Candidate, Health, ROUND_INTERVAL, ROUND_SAMPLE, SiteMemory, rank, sample};
```

`crates/rurge-policy/src/registry.rs`——把

```rust
    /// `MAX_CONCURRENT_TESTS` at a time, each within its own timeout.
    pub fn round_timeout(&self, group: &str) -> Duration {
        let mut groups = Vec::new();
        let mut policies = Vec::new();
        self.gather(group, 0, &mut groups, &mut policies);
```

换成

```rust
    /// `MAX_CONCURRENT_TESTS` at a time, each within its own timeout; a big
    /// `smart` group's round tests a sample.
    pub fn round_timeout(&self, group: &str) -> Duration {
        let policies = self.sample_of(group).unwrap_or_else(|| {
            let mut groups = Vec::new();
            let mut policies = Vec::new();
            self.gather(group, 0, &mut groups, &mut policies);
            policies
        });
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        self.gather(group, 0, &mut groups, &mut policies);
        if groups.is_empty() {
```

换成

```rust
        self.gather(group, 0, &mut groups, &mut policies);
        self.round(group, groups, policies).await
    }

    /// The round a dial asked for: `test_group`, but of a `smart` group
    /// with more than `ROUND_SAMPLE` members only a sample (M3c design 8.1).
    pub async fn test_round(&self, group: &str) -> Vec<String> {
        match self.sample_of(group) {
            Some(policies) => self.round(group, vec![group.to_string()], policies).await,
            None => self.test_group(group).await,
        }
    }

    /// The members a regular round of `group` tests when it is a `smart`
    /// group with more than `ROUND_SAMPLE` of them.
    fn sample_of(&self, group: &str) -> Option<Vec<String>> {
        let Some(Entry::Group { spec, members, .. }) = self.entries.get(group) else {
            return None;
        };
        if spec.kind != GroupKind::Smart || members.len() <= ROUND_SAMPLE {
            return None;
        }
        let used = self.auto.smart.most_used(group, members, Instant::now());
        Some(sample(members, &used, |m| self.tested_at(m)))
    }

    /// When `name` was last tested, for what it is now.
    fn tested_at(&self, name: &str) -> Option<Instant> {
        let (policy, test) = self.test_slot(name)?;
        self.auto.tests.result(policy, test.key).map(|r| r.at)
    }

    /// Tests `policies` and records the round for `groups`; the members of
    /// `group` that pass.
    async fn round(
        &self,
        group: &str,
        mut groups: Vec<String>,
        policies: Vec<String>,
    ) -> Vec<String> {
        if groups.is_empty() {
```

`crates/rurge-engine/src/auto.rs`——把

```rust
/// selection: `url-test`, `fallback`, `load-balance`.
```

换成

```rust
/// selection: `url-test`, `fallback`, `load-balance`, `smart`.
```

`crates/rurge-engine/src/auto.rs`——把

```rust
        GroupKind::UrlTest | GroupKind::Fallback | GroupKind::LoadBalance
```

换成

```rust
        GroupKind::UrlTest | GroupKind::Fallback | GroupKind::LoadBalance | GroupKind::Smart
```

`crates/rurge-engine/src/auto.rs`——把

```rust
                    registry.test_group(&group).await;
```

换成

```rust
                    registry.test_round(&group).await;
```

`crates/rurge-engine/src/engine.rs`——把

```rust
        self.shared.cell.store(
            next.registry
                .take()
                .expect("a generation is published once"),
        );
```

换成

```rust
        let registry = next
            .registry
            .take()
            .expect("a generation is published once");
        // what the `smart` groups knew of what is gone goes with it (phase 2
        // M3c design 5.1)
        self.shared
            .auto
            .smart
            .retain(|name| registry.contains(name));
        self.shared.cell.store(registry);
```

`crates/rurge-engine/src/subscriptions.rs`——把

```rust
            if Arc::ptr_eq(&self.runtime(), rt) {
```

换成

```rust
            if Arc::ptr_eq(&self.runtime(), rt) {
                shared.auto.smart.retain(|name| registry.contains(name));
```

要点：
- `sample`：12 个以内原样返回；否则先取 `most_used` 里仍是成员的前 6 个，再把其余成员按"上次测试时间"升序（没测过的最先，同样的按成员顺序）补足 12 个。
- `test_round` 对抽样的组只给这个组记一轮（它没有嵌套组）；其余情况就是 `test_group`。`tested_at` 读 `TestBook` 里该成员当前定义的结果时间。
- `round_timeout` 用 `sample_of(group)`（有抽样时）或原来的 `gather` 得到要测的策略，所以 `evaluate-before-use` 的等待上限随抽样变小。
- 首次启动时，大组的成员大多"未知"，连续几轮会把全部成员各测一遍（每轮 12 个，没测过的优先），之后回到 5 分钟一轮。
- `SmartBook::retain(|name| registry.contains(name))`：`contains` 覆盖策略与组；两处发布都在代际锁内，`retain` 只在内存里操作。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-policy` → 119 passed（新增 `a_round_of_a_small_group_tests_every_member`、`a_round_of_a_big_group_tests_the_most_used_and_the_longest_untested`、`a_regular_round_of_a_big_smart_group_tests_a_sample`）。
Run: `cargo test -p rurge-engine --test smart` → 9 passed。
Run: `cargo test -p rurge-engine --test outbounds` → 17 passed。

- [ ] **Step 5: 门禁与提交**

跑门禁（41 个测试二进制，986 通过 / 1 忽略）。

```bash
git add crates/rurge-policy/src/smart.rs crates/rurge-policy/src/registry.rs crates/rurge-engine/src/auto.rs crates/rurge-engine/src/engine.rs crates/rurge-engine/src/subscriptions.rs crates/rurge-engine/tests/smart.rs crates/rurge-engine/tests/outbounds.rs
git commit -m "feat(policy,engine): smart 组的测速抽样，select 覆盖与测试端点接纳 smart，发布注册表时清理 SmartBook"
```

### Task 7: 能力表翻转 `smart`

`smart` 组的全部行为已经就位（Task 3 ～ 6），翻转 bin 的能力表：此后 `W0008` 只会因 `subnet` 出现（阶段 3）。翻转前核对设计承诺的行为都已存在（M2 设计第 8 节的教训）：成员过滤与因子（Task 4）、选择与重试列表（Task 4）、拨号重试与回报（Task 5）、测速节奏、抽样与控制面（Task 6）；`policy-priority` 的解析与校验、`smart` 上 `interval` 的 `W0028` 是 M3a 就有的。

**Files:**
- Modify: `crates/rurge/src/capabilities.rs`
- Test: `crates/rurge/tests/cli.rs`（`check_knows_the_automatic_groups`）

**Interfaces:**
- Consumes: Task 3 ～ 6。
- Produces: 无新接口；`capabilities()` 的 `group_kinds` 多了 `GroupKind::Smart`。

- [ ] **Step 1: 先改用例**

`crates/rurge/tests/cli.rs`——把

```rust
L = load-balance, H, DIRECT, persistent=true\nS = smart, H, DIRECT\n[Rule]\nFINAL,U\n";
```

换成

```rust
L = load-balance, H, DIRECT, persistent=true\nS = smart, H, DIRECT, policy-priority=\"H:0.8\"\n\
N = subnet, default=H\n[Rule]\nFINAL,U\n";
```

`crates/rurge/tests/cli.rs`——把

```rust
    // `smart` is still a later milestone; the three automatic groups are
    // not, and the testing options are in effect (no W0029)
    assert_eq!(out.matches("W0008").count(), 1, "{out}");
    assert!(out.contains("`smart`"), "{out}");
```

换成

```rust
    // `subnet` is still a later phase; the four automatic groups are not,
    // and the testing options are in effect (no W0029)
    assert_eq!(out.matches("W0008").count(), 1, "{out}");
    assert!(out.contains("`subnet`"), "{out}");
    assert!(!out.contains("`smart`"), "{out}");
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge --test cli check_knows_the_automatic_groups`
Expected: FAIL——

```text
test check_knows_the_automatic_groups ... FAILED
thread 'check_knows_the_automatic_groups' panicked at crates\rurge\tests\cli.rs:188:5:
assertion `left == right` failed: warning[W0008] …groups.conf:8: policy group type `smart` is not implemented in this version; the first member is used
  left: 2
 right: 1
```

- [ ] **Step 3: 实现**

`crates/rurge/src/capabilities.rs`——把

```rust
            GroupKind::LoadBalance,
```

换成

```rust
            GroupKind::LoadBalance,
            GroupKind::Smart,
```

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge --test cli check_knows_the_automatic_groups` → 1 passed（`W0008` 只报 `subnet`，`policy-priority` 不报任何告警）。

- [ ] **Step 5: 门禁与提交**

跑门禁（41 个测试二进制，986 通过 / 1 忽略）。

```bash
git add crates/rurge/src/capabilities.rs crates/rurge/tests/cli.rs
git commit -m "feat(rurge): 能力表翻转 smart（W0008 只剩 subnet）"
```

### Task 8: 文档

兼容性清单（设计第 13 节各行）、两份 API 参考、两份 README、`CLAUDE.md`（状态、先读文档、常用命令）、手工验收清单的 M3c 一节、M3c 设计新增的第 17 节（本计划与设计文字不同的地方）。本计划末尾的「执行期修正记录」与「延后事项」两张表，由控制者在派发本任务时给出要补的行（执行中的偏差、各任务门禁的实际数字、执行中新发现的延后事项），一并写入。

**Files:**
- Modify: `docs/surge-compatibility-matrix.md`、`docs/api/phase1.md`、`docs/api/phase2.md`、`README.md`、`README_en.md`、`CLAUDE.md`、`docs/acceptance/phase2-manual.md`、`docs/superpowers/specs/2026-09-26-phase2-m3c-smart-design.md`
- Modify: 本计划文件末尾两张表（按控制者给的行）

**Interfaces:** 无。

- [ ] **Step 1: 兼容性清单**

`docs/surge-compatibility-matrix.md`——把

```markdown
| `smart` | 按真实连接质量动态选择：首响应延迟时间加权均值 + 重传惩罚（约每 1% 丢包 50 ms）× `policy-priority`；接近最优者构成优选集，其余为重试列表；按站点记忆约 1 小时；固定 5 分钟重测，`interval` 无效；>12 成员只测子集；忽略嵌套组与内置策略 | 🟡 | 2 | 算法细节非公开，rurge 按手册描述近似实现 |
| `subnet`（旧名 `ssid`） | 按当前网络选择；条件按声明顺序首个命中；网络变化重算；无命中用 `default` | 🟡 | 3 | `TYPE:CELLULAR` / `MCCMNC:` 永不匹配；阶段 3 之前整组代表它的 `default`（M3a；没写 `default` 时按空组兜底，见下一行）；`category` 等界面参数不再被当成网络条件 |
| 嵌套与循环 | 组可嵌套；循环引用告警且该组临时表现为 REJECT；无可用成员回退 DIRECT | ✅ | 2 | M3a 已实现：循环在加载期报 `W0030`（取代 `E0009`，不再阻止加载），装配后按成员与 `include-other-group` 再检测一次，环上的组解析为 REJECT，会话记录 `policy group cycle: A → B → A`；不在环上、但选到成环成员的组只在那一次 REJECT。没有成员的组（订阅还没下载到、过滤滤光了）回退 DIRECT，会话记录 `policy group has no members; DIRECT substituted`（回退的 DIRECT 本身拨号也失败时，后面再接失败原因）；rurge 专有的 `--empty-group-reject`（环境变量 `RURGE_EMPTY_GROUP_REJECT=true`）改为 REJECT。每构建一次策略表，每个环、每个空组各告警一次；M3b：嵌套组作为自动组的成员时，`select` 组按它当前的选择计分，`url-test` / `fallback` 按它当前选中的成员计分，`load-balance` 见上面该行，成环的组算失败；被当作中继（策略或组的 `underlying-proxy`）的空组不回退 DIRECT，经它的连接一律失败（见 4.3 节 `underlying-proxy`） |
| 临时覆盖 | 自动类型组可手动指定成员，期间停止自动测试 | ✅ | 2 | M3b 已实现：经 `POST /v1/policy_groups/select` 设置（Surge 未定义自动组上的这个端点，属 rurge 的扩展），`policy` 为空字符串清除；rurge CLI 暂无对应命令；覆盖期间该组不因使用而测试；组定义（不计行位置）不变的重载保留覆盖，组消失或定义变了就清除；进程重启不保留，不写 `state.json`；覆盖的成员从成员表里消失（订阅更新）时覆盖失效并告警一次 |
```

换成

```markdown
| `smart` | 按真实连接质量动态选择：首响应延迟时间加权均值 + 重传惩罚（约每 1% 丢包 50 ms）× `policy-priority`；接近最优者构成优选集，其余为重试列表；按站点记忆约 1 小时；固定 5 分钟重测，`interval` 无效；>12 成员只测子集；忽略嵌套组与内置策略 | 🟡 | 2 | M3c 已实现，算法细节手册未公开，rurge 按手册描述近似实现（常数见 M3c 设计第 9 节）。差异：用失败罚分（每次 800 ms，5 分钟减半）近似重传率；只在拨号阶段换成员——选中的成员连不上时依次再试排在后面的两个，每次最多用"剩余时间 ÷ 剩余尝试次数"，总计 10 秒；"3 秒无响应"只计入打分，不在已建立的连接上换成员；首字节耗时与 3 秒都从出站就绪起算；分数与站点记忆按策略记，几个 `smart` 组共享；站点按目标主机名原样区分；被忽略的成员（嵌套组、内置策略、`direct` / `reject` 别名）在加载时记一行 INFO（Surge 不提示）；DNS 会话与链的中间跳不换成员；改了策略的 `test-url` / `test-timeout` 时该策略的打分从头积累；没有 UDP 信号（M5） |
| `subnet`（旧名 `ssid`） | 按当前网络选择；条件按声明顺序首个命中；网络变化重算；无命中用 `default` | 🟡 | 3 | `TYPE:CELLULAR` / `MCCMNC:` 永不匹配；阶段 3 之前整组代表它的 `default`（M3a；没写 `default` 时按空组兜底，见下一行）；`category` 等界面参数不再被当成网络条件 |
| 嵌套与循环 | 组可嵌套；循环引用告警且该组临时表现为 REJECT；无可用成员回退 DIRECT | ✅ | 2 | M3a 已实现：循环在加载期报 `W0030`（取代 `E0009`，不再阻止加载），装配后按成员与 `include-other-group` 再检测一次，环上的组解析为 REJECT，会话记录 `policy group cycle: A → B → A`；不在环上、但选到成环成员的组只在那一次 REJECT。没有成员的组（订阅还没下载到、过滤滤光了）回退 DIRECT，会话记录 `policy group has no members; DIRECT substituted`（回退的 DIRECT 本身拨号也失败时，后面再接失败原因）；rurge 专有的 `--empty-group-reject`（环境变量 `RURGE_EMPTY_GROUP_REJECT=true`）改为 REJECT。每构建一次策略表，每个环、每个空组各告警一次；M3b：嵌套组作为自动组的成员时，`select` 组按它当前的选择计分，`url-test` / `fallback` 按它当前选中的成员计分，`load-balance` 见上面该行，成环的组算失败；被当作中继（策略或组的 `underlying-proxy`）的空组不回退 DIRECT，经它的连接一律失败（见 4.3 节 `underlying-proxy`） |
| 临时覆盖 | 自动类型组可手动指定成员，期间停止自动测试 | ✅ | 2 | M3b 已实现：经 `POST /v1/policy_groups/select` 设置（Surge 未定义自动组上的这个端点，属 rurge 的扩展），`policy` 为空字符串清除；rurge CLI 暂无对应命令；覆盖期间该组不因使用而测试；组定义（不计行位置）不变的重载保留覆盖，组消失或定义变了就清除；进程重启不保留，不写 `state.json`；覆盖的成员从成员表里消失（订阅更新）时覆盖失效并告警一次；M3c 起 `smart` 组同样适用，覆盖期间不换成员重试 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `evaluate-before-use` | 自动类型组 | 布尔；默认 false；M3b 已实现：组第一次被使用时等第一轮测完再选，最多等一轮测试可能用的时间（组里成员最长的测试超时 × 每 8 个一批的批数）；测完没有可用成员时这一次连接以 `policy group evaluation failed` 失败；DNS 会话（`encrypted-dns-follow-outbound-mode`）与经它作中继的连接都不等，用现有结果（DNS 会话若等，这一轮探针所需的名字解析又要经它，两者会互相卡住） | ✅ | 2 |
| `persistent` | load-balance | 布尔；默认 false；M3b 已实现（见 5.1 节 `load-balance` 行） | ✅ | 2 |
| `policy-priority` | smart | `"regex:factor;regex:factor"`；必须为正数 | ✅ | 2 |
```

换成

```markdown
| `evaluate-before-use` | 自动类型组 | 布尔；默认 false；M3b 已实现：组第一次被使用时等第一轮测完再选，最多等一轮测试可能用的时间（组里成员最长的测试超时 × 每 8 个一批的批数）；测完没有可用成员时这一次连接以 `policy group evaluation failed` 失败；DNS 会话（`encrypted-dns-follow-outbound-mode`）与经它作中继的连接都不等，用现有结果（DNS 会话若等，这一轮探针所需的名字解析又要经它，两者会互相卡住）；M3c 起 `smart` 组同样生效，测完没有健康成员即失败 | ✅ | 2 |
| `persistent` | load-balance | 布尔；默认 false；M3b 已实现（见 5.1 节 `load-balance` 行） | ✅ | 2 |
| `policy-priority` | smart | `"regex:factor;regex:factor"`；必须为正数；M3c 生效：成员名首个匹配的正则给出因子，没有匹配为 1.0 | ✅ | 2 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `GET /v1/policy_groups/test_results` | 自动组测试结果 | 全部 | 🟡 | 2 | M3b 已实现：列出每个 `url-test` / `fallback` / `load-balance` 组的成员与各自最近的结果（没有结果为 `null`；`smart` 随 M3c）；响应形状手册未给出，暂定结构见 `docs/api/phase2.md` |
| `GET/POST /v1/policy_groups/select` | 读 / 改 select 组选择 | 全部 | ✅ | 2 | M1 已实现；M3a 起按装配后的成员表校验，选择按名字保存，订阅更新后名字不在了就回落到第一个成员；M3b 起对 `url-test` / `fallback` / `load-balance` 组即临时覆盖（见 5.1 节「临时覆盖」），`policy` 为空字符串清除，覆盖不写 `state.json`；`smart`（M3c 之前）与 `subnet` 组仍 400；`GET` 对自动组返回当前生效的成员（覆盖优先，其次是按测试结果选出的） |
| `POST /v1/policy_groups/test` | 立即测试 → `{"available":[...]}` | 全部 | ✅ | 2 | M3b 已实现：立即测该组全部成员（嵌套组的成员一并测），不看 `interval`；任何类型的组都能测 |
| `GET /v1/requests/recent` `GET /v1/requests/active` `POST /v1/requests/kill` | 请求列表与终止 | 全部 | 🟡 | 1 / 4 | 响应结构手册未定义，以 Surge 实际输出为准做兼容测试；M4a 暂定结构见 `docs/api/phase1.md`；`kill` 命中 rurge 自身的内部会话（如 DNS 查询）→ 409；阶段 2 / M3b 起连通性测试也在请求记录里（内部会话，`rule` 为 `policy test`，`policy` 是被测的策略，目标只有测试 URL 的主机与端口） |
```

换成

```markdown
| `GET /v1/policy_groups/test_results` | 自动组测试结果 | 全部 | 🟡 | 2 | M3b 已实现：列出每个 `url-test` / `fallback` / `load-balance` 组与 `smart` 组（M3c 起）的成员与各自最近的测试结果（没有结果为 `null`）；响应形状手册未给出，暂定结构见 `docs/api/phase2.md` |
| `GET/POST /v1/policy_groups/select` | 读 / 改 select 组选择 | 全部 | ✅ | 2 | M1 已实现；M3a 起按装配后的成员表校验，选择按名字保存，订阅更新后名字不在了就回落到第一个成员；M3b 起对 `url-test` / `fallback` / `load-balance` 组即临时覆盖（见 5.1 节「临时覆盖」），`policy` 为空字符串清除，覆盖不写 `state.json`；M3c 起 `smart` 组同样即临时覆盖，`subnet` 组仍 400；`GET` 对自动组返回当前生效的成员（覆盖优先，其次是按测试结果选出的；`smart` 组是最近 10 分钟用得最多的成员，最近没用过时是排在第一的成员） |
| `POST /v1/policy_groups/test` | 立即测试 → `{"available":[...]}` | 全部 | ✅ | 2 | M3b 已实现：立即测该组全部成员（嵌套组的成员一并测），不看 `interval`；任何类型的组都能测；`smart` 组（M3c）测全部成员，返回测完后健康的成员 |
| `GET /v1/requests/recent` `GET /v1/requests/active` `POST /v1/requests/kill` | 请求列表与终止 | 全部 | 🟡 | 1 / 4 | 响应结构手册未定义，以 Surge 实际输出为准做兼容测试；M4a 暂定结构见 `docs/api/phase1.md`；`kill` 命中 rurge 自身的内部会话（如 DNS 查询）→ 409；阶段 2 / M3b 起连通性测试也在请求记录里（内部会话，`rule` 为 `policy test`，`policy` 是被测的策略，目标只有测试 URL 的主机与端口）；M3c 起每条记录多两个字段：`connectMs`（会话开始到出站就绪）与 `firstByteMs`（出站就绪到收到第一个上游字节），没有时为 `null` |
```

- [ ] **Step 2: 两份 API 参考**

`docs/api/phase1.md`——把

```markdown
{"id":12,"listener":"http","src":"127.0.0.1:51234","dst":"example.com:443","rule":"DOMAIN-SUFFIX,example.com,Proxy","policy":["Proxy","HK"],"sni":"example.com","protocol":"https","up":1234,"down":56789,"startedMs":1757200000000,"elapsedMs":812,"status":"completed","rejectKind":null,"error":null}
```

换成

```markdown
{"id":12,"listener":"http","src":"127.0.0.1:51234","dst":"example.com:443","rule":"DOMAIN-SUFFIX,example.com,Proxy","policy":["Proxy","HK"],"sni":"example.com","protocol":"https","up":1234,"down":56789,"startedMs":1757200000000,"elapsedMs":812,"connectMs":35,"firstByteMs":120,"status":"completed","rejectKind":null,"error":null}
```

`docs/api/phase1.md`——把

```markdown
`listener` ∈ `http` `socks5` `tun` `forward` `internal`；`status` ∈ `active` `completed` `rejected` `failed`；`rejectKind` 在 `rejected` 时是 `REJECT` / `REJECT-DROP` / `REJECT-NO-DROP` / `REJECT-TINYGIF`；`protocol` 是嗅探到的协议小写名或 `null`。
```

换成

```markdown
`listener` ∈ `http` `socks5` `tun` `forward` `internal`；`status` ∈ `active` `completed` `rejected` `failed`；`rejectKind` 在 `rejected` 时是 `REJECT` / `REJECT-DROP` / `REJECT-NO-DROP` / `REJECT-TINYGIF`；`protocol` 是嗅探到的协议小写名或 `null`。

`connectMs` 是会话开始到出站就绪的毫秒数（规则匹配、DNS、`evaluate-before-use` 的等待与 `smart` 组换成员的重试都在内），`firstByteMs` 是出站就绪到收到第一个上游字节的毫秒数；还没有对应时刻（被拒绝、拨号失败、还没收到数据）时为 `null`（阶段 2 / M3c 起）。
```

`docs/api/phase2.md`——把

```markdown
| POST | `/v1/policy_groups/select` | `{"group_name":"<name>","policy":"<member>"}` | `{}`；组或成员无效，或是 `smart` / `subnet` 组 → 400；对自动组即临时覆盖（M3b） |
```

换成

```markdown
| POST | `/v1/policy_groups/select` | `{"group_name":"<name>","policy":"<member>"}` | `{}`；组或成员无效，或是 `subnet` 组 → 400；对自动组即临时覆盖（M3b；`smart` 组 M3c 起） |
```

`docs/api/phase2.md`——把

```markdown
参数 `group_name`（必填，查询参数）。响应 `{"policy": "<当前生效的成员>"}`：`select` 组是它当前的选择（没有选择时是第一个成员）；自动组（`url-test` / `fallback` / `load-balance`）是当前生效的成员：临时覆盖优先，其次是按测试结果选出的（没有结果时是第一个成员；`load-balance` 显示第一个通过测试的成员，M3b）；`smart`（M3c 之前）是第一个成员；**一个没有成员的组**（订阅还没下载到，或过滤把成员滤光了）返回 `{"policy": ""}`，这不是错误；`subnet` 组在阶段 3 之前代表它的 `default`（M3a）。
```

换成

```markdown
参数 `group_name`（必填，查询参数）。响应 `{"policy": "<当前生效的成员>"}`：`select` 组是它当前的选择（没有选择时是第一个成员）；自动组（`url-test` / `fallback` / `load-balance`）是当前生效的成员：临时覆盖优先，其次是按测试结果选出的（没有结果时是第一个成员；`load-balance` 显示第一个通过测试的成员，M3b）；`smart`（M3c 起）是最近 10 分钟用得最多的成员，最近没用过时是排在第一的成员；**一个没有成员的组**（订阅还没下载到，或过滤把成员滤光了）返回 `{"policy": ""}`，这不是错误；`subnet` 组在阶段 3 之前代表它的 `default`（M3a）。
```

`docs/api/phase2.md`——把

```markdown
| `group_name` 是 `smart`（M3c 之前）或 `subnet` 组 | `` `Smart` does not take a selection `` |
```

换成

```markdown
| `group_name` 是 `subnet` 组 | `` `Office` does not take a selection `` |
```

`docs/api/phase2.md`——把

```markdown
无参数。响应 `{"<组名>": {"<成员>": Result 或 null}}`，只列 `url-test` / `fallback` / `load-balance` 组（`smart` 随 M3c），成员是装配后的成员表；`null` 表示没有结果：还没测过，或结果对应的定义、测试 URL、超时已经变了；嵌套组、`REJECT` 族、尚未实现的协议与测试 URL 解析不了的策略恒为 `null`（组在选择时把后三种当作失败）。
```

换成

```markdown
无参数。响应 `{"<组名>": {"<成员>": Result 或 null}}`，只列 `url-test` / `fallback` / `load-balance` / `smart` 组（`smart` 自 M3c 起），成员是装配后的成员表；`null` 表示没有结果：还没测过，或结果对应的定义、测试 URL、超时已经变了；嵌套组、`REJECT` 族、尚未实现的协议与测试 URL 解析不了的策略恒为 `null`（组在选择时把后三种当作失败）。
```

`docs/api/phase2.md`——把

```markdown
对 `url-test` / `fallback` / `load-balance` 组，同一个请求体设置**临时覆盖**：该组从下一条连接起固定用这个成员，期间不因使用而测试；`"policy": ""` 清除覆盖，恢复按测试结果选择。覆盖不写 `state.json`，进程重启后不在；组定义（不计行位置）不变的重载保留它，组消失或定义变了就清除；覆盖的成员从成员表里消失（订阅更新）时覆盖失效。
```

换成

```markdown
对 `url-test` / `fallback` / `load-balance` / `smart` 组，同一个请求体设置**临时覆盖**：该组从下一条连接起固定用这个成员，期间不因使用而测试；`"policy": ""` 清除覆盖，恢复按测试结果选择。覆盖不写 `state.json`，进程重启后不在；组定义（不计行位置）不变的重载保留它，组消失或定义变了就清除；覆盖的成员从成员表里消失（订阅更新）时覆盖失效。
```

`docs/api/phase2.md`——把

```markdown
每次测试是请求记录（`GET /v1/requests/recent`）里的一条内部会话：`listener` 为 `internal`、`rule` 为 `policy test`、`policy` 是被测的策略、目标是测试 URL 的主机与端口（不含路径与参数）；失败时 `error` 是上面 Result 里的原因。它们与 DNS 会话一样不能经 `POST /v1/requests/kill` 终止（409）。

```

换成

```markdown
每次测试是请求记录（`GET /v1/requests/recent`）里的一条内部会话：`listener` 为 `internal`、`rule` 为 `policy test`、`policy` 是被测的策略、目标是测试 URL 的主机与端口（不含路径与参数）；失败时 `error` 是上面 Result 里的原因。它们与 DNS 会话一样不能经 `POST /v1/requests/kill` 终止（409）。

## `smart` 组（M3c）

- **选择**：按成员的分数——真实会话的首字节耗时与测速结果的时间加权平均，加上失败罚分，乘以 `policy-priority` 因子——排序；最优者 1.2 倍以内的成员里随机选一个；最近在同一目标主机上成功过、分数不超过最优者 2 倍的成员优先，最近在那里失败过的排到最后。细节见 `docs/superpowers/specs/2026-09-26-phase2-m3c-smart-design.md`。
- **换成员**：选中的成员连不上时，这次会话依次再试排在后面的两个成员，每次最多用"剩余时间 ÷ 剩余尝试次数"，总计 10 秒。请求记录的 `policy` 是实际用上的那条链；换过成员时 `error` 是 ``smart group `S`: `A` failed to connect, used `B` ``（会话本身成功）；都连不上时是 ``smart group `S`: tried `A`, `B`, `C`; <最后一次的错误>``。DNS 会话与链的中间跳（`underlying-proxy`）不换成员。
- **请求记录**：`connectMs` / `firstByteMs` 两个字段见 `docs/api/phase1.md`（所有会话都有）。
- **`GET /v1/policy_groups/select`**：最近 10 分钟用得最多的成员，最近没用过时是排在第一的成员；读取不触发测速。
- **`POST /v1/policy_groups/select`**：临时覆盖，同其它自动组；覆盖期间不换成员重试。
- **`GET /v1/policy_groups/test_results`**：列出 `smart` 组，内容与其它自动组相同（成员 → 最近测速结果）；分数不经 API 给出。
- **`POST /v1/policy_groups/test`**：测全部成员（拨号触发的常规轮次在成员超过 12 个时只测 12 个），返回测完后健康的成员。

```

- [ ] **Step 3: 两份 README 与 `CLAUDE.md`**

`README.md`——把

```markdown
> **阶段 1 功能齐备：M1 ～ M4b**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）进行中：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；M3b（测速与自动组）已完成——`url-test` / `fallback` / `load-balance` 组按连通性测试（经各节点在同一条连接上两次 `HEAD`，HTTPS 测试 URL 在已建立的 TLS 连接上测第二次）自动选择，`interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` 与策略的 `test-url` / `test-timeout` 生效，自动组可经 `POST /v1/policy_groups/select` 临时覆盖，新增 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 三个端点；其余出站协议与 `smart` / `subnet` 策略组仍在阶段 2 后续里程碑。
```

换成

```markdown
> **阶段 1 功能齐备：M1 ～ M4b**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）进行中：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；M3b（测速与自动组）已完成——`url-test` / `fallback` / `load-balance` 组按连通性测试（经各节点在同一条连接上两次 `HEAD`，HTTPS 测试 URL 在已建立的 TLS 连接上测第二次）自动选择，`interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` 与策略的 `test-url` / `test-timeout` 生效，自动组可经 `POST /v1/policy_groups/select` 临时覆盖，新增 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 三个端点；M3c（`smart`）已完成——`smart` 组按真实会话的首字节耗时与测速结果打分（失败罚分、`policy-priority` 因子、约 1 小时的站点记忆），选中的成员连不上时自动换下一个（最多再试 2 个），请求记录增加 `connectMs` / `firstByteMs`；其余出站协议仍在阶段 2 后续里程碑，`subnet` 策略组在阶段 3。
```

`README.md`——把

```markdown
| 策略组         | select / url-test / fallback / load-balance / smart / subnet、策略引入与订阅、延迟测试（`select` 组的选择经 API 读取与切换已实现，阶段 2 / M1；策略引入与订阅、组级 `underlying-proxy` 已实现，阶段 2 / M3a；url-test / fallback / load-balance、延迟测试与临时覆盖已实现，阶段 2 / M3b）                                                                        | 2     |
```

换成

```markdown
| 策略组         | select / url-test / fallback / load-balance / smart / subnet、策略引入与订阅、延迟测试（`select` 组的选择经 API 读取与切换已实现，阶段 2 / M1；策略引入与订阅、组级 `underlying-proxy` 已实现，阶段 2 / M3a；url-test / fallback / load-balance、延迟测试与临时覆盖已实现，阶段 2 / M3b；smart 已实现，阶段 2 / M3c）                                                                        | 2     |
```

`README.md`——把

```markdown
3. **阶段 2** 出站协议全集、策略组、策略订阅（进行中：M1 出站地基与 HTTP / SOCKS5 上游已完成；M2a（Trojan，TCP + WebSocket）已完成；M2b（VMess / AnyTLS）已完成；M2c（Shadow TLS）已完成；M3a（成员装配与订阅）已完成；M3b（测速与自动组）已完成）
```

换成

```markdown
3. **阶段 2** 出站协议全集、策略组、策略订阅（进行中：M1 出站地基与 HTTP / SOCKS5 上游已完成；M2a（Trojan，TCP + WebSocket）已完成；M2b（VMess / AnyTLS）已完成；M2c（Shadow TLS）已完成；M3a（成员装配与订阅）已完成；M3b（测速与自动组）已完成；M3c（`smart`）已完成）
```

`README.md`——把

```markdown
> `rurge check`、`rurge rule match`、`rurge dns lookup` 与 `rurge run`（HTTP / SOCKS5 代理，DIRECT / REJECT，以及 `http` / `https` / `socks5` / `socks5-tls` / `trojan` / `vmess` / `anytls` 上游——均可叠加 Shadow TLS，含 `underlying-proxy` 链）已可用；`select` / `url-test` / `fallback` / `load-balance` 组已可用，`smart` 与 `subnet` 在后续里程碑。HTTP API 与 `rurge reload` / `stop` / `status` 已可用（见 [docs/api/phase1.md](docs/api/phase1.md)，阶段 2 新增端点见 [docs/api/phase2.md](docs/api/phase2.md)）。`rurge run --system-proxy` 可以把系统代理指向 rurge，退出时恢复、崩溃后在下次启动时恢复；`rurge service install | uninstall [--user] [--dry-run]` 可以注册 / 移除开机自启（systemd / launchd / Windows 计划任务）。macOS 经 `networksetup` 设置，通常需要管理员账户；是否需要 `sudo` 尚未在真机验证（见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)），失败时 rurge 原样报出工具的错误。`rurge run` 另支持 `--idle-timeout`、`--request-log-size`、`--watch`（配置热重载）、`--log-file`（按天滚动）、`--empty-group-reject`（没有成员的策略组拒绝而不是直连）等 rurge 专有运行时选项，只经命令行参数 / 环境变量提供，不写入 Surge 配置文件。
```

换成

```markdown
> `rurge check`、`rurge rule match`、`rurge dns lookup` 与 `rurge run`（HTTP / SOCKS5 代理，DIRECT / REJECT，以及 `http` / `https` / `socks5` / `socks5-tls` / `trojan` / `vmess` / `anytls` 上游——均可叠加 Shadow TLS，含 `underlying-proxy` 链）已可用；`select` / `url-test` / `fallback` / `load-balance` / `smart` 组已可用，`subnet` 在阶段 3。HTTP API 与 `rurge reload` / `stop` / `status` 已可用（见 [docs/api/phase1.md](docs/api/phase1.md)，阶段 2 新增端点见 [docs/api/phase2.md](docs/api/phase2.md)）。`rurge run --system-proxy` 可以把系统代理指向 rurge，退出时恢复、崩溃后在下次启动时恢复；`rurge service install | uninstall [--user] [--dry-run]` 可以注册 / 移除开机自启（systemd / launchd / Windows 计划任务）。macOS 经 `networksetup` 设置，通常需要管理员账户；是否需要 `sudo` 尚未在真机验证（见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)），失败时 rurge 原样报出工具的错误。`rurge run` 另支持 `--idle-timeout`、`--request-log-size`、`--watch`（配置热重载）、`--log-file`（按天滚动）、`--empty-group-reject`（没有成员的策略组拒绝而不是直连）等 rurge 专有运行时选项，只经命令行参数 / 环境变量提供，不写入 Surge 配置文件。
```

`README_en.md`——把

```markdown
> **Phase 1 feature-complete: M1 through M4b** (profile parsing, rule engine, rule sets, GeoIP, external resource management, DNS client, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`; M3b added the request log and traffic stats, SNI recording, idle timeout, REJECT auto-escalation, a 502 for failed CONNECT dials, graceful shutdown, hot reload (SIGHUP / `--watch`), `--log-file`, and `encrypted-dns-follow-outbound-mode`; M4a added a Surge-compatible HTTP API (phase-1 endpoints, `X-Key` auth with banning), outbound mode / global policy persisted to `state.json`, and `rurge reload` / `stop` / `status`; M4b added the system proxy (Windows registry + a WinINet notification, macOS `networksetup`, Linux GNOME / KDE, `--system-proxy` and `POST /v1/features/system_proxy`, automatic recovery on exit or crash, and following listener changes across a reload) and `rurge service install / uninstall [--dry-run]` (systemd / launchd / a Windows scheduled task)) — the phase-1 acceptance criteria that need a real desktop environment (the three-platform system proxy, service install) have not been through manual acceptance yet; see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md) for the checklist. The dashboard is phase 6. Phase 2 (outbound protocols and policy groups) is in progress: M1 (outbound foundation and HTTP / SOCKS5 upstreams) is done — `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections by rule through DIRECT / REJECT or `http` / `https` / `socks5` / `socks5-tls` upstreams (TCP, including two-or-more-hop `underlying-proxy` chains), and a `select` group's choice can be read and switched over the HTTP API (see [docs/api/phase2.md](docs/api/phase2.md)); M2a (the TLS family, Trojan first) is done — the `trojan` policy (TCP, TLS always on, optionally over WebSocket) is usable; M2b (VMess / AnyTLS) is done — the `vmess` (AEAD handshake, TCP, optionally over TLS / WebSocket) and `anytls` (TCP, session reuse) policies are usable, and a reload reuses outbounds by fingerprint without disturbing connection pools still in use; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); M3a (member assembly and subscriptions) is done — `policy-path` subscriptions (a local file or a URL, in Surge format; an update rebuilds only the policy table, without disturbing unrelated connections), `include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`, the group-level `underlying-proxy` (derived `Name (via Relay)` members), group cycle warnings (`W0030`) and the empty-group fallback (DIRECT by default, REJECT with `--empty-group-reject`) are usable; M3b (connectivity tests and automatic groups) is done — `url-test` / `fallback` / `load-balance` groups choose by connectivity tests (two `HEAD`s over one connection through each member, the second one on the established TLS connection for an HTTPS test URL), `interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` and the policies' `test-url` / `test-timeout` take effect, an automatic group takes a temporary override through `POST /v1/policy_groups/select`, and `POST /v1/policies/test`, `GET /v1/policy_groups/test_results` and `POST /v1/policy_groups/test` are available; the remaining outbound protocols and the `smart` / `subnet` groups are later phase-2 milestones.
```

换成

```markdown
> **Phase 1 feature-complete: M1 through M4b** (profile parsing, rule engine, rule sets, GeoIP, external resource management, DNS client, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`; M3b added the request log and traffic stats, SNI recording, idle timeout, REJECT auto-escalation, a 502 for failed CONNECT dials, graceful shutdown, hot reload (SIGHUP / `--watch`), `--log-file`, and `encrypted-dns-follow-outbound-mode`; M4a added a Surge-compatible HTTP API (phase-1 endpoints, `X-Key` auth with banning), outbound mode / global policy persisted to `state.json`, and `rurge reload` / `stop` / `status`; M4b added the system proxy (Windows registry + a WinINet notification, macOS `networksetup`, Linux GNOME / KDE, `--system-proxy` and `POST /v1/features/system_proxy`, automatic recovery on exit or crash, and following listener changes across a reload) and `rurge service install / uninstall [--dry-run]` (systemd / launchd / a Windows scheduled task)) — the phase-1 acceptance criteria that need a real desktop environment (the three-platform system proxy, service install) have not been through manual acceptance yet; see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md) for the checklist. The dashboard is phase 6. Phase 2 (outbound protocols and policy groups) is in progress: M1 (outbound foundation and HTTP / SOCKS5 upstreams) is done — `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections by rule through DIRECT / REJECT or `http` / `https` / `socks5` / `socks5-tls` upstreams (TCP, including two-or-more-hop `underlying-proxy` chains), and a `select` group's choice can be read and switched over the HTTP API (see [docs/api/phase2.md](docs/api/phase2.md)); M2a (the TLS family, Trojan first) is done — the `trojan` policy (TCP, TLS always on, optionally over WebSocket) is usable; M2b (VMess / AnyTLS) is done — the `vmess` (AEAD handshake, TCP, optionally over TLS / WebSocket) and `anytls` (TCP, session reuse) policies are usable, and a reload reuses outbounds by fingerprint without disturbing connection pools still in use; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); M3a (member assembly and subscriptions) is done — `policy-path` subscriptions (a local file or a URL, in Surge format; an update rebuilds only the policy table, without disturbing unrelated connections), `include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`, the group-level `underlying-proxy` (derived `Name (via Relay)` members), group cycle warnings (`W0030`) and the empty-group fallback (DIRECT by default, REJECT with `--empty-group-reject`) are usable; M3b (connectivity tests and automatic groups) is done — `url-test` / `fallback` / `load-balance` groups choose by connectivity tests (two `HEAD`s over one connection through each member, the second one on the established TLS connection for an HTTPS test URL), `interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` and the policies' `test-url` / `test-timeout` take effect, an automatic group takes a temporary override through `POST /v1/policy_groups/select`, and `POST /v1/policies/test`, `GET /v1/policy_groups/test_results` and `POST /v1/policy_groups/test` are available; M3c (`smart`) is done — a `smart` group scores its members by the first-byte time of real sessions and by the tests (failure penalties, `policy-priority` factors, per-site memory for about an hour) and moves on to the next member when the one it picked does not connect (up to two more), and the request log gains `connectMs` / `firstByteMs`; the remaining outbound protocols are later phase-2 milestones, and the `subnet` group comes in phase 3.
```

`README_en.md`——把

```markdown
| Policy groups       | select / url-test / fallback / load-balance / smart / subnet, policy including and subscriptions, latency tests (a `select` group's choice can be read and switched over the API, phase 2 / M1; policy including, subscriptions and the group-level `underlying-proxy` implemented, phase 2 / M3a; url-test / fallback / load-balance, latency tests and temporary overrides implemented, phase 2 / M3b)                                                         | 2     |
```

换成

```markdown
| Policy groups       | select / url-test / fallback / load-balance / smart / subnet, policy including and subscriptions, latency tests (a `select` group's choice can be read and switched over the API, phase 2 / M1; policy including, subscriptions and the group-level `underlying-proxy` implemented, phase 2 / M3a; url-test / fallback / load-balance, latency tests and temporary overrides implemented, phase 2 / M3b; smart implemented, phase 2 / M3c)                                                         | 2     |
```

`README_en.md`——把

```markdown
3. **Phase 2** All outbound protocols, policy groups, subscriptions (in progress: M1, the outbound foundation and HTTP / SOCKS5 upstreams, is done; M2a, Trojan (TCP + WebSocket), is done; M2b, VMess / AnyTLS, is done; M2c, Shadow TLS, is done; M3a, member assembly and subscriptions, is done; M3b, connectivity tests and automatic groups, is done)
```

换成

```markdown
3. **Phase 2** All outbound protocols, policy groups, subscriptions (in progress: M1, the outbound foundation and HTTP / SOCKS5 upstreams, is done; M2a, Trojan (TCP + WebSocket), is done; M2b, VMess / AnyTLS, is done; M2c, Shadow TLS, is done; M3a, member assembly and subscriptions, is done; M3b, connectivity tests and automatic groups, is done; M3c, `smart` groups, is done)
```

`README_en.md`——把

```markdown
> `rurge check`, `rurge rule match`, `rurge dns lookup` and `rurge run` (HTTP / SOCKS5 proxy, DIRECT / REJECT, and `http` / `https` / `socks5` / `socks5-tls` / `trojan` / `vmess` / `anytls` upstreams — all optionally wrapped in Shadow TLS — including `underlying-proxy` chains) work today; `select` / `url-test` / `fallback` / `load-balance` groups work, `smart` and `subnet` arrive in later milestones. The HTTP API and `rurge reload` / `stop` / `status` are available (see [docs/api/phase1.md](docs/api/phase1.md); phase-2 additions in [docs/api/phase2.md](docs/api/phase2.md)). `rurge run --system-proxy` points the system proxy at rurge and restores it on exit, or at the next start after a crash; `rurge service install | uninstall [--user] [--dry-run]` registers or removes automatic startup (systemd / launchd / a Windows scheduled task). On macOS, `networksetup` usually needs an administrator account; whether `sudo` is required has not been verified on real hardware yet (see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)) — on failure, rurge passes the tool's error through as-is. `rurge run` also takes rurge-specific runtime options — `--idle-timeout`, `--request-log-size`, `--watch` (hot reload), `--log-file` (daily rotation), `--empty-group-reject` (a policy group without members rejects instead of going direct) — as CLI flags / env vars only, never written into the Surge profile.
```

换成

```markdown
> `rurge check`, `rurge rule match`, `rurge dns lookup` and `rurge run` (HTTP / SOCKS5 proxy, DIRECT / REJECT, and `http` / `https` / `socks5` / `socks5-tls` / `trojan` / `vmess` / `anytls` upstreams — all optionally wrapped in Shadow TLS — including `underlying-proxy` chains) work today; `select` / `url-test` / `fallback` / `load-balance` / `smart` groups work, `subnet` arrives in phase 3. The HTTP API and `rurge reload` / `stop` / `status` are available (see [docs/api/phase1.md](docs/api/phase1.md); phase-2 additions in [docs/api/phase2.md](docs/api/phase2.md)). `rurge run --system-proxy` points the system proxy at rurge and restores it on exit, or at the next start after a crash; `rurge service install | uninstall [--user] [--dry-run]` registers or removes automatic startup (systemd / launchd / a Windows scheduled task). On macOS, `networksetup` usually needs an administrator account; whether `sudo` is required has not been verified on real hardware yet (see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)) — on failure, rurge passes the tool's error through as-is. `rurge run` also takes rurge-specific runtime options — `--idle-timeout`, `--request-log-size`, `--watch` (hot reload), `--log-file` (daily rotation), `--empty-group-reject` (a policy group without members rejects instead of going direct) — as CLI flags / env vars only, never written into the Surge profile.
```

`CLAUDE.md`——把

```markdown
阶段 1 功能已齐备（M1 ～ M4b）；阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`。M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。M3b（测速与自动组）已完成——`[General] test-timeout` 区分未设置（`General::test_target`；策略的 `test-url` / `test-timeout` 生效，`W0029` 对它们退役）；`rurge-policy` 的 `probe`（经策略自己的出站在一条连接上两次 HEAD，HTTPS 在已建立的 TLS 连接上测第二次，根证书取 `OutboundFactory::roots`）、`testbook`（`TestBook`：结果按策略与定义保存、同一策略同时只测一次、最多 8 个并发、`TestObserver`、一次性的 `test_once`）与 `auto`（`url-test` / `fallback` / `load-balance` 三种选法、`SelectCtx`、`AutoGroups`：临时覆盖、`url-test` 保持的成员、每组上一轮测试的时间与测试请求）；注册表按测试结果选成员（只有拨号触发测试、`evaluate-before-use` 的 `Resolution.pending`、嵌套组的分数、`round_timeout`）、`resolve_relay`（空组当中继一律拒绝）、订阅行不能动用主配置的 `client-cert` 与策略（`W0023`）；`rurge-engine` 的测试调度任务、测试会话进请求记录（规则 `policy test`）、`evaluate-before-use` 的有界等待（`policy group evaluation failed`）、重载时保留未变组的覆盖、`Engine::snapshot`（运行时与注册表在代际锁下成对读取）与 DNS 会话在链底为 REJECT 时的旁路；`rurge-api` 的 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 与 `POST /v1/policy_groups/select` 对自动组的临时覆盖；能力表翻转 `url-test` / `fallback` / `load-balance`（`W0008` 只剩 `smart` 与 `subnet`）。M3c（`smart`）尚未开始。
```

换成

```markdown
阶段 1 功能已齐备（M1 ～ M4b）；阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`。M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。M3b（测速与自动组）已完成——`[General] test-timeout` 区分未设置（`General::test_target`；策略的 `test-url` / `test-timeout` 生效，`W0029` 对它们退役）；`rurge-policy` 的 `probe`（经策略自己的出站在一条连接上两次 HEAD，HTTPS 在已建立的 TLS 连接上测第二次，根证书取 `OutboundFactory::roots`）、`testbook`（`TestBook`：结果按策略与定义保存、同一策略同时只测一次、最多 8 个并发、`TestObserver`、一次性的 `test_once`）与 `auto`（`url-test` / `fallback` / `load-balance` 三种选法、`SelectCtx`、`AutoGroups`：临时覆盖、`url-test` 保持的成员、每组上一轮测试的时间与测试请求）；注册表按测试结果选成员（只有拨号触发测试、`evaluate-before-use` 的 `Resolution.pending`、嵌套组的分数、`round_timeout`）、`resolve_relay`（空组当中继一律拒绝）、订阅行不能动用主配置的 `client-cert` 与策略（`W0023`）；`rurge-engine` 的测试调度任务、测试会话进请求记录（规则 `policy test`）、`evaluate-before-use` 的有界等待（`policy group evaluation failed`）、重载时保留未变组的覆盖、`Engine::snapshot`（运行时与注册表在代际锁下成对读取）与 DNS 会话在链底为 REJECT 时的旁路；`rurge-api` 的 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 与 `POST /v1/policy_groups/select` 对自动组的临时覆盖；能力表翻转 `url-test` / `fallback` / `load-balance`（`W0008` 只剩 `smart` 与 `subnet`）。M3c（`smart`）已完成——`rurge-policy::smart`（`SmartBook`：按策略的时间加权首字节分数与失败罚分、健康 / 未知 / 失败三种状态、站点记忆、使用计数；`rank` 给出选中的成员与重试列表；`sample` 给大组的常规轮抽样）、注册表的 `smart` 成员过滤与 `policy-priority` 因子、`Resolution.smart`（`SmartPick`）、`resolve_member`、`test_round`，以及每代预存的测试查表（`TestBook::outcome`，M3b 延后事项 #14）；`rurge-inbound` 的 `SessionHandle` 两个时刻（出站就绪、首字节）、可挂多个的结束钩子与首字节钩子、`mark_upstream_failed`；`rurge-engine` 的 `smart` 模块（拨号时按重试列表换成员，单次时限"剩余时间 ÷ 剩余尝试次数"；质量回报：首字节、3 秒无响应、上游先断）、请求记录的 `connectMs` / `firstByteMs`、`automatic()` 纳入 `smart`、每次发布注册表时 `SmartBook::retain`；能力表翻转 `smart`（`W0008` 只剩 `subnet`）。
```

`CLAUDE.md`——把

```markdown
- `docs/superpowers/specs/2026-09-26-phase2-m3c-smart-design.md`：阶段 2 / M3c 细化设计（`smart` 组），细化 M3 设计第 7 节、不一致处以它为准。已决事项 M3c-D1 ～ D10（沿用 M3b 的分工、分数与站点记忆按策略记、首字节与 3 秒从出站就绪起算、只在会话拨号里重试且单次时限为"剩余时间 ÷ 剩余尝试次数"、测速结果由 `TestBook` 推送、当前选择为 10 分钟内最常用的成员、每一代预先算好测试查表）；精确的打分算法、站点记忆、测速抽样、请求记录的 `connectMs` / `firstByteMs` 两列；第 15 节 V1–V11 是写计划时必须核对的事项。
```

换成

```markdown
- `docs/superpowers/specs/2026-09-26-phase2-m3c-smart-design.md`：阶段 2 / M3c 细化设计（`smart` 组），细化 M3 设计第 7 节、不一致处以它为准。已决事项 M3c-D1 ～ D10（沿用 M3b 的分工、分数与站点记忆按策略记、首字节与 3 秒从出站就绪起算、只在会话拨号里重试且单次时限为"剩余时间 ÷ 剩余尝试次数"、测速结果由 `TestBook` 推送、当前选择为 10 分钟内最常用的成员、每一代预先算好测试查表）；精确的打分算法、站点记忆、测速抽样、请求记录的 `connectMs` / `firstByteMs` 两列；第 15 节 V1–V11 是写计划时必须核对的事项。
- `docs/superpowers/plans/2026-09-26-phase2-m3c-smart-plan.md`：阶段 2 / M3c（`smart` 组）实施计划（8 个任务）。开头「计划期决定」表（P1–P21）记录核对源码得出的结论与和设计文字不同的决定（不另设 `SessionReporter` trait、"同一定义"按出站对象判断、DNS 会话只回报三种、健康切换的日志只由会话回报触发、单次时限另由引擎计时、重试列表跳过不是代理的成员、大组抽样的 `test_round` 等）与「承接事项」；末尾「执行期修正记录」与「延后事项」两张表。
```

`CLAUDE.md`——把

```markdown
cargo test -p rurge-policy assemble             # 成员装配：顺序、过滤 / 前缀 / 修饰、全局重名、中继派生、组环
```

换成

```markdown
cargo test -p rurge-policy assemble             # 成员装配：顺序、过滤 / 前缀 / 修饰、全局重名、中继派生、组环
cargo test -p rurge-policy smart                # smart：打分、排序与重试列表、站点记忆、使用计数、抽样（时间由调用方注入）
cargo test -p rurge-engine --test smart         # smart 端到端：换成员与单次时限、首字节 / 3 秒无响应 / 上游先断的回报、抽样轮、覆盖与测试端点、重载清理
```

- [ ] **Step 4: 手工验收与设计文档**

`docs/acceptance/phase2-manual.md`——把

```markdown
- [ ] 日志（含 `--log-level verbose`）里搜不到任何策略的 `test-url` 的路径与参数（订阅行可能把 token 放在测试 URL 里）。

```

换成

```markdown
- [ ] 日志（含 `--log-level verbose`）里搜不到任何策略的 `test-url` 的路径与参数（订阅行可能把 token 放在测试 URL 里）。

## M3c　smart

需要至少三个真实节点，其中一个可以随时停掉（或把它的端口写错后 `rurge reload`），自动化测试（只用回环）覆盖不了。

- [ ] `Smart = smart, <节点 A>, <节点 B>, <节点 C>`：经 `Smart` 正常浏览十几分钟，`GET /v1/requests/recent` 里的会话都有 `connectMs` 与 `firstByteMs`，数值与节点的实际延迟量级相当；`GET /v1/policy_groups/select?group_name=Smart` 是这段时间用得最多的节点。
- [ ] 停掉节点 A：之后经 `Smart` 的请求仍然正常，个别会话的 `error` 是 ``smart group `Smart`: `A` failed to connect, used `B` ``；几分钟后新会话基本不再选 A。恢复 A 后，下一轮测速（5 分钟内）或经它成功的会话之后它又会被选到。
- [ ] 一个能连上、但出口访问不了某个网站的节点：访问那个网站几次之后，经 `Smart` 访问它改走别的节点，访问其它网站仍可能用这个节点（站点记忆）。
- [ ] `policy-priority="<节点 C>:0.5"`：C 明显更常被选中；改为 `"<节点 C>:3"` 后 C 很少被选中。
- [ ] `POST /v1/policy_groups/select {"group_name":"Smart","policy":"<节点 B>"}`：之后固定走 B，停掉 B 时请求失败、不换成员；`{"policy":""}` 清除后恢复。
- [ ] 日志（含 `--log-level verbose`）里 `smart` 相关的行只有节点名，没有 URL 与凭据；节点连续失败时有一条 `smart: the policy counts as failed`，恢复后有 `smart: the policy works again`。

```

`docs/superpowers/specs/2026-09-26-phase2-m3c-smart-design.md`——把

```markdown
8. 文档：兼容性清单、`docs/api` 两份、README 两份、`CLAUDE.md`、手工验收、本文件与 M3 设计的实施期订正。

```

换成

```markdown
8. 文档：兼容性清单、`docs/api` 两份、README 两份、`CLAUDE.md`、手工验收、本文件与 M3 设计的实施期订正。

## 17. 计划期的订正

写 M3c 计划（`docs/superpowers/plans/2026-09-26-phase2-m3c-smart-plan.md`）时核对源码得出、与上文不同的地方；P 编号是计划「计划期决定」表的编号。

| 本文原文 | 计划 | 依据 |
| -------- | ---- | ---- |
| 4.2 `rurge-policy` 定义 `SessionReporter` trait，`SmartBook` 实现它 | 不另设 trait：引擎直接调用 `SmartBook::report_success` / `report_failure`（P1） | 只有一个实现；依赖方向不变 |
| 5.1 只改测试 URL 或超时不清零 | "同一个定义"按出站对象判断：M2b 在指纹不变时沿用同一个出站对象，而指纹含 `test-url` / `test-timeout`，所以改它们也会清零（P2） | 另做一套不含测试参数的定义标识要改注册表的复用逻辑；改测试参数少见，清零后几分钟内就会重新积累 |
| 4.3 / M3c-D4 回报含 DNS 会话 | DNS 会话只回报拨号失败、首字节与 3 秒无响应（P3） | "上游先断"在 `pump` 与明文 HTTP 转发里判断，DNS 会话的流不经过这两处 |
| §10 健康 ⇄ 失败切换时记一行 INFO（组名、成员名） | 只在会话回报引起切换时记，只写策略名（P4） | `SmartBook` 按策略记、不知道组；测速推送覆盖所有被测策略（多数不是 `smart` 成员），由它触发会刷屏 |
| 6.1 被忽略的成员 | 组的成员表只剩代理，视图、`select`、测试端点都用它（P5） | 手册说忽略，各处口径一致 |
| §7 每次尝试最多用"剩余时间 ÷ 剩余尝试次数" | 有两个以上候选时另由引擎计时（P6） | 出站若不严格遵守 `ConnectOpts.timeout`，也不会超出它的那一份 |
| 6.2 重试列表 | 跳过不是代理的成员（尚未实现的协议）（P7） | 试它只会把一次连接失败变成 REJECT |
| 8.1 有成员处于"未知"时也请求一轮 | 同原文，"未知"指 `SmartBook` 的状态：有真实会话样本的成员不算未测（P11） | M3b 按"有没有测速结果"判断，对 `smart` 不合适 |
| 第 16 节任务草图 | 三条"换成员 / 3 秒无响应 / 上游先断"的端到端用例放在 Task 5（它们验证的是 Task 5 的行为），Task 7 只翻转能力表（P21） | TDD：用例与它验证的实现同一个任务 |
| — | M3a 的组环检测仍把 `smart` 组写进去的嵌套组当作边：加载时 `W0030`、运行期 REJECT，虽然 `smart` 实际忽略它（P20，计划「延后事项」） | 这种写法少见；改组环检测要动 `rurge-config` 与 `assemble` 两处 |

```

- [ ] **Step 5: 门禁与提交**

跑门禁（只改了文档：41 个测试二进制，986 通过 / 1 忽略）。

```bash
git add docs README.md README_en.md CLAUDE.md
git commit -m "docs: M3c smart——兼容性清单、API 参考、手工验收、设计第 17 节、README 与 CLAUDE.md"
```

---

## 验收对照（设计第 12 节）

| # | 验收项 | 由谁保证 |
| - | ------ | -------- |
| 1 | 第 11 节的单元与端到端用例全部通过；fmt / clippy 零警告 / `cargo test --workspace` 全绿 | 各任务的门禁 |
| 2 | 选中成员拨号失败时，会话在 10 秒内由重试列表里的成员接上，单次尝试不超过"剩余时间 ÷ 剩余尝试次数" | Task 5：`a_member_that_does_not_connect_hands_over_to_the_next`、`a_member_that_never_answers_has_its_share_of_the_time`、`when_no_member_connects_the_session_fails_naming_them` |
| 3 | 请求记录与 API 有 `connectMs` / `firstByteMs` | Task 1：`the_request_log_times_the_outbound_and_the_first_byte`、`the_timings_are_null_until_known` |
| 4 | `smart` 组在三个测试端点与 `select` 上的行为符合设计 8.3 | Task 4：`the_view_of_a_smart_group_is_its_most_used_member`；Task 6：`a_smart_group_takes_an_override_and_the_test_endpoints` |
| 5 | `W0008` 只因 `subnet` 出现 | Task 7：`check_knows_the_automatic_groups` |
| 6 | 回报、站点记忆、备注与日志里没有 URL 与凭据 | 按构造：它们只存组名、策略名与目标主机名（Task 4、5）；手工验收（Task 8） |
| 7 | 需要真实节点的项目进手工验收清单 | Task 8：`docs/acceptance/phase2-manual.md` 的 M3c 一节 |

## 执行期修正记录

| 任务 | 计划原文 | 实际做法 | 原因 | 提交 |
| ---- | -------- | -------- | ---- | ---- |
| 4 | 选成员时，协议尚未实现的成员也作为候选进入 `rank`（`smart_health` 给它恒为"失败"的状态） | 这种成员不进排序，既不会被选中，也不进重试列表；组里一个能用的成员都没有时，才取组的第一个成员（普通选择，没有重试列表），得到带说明的 REJECT；新增用例 `a_member_that_never_works_stands_in_only_when_nothing_else_can` | 所有能用的成员都在某个站点失败过一次时，`rank` 会把从不被尝试、因而不在该站点失败名单里的它排到最前并选中：该站点的会话在站点记忆过期前（至多一小时）都得到不重试的 REJECT，违背设计 6.1"只有别无选择时才会被用上" | 29f8adc |
| 5 | `relay.rs` 的两条新用例放在 `task.await.unwrap();` / `    }` 之后（测试模块中部） | 放在测试模块末尾 | 只是位置，内容逐字相同 | 5babf5a |
| 5 | 明文 HTTP 转发拿不到响应头时（`send_request` 出错）一律标记"上游先断" | 只在错误不是 hyper 的用户侧错误（`Error::is_user()`：请求体自身出错——客户端上传到一半离开，请求头组合不合法，分发任务已不在）时标记；新增用例 `a_client_leaving_mid_upload_is_no_upstream_failure` | 客户端上传到一半离开时，hyper 把请求体的错误交给 `send_request`，原写法会把它记成成员的失败（罚分，并在该站点记一小时"失败过"），违背"客户端先走不算"（设计 4.3） | 8855130 |
| 6 | `registry.rs` 的新用例 `a_regular_round_of_a_big_smart_group_tests_a_sample` 接在 `a_smart_group_is_available_by_its_health` 之后 | 接在 Task 4 修正新增的用例之后 | Task 4 的修正在同一位置新增了用例；只是位置，内容逐字相同 | de46058 |
| 6 | `sample_of` 从全部成员里抽样；拨号时只要有成员"未知"就请求一轮（Task 4） | 只从测得了的成员（`test_slot` 有值）里抽样，测得了的成员不超过 12 个时整组测；测不了的成员"未知"时不请求轮次；大组用例改为 17 个成员，让 `round_timeout` 的断言分得出抽样与整组；新增用例 `members_no_round_can_test_take_no_place_in_the_sample`、`a_member_no_round_can_test_asks_for_no_round` | 协议尚未实现（如 M6 之前的 `ss`）或测试 URL 解析不了的成员永远"没测过"，会占掉每一轮"最久没测"的名额；排不进样本的成员一直"未知"，拨号就一轮接一轮地请求测试，却总也测不到它们 | 3ebbb1d |
| 1 – 8 | 各任务门禁的预期总数（Task 1 起 937、938、948、965、980、986、986、986） | 实际依次是 937、938、948、966、982、990、990、990（各任务的修正做完之后）：Task 4、5 的修正各多一条用例，Task 6 的修正多两条；最后一次门禁 41 个测试二进制，990 通过 / 0 失败 / 1 忽略 | 见上面几行 | — |
| 终审 | Task 5：3 秒定时到点时只看有没有首字节、会话结束没有 | 客户端一侧已经结束（隧道里客户端先关了连接）时也不算失败：`SessionHandle` 加 `mark_client_gone` / `client_gone`，`pump` 的上行读端结束时标记（原来的局部标志改用它），定时任务查它；新增用例 `silence_after_the_client_left_is_no_failure` | 浏览器取消一个经隧道的明文请求、上游又一直不回话时，3 秒后成员会被记一次失败，违背"客户端先走不算"（设计 4.3） | e728ce3 |
| 终审 | P4：只有会话回报引起的"健康 ⇄ 失败"切换记 INFO | 有过会话回报的策略，测速推送引起的切换也记；只被测速过的策略照旧不记；新增用例 `flips_are_said_for_the_policies_sessions_reported_on` | 节点掉线后第一次失败就不再被选中，此后的切换都由测速推动，照 P4 这两行几乎不会出现 | e728ce3 |
| 终审 | P17：每次读只多一次原子读 | `mark_first_byte` 在已有首字节时直接返回（原来每次都先取一次时间） | 与 P17 的开销说法一致 | e728ce3 |
| 终审 | （注释与文档） | `capabilities.rs` 的模块说明加上 `smart`；`AutoGroups::connect` 的说明改为调度任务跑 `test_round`；手工验收 M3c 一节五处改成代码的实际表现（恢复后要等罚分衰减、测试会话两列为 `null`、HTTPS 网站、切换日志只对用过的节点、连不上的节点测三次才有结论）；API 参考补上 M3c 的指引、测试会话的两列、`smart` 组的成员表与尚未实现协议的成员；兼容性清单补登两处差异（还没有结论的成员触发轮次、抽样按能测的成员算）；`CLAUDE.md` 的 API 参考条目提到 M3c 一节；设计第 18 节加两行 | 文档与注释说的不是代码现在的行为 | e728ce3 |
| 终审 | 门禁 990 通过 | 最后一次门禁：41 个测试二进制，992 通过 / 0 失败 / 1 忽略（终审修正新增 2 条用例） | 见上面几行 | e728ce3 |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | M3a 的组环检测仍把 `smart` 组写进去的嵌套组当作边：加载时 `W0030`、运行期 REJECT，虽然 `smart` 实际忽略它（P20） | 单独小改动 |
| 2 | 改了策略的 `test-url` / `test-timeout` 时，`SmartBook` 里该策略的记录清零（"同一定义"按出站对象判断，P2） | 有用户报告再说 |
| 3 | DNS 会话不判断"上游先断"（P3） | 有用户报告再说 |
| 4 | `CORE_VERSION` 仍报告 20：FR-CFG-08 写"阶段 2 完成 smart 组后报告 22"，也写"每个阶段评审时决定是否上调" | 阶段 2 收尾（M8）时由项目所有者决定 |
| 5 | 站点按目标主机名原样区分（`a.example.com` 与 `b.example.com` 是两个站点，不按可注册域名合并） | 有用户报告再说 |
| 6 | UDP 响应延迟与 UDP 的静默失败不计入 `smart` 的打分（M3-D11） | M5 |
| 7 | 重载改了（或删掉）某个策略之后的短时间内（连接前的等待 + 至多 10 秒连接 + 3 秒无响应），经上一代出站的迟到回报仍会写进 `SmartBook`：该策略的记录换回旧出站、从头积累；已删掉的策略的记录、站点记忆与使用计数会在 `retain` 之后重新出现，直到下一次发布（记录）或至多一小时（站点） | 有用户报告再说（做法：回报前核对出站仍属当前一代） |
| 8 | 改了策略的定义（例如换了服务器）之后，该策略名下的站点记忆仍是旧定义的结果，至多一小时（站点记忆按策略名记，P14） | 有用户报告再说 |
| 9 | 拨号路径的开销：每个成员各取一次 `SmartBook` 的记录锁（设计第 10 节写的是每次选择一把短锁）；`rank` 建出完整的重试列表、`attempts` 再整份克隆，而引擎最多只用两个；站点记满 4096 个时逐个扫描找最久没用的 | 有性能数据再说 |
| 10 | 只靠阅读代码核对、还没有用例的行为：`test_slot` 经带套接字选项的 `direct` 别名与桌面上代替 DIRECT 的内置策略（`CELLULAR` 等）；被取代的测试不推给 `SmartBook`；`retain` 清理站点记忆与使用计数；`policy-priority` 的几个模式都匹配时取第一个；整组都是尚未实现协议的 `smart` 组在引擎里只拨一次、得到带说明的 REJECT | 顺手补用例 |
| 11 | 目标网站本身不通（站点下线、拒绝连接）时，失败也计入成员的全局分数（每次 800 ms、5 分钟减半）：一个后台应用反复访问一个失效的地址，就能把整组的排序拉乱，访问足够频繁时全部成员都被判为失败。设计 4.3 / 5.3 本就如此规定，Surge 的做法手册没有写。可选的改法：(a) 只把指向成员自己服务器的失败（连接、TLS、握手超时、服务器名解析失败）计入全局分数，目标一侧的结果（代理回的目标错误、首字节之前就断开）只进站点记忆；(b) 同一主机最近经别的成员也失败过时，只记站点、不罚全局分数；(c) 本地就拒绝的目标名（超长、不合法）既不计入也不换成员 | 项目所有者决定（在把 `smart` 当作默认组推荐之前） |
| 12 | 客户端正在上传、上游在回话之前重置连接时，错误先由上行的写端发现：不标记"上游先断"，这次失败不计入成员 | 有用户报告再说 |
