# 阶段 2 / M3b「测速与自动组」Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 `url-test` / `fallback` / `load-balance` 组按连通性测试的结果自动选成员：经每个成员自己的出站在同一条连接上发两次 `HEAD` 计分，结果跨重载保留；拨号触发过期组的后台重测，`evaluate-before-use` 的组第一次使用前等第一轮测完；自动组可经 `POST /v1/policy_groups/select` 临时覆盖；新增三个测试端点；能力表翻转这三种组。先做项目所有者定下的两项承接决定（订阅行不能动用主配置的证书与策略；空组当中继一律拒绝），并处理 M3a 留给本计划的两条拨号入口问题。

**Architecture:** `rurge-policy` 新增三个模块：`probe`（一次测试：`Outbound::connect_tcp` → 可选 TLS → `hyper` HTTP/1 客户端连接层上两次 `HEAD`）、`testbook`（`TestBook`：按策略保存结果、同一策略同时只测一次、全进程最多 8 个测试、经 `TestObserver` 让引擎把每次测试记成会话）、`auto`（三种选法的纯函数、`SelectCtx`、`AutoGroups`：临时覆盖、`url-test` 保持的成员、每组上一轮测试的时间、测试请求的去重与投递）。注册表按测试结果选成员：只有拨号（`live`）会请求一轮测试、让 `url-test` 换成员，控制面的读取不会；`evaluate-before-use` 的组没测过时在 `Resolution.pending` 里说出来。`rurge-engine` 起一个随引擎存续的调度任务，按请求在当时的注册表上跑一轮测试；拨号时带上 `SelectCtx { host }`，遇到 `pending` 就在有界时间内等这一轮结束；重载时 `AutoGroups::retain` 只留下定义没变的组的覆盖。`rurge-api` 新增三个端点并扩展 `select`；bin 的能力表翻转三种组。

**Tech Stack:** Rust 1.89 / edition 2024；`hyper` 1.x 的 `client::conn::http1`、`hyper-util` 的 `TokioIo`、`http-body-util` 的 `Empty`、`tokio-rustls` / `rustls`（ring 提供者）、`url` 2、`tokio` 的 `watch` / `mpsc` / `Semaphore`——全部是工作区已有的依赖，**不新增任何第三方 crate**。

**Spec:** `docs/superpowers/specs/2026-09-23-phase2-m3-groups-subscriptions-design.md`（第 1.4 节 M3b 行、第 2 节 M3-D3 / D6 / D7 / D8 / D10、第 6 节、第 8 ～ 12 节、第 14 节 V1 / V7 / V8 / V9、第 15 节 M3b 草图）；M3a 计划 `docs/superpowers/plans/2026-09-23-phase2-m3a-subscriptions-plan.md` 末尾「延后事项」里标给 M3b 的条目。与本计划「计划期决定」表不一致处，以该表为准，并由 Task 11 写回设计文档新增的第 17 节。

## Global Constraints

- MSRV **1.89**，edition 2024。`unsafe_code`：全工作区 `forbid`；只有 `rurge-platform` 是 `deny` + 唯一一个 `#[allow(unsafe_code)]` 函数。**本计划不新增任何 unsafe。**
- 依赖方向不变：`rurge-policy` 不依赖 `rurge-engine`，也不依赖任何具体协议实现（探针只用 `Outbound` 与 TLS 根证书）；测试会话经 `TestObserver` trait 注入；`rurge-engine` / `rurge-api` / `rurge-policy` 不依赖 `rurge-platform`；平台代码只在 `rurge-platform`（AR-02）。**不新增任何第三方依赖**：`rurge-policy` 引用工作区已有的 `bytes` `http` `http-body-util` `hyper` `hyper-util` `rustls` `tokio` `tokio-rustls` `tracing` `url`（dev：`rurge-net` / `rurge-proto` 的 `testing` 特性），`rurge-engine` 与 `rurge-api` 各引用 `url`；`Cargo.lock` 只多这几个 crate 依赖列表里的行，不下载任何东西。
- **测试绝不碰公网**：只用回环 + 端口 0 + 有界等待；不用固定 sleep 当同步手段（轮询 + 截止时间）。**任何带 `url-test` / `fallback` / `load-balance` 组的测试配置，`proxy-test-url` 与 `internet-test-url` 都必须指向回环**——默认值 `http://bing.com/` 在调度任务跑起来之后会被真的访问。引擎用例的 `Profile::text` 与 API 用例的夹具由本计划改为默认指向回环（`http://127.0.0.1:9/`，没人监听），不要删掉它。
- **测试与任何命令都不得修改本机的系统代理、注册表、网络设置，不得注册真实服务 / 计划任务**：不在 CLI 测试夹具之外运行 `rurge run --system-proxy`；不运行不带 `--dry-run` 的 `rurge service install | uninstall`。
- **不在本机下载或安装任何东西**（不装 sing-box、不装 xray、不 `rustup target add`、不 `cargo install`，也不装 `cargo-insta`）。
- **凭据及其派生物永不外泄**（M3-D7）：测试 URL 可能来自订阅行（`test-url=` 里带 token），所以它永不进日志、错误文本与请求记录——日志只写策略名；测试会话的目标只有测试 URL 的主机与端口；探针的失败原因不引用 URL。订阅行、订阅链接、`external-policy-modifier` 的值照 M3a 的规矩。
- rurge 专有的运行时选项只经命令行与环境变量提供，不扩展 Surge 配置格式（FR-CFG-17）。与 Surge 的每一处行为差异都登记进 `docs/surge-compatibility-matrix.md`。
- 日志、错误文本、CLI 输出、代码注释用英文；文档与提交标题用中文。注释密度、命名、惯用法与相邻代码保持一致；注释里不写评审轮次的标签。
- 提交信息结尾两行：`Co-Authored-By: <执行者自己的署名>` 与 `Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th`。**不 push、不 merge**，不 amend。
- 每个任务结束的门禁（全部通过才算完成）：

  ```bash
  RUSTFMT="C:\Users\SZV01065\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe" cargo fmt --all --check \
    && cargo clippy --all-targets -- -D warnings \
    && timeout 1500 cargo test --workspace --no-fail-fast
  ```

  `timeout` 不能省：`rurge-dns` 的一个用例曾让测试进程以 100% CPU 空转数小时（M3a「延后事项」#20）。测试二进制异常退出而没有失败用例时（`STATUS_ACCESS_VIOLATION`、`STATUS_HEAP_CORRUPTION` / `0xc0000374`、段错误——本机已知的既有问题，见 P21），或整轮被 `timeout` 杀掉时，重跑一次并保留两次的日志，**不要在任务里去修它**。
- 第三方 crate 的 API 以本机源码为准：`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`。
- 本机的 bash 处理不了超过约 8 KB 或引号复杂的 heredoc：新文件一律用写文件的工具落盘，不用 heredoc。

## Review Focus

设计没有逐条写到、而最可能伤到使用者的五类输入或失败方式；每一条都在负责它的任务里配了用例。

1. **测试 URL 里的 token 走漏**：订阅行常把 token 放进 `test-url=`；测试失败的原因、"服务端不保持连接"的告警、请求记录里的测试会话、API 的测试结果——没有一处能带出 URL 的路径与参数。用例：Task 4 `a_failure_says_which_step_failed`（原因只说哪一步失败）；Task 7 `every_test_is_a_session_of_the_request_log`（目标只有主机与端口）；Task 8 `policies_are_tested_on_request`。
2. **测试服务器慢、挂住、或测试任务自己死掉**：每次测试受超时约束，全进程最多 8 个同时进行；一个没有结果就结束的测试（任务 panic）不能让这个策略从此再也测不了；`evaluate-before-use` 的等待有上限。用例：Task 5 `no_more_than_eight_tests_run_at_once`、`a_test_that_died_is_run_again`、`a_test_ends_even_when_nobody_waits_any_more`；Task 7 `a_round_may_take_a_timeout_per_eight_tests`、`evaluate_before_use_waits_for_the_first_round`。
3. **服务端不保持连接、或回一个错误状态码**：分数退化为第一次的完整往返并告警一次；4xx / 5xx 也是"连得上"。用例：Task 4 `a_connection_that_is_not_kept_gives_the_first_round_trip`、`any_status_passes`。
4. **重载与测试、覆盖交错**：定义没变的组保留覆盖，变了就清除；重载拿掉了一个已请求测试的组，这个请求仍要结束，否则同名的新组永远不会再被测；会话不会拿旧一代的规则选名、到新一代的注册表里解析。用例：Task 6 `a_round_of_a_group_that_is_gone_ends_the_request`；Task 7 `a_reload_keeps_an_override_while_the_group_stays_the_same`；Task 9 的 `Engine::snapshot`（按构造保证，见 P15）。
5. **组套组、覆盖的成员消失、空组当中继**：嵌套组按它当前的选择计分；覆盖的成员被订阅更新删掉时覆盖失效；空组当中继时拒绝而不是直连。用例：Task 2 `an_empty_group_as_a_relay_rejects`、`a_relay_group_without_members_refuses`；Task 6 `a_group_member_scores_by_its_pick`、`an_override_lasts_while_the_group_stays_the_same`；Task 7 `a_derived_member_is_tested_through_its_relay`。

## 计划期决定

写计划时对照设计、Surge 手册与本仓库源码核对后定下的事；与设计文档文字不同的，由 Task 11 写回设计文档第 17 节。

**本计划里的代码不是凭空写的。** 全部 11 个任务的改动在仓库的一份副本上按任务顺序真实做了一遍，每个任务之后跑一次全工作区门禁：最后一次是 **40 个测试二进制，923 通过 / 0 失败 / 1 忽略**（本计划开工前的 main 是 39 个二进制、880 通过）；`rurge-engine --test auto_groups` 与 `rurge-policy` 的全部单元用例各连跑 40 轮、`rurge-api` 15 轮、`pipeline` 10 轮，无一断言失败（各有 1 轮以 `STATUS_HEAP_CORRUPTION` 退出，即 P21 的既有崩溃）。计划里新文件的全文取自副本上该任务的提交，修改处的"把 … 换成 …"由脚本从相邻两个任务提交的差异生成，并在拼好之后按计划的顺序套到开工前的源码上逐字核对过——计划文本与验证过的代码一字不差。

| # | 事项 | 决定与依据 |
| - | ---- | ---------- |
| P1 | M3a「延后事项」#11：订阅行能按名字用到主配置的私有材料 | 项目所有者 2026-09-25 定为"只认修饰里设的"：订阅行**自己写的** `client-cert=`、以及指向主配置策略或组的 `underlying-proxy=` 不生效，该行跳过并 `W0023`（固定说法，不带取值）；经 `external-policy-modifier` 设上的照常生效；订阅内部导入行之间的中继仍允许。检查在 `Imports::collect` 里、加前缀与修饰**之前**做（看的是订阅作者写的原行）。登记进兼容性清单（`policy-path` 行） |
| P2 | M3a「延后事项」#12：空组被当作中继时回退 DIRECT | 项目所有者 2026-09-25 定为"中继时一律拒绝"：新增 `PolicyRegistry::resolve_relay(name)`，它以 `EmptyGroup::Reject` 解析；`ChainConnector` 与引擎的 `socket_opener` 都改用它。作为规则或全局策略直接使用的空组仍按 M3-D3（默认 DIRECT） |
| P3 | V1：`[General] test-timeout` 分不出"没写"与"写了 5" | `General.test_timeout: Option<Duration>`（默认 `None`）；新增 `General::test_target(own_url, own_timeout, direct) -> (&str, Duration)`：URL 取策略自己的 → `internet-test-url`（直连类）/ `proxy-test-url`；超时取策略自己的 → 全局 → 10 秒（直连类）/ 5 秒。"直连类"是内置 `DIRECT`、`direct` 别名，以及在桌面上代替 DIRECT 的 iOS 专属内置策略。`test-url` / `test-timeout` 从 `W0029` 的"解析但未生效"名单里去掉（`test-udp` 留到 M5）。语料库快照不含这个字段，不受影响；没有别的调用方 |
| P4 | V8：`hyper` 的 HTTP/1 客户端连接层 | `hyper::client::conn::http1::handshake(TokioIo::new(stream))` 得到 `(SendRequest, Connection)`；连接驱动放进一个任务，`Driver` 在测试结束时 `abort` 它。第一次 `HEAD` 的响应头没有 `Connection: close` 且 `sender.ready()` 成功，才算"连接可复用"，这时发第二次并以它的耗时计分；第二次仍因连接关闭 / 不完整 / 取消而失败时，退回第一次的完整耗时（`reused = false`）。请求行用 origin-form，`Host` 带显式端口，`User-Agent: rurge/<版本>`。HTTPS 经 `tokio-rustls`，ring 提供者，ALPN 只报 `http/1.1`（报 `h2` 会让 HTTP/1 握手失败）；IP 字面量的测试 URL 也能用（`ServerName::IpAddress`） |
| P5 | HTTPS 测试 URL 的根证书 | 设计写"系统根证书；测试注入 `EngineShared.roots`"。`TestBook` 跨代存续、在 `EngineShared::new` 里创建，而测试夹具是用 `EngineShared { roots: Some(…), ..EngineShared::default() }` 注入 CA 的——在 `TestBook` 里另存一份根证书会与出站的对不上。所以根证书随每个 `TestCase` 走：`OutboundFactory` 新增 `roots()`，注册表构建时取一次，`test_case` 填进去；于是测试与出站的 TLS 永远同源 |
| P6 | `TestBook` 的形状 | 每个策略只存一条结果，带着"测试时的指纹" `key`（定义行、测试 URL、超时三者的哈希）；键不同的结果视同没有。同一 `(策略, key)` 同时只有一个测试（`watch` 通道，后来者等它）；测试在自己的任务里跑，等的人走了测试照样结束并保存结果；一个没有结果就结束的测试（任务 panic）不会让这个策略从此测不了——`running` 里关闭了的通道在下一次请求时被替换，并记一条 ERROR。全进程并发上限 `MAX_CONCURRENT_TESTS = 8`（信号量）。`invalidate_all()` 预留给"网络已变化"。设计 6.2 的"带 `test` 标记"：请求记录没有标记字段，测试会话是内部会话、`rule` 为 `policy test`（Task 7）。API 给了 `url` 的测试走 `test_once`：同样受并发上限、同样记成会话，但**不保存**——否则它会挤掉该策略按自己 URL 测得的结果 |
| P7 | 设计 6.3："组被解析到、且成员结果比 `interval` 旧"时测 | 只有拨号（引擎的会话、DNS 会话、链的中间跳）会请求测试、会让 `url-test` 换成员；控制面的读取（`GET /v1/policy_groups/select`、`GET /v1/policy_groups`）只看不动——否则刷新一次 Dashboard 就能让所有组各测一轮。"旧"按**组**上一轮测试结束的时间判断（`AutoGroups::last_round`），不逐个看成员结果的年龄。一轮测试覆盖嵌套组的成员，并给每个被覆盖的组记一轮。一轮在跑完之前不会被重复请求；请求在调度任务接上之前先攒着；对一个重载后已经不存在的组的请求仍要结束（否则同名的新组再也不会被测） |
| P8 | 设计 6.4 的细节 | `url-test` 保持的成员存在 `AutoGroups`（跨代），只由拨号更新。嵌套组作为成员时的分数：`select` 按它当前的选择；`url-test` / `fallback` 按它当前选中的成员；`load-balance` 取通过者的均值——成员都还没测过时算"未知"，测过而没有一个通过才算失败（设计写的是"没有通过者算失败"）；成环的组算失败；空组算未知。控制面显示 `load-balance` 的当前成员时取第一个通过的（随机选出的每次不同）。`smart`（M3c）与 `subnet`（阶段 3）仍取第一个成员 |
| P9 | 设计 6.3 / 9：`evaluate-before-use` 的等待 | `Resolution.pending` 带出链上最外层一个还没测过的 `evaluate-before-use` 组；引擎（`resolve_ready`）订阅 `AutoGroups::rounds()` 后再看 `last_round`（不会错过恰好在中间结束的一轮），等待上限是 `PolicyRegistry::round_timeout(group)`：该组（含嵌套组）成员里最长的测试超时 × 每 8 个一批的批数。等完仍没有通过的成员 → 这次连接以 `policy group evaluation failed` 失败，会话的链记到该组为止；否则重新解析一次。DNS 会话同样等待 |
| P10 | V9：链的中间跳（`ChainConnector`） | 仍用 `resolve_relay` 同步解析：它是"拨号"（会请求测试、会让 `url-test` 换成员），但**不等** `evaluate-before-use`——`Connector::connect` 里等一轮测试会把上层会话的连接超时吃掉。中间跳没有目标主机名，`load-balance` + `persistent` 当中继时退化为随机 |
| P11 | `SelectCtx.host` | 会话目标主机的文本（IP 目标就是 IP 的文本）；M3c 的站点记忆也用它 |
| P12 | 设计 6.5 的 `GroupState` | 叫 `AutoGroups`，在 `rurge-policy::auto`，由 `EngineShared.auto` 持有、注册表各代共用。覆盖按组名保存，连同设覆盖时的 `GroupSpec`（去掉 `span`）；`retain(&cfg.group_specs)` 在 `publish_generation` 里调用（配置重载），定义变了或组消失的覆盖清除，消失的组的 `url-test` 保持成员与上一轮时间一并清除。订阅重建不改组定义，不调用它；覆盖的成员从成员表里消失时 `override_of` 忽略它并告警一次 |
| P13 | 设计 6.6 的 API 形状 | 结果是 `{"delay": <毫秒>, "time": <Unix 秒>}` 或 `{"error": <原因>, "time": …}`。`POST /v1/policies/test`：名字先校验（未知 → 400 `` unknown policy `X` ``）；`url` 省略或为空串 → 各自的测试 URL、结果保存；给了 → `test_once`，不保存；不能测的（组、REJECT 族、未实现的协议）→ `{"error": "not testable"}`；全部同时测，测完才返回。`GET /v1/policy_groups/test_results` 只列三种自动组（`smart` 随 M3c），没有结果为 `null`。`POST /v1/policy_groups/test` 任何类型的组都能测，未知组 → 400。`select` 对三种自动组即覆盖（空串清除），`smart`（M3c 之前）与 `subnet` 仍 400，文本从 `` `G` is not a select group `` 改为 `` `G` does not take a selection ``；覆盖不写 `state.json` |
| P14 | 调度任务 | `Engine::new` 里 `start_tests`：给 `TestBook` 装上 `TestSessions` 观察者（持 `Weak<Engine>`），`AutoGroups::connect()` 拿到请求的接收端，一个任务逐个取组名，在**当时**的注册表上 `spawn(registry.test_group(group))`。任务只持 `Weak<Engine>`，引擎没了就退出；`AutoGroups` 被释放（发送端随之释放）时也退出 |
| P15 | M3a「延后事项」#14：会话先取当前代、后取注册表 | 新增 `Engine::snapshot()`：在代际锁下成对读取运行时与注册表。两处发布都持这把锁（重载的"发布 + 切换"、订阅重建的"核对 + 发布"），锁里不做 I/O，拨号只多一次无竞争的加锁。这个竞争窗口只有几条指令，没有稳定复现的用例，按构造保证 |
| P16 | M3a「延后事项」#3（C4）：链底下的 REJECT 到不了 `dial_internal` 的旁路；链深超限时两边说法不一 | `dial_internal` 里 `socket_opener` 返回 `None`（链底是 REJECT，或比 `MAX_DEPTH` 还深——注册表对后者也是 REJECT）时，同 REJECT 一样告警并直连（`dns-follow: reject bypassed to keep DNS working`），不让 DNS 查询失败 |
| P17 | 能力表 | 翻转 `url-test` / `fallback` / `load-balance`。设计 1.4 的 M3b 行写"`W0008` 只剩 `subnet`"，但 `smart` 要到 M3c 才翻转：M3b 之后 `W0008` 会因 `smart` 与 `subnet` 出现（与设计第 8 节一致）。翻转前核对了设计承诺的守卫都已在：测速类参数的校验与 `W0028`（M3a），`test-url` / `test-timeout` 生效（Task 3） |
| P18 | V7：端到端里的快慢 | `FakeHttpProxy` / `FakeSocks5` 的延迟只作用于它们自己的握手应答，两次 `HEAD` 都不受影响；改用回环 `TestServer` 的按路径延迟（`set_delay`），各策略用自己的 `test-url` 指向不同路径（`/slow` 慢 300 毫秒、`/fast` 不慢）。`TestServer` 新增 `spawn_tls_with(acceptor)`，让 HTTPS 探针用例用 `TlsFixture` 的证书 |
| P19 | 任务的切分 | 设计草图约 8 个任务，本计划 11 个：两项承接决定（Task 1、2）先做；M3a 延后事项 #3 / #14 单独成 Task 9；文档 Task 11 |
| P20 | 测试会话进请求记录 | 引擎的 `TestSessions`：每次测试开一个 `Internal` 会话（经 `new_handle`，所以进活动表与环形缓冲），`rule` 为 `policy test`，`policy` 是被测的策略，目标是测试 URL 的主机与端口；测试结束时 `Completed` 或 `Failed(原因)`。字节数不计（探针的流不经 `Counting`，见「延后事项」） |
| P21 | 既有的偶发崩溃 | 写计划时在副本上连跑：`auto_groups` 40 轮、`rurge-policy` 单元 40 轮、`rurge-api` 15 轮、`pipeline` 10 轮，各有 1 轮以 `STATUS_HEAP_CORRUPTION`（`0xc0000374`）退出，没有任何断言失败。对照组是未改动的 main：`pipeline` 30 轮 1 次，`rurge-api` 与 `rurge-policy` 单元各 30 轮 0 次——崩溃本身是既有的（M3a 计划 P22、M2b 计划记下的 `STATUS_ACCESS_VIOLATION` 同类，根因在某个依赖的原生代码里，未查明），M3b 的用例多用了 TLS 与短连接，可能让它更常出现。本计划不处理；门禁遇到时重跑（Global Constraints），并在「延后事项」里跟踪 |

## 承接事项

之前计划「延后事项」表里标给 M3b 的条目，以及写本计划时要复核的。

| # | 来源 | 事项 | 处理 | 任务 |
| - | ---- | ---- | ---- | ---- |
| C1 | M3a #11 | 订阅行能按名字用到主配置的 `client-cert` 与策略 | 项目所有者 2026-09-25 决定，见 P1 | 1 |
| C2 | M3a #12 | 空组当中继时回退 DIRECT，依赖它的策略直连自己的服务器 | 项目所有者 2026-09-25 决定，见 P2 | 2 |
| C3 | M1b C5 / M3a | `test-url` / `test-timeout` 的使用 | Task 3（取值）、Task 6（注册表按它们建 `TestCase`）；"请求记录的计时字段"属 M3c | 3、6 |
| C4 | M3a #3（C4） | 链底下的 REJECT 到不了 `dial_internal` 的旁路 | 见 P16 | 9 |
| C5 | M3a #14 | 一个会话先取当前代、后取注册表 | 见 P15 | 9 |
| C6 | M3a #2（C3） | `ChainConnector` 没有运行期的深度守卫 | 不变：M3b 没有改变链的构成，有界性仍由装配期的环检查静态保证；仍在 M8 | — |
| C7 | M3a #4、#20 | 测试二进制偶发崩溃；`rurge-dns` 用例空转 | 见 P21；门禁一律套 `timeout` | — |

## File Structure

新建：

| 文件 | 职责 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-policy/src/probe.rs` | 一次测试：`probe(outbound, url, timeout, roots) -> Probed`，与回环用例 | 4 |
| `crates/rurge-policy/src/testbook.rs` | `TestBook`、`TestCase`、`TestResult`、`TestObserver` / `TestRecord`、`MAX_CONCURRENT_TESTS`，与用例 | 5 |
| `crates/rurge-policy/src/auto.rs` | `SelectCtx`、`Standing`、`url_test` / `fallback` / `load_balance`、`AutoGroups`，与用例 | 6 |
| `crates/rurge-engine/src/auto.rs` | 引擎一侧：`TestSessions` 观察者、调度任务 `start_tests`、`resolve_ready`（7）；`test_policies` / `test_group` / `test_results`、`automatic`（8） | 7、8 |
| `crates/rurge-engine/tests/auto_groups.rs` | 自动组经整个引擎的用例 | 7、8 |

修改：

| 文件 | 改动 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-policy/src/assemble.rs` | 订阅行的 `client-cert` 与指向主配置的 `underlying-proxy` 不生效，与用例 | 1 |
| `crates/rurge-policy/src/{registry.rs, cell.rs}` | `resolve_relay`（2）；`Resolution.pending`、组条目带 `GroupSpec`、`TestSpec`、`build` 多一个 `auto` 参数、`choose` / `standing` / `test_case` / `test_result` / `available` / `test_group` / `resolve_with`（6）；`round_timeout`（7）；用例 | 2、6、7 |
| `crates/rurge-policy/src/{lib.rs, factory.rs, testing.rs}`、`crates/rurge-policy/Cargo.toml` | 模块声明（4、5、6）；`OutboundFactory::roots`（6）；测试夹具（6）；依赖（4） | 4 – 6 |
| `crates/rurge-config/src/{general.rs, spec/common.rs}` | `test_timeout: Option`、`test_target`；`W0029` 名单去掉两项 | 3 |
| `crates/rurge-net/src/testing.rs` | `TestServer::spawn_tls_with` | 4 |
| `crates/rurge-engine/src/{engine.rs, shared.rs, runtime.rs, subscriptions.rs, outbounds.rs, views.rs, lib.rs}`、`crates/rurge-engine/Cargo.toml` | `socket_opener` 用 `resolve_relay`（2）；`EngineShared.auto`、`build` 的新参数、`EngineFactory::roots`（6）；调度、拨号等待、`retain`（7）；`select` 扩展、导出（8）；`snapshot` 与 DNS 旁路（9） | 2、6 – 9 |
| `crates/rurge-engine/tests/{subscriptions.rs, common/mod.rs, outbounds.rs, pipeline.rs}` | 空组中继（2）；测试 URL 默认指向回环（7）；可选中的组（8）；DNS 旁路（9） | 2、7 – 9 |
| `crates/rurge-api/src/{lib.rs, routes/policies.rs, routes/policy_groups.rs}`、`crates/rurge-api/Cargo.toml`、`crates/rurge-api/tests/api.rs` | 三个端点、结果的 JSON、`select` 扩展与用例 | 8 |
| `crates/rurge/src/capabilities.rs`、`crates/rurge/tests/cli.rs` | 能力表翻转与用例 | 10 |
| 文档（清单、API 参考、手工验收、M3 设计第 17 节、M3a 计划的延后事项、两份 README、CLAUDE.md、本计划末尾两张表） | 见 Task 11 | 11 |

## 任务一览

| 任务 | 交付物 | 依赖 |
| ---- | ------ | ---- |
| 1 | 订阅行不能动用主配置的 `client-cert` 与策略（C1） | — |
| 2 | 空组当中继一律拒绝：`resolve_relay`（C2） | — |
| 3 | `[General] test-timeout` 区分未设置；`test_target`；`test-url` / `test-timeout` 生效 | — |
| 4 | 探针：两次 `HEAD`、HTTPS、不保持连接时的退化 | — |
| 5 | `TestBook` | 4 |
| 6 | 三种选法与 `AutoGroups`；注册表按测试结果选成员 | 3、5 |
| 7 | 引擎：调度任务、测试会话、拨号的等待与 `SelectCtx`、重载保留覆盖 | 6 |
| 8 | 三个测试端点与 `select` 对自动组的覆盖 | 7 |
| 9 | 拨号入口的两条承接问题（C4、C5） | 7 |
| 10 | 能力表翻转 | 8 |
| 11 | 文档 | 10 |

---


### Task 1: 订阅行不能动用主配置的 `client-cert` 与策略（承接 C1）

订阅是别人写的内容。今天一条订阅行可以写 `client-cert=<主配置里的 Keystore 条目>`，或 `underlying-proxy=<主配置的策略 / 组>`，于是订阅作者能让 rurge 向他指定的主机出示用户的客户端证书、或经用户自己的代理连过去；M3b 起自动测速会在用户选中该节点之前就这样做。项目所有者 2026-09-25 定为"只认修饰里设的"（P1）：这两项只有用户自己经 `external-policy-modifier` 设上的才生效；订阅行自己写的，该行跳过并 `W0023`（固定说法，不带取值）。指向订阅里另一条导入行、或指向 `DIRECT` 的中继是订阅自己的事，照常允许。

**Files:**
- Modify: `crates/rurge-policy/src/assemble.rs`（新函数 `reaches_into_profile`，`Imports::collect` 里调用；两条已有用例改用修饰设中继；一条新用例）

**Interfaces:**
- Consumes: M3a 的 `Imports::collect`（`cfg`、组的 `modifier: &[(String, String)]`、订阅行 `ProxyPolicy`）、`Config::name_kind`、`codes::W_SET_LINES_SKIPPED`（`W0023`）。
- Produces: 无新的公开接口。装配结果里被跳过的行多了两种原因：
  - `` policy group `G`: `policy-path` line N: a subscription line's own `client-cert` is not honoured (only `external-policy-modifier` may set it); skipped ``
  - `` policy group `G`: `policy-path` line N: a subscription line's own `underlying-proxy` may not name a policy or group of the profile (only `external-policy-modifier` may); skipped ``

- [ ] **Step 1: 用例先行**

两条已有的成环用例原来靠订阅行自己的 `underlying-proxy` 指向主配置；新规则下它们要改用修饰设中继才还在测"成环的导入行被丢弃"。新用例钉住三种被跳过的行与两种照常可用的行：

`crates/rurge-policy/src/assemble.rs`——把

```rust
    /// dialling: the import that closes it is left out.
```

换成

```rust
    /// dialling: the import that closes it is left out. (Only the user's own
    /// modifier can point an imported line at the profile's policies.)
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
            "Pool = select, policy-path=https://sub.test/p",
```

换成

```rust
            "Pool = select, policy-path=https://sub.test/p, include-other-group=Other, \
external-policy-modifier=\"underlying-proxy=Entry\"\nOther = select, policy-path=https://sub.test/o",
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
                &[(
                    "Pool",
                    "Loop = http, l.test, 80, underlying-proxy=Entry\nFine = http, f.test, 80",
                )],
```

换成

```rust
                &[
                    ("Pool", "Loop = http, l.test, 80"),
                    ("Other", "Fine = http, f.test, 80"),
                ],
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
    /// through the group whose relay imported it would never finish
    /// dialling, so it is left out and the group's own members keep theirs.
```

换成

```rust
    /// through the group whose relay reaches it would never finish dialling,
    /// so it is left out and the other members keep theirs. (Only the user's
    /// own modifier can point an imported line at a group of the profile.)
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
            "G = select, A, underlying-proxy=R\nR = select, policy-path=https://sub.test/r",
```

换成

```rust
            "G = select, A, underlying-proxy=R\n\
R = select, policy-path=https://sub.test/r, include-other-group=M\n\
M = select, policy-path=https://sub.test/m, external-policy-modifier=\"underlying-proxy=G\"",
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
                &[(
                    "R",
                    "X = http, x.test, 80, underlying-proxy=G\nY = http, y.test, 80",
                )],
```

换成

```rust
                &[("R", "Y = http, y.test, 80"), ("M", "X = http, x.test, 80")],
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
        assert_eq!(members(&a, "R"), ["Y"]);
```

换成

```rust
        assert_eq!(members(&a, "R"), ["Y"]);
        assert!(members(&a, "M").is_empty());
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
                "policy group `R`: `policy-path` line 1: the `underlying-proxy` of `X` leads back to the policy itself; skipped"
                    .to_string()
            )]
```

换成

```rust
                "policy group `M`: `policy-path` line 1: the `underlying-proxy` of `X` leads back to the policy itself; skipped"
                    .to_string()
            )]
        );
    }

    /// A subscription is somebody else's content: its lines may not reach
    /// for the profile's own material by name — a `[Keystore]` item, a
    /// policy or group as a relay — unless the user's own modifier sets that
    /// parameter. A relay to another imported line stays allowed.
    #[test]
    fn a_subscription_line_may_not_reach_into_the_profile() {
        let cfg = profile(
            "Corp = http, corp.test, 80",
            "G = select, policy-path=https://sub.test/g\n\
Pool = select, Corp\n\
H = select, policy-path=https://sub.test/h, external-policy-modifier=\"underlying-proxy=Corp\"",
        );
        let a = assemble(
            &cfg,
            &snapshots(
                &cfg,
                &[
                    (
                        "G",
                        "Cert = https, c.test, 443, client-cert=corp-cert\n\
Relay = http, r.test, 80, underlying-proxy=Corp\nVia = http, v.test, 80, underlying-proxy=Pool\n\
Inner = http, i.test, 80, underlying-proxy=Hop\nHop = http, h.test, 80",
                    ),
                    ("H", "Mod = http, m.test, 80, underlying-proxy=Elsewhere"),
                ],
            ),
        );
        assert_eq!(members(&a, "G"), ["Inner", "Hop"]);
        assert_eq!(members(&a, "H"), ["Mod"]);
        let relay = |name: &str| {
            a.imported
                .iter()
                .find(|i| i.policy.name == name)
                .and_then(|i| i.spec.as_ref())
                .and_then(|s| s.common.underlying_proxy.clone())
        };
        assert_eq!(relay("Inner").as_deref(), Some("Hop"));
        assert_eq!(relay("Mod").as_deref(), Some("Corp"));
        assert_eq!(
            warnings(&a),
            [
                (
                    codes::W_SET_LINES_SKIPPED,
                    "policy group `G`: `policy-path` line 1: a subscription line's own `client-cert` is not honoured (only `external-policy-modifier` may set it); skipped".to_string()
                ),
                (
                    codes::W_SET_LINES_SKIPPED,
                    "policy group `G`: `policy-path` line 2: a subscription line's own `underlying-proxy` may not name a policy or group of the profile (only `external-policy-modifier` may); skipped".to_string()
                ),
                (
                    codes::W_SET_LINES_SKIPPED,
                    "policy group `G`: `policy-path` line 3: a subscription line's own `underlying-proxy` may not name a policy or group of the profile (only `external-policy-modifier` may); skipped".to_string()
                ),
            ]
```

Run: `cargo test -p rurge-policy assemble`

Expected: FAIL——`a_subscription_line_may_not_reach_into_the_profile` 的第一条断言：`left: ["Relay", "Via", "Inner", "Hop"]`、`right: ["Inner", "Hop"]`（`Cert` 今天因 Keystore 条目不存在被跳过，但原因不同，警告那条断言也对不上）。两条改过的成环用例照常通过。

- [ ] **Step 2: 写实现**

`crates/rurge-policy/src/assemble.rs`——把

```rust
        ));
    }
}

/// The policies groups took in, each with the group that brought it.
```

换成

```rust
        ));
    }
}

/// Why subscription line `p` may not be taken in: it reaches by name for
/// the profile's own material — a `[Keystore]` item, or a policy or group of
/// the profile as its relay — which only the user may hand it, through the
/// group's `external-policy-modifier`. A subscription is somebody else's
/// content, and a relay or a client certificate would be used as soon as the
/// line is tested, chosen or not. A relay to another imported line, or to
/// `DIRECT`, is the subscription's own business.
fn reaches_into_profile(
    cfg: &Config,
    p: &ProxyPolicy,
    modifier: &[(String, String)],
) -> Option<&'static str> {
    let set_by_modifier = |key: &str| modifier.iter().any(|(k, _)| k.eq_ignore_ascii_case(key));
    if p.params.get("client-cert").is_some() && !set_by_modifier("client-cert") {
        return Some(
            "a subscription line's own `client-cert` is not honoured (only `external-policy-modifier` may set it)",
        );
    }
    if let Some(relay) = p.params.get("underlying-proxy")
        && !set_by_modifier("underlying-proxy")
        && matches!(
            cfg.name_kind(relay.trim()),
            Some(NameKind::Policy(_) | NameKind::Group)
        )
    {
        return Some(
            "a subscription line's own `underlying-proxy` may not name a policy or group of the profile (only `external-policy-modifier` may)",
        );
    }
    None
}

/// The policies groups took in, each with the group that brought it.
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
                let line = p.span.line;
```

