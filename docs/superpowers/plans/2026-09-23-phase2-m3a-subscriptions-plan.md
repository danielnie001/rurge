# 阶段 2 / M3a「成员装配与订阅」Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 `[Proxy Group]` 从订阅（`policy-path`）、其它组（`include-other-group`）与全部代理（`include-all-proxies`）装配成员，支持过滤 / 前缀 / 修饰与组级 `underlying-proxy`（派生 `M (via R)`）；组环与空组在运行期兜底（环上的组 REJECT，空组默认 DIRECT、`--empty-group-reject` 时 REJECT）；订阅更新只重建策略表，没变的成员沿用原来的出站；拨号与控制面改读运行中的策略表。

**Architecture:** 配置层新增 `rurge_config::spec::GroupSpec`（组参数的强类型与校验，`W0030` 取代 `E0009`）。`rurge-policy` 新增两个纯函数模块：`subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源按手册顺序装配、过滤 → 前缀 → 修饰、全局重名、导入行的 `to_spec` 与中继成环检查、组级中继派生、组环）；注册表改为从装配结果构建（导入 / 派生条目按指纹复用，构建失败只略去该条，环上的组 REJECT，空组按 `EmptyGroup` 兜底，并保存控制面要看的定义行）。`rurge-engine` 每一代把订阅登记到资源管理器（它本来就在登记时同步载入磁盘缓存），一个随代存续的任务在订阅更新后重新装配、重建策略表并经 `EngineShared.cell` 发布——代际锁保证被替换的一代不会覆盖新一代；拨号与视图都从 cell 取策略表，`Runtime.policies` 去掉。

**Tech Stack:** Rust 1.89 / edition 2024；`fancy-regex` 0.14（沿用 URL-REGEX 的 `rurge_config::rule::Pattern`）、`url` 2（`rurge-config` 新增对工作区已有依赖的引用）、`tokio` 1.53（`watch::Receiver::mark_unchanged`）、`tokio-util` 0.7.19（`task::AbortOnDropHandle`）——**不新增任何第三方依赖**。

**Spec:** `docs/superpowers/specs/2026-09-23-phase2-m3-groups-subscriptions-design.md`（第 1.4 节 M3a 行、第 2 节 M3-D2 ～ D7 / D9、第 4 节、第 5 节、第 8 ～ 12 节、第 14 节 V2 ～ V6、第 15 节 M3a 草图）；总设计 `docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`。与本计划「计划期决定」表不一致处，以该表为准，并由 Task 10 写回设计文档新增的第 16 节。

## Global Constraints

- MSRV **1.89**，edition 2024。`unsafe_code`：全工作区 `forbid`；只有 `rurge-platform` 是 `deny` + 唯一一个 `#[allow(unsafe_code)]` 函数。**本计划不新增任何 unsafe。**
- 依赖方向不变：`rurge-policy` 不依赖 `rurge-engine`，也不依赖任何协议实现；`rurge-engine` / `rurge-api` / `rurge-policy` 不依赖 `rurge-platform`；平台代码只在 `rurge-platform`（AR-02）。**不新增任何第三方依赖**：`rurge-config` 新增对工作区已有 `url` 的引用，`Cargo.lock` 因此只多一行（`rurge-config` 的依赖列表里的 `"url"`），不下载任何东西。
- **测试绝不碰公网**：只用回环 + 端口 0 + 有界等待；不用固定 sleep 当同步手段（轮询 + 截止时间）。URL 订阅由回环 `TestServer` 提供；需要联网模式的引擎用例把 GeoIP 更新器的两个 URL 也指向这个 `TestServer`（得到 404），绝不访问默认的公网地址。
- **测试与任何命令都不得修改本机的系统代理、注册表、网络设置，不得注册真实服务 / 计划任务**：不在 CLI 测试夹具之外运行 `rurge run --system-proxy`；不运行不带 `--dry-run` 的 `rurge service install | uninstall`。CLI 测试一律经夹具里的 `rurge_run`（它接上文件后端，并清掉 `RURGE_SYSTEM_PROXY` 与本计划新增的 `RURGE_EMPTY_GROUP_REJECT`）。
- **不在本机下载或安装任何东西**（不装 sing-box、不装 xray、不 `rustup target add`、不 `cargo install`，也不装 `cargo-insta`：新快照用 `INSTA_UPDATE=always` 写出后逐字核对）。
- **凭据及其派生物永不外泄**（M3-D7）：订阅链接（常带 `?token=`）、订阅行（可能带口令）、`external-policy-modifier` 的值，永不出现在日志、诊断、错误文本、API 输出与 `Debug` 输出里。日志里的订阅只以组名出现；跳过的订阅行只报行号与原因，原因里不引用行内任何片段；持有订阅内容的类型（`Subscription`、`Assembly`、`Imported`、`Derived`）不实现 `Debug`；`PolicyPath::Url` 与 `ImportOpts::modifier` 用 `Secret` 包着。
- 长度先校验后分配：单个订阅资源 ≤ 64 MiB（资源管理器已有的上限），≤ 10 000 条策略（超出截断并 `W0024`）。
- rurge 专有的运行时选项只经命令行与环境变量提供，不扩展 Surge 配置格式（FR-CFG-17）。与 Surge 的每一处行为差异都登记进 `docs/surge-compatibility-matrix.md`。
- 日志、错误文本、CLI 输出、代码注释用英文；文档与提交标题用中文。注释密度、命名、惯用法与相邻代码保持一致。
- 提交信息结尾两行：`Co-Authored-By: <执行者自己的署名>` 与 `Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th`。**不 push、不 merge**，不 amend。
- 每个任务结束的门禁（全部通过才算完成）：

  ```bash
  RUSTFMT="C:\Users\SZV01065\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe" cargo fmt --all --check \
    && cargo clippy --all-targets -- -D warnings \
    && cargo test --workspace --no-fail-fast
  ```

  测试二进制异常退出而没有失败用例时（`STATUS_ACCESS_VIOLATION`、`STATUS_HEAP_CORRUPTION` / `0xc0000374`、段错误——本机已知的既有问题，见 P22），重跑一次并保留两次的日志，**不要在任务里去修它**。
- 第三方 crate 的 API 以本机源码为准：`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`。
- 本机的 bash 处理不了超过约 8 KB 或引号复杂的 heredoc：新文件一律用写文件的工具落盘，不用 heredoc。

## Review Focus

设计没有逐条写到、而最可能伤到使用者的五类输入或失败方式；每一条都在负责它的任务里配了用例。

1. **订阅链接里的 token 走漏**：链接写错（`ftp://`、空主机、空值）时的诊断、`Debug` 输出、`profiles/current` / `policies/detail`、资源管理器的日志、`rurge check` 的输出、"还没下载到"的告警——没有一处能带出 token。用例：Task 1 `a_bad_policy_path_is_an_error_that_never_echoes_it`、`a_group_line_loses_its_subscription_and_its_modifier`；Task 3 `a_subscription_not_downloaded_yet_is_said_without_its_url`；Task 6 `a_labelled_resource_is_logged_by_its_label`、`check_assembles_the_subscriptions_it_has`；Task 7 `the_views_show_imported_and_derived_policies`（`policy-path=***`）。
2. **订阅内容是别人写的**：`vmess://` 链接列表（`=` 结尾的 base64 被当成"名字 = 空定义"）、Clash YAML、`#!include /etc/passwd`、上万行——不回显行文本、不读任何本地文件、截断在 10 000 条、组按空组兜底并提示"可能不是 Surge 格式"。用例：Task 2 全部六条；Task 3 `a_shared_source_that_holds_nothing_is_reported_once`。
3. **导入行的 `underlying-proxy` 绕回自己**（直接绕回，或经主配置的策略、经组级中继绕回）：这样的行在装配时丢弃，拨号不会无限递归（`ChainConnector` 没有运行期深度守卫，见承接事项 C3）。用例：Task 3 `an_imported_chain_that_leads_back_is_dropped`；Task 4 `an_imported_chain_through_a_group_relay_is_dropped`。
4. **订阅更新与配置重载同时发生**：被替换的那一代在重载之后才完成重建，它不能把旧配置的策略表发布出去。用例：Task 8 `a_rebuild_of_a_replaced_generation_publishes_nothing`。
5. **`external-policy-modifier` 的值带逗号、引号、反斜杠、括号，或为空**：改写后的导入行必须读回完全相同的值，位置参数（`http, host, port, user, pass` 里的凭据）一字不动。用例：Task 3 `with_params_overrides_in_place_and_appends_the_rest`、`a_value_set_by_with_params_reads_back_unchanged`。


## 计划期决定

写计划时对照设计、Surge 手册与本仓库源码核对后定下的事；与设计文档文字不同的，由 Task 10 写回设计文档第 16 节。

**本计划里的代码不是凭空写的。** 全部 10 个任务的改动在仓库的一份副本上按任务顺序真实做了一遍：每个任务之后跑受影响 crate 的 fmt / clippy / 测试，任务 5 起每个任务之后跑一次全工作区门禁；最后一次全工作区门禁是 **39 个测试二进制，853 通过 / 0 失败 / 1 忽略**（本计划开工前的 main 是 38 个二进制、800 通过）；`rurge-engine` 的订阅集成用例与 `rurge-policy` 的全部单元用例各连跑 40 轮，无一失败。计划里新文件的全文取自副本上该任务的提交，修改处的"把 … 换成 …"取自同一份改动的原样文本——计划文本与验证过的代码一字不差。

| # | 事项 | 决定与依据 |
| - | ---- | ---------- |
| P1 | V2：`policy-regex-filter` 与 `policy-priority` 用哪个正则引擎 | `fancy-regex` 0.14，经 `rurge_config::rule::Pattern`（URL-REGEX 已在用，按源文本比较相等，`GroupSpec` 因此能派生 `PartialEq`）。Surge 用 NSRegularExpression（ICU）：`fancy-regex` 支持环视与反向引用，比 `regex` 更接近；ICU 专有的写法可能编译不过（`E0018`，回显正则本身）。匹配出错（回溯超过上限）按"不匹配"处理，与 URL-REGEX 一致 |
| P2 | V3："构建前同步载入磁盘缓存"放在哪；资源管理器的日志是否带 URL | 读 `crates/rurge-net/src/resource/mod.rs`：`ResourceManager::get` 在返回句柄之前已经同步读了 URL 的磁盘缓存（`start` 里的 `CacheDir::load`）与本地文件（`read_file`），**不需要新入口**，M3-D5 只要求"每一代构建前先 `get` 再读快照"。它的日志有四处带资源名（`url = %url` 三处、文件监视失败时的 `path`）：新增 `ResourceManager::get_labelled(spec, label)`，登记过标签的资源在日志里只叫标签（`policy-path of` 加上用反引号括起的组名），第一个标签保留；规则集照旧用 `get`。`rurge check` 不起资源管理器，改用新增的 `rurge_net::resource::cached(root, source)` 离线读缓存 |
| P3 | V4：订阅文本的节识别与行号 | 先用配置解析器 `rurge_config::text::parse_str` 解析：有 `[Proxy]` 节就只取它的条目（`active_entries`，行号来自解析器；requirement 指令不求值；`#!include` 只是一行"不是策略"的文本，**从不展开**——订阅内容绝不能让 rurge 读本地文件）。没有 `[Proxy]` 节时整个文本逐行读，跳过空行与 `#` `//` `;` 开头的行（设计只写了 `#` 与 `//`；`;` 与配置解析器的注释规则一致），行内注释照配置解析器去掉。导入行的 `Span` 文件名固定为 `policy-path`，行号是订阅里的行号。跳过的原因不引用行内任何片段：`parse_policy` 自己的消息会引用类型关键字与端口，而一条 `vmess://…=` 链接会被读成"名字 = 空定义"，那个"名字"就是凭据——所以原因只分"not a valid policy line / unknown policy type / not a policy line"三种 |
| P4 | V5：`external-policy-modifier` 的切分与应用 | 组行上的值已由 `split_list` 去掉一层引号（`test-url=http://apple.com/,tfo=true`）；再 `split_list` 一次得到各项，每项 `parse_key_value`，键转小写；空列表或有一项不是 `key=value` → `E0018`（不回显取值：可能是口令）。应用在文本层：新增 `rurge_config::policy::with_params(definition, overrides)`，按解析器自己的顶层逗号规则（复用 `redact` 里的 `split_top_level`，改成 `pub(crate)`）逐项找同名参数——第一处原地替换、后面的重复删掉、没有的追加；值里有逗号、引号、括号、反斜杠或首尾空白时加引号并转义。改写后的整行再经 `parse_policy` 读一遍，所以 `definition`（控制面看到的）与参数（`to_spec` 读的）始终一致；位置参数一字不动 |
| P5 | V6：`Runtime.policies` 的去留、`resolve` 的调用处、代际锁 | `Runtime.policies` 去掉，换成 `pub(crate) registry: Option<Arc<PolicyRegistry>>`——只在 `Engine::new` / `swap_runtime` 发布时被取走一次，之后正在用的永远是 `EngineShared.cell` 里的。新增 `Engine::registry()`；拨号每条会话只取一次（名字的校验与解析用同一份），`socket_opener` 改为从注册表取 spec（`PolicyRegistry::spec`，即指纹里的那份），视图与 `policies_view` 都读注册表。代际锁是 `std::sync::Mutex<()>`（`Engine::generation_lock`），**只包住"核对当前代 + 发布"与重载的"发布 + 切换"**；重建本身在锁外（设计写的是 tokio 锁、构建期间持有）——构建期间读到的 `previous` 若已过时，代价只是少复用几个出站，而锁里从不做 I/O |
| P6 | `subnet` 组 | 空组兜底（M3-D3）会让今天解析为 REJECT 的 `subnet` 组悄悄改走 DIRECT。所以阶段 3 之前 `GroupSpec.members` 对 `subnet` 组取它的 `default`（没写 `default` 才按空组兜底）；`W0008` 的"用第一个成员"对它也就讲得通。顺带：`subnet` 组上已知的组参数（`category`、`policy-path` 等）不再被 `parse_group` 当成网络条件——`SUBNET_GROUP_PARAMS` 换成列出全部组参数的 `GROUP_PARAMS`，装配类参数写在 `subnet` 上因此能报 `W0028`（设计 4.2 的那一行），而不是被读成一个指向未知策略的条件（`E0008`） |
| P7 | 脱敏名单 | 除设计要求的 `policy-path` 外，再加 `external-policy-modifier`：修饰列表能给导入行设任何参数，口令也在内，而名单里的 `password` 只在参数边界匹配——`external-policy-modifier="password=…"` 里紧跟引号的那个找不到。整个值变成 `***`，偏安全一侧 |
| P8 | 装配告警的诊断码（设计 5.9 只说"一并列出"） | 沿用集合的两个码并扩写其注释：跳过的订阅行、重名、与配置同名、另一个组已导入了不同定义、导入行的错误、导入行的中继成环、派生名被占用 → `W0023`；超过 10 000 条 → `W0024`；订阅没有内容 → `W0022`（与规则集"资源不可用"同一条）；导入了未实现的协议 → `W0007`（每种一次）。每条都挂在组那一行的 `Span` 上，文本以 `` policy group `G`: `` 开头 |
| P9 | `W0022` 的措辞 | 设计 5.9 的 "has not been downloaded yet" 对读不到的本地文件不成立。改为 `` `policy-path` has no content yet (never downloaded, or the file cannot be read); its imported members are unknown `` |
| P10 | `include-other-group` 取派生前还是派生后的成员 | 派生前：全部组先装配完，最后才对设了中继的组派生；被引用组的中继不会带进引用它的组（它有自己的 `underlying-proxy` 可写） |
| P11 | 派生名撞上已有的名字 | 配置或订阅里已有一个叫 `M (via R)` 的策略时，略去该成员并 `W0023`，**绝不改用不经中继的 M**——使用者设中继往往就是为了不让流量直出 |
| P12 | 成环检查放在哪 | 两种环都在装配里算：① 导入行的 `underlying-proxy` 成环——图的边是主配置与导入策略的 `underlying-proxy`、各组装配后的成员、以及**各组自己的中继**（设了 `underlying-proxy = R` 的组 G，每个代理成员都经 R 出站，所以 G → R 也是一条边）；成环的导入行丢弃。② 组环——边是装配后的成员与 `include-other-group`；`Assembly.cycles` 列出每个环（`[A, B, A]`），注册表据此把环上的组标成 REJECT。`include-other-group` 形成的环在展开时先算出来，环上的组不展开给任何组（否则展开没有尽头） |
| P13 | 组级 `underlying-proxy` 的 `E0019` | `underlying_cycles` 扩成同时检查策略与组：组的边加上 `include-other-group` 与它自己的中继；经中继绕回本组 → `` policy group `G`: `underlying-proxy` leads back to the group itself (via `R`) ``。这条边也让"策略 P 的中继是组 G、G 的中继又是 P"这类环在策略那一侧被查出来 |
| P14 | `rurge check` 怎样找到缓存 | 新增 `--data-dir`（环境变量 `RURGE_DATA_DIR`，默认平台数据目录，与 `run` 相同）；`POST /v1/profiles/check` 用守护进程自己的数据目录（`Engine::data_dir`）。两者都经 `rurge_engine::check_profile(path, opts, data_dir)`：`load_checked` 之后，配置本身无错时才做订阅装配检查 |
| P15 | 环境变量的取值 | 设计写的是 `RURGE_EMPTY_GROUP_REJECT=1`。rurge 的布尔开关（`RURGE_WATCH`、`RURGE_NO_NETWORK`、`RURGE_SYSTEM_PROXY`）都由 clap 的布尔解析器读，**只接受 `true` / `false`**——写计划时实测 `RURGE_NO_NETWORK=1 rurge rule match …` 报 `invalid value '1' for '--no-network'`。本计划与它们保持一致：`RURGE_EMPTY_GROUP_REJECT=true` |
| P16 | 任务的切分 | 设计草图约 9 个任务，本计划 10 个：草图第 ⑦ 项拆成"拨号与视图改读注册表、`EmptyGroup`"（Task 7）与"订阅热重建与代际锁"（Task 8），各自有能单独评审的交付物 |
| P17 | 导入行的 `to_spec` 诊断 | 只有错误让该行被跳过（`W0023`，带原因）；导入行上的未知参数（`W0001`）、无效参数（`W0028`）、暂不生效的参数（`W0029`）不输出——一个上万行的订阅会把日志淹没。未实现的协议每种说一次（`W0007`） |
| P18 | 持有订阅内容的类型 | `Subscription`、`Assembly`、`Imported`、`Derived` 不派生 `Debug`（它们带着订阅行的原文）；`PolicyPath::Url(Secret<Url>)`、`ImportOpts::modifier: Secret<Vec<(String, String)>>`，于是 `GroupSpec` 的 `Debug` 不带 token 与修饰值（`Secret` 为此加派生 `Hash`，`PolicyPath` 要当快照表的键） |
| P19 | 导入节点的主机名与 `[Host]` | 解析器每代只从 `Config::proxy_hostnames()`（主配置的代理）取"不受 `[Host]` 影响"的名单；订阅里节点的主机名不在其中，`[Host]` 对它们照常生效。订阅在运行期变化而解析器不随之重建，本计划不改，登记进「延后事项」 |
| P20 | 重建的细节 | 去抖常量 `REBUILD_DEBOUNCE = 1 s`：第一次变化之后等 1 秒，把这段时间里的其它变化一起并进这次重建（`mark_unchanged`）。每次重建重新解析全部订阅（便宜，不做按来源的缓存）；装配告警逐条 WARN；成员有变化的组各记一条 INFO（组名与增减数量）；主配置策略的构建在重建时失败（理论上不会：它们与这一代首次构建时相同）→ WARN 并保留当前的策略表 |
| P21 | 全局策略（`proxy` 模式） | 与拨号一样按运行中的策略表校验，所以导入的策略名也能设成全局策略（`policy_exists` / `set_global_policy`）；设计没有提到，登记进第 16 节 |
| P22 | 既有的偶发崩溃 | 写计划时 `outbounds_tls_family` 测试二进制偶发以 `STATUS_HEAP_CORRUPTION`（`0xc0000374`）或段错误退出，约 0.25 – 0.5%；在**未做任何改动的副本**上同样复现（2 / 800），与 M2b 计划「延后事项」里的 `STATUS_ACCESS_VIOLATION` 是同一类现象。全工作区 `forbid(unsafe_code)`，根因在某个依赖的原生代码里，未查明。本计划不处理；门禁遇到时重跑（Global Constraints） |

## 承接事项

之前计划「延后事项」表里标给 M3 的条目，以及写本计划时要复核的。

| # | 来源 | 事项 | 处理 | 任务 |
| - | ---- | ---- | ---- | ---- |
| C1 | M1b | 组的环降级、空组回退 | 本计划的主体：`W0030` 取代 `E0009`（Task 1），装配期的环（Task 3），注册表的 REJECT 与空组兜底（Task 5），`EmptyGroup` 与命令行开关（Task 7） | 1、3、5、7 |
| C2 | M1b | "策略存在但不可用 → REJECT" | 未实现的协议照旧是 `Note::Unsupported`（REJECT）；导入与派生的策略构建失败时只略去那一条、成员表随之去掉它（M3-D6）；主配置策略的构建失败仍是加载期错误 | 5 |
| C3 | M1b | `ChainConnector` 的运行期深度兜底（订阅引入动态成员时） | 静态兜住：装配后的图含每个组的全部成员与各组的中继，任何选择都走不出这张图，而这张图里的导入行成环在装配时丢弃（P12），主配置里的环是 `E0019`（P13）。运行期的深度守卫仍然没有（要给 `ConnectOpts` 加深度并改动它的全部构造点），带进本计划「延后事项」 | 3、4 |
| C4 | M1b | 链底下的 REJECT 到不了 `dial_internal` 的旁路；链深超过 `MAX_DEPTH` 时 `socket_opener` 与注册表说法不一致 | 与订阅无关，M3a 不动；M3b 会改拨号入口（`SelectCtx`、可等待的 `resolve`），届时一并处理，带进「延后事项」 | — |
| C5 | M1b | `test-url` / `test-timeout` 的使用、请求记录的计时字段 | M3b / M3c | — |
| C6 | M2b | `environment()` 是人工维护的约定，M3 动工厂时复核 | M3a 只把工厂装进 `Arc` 由 `Runtime` 保存（重建要用），没有给 `EngineFactory` 加任何按值捕获的字段，约定不受影响；继续跟踪 | 8 |
| C7 | M2b | 测试二进制偶发 `STATUS_ACCESS_VIOLATION` | 新的观察见 P22；继续跟踪 | — |


## File Structure

新建：

| 文件 | 职责 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-config/src/spec/group.rs` | `GroupSpec` / `ImportOpts` / `TestOpts` / `Priority` / `PolicyPath`、`to_group_spec`（组参数的读取、校验与 `W0028` / `W0006` / `W0001`）与用例 | 1 |
| `crates/rurge-config/tests/snapshots/corpus__corpus__group-cycle.snap` | 组环从"无效语料"移到"有效语料"后的快照（`W0030`） | 1 |
| `crates/rurge-policy/src/subscription.rs` | 订阅文本 → `Subscription`（策略行、跳过的行号与原因、是否截断） | 2 |
| `crates/rurge-policy/src/assemble.rs` | `assemble(cfg, snapshots) -> Assembly`：四类来源的装配、导入行的 `to_spec` 与中继成环检查、组环（3）；组级中继派生（4） | 3、4 |
| `crates/rurge-engine/src/subscriptions.rs` | 每一代的订阅登记与快照、`check_profile`（6）；订阅热重建任务、`Engine::rebuild_registry`（8） | 6、8 |
| `crates/rurge-engine/tests/subscriptions.rs` | 订阅经整个引擎的用例：首代即有成员（6）、空组兜底与视图（7）、文件与 URL 订阅的热重建与缓存（8）、经导入 / 派生成员出站（9） | 6 – 9 |

修改：

| 文件 | 改动 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-config/Cargo.toml`、`Cargo.lock` | 引用工作区已有的 `url` | 1 |
| `crates/rurge-config/src/spec/{mod.rs, secret.rs}` | 声明并导出 `group`；`Secret` 派生 `Hash` | 1 |
| `crates/rurge-config/src/diagnostic.rs` | `W0030`，`E0009` 标为退役（1）；`W0023` / `W0024` 的注释（3、4） | 1、3、4 |
| `crates/rurge-config/src/policy.rs` | `GROUP_PARAMS` 取代 `SUBNET_GROUP_PARAMS`（1）；`with_params` 与用例（3） | 1、3 |
| `crates/rurge-config/src/redact.rs` | 名单加 `policy-path` / `external-policy-modifier` 与用例（1）；`split_top_level` 改 `pub(crate)`（3） | 1、3 |
| `crates/rurge-config/src/config.rs` | `Config.group_specs`、`Config::name_kind`、`group_edges`、`underlying_cycles` 覆盖组、`W0030`、校验调用 `to_group_spec`，与用例 | 1 |
| `tests/corpus/invalid/group-cycle.{conf,expect}` | `.conf` 移到 `tests/corpus/valid/`，`.expect` 删除 | 1 |
| `crates/rurge-policy/src/lib.rs` | 声明 `subscription`（2）、`assemble` 与导出（3）、注册表新类型的导出（5） | 2、3、5 |
| `crates/rurge-policy/src/registry.rs` | `build` 接受 `Assembly` 与 `EmptyGroup`；导入 / 派生条目；环与空组；`Note` 的两个新变体与 `Display`；`Line` / `GroupInfo` 与访问方法；用例 | 5 |
| `crates/rurge-policy/src/cell.rs` | 用例里的 `build` 调用 | 5 |
| `crates/rurge-engine/src/runtime.rs` | 按新签名构建注册表（5）；登记订阅、首次装配、诊断（6）；`registry` 取代 `policies`、`empty_group`（7）；`factory` / `subscriptions` / `watcher` 字段（8） | 5 – 8 |
| `crates/rurge-engine/src/engine.rs` | 会话说明用 `Note` 的 `Display`（5）；`registry()`、`data_dir()`（6）；拨号与 `policies_view` 改读注册表（7）；代际锁与监听任务的启动（8） | 5 – 8 |
| `crates/rurge-engine/src/{lib.rs, outbounds.rs}` | 模块与导出；`load_checked` 的注释 | 6、7 |
| `crates/rurge-engine/src/{shared.rs, reload.rs, views.rs}` | `EngineShared.empty_group`；`swap_runtime` 取走注册表（7）并在代际锁下发布、启动新一代的监听任务（8）；视图读注册表 | 7、8 |
| `crates/rurge-engine/tests/{common/mod.rs, outbounds.rs}` | `harness_with` 改 `pub`；两处 `runtime().policies` 改读 `registry()` | 7 |
| `crates/rurge-net/src/resource/mod.rs` | `Entry.label` 与 `log_name`、`get_labelled`、`cached`，日志不再带 URL，与用例 | 6 |
| `crates/rurge-api/src/routes/{profiles.rs, policy_groups.rs}` | `check` 经 `check_profile`（6）；一句过时的注释（10） | 6、10 |
| `crates/rurge/src/cli/{check.rs, run.rs}` | `check --data-dir`（6）；`--empty-group-reject` 与启动行的计数（7） | 6、7 |
| `crates/rurge/tests/cli.rs` | `check` 的订阅用例（6）；`spawn_command` 与空组开关的用例（9） | 6、9 |
| 文档（清单、API 参考、手工验收、M3 设计第 16 节、两份 README、CLAUDE.md、本计划末尾两张表） | 见 Task 10 | 10 |

## 任务一览

| 任务 | 交付物 | 依赖 |
| ---- | ------ | ---- |
| 1 | `GroupSpec` 与组参数的全部校验；`W0030` 取代 `E0009`；脱敏名单 | — |
| 2 | 订阅文本解析 | — |
| 3 | 成员装配（不含派生）与 `with_params` | 1、2 |
| 4 | 组级中继派生 | 3 |
| 5 | 注册表接受装配结果；环与空组兜底；引擎按新签名构建 | 4 |
| 6 | 资源管理器的标签与离线读缓存；引擎登记订阅（构建时同步载入）；`check --data-dir` | 5 |
| 7 | 拨号与视图改读注册表；`EmptyGroup` 与 `--empty-group-reject` | 6 |
| 8 | 订阅热重建任务与代际锁 | 7 |
| 9 | 端到端：经导入 / 派生成员出站；CLI 的空组开关 | 8 |
| 10 | 文档 | 9 |

---


### Task 1: 配置层——`GroupSpec`、组参数的校验、`W0030` 取代 `E0009`、脱敏名单

组参数从一张无类型的 `ParamMap` 变成强类型的 `GroupSpec`：装配类参数（`policy-path` 等）、组级中继、测速类参数（M3b 才生效，这里只校验）、`policy-priority`（M3c）。设计 4.2 的校验表全部落在这里；组环从加载错误变成告警（M3-D9）。本任务不改变任何运行期行为——注册表到 Task 5 才读 `GroupSpec`。

**Files:**
- Create: `crates/rurge-config/src/spec/group.rs`
- Create: `crates/rurge-config/tests/snapshots/corpus__corpus__group-cycle.snap`
- Modify: `crates/rurge-config/Cargo.toml`、`Cargo.lock`
- Modify: `crates/rurge-config/src/spec/mod.rs`、`crates/rurge-config/src/spec/secret.rs`
- Modify: `crates/rurge-config/src/diagnostic.rs`、`crates/rurge-config/src/policy.rs`、`crates/rurge-config/src/redact.rs`、`crates/rurge-config/src/config.rs`
- Move: `tests/corpus/invalid/group-cycle.conf` → `tests/corpus/valid/group-cycle.conf`；Delete: `tests/corpus/invalid/group-cycle.expect`

**Interfaces:**
- Consumes: `rurge_config::rule::Pattern`（`Pattern::new(&str) -> Result<Pattern, String>`，字段 `source`、`regex`）、`rurge_config::spec::{NameKind, Secret}`、`value::{parse_bool, parse_key_value, split_list}`、`policy::{Builtin, GroupKind, PolicyGroup}`、诊断码 `E0007` `E0008` `E0018` `E0019` `W0001` `W0006` `W0028`。
- Produces（后续任务按这些名字与类型使用）:
  - `rurge_config::spec::PolicyPath { Url(Secret<url::Url>), File(PathBuf) }`：`Clone + Debug + PartialEq + Eq + Hash`
  - `rurge_config::spec::ImportOpts { policy_path: Option<PolicyPath>, update_interval: Option<u64>, regex_filter: Option<Pattern>, name_prefix: Option<String>, modifier: Secret<Vec<(String, String)>>, include_all_proxies: bool, include_other_groups: Vec<String> }`：`Clone + Debug + Default + PartialEq + Eq`
  - `rurge_config::spec::TestOpts { interval: Duration, tolerance: Duration, timeout: Option<Duration>, evaluate_before_use: bool, persistent: bool }`（`Default`：600 s / 100 ms / `None` / `false` / `false`）
  - `rurge_config::spec::Priority { pattern: Pattern, factor: f64 }`
  - `rurge_config::spec::GroupSpec { name, kind: GroupKind, members: Vec<String>, import: ImportOpts, underlying_proxy: Option<String>, test: TestOpts, priority: Vec<Priority>, hidden: bool, span: Span }`：`Clone + Debug + PartialEq + Eq`
  - `rurge_config::spec::to_group_spec(group: &PolicyGroup, base_dir: &Path, lookup: &dyn Fn(&str) -> Option<NameKind>) -> GroupOutcome`（`GroupOutcome { spec: Option<GroupSpec>, diagnostics: Vec<Diagnostic> }`）
  - `Config.group_specs: Vec<GroupSpec>`（与 `groups` 同序；有错误的组不在其中）、`Config::name_kind(&self, name) -> Option<NameKind>`
  - `codes::W_GROUP_CYCLE`（`W0030`）；`codes::E_GROUP_CYCLE`（`E0009`）保留但不再发出（"Never renumber"）

- [ ] **Step 1: 脱敏的回归用例**

`crates/rurge-config/src/redact.rs`——把

```rust
            "snell, 1.2.3.4, 443, psk=***, shadow-tls-password=***, shadow-tls-sni=example.com"
        );
    }
```

换成

```rust
            "snell, 1.2.3.4, 443, psk=***, shadow-tls-password=***, shadow-tls-sni=example.com"
        );
    }

    /// A subscription URL usually carries a token, and a modifier can set any
    /// parameter of the imported lines, a password included (M3-D7).
    #[test]
    fn a_group_line_loses_its_subscription_and_its_modifier() {
        assert_eq!(
            redact_definition(
                "select, A, policy-path=https://sub.test/nodes?token=t0k3n, update-interval=3600, external-policy-modifier=\"password=hunter2,tfo=true\", policy-regex-filter=^HK"
            ),
            "select, A, policy-path=***, update-interval=3600, external-policy-modifier=***, policy-regex-filter=^HK"
        );
        // a local file goes all the same: the safe side
        assert_eq!(
            redact_profile("G = select, policy-path=nodes.txt"),
            "G = select, policy-path=***"
        );
    }
```

- [ ] **Step 2: 运行，确认它在旧名单上变红**

Run: `cargo test -p rurge-config --lib redact`

Expected: `a_group_line_loses_its_subscription_and_its_modifier` 失败，左边的 `policy-path=https://sub.test/nodes?token=t0k3n` 与 `external-policy-modifier="password=hunter2,tfo=true"` 原样出现（`token` 前面是 `?`、`password` 前面是引号，旧名单都够不着）。

- [ ] **Step 3: 名单加两项**

`crates/rurge-config/src/redact.rs`——把

```rust
/// nodes behind a CDN routinely use as a shared secret. `shadow-tls-password`
/// needs its own entry: `password` only matches at a token boundary.
/// Over-redacting is the safe side for an endpoint whose purpose is safe output.
const SECRET_PARAMS: [&str; 12] = [
```

换成

```rust
/// nodes behind a CDN routinely use as a shared secret. `shadow-tls-password`
/// needs its own entry: `password` only matches at a token boundary. A
/// group's `policy-path` usually carries a subscription token, and its
/// `external-policy-modifier` can set any parameter, a password included.
/// Over-redacting is the safe side for an endpoint whose purpose is safe output.
const SECRET_PARAMS: [&str; 14] = [
```

`crates/rurge-config/src/redact.rs`——把

```rust
    "ws-path",
    "shadow-tls-password",
];
```

换成

```rust
    "ws-path",
    "shadow-tls-password",
    "policy-path",
    "external-policy-modifier",
];
```

Run: `cargo test -p rurge-config --lib redact` → 全部通过。

- [ ] **Step 4: 依赖、`Secret` 的 `Hash`、诊断码与模块声明**

`crates/rurge-config/Cargo.toml`——把

```toml
base64.workspace = true

[dev-dependencies]
```

换成

```toml
base64.workspace = true
url.workspace = true

[dev-dependencies]
```

改完跑一次 `cargo build -p rurge-config`，让 `Cargo.lock` 里 `rurge-config` 的依赖列表多出 `"url"` 这一行（`url` 已在锁文件里，不下载任何东西）。

`crates/rurge-config/src/spec/secret.rs`——把

```rust
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret<T>(T);
```

换成

```rust
#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub struct Secret<T>(T);
```

`crates/rurge-config/src/diagnostic.rs`——把

```rust
    pub const E_UNKNOWN_GROUP_MEMBER: &str = "E0008";
    pub const E_GROUP_CYCLE: &str = "E0009";
```

换成

```rust
    pub const E_UNKNOWN_GROUP_MEMBER: &str = "E0008";
    /// Retired in phase 2 M3: a group cycle is `W0030` and no longer stops a load.
    pub const E_GROUP_CYCLE: &str = "E0009";
```

`crates/rurge-config/src/diagnostic.rs`——把

```rust
    pub const W_PARAM_NOT_EFFECTIVE: &str = "W0029";
```

换成

```rust
    pub const W_PARAM_NOT_EFFECTIVE: &str = "W0029";
    /// Policy groups that reference each other; they behave as REJECT.
    pub const W_GROUP_CYCLE: &str = "W0030";
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
pub mod common;
pub mod http;
```

换成

```rust
pub mod common;
pub mod group;
pub mod http;
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
pub use common::{Applies, CommonOpts, IpVersion, Tristate};
```

换成

```rust
pub use common::{Applies, CommonOpts, IpVersion, Tristate};
pub use group::{
    GroupOutcome, GroupSpec, ImportOpts, PolicyPath, Priority, TestOpts, to_group_spec,
};
```

- [ ] **Step 5: `group.rs` 的用例先行**

新建 `crates/rurge-config/src/spec/group.rs`，**先只写文件头的 `use` 与文件末尾的 `#[cfg(test)] mod tests { … }`**（全文见 Step 6）。

Run: `cargo test -p rurge-config --lib spec::group`

Expected: 编译错误——`cannot find function `to_group_spec``、`cannot find type `GroupSpec`` 等。

- [ ] **Step 6: 写 `group.rs`**

`crates/rurge-config/src/spec/group.rs` 全文：