换成

```rust
                let line = p.span.line;
                if let Some(why) = reaches_into_profile(cfg, p, modifier) {
                    diags.push(warn(
                        g,
                        codes::W_SET_LINES_SKIPPED,
                        format!("`policy-path` line {line}: {why}; skipped"),
                    ));
                    continue;
                }
```

检查放在加前缀与修饰**之前**：看的是订阅作者写的原行（`p.params`），修饰设了同名参数（大小写不敏感）就交给修饰。`underlying-proxy` 只在它指向主配置的策略或组（`NameKind::Policy` / `NameKind::Group`）时才拦——指向另一条导入行时 `name_kind` 找不到它，指向 `DIRECT` 是内置名。

- [ ] **Step 3: 运行**

Run: `cargo test -p rurge-policy assemble` → 全部通过（新增 `a_subscription_line_may_not_reach_into_the_profile`）。

- [ ] **Step 4: 门禁与提交**

跑门禁（全工作区 39 个测试二进制，881 通过 / 1 忽略）。

```bash
git add crates/rurge-policy/src/assemble.rs
git commit -m "feat(policy): 订阅行自己写的 client-cert 与指向主配置的 underlying-proxy 不生效（W0023），只认 external-policy-modifier 设上的"
```


### Task 2: 空组当中继一律拒绝——`resolve_relay`（承接 C2）

M3-D3 让没有成员的组回退 DIRECT。可当这个组是别的策略的 `underlying-proxy`（或组级中继）时，回退 DIRECT 就等于让依赖它的策略直连自己的服务器、绕过使用者设的中继——而设中继往往正是为了不让流量直出（M3a P11 的理由）。项目所有者 2026-09-25 定为"中继时一律拒绝"（P2）：新增 `PolicyRegistry::resolve_relay(name)`，以 `EmptyGroup::Reject` 解析；`ChainConnector` 与引擎的 `socket_opener` 改用它。作为规则或全局策略直接使用的空组不变（仍按 M3-D3 与 `--empty-group-reject`）。

**Files:**
- Modify: `crates/rurge-policy/src/registry.rs`（`resolve_relay`；`named` / `empty` 多一个 `empty: EmptyGroup` 参数；用例）
- Modify: `crates/rurge-policy/src/cell.rs`（`ChainConnector` 用 `resolve_relay`；用例）
- Modify: `crates/rurge-engine/src/engine.rs`（`socket_opener` 用 `resolve_relay`）
- Test: `crates/rurge-engine/tests/subscriptions.rs`

**Interfaces:**
- Consumes: M3a 的 `PolicyRegistry::{resolve, named, empty}`、`EmptyGroup`、`Note::EmptyGroup { substituted }`、`ChainConnector::connect`（`TerminalKind::Reject` 带 `note` 时报 `via <name>: <note>`）。
- Produces: `PolicyRegistry::resolve_relay(&self, name: &str) -> Resolution`——`name` 作为另一个策略的 `underlying-proxy` 时的解析；没有成员的组在这里是 `REJECT` + `Note::EmptyGroup { substituted: false }`，不看注册表的 `EmptyGroup`。

- [ ] **Step 1: 用例先行**

注册表：同一个空组，直接解析时回退 DIRECT，当中继时拒绝；`ChainConnector`：中继是空组时报 `via Empty: policy group has no members`，一个字节也不往外发；引擎：经空组中继的策略拨号失败，它自己的服务器没有收到任何请求。

`crates/rurge-policy/src/registry.rs`——把

```rust
            (vec!["H", "A"], TerminalKind::Proxy, None)
        );
    }

    #[test]
    fn selections_api() {
```

换成

```rust
            (vec!["H", "A"], TerminalKind::Proxy, None)
        );
    }

    /// A relay is set so that traffic does not leave directly: an empty
    /// group used as one refuses, whatever `EmptyGroup` says — also when it
    /// is reached through a group that picks it. Dialled for itself, the
    /// group still stands in DIRECT.
    #[test]
    fn an_empty_group_as_a_relay_rejects() {
        let text = "[Proxy]\nA = http, a.example, 80\n\
[Proxy Group]\nE = select, policy-path=https://sub.example/e\nH = select, E, A\n[Rule]\nFINAL,H\n";
        let reg = generation(text, &FakeFactory::new(), None);
        let top = reg.resolve(&PolicyRef::parse("E"));
        assert_eq!(
            (top.terminal, top.note),
            (
                TerminalKind::Direct,
                Some(Note::EmptyGroup { substituted: true })
            )
        );
        for (relay, expected) in [("E", vec!["E", "REJECT"]), ("H", vec!["H", "E", "REJECT"])] {
            let r = reg.resolve_relay(relay);
            assert_eq!(
                (chain(&r), r.terminal, r.note.clone()),
                (
                    expected,
                    TerminalKind::Reject,
                    Some(Note::EmptyGroup { substituted: false })
                ),
                "{relay}"
            );
        }
        // anything else resolves as a policy would
        let a = reg.resolve_relay("A");
        assert_eq!((chain(&a), a.terminal), (vec!["A"], TerminalKind::Proxy));
    }

    #[test]
    fn selections_api() {
```

`crates/rurge-policy/src/cell.rs`——把

```rust
    const PROFILE: &str = "[General]\nloglevel = notify\n[Proxy]\nD = direct\nBlock = reject\n[Proxy Group]\nPick = select, D, DIRECT\n[Rule]\nFINAL,DIRECT\n";
```

换成

```rust
    const PROFILE: &str = "[General]\nloglevel = notify\n[Proxy]\nD = direct\nBlock = reject\n[Proxy Group]\nPick = select, D, DIRECT\nEmpty = select, policy-path=https://sub.example/e\n[Rule]\nFINAL,DIRECT\n";
```

`crates/rurge-policy/src/cell.rs`——把

```rust
        assert_eq!(e.to_string(), "via Block: rejected by REJECT");
    }

    #[test]
```

换成

```rust
        assert_eq!(e.to_string(), "via Block: rejected by REJECT");
    }

    /// A relay group without members stands for nothing: the connection is
    /// refused, never sent out directly behind the user's back.
    #[tokio::test]
    async fn an_empty_group_as_the_relay_refuses() {
        let connector = Arc::new(RecordingConnector::default());
        let cell = RegistryCell::new();
        cell.store(registry(connector.clone()));
        let e = ChainConnector::new(cell, "Empty")
            .connect(&server(), &ConnectOpts::default())
            .await
            .err()
            .expect("no member to relay through");
        assert_eq!(e.to_string(), "via Empty: policy group has no members");
        assert!(connector.seen().is_empty());
    }

    #[test]
```

`crates/rurge-engine/tests/subscriptions.rs`——把

```rust
        Ok(_) => panic!("expected a reject, got a stream"),
    }
}

/// The control plane sees what the registry holds: imported and derived
```

换成

```rust
        Ok(_) => panic!("expected a reject, got a stream"),
    }
}

/// A relay is set so that traffic does not leave directly: a policy relayed
/// through a group without members fails and says why, and nothing goes out
/// to its server — even though the same group, dialled for itself, would
/// stand in DIRECT.
#[tokio::test]
async fn a_relay_group_without_members_refuses() {
    let origin = TestServer::spawn().await;
    let upstream = FakeHttpProxy::spawn(HttpProxyScript::default()).await;
    let proxies = format!(
        "Hop = http, 127.0.0.1, {}, underlying-proxy=Pool",
        upstream.addr().port()
    );
    let h = harness(Profile {
        proxies: &proxies,
        groups: "Pool = select, policy-path=missing.txt",
        rules: "DOMAIN,relay.test,Hop",
        ..Profile::default()
    })
    .await;
    h.dns.set("relay.test", &["127.0.0.1"], &[], 60);
    let session = SessionInfo::tcp(
        rurge_config::HostName::parse("relay.test"),
        origin_addr(&origin).port(),
    );
    match h.engine.dial(session).await {
        Err(DialError::Failed { message, .. }) => {
            assert!(
                message.contains("via Pool: policy group has no members"),
                "{message}"
            );
        }
        Err(DialError::Reject { .. }) => panic!("expected a failure, got a reject"),
        Ok(_) => panic!("expected a failure, got a stream"),
    }
    assert!(upstream.heads().is_empty());
}

/// The control plane sees what the registry holds: imported and derived
```

Run: `cargo test -p rurge-policy`

Expected: 编译错误——`no method named `resolve_relay` found for struct `registry::PolicyRegistry``。

Run: `cargo test -p rurge-engine --test subscriptions relay`

Expected: FAIL——`a_relay_group_without_members_refuses` 的断言：拨号失败的原因是 `http proxy answered 502 Bad Gateway`，不是 `via Pool: policy group has no members`（空组回退了 DIRECT，`Hop` 直接连到了它自己的服务器）。

- [ ] **Step 2: 写实现**

`crates/rurge-policy/src/registry.rs`——把

```rust
            PolicyRef::Named(name) => self.named(name, &mut chain, 0),
        }
```

换成

```rust
            PolicyRef::Named(name) => self.named(name, &mut chain, 0, self.empty_group),
        }
    }

    /// `name` as the `underlying-proxy` of another policy. A relay is set so
    /// that traffic does not leave directly: a group without members refuses
    /// here, whatever `EmptyGroup` says for a group dialled for itself.
    pub fn resolve_relay(&self, name: &str) -> Resolution {
        self.named(name, &mut Vec::new(), 0, EmptyGroup::Reject)
```

`crates/rurge-policy/src/registry.rs`——把

```rust
    fn named(&self, name: &str, chain: &mut Vec<String>, depth: usize) -> Resolution {
```

换成

```rust
    fn named(
        &self,
        name: &str,
        chain: &mut Vec<String>,
        depth: usize,
        empty: EmptyGroup,
    ) -> Resolution {
```

`crates/rurge-policy/src/registry.rs`——把

```rust
                    PolicyRef::Named(n) => self.named(&n, chain, depth + 1),
                },
                None => self.empty(chain),
```

换成

```rust
                    PolicyRef::Named(n) => self.named(&n, chain, depth + 1, empty),
                },
                None => self.empty(chain, empty),
```

`crates/rurge-policy/src/registry.rs`——把

```rust
    fn empty(&self, chain: &mut Vec<String>) -> Resolution {
        match self.empty_group {
```

换成

```rust
    fn empty(&self, chain: &mut Vec<String>, empty: EmptyGroup) -> Resolution {
        match empty {
```

`crates/rurge-policy/src/cell.rs`——把

```rust
use crate::registry::PolicyRegistry;
use arc_swap::ArcSwapOption;
use rurge_config::rule::PolicyRef;
```

换成

```rust
use crate::registry::{PolicyRegistry, TerminalKind};
use arc_swap::ArcSwapOption;
```

`crates/rurge-policy/src/cell.rs`——把

```rust
            let resolution = registry.resolve(&PolicyRef::Named(self.name.clone()));
```

换成

```rust
            let resolution = registry.resolve_relay(&self.name);
            if let (TerminalKind::Reject, Some(note)) = (resolution.terminal, &resolution.note) {
                // why the relay refuses says more than "rejected": a group
                // without members, a group cycle, a protocol not implemented
                return Err(io::Error::other(format!("via {}: {note}", self.name)));
            }
```

`crates/rurge-engine/src/engine.rs`——把

```rust
        let below = registry.resolve(&PolicyRef::Named(under.to_string()));
```

换成

```rust
        // as the hop's `ChainConnector` will: a relay group without members
        // refuses there, so it is REJECT here too
        let below = registry.resolve_relay(under);
```

`named` 递归时把 `empty` 一路传下去：中继是一个选中了空组的 `select` 组时，同样拒绝。`cell.rs` 不再需要 `PolicyRef`，删掉这个 import。

- [ ] **Step 3: 运行**

Run: `cargo test -p rurge-policy` → 全部通过（新增 `an_empty_group_as_a_relay_rejects`、`an_empty_group_as_the_relay_refuses`）。

Run: `cargo test -p rurge-engine --test subscriptions` → 全部通过（新增 `a_relay_group_without_members_refuses`）。

- [ ] **Step 4: 门禁与提交**

跑门禁（全工作区 39 个测试二进制，884 通过 / 1 忽略）。

```bash
git add crates/rurge-policy/src/registry.rs crates/rurge-policy/src/cell.rs crates/rurge-engine/src/engine.rs crates/rurge-engine/tests/subscriptions.rs
git commit -m "feat(policy): 空组被当作中继时一律拒绝（resolve_relay），不回退 DIRECT"
```


### Task 3: `[General] test-timeout` 区分未设置；`test_target`；`test-url` / `test-timeout` 生效

设计 6.1：一次测试的 URL 取策略自己的 `test-url`，否则 `[General]` 的 `proxy-test-url`（直连类策略用 `internet-test-url`）；超时取策略自己的 `test-timeout`，否则 `[General]` 的 `test-timeout`，否则 5 秒（直连类 10 秒）。今天 `General.test_timeout` 直接存成 5 秒，分不出"没写"与"写了 5"——而直连类的 10 秒只在全局没写时适用（V1，P3）。本任务把它改成 `Option`，加上取值函数 `test_target`，并把 `test-url` / `test-timeout` 从 `W0029`（解析但未生效）名单里拿掉。

**Files:**
- Modify: `crates/rurge-config/src/general.rs`（`test_timeout: Option<Duration>`、`General::test_target`、用例）
- Modify: `crates/rurge-config/src/spec/common.rs`（`W0029` 名单去掉两项，用例随之）

**Interfaces:**
- Consumes: `CommonOpts.test_url: Option<String>`、`CommonOpts.test_timeout: Option<Duration>`（M1 已解析并校验）。
- Produces:
  - `General.test_timeout: Option<Duration>`（`None` = 配置没写）
  - `General::test_target<'a>(&'a self, own_url: Option<&'a str>, own_timeout: Option<Duration>, direct: bool) -> (&'a str, Duration)`——Task 6 的注册表为每个策略算测试 URL 与超时时调用

- [ ] **Step 1: 用例先行**

`crates/rurge-config/src/general.rs`——把

```rust
        assert_eq!(g.test_timeout, Duration::from_secs(8));
```

换成

```rust
        assert_eq!(g.test_timeout, Some(Duration::from_secs(8)));
```

`crates/rurge-config/src/general.rs`——把

```rust
        assert_eq!(g.test_timeout, Duration::from_secs(5));
```

换成

```rust
        assert_eq!(g.test_timeout, None);
```

`crates/rurge-config/src/general.rs`——把

```rust
        assert!(g.http_listen.is_empty());
    }
```

换成

```rust
        assert!(g.http_listen.is_empty());
    }

    /// A connectivity test's URL and timeout: the policy's own, else the
    /// profile's — `internet-test-url` for the direct kind, `proxy-test-url`
    /// for the rest — else 5 seconds, 10 for the direct kind (phase 2 M3
    /// design 6.1).
    #[test]
    fn a_test_goes_where_the_policy_or_the_profile_says() {
        let (g, _) = parse(
            "[General]
internet-test-url = http://i.test/
proxy-test-url = http://p.test/
",
        );
        assert_eq!(
            g.test_target(None, None, false),
            ("http://p.test/", Duration::from_secs(5))
        );
        assert_eq!(
            g.test_target(None, None, true),
            ("http://i.test/", Duration::from_secs(10))
        );
        assert_eq!(
            g.test_target(Some("http://own.test/"), Some(Duration::from_secs(2)), true),
            ("http://own.test/", Duration::from_secs(2))
        );
        let (g, _) = parse(
            "[General]
test-timeout = 3
",
        );
        assert_eq!(
            g.test_target(None, None, true),
            ("http://bing.com/", Duration::from_secs(3))
        );
    }
```

`crates/rurge-config/src/spec/common.rs`——把

```rust
                "tfo",
                "test-url",
                "test-timeout",
```

换成

```rust
                "tfo",
```

Run: `cargo test -p rurge-config --lib general`

Expected: 编译错误——`mismatched types`（`g.test_timeout` 还是 `Duration`）与 `no method named `test_target` found for struct `general::General``。

- [ ] **Step 2: 写实现**

`crates/rurge-config/src/general.rs`——把

```rust
    pub test_timeout: Duration,
```

换成

```rust
    /// `None` when the profile does not say: the default depends on the
    /// policy (`test_target`).
    pub test_timeout: Option<Duration>,
```

`crates/rurge-config/src/general.rs`——把

```rust
    pub unknown: Vec<UnknownKey>,
}

impl Default for General {
```

换成

```rust
    pub unknown: Vec<UnknownKey>,
}

impl General {
    /// Where a connectivity test of a policy goes and how long it may take
    /// (phase 2 M3 design 6.1): the policy's own `test-url` / `test-timeout`,
    /// else the profile's — `internet-test-url` for the direct kind (`DIRECT`
    /// and `direct` policies), `proxy-test-url` for the rest — else 5
    /// seconds, 10 for the direct kind.
    pub fn test_target<'a>(
        &'a self,
        own_url: Option<&'a str>,
        own_timeout: Option<Duration>,
        direct: bool,
    ) -> (&'a str, Duration) {
        let url = own_url.unwrap_or(if direct {
            &self.internet_test_url
        } else {
            &self.proxy_test_url
        });
        let fallback = Duration::from_secs(if direct { 10 } else { 5 });
        (url, own_timeout.or(self.test_timeout).unwrap_or(fallback))
    }
}

impl Default for General {
```

`crates/rurge-config/src/general.rs`——把

```rust
            test_timeout: Duration::from_secs(5),
```

换成

```rust
            test_timeout: None,
```

`crates/rurge-config/src/general.rs`——把

```rust
                Ok(s) => g.test_timeout = Duration::from_secs(s),
```

换成

```rust
                Ok(s) => g.test_timeout = Some(Duration::from_secs(s)),
```

`crates/rurge-config/src/spec/common.rs`——把

```rust
            ("tfo", tfo),
            ("test-url", test_url.is_some()),
            ("test-timeout", test_timeout.is_some()),
```

换成

```rust
            ("tfo", tfo),
```

写计划时核对过：`General.test_timeout` 没有别的调用方；语料库快照（`crates/rurge-config/tests/snapshots/`）不含这个字段，不受影响。`test-udp` 仍在 `W0029` 名单里（M5 生效）。

- [ ] **Step 3: 运行**

Run: `cargo test -p rurge-config` → 全部通过（新增 `a_test_goes_where_the_policy_or_the_profile_says`；`spec::common` 的 `W0029` 用例少了两项）。

- [ ] **Step 4: 门禁与提交**

跑门禁（全工作区 39 个测试二进制，885 通过 / 1 忽略）。

```bash
git add crates/rurge-config/src/general.rs crates/rurge-config/src/spec/common.rs
git commit -m "feat(config): [General] test-timeout 区分未设置，新增 test_target；策略的 test-url / test-timeout 不再报 W0029"
```


### Task 4: 探针——两次 `HEAD`、HTTPS、不保持连接时的退化

一次连通性测试（设计 6.1、M3-D8，P4）：经被测策略自己的出站 `connect_tcp(测试 URL 的主机, 端口)` 拿到流（主机名原样交给代理，远程解析）；HTTPS 时在流上套 TLS；然后用 `hyper` 的 HTTP/1 客户端连接层在**这一条**连接上发第一次 `HEAD`，连接可复用时再发第二次，以第二次从发出到收到响应头的耗时计分；不可复用时以第一次从开始拨号起的完整耗时计分。收到任何状态码的完整响应头都算通过；超时、拨号失败、连接中断算失败，失败原因只说是哪一步（`connect:` / `tls:` / `http:` / `timed out`），不引用 URL。整个测试受超时约束。不用 `rurge-net` 的池化 `HttpClient`——连接池会自己决定哪次请求开新连接。

**Files:**
- Create: `crates/rurge-policy/src/probe.rs`（`Probed`、`probe`，与回环用例）
- Modify: `crates/rurge-policy/src/lib.rs`（`pub mod probe;`）、`crates/rurge-policy/Cargo.toml`（依赖）、`Cargo.lock`（cargo 自动写入，只多 `rurge-policy` 依赖列表里的几行）
- Modify: `crates/rurge-net/src/testing.rs`（`TestServer::spawn_tls_with`）

**Interfaces:**
- Consumes: `rurge_proto::OutboundRef`（`connect_tcp(&Target, &ConnectOpts)`）、`rurge_net::connector::{Target, ConnectOpts, BoxedStream}`；测试用 `rurge_net::testing::TestServer`（`set` / `set_status` / `set_delay` / `set_header` / `requests`）与 `rurge_proto::testing::TlsFixture`（`acceptor(false)`、`roots()`）。
- Produces:
  - `pub enum Probed { Passed { score: Duration, reused: bool }, Failed(String) }`（`Clone + Debug + PartialEq + Eq`）
  - `pub async fn probe(outbound: &OutboundRef, url: &Url, timeout: Duration, roots: Arc<RootCertStore>) -> Probed`
  - `TestServer::spawn_tls_with(acceptor: tokio_rustls::TlsAcceptor) -> TestServer`

- [ ] **Step 1: 依赖与测试夹具**

`rurge-policy` 引用工作区已有的依赖（不下载任何东西；`cargo` 会把它们写进 `Cargo.lock` 里 `rurge-policy` 的依赖列表）；dev 依赖换成 `rurge-net` / `rurge-proto` 的 `testing` 特性（`tokio` 已经是正式依赖）：

`crates/rurge-policy/Cargo.toml`——把

```toml
tracing.workspace = true

[dev-dependencies]
tokio.workspace = true
```

换成

```toml
bytes.workspace = true
http.workspace = true
http-body-util.workspace = true
hyper.workspace = true
hyper-util.workspace = true
rustls.workspace = true
tokio.workspace = true
tokio-rustls.workspace = true
tracing.workspace = true
url.workspace = true

[dev-dependencies]
rurge-net = { workspace = true, features = ["testing"] }
rurge-proto = { workspace = true, features = ["testing"] }
```

`TestServer` 能用调用方给的证书起 HTTPS，探针的 HTTPS 用例才能把信任它的根证书交给探针：

`crates/rurge-net/src/testing.rs`——把

```rust
        Self::start(false).await
```

换成

```rust
        Self::start(None).await
```

`crates/rurge-net/src/testing.rs`——把

```rust
        Self::start(true).await
    }

    async fn start(tls: bool) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let acceptor = if tls { Some(tls_acceptor()) } else { None };
```

换成

```rust
        Self::start(Some(tls_acceptor())).await
    }

    /// HTTPS with a certificate of the caller's making, so a client can be
    /// given the roots that trust it.
    pub async fn spawn_tls_with(acceptor: TlsAcceptor) -> TestServer {
        Self::start(Some(acceptor)).await
    }

    async fn start(acceptor: Option<TlsAcceptor>) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let tls = acceptor.is_some();
```

- [ ] **Step 2: 新模块（实现与用例一起）**

新建 `crates/rurge-policy/src/probe.rs`：

```rust
//! One connectivity test of a policy (phase 2 M3 design 6.1, M3-D8): through
//! the policy's own outbound — never the pooled `HttpClient`, whose pool
//! would decide on its own which request opens a connection — two `HEAD`s on
//! one connection, the second one timed.

use bytes::Bytes;
use http::Request;
use http::header::{CONNECTION, HOST, USER_AGENT};
use http_body_util::Empty;
use hyper_util::rt::TokioIo;
use rurge_config::HostName;
use rurge_net::connector::{BoxedStream, ConnectOpts, Target};
use rurge_proto::OutboundRef;
use rustls::RootCertStore;
use rustls::pki_types::ServerName;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use tokio_rustls::TlsConnector;
use url::{Host, Url};

/// What one test found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probed {
    /// The response head came back. `score` is the second `HEAD`'s time on
    /// the kept-alive connection (`reused`), or, when the connection could
    /// not be kept, the whole first round trip from the start of the dial.
    Passed { score: Duration, reused: bool },
    /// Why not; nothing of the test URL in it (a subscription line may have
    /// set the URL).
    Failed(String),
}

/// Stops the connection task however the test ends.
struct Driver(JoinHandle<()>);

impl Drop for Driver {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Tests `outbound` against `url` (`http` or `https`), within `timeout`
/// overall. `roots` verify an `https` URL's certificate. Any status counts:
/// the response head coming back is the test.
pub async fn probe(
    outbound: &OutboundRef,
    url: &Url,
    timeout: Duration,
    roots: Arc<RootCertStore>,
) -> Probed {
    match tokio::time::timeout(timeout, run(outbound, url, timeout, roots)).await {
        Ok(Ok(passed)) => passed,
        Ok(Err(why)) => Probed::Failed(why),
        Err(_) => Probed::Failed("timed out".to_string()),
    }
}

async fn run(
    outbound: &OutboundRef,
    url: &Url,
    timeout: Duration,
    roots: Arc<RootCertStore>,
) -> Result<Probed, String> {
    let tls = match url.scheme() {
        "http" => false,
        "https" => true,
        _ => return Err("the test URL is neither http nor https".to_string()),
    };
    let (host, server_name) = match url.host() {
        Some(Host::Domain(d)) => (HostName::parse(d), d.to_string()),
        Some(Host::Ipv4(ip)) => (HostName::Ip(ip.into()), ip.to_string()),
        Some(Host::Ipv6(ip)) => (HostName::Ip(ip.into()), ip.to_string()),
        None => return Err("the test URL has no host".to_string()),
    };
    let port = url
        .port_or_known_default()
        .ok_or("the test URL has no port")?;
    let started = Instant::now();
    let stream = outbound
        .connect_tcp(&Target::new(host, port), &ConnectOpts { timeout })
        .await
        .map_err(|e| format!("connect: {e}"))?;
    let stream: BoxedStream = if tls {
        let name = ServerName::try_from(server_name)
            .map_err(|_| "the test URL's host is no TLS server name".to_string())?;
        let config = client_config(roots)?;
        Box::new(
            TlsConnector::from(config)
                .connect(name, stream)
                .await
                .map_err(|e| format!("tls: {e}"))?,
        )
    } else {
        stream
    };
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|e| format!("http: {e}"))?;
    let _driver = Driver(tokio::spawn(async move {
        let _ = connection.await;
    }));
    let authority = match url.port() {
        Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
        None => url.host_str().unwrap_or_default().to_string(),
    };
    let head = || {
        Request::head(url[url::Position::BeforePath..url::Position::AfterQuery].to_string())
            .header(HOST, &authority)
            .header(USER_AGENT, concat!("rurge/", env!("CARGO_PKG_VERSION")))
            .body(Empty::<Bytes>::new())
            .expect("a HEAD request")
    };
    let first = sender
        .send_request(head())
        .await
        .map_err(|e| format!("http: {e}"))?;
    let whole = started.elapsed();
    let closing = first.headers().get_all(CONNECTION).iter().any(|v| {
        v.to_str()
            .is_ok_and(|v| v.to_ascii_lowercase().contains("close"))
    });
    drop(first);
    if closing || sender.ready().await.is_err() {
        return Ok(Probed::Passed {
            score: whole,
            reused: false,
        });
    }
    let second = Instant::now();
    match sender.send_request(head()).await {
        Ok(_) => Ok(Probed::Passed {
            score: second.elapsed(),
            reused: true,
        }),
        // the server let the connection go after all
        Err(e) if e.is_closed() || e.is_incomplete_message() || e.is_canceled() => {
            Ok(Probed::Passed {
                score: whole,
                reused: false,
            })
        }
        Err(e) => Err(format!("http: {e}")),
    }
}

fn client_config(roots: Arc<RootCertStore>) -> Result<Arc<rustls::ClientConfig>, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("tls: {e}"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    // the connection is spoken over HTTP/1, with no room for h2
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use rurge_net::testing::TestServer;
    use rurge_proto::testing::TlsFixture;
    use rurge_proto::{Direct, Reject, RejectKind};

    fn direct() -> OutboundRef {
        Arc::new(Direct::new(Arc::new(DirectConnector::new(Arc::new(
            SystemResolve,
        )))))
    }

    fn heads(server: &TestServer, path: &str) -> usize {
        server
            .requests()
            .iter()
            .filter(|r| r.path == path && r.method == "HEAD")
            .count()
    }

    #[tokio::test]
    async fn the_second_head_on_a_kept_connection_is_timed() {
        let server = TestServer::spawn().await;
        server.set("/ok", "hi");
        let probed = probe(
            &direct(),
            &server.url("/ok"),
            Duration::from_secs(5),
            rurge_net::tls::root_store(),
        )
        .await;
        assert!(
            matches!(probed, Probed::Passed { reused: true, .. }),
            "{probed:?}"
        );
        assert_eq!(heads(&server, "/ok"), 2);
    }

    /// A server that closes after the first answer is measured by that
    /// answer, dial included.
    #[tokio::test]
    async fn a_connection_that_is_not_kept_gives_the_first_round_trip() {
        let server = TestServer::spawn().await;
        server.set("/once", "");
        server.set_header("/once", "connection", "close");
        let probed = probe(
            &direct(),
            &server.url("/once"),
            Duration::from_secs(5),
            rurge_net::tls::root_store(),
        )
        .await;
        assert!(
            matches!(probed, Probed::Passed { reused: false, .. }),
            "{probed:?}"
        );
        assert_eq!(heads(&server, "/once"), 1);
    }

    /// Any status is an answer: the test is about the way there.
    #[tokio::test]
    async fn any_status_passes() {
        let server = TestServer::spawn().await;
        server.set("/gone", "");
        server.set_status("/gone", 404);
        let probed = probe(
            &direct(),
            &server.url("/gone"),
            Duration::from_secs(5),
            rurge_net::tls::root_store(),
        )
        .await;
        assert!(matches!(probed, Probed::Passed { .. }), "{probed:?}");
    }

    /// An `https` URL is tested inside the TLS connection it opened, the
    /// second request on the same session.
    #[tokio::test]
    async fn https_is_tested_on_one_tls_connection() {
        let fixture = TlsFixture::new(&["127.0.0.1"]);
        let server = TestServer::spawn_tls_with(fixture.acceptor(false)).await;
        server.set("/ok", "");
        let probed = probe(
            &direct(),
            &server.url("/ok"),
            Duration::from_secs(5),
            fixture.roots(),
        )
        .await;
        assert!(
            matches!(probed, Probed::Passed { reused: true, .. }),
            "{probed:?}"
        );
        assert_eq!(heads(&server, "/ok"), 2);
        // a certificate nobody vouches for fails the test
        let untrusted = probe(
            &direct(),
            &server.url("/ok"),
            Duration::from_secs(5),
            Arc::new(RootCertStore::empty()),
        )
        .await;
        assert!(
            matches!(&untrusted, Probed::Failed(why) if why.starts_with("tls: ")),
            "{untrusted:?}"
        );
    }

    #[tokio::test]
    async fn a_failure_says_which_step_failed() {
        let server = TestServer::spawn().await;
        server.set("/slow", "");
        server.set_delay("/slow", Duration::from_secs(5));
        let slow = probe(
            &direct(),
            &server.url("/slow"),
            Duration::from_millis(200),
            rurge_net::tls::root_store(),
        )
        .await;
        assert_eq!(slow, Probed::Failed("timed out".to_string()));
        let reject: OutboundRef = Arc::new(Reject::new(RejectKind::Reject));
        let rejected = probe(
            &reject,
            &server.url("/slow"),
            Duration::from_secs(5),
            rurge_net::tls::root_store(),
        )
        .await;
        assert!(
            matches!(&rejected, Probed::Failed(why) if why.starts_with("connect: ")),
            "{rejected:?}"
        );
        let ftp = probe(
            &direct(),
            &Url::parse("ftp://127.0.0.1/").unwrap(),
            Duration::from_secs(5),
            rurge_net::tls::root_store(),
        )
        .await;
        assert_eq!(
            ftp,
            Probed::Failed("the test URL is neither http nor https".to_string())
        );
    }
}
```

`crates/rurge-policy/src/lib.rs`——把

```rust
pub mod factory;
```

换成

```rust
pub mod factory;
pub mod probe;
```

要点：
- 可复用的判断：第一次响应头里没有 `Connection: close`，且 `sender.ready()` 成功。第二次 `HEAD` 若仍因连接关闭 / 消息不完整 / 被取消而失败（服务端最终还是放掉了连接），退回第一次的完整耗时——这不是失败。
- 连接驱动任务由 `Driver` 持有，测试不管怎样结束都会 `abort` 它。
- TLS：ring 提供者 + 安全默认版本，ALPN 只报 `http/1.1`；SNI 是 URL 的主机（IP 字面量也行）。
- 用例全部在回环上：明文、`Connection: close`、404、HTTPS（`TlsFixture` 的根证书能过、空根证书以 `tls:` 失败）、超时 / REJECT / 非 http(s) 三种失败。

- [ ] **Step 3: 运行**

Run: `cargo test -p rurge-policy probe` → 5 passed（`the_second_head_on_a_kept_connection_is_timed`、`a_connection_that_is_not_kept_gives_the_first_round_trip`、`any_status_passes`、`https_is_tested_on_one_tls_connection`、`a_failure_says_which_step_failed`）。

- [ ] **Step 4: 门禁与提交**

跑门禁（全工作区 39 个测试二进制，890 通过 / 1 忽略）。

```bash
git add Cargo.lock crates/rurge-policy crates/rurge-net/src/testing.rs
git commit -m "feat(policy): 连通性测试探针——经策略自己的出站在一条连接上两次 HEAD，HTTPS 在同一 TLS 连接上测第二次"
```


### Task 5: `TestBook`

测试结果的簿子（设计 6.2，P6）：每个策略存最近一次结果与它对应的 `key`（定义行、测试 URL、超时的哈希——策略改了、测试 URL 或超时改了，旧结果作废）；跨配置代保留（`TestBook` 随引擎存续），不写 `state.json`。同一 `(策略, key)` 同时只有一个测试，后来的请求等它的结果；测试在自己的任务里跑，等的人走了测试照样结束、结果照样保存；一个没留下结果就结束的测试（任务 panic）不会让这个策略从此再也测不了。全进程最多 `MAX_CONCURRENT_TESTS = 8` 个测试同时进行。引擎经 `TestObserver` 把每次测试记成一条会话（Task 7）。

**Files:**
- Create: `crates/rurge-policy/src/testbook.rs`
- Modify: `crates/rurge-policy/src/lib.rs`（`pub mod testbook;`）

**Interfaces:**
- Consumes: Task 4 的 `probe(outbound, url, timeout, roots) -> Probed`。
- Produces:
  - `pub const MAX_CONCURRENT_TESTS: usize = 8`
  - `pub struct TestResult { pub outcome: Result<Duration, String>, pub at: Instant, pub when: SystemTime }`（`Clone + Debug + PartialEq + Eq`）
  - `pub struct TestCase { pub policy: String, pub outbound: OutboundRef, pub url: Url, pub timeout: Duration, pub key: u64, pub roots: Arc<RootCertStore> }`（`Clone`）
  - `pub trait TestObserver: Send + Sync { fn begin(&self, policy: &str, url: &Url) -> Box<dyn TestRecord>; }`
  - `pub trait TestRecord: Send { fn end(self: Box<Self>, outcome: &Result<Duration, String>); }`
  - `TestBook::new() -> TestBook`（及 `Default`）、`observe(&self, Arc<dyn TestObserver>)`（只认第一次）、`result(&self, policy: &str, key: u64) -> Option<TestResult>`、`invalidate_all(&self)`、`async fn test(self: &Arc<Self>, case: TestCase) -> TestResult`

- [ ] **Step 1: 新模块（实现与用例一起）**

新建 `crates/rurge-policy/src/testbook.rs`：

```rust
//! Connectivity test results, kept across config generations (phase 2 M3
//! design 6.2): at most one test of a policy at a time, at most
//! `MAX_CONCURRENT_TESTS` tests at once, every test a session in the
//! request log through `TestObserver`.

use crate::probe::{Probed, probe};
use rurge_proto::OutboundRef;
use rustls::RootCertStore;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::{Semaphore, watch};
use url::Url;

/// How many tests may run at the same time, whoever asked for them.
pub const MAX_CONCURRENT_TESTS: usize = 8;

/// One test's result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestResult {
    /// The score, or why the test failed.
    pub outcome: Result<Duration, String>,
    /// When the test ended, for telling how old the result is.
    pub at: Instant,
    /// The same moment on the wall clock, for the control plane.
    pub when: SystemTime,
}

/// What to test, as the registry in use has it.
#[derive(Clone)]
pub struct TestCase {
    pub policy: String,
    pub outbound: OutboundRef,
    pub url: Url,
    pub timeout: Duration,
    /// What a result is good for: a result of the policy under another
    /// definition, test URL or timeout is no result (M3 design 6.2).
    pub key: u64,
    /// What verifies an `https` test URL: the outbounds' own trust anchors
    /// (`OutboundFactory::roots`).
    pub roots: Arc<RootCertStore>,
}

/// Where a test shows up while it runs: the engine writes a session into
/// the request log (M3 design 6.2).
pub trait TestObserver: Send + Sync {
    fn begin(&self, policy: &str, url: &Url) -> Box<dyn TestRecord>;
}

/// The running test's record, told how the test ended.
pub trait TestRecord: Send {
    fn end(self: Box<Self>, outcome: &Result<Duration, String>);
}

type Running = (u64, watch::Receiver<Option<TestResult>>);

pub struct TestBook {
    results: RwLock<HashMap<String, (u64, TestResult)>>,
    running: Mutex<HashMap<String, Running>>,
    permits: Arc<Semaphore>,
    observer: OnceLock<Arc<dyn TestObserver>>,
    /// Test URLs already said to be imprecise (the server does not keep the
    /// connection): once each.
    warned: Mutex<HashSet<String>>,
}

impl Default for TestBook {
    fn default() -> TestBook {
        TestBook::new()
    }
}

impl TestBook {
    pub fn new() -> TestBook {
        TestBook {
            results: RwLock::default(),
            running: Mutex::default(),
            permits: Arc::new(Semaphore::new(MAX_CONCURRENT_TESTS)),
            observer: OnceLock::new(),
            warned: Mutex::default(),
        }
    }

    /// Where tests are reported from now on; set once, later calls are
    /// ignored.
    pub fn observe(&self, observer: Arc<dyn TestObserver>) {
        let _ = self.observer.set(observer);
    }

    /// The last result of `policy` for the definition `key` stands for.
    pub fn result(&self, policy: &str, key: u64) -> Option<TestResult> {
        self.results
            .read()
            .expect("test results")
            .get(policy)
            .filter(|(k, _)| *k == key)
            .map(|(_, r)| r.clone())
    }

    /// Forgets every result: the network is not the one they were made on
    /// (the "network changed" entry of phase 3).
    pub fn invalidate_all(&self) {
        self.results.write().expect("test results").clear();
    }

    /// Tests `case` now, or waits for the test of it already running. The
    /// test itself runs on its own task: whoever asked may stop waiting, the
    /// test still ends and its result is kept.
    pub async fn test(self: &Arc<Self>, case: TestCase) -> TestResult {
        let policy = case.policy.clone();
        let mut rx = {
            let mut running = self.running.lock().expect("running tests");
            match running.get(&case.policy) {
                // a closed channel: that test's task died without a result
                Some((key, rx)) if *key == case.key && rx.has_changed().is_ok() => rx.clone(),
                _ => {
                    let (tx, rx) = watch::channel(None);
                    running.insert(case.policy.clone(), (case.key, rx.clone()));
                    tokio::spawn(self.clone().run(case, tx));
                    rx
                }
            }
        };
        loop {
            if let Some(result) = rx.borrow_and_update().clone() {
                return result;
            }
            if rx.changed().await.is_err() {
                tracing::error!(policy = %policy, "a connectivity test ended without a result");
                return TestResult {
                    outcome: Err("the test did not finish".to_string()),
                    at: Instant::now(),
                    when: SystemTime::now(),
                };
            }
        }
    }

    async fn run(self: Arc<Self>, case: TestCase, tx: watch::Sender<Option<TestResult>>) {
        let outcome = {
            let _permit = self.permits.acquire().await.expect("never closed");
            let record = self
                .observer
                .get()
                .map(|observer| observer.begin(&case.policy, &case.url));
            let outcome = match probe(&case.outbound, &case.url, case.timeout, case.roots.clone())
                .await
            {
                Probed::Passed { score, reused } => {
                    if !reused
                        && self
                            .warned
                            .lock()
                            .expect("warned")
                            .insert(case.url.to_string())
                    {
                        // the URL stays out of the log: a subscription
                        // line may have set it (M3-D7)
                        tracing::warn!(
                            policy = %case.policy,
                            "the test server does not keep the connection: the score includes the dial"
                        );
                    }
                    Ok(score)
                }
                Probed::Failed(why) => Err(why),
            };
            if let Some(record) = record {
                record.end(&outcome);
            }
            outcome
        };
        let result = TestResult {
            outcome,
            at: Instant::now(),
            when: SystemTime::now(),
        };
        self.results
            .write()
            .expect("test results")
            .insert(case.policy.clone(), (case.key, result.clone()));
        {
            let mut running = self.running.lock().expect("running tests");
            if running
                .get(&case.policy)
                .is_some_and(|(key, _)| *key == case.key)
            {
                running.remove(&case.policy);
            }
        }
        let _ = tx.send(Some(result));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_net::BoxFuture;
    use rurge_net::connector::{BoxedStream, ConnectOpts, Target};
    use rurge_proto::{Outbound, OutboundError};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Holds every connection for `hold`, counting how many are open at
    /// once, then refuses it.
    #[derive(Default)]
    struct Gate {
        hold: Duration,
        open: AtomicUsize,
        most: AtomicUsize,
        dials: AtomicUsize,
    }

    impl Outbound for Gate {
        fn name(&self) -> &str {
            "Gate"
        }
        fn connect_tcp<'a>(
            &'a self,
            _target: &'a Target,
            _opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
            Box::pin(async move {
                self.dials.fetch_add(1, Ordering::SeqCst);
                let now = self.open.fetch_add(1, Ordering::SeqCst) + 1;
                self.most.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(self.hold).await;
                self.open.fetch_sub(1, Ordering::SeqCst);
                Err(OutboundError::Proxy("closed by the gate".to_string()))
            })
        }
    }

    fn book() -> Arc<TestBook> {
        Arc::new(TestBook::new())
    }

    fn case(policy: &str, outbound: OutboundRef, key: u64) -> TestCase {
        TestCase {
            policy: policy.to_string(),
            outbound,
            url: Url::parse("http://127.0.0.1:9/").unwrap(),
            timeout: Duration::from_secs(5),
            key,
            roots: Arc::new(RootCertStore::empty()),
        }
    }

    #[tokio::test]
    async fn a_policy_is_tested_once_at_a_time() {
        let gate = Arc::new(Gate {
            hold: Duration::from_millis(100),
            ..Gate::default()
        });
        let book = book();
        let (a, b) = tokio::join!(
            book.test(case("P", gate.clone(), 1)),
            book.test(case("P", gate.clone(), 1))
        );
        assert_eq!(gate.dials.load(Ordering::SeqCst), 1);
        assert_eq!(a, b);
        assert_eq!(a.outcome, Err("connect: closed by the gate".to_string()));
        assert_eq!(book.result("P", 1), Some(a));
    }

    #[tokio::test]
    async fn no_more_than_eight_tests_run_at_once() {
        let gate = Arc::new(Gate {
            hold: Duration::from_millis(100),
            ..Gate::default()
        });
        let book = book();
        let tests: Vec<_> = (0..12)
            .map(|i| {
                let (book, case) = (book.clone(), case(&format!("P{i}"), gate.clone(), 1));
                tokio::spawn(async move { book.test(case).await })
            })
            .collect();
        for test in tests {
            assert!(test.await.expect("the test task").outcome.is_err());
        }
        assert_eq!(gate.dials.load(Ordering::SeqCst), 12);
        assert_eq!(gate.most.load(Ordering::SeqCst), MAX_CONCURRENT_TESTS);
    }

    /// A result is kept for the definition, URL and timeout it was made
    /// with; `invalidate_all` forgets them all.
    #[tokio::test]
    async fn a_result_is_good_for_what_was_tested() {
        let gate = Arc::new(Gate::default());
        let book = book();
        book.test(case("P", gate.clone(), 1)).await;
        assert!(book.result("P", 1).is_some());
        assert_eq!(book.result("P", 2), None);
        assert_eq!(book.result("Q", 1), None);
        book.invalidate_all();
        assert_eq!(book.result("P", 1), None);
    }

    #[derive(Default)]
    struct Seen(Mutex<Vec<String>>);

    struct Record(Arc<Seen>, String);

    impl TestObserver for Arc<Seen> {
        fn begin(&self, policy: &str, url: &Url) -> Box<dyn TestRecord> {
            self.0.lock().unwrap().push(format!("begin {policy} {url}"));
            Box::new(Record(self.clone(), policy.to_string()))
        }
    }

    impl TestRecord for Record {
        fn end(self: Box<Self>, outcome: &Result<Duration, String>) {
            let how = match outcome {
                Ok(_) => "passed".to_string(),
                Err(why) => why.clone(),
            };
            self.0
                .0
                .lock()
                .unwrap()
                .push(format!("end {} {how}", self.1));
        }
    }

    #[tokio::test]
    async fn every_test_is_seen_from_begin_to_end() {
        let seen = Arc::new(Seen::default());
        let book = book();
        book.observe(Arc::new(seen.clone()));
        book.test(case("P", Arc::new(Gate::default()), 1)).await;
        assert_eq!(
            *seen.0.lock().unwrap(),
            [
                "begin P http://127.0.0.1:9/",
                "end P connect: closed by the gate"
            ]
        );
    }

    /// Panics on every dial.
    struct Broken;

    impl Outbound for Broken {
        fn name(&self) -> &str {
            "Broken"
        }
        fn connect_tcp<'a>(
            &'a self,
            _target: &'a Target,
            _opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
            panic!("the outbound is broken")
        }
    }

    /// A test whose task dies ends without a result, and does not stand in
    /// the way of the next test of the policy.
    #[tokio::test]
    async fn a_test_that_died_is_run_again() {
        let book = book();
        let died = book.test(case("P", Arc::new(Broken), 1)).await;
        assert_eq!(died.outcome, Err("the test did not finish".to_string()));
        assert_eq!(book.result("P", 1), None);
        let gate = Arc::new(Gate::default());
        book.test(case("P", gate.clone(), 1)).await;
        assert_eq!(gate.dials.load(Ordering::SeqCst), 1);
        assert!(book.result("P", 1).is_some());
    }

    /// A test outlives whoever asked for it: its result is kept.
    #[tokio::test]
    async fn a_test_ends_even_when_nobody_waits_any_more() {
        let gate = Arc::new(Gate {
            hold: Duration::from_millis(100),
            ..Gate::default()
        });
        let book = book();
        let waiting = tokio::time::timeout(
            Duration::from_millis(10),
            book.test(case("P", gate.clone(), 1)),
        )
        .await;
        assert!(waiting.is_err(), "gave up waiting");
        let deadline = Instant::now() + Duration::from_secs(5);
        while book.result("P", 1).is_none() {
            assert!(Instant::now() < deadline, "the test never ended");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(gate.dials.load(Ordering::SeqCst), 1);
    }
}
```

`crates/rurge-policy/src/lib.rs`——把

```rust
pub mod subscription;
```

换成

```rust
pub mod subscription;
pub mod testbook;
```

要点：
- `running` 表按策略名存 `(key, watch::Receiver)`；取到的通道若已关闭（`has_changed()` 出错：跑它的任务没留下结果就结束了），当作没有在跑，换一个新测试。等待者从关闭的通道得到 `the test did not finish`，并记一条 ERROR。
- `run` 先存结果、再从 `running` 表里摘掉自己（只摘 `key` 相同的那一项）、最后发给等待者。
- 用例里的假出站 `Gate` 把每次拨号握一段时间再拒绝，数着同时打开的连接；`Broken` 一拨号就 panic。
- "服务端不保持连接"的告警对每个测试 URL 只打一次，日志里只有策略名（M3-D7：URL 可能来自订阅行）。

- [ ] **Step 2: 运行**

Run: `cargo test -p rurge-policy testbook` → 6 passed（`a_policy_is_tested_once_at_a_time`、`no_more_than_eight_tests_run_at_once`、`a_result_is_good_for_what_was_tested`、`every_test_is_seen_from_begin_to_end`、`a_test_that_died_is_run_again`、`a_test_ends_even_when_nobody_waits_any_more`）。

- [ ] **Step 3: 门禁与提交**

跑门禁（全工作区 39 个测试二进制，896 通过 / 1 忽略）。

```bash
git add crates/rurge-policy/src/testbook.rs crates/rurge-policy/src/lib.rs
git commit -m "feat(policy): TestBook——按策略与定义保存测试结果，同一策略同时只测一次，最多 8 个并发，测试经 TestObserver 可见"
```


### Task 6: 三种选法与 `AutoGroups`；注册表按测试结果选成员

设计 6.3 ～ 6.5。新模块 `rurge_policy::auto` 放三种选法的纯函数（`url_test` 带迟滞、`fallback`、`load_balance` 带 `persistent`）与 `AutoGroups`——随引擎存续的自动组状态：临时覆盖、`url-test` 保持的成员、每组上一轮测试结束的时间、已请求而还没跑的测试轮次（请求经一个通道投给引擎的调度任务，Task 7）。注册表改为按 `TestBook` 里的结果为自动组选成员：

- 只有**拨号**（`live`）会请求一轮测试（组上一轮比 `interval` 旧或从没测过时）、会让 `url-test` 换成员；控制面的读取（`current_member`）只看不动（P7）。
- 覆盖存在时直接用它，不请求测试。
- `evaluate-before-use` 的组还没测过时，`Resolution.pending` 带出它（最外层的一个）；Task 7 的引擎据此等待（P9）。
- 嵌套组作为成员时的分数：`select` 按选择、`url-test` / `fallback` 按当前选中的成员、`load-balance` 取通过者的均值（都没测过算未知）、成环的组算失败（P8）。
- 每个策略的测试方式（URL、超时、`key`）在构建时按 `General::test_target` 算好；`REJECT` 族、未实现的协议、URL 不能解析的策略没有测试方式，永远不算通过。根证书从出站工厂取（`OutboundFactory::roots`，P5）。
- `test_group(group)` 测一轮：该组与嵌套组的全部策略成员，每个测试在自己的任务里（`TestBook::test`），结束后给覆盖到的每个组记一轮；组已不存在时也要结束这次请求。

`PolicyRegistry::build` 因此多一个参数 `auto: &Arc<AutoGroups>`；引擎的 `EngineShared` 持有这份状态，三处构建都传它。

**Files:**
- Create: `crates/rurge-policy/src/auto.rs`
- Modify: `crates/rurge-policy/src/registry.rs`（实现与用例）、`crates/rurge-policy/src/factory.rs`（`roots`）、`crates/rurge-policy/src/testbook.rs`（测试用的 `record`）、`crates/rurge-policy/src/testing.rs`（夹具）、`crates/rurge-policy/src/cell.rs`（用例里的 `build` 调用）、`crates/rurge-policy/src/lib.rs`
- Modify: `crates/rurge-engine/src/{shared.rs, runtime.rs, subscriptions.rs, outbounds.rs, views.rs}`（`EngineShared.auto`、`build` 的新参数、`EngineFactory::roots`、用例里的 `build` 调用）

**Interfaces:**
- Consumes: Task 3 的 `General::test_target`；Task 5 的 `TestBook`（`result`、`test`）、`TestCase`、`TestResult`；M3a 的 `GroupSpec`（`kind`、`test: TestOpts { interval, tolerance, timeout, evaluate_before_use, persistent }`、`span`）、`Assembly`、`SelectionTable`、`EmptyGroup`。
- Produces:
  - `rurge_policy::auto::SelectCtx { pub host: Option<String> }`（`Clone + Debug + Default`）
  - `rurge_policy::auto::Standing { Passed(Duration), Failed, Unknown }`，`Standing::passes(self, &TestOpts) -> Option<Duration>`
  - `rurge_policy::auto::{url_test(members: &[(String, Standing)], current: Option<&str>, opts: &TestOpts) -> Option<String>, fallback(members, opts) -> Option<String>, load_balance(members, opts, ctx: &SelectCtx) -> Option<String>}`
  - `rurge_policy::auto::AutoGroups`：`pub tests: Arc<TestBook>`；`new(Arc<TestBook>)`、`connect(&self) -> mpsc::UnboundedReceiver<String>`、`requested(&self) -> Vec<String>`、`last_round(&self, group) -> Option<Instant>`、`rounds(&self) -> watch::Receiver<u64>`、`set_override(&self, spec: &GroupSpec, member: &str)`、`clear_override(&self, group) -> bool`、`retain(&self, groups: &[GroupSpec])`
  - `Resolution.pending: Option<String>`
  - `PolicyRegistry::build(cfg, assembly, factory, cell, selections, previous, empty_group, auto: &Arc<AutoGroups>)`（`#[allow(clippy::too_many_arguments)]`）
  - `PolicyRegistry::{group_spec(&self, name) -> Option<&GroupSpec>, auto(&self) -> &Arc<AutoGroups>, test_case(&self, name) -> Option<TestCase>, test_result(&self, name) -> Option<TestResult>, available(&self, group) -> Vec<String>, async test_group(&self, group) -> Vec<String>, resolve_with(&self, policy: &PolicyRef, ctx: &SelectCtx) -> Resolution}`；`resolve(p)` 等于 `resolve_with(p, &SelectCtx::default())`
  - `OutboundFactory::roots(&self) -> Arc<RootCertStore>`
  - `rurge_engine::EngineShared.auto: Arc<AutoGroups>`（`EngineShared::new` 里创建）

- [ ] **Step 1: 三种选法与 `AutoGroups`（新模块，实现与用例一起）**

新建 `crates/rurge-policy/src/auto.rs`：

```rust
//! The automatic groups — `url-test`, `fallback`, `load-balance` (phase 2
//! M3 design 6.3 – 6.5): how each one picks from its members' test results,
//! and what they keep across config generations — the temporary overrides,
//! the member `url-test` holds on to, when each group was last tested — plus
//! the way the registry asks the engine for a new round of tests.

use crate::testbook::TestBook;
use rurge_config::Span;
use rurge_config::spec::{GroupSpec, TestOpts};
use std::collections::hash_map::RandomState;
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};

/// What the dial knows about a session that a group may pick by.
#[derive(Clone, Debug, Default)]
pub struct SelectCtx {
    /// The target's host name: `load-balance` with `persistent=true` sends
    /// one host to one member.
    pub host: Option<String>,
}

/// A member's standing in the tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    /// Its last test passed with this score.
    Passed(Duration),
    /// Its last test failed, or it can never pass (a REJECT).
    Failed,
    /// Not tested yet.
    Unknown,
}

impl Standing {
    /// Passed, and below the group's `timeout` when it has one (M3 design
    /// 6.4).
    pub fn passes(self, opts: &TestOpts) -> Option<Duration> {
        match self {
            Standing::Passed(score) if opts.timeout.is_none_or(|t| score < t) => Some(score),
            _ => None,
        }
    }
}

/// `url-test`: the fastest member that passes — but the member it holds
/// (`current`) stays while it passes and the fastest is not quicker than
/// it by more than `tolerance`. The first member when none passes.
pub fn url_test(
    members: &[(String, Standing)],
    current: Option<&str>,
    opts: &TestOpts,
) -> Option<String> {
    let mut best: Option<(&str, Duration)> = None;
    for (name, standing) in members {
        if let Some(score) = standing.passes(opts)
            && best.is_none_or(|(_, b)| score < b)
        {
            best = Some((name, score));
        }
    }
    let Some((best, best_score)) = best else {
        return members.first().map(|(name, _)| name.clone());
    };
    let held = current.and_then(|current| {
        members
            .iter()
            .find(|(name, _)| name == current)
            .and_then(|(name, standing)| standing.passes(opts).map(|score| (name, score)))
    });
    match held {
        Some((name, score)) if score.saturating_sub(best_score) <= opts.tolerance => {
            Some(name.clone())
        }
        _ => Some(best.to_string()),
    }
}

/// `fallback`: the first member, in order, that passes; the first member
/// when none does.
pub fn fallback(members: &[(String, Standing)], opts: &TestOpts) -> Option<String> {
    members
        .iter()
        .find(|(_, standing)| standing.passes(opts).is_some())
        .or(members.first())
        .map(|(name, _)| name.clone())
}

/// `load-balance`: any member that passes — every member when none does —
/// at random, or, with `persistent`, the one the target host hashes to.
pub fn load_balance(
    members: &[(String, Standing)],
    opts: &TestOpts,
    ctx: &SelectCtx,
) -> Option<String> {
    let passing: Vec<&str> = members
        .iter()
        .filter(|(_, standing)| standing.passes(opts).is_some())
        .map(|(name, _)| name.as_str())
        .collect();
    let candidates: Vec<&str> = if passing.is_empty() {
        members.iter().map(|(name, _)| name.as_str()).collect()
    } else {
        passing
    };
    if candidates.is_empty() {
        return None;
    }
    let index = match (&ctx.host, opts.persistent) {
        (Some(host), true) => {
            // a fixed hasher: the same host goes to the same member for as
            // long as the candidates stay the same
            let mut h = DefaultHasher::new();
            host.hash(&mut h);
            h.finish()
        }
        _ => RandomState::new().hash_one(Instant::now()),
    } as usize
        % candidates.len();
    Some(candidates[index].to_string())
}

struct Override {
    member: String,
    /// The group as it was when the override was set, span left out: a
    /// reload that changes the group drops the override (M3 design 6.5).
    spec: GroupSpec,
    warned: bool,
}

#[derive(Default)]
struct State {
    overrides: HashMap<String, Override>,
    /// The member each `url-test` group holds on to.
    picks: HashMap<String, String>,
    /// When each group's last round of tests ended.
    rounds: HashMap<String, Instant>,
    /// Groups a round was asked for that has not run yet.
    requested: HashSet<String>,
}

/// The automatic groups' state, one per engine: it outlives the config
/// generations, as the test results do.
pub struct AutoGroups {
    pub tests: Arc<TestBook>,
    state: Mutex<State>,
    wake: Mutex<Option<mpsc::UnboundedSender<String>>>,
    rounds: watch::Sender<u64>,
}

fn without_span(spec: &GroupSpec) -> GroupSpec {
    GroupSpec {
        span: Span::new(Arc::from(Path::new("")), 0),
        ..spec.clone()
    }
}

impl AutoGroups {
    pub fn new(tests: Arc<TestBook>) -> AutoGroups {
        AutoGroups {
            tests,
            state: Mutex::default(),
            wake: Mutex::default(),
            rounds: watch::Sender::new(0),
        }
    }

    /// Where requests for a round of tests go from now on: the engine's
    /// scheduler, which runs `PolicyRegistry::test_group` for each name it
    /// receives. Until then requests only pile up in `requested`.
    pub fn connect(&self) -> mpsc::UnboundedReceiver<String> {
        let (tx, rx) = mpsc::unbounded_channel();
        let requested: Vec<String> = self
            .state
            .lock()
            .expect("auto groups")
            .requested
            .iter()
            .cloned()
            .collect();
        for group in requested {
            let _ = tx.send(group);
        }
        *self.wake.lock().expect("wake") = Some(tx);
        rx
    }

    /// Asks for a round of tests of `group`, unless one is asked for
    /// already.
    pub(crate) fn wake(&self, group: &str) {
        if !self
            .state
            .lock()
            .expect("auto groups")
            .requested
            .insert(group.to_string())
        {
            return;
        }
        if let Some(tx) = self.wake.lock().expect("wake").as_ref() {
            let _ = tx.send(group.to_string());
        }
    }

    /// The groups a round was asked for that has not run yet.
    pub fn requested(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .state
            .lock()
            .expect("auto groups")
            .requested
            .iter()
            .cloned()
            .collect();
        out.sort();
        out
    }

    /// A round of tests of `groups` has ended.
    pub(crate) fn round_done(&self, groups: &[String]) {
        {
            let mut state = self.state.lock().expect("auto groups");
            let now = Instant::now();
            for group in groups {
                state.rounds.insert(group.clone(), now);
                state.requested.remove(group);
            }
        }
        self.rounds.send_modify(|n| *n += 1);
    }

    /// When the last round of tests of `group` ended.
    pub fn last_round(&self, group: &str) -> Option<Instant> {
        self.state
            .lock()
            .expect("auto groups")
            .rounds
            .get(group)
            .copied()
    }

    /// Changes whenever a round ends: what `evaluate-before-use` waits on.
    pub fn rounds(&self) -> watch::Receiver<u64> {
        self.rounds.subscribe()
    }

    /// Makes `member` the choice of the automatic group `spec` until it is
    /// cleared, the group changes in a reload, or the process ends (M3
    /// design 6.5). The caller has checked that it is a member.
    pub fn set_override(&self, spec: &GroupSpec, member: &str) {
        self.state.lock().expect("auto groups").overrides.insert(
            spec.name.clone(),
            Override {
                member: member.to_string(),
                spec: without_span(spec),
                warned: false,
            },
        );
    }

    /// Whether there was an override to clear.
    pub fn clear_override(&self, group: &str) -> bool {
        self.state
            .lock()
            .expect("auto groups")
            .overrides
            .remove(group)
            .is_some()
    }

    /// The override of `group`, while it names one of `members`; one that
    /// no longer does (a subscription update took the member away) is said
    /// once and ignored.
    pub(crate) fn override_of(&self, group: &str, members: &[String]) -> Option<String> {
        let mut state = self.state.lock().expect("auto groups");
        let o = state.overrides.get_mut(group)?;
        if members.contains(&o.member) {
            return Some(o.member.clone());
        }
        if !o.warned {
            o.warned = true;
            tracing::warn!(group, member = %o.member, "the overriding member is gone from the group; the override has no effect");
        }
        None
    }

    pub(crate) fn pick(&self, group: &str) -> Option<String> {
        self.state
            .lock()
            .expect("auto groups")
            .picks
            .get(group)
            .cloned()
    }

    pub(crate) fn set_pick(&self, group: &str, member: &str) {
        let mut state = self.state.lock().expect("auto groups");
        if state.picks.get(group).is_none_or(|m| m != member) {
            state.picks.insert(group.to_string(), member.to_string());
        }
    }

    /// A new generation of the profile: overrides of groups that are gone
    /// or defined differently now go, and so does what is kept for groups
    /// that are gone (M3 design 6.5).
    pub fn retain(&self, groups: &[GroupSpec]) {
        let by_name: HashMap<&str, GroupSpec> = groups
            .iter()
            .map(|g| (g.name.as_str(), without_span(g)))
            .collect();
        let mut state = self.state.lock().expect("auto groups");
        state
            .overrides
            .retain(|group, o| by_name.get(group.as_str()) == Some(&o.spec));
        state
            .picks
            .retain(|group, _| by_name.contains_key(group.as_str()));
        state
            .rounds
            .retain(|group, _| by_name.contains_key(group.as_str()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::config::{LoadOptions, from_text};

    fn ms(n: u64) -> Standing {
        Standing::Passed(Duration::from_millis(n))
    }

    fn members(list: &[(&str, Standing)]) -> Vec<(String, Standing)> {
        list.iter().map(|(n, s)| (n.to_string(), *s)).collect()
    }

    fn opts(tolerance: u64, timeout: Option<u64>) -> TestOpts {
        TestOpts {
            tolerance: Duration::from_millis(tolerance),
            timeout: timeout.map(Duration::from_millis),
            ..TestOpts::default()
        }
    }

    #[test]
    fn url_test_holds_its_member_within_the_tolerance() {
        let m = members(&[("A", ms(120)), ("B", ms(50)), ("C", Standing::Failed)]);
        // no member held yet: the fastest
        assert_eq!(url_test(&m, None, &opts(100, None)).as_deref(), Some("B"));
        // A is 70 ms slower than B: within 100 ms, A stays
        assert_eq!(
            url_test(&m, Some("A"), &opts(100, None)).as_deref(),
            Some("A")
        );
        // with tolerance 0 every change goes to the fastest
        assert_eq!(
            url_test(&m, Some("A"), &opts(0, None)).as_deref(),
            Some("B")
        );
        // a held member that fails is dropped
        assert_eq!(
            url_test(&m, Some("C"), &opts(100, None)).as_deref(),
            Some("B")
        );
        // `timeout` takes B out of the running
        assert_eq!(
            url_test(&m, None, &opts(100, Some(100))).as_deref(),
            Some("B")
        );
        assert_eq!(
            url_test(
                &members(&[("A", ms(120)), ("B", ms(150))]),
                None,
                &opts(100, Some(100))
            )
            .as_deref(),
            Some("A"),
            "none passes: the first member"
        );
        assert_eq!(url_test(&[], None, &opts(100, None)), None);
    }

    #[test]
    fn fallback_takes_the_first_that_passes() {
        let m = members(&[
            ("A", Standing::Failed),
            ("B", Standing::Unknown),
            ("C", ms(300)),
            ("D", ms(10)),
        ]);
        assert_eq!(fallback(&m, &opts(0, None)).as_deref(), Some("C"));
        assert_eq!(
            fallback(&m, &opts(0, Some(200))).as_deref(),
            Some("D"),
            "C is not under the timeout"
        );
        assert_eq!(
            fallback(
                &members(&[("A", Standing::Failed), ("B", Standing::Failed)]),
                &opts(0, None)
            )
            .as_deref(),
            Some("A")
        );
    }

    #[test]
    fn load_balance_spreads_over_those_that_pass() {
        let m = members(&[("A", ms(10)), ("B", Standing::Failed), ("C", ms(20))]);
        let plain = TestOpts::default();
        let any = SelectCtx::default();
        for _ in 0..50 {
            let pick = load_balance(&m, &plain, &any).unwrap();
            assert!(pick == "A" || pick == "C", "{pick}");
        }
        // none passes: every member is a candidate
        let failed = members(&[("A", Standing::Failed), ("B", Standing::Unknown)]);
        let seen: HashSet<String> = (0..200)
            .map(|_| load_balance(&failed, &plain, &any).unwrap())
            .collect();
        assert_eq!(seen.len(), 2);
        // persistent: one host, one member
        let sticky = TestOpts {
            persistent: true,
            ..TestOpts::default()
        };
        let ctx = SelectCtx {
            host: Some("example.com".into()),
        };
        let first = load_balance(&m, &sticky, &ctx).unwrap();
        for _ in 0..20 {
            assert_eq!(load_balance(&m, &sticky, &ctx).unwrap(), first);
        }
        assert_eq!(load_balance(&[], &plain, &any), None);
    }

    fn spec_of(text: &str, group: &str) -> GroupSpec {
        let loaded = from_text(
            text,
            std::path::Path::new("t.conf"),
            &LoadOptions::for_tests(),
        );
        assert!(!loaded.diagnostics.has_errors());
        loaded
            .config
            .group_specs
            .iter()
            .find(|g| g.name == group)
            .cloned()
            .unwrap()
    }

    fn auto() -> AutoGroups {
        AutoGroups::new(Arc::new(TestBook::new()))
    }

    /// An override lasts while the group is defined the same way: a reload
    /// that moves the line (another span) keeps it, one that changes the
    /// group or removes it drops it.
    #[test]
    fn an_override_lasts_while_the_group_stays_the_same() {
        let a =
            "[Proxy]\nA = direct\nB = direct\n[Proxy Group]\nU = url-test, A, B\n[Rule]\nFINAL,U\n";
        let moved = "[Proxy]\nA = direct\nB = direct\n\n\n[Proxy Group]\nU = url-test, A, B\n[Rule]\nFINAL,U\n";
        let changed = "[Proxy]\nA = direct\nB = direct\n[Proxy Group]\nU = url-test, A, B, interval=60\n[Rule]\nFINAL,U\n";
        let auto = auto();
        let members = ["A".to_string(), "B".to_string()];
        auto.set_override(&spec_of(a, "U"), "B");
        auto.retain(&[spec_of(moved, "U")]);
        assert_eq!(auto.override_of("U", &members).as_deref(), Some("B"));
        auto.retain(&[spec_of(changed, "U")]);
        assert_eq!(auto.override_of("U", &members), None);
        auto.set_override(&spec_of(a, "U"), "B");
        auto.retain(&[]);
        assert_eq!(auto.override_of("U", &members), None);
        // an override whose member is gone does nothing
        auto.set_override(&spec_of(a, "U"), "B");
        assert_eq!(auto.override_of("U", &["A".to_string()]), None);
        assert!(auto.clear_override("U"));
        assert!(!auto.clear_override("U"));
    }

    /// A round is asked for once until it has run; the requests wait for
    /// the scheduler to connect.
    #[tokio::test]
    async fn a_round_is_asked_for_once_until_it_runs() {
        let auto = auto();
        auto.wake("U");
        auto.wake("U");
        assert_eq!(auto.requested(), ["U"]);
        let mut rx = auto.connect();
        assert_eq!(rx.recv().await.as_deref(), Some("U"));
        auto.wake("U");
        assert!(rx.try_recv().is_err(), "still asked for");
        let mut rounds = auto.rounds();
        auto.round_done(&["U".to_string()]);
        assert!(auto.requested().is_empty());
        assert!(auto.last_round("U").is_some());
        rounds.changed().await.unwrap();
        auto.wake("U");
        assert_eq!(rx.recv().await.as_deref(), Some("U"));
    }
}
```

`crates/rurge-policy/src/lib.rs`——把

```rust
pub mod assemble;
```

换成

```rust
pub mod assemble;
pub mod auto;
```

Run: `cargo test -p rurge-policy auto::` → 5 passed（`url_test_holds_its_member_within_the_tolerance`、`fallback_takes_the_first_that_passes`、`load_balance_spreads_over_those_that_pass`、`an_override_lasts_while_the_group_stays_the_same`、`a_round_is_asked_for_once_until_it_runs`）。

- [ ] **Step 2: 注册表的用例先行**

测试夹具：一份没有任何结果的 `AutoGroups`；假工厂的 `roots`（空的，单元测试只测 `http` URL）；原有用例的 `build` 调用多传 `&crate::testing::auto_groups()`。新用例用 `AUTO` 配置（`U` / `F` / `L` / `N` / `Outer` / `Avg` / `E` / `S` 八个组）与 `seed`（直接往 `TestBook` 里写结果）驱动：