```rust
//! Typed view of `[Proxy Group]` parameters (phase 2 M3 design §4): where a
//! group's members come from, the relay it chains them through, and the
//! testing options the automatic groups act on (M3b / M3c).

use super::{NameKind, Secret};
use crate::diagnostic::{Diagnostic, Severity, codes};
use crate::policy::{Builtin, GroupKind, PolicyGroup};
use crate::rule::Pattern;
use crate::span::Span;
use crate::value::{parse_bool, parse_key_value, split_list};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;
use url::Url;

/// How long a test result stays valid when `interval` is not written.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(600);
/// `url-test` switch damping when `tolerance` is not written.
pub const DEFAULT_TOLERANCE: Duration = Duration::from_millis(100);

/// Where a group's `policy-path` points.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PolicyPath {
    /// A subscription URL usually carries a token: `Debug` hides it.
    Url(Secret<Url>),
    /// Already resolved against the main profile's directory.
    File(PathBuf),
}

/// What a group takes in besides the members written on its line (manual:
/// Policy Including).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportOpts {
    pub policy_path: Option<PolicyPath>,
    /// Seconds; only a URL `policy-path` is refreshed.
    pub update_interval: Option<u64>,
    /// Applies to every member that is not written on the line.
    pub regex_filter: Option<Pattern>,
    pub name_prefix: Option<String>,
    /// `external-policy-modifier`: parameters that override those of every
    /// `policy-path` line, keys lowercase. A value may be a credential.
    pub modifier: Secret<Vec<(String, String)>>,
    pub include_all_proxies: bool,
    pub include_other_groups: Vec<String>,
}

/// When and how the automatic groups test their members.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestOpts {
    pub interval: Duration,
    /// `url-test` only; an explicit 0 is honoured.
    pub tolerance: Duration,
    /// A member whose score is not below this is not a candidate.
    pub timeout: Option<Duration>,
    pub evaluate_before_use: bool,
    /// `load-balance` only.
    pub persistent: bool,
}

impl Default for TestOpts {
    fn default() -> TestOpts {
        TestOpts {
            interval: DEFAULT_INTERVAL,
            tolerance: DEFAULT_TOLERANCE,
            timeout: None,
            evaluate_before_use: false,
            persistent: false,
        }
    }
}

/// One `policy-priority` pair: the first pattern that matches a member's
/// name scales its `smart` score by `factor`.
#[derive(Clone, Debug, PartialEq)]
pub struct Priority {
    pub pattern: Pattern,
    pub factor: f64,
}

// A factor is finite and above zero (checked at load), so `==` is total.
impl Eq for Priority {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupSpec {
    pub name: String,
    pub kind: GroupKind,
    /// Written on the line, in order; a `subnet` group's is its `default`.
    pub members: Vec<String>,
    pub import: ImportOpts,
    /// The relay every proxy member is chained through (FR-GRP-07); `DIRECT`
    /// is the same as none.
    pub underlying_proxy: Option<String>,
    pub test: TestOpts,
    pub priority: Vec<Priority>,
    pub hidden: bool,
    pub span: Span,
}

#[derive(Debug, Default)]
pub struct GroupOutcome {
    /// `None` when an error was found.
    pub spec: Option<GroupSpec>,
    pub diagnostics: Vec<Diagnostic>,
}

/// The parameters that decide the members; a `subnet` group takes none.
const IMPORT_PARAMS: [&str; 8] = [
    "policy-path",
    "update-interval",
    "policy-regex-filter",
    "external-policy-name-prefix",
    "external-policy-modifier",
    "include-all-proxies",
    "include-other-group",
    "underlying-proxy",
];

/// Typed reads over one group's parameters. Remembers which keys were read
/// so that `finish` can warn about the rest.
struct Reader<'a> {
    group: &'a PolicyGroup,
    used: HashSet<&'static str>,
    diags: Vec<Diagnostic>,
}

impl<'a> Reader<'a> {
    fn has(&self, key: &str) -> bool {
        self.group.params.contains(key)
    }

    fn str(&mut self, key: &'static str) -> Option<&'a str> {
        self.used.insert(key);
        let group = self.group;
        group.params.get(key)
    }

    fn bool(&mut self, key: &'static str) -> Option<bool> {
        let value = self.str(key)?;
        let parsed = parse_bool(value);
        if parsed.is_none() {
            self.invalid(key, value, "true or false");
        }
        parsed
    }

    /// A whole number; `positive` rules out 0.
    fn number(&mut self, key: &'static str, positive: bool, expected: &str) -> Option<u64> {
        let value = self.str(key)?;
        match value.trim().parse::<u64>() {
            Ok(n) if n > 0 || !positive => Some(n),
            _ => {
                self.invalid(key, value, expected);
                None
            }
        }
    }

    /// `E0018`. Never call this for a value that may carry a credential.
    fn invalid(&mut self, key: &str, value: &str, expected: &str) {
        self.error(
            codes::E_INVALID_POLICY_PARAM,
            format!("invalid value `{value}` for `{key}` (expected {expected})"),
        );
    }

    fn error(&mut self, code: &'static str, message: String) {
        let message = format!("policy group `{}`: {message}", self.group.name);
        self.diags
            .push(Diagnostic::error(code, message).at(self.group.span.clone()));
    }

    fn warn(&mut self, code: &'static str, message: String) {
        let message = format!("policy group `{}`: {message}", self.group.name);
        self.diags
            .push(Diagnostic::warning(code, message).at(self.group.span.clone()));
    }

    /// `W0028` when `key` is written although this group type ignores it;
    /// the value is not read.
    fn not_applicable(&mut self, key: &'static str) {
        if self.has(key) {
            self.used.insert(key);
            let kind = self.group.kind.keyword();
            self.warn(
                codes::W_PARAM_NOT_APPLICABLE,
                format!("`{key}` has no effect on a `{kind}` group; ignored"),
            );
        }
    }

    fn has_errors(&self) -> bool {
        self.diags.iter().any(|d| d.severity == Severity::Error)
    }

    /// Warns about every parameter nobody read.
    fn finish(mut self) -> Vec<Diagnostic> {
        let group = self.group;
        let mut reported = HashSet::new();
        for (key, _) in group.params.iter() {
            if !self.used.contains(key) && reported.insert(key) {
                self.warn(
                    codes::W_UNKNOWN_KEY,
                    format!("unknown parameter `{key}` ignored"),
                );
            }
        }
        self.diags
    }
}

/// `lookup` says what a name refers to; `base_dir` is the main profile's
/// directory, which a relative `policy-path` is resolved against.
pub fn to_group_spec(
    group: &PolicyGroup,
    base_dir: &Path,
    lookup: &dyn Fn(&str) -> Option<NameKind>,
) -> GroupOutcome {
    let mut r = Reader {
        group,
        used: HashSet::new(),
        diags: Vec::new(),
    };
    // only the user interface looks at these
    for key in ["no-alert", "icon-url", "category"] {
        r.used.insert(key);
    }
    let hidden = r.bool("hidden").unwrap_or(false);
    if r.has("url") {
        r.used.insert("url");
        r.warn(
            codes::W_VANISHED_KEY,
            "`url` has no effect in current versions; use the policy's `test-url` or `proxy-test-url`"
                .to_string(),
        );
    }
    let (members, import, underlying_proxy) = if group.kind == GroupKind::Subnet {
        // Which network this is only arrives in phase 3; until then the
        // group stands for its `default`.
        for key in ["default", "cellular"] {
            r.used.insert(key);
        }
        for key in IMPORT_PARAMS {
            r.not_applicable(key);
        }
        let default = group.params.get("default").map(str::to_string);
        (default.into_iter().collect(), ImportOpts::default(), None)
    } else {
        let import = read_import(&mut r, base_dir, lookup);
        let underlying_proxy = read_underlying(&mut r, lookup);
        (group.members.clone(), import, underlying_proxy)
    };
    let test = read_test(&mut r, group.kind);
    let priority = if group.kind == GroupKind::Smart {
        read_priority(&mut r)
    } else {
        r.not_applicable("policy-priority");
        Vec::new()
    };
    let failed = r.has_errors();
    let diagnostics = r.finish();
    let spec = (!failed).then(|| GroupSpec {
        name: group.name.clone(),
        kind: group.kind,
        members,
        import,
        underlying_proxy,
        test,
        priority,
        hidden,
        span: group.span.clone(),
    });
    GroupOutcome { spec, diagnostics }
}

fn read_import(
    r: &mut Reader<'_>,
    base_dir: &Path,
    lookup: &dyn Fn(&str) -> Option<NameKind>,
) -> ImportOpts {
    let mut import = ImportOpts::default();
    if let Some(value) = r.str("policy-path") {
        import.policy_path = policy_path(r, value, base_dir);
    }
    import.update_interval = r.number("update-interval", true, "a positive number of seconds");
    if let Some(value) = r.str("policy-regex-filter") {
        match Pattern::new(value) {
            Ok(pattern) => import.regex_filter = Some(pattern),
            Err(e) => r.error(
                codes::E_INVALID_POLICY_PARAM,
                format!("invalid `policy-regex-filter` `{value}`: {e}"),
            ),
        }
    }
    if let Some(value) = r.str("external-policy-name-prefix") {
        if value.contains('=') {
            r.invalid("external-policy-name-prefix", value, "a prefix without `=`");
        } else {
            import.name_prefix = Some(value.to_string());
        }
    }
    if let Some(value) = r.str("external-policy-modifier") {
        match modifier(value) {
            Some(pairs) => import.modifier = Secret::new(pairs),
            // never echoed: a value may be a credential
            None => r.error(
                codes::E_INVALID_POLICY_PARAM,
                "invalid `external-policy-modifier` (expected a quoted list of key=value pairs)"
                    .to_string(),
            ),
        }
    }
    import.include_all_proxies = r.bool("include-all-proxies").unwrap_or(false);
    if let Some(value) = r.str("include-other-group") {
        let names = split_list(value);
        if names.is_empty() {
            r.invalid("include-other-group", value, "a list of policy group names");
        }
        for name in &names {
            if lookup(name) != Some(NameKind::Group) {
                r.error(
                    codes::E_UNKNOWN_GROUP_MEMBER,
                    format!("`include-other-group` references unknown policy group `{name}`"),
                );
            }
        }
        import.include_other_groups = names;
    }
    import
}

/// A URL when the value has a scheme, else a file. The value is never
/// echoed: a subscription URL usually carries a token (M3-D7).
fn policy_path(r: &mut Reader<'_>, value: &str, base_dir: &Path) -> Option<PolicyPath> {
    let value = value.trim();
    if value.is_empty() {
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            "`policy-path` is empty".to_string(),
        );
        return None;
    }
    if value.contains("://") {
        return match Url::parse(value) {
            Ok(url) if matches!(url.scheme(), "http" | "https") => {
                Some(PolicyPath::Url(Secret::new(url)))
            }
            _ => {
                r.error(
                    codes::E_INVALID_POLICY_PARAM,
                    "`policy-path` is not a valid http:// or https:// URL".to_string(),
                );
                None
            }
        };
    }
    let path = Path::new(value);
    Some(PolicyPath::File(if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    }))
}

/// `key=value` items the way `split_list` reads a quoted list; `None` when
/// the list is empty or an item is not a pair.
fn modifier(value: &str) -> Option<Vec<(String, String)>> {
    let items = split_list(value);
    if items.is_empty() {
        return None;
    }
    items
        .iter()
        .map(|item| parse_key_value(item).map(|(k, v)| (k.to_ascii_lowercase(), v.to_string())))
        .collect()
}

fn read_underlying(
    r: &mut Reader<'_>,
    lookup: &dyn Fn(&str) -> Option<NameKind>,
) -> Option<String> {
    let name = r.str("underlying-proxy")?.trim();
    match lookup(name) {
        None => {
            r.error(
                codes::E_UNKNOWN_POLICY_REF,
                format!("`underlying-proxy` references unknown policy `{name}`"),
            );
            None
        }
        // DIRECT is the absence of a chain
        Some(NameKind::Builtin(Builtin::Direct)) => None,
        Some(NameKind::Builtin(_)) => {
            r.invalid("underlying-proxy", name, "a proxy policy or a policy group");
            None
        }
        Some(NameKind::Policy(_) | NameKind::Group) => Some(name.to_string()),
    }
}

fn read_test(r: &mut Reader<'_>, kind: GroupKind) -> TestOpts {
    let mut test = TestOpts::default();
    // a select or subnet group never tests; a smart group tests every 5 minutes
    let tests = !matches!(kind, GroupKind::Select | GroupKind::Subnet);
    if tests && kind != GroupKind::Smart {
        if let Some(secs) = r.number("interval", true, "a positive number of seconds") {
            test.interval = Duration::from_secs(secs);
        }
    } else {
        r.not_applicable("interval");
    }
    if kind == GroupKind::UrlTest {
        if let Some(ms) = r.number("tolerance", false, "a number of milliseconds") {
            test.tolerance = Duration::from_millis(ms);
        }
    } else {
        r.not_applicable("tolerance");
    }
    if tests {
        test.timeout = r
            .number("timeout", false, "a number of seconds")
            .map(Duration::from_secs);
        test.evaluate_before_use = r.bool("evaluate-before-use").unwrap_or(false);
    } else {
        r.not_applicable("timeout");
        r.not_applicable("evaluate-before-use");
    }
    if kind == GroupKind::LoadBalance {
        test.persistent = r.bool("persistent").unwrap_or(false);
    } else {
        r.not_applicable("persistent");
    }
    test
}

/// `regex:factor` pairs separated by `;`; the factor follows the last `:`.
fn read_priority(r: &mut Reader<'_>) -> Vec<Priority> {
    let Some(value) = r.str("policy-priority") else {
        return Vec::new();
    };
    let parsed: Option<Vec<Priority>> = value
        .split(';')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| {
            let (source, factor) = item.rsplit_once(':')?;
            let factor: f64 = factor.trim().parse().ok()?;
            if !(factor.is_finite() && factor > 0.0) {
                return None;
            }
            let pattern = Pattern::new(source).ok()?;
            Some(Priority { pattern, factor })
        })
        .collect();
    match parsed {
        Some(pairs) if !pairs.is_empty() => pairs,
        _ => {
            r.invalid(
                "policy-priority",
                value,
                "`regex:factor` pairs separated by `;`, every factor above 0",
            );
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{PolicyKind, parse_group};
    use std::sync::Arc;

    fn span() -> Span {
        Span::new(Arc::from(Path::new("p.conf")), 4)
    }

    fn lookup(name: &str) -> Option<NameKind> {
        match name {
            "A" | "B" | "Relay" => Some(NameKind::Policy(PolicyKind::Socks5)),
            "g1" | "g2" | "Pick" => Some(NameKind::Group),
            "DIRECT" => Some(NameKind::Builtin(Builtin::Direct)),
            "REJECT" => Some(NameKind::Builtin(Builtin::Reject)),
            _ => None,
        }
    }

    fn outcome(def: &str) -> GroupOutcome {
        let group = parse_group("G", def, &span()).unwrap();
        to_group_spec(&group, Path::new("/profiles"), &lookup)
    }

    fn spec(def: &str) -> GroupSpec {
        let o = outcome(def);
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        o.spec.expect("no errors")
    }

    fn messages(def: &str) -> Vec<(&'static str, String)> {
        outcome(def)
            .diagnostics
            .into_iter()
            .map(|d| (d.code, d.message))
            .collect()
    }

    #[test]
    fn the_manual_examples_read_into_the_spec() {
        let s = spec("select, policy-path=proxies.txt");
        assert_eq!(
            s.import.policy_path,
            Some(PolicyPath::File(Path::new("/profiles").join("proxies.txt")))
        );
        assert!(s.members.is_empty());

        let s = spec(
            "select, A, policy-path=https://sub.test/nodes?token=t0k3n, update-interval=3600, \
policy-regex-filter=^HK, external-policy-name-prefix=Sub-, \
external-policy-modifier=\"test-url=http://apple.com/,tfo=true\", include-all-proxies=true, \
include-other-group=\"g1,g2\", underlying-proxy=Relay, hidden=true, no-alert=true, \
icon-url=https://example.com/i.png, category=Media",
        );
        assert_eq!(s.members, ["A"]);
        let Some(PolicyPath::Url(url)) = &s.import.policy_path else {
            panic!("{:?}", s.import.policy_path)
        };
        assert_eq!(url.expose().as_str(), "https://sub.test/nodes?token=t0k3n");
        assert_eq!(s.import.update_interval, Some(3600));
        assert_eq!(
            s.import.regex_filter.as_ref().map(|p| p.source.as_str()),
            Some("^HK")
        );
        assert_eq!(s.import.name_prefix.as_deref(), Some("Sub-"));
        assert_eq!(
            s.import.modifier.expose(),
            &[
                ("test-url".to_string(), "http://apple.com/".to_string()),
                ("tfo".to_string(), "true".to_string())
            ]
        );
        assert!(s.import.include_all_proxies);
        assert_eq!(s.import.include_other_groups, ["g1", "g2"]);
        assert_eq!(s.underlying_proxy.as_deref(), Some("Relay"));
        assert!(s.hidden);

        let s =
            spec("url-test, A, B, interval=300, tolerance=0, timeout=5, evaluate-before-use=true");
        assert_eq!(
            s.test,
            TestOpts {
                interval: Duration::from_secs(300),
                tolerance: Duration::ZERO,
                timeout: Some(Duration::from_secs(5)),
                evaluate_before_use: true,
                persistent: false,
            }
        );
        assert_eq!(spec("fallback, A, B").test, TestOpts::default());
        assert!(spec("load-balance, A, B, persistent=true").test.persistent);

        let s = spec("smart, A, B, policy-priority=\"Premium:0.9;Backup:1.3\"");
        let pairs: Vec<(&str, f64)> = s
            .priority
            .iter()
            .map(|p| (p.pattern.source.as_str(), p.factor))
            .collect();
        assert_eq!(pairs, [("Premium", 0.9), ("Backup", 1.3)]);
    }

    #[test]
    fn direct_as_the_relay_is_no_relay_at_all() {
        assert_eq!(
            spec("select, A, underlying-proxy=DIRECT").underlying_proxy,
            None
        );
    }

    /// A subscription URL is a credential: no diagnostic repeats it, and
    /// neither does `Debug` — of the path or of the modifier.
    #[test]
    fn a_bad_policy_path_is_an_error_that_never_echoes_it() {
        for def in [
            "select, policy-path=ftp://sub.test/nodes?token=t0k3n",
            "select, policy-path=https://?token=t0k3n",
            "select, policy-path=\"\"",
        ] {
            let found = messages(def);
            assert_eq!(found.len(), 1, "{def}: {found:?}");
            assert_eq!(found[0].0, codes::E_INVALID_POLICY_PARAM);
            assert!(!found[0].1.contains("t0k3n"), "{}", found[0].1);
        }
        let found = messages("select, external-policy-modifier=\"password=hunter2,tfo\"");
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(!found[0].1.contains("hunter2"), "{}", found[0].1);

        let s = spec(
            "select, policy-path=https://sub.test/n?token=t0k3n, external-policy-modifier=\"password=hunter2\"",
        );
        let debug = format!("{s:?}");
        assert!(
            !debug.contains("t0k3n") && !debug.contains("hunter2"),
            "{debug}"
        );
    }

    #[test]
    fn values_that_cannot_be_used_are_errors() {
        for (def, message) in [
            (
                "select, policy-path=a.txt, update-interval=0",
                "policy group `G`: invalid value `0` for `update-interval` (expected a positive number of seconds)",
            ),
            (
                "select, policy-regex-filter=(",
                "policy group `G`: invalid `policy-regex-filter` `(`: ",
            ),
            (
                "select, external-policy-name-prefix=a=b",
                "policy group `G`: invalid value `a=b` for `external-policy-name-prefix` (expected a prefix without `=`)",
            ),
            (
                "select, include-all-proxies=maybe",
                "policy group `G`: invalid value `maybe` for `include-all-proxies` (expected true or false)",
            ),
            (
                "url-test, A, interval=0",
                "policy group `G`: invalid value `0` for `interval` (expected a positive number of seconds)",
            ),
            (
                "url-test, A, tolerance=-1",
                "policy group `G`: invalid value `-1` for `tolerance` (expected a number of milliseconds)",
            ),
            (
                "fallback, A, timeout=soon",
                "policy group `G`: invalid value `soon` for `timeout` (expected a number of seconds)",
            ),
            (
                "smart, A, policy-priority=\"A:0\"",
                "policy group `G`: invalid value `A:0` for `policy-priority` (expected `regex:factor` pairs separated by `;`, every factor above 0)",
            ),
            (
                "smart, A, policy-priority=\"A:-1;B:1\"",
                "policy group `G`: invalid value `A:-1;B:1` for `policy-priority` (expected `regex:factor` pairs separated by `;`, every factor above 0)",
            ),
            (
                "smart, A, policy-priority=Premium",
                "policy group `G`: invalid value `Premium` for `policy-priority` (expected `regex:factor` pairs separated by `;`, every factor above 0)",
            ),
            (
                "smart, A, policy-priority=\"(:1\"",
                "policy group `G`: invalid value `(:1` for `policy-priority` (expected `regex:factor` pairs separated by `;`, every factor above 0)",
            ),
        ] {
            let o = outcome(def);
            assert!(o.spec.is_none(), "{def}");
            let found: Vec<&str> = o.diagnostics.iter().map(|d| d.message.as_str()).collect();
            assert_eq!(found.len(), 1, "{def}: {found:?}");
            assert!(found[0].starts_with(message), "{def}: {}", found[0]);
            assert_eq!(o.diagnostics[0].code, codes::E_INVALID_POLICY_PARAM);
        }
    }

    #[test]
    fn names_are_checked_against_the_profile() {
        assert_eq!(
            messages("select, A, underlying-proxy=Nope"),
            [(
                codes::E_UNKNOWN_POLICY_REF,
                "policy group `G`: `underlying-proxy` references unknown policy `Nope`".to_string()
            )]
        );
        assert_eq!(
            messages("select, A, underlying-proxy=REJECT"),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy group `G`: invalid value `REJECT` for `underlying-proxy` (expected a proxy policy or a policy group)".to_string()
            )]
        );
        // a proxy is not a group whose members could be taken
        assert_eq!(
            messages("select, include-other-group=\"g1, A, missing\""),
            [
                (
                    codes::E_UNKNOWN_GROUP_MEMBER,
                    "policy group `G`: `include-other-group` references unknown policy group `A`"
                        .to_string()
                ),
                (
                    codes::E_UNKNOWN_GROUP_MEMBER,
                    "policy group `G`: `include-other-group` references unknown policy group `missing`"
                        .to_string()
                ),
            ]
        );
    }

    #[test]
    fn parameters_a_group_type_ignores_are_warned_about() {
        for (def, key, kind) in [
            ("fallback, A, tolerance=10", "tolerance", "fallback"),
            ("url-test, A, persistent=true", "persistent", "url-test"),
            (
                "url-test, A, policy-priority=\"A:1\"",
                "policy-priority",
                "url-test",
            ),
            ("smart, A, interval=60", "interval", "smart"),
            ("select, A, interval=60", "interval", "select"),
            ("select, A, timeout=5", "timeout", "select"),
            (
                "select, A, evaluate-before-use=true",
                "evaluate-before-use",
                "select",
            ),
            (
                "subnet, default=A, policy-path=a.txt",
                "policy-path",
                "subnet",
            ),
            (
                "subnet, default=A, underlying-proxy=Relay",
                "underlying-proxy",
                "subnet",
            ),
        ] {
            let found = messages(def);
            assert_eq!(
                found,
                [(
                    codes::W_PARAM_NOT_APPLICABLE,
                    format!("policy group `G`: `{key}` has no effect on a `{kind}` group; ignored")
                )],
                "{def}"
            );
        }
        assert_eq!(
            messages("url-test, A, url=http://bing.com/, mystery=1, default=A"),
            [
                (
                    codes::W_VANISHED_KEY,
                    "policy group `G`: `url` has no effect in current versions; use the policy's `test-url` or `proxy-test-url`".to_string()
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy group `G`: unknown parameter `mystery` ignored".to_string()
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy group `G`: unknown parameter `default` ignored".to_string()
                ),
            ]
        );
    }

    /// Which network this is only arrives in phase 3: until then a subnet
    /// group stands for its `default`, and its user-interface parameters
    /// are no conditions.
    #[test]
    fn a_subnet_group_stands_for_its_default() {
        let group = parse_group(
            "G",
            "subnet, default = A, SSID:Home = DIRECT, category=Home, hidden=true",
            &span(),
        )
        .unwrap();
        assert_eq!(group.conditions.len(), 1);
        let o = to_group_spec(&group, Path::new("/profiles"), &lookup);
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        let s = o.spec.unwrap();
        assert_eq!(s.members, ["A"]);
        assert!(s.hidden);
    }
}
```

要点（评审时对照设计 4.2）：
- `policy-path`：含 `://` 按 URL 解析，只收 `http` / `https`；空值、别的 scheme、解析不了 → `E0018`，**文本不回显取值**；否则是文件，相对路径按主配置所在目录解析。
- `external-policy-modifier`：`split_list` 后每项必须是 `key=value`；不合格 → `E0018`，同样不回显。
- `include-other-group` 的每个名字必须是组（`lookup` 给 `NameKind::Group`），否则 `E0008`；组级 `underlying-proxy`：未知 `E0007`、`DIRECT` 等于没写、其它内置策略 `E0018`；绕回本组的 `E0019` 在 `config.rs` 里查（Step 9）。
- 适用性（`W0028`，值不读）：`tolerance` 只在 `url-test`；`persistent` 只在 `load-balance`；`policy-priority` 只在 `smart`；`interval` 不在 `select` / `subnet` / `smart`（手册：`smart` 固定 5 分钟）；`timeout` 与 `evaluate-before-use` 不在 `select` / `subnet`；装配类参数与 `underlying-proxy` 不在 `subnet`。`url=` → `W0006`；`no-alert` `icon-url` `category` 静默接受；其余未知参数 `W0001`。
- `subnet` 组的 `members` 是它的 `default`（P6）。

- [ ] **Step 7: `subnet` 组的已知参数不再被当成网络条件（P6）**

`crates/rurge-config/src/policy.rs`——把

```rust
const SUBNET_GROUP_PARAMS: &[&str] = &["default", "cellular", "hidden", "icon-url"];
```

换成

```rust
/// Every parameter a group line can carry. On a `subnet` group these stay
/// parameters; any other `key = value` item there is a network condition.
const GROUP_PARAMS: &[&str] = &[
    "default",
    "cellular",
    "hidden",
    "icon-url",
    "category",
    "no-alert",
    "url",
    "underlying-proxy",
    "policy-path",
    "update-interval",
    "policy-regex-filter",
    "external-policy-name-prefix",
    "external-policy-modifier",
    "include-all-proxies",
    "include-other-group",
    "interval",
    "tolerance",
    "timeout",
    "evaluate-before-use",
    "persistent",
    "policy-priority",
];
```

`crates/rurge-config/src/policy.rs`——把

```rust
                    && !SUBNET_GROUP_PARAMS.contains(&k.to_ascii_lowercase().as_str())
```

换成

```rust
                    && !GROUP_PARAMS.contains(&k.to_ascii_lowercase().as_str())
```

`group.rs` 里 `subnet` 的两条用例（`category` 不是条件、装配类参数报 `W0028`）依赖这一步，所以测试放在它之后跑。

Run: `cargo test -p rurge-config --lib spec::group` → 7 passed。

- [ ] **Step 8: `config.rs` 的用例先行**

`crates/rurge-config/src/config.rs`——把

```rust
        assert!(c.contains(&codes::E_UNKNOWN_GROUP_MEMBER));
        assert!(c.contains(&codes::E_GROUP_CYCLE));
        assert!(l.diagnostics.has_errors());
    }
```

换成

```rust
        assert!(c.contains(&codes::E_UNKNOWN_GROUP_MEMBER));
        assert!(c.contains(&codes::W_GROUP_CYCLE));
        assert!(l.diagnostics.has_errors());
    }

    /// Through members and through `include-other-group` alike; no longer
    /// an error (M3 design 5.6).
    #[test]
    fn a_group_cycle_is_a_warning() {
        let l = load_text(
            "[Proxy]\nA = direct\n[Proxy Group]\nG1 = select, G2, A\nG2 = select, G1\n\
G3 = select, A, include-other-group=G4\nG4 = select, A, include-other-group=G3\n[Rule]\nFINAL,G1\n",
        );
        assert!(
            !l.diagnostics.has_errors(),
            "{:?}",
            l.diagnostics.into_vec()
        );
        let found: Vec<&str> = l
            .diagnostics
            .iter()
            .filter(|d| d.code == codes::W_GROUP_CYCLE)
            .map(|d| d.message.as_str())
            .collect();
        assert_eq!(
            found,
            [
                "policy groups `G2` and `G1` form a cycle; they behave as REJECT",
                "policy groups `G4` and `G3` form a cycle; they behave as REJECT"
            ]
        );
        let names: Vec<&str> = l
            .config
            .group_specs
            .iter()
            .map(|g| g.name.as_str())
            .collect();
        assert_eq!(names, ["G1", "G2", "G3", "G4"]);
    }

    #[test]
    fn a_group_relay_that_leads_back_to_the_group_is_an_error() {
        let l = load_text(
            "[Proxy]\nA = http, a.test, 80\n[Proxy Group]\nG = select, A, underlying-proxy=Pick\n\
Pick = select, G, DIRECT\n[Rule]\nFINAL,G\n",
        );
        let errors: Vec<String> = l
            .diagnostics
            .iter()
            .filter(|d| d.severity == crate::Severity::Error)
            .map(|d| format!("{} {}", d.code, d.message))
            .collect();
        assert_eq!(
            errors,
            [
                "E0019 policy group `G`: `underlying-proxy` leads back to the group itself (via `Pick`)"
            ]
        );
    }

    #[test]
    fn a_relative_policy_path_is_resolved_against_the_main_profile() {
        let l =
            load_text("[Proxy Group]\nG = select, policy-path=sub/nodes.txt\n[Rule]\nFINAL,G\n");
        assert!(
            !l.diagnostics.has_errors(),
            "{:?}",
            l.diagnostics.into_vec()
        );
        assert_eq!(
            l.config.group_specs[0].import.policy_path,
            Some(crate::spec::PolicyPath::File(
                Path::new("/profiles").join("sub/nodes.txt")
            ))
        );
    }
```

Run: `cargo test -p rurge-config --lib config`

Expected: 编译错误——`no field `group_specs` on type `Config``。

- [ ] **Step 9: `config.rs` 的实现**

`crates/rurge-config/src/config.rs`——把

```rust
use crate::spec::{NameKind, PolicySpec, SpecEnv, to_spec};
```

换成

```rust
use crate::spec::{GroupSpec, NameKind, PolicySpec, SpecEnv, to_group_spec, to_spec};
```

`crates/rurge-config/src/config.rs`——把

```rust
use crate::value::split_definition;
```

换成

```rust
use crate::value::{split_definition, split_list};
```

`crates/rurge-config/src/config.rs`——把

```rust
    pub specs: Vec<PolicySpec>,
    pub groups: Vec<PolicyGroup>,
```

换成

```rust
    pub specs: Vec<PolicySpec>,
    pub groups: Vec<PolicyGroup>,
    /// Typed parameters of every group (same order as `groups`; groups
    /// with errors are absent).
    pub group_specs: Vec<GroupSpec>,
```

`crates/rurge-config/src/config.rs`——把

```rust
    pub fn spec(&self, name: &str) -> Option<&PolicySpec> {
        self.specs.iter().find(|s| s.name == name)
    }
```

换成

```rust
    pub fn spec(&self, name: &str) -> Option<&PolicySpec> {
        self.specs.iter().find(|s| s.name == name)
    }

    /// What `name` refers to, as a policy or group line sees it.
    pub fn name_kind(&self, name: &str) -> Option<NameKind> {
        Some(match self.resolve_policy(name)? {
            PolicyTarget::Builtin(b) => NameKind::Builtin(b),
            PolicyTarget::Proxy(p) => NameKind::Policy(p.kind),
            PolicyTarget::Group(_) => NameKind::Group,
        })
    }
```

`crates/rurge-config/src/config.rs`——把

```rust
        specs: Vec::new(),
        groups,
```

换成

```rust
        specs: Vec::new(),
        groups,
        group_specs: Vec::new(),
```

`crates/rurge-config/src/config.rs`——把

```rust
    validate(&mut config, opts, &mut diags);
```

换成

```rust
    validate(&mut config, base_dir, opts, &mut diags);
```

`crates/rurge-config/src/config.rs`——把

```rust
fn is_base64(text: &str) -> bool {
```

换成

```rust
/// `group_refs` plus the groups whose members `include-other-group` takes:
/// every edge a group cycle can run along.
fn group_edges(g: &PolicyGroup) -> Vec<String> {
    let mut edges = group_refs(g);
    if let Some(v) = g.params.get("include-other-group") {
        edges.extend(split_list(v));
    }
    edges
}

fn is_base64(text: &str) -> bool {
```

`crates/rurge-config/src/config.rs`——把

```rust
/// `E0019`: follows `underlying-proxy` edges and group membership from each
/// chained policy; reaching the policy again is a cycle.
fn underlying_cycles(config: &Config, specs: &[PolicySpec], diags: &mut Diagnostics) {
    let mut edges: HashMap<&str, Vec<String>> = HashMap::new();
    for s in specs {
        if let Some(u) = &s.common.underlying_proxy {
            edges.insert(s.name.as_str(), vec![u.clone()]);
        }
    }
    for g in &config.groups {
        edges.insert(g.name.as_str(), group_refs(g));
    }
    for s in specs {
        let Some(first) = &s.common.underlying_proxy else {
            continue;
        };
        let mut seen: HashSet<String> = HashSet::new();
        let mut stack = vec![first.clone()];
        let mut cyclic = false;
        while let Some(name) = stack.pop() {
            if name == s.name {
                cyclic = true;
                break;
            }
            if seen.insert(name.clone())
                && let Some(next) = edges.get(name.as_str())
            {
                stack.extend(next.iter().cloned());
            }
        }
        if cyclic {
            diags.push(
                Diagnostic::error(
                    codes::E_UNDERLYING_PROXY_CYCLE,
                    format!(
                        "policy `{}`: `underlying-proxy` leads back to the policy itself (via `{first}`)",
                        s.name
                    ),
                )
                .at(s.span.clone()),
            );
        }
    }
}
```

换成

```rust
/// `E0019`: follows `underlying-proxy` edges, group membership and
/// `include-other-group` from each chained policy and from each group with a
/// relay of its own; reaching the start again is a cycle.
fn underlying_cycles(
    config: &Config,
    specs: &[PolicySpec],
    groups: &[GroupSpec],
    diags: &mut Diagnostics,
) {
    let mut edges: HashMap<&str, Vec<String>> = HashMap::new();
    for s in specs {
        if let Some(u) = &s.common.underlying_proxy {
            edges.insert(s.name.as_str(), vec![u.clone()]);
        }
    }
    for g in &config.groups {
        let mut next = group_edges(g);
        // every proxy member of a group with a relay is dialled through it
        if let Some(relay) = groups
            .iter()
            .find(|s| s.name == g.name)
            .and_then(|s| s.underlying_proxy.clone())
        {
            next.push(relay);
        }
        edges.insert(g.name.as_str(), next);
    }
    let leads_back = |start: &str, first: &str| {
        let mut seen: HashSet<String> = HashSet::new();
        let mut stack = vec![first.to_string()];
        while let Some(name) = stack.pop() {
            if name == start {
                return true;
            }
            if seen.insert(name.clone())
                && let Some(next) = edges.get(name.as_str())
            {
                stack.extend(next.iter().cloned());
            }
        }
        false
    };
    for s in specs {
        if let Some(first) = &s.common.underlying_proxy
            && leads_back(&s.name, first)
        {
            diags.push(
                Diagnostic::error(
                    codes::E_UNDERLYING_PROXY_CYCLE,
                    format!(
                        "policy `{}`: `underlying-proxy` leads back to the policy itself (via `{first}`)",
                        s.name
                    ),
                )
                .at(s.span.clone()),
            );
        }
    }
    for g in groups {
        if let Some(first) = &g.underlying_proxy
            && leads_back(&g.name, first)
        {
            diags.push(
                Diagnostic::error(
                    codes::E_UNDERLYING_PROXY_CYCLE,
                    format!(
                        "policy group `{}`: `underlying-proxy` leads back to the group itself (via `{first}`)",
                        g.name
                    ),
                )
                .at(g.span.clone()),
            );
        }
    }
}
```

`crates/rurge-config/src/config.rs`——把

```rust
fn validate(config: &mut Config, opts: &LoadOptions, diags: &mut Diagnostics) {
```

换成

```rust
fn validate(config: &mut Config, base_dir: &Path, opts: &LoadOptions, diags: &mut Diagnostics) {
```

`crates/rurge-config/src/config.rs`——把

```rust
    // Group cycles (DFS with colours) over every reference kind.
```

换成

```rust
    // Group cycles (DFS with colours) over every reference kind: a warning,
    // the groups on one behave as REJECT (phase 2 M3 design 5.6).
```

`crates/rurge-config/src/config.rs`——把

```rust
        colour[i] = 1;
        for m in group_refs(&groups[i]) {
            if let Some(&j) = index.get(m.as_str()) {
                if colour[j] == 1 {
                    diags.push(
                        Diagnostic::error(
                            codes::E_GROUP_CYCLE,
                            format!(
                                "policy group `{}` and `{}` reference each other",
                                groups[i].name, groups[j].name
                            ),
                        )
```

换成

```rust
        colour[i] = 1;
        for m in group_edges(&groups[i]) {
            if let Some(&j) = index.get(m.as_str()) {
                if colour[j] == 1 {
                    diags.push(
                        Diagnostic::warning(
                            codes::W_GROUP_CYCLE,
                            format!(
                                "policy groups `{}` and `{}` form a cycle; they behave as REJECT",
                                groups[i].name, groups[j].name
                            ),
                        )
```

`crates/rurge-config/src/config.rs`——把

```rust
    // Typed policy parameters (phase 2 M1 design §4).
    let specs = {
        let cfg: &Config = config;
        let lookup = |name: &str| -> Option<NameKind> {
            Some(match cfg.resolve_policy(name)? {
                PolicyTarget::Builtin(b) => NameKind::Builtin(b),
                PolicyTarget::Proxy(p) => NameKind::Policy(p.kind),
                PolicyTarget::Group(_) => NameKind::Group,
            })
        };
```

换成

```rust
    // Typed policy and group parameters (phase 2 M1 design §4, M3 design §4).
    let (specs, group_specs) = {
        let cfg: &Config = config;
        let lookup = |name: &str| cfg.name_kind(name);
```

`crates/rurge-config/src/config.rs`——把

```rust
        underlying_cycles(cfg, &specs, diags);
        specs
    };
    config.specs = specs;
```

换成

```rust
        let mut group_specs = Vec::new();
        for g in &cfg.groups {
            let outcome = to_group_spec(g, base_dir, &lookup);
            for d in outcome.diagnostics {
                diags.push(d);
            }
            group_specs.extend(outcome.spec);
        }
        underlying_cycles(cfg, &specs, &group_specs, diags);
        (specs, group_specs)
    };
    config.specs = specs;
    config.group_specs = group_specs;
```

要点：
- `validate` 多一个参数 `base_dir`（主配置所在目录，`from_profile` 已有）。
- 组环的深度优先搜索改走 `group_edges`（成员、`subnet` 条件、`default`，再加 `include-other-group` 的每个名字），每条回边一条 `W0030`：`` policy groups `A` and `B` form a cycle; they behave as REJECT ``——**不再是错误**，配置照常加载。
- 策略的 `lookup` 抽成 `Config::name_kind`，策略与组的读取共用它；读完全部 `GroupSpec` 之后才跑 `underlying_cycles`，它现在也检查组：组的边加上它自己的中继。

Run: `cargo test -p rurge-config --lib` → 全部通过。

- [ ] **Step 10: 组环的语料从"无效"挪到"有效"**

```bash
git mv tests/corpus/invalid/group-cycle.conf tests/corpus/valid/group-cycle.conf
git rm tests/corpus/invalid/group-cycle.expect
INSTA_UPDATE=always cargo test -p rurge-config --test corpus
```

`INSTA_UPDATE=always` 会写出新快照；逐字核对它与下面完全一致（本机没有 `cargo-insta`，也不要安装）：

`crates/rurge-config/tests/snapshots/corpus__corpus__group-cycle.snap`：

```text
---
source: crates/rurge-config/tests/corpus.rs
expression: "(loaded.config.summary(), diags)"
---
- listeners: []
  policies: []
  groups:
    - "A (select) -> [B]"
    - "B (select) -> [A]"
  rules:
    - "FINAL,A"
  rulesets: []
  hosts: []
  keystore: []
  deferred: []
  unknown_sections: []
  managed: ~
- - "warning[W0030] valid/group-cycle.conf:3: policy groups `B` and `A` form a cycle; they behave as REJECT"
```

再跑一次不带环境变量的 `cargo test -p rurge-config --test corpus` → 2 passed（其余语料的快照不变：它们的组参数都合法）。

- [ ] **Step 11: 门禁与提交**

跑 Global Constraints 里的门禁。此时注册表仍按 `PolicyGroup.members` 构建：组环的配置能加载了，运行期由 `MAX_DEPTH` 兜住（Task 5 才换成"环上的组 REJECT"）。

```bash
git add -A crates/rurge-config Cargo.lock tests/corpus
git commit -m "feat(config): GroupSpec 与组参数校验；W0030 组环告警取代 E0009；组级 underlying-proxy 的 E0019；脱敏名单加 policy-path 与 external-policy-modifier"
```

---

### Task 2: 订阅文本解析——`rurge_policy::subscription`

订阅资源的内容 → 策略行。纯函数，不碰网络与磁盘；设计 5.2 与 P3。

**Files:**
- Create: `crates/rurge-policy/src/subscription.rs`
- Modify: `crates/rurge-policy/src/lib.rs`

**Interfaces:**
- Consumes: `rurge_config::text::{parse_str, strip_inline_comment, Origin}`、`rurge_config::value::split_definition`、`rurge_config::policy::{parse_policy, Builtin, ProxyPolicy}`、`codes::E_UNKNOWN_POLICY_TYPE`。
- Produces:
  - `rurge_policy::subscription::MAX_POLICIES: usize = 10_000`
  - `rurge_policy::subscription::SPAN_FILE: &str = "policy-path"`
  - `rurge_policy::subscription::Subscription { policies: Vec<ProxyPolicy>, skipped: Vec<(u32, String)>, truncated: bool }`：`Clone + Default`，**没有 `Debug`**
  - `rurge_policy::subscription::parse(text: &str) -> Subscription`

- [ ] **Step 1: 模块声明与用例先行**

`crates/rurge-policy/src/lib.rs`——把

```rust
//! Policy registry (M3 design §5, M1 design 6.1 – 6.3): resolves a `PolicyRef`
//! through aliases and groups to a concrete `Outbound`, recording the chain
//! it took; the factory trait real outbounds come from; the cell and the
//! selection table that outlive a config generation.
```

换成

```rust
//! Policy registry (M3 design §5, M1 design 6.1 – 6.3): resolves a `PolicyRef`
//! through aliases and groups to a concrete `Outbound`, recording the chain
//! it took; the factory trait real outbounds come from; the cell and the
//! selection table that outlive a config generation; what a `policy-path`
//! subscription holds (phase 2 M3 design 5.2).
```

`crates/rurge-policy/src/lib.rs`——把

```rust
pub mod selections;
```

换成

```rust
pub mod selections;
pub mod subscription;
```

新建 `crates/rurge-policy/src/subscription.rs`，先只写文件头的 `use` 与文件末尾的 `#[cfg(test)] mod tests { … }`（全文见 Step 2）。

Run: `cargo test -p rurge-policy subscription`

Expected: 编译错误——`cannot find function `parse``、`cannot find type `Subscription``。

- [ ] **Step 2: 写实现**

`crates/rurge-policy/src/subscription.rs` 全文：

```rust
//! What a `policy-path` resource holds (M3 design 5.2): Surge policy lines,
//! as a plain list or as the `[Proxy]` section of a whole profile. Parsing
//! never fails: a line that cannot be used is skipped and reported by its
//! number — its text never leaves this module, it may carry a credential.

use rurge_config::diagnostic::codes;
use rurge_config::policy::{Builtin, ProxyPolicy, parse_policy};
use rurge_config::span::Span;
use rurge_config::text::{Origin, parse_str, strip_inline_comment};
use rurge_config::value::split_definition;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

/// More policies than this in one subscription are dropped (M3 design §9).
pub const MAX_POLICIES: usize = 10_000;

/// What the span of an imported line names in place of a file: never the
/// subscription's URL.
pub const SPAN_FILE: &str = "policy-path";

/// A subscription's policies. No `Debug`: the lines carry credentials.
#[derive(Clone, Default)]
pub struct Subscription {
    /// In file order, names unique; each span holds the line number.
    pub policies: Vec<ProxyPolicy>,
    /// Line number and reason of every line that was skipped.
    pub skipped: Vec<(u32, String)>,
    /// There were more than `MAX_POLICIES` policies: the rest was dropped.
    pub truncated: bool,
}

pub fn parse(text: &str) -> Subscription {
    let file: Arc<Path> = Arc::from(Path::new(SPAN_FILE));
    let mut out = Subscription::default();
    let mut names: HashSet<String> = HashSet::new();
    for (line, raw) in lines(text, &file) {
        let Some((name, definition)) = split_definition(&raw) else {
            out.skipped
                .push((line, "not a policy line (`Name = type, ...`)".to_string()));
            continue;
        };
        if Builtin::parse(name).is_some() {
            out.skipped
                .push((line, format!("`{name}` is the name of a built-in policy")));
            continue;
        }
        let policy = match parse_policy(name, definition, &Span::new(file.clone(), line)) {
            Ok(policy) => policy,
            Err(e) => {
                out.skipped.push((line, reason(e.code)));
                continue;
            }
        };
        if !names.insert(policy.name.clone()) {
            out.skipped.push((
                line,
                format!(
                    "duplicate policy name `{}`; the first one is kept",
                    policy.name
                ),
            ));
            continue;
        }
        if out.policies.len() == MAX_POLICIES {
            out.truncated = true;
            break;
        }
        out.policies.push(policy);
    }
    out
}

/// Why `parse_policy` refused a line, with nothing of the line in it: its
/// own messages quote the type and the port, and in a line that is not a
/// policy at all (a `vmess://` link) those are pieces of a credential.
fn reason(code: &str) -> String {
    if code == codes::E_UNKNOWN_POLICY_TYPE {
        "unknown policy type".to_string()
    } else {
        "not a valid policy line".to_string()
    }
}

/// Line number and content of every line that may hold a policy: the
/// `[Proxy]` section's, by the profile parser's own rules, when the text has
/// one; else every line that is neither blank nor a comment. Nothing is ever
/// included from elsewhere: a `#!include` line is just a line that is no
/// policy.
fn lines(text: &str, file: &Arc<Path>) -> Vec<(u32, String)> {
    let (profile, _) = parse_str(text, file.clone(), Origin::Main);
    if let Some(section) = profile.section("Proxy") {
        return section
            .active_entries()
            .map(|e| (e.span.line, e.raw.clone()))
            .collect();
    }
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    text.lines()
        .enumerate()
        .filter_map(|(i, line)| {
            let line = line.trim();
            if line.is_empty()
                || line.starts_with('#')
                || line.starts_with("//")
                || line.starts_with(';')
            {
                return None;
            }
            let content = strip_inline_comment(line);
            (!content.is_empty()).then(|| (i as u32 + 1, content.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::PolicyKind;

    fn names(sub: &Subscription) -> Vec<(&str, u32)> {
        sub.policies
            .iter()
            .map(|p| (p.name.as_str(), p.span.line))
            .collect()
    }

    #[test]
    fn a_plain_list_is_read_line_by_line() {
        let sub = parse(
            "\u{FEFF}#!name=Nodes\r\n\r\n# a comment\r\n// another\r\n; and another\r\n\
HK-1 = http, hk.test, 8080 // inline\r\nUS-1 = trojan, us.test, 443, password=pw\r\n",
        );
        assert_eq!(names(&sub), [("HK-1", 6), ("US-1", 7)]);
        assert!(sub.skipped.is_empty(), "{:?}", sub.skipped);
        assert_eq!(sub.policies[0].kind, PolicyKind::Http);
        assert_eq!(sub.policies[0].definition, "http, hk.test, 8080");
        assert_eq!(sub.policies[0].span.file.as_ref(), Path::new(SPAN_FILE));
        assert!(!sub.truncated);
    }

    #[test]
    fn a_whole_profile_gives_its_proxy_section_only() {
        let sub = parse(
            "[General]\nloglevel = notify\n[Proxy]\nA = http, a.test, 80\n# gone\nB = socks5, b.test, 1080\n\
[Proxy Group]\nG = select, A, B\n[Rule]\nFINAL,G\n",
        );
        assert_eq!(names(&sub), [("A", 4), ("B", 6)]);
        assert!(sub.skipped.is_empty(), "{:?}", sub.skipped);
    }

    /// The reasons name the problem, never a piece of the line: a
    /// `vmess://` link or a stray password must not reach the logs.
    #[test]
    fn a_line_that_cannot_be_used_is_skipped_by_number_without_its_text() {
        let text = "vmess://eyJpZCI6IjAyMzNkMTFjLTE1YTQtNDdkMy1hZGUzLTQ4ZmZjYTBjZTExOSJ9=\n\
Odd = vless, odd.test, 443\nPort = http, p.test, s3cretport\nDIRECT = direct\nA = http, a.test, 80\n\
A = http, other.test, 80\nnot a policy\n";
        let sub = parse(text);
        assert_eq!(names(&sub), [("A", 5)]);
        assert_eq!(
            sub.skipped,
            [
                (1, "not a valid policy line".to_string()),
                (2, "unknown policy type".to_string()),
                (3, "not a valid policy line".to_string()),
                (4, "`DIRECT` is the name of a built-in policy".to_string()),
                (
                    6,
                    "duplicate policy name `A`; the first one is kept".to_string()
                ),
                (7, "not a policy line (`Name = type, ...`)".to_string()),
            ]
        );
        for (_, reason) in &sub.skipped {
            for piece in ["eyJ", "vless", "s3cretport"] {
                assert!(!reason.contains(piece), "{reason}");
            }
        }
    }

    /// A Clash or base64 subscription holds no Surge line: nothing is
    /// imported, and the caller warns (M3-D2).
    #[test]
    fn a_subscription_in_another_format_yields_nothing() {
        for text in [
            "proxies:\n  - name: \"hk\"\n    type: ss\n    server: hk.test\n",
            "c3M6Ly9ZV1Z6TFRJMU5pMW5ZMjA2Y0hjPUBoay50ZXN0Ojg0NDM=\n",
        ] {
            let sub = parse(text);
            assert!(sub.policies.is_empty());
            assert!(!sub.skipped.is_empty());
        }
        assert!(parse("").policies.is_empty());
    }

    /// Subscription content is somebody else's text: it can never make
    /// rurge read a local file.
    #[test]
    fn an_include_line_is_never_followed() {
        let sub = parse("[Proxy]\n#!include /etc/passwd\nA = http, a.test, 80\n");
        assert_eq!(names(&sub), [("A", 3)]);
        assert_eq!(
            sub.skipped,
            [(2, "not a policy line (`Name = type, ...`)".to_string())]
        );
    }

    #[test]
    fn more_than_the_limit_is_dropped() {
        let text: String = (0..MAX_POLICIES + 5)
            .map(|i| format!("N{i} = http, n{i}.test, 80\n"))
            .collect();
        let sub = parse(&text);
        assert_eq!(sub.policies.len(), MAX_POLICIES);
        assert!(sub.truncated);
        assert_eq!(
            sub.policies[MAX_POLICIES - 1].name,
            format!("N{}", MAX_POLICIES - 1)
        );
    }
}
```

要点：
- 有 `[Proxy]` 节就只取它（配置解析器的节识别）；没有就逐行读，跳过空行与 `#` `//` `;` 开头的行，行内注释照配置解析器去掉。
- 每行一个 `(行号, 原因)`，原因只有五种固定文本，**不引用行内任何片段**；内置策略名、重名是可以说出来的（名字本身不是凭据），`parse_policy` 的错误一律归成两种笼统的原因。
- 超过 10 000 条时截断并置 `truncated`；一条都没读出来时由装配（Task 3）告警。

- [ ] **Step 3: 运行**

Run: `cargo test -p rurge-policy subscription` → 6 passed（`a_plain_list_is_read_line_by_line`、`a_whole_profile_gives_its_proxy_section_only`、`a_line_that_cannot_be_used_is_skipped_by_number_without_its_text`、`a_subscription_in_another_format_yields_nothing`、`an_include_line_is_never_followed`、`more_than_the_limit_is_dropped`）。

- [ ] **Step 4: 门禁与提交**

```bash
git add crates/rurge-policy/src/subscription.rs crates/rurge-policy/src/lib.rs
git commit -m "feat(policy): 订阅文本解析——Surge 策略行列表或 [Proxy] 节，坏行只报行号与原因"
```

---


### Task 3: 成员装配——`rurge_policy::assemble` 与 `with_params`

一个组的成员 = 写在组行上的 → `include-other-group` 取来的（递归、装配后）→ `include-all-proxies` 取来的 → `policy-path` 导入的；重名保留第一个。导入行按"过滤 → 前缀 → 修饰"处理，再过全局命名空间（配置优先，两个组之间先声明的优先）、`to_spec`、中继成环检查；最后算出组环。纯函数，设计 5.3 与 P4、P8、P12。派生（组级中继）在 Task 4。

**Files:**
- Create: `crates/rurge-policy/src/assemble.rs`
- Modify: `crates/rurge-config/src/policy.rs`（`with_params` 与用例）
- Modify: `crates/rurge-config/src/redact.rs`（`split_top_level` 的可见性）
- Modify: `crates/rurge-config/src/diagnostic.rs`（`W0023` / `W0024` 的注释）
- Modify: `crates/rurge-policy/src/lib.rs`

**Interfaces:**
- Consumes: Task 1 的 `GroupSpec` / `ImportOpts` / `PolicyPath`、`Config::name_kind`、`Config.group_specs`；Task 2 的 `Subscription` / `MAX_POLICIES`；`rurge_config::spec::{to_spec, SpecEnv, NameKind, PolicySpec}`（`SpecOutcome` 的 `spec` / `diagnostics` / `legacy_vmess`）。
- Produces:
  - `rurge_config::policy::with_params(definition: &str, overrides: &[(String, String)]) -> String`
  - `rurge_policy::Snapshots = HashMap<PolicyPath, Arc<Subscription>>`（缺席的来源 = 还没有内容）
  - `rurge_policy::assemble::Imported { policy: ProxyPolicy, spec: Option<PolicySpec> }`：`Clone`，没有 `Debug`
  - `rurge_policy::Assembly { members: HashMap<String, Vec<String>>, imported: Vec<Imported>, cycles: Vec<Vec<String>>, diagnostics: Diagnostics }`：`Clone + Default`，没有 `Debug`；`Assembly::members_of(&self, group: &str) -> &[String]`
  - `rurge_policy::assemble(cfg: &Config, snapshots: &Snapshots) -> Assembly`

- [ ] **Step 1: `with_params` 的用例先行**

`crates/rurge-config/src/policy.rs`——把

```rust
        let g = parse_group("Pick", " select, Up, DIRECT, hidden=true", &span()).unwrap();
        assert_eq!(g.definition, "select, Up, DIRECT, hidden=true");
    }
```

换成

```rust
        let g = parse_group("Pick", " select, Up, DIRECT, hidden=true", &span()).unwrap();
        assert_eq!(g.definition, "select, Up, DIRECT, hidden=true");
    }

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn with_params_overrides_in_place_and_appends_the_rest() {
        assert_eq!(
            with_params(
                "trojan, t.test, 443, password=pw, TFO=false, sni=a.test, tfo=false",
                &pairs(&[("tfo", "true"), ("test-url", "http://apple.com/")])
            ),
            "trojan, t.test, 443, password=pw, tfo=true, sni=a.test, test-url=http://apple.com/"
        );
        assert_eq!(with_params("http, h.test, 80", &[]), "http, h.test, 80");
    }

    /// A value the list splitter would act on is quoted, and the line reads
    /// back with exactly the values that were set.
    #[test]
    fn a_value_set_by_with_params_reads_back_unchanged() {
        let line = with_params(
            "http, h.test, 80, alice, s3cret",
            &pairs(&[
                ("password", "a,b\"c\\d"),
                ("headers", "X-A:1|X-B:(2)"),
                ("sni", ""),
            ]),
        );
        let p = parse_policy("P", &line, &span()).unwrap();
        assert_eq!(p.params.get("password"), Some("a,b\"c\\d"));
        assert_eq!(p.params.get("headers"), Some("X-A:1|X-B:(2)"));
        assert_eq!(p.params.get("sni"), Some(""));
        assert_eq!(p.positional, ["alice", "s3cret"]);
    }
```

Run: `cargo test -p rurge-config --lib policy`

Expected: 编译错误——`cannot find function `with_params``。

- [ ] **Step 2: 写 `with_params`**

`crates/rurge-config/src/redact.rs`——把

```rust
fn split_top_level(value: &str) -> Vec<&str> {
```

换成

```rust
pub(crate) fn split_top_level(value: &str) -> Vec<&str> {
```

`crates/rurge-config/src/policy.rs`——把

```rust
use crate::value::{ParamMap, parse_key_value, split_list, strip_prefix_ci};
use std::net::IpAddr;
```

换成

```rust
use crate::value::{ParamMap, parse_key_value, split_list, strip_prefix_ci};
use std::collections::HashSet;
use std::net::IpAddr;
```

`crates/rurge-config/src/policy.rs`——把

```rust

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GroupKind {
```

换成

```rust

/// `definition` with the parameters of `overrides` in place (phase 2 M3
/// design 5.3, `external-policy-modifier`): a key the line has is replaced
/// where it first appears and dropped where it repeats, a key it lacks is
/// appended. Keys compare case-insensitively; every other item keeps its
/// text.
pub fn with_params(definition: &str, overrides: &[(String, String)]) -> String {
    let mut written: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for item in crate::redact::split_top_level(definition) {
        let key = parse_key_value(item.trim()).map(|(k, _)| k.to_ascii_lowercase());
        let found = key
            .as_ref()
            .and_then(|k| overrides.iter().find(|(o, _)| o.eq_ignore_ascii_case(k)));
        match found {
            Some((k, v)) => {
                if written.insert(k.to_ascii_lowercase()) {
                    let lead = &item[..item.len() - item.trim_start().len()];
                    out.push(format!("{lead}{k}={}", param_value(v)));
                }
            }
            None => out.push(item.to_string()),
        }
    }
    for (k, v) in overrides {
        if written.insert(k.to_ascii_lowercase()) {
            out.push(format!(" {k}={}", param_value(v)));
        }
    }
    out.join(",")
}

/// `value` as a line has to spell it to read it back the same: quoted when
/// the list splitter would act on anything in it.
fn param_value(value: &str) -> String {
    let plain = !value.is_empty()
        && value.trim() == value
        && !value.contains([',', '"', '\'', '(', ')', '\\']);
    if plain {
        value.to_string()
    } else {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GroupKind {
```

Run: `cargo test -p rurge-config --lib policy` → 全部通过（新增 `with_params_overrides_in_place_and_appends_the_rest`、`a_value_set_by_with_params_reads_back_unchanged`）。

- [ ] **Step 3: 诊断码注释与模块声明**

`crates/rurge-config/src/diagnostic.rs`——把

```rust
    /// A set file contained lines that were skipped.
```

换成

```rust
    /// A set file or a `policy-path` subscription contained lines that were skipped.
```

`crates/rurge-config/src/diagnostic.rs`——把

```rust
    /// A set file exceeded MAX_ENTRIES and was truncated.
```

换成

```rust
    /// A set file or a `policy-path` subscription exceeded its limit and was truncated.
```

`crates/rurge-policy/src/lib.rs`——把

```rust
//! selection table that outlive a config generation; what a `policy-path`
//! subscription holds (phase 2 M3 design 5.2).
```

换成

```rust
//! selection table that outlive a config generation; what a `policy-path`
//! subscription holds and the members a group assembles from everything it
//! takes in (phase 2 M3 design 5.2, 5.3).
```

`crates/rurge-policy/src/lib.rs`——把

```rust
pub mod cell;
```

换成

```rust
pub mod assemble;
pub mod cell;
```

`crates/rurge-policy/src/lib.rs`——把

```rust
pub use cell::{ChainConnector, RegistryCell};
```

换成

```rust
pub use assemble::{Assembly, Snapshots, assemble};
pub use cell::{ChainConnector, RegistryCell};
```

- [ ] **Step 4: `assemble.rs` 的用例先行**

新建 `crates/rurge-policy/src/assemble.rs`，先只写文件头的 `use` 与文件末尾的 `#[cfg(test)] mod tests { … }`（全文见 Step 5）。

Run: `cargo test -p rurge-policy assemble`

Expected: 编译错误——`cannot find function `assemble``、`cannot find type `Snapshots`` 等。

- [ ] **Step 5: 写实现**

`crates/rurge-policy/src/assemble.rs` 全文（Task 3 的版本；Task 4 在它上面加派生）：

```rust
//! A group's members once everything it takes in is added (M3 design 5.3):
//! the members written on its line, those of the groups `include-other-group`
//! names, the profile's proxies (`include-all-proxies`) and the policies of
//! its `policy-path`. Pure: no network, no disk.

use crate::subscription::{MAX_POLICIES, Subscription};
use rurge_config::Config;
use rurge_config::diagnostic::{Diagnostic, Diagnostics, Severity, codes};
use rurge_config::policy::{PolicyKind, ProxyPolicy, parse_policy, with_params};
use rurge_config::spec::{GroupSpec, NameKind, PolicyPath, PolicySpec, SpecEnv, to_spec};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// The current content of every subscription; a source that is absent has
/// not been downloaded yet.
pub type Snapshots = HashMap<PolicyPath, Arc<Subscription>>;

/// A policy a group took in through `policy-path`.
#[derive(Clone)]
pub struct Imported {
    /// Named with the group's prefix, parameters overridden by its modifier.
    pub policy: ProxyPolicy,
    /// `None`: a protocol this version does not implement; it behaves as
    /// REJECT.
    pub spec: Option<PolicySpec>,
}

/// No `Debug`: imported policies carry credentials.
#[derive(Clone, Default)]
pub struct Assembly {
    /// Every group's members, by group name.
    pub members: HashMap<String, Vec<String>>,
    /// Names unique, none of them a name of the profile.
    pub imported: Vec<Imported>,
    /// Every group cycle, through members or `include-other-group`: the
    /// groups along it, the first one repeated at the end.
    pub cycles: Vec<Vec<String>>,
    /// Warnings, each at the line of the group it concerns.
    pub diagnostics: Diagnostics,
}

impl Assembly {
    /// The members of `group`; none for a name that is not a group.
    pub fn members_of(&self, group: &str) -> &[String] {
        self.members.get(group).map(Vec::as_slice).unwrap_or(&[])
    }
}

pub fn assemble(cfg: &Config, snapshots: &Snapshots) -> Assembly {
    let mut diagnostics = Diagnostics::default();
    let mut imports = Imports::collect(cfg, snapshots, &mut diagnostics);
    imports.read_specs(cfg, &mut diagnostics);
    let mut members = members(cfg, &imports);
    imports.drop_chain_cycles(cfg, &mut members, &mut diagnostics);
    let cycles = group_cycles(cfg, &members);
    Assembly {
        members,
        imported: imports.list.into_iter().map(|(i, _)| i).collect(),
        cycles,
        diagnostics,
    }
}

/// Warning `code` at `group`'s line.
fn warn(group: &GroupSpec, code: &'static str, message: String) -> Diagnostic {
    Diagnostic::warning(code, format!("policy group `{}`: {message}", group.name))
        .at(group.span.clone())
}

/// Whether `name` passes `group`'s `policy-regex-filter`.
fn passes(group: &GroupSpec, name: &str) -> bool {
    group
        .import
        .regex_filter
        .as_ref()
        .is_none_or(|p| p.regex.is_match(name).unwrap_or(false))
}

/// What parsing `sub` left out; said once per source, by the first group.
fn report(group: &GroupSpec, sub: &Subscription, diags: &mut Diagnostics) {
    for (line, reason) in &sub.skipped {
        diags.push(warn(
            group,
            codes::W_SET_LINES_SKIPPED,
            format!("`policy-path` line {line} skipped: {reason}"),
        ));
    }
    if sub.truncated {
        diags.push(warn(
            group,
            codes::W_SET_TRUNCATED,
            format!("`policy-path` holds more than {MAX_POLICIES} policies; the rest are ignored"),
        ));
    }
    if sub.policies.is_empty() {
        diags.push(warn(
            group,
            codes::W_SET_LINES_SKIPPED,
            "`policy-path` holds no policy; the content may not be in Surge format (policy lines, or a profile with a `[Proxy]` section)".to_string(),
        ));
    }
}

/// The policies groups took in, each with the group that brought it.
struct Imports<'a> {
    /// In import order.
    list: Vec<(Imported, &'a GroupSpec)>,
    /// What each group took in, by group name, in file order.
    by_group: HashMap<&'a str, Vec<String>>,
}

impl<'a> Imports<'a> {
    /// Filter → prefix → modifier (the manual's order), then the global
    /// namespace: the profile's names win, and between two groups the copy
    /// of the one declared first.
    fn collect(cfg: &'a Config, snapshots: &Snapshots, diags: &mut Diagnostics) -> Imports<'a> {
        let mut out = Imports {
            list: Vec::new(),
            by_group: HashMap::new(),
        };
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut reported: HashSet<&PolicyPath> = HashSet::new();
        for g in &cfg.group_specs {
            let Some(path) = &g.import.policy_path else {
                continue;
            };
            let Some(sub) = snapshots.get(path) else {
                diags.push(warn(
                    g,
                    codes::W_RESOURCE_UNAVAILABLE,
                    "`policy-path` has no content yet (never downloaded, or the file cannot be read); its imported members are unknown".to_string(),
                ));
                continue;
            };
            if reported.insert(path) {
                report(g, sub, diags);
            }
            let names = out.by_group.entry(g.name.as_str()).or_default();
            let prefix = g.import.name_prefix.as_deref().unwrap_or("");
            let modifier = g.import.modifier.expose();
            for p in &sub.policies {
                if !passes(g, &p.name) {
                    continue;
                }
                let line = p.span.line;
                let name = format!("{prefix}{}", p.name);
                let definition = if modifier.is_empty() {
                    p.definition.clone()
                } else {
                    with_params(&p.definition, modifier)
                };
                let Ok(policy) = parse_policy(&name, &definition, &p.span) else {
                    diags.push(warn(
                        g,
                        codes::W_SET_LINES_SKIPPED,
                        format!("`policy-path` line {line}: the line is no valid policy once modified; skipped"),
                    ));
                    continue;
                };
                if cfg.name_kind(&name).is_some() {
                    diags.push(warn(
                        g,
                        codes::W_SET_LINES_SKIPPED,
                        format!("`policy-path` line {line}: `{name}` is already a name of the profile; skipped"),
                    ));
                    continue;
                }
                match index.get(&name) {
                    None => {
                        index.insert(name.clone(), out.list.len());
                        out.list.push((Imported { policy, spec: None }, g));
                        names.push(name);
                    }
                    // the same line through another group: one policy
                    Some(&i) if out.list[i].0.policy.definition == policy.definition => {
                        names.push(name)
                    }
                    Some(&i) => diags.push(warn(
                        g,
                        codes::W_SET_LINES_SKIPPED,
                        format!(
                            "`policy-path` line {line}: `{name}` is already imported by `{}` with another definition; skipped",
                            out.list[i].1.name
                        ),
                    )),
                }
            }
        }
        out
    }

    /// The typed parameters of every import. A line with an error is
    /// skipped (M3-D6); one of a protocol this version does not implement
    /// stays, as REJECT, which is said once per protocol.
    fn read_specs(&mut self, cfg: &Config, diags: &mut Diagnostics) {
        let kinds: HashMap<String, PolicyKind> = self
            .list
            .iter()
            .map(|(i, _)| (i.policy.name.clone(), i.policy.kind))
            .collect();
        let lookup = |name: &str| {
            cfg.name_kind(name)
                .or_else(|| kinds.get(name).copied().map(NameKind::Policy))
        };
        let env = SpecEnv {
            keystore: &cfg.keystore,
            lookup: &lookup,
        };
        let mut failed: HashSet<String> = HashSet::new();
        let mut said: HashSet<String> = HashSet::new();
        for (imported, group) in &mut self.list {
            let group: &GroupSpec = group;
            let outcome = to_spec(&imported.policy, &env);
            if let Some(e) = outcome
                .diagnostics
                .iter()
                .find(|d| d.severity == Severity::Error)
            {
                diags.push(warn(
                    group,
                    codes::W_SET_LINES_SKIPPED,
                    format!(
                        "`policy-path` line {}: {}; skipped",
                        imported.policy.span.line, e.message
                    ),
                ));
                failed.insert(imported.policy.name.clone());
                continue;
            }
            imported.spec = outcome.spec;
            if imported.spec.is_none() {
                let what = if outcome.legacy_vmess {
                    "`vmess` without `vmess-aead=true` (the legacy handshake)".to_string()
                } else {
                    format!("`{}`", imported.policy.kind.keyword())
                };
                if said.insert(what.clone()) {
                    diags.push(warn(
                        group,
                        codes::W_PROTOCOL_NOT_IMPLEMENTED,
                        format!(
                            "imported policies of type {what} are not implemented in this version; they behave as REJECT"
                        ),
                    ));
                }
            }
        }
        self.forget(&failed);
    }

    /// An import whose `underlying-proxy` leads back to itself would never
    /// finish dialling: it is skipped, and so is every membership of it.
    fn drop_chain_cycles(
        &mut self,
        cfg: &Config,
        members: &mut HashMap<String, Vec<String>>,
        diags: &mut Diagnostics,
    ) {
        let mut edges: HashMap<&str, Vec<&str>> = HashMap::new();
        let imported = self.list.iter().filter_map(|(i, _)| i.spec.as_ref());
        for s in cfg.specs.iter().chain(imported) {
            if let Some(under) = &s.common.underlying_proxy {
                edges.insert(&s.name, vec![under]);
            }
        }
        for g in &cfg.group_specs {
            let mut next: Vec<&str> = members
                .get(&g.name)
                .map(|m| m.iter().map(String::as_str).collect())
                .unwrap_or_default();
            // every proxy member of a group with a relay is dialled through it
            next.extend(g.underlying_proxy.as_deref());
            edges.insert(&g.name, next);
        }
        let mut cyclic: HashSet<String> = HashSet::new();
        for (imported, group) in &self.list {
            let Some(first) = imported
                .spec
                .as_ref()
                .and_then(|s| s.common.underlying_proxy.as_deref())
            else {
                continue;
            };
            if leads_back(&edges, &imported.policy.name, first) {
                diags.push(warn(
                    group,
                    codes::W_SET_LINES_SKIPPED,
                    format!(
                        "`policy-path` line {}: the `underlying-proxy` of `{}` leads back to the policy itself; skipped",
                        imported.policy.span.line, imported.policy.name
                    ),
                ));
                cyclic.insert(imported.policy.name.clone());
            }
        }
        self.forget(&cyclic);
        for list in members.values_mut() {
            list.retain(|name| !cyclic.contains(name));
        }
    }

    fn forget(&mut self, names: &HashSet<String>) {
        if names.is_empty() {
            return;
        }
        self.list.retain(|(i, _)| !names.contains(&i.policy.name));
        for list in self.by_group.values_mut() {
            list.retain(|name| !names.contains(name));
        }
    }
}

/// Whether following `edges` from `first` reaches `start`.
fn leads_back(edges: &HashMap<&str, Vec<&str>>, start: &str, first: &str) -> bool {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut stack = vec![first];
    while let Some(name) = stack.pop() {
        if name == start {
            return true;
        }
        if seen.insert(name)
            && let Some(next) = edges.get(name)
        {
            stack.extend(next.iter().copied());
        }
    }
    false
}

/// A member list that keeps each name where it first appears.
#[derive(Default)]
struct Members {
    list: Vec<String>,
    seen: HashSet<String>,
}

impl Members {
    fn add(&mut self, name: &str) {
        if self.seen.insert(name.to_string()) {
            self.list.push(name.to_string());
        }
    }
}

/// Every group's members in the manual's order — written, then
/// `include-other-group`, then `include-all-proxies`, then `policy-path`.
fn members(cfg: &Config, imports: &Imports<'_>) -> HashMap<String, Vec<String>> {
    let expand = Expand {
        cfg,
        imports,
        groups: cfg
            .group_specs
            .iter()
            .map(|g| (g.name.as_str(), g))
            .collect(),
        // a group on an `include-other-group` cycle gives its members to
        // nobody: the cycle would have no end (5.3)
        on_cycle: cycles(cfg, &|g| g.import.include_other_groups.clone())
            .into_iter()
            .flatten()
            .collect(),
    };
    let mut done: HashMap<String, Vec<String>> = HashMap::new();
    for g in &cfg.group_specs {
        expand.group(g, &mut done);
    }
    done
}

struct Expand<'a> {
    cfg: &'a Config,
    imports: &'a Imports<'a>,
    groups: HashMap<&'a str, &'a GroupSpec>,
    on_cycle: HashSet<String>,
}

impl Expand<'_> {
    fn group(&self, g: &GroupSpec, done: &mut HashMap<String, Vec<String>>) -> Vec<String> {
        if let Some(members) = done.get(&g.name) {
            return members.clone();
        }
        let mut out = Members::default();
        for m in &g.members {
            out.add(m);
        }
        for name in &g.import.include_other_groups {
            if self.on_cycle.contains(name) {
                continue;
            }
            let Some(other) = self.groups.get(name.as_str()) else {
                continue;
            };
            for m in self.group(other, done) {
                if passes(g, &m) {
                    out.add(&m);
                }
            }
        }
        if g.import.include_all_proxies {
            for p in &self.cfg.policies {
                if !p.kind.is_builtin_alias() && passes(g, &p.name) {
                    out.add(&p.name);
                }
            }
        }
        // filtered on their names before the prefix, when they were taken in
        for name in self
            .imports
            .by_group
            .get(g.name.as_str())
            .into_iter()
            .flatten()
        {
            out.add(name);
        }
        done.insert(g.name.clone(), out.list.clone());
        out.list
    }
}

/// Group cycles through members (as assembled) and `include-other-group`.
fn group_cycles(cfg: &Config, members: &HashMap<String, Vec<String>>) -> Vec<Vec<String>> {
    cycles(cfg, &|g| {
        let mut next = members.get(&g.name).cloned().unwrap_or_default();
        next.extend(g.import.include_other_groups.iter().cloned());
        next
    })
}

/// Every cycle a depth-first walk along `edges` meets, as the groups along
/// it with the first repeated at the end. Every group on some cycle is on
/// one of these.
fn cycles(cfg: &Config, edges: &dyn Fn(&GroupSpec) -> Vec<String>) -> Vec<Vec<String>> {
    let mut walk = Walk {
        specs: &cfg.group_specs,
        index: cfg
            .group_specs
            .iter()
            .enumerate()
            .map(|(i, g)| (g.name.as_str(), i))
            .collect(),
        edges,
        colour: vec![Colour::New; cfg.group_specs.len()],
        stack: Vec::new(),
        found: Vec::new(),
    };
    for i in 0..cfg.group_specs.len() {
        if walk.colour[i] == Colour::New {
            walk.visit(i);
        }
    }
    walk.found
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Colour {
    New,
    OnStack,
    Done,
}

struct Walk<'a> {
    specs: &'a [GroupSpec],
    index: HashMap<&'a str, usize>,
    edges: &'a dyn Fn(&GroupSpec) -> Vec<String>,
    colour: Vec<Colour>,
    stack: Vec<usize>,
    found: Vec<Vec<String>>,
}

impl Walk<'_> {
    fn visit(&mut self, i: usize) {
        self.colour[i] = Colour::OnStack;
        self.stack.push(i);
        for next in (self.edges)(&self.specs[i]) {
            let Some(&j) = self.index.get(next.as_str()) else {
                continue;
            };
            match self.colour[j] {
                Colour::New => self.visit(j),
                Colour::OnStack => {
                    let from = self
                        .stack
                        .iter()
                        .position(|&k| k == j)
                        .expect("a group on the stack");
                    let mut cycle: Vec<String> = self.stack[from..]
                        .iter()
                        .map(|&k| self.specs[k].name.clone())
                        .collect();
                    cycle.push(self.specs[j].name.clone());
                    self.found.push(cycle);
                }
                Colour::Done => {}
            }
        }
        self.stack.pop();
        self.colour[i] = Colour::Done;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subscription;
    use rurge_config::config::{LoadOptions, from_text};
    use std::path::Path;

    fn profile(proxies: &str, groups: &str) -> Config {
        let text = format!("[Proxy]\n{proxies}\n[Proxy Group]\n{groups}\n[Rule]\nFINAL,DIRECT\n");
        let loaded = from_text(&text, Path::new("/p/t.conf"), &LoadOptions::for_tests());
        assert!(
            !loaded.diagnostics.has_errors(),
            "{:?}",
            loaded
                .diagnostics
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
        );
        loaded.config
    }

    /// What the `policy-path` of each named group serves.
    fn snapshots(cfg: &Config, served: &[(&str, &str)]) -> Snapshots {
        served
            .iter()
            .map(|(group, text)| {
                let g = cfg.group_specs.iter().find(|g| g.name == *group).unwrap();
                let path = g.import.policy_path.clone().expect("a policy-path");
                (path, Arc::new(subscription::parse(text)))
            })
            .collect()
    }

    fn members<'a>(a: &'a Assembly, group: &str) -> Vec<&'a str> {
        a.members_of(group).iter().map(String::as_str).collect()
    }

    fn warnings(a: &Assembly) -> Vec<(&'static str, String)> {
        a.diagnostics
            .iter()
            .map(|d| (d.code, d.message.clone()))
            .collect()
    }

    #[test]
    fn members_come_in_the_manual_order_each_once() {
        let cfg = profile(
            "A = http, a.test, 80\nB = http, b.test, 80\nC = http, c.test, 80\nBlock = reject",
            "H = select, B, C\n\
G = select, A, DIRECT, include-other-group=H, include-all-proxies=true, policy-path=https://sub.test/g",
        );
        let a = assemble(
            &cfg,
            &snapshots(
                &cfg,
                &[(
                    "G",
                    "S1 = http, s1.test, 80\nA = http, dup.test, 80\nS2 = http, s2.test, 80",
                )],
            ),
        );
        // `include-all-proxies` takes proxies only: `Block` is a reject alias
        assert_eq!(members(&a, "G"), ["A", "DIRECT", "B", "C", "S1", "S2"]);
        assert_eq!(members(&a, "H"), ["B", "C"]);
        assert_eq!(
            warnings(&a),
            [(
                codes::W_SET_LINES_SKIPPED,
                "policy group `G`: `policy-path` line 2: `A` is already a name of the profile; skipped"
                    .to_string()
            )]
        );
        let imported: Vec<&str> = a.imported.iter().map(|i| i.policy.name.as_str()).collect();
        assert_eq!(imported, ["S1", "S2"]);
        assert!(a.cycles.is_empty());
    }

    /// The filter spares the members written on the line and sees an
    /// imported name before the prefix is put in front of it.
    #[test]
    fn the_filter_and_the_prefix_act_in_the_manual_order() {
        let cfg = profile(
            "A = http, a.test, 80\nB = http, b.test, 80\nHK-Home = http, h.test, 80",
            "G = select, A, policy-regex-filter=^HK, external-policy-name-prefix=Sub-, \
include-all-proxies=true, policy-path=https://sub.test/g",
        );
        let a = assemble(
            &cfg,
            &snapshots(
                &cfg,
                &[("G", "HK-1 = http, hk1.test, 80\nUS-1 = http, us1.test, 80")],
            ),
        );
        assert_eq!(members(&a, "G"), ["A", "HK-Home", "Sub-HK-1"]);
        assert_eq!(a.imported[0].policy.name, "Sub-HK-1");
        assert!(a.diagnostics.is_empty());
    }

    #[test]
    fn the_modifier_rewrites_the_imported_lines_only() {
        let cfg = profile(
            "A = http, a.test, 80",
            "G = select, A, policy-path=https://sub.test/g, \
external-policy-modifier=\"tfo=true,test-url=http://apple.com/\"",
        );
        let a = assemble(
            &cfg,
            &snapshots(&cfg, &[("G", "N = http, n.test, 80, tfo=false")]),
        );
        let n = &a.imported[0];
        assert_eq!(
            n.policy.definition,
            "http, n.test, 80, tfo=true, test-url=http://apple.com/"
        );
        let spec = n.spec.as_ref().expect("an http policy has a spec");
        assert!(spec.common.tfo);
        assert_eq!(spec.common.test_url.as_deref(), Some("http://apple.com/"));
        assert!(!cfg.spec("A").unwrap().common.tfo);
    }

    #[test]
    fn include_other_group_is_recursive_and_a_cycle_gives_nothing() {
        let cfg = profile(
            "M1 = http, m.test, 80\nL1 = http, l.test, 80\nX1 = http, x1.test, 80\nX2 = http, x2.test, 80",
            "Top = select, include-other-group=Mid\nMid = select, M1, include-other-group=Low\n\
Low = select, L1\nLoop1 = select, X1, include-other-group=Loop2\n\
Loop2 = select, X2, include-other-group=Loop1\nOuter = select, DIRECT, include-other-group=Loop1",
        );
        let a = assemble(&cfg, &Snapshots::new());
        assert_eq!(members(&a, "Top"), ["M1", "L1"]);
        assert_eq!(members(&a, "Mid"), ["M1", "L1"]);
        assert_eq!(members(&a, "Outer"), ["DIRECT"]);
        assert_eq!(a.cycles, [["Loop1", "Loop2", "Loop1"]]);
    }

    #[test]
    fn two_groups_share_an_identical_import_but_not_another_definition() {
        let cfg = profile(
            "A = http, a.test, 80",
            "G1 = select, policy-path=https://sub.test/a\nG2 = select, policy-path=https://sub.test/a\n\
G3 = select, policy-path=https://sub.test/b",
        );
        let a = assemble(
            &cfg,
            &snapshots(
                &cfg,
                &[
                    ("G1", "N = http, n.test, 80"),
                    ("G3", "N = http, other.test, 80"),
                ],
            ),
        );
        assert_eq!(members(&a, "G1"), ["N"]);
        assert_eq!(members(&a, "G2"), ["N"]);
        assert!(members(&a, "G3").is_empty());
        assert_eq!(a.imported.len(), 1);
        assert_eq!(
            warnings(&a),
            [(
                codes::W_SET_LINES_SKIPPED,
                "policy group `G3`: `policy-path` line 1: `N` is already imported by `G1` with another definition; skipped"
                    .to_string()
            )]
        );
    }

    #[test]
    fn an_imported_line_that_cannot_be_used_is_skipped_by_its_number() {
        let cfg = profile(
            "A = http, a.test, 80",
            "G = select, policy-path=https://sub.test/g",
        );
        let a = assemble(
            &cfg,
            &snapshots(
                &cfg,
                &[(
                    "G",
                    "Bad = http, b.test, 80, tos=999\nUp = http, u.test, 80, underlying-proxy=Nowhere\n\
SS = ss, s.test, 8388, encrypt-method=aes-128-gcm, password=pw\n\
Old = vmess, v.test, 443, username=0233d11c-15a4-47d3-ade3-48ffca0ce119\nGood = http, g.test, 80",
                )],
            ),
        );
        assert_eq!(members(&a, "G"), ["SS", "Old", "Good"]);
        let specs: Vec<bool> = a.imported.iter().map(|i| i.spec.is_some()).collect();
        assert_eq!(specs, [false, false, true]);
        assert_eq!(
            warnings(&a),
            [
                (
                    codes::W_SET_LINES_SKIPPED,
                    "policy group `G`: `policy-path` line 1: policy `Bad`: invalid value `999` for `tos` (expected 0-255 or 0x00-0xff); skipped".to_string()
                ),
                (
                    codes::W_SET_LINES_SKIPPED,
                    "policy group `G`: `policy-path` line 2: policy `Up`: `underlying-proxy` references unknown policy `Nowhere`; skipped".to_string()
                ),
                (
                    codes::W_PROTOCOL_NOT_IMPLEMENTED,
                    "policy group `G`: imported policies of type `ss` are not implemented in this version; they behave as REJECT".to_string()
                ),
                (
                    codes::W_PROTOCOL_NOT_IMPLEMENTED,
                    "policy group `G`: imported policies of type `vmess` without `vmess-aead=true` (the legacy handshake) are not implemented in this version; they behave as REJECT".to_string()
                ),
            ]
        );
    }

    /// A chain that comes back to where it started would never finish
    /// dialling: the import that closes it is left out.
    #[test]
    fn an_imported_chain_that_leads_back_is_dropped() {
        let cfg = profile(
            "Entry = http, e.test, 80, underlying-proxy=Pool",
            "Pool = select, policy-path=https://sub.test/p",
        );
        let a = assemble(
            &cfg,
            &snapshots(
                &cfg,
                &[(
                    "Pool",
                    "Loop = http, l.test, 80, underlying-proxy=Entry\nFine = http, f.test, 80",
                )],
            ),
        );
        assert_eq!(members(&a, "Pool"), ["Fine"]);
        let imported: Vec<&str> = a.imported.iter().map(|i| i.policy.name.as_str()).collect();
        assert_eq!(imported, ["Fine"]);
        assert_eq!(
            warnings(&a),
            [(
                codes::W_SET_LINES_SKIPPED,
                "policy group `Pool`: `policy-path` line 1: the `underlying-proxy` of `Loop` leads back to the policy itself; skipped"
                    .to_string()
            )]
        );
    }

    /// Nothing about a subscription's URL reaches a warning: it usually
    /// carries a token.
    #[test]
    fn a_subscription_not_downloaded_yet_is_said_without_its_url() {
        let cfg = profile(
            "A = http, a.test, 80",
            "G = select, DIRECT, policy-path=https://sub.test/g?token=t0k3n",
        );
        let a = assemble(&cfg, &Snapshots::new());
        assert_eq!(members(&a, "G"), ["DIRECT"]);
        assert_eq!(
            warnings(&a),
            [(
                codes::W_RESOURCE_UNAVAILABLE,
                "policy group `G`: `policy-path` has no content yet (never downloaded, or the file cannot be read); its imported members are unknown"
                    .to_string()
            )]
        );
        assert!(a.diagnostics.iter().all(|d| !d.message.contains("t0k3n")));
    }

    #[test]
    fn a_shared_source_that_holds_nothing_is_reported_once() {
        let cfg = profile(
            "A = http, a.test, 80",
            "G1 = select, policy-path=https://sub.test/a\nG2 = select, policy-path=https://sub.test/a",
        );
        let a = assemble(&cfg, &snapshots(&cfg, &[("G1", "proxies: []")]));
        assert_eq!(
            warnings(&a),
            [
                (
                    codes::W_SET_LINES_SKIPPED,
                    "policy group `G1`: `policy-path` line 1 skipped: not a policy line (`Name = type, ...`)".to_string()
                ),
                (
                    codes::W_SET_LINES_SKIPPED,
                    "policy group `G1`: `policy-path` holds no policy; the content may not be in Surge format (policy lines, or a profile with a `[Proxy]` section)".to_string()
                ),
            ]
        );
    }

    #[test]
    fn group_cycles_through_members_are_listed() {
        let cfg = profile("A = http, a.test, 80", "P = select, Q, A\nQ = select, P");
        let a = assemble(&cfg, &Snapshots::new());
        assert_eq!(a.cycles, [["P", "Q", "P"]]);
    }
}
```

要点（评审时对照设计 5.3）：
- 过滤作用于第 2、3、4 类成员（订阅成员按加前缀之前的原名），不作用于写在组行上的成员；前缀与修饰只作用于订阅成员；修饰先于 `to_spec`（改写后的行重新 `parse_policy`）。
- 全局命名空间：导入名与配置里的策略、组、内置名相同 → 跳过并 `W0023`；两个组导入了同名策略：定义相同是同一个策略（两个组都有它），不同则先声明的组那份生效，后者 `W0023`。
- 导入行的 `to_spec` 用"配置里的名字 + 本次导入的名字"查名字；有错误的行跳过（`W0023`，带 `to_spec` 的原因——它的消息从不回显口令）；没有 spec 的（未实现的协议、旧式 vmess）留下，每种 `W0007` 一次。
- 中继成环（P12）：边是全部 `underlying-proxy`、各组装配后的成员、各组自己的中继；导入行沿自己的 `underlying-proxy` 能走回自己 → 丢弃，并从所有组的成员表里删掉。
- 同一来源被几个组引用时，解析阶段的问题（跳过的行、截断、"可能不是 Surge 格式"）只由第一个组报一次；没有快照的来源报 `W0022`（P9 的措辞，不带 URL）。
- 组环：`include-other-group` 形成的环先算出来，环上的组不展开给任何组；最后按"装配后的成员 + `include-other-group`"再找一次，全部环进 `Assembly.cycles`。

- [ ] **Step 6: 运行**

Run: `cargo test -p rurge-policy assemble` → 10 passed。

- [ ] **Step 7: 门禁与提交**

```bash
git add crates/rurge-policy/src/assemble.rs crates/rurge-policy/src/lib.rs crates/rurge-config/src/policy.rs crates/rurge-config/src/redact.rs crates/rurge-config/src/diagnostic.rs
git commit -m "feat(policy): 成员装配——四类来源、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查与组环；with_params"
```

---

### Task 4: 组级中继派生——`M (via R)`

设了 `underlying-proxy = R` 的组，每个代理成员换成派生策略 `M (via R)`：M 的参数、R 的链，M 自己的中继被覆盖；组、内置策略、`direct` / `reject` 别名与没有 spec 的协议原样保留（设计 5.4，P10、P11）。

**Files:**
- Modify: `crates/rurge-policy/src/assemble.rs`
- Modify: `crates/rurge-config/src/diagnostic.rs`（`W0023` 的注释）

**Interfaces:**
- Consumes: Task 3 的 `Assembly` / `Imported` / `Imports`；`with_params`。
- Produces:
  - `rurge_policy::assemble::Derived { spec: PolicySpec, definition: String }`：`Clone`，没有 `Debug`；`spec.name` 是 `"<M> (via <R>)"`，`spec.common.underlying_proxy == Some(R)`，`definition` 是 M 的定义加 `underlying-proxy=R`
  - `Assembly.derived: Vec<Derived>`（每个派生名一次）；设了中继的组的 `members` 里是派生名

- [ ] **Step 1: 用例先行**

`crates/rurge-policy/src/assemble.rs`——把

```rust
    #[test]
    fn group_cycles_through_members_are_listed() {
```

换成

```rust
    #[test]
    fn every_proxy_member_is_chained_through_the_group_relay() {
        let cfg = profile(
            "Relay = http, r.test, 80\nHop = http, h.test, 80\nA = http, a.test, 80, underlying-proxy=Hop\n\
Corp = direct, interface=eth9\nBlock = reject\nSS = ss, s.test, 8388, encrypt-method=aes-128-gcm, password=pw",
            "Inner = select, A\n\
G = select, A, Corp, Block, DIRECT, Inner, SS, policy-path=https://sub.test/g, underlying-proxy=Relay",
        );
        let a = assemble(&cfg, &snapshots(&cfg, &[("G", "N = http, n.test, 80")]));
        assert_eq!(
            members(&a, "G"),
            [
                "A (via Relay)",
                "Corp",
                "Block",
                "DIRECT",
                "Inner",
                "SS",
                "N (via Relay)"
            ]
        );
        assert_eq!(members(&a, "Inner"), ["A"]);
        let names: Vec<&str> = a.derived.iter().map(|d| d.spec.name.as_str()).collect();
        assert_eq!(names, ["A (via Relay)", "N (via Relay)"]);
        let d = &a.derived[0];
        // the group's relay overrides the member's own
        assert_eq!(d.spec.common.underlying_proxy.as_deref(), Some("Relay"));
        assert_eq!(d.spec.server, cfg.spec("A").unwrap().server);
        assert_eq!(d.definition, "http, a.test, 80, underlying-proxy=Relay");
        assert_eq!(
            a.derived[1].definition,
            "http, n.test, 80, underlying-proxy=Relay"
        );
        assert!(a.diagnostics.is_empty());
    }

    #[test]
    fn groups_with_the_same_relay_share_a_derived_policy() {
        let cfg = profile(
            "Relay = http, r.test, 80\nA = http, a.test, 80",
            "G1 = select, A, underlying-proxy=Relay\nG2 = select, A, underlying-proxy=Relay\n\
G3 = select, include-other-group=G1",
        );
        let a = assemble(&cfg, &Snapshots::new());
        assert_eq!(members(&a, "G1"), ["A (via Relay)"]);
        assert_eq!(members(&a, "G2"), ["A (via Relay)"]);
        // members are all assembled before any is chained
        assert_eq!(members(&a, "G3"), ["A"]);
        assert_eq!(a.derived.len(), 1);
    }

    /// Leaving the member out is the safe side: a relay is never bypassed.
    #[test]
    fn a_derived_name_that_is_taken_leaves_the_member_out() {
        let cfg = profile(
            "Relay = http, r.test, 80\nA = http, a.test, 80\nA (via Relay) = http, x.test, 80",
            "G = select, A, DIRECT, underlying-proxy=Relay",
        );
        let a = assemble(&cfg, &Snapshots::new());
        assert_eq!(members(&a, "G"), ["DIRECT"]);
        assert!(a.derived.is_empty());
        assert_eq!(
            warnings(&a),
            [(
                codes::W_SET_LINES_SKIPPED,
                "policy group `G`: `A (via Relay)` is already the name of a policy; `A` is left out"
                    .to_string()
            )]
        );
    }

    /// The relay of a group is an edge too: an imported line that goes back
    /// through the group whose relay imported it would never finish
    /// dialling, so it is left out and the group's own members keep theirs.
    #[test]
    fn an_imported_chain_through_a_group_relay_is_dropped() {
        let cfg = profile(
            "A = http, a.test, 80",
            "G = select, A, underlying-proxy=R\nR = select, policy-path=https://sub.test/r",
        );
        let a = assemble(
            &cfg,
            &snapshots(
                &cfg,
                &[(
                    "R",
                    "X = http, x.test, 80, underlying-proxy=G\nY = http, y.test, 80",
                )],
            ),
        );
        assert_eq!(members(&a, "R"), ["Y"]);
        assert_eq!(members(&a, "G"), ["A (via R)"]);
        assert_eq!(
            warnings(&a),
            [(
                codes::W_SET_LINES_SKIPPED,
                "policy group `R`: `policy-path` line 1: the `underlying-proxy` of `X` leads back to the policy itself; skipped"
                    .to_string()
            )]
        );
    }

    #[test]
    fn group_cycles_through_members_are_listed() {
```

Run: `cargo test -p rurge-policy assemble`

Expected: 编译错误——`no field `derived` on type `Assembly``。

- [ ] **Step 2: 写实现**

`crates/rurge-policy/src/assemble.rs`——把

```rust
use rurge_config::spec::{GroupSpec, NameKind, PolicyPath, PolicySpec, SpecEnv, to_spec};
```

换成

```rust
use rurge_config::spec::{
    GroupSpec, NameKind, PolicyPath, PolicySpec, ProtoSpec, SpecEnv, to_spec,
};
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
    pub spec: Option<PolicySpec>,
}

/// No `Debug`: imported policies carry credentials.
```

换成

```rust
    pub spec: Option<PolicySpec>,
}

/// `M (via R)`: a member M of a group with `underlying-proxy = R` (5.4).
#[derive(Clone)]
pub struct Derived {
    /// M's, named `M (via R)` and chained through R.
    pub spec: PolicySpec,
    /// M's definition with `underlying-proxy` set to R.
    pub definition: String,
}

/// No `Debug`: imported policies carry credentials.
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
    /// Names unique, none of them a name of the profile.
    pub imported: Vec<Imported>,
```

换成

```rust
    /// Names unique, none of them a name of the profile.
    pub imported: Vec<Imported>,
    /// Each derived name once, however many groups share it.
    pub derived: Vec<Derived>,
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
    imports.drop_chain_cycles(cfg, &mut members, &mut diagnostics);
    let cycles = group_cycles(cfg, &members);
    Assembly {
        members,
        imported: imports.list.into_iter().map(|(i, _)| i).collect(),
        cycles,
```

换成

```rust
    imports.drop_chain_cycles(cfg, &mut members, &mut diagnostics);
    let derived = derive(cfg, &imports, &mut members, &mut diagnostics);
    let cycles = group_cycles(cfg, &members);
    Assembly {
        members,
        imported: imports.list.into_iter().map(|(i, _)| i).collect(),
        derived,
        cycles,
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
/// Group cycles through members (as assembled) and `include-other-group`.
```

换成

```rust
/// Every proxy member M of a group with `underlying-proxy = R` becomes the
/// derived policy `M (via R)`: M's parameters on R's chain, whatever relay M
/// has of its own overridden. Groups, built-ins, `direct` / `reject` aliases
/// and protocols without a spec stay as they are (5.4). A group that takes
/// another's members through `include-other-group` takes them as written,
/// since members are all assembled first. A derived name that is taken
/// leaves the member out: a relay is never bypassed.
fn derive(
    cfg: &Config,
    imports: &Imports<'_>,
    members: &mut HashMap<String, Vec<String>>,
    diags: &mut Diagnostics,
) -> Vec<Derived> {
    let imported: HashMap<&str, &Imported> = imports
        .list
        .iter()
        .map(|(i, _)| (i.policy.name.as_str(), i))
        .collect();
    let mut derived: Vec<Derived> = Vec::new();
    let mut made: HashSet<String> = HashSet::new();
    for g in &cfg.group_specs {
        let Some(relay) = &g.underlying_proxy else {
            continue;
        };
        let Some(list) = members.get_mut(&g.name) else {
            continue;
        };
        let mut chained = Vec::with_capacity(list.len());
        for m in list.drain(..) {
            let source = match imported.get(m.as_str()) {
                Some(i) => i.spec.as_ref().map(|s| (s, i.policy.definition.as_str())),
                None => cfg.spec(&m).and_then(|s| {
                    let p = cfg.policies.iter().find(|p| p.name == m)?;
                    Some((s, p.definition.as_str()))
                }),
            };
            let Some((spec, definition)) = source
                .filter(|(s, _)| !matches!(s.proto, ProtoSpec::Direct | ProtoSpec::Reject(_)))
            else {
                chained.push(m);
                continue;
            };
            let name = format!("{m} (via {relay})");
            if cfg.name_kind(&name).is_some() || imported.contains_key(name.as_str()) {
                diags.push(warn(
                    g,
                    codes::W_SET_LINES_SKIPPED,
                    format!("`{name}` is already the name of a policy; `{m}` is left out"),
                ));
                continue;
            }
            if made.insert(name.clone()) {
                let mut spec = spec.clone();
                spec.name = name.clone();
                spec.common.underlying_proxy = Some(relay.clone());
                let relay = [("underlying-proxy".to_string(), relay.clone())];
                derived.push(Derived {
                    spec,
                    definition: with_params(definition, &relay),
                });
            }
            chained.push(name);
        }
        *list = chained;
    }
    derived
}

/// Group cycles through members (as assembled) and `include-other-group`.
```

`crates/rurge-config/src/diagnostic.rs`——把

```rust
    /// A set file or a `policy-path` subscription contained lines that were skipped.
```

换成

```rust
    /// A set file or a `policy-path` subscription contained lines that were
    /// skipped, or a group had to leave a member out.
```

要点：派生在全部组装配完之后才做，所以 `include-other-group` 取到的是未派生的成员（P10）；派生名已被占用 → 该成员略去并 `W0023`，不改用不经中继的原成员（P11）；同一个派生名只建一份（两个组共用中继 R 与成员 M）。`an_imported_chain_through_a_group_relay_is_dropped` 钉住 Task 3 的"组自己的中继也是一条边"：导入行经组级中继绕回自己时被丢弃。

- [ ] **Step 3: 运行**

Run: `cargo test -p rurge-policy assemble` → 14 passed；`cargo test -p rurge-policy` → 37 passed。

- [ ] **Step 4: 门禁与提交**

```bash
git add crates/rurge-policy/src/assemble.rs crates/rurge-config/src/diagnostic.rs
git commit -m "feat(policy): 组级 underlying-proxy 派生 M (via R)；派生名被占用时略去该成员"
```

---


### Task 5: 注册表接受装配结果——导入 / 派生条目、环与空组兜底

`PolicyRegistry::build` 的签名改为设计 5.5 的七个参数：多了 `assembly` 与 `empty_group`。组的成员表来自装配；导入与派生的策略同主配置的策略一样按指纹复用上一代的出站，但构建失败只略去那一条（M3-D6）；环上的组 REJECT、会话说明写出整个环；没有成员的组按 `EmptyGroup` 兜底（M3-D3，**行为变化**：今天是 REJECT，默认改为 DIRECT）；注册表保存每个名字的定义行，供 Task 7 的视图读。引擎只做最小的跟进：按新签名构建（暂时传空快照与 `EmptyGroup::Direct`），会话说明改用 `Note` 的 `Display`。

**Files:**
- Modify: `crates/rurge-policy/src/registry.rs`、`crates/rurge-policy/src/cell.rs`（用例）、`crates/rurge-policy/src/lib.rs`
- Modify: `crates/rurge-engine/src/runtime.rs`、`crates/rurge-engine/src/engine.rs`

**Interfaces:**
- Consumes: Task 3 / 4 的 `Assembly`（`members_of`、`imported`、`derived`、`cycles`）；`Config.group_specs`、`GroupSpec.{kind, hidden}`、`PolicyGroup.definition`。
- Produces:
  - `PolicyRegistry::build(cfg: &Config, assembly: &Assembly, factory: &dyn OutboundFactory, cell: &Arc<RegistryCell>, selections: Arc<SelectionTable>, previous: Option<&PolicyRegistry>, empty_group: EmptyGroup) -> Result<PolicyRegistry, BuildError>`
  - `rurge_policy::EmptyGroup { Direct /* Default */, Reject }`：`Clone + Copy + Debug + Default + PartialEq + Eq`
  - `rurge_policy::Note` 新增 `GroupCycle(String)`、`EmptyGroup { substituted: bool }`，并实现 `Display`——会话日志的原文：`policy protocol not implemented: <kind>` / `policy group cycle: A → B → A` / `policy group has no members; DIRECT substituted` / `policy group has no members`
  - `rurge_policy::Line { is_group: bool, keyword: &'static str, definition: String }`（未脱敏）、`rurge_policy::GroupInfo<'a> { kind: GroupKind, hidden: bool, members: &'a [String] }`
  - `PolicyRegistry::{policy_names() -> Vec<String>, group_names() -> Vec<String>, group(&str) -> Option<GroupInfo<'_>>, members(&str) -> Option<&[String]>, line(&str) -> Option<&Line>, spec(&str) -> Option<&PolicySpec>}`

- [ ] **Step 1: 用例先行**

测试夹具按新签名构建，新增六条用例：

`crates/rurge-policy/src/registry.rs`——把

```rust
    use super::*;
    use crate::selections::GroupSelections;
    use crate::testing::FakeFactory;
```

换成

```rust
    use super::*;
    use crate::assemble::{Snapshots, assemble};
    use crate::selections::GroupSelections;
    use crate::subscription;
    use crate::testing::FakeFactory;
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        let registry = Arc::new(
            PolicyRegistry::build(&loaded.config, &factory, &cell, table.clone(), None)
                .expect("builds"),
        );
```

换成

```rust
        let registry = Arc::new(
            PolicyRegistry::build(
                &loaded.config,
                &assemble(&loaded.config, &Snapshots::new()),
                &factory,
                &cell,
                table.clone(),
                None,
                EmptyGroup::Direct,
            )
            .expect("builds"),
        );
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        let e = PolicyRegistry::build(
            &loaded.config,
            &factory,
            &RegistryCell::new(),
            Arc::new(SelectionTable::default()),
            None,
        )
        .err()
        .expect("EntryB does not build");
```

换成

```rust
        let e = PolicyRegistry::build(
            &loaded.config,
            &assemble(&loaded.config, &Snapshots::new()),
            &factory,
            &RegistryCell::new(),
            Arc::new(SelectionTable::default()),
            None,
            EmptyGroup::Direct,
        )
        .err()
        .expect("EntryB does not build");
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        PolicyRegistry::build(
            &loaded.config,
            factory,
            &RegistryCell::new(),
            Arc::new(SelectionTable::new(GroupSelections::new())),
            previous,
        )
        .expect("builds")
    }
```

换成

```rust
        PolicyRegistry::build(
            &loaded.config,
            &assemble(&loaded.config, &Snapshots::new()),
            factory,
            &RegistryCell::new(),
            Arc::new(SelectionTable::new(GroupSelections::new())),
            previous,
            EmptyGroup::Direct,
        )
        .expect("builds")
    }
```

`crates/rurge-policy/src/registry.rs`——把

```rust
    #[test]
    fn selections_api() {
```

换成

```rust
    const SUBSCRIBED: &str = "[Proxy]\nRelay = http, r.example, 80\nA = http, a.example, 80\n\
[Proxy Group]\nSub = select, A, policy-path=https://sub.example/nodes, underlying-proxy=Relay, hidden=true\n\
Plain = select, DIRECT, policy-path=https://sub.example/nodes\n[Rule]\nFINAL,Sub\n";

    /// One generation of `SUBSCRIBED`, the subscription serving `nodes`,
    /// built on `previous` into `cell`.
    fn subscribed(
        nodes: &str,
        factory: &FakeFactory,
        cell: &Arc<RegistryCell>,
        previous: Option<&PolicyRegistry>,
    ) -> PolicyRegistry {
        let loaded = from_text(SUBSCRIBED, Path::new("t.conf"), &LoadOptions::for_tests());
        assert!(!loaded.diagnostics.has_errors());
        let cfg = loaded.config;
        let path = cfg.group_specs[0].import.policy_path.clone().unwrap();
        let snapshots = Snapshots::from([(path, Arc::new(subscription::parse(nodes)))]);
        PolicyRegistry::build(
            &cfg,
            &assemble(&cfg, &snapshots),
            factory,
            cell,
            Arc::new(SelectionTable::default()),
            previous,
            EmptyGroup::Direct,
        )
        .expect("builds")
    }

    #[test]
    fn imported_and_derived_policies_are_entries_of_their_own() {
        let factory = FakeFactory::new();
        let reg = subscribed(
            "N1 = http, n1.example, 80\nN2 = socks5, n2.example, 1080",
            &factory,
            &RegistryCell::new(),
            None,
        );
        assert_eq!(
            reg.policy_names(),
            [
                "Relay",
                "A",
                "N1",
                "N2",
                "A (via Relay)",
                "N1 (via Relay)",
                "N2 (via Relay)"
            ]
        );
        assert_eq!(reg.group_names(), ["Sub", "Plain"]);
        let sub = reg.group("Sub").unwrap();
        assert_eq!(
            (sub.kind, sub.hidden, sub.members),
            (
                GroupKind::Select,
                true,
                &[
                    "A (via Relay)".to_string(),
                    "N1 (via Relay)".to_string(),
                    "N2 (via Relay)".to_string()
                ][..]
            )
        );
        assert_eq!(
            reg.members("Plain"),
            Some(&["DIRECT".to_string(), "N1".to_string(), "N2".to_string()][..])
        );
        let n2 = reg.line("N2").unwrap();
        assert_eq!(
            (n2.is_group, n2.keyword, n2.definition.as_str()),
            (false, "socks5", "socks5, n2.example, 1080")
        );
        assert_eq!(
            reg.line("N1 (via Relay)").unwrap().definition,
            "http, n1.example, 80, underlying-proxy=Relay"
        );
        assert!(reg.line("Sub").unwrap().is_group);
        assert_eq!(reg.line("DIRECT"), None);
        assert_eq!(
            reg.spec("N1 (via Relay)")
                .unwrap()
                .common
                .underlying_proxy
                .as_deref(),
            Some("Relay")
        );
        assert!(reg.spec("Sub").is_none());
    }

    /// A subscription update keeps every outbound whose line did not change,
    /// the derived ones included (M3 design 5.7).
    #[test]
    fn an_update_keeps_the_outbounds_of_lines_that_did_not_change() {
        let factory = FakeFactory::new();
        let cell = RegistryCell::new();
        let first = subscribed(
            "N1 = http, n1.example, 80\nN2 = http, n2.example, 80",
            &factory,
            &cell,
            None,
        );
        let second = subscribed(
            "N1 = http, n1.example, 80\nN2 = http, moved.example, 80\nN3 = http, n3.example, 80",
            &factory,
            &cell,
            Some(&first),
        );
        for same in ["N1", "N1 (via Relay)", "A", "A (via Relay)", "Relay"] {
            assert!(
                Arc::ptr_eq(&outbound_of(&first, same), &outbound_of(&second, same)),
                "{same} was rebuilt"
            );
        }
        for changed in ["N2", "N2 (via Relay)"] {
            assert!(!Arc::ptr_eq(
                &outbound_of(&first, changed),
                &outbound_of(&second, changed)
            ));
        }
        assert!(second.contains("N3 (via Relay)"));
    }

    #[tokio::test]
    async fn a_derived_policy_dials_through_the_group_relay() {
        let factory = FakeFactory::new();
        let cell = RegistryCell::new();
        let reg = Arc::new(subscribed(
            "N1 = http, n1.example, 80",
            &factory,
            &cell,
            None,
        ));
        cell.store(reg.clone());
        let n1 = reg.resolve(&PolicyRef::parse("Sub"));
        assert_eq!(
            (chain(&n1), n1.terminal),
            (vec!["Sub", "A (via Relay)"], TerminalKind::Proxy)
        );
        reg.resolve(&PolicyRef::parse("N1 (via Relay)"))
            .outbound
            .connect_tcp(
                &Target::new(HostName::parse("site.example"), 443),
                &ConnectOpts::default(),
            )
            .await
            .unwrap();
        assert_eq!(
            factory.connector.seen(),
            [
                "N1 (via Relay) -> site.example:443",
                "Relay -> n1.example:80",
                "dial r.example:80",
            ]
        );
        cell.clear();
    }

    #[test]
    fn an_imported_policy_that_cannot_be_built_is_left_out() {
        let factory = FakeFactory {
            broken: Some("N2"),
            ..FakeFactory::new()
        };
        let reg = subscribed(
            "N1 = http, n1.example, 80\nN2 = http, n2.example, 80",
            &factory,
            &RegistryCell::new(),
            None,
        );
        assert!(!reg.contains("N2"));
        assert_eq!(
            reg.members("Plain"),
            Some(&["DIRECT".to_string(), "N1".to_string()][..])
        );
    }

    #[test]
    fn a_group_on_a_cycle_rejects_and_names_the_cycle() {
        let text = "[Proxy]\nA = http, a.example, 80\n[Proxy Group]\nP = select, Q\nQ = select, P\n\
K = select, P, A\n[Rule]\nFINAL,K\n";
        let reg = generation(text, &FakeFactory::new(), None);
        let p = reg.resolve(&PolicyRef::parse("P"));
        assert_eq!(
            (chain(&p), p.terminal, p.note.clone()),
            (
                vec!["P", "REJECT"],
                TerminalKind::Reject,
                Some(Note::GroupCycle("P → Q → P".into()))
            )
        );
        assert_eq!(p.note.unwrap().to_string(), "policy group cycle: P → Q → P");
        let q = reg.resolve(&PolicyRef::parse("Q"));
        assert_eq!(q.note, Some(Note::GroupCycle("P → Q → P".into())));
        // a group that is not on it rejects only when it picks the member that is
        let k = reg.resolve(&PolicyRef::parse("K"));
        assert_eq!(chain(&k), vec!["K", "P", "REJECT"]);
        assert_eq!(k.note, Some(Note::GroupCycle("P → Q → P".into())));
    }

    #[test]
    fn an_empty_group_stands_in_direct_or_rejects() {
        let text =
            "[Proxy Group]\nG = select, policy-path=https://sub.example/g\n[Rule]\nFINAL,G\n";
        let loaded = from_text(text, Path::new("t.conf"), &LoadOptions::for_tests());
        let cfg = loaded.config;
        let assembly = assemble(&cfg, &Snapshots::new());
        let build = |empty_group| {
            PolicyRegistry::build(
                &cfg,
                &assembly,
                &FakeFactory::new(),
                &RegistryCell::new(),
                Arc::new(SelectionTable::default()),
                None,
                empty_group,
            )
            .expect("builds")
        };
        let direct = build(EmptyGroup::Direct).resolve(&PolicyRef::parse("G"));
        assert_eq!(
            (chain(&direct), direct.terminal),
            (vec!["G", "DIRECT"], TerminalKind::Direct)
        );
        assert_eq!(
            direct.note.unwrap().to_string(),
            "policy group has no members; DIRECT substituted"
        );
        let reject = build(EmptyGroup::Reject).resolve(&PolicyRef::parse("G"));
        assert_eq!(
            (chain(&reject), reject.terminal),
            (vec!["G", "REJECT"], TerminalKind::Reject)
        );
        assert_eq!(
            reject.note.unwrap().to_string(),
            "policy group has no members"
        );
    }

    #[test]
    fn selections_api() {
```

`crates/rurge-policy/src/cell.rs`——把

```rust
        Arc::new(
            PolicyRegistry::build(
                &loaded.config,
                &factory,
                &RegistryCell::new(),
                Arc::new(crate::selections::SelectionTable::default()),
                None,
            )
            .expect("builds"),
        )
```

换成

```rust
        Arc::new(
            PolicyRegistry::build(
                &loaded.config,
                &crate::assemble(&loaded.config, &crate::Snapshots::new()),
                &factory,
                &RegistryCell::new(),
                Arc::new(crate::selections::SelectionTable::default()),
                None,
                crate::EmptyGroup::Direct,
            )
            .expect("builds"),
        )
```

Run: `cargo test -p rurge-policy`

Expected: 编译错误——`this function takes 5 arguments but 7 arguments were supplied`、`cannot find type `EmptyGroup``、`no method named `policy_names``。

- [ ] **Step 2: 写实现**

`crates/rurge-policy/src/registry.rs`——把

```rust
//! Name → outbound resolution (M3 design §5, M1 design 6.2). Built once per
//! config generation; `resolve` is a table walk with no allocation beyond the
//! chain and the group selections it reads.

use crate::cell::{ChainConnector, RegistryCell};
```

换成

```rust
//! Name → outbound resolution (M3 design §5, M1 design 6.2; phase 2 M3
//! design 5.5, 5.6). Built for every config generation and every
//! subscription update; `resolve` is a table walk with no allocation beyond
//! the chain and the group selections it reads.

use crate::assemble::Assembly;
use crate::cell::{ChainConnector, RegistryCell};
```

`crates/rurge-policy/src/registry.rs`——把

```rust
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Deeper chains than this are treated as a defect (group cycles are load errors).
pub const MAX_DEPTH: usize = 16;
```

换成

```rust
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::Path;
use std::sync::Arc;

/// Deeper chains than this are treated as a defect (a group cycle resolves
/// to REJECT before it gets that deep).
pub const MAX_DEPTH: usize = 16;
```

`crates/rurge-policy/src/registry.rs`——把

```rust
/// Why a resolution ended where it did, when that needs saying.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Note {
    /// The terminal policy's protocol keyword (or `DEVICE`) is not
    /// implemented in this version: the outbound is REJECT.
    Unsupported(String),
}
```

换成

```rust
/// Why a resolution ended where it did, when that needs saying.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Note {
    /// The terminal policy's protocol keyword (or `DEVICE`) is not
    /// implemented in this version: the outbound is REJECT.
    Unsupported(String),
    /// A group on a cycle of groups, written out (`A → B → A`): REJECT.
    GroupCycle(String),
    /// A group without members; `substituted` when DIRECT stood in for it.
    EmptyGroup { substituted: bool },
}

/// What the session log says (M3 design 5.6).
impl fmt::Display for Note {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Note::Unsupported(kind) => write!(f, "policy protocol not implemented: {kind}"),
            Note::GroupCycle(cycle) => write!(f, "policy group cycle: {cycle}"),
            Note::EmptyGroup { substituted: true } => {
                f.write_str("policy group has no members; DIRECT substituted")
            }
            Note::EmptyGroup { substituted: false } => f.write_str("policy group has no members"),
        }
    }
}

/// What a group without members resolves to (M3-D3): DIRECT, as in Surge,
/// unless `--empty-group-reject` asks for REJECT.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EmptyGroup {
    #[default]
    Direct,
    Reject,
}

/// How a policy or group is written, for the control plane. Secrets
/// included: whoever shows it redacts it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub is_group: bool,
    /// A policy's type keyword or a group's kind keyword.
    pub keyword: &'static str,
    /// Right of `name =`: as written, as imported, or — for `M (via R)` —
    /// M's with `underlying-proxy` set.
    pub definition: String,
}

impl Line {
    fn policy(kind: PolicyKind, definition: &str) -> Line {
        Line {
            is_group: false,
            keyword: kind.keyword(),
            definition: definition.to_string(),
        }
    }
}

/// A group as the control plane shows it.
pub struct GroupInfo<'a> {
    pub kind: GroupKind,
    pub hidden: bool,
    /// As assembled.
    pub members: &'a [String],
}
```

`crates/rurge-policy/src/registry.rs`——把

```rust
    Group {
        kind: GroupKind,
        members: Vec<String>,
    },
}

pub struct PolicyRegistry {
    entries: HashMap<String, Entry>,
    order: Vec<String>,
    direct: OutboundRef,
    rejects: [OutboundRef; 4],
    selections: Arc<SelectionTable>,
}
```

换成

```rust
    Group {
        kind: GroupKind,
        members: Vec<String>,
        hidden: bool,
        /// The cycle it is on, written out: it resolves to REJECT.
        cycle: Option<String>,
    },
}

pub struct PolicyRegistry {
    entries: HashMap<String, Entry>,
    order: Vec<String>,
    lines: HashMap<String, Line>,
    direct: OutboundRef,
    rejects: [OutboundRef; 4],
    selections: Arc<SelectionTable>,
    empty_group: EmptyGroup,
}

/// What `build` fills in, name by name.
#[derive(Default)]
struct Table {
    entries: HashMap<String, Entry>,
    order: Vec<String>,
    lines: HashMap<String, Line>,
}

impl Table {
    fn add(&mut self, name: &str, entry: Entry, line: Line) {
        self.entries.insert(name.to_string(), entry);
        self.order.push(name.to_string());
        self.lines.insert(name.to_string(), line);
    }
}
```

`crates/rurge-policy/src/registry.rs`——把

```rust
    factory
        .build(spec, connector)
        .map_err(|e| BuildError::new(format!("policy `{}`: {}", spec.name, e.message)))
}
```

换成

```rust
    factory
        .build(spec, connector)
        .map_err(|e| BuildError::new(format!("policy `{}`: {}", spec.name, e.message)))
}

/// Every group on a cycle, with the cycle written out; each cycle is said
/// once per build.
fn on_cycle(assembly: &Assembly) -> HashMap<&str, String> {
    let mut out = HashMap::new();
    for cycle in &assembly.cycles {
        let text = cycle.join(" → ");
        tracing::warn!(cycle = %text, "policy group cycle; the groups on it behave as REJECT");
        // the last group is the first one again
        for group in &cycle[..cycle.len().saturating_sub(1)] {
            out.entry(group.as_str()).or_insert_with(|| text.clone());
        }
    }
    out
}
```

`crates/rurge-policy/src/registry.rs`——把

```rust
    /// all (M2 design 7.1). `previous` must have been built against this same
    /// `cell`: a reused outbound keeps the chain connectors it was built
    /// with, and they resolve through that cell.
    pub fn build(
        cfg: &Config,
        factory: &dyn OutboundFactory,
        cell: &Arc<RegistryCell>,
        selections: Arc<SelectionTable>,
        previous: Option<&PolicyRegistry>,
    ) -> Result<PolicyRegistry, BuildError> {
        let direct: OutboundRef = Arc::new(Direct::new(
            factory.direct_connector(&CommonOpts::default()),
        ));
        let mut entries = HashMap::new();
        let mut order = Vec::new();
        let environment = factory.environment();
```

换成

```rust
    /// all (M2 design 7.1). `previous` must have been built against this same
    /// `cell`: a reused outbound keeps the chain connectors it was built
    /// with, and they resolve through that cell. The groups take their
    /// members from `assembly`, which also brings the imported and the
    /// derived policies (M3 design 5.5).
    pub fn build(
        cfg: &Config,
        assembly: &Assembly,
        factory: &dyn OutboundFactory,
        cell: &Arc<RegistryCell>,
        selections: Arc<SelectionTable>,
        previous: Option<&PolicyRegistry>,
        empty_group: EmptyGroup,
    ) -> Result<PolicyRegistry, BuildError> {
        let direct: OutboundRef = Arc::new(Direct::new(
            factory.direct_connector(&CommonOpts::default()),
        ));
        let environment = factory.environment();
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        for p in &cfg.policies {
            let entry = match (alias_terminal(p.kind), cfg.spec(&p.name)) {
                (Some(Terminal::Direct), Some(spec)) if has_socket_opts(&spec.common) => {
                    outbound_entry(spec, false)?
                }
                (Some(terminal), _) => Entry::Alias(terminal),
                (None, Some(spec)) => outbound_entry(spec, true)?,
                // no spec: a protocol of a later milestone
                (None, None) => Entry::Unsupported { kind: p.kind },
            };
            entries.insert(p.name.clone(), entry);
            order.push(p.name.clone());
        }
        for g in &cfg.groups {
            entries.insert(
                g.name.clone(),
                Entry::Group {
                    kind: g.kind,
                    members: g.members.clone(),
                },
            );
            order.push(g.name.clone());
        }
```

换成

```rust
        let policy_entry = |kind: PolicyKind, spec: Option<&PolicySpec>| {
            Ok::<Entry, BuildError>(match (alias_terminal(kind), spec) {
                (Some(Terminal::Direct), Some(spec)) if has_socket_opts(&spec.common) => {
                    outbound_entry(spec, false)?
                }
                (Some(terminal), _) => Entry::Alias(terminal),
                (None, Some(spec)) => outbound_entry(spec, true)?,
                // no spec: a protocol of a later milestone
                (None, None) => Entry::Unsupported { kind },
            })
        };
        let mut table = Table::default();
        // The profile's own policies: the dry build has made a failure here a
        // load error, so one fails the whole generation.
        for p in &cfg.policies {
            let entry = policy_entry(p.kind, cfg.spec(&p.name))?;
            table.add(&p.name, entry, Line::policy(p.kind, &p.definition));
        }
        // What subscriptions brought in and the `M (via R)` of relayed groups:
        // a failure leaves that policy out and nothing else (M3-D6).
        let left_out = |name: &str, e: BuildError| {
            tracing::warn!(policy = %name, error = %e.message, "policy cannot be built; it is left out");
        };
        for i in &assembly.imported {
            match policy_entry(i.policy.kind, i.spec.as_ref()) {
                Ok(entry) => table.add(
                    &i.policy.name,
                    entry,
                    Line::policy(i.policy.kind, &i.policy.definition),
                ),
                Err(e) => left_out(&i.policy.name, e),
            }
        }
        for d in &assembly.derived {
            match outbound_entry(&d.spec, true) {
                Ok(entry) => table.add(
                    &d.spec.name,
                    entry,
                    Line::policy(d.spec.kind, &d.definition),
                ),
                Err(e) => left_out(&d.spec.name, e),
            }
        }
        let on_cycle = on_cycle(assembly);
        let groups: HashSet<&str> = cfg.group_specs.iter().map(|g| g.name.as_str()).collect();
        for g in &cfg.group_specs {
            // a member whose policy was left out is left out too
            let members: Vec<String> = assembly
                .members_of(&g.name)
                .iter()
                .filter(|m| {
                    groups.contains(m.as_str())
                        || table.entries.contains_key(m.as_str())
                        || !matches!(PolicyRef::parse(m), PolicyRef::Named(_))
                })
                .cloned()
                .collect();
            let cycle = on_cycle.get(g.name.as_str()).cloned();
            if cycle.is_none() && members.is_empty() {
                let note = Note::EmptyGroup {
                    substituted: empty_group == EmptyGroup::Direct,
                };
                tracing::warn!(group = %g.name, "{note}");
            }
            let definition = cfg
                .groups
                .iter()
                .find(|written| written.name == g.name)
                .map(|written| written.definition.clone())
                .unwrap_or_default();
            let line = Line {
                is_group: true,
                keyword: g.kind.keyword(),
                definition,
            };
            let entry = Entry::Group {
                kind: g.kind,
                members,
                hidden: g.hidden,
                cycle,
            };
            table.add(&g.name, entry, line);
        }
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        Ok(PolicyRegistry {
            entries,
            order,
            direct,
            rejects,
            selections,
        })
    }
```

换成

```rust
        Ok(PolicyRegistry {
            entries: table.entries,
            order: table.order,
            lines: table.lines,
            direct,
            rejects,
            selections,
            empty_group,
        })
    }
```

`crates/rurge-policy/src/registry.rs`——把

```rust
    pub fn contains(&self, name: &str) -> bool {
        self.order.iter().any(|n| n == name)
    }
```

换成

```rust
    pub fn contains(&self, name: &str) -> bool {
        self.order.iter().any(|n| n == name)
    }

    /// Every policy that is not a group, in the order they were built: the
    /// profile's, the imported ones, then the derived ones.
    pub fn policy_names(&self) -> Vec<String> {
        self.order
            .iter()
            .filter(|n| !matches!(self.entries.get(n.as_str()), Some(Entry::Group { .. })))
            .cloned()
            .collect()
    }

    /// Every group, in profile order.
    pub fn group_names(&self) -> Vec<String> {
        self.order
            .iter()
            .filter(|n| matches!(self.entries.get(n.as_str()), Some(Entry::Group { .. })))
            .cloned()
            .collect()
    }

    pub fn group(&self, name: &str) -> Option<GroupInfo<'_>> {
        match self.entries.get(name)? {
            Entry::Group {
                kind,
                members,
                hidden,
                ..
            } => Some(GroupInfo {
                kind: *kind,
                hidden: *hidden,
                members,
            }),
            _ => None,
        }
    }

    /// The members of `group` as assembled; `None` when it is not a group.
    pub fn members(&self, group: &str) -> Option<&[String]> {
        self.group(group).map(|g| g.members)
    }

    /// How `name` is written; `None` for a built-in.
    pub fn line(&self, name: &str) -> Option<&Line> {
        self.lines.get(name)
    }

    /// What `name` was built from: a proxy, or a `direct` alias with socket
    /// options of its own.
    pub fn spec(&self, name: &str) -> Option<&PolicySpec> {
        match self.entries.get(name)? {
            Entry::Outbound { fingerprint, .. } => Some(&fingerprint.spec),
            _ => None,
        }
    }
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        let Some(Entry::Group { kind, members }) = self.entries.get(group) else {
            return None;
        };
```

换成

```rust
        let Some(Entry::Group { kind, members, .. }) = self.entries.get(group) else {
            return None;
        };
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            Some(Entry::Group { .. }) => match self.current_member(name) {
                Some(member) => match PolicyRef::parse(&member) {
                    PolicyRef::Builtin(b) => self.builtin(b, chain),
                    PolicyRef::Device(d) => self.device(&d, chain),
                    PolicyRef::Named(n) => self.named(&n, chain, depth + 1),
                },
                None => {
                    tracing::error!(
                        group = name,
                        "policy group has no members; treating as REJECT"
                    );
                    self.rejected(chain, None)
                }
            },
        }
    }
```

换成

```rust
            Some(Entry::Group {
                cycle: Some(cycle), ..
            }) => self.rejected(chain, Some(Note::GroupCycle(cycle.clone()))),
            Some(Entry::Group { .. }) => match self.current_member(name) {
                Some(member) => match PolicyRef::parse(&member) {
                    PolicyRef::Builtin(b) => self.builtin(b, chain),
                    PolicyRef::Device(d) => self.device(&d, chain),
                    PolicyRef::Named(n) => self.named(&n, chain, depth + 1),
                },
                None => self.empty(chain),
            },
        }
    }

    /// A group without members: DIRECT stands in, or REJECT (M3-D3).
    fn empty(&self, chain: &mut Vec<String>) -> Resolution {
        match self.empty_group {
            EmptyGroup::Direct => {
                chain.push("DIRECT".to_string());
                let note = Note::EmptyGroup { substituted: true };
                self.done(chain, self.direct(), TerminalKind::Direct, Some(note))
            }
            EmptyGroup::Reject => {
                self.rejected(chain, Some(Note::EmptyGroup { substituted: false }))
            }
        }
    }
```

`crates/rurge-policy/src/lib.rs`——把

```rust
pub use registry::{Note, PolicyRegistry, Resolution, TerminalKind};
```

换成

```rust
pub use registry::{EmptyGroup, GroupInfo, Line, Note, PolicyRegistry, Resolution, TerminalKind};
```

要点：
- 条目的构建顺序：配置的策略（失败 → 整代失败，干构建早已把它变成加载错误）→ 导入的策略 → 派生的策略（失败 → WARN 并略去）→ 组。组的成员表只留下有着落的名字：内置策略、`DEVICE:`、组，或者上面建成了条目的策略——构建失败被略去的成员随之从成员表里消失。
- 环：`Assembly.cycles` 里每个环 WARN 一次（`cycle = A → B → A`），环上每个组记下自己所在的那个环；解析到它 → `REJECT` + `Note::GroupCycle`。
- 空组：构建时每组 WARN 一次（说明文字同会话日志）；解析到它 → `EmptyGroup::Direct` 时经 DIRECT 出站、`Note::EmptyGroup { substituted: true }`，`EmptyGroup::Reject` 时 REJECT、`substituted: false`。原来那条每次解析都打的 ERROR 日志去掉。
- `spec(name)` 返回指纹里的那份 spec（span 已抹掉）：Task 7 的 `socket_opener` 用它代替 `Config::spec`，导入与派生的策略也就有了。

- [ ] **Step 3: 运行**

Run: `cargo test -p rurge-policy` → 43 passed（新增 `imported_and_derived_policies_are_entries_of_their_own`、`an_update_keeps_the_outbounds_of_lines_that_did_not_change`、`a_derived_policy_dials_through_the_group_relay`、`an_imported_policy_that_cannot_be_built_is_left_out`、`a_group_on_a_cycle_rejects_and_names_the_cycle`、`an_empty_group_stands_in_direct_or_rejects`）。

- [ ] **Step 4: 引擎按新签名构建，会话说明用 `Display`**

`crates/rurge-engine/src/runtime.rs`——把

```rust
            PolicyRegistry::build(
                &config,
                &factory,
                &opts.shared.cell,
                opts.shared.selections.clone(),
                previous.as_deref(),
            )
```

换成

```rust
            PolicyRegistry::build(
                &config,
                &rurge_policy::assemble(&config, &rurge_policy::Snapshots::new()),
                &factory,
                &opts.shared.cell,
                opts.shared.selections.clone(),
                previous.as_deref(),
                rurge_policy::EmptyGroup::Direct,
            )
```

`crates/rurge-engine/src/engine.rs`——把

```rust
            if let Some(rurge_policy::Note::Unsupported(kind)) = &resolution.note {
                // The policy is sound but rurge cannot speak it yet, so the
                // outbound below is REJECT; say so in the session log (§7.2).
                handle.set_error(format!("policy protocol not implemented: {kind}"));
            }
```

换成

```rust
            if let Some(note) = &resolution.note {
                // A protocol rurge cannot speak yet, a group cycle, a group
                // without members: say why in the session log (§7.2; phase 2
                // M3 design 5.6).
                handle.set_error(note.to_string());
            }
```

`rurge-engine` 在 Task 6 才登记订阅，这里传空快照：配置里没有 `policy-path` 的组，成员表与今天完全一样。`Note::Unsupported` 的 `Display` 与原来 `format!` 出的文字相同，`pipeline.rs` 里断言它的用例不用改。

- [ ] **Step 5: 门禁与提交**

跑门禁（全工作区 38 个测试二进制，839 通过 / 1 忽略）。

```bash
git add crates/rurge-policy crates/rurge-engine/src/runtime.rs crates/rurge-engine/src/engine.rs
git commit -m "feat(policy): 注册表接受装配结果——导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 EmptyGroup 兜底、视图数据"
```

---


### Task 6: 订阅接入——资源管理器的标签与离线读缓存、构建时同步载入、`rurge check --data-dir`

每一代把全部 `policy-path` 登记到这一代的资源管理器，读出快照做首次装配——资源管理器在登记时就同步读了磁盘缓存与本地文件（P2），所以已有缓存时，启动与重载都没有空组窗口（M3-D5）。订阅链接不进日志：资源管理器改用登记时给的标签（M3-D7）。`rurge check` 与 `POST /v1/profiles/check` 不起资源管理器，离线读缓存做装配检查（设计 5.9，P14）。本任务还没有热重建（Task 8）：订阅在运行期更新，要等下一次重载才生效。

**Files:**
- Create: `crates/rurge-engine/src/subscriptions.rs`
- Create: `crates/rurge-engine/tests/subscriptions.rs`
- Modify: `crates/rurge-net/src/resource/mod.rs`
- Modify: `crates/rurge-engine/src/{lib.rs, outbounds.rs, runtime.rs, engine.rs}`
- Modify: `crates/rurge-api/src/routes/profiles.rs`
- Modify: `crates/rurge/src/cli/check.rs`、`crates/rurge/tests/cli.rs`

**Interfaces:**
- Consumes: Task 3 的 `assemble` / `Snapshots`、Task 2 的 `subscription::parse`；`ResourceManager`、`ResourceHandle::{current, subscribe}`、`ResourceState::data`；`load_checked`。
- Produces:
  - `ResourceManager::get_labelled(&self, spec: &ResourceSpec, label: &str) -> ResourceHandle`（`get` 不变）
  - `rurge_net::resource::cached(root: &Path, source: &ResourceSource) -> Option<Bytes>`
  - `rurge_engine::check_profile(path: &Path, opts: &LoadOptions, data_dir: &Path) -> Result<Loaded, LoadError>`
  - `Engine::registry(&self) -> Arc<PolicyRegistry>`、`Engine::data_dir(&self) -> PathBuf`
  - crate 内：`subscriptions::Subscriptions::{register(cfg, resources) -> Subscriptions, snapshots(&self) -> Snapshots}`
  - `rurge check` 的 `--data-dir <DIR>`（环境变量 `RURGE_DATA_DIR`，默认平台数据目录）

- [ ] **Step 1: 资源管理器的用例先行**

`crates/rurge-net/src/resource/mod.rs`——把

```rust
        assert!(st[0].last_error.as_deref().unwrap().contains("cannot read"));
    }
}
```

换成

```rust
        assert!(st[0].last_error.as_deref().unwrap().contains("cannot read"));
    }

    #[test]
    fn cached_reads_what_a_manager_would_start_with() {
        let root = tempfile::tempdir().unwrap();
        let url = Url::parse("https://sub.test/nodes?token=t0k3n").unwrap();
        let remote = ResourceSource::Url(url.clone());
        assert!(cached(root.path(), &remote).is_none());
        let meta = Meta {
            url: url.to_string(),
            ..Meta::default()
        };
        cache::CacheDir::for_url(root.path(), url.as_str())
            .store(b"cached", &meta)
            .unwrap();
        assert_eq!(&cached(root.path(), &remote).unwrap()[..], b"cached");
        let file = root.path().join("nodes.txt");
        let local = ResourceSource::File(file.clone());
        assert!(cached(root.path(), &local).is_none());
        std::fs::write(&file, "local").unwrap();
        assert_eq!(&cached(root.path(), &local).unwrap()[..], b"local");
    }

    /// A subscription URL usually carries a token: once somebody labels the
    /// resource, no log line names it by its URL any more.
    #[tokio::test]
    async fn a_labelled_resource_is_logged_by_its_label() {
        let root = tempfile::tempdir().unwrap();
        let offline = ResourceOptions {
            offline: true,
            ..fast()
        };
        let mgr = manager(root.path(), offline);
        let url = Url::parse("https://sub.test/nodes?token=t0k3n").unwrap();
        let plain = mgr.get(&url_spec(url.clone(), None));
        assert_eq!(plain.entry.log_name(), url.as_str());
        let labelled = mgr.get_labelled(&url_spec(url.clone(), None), "policy-path of `G`");
        assert!(Arc::ptr_eq(&plain.entry, &labelled.entry));
        assert_eq!(plain.entry.log_name(), "policy-path of `G`");
        // the first label stays
        mgr.get_labelled(&url_spec(url, None), "policy-path of `H`");
        assert_eq!(plain.entry.log_name(), "policy-path of `G`");
    }
}
```

Run: `cargo test -p rurge-net resource`

Expected: 编译错误——`cannot find function `cached``、`no method named `get_labelled``、`no method named `log_name``。

- [ ] **Step 2: 标签与离线读缓存**

`crates/rurge-net/src/resource/mod.rs`——把

```rust
struct Entry {
    source: ResourceSource,
    state: Mutex<ResourceState>,
```

换成

```rust
struct Entry {
    source: ResourceSource,
    /// What log lines call the resource instead of its URL (`get_labelled`).
    label: Mutex<Option<String>>,
    state: Mutex<ResourceState>,
```

`crates/rurge-net/src/resource/mod.rs`——把

```rust
impl Entry {
    fn version(&self) -> u64 {
```

换成

```rust
impl Entry {
    /// What log lines call this resource: its label, else its source.
    fn log_name(&self) -> String {
        match &*self.label.lock().expect("label") {
            Some(label) => label.clone(),
            None => self.source.to_string(),
        }
    }

    fn version(&self) -> u64 {
```

`crates/rurge-net/src/resource/mod.rs`——把

```rust
    Duration::from_millis(u64::try_from(adjusted).unwrap_or(0))
}
```

换成

```rust
    Duration::from_millis(u64::try_from(adjusted).unwrap_or(0))
}

/// What `get` would start a resource with, read without a manager and
/// without the network: the disk cache of a URL under `root`, the content of
/// a file. For offline checks.
pub fn cached(root: &Path, source: &ResourceSource) -> Option<Bytes> {
    match source {
        ResourceSource::Url(url) => cache::CacheDir::for_url(root, url.as_str())
            .load()
            .map(|(data, _)| data),
        ResourceSource::File(path) => std::fs::read(path).ok().map(Bytes::from),
    }
}
```

`crates/rurge-net/src/resource/mod.rs`——把

```rust
    pub fn get(&self, spec: &ResourceSpec) -> ResourceHandle {
        let key = spec.source.key();
```

换成

```rust
    pub fn get(&self, spec: &ResourceSpec) -> ResourceHandle {
        self.register(spec, None)
    }

    /// `get` for a resource whose URL must not reach the logs — a
    /// subscription URL usually carries a token (phase 2 M3-D7): log lines
    /// call it `label` instead, also when another caller shares it. The first
    /// label a resource gets is the one it keeps.
    pub fn get_labelled(&self, spec: &ResourceSpec, label: &str) -> ResourceHandle {
        self.register(spec, Some(label))
    }

    fn register(&self, spec: &ResourceSpec, label: Option<&str>) -> ResourceHandle {
        let key = spec.source.key();
```

`crates/rurge-net/src/resource/mod.rs`——把

```rust
                merge_interval(existing, spec.update_interval);
                (existing.clone(), false)
```

换成

```rust
                merge_interval(existing, spec.update_interval);
                if let Some(label) = label {
                    existing
                        .label
                        .lock()
                        .expect("label")
                        .get_or_insert_with(|| label.to_string());
                }
                (existing.clone(), false)
```

`crates/rurge-net/src/resource/mod.rs`——把

```rust
                    source: spec.source.clone(),
                    state: Mutex::new(ResourceState::Missing),
```

换成

```rust
                    source: spec.source.clone(),
                    label: Mutex::new(label.map(str::to_string)),
                    state: Mutex::new(ResourceState::Missing),
```

`crates/rurge-net/src/resource/mod.rs`——把

```rust
                        tracing::warn!(path = %path.display(), error = %e, "cannot watch file; changes need a reload");
```

换成

```rust
                        tracing::warn!(resource = %entry.log_name(), error = %e, "cannot watch file; changes need a reload");
```

`crates/rurge-net/src/resource/mod.rs`——把

```rust
                    tracing::warn!(url = %url, error = %e, "cannot write resource cache");
```

换成

```rust
                    tracing::warn!(resource = %entry.log_name(), error = %e, "cannot write resource cache");
```

`crates/rurge-net/src/resource/mod.rs`——把

```rust
                tracing::info!(url = %url, version, "resource updated");
```

换成

```rust
                tracing::info!(resource = %entry.log_name(), version, "resource updated");
```

`crates/rurge-net/src/resource/mod.rs`——把

```rust
                tracing::warn!(url = %url, error = %e, "resource fetch failed");
```

换成

```rust
                tracing::warn!(resource = %entry.log_name(), error = %e, "resource fetch failed");
```

要点：四处日志字段从 `url` / `path` 统一成 `resource = <标签或来源>`（规则集没有标签，仍显示 URL，只是字段名变了）；同一来源被多次登记时，**第一个**标签保留，后来者不能改名，也不能把已有的标签抹掉。

Run: `cargo test -p rurge-net resource` → 15 passed。

- [ ] **Step 3: 引擎的 `subscriptions` 模块——用例先行**

`crates/rurge-engine/src/lib.rs`——把

```rust
pub mod state;
pub mod views;
```

换成

```rust
pub mod state;
mod subscriptions;
pub mod views;
```

`crates/rurge-engine/src/lib.rs`——把

```rust
pub use shared::{EngineShared, ResolverCell};
```

换成

```rust
pub use shared::{EngineShared, ResolverCell};
pub use subscriptions::check_profile;
```

新建 `crates/rurge-engine/src/subscriptions.rs`，先只写文件头的 `use` 与文件末尾的 `#[cfg(test)] mod tests { … }`（全文见 Step 4）。

Run: `cargo test -p rurge-engine --lib subscriptions`

Expected: 编译错误——`cannot find function `subscription_diagnostics``、`cannot find function `check_profile``。

- [ ] **Step 4: 写 `subscriptions.rs`，接进 `Runtime::build`**

`crates/rurge-engine/src/subscriptions.rs` 全文（Task 6 的版本；Task 8 在它上面加热重建）：

```rust
//! The `policy-path` subscriptions of one config generation (phase 2 M3
//! design 5.1, 5.9): registered with its resource manager, read into
//! snapshots for the assembly, and checked offline for `rurge check`.

use rurge_config::config::{LoadError, LoadOptions, Loaded};
use rurge_config::spec::PolicyPath;
use rurge_config::{Config, Diagnostics};
use rurge_net::resource::{ResourceHandle, ResourceManager, ResourceSource, ResourceSpec};
use rurge_policy::{Snapshots, assemble, subscription};
use std::path::Path;
use std::sync::Arc;

pub(crate) struct Subscriptions {
    /// One per source, in the order the groups first name them.
    handles: Vec<(PolicyPath, ResourceHandle)>,
}

impl Subscriptions {
    /// Registers every `policy-path` with this generation's resource
    /// manager. What earlier runs cached is loaded right here, synchronously
    /// and without the network, so a start or a reload keeps the members it
    /// had (M3-D5). Log lines name a source after the first group that uses
    /// it, never by its URL (M3-D7).
    pub(crate) fn register(cfg: &Config, resources: &ResourceManager) -> Subscriptions {
        let mut handles: Vec<(PolicyPath, ResourceHandle)> = Vec::new();
        for g in &cfg.group_specs {
            let Some(path) = &g.import.policy_path else {
                continue;
            };
            let spec = ResourceSpec {
                source: source_of(path),
                update_interval: g
                    .import
                    .update_interval
                    .map(|secs| i64::try_from(secs).unwrap_or(i64::MAX)),
            };
            // every group registers: a shared source refreshes at the
            // shortest interval any of them asks for
            let label = format!("policy-path of `{}`", g.name);
            let handle = resources.get_labelled(&spec, &label);
            if !handles.iter().any(|(p, _)| p == path) {
                handles.push((path.clone(), handle));
            }
        }
        Subscriptions { handles }
    }

    /// What every subscription holds right now; one that holds nothing yet
    /// is absent.
    pub(crate) fn snapshots(&self) -> Snapshots {
        self.handles
            .iter()
            .filter_map(|(path, handle)| {
                let (data, _) = handle.current().data()?;
                let text = String::from_utf8_lossy(&data);
                Some((path.clone(), Arc::new(subscription::parse(&text))))
            })
            .collect()
    }
}

fn source_of(path: &PolicyPath) -> ResourceSource {
    match path {
        PolicyPath::Url(url) => ResourceSource::Url(url.expose().clone()),
        PolicyPath::File(file) => ResourceSource::File(file.clone()),
    }
}

/// The warnings an assembly from what earlier runs cached gives (M3 design
/// 5.9). Offline: reads the data directory and local files, nothing else.
fn subscription_diagnostics(cfg: &Config, data_dir: &Path) -> Diagnostics {
    let mut snapshots = Snapshots::new();
    for g in &cfg.group_specs {
        let Some(path) = &g.import.policy_path else {
            continue;
        };
        if snapshots.contains_key(path) {
            continue;
        }
        if let Some(data) = rurge_net::resource::cached(data_dir, &source_of(path)) {
            let text = String::from_utf8_lossy(&data);
            snapshots.insert(path.clone(), Arc::new(subscription::parse(&text)));
        }
    }
    assemble(cfg, &snapshots).diagnostics
}

/// `load_checked`, plus — when the profile itself is sound — what the
/// cached subscriptions say: what `rurge check` and `POST /v1/profiles/check`
/// report.
pub fn check_profile(
    path: &Path,
    opts: &LoadOptions,
    data_dir: &Path,
) -> Result<Loaded, LoadError> {
    let mut loaded = crate::outbounds::load_checked(path, opts)?;
    if !loaded.diagnostics.has_errors() {
        loaded
            .diagnostics
            .extend(subscription_diagnostics(&loaded.config, data_dir));
    }
    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::codes;
    use rurge_config::config::from_text;

    const PROFILE: &str = "[Proxy Group]\nLocal = select, DIRECT, policy-path=nodes.txt\n\
Remote = select, DIRECT, policy-path=https://sub.test/nodes?token=t0k3n\n[Rule]\nFINAL,Local\n";

    fn messages(d: &Diagnostics) -> Vec<(&'static str, String)> {
        d.iter().map(|d| (d.code, d.message.clone())).collect()
    }

    /// A URL is looked for in the cache only: never fetched, never printed.
    #[test]
    fn the_offline_check_reads_files_and_caches_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("nodes.txt"),
            "N1 = http, n1.test, 80\nnot a policy\n",
        )
        .unwrap();
        let loaded = from_text(
            PROFILE,
            &dir.path().join("t.conf"),
            &LoadOptions::for_tests(),
        );
        assert!(!loaded.diagnostics.has_errors());
        let data = tempfile::tempdir().unwrap();
        assert_eq!(
            messages(&subscription_diagnostics(&loaded.config, data.path())),
            [
                (
                    codes::W_SET_LINES_SKIPPED,
                    "policy group `Local`: `policy-path` line 2 skipped: not a policy line (`Name = type, ...`)".to_string()
                ),
                (
                    codes::W_RESOURCE_UNAVAILABLE,
                    "policy group `Remote`: `policy-path` has no content yet (never downloaded, or the file cannot be read); its imported members are unknown".to_string()
                ),
            ]
        );
    }

    #[test]
    fn a_profile_with_errors_gets_no_subscription_warnings() {
        let dir = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let sound = dir.path().join("sound.conf");
        std::fs::write(&sound, PROFILE).unwrap();
        let loaded = check_profile(&sound, &LoadOptions::for_tests(), data.path()).unwrap();
        let found = messages(&loaded.diagnostics);
        assert!(
            found
                .iter()
                .any(|(code, _)| *code == codes::W_RESOURCE_UNAVAILABLE),
            "{found:?}"
        );
        assert!(found.iter().all(|(_, m)| !m.contains("t0k3n")), "{found:?}");
        let broken = dir.path().join("broken.conf");
        std::fs::write(&broken, PROFILE.replace("FINAL,Local", "FINAL,Nope")).unwrap();
        let loaded = check_profile(&broken, &LoadOptions::for_tests(), data.path()).unwrap();
        assert!(loaded.diagnostics.has_errors());
        assert!(
            loaded
                .diagnostics
                .iter()
                .all(|d| d.code != codes::W_RESOURCE_UNAVAILABLE)
        );
    }
}
```

`crates/rurge-engine/src/runtime.rs`——把

```rust
use crate::stack::{Stack, StackOptions, build_stack};
```

换成

```rust
use crate::stack::{Stack, StackOptions, build_stack};
use crate::subscriptions::Subscriptions;
```

`crates/rurge-engine/src/runtime.rs`——把

```rust
        let stack = build_stack(&config, &opts.stack).await?;
```

换成

```rust
        let mut stack = build_stack(&config, &opts.stack).await?;
```

`crates/rurge-engine/src/runtime.rs`——把

```rust
        // The generation being replaced (none on the first build): whatever
        // it built from the same fingerprint is kept, connection pools and all.
        let previous = opts.shared.cell.load();
```

换成

```rust
        // What earlier runs cached is in the first assembly already: no group
        // starts empty for want of a download (M3-D5).
        let subscriptions = Subscriptions::register(&config, &stack.resources);
        let assembly = rurge_policy::assemble(&config, &subscriptions.snapshots());
        stack.diagnostics.extend(assembly.diagnostics.clone());
        // The generation being replaced (none on the first build): whatever
        // it built from the same fingerprint is kept, connection pools and all.
        let previous = opts.shared.cell.load();
```

`crates/rurge-engine/src/runtime.rs`——把

```rust
                &config,
                &rurge_policy::assemble(&config, &rurge_policy::Snapshots::new()),
                &factory,
```

换成

```rust
                &config,
                &assembly,
                &factory,
```

`crates/rurge-engine/src/engine.rs`——把

```rust
    /// The current config generation (sessions snapshot it once at dial time).
    pub fn runtime(&self) -> Arc<Runtime> {
        self.runtime.load_full()
    }
```

换成

```rust
    /// The current config generation (sessions snapshot it once at dial time).
    pub fn runtime(&self) -> Arc<Runtime> {
        self.runtime.load_full()
    }

    /// The registry the engine resolves against right now: the one in
    /// `EngineShared.cell`.
    pub fn registry(&self) -> Arc<rurge_policy::PolicyRegistry> {
        self.shared
            .cell
            .load()
            .expect("published before the engine is handed out")
    }
```

`crates/rurge-engine/src/engine.rs`——把

```rust
    /// The main profile file of the current config generation.
    pub fn profile_path(&self) -> std::path::PathBuf {
        self.runtime().config.source.main.clone()
    }
```

换成

```rust
    /// The main profile file of the current config generation.
    pub fn profile_path(&self) -> std::path::PathBuf {
        self.runtime().config.source.main.clone()
    }

    /// Where this engine keeps its caches.
    pub fn data_dir(&self) -> std::path::PathBuf {
        self.runtime().stack.resources.root().to_path_buf()
    }
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
/// `rurge_config::config::load` plus the dry build: the one way `check`,
/// `run`, a reload and `POST /v1/profiles/check` read a profile.
```

换成

```rust
/// `rurge_config::config::load` plus the dry build: the one way `run`, a
/// reload and — under `check_profile` — `check` and `POST /v1/profiles/check`
/// read a profile.
```

要点：每个组都登记一次（同一来源被几个组共用时，资源管理器取其中最短的 `update-interval`），但每个来源只留一个句柄；标签用第一个组的名字。装配的告警并进 `Runtime` 的诊断，`rurge run` 在启动与重载时打印它们（与规则集的 `W0022` 同一条路）。`check_profile` 只在配置本身无错时才读缓存装配——有错的配置，订阅的告警没有意义。

Run: `cargo test -p rurge-engine --lib subscriptions` → 2 passed。

- [ ] **Step 5: 引擎的集成用例**

新建 `crates/rurge-engine/tests/subscriptions.rs`（Task 6 的版本；Task 7 – 9 往里加用例）：

```rust
//! `policy-path` subscriptions through the whole engine (phase 2 M3 design
//! §5): what the registry holds from the first build on.

mod common;
use common::*;

/// A local subscription is read while the generation is built: the group
/// has its members from the first dial on (M3-D5).
#[tokio::test]
async fn a_subscription_file_is_in_from_the_first_generation() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("nodes.txt"),
        "N1 = http, n1.test, 80\nN2 = http, n2.test, 80\n",
    )
    .unwrap();
    let dns = MockDns::spawn().await;
    let text = Profile {
        groups: "Sub = select, policy-path=nodes.txt",
        ..Profile::default()
    }
    .text(dns.addr());
    let engine = Engine::new(runtime(dir.path(), &text, EngineShared::default()).await);
    assert_eq!(engine.registry().members("Sub").unwrap(), ["N1", "N2"]);
}
```

Run: `cargo test -p rurge-engine --test subscriptions` → 1 passed。它钉住 M3-D5 的"同步载入"：本地订阅在第一代构建时就读进来，`Engine::new` 之后成员立即在。

- [ ] **Step 6: `rurge check --data-dir` 与 API 的检查**

`crates/rurge-api/src/routes/profiles.rs`——把

```rust
use rurge_engine::load_checked;
```

换成

```rust
use rurge_engine::check_profile;
```

`crates/rurge-api/src/routes/profiles.rs`——把

```rust
/// Re-validates the profile on disk with the daemon's load options; never
/// touches the running config. The dry build runs too, so a policy that
/// cannot be built is reported here exactly as a reload would report it.
pub async fn check(State(app): State<App>) -> ApiResult<Json<CheckJson>> {
    let path = app.engine.profile_path();
    let opts = app.load_options.clone();
    let loaded = tokio::task::spawn_blocking(move || load_checked(&path, &opts))
```

换成

```rust
/// Re-validates the profile on disk with the daemon's load options; never
/// touches the running config. The dry build runs too, so a policy that
/// cannot be built is reported here exactly as a reload would report it, and
/// so does an offline assembly from the cached subscriptions.
pub async fn check(State(app): State<App>) -> ApiResult<Json<CheckJson>> {
    let path = app.engine.profile_path();
    let opts = app.load_options.clone();
    let data_dir = app.engine.data_dir();
    let loaded = tokio::task::spawn_blocking(move || check_profile(&path, &opts, &data_dir))
```

`crates/rurge/src/cli/check.rs`——把

```rust
    /// Override the CORE_VERSION reported to requirement expressions
    #[arg(long)]
    pub core_version: Option<u64>,
}
```

换成

```rust
    /// Override the CORE_VERSION reported to requirement expressions
    #[arg(long)]
    pub core_version: Option<u64>,
    /// Data directory whose cached subscriptions are checked too (default:
    /// the platform data dir); nothing is downloaded
    #[arg(long, env = "RURGE_DATA_DIR", value_name = "DIR")]
    pub data_dir: Option<PathBuf>,
}
```

`crates/rurge/src/cli/check.rs`——把

```rust
    let loaded = rurge_engine::load_checked(&args.config, &opts)?;
```

换成

```rust
    let data_dir = args
        .data_dir
        .clone()
        .unwrap_or_else(rurge_platform::dirs::data_dir);
    let loaded = rurge_engine::check_profile(&args.config, &opts, &data_dir)?;
```

`crates/rurge/tests/cli.rs`——把

```rust
mod rule_match {
    use assert_cmd::Command;
```

换成

```rust
const SUBSCRIBED: &str = "[General]\n[Proxy Group]\nLocal = select, DIRECT, policy-path=nodes.txt\n\
Remote = select, DIRECT, policy-path=https://sub.test/nodes?token=t0k3n\n[Rule]\nFINAL,Local\n";

/// Offline: the local file is read and the URL looked for in the data
/// directory's cache — and the URL is never printed (M3 design 5.9, M3-D7).
#[test]
fn check_assembles_the_subscriptions_it_has() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir, "nodes.txt", "N1 = http, n1.test, 80\nnot a policy\n");
    let data = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "--data-dir"])
        .arg(data.path())
        .arg("-c")
        .arg(write(&dir, "sub.conf", SUBSCRIBED))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    assert!(
        out.contains("W0023") && out.contains("policy group `Local`: `policy-path` line 2 skipped"),
        "{out}"
    );
    assert!(
        out.contains("W0022")
            && out.contains("policy group `Remote`: `policy-path` has no content yet"),
        "{out}"
    );
    assert!(!out.contains("t0k3n"), "{out}");
}

mod rule_match {
    use assert_cmd::Command;
```

Run: `cargo test -p rurge --test cli check_` → 6 passed（新增 `check_assembles_the_subscriptions_it_has`）。现有的 `check` 用例不传 `--data-dir`：它们的配置里没有 `policy-path`，不会去读平台数据目录。

- [ ] **Step 7: 门禁与提交**

跑门禁（全工作区 39 个测试二进制，845 通过 / 1 忽略）。

```bash
git add crates/rurge-net/src/resource/mod.rs crates/rurge-engine crates/rurge-api/src/routes/profiles.rs crates/rurge/src/cli/check.rs crates/rurge/tests/cli.rs
git commit -m "feat(engine): 订阅接入——构建时同步载入缓存、资源日志用标签不写 URL、rurge check --data-dir 离线检查订阅"
```

---


### Task 7: 拨号与视图改读注册表；`EmptyGroup` 与 `--empty-group-reject`

只换注册表而不换整代（Task 8 的订阅重建）的前提：拨号、视图、`policies_view`、`policy_exists` 都从 `EngineShared.cell` 取当前的注册表，而不是从 `Runtime`（设计 5.8，P5）。`Runtime.policies` 去掉，换成只在发布时被取走一次的 `registry`。空组策略随引擎存续：`EngineShared.empty_group`，由 bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 设置（M3-D3，P15）。

**Files:**
- Modify: `crates/rurge-engine/src/{shared.rs, lib.rs, runtime.rs, reload.rs, engine.rs, views.rs}`
- Modify: `crates/rurge-engine/tests/{common/mod.rs, outbounds.rs, subscriptions.rs}`
- Modify: `crates/rurge/src/cli/run.rs`

**Interfaces:**
- Consumes: Task 5 的 `PolicyRegistry::{group, group_names, policy_names, line, spec, members, current_member, contains, resolve}`、`EmptyGroup`；Task 6 的 `Engine::registry()`。
- Produces:
  - `EngineShared.empty_group: EmptyGroup`（`EngineShared::new` 里是 `Direct`）；`rurge_engine::EmptyGroup`（再导出）
  - `Runtime` 不再有 `policies` 字段；crate 内 `Runtime.registry: Option<Arc<PolicyRegistry>>`；`Engine::publish_generation(&self, next: &mut Runtime)`；`Engine::swap_runtime(self: &Arc<Self>, next: Runtime)`（签名对外不变）
  - 视图（`groups_view`、`policy_detail`、`group_selection`、`select_group`）与 `policies_view` 读注册表：成员是装配后的，导入与派生的策略可查（脱敏）
  - `rurge run --empty-group-reject`（环境变量 `RURGE_EMPTY_GROUP_REJECT`，`true` / `false`）
  - 测试夹具：`common::harness_with(p: Profile<'_>, shared: EngineShared) -> Harness` 改为 `pub`

- [ ] **Step 1: 用例先行**

引擎集成用例（空组的两种兜底经真实的 `dial`；视图看到导入与派生的策略）：

`crates/rurge-engine/tests/subscriptions.rs`——把

```rust
    let engine = Engine::new(runtime(dir.path(), &text, EngineShared::default()).await);
    assert_eq!(engine.registry().members("Sub").unwrap(), ["N1", "N2"]);
}
```

换成

```rust
    let engine = Engine::new(runtime(dir.path(), &text, EngineShared::default()).await);
    assert_eq!(engine.registry().members("Sub").unwrap(), ["N1", "N2"]);
}

/// A dial through a group without members: DIRECT stands in by default,
/// REJECT with `--empty-group-reject`; the session says which (M3-D3).
#[tokio::test]
async fn an_empty_group_stands_in_direct_or_rejects() {
    let origin = TestServer::spawn().await;
    let profile = || Profile {
        groups: "Sub = select, policy-path=missing.txt",
        rules: "DOMAIN,empty.test,Sub",
        ..Profile::default()
    };
    let session = || {
        SessionInfo::tcp(
            rurge_config::HostName::parse("empty.test"),
            origin_addr(&origin).port(),
        )
    };

    let h = harness(profile()).await;
    h.dns.set("empty.test", &["127.0.0.1"], &[], 60);
    let Ok(dialed) = h.engine.dial(session()).await else {
        panic!("DIRECT stands in")
    };
    assert_eq!(
        dialed.handle.error().as_deref(),
        Some("policy group has no members; DIRECT substituted")
    );
    assert_eq!(dialed.handle.policy_chain(), ["Sub", "DIRECT"]);

    let shared = EngineShared {
        empty_group: EmptyGroup::Reject,
        ..EngineShared::default()
    };
    let h = harness_with(profile(), shared).await;
    match h.engine.dial(session()).await {
        Err(DialError::Reject { kind, handle, .. }) => {
            assert_eq!(kind, rurge_proto::RejectKind::Reject);
            assert_eq!(
                handle.error().as_deref(),
                Some("policy group has no members")
            );
        }
        Err(DialError::Failed { message, .. }) => panic!("expected a reject, failed: {message}"),
        Ok(_) => panic!("expected a reject, got a stream"),
    }
}

/// The control plane sees what the registry holds: imported and derived
/// policies, with their secrets blanked (M3 design §8, M3-D7).
#[tokio::test]
async fn the_views_show_imported_and_derived_policies() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("nodes.txt"),
        "N1 = trojan, n1.test, 443, password=s3cret\nN2 = http, n2.test, 80\n",
    )
    .unwrap();
    let dns = MockDns::spawn().await;
    let text = Profile {
        proxies: "R = http, r.test, 80",
        groups: "Sub = select, DIRECT, policy-path=nodes.txt\n\
Chained = select, include-other-group=Sub, underlying-proxy=R",
        ..Profile::default()
    }
    .text(dns.addr());
    let engine = Engine::new(runtime(dir.path(), &text, EngineShared::default()).await);

    let proxies = engine.policies_view().proxies;
    assert_eq!(
        proxies[5..],
        ["R", "N1", "N2", "N1 (via R)", "N2 (via R)"],
        "{proxies:?}"
    );
    let groups = engine.groups_view();
    let chained = groups.iter().find(|g| g.name == "Chained").unwrap();
    let members: Vec<(&str, &str)> = chained
        .members
        .iter()
        .map(|m| (m.name.as_str(), m.type_description.as_str()))
        .collect();
    assert_eq!(
        members,
        [
            ("DIRECT", "DIRECT"),
            ("N1 (via R)", "trojan"),
            ("N2 (via R)", "http")
        ]
    );
    assert_eq!(
        engine.policy_detail("N1").as_deref(),
        Some("trojan, n1.test, 443, password=***")
    );
    assert_eq!(
        engine.policy_detail("N1 (via R)").as_deref(),
        Some("trojan, n1.test, 443, password=***, underlying-proxy=R")
    );
    assert_eq!(
        engine.policy_detail("Sub").as_deref(),
        Some("select, DIRECT, policy-path=***")
    );

    engine.select_group("Sub", "N2").await.unwrap();
    assert_eq!(engine.group_selection("Sub").unwrap(), "N2");
    assert_eq!(
        engine.select_group("Sub", "N1 (via R)").await,
        Err(SelectError::NotAMember {
            group: "Sub".into(),
            member: "N1 (via R)".into()
        })
    );
}
```

`crates/rurge-engine/tests/subscriptions.rs`——把

```rust
mod common;
use common::*;
```

换成

```rust
mod common;
use common::*;
use rurge_config::session::SessionInfo;
use rurge_engine::EmptyGroup;
use rurge_inbound::{DialError, Dialer};
```

夹具与两处旧写法：

`crates/rurge-engine/tests/common/mod.rs`——把

```rust
async fn harness_with(p: Profile<'_>, shared: EngineShared) -> Harness {
```

换成

```rust
pub async fn harness_with(p: Profile<'_>, shared: EngineShared) -> Harness {
```

`crates/rurge-engine/tests/common/mod.rs`——把

```rust
pub fn outbound_now(h: &Harness, name: &str) -> rurge_proto::OutboundRef {
    h.engine
        .runtime()
        .policies
        .resolve(&rurge_config::rule::PolicyRef::parse(name))
        .outbound
}
```

换成

```rust
pub fn outbound_now(h: &Harness, name: &str) -> rurge_proto::OutboundRef {
    h.engine
        .registry()
        .resolve(&rurge_config::rule::PolicyRef::parse(name))
        .outbound
}
```

`crates/rurge-engine/tests/outbounds.rs`——把

```rust
    assert_eq!(
        h.engine
            .runtime()
            .policies
            .current_member("Pick")
            .as_deref(),
        Some("A")
    );
```

换成

```rust
    assert_eq!(
        h.engine.registry().current_member("Pick").as_deref(),
        Some("A")
    );
```

`crates/rurge-engine/tests/outbounds.rs`——把

```rust
    let old = h
        .engine
        .runtime()
        .policies
        .resolve(&rurge_config::rule::PolicyRef::parse("S"))
        .outbound;
```

换成

```rust
    let old = h
        .engine
        .registry()
        .resolve(&rurge_config::rule::PolicyRef::parse("S"))
        .outbound;
```

`views.rs` 的用例改为在注册表上算 `lineHash`（辅助函数改名 `registry`，13 处调用随之改名）：

`crates/rurge-engine/src/views.rs`——把

```rust
    use super::*;
    use rurge_config::config::{LoadOptions, from_text};
    use std::path::Path;

    fn config(policy_line: &str) -> Config {
        let text = format!("[General]\n[Proxy]\n{policy_line}\n[Rule]\nFINAL,DIRECT\n");
        let loaded = from_text(&text, Path::new("t.conf"), &LoadOptions::for_tests());
        assert!(
            !loaded.diagnostics.has_errors(),
            "{:?}",
            loaded
                .diagnostics
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
        );
        loaded.config
    }
```

换成

```rust
    use super::*;
    use crate::outbounds::EngineFactory;
    use rurge_config::config::{LoadOptions, from_text};
    use rurge_net::connector::SystemResolve;
    use rurge_net::socket::NoopSocketHook;
    use rurge_policy::{EmptyGroup, RegistryCell, SelectionTable, Snapshots, assemble};
    use std::path::Path;
    use std::sync::Arc;

    fn registry(policy_line: &str) -> PolicyRegistry {
        let text = format!("[General]\n[Proxy]\n{policy_line}\n[Rule]\nFINAL,DIRECT\n");
        let loaded = from_text(&text, Path::new("t.conf"), &LoadOptions::for_tests());
        assert!(
            !loaded.diagnostics.has_errors(),
            "{:?}",
            loaded
                .diagnostics
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
        );
        let cfg = loaded.config;
        let factory = EngineFactory::new(&cfg, Arc::new(SystemResolve), Arc::new(NoopSocketHook));
        PolicyRegistry::build(
            &cfg,
            &assemble(&cfg, &Snapshots::new()),
            &factory,
            &RegistryCell::new(),
            Arc::new(SelectionTable::default()),
            None,
            EmptyGroup::Direct,
        )
        .expect("builds")
    }
```

`crates/rurge-engine/src/views.rs`——把

```rust
        // the hash is over the literal redacted line — computed here
        // independently rather than hard-coding `***` spacing
        let p = a.policies.iter().find(|p| p.name == "A").unwrap();
        let expected = format!(
            "{} = {}",
            p.name,
            rurge_config::redact::redact_definition(&p.definition)
        );
```

换成

```rust
        // the hash is over the literal redacted line — computed here
        // independently rather than hard-coding `***` spacing
        let expected = format!(
            "A = {}",
            rurge_config::redact::redact_definition(&a.line("A").unwrap().definition)
        );
```

`crates/rurge-engine/src/views.rs`（共 13 处，全部替换）——把

```rust
config(
```

换成

```rust
registry(
```

Run: `cargo test -p rurge-engine --tests --lib`

Expected: 编译错误——`no field `empty_group` on type `EngineShared``、`unresolved import `rurge_engine::EmptyGroup``，以及 `views.rs` 用例里 `member_view` 的参数类型不符（它还收 `&Config`）。

- [ ] **Step 2: `EngineShared.empty_group` 与导出**

`crates/rurge-engine/src/shared.rs`——把

```rust
use rurge_policy::{GroupSelections, RegistryCell, SelectionTable};
```

换成

```rust
use rurge_policy::{EmptyGroup, GroupSelections, RegistryCell, SelectionTable};
```

`crates/rurge-engine/src/shared.rs`——把

```rust
    /// engine lives: a reload reuses outbounds by a fingerprint the roots are
    /// not part of. Tests bring their own CA this way.
    pub roots: Option<Arc<RootCertStore>>,
}
```

换成

```rust
    /// engine lives: a reload reuses outbounds by a fingerprint the roots are
    /// not part of. Tests bring their own CA this way.
    pub roots: Option<Arc<RootCertStore>>,
    /// What a group without members resolves to (M3-D3):
    /// `--empty-group-reject` sets it once, for the engine's lifetime.
    pub empty_group: EmptyGroup,
}
```

`crates/rurge-engine/src/shared.rs`——把

```rust
            resolver: ResolverCell::new(),
            roots: None,
        }
```

换成

```rust
            resolver: ResolverCell::new(),
            roots: None,
            empty_group: EmptyGroup::Direct,
        }
```

`crates/rurge-engine/src/lib.rs`——把

```rust
pub use rurge_inbound::Running;
```

换成

```rust
pub use rurge_inbound::Running;
pub use rurge_policy::EmptyGroup;
```

- [ ] **Step 3: `Runtime` 交出注册表；发布时取走**

`crates/rurge-engine/src/runtime.rs`——把

```rust
    pub rules: RuleEngine,
    pub policies: Arc<PolicyRegistry>,
    pub outbound_mode: OutboundMode,
```

换成

```rust
    pub rules: RuleEngine,
    pub outbound_mode: OutboundMode,
```

`crates/rurge-engine/src/runtime.rs`——把

```rust
    /// attaches itself to it so DNS upstream connections take the dial pipeline.
    pub(crate) dns_pipeline: Option<Arc<crate::dns_pipeline::PipelineConnector>>,
}
```

换成

```rust
    /// attaches itself to it so DNS upstream connections take the dial pipeline.
    pub(crate) dns_pipeline: Option<Arc<crate::dns_pipeline::PipelineConnector>>,
    /// The registry built with this generation, until the engine publishes
    /// it: from then on the one in use is `EngineShared.cell`'s (M3 design
    /// 5.8).
    pub(crate) registry: Option<Arc<PolicyRegistry>>,
}
```

`crates/rurge-engine/src/runtime.rs`——把

```rust
        let policies = Arc::new(
            PolicyRegistry::build(
```

换成

```rust
        let registry = Arc::new(
            PolicyRegistry::build(
```

`crates/rurge-engine/src/runtime.rs`——把

```rust
                previous.as_deref(),
                rurge_policy::EmptyGroup::Direct,
            )
```

换成

```rust
                previous.as_deref(),
                opts.shared.empty_group,
            )
```

`crates/rurge-engine/src/runtime.rs`——把

```rust
            rules,
            policies,
            outbound_mode: opts.outbound_mode,
```

换成

```rust
            rules,
            outbound_mode: opts.outbound_mode,
```

`crates/rurge-engine/src/runtime.rs`——把

```rust
            shared: opts.shared,
            dns_pipeline,
        })
```

换成

```rust
            shared: opts.shared,
            dns_pipeline,
            registry: Some(registry),
        })
```

`crates/rurge-engine/src/reload.rs`——把

```rust
    pub fn swap_runtime(self: &std::sync::Arc<Self>, next: Runtime) -> bool {
```

换成

```rust
    pub fn swap_runtime(self: &std::sync::Arc<Self>, mut next: Runtime) -> bool {
```

`crates/rurge-engine/src/reload.rs`——把

```rust
        self.publish_generation(&next);
        self.store_runtime(next);
```

换成

```rust
        self.publish_generation(&mut next);
        self.store_runtime(next);
```

- [ ] **Step 4: 拨号改读注册表**

`crates/rurge-engine/src/engine.rs`——把

```rust
use rurge_policy::TerminalKind;
```

换成

```rust
use rurge_policy::{PolicyRegistry, TerminalKind};
```

`crates/rurge-engine/src/engine.rs`——把

```rust
    pub fn new(runtime: Runtime) -> Arc<Engine> {
```

换成

```rust
    pub fn new(mut runtime: Runtime) -> Arc<Engine> {
```

`crates/rurge-engine/src/engine.rs`——把

```rust
        let shared = runtime.shared.clone();
        shared.cell.store(runtime.policies.clone());
        shared.resolver.store(runtime.stack.resolver.clone());
```

换成

```rust
        let shared = runtime.shared.clone();
        shared.cell.store(
            runtime
                .registry
                .take()
                .expect("a generation is published once"),
        );
        shared.resolver.store(runtime.stack.resolver.clone());
```

`crates/rurge-engine/src/engine.rs`——把

```rust
    /// The registry the engine resolves against right now: the one in
    /// `EngineShared.cell`.
    pub fn registry(&self) -> Arc<rurge_policy::PolicyRegistry> {
```

换成

```rust
    /// The registry the engine resolves against right now: the one in
    /// `EngineShared.cell`.
    pub fn registry(&self) -> Arc<PolicyRegistry> {
```

`crates/rurge-engine/src/engine.rs`——把

```rust
    /// Makes `next` the generation that outlives-a-reload objects see: chain
    /// connectors resolve against its registry, direct connectors through
    /// its resolver.
    pub(crate) fn publish_generation(&self, next: &Runtime) {
        assert!(
            Arc::ptr_eq(&self.shared.cell, &next.shared.cell)
                && Arc::ptr_eq(&self.shared.resolver, &next.shared.resolver),
            "the next generation must be built with `Engine::shared()`"
        );
        self.shared.cell.store(next.policies.clone());
        self.shared.resolver.store(next.stack.resolver.clone());
    }
```

换成

```rust
    /// Makes `next` the generation that outlives-a-reload objects see: chain
    /// connectors and dials resolve against its registry, direct connectors
    /// through its resolver.
    pub(crate) fn publish_generation(&self, next: &mut Runtime) {
        assert!(
            Arc::ptr_eq(&self.shared.cell, &next.shared.cell)
                && Arc::ptr_eq(&self.shared.resolver, &next.shared.resolver),
            "the next generation must be built with `Engine::shared()`"
        );
        self.shared.cell.store(
            next.registry
                .take()
                .expect("a generation is published once"),
        );
        self.shared.resolver.store(next.stack.resolver.clone());
    }
```

`crates/rurge-engine/src/engine.rs`——把

```rust
    /// `true` for the built-in policies and every configured policy / group,
    /// against the *current* runtime generation (for API / CLI callers).
    pub fn policy_exists(&self, name: &str) -> bool {
        policy_known(&self.runtime(), name)
    }
```

换成

```rust
    /// `true` for the built-in policies and every policy / group of the
    /// current registry (for API / CLI callers).
    pub fn policy_exists(&self, name: &str) -> bool {
        policy_known(&self.registry(), name)
    }
```

`crates/rurge-engine/src/engine.rs`——把

```rust
    pub fn policies_view(&self) -> PoliciesView {
        let rt = self.runtime();
        let mut proxies: Vec<String> = [
```

换成

```rust
    pub fn policies_view(&self) -> PoliciesView {
        let registry = self.registry();
        let mut proxies: Vec<String> = [
```

`crates/rurge-engine/src/engine.rs`——把

```rust
        proxies.extend(rt.config.policies.iter().map(|p| p.name.clone()));
        let groups = rt.config.groups.iter().map(|g| g.name.clone()).collect();
        PoliciesView { proxies, groups }
```

换成

```rust
        // the profile's, the imported and the derived ones (M3 design §8)
        proxies.extend(registry.policy_names());
        PoliciesView {
            proxies,
            groups: registry.group_names(),
        }
```

`crates/rurge-engine/src/engine.rs`——把

```rust
    /// Mode / rule → policy, shared by `dial` and `dial_internal` (M4 §4.1).
    async fn choose_policy(&self, rt: &Runtime, handle: &SessionHandle) -> Chosen {
        match self.mode() {
            Mode::Direct => return Chosen::Policy(PolicyRef::Builtin(Builtin::Direct)),
            Mode::Proxy => match self.global_policy() {
                Some(name) if policy_known(rt, &name) => {
```

换成

```rust
    /// Mode / rule → policy, shared by `dial` and `dial_internal` (M4 §4.1).
    async fn choose_policy(
        &self,
        rt: &Runtime,
        registry: &PolicyRegistry,
        handle: &SessionHandle,
    ) -> Chosen {
        match self.mode() {
            Mode::Direct => return Chosen::Policy(PolicyRef::Builtin(Builtin::Direct)),
            Mode::Proxy => match self.global_policy() {
                Some(name) if policy_known(registry, &name) => {
```

`crates/rurge-engine/src/engine.rs`——把

```rust
/// `true` for the built-in policies and every policy / group configured in
/// `rt` — checked against the *session's own* runtime snapshot, not whatever
/// generation is current when this runs, so a reload racing a dial can never
/// approve a name against one generation's registry and then resolve it
/// (`PolicyRegistry::resolve`) against another's.
fn policy_known(rt: &Runtime, name: &str) -> bool {
    matches!(PolicyRef::parse(name), PolicyRef::Builtin(_)) || rt.policies.contains(name)
}
```

换成

```rust
/// `true` for the built-in policies and every policy / group of `registry` —
/// the one the session loaded once, not whatever is current when this runs,
/// so a reload or a subscription update racing a dial can never approve a
/// name against one registry and then resolve it (`PolicyRegistry::resolve`)
/// against another.
fn policy_known(registry: &PolicyRegistry, name: &str) -> bool {
    matches!(PolicyRef::parse(name), PolicyRef::Builtin(_)) || registry.contains(name)
}
```

`crates/rurge-engine/src/engine.rs`——把

```rust
fn socket_opener<'a>(rt: &'a Runtime, name: &str) -> Option<&'a PolicySpec> {
    let mut current = name.to_string();
    for _ in 0..rurge_policy::registry::MAX_DEPTH {
        let spec = rt.config.spec(&current)?;
        let Some(under) = spec.common.underlying_proxy.as_deref() else {
            return Some(spec);
        };
        let below = rt.policies.resolve(&PolicyRef::Named(under.to_string()));
```

换成

```rust
fn socket_opener<'a>(registry: &'a PolicyRegistry, name: &str) -> Option<&'a PolicySpec> {
    let mut current = name.to_string();
    for _ in 0..rurge_policy::registry::MAX_DEPTH {
        let spec = registry.spec(&current)?;
        let Some(under) = spec.common.underlying_proxy.as_deref() else {
            return Some(spec);
        };
        let below = registry.resolve(&PolicyRef::Named(under.to_string()));
```

`crates/rurge-engine/src/engine.rs`——把

```rust
        let rt = self.runtime();
        let handle = self.new_handle(session);
        let policy = match self.choose_policy(&rt, &handle).await {
            Chosen::Policy(p) => p,
            // An IP-literal DNS session never needs resolution; a DnsFailed here
            // would only come from a misconfigured rule → direct.
            Chosen::DnsFailed => PolicyRef::Builtin(Builtin::Direct),
        };
        let resolution = rt.policies.resolve(&policy);
```

换成

```rust
        let rt = self.runtime();
        let registry = self.registry();
        let handle = self.new_handle(session);
        let policy = match self.choose_policy(&rt, &registry, &handle).await {
            Chosen::Policy(p) => p,
            // An IP-literal DNS session never needs resolution; a DnsFailed here
            // would only come from a misconfigured rule → direct.
            Chosen::DnsFailed => PolicyRef::Builtin(Builtin::Direct),
        };
        let resolution = registry.resolve(&policy);
```

`crates/rurge-engine/src/engine.rs`——把

```rust
            && let Some(spec) = socket_opener(&rt, terminal)
```

换成

```rust
            && let Some(spec) = socket_opener(&registry, terminal)
```

`crates/rurge-engine/src/engine.rs`——把

```rust
            let rt = self.runtime();
            let handle = self.new_handle(session);
            let policy = match self.choose_policy(&rt, &handle).await {
                Chosen::Policy(p) => p,
                Chosen::DnsFailed => return fail(handle, FailKind::Dns, "dns lookup failed"),
            };
            let resolution = rt.policies.resolve(&policy);
```

换成

```rust
            let rt = self.runtime();
            // loaded once: the name is approved and resolved against the same one
            let registry = self.registry();
            let handle = self.new_handle(session);
            let policy = match self.choose_policy(&rt, &registry, &handle).await {
                Chosen::Policy(p) => p,
                Chosen::DnsFailed => return fail(handle, FailKind::Dns, "dns lookup failed"),
            };
            let resolution = registry.resolve(&policy);
```

要点：`dial` 与 `dial_internal` 各在会话开始时取**一次**注册表，名字的校验（`policy_known`）与解析（`resolve`）、`socket_opener` 的链上查找都用这同一份——重载或订阅重建与拨号同时发生时，不会用一份注册表批准名字、再到另一份里解析它。`policy_exists` 与 `policies_view` 读当前的注册表：导入的策略名因此也能设成全局策略（P21），`GET /v1/policies` 的 `proxies` 含导入与派生的策略。

- [ ] **Step 5: 视图改读注册表**

`crates/rurge-engine/src/views.rs`——把

```rust
//! Policy groups as the control plane sees them, and the one thing it may
//! change: the selection of a `select` group (M1 design 6.3).

use crate::engine::Engine;
use crate::state::profile_key;
use rurge_config::rule::PolicyRef;
use rurge_config::{Config, GroupKind};
use sha2::{Digest, Sha256};
```

换成

```rust
//! Policy groups as the control plane sees them, and the one thing it may
//! change: the selection of a `select` group (M1 design 6.3). Everything is
//! read from the registry in use, so imported and derived members show up
//! as they come and go (phase 2 M3 design 5.5).

use crate::engine::Engine;
use crate::state::profile_key;
use rurge_config::GroupKind;
use rurge_config::rule::PolicyRef;
use rurge_policy::PolicyRegistry;
use sha2::{Digest, Sha256};
```

`crates/rurge-engine/src/views.rs`——把

```rust
    /// In profile order.
    pub members: Vec<MemberView>,
```

换成

```rust
    /// As assembled.
    pub members: Vec<MemberView>,
```

`crates/rurge-engine/src/views.rs`——把

```rust
fn member_view(cfg: &Config, name: &str) -> MemberView {
    if let Some(p) = cfg.policies.iter().find(|p| p.name == name) {
        return MemberView {
            name: name.to_string(),
            is_group: false,
            type_description: p.kind.keyword().to_string(),
            line_hash: line_hash(&format!(
                "{} = {}",
                p.name,
                rurge_config::redact::redact_definition(&p.definition)
            )),
        };
    }
    if let Some(g) = cfg.groups.iter().find(|g| g.name == name) {
        return MemberView {
            name: name.to_string(),
            is_group: true,
            type_description: g.kind.keyword().to_string(),
            line_hash: line_hash(&format!(
                "{} = {}",
                g.name,
                rurge_config::redact::redact_definition(&g.definition)
            )),
        };
    }
```

换成

```rust
fn member_view(registry: &PolicyRegistry, name: &str) -> MemberView {
    if let Some(line) = registry.line(name) {
        return MemberView {
            name: name.to_string(),
            is_group: line.is_group,
            type_description: line.keyword.to_string(),
            line_hash: line_hash(&format!(
                "{name} = {}",
                rurge_config::redact::redact_definition(&line.definition)
            )),
        };
    }
```

`crates/rurge-engine/src/views.rs`——把

```rust
    pub fn groups_view(&self) -> Vec<GroupView> {
        let rt = self.runtime();
        rt.config
            .groups
            .iter()
            .map(|g| GroupView {
                name: g.name.clone(),
                kind: g.kind,
                hidden: g.params.bool("hidden").unwrap_or(false),
                members: g
                    .members
                    .iter()
                    .map(|m| member_view(&rt.config, m))
                    .collect(),
                selected: rt.policies.current_member(&g.name),
            })
            .collect()
    }

    /// The definition of a policy or group with its secrets blanked; a
    /// built-in is described by its own name.
    pub fn policy_detail(&self, name: &str) -> Option<String> {
        let rt = self.runtime();
        if let Some(p) = rt.config.policies.iter().find(|p| p.name == name) {
            return Some(rurge_config::redact::redact_definition(&p.definition));
        }
        if let Some(g) = rt.config.groups.iter().find(|g| g.name == name) {
            return Some(rurge_config::redact::redact_definition(&g.definition));
        }
        match PolicyRef::parse(name) {
```

换成

```rust
    pub fn groups_view(&self) -> Vec<GroupView> {
        let registry = self.registry();
        registry
            .group_names()
            .into_iter()
            .filter_map(|name| {
                let g = registry.group(&name)?;
                Some(GroupView {
                    kind: g.kind,
                    hidden: g.hidden,
                    members: g
                        .members
                        .iter()
                        .map(|m| member_view(&registry, m))
                        .collect(),
                    selected: registry.current_member(&name),
                    name,
                })
            })
            .collect()
    }

    /// The definition of a policy or group with its secrets blanked — an
    /// imported policy's as imported, a derived one's with its relay; a
    /// built-in is described by its own name.
    pub fn policy_detail(&self, name: &str) -> Option<String> {
        if let Some(line) = self.registry().line(name) {
            return Some(rurge_config::redact::redact_definition(&line.definition));
        }
        match PolicyRef::parse(name) {
```

`crates/rurge-engine/src/views.rs`——把

```rust
    pub fn group_selection(&self, group: &str) -> Result<String, SelectError> {
        let rt = self.runtime();
        if !rt.config.groups.iter().any(|g| g.name == group) {
            return Err(SelectError::UnknownGroup(group.to_string()));
        }
        Ok(rt.policies.current_member(group).unwrap_or_default())
    }
```

换成

```rust
    pub fn group_selection(&self, group: &str) -> Result<String, SelectError> {
        let registry = self.registry();
        if registry.group(group).is_none() {
            return Err(SelectError::UnknownGroup(group.to_string()));
        }
        Ok(registry.current_member(group).unwrap_or_default())
    }
```

`crates/rurge-engine/src/views.rs`——把

```rust
    pub async fn select_group(&self, group: &str, member: &str) -> Result<(), SelectError> {
        let rt = self.runtime();
        let Some(g) = rt.config.groups.iter().find(|g| g.name == group) else {
            return Err(SelectError::UnknownGroup(group.to_string()));
        };
        if g.kind != GroupKind::Select {
            return Err(SelectError::NotSelectable(group.to_string()));
        }
        if !g.members.iter().any(|m| m == member) {
            return Err(SelectError::NotAMember {
                group: group.to_string(),
                member: member.to_string(),
            });
        }
        self.shared().selections.set(group, member);
        if let Some(store) = self.state_store() {
            let profile = profile_key(&rt.config.source.main);
```

换成

```rust
    pub async fn select_group(&self, group: &str, member: &str) -> Result<(), SelectError> {
        {
            let registry = self.registry();
            let Some(g) = registry.group(group) else {
                return Err(SelectError::UnknownGroup(group.to_string()));
            };
            if g.kind != GroupKind::Select {
                return Err(SelectError::NotSelectable(group.to_string()));
            }
            if !g.members.iter().any(|m| m == member) {
                return Err(SelectError::NotAMember {
                    group: group.to_string(),
                    member: member.to_string(),
                });
            }
        }
        self.shared().selections.set(group, member);
        if let Some(store) = self.state_store() {
            let profile = profile_key(&self.runtime().config.source.main);
```

要点：`member_view` 与 `policy_detail` 用注册表保存的定义行，照旧经 `redact_definition` 脱敏后才出去——导入行里的口令、派生行里的一切都一样；`select_group` 按装配后的成员表校验，持有注册表的借用不跨 `await`。

- [ ] **Step 6: 运行**

Run: `cargo test -p rurge-engine --test subscriptions` → 3 passed；`cargo test -p rurge-engine --lib views` → 1 passed；`cargo test -p rurge-engine --test outbounds` → 全部通过。

- [ ] **Step 7: bin 的开关**

`crates/rurge/src/cli/run.rs`——把

```rust
use rurge_engine::{Engine, EngineShared, ListenerSpec, Running, Runtime, RuntimeOptions};
```

换成

```rust
use rurge_engine::{
    EmptyGroup, Engine, EngineShared, ListenerSpec, Running, Runtime, RuntimeOptions,
};
```

`crates/rurge/src/cli/run.rs`——把

```rust
    /// Point the operating system's proxy settings at rurge while it runs
    #[arg(long, env = "RURGE_SYSTEM_PROXY")]
    pub system_proxy: bool,
```

换成

```rust
    /// Point the operating system's proxy settings at rurge while it runs
    #[arg(long, env = "RURGE_SYSTEM_PROXY")]
    pub system_proxy: bool,
    /// A policy group without members rejects instead of going DIRECT
    #[arg(long, env = "RURGE_EMPTY_GROUP_REJECT")]
    pub empty_group_reject: bool,
```

`crates/rurge/src/cli/run.rs`——把

```rust
        let shared = EngineShared::new(state.selections_for(&profile_key(&cfg.source.main)));
        let engine_rt =
            build_engine_runtime(cfg, &rt, &run_opts, outbound_mode.clone(), &shared).await?;
        print_diagnostics(engine_rt.diagnostics());
        let (policies, rules) = (
            engine_rt.policies.names().len(),
            engine_rt.rules.rules().len(),
        );
        let engine = Engine::new(engine_rt);
```

换成

```rust
        let mut shared = EngineShared::new(state.selections_for(&profile_key(&cfg.source.main)));
        if args.empty_group_reject {
            shared.empty_group = EmptyGroup::Reject;
        }
        let engine_rt =
            build_engine_runtime(cfg, &rt, &run_opts, outbound_mode.clone(), &shared).await?;
        print_diagnostics(engine_rt.diagnostics());
        let engine = Engine::new(engine_rt);
        let (policies, rules) = (
            engine.registry().names().len(),
            engine.runtime().rules.rules().len(),
        );
```

启动行里的"N policies"改在 `Engine::new` 之后从注册表数（含导入、派生的策略与组），`rules` 从当前的一代数。CLI 用例在 Task 9。

- [ ] **Step 8: 门禁与提交**

跑门禁（847 通过 / 1 忽略）。

```bash
git add crates/rurge-engine crates/rurge/src/cli/run.rs
git commit -m "feat(engine): 拨号与视图改读 EngineShared.cell 里的注册表；EmptyGroup 与 --empty-group-reject"
```

---


### Task 8: 订阅热重建与代际锁

每一代一个监听任务：本代任一订阅的版本变化 → 去抖 1 秒 → 用本代的 `Config` 与最新快照重新装配，以当前注册表为 `previous` 构建新注册表（没变的成员沿用原出站），再**在代际锁下**核对"本代仍是当前代"后经 cell 发布（设计 5.7，M3-D4，P5、P20）。任务随代存续：`Runtime` 持有它的 `AbortOnDropHandle`，这一代被丢弃时任务随之中止；它自己也在发现本代已被替换时退出。

**Files:**
- Modify: `crates/rurge-engine/src/subscriptions.rs`
- Modify: `crates/rurge-engine/src/{runtime.rs, engine.rs, reload.rs}`
- Modify: `crates/rurge-engine/tests/subscriptions.rs`

**Interfaces:**
- Consumes: Task 6 的 `Subscriptions`、Task 7 的 `Runtime.registry` / `publish_generation(&mut Runtime)` / `EngineShared.empty_group`；`ResourceHandle::subscribe`（`tokio::sync::watch::Receiver<u64>`）；`tokio_util::task::AbortOnDropHandle`。
- Produces:
  - `rurge_engine::subscriptions::REBUILD_DEBOUNCE: Duration = 1 s`（模块是私有的，常量供本 crate 使用）
  - crate 内：`Subscriptions::take_receivers(&mut self) -> Vec<watch::Receiver<u64>>`（在读首个快照之前订阅，其间的更新不会漏掉）；`Runtime.{factory: Arc<EngineFactory>, subscriptions: Subscriptions, watcher: OnceLock<AbortOnDropHandle<()>>}`
  - crate 内：`Engine::generation_lock(&self) -> MutexGuard<'_, ()>`、`Engine::watch_subscriptions(self: &Arc<Self>, receivers)`、`Engine::rebuild_registry(&self, rt: &Arc<Runtime>) -> bool`（`false`：`rt` 已不是当前代，什么也没发布）

- [ ] **Step 1: 用例先行**

单元用例（被替换的一代重建之后什么也不发布）与两条集成用例（改本地订阅文件 → 重建；URL 订阅首次启动为空、下载到后出现、服务端改了跟着变、重载时从缓存立即得到成员）：

`crates/rurge-engine/src/subscriptions.rs`——把

```rust
    use super::*;
    use rurge_config::codes;
    use rurge_config::config::from_text;
```

换成

```rust
    use super::*;
    use crate::runtime::RuntimeOptions;
    use crate::shared::EngineShared;
    use crate::stack::StackOptions;
    use rurge_config::codes;
    use rurge_config::config::from_text;
    use rurge_dns::system::StaticSystemDns;
    use rurge_net::socket::NoopSocketHook;
    use rurge_rules::{GeoUrls, OutboundMode};
```

`crates/rurge-engine/tests/subscriptions.rs`——把

```rust
mod common;
use common::*;
use rurge_config::session::SessionInfo;
use rurge_engine::EmptyGroup;
use rurge_inbound::{DialError, Dialer};
```

换成

```rust
mod common;
use common::*;
use rurge_config::rule::PolicyRef;
use rurge_config::session::SessionInfo;
use rurge_engine::EmptyGroup;
use rurge_inbound::{DialError, Dialer};

/// Like `common::wait_until`, with room for a file watcher's or a refresh
/// interval's delay plus the rebuild's own pause.
async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !check() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn members(engine: &Engine, group: &str) -> Vec<String> {
    engine
        .registry()
        .members(group)
        .unwrap_or_default()
        .to_vec()
}

fn outbound_of(engine: &Engine, name: &str) -> rurge_proto::OutboundRef {
    engine.registry().resolve(&PolicyRef::parse(name)).outbound
}

/// A generation that downloads: its GeoIP updater asks `server` (and gets a
/// 404) instead of the public default URLs.
async fn online_runtime(
    dir: &std::path::Path,
    profile: &str,
    server: &TestServer,
    shared: EngineShared,
) -> Runtime {
    std::fs::write(dir.join("t.conf"), profile).unwrap();
    let loaded = from_text(profile, &dir.join("t.conf"), &LoadOptions::for_tests());
    assert!(!loaded.diagnostics.has_errors());
    let mut stack = stack_options(dir);
    stack.no_network = false;
    stack.geo_urls = GeoUrls {
        country: server.url("/geo/country.mmdb"),
        asn: server.url("/geo/asn.mmdb"),
    };
    let opts = RuntimeOptions {
        stack,
        outbound_mode: OutboundMode::Rule,
        idle_timeout: Duration::from_secs(600),
        shared,
        request_log_size: 1000,
    };
    Runtime::build(loaded.config, opts).await.unwrap()
}
```

`crates/rurge-engine/tests/subscriptions.rs`——把

```rust
    let engine = Engine::new(runtime(dir.path(), &text, EngineShared::default()).await);
    assert_eq!(engine.registry().members("Sub").unwrap(), ["N1", "N2"]);
}
```

换成

```rust
    let engine = Engine::new(runtime(dir.path(), &text, EngineShared::default()).await);
    assert_eq!(engine.registry().members("Sub").unwrap(), ["N1", "N2"]);
}

/// An edit of the file rebuilds the registry alone: the members follow, and
/// a line that did not change keeps its outbound (M3 design 5.7).
#[tokio::test]
async fn an_edited_subscription_file_rebuilds_the_registry() {
    let dir = tempfile::tempdir().unwrap();
    let nodes = dir.path().join("nodes.txt");
    std::fs::write(&nodes, "N1 = http, n1.test, 80\nN2 = http, n2.test, 80\n").unwrap();
    let dns = MockDns::spawn().await;
    let text = Profile {
        groups: "Sub = select, policy-path=nodes.txt",
        ..Profile::default()
    }
    .text(dns.addr());
    let engine = Engine::new(runtime(dir.path(), &text, EngineShared::default()).await);
    let generation = engine.runtime();
    let n1 = outbound_of(&engine, "N1");
    std::fs::write(&nodes, "N1 = http, n1.test, 80\nN3 = http, n3.test, 80\n").unwrap();
    eventually("the rebuild", || members(&engine, "Sub") == ["N1", "N3"]).await;
    assert!(Arc::ptr_eq(&n1, &outbound_of(&engine, "N1")));
    assert!(!engine.registry().contains("N2"));
    assert!(
        Arc::ptr_eq(&generation, &engine.runtime()),
        "the generation stays"
    );
}

/// Nothing cached on a first start: the group is empty until the download
/// arrives, then follows the server; a reload finds the download in the
/// cache, so it has the members at once, even offline (M3-D5).
#[tokio::test]
async fn a_url_subscription_arrives_after_the_start_and_is_cached() {
    let server = TestServer::spawn().await;
    server.set("/nodes", "N1 = http, n1.test, 80\nN2 = http, n2.test, 80\n");
    let dir = tempfile::tempdir().unwrap();
    let dns = MockDns::spawn().await;
    let groups = format!(
        "Sub = select, policy-path={}, update-interval=1",
        server.url("/nodes")
    );
    let text = Profile {
        groups: &groups,
        ..Profile::default()
    }
    .text(dns.addr());
    let engine =
        Engine::new(online_runtime(dir.path(), &text, &server, EngineShared::default()).await);
    assert!(members(&engine, "Sub").is_empty(), "nothing is cached yet");
    eventually("the first download", || {
        members(&engine, "Sub") == ["N1", "N2"]
    })
    .await;
    let n1 = outbound_of(&engine, "N1");
    server.set("/nodes", "N1 = http, n1.test, 80\nN3 = http, n3.test, 80\n");
    eventually("the update", || members(&engine, "Sub") == ["N1", "N3"]).await;
    assert!(Arc::ptr_eq(&n1, &outbound_of(&engine, "N1")));

    // `runtime` builds offline: only the cache can give the members now
    engine.swap_runtime(runtime(dir.path(), &text, engine.shared()).await);
    assert_eq!(members(&engine, "Sub"), ["N1", "N3"]);
}
```

Run: `cargo test -p rurge-engine --lib subscriptions` 与 `cargo test -p rurge-engine --test subscriptions`

Expected: 单元用例编译错误——`no method named `rebuild_registry``；集成用例能编译，但 `an_edited_subscription_file_rebuilds_the_registry` 与 `a_url_subscription_arrives_after_the_start_and_is_cached` 在 15 秒后失败：`timed out waiting for the rebuild` / `timed out waiting for the first download`（此时还没有人在订阅更新后重建注册表）。

- [ ] **Step 2: 订阅句柄的接收端、重建与监听任务**

`crates/rurge-engine/src/subscriptions.rs`——把

```rust
//! The `policy-path` subscriptions of one config generation (phase 2 M3
//! design 5.1, 5.9): registered with its resource manager, read into
//! snapshots for the assembly, and checked offline for `rurge check`.

use rurge_config::config::{LoadError, LoadOptions, Loaded};
use rurge_config::spec::PolicyPath;
use rurge_config::{Config, Diagnostics};
use rurge_net::resource::{ResourceHandle, ResourceManager, ResourceSource, ResourceSpec};
use rurge_policy::{Snapshots, assemble, subscription};
use std::path::Path;
use std::sync::Arc;

pub(crate) struct Subscriptions {
    /// One per source, in the order the groups first name them.
    handles: Vec<(PolicyPath, ResourceHandle)>,
}
```

换成

```rust
//! The `policy-path` subscriptions of one config generation (phase 2 M3
//! design 5.1, 5.7, 5.9): registered with its resource manager, read into
//! snapshots for the assembly, watched so that an update rebuilds the
//! registry, and checked offline for `rurge check`.

use crate::engine::Engine;
use crate::runtime::Runtime;
use rurge_config::config::{LoadError, LoadOptions, Loaded};
use rurge_config::spec::PolicyPath;
use rurge_config::{Config, Diagnostics};
use rurge_net::resource::{ResourceHandle, ResourceManager, ResourceSource, ResourceSpec};
use rurge_policy::{PolicyRegistry, Snapshots, assemble, subscription};
use std::collections::HashSet;
use std::future::Future;
use std::path::Path;
use std::sync::{Arc, Weak};
use std::task::Poll;
use std::time::Duration;
use tokio::sync::watch;
use tokio_util::task::AbortOnDropHandle;

/// Updates that arrive within this long of each other make one rebuild.
pub const REBUILD_DEBOUNCE: Duration = Duration::from_secs(1);

pub(crate) struct Subscriptions {
    /// One per source, in the order the groups first name them.
    handles: Vec<(PolicyPath, ResourceHandle)>,
    /// For the engine's watcher, which takes them. Subscribed before the
    /// first snapshot is read, so no update in between goes unseen.
    receivers: Vec<watch::Receiver<u64>>,
}
```

`crates/rurge-engine/src/subscriptions.rs`——把

```rust
        let mut handles: Vec<(PolicyPath, ResourceHandle)> = Vec::new();
        for g in &cfg.group_specs {
```

换成

```rust
        let mut handles: Vec<(PolicyPath, ResourceHandle)> = Vec::new();
        let mut receivers = Vec::new();
        for g in &cfg.group_specs {
```

`crates/rurge-engine/src/subscriptions.rs`——把

```rust
            if !handles.iter().any(|(p, _)| p == path) {
                handles.push((path.clone(), handle));
            }
        }
        Subscriptions { handles }
    }
```

换成

```rust
            if !handles.iter().any(|(p, _)| p == path) {
                receivers.push(handle.subscribe());
                handles.push((path.clone(), handle));
            }
        }
        Subscriptions { handles, receivers }
    }

    /// The receivers, for the one watcher of this generation.
    pub(crate) fn take_receivers(&mut self) -> Vec<watch::Receiver<u64>> {
        std::mem::take(&mut self.receivers)
    }
```

`crates/rurge-engine/src/subscriptions.rs`——把

```rust
/// The warnings an assembly from what earlier runs cached gives (M3 design
/// 5.9). Offline: reads the data directory and local files, nothing else.
```

换成

```rust
impl Engine {
    /// Rebuilds the registry of the current generation after every burst of
    /// subscription updates (M3 design 5.7). The task goes with the
    /// generation: the runtime keeps its handle and aborts it when dropped.
    pub(crate) fn watch_subscriptions(self: &Arc<Self>, receivers: Vec<watch::Receiver<u64>>) {
        if receivers.is_empty() {
            return;
        }
        let rt = self.runtime();
        let task = tokio::spawn(rebuild_on_change(
            Arc::downgrade(self),
            Arc::downgrade(&rt),
            receivers,
        ));
        let _ = rt.watcher.set(AbortOnDropHandle::new(task));
    }

    /// Assembles `rt`'s profile anew from what its subscriptions hold now and
    /// publishes the registry built from it; every outbound whose line did
    /// not change is kept (M2 design 7.1). `false` when `rt` is no longer the
    /// current generation: then nothing is published.
    pub(crate) fn rebuild_registry(&self, rt: &Arc<Runtime>) -> bool {
        let assembly = assemble(&rt.config, &rt.subscriptions.snapshots());
        for d in assembly.diagnostics.iter() {
            tracing::warn!("{d}");
        }
        let shared = self.shared();
        // Built outside the lock, which is never held across I/O: a
        // `previous` that goes stale meanwhile costs some reuse, nothing else.
        let previous = shared.cell.load();
        let built = PolicyRegistry::build(
            &rt.config,
            &assembly,
            rt.factory.as_ref(),
            &shared.cell,
            shared.selections.clone(),
            previous.as_deref(),
            shared.empty_group,
        );
        let registry = match built {
            Ok(registry) => Arc::new(registry),
            Err(e) => {
                tracing::warn!(error = %e.message, "cannot rebuild the policies after a subscription update; the current ones stay");
                return true;
            }
        };
        let _generation = self.generation_lock();
        if !Arc::ptr_eq(&self.runtime(), rt) {
            return false;
        }
        log_member_changes(previous.as_deref(), &registry);
        shared.cell.store(registry);
        true
    }
}

/// Waits for a change, lets the burst settle, rebuilds; ends when the
/// generation is gone or replaced.
async fn rebuild_on_change(
    engine: Weak<Engine>,
    rt: Weak<Runtime>,
    mut receivers: Vec<watch::Receiver<u64>>,
) {
    while any_changed(&mut receivers).await {
        tokio::time::sleep(REBUILD_DEBOUNCE).await;
        // what changed during the pause is in this rebuild already
        for rx in &mut receivers {
            rx.mark_unchanged();
        }
        let (Some(engine), Some(rt)) = (engine.upgrade(), rt.upgrade()) else {
            return;
        };
        if !engine.rebuild_registry(&rt) {
            return;
        }
    }
}

/// Waits until one of `receivers` sees a new version; `false` once one is
/// closed: the resource manager, and with it the generation, is gone.
async fn any_changed(receivers: &mut [watch::Receiver<u64>]) -> bool {
    let mut waits: Vec<_> = receivers
        .iter_mut()
        .map(|rx| Box::pin(rx.changed()))
        .collect();
    std::future::poll_fn(|cx| {
        for wait in &mut waits {
            if let Poll::Ready(result) = wait.as_mut().poll(cx) {
                return Poll::Ready(result.is_ok());
            }
        }
        Poll::Pending
    })
    .await
}

/// One INFO line per group whose members changed: its name and how many came
/// and went — never where they came from (M3-D7).
fn log_member_changes(before: Option<&PolicyRegistry>, after: &PolicyRegistry) {
    for group in after.group_names() {
        let now: HashSet<&String> = after.members(&group).unwrap_or_default().iter().collect();
        let was: HashSet<&String> = before
            .and_then(|b| b.members(&group))
            .unwrap_or_default()
            .iter()
            .collect();
        let added = now.difference(&was).count();
        let removed = was.difference(&now).count();
        if added + removed > 0 {
            tracing::info!(group = %group, added, removed, "policy group members updated");
        }
    }
}

/// The warnings an assembly from what earlier runs cached gives (M3 design
/// 5.9). Offline: reads the data directory and local files, nothing else.
```

要点：
- `any_changed` 同时等全部接收端：`changed()` 可以安全地取消，每轮重新建一组等待的 future；任一接收端关闭（资源管理器随这一代没了）→ 任务结束。
- 去抖之后先 `mark_unchanged` 全部接收端再读快照：这 1 秒里的其它更新都并进这一次。
- `rebuild_registry`：装配与构建都在锁外；拿到锁之后若 `engine.runtime()` 已不是 `rt`，返回 `false`，新注册表丢弃（它会带着旧配置的组）；否则记每个成员有变化的组一条 INFO（组名与增减数量），再发布。主配置策略在重建时构建失败 → WARN，保留当前的注册表，任务继续。

- [ ] **Step 3: `Runtime` 保存工厂、订阅与任务句柄；发布处启动任务**

`crates/rurge-engine/src/runtime.rs`——把

```rust
use crate::shared::EngineShared;
use crate::stack::{Stack, StackOptions, build_stack};
use crate::subscriptions::Subscriptions;
use rurge_config::{Config, Diagnostics};
use rurge_policy::PolicyRegistry;
use rurge_rules::{OutboundMode, RuleEngine};
use std::sync::Arc;
use std::time::Duration;
```

换成

```rust
use crate::outbounds::EngineFactory;
use crate::shared::EngineShared;
use crate::stack::{Stack, StackOptions, build_stack};
use crate::subscriptions::Subscriptions;
use rurge_config::{Config, Diagnostics};
use rurge_policy::PolicyRegistry;
use rurge_rules::{OutboundMode, RuleEngine};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio_util::task::AbortOnDropHandle;
```

`crates/rurge-engine/src/runtime.rs`——把

```rust
    /// 5.8).
    pub(crate) registry: Option<Arc<PolicyRegistry>>,
}
```

换成

```rust
    /// 5.8).
    pub(crate) registry: Option<Arc<PolicyRegistry>>,
    /// What rebuilds the registry when a subscription changes (5.7).
    pub(crate) factory: Arc<EngineFactory>,
    pub(crate) subscriptions: Subscriptions,
    /// The task watching `subscriptions`; it goes with the generation.
    pub(crate) watcher: OnceLock<AbortOnDropHandle<()>>,
}
```

`crates/rurge-engine/src/runtime.rs`——把

```rust
        let factory = match &opts.shared.roots {
            Some(roots) => crate::outbounds::EngineFactory::with_roots(
                &config,
                opts.shared.resolver.clone(),
                opts.stack.socket_hook.clone(),
                roots.clone(),
            ),
            None => crate::outbounds::EngineFactory::new(
                &config,
                opts.shared.resolver.clone(),
                opts.stack.socket_hook.clone(),
            ),
        };
```

换成

```rust
        let factory = Arc::new(match &opts.shared.roots {
            Some(roots) => EngineFactory::with_roots(
                &config,
                opts.shared.resolver.clone(),
                opts.stack.socket_hook.clone(),
                roots.clone(),
            ),
            None => EngineFactory::new(
                &config,
                opts.shared.resolver.clone(),
                opts.stack.socket_hook.clone(),
            ),
        });
```

`crates/rurge-engine/src/runtime.rs`——把

```rust
                &assembly,
                &factory,
                &opts.shared.cell,
```

换成

```rust
                &assembly,
                factory.as_ref(),
                &opts.shared.cell,
```

`crates/rurge-engine/src/runtime.rs`——把

```rust
            dns_pipeline,
            registry: Some(registry),
        })
```

换成

```rust
            dns_pipeline,
            registry: Some(registry),
            factory,
            subscriptions,
            watcher: OnceLock::new(),
        })
```

`crates/rurge-engine/src/engine.rs`——把

```rust
    state: OnceLock<Arc<StateStore>>,
    shared: EngineShared,
}

impl Engine {
    pub fn new(mut runtime: Runtime) -> Arc<Engine> {
```

换成

```rust
    state: OnceLock<Arc<StateStore>>,
    shared: EngineShared,
    /// Held while a registry or a whole generation is published.
    generation: std::sync::Mutex<()>,
}

impl Engine {
    pub fn new(mut runtime: Runtime) -> Arc<Engine> {
```

`crates/rurge-engine/src/engine.rs`——把

```rust
        shared.resolver.store(runtime.stack.resolver.clone());
        let engine = Arc::new(Engine {
```

换成

```rust
        shared.resolver.store(runtime.stack.resolver.clone());
        let receivers = runtime.subscriptions.take_receivers();
        let engine = Arc::new(Engine {
```

`crates/rurge-engine/src/engine.rs`——把

```rust
            state: OnceLock::new(),
            shared,
        });
        if let Some(pc) = engine.runtime().dns_pipeline() {
            pc.attach(Arc::downgrade(&engine));
        }
        engine
    }
```

换成

```rust
            state: OnceLock::new(),
            shared,
            generation: std::sync::Mutex::new(()),
        });
        if let Some(pc) = engine.runtime().dns_pipeline() {
            pc.attach(Arc::downgrade(&engine));
        }
        engine.watch_subscriptions(receivers);
        engine
    }

    /// Held while a registry or a whole generation is published, so a
    /// subscription rebuild of a generation that is going out never publishes
    /// after its successor (M3 design 5.7). Never held across I/O.
    pub(crate) fn generation_lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.generation.lock().expect("generation lock")
    }
```

`crates/rurge-engine/src/reload.rs`——把

```rust
        if let Some(pc) = next.dns_pipeline() {
            pc.attach(std::sync::Arc::downgrade(self));
        }
        self.publish_generation(&mut next);
        self.store_runtime(next);
        before != after
```

换成

```rust
        if let Some(pc) = next.dns_pipeline() {
            pc.attach(std::sync::Arc::downgrade(self));
        }
        let receivers = next.subscriptions.take_receivers();
        {
            let _generation = self.generation_lock();
            self.publish_generation(&mut next);
            self.store_runtime(next);
        }
        self.watch_subscriptions(receivers);
        before != after
```

要点：`Engine::new` 与 `swap_runtime` 都在把 `Runtime` 交出去之前取走它的接收端，发布之后为新的当前代启动监听任务（没有订阅就不启动）；`swap_runtime` 的"发布 + 切换"在代际锁下进行，与 `rebuild_registry` 的"核对 + 发布"互斥。任务持有的是引擎与这一代的 `Weak`，不会让它们活得比应有的更久。

- [ ] **Step 4: 运行**

Run: `cargo test -p rurge-engine --lib subscriptions` → 3 passed；`cargo test -p rurge-engine --test subscriptions` → 5 passed（两条热重建用例各需几秒：文件监视 / 刷新间隔 + 1 秒去抖）。

- [ ] **Step 5: 门禁与提交**

跑门禁（850 通过 / 1 忽略）。

```bash
git add crates/rurge-engine
git commit -m "feat(engine): 订阅热重建——去抖 1 秒、只重建注册表并按指纹复用出站、代际锁下被替换的一代不再发布"
```

---

### Task 9: 端到端——经导入 / 派生成员出站；CLI 的空组开关

前面几个任务已经实现的行为，在整条路径上钉住：会话经一个只在订阅里存在的策略出站；设了中继的组，会话经中继到派生成员的服务器（名字由中继远程解析，本机不解析）；`rurge run` 的空组默认直连，`--empty-group-reject` 与 `RURGE_EMPTY_GROUP_REJECT=true` 都改为拒绝。这些用例第一次运行就应当通过。

**Files:**
- Modify: `crates/rurge-engine/tests/subscriptions.rs`
- Modify: `crates/rurge/tests/cli.rs`

**Interfaces:**
- Consumes: 前面全部任务；测试夹具 `FakeSocks5` / `Socks5Script`（`connect_to`、`requests()` 的 `atyp` / `host` / `port`）、`connect_via_http`、`get`、`MockDns::queries`。
- Produces: 引擎测试里的 `harness_in(files, profile) -> Harness`（先把文件写进配置目录再构建第一代）；CLI 测试里的 `spawn_command(cmd: Command) -> Daemon`（`spawn_daemon_full` 的后半截，供需要自定环境变量的用例使用），`rurge_run` 另外清掉 `RURGE_EMPTY_GROUP_REJECT`。

- [ ] **Step 1: 引擎的两条端到端用例**

`crates/rurge-engine/tests/subscriptions.rs`——把

```rust
fn members(engine: &Engine, group: &str) -> Vec<String> {
```

换成

```rust
/// An engine with its listeners, whose profile directory holds `files`
/// before the first generation is built.
async fn harness_in(files: &[(&str, &str)], p: Profile<'_>) -> Harness {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in files {
        std::fs::write(dir.path().join(name), text).unwrap();
    }
    let text = p.text(dns.addr());
    let engine = Engine::new(runtime(dir.path(), &text, EngineShared::default()).await);
    let listeners = engine.bind_listeners().await.unwrap();
    Harness {
        dir,
        engine,
        listeners,
        dns,
    }
}

fn members(engine: &Engine, group: &str) -> Vec<String> {
```

`crates/rurge-engine/tests/subscriptions.rs`——把

```rust
/// Nothing cached on a first start: the group is empty until the download
```

换成

```rust
/// A session leaves through a policy that exists only in the subscription;
/// the proxy gets the target's name, as from any other proxy policy.
#[tokio::test]
async fn a_session_leaves_through_an_imported_policy() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi from the origin");
    let node = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&origin)),
        ..Socks5Script::default()
    })
    .await;
    let nodes = format!("Node = socks5, 127.0.0.1, {}\n", node.addr().port());
    let h = harness_in(
        &[("nodes.txt", &nodes)],
        Profile {
            groups: "Sub = select, policy-path=nodes.txt",
            rules: "DOMAIN,target.test,Sub",
            ..Profile::default()
        },
    )
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
    assert!(
        get(&mut tunnel, "target.test", "/hello")
            .await
            .ends_with("hi from the origin")
    );
    let seen = node.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!((seen[0].host.as_str(), seen[0].port), ("target.test", 8080));
}

/// The relay of a group carries every member it took from its
/// subscription: the relay is asked for the member's server by name, the
/// member for the target by name, and nothing is resolved here (FR-GRP-07).
#[tokio::test]
async fn a_derived_member_is_reached_through_the_group_relay() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi through the relay");
    let exit = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&origin)),
        ..Socks5Script::default()
    })
    .await;
    let relay = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(exit.addr()),
        ..Socks5Script::default()
    })
    .await;
    let proxies = format!("Relay = socks5, 127.0.0.1, {}", relay.addr().port());
    let h = harness_in(
        &[("nodes.txt", "Exit = socks5, exit.example, 1080\n")],
        Profile {
            proxies: &proxies,
            groups: "Pool = select, policy-path=nodes.txt, underlying-proxy=Relay",
            rules: "DOMAIN,target.test,Pool",
            ..Profile::default()
        },
    )
    .await;
    assert_eq!(members(&h.engine, "Pool"), ["Exit (via Relay)"]);
    let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
    assert!(
        get(&mut tunnel, "target.test", "/hello")
            .await
            .ends_with("hi through the relay")
    );
    let relay_seen = relay.requests();
    let first = relay_seen.first().expect("the relay never saw a request");
    assert_eq!(
        (first.atyp, first.host.as_str(), first.port),
        (3, "exit.example", 1080)
    );
    let exit_seen = exit.requests();
    assert_eq!(
        exit_seen.first().map(|r| r.host.as_str()),
        Some("target.test")
    );
    assert!(
        h.dns.queries().is_empty(),
        "nothing on this path is resolved locally"
    );
}

/// Nothing cached on a first start: the group is empty until the download
```

Run: `cargo test -p rurge-engine --test subscriptions` → 7 passed。

- [ ] **Step 2: CLI 的空组开关**

`crates/rurge/tests/cli.rs`——把

```rust
            .env(
                "RURGE_SYSTEM_PROXY_BACKEND",
                format!("file:{}", sysproxy_file(data).display()),
            )
            .env_remove("RURGE_SYSTEM_PROXY");
        cmd
    }
```

换成

```rust
            .env(
                "RURGE_SYSTEM_PROXY_BACKEND",
                format!("file:{}", sysproxy_file(data).display()),
            )
            .env_remove("RURGE_SYSTEM_PROXY")
            .env_remove("RURGE_EMPTY_GROUP_REJECT");
        cmd
    }
```

`crates/rurge/tests/cli.rs`——把

```rust
        cmd.args(extra);
        let mut child = cmd
            .stdout(Stdio::piped())
```

换成

```rust
        cmd.args(extra);
        spawn_command(cmd)
    }

    /// Spawns a prepared `rurge_run` command and waits for both `listening
    /// on` lines.
    fn spawn_command(mut cmd: Command) -> Daemon {
        let mut child = cmd
            .stdout(Stdio::piped())
```

`crates/rurge/tests/cli.rs`——把

```rust
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_shuts_down_gracefully_on_sigint() {
```

换成

```rust
    /// A group without members goes DIRECT, as in Surge; the switch, and its
    /// environment variable, make it reject instead (M3-D3).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_empty_group_goes_direct_unless_told_to_reject() {
        let target = TestServer::spawn().await;
        target.set("/hello", "hi from target");
        let url = format!("http://127.0.0.1:{}/hello", target.url("/").port().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join("t.conf");
        std::fs::write(
            &conf,
            "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nloglevel = warning\n\
[Proxy Group]\nSub = select, policy-path=missing.txt\n[Rule]\nFINAL,Sub\n",
        )
        .unwrap();
        for (switch, env, rejects) in [
            (None, None, false),
            (Some("--empty-group-reject"), None, true),
            (None, Some("true"), true),
        ] {
            let (conf, url) = (conf.clone(), url.clone());
            let data = dir.path().join(format!("data-{rejects}-{}", env.is_some()));
            let answer = tokio::task::spawn_blocking(move || {
                let mut cmd = rurge_run(&conf, &data);
                cmd.args(switch);
                if let Some(value) = env {
                    cmd.env("RURGE_EMPTY_GROUP_REJECT", value);
                }
                let daemon = spawn_command(cmd);
                http_get(daemon.http, &url)
            })
            .await
            .unwrap();
            if rejects {
                assert!(answer.is_empty(), "REJECT closes the connection: {answer}");
            } else {
                assert!(
                    answer.starts_with("HTTP/1.1 200") && answer.ends_with("hi from target"),
                    "{answer}"
                );
            }
        }
        assert_eq!(target.requests().len(), 1);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_shuts_down_gracefully_on_sigint() {
```

Run: `cargo test -p rurge --test cli empty_group` → 1 passed。三次启动各用自己的数据目录（实例锁按数据目录）；REJECT 对明文 HTTP 请求是直接断开（与 `run_proxies_http_and_rejects_by_rule` 的断言相同）。

- [ ] **Step 3: 门禁与提交**

跑门禁（853 通过 / 1 忽略）。

```bash
git add crates/rurge-engine/tests/subscriptions.rs crates/rurge/tests/cli.rs
git commit -m "test: 经导入与派生成员的端到端出站；rurge run 的空组开关与环境变量"
```

---

### Task 10: 文档

**Files:**
- Modify: `docs/surge-compatibility-matrix.md`、`docs/api/phase2.md`、`docs/acceptance/phase2-manual.md`
- Modify: `docs/superpowers/specs/2026-09-23-phase2-m3-groups-subscriptions-design.md`（新增第 16 节）
- Modify: `README.md`、`README_en.md`、`CLAUDE.md`
- Modify: `crates/rurge-api/src/routes/policy_groups.rs`（一句过时的注释：`subnet` 组现在代表它的 `default`）
- Modify: 本计划末尾「执行期修正记录」与「延后事项」两张表

- [ ] **Step 1: 兼容性清单**

`docs/surge-compatibility-matrix.md`——把

```markdown
| `test-url` | HTTP(S) URL；默认全局设置 | ✅ | 2 | M1 解析并校验取值，`W0029`；M3 生效 |
```

换成

```markdown
| `test-url` | HTTP(S) URL；默认全局设置 | ✅ | 2 | M1 解析并校验取值，`W0029`；M3b 生效 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `test-timeout` | 秒；默认全局设置 | ✅ | 2 | M1 解析并校验取值，`W0029`；M3 生效 |
```

换成

```markdown
| `test-timeout` | 秒；默认全局设置 | ✅ | 2 | M1 解析并校验取值，`W0029`；M3b 生效 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `test-udp` | `hostname@ipv4` | ✅ | 2 | M1 解析并校验取值，`W0029`；M3 生效 |
```

换成

```markdown
| `test-udp` | `hostname@ipv4` | ✅ | 2 | M1 解析并校验取值，`W0029`；M5 生效（M3 细化设计订正了原来的"M3 生效"） |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| 🟡 | 3 | `TYPE:CELLULAR` / `MCCMNC:` 永不匹配 |
```

换成

```markdown
| 🟡 | 3 | `TYPE:CELLULAR` / `MCCMNC:` 永不匹配；阶段 3 之前整组代表它的 `default`（M3a；没写 `default` 时按空组兜底，见下一行）；`category` 等界面参数不再被当成网络条件 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| 嵌套与循环 | 组可嵌套；循环引用告警且该组临时表现为 REJECT；无可用成员回退 DIRECT | ✅ | 2 | |
```

换成

```markdown
| 嵌套与循环 | 组可嵌套；循环引用告警且该组临时表现为 REJECT；无可用成员回退 DIRECT | ✅ | 2 | M3a 已实现：循环在加载期报 `W0030`（取代 `E0009`，不再阻止加载），装配后按成员与 `include-other-group` 再检测一次，环上的组解析为 REJECT，会话记录 `policy group cycle: A → B → A`；不在环上、但选到成环成员的组只在那一次 REJECT。没有成员的组（订阅还没下载到、过滤滤光了）回退 DIRECT，会话记录 `policy group has no members; DIRECT substituted`；rurge 专有的 `--empty-group-reject`（环境变量 `RURGE_EMPTY_GROUP_REJECT=true`）改为 REJECT。每构建一次策略表，每个环、每个空组各告警一次 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `icon-url` | 全部（Mac 6.5+） | URL | ✅ | 6 |
```

换成

```markdown
| `icon-url` | 全部（Mac 6.5+） | URL | ✅ | 6 |
| `category` | 全部（iOS 5.23 / Mac 6.10+） | 界面分类；不影响路由。M3a 起接受，不报诊断 | 🔁 | 2 |
| `url`（旧参数） | 自动类型组 | 当前版本无效；M3a 起报 `W0006`，提示改用策略的 `test-url` 或 `proxy-test-url` | 🔁 | 2 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `underlying-proxy` | 全部（iOS 5.22 / Mac 6.9+） | 策略名；整组链式代理，派生策略名 `Name (via Relay)` | ✅ | 2 |
```

换成

```markdown
| `underlying-proxy` | 全部（iOS 5.22 / Mac 6.9+） | 策略名；整组链式代理，派生策略名 `Name (via Relay)`。M3a 已实现：组上的中继覆盖成员自己的；组、内置策略与 `direct` / `reject` 别名成员原样保留；经 `include-other-group` 取到的是未派生的成员；派生名已被占用时略去该成员并告警（不绕过中继）；经它绕回本组是 `E0019` | ✅ | 2 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `policy-path` | 除 subnet 外 | 文件路径或 URL；内容为策略行列表或含 `[Proxy]` 的完整配置；远程缓存并定期更新 | ✅ | 2 |
```

换成

```markdown
| `policy-path` | 除 subnet 外 | 文件路径或 URL；内容为策略行列表或含 `[Proxy]` 的完整配置；远程缓存并定期更新。M3a 已实现，差异：只接受 Surge 格式（Clash / base64 解析不出策略时告警）；下载经 rurge 自己的直连，不经代理（未与 Surge 核对）；值在 `profiles/current` 与 `policies/detail` 里脱敏，日志只写组名、不写 URL；单个订阅最多 10 000 条策略；首次下载不阻塞启动（组先按空组兜底），已有缓存时启动与重载同步载入；订阅更新只重建策略表（没变的成员沿用原出站），不打断无关的连接；坏行、重名、与配置同名的行跳过并告警（只报行号与原因） | 🟡 | 2 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `update-interval` | 同上 | 秒；默认 86400 | ✅ | 2 |
```

换成

```markdown
| `update-interval` | 同上 | 秒；默认 86400；M3a 已实现（几个组共用一个来源时取最短的） | ✅ | 2 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `policy-regex-filter` | 同上 | 正则；作用于导入成员，不作用于显式成员 | ✅ | 2 |
```

换成

```markdown
| `policy-regex-filter` | 同上 | 正则；作用于导入成员，不作用于显式成员；M3a 已实现（`fancy-regex` 语法，与 URL-REGEX 相同；订阅成员按加前缀之前的原名过滤） | ✅ | 2 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `external-policy-modifier` | 同上 | 引号包裹的 `key=value` 列表；覆盖导入策略参数 | ✅ | 2 |
```

换成

```markdown
| `external-policy-modifier` | 同上 | 引号包裹的 `key=value` 列表；覆盖导入策略参数；M3a 已实现（在文本层改写导入行：同名参数原地替换、没有的追加）；值可能含凭据，在 `profiles/current` 与 `policies/detail` 里整体脱敏 | ✅ | 2 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `external-policy-name-prefix` | 同上 | 前缀（不能含 `=`） | ✅ | 2 |
```

换成

```markdown
| `external-policy-name-prefix` | 同上 | 前缀（不能含 `=`）；M3a 已实现 | ✅ | 2 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `include-all-proxies` | 同上（iOS 4.12 / Mac 4.5+） | 布尔；含 `[Proxy]` 全部代理策略，不含内置与组 | ✅ | 2 |
```

换成

```markdown
| `include-all-proxies` | 同上（iOS 4.12 / Mac 4.5+） | 布尔；含 `[Proxy]` 全部代理策略，不含内置与组；M3a 已实现（也不含 `direct` / `reject` 别名） | ✅ | 2 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `include-other-group` | 同上 | `"g1,g2"`；递归展开 | ✅ | 2 |
```

换成

```markdown
| `include-other-group` | 同上 | `"g1,g2"`；递归展开；M3a 已实现（引用未知的组是 `E0008`；成环的组不展开给别人） | ✅ | 2 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| 成员装配顺序 | 显式成员 → `include-other-group` → `include-all-proxies` → `policy-path`；重名保留首个；导入项按 过滤 → 前缀 → 修饰 处理 | | ✅ | 2 |
```

换成

```markdown
| 成员装配顺序 | 显式成员 → `include-other-group` → `include-all-proxies` → `policy-path`；重名保留首个；导入项按 过滤 → 前缀 → 修饰 处理 | M3a 已实现；两个组导入了同名策略：定义相同视为同一个，不同则先声明的组那份生效；规则不能直接引用导入的策略名（`E0007`，未与 Surge 核对） | ✅ | 2 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `--check / -c <path>` | 校验配置 | `rurge check -c <path>` | 1 | 自阶段 2 / M1 起含干构建：构建不出来的策略（如 p12 解不开）是带行号的 `E0022` |
```

换成

```markdown
| `--check / -c <path>` | 校验配置 | `rurge check -c <path> [--data-dir <dir>]` | 1 | 自阶段 2 / M1 起含干构建：构建不出来的策略（如 p12 解不开）是带行号的 `E0022`；自 M3a 起配置本身无错时，连同数据目录（`--data-dir`，环境变量 `RURGE_DATA_DIR`，默认平台数据目录）里已缓存的 `policy-path` 订阅一起装配检查，不联网：没有内容的订阅报 `W0022`，订阅里跳过的行报 `W0023`，订阅的 URL 不出现在输出里 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| rurge 专有命令 | 前台运行 HTTP / SOCKS5 代理；出站模式初值来自 `--outbound-mode`（M4 起 `state.json` 优先）；`--log-level` 覆盖 `loglevel` | `rurge run -c <conf> [--outbound-mode direct\|proxy=<p>\|rule] [--log-level <l>] [--idle-timeout <secs>] [--request-log-size <n>] [--watch] [--log-file <path>]` | 1 | 见 M3 设计文档 §9.3；
```

换成

```markdown
| rurge 专有命令 | 前台运行 HTTP / SOCKS5 代理；出站模式初值来自 `--outbound-mode`（M4 起 `state.json` 优先）；`--log-level` 覆盖 `loglevel` | `rurge run -c <conf> [--outbound-mode direct\|proxy=<p>\|rule] [--log-level <l>] [--idle-timeout <secs>] [--request-log-size <n>] [--watch] [--log-file <path>] [--empty-group-reject]` | 1 | 见 M3 设计文档 §9.3；`--empty-group-reject`（环境变量 `RURGE_EMPTY_GROUP_REJECT`，与其它 rurge 开关一样只接受 `true` / `false`）让没有成员的组解析为 REJECT 而不是 DIRECT（阶段 2 / M3a）；
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `GET /v1/policies` | 列出策略 | 全部 | 🟡 | 1 | M4a 已实现；JSON 结构手册未定义，暂定结构见 `docs/api/phase1.md`，阶段 6 对齐真实 Surge |
```

换成

```markdown
| `GET /v1/policies` | 列出策略 | 全部 | 🟡 | 1 | M4a 已实现；JSON 结构手册未定义，暂定结构见 `docs/api/phase1.md`，阶段 6 对齐真实 Surge；阶段 2 / M3a 起 `proxies` 含订阅导入的策略与组级中继派生的 `Name (via Relay)` |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `GET /v1/policies/detail?policy_name=` | 策略详情 | 全部 | 🟡 | 2 | M1 已实现；响应形状手册未给出，暂定结构见 `docs/api/phase2.md` |
```

换成

```markdown
| `GET /v1/policies/detail?policy_name=` | 策略详情 | 全部 | 🟡 | 2 | M1 已实现；响应形状手册未给出，暂定结构见 `docs/api/phase2.md`；M3a 起也能查导入与派生的策略（同样脱敏） |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `GET /v1/policy_groups` | 列出组与选项 | 全部 | 🟡 | 2 | M1 已实现；响应形状手册未给出，暂定结构见 `docs/api/phase2.md` |
```

换成

```markdown
| `GET /v1/policy_groups` | 列出组与选项 | 全部 | 🟡 | 2 | M1 已实现；响应形状手册未给出，暂定结构见 `docs/api/phase2.md`；M3a 起成员是装配后的成员表（含订阅导入与派生成员），订阅更新后随之变化 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `GET/POST /v1/policy_groups/select` | 读 / 改 select 组选择 | 全部 | ✅ | 2 | M1 已实现 |
```

换成

```markdown
| `GET/POST /v1/policy_groups/select` | 读 / 改 select 组选择 | 全部 | ✅ | 2 | M1 已实现；M3a 起按装配后的成员表校验，选择按名字保存，订阅更新后名字不在了就回落到第一个成员 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
`base64` / `token` / `uuid` / `username` / `headers` / `ws-headers` / `ws-path` / `shadow-tls-password`；
```

换成

```markdown
`base64` / `token` / `uuid` / `username` / `headers` / `ws-headers` / `ws-path` / `shadow-tls-password` / `policy-path` / `external-policy-modifier`（后两个自 M3a 起：订阅链接常带 token，修饰列表能设任何参数）；
```

`docs/surge-compatibility-matrix.md`——把

```markdown
`check` 已实现（M4a，校验磁盘上的当前配置文件，不影响运行中的实例）；`switch` 与 `GET /v1/profiles` 的多配置目录管理仍在阶段 6；自 M1 起 `check` 含干构建（`E0022`） |
```

换成

```markdown
`check` 已实现（M4a，校验磁盘上的当前配置文件，不影响运行中的实例）；`switch` 与 `GET /v1/profiles` 的多配置目录管理仍在阶段 6；自 M1 起 `check` 含干构建（`E0022`）；自 M3a 起连同守护进程数据目录里已缓存的订阅一起装配检查（同 10.3 `--check` 行） |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| 5. 策略组 | 29 | 26 | 2 | 1 | 0 | 0 |
```

换成

```markdown
| 5. 策略组 | 31 | 25 | 3 | 3 | 0 | 0 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| **合计** | **527** | **426** | **56** | **27** | **10** | **8** |
```

换成

```markdown
| **合计** | **529** | **425** | **57** | **29** | **10** | **8** |
```

附录统计只按本计划的改动增减：第 5 节多了 `category` 与旧参数 `url` 两行（都是 🔁），`policy-path` 从 ✅ 改为 🟡。

- [ ] **Step 2: API 参考与手工验收清单**

`docs/api/phase2.md`——把

```markdown
**一个配置合法但没有成员的组**（例如 `subnet` 组——它的目标写在 `conditions` / `default` 里，不写在成员列表）返回 `{"policy": ""}`，这不是错误。
```

换成

```markdown
**一个没有成员的组**（订阅还没下载到，或过滤把成员滤光了）返回 `{"policy": ""}`，这不是错误；`subnet` 组在阶段 3 之前代表它的 `default`（M3a）。
```

`docs/api/phase2.md`——把

```markdown
自阶段 2 / M1 起，`POST /v1/profiles/check`（阶段 1 端点，见 `docs/api/phase1.md`）除原有的加载诊断外，还包含干构建：
```

换成

```markdown
自阶段 2 / M1 起，`POST /v1/profiles/check`（阶段 1 端点，见 `docs/api/phase1.md`）除原有的加载诊断外，还包含干构建（自 M3a 起还有订阅装配，见下一节）：
```

`docs/api/phase2.md`——把

```markdown
`rurge check`、`run`、`reload` 的行为相同（见 `docs/surge-compatibility-matrix.md` 10.3 / 10.4 节）。
```

换成

```markdown
`rurge check`、`run`、`reload` 的行为相同（见 `docs/surge-compatibility-matrix.md` 10.3 / 10.4 节）。

## 订阅导入与派生的策略（M3a）

自阶段 2 / M3a 起（`docs/superpowers/specs/2026-09-23-phase2-m3-groups-subscriptions-design.md` 第 5、8 节），上面几个端点读的都是运行中的策略表，而策略表会随订阅更新重建：

- `GET /v1/policies` 的 `proxies` 依次是 5 个内置策略、配置里的策略、订阅导入的策略、组级 `underlying-proxy` 派生的 `Name (via Relay)`；`policy-groups` 不变。
- `GET /v1/policies/detail` 能查导入与派生的策略：导入的是订阅里那一行（前缀与 `external-policy-modifier` 已应用），派生的是其来源策略的定义加上 `underlying-proxy=<中继>`；脱敏规则同上，`policy-path` 与 `external-policy-modifier` 也在脱敏名单里。
- `GET /v1/policy_groups` 的成员表是装配后的：写在组行上的、`include-other-group` 与 `include-all-proxies` 取来的、订阅导入的，经过滤与去重，有中继的组里代理成员换成派生名。`lineHash` 对导入与派生的成员同样建立在脱敏后的文本上。
- `GET` / `POST /v1/policy_groups/select` 按装配后的成员表读与校验；选择按名字保存，订阅更新后名字不在了就回落到第一个成员。
- `POST /v1/profiles/check`：配置本身无错时，连同守护进程数据目录里已缓存的订阅一起装配检查，不联网；没有内容的订阅报 `W0022`，订阅里跳过的行报 `W0023`，订阅的 URL 不出现在输出里。
```

`docs/acceptance/phase2-manual.md`——把

```markdown
- [ ] `GET /v1/policies/detail` 与 `GET /v1/profiles/current` 里 `shadow-tls-password` 的值是 `***`。
```

换成

```markdown
- [ ] `GET /v1/policies/detail` 与 `GET /v1/profiles/current` 里 `shadow-tls-password` 的值是 `***`。

## M3a　成员装配与订阅

需要一个真实的机场订阅链接（Surge 格式）与其中至少两个可用节点，自动化测试（只用回环）覆盖不了。

- [ ] `G = select, policy-path=<订阅 URL>, update-interval=3600`：数据目录里没有缓存时首次启动，`GET /v1/policy_groups` 里 `G` 先是空的，几秒内出现订阅里的节点；经 `POST /v1/policy_groups/select` 选一个节点后浏览正常。
- [ ] 重启 rurge：`G` 的成员一启动就在（从缓存载入），没有空组阶段；`rurge reload` 同样。
- [ ] 标准输出、`--log-file` 的日志（含 `--log-level verbose`）里搜不到订阅链接里的 token；`GET /v1/profiles/current` 与 `GET /v1/policies/detail?policy_name=G` 里 `policy-path` 的值是 `***`。
- [ ] `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier="tfo=true"`：成员名与 `policies/detail` 里的定义符合预期。
- [ ] 组级 `underlying-proxy=<中继>`：成员名显示为 `<节点> (via <中继>)`；经它浏览时，中继服务器一侧能看到到节点服务器的连接。
- [ ] 把订阅换成一个 Clash YAML 链接：启动输出里有 `W0023`（内容可能不是 Surge 格式），经该组的请求直连（空组兜底）；加 `--empty-group-reject` 后同样的请求被拒绝。
- [ ] 机场更新了订阅（或手动改一个本地订阅文件）：不重启、不重载，组成员随之变化，日志里有一条 `policy group members updated`（只有组名与增减数量）；一个正在进行的大文件下载不中断。
```

- [ ] **Step 3: M3 设计第 16 节（实施期的订正）**

`docs/superpowers/specs/2026-09-23-phase2-m3-groups-subscriptions-design.md`——把

```markdown
**M3c（约 6 个任务）**：① 请求记录两列与 `SessionReporter` → ② `SmartBook` 打分 → ③ 选择、站点记忆与重试列表 → ④ 引擎拨号重试 → ⑤ 5 分钟测速与抽样、能力表翻转、端到端 → ⑥ 文档。
```

换成

```markdown
**M3c（约 6 个任务）**：① 请求记录两列与 `SessionReporter` → ② `SmartBook` 打分 → ③ 选择、站点记忆与重试列表 → ④ 引擎拨号重试 → ⑤ 5 分钟测速与抽样、能力表翻转、端到端 → ⑥ 文档。

## 16. M3a 实施期的订正

本节登记 M3a 计划的「计划期决定」里与本文件文字不同的地方。逐条对应实现的提交见 `docs/superpowers/plans/2026-09-23-phase2-m3a-subscriptions-plan.md` 末尾「执行期修正记录」。

| 编号 | 设计原文 | 订正 |
| ---- | -------- | ---- |
| P2 | 3、V3："同步载入磁盘缓存"的入口位置待定；5.1 未提资源管理器的日志 | `ResourceManager::get` 本来就在返回前同步载入 URL 的磁盘缓存与本地文件，无需新入口；它的日志原先带资源 URL，改为 `get_labelled` 登记的标签（`policy-path of` 加上组名）。`rurge check` 用新增的 `rurge_net::resource::cached` 离线读缓存 |
| P3 | 5.2："空行、`#` 与 `//` 开头的行跳过" | 没有 `[Proxy]` 节时，`;` 开头的行也跳过（与配置解析器的注释规则一致）；跳过的行只报行号与原因，原因不引用行内任何片段（`parse_policy` 自己的消息会引用类型与端口，在 `vmess://` 链接这种非策略行里那是凭据的一部分） |
| P5 | 5.7：代际锁是 `tokio::sync::Mutex`，重建期间持有 | 锁是 `std::sync::Mutex`，只包住"核对当前代 + 发布"与重载的"发布 + 切换"两步；构建在锁外进行——构建期间读到的 `previous` 可能过时，代价只是少复用几个出站 |
| P6 | 1.3：`subnet` 组在阶段 3 之前保持现状 | 空组兜底（M3-D3）会让原本 REJECT 的 `subnet` 组悄悄改走 DIRECT，所以阶段 3 之前 `subnet` 组代表它的 `default`；`category` 等界面参数在 `subnet` 组上不再被当成网络条件 |
| P7 | 4.3 / M3-D7：脱敏名单加 `policy-path` | 同时加 `external-policy-modifier`：它能给导入行设任何参数，口令也在内，而 `password` 只在参数边界处匹配，找不到引号后面的那一个 |
| P8 | 5.9："把 5.2 / 5.3 的告警一并列出"（未定诊断码） | 订阅与装配的告警沿用集合的两个码：跳过的行、重名、派生名被占用、导入行的错误与成环一律 `W0023`，超过 10 000 条 `W0024`；没有内容 `W0022`；导入了未实现的协议 `W0007`（每种一次） |
| P9 | 5.9：`W0022` 文本 "has not been downloaded yet" | 改为 `` `policy-path` has no content yet (never downloaded, or the file cannot be read); its imported members are unknown ``——本地文件读不到时同一条告警也要说得对 |
| P10 | 5.3 / 5.4：未写明 `include-other-group` 取派生前还是派生后的成员 | 取派生前的：全部组先装配，最后才对有中继的组派生，被引用组的中继不传给引用它的组 |
| P11 | 5.4：未写派生名与已有名字冲突 | 派生名已被占用时略去该成员并 `W0023`，绝不改用不经中继的原成员 |
| P14 | 5.9：`rurge check` 读数据目录里的缓存（未写怎样找到数据目录） | `rurge check` 新增 `--data-dir`（环境变量 `RURGE_DATA_DIR`，默认平台数据目录）；`POST /v1/profiles/check` 用守护进程自己的数据目录；两者都经 `rurge_engine::check_profile`，配置本身有错时不做订阅检查 |
| P15 | M3-D3：环境变量 `RURGE_EMPTY_GROUP_REJECT=1` | `RURGE_EMPTY_GROUP_REJECT=true`：rurge 的布尔环境变量（`RURGE_WATCH`、`RURGE_NO_NETWORK` 等）都经 clap 的布尔解析器，只接受 `true` / `false`，`1` 会报 invalid value |
| P16 | 15：M3a 约 9 个任务 | 10 个：原第 ⑦ 项拆成"拨号与视图改读注册表、`EmptyGroup`"与"订阅热重建与代际锁"两个任务 |
| P17 | 5.3：导入行 `to_spec` 的诊断未细分 | 只有错误让该行被跳过（`W0023`）；导入行上的未知参数、无效参数等警告不输出——一个上万行的订阅会把日志淹没 |
| P21 | （无对应文字） | 全局策略（`proxy` 模式）与拨号一样按运行中的策略表校验，导入的策略名也可以当全局策略 |

实施中发现的新出入由各任务追加。
```

执行中若有新的出入，追加到这张表（编号用任务号，如"任务 6（执行期）"）。

- [ ] **Step 4: README 与 CLAUDE.md**

`README.md`——把

```markdown
M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；其余出站协议与 `url-test` / `fallback` / `load-balance` / `smart` / `subnet` 等策略组算法、策略订阅仍在阶段 2 后续里程碑。
```

换成

```markdown
M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；其余出站协议与 `url-test` / `fallback` / `load-balance` / `smart` / `subnet` 等策略组算法仍在阶段 2 后续里程碑。
```

`README.md`——把

```markdown
（`select` 组的选择经 API 读取与切换已实现，阶段 2 / M1）
```

换成

```markdown
（`select` 组的选择经 API 读取与切换已实现，阶段 2 / M1；策略引入与订阅、组级 `underlying-proxy` 已实现，阶段 2 / M3a）
```

`README.md`——把

```markdown
M2b（VMess / AnyTLS）已完成；M2c（Shadow TLS）已完成）
```

换成

```markdown
M2b（VMess / AnyTLS）已完成；M2c（Shadow TLS）已完成；M3a（成员装配与订阅）已完成）
```

`README.md`——把

```markdown
`rurge run` 另支持 `--idle-timeout`、`--request-log-size`、`--watch`（配置热重载）、`--log-file`（按天滚动）等 rurge 专有运行时选项
```

换成

```markdown
`rurge run` 另支持 `--idle-timeout`、`--request-log-size`、`--watch`（配置热重载）、`--log-file`（按天滚动）、`--empty-group-reject`（没有成员的策略组拒绝而不是直连）等 rurge 专有运行时选项
```

`README_en.md`——把

```markdown
M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); the remaining outbound protocols, the group algorithms (`url-test` / `fallback` / `load-balance` / `smart` / `subnet`) and policy subscriptions are later phase-2 milestones.
```

换成

```markdown
M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); M3a (member assembly and subscriptions) is done — `policy-path` subscriptions (a local file or a URL, in Surge format; an update rebuilds only the policy table, without disturbing unrelated connections), `include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`, the group-level `underlying-proxy` (derived `Name (via Relay)` members), group cycle warnings (`W0030`) and the empty-group fallback (DIRECT by default, REJECT with `--empty-group-reject`) are usable; the remaining outbound protocols and the group algorithms (`url-test` / `fallback` / `load-balance` / `smart` / `subnet`) are later phase-2 milestones.
```

`README_en.md`——把

```markdown
(a `select` group's choice can be read and switched over the API, phase 2 / M1)
```

换成

```markdown
(a `select` group's choice can be read and switched over the API, phase 2 / M1; policy including, subscriptions and the group-level `underlying-proxy` implemented, phase 2 / M3a)
```

`README_en.md`——把

```markdown
M2b, VMess / AnyTLS, is done; M2c, Shadow TLS, is done)
```

换成

```markdown
M2b, VMess / AnyTLS, is done; M2c, Shadow TLS, is done; M3a, member assembly and subscriptions, is done)
```

`README_en.md`——把

```markdown
`rurge run` also takes rurge-specific runtime options — `--idle-timeout`, `--request-log-size`, `--watch` (hot reload), `--log-file` (daily rotation) — as CLI flags / env vars only
```

换成

```markdown
`rurge run` also takes rurge-specific runtime options — `--idle-timeout`, `--request-log-size`, `--watch` (hot reload), `--log-file` (daily rotation), `--empty-group-reject` (a policy group without members rejects instead of going direct) — as CLI flags / env vars only
```

`CLAUDE.md`——把

```markdown
对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。
```

换成

```markdown
对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。M3b（测速与自动组）尚未开始。
```

`CLAUDE.md`——把

```markdown
- `docs/acceptance/phase2-manual.md`：阶段 2 手工验收清单（M2a 起新建），
```

换成

```markdown
- `docs/superpowers/plans/2026-09-23-phase2-m3a-subscriptions-plan.md`：阶段 2 / M3a（成员装配与订阅）实施计划（10 个任务）。开头「计划期决定」表（P1–P22）记录核对源码得出的结论与和设计文字不同的决定（资源管理器本来就同步载入缓存、订阅 URL 改用标签记日志、代际锁只包住发布、`subnet` 组暂时代表它的 `default`、`external-policy-modifier` 也脱敏、环境变量取 `true` 等）与「承接事项」；末尾「执行期修正记录」与「延后事项」两张表。
- `docs/acceptance/phase2-manual.md`：阶段 2 手工验收清单（M2a 起新建），
```

`CLAUDE.md`——把

```markdown
cargo test -p rurge-engine --test outbounds_shadow_tls   # 经 Shadow TLS 的端到端用例（回环假服务端 + 夹具自己的伪装站点）
```

换成

```markdown
cargo test -p rurge-engine --test outbounds_shadow_tls   # 经 Shadow TLS 的端到端用例（回环假服务端 + 夹具自己的伪装站点）
cargo test -p rurge-engine --test subscriptions   # 订阅：构建时同步载入、热重建与出站复用、经导入 / 派生成员出站、空组兜底
cargo test -p rurge-policy assemble             # 成员装配：顺序、过滤 / 前缀 / 修饰、全局重名、中继派生、组环
```

`CLAUDE.md`——把

```markdown
cargo run -p rurge -- check -c config.conf      # 校验 Surge 配置（--json / --strict / --platform）
```

换成

```markdown
cargo run -p rurge -- check -c config.conf      # 校验 Surge 配置（--json / --strict / --platform；--data-dir 连同已缓存的订阅一起检查，不联网）
```

- [ ] **Step 5: 过时的注释**

`crates/rurge-api/src/routes/policy_groups.rs`——把

```rust
    // A validly configured group without members (e.g. `subnet`, which keeps
    // its targets in `conditions` / `default`) yields "" here, not an error.
```

换成

```rust
    // A group without members (a subscription not downloaded yet, a filter
    // that let nothing through) yields "" here, not an error.
```

- [ ] **Step 6: 本计划末尾的两张表**

把执行期间与本计划不同的地方逐条写进「执行期修正记录」（哪个任务、计划原文、实际做法、原因、提交），把没做完或新发现、留给以后的事写进「延后事项」（去向写清楚：M3b / M3c / M8 / 有用户报告再说）。计划期已知的几条已经在表里。

- [ ] **Step 7: 门禁与提交**

```bash
git add docs README.md README_en.md CLAUDE.md crates/rurge-api/src/routes/policy_groups.rs
git commit -m "docs: M3a——兼容性清单、API 参考、手工验收、M3 设计第 16 节、README、CLAUDE.md 与计划收尾表"
```

---


## 验收对照（设计第 11 节中属于 M3a 的条目）

| 条目 | 由谁证明 |
| ---- | -------- |
| 1. 两种订阅样本解析正确；装配顺序、过滤、前缀、修饰符合清单 5.2 | Task 2：`a_plain_list_is_read_line_by_line`、`a_whole_profile_gives_its_proxy_section_only`；Task 3：`members_come_in_the_manual_order_each_once`、`the_filter_and_the_prefix_act_in_the_manual_order`、`the_modifier_rewrites_the_imported_lines_only`、`include_other_group_is_recursive_and_a_cycle_gives_nothing`、`two_groups_share_an_identical_import_but_not_another_definition` |
| 2. 订阅更新热重建：没变的成员复用出站；与配置重载并发正确 | Task 5：`an_update_keeps_the_outbounds_of_lines_that_did_not_change`；Task 8：`an_edited_subscription_file_rebuilds_the_registry`、`a_url_subscription_arrives_after_the_start_and_is_cached`、`a_rebuild_of_a_replaced_generation_publishes_nothing` |
| 3. 空组与环按 5.6 处理；`--empty-group-reject` 生效 | Task 1：`a_group_cycle_is_a_warning`；Task 3：`group_cycles_through_members_are_listed`；Task 5：`a_group_on_a_cycle_rejects_and_names_the_cycle`、`an_empty_group_stands_in_direct_or_rejects`；Task 7：`an_empty_group_stands_in_direct_or_rejects`（经真实的 `dial`）；Task 9：`an_empty_group_goes_direct_unless_told_to_reject`（CLI） |
| 4. 组级 `underlying-proxy` 派生正确，派生成员可选、可经链拨号（"可测"在 M3b） | Task 4 的四条用例；Task 5：`a_derived_policy_dials_through_the_group_relay`；Task 7：`the_views_show_imported_and_derived_policies`；Task 9：`a_derived_member_is_reached_through_the_group_relay` |
| 7. 凭据不外泄：`policy-path` 的值、订阅行在 API 输出、日志与错误文本里都不出现 | 见 Review Focus 第 1、2 条列出的用例 |
| 8. fmt / clippy 零警告 / `cargo test --workspace` 全绿 | 每个任务的门禁；`W0008` 在 M3a 仍会因 `url-test` / `fallback` / `load-balance` / `smart` 出现（M3b / M3c 翻转） |
| 9. 需要真实机场订阅与真实节点的项目 | Task 10 写进 `docs/acceptance/phase2-manual.md` 的「M3a」一节，由项目所有者验收 |

第 5、6 条（算法、临时覆盖、三个测试 API）属于 M3b / M3c。

## 执行期修正记录

执行中与本计划不同的每一处：哪个任务、计划原文、实际做法、原因、提交。

| 任务 | 计划原文 | 实际做法 | 原因 | 提交 |
| ---- | -------- | -------- | ---- | ---- |
| 1 | `underlying_cycles`（`E0019`）的图：写出的成员、`subnet` 条件、`default`、`include-other-group`、组自己的中继 | 另加 `include-all-proxies` 收进来的代理（非别名、经 `policy-regex-filter` 放行的 `[Proxy]` 策略）；新增 `ImportOpts::admits`（P1 过滤规则的唯一实现，装配也用它）；`subnet` 组的 `include-other-group` 不再成边；补三条用例 | 评审发现：`Exit = …, underlying-proxy=Hop` + `Hop = select, include-all-proxies=true` 能加载，到装配后拨号 Exit 会无界递归（C3 靠 `E0019` 兜住主配置里的环）；`subnet` 组忽略的参数产生了误报的 `W0030` / `E0019` | ba407d8 |
| 1 | 脱敏用例插在 `snell` 用例之后 | 放在测试模块末尾 | 只是位置 | 5326d77 |
| 3 | 私有函数 `passes(group, name)` | 调 `ImportOpts::admits` | 与加载期的 `E0019` 检查共用一处 P1 规则 | 775023e |
| 3 | 组环：深度优先、只在遇到栈上的组时记环 | 迭代版 Tarjan 求强连通分量：环上的组一个不漏（与成员顺序无关）；每个环上的组各取一条最短环、从先声明的组写起、去重后列出；`include-other-group` 的"成环的组不展开给别人"与导入行的中继成环检查（改为线性）用同一个函数 | 评审发现：经横叉边回到环上的组会漏列，环上的组 REJECT 的集合随成员书写顺序变化 | 5e5478f |
| 3 | 导入行 `to_spec` 出错时，`W0023` 的原因是 `to_spec` 的消息 | 原因只写固定说法加诊断码：`policy `N` has a parameter whose value cannot be used (E0018)`、`… has an `underlying-proxy` that names no policy (E0007)`、`… names a `[Keystore]` item that is missing or of another kind (E0020)`，其余 `… cannot be used (<码>)` | M3-D7：`to_spec` 的消息会引用取值（如 `invalid value `999` for `tos``、整条请求头），修饰值因此会进日志 | 5e5478f |
| 3 | （计划未写） | 中继指向一个已被略去的导入行的导入行一并略去（`W0023` "names a policy that was left out"），沿反向中继边一次遍历 | 否则它留在成员表里、每次拨号都失败；逐轮剔除在长链上是平方复杂度（1 万行 4–10 秒） | 5e5478f、434dee7 |
| 3 | `with_params`：需要时加一层引号 | 值首尾带引号或带首尾空白时加两层引号，读回的值与设下的完全相同 | 解析器对 `key="value"` 去两次引号（先按列表字段、再按参数值并先 trim） | 5e5478f |
| 3 | （计划未涉及，M1a 起的代码） | `headers` 解析错误按序号报：`header #N has no `:``、`header #N has an invalid name` | 原文引用整条请求头，`Authorization Bearer …` 就是凭据 | 5e5478f |
| 5 | 导入 / 派生策略构建失败：`WARN policy=… error=<工厂错误文本>` | 只记策略名 | M3-D7：工厂的错误文本会引用导入行的取值（如不合法的 `sni`） | 66eb655 |
| 5 | `Line` 派生 `Debug` | 手写 `Debug`，不打印定义行 | P18：定义行带导入行的口令与组行的订阅链接 | 66eb655 |
| 5 | 会话说明一律写进请求记录的 `error` | 空组代以 DIRECT 而拨号失败时，请求记录写"说明; 失败原因" | 否则失败原因被说明盖住 | 66eb655 |
| 5 | `PolicyRegistry::contains` 线性扫描 | 查表 | 每次拨号、每一跳链式连接都调用，而条目可达上万 | 66eb655 |
| 8 | Step 1 列出了 `a_rebuild_of_a_replaced_generation_publishes_nothing`，但计划正文漏了这条用例的代码（拼装计划时漏选了该补丁的最后一处修改） | 用副本上验证过的版本（新一代换了配置，并核对当前一代的重建照常发布） | 计划缺陷 | 61b2a4b |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | 订阅导入的节点，其主机名不在"不受 `[Host]` 影响"的名单里（P19）：解析器每代只从主配置取这份名单，订阅在运行期变化而解析器不随之重建 | 有用户报告再说；最迟 M8 |
| 2 | `ChainConnector` 没有运行期的深度守卫（C3）：有界性由装配期的环检查静态保证 | M8 |
| 3 | 链底下的 REJECT 到不了 `dial_internal` 的旁路；链深超过 `MAX_DEPTH` 时 `socket_opener` 给 `None` 而注册表给 REJECT（C4，M1b 起） | M3b（拨号入口会改） |
| 4 | 测试二进制偶发 `STATUS_HEAP_CORRUPTION` / `STATUS_ACCESS_VIOLATION` / 段错误（P22）：未改动的代码上同样复现，根因在某个依赖的原生代码里 | 单独排查，保持跟踪 |
| 5 | rurge 的布尔环境变量（`RURGE_WATCH`、`RURGE_NO_NETWORK`、`RURGE_SYSTEM_PROXY`、本计划的 `RURGE_EMPTY_GROUP_REJECT`）只接受 `true` / `false`，`=1` 报 invalid value；可统一改用 clap 的 `FalseyValueParser`（`1` / `yes` / `on` 也算真） | 等项目所有者决定；单独小改动 |
| 6 | 每次重建都把装配告警逐条再 WARN 一遍：坏行很多的订阅会在每次更新时刷屏 | 有用户报告再说 |
| 7 | 每次重建重新解析全部订阅，不按来源缓存解析结果 | 需要时（M8） |
| 8 | 兼容性清单附录的统计：本计划只按第 5 节的增减改了数字；按"每行第一个状态标记"重数时，另外几节与表里的数字对不上，原因未核对 | 单独核对 |
| 9 | 崩溃恢复顺序缺陷（`rurge run` 在坏配置上先退出、后 `sysproxy.recover()`），与 M3a 无关 | 单独跟进（等项目所有者点头） |
| 10 | 两个组导入同名策略时，命名空间在校验之前就裁决：先声明的组那份若有错被跳过，后一个组那份好的也已被跳过 | 有用户报告再说 |
| 11 | 订阅行能按名字用到主配置的私有材料（`client-cert=<Keystore 条目>`、`underlying-proxy=<主配置策略>`），即订阅作者能让 rurge 向他指定的主机出示用户的客户端证书、或经用户自己的代理连过去；M3b 的自动测速之后无需用户选中该节点也会发生 | 等项目所有者决定（M3b 之前） |
| 12 | 空组被当作中继（策略的 `underlying-proxy` 或组级中继指向一个还没有内容的订阅组）时解析到 DIRECT 兜底，依赖它的策略直连自己的服务器、绕过了使用者设的中继（M3-D3 的直接结果，与 P11 的理由相悖；`--empty-group-reject` 可全局改为拒绝） | 等项目所有者决定 |
| 13 | 导入策略 X 在注册表构建时失败被略去，中继指向 X 的另一条导入策略仍留在成员表里，每次拨号失败（`via X: the policy no longer exists`） | 有用户报告再说；最迟 M8 |
| 14 | 一个会话先取当前代、后取注册表，而重载先发布注册表、后换代：恰好跨过重载的会话可能用旧一代的规则选名、到新一代的注册表里解析，重载删掉或改名的策略会让这一条连接 REJECT 并打一条 ERROR | M3b（拨号入口会改，与 C4 一起） |
| 15 | 订阅重建（解析、装配、构建出站）在 tokio 工作线程上同步执行，大订阅会占住一个工作线程；与重载时 `Runtime::build` 的既有做法相同 | 需要时（M8，与 #7 一起） |
| 16 | 加载期的 `W0030` 只点名每条回边的两端；经横叉边在环上的组不在告警里点名（运行期每个环都 WARN、会话记录写出整个环） | 有用户报告再说 |
| 17 | 坏的 `external-policy-modifier` 值让每条导入行各报一条 `W0023` | 与 #6 一起 |
| 18 | `derive` 按名字线性查主配置的定义行；两个不同的（成员，中继）组合只有在名字里本身含 ` (via ` 时才可能拼出同一个派生名，此时后者静默共用前者的 spec | 有用户报告再说 |
| 19 | `spec/http.rs` 的 `<random-string(…)>` 长度错误会引用括号里的文字（只影响主配置；导入行已由固定说法兜住） | 单独小改动 |
| 20 | 测试偶发失败（计时类，与本分支无关）：`rurge-dns` 的 `bootstrap::set_upstreams_takes_effect_for_the_next_resolve` 与 `resolver::tests::a_partial_result_completes_aaaa_in_the_background`，全工作区运行中各见一两次，单独重跑通过 | 单独排查 |
| 21 | 1 秒内的多次订阅更新只触发一次重建，没有用例钉住（去抖由代码与常数保证） | 有用户报告再说 |