`crates/rurge-policy/src/testing.rs`——把

```rust
use crate::factory::{BuildError, OutboundFactory};
```

换成

```rust
use crate::auto::AutoGroups;
use crate::factory::{BuildError, OutboundFactory};
use crate::testbook::TestBook;
```

`crates/rurge-policy/src/testing.rs`——把

```rust
use std::io;
use std::sync::{Arc, Mutex};
```

换成

```rust
use rustls::RootCertStore;
use std::io;
use std::sync::{Arc, Mutex};

/// Automatic groups with no test results.
pub(crate) fn auto_groups() -> Arc<AutoGroups> {
    Arc::new(AutoGroups::new(Arc::new(TestBook::new())))
}
```

`crates/rurge-policy/src/testing.rs`——把

```rust
        self.environment.to_string()
    }

    fn build(
```

换成

```rust
        self.environment.to_string()
    }

    /// Trusts nobody: the unit tests test `http` URLs only.
    fn roots(&self) -> Arc<RootCertStore> {
        Arc::new(RootCertStore::empty())
    }

    fn build(
```

`crates/rurge-policy/src/cell.rs`——把

```rust
                crate::EmptyGroup::Direct,
```

换成

```rust
                crate::EmptyGroup::Direct,
                &crate::testing::auto_groups(),
```

`crates/rurge-policy/src/registry.rs`——把

```rust
                None,
                EmptyGroup::Direct,
            )
            .expect("builds"),
```

换成

```rust
                None,
                EmptyGroup::Direct,
                &crate::testing::auto_groups(),
            )
            .expect("builds"),
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            None,
            EmptyGroup::Direct,
        )
        .err()
```

换成

```rust
            None,
            EmptyGroup::Direct,
            &crate::testing::auto_groups(),
        )
        .err()
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            Arc::new(SelectionTable::new(GroupSelections::new())),
            previous,
            EmptyGroup::Direct,
        )
        .expect("builds")
```

换成

```rust
            Arc::new(SelectionTable::new(GroupSelections::new())),
            previous,
            EmptyGroup::Direct,
            &crate::testing::auto_groups(),
        )
        .expect("builds")
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            Arc::new(SelectionTable::default()),
            previous,
            EmptyGroup::Direct,
        )
        .expect("builds")
```

换成

```rust
            Arc::new(SelectionTable::default()),
            previous,
            EmptyGroup::Direct,
            &crate::testing::auto_groups(),
        )
        .expect("builds")
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            None,
            EmptyGroup::Direct,
        )
        .expect("builds");
```

换成

```rust
            None,
            EmptyGroup::Direct,
            &crate::testing::auto_groups(),
        )
        .expect("builds");
```

`crates/rurge-policy/src/registry.rs`——把

```rust
                empty_group,
```

换成

```rust
                empty_group,
                &crate::testing::auto_groups(),
```

`crates/rurge-policy/src/registry.rs`——把

```rust
                Arc::new(SelectionTable::new(selections)),
                None,
                EmptyGroup::Direct,
            )
            .expect("builds")
```

换成

```rust
                Arc::new(SelectionTable::new(selections)),
                None,
                EmptyGroup::Direct,
                &crate::testing::auto_groups(),
            )
            .expect("builds")
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        assert_eq!((chain(&a), a.terminal), (vec!["A"], TerminalKind::Proxy));
    }

    #[test]
    fn selections_api() {
```

换成

```rust
        assert_eq!((chain(&a), a.terminal), (vec!["A"], TerminalKind::Proxy));
    }

    const AUTO: &str = "[Proxy]\nA = http, a.example, 80\nB = http, b.example, 80\nC = http, c.example, 80\n\
[Proxy Group]\nU = url-test, A, B, C\nF = fallback, A, B, C\nL = load-balance, A, B, C, persistent=true\n\
N = fallback, A, B\nOuter = url-test, N, C\nAvg = load-balance, B, C\n\
E = url-test, A, B, evaluate-before-use=true\nS = select, E\n[Rule]\nFINAL,U\n";

    /// As if `name`'s last test had passed in `ms`, or failed.
    fn seed(reg: &PolicyRegistry, name: &str, ms: Option<u64>) {
        let case = reg.test_case(name).expect("a policy that is tested");
        let outcome = ms
            .map(Duration::from_millis)
            .ok_or_else(|| "refused".to_string());
        reg.auto().tests.record(&case.policy, case.key, outcome);
    }

    fn picked(reg: &PolicyRegistry, group: &str) -> String {
        reg.resolve(&PolicyRef::parse(group))
            .chain
            .get(1)
            .cloned()
            .unwrap_or_default()
    }

    /// Phase 2 M3 design 6.4: the fastest that passes, the first that
    /// passes, any that passes — the first member (`load-balance`: any) when
    /// nothing passes or nothing was tested yet.
    #[test]
    fn the_automatic_groups_pick_by_the_tests() {
        let reg = generation(AUTO, &FakeFactory::new(), None);
        assert_eq!(picked(&reg, "U"), "A", "nothing tested: the first");
        assert_eq!(picked(&reg, "F"), "A");
        seed(&reg, "A", None);
        seed(&reg, "B", Some(200));
        seed(&reg, "C", Some(50));
        assert_eq!(picked(&reg, "U"), "C");
        assert_eq!(picked(&reg, "F"), "B");
        let ctx = SelectCtx {
            host: Some("example.com".into()),
        };
        let first = reg.resolve_with(&PolicyRef::parse("L"), &ctx).chain[1].clone();
        assert!(first == "B" || first == "C", "{first}");
        for _ in 0..10 {
            assert_eq!(
                reg.resolve_with(&PolicyRef::parse("L"), &ctx).chain[1],
                first,
                "persistent: one host, one member"
            );
        }
        // the views answer the same, and move nothing
        assert_eq!(reg.current_member("U").as_deref(), Some("C"));
        assert_eq!(reg.current_member("L").as_deref(), Some("B"));
        assert_eq!(reg.available("U"), ["B", "C"]);
    }

    /// A group scores as its pick; a `load-balance` group as the average of
    /// the members that pass (M3 design 6.4).
    #[test]
    fn a_group_member_scores_by_its_pick() {
        let reg = generation(AUTO, &FakeFactory::new(), None);
        seed(&reg, "A", None);
        seed(&reg, "B", Some(100));
        seed(&reg, "C", Some(300));
        assert_eq!(
            reg.resolve(&PolicyRef::parse("Outer")).chain,
            ["Outer", "N", "B"],
            "N scores as B, 100 ms"
        );
        assert_eq!(
            reg.standing("Avg", 0),
            Standing::Passed(Duration::from_millis(200))
        );
    }

    /// A dial asks for a round of the group whose results are older than its
    /// `interval` (none yet: older than anything); the views ask for
    /// nothing. Once the round is in, nothing more is asked until the
    /// interval has passed.
    #[test]
    fn a_dial_asks_for_a_round_and_the_views_do_not() {
        let reg = generation(AUTO, &FakeFactory::new(), None);
        reg.current_member("U");
        assert!(reg.auto().requested().is_empty());
        reg.resolve(&PolicyRef::parse("U"));
        assert_eq!(reg.auto().requested(), ["U"]);
        reg.auto().round_done(&["U".to_string()]);
        reg.resolve(&PolicyRef::parse("U"));
        assert!(reg.auto().requested().is_empty());
    }

    /// `evaluate-before-use`: until its first round is in, a dial that goes
    /// through the group is told to wait for it (M3 design 6.3).
    #[test]
    fn evaluate_before_use_waits_for_the_first_round() {
        let reg = generation(AUTO, &FakeFactory::new(), None);
        assert_eq!(
            reg.resolve(&PolicyRef::parse("S")).pending.as_deref(),
            Some("E")
        );
        assert_eq!(reg.resolve(&PolicyRef::parse("U")).pending, None);
        reg.auto().round_done(&["E".to_string()]);
        assert_eq!(reg.resolve(&PolicyRef::parse("E")).pending, None);
    }

    /// An override stands while it names a member, and asks for no test.
    #[test]
    fn an_override_stands_and_asks_for_no_round() {
        let reg = generation(AUTO, &FakeFactory::new(), None);
        seed(&reg, "B", Some(10));
        let spec = reg.group_spec("U").expect("a group").clone();
        reg.auto().set_override(&spec, "A");
        assert_eq!(picked(&reg, "U"), "A");
        assert_eq!(reg.current_member("U").as_deref(), Some("A"));
        assert!(reg.auto().requested().is_empty());
        reg.auto().set_override(&spec, "Gone");
        assert_eq!(picked(&reg, "U"), "B");
    }

    /// A round tests every member of the group and of the groups in it,
    /// and is recorded for each of those groups.
    #[tokio::test]
    async fn a_round_tests_every_member_of_the_group_and_its_groups() {
        let reg = generation(AUTO, &FakeFactory::new(), None);
        let available = reg.test_group("Outer").await;
        // the fake outbounds lead nowhere: every test fails
        assert!(available.is_empty());
        for name in ["A", "B", "C"] {
            assert!(reg.test_result(name).is_some_and(|r| r.outcome.is_err()));
        }
        assert!(reg.auto().last_round("Outer").is_some());
        assert!(reg.auto().last_round("N").is_some());
        assert!(reg.auto().last_round("U").is_none());
        // REJECT and DIRECT: never passes, and tested like the rest
        assert!(reg.test_case("REJECT").is_none());
        assert_eq!(
            reg.test_case("DIRECT").map(|c| c.policy).as_deref(),
            Some("DIRECT")
        );
    }

    /// A round asked for a group that a reload then took away still ends
    /// the request: a group of that name is tested again when asked.
    #[tokio::test]
    async fn a_round_of_a_group_that_is_gone_ends_the_request() {
        let reg = generation(AUTO, &FakeFactory::new(), None);
        reg.auto().wake("Gone");
        assert_eq!(reg.auto().requested(), ["Gone"]);
        assert!(reg.test_group("Gone").await.is_empty());
        assert!(reg.auto().requested().is_empty());
    }

    #[test]
    fn selections_api() {
```

Run: `cargo test -p rurge-policy`

Expected: 编译错误——`this function takes 7 arguments but 8 arguments were supplied`、`method `roots` is not a member of trait `OutboundFactory``、`no method named `resolve_with` / `test_case` / `auto` found`、`no field `pending` on type `registry::Resolution``。

- [ ] **Step 3: 写实现**

`crates/rurge-policy/src/factory.rs`——把

```rust
use rurge_proto::OutboundRef;
```

换成

```rust
use rurge_proto::OutboundRef;
use rustls::RootCertStore;
```

`crates/rurge-policy/src/factory.rs`——把

```rust
    fn environment(&self) -> String;
```

换成

```rust
    fn environment(&self) -> String;

    /// The trust anchors of the outbounds' TLS. A connectivity test of an
    /// `https` URL verifies the server with them too (M3 design 6.1).
    fn roots(&self) -> Arc<RootCertStore>;
```

`crates/rurge-policy/src/testbook.rs`——把

```rust
            .map(|(_, r)| r.clone())
```

换成

```rust
            .map(|(_, r)| r.clone())
    }

    /// Records a result as if a test had ended with `outcome`.
    #[cfg(test)]
    pub(crate) fn record(&self, policy: &str, key: u64, outcome: Result<Duration, String>) {
        let result = TestResult {
            outcome,
            at: Instant::now(),
            when: SystemTime::now(),
        };
        self.results
            .write()
            .expect("test results")
            .insert(policy.to_string(), (key, result));
```

`crates/rurge-policy/src/registry.rs`——把

```rust
use crate::assemble::Assembly;
```

换成

```rust
use crate::assemble::Assembly;
use crate::auto::{AutoGroups, SelectCtx, Standing, fallback, load_balance, url_test};
```

`crates/rurge-policy/src/registry.rs`——把

```rust
use rurge_config::rule::PolicyRef;
use rurge_config::spec::{CommonOpts, IpVersion, PolicySpec};
```

换成

```rust
use crate::testbook::{TestCase, TestResult};
use rurge_config::rule::PolicyRef;
use rurge_config::spec::{CommonOpts, GroupSpec, IpVersion, PolicySpec};
```

`crates/rurge-policy/src/registry.rs`——把

```rust
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::Path;
use std::sync::Arc;
```

换成

```rust
use rustls::RootCertStore;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use url::Url;
```

`crates/rurge-policy/src/registry.rs`——把

```rust
    pub note: Option<Note>,
```

换成

```rust
    pub note: Option<Note>,
    /// An `evaluate-before-use` group on the way that has not had its first
    /// round of tests: the dial waits for it and resolves again (M3 design
    /// 6.3). The outermost such group when there are several.
    pub pending: Option<String>,
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        kind: GroupKind,
        members: Vec<String>,
        hidden: bool,
```

换成

```rust
        spec: Arc<GroupSpec>,
        members: Vec<String>,
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        cycle: Option<String>,
    },
}
```

换成

```rust
        cycle: Option<String>,
    },
}

/// How a policy is tested (M3 design 6.1), worked out as the registry is
/// built.
struct TestSpec {
    /// `None`: the test URL does not parse, and the policy never passes.
    url: Option<Url>,
    timeout: Duration,
    /// What a result is good for (`TestCase::key`).
    key: u64,
}

impl TestSpec {
    fn new(definition: &str, url: &str, timeout: Duration) -> TestSpec {
        let mut h = DefaultHasher::new();
        (definition, url, timeout).hash(&mut h);
        TestSpec {
            url: Url::parse(url).ok(),
            timeout,
            key: h.finish(),
        }
    }
}
```

`crates/rurge-policy/src/registry.rs`——把

```rust
    empty_group: EmptyGroup,
}
```

换成

```rust
    empty_group: EmptyGroup,
    auto: Arc<AutoGroups>,
    /// By policy name; `DIRECT` for the built-in.
    tests: HashMap<String, TestSpec>,
    /// What verifies an `https` test URL (`OutboundFactory::roots`).
    roots: Arc<RootCertStore>,
}
```

`crates/rurge-policy/src/registry.rs`——把

```rust
    lines: HashMap<String, Line>,
}
```

换成

```rust
    lines: HashMap<String, Line>,
    tests: HashMap<String, TestSpec>,
}
```

`crates/rurge-policy/src/registry.rs`——把

```rust
    /// derived policies (M3 design 5.5).
```

换成

```rust
    /// derived policies (M3 design 5.5). The automatic groups pick by the
    /// test results `auto` keeps (M3 design 6.4).
    #[allow(clippy::too_many_arguments)]
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        empty_group: EmptyGroup,
```

换成

```rust
        empty_group: EmptyGroup,
        auto: &Arc<AutoGroups>,
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        let mut table = Table::default();
```

换成

```rust
        // How each policy is tested: its own `test-url` / `test-timeout`,
        // else the profile's (M3 design 6.1). A REJECT and a protocol not
        // implemented never pass, so they have none.
        let test_spec = |kind: PolicyKind, spec: Option<&PolicySpec>, definition: &str| {
            let direct = match (alias_terminal(kind), spec) {
                (Some(Terminal::Direct), _) => true,
                (Some(Terminal::Reject(_)), _) | (None, None) => return None,
                (None, Some(_)) => false,
            };
            let common = spec.map(|s| &s.common);
            let (url, timeout) = cfg.general.test_target(
                common.and_then(|c| c.test_url.as_deref()),
                common.and_then(|c| c.test_timeout),
                direct,
            );
            Some(TestSpec::new(definition, url, timeout))
        };
        let mut table = Table::default();
        let (url, timeout) = cfg.general.test_target(None, None, true);
        table
            .tests
            .insert("DIRECT".to_string(), TestSpec::new("DIRECT", url, timeout));
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            let entry = policy_entry(p.kind, cfg.spec(&p.name))?;
            table.add(&p.name, entry, Line::policy(p.kind, &p.definition));
```

换成

```rust
            let spec = cfg.spec(&p.name);
            let entry = policy_entry(p.kind, spec)?;
            table.add(&p.name, entry, Line::policy(p.kind, &p.definition));
            if let Some(t) = test_spec(p.kind, spec, &p.definition) {
                table.tests.insert(p.name.clone(), t);
            }
```

`crates/rurge-policy/src/registry.rs`——把

```rust
                Ok(entry) => table.add(
                    &i.policy.name,
                    entry,
                    Line::policy(i.policy.kind, &i.policy.definition),
                ),
```

换成

```rust
                Ok(entry) => {
                    table.add(
                        &i.policy.name,
                        entry,
                        Line::policy(i.policy.kind, &i.policy.definition),
                    );
                    if let Some(t) = test_spec(i.policy.kind, i.spec.as_ref(), &i.policy.definition)
                    {
                        table.tests.insert(i.policy.name.clone(), t);
                    }
                }
```

`crates/rurge-policy/src/registry.rs`——把

```rust
                Ok(entry) => table.add(
                    &d.spec.name,
                    entry,
                    Line::policy(d.spec.kind, &d.definition),
                ),
```

换成

```rust
                Ok(entry) => {
                    table.add(
                        &d.spec.name,
                        entry,
                        Line::policy(d.spec.kind, &d.definition),
                    );
                    if let Some(t) = test_spec(d.spec.kind, Some(&d.spec), &d.definition) {
                        table.tests.insert(d.spec.name.clone(), t);
                    }
                }
```

`crates/rurge-policy/src/registry.rs`——把

```rust
                kind: g.kind,
                members,
                hidden: g.hidden,
```

换成

```rust
                spec: Arc::new(g.clone()),
                members,
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            empty_group,
        })
```

换成

```rust
            empty_group,
            auto: auto.clone(),
            tests: table.tests,
            roots: factory.roots(),
        })
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            Entry::Group {
                kind,
                members,
                hidden,
                ..
            } => Some(GroupInfo {
                kind: *kind,
                hidden: *hidden,
```

换成

```rust
            Entry::Group { spec, members, .. } => Some(GroupInfo {
                kind: spec.kind,
                hidden: spec.hidden,
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            }),
            _ => None,
        }
    }

    /// The members of `group` as assembled; `None` when it is not a group.
```

换成

```rust
            }),
            _ => None,
        }
    }

    /// The group's definition, as the automatic groups' overrides keep it.
    pub fn group_spec(&self, name: &str) -> Option<&GroupSpec> {
        match self.entries.get(name)? {
            Entry::Group { spec, .. } => Some(spec),
            _ => None,
        }
    }

    /// The automatic groups' state this registry picks by.
    pub fn auto(&self) -> &Arc<AutoGroups> {
        &self.auto
    }

    /// The members of `group` as assembled; `None` when it is not a group.
```

`crates/rurge-policy/src/registry.rs`——把

```rust
    /// The member `group` points at right now: the live selection of a
    /// `select` group when it still names a member, else the first member.
    /// `None` when `group` is not a group or has no members.
    pub fn current_member(&self, group: &str) -> Option<String> {
        let Some(Entry::Group { kind, members, .. }) = self.entries.get(group) else {
            return None;
        };
        let selected = (*kind == GroupKind::Select)
            .then(|| self.selections.get(group))
            .flatten()
            .filter(|m| members.contains(m));
        selected.or_else(|| members.first().cloned())
```

换成

```rust
    /// The member `group` points at right now, as the control plane shows
    /// it: nothing changes for asking (no test is started, `url-test` does
    /// not move). `None` when `group` is not a group or has no members.
    pub fn current_member(&self, group: &str) -> Option<String> {
        self.choose(group, &SelectCtx::default(), false, 0)
            .map(|(member, _)| member)
    }

    /// The member of `group`, as a dial picks it. `select`: the live
    /// selection while it names a member, else the first member. The
    /// automatic groups: an override while it names a member, else by the
    /// test results (M3 design 6.4) — and when those are older than the
    /// group's `interval` a round is asked for. `live` is a dial: only then
    /// may `url-test` move the member it holds and a round be asked for;
    /// the second value then says whether the group wants its first round
    /// before it is used (`evaluate-before-use`).
    fn choose(
        &self,
        group: &str,
        ctx: &SelectCtx,
        live: bool,
        depth: usize,
    ) -> Option<(String, bool)> {
        let Some(Entry::Group { spec, members, .. }) = self.entries.get(group) else {
            return None;
        };
        match spec.kind {
            GroupKind::Select => {
                let selected = self.selections.get(group).filter(|m| members.contains(m));
                selected
                    .or_else(|| members.first().cloned())
                    .map(|m| (m, false))
            }
            GroupKind::UrlTest | GroupKind::Fallback | GroupKind::LoadBalance => {
                // an override stands, and asks for no test (M3 design 6.3)
                if let Some(member) = self.auto.override_of(group, members) {
                    return Some((member, false));
                }
                let last = self.auto.last_round(group);
                let pending = live && spec.test.evaluate_before_use && last.is_none();
                if live && last.is_none_or(|t| t.elapsed() >= spec.test.interval) {
                    self.auto.wake(group);
                }
                let standings: Vec<(String, Standing)> = members
                    .iter()
                    .map(|m| (m.clone(), self.standing(m, depth + 1)))
                    .collect();
                let member = match spec.kind {
                    GroupKind::UrlTest => {
                        let pick =
                            url_test(&standings, self.auto.pick(group).as_deref(), &spec.test)?;
                        if live {
                            self.auto.set_pick(group, &pick);
                        }
                        pick
                    }
                    GroupKind::Fallback => fallback(&standings, &spec.test)?,
                    _ if live => load_balance(&standings, &spec.test, ctx)?,
                    // the views: the first that passes stands for the group
                    _ => fallback(&standings, &spec.test)?,
                };
                Some((member, pending))
            }
            // `smart` (M3c) and `subnet` (phase 3): the first member
            GroupKind::Smart | GroupKind::Subnet => members.first().map(|m| (m.clone(), false)),
        }
    }

    /// What the tests say of `name`: its own last result, or — a group — its
    /// pick's, the average of those that pass for `load-balance` (M3 design
    /// 6.4). A REJECT, a protocol not implemented and a test URL that does
    /// not parse never pass.
    fn standing(&self, name: &str, depth: usize) -> Standing {
        if depth > MAX_DEPTH {
            return Standing::Failed;
        }
        if let PolicyRef::Named(n) = PolicyRef::parse(name)
            && let Some(Entry::Group {
                spec,
                members,
                cycle,
            }) = self.entries.get(&n)
        {
            if cycle.is_some() {
                return Standing::Failed;
            }
            if spec.kind == GroupKind::LoadBalance {
                let all: Vec<Standing> = members
                    .iter()
                    .map(|m| self.standing(m, depth + 1))
                    .collect();
                let passing: Vec<Duration> =
                    all.iter().filter_map(|s| s.passes(&spec.test)).collect();
                if passing.is_empty() {
                    return if all.iter().all(|s| *s == Standing::Unknown) {
                        Standing::Unknown
                    } else {
                        Standing::Failed
                    };
                }
                return Standing::Passed(passing.iter().sum::<Duration>() / passing.len() as u32);
            }
            return match self.choose(&n, &SelectCtx::default(), false, depth) {
                Some((member, _)) => self.standing(&member, depth + 1),
                None => Standing::Unknown,
            };
        }
        match self.test_case(name) {
            Some(case) => match self.auto.tests.result(&case.policy, case.key) {
                Some(result) => match result.outcome {
                    Ok(score) => Standing::Passed(score),
                    Err(_) => Standing::Failed,
                },
                None => Standing::Unknown,
            },
            None => Standing::Failed,
        }
    }

    /// How to test `name` now (M3 design 6.1): through its outbound, at its
    /// test URL. `None` for what never passes — a REJECT, a protocol not
    /// implemented, a test URL that does not parse — and for a group.
    pub fn test_case(&self, name: &str) -> Option<TestCase> {
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
        Some(TestCase {
            url: test.url.clone()?,
            timeout: test.timeout,
            key: test.key,
            roots: self.roots.clone(),
            policy,
            outbound,
        })
    }

    /// The last test result of `name` that still counts.
    pub fn test_result(&self, name: &str) -> Option<TestResult> {
        let case = self.test_case(name)?;
        self.auto.tests.result(&case.policy, case.key)
    }

    /// The members of `group` that pass their tests now.
    pub fn available(&self, group: &str) -> Vec<String> {
        let Some(Entry::Group { spec, members, .. }) = self.entries.get(group) else {
            return Vec::new();
        };
        members
            .iter()
            .filter(|m| self.standing(m, 1).passes(&spec.test).is_some())
            .cloned()
            .collect()
    }

    /// Tests every member of `group` now — the members of the groups in it
    /// too — and records the round for each of those groups; the members of
    /// `group` that pass (M3 design 6.3, 6.6). Every test runs on its own
    /// task (`TestBook::test`).
    pub async fn test_group(&self, group: &str) -> Vec<String> {
        let mut groups = Vec::new();
        let mut policies = Vec::new();
        self.gather(group, 0, &mut groups, &mut policies);
        if groups.is_empty() {
            // a reload took the group away after the round was asked for:
            // the request still ends, or it would stand in the way of the
            // next one for a group of that name
            groups.push(group.to_string());
        }
        let tests: Vec<_> = policies
            .iter()
            .filter_map(|p| self.test_case(p))
            .map(|case| {
                let book = self.auto.tests.clone();
                tokio::spawn(async move { book.test(case).await })
            })
            .collect();
        for test in tests {
            let _ = test.await;
        }
        self.auto.round_done(&groups);
        self.available(group)
    }

    /// The groups a round of `group` covers and the policies it tests.
    fn gather(
        &self,
        group: &str,
        depth: usize,
        groups: &mut Vec<String>,
        policies: &mut Vec<String>,
    ) {
        if depth > MAX_DEPTH || groups.iter().any(|g| g == group) {
            return;
        }
        let Some(Entry::Group { members, cycle, .. }) = self.entries.get(group) else {
            return;
        };
        groups.push(group.to_string());
        if cycle.is_some() {
            return;
        }
        for m in members {
            if let Some(Entry::Group { .. }) = self.entries.get(m.as_str()) {
                self.gather(m, depth + 1, groups, policies);
            } else if !policies.contains(m) {
                policies.push(m.clone());
            }
        }
```

`crates/rurge-policy/src/registry.rs`——把

```rust
    pub fn resolve(&self, policy: &PolicyRef) -> Resolution {
```

换成

```rust
    pub fn resolve(&self, policy: &PolicyRef) -> Resolution {
        self.resolve_with(policy, &SelectCtx::default())
    }

    /// `resolve`, for a dial that knows its target: `load-balance` with
    /// `persistent=true` picks by the host (M3 design 6.4).
    pub fn resolve_with(&self, policy: &PolicyRef, ctx: &SelectCtx) -> Resolution {
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            PolicyRef::Named(name) => self.named(name, &mut chain, 0, self.empty_group),
```

换成

```rust
            PolicyRef::Named(name) => self.named(name, &mut chain, 0, self.empty_group, ctx),
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        self.named(name, &mut Vec::new(), 0, EmptyGroup::Reject)
```

换成

```rust
        self.named(
            name,
            &mut Vec::new(),
            0,
            EmptyGroup::Reject,
            &SelectCtx::default(),
        )
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        empty: EmptyGroup,
```

换成

```rust
        empty: EmptyGroup,
        ctx: &SelectCtx,
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            Some(Entry::Group { .. }) => match self.current_member(name) {
                Some(member) => match PolicyRef::parse(&member) {
                    PolicyRef::Builtin(b) => self.builtin(b, chain),
                    PolicyRef::Device(d) => self.device(&d, chain),
                    PolicyRef::Named(n) => self.named(&n, chain, depth + 1, empty),
                },
```

换成

```rust
            Some(Entry::Group { .. }) => match self.choose(name, ctx, true, depth) {
                Some((member, pending)) => {
                    let mut resolution = match PolicyRef::parse(&member) {
                        PolicyRef::Builtin(b) => self.builtin(b, chain),
                        PolicyRef::Device(d) => self.device(&d, chain),
                        PolicyRef::Named(n) => self.named(&n, chain, depth + 1, empty, ctx),
                    };
                    if pending {
                        resolution.pending = Some(name.to_string());
                    }
                    resolution
                }
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            note,
        }
```

换成

```rust
            note,
            pending: None,
        }
```

要点：
- `choose(group, ctx, live, depth)` 是唯一的选择入口：`select` 读选择表；三种自动组先看覆盖，再按 `standing` 排好的成员选；`live` 时才请求测试（`AutoGroups::wake`，已请求的不会重复）、才更新 `url-test` 保持的成员、`load-balance` 才随机（控制面看到的是第一个通过的）。
- `standing(name, depth)` 与 `named` 一样受 `MAX_DEPTH` 约束。
- `TestSpec::new(definition, url, timeout)` 的 `key` 是三者的哈希；内置 `DIRECT` 也有一份（按直连类取 URL 与超时），在桌面上代替 DIRECT 的 iOS 专属内置策略也用它。

- [ ] **Step 4: 引擎按新签名构建**

`crates/rurge-engine/src/shared.rs`——把

```rust
use rurge_net::connector::Resolve;
use rurge_policy::{EmptyGroup, GroupSelections, RegistryCell, SelectionTable};
```

换成

```rust
use rurge_net::connector::Resolve;
use rurge_policy::auto::AutoGroups;
use rurge_policy::testbook::TestBook;
use rurge_policy::{EmptyGroup, GroupSelections, RegistryCell, SelectionTable};
```

`crates/rurge-engine/src/shared.rs`——把

```rust
    pub empty_group: EmptyGroup,
```

换成

```rust
    pub empty_group: EmptyGroup,
    /// The automatic groups' test results and state (phase 2 M3 design 6.2,
    /// 6.5): kept across generations, as the selections are.
    pub auto: Arc<AutoGroups>,
```

`crates/rurge-engine/src/shared.rs`——把

```rust
            empty_group: EmptyGroup::Direct,
```

换成

```rust
            empty_group: EmptyGroup::Direct,
            auto: Arc::new(AutoGroups::new(Arc::new(TestBook::new()))),
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
        format!("ipv6={}", self.v6_first)
```

换成

```rust
        format!("ipv6={}", self.v6_first)
    }

    fn roots(&self) -> Arc<RootCertStore> {
        self.roots.clone()
```

`crates/rurge-engine/src/runtime.rs`——把

```rust
                opts.shared.empty_group,
```

换成

```rust
                opts.shared.empty_group,
                &opts.shared.auto,
```

`crates/rurge-engine/src/subscriptions.rs`——把

```rust
            shared.empty_group,
```

换成

```rust
            shared.empty_group,
            &shared.auto,
```

`crates/rurge-engine/src/views.rs`——把

```rust
            EmptyGroup::Direct,
```

换成

```rust
            EmptyGroup::Direct,
            &crate::shared::EngineShared::default().auto,
```

本任务里引擎还不跑测试（调度任务在 Task 7）：拨号只会请求测试（请求攒在 `AutoGroups` 里），自动组照旧按"没有结果"选第一个成员。

- [ ] **Step 5: 运行**

Run: `cargo test -p rurge-policy` → 全部通过（新增 `the_automatic_groups_pick_by_the_tests`、`a_group_member_scores_by_its_pick`、`a_dial_asks_for_a_round_and_the_views_do_not`、`evaluate_before_use_waits_for_the_first_round`、`an_override_stands_and_asks_for_no_round`、`a_round_tests_every_member_of_the_group_and_its_groups`、`a_round_of_a_group_that_is_gone_ends_the_request`）。

Run: `cargo test -p rurge-engine` → 全部通过（行为不变）。

- [ ] **Step 6: 门禁与提交**

跑门禁（全工作区 39 个测试二进制，908 通过 / 1 忽略）。

```bash
git add crates/rurge-policy crates/rurge-engine/src
git commit -m "feat(policy): url-test / fallback / load-balance 按测试结果选成员——AutoGroups（覆盖、保持的成员、测试轮次），只有拨号请求测试，evaluate-before-use 的 pending"
```


### Task 7: 引擎——调度任务、测试会话、拨号的等待与 `SelectCtx`、重载保留覆盖

把 Task 6 的状态接进引擎（新模块 `rurge_engine::auto`）：

- **调度任务**（P14）：`Engine::new` 里 `start_tests`——给 `TestBook` 装上观察者，`AutoGroups::connect()` 取得测试请求，每个请求在**当时**的注册表上 `spawn(registry.test_group(group))`。任务只持 `Weak<Engine>`。
- **测试会话**（P20）：观察者 `TestSessions` 为每次测试开一个 `Internal` 会话：`rule` 为 `policy test`、`policy` 是被测的策略、目标是测试 URL 的主机与端口（不含路径与参数，M3-D7），测试结束时记 `Completed` 或 `Failed(原因)`。
- **拨号**：会话与 DNS 会话都经 `resolve_ready` 解析，带上 `SelectCtx { host: 目标主机 }`（P11）；解析结果有 `pending` 时，订阅 `rounds()`、在 `round_timeout` 之内等该组的第一轮测完，再解析一次；等完没有通过的成员 → `policy group evaluation failed`，会话的链记到该组为止（P9）。
- **重载**：`publish_generation` 里 `AutoGroups::retain(&next.config.group_specs)`（P12）。
- 注册表新增 `round_timeout(group)`：该组（含嵌套组）成员里最长的测试超时 × 每 8 个一批的批数。

**测试安全**：从本任务起，拨号会真的触发测试。引擎用例的 `Profile::text` 改为默认把 `proxy-test-url` / `internet-test-url` 指向没人监听的回环端口（`NO_TEST = http://127.0.0.1:9/`），用例自己的 `general` 写在后面、可以覆盖它——**绝不能**让任何用例测到默认的 `http://bing.com/`。写计划时核对过：其它测试配置里没有自动组（`tests/common/mod.rs` 的 `PICK` 里有个 `url-test` 组，但用到它的用例都不经它拨号；Task 8 给 API 夹具加同样的默认值）。

**Files:**
- Create: `crates/rurge-engine/src/auto.rs`、`crates/rurge-engine/tests/auto_groups.rs`
- Modify: `crates/rurge-engine/src/engine.rs`、`crates/rurge-engine/src/lib.rs`（`mod auto;`）、`crates/rurge-engine/Cargo.toml`（引用工作区的 `url`）、`Cargo.lock`
- Modify: `crates/rurge-engine/tests/common/mod.rs`（`NO_TEST` 与 `Profile::text`）
- Modify: `crates/rurge-policy/src/registry.rs`（`round_timeout` 与用例）

**Interfaces:**
- Consumes: Task 6 的 `AutoGroups::{connect, rounds, last_round, retain}`、`AutoGroups.tests`、`TestBook::observe`、`TestObserver` / `TestRecord`、`PolicyRegistry::{resolve_with, available, test_group, auto}`、`Resolution.pending`、`SelectCtx`；`Engine::{registry, shared, new_handle}`。
- Produces:
  - `PolicyRegistry::round_timeout(&self, group: &str) -> Duration`
  - `rurge_engine::auto`（crate 内）：`TEST_RULE = "policy test"`、`EVALUATION_FAILED = "policy group evaluation failed"`、`Engine::start_tests(self: &Arc<Self>)`、`resolve_ready(registry, policy, ctx) -> Result<Resolution, Vec<String>>`
  - `Engine::new_handle` 改为 `pub(crate)`
  - 引擎用例的 `common::NO_TEST`

- [ ] **Step 1: 用例先行**

测试 URL 默认指向回环：

`crates/rurge-engine/tests/common/mod.rs`——把

```rust
    .unwrap()
}

/// The variable parts of a test profile; everything else is fixed.
```

换成

```rust
    .unwrap()
}

/// A test URL nothing answers at.
pub const NO_TEST: &str = "http://127.0.0.1:9/";

/// The variable parts of a test profile; everything else is fixed.
```

`crates/rurge-engine/tests/common/mod.rs`——把

```rust
    pub fn text(&self, dns: SocketAddr) -> String {
        format!(
            "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\ndns-server = {dns}\nipv6 = false\n{}\n\
```

换成

```rust
    /// Connectivity tests go to a closed loopback port unless `general`
    /// says otherwise: never to the default `http://bing.com/`.
    pub fn text(&self, dns: SocketAddr) -> String {
        format!(
            "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\ndns-server = {dns}\nipv6 = false\n\
proxy-test-url = {NO_TEST}\ninternet-test-url = {NO_TEST}\n{}\n\
```

`round_timeout` 的单元用例：

`crates/rurge-policy/src/registry.rs`——把

```rust
            Some("DIRECT")
        );
    }

    /// A round asked for a group that a reload then took away still ends
```

换成

```rust
            Some("DIRECT")
        );
    }

    /// A round's tests run eight at a time, each within its own timeout.
    #[test]
    fn a_round_may_take_a_timeout_per_eight_tests() {
        let members: Vec<String> = (1..=9).map(|i| format!("P{i}")).collect();
        let proxies: String = members
            .iter()
            .map(|m| format!("{m} = http, 127.0.0.1, 80\n"))
            .collect();
        let profile = format!(
            "[Proxy]\n{proxies}Slow = http, 127.0.0.1, 80, test-timeout=7\n[Proxy Group]\n\
             Two = url-test, P1, P2\nNine = url-test, {}\nWithSlow = fallback, P1, Slow, REJECT\n\
             Nothing = fallback, REJECT\n[Rule]\nFINAL,DIRECT\n",
            members.join(", ")
        );
        let reg = generation(&profile, &FakeFactory::new(), None);
        assert_eq!(reg.round_timeout("Two"), Duration::from_secs(5));
        assert_eq!(reg.round_timeout("Nine"), Duration::from_secs(10));
        assert_eq!(reg.round_timeout("WithSlow"), Duration::from_secs(7));
        assert_eq!(reg.round_timeout("Nothing"), Duration::ZERO);
    }

    /// A round asked for a group that a reload then took away still ends
```

端到端用例：两个 SOCKS5 假上游 A、B 都把连接转到回环 `TestServer`，A 的测试 URL 是慢 300 毫秒的 `/slow`、B 的是 `/fast`（P18）；`Dead` 指向一个关着的回环端口。

新建 `crates/rurge-engine/tests/auto_groups.rs`：

```rust
//! The automatic groups through the whole engine (phase 2 M3 design 6.3,
//! 6.4, 10): a dial asks for a round of tests, the round runs on its own,
//! every test is a session of the request log, and the groups pick by the
//! results.

mod common;
use common::*;
use rurge_config::HostName;
use rurge_config::session::SessionInfo;
use rurge_engine::RequestRecord;
use rurge_inbound::{DialError, Dialer};
use std::collections::HashSet;

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

/// A and B: SOCKS5 upstreams that reach `origin` whatever they are asked
/// for, A tested at `/slow` (300 ms late), B at `/fast`. Dead: its server
/// refuses.
async fn upstreams(origin: &TestServer) -> (FakeSocks5, FakeSocks5, String) {
    origin.set("/slow", "");
    origin.set("/fast", "");
    origin.set_delay("/slow", Duration::from_millis(300));
    let script = || Socks5Script {
        connect_to: Some(origin_addr(origin)),
        ..Socks5Script::default()
    };
    let (a, b) = (
        FakeSocks5::spawn(script()).await,
        FakeSocks5::spawn(script()).await,
    );
    let proxies = format!(
        "A = socks5, 127.0.0.1, {}, test-url={}\nB = socks5, 127.0.0.1, {}, test-url={}\n\
         Dead = socks5, 127.0.0.1, {}",
        a.addr().port(),
        origin.url("/slow"),
        b.addr().port(),
        origin.url("/fast"),
        closed_port().await
    );
    (a, b, proxies)
}

async fn round_of(h: &Harness, group: &str) {
    wait_until(&format!("a round of tests of {group}"), || {
        h.engine.registry().auto().last_round(group).is_some()
    })
    .await;
}

#[tokio::test]
async fn url_test_moves_to_the_quicker_member_after_a_round() {
    let origin = TestServer::spawn().await;
    let (_a, _b, proxies) = upstreams(&origin).await;
    let h = harness(Profile {
        proxies: &proxies,
        groups: "U = url-test, A, B",
        rules: "DOMAIN,target.test,U",
        ..Profile::default()
    })
    .await;
    // no results yet: the first member; the dial asks for a round
    assert_eq!(chain_of(&h, "target.test").await, ["U", "A"]);
    round_of(&h, "U").await;
    // A answers 300 ms late: B is quicker by more than the tolerance
    assert_eq!(chain_of(&h, "target.test").await, ["U", "B"]);
}

#[tokio::test]
async fn fallback_passes_over_a_member_that_fails_its_test() {
    let origin = TestServer::spawn().await;
    let (_a, _b, proxies) = upstreams(&origin).await;
    let h = harness(Profile {
        proxies: &proxies,
        groups: "F = fallback, Dead, A, B",
        rules: "DOMAIN,target.test,F",
        ..Profile::default()
    })
    .await;
    // no results yet: the first member, whose server refuses
    assert!(matches!(
        h.engine.dial(session("target.test")).await,
        Err(DialError::Failed { .. })
    ));
    round_of(&h, "F").await;
    assert_eq!(chain_of(&h, "target.test").await, ["F", "A"]);
}

#[tokio::test]
async fn load_balance_with_persistent_keeps_each_host_on_one_member() {
    let origin = TestServer::spawn().await;
    let (_a, _b, proxies) = upstreams(&origin).await;
    let h = harness(Profile {
        proxies: &proxies,
        groups: "L = load-balance, Dead, A, B, persistent=true",
        rules: "DOMAIN-SUFFIX,lb.test,L",
        ..Profile::default()
    })
    .await;
    // any member may take the first dial: it only has to ask for the round
    let _ = h.engine.dial(session("first.lb.test")).await;
    round_of(&h, "L").await;
    let mut members = HashSet::new();
    for i in 0..20 {
        let host = format!("h{i}.lb.test");
        let chain = chain_of(&h, &host).await;
        for _ in 0..3 {
            assert_eq!(chain_of(&h, &host).await, chain, "{host} moved");
        }
        members.insert(chain[1].clone());
    }
    // Dead never passes; the hosts spread over the two that do
    assert_eq!(members, HashSet::from(["A".to_string(), "B".to_string()]));
}

#[tokio::test]
async fn evaluate_before_use_waits_for_the_first_round() {
    let origin = TestServer::spawn().await;
    let (_a, _b, proxies) = upstreams(&origin).await;
    let h = harness(Profile {
        proxies: &proxies,
        groups: "E = fallback, Dead, A, evaluate-before-use=true\n\
                 N = fallback, Dead, evaluate-before-use=true",
        rules: "DOMAIN,e.test,E\nDOMAIN,n.test,N",
        ..Profile::default()
    })
    .await;
    // the first dial waits for the round instead of trying Dead
    assert_eq!(chain_of(&h, "e.test").await, ["E", "A"]);
    match h.engine.dial(session("n.test")).await {
        Err(DialError::Failed {
            message, handle, ..
        }) => {
            assert_eq!(message, "policy group evaluation failed");
            assert_eq!(handle.policy_chain(), ["N"]);
        }
        Err(DialError::Reject { .. }) => panic!("expected a failure, got a reject"),
        Ok(_) => panic!("expected a failure, got a stream"),
    }
}

#[tokio::test]
async fn every_test_is_a_session_of_the_request_log() {
    let origin = TestServer::spawn().await;
    let (_a, _b, proxies) = upstreams(&origin).await;
    let h = harness(Profile {
        proxies: &proxies,
        groups: "F = fallback, Dead, A",
        rules: "DOMAIN,target.test,F",
        ..Profile::default()
    })
    .await;
    let _ = h.engine.dial(session("target.test")).await;
    round_of(&h, "F").await;
    let tests: Vec<RequestRecord> = h
        .engine
        .request_log()
        .recent(100)
        .into_iter()
        .filter(|r| r.rule.as_deref() == Some("policy test"))
        .collect();
    let a = tests.iter().find(|r| r.policy == ["A"]).expect("A tested");
    assert_eq!(a.listener, ListenerKind::Internal);
    // the test URL's host and port, nothing of its path
    assert_eq!(a.dst, origin_addr(&origin).to_string());
    assert_eq!(a.status, RecordStatus::Completed);
    let dead = tests
        .iter()
        .find(|r| r.policy == ["Dead"])
        .expect("Dead tested");
    assert_eq!(dead.status, RecordStatus::Failed);
    assert!(
        dead.error
            .as_deref()
            .is_some_and(|e| e.starts_with("connect: ")),
        "{:?}",
        dead.error
    );
}

/// A group's own `underlying-proxy` makes its members `M (via R)`; such a
/// member is tested through the relay, as it is dialled (M3 design 5.4).
#[tokio::test]
async fn a_derived_member_is_tested_through_its_relay() {
    let origin = TestServer::spawn().await;
    let (a, _b, proxies) = upstreams(&origin).await;
    // whatever it is asked for, the relay tunnels to exactly that
    let relay = FakeSocks5::spawn(Socks5Script::default()).await;
    let h = harness(Profile {
        proxies: &format!("{proxies}\nR = socks5, 127.0.0.1, {}", relay.addr().port()),
        groups: "U = url-test, A, underlying-proxy=R",
        rules: "DOMAIN,target.test,U",
        ..Profile::default()
    })
    .await;
    assert_eq!(chain_of(&h, "target.test").await, ["U", "A (via R)"]);
    round_of(&h, "U").await;
    let result = h
        .engine
        .registry()
        .test_result("A (via R)")
        .expect("tested");
    assert!(result.outcome.is_ok(), "{:?}", result.outcome);
    // the relay carried both the dial and the test to A's server
    let to_a = relay
        .requests()
        .iter()
        .filter(|r| r.port == a.addr().port())
        .count();
    assert_eq!(to_a, 2);
}

/// An override stands while the group is defined the same way (M3 design
/// 6.5): a reload of the same profile keeps it, one that changes the group
/// drops it.
#[tokio::test]
async fn a_reload_keeps_an_override_while_the_group_stays_the_same() {
    let origin = TestServer::spawn().await;
    let (_a, _b, proxies) = upstreams(&origin).await;
    let h = harness(Profile {
        proxies: &proxies,
        groups: "U = url-test, A, B",
        ..Profile::default()
    })
    .await;
    let registry = h.engine.registry();
    registry
        .auto()
        .set_override(registry.group_spec("U").unwrap(), "B");
    let current = || h.engine.registry().current_member("U");
    assert_eq!(current().as_deref(), Some("B"));
    let text = std::fs::read_to_string(h.dir.path().join("t.conf")).unwrap();
    h.engine
        .swap_runtime(runtime(h.dir.path(), &text, h.engine.shared()).await);
    assert_eq!(current().as_deref(), Some("B"));
    let changed = text.replace("U = url-test, A, B", "U = url-test, A, B, interval=60");
    h.engine
        .swap_runtime(runtime(h.dir.path(), &changed, h.engine.shared()).await);
    assert_eq!(
        current().as_deref(),
        Some("A"),
        "no results: the first member"
    );
}
```

Run: `cargo test -p rurge-engine --test auto_groups`

Expected: 7 个全部 FAIL——`url_test_moves_to_the_quicker_member_after_a_round`、`fallback_passes_over_a_member_that_fails_its_test`、`load_balance_with_persistent_keeps_each_host_on_one_member`、`every_test_is_a_session_of_the_request_log`、`a_derived_member_is_tested_through_its_relay` 都是 `timed out waiting for a round of tests of <组>`（没有调度任务，测试请求没人跑）；`evaluate_before_use_waits_for_the_first_round` 是 `e.test: … (os error 10061)`（没有等待，第一次拨号直接试了 `Dead`）；`a_reload_keeps_an_override_while_the_group_stays_the_same` 是 `no results: the first member`（`left: Some("B")`，重载没有清掉覆盖）。

Run: `cargo test -p rurge-policy round`

Expected: 编译错误——`no method named `round_timeout` found for struct `registry::PolicyRegistry``。

- [ ] **Step 2: 写实现**

`crates/rurge-policy/src/registry.rs`——把

```rust
use crate::testbook::{TestCase, TestResult};
```

换成

```rust
use crate::testbook::{MAX_CONCURRENT_TESTS, TestCase, TestResult};
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            .filter(|m| self.standing(m, 1).passes(&spec.test).is_some())
            .cloned()
            .collect()
    }

    /// Tests every member of `group` now — the members of the groups in it
```

换成

```rust
            .filter(|m| self.standing(m, 1).passes(&spec.test).is_some())
            .cloned()
            .collect()
    }

    /// How long a round of tests of `group` may take: its tests run
    /// `MAX_CONCURRENT_TESTS` at a time, each within its own timeout.
    pub fn round_timeout(&self, group: &str) -> Duration {
        let mut groups = Vec::new();
        let mut policies = Vec::new();
        self.gather(group, 0, &mut groups, &mut policies);
        let timeouts: Vec<Duration> = policies
            .iter()
            .filter_map(|p| self.test_case(p))
            .map(|case| case.timeout)
            .collect();
        let longest = timeouts.iter().max().copied().unwrap_or_default();
        longest * timeouts.len().div_ceil(MAX_CONCURRENT_TESTS) as u32
    }

    /// Tests every member of `group` now — the members of the groups in it
```

`crates/rurge-engine/Cargo.toml`——把

```toml
sha2.workspace = true
```

换成

```toml
sha2.workspace = true
url.workspace = true
```

新建 `crates/rurge-engine/src/auto.rs`：

```rust
//! The engine's side of the automatic groups (phase 2 M3 design 6.2, 6.3):
//! the task that runs the rounds of tests the registry asks for, every test
//! a session of the request log, and the dial's wait for the first round of
//! an `evaluate-before-use` group.

use crate::engine::Engine;
use rurge_config::HostName;
use rurge_config::rule::PolicyRef;
use rurge_config::session::{ListenerKind, SessionInfo};
use rurge_inbound::{SessionHandle, SessionOutcome};
use rurge_policy::auto::SelectCtx;
use rurge_policy::testbook::{TestObserver, TestRecord};
use rurge_policy::{PolicyRegistry, Resolution};
use std::sync::{Arc, Weak};
use std::time::Duration;
use url::Url;

/// The rule a test session shows in the request log.
pub(crate) const TEST_RULE: &str = "policy test";

/// Why a session through an `evaluate-before-use` group fails when no
/// member passes the group's first round of tests (M3 design 6.3).
pub(crate) const EVALUATION_FAILED: &str = "policy group evaluation failed";

/// Every connectivity test is a session of the request log (M3 design
/// 6.2): `Internal`, the rule `policy test`, the tested policy for its
/// chain, and for its target the test URL's host and port — never the rest
/// of the URL, which a subscription line may have set (M3-D7).
struct TestSessions(Weak<Engine>);

struct TestSession(Arc<SessionHandle>);

/// A test that begins while the engine goes away.
struct Unrecorded;

impl TestObserver for TestSessions {
    fn begin(&self, policy: &str, url: &Url) -> Box<dyn TestRecord> {
        let Some(engine) = self.0.upgrade() else {
            return Box::new(Unrecorded);
        };
        let host = HostName::parse(url.host_str().unwrap_or_default());
        let mut session = SessionInfo::tcp(host, url.port_or_known_default().unwrap_or(0));
        session.listener = ListenerKind::Internal;
        let handle = engine.new_handle(session);
        handle.set_rule(Some(TEST_RULE.to_string()));
        handle.set_policy_chain(vec![policy.to_string()]);
        Box::new(TestSession(handle))
    }
}

impl TestRecord for TestSession {
    fn end(self: Box<Self>, outcome: &Result<Duration, String>) {
        self.0.finish(match outcome {
            Ok(_) => SessionOutcome::Completed,
            Err(why) => SessionOutcome::Failed(why.clone()),
        });
    }
}

impl TestRecord for Unrecorded {
    fn end(self: Box<Self>, _outcome: &Result<Duration, String>) {}
}

impl Engine {
    /// Starts the task that runs the rounds of tests the registry asks for,
    /// each against the registry in use when the round starts (M3 design
    /// 6.3); it ends with the engine. Called once, by `Engine::new`.
    pub(crate) fn start_tests(self: &Arc<Self>) {
        let auto = self.shared().auto;
        auto.tests
            .observe(Arc::new(TestSessions(Arc::downgrade(self))));
        let mut requests = auto.connect();
        let engine = Arc::downgrade(self);
        tokio::spawn(async move {
            while let Some(group) = requests.recv().await {
                let Some(registry) = engine.upgrade().map(|e| e.registry()) else {
                    break;
                };
                tokio::spawn(async move {
                    registry.test_group(&group).await;
                });
            }
        });
    }
}

/// `registry.resolve_with`, and — when an `evaluate-before-use` group on
/// the way has not had its first round of tests yet — the same again once
/// that round has ended, waiting at most as long as the round may take (M3
/// design 6.3, 9). `Err`: no member of that group passes after the wait; it
/// carries the chain up to the group.
pub(crate) async fn resolve_ready(
    registry: &PolicyRegistry,
    policy: &PolicyRef,
    ctx: &SelectCtx,
) -> Result<Resolution, Vec<String>> {
    let first = registry.resolve_with(policy, ctx);
    let Some(group) = first.pending.clone() else {
        return Ok(first);
    };
    let auto = registry.auto();
    // subscribed before looking: a round that ends in between is not missed
    let mut rounds = auto.rounds();
    let _ = tokio::time::timeout(registry.round_timeout(&group), async {
        while auto.last_round(&group).is_none() {
            if rounds.changed().await.is_err() {
                break;
            }
        }
    })
    .await;
    if registry.available(&group).is_empty() {
        let mut chain = first.chain;
        if let Some(i) = chain.iter().position(|name| *name == group) {
            chain.truncate(i + 1);
        }
        return Err(chain);
    }
    Ok(registry.resolve_with(policy, ctx))
}
```

`crates/rurge-engine/src/lib.rs`——把

```rust
//! listeners, and the session log.

pub mod control;
```

换成

```rust
//! listeners, and the session log.

mod auto;
pub mod control;
```

`crates/rurge-engine/src/engine.rs`——把

```rust
//! relays bytes, binds listeners from `[General]`, writes the session log.

use crate::control::Mode;
```

换成

```rust
//! relays bytes, binds listeners from `[General]`, writes the session log.

use crate::auto::{EVALUATION_FAILED, resolve_ready};
use crate::control::Mode;
```

`crates/rurge-engine/src/engine.rs`——把

```rust
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
```

换成

```rust
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rurge_policy::auto::SelectCtx;
```

`crates/rurge-engine/src/engine.rs`——把

```rust
        engine.watch_subscriptions(&rt, receivers);
```

换成

```rust
        engine.watch_subscriptions(&rt, receivers);
        engine.start_tests();
```

`crates/rurge-engine/src/engine.rs`——把

```rust
    fn new_handle(&self, session: SessionInfo) -> Arc<SessionHandle> {
```

换成

```rust
    pub(crate) fn new_handle(&self, session: SessionInfo) -> Arc<SessionHandle> {
```

`crates/rurge-engine/src/engine.rs`——把

```rust
    /// through its resolver.
```

换成

```rust
    /// through its resolver; the automatic groups keep what still applies
    /// to its groups (M3 design 6.5).
```

`crates/rurge-engine/src/engine.rs`——把

```rust
        self.shared.resolver.store(next.stack.resolver.clone());
```

换成

```rust
        self.shared.resolver.store(next.stack.resolver.clone());
        self.shared.auto.retain(&next.config.group_specs);
```

`crates/rurge-engine/src/engine.rs`——把

```rust
        };
        let resolution = registry.resolve(&policy);
```

换成

```rust
        };
        let ctx = SelectCtx {
            host: Some(handle.session().dst_host.to_string()),
        };
        let resolution = match resolve_ready(&registry, &policy, &ctx).await {
            Ok(resolution) => resolution,
            Err(chain) => {
                handle.set_policy_chain(chain);
                handle.finish(SessionOutcome::Failed(EVALUATION_FAILED.to_string()));
                return Err(io::Error::other(EVALUATION_FAILED));
            }
        };
```

`crates/rurge-engine/src/engine.rs`——把

```rust
            let resolution = registry.resolve(&policy);
```

换成

```rust
            // `load-balance` with `persistent=true` picks by the host
            let ctx = SelectCtx {
                host: Some(handle.session().dst_host.to_string()),
            };
            let resolution = match resolve_ready(&registry, &policy, &ctx).await {
                Ok(resolution) => resolution,
                Err(chain) => {
                    handle.set_policy_chain(chain);
                    return fail(handle, FailKind::Connect, EVALUATION_FAILED);
                }
            };
```

要点：
- `resolve_ready` 先订阅 `rounds()`、再看 `last_round`：恰好在两者之间结束的一轮不会被错过。等待有上限（`round_timeout`），过了上限就按当时的结果判断。
- `dial_internal`（DNS 会话）同样等待；评估失败时结束会话并返回 `policy group evaluation failed` 的 I/O 错误（不走"直连保 DNS"的旁路——所有成员都测不通，直连也未必通，与代理拨号失败时一致）。
- 链的中间跳（`ChainConnector`）不等（P10）。
- `Cargo.lock` 由 cargo 写入 `rurge-engine` 依赖列表里的 `url` 一行。

- [ ] **Step 3: 运行**

Run: `cargo test -p rurge-engine --test auto_groups` → 7 passed。

Run: `cargo test -p rurge-policy` → 全部通过（新增 `a_round_may_take_a_timeout_per_eight_tests`）。

Run: `cargo test -p rurge-engine` → 全部通过。

- [ ] **Step 4: 门禁与提交**

跑门禁（全工作区 **40** 个测试二进制，916 通过 / 1 忽略）。

```bash
git add Cargo.lock crates/rurge-engine crates/rurge-policy/src/registry.rs
git commit -m "feat(engine): 自动组的测试调度任务、测试会话进请求记录、evaluate-before-use 的有界等待、拨号带 SelectCtx、重载时只保留未变组的覆盖"
```


### Task 8: 三个测试端点与 `select` 对自动组的覆盖

设计 6.6 与 M3-D10（P13）。引擎新增三个方法：`test_policies`（按名字立即测；省略 URL 时各自的测试 URL、结果保存，给了 URL 时一次性测试、结果不保存）、`test_group`（立即测一个组的全部成员，返回通过的）、`test_results`（每个自动组的成员与最近结果）；`TestBook` 为此新增 `test_once`。`select_group` 对三种自动组设覆盖、空串清除；`smart`（M3c 之前）与 `subnet` 仍不接受，错误文本改为 `` `G` does not take a selection ``。`rurge-api` 新增 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test`。

**测试安全**：API 用例的夹具加上 `proxy-test-url = http://127.0.0.1:9/`（它的 `internet-test-url` 早已指向回环的 `TestServer`）；新用例的自动组只含 `DIRECT` 与未实现的 `ss`，测试只会去回环。

**Files:**
- Modify: `crates/rurge-policy/src/testbook.rs`（`test_once`，`run` 拆出 `measure`；用例）
- Modify: `crates/rurge-engine/src/{auto.rs, engine.rs, views.rs, lib.rs}`；`crates/rurge-engine/tests/{auto_groups.rs, outbounds.rs}`
- Modify: `crates/rurge-api/src/{lib.rs, routes/policies.rs, routes/policy_groups.rs}`、`crates/rurge-api/Cargo.toml`（引用工作区的 `url`）、`Cargo.lock`、`crates/rurge-api/tests/api.rs`

**Interfaces:**
- Consumes: Task 5 的 `TestBook::{test, observe}`、`TestCase`；Task 6 的 `PolicyRegistry::{test_case, test_result, test_group, group_spec, members, auto}`、`AutoGroups::{set_override, clear_override}`；Task 7 的 `rurge_engine::auto` 模块；`engine::{UnknownPolicy, policy_known}`、`views::SelectError`。
- Produces:
  - `TestBook::test_once(&self, case: &TestCase) -> TestResult`
  - `Engine::test_policies(&self, names: &[String], url: Option<Url>) -> Result<Vec<(String, Option<TestResult>)>, UnknownPolicy>`
  - `Engine::test_group(&self, group: &str) -> Result<Vec<String>, SelectError>`
  - `Engine::test_results(&self) -> Vec<(String, Vec<(String, Option<TestResult>)>)>`
  - `rurge_engine::TestResult`（再导出 `rurge_policy::testbook::TestResult`）
  - `Engine::select_group` 对自动组的覆盖；`SelectError::NotSelectable` 的文本 `` `G` does not take a selection ``
  - 引擎内：`auto::automatic(GroupKind) -> bool`、`auto::Results`；`engine::policy_known` 改为 `pub(crate)`
  - API：`routes::policies::test`、`routes::policies::result_json`（crate 内）、`routes::policy_groups::{test, test_results}`

- [ ] **Step 1: 用例先行**

`TestBook`：一次性测试同样被观察、不留结果：

`crates/rurge-policy/src/testbook.rs`——把

```rust
        assert!(book.result("P", 1).is_some());
    }

    /// A test outlives whoever asked for it: its result is kept.
```

换成

```rust
        assert!(book.result("P", 1).is_some());
    }

    /// A one-off test is seen like any other, and leaves nothing behind.
    #[tokio::test]
    async fn a_one_off_test_keeps_nothing() {
        let seen = Arc::new(Seen::default());
        let book = book();
        book.observe(Arc::new(seen.clone()));
        let once = book
            .test_once(&case("P", Arc::new(Gate::default()), 1))
            .await;
        assert_eq!(once.outcome, Err("connect: closed by the gate".to_string()));
        assert_eq!(book.result("P", 1), None);
        assert_eq!(seen.0.lock().unwrap().len(), 2, "begin and end");
    }

    /// A test outlives whoever asked for it: its result is kept.
```

引擎：`select_group` 对自动组设覆盖——覆盖期间不请求测试，清除后第一次拨号请求一轮；原来断言 `Auto` 不可选的用例改为用 `smart` 组断言，并补一条"不是成员"：

`crates/rurge-engine/tests/auto_groups.rs`——把

```rust
    assert_eq!(to_a, 2);
}

/// An override stands while the group is defined the same way (M3 design
```

换成

```rust
    assert_eq!(to_a, 2);
}

/// An override is what the group dials, and while it stands the group
/// asks for no round of tests (M3 design 6.3, 6.5); clearing it gives the
/// group back to its tests.
#[tokio::test]
async fn select_on_an_automatic_group_overrides_it_until_cleared() {
    let origin = TestServer::spawn().await;
    let (_a, _b, proxies) = upstreams(&origin).await;
    let h = harness(Profile {
        proxies: &proxies,
        groups: "U = url-test, A, B",
        rules: "DOMAIN,target.test,U",
        ..Profile::default()
    })
    .await;
    h.engine.select_group("U", "B").await.unwrap();
    assert_eq!(chain_of(&h, "target.test").await, ["U", "B"]);
    let auto = h.engine.registry().auto().clone();
    assert!(auto.requested().is_empty() && auto.last_round("U").is_none());
    h.engine.select_group("U", "").await.unwrap();
    assert_eq!(chain_of(&h, "target.test").await, ["U", "A"]);
    round_of(&h, "U").await;
}

/// An override stands while the group is defined the same way (M3 design
```

`crates/rurge-engine/tests/outbounds.rs`——把

```rust
async fn only_a_member_of_a_select_group_can_be_selected() {
```

换成

```rust
async fn only_a_member_can_be_selected() {
```

`crates/rurge-engine/tests/outbounds.rs`——把

```rust
        proxies: &proxies,
        groups: PICK,
        ..Profile::default()
```

换成

```rust
        proxies: &proxies,
        groups: &format!("{PICK}\nSmart = smart, A, B"),
        ..Profile::default()
```

`crates/rurge-engine/tests/outbounds.rs`——把

```rust
        h.engine.select_group("Auto", "A").await,
        Err(SelectError::NotSelectable("Auto".into()))
```

换成

```rust
        h.engine.select_group("Smart", "A").await,
        Err(SelectError::NotSelectable("Smart".into()))
    );
    assert_eq!(
        h.engine.select_group("Auto", "C").await,
        Err(SelectError::NotAMember {
            group: "Auto".into(),
            member: "C".into()
        })
```

`crates/rurge-engine/tests/outbounds.rs`——把

```rust
        "`G` is not a select group"
```

换成

```rust
        "`G` does not take a selection"
```

API：夹具多一个 `api_in(rules, groups)`，三条新用例：

`crates/rurge-api/tests/api.rs`——把

```rust
async fn api_with(rules: &str) -> Api {
```

换成

```rust
async fn api_with(rules: &str) -> Api {
    api_in(rules, "").await
}

/// `groups` go after `Pick`. A proxy is tested at a closed loopback port,
/// never at the default `http://bing.com/`.
async fn api_in(rules: &str, groups: &str) -> Api {
```

`crates/rurge-api/tests/api.rs`——把

```rust
internet-test-url = http://target.test:{}/hello\n\
[Proxy]\nHK = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\nBlock = reject-tinygif\n\
[Proxy Group]\nPick = select, HK, DIRECT\n\
```

换成

```rust
internet-test-url = http://target.test:{}/hello\nproxy-test-url = http://127.0.0.1:9/\n\
[Proxy]\nHK = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\nBlock = reject-tinygif\n\
[Proxy Group]\nPick = select, HK, DIRECT\n{groups}\n\
```

`crates/rurge-api/tests/api.rs`——把

```rust
        "nothing changed"
    );
}

#[tokio::test]
async fn profile_check_includes_the_dry_build() {
```

换成

```rust
        "nothing changed"
    );
}

/// Falls over from HK (a protocol rurge does not speak yet: it never
/// passes) to DIRECT, tested at `internet-test-url` on the loopback server.
const AUTO: &str = "Auto = fallback, HK, DIRECT";

#[tokio::test]
async fn policies_are_tested_on_request() {
    let api = api_in("", AUTO).await;
    // at a URL of the caller's: a one-off, nothing is kept
    let (status, body) = post(
        &api,
        "/v1/policies/test",
        json!({ "policy_names": ["DIRECT"], "url": api.target_url() }),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert!(body["DIRECT"]["delay"].is_u64(), "{body}");
    assert!(body["DIRECT"]["time"].is_f64(), "{body}");
    let (_, results) = get(&api, "/v1/policy_groups/test_results").await;
    assert_eq!(results["Auto"]["DIRECT"], Value::Null, "{results}");
    // at each one's own test URL: kept, and the groups pick by it
    let (status, body) = post(
        &api,
        "/v1/policies/test",
        json!({ "policy_names": ["DIRECT", "Block", "HK", "Pick"] }),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert!(body["DIRECT"]["delay"].is_u64(), "{body}");
    for name in ["Block", "HK", "Pick"] {
        assert_eq!(body[name], json!({ "error": "not testable" }), "{name}");
    }
    let (_, results) = get(&api, "/v1/policy_groups/test_results").await;
    assert!(results["Auto"]["DIRECT"]["delay"].is_u64(), "{results}");
    assert_eq!(
        post(
            &api,
            "/v1/policies/test",
            json!({ "policy_names": ["DIRECT", "Nope"] })
        )
        .await,
        (400, json!({ "error": "unknown policy `Nope`" }))
    );
    assert_eq!(
        post(
            &api,
            "/v1/policies/test",
            json!({ "policy_names": ["DIRECT"], "url": "not a url" })
        )
        .await,
        (400, json!({ "error": "`url` is not a URL" }))
    );
}

#[tokio::test]
async fn a_group_is_tested_on_request() {
    let api = api_in("", AUTO).await;
    assert_eq!(
        get(&api, "/v1/policy_groups/test_results").await,
        (200, json!({ "Auto": { "HK": null, "DIRECT": null } })),
        "the automatic groups only, nothing tested yet"
    );
    assert_eq!(
        post(
            &api,
            "/v1/policy_groups/test",
            json!({ "group_name": "Auto" })
        )
        .await,
        (200, json!({ "available": ["DIRECT"] }))
    );
    let (_, results) = get(&api, "/v1/policy_groups/test_results").await;
    assert!(results["Auto"]["DIRECT"]["delay"].is_u64(), "{results}");
    assert_eq!(results["Auto"]["HK"], Value::Null);
    assert_eq!(
        get(&api, "/v1/policy_groups/select?group_name=Auto").await,
        (200, json!({ "policy": "DIRECT" })),
        "HK fails: the group falls over to DIRECT"
    );
    assert_eq!(
        post(
            &api,
            "/v1/policy_groups/test",
            json!({ "group_name": "Nope" })
        )
        .await,
        (400, json!({ "error": "unknown policy group `Nope`" }))
    );
}

#[tokio::test]
async fn select_on_an_automatic_group_overrides_it_until_cleared() {
    let api = api_in("DOMAIN,target.test,Auto", AUTO).await;
    let select = |policy: &str| json!({ "group_name": "Auto", "policy": policy });
    assert_eq!(
        post(&api, "/v1/policy_groups/select", select("DIRECT")).await,
        (200, json!({}))
    );
    assert_eq!(
        get(&api, "/v1/policy_groups/select?group_name=Auto").await,
        (200, json!({ "policy": "DIRECT" }))
    );
    let (head, body) = get_via_proxy(api.http(), &api.target_url()).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, b"hi there");
    // an override is not written to state.json
    let saved: Value = std::fs::read_to_string(&api.state_path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null);
    assert_eq!(saved["group_selections"]["t.conf"]["Auto"], Value::Null);
    // an empty `policy` clears it: nothing tested yet, the first member
    assert_eq!(
        post(&api, "/v1/policy_groups/select", select("")).await,
        (200, json!({}))
    );
    assert_eq!(
        get(&api, "/v1/policy_groups/select?group_name=Auto").await,
        (200, json!({ "policy": "HK" }))
    );
    assert_eq!(
        post(&api, "/v1/policy_groups/select", select("Block")).await,
        (400, json!({ "error": "`Block` is not a member of `Auto`" }))
    );
}

#[tokio::test]
async fn profile_check_includes_the_dry_build() {
```

Run: `cargo test -p rurge-policy testbook`

Expected: 编译错误——`no method named `test_once` found`。

Run: `cargo test -p rurge-engine --no-fail-fast --test auto_groups --test outbounds`

Expected: FAIL——`select_on_an_automatic_group_overrides_it_until_cleared` 在第一行 `select_group("U", "B").await.unwrap()` 上 panic（`NotSelectable`）；`only_a_member_can_be_selected` 的 `left: Err(NotSelectable("Auto"))`、`right: Err(NotAMember { group: "Auto", member: "C" })`。

Run: `cargo test -p rurge-api --test api`

Expected: 3 个 FAIL——`policies_are_tested_on_request`（`left: 404`，`{"error":"no such endpoint"}`）、`a_group_is_tested_on_request`（同样 404）、`select_on_an_automatic_group_overrides_it_until_cleared`（400 `` `Auto` is not a select group ``）。

- [ ] **Step 2: `TestBook::test_once`**

`crates/rurge-policy/src/testbook.rs`——把

```rust
    async fn run(self: Arc<Self>, case: TestCase, tx: watch::Sender<Option<TestResult>>) {
        let outcome = {
            let _permit = self.permits.acquire().await.expect("never closed");
            let record = self
                .observer
                .get()
                .map(|observer| observer.begin(&case.policy, &case.url));
            let outcome = match probe(&case.outbound, &case.url, case.timeout, case.roots.clone())
                .await
            {
                Probed::Passed { score, reused } => {
                    if !reused
                        && self
                            .warned
                            .lock()
                            .expect("warned")
                            .insert(case.url.to_string())
                    {
                        // the URL stays out of the log: a subscription
                        // line may have set it (M3-D7)
                        tracing::warn!(
                            policy = %case.policy,
                            "the test server does not keep the connection: the score includes the dial"
                        );
                    }
                    Ok(score)
                }
                Probed::Failed(why) => Err(why),
            };
            if let Some(record) = record {
                record.end(&outcome);
            }
            outcome
        };
```

换成

```rust
    /// Tests `case` once, beside whatever else runs: the result is not
    /// kept and nobody else waits for it — the API's test at a URL of the
    /// caller's choosing (M3 design 6.6).
    pub async fn test_once(&self, case: &TestCase) -> TestResult {
        let outcome = self.measure(case).await;
        TestResult {
            outcome,
            at: Instant::now(),
            when: SystemTime::now(),
        }
    }

    /// One test, within the concurrency limit, seen by the observer.
    async fn measure(&self, case: &TestCase) -> Result<Duration, String> {
        let _permit = self.permits.acquire().await.expect("never closed");
        let record = self
            .observer
            .get()
            .map(|observer| observer.begin(&case.policy, &case.url));
        let outcome = match probe(&case.outbound, &case.url, case.timeout, case.roots.clone()).await
        {
            Probed::Passed { score, reused } => {
                if !reused
                    && self
                        .warned
                        .lock()
                        .expect("warned")
                        .insert(case.url.to_string())
                {
                    // the URL stays out of the log: a subscription line may
                    // have set it (M3-D7)
                    tracing::warn!(
                        policy = %case.policy,
                        "the test server does not keep the connection: the score includes the dial"
                    );
                }
                Ok(score)
            }
            Probed::Failed(why) => Err(why),
        };
        if let Some(record) = record {
            record.end(&outcome);
        }
        outcome
    }

    async fn run(self: Arc<Self>, case: TestCase, tx: watch::Sender<Option<TestResult>>) {
        let outcome = self.measure(&case).await;
```

`run` 的测量部分原样挪进 `measure`（并发许可、观察者、探针、告警一次），`test_once` 与 `run` 共用它。

- [ ] **Step 3: 引擎的三个方法与 `select` 扩展**

`crates/rurge-engine/src/auto.rs`——把

```rust
use crate::engine::Engine;
use rurge_config::HostName;
use rurge_config::rule::PolicyRef;
use rurge_config::session::{ListenerKind, SessionInfo};
use rurge_inbound::{SessionHandle, SessionOutcome};
use rurge_policy::auto::SelectCtx;
use rurge_policy::testbook::{TestObserver, TestRecord};
```

换成

```rust
use crate::engine::{Engine, UnknownPolicy, policy_known};
use crate::views::SelectError;
use rurge_config::rule::PolicyRef;
use rurge_config::session::{ListenerKind, SessionInfo};
use rurge_config::{GroupKind, HostName};
use rurge_inbound::{SessionHandle, SessionOutcome};
use rurge_policy::auto::SelectCtx;
use rurge_policy::testbook::{TestObserver, TestRecord, TestResult};
```

`crates/rurge-engine/src/auto.rs`——把

```rust
pub(crate) const EVALUATION_FAILED: &str = "policy group evaluation failed";
```

换成

```rust
pub(crate) const EVALUATION_FAILED: &str = "policy group evaluation failed";

/// Test results by policy (or member), in the order asked for (or
/// listed); `None` where there is none.
pub(crate) type Results = Vec<(String, Option<TestResult>)>;

/// The groups that pick by the tests, and take an override for a
/// selection: `url-test`, `fallback`, `load-balance`.
pub(crate) fn automatic(kind: GroupKind) -> bool {
    matches!(
        kind,
        GroupKind::UrlTest | GroupKind::Fallback | GroupKind::LoadBalance
    )
}
```

`crates/rurge-engine/src/auto.rs`——把

```rust
                });
            }
        });
    }
}

/// `registry.resolve_with`, and — when an `evaluate-before-use` group on
```

换成

```rust
                });
            }
        });
    }

    /// Tests `names` now, side by side, whatever their groups' `interval`
    /// (M3 design 6.3, 6.6): each at its own test URL — the groups then
    /// pick by these results — or all at `url`, a one-off whose results
    /// are not kept. `None` for what cannot be tested: a group, a REJECT, a
    /// protocol not implemented yet, a test URL that does not parse.
    pub async fn test_policies(
        &self,
        names: &[String],
        url: Option<Url>,
    ) -> Result<Results, UnknownPolicy> {
        let registry = self.registry();
        if let Some(name) = names.iter().find(|name| !policy_known(&registry, name)) {
            return Err(UnknownPolicy(name.clone()));
        }
        let book = registry.auto().tests.clone();
        let tests: Vec<_> = names
            .iter()
            .map(|name| {
                let (case, book, url) = (registry.test_case(name), book.clone(), url.clone());
                tokio::spawn(async move {
                    let mut case = case?;
                    Some(match url {
                        None => book.test(case).await,
                        Some(url) => {
                            case.url = url;
                            book.test_once(&case).await
                        }
                    })
                })
            })
            .collect();
        let mut out = Vec::with_capacity(names.len());
        for (name, test) in names.iter().zip(tests) {
            out.push((name.clone(), test.await.ok().flatten()));
        }
        Ok(out)
    }

    /// Tests every member of `group` now, whatever its `interval`, and the
    /// members of the groups in it; the members of `group` that pass (M3
    /// design 6.3, 6.6).
    pub async fn test_group(&self, group: &str) -> Result<Vec<String>, SelectError> {
        let registry = self.registry();
        if registry.group(group).is_none() {
            return Err(SelectError::UnknownGroup(group.to_string()));
        }
        Ok(registry.test_group(group).await)
    }

    /// The last test result of every member of every automatic group, the
    /// groups in profile order; `None` for a member without one (not tested
    /// yet, or never tested: a group, a REJECT).
    pub fn test_results(&self) -> Vec<(String, Results)> {
        let registry = self.registry();
        registry
            .group_names()
            .into_iter()
            .filter_map(|name| {
                let group = registry.group(&name)?;
                if !automatic(group.kind) {
                    return None;
                }
                let members = group
                    .members
                    .iter()
                    .map(|m| (m.clone(), registry.test_result(m)))
                    .collect();
                Some((name, members))
            })
            .collect()
    }
}

/// `registry.resolve_with`, and — when an `evaluate-before-use` group on
```

`crates/rurge-engine/src/engine.rs`——把

```rust
fn policy_known(registry: &PolicyRegistry, name: &str) -> bool {
```

换成

```rust
pub(crate) fn policy_known(registry: &PolicyRegistry, name: &str) -> bool {
```

`crates/rurge-engine/src/views.rs`——把

```rust
//! change: the selection of a `select` group (M1 design 6.3). Everything is
//! read from the registry in use, so imported and derived members show up
//! as they come and go (phase 2 M3 design 5.5).

use crate::engine::Engine;
```

换成

```rust
//! change: the selection of a `select` group (M1 design 6.3), which for an
//! automatic group is an override (phase 2 M3-D10). Everything is read from
//! the registry in use, so imported and derived members show up as they
//! come and go (phase 2 M3 design 5.5).

use crate::auto::automatic;
use crate::engine::Engine;
```

`crates/rurge-engine/src/views.rs`——把

```rust
            SelectError::NotSelectable(g) => write!(f, "`{g}` is not a select group"),
```

换成

```rust
            SelectError::NotSelectable(g) => write!(f, "`{g}` does not take a selection"),
```

`crates/rurge-engine/src/views.rs`——把

```rust
    /// Takes effect for the next connection and is written to `state.json`
    /// under the profile's file name.
```

换成

```rust
    /// A `select` group: the selection, which takes effect for the next
    /// connection and is written to `state.json` under the profile's file
    /// name. An automatic group: an override, which stands until an empty
    /// `member` clears it, a reload changes the group, or the process ends
    /// (M3-D10, M3 design 6.5).
```

`crates/rurge-engine/src/views.rs`——把

```rust
            let Some(g) = registry.group(group) else {
                return Err(SelectError::UnknownGroup(group.to_string()));
            };
            if g.kind != GroupKind::Select {
                return Err(SelectError::NotSelectable(group.to_string()));
            }
            if !g.members.iter().any(|m| m == member) {
```

换成

```rust
            let Some(spec) = registry.group_spec(group) else {
                return Err(SelectError::UnknownGroup(group.to_string()));
            };
            let is_auto = automatic(spec.kind);
            if !is_auto && spec.kind != GroupKind::Select {
                return Err(SelectError::NotSelectable(group.to_string()));
            }
            if is_auto && member.is_empty() {
                registry.auto().clear_override(group);
                return Ok(());
            }
            if !registry
                .members(group)
                .unwrap_or_default()
                .iter()
                .any(|m| m == member)
            {
```

`crates/rurge-engine/src/views.rs`——把

```rust
                });
```

换成

```rust
                });
            }
            if is_auto {
                registry.auto().set_override(spec, member);
                return Ok(());
```

`crates/rurge-engine/src/lib.rs`——把

```rust
pub use rurge_policy::EmptyGroup;
```

换成

```rust
pub use rurge_policy::EmptyGroup;
pub use rurge_policy::testbook::TestResult;
```

要点：
- `test_policies` 先校验全部名字（有一个不认识就整个请求失败），再为每个名字 `spawn` 一个任务：请求被放弃时测试照样结束。给了 `url` 时只换 `TestCase.url`，超时仍用策略自己的；走 `test_once`，结果不进 `TestBook`。
- `test_results` 只列 `automatic` 的三种组（`smart` 随 M3c）。
- `select_group`：先按 `group_spec` 取组；自动组 + 空串 → 清除覆盖；非成员 → `NotAMember`；自动组 → `set_override`（不写 `state.json`）；`select` 组的路径不变。

- [ ] **Step 4: API**

`crates/rurge-api/Cargo.toml`——把

```toml
tracing.workspace = true
```

换成

```toml
tracing.workspace = true
url.workspace = true
```

`crates/rurge-api/src/lib.rs`——把

```rust
        .route("/v1/policies/detail", get(routes::policy_groups::detail))
```

换成

```rust
        .route("/v1/policies/detail", get(routes::policy_groups::detail))
        .route("/v1/policies/test", post(routes::policies::test))
```

`crates/rurge-api/src/lib.rs`——把

```rust
            get(routes::policy_groups::selection).post(routes::policy_groups::select),
```

换成

```rust
            get(routes::policy_groups::selection).post(routes::policy_groups::select),
        )
        .route("/v1/policy_groups/test", post(routes::policy_groups::test))
        .route(
            "/v1/policy_groups/test_results",
            get(routes::policy_groups::test_results),
```

`crates/rurge-api/src/routes/policies.rs`——把

```rust
//! `GET /v1/policies` and `GET /v1/rules`.

use crate::App;
use axum::Json;
use axum::extract::State;
use serde::Serialize;
```

换成

```rust
//! `GET /v1/policies`, `POST /v1/policies/test` and `GET /v1/rules`.

use crate::App;
use crate::error::{ApiError, ApiResult, json_body};
use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use rurge_engine::TestResult;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::time::UNIX_EPOCH;
use url::Url;
```

`crates/rurge-api/src/routes/policies.rs`——把

```rust
    })
}
```

换成

```rust
    })
}

#[derive(Deserialize)]
pub struct TestPolicies {
    pub policy_names: Vec<String>,
    /// Test every policy here instead of at its own test URL.
    #[serde(default)]
    pub url: Option<String>,
}

/// `POST /v1/policies/test` (phase 2 M3 design 6.6): `{"<name>": Result…}`.
/// The manual gives no response sample: the shape is provisional.
pub async fn test(
    State(app): State<App>,
    body: Result<Json<TestPolicies>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let body = json_body(body)?;
    let url = match body.url.as_deref() {
        None | Some("") => None,
        Some(text) => {
            Some(Url::parse(text).map_err(|_| ApiError::bad_request("`url` is not a URL"))?)
        }
    };
    let results = app
        .engine
        .test_policies(&body.policy_names, url)
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let mut out = Map::new();
    for (name, result) in results {
        let result = match result {
            Some(result) => result_json(&result),
            None => json!({ "error": "not testable" }),
        };
        out.insert(name, result);
    }
    Ok(Json(Value::Object(out)))
}

/// A test result as the API shows it: `delay` in milliseconds, or the
/// `error` the test failed with; `time`, when it ended, in Unix seconds.
pub(crate) fn result_json(result: &TestResult) -> Value {
    let time = result
        .when
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    match &result.outcome {
        Ok(delay) => json!({ "delay": delay.as_millis() as u64, "time": time }),
        Err(error) => json!({ "error": error, "time": time }),
    }
}
```

`crates/rurge-api/src/routes/policy_groups.rs`——把

```rust
//! `GET /v1/policies/detail`, `GET /v1/policy_groups` and
//! `GET/POST /v1/policy_groups/select` (M1 design 6.6). The first two have no
//! response sample in the manual: their shapes are provisional.
```

换成

```rust
//! `GET /v1/policies/detail`, `GET /v1/policy_groups`,
//! `GET/POST /v1/policy_groups/select` (M1 design 6.6), and
//! `POST /v1/policy_groups/test` and `GET /v1/policy_groups/test_results`
//! (phase 2 M3 design 6.6). The manual has no response sample for
//! `detail`, `policy_groups` and `test_results`: their shapes are
//! provisional.
```

`crates/rurge-api/src/routes/policy_groups.rs`——把

```rust
use crate::error::{ApiError, ApiResult, json_body, query_params};
```

换成

```rust
use crate::error::{ApiError, ApiResult, json_body, query_params};
use crate::routes::policies::result_json;
```

`crates/rurge-api/src/routes/policy_groups.rs`——把

```rust
    Ok(Json(json!({})))
}

```

换成

```rust
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
pub struct TestGroup {
    pub group_name: String,
}

pub async fn test(
    State(app): State<App>,
    body: Result<Json<TestGroup>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let body = json_body(body)?;
    let available = app
        .engine
        .test_group(&body.group_name)
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(Json(json!({ "available": available })))
}

/// `{"<group>": {"<member>": Result… | null}}` for every automatic group.
pub async fn test_results(State(app): State<App>) -> Json<Value> {
    let mut body = Map::new();
    for (group, members) in app.engine.test_results() {
        let members: Map<String, Value> = members
            .into_iter()
            .map(|(member, result)| (member, result.as_ref().map_or(Value::Null, result_json)))
            .collect();
        body.insert(group, Value::Object(members));
    }
    Json(Value::Object(body))
}

```

`url` 为空串等于省略；解析不了 → 400 `` `url` is not a URL ``（不回显取值）。结果的 JSON 见 P13；`Cargo.lock` 由 cargo 写入 `rurge-api` 依赖列表里的 `url` 一行。

- [ ] **Step 5: 运行**

Run: `cargo test -p rurge-policy testbook` → 7 passed（新增 `a_one_off_test_keeps_nothing`）。

Run: `cargo test -p rurge-engine` → 全部通过（`auto_groups` 8 条；`outbounds` 里改名后的 `only_a_member_can_be_selected`）。

Run: `cargo test -p rurge-api` → 全部通过（新增 `policies_are_tested_on_request`、`a_group_is_tested_on_request`、`select_on_an_automatic_group_overrides_it_until_cleared`）。

- [ ] **Step 6: 门禁与提交**

跑门禁（全工作区 40 个测试二进制，921 通过 / 1 忽略）。

```bash
git add Cargo.lock crates/rurge-policy/src/testbook.rs crates/rurge-engine crates/rurge-api
git commit -m "feat(api): POST /v1/policies/test、GET /v1/policy_groups/test_results、POST /v1/policy_groups/test；select 对自动组即临时覆盖"
```


### Task 9: 拨号入口的两条承接问题（承接 C4、C5）

M3a 把这两条留到"拨号入口会改"的 M3b：

- **C5（M3a #14，P15）**：拨号先取当前代（规则）、后取注册表，而重载先发布注册表、后换代；恰好跨过重载的会话可能用旧一代的规则选出一个名字，再到新一代的注册表里解析——重载删掉或改了名的策略会让这条连接 REJECT 并打一条 ERROR。新增 `Engine::snapshot()`：在代际锁下成对读取运行时与注册表；两处发布（重载的"发布 + 切换"、订阅重建的"核对 + 发布"）都持这把锁，所以拿到的一对永远属于同一代。锁里只有两次 `ArcSwap` 读取，不做 I/O。这个窗口只有几条指令宽，没有稳定复现它的用例——按构造保证，评审时核对两处发布都在锁里即可。
- **C4（M3a #3，P16）**：DNS 会话（`encrypted-dns-follow-outbound-mode`）命中的代理，链底下若是 REJECT（例如中继是当前选中 REJECT 的组），今天 `ChainConnector` 报 `via <组>: rejected by REJECT`，DNS 查询就此失败；而会话自己命中 REJECT 时是告警并直连的。`socket_opener` 返回 `None`（链底是 REJECT，或比 `MAX_DEPTH` 还深——注册表对后者也是 REJECT）时，同样告警并直连。

**Files:**
- Modify: `crates/rurge-engine/src/engine.rs`
- Test: `crates/rurge-engine/tests/pipeline.rs`

**Interfaces:**
- Consumes: M3a 的 `Engine::generation_lock`、`socket_opener`、`bypass_to_direct`；Task 7 的 `resolve_ready`。
- Produces: `Engine::snapshot(&self) -> (Arc<Runtime>, Arc<PolicyRegistry>)`（私有）；`dial_internal` 的新旁路，会话记录的 `error` 为 `dns-follow: reject bypassed to keep DNS working`（与会话自己被 REJECT 时的文本相同）。

- [ ] **Step 1: 用例先行**

`crates/rurge-engine/tests/pipeline.rs`——把

```rust
        );
    }
}

/// The other direction: `Up` is named by host name too, but its own server
```

换成

```rust
        );
    }
}

/// A chain whose hop below the proxy is REJECT can never carry the DNS
/// session: it is bypassed like a REJECT of the session itself, instead of
/// failing the lookup.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dns_session_bypasses_a_proxy_chain_that_ends_at_a_reject() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let dir = tempfile::tempdir().unwrap();
    let profile = format!(
        "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\n\
encrypted-dns-follow-outbound-mode = true\nencrypted-dns-server = tcp://127.0.0.1:{}\nipv6 = false\n\
[Proxy]\nUpIp = http, 127.0.0.1, 9, underlying-proxy=Pick\n\
[Proxy Group]\nPick = select, REJECT, DIRECT\n[Rule]\nPROTOCOL,DNS,UpIp\nFINAL,DIRECT\n",
        dns.addr().port()
    );
    let engine = engine_from_profile(dir.path(), &profile).await;
    let res = tokio::time::timeout(
        Duration::from_secs(5),
        engine
            .runtime()
            .stack
            .resolver
            .lookup("target.test", rurge_dns::resolver::LookupOpts::default()),
    )
    .await
    .expect("the lookup ends inside the bound");
    assert!(res.is_ok(), "resolution around the chain: {res:?}");
    let internal = internal_sessions(&engine);
    assert!(
        internal.iter().any(|r| {
            r.error.as_deref() == Some("dns-follow: reject bypassed to keep DNS working")
                && r.policy.first().map(String::as_str) == Some("UpIp")
        }),
        "a bypassed internal DNS session: {internal:?}"
    );
}

/// The other direction: `Up` is named by host name too, but its own server
```

Run: `cargo test -p rurge-engine --test pipeline ends_at_a_reject`

Expected: FAIL——`resolution around the chain: Err(AllFailed([("tcp://127.0.0.1:<端口>", "io: via Pick: rejected by REJECT")]))`。

- [ ] **Step 2: 写实现**

`crates/rurge-engine/src/engine.rs`——把

```rust
            .expect("published before the engine is handed out")
```

换成

```rust
            .expect("published before the engine is handed out")
    }

    /// The current generation and the registry in use, as one pair. Both
    /// are published under the generation lock — a reload publishes the
    /// two together, a subscription rebuild a registry for the same
    /// generation — so a session never picks a name by one generation's
    /// rules and resolves it in another generation's registry.
    fn snapshot(&self) -> (Arc<Runtime>, Arc<PolicyRegistry>) {
        let _generation = self.generation_lock();
        (self.runtime(), self.registry())
```

`crates/rurge-engine/src/engine.rs`——把

```rust
        let rt = self.runtime();
        let registry = self.registry();
```

换成

```rust
        let (rt, registry) = self.snapshot();
```

`crates/rurge-engine/src/engine.rs`——把

```rust
        // dialling. A proxy configured by IP literal has no such loop.
        if resolution.terminal == TerminalKind::Proxy
            && let Some(terminal) = resolution.chain.last()
            && let Some(spec) = socket_opener(&registry, terminal)
            && matches!(spec.server, Some(HostName::Domain(_)))
        {
            tracing::warn!(
                policy = %spec.name,
                "DNS session routed to a proxy configured by host name; connecting directly to avoid a resolution loop"
            );
            return bypass_to_direct(
                handle,
                fallback,
                &target,
                &opts,
                "dns-follow: proxy configured by host name bypassed to avoid a resolution loop",
            )
            .await;
```

换成

```rust
        // dialling. A proxy configured by IP literal has no such loop. A chain
        // that ends at REJECT below the proxy is bypassed like a REJECT of the
        // session itself.
        if resolution.terminal == TerminalKind::Proxy
            && let Some(terminal) = resolution.chain.last()
        {
            match socket_opener(&registry, terminal) {
                Some(spec) if matches!(spec.server, Some(HostName::Domain(_))) => {
                    tracing::warn!(
                        policy = %spec.name,
                        "DNS session routed to a proxy configured by host name; connecting directly to avoid a resolution loop"
                    );
                    return bypass_to_direct(
                        handle,
                        fallback,
                        &target,
                        &opts,
                        "dns-follow: proxy configured by host name bypassed to avoid a resolution loop",
                    )
                    .await;
                }
                None => {
                    tracing::warn!(
                        policy = %terminal,
                        "DNS session routed to a proxy chain that ends at a reject; connecting directly to keep DNS working"
                    );
                    return bypass_to_direct(
                        handle,
                        fallback,
                        &target,
                        &opts,
                        "dns-follow: reject bypassed to keep DNS working",
                    )
                    .await;
                }
                Some(_) => {}
            }
```

`crates/rurge-engine/src/engine.rs`——把

```rust
            let rt = self.runtime();
            // loaded once: the name is approved and resolved against the same one
            let registry = self.registry();
```

换成

```rust
            // loaded once, together: the name is picked, approved and
            // resolved against the same generation
            let (rt, registry) = self.snapshot();
```

- [ ] **Step 3: 运行**

Run: `cargo test -p rurge-engine --test pipeline` → 全部通过（新增 `a_dns_session_bypasses_a_proxy_chain_that_ends_at_a_reject`；原有三条"以域名配置的代理被旁路"与"以 IP 配置的代理照常承载"的用例不变）。

- [ ] **Step 4: 门禁与提交**

跑门禁（全工作区 40 个测试二进制，922 通过 / 1 忽略）。

```bash
git add crates/rurge-engine/src/engine.rs crates/rurge-engine/tests/pipeline.rs
git commit -m "fix(engine): 拨号在代际锁下成对读取运行时与注册表；DNS 会话命中链底为 REJECT 的代理时告警并直连"
```


### Task 10: 能力表翻转

bin 的能力表加上 `url-test` / `fallback` / `load-balance`：`rurge check` 与启动时不再为它们报 `W0008`（`policy group type … is not implemented in this version; the first member is used`）。`smart`（M3c）与 `subnet`（阶段 3）仍报 `W0008`（P17）。翻转前核对设计承诺的守卫都已在：测速类参数的读取、校验与 `W0028`（M3a Task 1）；`test-url` / `test-timeout` 生效、不再 `W0029`（Task 3）；组算法、测试、覆盖与 API（Task 6 – 8）。

**Files:**
- Modify: `crates/rurge/src/capabilities.rs`
- Test: `crates/rurge/tests/cli.rs`

**Interfaces:**
- Consumes: `rurge_config::config::Capabilities.group_kinds`。
- Produces: `rurge::capabilities::current().group_kinds` = `{Select, UrlTest, Fallback, LoadBalance}`。

- [ ] **Step 1: 用例先行**

`crates/rurge/tests/cli.rs`——把

```rust
        .stdout(predicate::str::contains("hunter2").not());
```

换成

```rust
        .stdout(predicate::str::contains("hunter2").not());
}

const GROUPS: &str = "[General]\n[Proxy]\n\
H = http, proxy.test, 8080, test-url=http://127.0.0.1:9/, test-timeout=3\n[Proxy Group]\n\
U = url-test, H, DIRECT, tolerance=50\nF = fallback, H, DIRECT, evaluate-before-use=true\n\
L = load-balance, H, DIRECT, persistent=true\nS = smart, H, DIRECT\n[Rule]\nFINAL,U\n";

#[test]
fn check_knows_the_automatic_groups() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "groups.conf", GROUPS))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    // `smart` is still a later milestone; the three automatic groups are
    // not, and the testing options are in effect (no W0029)
    assert_eq!(out.matches("W0008").count(), 1, "{out}");
    assert!(out.contains("`smart`"), "{out}");
    assert!(!out.contains("W0029"), "{out}");
```

Run: `cargo test -p rurge --test cli automatic_groups`

Expected: FAIL——`check_knows_the_automatic_groups` 的第一条断言：`W0008` 出现 4 次而不是 1 次。

- [ ] **Step 2: 写实现**

`crates/rurge/src/capabilities.rs`——把

```rust
//! M2b), and `select` groups.
```

换成

```rust
//! M2b), `select` groups, and `url-test` / `fallback` / `load-balance`
//! groups (phase 2 M3b).
```

`crates/rurge/src/capabilities.rs`——把

```rust
        group_kinds: HashSet::from([GroupKind::Select]),
```

换成

```rust
        group_kinds: HashSet::from([
            GroupKind::Select,
            GroupKind::UrlTest,
            GroupKind::Fallback,
            GroupKind::LoadBalance,
        ]),
```

- [ ] **Step 3: 运行**

Run: `cargo test -p rurge --test cli` → 全部通过（新增 `check_knows_the_automatic_groups`）。

- [ ] **Step 4: 门禁与提交**

跑门禁（全工作区 40 个测试二进制，923 通过 / 1 忽略）。

```bash
git add crates/rurge/src/capabilities.rs crates/rurge/tests/cli.rs
git commit -m "feat(rurge): 能力表翻转 url-test / fallback / load-balance（W0008 只剩 smart 与 subnet）"
```


### Task 11: 文档

**Files:**
- Modify: `docs/surge-compatibility-matrix.md`、`docs/api/phase2.md`、`docs/acceptance/phase2-manual.md`
- Modify: `docs/superpowers/specs/2026-09-23-phase2-m3-groups-subscriptions-design.md`（新增第 17 节）
- Modify: `docs/superpowers/plans/2026-09-23-phase2-m3a-subscriptions-plan.md`（「延后事项」#3 / #11 / #12 / #14 的去向）
- Modify: `README.md`、`README_en.md`、`CLAUDE.md`
- Modify: 本计划末尾「执行期修正记录」与「延后事项」两张表

- [ ] **Step 1: 兼容性清单**

`[General]` 的三个测试项、策略参数 `test-url` / `test-timeout` / `underlying-proxy`、5.1 的三种组与"嵌套与循环""临时覆盖"、5.2 的五个测速参数与组级 `underlying-proxy` / `policy-path`（P1）/ "测试 URL / 超时解析顺序"、两行 `encrypted-dns-follow-outbound-mode`（P16）、四个 API 端点与请求记录。`POST /v1/policies/test` 与 `GET /v1/policy_groups/test_results` 的响应形状是暂定的，从 ✅ 改为 🟡；附录统计随之调整（第 10 节 ✅ 36 → 34、🟡 6 → 8）。

`docs/surge-compatibility-matrix.md`——把

```markdown
| `encrypted-dns-follow-outbound-mode` | 布尔；默认 false | 全部 | 🟡 | 1 | 含"代理服务器为域名时回退 DIRECT 并告警"的防环逻辑；M3b：TCP/DoT/DoH 上游连接走流水线（成 Internal 会话，`PROTOCOL,DOH/DOT/DNS` 可匹配）；上游主机名由 Bootstrap 解析，流水线只见 IP 目标，故域名规则不匹配上游主机名；协议标签按端口启发（853→DoT，443→DoH，其余→DNS）；被规则 REJECT 时告警并直连以保 DNS；UDP 上游不经连接器；这类内部会话的 `SRC-IP` 恒为 `127.0.0.1`、`IN-PORT` 恒为 `0`，`SRC-IP,127.0.0.1/32` / `IN-PORT,0` 规则可能意外匹配到它们，且它们的 `kill` 是空操作（DNS 路径不监听取消令牌）；防环回退自阶段 2 / M1b 起实现：DNS 会话命中的代理（沿 underlying-proxy 找到真正打开 socket 的那一跳；底下是 DIRECT——direct 别名策略或当前选中 DIRECT 的组——时取它上面那一跳，因为 DIRECT 要在本机解析的正是那一跳的服务器名）若以域名配置，则告警并直连；以 IP 配置的代理照常承载 DNS 会话 |
```

换成

```markdown
| `encrypted-dns-follow-outbound-mode` | 布尔；默认 false | 全部 | 🟡 | 1 | 含"代理服务器为域名时回退 DIRECT 并告警"的防环逻辑；M3b：TCP/DoT/DoH 上游连接走流水线（成 Internal 会话，`PROTOCOL,DOH/DOT/DNS` 可匹配）；上游主机名由 Bootstrap 解析，流水线只见 IP 目标，故域名规则不匹配上游主机名；协议标签按端口启发（853→DoT，443→DoH，其余→DNS）；被规则 REJECT 时告警并直连以保 DNS；UDP 上游不经连接器；这类内部会话的 `SRC-IP` 恒为 `127.0.0.1`、`IN-PORT` 恒为 `0`，`SRC-IP,127.0.0.1/32` / `IN-PORT,0` 规则可能意外匹配到它们，且它们的 `kill` 是空操作（DNS 路径不监听取消令牌）；防环回退自阶段 2 / M1b 起实现：DNS 会话命中的代理（沿 underlying-proxy 找到真正打开 socket 的那一跳；底下是 DIRECT——direct 别名策略或当前选中 DIRECT 的组——时取它上面那一跳，因为 DIRECT 要在本机解析的正是那一跳的服务器名）若以域名配置，则告警并直连；以 IP 配置的代理照常承载 DNS 会话；阶段 2 / M3b 起，链底下是 REJECT 的代理（中继是当前选中 REJECT 的组等）同样告警并直连，不让 DNS 查询失败 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `internet-test-url` | URL；默认 `http://bing.com/` | 全部 | ✅ | 2 | |
| `proxy-test-url` | URL；默认 `http://bing.com/` | 全部 | ✅ | 2 | |
| `test-timeout` | 秒；默认 5（DIRECT 为 10） | 全部 | ✅ | 2 | |
```

换成

```markdown
| `internet-test-url` | URL；默认 `http://bing.com/` | 全部 | ✅ | 2 | M3b 起是直连类策略（`DIRECT` 与 `direct` 别名）连通性测试的默认 URL（策略自己的 `test-url` 优先） |
| `proxy-test-url` | URL；默认 `http://bing.com/` | 全部 | ✅ | 2 | M3b 起是代理策略连通性测试的默认 URL（策略自己的 `test-url` 优先） |
| `test-timeout` | 秒；默认 5（DIRECT 为 10） | 全部 | ✅ | 2 | M3b 生效：没写时代理类默认 5 秒、直连类 10 秒；写了就对两类都适用（策略自己的 `test-timeout` 优先） |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `test-url` | HTTP(S) URL；默认全局设置 | ✅ | 2 | M1 解析并校验取值，`W0029`；M3b 生效 |
| `test-timeout` | 秒；默认全局设置 | ✅ | 2 | M1 解析并校验取值，`W0029`；M3b 生效 |
| `test-udp` | `hostname@ipv4` | ✅ | 2 | M1 解析并校验取值，`W0029`；M5 生效（M3 细化设计订正了原来的"M3 生效"） |
| `underlying-proxy` | 另一策略或策略组名；仅代理策略；不能与 `port-hopping` 同用 | ✅ | 2 | 目标代理主机名在上游远程解析；M1 已实现（TCP）：底层可以是策略或组，按名字在拨号时解析，组的选择变了链的入口随之变；成环是 `E0019`；链上某一跳失败时错误文本带 `via <名字>:` 前缀 |
```

换成

```markdown
| `test-url` | HTTP(S) URL；默认全局设置 | ✅ | 2 | M1 解析并校验取值；M3b 生效，不再报 `W0029`（取值顺序见 5.2 节「测试 URL / 超时解析顺序」） |
| `test-timeout` | 秒；默认全局设置 | ✅ | 2 | M1 解析并校验取值；M3b 生效，不再报 `W0029`（取值顺序见 5.2 节「测试 URL / 超时解析顺序」） |
| `test-udp` | `hostname@ipv4` | ✅ | 2 | M1 解析并校验取值，`W0029`；M5 生效（M3 细化设计订正了原来的"M3 生效"） |
| `underlying-proxy` | 另一策略或策略组名；仅代理策略；不能与 `port-hopping` 同用 | ✅ | 2 | 目标代理主机名在上游远程解析；M1 已实现（TCP）：底层可以是策略或组，按名字在拨号时解析，组的选择变了链的入口随之变；成环是 `E0019`；链上某一跳失败时错误文本带 `via <名字>:` 前缀；中继是没有成员的组（订阅还没下载到、过滤滤光了）时，经它的连接一律失败（`via <组名>: policy group has no members`），不回退 DIRECT，也不看 `--empty-group-reject`（阶段 2 / M3b：设中继就是不让流量直出；Surge 未说明） |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `url-test` | 选延迟最低者；HEAD 两次（第二次计分）；用时且过期或网络变化才重测；变更时通知（除非 `no-alert`） | ✅ | 2 | |
| `fallback` | 按声明顺序取第一个可用；全部不可用则用第一个 | ✅ | 2 | |
| `load-balance` | 可用集合内随机；`persistent` 时按目标主机名哈希；从不通知；嵌套时分数取均值 | ✅ | 2 | |
| `smart` | 按真实连接质量动态选择：首响应延迟时间加权均值 + 重传惩罚（约每 1% 丢包 50 ms）× `policy-priority`；接近最优者构成优选集，其余为重试列表；按站点记忆约 1 小时；固定 5 分钟重测，`interval` 无效；>12 成员只测子集；忽略嵌套组与内置策略 | 🟡 | 2 | 算法细节非公开，rurge 按手册描述近似实现 |
| `subnet`（旧名 `ssid`） | 按当前网络选择；条件按声明顺序首个命中；网络变化重算；无命中用 `default` | 🟡 | 3 | `TYPE:CELLULAR` / `MCCMNC:` 永不匹配；阶段 3 之前整组代表它的 `default`（M3a；没写 `default` 时按空组兜底，见下一行）；`category` 等界面参数不再被当成网络条件 |
| 嵌套与循环 | 组可嵌套；循环引用告警且该组临时表现为 REJECT；无可用成员回退 DIRECT | ✅ | 2 | M3a 已实现：循环在加载期报 `W0030`（取代 `E0009`，不再阻止加载），装配后按成员与 `include-other-group` 再检测一次，环上的组解析为 REJECT，会话记录 `policy group cycle: A → B → A`；不在环上、但选到成环成员的组只在那一次 REJECT。没有成员的组（订阅还没下载到、过滤滤光了）回退 DIRECT，会话记录 `policy group has no members; DIRECT substituted`（回退的 DIRECT 本身拨号也失败时，后面再接失败原因）；rurge 专有的 `--empty-group-reject`（环境变量 `RURGE_EMPTY_GROUP_REJECT=true`）改为 REJECT。每构建一次策略表，每个环、每个空组各告警一次 |
| 临时覆盖 | 自动类型组可手动指定成员，期间停止自动测试 | ✅ | 2 | API / CLI 提供 |
```

换成

```markdown
| `url-test` | 选延迟最低者；HEAD 两次（第二次计分）；用时且过期或网络变化才重测；变更时通知（除非 `no-alert`） | ✅ | 2 | M3b 已实现：拨号经过它、且它上一轮测试比 `interval` 旧（或还没测过）时，后台测一轮全部成员（嵌套组的成员一并测），这一次先用现有结果（没有结果时用第一个成员）；只看不拨（API、Dashboard）不触发测试，也不让它换成员；迟滞按 `tolerance`（显式 0 时每次结果变化都换最快的）；写了 `timeout` 时分数不低于它的成员不算可用；没有可用成员时用第一个成员（手册未说明）；没有"网络变化"触发的重测（阶段 3）；变更通知随阶段 6 |
| `fallback` | 按声明顺序取第一个可用；全部不可用则用第一个 | ✅ | 2 | M3b 已实现：按成员顺序取第一个测试通过的；测试的触发同 `url-test` |
| `load-balance` | 可用集合内随机；`persistent` 时按目标主机名哈希；从不通知；嵌套时分数取均值 | ✅ | 2 | M3b 已实现：在测试通过的成员里均匀随机；`persistent=true` 时对目标主机名（IP 目标是 IP 的文本）哈希后取模——哈希函数是 rurge 自己的，同一主机在可用成员不变时固定落在同一成员，可用成员变了可能换；没有可用成员时全部成员都是候选；作为别的自动组的成员时分数取可用成员的均值，成员测过而没有一个可用时算失败；API 与 Dashboard 里显示的当前成员是第一个可用的 |
| `smart` | 按真实连接质量动态选择：首响应延迟时间加权均值 + 重传惩罚（约每 1% 丢包 50 ms）× `policy-priority`；接近最优者构成优选集，其余为重试列表；按站点记忆约 1 小时；固定 5 分钟重测，`interval` 无效；>12 成员只测子集；忽略嵌套组与内置策略 | 🟡 | 2 | 算法细节非公开，rurge 按手册描述近似实现 |
| `subnet`（旧名 `ssid`） | 按当前网络选择；条件按声明顺序首个命中；网络变化重算；无命中用 `default` | 🟡 | 3 | `TYPE:CELLULAR` / `MCCMNC:` 永不匹配；阶段 3 之前整组代表它的 `default`（M3a；没写 `default` 时按空组兜底，见下一行）；`category` 等界面参数不再被当成网络条件 |
| 嵌套与循环 | 组可嵌套；循环引用告警且该组临时表现为 REJECT；无可用成员回退 DIRECT | ✅ | 2 | M3a 已实现：循环在加载期报 `W0030`（取代 `E0009`，不再阻止加载），装配后按成员与 `include-other-group` 再检测一次，环上的组解析为 REJECT，会话记录 `policy group cycle: A → B → A`；不在环上、但选到成环成员的组只在那一次 REJECT。没有成员的组（订阅还没下载到、过滤滤光了）回退 DIRECT，会话记录 `policy group has no members; DIRECT substituted`（回退的 DIRECT 本身拨号也失败时，后面再接失败原因）；rurge 专有的 `--empty-group-reject`（环境变量 `RURGE_EMPTY_GROUP_REJECT=true`）改为 REJECT。每构建一次策略表，每个环、每个空组各告警一次；M3b：嵌套组作为自动组的成员时，`select` 组按它当前的选择计分，`url-test` / `fallback` 按它当前选中的成员计分，`load-balance` 见上面该行，成环的组算失败；被当作中继（策略或组的 `underlying-proxy`）的空组不回退 DIRECT，经它的连接一律失败（见 4.3 节 `underlying-proxy`） |
| 临时覆盖 | 自动类型组可手动指定成员，期间停止自动测试 | ✅ | 2 | M3b 已实现：经 `POST /v1/policy_groups/select` 设置（Surge 未定义自动组上的这个端点，属 rurge 的扩展），`policy` 为空字符串清除；rurge CLI 暂无对应命令；覆盖期间该组不因使用而测试；组定义（不计行位置）不变的重载保留覆盖，组消失或定义变了就清除；进程重启不保留，不写 `state.json`；覆盖的成员从成员表里消失（订阅更新）时覆盖失效并告警一次 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `interval` | url-test / fallback / load-balance（smart 忽略） | 秒；默认 600 | ✅ | 2 |
| `tolerance` | url-test | 毫秒；默认 100 | ✅ | 2 |
| `timeout` | url-test / fallback / load-balance | 秒；无默认；延迟低于此值才算可用 | ✅ | 2 |
| `evaluate-before-use` | 自动类型组 | 布尔；默认 false | ✅ | 2 |
| `persistent` | load-balance | 布尔；默认 false | ✅ | 2 |
```

换成

```markdown
| `interval` | url-test / fallback / load-balance（smart 忽略） | 秒；默认 600；M3b 已实现（按组上一轮测试结束的时间判断过期，只由拨号触发，见 5.1 节 `url-test` 行） | ✅ | 2 |
| `tolerance` | url-test | 毫秒；默认 100；M3b 已实现（显式 0 生效） | ✅ | 2 |
| `timeout` | url-test / fallback / load-balance | 秒；无默认；延迟低于此值才算可用；M3b 已实现 | ✅ | 2 |
| `evaluate-before-use` | 自动类型组 | 布尔；默认 false；M3b 已实现：组第一次被使用时等第一轮测完再选，最多等一轮测试可能用的时间（组里成员最长的测试超时 × 每 8 个一批的批数）；测完没有可用成员时这一次连接以 `policy group evaluation failed` 失败；DNS 会话（`encrypted-dns-follow-outbound-mode`）同样等待；经它作中继的连接不等，用现有结果 | ✅ | 2 |
| `persistent` | load-balance | 布尔；默认 false；M3b 已实现（见 5.1 节 `load-balance` 行） | ✅ | 2 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `underlying-proxy` | 全部（iOS 5.22 / Mac 6.9+） | 策略名；整组链式代理，派生策略名 `Name (via Relay)`。M3a 已实现：组上的中继覆盖成员自己的；组、内置策略与 `direct` / `reject` 别名成员原样保留；经 `include-other-group` 取到的是未派生的成员；派生名已被占用时略去该成员并告警（不绕过中继）；经它绕回本组是 `E0019` | ✅ | 2 |
| `policy-path` | 除 subnet 外 | 文件路径或 URL；内容为策略行列表或含 `[Proxy]` 的完整配置；远程缓存并定期更新。M3a 已实现，差异：只接受 Surge 格式（Clash / base64 解析不出策略时告警）；下载经 rurge 自己的直连，不经代理（未与 Surge 核对）；下载请求的 `User-Agent` 是 `rurge/<版本> (Surge-compatible)`：按 UA 里有没有 "surge" 选格式的机场面板（如 V2Board）因此返回 Surge 格式（未在真实面板上核对）；不认这个 UA 的面板可能返回非 Surge 格式的正文（组按空组兜底并提示"可能不是 Surge 格式"），订阅链接自带的格式参数（如 V2Board 的 `flag=surge`）能避开；值在 `profiles/current` 与 `policies/detail` 里脱敏，日志只写组名、不写 URL；单个订阅最多 10 000 条策略、只读前 100 000 行；跳过的行只逐条报前 20 条，其余合计一条；首次下载不阻塞启动（组先按空组兜底），已有缓存时启动与重载同步载入；订阅更新只重建策略表（没变的成员沿用原出站），不打断无关的连接；坏行、重名、与配置同名的行跳过并告警（只报行号与原因） | 🟡 | 2 |
```

换成

```markdown
| `underlying-proxy` | 全部（iOS 5.22 / Mac 6.9+） | 策略名；整组链式代理，派生策略名 `Name (via Relay)`。M3a 已实现：组上的中继覆盖成员自己的；组、内置策略与 `direct` / `reject` 别名成员原样保留；经 `include-other-group` 取到的是未派生的成员；派生名已被占用时略去该成员并告警（不绕过中继）；经它绕回本组是 `E0019`；M3b 起派生成员经中继测速（测试与拨号走同一条链）；中继是没有成员的组时见 4.3 节 `underlying-proxy` | ✅ | 2 |
| `policy-path` | 除 subnet 外 | 文件路径或 URL；内容为策略行列表或含 `[Proxy]` 的完整配置；远程缓存并定期更新。M3a 已实现，差异：只接受 Surge 格式（Clash / base64 解析不出策略时告警）；下载经 rurge 自己的直连，不经代理（未与 Surge 核对）；下载请求的 `User-Agent` 是 `rurge/<版本> (Surge-compatible)`：按 UA 里有没有 "surge" 选格式的机场面板（如 V2Board）因此返回 Surge 格式（未在真实面板上核对）；不认这个 UA 的面板可能返回非 Surge 格式的正文（组按空组兜底并提示"可能不是 Surge 格式"），订阅链接自带的格式参数（如 V2Board 的 `flag=surge`）能避开；值在 `profiles/current` 与 `policies/detail` 里脱敏，日志只写组名、不写 URL；单个订阅最多 10 000 条策略、只读前 100 000 行；跳过的行只逐条报前 20 条，其余合计一条；首次下载不阻塞启动（组先按空组兜底），已有缓存时启动与重载同步载入；订阅更新只重建策略表（没变的成员沿用原出站），不打断无关的连接；坏行、重名、与配置同名的行跳过并告警（只报行号与原因）；订阅行自己写的 `client-cert=` 与指向主配置策略或组的 `underlying-proxy=` 不生效，该行跳过并 `W0023`（M3b；只有 `external-policy-modifier` 能给导入行设这两项，订阅内部导入行之间的 `underlying-proxy` 照常可用）——Surge 未这样限制，rurge 不让订阅作者动用主配置里的证书与代理 | 🟡 | 2 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| 测试 URL / 超时解析顺序 | 策略自身 `test-url` → 全局 `proxy-test-url` / `internet-test-url`；策略 `test-timeout` → 全局 `test-timeout`（默认 5，直连类 10） | | ✅ | 2 |
```

换成

```markdown
| 测试 URL / 超时解析顺序 | 策略自身 `test-url` → 全局 `proxy-test-url` / `internet-test-url`；策略 `test-timeout` → 全局 `test-timeout`（默认 5，直连类 10） | M3b 已实现：两次 HEAD 在同一条连接上，服务端不保持连接时以第一次的完整耗时（含拨号）计分，并对该 URL 告警一次（日志不写 URL）；HTTPS 测试 URL 在已建立的 TLS 连接上测第二次（手册 Mac 6.10 的行为），证书按系统根证书校验；收到任何状态码的完整响应头都算通过；测试请求的 `User-Agent` 是 `rurge/<版本>`；最多 8 个测试同时进行；每次测试是请求记录里的一条内部会话（规则 `policy test`，目标只有测试 URL 的主机与端口）；结果按策略保存，策略的定义、测试 URL 或超时变了就作废，跨重载保留、不写 `state.json` | ✅ | 2 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `encrypted-dns-follow-outbound-mode`：DNS 连接走规则；`PROTOCOL,DOH/DOH3/DOQ/DOT/DNS` 可匹配；命中的代理若以域名配置则告警并回退 DIRECT | 默认 false | 🟡 | 1 | M3b：TCP/DoT/DoH 上游走流水线（Internal 会话，`PROTOCOL` 可匹配）；上游主机名先由 Bootstrap 解析，流水线只见 IP 目标，域名规则不匹配上游主机名；协议标签按端口启发（853→DoT，443→DoH，其余→DNS）；被 REJECT 时告警并直连保底；UDP 上游不经连接器；这类会话的 `SRC-IP`/`IN-PORT` 为占位值（`127.0.0.1:0`/`0`），`kill` 对其无效；防环回退自阶段 2 / M1b 起实现：DNS 会话命中的代理（沿 underlying-proxy 找到真正打开 socket 的那一跳；底下是 DIRECT——direct 别名策略或当前选中 DIRECT 的组——时取它上面那一跳，因为 DIRECT 要在本机解析的正是那一跳的服务器名）若以域名配置，则告警并直连；以 IP 配置的代理照常承载 DNS 会话 |
```

换成

```markdown
| `encrypted-dns-follow-outbound-mode`：DNS 连接走规则；`PROTOCOL,DOH/DOH3/DOQ/DOT/DNS` 可匹配；命中的代理若以域名配置则告警并回退 DIRECT | 默认 false | 🟡 | 1 | M3b：TCP/DoT/DoH 上游走流水线（Internal 会话，`PROTOCOL` 可匹配）；上游主机名先由 Bootstrap 解析，流水线只见 IP 目标，域名规则不匹配上游主机名；协议标签按端口启发（853→DoT，443→DoH，其余→DNS）；被 REJECT 时告警并直连保底；UDP 上游不经连接器；这类会话的 `SRC-IP`/`IN-PORT` 为占位值（`127.0.0.1:0`/`0`），`kill` 对其无效；防环回退自阶段 2 / M1b 起实现：DNS 会话命中的代理（沿 underlying-proxy 找到真正打开 socket 的那一跳；底下是 DIRECT——direct 别名策略或当前选中 DIRECT 的组——时取它上面那一跳，因为 DIRECT 要在本机解析的正是那一跳的服务器名）若以域名配置，则告警并直连；以 IP 配置的代理照常承载 DNS 会话；阶段 2 / M3b 起，链底下是 REJECT 的代理（中继是当前选中 REJECT 的组等）同样告警并直连，不让 DNS 查询失败 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `POST /v1/policies/test` | `{"policy_names":[...],"url":...}` | 全部 | ✅ | 2 | |
| `GET /v1/policy_groups` | 列出组与选项 | 全部 | 🟡 | 2 | M1 已实现；响应形状手册未给出，暂定结构见 `docs/api/phase2.md`；M3a 起成员是装配后的成员表（含订阅导入与派生成员），订阅更新后随之变化 |
| `GET /v1/policy_groups/test_results` | 自动组测试结果 | 全部 | ✅ | 2 | |
| `GET/POST /v1/policy_groups/select` | 读 / 改 select 组选择 | 全部 | ✅ | 2 | M1 已实现；M3a 起按装配后的成员表校验，选择按名字保存，订阅更新后名字不在了就回落到第一个成员 |
| `POST /v1/policy_groups/test` | 立即测试 → `{"available":[...]}` | 全部 | ✅ | 2 | |
| `GET /v1/requests/recent` `GET /v1/requests/active` `POST /v1/requests/kill` | 请求列表与终止 | 全部 | 🟡 | 1 / 4 | 响应结构手册未定义，以 Surge 实际输出为准做兼容测试；M4a 暂定结构见 `docs/api/phase1.md`；`kill` 命中 rurge 自身的内部会话（如 DNS 查询）→ 409 |
```

换成

```markdown
| `POST /v1/policies/test` | `{"policy_names":[...],"url":...}` | 全部 | 🟡 | 2 | M3b 已实现：省略 `url` 时各自按自己的测试 URL 测，结果保存、自动组据此选择；给了 `url` 时是一次性测试，结果不保存；策略组、REJECT 族与未实现的协议回 `{"error":"not testable"}`；响应形状手册未给出，暂定结构见 `docs/api/phase2.md` |
| `GET /v1/policy_groups` | 列出组与选项 | 全部 | 🟡 | 2 | M1 已实现；响应形状手册未给出，暂定结构见 `docs/api/phase2.md`；M3a 起成员是装配后的成员表（含订阅导入与派生成员），订阅更新后随之变化 |
| `GET /v1/policy_groups/test_results` | 自动组测试结果 | 全部 | 🟡 | 2 | M3b 已实现：列出每个 `url-test` / `fallback` / `load-balance` 组的成员与各自最近的结果（没有结果为 `null`；`smart` 随 M3c）；响应形状手册未给出，暂定结构见 `docs/api/phase2.md` |
| `GET/POST /v1/policy_groups/select` | 读 / 改 select 组选择 | 全部 | ✅ | 2 | M1 已实现；M3a 起按装配后的成员表校验，选择按名字保存，订阅更新后名字不在了就回落到第一个成员；M3b 起对 `url-test` / `fallback` / `load-balance` 组即临时覆盖（见 5.1 节「临时覆盖」），`policy` 为空字符串清除，覆盖不写 `state.json`；`smart`（M3c 之前）与 `subnet` 组仍 400；`GET` 对自动组返回当前生效的成员（覆盖优先，其次是按测试结果选出的） |
| `POST /v1/policy_groups/test` | 立即测试 → `{"available":[...]}` | 全部 | ✅ | 2 | M3b 已实现：立即测该组全部成员（嵌套组的成员一并测），不看 `interval`；任何类型的组都能测 |
| `GET /v1/requests/recent` `GET /v1/requests/active` `POST /v1/requests/kill` | 请求列表与终止 | 全部 | 🟡 | 1 / 4 | 响应结构手册未定义，以 Surge 实际输出为准做兼容测试；M4a 暂定结构见 `docs/api/phase1.md`；`kill` 命中 rurge 自身的内部会话（如 DNS 查询）→ 409；阶段 2 / M3b 起连通性测试也在请求记录里（内部会话，`rule` 为 `policy test`，`policy` 是被测的策略，目标只有测试 URL 的主机与端口） |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| 10. 工具与可观测性 | 50 | 36 | 6 | 4 | 3 | 1 |
| **合计** | **529** | **425** | **57** | **29** | **10** | **8** |
```

换成

```markdown
| 10. 工具与可观测性 | 50 | 34 | 8 | 4 | 3 | 1 |
| **合计** | **529** | **423** | **59** | **29** | **10** | **8** |
```

- [ ] **Step 2: API 参考与手工验收清单**

`docs/api/phase2.md`——把

```markdown
# rurge HTTP API（阶段 2 / M1）

阶段 2 / M1（装配与控制面，见 `docs/superpowers/specs/2026-09-19-phase2-m1-outbound-foundation-design.md` 第 6.6 节）新增四个策略 / 策略组端点。鉴权（`X-Key` 头 / `?x-key=` 查询参数、常量时间比较、封禁）、错误体（`{"error":"<message>"}`）与无内容的成功响应（`{}`）沿用阶段 1 的约定，见 `docs/api/phase1.md`；本页只记录本页新增端点自己的形状。
```

换成

```markdown
# rurge HTTP API（阶段 2）

阶段 2 / M1（装配与控制面，见 `docs/superpowers/specs/2026-09-19-phase2-m1-outbound-foundation-design.md` 第 6.6 节）新增四个策略 / 策略组端点；阶段 2 / M3b（测速与自动组）又新增三个测试端点，并让 `POST /v1/policy_groups/select` 对自动组设临时覆盖（见末节「连通性测试与自动组（M3b）」）。鉴权（`X-Key` 头 / `?x-key=` 查询参数、常量时间比较、封禁）、错误体（`{"error":"<message>"}`）与无内容的成功响应（`{}`）沿用阶段 1 的约定，见 `docs/api/phase1.md`；本页只记录本页新增端点自己的形状。
```

`docs/api/phase2.md`——把

```markdown
| POST | `/v1/policy_groups/select` | `{"group_name":"<name>","policy":"<member>"}` | `{}`；组或成员无效、或不是 `select` 组 → 400 |

**"暂定"标注**：手册没有给出 `/v1/policies/detail` 与 `/v1/policy_groups` 的响应示例（M1 设计 O2）；下面的形状按社区已知的 Surge 响应实现，拿到真实 Surge 实例的样本后再对齐，届时可能是破坏性变更。
```

换成

```markdown
| POST | `/v1/policy_groups/select` | `{"group_name":"<name>","policy":"<member>"}` | `{}`；组或成员无效，或是 `smart` / `subnet` 组 → 400；对自动组即临时覆盖（M3b） |
| POST | `/v1/policies/test` | `{"policy_names":["<name>",…],"url":"<可省略>"}` | `{"<name>": Result…}`；未知策略、`url` 不是 URL → 400（M3b） |
| GET | `/v1/policy_groups/test_results` | | `{"<组名>": {"<成员>": Result 或 null}}`（M3b） |
| POST | `/v1/policy_groups/test` | `{"group_name":"<name>"}` | `{"available":["<成员>",…]}`；未知组 → 400（M3b） |

**"暂定"标注**：手册没有给出 `/v1/policies/detail`、`/v1/policy_groups`（M1 设计 O2）与 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`（M3 设计 6.6）的响应示例；下面的形状按社区已知的 Surge 响应实现，拿到真实 Surge 实例的样本后再对齐，届时可能是破坏性变更。
```

`docs/api/phase2.md`——把

```markdown
参数 `group_name`（必填，查询参数）。响应 `{"policy": "<当前生效的成员>"}`：`select` 组是它当前的选择（没有选择时是第一个成员）；非 `select` 组（`url-test` / `fallback` / `subnet` 等）是它当前解析到的成员，同样按"没有选择用第一个成员"的规则；**一个没有成员的组**（订阅还没下载到，或过滤把成员滤光了）返回 `{"policy": ""}`，这不是错误；`subnet` 组在阶段 3 之前代表它的 `default`（M3a）。
```

换成

```markdown
参数 `group_name`（必填，查询参数）。响应 `{"policy": "<当前生效的成员>"}`：`select` 组是它当前的选择（没有选择时是第一个成员）；自动组（`url-test` / `fallback` / `load-balance`）是当前生效的成员：临时覆盖优先，其次是按测试结果选出的（没有结果时是第一个成员；`load-balance` 显示第一个通过测试的成员，M3b）；`smart`（M3c 之前）是第一个成员；**一个没有成员的组**（订阅还没下载到，或过滤把成员滤光了）返回 `{"policy": ""}`，这不是错误；`subnet` 组在阶段 3 之前代表它的 `default`（M3a）。
```

`docs/api/phase2.md`——把

```markdown
| `group_name` 存在但不是 `select` 组 | `` `Auto` is not a select group `` |
| `policy` 不是该组的成员 | `` `Somewhere` is not a member of `Proxy` `` |

选择立即生效：**下一条**使用该组（或途经该组的链）的连接就会解析到新成员，正在进行中的连接不受影响。选择按 Profile 持久化到 `state.json` 的 `group_selections[<Profile 文件名>][<组名>]`（文件名，不含目录，如 `surge.conf`）；rurge 重启后从 `state.json` 恢复，已消失的旧选择（成员被从配置里删除）按"没有选择"处理，回落到第一个成员。
```

换成

```markdown
| `group_name` 是 `smart`（M3c 之前）或 `subnet` 组 | `` `Smart` does not take a selection `` |
| `policy` 不是该组的成员 | `` `Somewhere` is not a member of `Proxy` `` |

选择立即生效：**下一条**使用该组（或途经该组的链）的连接就会解析到新成员，正在进行中的连接不受影响。选择按 Profile 持久化到 `state.json` 的 `group_selections[<Profile 文件名>][<组名>]`（文件名，不含目录，如 `surge.conf`）；rurge 重启后从 `state.json` 恢复，已消失的旧选择（成员被从配置里删除）按"没有选择"处理，回落到第一个成员。对自动组的同一个请求是临时覆盖，见末节。
```

`docs/api/phase2.md`——把

```markdown
- `POST /v1/profiles/check`：配置本身无错时，连同守护进程数据目录里已缓存的订阅一起装配检查，不联网；没有内容的订阅报 `W0022`，订阅里跳过的行报 `W0023`，订阅的 URL 不出现在输出里。

```

换成

````markdown
- `POST /v1/profiles/check`：配置本身无错时，连同守护进程数据目录里已缓存的订阅一起装配检查，不联网；没有内容的订阅报 `W0022`，订阅里跳过的行报 `W0023`，订阅的 URL 不出现在输出里。

## 连通性测试与自动组（M3b）

自阶段 2 / M3b 起（`docs/superpowers/specs/2026-09-23-phase2-m3-groups-subscriptions-design.md` 第 6 节），`url-test` / `fallback` / `load-balance` 组按连通性测试的结果选成员。下面三个端点里，手册只给了 `POST /v1/policy_groups/test` 的响应示例，另外两个的形状是"暂定"。

### 测试结果（Result）

| 字段 | 类型 | 含义 |
| --- | --- | --- |
| `delay` | 整数 | 通过时：毫秒。同一条连接上第二次 `HEAD` 从发出到收到响应头的时间；服务端不保持连接时是第一次 `HEAD` 从开始拨号起的完整时间 |
| `error` | string | 失败时：原因，如 `connect: <出站的错误>`、`tls: <原因>`、`http: <原因>`、`timed out`；不含测试 URL |
| `time` | 数字 | 这次测试结束的时间，Unix 秒（带小数） |

`delay` 与 `error` 二者有一。

```json
{"delay": 128, "time": 1758790000.25}
```

```json
{"error": "timed out", "time": 1758790005.02}
```

### `POST /v1/policies/test`

请求体 `{"policy_names": ["<名字>", …], "url": "<URL>"}`，`url` 可省略（或为空字符串）。

- 省略 `url`：每个策略按它自己的测试 URL 与超时测（策略的 `test-url` / `test-timeout`，否则 `[General]` 的 `proxy-test-url`——直连类用 `internet-test-url`——与 `test-timeout`）；结果保存，自动组随即按它选择。
- 给了 `url`：全部策略在这个 URL 上各测一次，超时仍用各自的；结果**不保存**，不影响任何组。
- 各策略同时测（全进程最多 8 个测试同时进行，其余排队）；全部测完才返回。

响应 `{"<名字>": Result 或 {"error": "not testable"}}`；不能测的是策略组、`REJECT` 族与尚未实现的协议。

```json
{"HK": {"delay": 128, "time": 1758790000.25}, "Pick": {"error": "not testable"}}
```

失败（均为 400）：某个名字不是任何已知策略、组或内置名 → `` unknown policy `HK-typo` ``；`url` 不是 URL → `` `url` is not a URL ``。

### `GET /v1/policy_groups/test_results`

无参数。响应 `{"<组名>": {"<成员>": Result 或 null}}`，只列 `url-test` / `fallback` / `load-balance` 组（`smart` 随 M3c），成员是装配后的成员表；`null` 表示还没有结果（没测过，或结果对应的定义、测试 URL、超时已经变了），嵌套组与 `REJECT` 族成员恒为 `null`。

```json
{"Auto": {"HK": {"delay": 128, "time": 1758790000.25}, "JP": {"error": "timed out", "time": 1758790005.02}}}
```

### `POST /v1/policy_groups/test`

请求体 `{"group_name": "<name>"}`。立即测该组全部成员（嵌套组的成员一并测），不看 `interval`；任何类型的组都能测。响应是本轮通过的成员（写了 `timeout` 时分数须低于它），按成员顺序：

```json
{"available": ["HK", "JP"]}
```

失败：`group_name` 不是任何已知策略组 → 400 `` unknown policy group `Proxy-typo` ``。

### `POST /v1/policy_groups/select` 对自动组

对 `url-test` / `fallback` / `load-balance` 组，同一个请求体设置**临时覆盖**：该组从下一条连接起固定用这个成员，期间不因使用而测试；`"policy": ""` 清除覆盖，恢复按测试结果选择。覆盖不写 `state.json`，进程重启后不在；组定义（不计行位置）不变的重载保留它，组消失或定义变了就清除；覆盖的成员从成员表里消失（订阅更新）时覆盖失效。

### 测试会话

每次测试是请求记录（`GET /v1/requests/recent`）里的一条内部会话：`listener` 为 `internal`、`rule` 为 `policy test`、`policy` 是被测的策略、目标是测试 URL 的主机与端口（不含路径与参数）；失败时 `error` 是上面 Result 里的原因。它们与 DNS 会话一样不能经 `POST /v1/requests/kill` 终止（409）。

````

`docs/acceptance/phase2-manual.md`——把

```markdown
- [ ] 用一个不带格式参数的通用订阅链接（机场面板可能按 `User-Agent` 选格式，rurge 发的是 `rurge/<版本> (Surge-compatible)`）：组是否照常填充；填不出时记下面板名称与实际返回的内容（`docs/surge-compatibility-matrix.md` 的 `policy-path` 行）。

```

换成

```markdown
- [ ] 用一个不带格式参数的通用订阅链接（机场面板可能按 `User-Agent` 选格式，rurge 发的是 `rurge/<版本> (Surge-compatible)`）：组是否照常填充；填不出时记下面板名称与实际返回的内容（`docs/surge-compatibility-matrix.md` 的 `policy-path` 行）。

## M3b　测速与自动组

需要至少两个延迟明显不同的真实节点（可以来自 M3a 的订阅），自动化测试（只用回环）覆盖不了。

- [ ] `Auto = url-test, <节点 A>, <节点 B>`（不写 `test-url`，即默认的 `http://bing.com/`）：经 `Auto` 浏览几次后，`GET /v1/policy_groups/test_results` 里两个节点都有 `delay`，数值与 Surge（或节点面板）的延迟量级相当；`GET /v1/policy_groups/select?group_name=Auto` 是较快的那个。
- [ ] `GET /v1/requests/recent` 里能看到测试会话（`rule` 为 `policy test`），它们经各自的节点出去（`policy` 是节点名），目标只有 `bing.com:80`，没有路径。
- [ ] 给一个节点写 `test-url=https://www.gstatic.com/generate_204`：测试通过，`delay` 与 HTTP 测试 URL 的同量级（第二次 `HEAD` 复用已建立的 TLS 连接，不含握手）。
- [ ] `Fallback = fallback, <节点 A>, <节点 B>, interval=60`：停掉节点 A（或把它的端口写错后 `rurge reload`），一分钟内经 `Fallback` 的请求改走节点 B；`POST /v1/policy_groups/test {"group_name":"Fallback"}` 立即返回只含 B 的 `available`。
- [ ] `Balance = load-balance, <节点 A>, <节点 B>, persistent=true`：同一个网站的多次请求在请求记录里都经同一个节点，不同网站分散到两个节点。
- [ ] `evaluate-before-use=true`：刚启动后第一次经该组的请求多等一会（测完一轮）再经可用的节点出去；两个节点都不可用时这次请求失败，请求记录的错误是 `policy group evaluation failed`。
- [ ] `POST /v1/policy_groups/select {"group_name":"Auto","policy":"<较慢的节点>"}`：之后的请求经较慢的节点，且不再产生 `Auto` 的测试会话；`{"policy":""}` 清除后恢复自动选择；重启 rurge 后覆盖不在了。
- [ ] 日志（含 `--log-level verbose`）里搜不到任何策略的 `test-url` 的路径与参数（订阅行可能把 token 放在测试 URL 里）。

```

- [ ] **Step 3: M3 设计第 17 节；M3a 计划的延后事项**

`docs/superpowers/specs/2026-09-23-phase2-m3-groups-subscriptions-design.md`——把

```markdown
实施中发现的新出入由各任务追加。

```

换成

```markdown
实施中发现的新出入由各任务追加。

## 17. M3b 实施期的订正

本节登记 M3b 计划的「计划期决定」里与本文件文字不同的地方。逐条对应实现的提交见 `docs/superpowers/plans/2026-09-25-phase2-m3b-testing-auto-groups-plan.md` 末尾「执行期修正记录」。

| 编号 | 设计原文 | 订正 |
| ---- | -------- | ---- |
| P1 | 5.3 / M3-D6：导入行经 `to_spec` 生效（未限制 `client-cert` 与 `underlying-proxy`） | 订阅行自己写的 `client-cert=` 与指向主配置策略或组的 `underlying-proxy=` 不生效，该行跳过并 `W0023`（固定说法，不带取值）；经 `external-policy-modifier` 设上的照常生效，订阅内部导入行之间的中继仍允许。项目所有者 2026-09-25 决定（M3a 延后事项 #11） |
| P2 | 5.6 / M3-D3：没有成员的组回退 DIRECT | 被当作中继（策略的 `underlying-proxy`、组级中继）时一律拒绝，不看 `--empty-group-reject`；作为规则或全局策略直接使用时仍按 M3-D3。项目所有者 2026-09-25 决定（M3a 延后事项 #12） |
| P5 | 6.1：HTTPS 测试 URL 用系统根证书，测试注入 `EngineShared.roots` | 根证书取自构建注册表的出站工厂（新增 `OutboundFactory::roots`），与出站的 TLS 同源，`EngineShared.roots` 因此同样作用于测试 |
| P6 | 6.2："测试会话经观察者 trait 写入请求记录，带 `test` 标记" | 请求记录没有标记字段：测试会话是内部会话，`rule` 为 `policy test`，`policy` 是被测的策略，目标只有测试 URL 的主机与端口。另：`POST /v1/policies/test` 给了 `url` 时是一次性测试，结果不保存（否则会挤掉该策略按自己 URL 测得的结果） |
| P7 | 6.3："组被解析（`resolve`）到、且它的成员结果比组的 `interval` 旧" | 只有拨号（含 DNS 会话与链的中间跳）触发测试；控制面的读取不触发，也不让 `url-test` 换成员。"旧"按组上一轮测试结束的时间判断；一轮测试覆盖嵌套组的成员，并给每个被覆盖的组记一轮 |
| P8 | 6.4："`load-balance` 取它通过者分数的平均值，没有通过者算失败" | 成员都还没测过时算"未知"而不是失败；控制面显示 `load-balance` 组的当前成员时取第一个通过的（随机选出的每次不同） |
| P9 | 6.3 / 9："`evaluate-before-use` 的等待受同一超时约束" | 上限是一轮测试可能用的时间：组里（含嵌套组）成员最长的测试超时 × 每 8 个一批的批数（测试并发上限 8）；DNS 会话同样等待，链的中间跳不等（V9） |
| P13 | 6.6：`GET /v1/policy_groups/test_results` 列出 `url-test` / `fallback` / `load-balance` / `smart` 组；`POST /v1/policy_groups/select` 对非 `select` 组的 400 取消 | M3b 只列前三种（`smart` 随 M3c）；`smart`（M3c 之前）与 `subnet` 组上的 `select` 仍 400，文本改为 `` `G` does not take a selection `` |
| P15、P16 | （无对应文字；M3a 延后事项 #14、#3） | 拨号时运行时与注册表在代际锁下成对读取（`Engine::snapshot`）；DNS 会话命中的代理链底下是 REJECT 时，同 REJECT 一样告警并直连 |
| P17 | 1.4：M3b 行"能力表翻转（`W0008` 只剩 `subnet`）" | M3b 之后 `W0008` 还会因 `smart` 出现，M3c 翻转 `smart` 之后才只剩 `subnet`（与第 8 节一致） |
| P19 | 15：M3b 约 8 个任务 | 11 个：两项承接决定（Task 1、2）与 M3a 延后事项 #3 / #14（Task 9）各成任务 |

```

`docs/superpowers/plans/2026-09-23-phase2-m3a-subscriptions-plan.md`——把

```markdown
| 3 | 链底下的 REJECT 到不了 `dial_internal` 的旁路；链深超过 `MAX_DEPTH` 时 `socket_opener` 给 `None` 而注册表给 REJECT（C4，M1b 起） | M3b（拨号入口会改） |
```

换成

```markdown
| 3 | 链底下的 REJECT 到不了 `dial_internal` 的旁路；链深超过 `MAX_DEPTH` 时 `socket_opener` 给 `None` 而注册表给 REJECT（C4，M1b 起） | 已处理（M3b 计划 Task 9） |
```

`docs/superpowers/plans/2026-09-23-phase2-m3a-subscriptions-plan.md`——把

```markdown
| 11 | 订阅行能按名字用到主配置的私有材料（`client-cert=<Keystore 条目>`、`underlying-proxy=<主配置策略>`），即订阅作者能让 rurge 向他指定的主机出示用户的客户端证书、或经用户自己的代理连过去；M3b 的自动测速之后无需用户选中该节点也会发生 | 等项目所有者决定（M3b 之前） |
| 12 | 空组被当作中继（策略的 `underlying-proxy` 或组级中继指向一个还没有内容的订阅组）时解析到 DIRECT 兜底，依赖它的策略直连自己的服务器、绕过了使用者设的中继（M3-D3 的直接结果，与 P11 的理由相悖；`--empty-group-reject` 可全局改为拒绝） | 等项目所有者决定 |
| 13 | 导入策略 X 在注册表构建时失败被略去，中继指向 X 的另一条导入策略仍留在成员表里，每次拨号失败（`via X: the policy no longer exists`） | 有用户报告再说；最迟 M8 |
| 14 | 一个会话先取当前代、后取注册表，而重载先发布注册表、后换代：恰好跨过重载的会话可能用旧一代的规则选名、到新一代的注册表里解析，重载删掉或改名的策略会让这一条连接 REJECT 并打一条 ERROR | M3b（拨号入口会改，与 C4 一起） |
```

换成

```markdown
| 11 | 订阅行能按名字用到主配置的私有材料（`client-cert=<Keystore 条目>`、`underlying-proxy=<主配置策略>`），即订阅作者能让 rurge 向他指定的主机出示用户的客户端证书、或经用户自己的代理连过去；M3b 的自动测速之后无需用户选中该节点也会发生 | 已处理：项目所有者 2026-09-25 决定只认 `external-policy-modifier` 设上的（M3b 计划 Task 1） |
| 12 | 空组被当作中继（策略的 `underlying-proxy` 或组级中继指向一个还没有内容的订阅组）时解析到 DIRECT 兜底，依赖它的策略直连自己的服务器、绕过了使用者设的中继（M3-D3 的直接结果，与 P11 的理由相悖；`--empty-group-reject` 可全局改为拒绝） | 已处理：项目所有者 2026-09-25 决定中继时一律拒绝（M3b 计划 Task 2） |
| 13 | 导入策略 X 在注册表构建时失败被略去，中继指向 X 的另一条导入策略仍留在成员表里，每次拨号失败（`via X: the policy no longer exists`） | 有用户报告再说；最迟 M8 |
| 14 | 一个会话先取当前代、后取注册表，而重载先发布注册表、后换代：恰好跨过重载的会话可能用旧一代的规则选名、到新一代的注册表里解析，重载删掉或改名的策略会让这一条连接 REJECT 并打一条 ERROR | 已处理（M3b 计划 Task 9） |
```

- [ ] **Step 4: README 与 CLAUDE.md**

`README.md`——把

```markdown
> **阶段 1 功能齐备：M1 ～ M4b**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）进行中：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；其余出站协议与 `url-test` / `fallback` / `load-balance` / `smart` / `subnet` 等策略组算法仍在阶段 2 后续里程碑。
```

换成

```markdown
> **阶段 1 功能齐备：M1 ～ M4b**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）进行中：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；M3b（测速与自动组）已完成——`url-test` / `fallback` / `load-balance` 组按连通性测试（经各节点在同一条连接上两次 `HEAD`，HTTPS 测试 URL 在已建立的 TLS 连接上测第二次）自动选择，`interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` 与策略的 `test-url` / `test-timeout` 生效，自动组可经 `POST /v1/policy_groups/select` 临时覆盖，新增 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 三个端点；其余出站协议与 `smart` / `subnet` 策略组仍在阶段 2 后续里程碑。
```

`README.md`——把

```markdown
| 策略组         | select / url-test / fallback / load-balance / smart / subnet、策略引入与订阅、延迟测试（`select` 组的选择经 API 读取与切换已实现，阶段 2 / M1；策略引入与订阅、组级 `underlying-proxy` 已实现，阶段 2 / M3a）                                                                        | 2     |
```

换成

```markdown
| 策略组         | select / url-test / fallback / load-balance / smart / subnet、策略引入与订阅、延迟测试（`select` 组的选择经 API 读取与切换已实现，阶段 2 / M1；策略引入与订阅、组级 `underlying-proxy` 已实现，阶段 2 / M3a；url-test / fallback / load-balance、延迟测试与临时覆盖已实现，阶段 2 / M3b）                                                                        | 2     |
```

`README.md`——把

```markdown
3. **阶段 2** 出站协议全集、策略组、策略订阅（进行中：M1 出站地基与 HTTP / SOCKS5 上游已完成；M2a（Trojan，TCP + WebSocket）已完成；M2b（VMess / AnyTLS）已完成；M2c（Shadow TLS）已完成；M3a（成员装配与订阅）已完成）
```

换成

```markdown
3. **阶段 2** 出站协议全集、策略组、策略订阅（进行中：M1 出站地基与 HTTP / SOCKS5 上游已完成；M2a（Trojan，TCP + WebSocket）已完成；M2b（VMess / AnyTLS）已完成；M2c（Shadow TLS）已完成；M3a（成员装配与订阅）已完成；M3b（测速与自动组）已完成）
```

`README.md`——把

```markdown
> `rurge check`、`rurge rule match`、`rurge dns lookup` 与 `rurge run`（HTTP / SOCKS5 代理，DIRECT / REJECT，以及 `http` / `https` / `socks5` / `socks5-tls` / `trojan` / `vmess` / `anytls` 上游——均可叠加 Shadow TLS，含 `underlying-proxy` 链）已可用；策略组算法在阶段 2 后续里程碑。HTTP API 与 `rurge reload` / `stop` / `status` 已可用（见 [docs/api/phase1.md](docs/api/phase1.md)，阶段 2 新增端点见 [docs/api/phase2.md](docs/api/phase2.md)）。`rurge run --system-proxy` 可以把系统代理指向 rurge，退出时恢复、崩溃后在下次启动时恢复；`rurge service install | uninstall [--user] [--dry-run]` 可以注册 / 移除开机自启（systemd / launchd / Windows 计划任务）。macOS 经 `networksetup` 设置，通常需要管理员账户；是否需要 `sudo` 尚未在真机验证（见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)），失败时 rurge 原样报出工具的错误。`rurge run` 另支持 `--idle-timeout`、`--request-log-size`、`--watch`（配置热重载）、`--log-file`（按天滚动）、`--empty-group-reject`（没有成员的策略组拒绝而不是直连）等 rurge 专有运行时选项，只经命令行参数 / 环境变量提供，不写入 Surge 配置文件。
```

换成

```markdown
> `rurge check`、`rurge rule match`、`rurge dns lookup` 与 `rurge run`（HTTP / SOCKS5 代理，DIRECT / REJECT，以及 `http` / `https` / `socks5` / `socks5-tls` / `trojan` / `vmess` / `anytls` 上游——均可叠加 Shadow TLS，含 `underlying-proxy` 链）已可用；`select` / `url-test` / `fallback` / `load-balance` 组已可用，`smart` 与 `subnet` 在后续里程碑。HTTP API 与 `rurge reload` / `stop` / `status` 已可用（见 [docs/api/phase1.md](docs/api/phase1.md)，阶段 2 新增端点见 [docs/api/phase2.md](docs/api/phase2.md)）。`rurge run --system-proxy` 可以把系统代理指向 rurge，退出时恢复、崩溃后在下次启动时恢复；`rurge service install | uninstall [--user] [--dry-run]` 可以注册 / 移除开机自启（systemd / launchd / Windows 计划任务）。macOS 经 `networksetup` 设置，通常需要管理员账户；是否需要 `sudo` 尚未在真机验证（见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)），失败时 rurge 原样报出工具的错误。`rurge run` 另支持 `--idle-timeout`、`--request-log-size`、`--watch`（配置热重载）、`--log-file`（按天滚动）、`--empty-group-reject`（没有成员的策略组拒绝而不是直连）等 rurge 专有运行时选项，只经命令行参数 / 环境变量提供，不写入 Surge 配置文件。
```

`README_en.md`——把

```markdown
> **Phase 1 feature-complete: M1 through M4b** (profile parsing, rule engine, rule sets, GeoIP, external resource management, DNS client, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`; M3b added the request log and traffic stats, SNI recording, idle timeout, REJECT auto-escalation, a 502 for failed CONNECT dials, graceful shutdown, hot reload (SIGHUP / `--watch`), `--log-file`, and `encrypted-dns-follow-outbound-mode`; M4a added a Surge-compatible HTTP API (phase-1 endpoints, `X-Key` auth with banning), outbound mode / global policy persisted to `state.json`, and `rurge reload` / `stop` / `status`; M4b added the system proxy (Windows registry + a WinINet notification, macOS `networksetup`, Linux GNOME / KDE, `--system-proxy` and `POST /v1/features/system_proxy`, automatic recovery on exit or crash, and following listener changes across a reload) and `rurge service install / uninstall [--dry-run]` (systemd / launchd / a Windows scheduled task)) — the phase-1 acceptance criteria that need a real desktop environment (the three-platform system proxy, service install) have not been through manual acceptance yet; see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md) for the checklist. The dashboard is phase 6. Phase 2 (outbound protocols and policy groups) is in progress: M1 (outbound foundation and HTTP / SOCKS5 upstreams) is done — `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections by rule through DIRECT / REJECT or `http` / `https` / `socks5` / `socks5-tls` upstreams (TCP, including two-or-more-hop `underlying-proxy` chains), and a `select` group's choice can be read and switched over the HTTP API (see [docs/api/phase2.md](docs/api/phase2.md)); M2a (the TLS family, Trojan first) is done — the `trojan` policy (TCP, TLS always on, optionally over WebSocket) is usable; M2b (VMess / AnyTLS) is done — the `vmess` (AEAD handshake, TCP, optionally over TLS / WebSocket) and `anytls` (TCP, session reuse) policies are usable, and a reload reuses outbounds by fingerprint without disturbing connection pools still in use; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); M3a (member assembly and subscriptions) is done — `policy-path` subscriptions (a local file or a URL, in Surge format; an update rebuilds only the policy table, without disturbing unrelated connections), `include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`, the group-level `underlying-proxy` (derived `Name (via Relay)` members), group cycle warnings (`W0030`) and the empty-group fallback (DIRECT by default, REJECT with `--empty-group-reject`) are usable; the remaining outbound protocols and the group algorithms (`url-test` / `fallback` / `load-balance` / `smart` / `subnet`) are later phase-2 milestones.
```

换成

```markdown
> **Phase 1 feature-complete: M1 through M4b** (profile parsing, rule engine, rule sets, GeoIP, external resource management, DNS client, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`; M3b added the request log and traffic stats, SNI recording, idle timeout, REJECT auto-escalation, a 502 for failed CONNECT dials, graceful shutdown, hot reload (SIGHUP / `--watch`), `--log-file`, and `encrypted-dns-follow-outbound-mode`; M4a added a Surge-compatible HTTP API (phase-1 endpoints, `X-Key` auth with banning), outbound mode / global policy persisted to `state.json`, and `rurge reload` / `stop` / `status`; M4b added the system proxy (Windows registry + a WinINet notification, macOS `networksetup`, Linux GNOME / KDE, `--system-proxy` and `POST /v1/features/system_proxy`, automatic recovery on exit or crash, and following listener changes across a reload) and `rurge service install / uninstall [--dry-run]` (systemd / launchd / a Windows scheduled task)) — the phase-1 acceptance criteria that need a real desktop environment (the three-platform system proxy, service install) have not been through manual acceptance yet; see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md) for the checklist. The dashboard is phase 6. Phase 2 (outbound protocols and policy groups) is in progress: M1 (outbound foundation and HTTP / SOCKS5 upstreams) is done — `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections by rule through DIRECT / REJECT or `http` / `https` / `socks5` / `socks5-tls` upstreams (TCP, including two-or-more-hop `underlying-proxy` chains), and a `select` group's choice can be read and switched over the HTTP API (see [docs/api/phase2.md](docs/api/phase2.md)); M2a (the TLS family, Trojan first) is done — the `trojan` policy (TCP, TLS always on, optionally over WebSocket) is usable; M2b (VMess / AnyTLS) is done — the `vmess` (AEAD handshake, TCP, optionally over TLS / WebSocket) and `anytls` (TCP, session reuse) policies are usable, and a reload reuses outbounds by fingerprint without disturbing connection pools still in use; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); M3a (member assembly and subscriptions) is done — `policy-path` subscriptions (a local file or a URL, in Surge format; an update rebuilds only the policy table, without disturbing unrelated connections), `include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`, the group-level `underlying-proxy` (derived `Name (via Relay)` members), group cycle warnings (`W0030`) and the empty-group fallback (DIRECT by default, REJECT with `--empty-group-reject`) are usable; M3b (connectivity tests and automatic groups) is done — `url-test` / `fallback` / `load-balance` groups choose by connectivity tests (two `HEAD`s over one connection through each member, the second one on the established TLS connection for an HTTPS test URL), `interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` and the policies' `test-url` / `test-timeout` take effect, an automatic group takes a temporary override through `POST /v1/policy_groups/select`, and `POST /v1/policies/test`, `GET /v1/policy_groups/test_results` and `POST /v1/policy_groups/test` are available; the remaining outbound protocols and the `smart` / `subnet` groups are later phase-2 milestones.
```

`README_en.md`——把

```markdown
| Policy groups       | select / url-test / fallback / load-balance / smart / subnet, policy including and subscriptions, latency tests (a `select` group's choice can be read and switched over the API, phase 2 / M1; policy including, subscriptions and the group-level `underlying-proxy` implemented, phase 2 / M3a)                                                         | 2     |
```

换成

```markdown
| Policy groups       | select / url-test / fallback / load-balance / smart / subnet, policy including and subscriptions, latency tests (a `select` group's choice can be read and switched over the API, phase 2 / M1; policy including, subscriptions and the group-level `underlying-proxy` implemented, phase 2 / M3a; url-test / fallback / load-balance, latency tests and temporary overrides implemented, phase 2 / M3b)                                                         | 2     |
```

`README_en.md`——把

```markdown
3. **Phase 2** All outbound protocols, policy groups, subscriptions (in progress: M1, the outbound foundation and HTTP / SOCKS5 upstreams, is done; M2a, Trojan (TCP + WebSocket), is done; M2b, VMess / AnyTLS, is done; M2c, Shadow TLS, is done; M3a, member assembly and subscriptions, is done)
```

换成

```markdown
3. **Phase 2** All outbound protocols, policy groups, subscriptions (in progress: M1, the outbound foundation and HTTP / SOCKS5 upstreams, is done; M2a, Trojan (TCP + WebSocket), is done; M2b, VMess / AnyTLS, is done; M2c, Shadow TLS, is done; M3a, member assembly and subscriptions, is done; M3b, connectivity tests and automatic groups, is done)
```

`README_en.md`——把

```markdown
> `rurge check`, `rurge rule match`, `rurge dns lookup` and `rurge run` (HTTP / SOCKS5 proxy, DIRECT / REJECT, and `http` / `https` / `socks5` / `socks5-tls` / `trojan` / `vmess` / `anytls` upstreams — all optionally wrapped in Shadow TLS — including `underlying-proxy` chains) work today; the group algorithms arrive in later phase-2 milestones. The HTTP API and `rurge reload` / `stop` / `status` are available (see [docs/api/phase1.md](docs/api/phase1.md); phase-2 additions in [docs/api/phase2.md](docs/api/phase2.md)). `rurge run --system-proxy` points the system proxy at rurge and restores it on exit, or at the next start after a crash; `rurge service install | uninstall [--user] [--dry-run]` registers or removes automatic startup (systemd / launchd / a Windows scheduled task). On macOS, `networksetup` usually needs an administrator account; whether `sudo` is required has not been verified on real hardware yet (see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)) — on failure, rurge passes the tool's error through as-is. `rurge run` also takes rurge-specific runtime options — `--idle-timeout`, `--request-log-size`, `--watch` (hot reload), `--log-file` (daily rotation), `--empty-group-reject` (a policy group without members rejects instead of going direct) — as CLI flags / env vars only, never written into the Surge profile.
```

换成

```markdown
> `rurge check`, `rurge rule match`, `rurge dns lookup` and `rurge run` (HTTP / SOCKS5 proxy, DIRECT / REJECT, and `http` / `https` / `socks5` / `socks5-tls` / `trojan` / `vmess` / `anytls` upstreams — all optionally wrapped in Shadow TLS — including `underlying-proxy` chains) work today; `select` / `url-test` / `fallback` / `load-balance` groups work, `smart` and `subnet` arrive in later milestones. The HTTP API and `rurge reload` / `stop` / `status` are available (see [docs/api/phase1.md](docs/api/phase1.md); phase-2 additions in [docs/api/phase2.md](docs/api/phase2.md)). `rurge run --system-proxy` points the system proxy at rurge and restores it on exit, or at the next start after a crash; `rurge service install | uninstall [--user] [--dry-run]` registers or removes automatic startup (systemd / launchd / a Windows scheduled task). On macOS, `networksetup` usually needs an administrator account; whether `sudo` is required has not been verified on real hardware yet (see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)) — on failure, rurge passes the tool's error through as-is. `rurge run` also takes rurge-specific runtime options — `--idle-timeout`, `--request-log-size`, `--watch` (hot reload), `--log-file` (daily rotation), `--empty-group-reject` (a policy group without members rejects instead of going direct) — as CLI flags / env vars only, never written into the Surge profile.
```

`CLAUDE.md`——把

```markdown
阶段 1 功能已齐备（M1 ～ M4b）；阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`。M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。M3b（测速与自动组）尚未开始。
```

换成

```markdown
阶段 1 功能已齐备（M1 ～ M4b）；阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`。M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。M3b（测速与自动组）已完成——`[General] test-timeout` 区分未设置（`General::test_target`；策略的 `test-url` / `test-timeout` 生效，`W0029` 对它们退役）；`rurge-policy` 的 `probe`（经策略自己的出站在一条连接上两次 HEAD，HTTPS 在已建立的 TLS 连接上测第二次，根证书取 `OutboundFactory::roots`）、`testbook`（`TestBook`：结果按策略与定义保存、同一策略同时只测一次、最多 8 个并发、`TestObserver`、一次性的 `test_once`）与 `auto`（`url-test` / `fallback` / `load-balance` 三种选法、`SelectCtx`、`AutoGroups`：临时覆盖、`url-test` 保持的成员、每组上一轮测试的时间与测试请求）；注册表按测试结果选成员（只有拨号触发测试、`evaluate-before-use` 的 `Resolution.pending`、嵌套组的分数、`round_timeout`）、`resolve_relay`（空组当中继一律拒绝）、订阅行不能动用主配置的 `client-cert` 与策略（`W0023`）；`rurge-engine` 的测试调度任务、测试会话进请求记录（规则 `policy test`）、`evaluate-before-use` 的有界等待（`policy group evaluation failed`）、重载时保留未变组的覆盖、`Engine::snapshot`（运行时与注册表在代际锁下成对读取）与 DNS 会话在链底为 REJECT 时的旁路；`rurge-api` 的 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 与 `POST /v1/policy_groups/select` 对自动组的临时覆盖；能力表翻转 `url-test` / `fallback` / `load-balance`（`W0008` 只剩 `smart` 与 `subnet`）。M3c（`smart`）尚未开始。
```

`CLAUDE.md`——把

```markdown
- `docs/superpowers/specs/2026-09-23-phase2-m3-groups-subscriptions-design.md`：阶段 2 / M3 设计文档（策略组、订阅与连通性测试）。三份计划的拆分（M3a 成员装配与订阅 → M3b 测速与 `url-test` / `fallback` / `load-balance` → M3c `smart`）、`GroupSpec`、订阅解析与装配（顺序、过滤 / 前缀 / 修饰、全局重名）、组级 `underlying-proxy` 派生、运行期环（`W0030` 取代 `E0009`）与空组兜底（`--empty-group-reject`）、订阅更新只重建注册表、两次 HEAD 的探针与 `TestBook`、三种算法与临时覆盖、`smart` 的打分 / 站点记忆 / 拨号重试与常数、三个测试 API；已决事项 M3-D1 ～ D13，第 14 节列出写各份计划时必须核对的事项。
- `docs/superpowers/plans/2026-09-23-phase2-m3a-subscriptions-plan.md`：阶段 2 / M3a（成员装配与订阅）实施计划（10 个任务）。开头「计划期决定」表（P1–P22）记录核对源码得出的结论与和设计文字不同的决定（资源管理器本来就同步载入缓存、订阅 URL 改用标签记日志、代际锁只包住发布、`subnet` 组暂时代表它的 `default`、`external-policy-modifier` 也脱敏、环境变量取 `true` 等）与「承接事项」；末尾「执行期修正记录」与「延后事项」两张表。
- `docs/acceptance/phase2-manual.md`：阶段 2 手工验收清单（M2a 起新建），需要真实公网节点的项目，自动化测试（只用回环）覆盖不了，由项目所有者用自己的节点验收。
- `docs/api/phase1.md`：阶段 1 HTTP API 参考——端点、JSON 形状、鉴权与封禁、系统代理的地址 / `skip-proxy` 转换 / 生命周期、`rurge reload/stop/status` 客户端。
- `docs/api/phase2.md`：阶段 2 HTTP API 参考——M1 新增的四个策略 / 策略组端点（`policies/detail`、`policy_groups`、`policy_groups/select`）的响应形状、`lineHash` 的定义、选择的生效时机与持久化位置。
```

换成

```markdown
- `docs/superpowers/specs/2026-09-23-phase2-m3-groups-subscriptions-design.md`：阶段 2 / M3 设计文档（策略组、订阅与连通性测试）。三份计划的拆分（M3a 成员装配与订阅 → M3b 测速与 `url-test` / `fallback` / `load-balance` → M3c `smart`）、`GroupSpec`、订阅解析与装配（顺序、过滤 / 前缀 / 修饰、全局重名）、组级 `underlying-proxy` 派生、运行期环（`W0030` 取代 `E0009`）与空组兜底（`--empty-group-reject`）、订阅更新只重建注册表、两次 HEAD 的探针与 `TestBook`、三种算法与临时覆盖、`smart` 的打分 / 站点记忆 / 拨号重试与常数、三个测试 API；已决事项 M3-D1 ～ D13，第 14 节列出写各份计划时必须核对的事项，第 16、17 节是 M3a、M3b 实施期的订正。
- `docs/superpowers/plans/2026-09-23-phase2-m3a-subscriptions-plan.md`：阶段 2 / M3a（成员装配与订阅）实施计划（10 个任务）。开头「计划期决定」表（P1–P22）记录核对源码得出的结论与和设计文字不同的决定（资源管理器本来就同步载入缓存、订阅 URL 改用标签记日志、代际锁只包住发布、`subnet` 组暂时代表它的 `default`、`external-policy-modifier` 也脱敏、环境变量取 `true` 等）与「承接事项」；末尾「执行期修正记录」与「延后事项」两张表。
- `docs/superpowers/plans/2026-09-25-phase2-m3b-testing-auto-groups-plan.md`：阶段 2 / M3b（测速与自动组）实施计划（11 个任务）。开头「计划期决定」表（P1–P21）记录核对源码与手册得出的结论和与设计文字不同的决定（订阅行不能动用主配置的证书与策略、空组当中继一律拒绝、只有拨号触发测试、测试根证书取自出站工厂、`evaluate-before-use` 的等待上限、API 的响应形状等）与「承接事项」（两项用户决定、M3a 延后事项 #3 / #14）；末尾「执行期修正记录」与「延后事项」两张表。
- `docs/acceptance/phase2-manual.md`：阶段 2 手工验收清单（M2a 起新建），需要真实公网节点的项目，自动化测试（只用回环）覆盖不了，由项目所有者用自己的节点验收。
- `docs/api/phase1.md`：阶段 1 HTTP API 参考——端点、JSON 形状、鉴权与封禁、系统代理的地址 / `skip-proxy` 转换 / 生命周期、`rurge reload/stop/status` 客户端。
- `docs/api/phase2.md`：阶段 2 HTTP API 参考——M1 新增的四个策略 / 策略组端点（`policies/detail`、`policy_groups`、`policy_groups/select`）的响应形状、`lineHash` 的定义、选择的生效时机与持久化位置；M3b 新增的三个测试端点、测试结果的形状、`select` 对自动组的临时覆盖与测试会话。
```

`CLAUDE.md`——把

```markdown
cargo test -p rurge-engine --test subscriptions   # 订阅：构建时同步载入、热重建与出站复用、经导入 / 派生成员出站、空组兜底
```

换成

```markdown
cargo test -p rurge-engine --test subscriptions   # 订阅：构建时同步载入、热重建与出站复用、经导入 / 派生成员出站、空组兜底
cargo test -p rurge-engine --test auto_groups     # 自动组：拨号触发的测试轮次、三种选法、evaluate-before-use、测试会话、临时覆盖与重载
cargo test -p rurge-policy probe                # 探针：两次 HEAD、HTTPS、不保持连接时的退化（回环 TestServer）
```

- [ ] **Step 5: 本计划末尾的两张表**

把执行期间与本计划不同的地方逐条写进「执行期修正记录」（哪个任务、计划原文、实际做法、原因、提交），把没做完或新发现、留给以后的事写进「延后事项」（去向写清楚：M3c / M8 / 阶段 3 / 有用户报告再说）。计划期已知的几条已经在表里。

- [ ] **Step 6: 门禁与提交**

跑门禁（文档不影响用例：40 个测试二进制，923 通过 / 1 忽略）。

```bash
git add docs README.md README_en.md CLAUDE.md
git commit -m "docs: M3b 测速与自动组——兼容性清单、API 参考、手工验收、设计第 17 节、README 与 CLAUDE.md"
```


## 验收对照（设计第 11 节中属于 M3b 的条目）

| 设计第 11 节 | 覆盖 |
| ------------ | ---- |
| 4. 派生成员"可测"（M3a 留下的一半） | Task 7：`a_derived_member_is_tested_through_its_relay`（测试与拨号都经中继） |
| 5. 三种自动组的算法（`select` 已有，`smart` 属 M3c）；临时覆盖与清除；`select` 的选择重启后保留（回归） | Task 6：`auto` 的五条单元用例与注册表的七条；Task 7：`url_test_moves_to_the_quicker_member_after_a_round`、`fallback_passes_over_a_member_that_fails_its_test`、`load_balance_with_persistent_keeps_each_host_on_one_member`、`evaluate_before_use_waits_for_the_first_round`、`a_reload_keeps_an_override_while_the_group_stays_the_same`；Task 8：`select_on_an_automatic_group_overrides_it_until_cleared`（引擎与 API 各一条）；回归：M1 的 `a_selection_applies_to_the_next_connection_and_survives_a_restart` 与 API 的 `a_select_group_is_switched_for_the_next_request_and_persisted` 不改 |
| 6. 三个测试 API 与 `select` 扩展 | Task 8：`policies_are_tested_on_request`、`a_group_is_tested_on_request`、`select_on_an_automatic_group_overrides_it_until_cleared` |
| 7. 凭据不外泄（测试 URL） | Task 4：`a_failure_says_which_step_failed`（失败原因只说步骤）；Task 5：告警只带策略名；Task 7：`every_test_is_a_session_of_the_request_log`（目标只有主机与端口）；手工验收清单的最后一条 |
| 8. fmt / clippy 零警告 / `cargo test --workspace` 全绿；`W0008` 只因 `subnet` 出现 | 每个任务的门禁；`W0008` 在 M3b 之后还会因 `smart` 出现（P17），Task 10：`check_knows_the_automatic_groups` |
| 9. 需要真实节点的项目进手工验收 | Task 11：`docs/acceptance/phase2-manual.md` 的「M3b　测速与自动组」 |

第 1 ～ 3 条属于 M3a（已完成）；第 5 条里的 `smart` 属于 M3c。

## 执行期修正记录

| 任务 | 计划原文 | 实际做法 | 原因 | 提交 |
| ---- | -------- | -------- | ---- | ---- |
| 全部任务（开工时） | （无对应文字） | 写计划用的副本与核对工作树曾把产物编进本仓库的 `target/`：清掉工作区自身 crate 的产物（`cargo clean -p`，依赖不动）之后，由控制者在干净构建上重跑 Task 1 的门禁核对 | cargo 的 dep-info 按包根相对路径记源文件，本任务没改动的 crate（如 `rurge-config`）复用了副本里更靠后的状态编出的测试二进制，Task 1 的门禁里跑出了 Task 3 的用例 | — |
| 1 | P1：检查在 `Imports::collect` 里、加前缀与修饰之前做（看订阅作者写的原行），修饰设了同名参数就豁免 | 检查挪到套用修饰、`parse_policy` 之后，看真正用来建策略的那一行的每个取值：修饰设了该键时，行上只能是修饰的值，否则跳过（新原因 `` `external-policy-modifier` cannot set `<key>` on this line ``）；修饰没设时仍是原来的两条原因；新增用例 `a_modified_line_may_not_keep_its_own_value_by_its_spelling` | 修饰设了同名参数时，订阅行把整项加引号（`"underlying-proxy=Corp"`）就能绕过：`with_params` 按原文取键认不出它，修饰的值追加在后，解析器去掉引号后先读到订阅行自己的值；未闭合的引号还会吞掉追加的修饰，让该行直连 | b41574f |
| 1 | 新用例放在 `group_cycles_through_members_are_listed` 之前 | 放在它之后 | 只是位置，内容逐字相同 | 990f627 |
| 5 | 「要点」：`run` 先存结果、再从 `running` 表里摘掉自己（只摘 `key` 相同的那一项） | 只有 `running` 里登记的仍是本测试自己的通道（`same_channel`）时，才在持有 `running` 的同时存结果、摘登记；被取代的测试只把结果交给自己的等待者；新增用例 `a_superseded_test_leaves_the_current_result_alone` | 旧定义的测试若在新定义的测试之后才结束，会用过期结果覆盖当前结果，当前定义读作"未知"，直到组的 `interval` 过去都不会重测；只按 key 摘登记还会让 k1→k2→k1 摘掉别人的登记 | af5a4e5 |
| 6 | P7："旧"按组上一轮测试结束的时间判断 | 拨号时，有直接成员（嵌套组除外）对当前定义没有结果也请求一轮；`a_dial_asks_for_a_round_and_the_views_do_not` 在标记一轮完成之前先给成员写入结果，新增用例 `a_member_without_a_result_asks_for_a_round` | 设计 6.3 写的是"成员结果比 interval 旧（或没有结果）"；只看组的时间时，重载改了成员的测试参数会让全部结果作废而组仍算刚测过，最长 `interval` 内都按"未知"选成员。嵌套组除外，免得一个空的成员组让每次拨号都请求一轮 | 60e334b |
| 7 | P9 与「要点」："DNS 会话同样等待" | `dial_internal`（DNS 会话）不等，直接 `resolve_with`（仍会请求一轮）；会话拨号照旧等待；新增用例 `a_dns_session_does_not_wait_for_an_evaluate_before_use_group` | 开 `encrypted-dns-follow-outbound-mode` 时，这一轮的探针解析以主机名配置的成员又要经 DNS 会话，两者互相卡住直到查询超时：每次启动该组首轮全部失败，并一直维持到下一个 `interval` | 5e9eb11 |
| 1 – 10 | 各任务门禁的预期总数（Task 1 起 881、884、885、890、896、908、916、921、922、923） | 实际依次是 882、885、886、891、898、911、920、925、926、927（各任务的修正做完之后）：Task 1、5、6、7 的修正各多一条用例；最后一次门禁 40 个测试二进制，927 通过 / 0 失败 / 1 忽略 | 见上面几行 | — |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | 测试会话不计字节：探针的流不经 `Counting`，请求记录与 `GET /v1/traffic` 里测试会话的上下行都是 0 | 有用户报告再说 |
| 2 | `load-balance` + `persistent=true` 的哈希用 `std::hash::DefaultHasher`：同一版本的 rurge 里稳定，但 Rust 不保证它的算法跨版本不变，升级 rurge 可能让主机换到别的成员 | 有用户报告再说 |
| 3 | `evaluate-before-use` 的等待上限按"本组每 8 个一批"计算，没有算上别的组同时占用的测试许可（全进程共 8 个）：很多组同时第一次被使用时，等待可能在这一轮测完之前到期，会话按当时的结果走（或以 `policy group evaluation failed` 失败） | 有用户报告再说 |
| 4 | 链的中间跳是 `load-balance` 组时没有目标主机（`SelectCtx` 只在会话入口有），`persistent` 退化为随机；DNS 防环的 `socket_opener` 与真正的拨号各自解析，中间跳是 `load-balance` 时两者可能选中不同的成员 | 有用户报告再说 |
| 5 | "网络已变化"没有来源：`TestBook::invalidate_all` 只是入口 | 阶段 3（网卡 / 路由监视） |
| 6 | `smart` 组：仍 `W0008`、取第一个成员、不在 `test_results` 里、`select` 不接受；请求记录的两列耗时（承接 C3 的另一半） | M3c |
| 7 | 测试二进制偶发 `STATUS_HEAP_CORRUPTION`（P21）：主干上也有，M3b 的用例可能让它更常出现 | 单独排查（与 M3a「延后事项」#4 一起跟踪） |
| 8 | `POST /v1/policies/test` 的 `policy_names` 没有数量上限（每个名字一个任务；测试本身受 8 个并发约束） | 有用户报告再说 |
| 9 | 订阅行自己带引号的整项 `"k=v"` 碰上 `external-policy-modifier` 的同名参数：`with_params` 按原文取键认不出它，修饰的值追加在后，订阅行自己的值先被读到（`client-cert` / `underlying-proxy` 已按套用修饰后的整行检查兜住，其它键仍是订阅行的值生效）；订阅行以未闭合的引号结尾时，追加的修饰被吞掉 | 单独小改动 |
| 10 | M4 的 `ssh`（`private-key=` 指向 `[Keystore]` 条目）与 WireGuard（`section-name=` 指向主配置里的段）同样是按名字用到主配置的材料，届时要加进 `reaches_into_profile` | M4 设计必查 |
| 11 | `TestBook::invalidate_all` 管不到正在跑的测试：在旧网络上开始的测试，会在调用之后才写入结果 | 阶段 3（接上"网络已变化"时） |
| 12 | `dial_internal` 被调用方取消（DNS 查询的截止时间）时，已进活动表的会话句柄不结束、也不进请求记录（慢的 `connect_tcp` 本来就有同样的缺口） | 有用户报告再说 |
| 13 | "服务端不保持连接"只告警一次的分支没有用例（要断言得接 tracing 订阅者，本计划不新增 dev 依赖） | 有用户报告再说 |
