# 阶段 3 / M2「进程识别」实施计划

> **执行方式**：子代理驱动（superpowers:subagent-driven-development）——每个任务一个新的实现者，做完由新的评审者评审，全部做完后整分支评审一次。本计划写到"改哪里、接口是什么、要哪些用例、判据是什么"为止：实现者先写失败的用例、看到红，再实现到绿；与计划的出入记进末尾的「执行期修正记录」。

**目标**：`PROCESS-NAME` 规则在三个平台上按发起会话的进程匹配，请求记录与 API 带上进程的 PID 与路径。

**架构**：`rurge_platform::procinfo` 同步地查"一条本机连接属于哪个进程"（Windows 的两张带 PID 的表、Linux 的 `/proc/net` 加 `fd` 扫描、macOS 的 `pcblist_n`），表按快照、批量与限频；引擎对 HTTP 与 SOCKS5 监听器上可能从本机来的会话在开始时后台查一次，结果落进会话的进程槽；规则走到 `PROCESS-NAME` 时经 `Verdict::NeedsProcess` 等槽，至多等到查找发起之后 200 毫秒。

**技术栈**：`windows-sys` 0.61.2（IpHelper 与 Threading）、`/proc`、`sysctlbyname` 与 `proc_pidpath`（经 `nix::libc`）、`if-addrs` 0.15.0、tokio 的 `Notify` 与 `Semaphore`；不新增包。

**依据**：设计 `docs/superpowers/specs/2026-10-17-phase3-m2-process-design.md`（下面写"设计"指它；第 11 节 M2-D1 ～ D19）；总设计 `docs/superpowers/specs/2026-10-16-phase3-enhanced-mode-design.md`。

## 全局约束

- 不新增包，`Cargo.lock` 的包数不变（517 个）。`windows-sys` 加两个不带新依赖的特性 `Win32_Foundation`、`Win32_NetworkManagement_IpHelper`（任务 3）；macOS 的 `libc` 经 `nix::libc`（`nix` 0.31.3 无条件地重新导出它），不加依赖边；`nix` 的 `socket` / `net` 特性要 `memoffset`（锁文件里没有），**不开**。`rurge-api` 不加依赖：`ProcessSlot` 经 `rurge-engine` 重新导出。
- `unsafe_code`：全工作区 `forbid`；`rurge-platform` 新增**恰好五个**标 `#[allow(unsafe_code)]` 的函数（Windows 三个，任务 3；macOS 两个，任务 5），连同已有的两个共七个，由任务 3 的名单用例钉住；每个 `unsafe` 块前一行 `// SAFETY:` 写明前提。MSRV 1.89，edition 2024。
- 进程（名字、路径、PID）只进 DEBUG 日志、请求记录与 API，不进 INFO 及以上的行（M2-D13；Windows 的路径常带用户名）。不知道进程从不让会话失败，也不进会话的 `error`（M2-D14）。
- 测试只用回环地址与端口 0，等待都有时限，不用固定的 sleep 当同步（"断言什么都没发生"除外，注释写它等什么、余量多大）；不联网。进程识别的用例只读系统的表，从不改系统设置、不建网卡、不起本仓库之外的程序。**碰到进程查找的用例不用暂停的时钟**（设计 V9：`spawn_blocking` 不算运行时的工作，暂停的时钟会在阻塞线程干活时自动前进）。会在阻塞线程上挂住的假查找必须在 `Drop` 时放开（tokio 的运行时退出时等全部阻塞任务结束）。
- 起 `rurge` 二进制的用例经 `rurge_command()` / `rurge_process()`，给 `--data-dir`，网络与系统代理只经文件后端；任务 7 起这两个辅助函数缺省设 `RURGE_PROCESS_BACKEND=off`，查真实进程的用例在构造之后 `env_remove`。
- "改一行代码看用例变红"的验证之前先写下这一改让哪些会话、查找、下载与文件去哪里；可能离开回环或写到用例的临时目录之外的不做。
- 新写的计时用例提交前单独循环跑二十次以上。
- 只运行本仓库构建出来的二进制与系统自带的只读工具；不下载、不安装任何东西。
- **本机编译不到 `target_os = "linux"` / `"macos"` 的代码**：能做成纯函数的都做成纯函数，在每个宿主上编译与测试（用不到的平台上标 `#[cfg_attr(not(any(<平台>, test)), allow(dead_code))]`，照 `netenv/windows.rs` 的写法）；剩下的薄层照已有模块写，评审者逐行读，第一次 CI 运行时核对（延后事项）。
- 参考实现（mihomo、sing-box 是 GPL，XNU 是 APSL）只取事实，代码不抄。
- Surge 语法就是 rurge 语法；与 Surge 的每一处行为差异都进兼容性清单（任务 8）。
- 注释、日志与错误文本用英文，照所在文件的写法；提交说明的标题用中文。
- 先写失败的用例、看到红，再实现到绿。现有用例变红时：计划点了名的照计划改并在提交说明里说；没点名的停下来报告，不顺手改期望值。
- 门禁（每个任务末尾一次）：`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test -j 8 --workspace --no-fail-fast`（本机 `RUSTFMT` 指向 gnu 工具链的 rustfmt）。聚焦地跑用例也经整个工作区（`cargo test -j 8 --workspace --lib -- <过滤词>`，或 `--test <目标> -- <过滤词>`），不用 `-p`。测试二进制以 `STATUS_HEAP_CORRUPTION` 或 `STATUS_ACCESS_VIOLATION` 退出时单独重跑那一个。跑全量之前 `df -h /d`，剩余不足约 15 GiB 就停下。
- 没有新的诊断码；`W0013` 不再因 `PROCESS-NAME` 出现（任务 7）。

## 评审要点

计划的用例之外最可能咬到使用者的五种情形，每条的用例已加进负责的任务：

1. **一阵同时开的连接**（浏览器打开一个页面）：查找共用读取，没有一条会话因查找失败，每条至多多等 200 毫秒（任务 2 `one_read_answers_a_batch`、任务 6 `a_burst_of_sessions_with_a_slow_lookup_all_complete`）。
2. **客户端在查找之前就走了**（短命的命令行工具、连接已关、进程已退出）：查不到就是"不知道"，会话照常（任务 2 `a_row_that_is_gone_is_none`）。
3. **IPv6 回环上的客户端**（`[::1]`，或双栈套接字连 IPv4 地址时表里的映射形式）：一样查得到（任务 2 `an_ipv4_query_finds_the_mapped_row`；任务 3、4、5 的真读用例各加一次 `[::1]`）。
4. **只知道名字、不知道路径的进程**（Linux 的 `comm`、Windows 的 `System`）：名字写法照常命中，路径与前缀写法不命中，记录的 `pid` 有、`processPath` 是 `null`（任务 1 `a_process_without_a_path_matches_by_name_only`、任务 6 `a_record_without_a_process_sends_nulls`）。
5. **UDP 关联开始 200 毫秒之后才开的流**：期限从关联的查找发起时算，这条流不再等，按"不知道"判、`notes` 里有那一句（任务 6 `a_window_that_has_passed_is_not_waited_for`）。

---

## 文件结构

| 文件 | 任务 | 职责 |
| --- | --- | --- |
| `crates/rurge-config/src/session.rs`、`rule.rs` | 1 | `ProcessInfo.pid`；`PROCESS-NAME` 值的分类 |
| `crates/rurge-rules/src/{matcher.rs,set.rs,engine.rs,set_format.rs,registry.rs}` | 1 | `NeedsProcess`、`EvalCtx` 的进程、`LazyProcess`、`evaluate_with_process`、Windows 的比较、`SYSTEM` 的进程行 |
| `crates/rurge-platform/src/procinfo/mod.rs`（新） | 2 | 查询、快照、限频、是不是本机、`Source` |
| `crates/rurge-platform/src/procinfo/windows.rs`（新）、`Cargo.toml`、`tests/unsafe_functions.rs`（新） | 3 | 两张表与映像路径、unsafe 名单 |
| `crates/rurge-platform/src/procinfo/linux.rs`（新） | 4 | `/proc/net` 与 `fd` 扫描 |
| `crates/rurge-platform/src/procinfo/macos.rs`（新） | 5 | `pcblist_n` 的走法与 `proc_pidpath` |
| `crates/rurge-inbound/src/{process.rs（新）,session.rs,lib.rs}` | 6 | `ProcessSlot`、`SessionHandle` 的进程槽 |
| `crates/rurge-engine/src/{owner.rs（新）,shared.rs,engine.rs,udp.rs,observe.rs,lib.rs}`、`tests/process.rs`（新） | 6 | 查找、规则的等待、UDP 关联、记录 |
| `crates/rurge-api/src/routes/requests.rs` | 6 | `pid`、`processPath` |
| `crates/rurge/src/cli/{runtime.rs,run.rs}`、`src/capabilities.rs`、`tests/cli.rs` | 7 | `PlatformOwners`、只供测试的后端、能力表、端到端 |
| 文档 | 8 | 兼容性清单、`docs/api/phase3.md`、`docs/acceptance/phase3-manual.md`、CLAUDE.md、README 两份 |

---

## Task 1：配置与规则——`ProcessInfo.pid`、写法、`NeedsProcess`、Windows 的比较、`SYSTEM` 的进程行

**改哪里**

1. `rurge-config/src/session.rs`：`ProcessInfo` 加 `pub pid: Option<u32>`（放在最前），三个字段与 `SessionInfo.process` 的文档注释照设计第 2 节改（`SessionInfo.process` 是"调用方事先知道的进程"，离线的 `rule match --process` 填它；经监听器进来的会话不填，进程由引擎在评估时交给规则）。`git grep -n "ProcessInfo {"` 找到全部构造点（`crates/rurge/src/cli/rule.rs` 的 `--process`、`crates/rurge-rules/src/matcher.rs` 的用例）补 `pid: None`。
2. `rurge-config/src/rule.rs`：`PROCESS-NAME` 的分类抽成私有的 `fn process_pattern(value: &str) -> Result<ProcessPattern, String>`，照设计第 5.2 节：值以 `/` 开头、或以 ASCII 字母加 `:\` / `:/` 开头、或以 `\\` 开头是路径；路径而长于 1 个字符、以 `/` 或 `\` 结尾是 `Prefix(value)`；其余路径是 `Path(glob(value, false, false))`；不是路径的是 `Name(glob(value, false, false))`。`ProcessPattern` 不变。
3. `rurge-rules/src/matcher.rs`：
   - `Verdict` 加 `NeedsProcess`。
   - `EvalCtx` 加 `pub(crate) process: ProcessState`，`pub(crate) enum ProcessState { NotAsked, Asked(Option<ProcessInfo>) }`，`EvalCtx::new` 置 `NotAsked`。
   - `Matcher::ProcessName`：`s.process` 有值就比它；否则 `NotAsked` → `NeedsProcess`，`Asked(None)` → `NoMatch`，`Asked(Some(p))` → 比 `p`。
   - `And` / `Or` 与 `set.rs` 的逐条求值的行：一个标记拆成 `needs_process` 与 `needs_resolve`，短路的规矩不变（`AND` 遇到 `NoMatch`、`OR` 与集合遇到 `Match` 当场返回），定不下来时 `NeedsProcess` 优先于 `NeedsResolve`；`Not` 把两种原样传出。`set.rs` 里 ASN 与 IP 两处的 `Err(Verdict::NeedsResolve)` 只置 `needs_resolve`。
   - Windows 的比较：编译与比较各写成带平台参数的纯函数 `fn compile_process_pattern(windows: bool, p: &ProcessPattern) -> ProcessPattern`（`Path` 照今天从归一化的源重建；`windows` 时 `Name` 照 `g.source()` 重建为不分大小写，`GlobOptions { case_insensitive: true, classes: false }`）与 `fn process_matches_for(windows: bool, pattern: &ProcessPattern, p: &ProcessInfo) -> bool`；编译处与 `process_matches` 传 `cfg!(windows)`。不去掉 `.exe`。
4. `rurge-rules/src/engine.rs`：
   - `pub trait LazyProcess: Send + Sync { fn process(&self) -> BoxFuture<'_, Option<ProcessInfo>>; }` 与 `pub struct NoProcess;`（立刻答 `None`），放在 `LazyResolver` 旁边，在 crate 根与 `LazyResolver` 同处导出。
   - `pub async fn evaluate_with_process(&self, s: &SessionInfo, mode: OutboundMode, resolver: &dyn LazyResolver, process: &dyn LazyProcess) -> Decision`；`evaluate` 与 `evaluate_traced` 传 `&NoProcess`、签名不变；`run` 多一个参数。
   - `run` 的循环：先 `if verdict == NeedsProcess { ctx.process = Asked(process.process().await); verdict = 重评 }`，再照旧处理 `NeedsResolve`。一次评估至多问一次进程（之后的规则看同一个 `Asked`），一条规则至多评三遍。
   - `verdict_name` 加 `"needs-process"`。预匹配（`pre_pass`、`pre_match`）不改：`NeedsProcess` 不是 `Match`。
5. `rurge-rules/src/set_format.rs`：`pub fn internal_set_source(set: InternalSet, macos: bool) -> Cow<'static, str>`——`macos` 为假时去掉 `System` 里以 `PROCESS-NAME,` 开头的行，其余与 `internal_set_text` 相同；`registry.rs` 的 `Key::Internal` 分支与 `engine.rs` 用例里解析内部集的那一处改用 `internal_set_source(*i, cfg!(target_os = "macos"))`。`internal_set_text` 与 `the_built_in_sets_are_the_lists_the_manual_gives` 不变（M2-D17）。

**接口**

- 产出：`rurge_config::session::ProcessInfo { pid: Option<u32>, name: String, path: Option<String> }`；`rurge_rules::{LazyProcess, NoProcess}`；`RuleEngine::evaluate_with_process`（签名如上）；`Verdict::NeedsProcess`；`rurge_rules::set_format::internal_set_source`。

**用例**

- `rurge-config`：`process_name_values_are_classified`——`curl`、`Google*`、`C:` 是名字；`/usr/bin/curl`、`/Applications/*.app/Contents/MacOS/*`、`/`、`C:\Program Files\App\app.exe`、`c:/tools/app.exe`、`\\server\share\app.exe` 是路径；`/Applications/ChatGPT.app/`、`C:\Program Files\App\`、`d:/tools/`、`\\server\share\` 是前缀。
- `rurge-rules`（`matcher.rs`）：
  - `process_name_modes`（**点名改写**）：没有进程的会话、上下文没问过 → `NeedsProcess`；问过而不知道 → `NoMatch`；其余断言照旧。
  - `needing_the_process_passes_through_logic_and_sets`：`AND,((PROCESS-NAME,x),(DOMAIN,a.test))` 对 `a.test` 是 `NeedsProcess`、对 `b.test` 是 `NoMatch`；`OR,((PROCESS-NAME,x),(DOMAIN,a.test))` 对 `a.test` 是 `Match`、对 `b.test` 是 `NeedsProcess`；`NOT,((PROCESS-NAME,x))` 是 `NeedsProcess`；集合里 `PROCESS-NAME,x` 在一条命中的 `DOMAIN-KEYWORD` 之前时是 `Match`，集合只有那一行时是 `NeedsProcess`。
  - `needing_the_process_comes_before_needing_a_lookup`：对一个域名，`AND,((IP-CIDR,10.0.0.0/8),(PROCESS-NAME,x))` 与 `OR,((IP-CIDR,10.0.0.0/8),(PROCESS-NAME,x))` 都是 `NeedsProcess`。
  - `process_comparison_follows_the_platform`：`process_matches_for(true, …)` 里名字 `CHROME.EXE` 命中 `chrome.exe`、路径 `c:/program files/app/APP.EXE` 命中 `C:\Program Files\App\app.exe`、前缀 `C:\PROGRAM FILES\` 命中它；`(false, …)` 里 `Curl` 不命中 `curl`、路径区分大小写；两种平台上 `chrome` 都不命中 `chrome.exe`。
  - `a_process_without_a_path_matches_by_name_only`（评审要点 4）：`{ pid: Some(7), name: "app", path: None }` 命中 `PROCESS-NAME,app`，不命中 `PROCESS-NAME,/usr/bin/app` 与 `PROCESS-NAME,/usr/bin/`。
- `rurge-rules`（`engine.rs`，一个计数的 `LazyProcess` 与计数的 `LazyResolver`）：
  - `a_process_rule_asks_for_the_process_once`：`PROCESS-NAME,a,REJECT`、`PROCESS-NAME,b,REJECT`、`FINAL,DIRECT`，进程是 `b` → 第二条命中，问了 1 次。
  - `the_process_is_asked_before_the_name_is_resolved`：`AND,((PROCESS-NAME,x),(IP-CIDR,10.0.0.0/8)),REJECT` 加 `FINAL,DIRECT`，进程是 `y` → 解析器被问 0 次；进程是 `x` → 1 次。
  - `a_given_process_is_not_asked_for`：`s.process` 有值 → 问 0 次。
  - `pre_matching_does_not_ask_for_the_process`：带 `pre-matching` 的 `RULE-SET`（集合里有 `PROCESS-NAME,x`，用 `engine.rs` 用例里现有的集合夹具）→ `pre_match_domain` 是 `None`；`evaluate_with_process` 的决定的 `reason` 是 `Rule`（在自己的位置上命中），不是 `PreMatching`。
  - `the_system_set_has_its_process_lines_on_macos_only`（`set_format.rs`）：`internal_set_source(System, true)` 等于 `internal_set_text(System)`；`(System, false)` 是那 18 行域名、没有 `PROCESS-NAME`；`Lan` 两种都等于原文。
  - `a_system_rule_set_never_asks_for_the_process`（`#[cfg(not(target_os = "macos"))]`）：`RULE-SET,SYSTEM,DIRECT` 加 `FINAL,DIRECT`，到 `example.com` 的会话 → 问 0 次；`#[cfg(target_os = "macos")]` 的同名用例断言 1 次。
  - `evaluate_future_is_send` 加 `evaluate_with_process`。

**判据**：用例绿；现有用例里预期只有 `process_name_modes` 因 `NeedsProcess` 变红，别的变红停下报告；门禁绿。

## Task 2：`rurge_platform::procinfo`——查询、快照、限频、是不是本机

**改哪里**（`crates/rurge-platform/src/procinfo/mod.rs`，`lib.rs` 里 `pub mod procinfo`）

1. 公开的类型照设计第 3.1 节：`Protocol { Tcp, Udp }`（`Clone, Copy, Debug, PartialEq, Eq, Hash`）、`Query { protocol, local: SocketAddr, remote_port: Option<u16>, since: Instant }`、`Owner { pid: u32, name: String, path: Option<String> }`（`Clone, Debug, PartialEq, Eq`）、`Finder`（`Send + Sync`）：`new() -> Finder`、`owner(&self, &Query) -> Option<Owner>`（阻塞）、`is_local(&self, IpAddr) -> Option<bool>`（不阻塞）。
2. `pub(crate)`：`Row { local: SocketAddr, remote: Option<SocketAddr>, owner: RowOwner }`（UDP 的 `remote` 恒为 `None`）、`RowOwner { Pid(u32), Inode(u64) }`、`enum ReadError { Os(String), Layout }`、
   `trait Source: Send + Sync { fn read(&self, protocol: Protocol) -> Result<Vec<Row>, ReadError>; fn pid(&self, owner: RowOwner) -> Option<u32> /* 缺省：Pid(p) 是 Some(p)，Inode 是 None */; fn describe(&self, pid: u32) -> Option<(String, Option<String>)>; fn addresses(&self) -> Vec<IpAddr>; }`（`read` 返回两个地址族的行，IPv4 在前）、`Finder::with_source(source: Box<dyn Source>, address_ttl: Duration) -> Finder`；常数 `ADDRESS_TTL = 2 s`、`TCP_GRACE = 2 s`、`READ_GAP = 20 ms`。
3. 行为（设计第 3.1、3.2 节）：
   - 地址先 `to_canonical()` 再比；比较地址与端口，不看 IPv6 的 flowinfo 与作用域号。TCP 找本地端点等于 `local`、远端端口等于 `remote_port` 的行；UDP 先找本地端点相等的行，再找同端口、本地地址未指定的行——IPv4 的查询依次是 `0.0.0.0`、`::`，IPv6 的只有 `::`。
   - 是不是本机：回环（规范化之后的 `127.0.0.0/8` 与 `::1`）一定是；否则看本机地址的列表（`source.addresses()` 规范化后存下，超过 `address_ttl` 就在 `owner` 里重取；问系统时不持有任何锁）；不在列表里时 `owner` 不读任何表就是 `None`。`is_local` 从不问系统：回环 → `Some(true)`，列表不超过 `address_ttl` → 按列表答，否则 `None`。
   - 快照：每种协议一把锁、一份快照（行、读取开始的时刻、PID 的备忘）；TCP 找到了且快照不早于 `since - TCP_GRACE` 就用它；快照不早于 `since` 时找不到就是 `None`；其余情形重读一次（一次查询至多读一次）。UDP 找到了也要快照不早于 `since`。同一种协议的两次读取之间至少隔 `READ_GAP`（从上一次读取结束算，在锁里等够）。
   - `describe` 的结果按快照备忘（同一份快照里一个 PID 只问一次）；`describe` 是 `None` 时这次查询是 `None`；PID 0 永远不是主人。
   - 读取失败：这次是 `None`；每一种失败（TCP 表、UDP 表、布局对不上）第一次 WARN（`process lookup: the <tcp|udp> table could not be read: <错误>`，布局是 `… has a layout this build does not know`），之后 DEBUG；判断写成 `Warnings::first_time(&self, failure) -> bool`。
4. `NoTables`（`Source`：表都是空的）；这一任务里 `Finder::new()` 在每个平台上都用它，任务 3、4、5 各自换掉自己的平台。

**接口**

- 产出：上面的公开类型；`pub(crate)` 的 `Row`、`RowOwner`、`ReadError`、`Source`、`Finder::with_source`（任务 3 ～ 5 实现 `Source`）。

**用例**（`mod.rs` 的单元用例，一个假的 `Source`：可设的表、地址与名字，记下每次读取开始与结束的时刻和 `describe`、`addresses` 的次数；用将来的 `Instant` 作 `since` 来造新旧，不靠等）

- `a_tcp_hit_in_a_recent_snapshot_needs_no_read`（`since` 比快照晚 1.5 秒）、`a_tcp_hit_in_an_old_snapshot_reads_again`（晚 2.5 秒）、`a_miss_in_a_snapshot_from_after_the_start_is_final`、`a_miss_in_an_earlier_snapshot_reads_again`、`a_udp_hit_needs_a_snapshot_from_after_the_start`。
- `one_read_answers_a_batch`（评审要点 1）：16 个线程的查询（`since` 都早于第一次读取）→ 读取至多 2 次。
- `reads_of_a_table_are_20_ms_apart`：同一种协议相邻两次读取，后一次开始与前一次结束之间不短于 20 毫秒；TCP 与 UDP 互不等。
- `udp_falls_back_to_the_unspecified_address`：IPv4 的查询先 `0.0.0.0:p`、没有时 `[::]:p`；IPv6 的查询只认 `[::]:p`、不认 `0.0.0.0:p`。
- `an_ipv4_query_finds_the_mapped_row`（评审要点 3）：本地端点是 `[::ffff:127.0.0.1]:p` 的 TCP 行由 `127.0.0.1:p` 的查询找到；`the_scope_id_is_not_compared`。
- `a_source_that_is_not_this_machine_reads_nothing`：列表里没有的地址 → `None`，表读了 0 次。
- `the_address_list_follows_its_ttl`：`address_ttl` 为 0 时每次 `owner` 都重取列表、`is_local` 对非回环答 `None`；为一小时时只取一次、`is_local` 按列表答；回环在没有列表时也是 `Some(true)`，`addresses` 0 次。
- `a_process_is_described_once_per_snapshot`。
- `a_row_that_is_gone_is_none`（评审要点 2）：快照不早于 `since` 而没有那一行 → `None`；行在、`describe` 答 `None`（进程已退出）→ `None`。
- `rows_with_pid_0_are_never_an_owner`；`warnings_say_each_failure_once`。

**判据**：用例绿；门禁绿（这一任务没有 unsafe）。

## Task 3：Windows 的读取与 unsafe 名单

**改哪里**（`crates/rurge-platform/src/procinfo/windows.rs`）

1. 纯函数（每个宿主上编译）：`parse_tcp4(&[u8]) -> Vec<Row>`、`parse_tcp6`、`parse_udp4`、`parse_udp6`，布局照设计第 3.3 节（偏移 0 是行数，行从偏移 4 开始；TCP IPv4 24 字节；TCP IPv6 56 字节：本地地址 0、作用域号 16、本地端口 20、远端地址 24、作用域号 40、远端端口 44、状态 48、PID 52；UDP IPv4 12 字节；UDP IPv6 28 字节）；端口是那个 DWORD 的前两个字节、网络字节序，其余小端；状态 11（`TIME_WAIT`）与 PID 0 的行跳过；行数大于放得下的只读放得下的；`owner` 是 `RowOwner::Pid`。
2. 三个 unsafe 函数（`#[cfg(windows)]`，各自 `#[allow(unsafe_code)]`，文档注释写安全前提）：
   - `fn tcp_table(family: u32) -> io::Result<Vec<u8>>`：`GetExtendedTcpTable(缓冲区, &mut 大小, 0, family, TCP_TABLE_OWNER_PID_CONNECTIONS, 0)`；
   - `fn udp_table(family: u32) -> io::Result<Vec<u8>>`：`GetExtendedUdpTable(…, UDP_TABLE_OWNER_PID, 0)`；
   - 两者的缓冲区是 `Vec<u32>`（四字节对齐）：先以 0 字节问大小，得 `ERROR_INSUFFICIENT_BUFFER`（122）就按给出的大小分配再读，表长大了再来，至多 4 次；返回 0 是成功，别的是 `io::Error::from_raw_os_error`；交出的字节由每个 `u32` 的 `to_le_bytes` 拼成（不做指针转换）。
   - `fn image_path(pid: u32) -> Option<String>`：`OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid)`，空句柄是 `None`；句柄交给 `OwnedHandle::from_raw_handle`（同 `process::windows::job_for`）；`QueryFullProcessImageNameW(句柄, PROCESS_NAME_WIN32, 缓冲区, &mut 长度)`，缓冲区 32768 个 UTF-16 单元，失败是 `None`；`String::from_utf16_lossy`。
   - `AF_INET = 2`、`AF_INET6 = 23` 写成本地常数，注明取自 `ws2def.h`（不加 `Win32_Networking_WinSock`）。
3. `Tables`（`#[cfg(windows)]`，实现 `Source`）：`read` 按两个地址族各读一张、IPv4 在前；`describe`：PID 4 → `("System", None)`（不去打开它），其余经 `image_path`，名字是最后一个 `\` 之后的部分；`addresses` 经 `if_addrs::get_if_addrs()`（错误时空表）。`Finder::new()` 在 Windows 上用它。
4. `Cargo.toml`：`windows-sys` 的特性加 `Win32_Foundation` 与 `Win32_NetworkManagement_IpHelper`；`[lints.rust]` 上面的注释改为：通知 WinINet、把外部程序放进 Job Object、读套接字的表与进程的路径（`procinfo`）是 FFI 调用，lint 在这里是 `deny`，只有名单上的函数放开（`tests/unsafe_functions.rs`），别的 crate 照旧 `forbid`。
5. `crates/rurge-platform/tests/unsafe_functions.rs`：`every_unsafe_exception_is_on_the_list`——读 `src/` 下全部 `.rs`；每一行去掉首尾空白之后以 `#[` 或 `#![` 开头、含 `unsafe_code` 的，必须恰好是 `#[allow(unsafe_code)]`，它之后第一个不是属性、不是注释、不是空行的行必须是一个 `fn`（可带 `pub`、`pub(crate)`），取函数名；（以 `/` 分隔的相对路径，函数名）排序后等于名单：`src/process.rs job_for`、`src/procinfo/windows.rs image_path`、`src/procinfo/windows.rs tcp_table`、`src/procinfo/windows.rs udp_table`、`src/sysproxy/windows.rs notify_wininet`。用例的注释写：加一个放开的函数要改这张名单，并在提交说明与 CLAUDE.md 的 `unsafe_code` 一段里说。

**接口**

- 消费：任务 2 的 `Source`、`Row`、`RowOwner`、`ReadError`。
- 产出：名单用例（任务 5 往里加两行）。

**用例**

- 四种行的解析对手拼的缓冲：正常的行；行数说 3 而只放得下 2 行；行数为 0；`TIME_WAIT` 与 PID 0 的行被跳过；IPv6 的作用域号不进地址；端口 6152 写作 `0x18 0x08`。
- `this_process_owns_its_loopback_sockets`（`#[cfg(windows)]`，只读）：`127.0.0.1` 上一条 TCP 连接（查客户端那一侧）与一个 UDP 套接字各查一次 → PID 是 `std::process::id()`，名字与路径与 `std::env::current_exe()` 的不分大小写相等；在 `[::1]` 上再来一次（绑不上 `::1` 就跳过并打印一句；评审要点 3）。
- `the_system_process_has_no_path`（`#[cfg(windows)]`）：`describe(4)` 是 `("System", None)`。
- `every_unsafe_exception_is_on_the_list`。
- 基准（`#[ignore]`，不作门禁）`a_lookup_with_many_connections_open`：开 500 条回环连接，量一次新查询（读表加问路径）的耗时并打印；数字记进执行期修正记录。

**判据**：用例在本机全绿；`cargo clippy` 在本机覆盖了 Windows 的全部代码；门禁绿。

## Task 4：Linux 的读取

**改哪里**（`crates/rurge-platform/src/procinfo/linux.rs`，全部在每个宿主上编译）

1. `pub(crate) trait Fs: Send + Sync { fn read(&self, path: &str) -> io::Result<String>; fn list(&self, path: &str) -> io::Result<Vec<String>>; fn link(&self, path: &str) -> io::Result<String>; }`；`RealFs` 用 `std::fs`（`read_to_string`、`read_dir` 的条目名、`read_link` 的 `to_string_lossy`）。
2. `parse_proc_net(text: &str, v6: bool, little_endian: bool, udp: bool) -> Vec<Row>`（设计第 3.4 节）：第一行是表头；按空白切，第 2 列本地端点、第 3 列远端、第 10 列 inode；inode 为 0 与读不懂的行跳过；端口是普通的十六进制；IPv4 地址是 8 位十六进制，`little_endian` 时取它的 `to_le_bytes`、否则 `to_be_bytes`；IPv6 是四段这样的 8 位十六进制依次拼成 16 字节；`owner` 是 `RowOwner::Inode`。
3. `Tables<F: Fs>`（实现 `Source`）：`read` 读 `/proc/net/tcp` 与 `tcp6`（或 `udp`、`udp6`；`*6` 不在时当空表），字节序传 `cfg!(target_endian = "little")`；`pid(Inode(n))`：一张跨快照的 inode → PID 表，不在表里时先扫"最近找到过的进程"（至多 16 个，最近用到的在前）的 `/proc/<pid>/fd`，再扫 `/proc` 下全部数字目录，每个链接是 `socket:[N]` 的 `fd` 都记进表，读不了的目录不声不响地跳过；每次读表之后，表只留两种协议最近一次读到的 inode 的并集；`describe`：`/proc/<pid>/exe` 的链接去掉末尾的 ` (deleted)`，名字是最后一个 `/` 之后的部分；链接读不了时名字取 `/proc/<pid>/comm` 去掉末尾的换行、路径 `None`；都读不了是 `None`；`addresses` 同任务 3。`Finder::new()` 在 Linux 上用 `Tables<RealFs>`。`RealFs` 与 `Tables<RealFs>` 的构造在别的平台上标 dead_code 的 `cfg_attr`。

**接口**

- 消费：任务 2 的 `Source` 等。

**用例**（内存里的 `/proc`：`MemFs`，数 `list` 的调用）

- `parse_proc_net` 对 `tcp`、`tcp6`、`udp`、`udp6` 的夹具，两种字节序（同一个地址在两份夹具里写法不同、读出相同）；`::ffff:127.0.0.1` 的映射形式（小端写作 `0000000000000000FFFF00000100007F`）；inode 0；表头与坏行。夹具注明是按内核打印的格式写的。
- inode 扫描：最近的进程里有时不 `list("/proc")`；没有时扫全部；一次扫描记下的别的 inode 让下一次查找不再扫描；读不了的 `fd` 目录被跳过；表的修剪；最近的进程至多 16 个。
- `describe`：` (deleted)` 被去掉；只有 `comm`（15 个字节原样）；两个都读不了是 `None`。
- `this_process_owns_its_loopback_sockets`（`#[cfg(target_os = "linux")]`）：同任务 3，路径与 `canonicalize(current_exe())` 逐字节相等；`[::1]` 同样。

**判据**：用例在本机绿（真读用例在本机不编译）；门禁绿；Linux 上的真实读取第一次 CI 运行时核对（延后事项）。

## Task 5：macOS 的读取

**改哪里**（`crates/rurge-platform/src/procinfo/macos.rs`）

1. 纯函数 `walk(buf: &[u8], udp: bool) -> Result<Vec<Row>, ReadError>`（每个宿主上编译），照设计第 3.5 节：跳过开头的 `xinpgen`（它的第一个 u32 是自己的长度）；之后每一项以（长度 u32、种类 u32）开头，前进"长度向上取整到 8 的倍数"；长度不大于 24、剩下不足 8 字节或这一项越过缓冲区时停；种类 `0x10`（`XSO_INPCB`）开始一条记录：外部端口在 16、本地端口在 18（网络字节序），`inp_vflag` 在 44，设了 `0x1`（`INP_IPV4`）取 IPv4 的地址（外部在 60、本地在 76），否则设了 `0x2`（`INP_IPV6`）取 16 字节（外部在 48、本地在 64），都没设的记录不用，这一项短于 80 字节的不用；种类 `0x1`（`XSO_SOCKET`）属于当前记录，`so_last_pid` 在 68，短于 72 字节的不用；其余种类跳过；数字小端；有项而没有一条两项都够长的记录是 `Err(ReadError::Layout)`，没有项是空表；TCP 行的 `remote` 是外部端点，`udp` 时是 `None`；`owner` 是 `RowOwner::Pid`。
2. 两个 unsafe 函数（`#[cfg(target_os = "macos")]`，各自 `#[allow(unsafe_code)]`，`libc` 经 `nix::libc`）：`fn pcblist(name: &CStr) -> io::Result<Vec<u8>>`（`sysctlbyname` 先以空缓冲区问大小，分配大小加八分之一，`ENOMEM` 就加倍再来，至多 4 次，按返回的实际长度截断）；`fn pid_path(pid: u32) -> Option<String>`（`proc_pidpath`，缓冲区 `PROC_PIDPATHINFO_MAXSIZE` 字节，返回值不大于 0 是 `None`，`String::from_utf8_lossy`）。
3. `Tables`（`#[cfg(target_os = "macos")]`，实现 `Source`）：`read` 读 `c"net.inet.tcp.pcblist_n"` 或 `c"net.inet.udp.pcblist_n"` 再 `walk`；`describe` 经 `pid_path`，名字是文件名；`addresses` 同任务 3。`Finder::new()` 在 macOS 上用它。不读 `kern.osrelease`。
4. 任务 3 的名单加 `src/procinfo/macos.rs pcblist` 与 `src/procinfo/macos.rs pid_path`。

**接口**

- 消费：任务 2 的 `Source` 等、任务 3 的名单用例。

**用例**

- `walk` 对用例里拼的缓冲：两种 `xinpcb_n` 的长度（如 136 与 160 字节，`xsocket_n` 如 96 与 104）读出相同的行；不认识的种类被跳过；太短的项让那条记录不用；结尾的 `xinpgen` 停住走法；`inp_vflag` 为 `0x1`、`0x2`、`0x3`；有项而没有完整记录是 `Layout`；只有头尾的空表是空表；越过缓冲区的项不 panic。
- `this_process_owns_its_loopback_sockets`（`#[cfg(target_os = "macos")]`）：同任务 4。
- 名单用例照七行的名单绿。

**判据**：用例在本机绿；门禁绿；两个 unsafe 函数与真读用例第一次 CI 运行时核对（延后事项）。

## Task 6：引擎——进程槽、查找、规则的等待、记录与 API

**改哪里**

1. `crates/rurge-inbound/src/process.rs`（新，`lib.rs` 导出 `ProcessSlot`）：
   ```rust
   #[derive(Clone)]
   pub struct ProcessSlot(Arc<Inner>); // launched: Option<Instant>, value: OnceLock<Option<ProcessInfo>>, settled: tokio::sync::Notify
   impl ProcessSlot {
       pub fn unknown() -> ProcessSlot;                     // settled, no process, nothing launched
       pub fn known(process: ProcessInfo) -> ProcessSlot;   // settled with it, nothing launched
       pub fn pending() -> ProcessSlot;                     // pending_at(Instant::now())
       pub fn pending_at(launched: Instant) -> ProcessSlot;
       pub fn settle(&self, process: Option<ProcessInfo>);  // the first one wins
       pub fn get(&self) -> Option<&Option<ProcessInfo>>;   // None while pending
       pub fn found(&self) -> Option<&ProcessInfo>;
       pub fn launched(&self) -> Option<Instant>;
       /// `None`: `deadline` came first. A settled slot answers at once, whatever the deadline.
       pub async fn settled_by(&self, deadline: Instant) -> Option<Option<ProcessInfo>>;
   }
   ```
   `settled_by` 先 `enable` 一个 `notified()` 再看值（不丢唤醒），期限经 `tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), …)`。`Debug` 只显示落定与否和 PID。
2. `rurge-inbound/src/session.rs`：`SessionHandle` 加字段 `process: ProcessSlot`；`pub fn new_with_slot(id: u64, session: SessionInfo, token: CancellationToken, process: ProcessSlot) -> Arc<SessionHandle>`，`new_with_token` 转调它、传 `ProcessSlot::unknown()`（`new` 不变）；`pub fn process(&self) -> &ProcessSlot`。
3. `crates/rurge-engine/src/owner.rs`（新，`pub mod owner`；crate 根导出 `OwnerLookup`、`OwnerQuery`，并 `pub use rurge_inbound::ProcessSlot`）：设计第 4.3 节的 `OwnerQuery { transport, local, remote_port, since }` 与 `OwnerLookup`；常数 `PROCESS_WAIT = 200 ms`、`LOOKUPS = 2`、`NOT_IN_TIME = "Process not known within 200 ms: PROCESS-NAME did not match"`、`SLOW = "process lookup took longer than 200 ms; PROCESS-NAME rules decided without the process"`；`pub(crate) fn looked_up(listener: ListenerKind) -> bool`（`Http`、`Socks5` 为真）；`pub(crate) struct SlotProcess<'a>`：`new(&'a ProcessSlot)`，实现 `LazyProcess`（期限是 `launched + PROCESS_WAIT`，没有 `launched` 时是现在；到期时记下），`fn timed_out(&self) -> bool`。
4. `shared.rs`：`EngineShared.owners: Option<Arc<dyn OwnerLookup>>`，`new()` 里是 `None`（文档注释：`None` 什么也不查，缺省与现有的全部用例都是它；`rurge run` 设平台的查找）。
5. `engine.rs`：
   - `Engine` 加 `owner_permits: Arc<tokio::sync::Semaphore>`（`LOOKUPS` 个许可）与 `process_wait_warned: AtomicBool`。
   - `pub(crate) fn look_up_owner(&self, session: &SessionInfo) -> ProcessSlot`：没有 `owners`、`!looked_up(session.listener)`、`is_local(session.src.ip()) == Some(false)` 时是 `ProcessSlot::unknown()`；否则 `pending()`，`tokio::spawn` 一个任务：`acquire_owned` 许可 → `spawn_blocking` 调 `owner(&OwnerQuery { transport: Transport::Tcp, local: session.src, remote_port: Some(session.in_port), since: <槽的 launched> })` → `settle`（许可关闭与 `JoinError` 都按 `None`）→ 一行 DEBUG：字段 `src`、`listen_port`、`elapsed_ms`，查到时另有 `pid`、`name`、`path`，文字 `process lookup found the process` 或 `process lookup found no process`（计划期决定 P1）。
   - `pub(crate) fn new_handle_with(&self, session: SessionInfo, process: ProcessSlot) -> Arc<SessionHandle>`（今天 `new_handle` 的身体，改用 `SessionHandle::new_with_slot`）；`new_handle(session)` 转调它、传 `ProcessSlot::unknown()`（内部会话与测速照旧走它）。
   - `Dialer::dial`：`let process = self.look_up_owner(&session); let handle = self.new_handle_with(session, process);`。
   - `choose_policy`：`let process = SlotProcess::new(handle.process());`，规则用 `evaluate_with_process(…, &process)`；之后 `process.timed_out()` 时依次 `handle.add_note(NOT_IN_TIME)`、`tracing::debug!(session = handle.id(), "{NOT_IN_TIME}")`、`process_wait_warned` 第一次由假变真时 `tracing::warn!("{SLOW}")`——这几行写在子规则那一句之前。
6. `udp.rs`：`Association` 加 `process: ProcessSlot`，`serve` 里由 `engine.look_up_owner(&template)` 算一次（查的是控制连接：`template` 的 `src` 与 `in_port`）；用例里的 `stuck()` 填 `ProcessSlot::unknown()`；`run_flow` 用 `engine.new_handle_with(session, association.process.clone())`。
7. `observe.rs`：`RequestRecord` 加 `pub process: ProcessSlot`（文档注释：读记录时再看，会话结束之后才查到的进程也在）；`record_of` 填 `h.process().clone()`。
8. `crates/rurge-api/src/routes/requests.rs`：`RequestJson` 在 `listen_port` 之后加 `pid: Option<u32>` 与 `process_path: Option<String>`，取自 `r.process.found()` 的 `pid` 与 `path`。

**接口**

- 消费：任务 1 的 `LazyProcess`、`evaluate_with_process`、`ProcessInfo.pid`。
- 产出：`rurge_inbound::ProcessSlot`（上面的方法）、`SessionHandle::{new_with_slot, process}`；`rurge_engine::{OwnerLookup, OwnerQuery, ProcessSlot}`；`EngineShared.owners`；`RequestRecord.process`；API 记录的 `pid` 与 `processPath`（任务 7 的端到端读它们）。

**用例**

- `rurge-inbound`（`process.rs`）：`the_first_settle_wins`；`a_waiter_wakes_when_the_slot_settles`（不等到期限）；`a_deadline_that_comes_first_is_none`；`a_settled_slot_answers_whatever_the_deadline`；`the_debug_of_a_slot_shows_no_name_or_path`。
- `rurge-engine`（`owner.rs` 的单元用例）：`only_the_two_proxy_listeners_are_looked_up`（五种 `ListenerKind`）；`a_window_that_has_passed_is_not_waited_for`（评审要点 5：`pending_at(Instant::now() - 300 ms)` 的槽，`tokio::time::timeout(Duration::ZERO, SlotProcess::new(&slot).process())` 是 `Ok(None)`、`timed_out()` 为真——第一次轮询就答，不靠时间）；`a_settled_slot_is_not_a_timeout`（`known` 与 `unknown` 都在第一次轮询答、`timed_out()` 为假）。
- `rurge-engine`（`tests/process.rs`，新）：`harness_with` 加一个假的 `OwnerLookup`（`is_local` 的答案、`owner` 的答案、可选的闸——`owner` 在闸上阻塞，闸在 `Drop` 时放开——、两个方法各自的调用次数）；经真实的 HTTP 与 SOCKS5 监听器，目标是回环上的 `TestServer`；规则用 `PROCESS-NAME,app.exe,…`；不用暂停的时钟。
  - `a_process_rule_matches_through_both_listeners`：`PROCESS-NAME,app.exe,REJECT` 加 `FINAL,DIRECT`，假查找答 `app.exe` → HTTP 的 `CONNECT` 与 SOCKS5 的会话都被拒，记录的 `rule` 是那一条、`process.found()` 的 PID 是假查找给的。
  - `a_session_that_needs_no_process_does_not_wait`：规则里没有 `PROCESS-NAME`、闸关着 → 会话照样完成（回显成功）；放开闸之后记录带上进程。
  - `a_rule_waits_for_the_lookup`：假查找先睡 50 毫秒再答 → 规则命中。
  - `a_slow_lookup_is_decided_without_the_process`：闸关着 → 会话按不命中判（走 `FINAL,DIRECT`、回显成功），`notes` 含 `NOT_IN_TIME`；放开闸 → 有界时间里（`wait_until`）记录带上 PID 与路径。
  - `the_first_slow_lookup_is_warned_once`（`LogCapture`）：两条到期的会话 → `SLOW` 的 WARN 只有一行、不带地址与进程；`NOT_IN_TIME` 的 DEBUG 两行。
  - `a_source_that_is_not_local_is_not_looked_up`：`is_local` 答 `Some(false)` → `owner` 0 次、记录里没有进程、规则不命中、`notes` 里没有那一句；答 `None` 时照查。
  - `an_internal_session_is_not_looked_up`：`Engine::dial_internal` 一条到 `TestServer` 的会话，规则是 `PROCESS-NAME,app.exe,REJECT` → `owner` 0 次、会话不被拒。
  - `a_udp_association_is_looked_up_once`：`PROCESS-NAME,app.exe,DIRECT` 加 `FINAL,REJECT`，一个 SOCKS5 UDP 关联里发往两个回环 `udp_echo` 目标的流都收到回显 → `owner` 1 次，两条流的记录带同一个 PID。
  - `a_record_shows_a_process_found_after_the_session_ended`：`DOMAIN,ads.test,REJECT`（规则不看进程）、闸关着 → 会话被拒、结束；放开闸 → `recent` 里那条记录带上 PID。
  - `a_burst_of_sessions_with_a_slow_lookup_all_complete`（评审要点 1）：32 条并发会话经 HTTP 监听器，`PROCESS-NAME,app.exe,DIRECT` 加 `FINAL,DIRECT`，假查找每次睡 30 毫秒 → 全部完成、没有一条失败；之后每条记录都带上 PID（有界等待）。
- `rurge-api`：`every_field_of_a_record_is_sent_under_its_own_name`（**点名改写**：记录带 `ProcessSlot::known(ProcessInfo { pid: Some(4321), name: "app.exe", path: Some("C:\\Apps\\app.exe") })`，JSON 在 `listenPort` 之后多 `"pid": 4321, "processPath": "C:\\Apps\\app.exe"`）；`record()` 辅助函数填 `ProcessSlot::unknown()`；`a_record_without_a_process_sends_nulls`（评审要点 4：`unknown()` 与 `pending()` 两个键都是 `null`；`{ pid: Some(7), name: "app", path: None }` 是 `7` 与 `null`）。

**判据**：用例绿（计时的单独循环跑二十次）；门禁绿。这是最该细读的一个任务（并发、唤醒、期限、阻塞线程）。

## Task 7：可执行文件——平台的查找、只供测试的后端、能力表、端到端

**改哪里**

1. `crates/rurge/src/cli/runtime.rs`：`pub struct PlatformOwners(rurge_platform::procinfo::Finder)`，`new()`；实现 `OwnerLookup`：`OwnerQuery` 译成 `Query`（`Transport::Tcp` → `Protocol::Tcp`，`Udp` 同理），`Owner` 译成 `ProcessInfo { pid: Some(pid), name, path }`；`is_local` 直接转调。`pub fn owners_backend(value: Option<&OsStr>) -> Result<bool, String>`：`None` 是 `true`（查本机），恰好 `off` 是 `false`，别的取值是 ``unknown RURGE_PROCESS_BACKEND value `<取值>` (expected off)``（非 UTF-8 的取值按 `to_string_lossy` 写出）。
2. `run.rs`：在读 `network_backend` 的那一段之后，照同样的写法读 `owners_backend(std::env::var_os("RURGE_PROCESS_BACKEND").as_deref())`，错误时 `eprintln!("error: {e}")`、`recover_without_a_log(&args.runtime)`、以 2 退出；`shared.processes = …` 旁边设 `shared.owners`（`true` 时 `Some(Arc::new(PlatformOwners::new()))`）。不加命令行选项（M2-D19）。
3. `crates/rurge/src/capabilities.rs`：`INACTIVE_RULE_TYPES` 去掉 `PROCESS-NAME`（5 个）；文件头的说明加一句（`PROCESS-NAME` 按会话来自的进程匹配，阶段 3 M2）。
4. `crates/rurge/tests/cli.rs`：`rurge_command()` 与 `rurge_process()` 加 `.env("RURGE_PROCESS_BACKEND", "off")`，文档注释写：查真实进程的用例在构造之后 `env_remove("RURGE_PROCESS_BACKEND")`。

**接口**

- 消费：任务 2 ～ 5 的 `Finder`、`Query`、`Owner`、`Protocol`；任务 6 的 `OwnerLookup`、`OwnerQuery`、`EngineShared.owners` 与 API 的 `pid`、`processPath`。

**用例**

- `runtime.rs` 的单元用例：`the_process_backend_values`——没设、`off`、`OFF`（错误：只认 `off`）、`bogus`；错误文本一字不差。
- `--test cli`：
  - `check_does_not_say_a_process_rule_never_matches`：`PROCESS-NAME,curl,DIRECT`、`PROCESS-NAME,C:\Program Files\App\app.exe,DIRECT`、`DEVICE-NAME,Kids-iPad,DIRECT`、`FINAL,DIRECT` → 输出里没有 `PROCESS-NAME`，有 `` `DEVICE-NAME` rules never match ``，`0 error(s), 1 warning(s)`。
  - `an_unknown_process_backend_is_refused`：`RURGE_PROCESS_BACKEND=bogus` 的 `rurge run` 以 2 退出，标准错误里是那一句、没有 `listening on`。
  - `the_process_backend_off_looks_nothing_up`：缺省的 `off`、`PROCESS-NAME,<测试二进制的文件名>,REJECT` 加 `FINAL,DIRECT` → 请求不被拒，`GET /v1/requests/recent` 里那条的 `pid` 与 `processPath` 是 `null`。
  - `a_process_rule_matches_the_test_process`（`env_remove("RURGE_PROCESS_BACKEND")`）：一个守护进程，`[Host]` 把 `a.test`、`b.test`、`c.test`、`warm.test` 钉到 `127.0.0.1`；规则 `AND,((PROCESS-NAME,<文件名>),(DOMAIN,a.test)),REJECT`、`AND,((PROCESS-NAME,<完整路径>),(DOMAIN,b.test)),REJECT`、`AND,((PROCESS-NAME,<所在目录加分隔符>),(DOMAIN,c.test)),REJECT`、`FINAL,DIRECT`；先经 HTTP 代理向 `warm.test`（回环上的 `TestServer`）发一个请求暖身，再向另三个各发一个 → 三条记录各是自己那条规则、被拒，`pid` 是 `std::process::id()`，`processPath` 与测试进程的路径相等（计划期决定 P2）；有一条没命中时把那条记录（含 `notes`）打进失败信息。

**判据**：用例绿；门禁绿；Linux 与 macOS 上的端到端第一次 CI 运行时核对（延后事项）。

## Task 8：文档

- 兼容性清单（按内容找行）：`PROCESS-NAME` 一行（三个平台的查法与边界、Windows 的路径写法与不分大小写、不去掉 `.exe`、什么时候查、200 毫秒与 `notes` 那一句、看不见的来源、记录里怎样出现、写法的分类是读法的变化）；内部集 `SYSTEM` 一行（M2-D17 与 `rule match --platform` 不改变它）；`GET /v1/requests/recent` 与 `active` 一行（`pid`、`processPath`）；`suspend` 一行补"掉线不算离开"（M1 停放的第三件）；附录的统计按清单的规矩重数。
- `docs/api/phase3.md`：M2 一节——记录的 `pid` 与 `processPath`、什么时候是 `null`、DEBUG 与一次性的 WARN、`RURGE_PROCESS_BACKEND`；订正"DNS 服务器的取值任何级别的日志里都没有"（DEBUG 里并发查询的上游名带地址，如 `udp://<地址>:53`；M1 停放的第一件）；补写 `--system-proxy` 遇上慢的首读、又落在 `suspend` 的网络上时系统代理翻转一次（M1 停放的第二件）。
- `docs/acceptance/phase3-manual.md`：M2 一节，照设计第 7 节，三个平台各几项加"与平台无关"，每项写操作、期望与怎么看（API 的记录、DEBUG 行）。
- CLAUDE.md：状态一段（M2 做了什么、没做的与不要当成已经有的）；文档列表加本设计与本计划；`unsafe_code` 一段（七个函数与钉住名单的用例）；常用命令加 M2 的过滤词；`RURGE_PROCESS_BACKEND` 一句（同 `RURGE_NETWORK_BACKEND` 的写法）与测试约定的一句；订正 M1 那一句"本机地址与 DNS 服务器的取值任何级别的日志里都没有"。
- README 两份：阶段 3 的进度（两份一致）。
- 本计划末尾的两张表填齐（M1 停放的第四件已在延后事项第 3 行）。

**判据**：`git grep` 核对文档里引用的用例名、路径与常数都存在；门禁绿。

---

## 计划期决定

| 编号 | 事项 | 决定 |
| ---- | ---- | ---- |
| P1 | 查找结束时那一行 DEBUG 带什么（设计第 6.3 节写"带会话号"） | 带来源地址与监听端口，不带会话号：查找发起时会话号还没分配（`look_up_owner` 在 `new_handle_with` 之前），UDP 关联的一次查找服务它的全部流；会话自己的日志行也带 `src`，两行按地址对得上。规则等到期的那一行 DEBUG 照设计带会话号 |
| P2 | 端到端用例怎样比路径（设计 V10 写"两边各 `canonicalize`"） | Windows 上 `current_exe()` 原样写进规则、与 `processPath` 按 ASCII 不分大小写比：`canonicalize` 加的 `\\?\` 前缀在 rurge 的读法里是 UNC，写进规则就不再是那条路径；Unix 上两边用 `canonicalize(current_exe())`、逐字节比 |
| P3 | `windows-sys` 0.61.2（V1） | `GetExtendedTcpTable(*mut c_void, *mut u32, BOOL, u32, TCP_TABLE_CLASS, u32) -> u32` 与 `GetExtendedUdpTable`（同形）在 `Win32_NetworkManagement_IpHelper`；`TCP_TABLE_OWNER_PID_CONNECTIONS` = 4、`UDP_TABLE_OWNER_PID` = 1；`OpenProcess`、`QueryFullProcessImageNameW`、`PROCESS_QUERY_LIMITED_INFORMATION`（0x1000）、`PROCESS_NAME_WIN32`（0）在已开的 `Win32_System_Threading`；`ERROR_INSUFFICIENT_BUFFER`（122）与 `HANDLE` 在 `Win32_Foundation`，显式开上；`AF_INET` / `AF_INET6` 写本地常数，不为两个数开 `Win32_Networking_WinSock`。四种行与表头的布局对过 0.61.2 的结构体（24 / 56 / 12 / 28 字节，行从偏移 4 开始） |
| P4 | 进程槽与 `SessionHandle`（V2） | `SessionHandle::new` 的调用点（全在用例里：引擎的 `dns_pipeline`、`observe`、`relay`、`smart`、`udp` 的单元用例与 `tests/pipeline.rs`，`rurge-inbound` 的 `testing`，基准 `relay.rs`）与 `new_with_token`（引擎的 `new_handle`，和 `relay.rs` 的一条用例）签名不变、得到"不知道"的槽；新的 `new_with_slot` 只由 `new_handle_with` 调用。槽用 `OnceLock` 加 `tokio::sync::Notify`（不用 `watch`：只落定一次，读的一方多是同步地看）；`pending_at` 公开，是为了"窗口已过"的用例不靠等 |
| P5 | `Verdict` 的穷举匹配（V3） | 加变体会编不过的只有 `engine.rs` 的 `verdict_name` 与 `matcher.rs` 的 `Not`；`And`、`Or` 与 `set.rs` 的逐条求值的行是显式的 `match`，要加一支；`benches/rules.rs`、`tests/memory.rs` 与 `rurge-dns/src/hosts.rs` 只做相等比较，不变（`[Host]` 键里的集合遇到 `PROCESS-NAME` 行照旧不命中）；`registry.rs` 没有 |
| P6 | 请求记录（V4） | `RequestRecord` 只派生 `Clone, Debug`，在 `observe.rs` 的 `record_of` 里建（会话结束时进环、活动会话读时建）；带着 `ProcessSlot`（`Arc`，`Clone` 便宜），不需要 `PartialEq`；`rurge-api` 的两处字面量（`requests.rs` 的用例）要补字段 |
| P7 | macOS 的偏移（V5） | `xinpcb_n` 与 `xsocket_n` 在 XNU 的 `#pragma pack(4)` 里：外部端口 16、本地端口 18、`inp_vflag` 44、外部地址 48（IPv4 在 60）、本地地址 64（IPv4 在 76）、`so_last_pid` 68；`xinpgen` 24 字节；按项走、取整到 8（`ROUNDUP64`）。与 mihomo、sing-box 的偏移一致；只取事实 |
| P8 | Linux（V6） | 列位置照内核 `get_tcp4_sock` 一类函数的打印（第 2、3、4、8、10 列：本地、远端、状态、uid、inode）；`/proc/<pid>/exe` 的目标在文件被删后带 ` (deleted)`；`comm` 至多 15 个字节（`TASK_COMM_LEN` 16 含结尾的 0） |
| P9 | `if-addrs`（V7） | 0.15.0 的 `get_if_addrs()` 含回环；在 Windows 上一次的耗时没量过，由任务 3 的基准顺带量 |
| P10 | 明文 HTTP 的绝对 URI（V8） | 一个请求一个会话（`forward` 每个请求 `dial` 一次），每个请求各查一次；同一条客户端连接上 2 秒内的请求命中同一份 TCP 快照，不重读 |
| P11 | 计时（V9） | 碰到查找的用例不用暂停的时钟；"窗口已过"的用例用 `timeout(Duration::ZERO, …)`——tokio 的 `timeout` 先轮询里面的 future，第一次轮询就答的才是 `Ok`——，不靠时间 |
| P12 | `RURGE_PROCESS_BACKEND`（V11） | `run.rs` 在 `network::backend` 那一段之后以同样的写法读、同样以 2 退出；只认恰好的 `off`；不是 clap 的选项，所以不进 `--help`；`rurge rule match`、`dns lookup`、`check` 不读它（它们不查进程） |
| P13 | `notes` 与日志（V12） | `notes` 经 `add_note`；到期那一句写在子规则那一句之前；`LogCapture` 装在当前线程上，用例跑在当前线程的运行时里，会话任务与查找任务（`tokio::spawn`）的行都抓得到（DEBUG 在 `spawn_blocking` 返回之后、回到异步任务里才写） |
| P14 | `rule match --platform` | 不改变比较的读法：大小写与路径的归一化按 rurge 运行的系统（同 M2-D17 的 `SYSTEM` 集） |
| P15 | 查询的协议 | M2 里引擎只发 TCP 的查询（SOCKS5 的 UDP 关联也按控制连接查）；`OwnerQuery.transport` 留着给 M3 的虚拟网卡 |
| P16 | `describe` 不出来的进程 | 这次查询是 `None`（设计第 3.3 节"打不开是不知道"）：没有"只有 PID"的结果；只有名字的（`comm`、`System`）是 `Owner { path: None }` |

## 执行期修正记录

第 1 ～ 16 行与第 25 行是主持这一轮的会话在执行中代项目所有者作的裁定（所有者可以改），每行写着改掉了计划或设计的什么，说得出代价的写了代价；第 17 ～ 23 行与第 26、27 行是实现者做的、与计划或设计的文字不同的地方；第 24 行是基准的数字；第 28 ～ 33 行是整分支终审与它之后的修正波，每行一件，其中第 28、29、31 行的做法是代项目所有者作的裁定（所有者可以改）。

| # | 任务 | 修正 | 提交 |
| - | ---- | ---- | ---- |
| 1 | 3、5 | 钉住 unsafe 名单的用例（`crates/rurge-platform/tests/unsafe_functions.rs`）在 `fn` 前面接受任何可见性前缀（`pub`、`pub(crate)`、`pub(super)`、`pub(in …)`），只认普通的 `fn`：已有的 `job_for` 写作 `pub(super) fn`，照计划写的“可带 `pub`、`pub(crate)`”，新用例在现有的代码上就是红的；`#[allow(unsafe_code)]` 后面是 `unsafe fn`、`const fn` 或 `extern fn` 的，当作“后面不是函数”而拒绝（设计 3.6 的“紧跟它的 fn”）。代价：以后写成 `unsafe fn` 的例外让名单用例变红——这正是要它做的 | `4f5294d` |
| 2 | 3、4、5 | 列出本机地址只有一处：`procinfo` 的 `interface_addresses()`（`if_addrs::get_if_addrs()`；出错时是空表，并记一行不带地址的 DEBUG），三个平台的读取都调它；计划在任务 4、5 各写一次“同任务 3”，照写是三份同样的逻辑 | `4f5294d`、`48d2dff`、`321fa58` |
| 3 | 2 | 一次查询至多重读一次表，重读之后由新快照回答，不论它的时刻（找到是那个进程，没找到是“不知道”）；新旧的规则只决定已有的快照能不能不读就回答。实际使用里 `since` 不晚于现在，新读的快照总满足它们，只有 `since` 在将来的用例走到这一支。代价：只改变一种用例里才有的语义 | `b880a90` |
| 4 | 2 | “TCP 与 UDP 互不等”用闸证明（假的数据源把一次 TCP 的读取扣住，同时一次 UDP 的查询必须完成，两个方向都试），不用“至多多少毫秒”的计时断言：后者在全量门禁的负载下会偶发失败 | `b880a90` |
| 5 | 2 | “第一个对得上的行”读作“第一个有可用主人的对得上的行”：TCP 里 PID 0 的行不算行；读不出进程的行算“找到了、没有主人”，不遮住后面有主人的行，也让这份快照算作“找到”（2 秒的宽限照样适用）；UDP 里第一种有行的地址（确切的地址、`0.0.0.0`、`::`）决定，它的行都没有主人时答“不知道”，不再看更宽的地址（同一端口上内核的或看不见的确切套接字不能算到通配套接字的主人头上）。代价：两行共用一个本地端点而第一行没有主人时，会话得到后一行的进程，而不是“不知道” | `b880a90` |
| 6 | 3、5 | 设计的“至多 4 次”读作先问一次要多大（不给缓冲）、再至多读 4 次，合计至多 5 次系统调用；macOS 的 `sysctl` 循环同样。代价：病态的循环里多一次系统调用 | `4f5294d`、`321fa58` |
| 7 | 3 | Windows 上一种协议的两张表（IPv4、IPv6）任何一张读不了，这一次这种协议的读取就整个失败（设计 3.3 的“别的返回值是这次读取失败”；`Source::read` 没有交出一半结果的路子），错误里写是哪一种地址族。代价：只有 IPv6 表读不了的机器上，这种协议的会话一个也认不出，进程的一生里 WARN 一次 | `4f5294d` |
| 8 | 3 | 解析器去掉 PID 0 的行（计划任务 3 与设计 3.1）。`Finder` 本来会把仍在表里的 PID 0 的 UDP 行当作“找到了、没有主人”；两层现在都跳过它，差别只在一种地址的行全是 PID 0 时会不会退到通配地址——PID 0 实际上不持有套接字 | `4f5294d` |
| 9 | 4 | `comm` 不是 UTF-8、`exe` 链接又读不到的进程是“不知道”（计划写的是 `read_to_string`）。代价：这样的进程没有名字 | `48d2dff` |
| 10 | 5 | 完整的记录都既没有 IPv4 也没有 IPv6 标志的表读作空表，不是“布局对不上”：两个标志都没有的记录本来就不是 rurge 能对上的套接字。代价：一次把每条记录的两个标志都清掉的布局变化读成一张空表、不告警（与 Linux 一侧“整张表一行也解析不出”同类，见延后事项） | `321fa58` |
| 11 | 5 | 双栈的 pcb（`inp_vflag` 为 0x3）读它的 IPv4 视图（计划任务 5）：连到 IPv4 映射地址的 v6 套接字带 `INP_IPV4`，地址在 in4 那一半，从 `127.0.0.1` 来的会话要的正是它；M2 只发 TCP 的查询（计划期决定 P15）。代价：M3 里来自 IPv6 地址、落在双栈套接字上的 UDP 查询可能对不上，到时再看 | `321fa58` |
| 12 | 6 | 假的进程查找的闸在用例自己持有的那一头被丢掉时放开，不是在假查找被丢掉时：引擎持有假查找，tokio 的运行时关闭时等阻塞任务结束，闸不放开的用例会挂住 | `5769bee` |
| 13 | 6 | 一阵并发会话的用例断言同时在查的恰好是 2 个（`LOOKUPS`）：等式才看得出两次查找真的同时进行（只放一个许可的改动通过“至多 2 个”）；要它失败，16 轮里每一对 30 毫秒的阻塞调用都得错开，调度的延迟做不到。代价：极重的负载下偶尔一红，那时换成两方的栅栏 | `5769bee` |
| 14 | 6 | 两条“不等”的用例（`a_session_that_needs_no_process_does_not_wait`、`a_flow_opened_after_the_window_decides_without_waiting`）不加时间的上界：上界断言在全量门禁的负载下会偶发失败（第 4 行同样的理由）；按每条流起算窗口的错误由单元用例 `a_window_that_has_passed_is_not_waited_for` 钉住（`SlotProcess` 只看得到槽的发起时刻），先等了槽的错误会让会话多出 `notes` 那一句。代价：一个“等 200 毫秒再丢掉答案”的回归看不出来 | `5769bee` |
| 15 | 6 | 查找在 `Dialer::dial` 里、暂停的检查之前发起（计划任务 6 写的位置）：暂停时被拒的会话也各查一次，它的记录照样说出进程；`connect_session` 里“在规则之前拒绝”的注释改成说明这一点 | `5769bee`、`038aaee` |
| 16 | 1、8 | 任务 1 之后 `rurge_rules::matcher::normalize_process_path` 没有调用者了（新的比较经 `normalize_process_path_for`），只有阶段 1 验收文档第 198 行引用它：删掉，那一行指到现在做归一化的函数 | `f6327f0` |
| 17 | 2 | 新旧的规则写成 `at + TCP_GRACE >= since`（`Instant::checked_add`，溢出当作“新的”），与计划的 `since - TCP_GRACE` 同义，在时钟的原点附近没有下溢的分支 | `b880a90` |
| 18 | 2 | 每份快照除了“PID → 进程的描述”再备忘“行的主人 → PID”：Linux 那样按 inode 找进程的 `Source::pid` 每份快照每行只做一次 | `b880a90` |
| 19 | 3 | 读表的循环 `read_table` 是安全的函数，用例里拿假的系统调用测（变大的表；一直变大的表在第四次读取后放弃）；名单扫描器有五条自测；读不了的地址族写进读取错误 | `4f5294d` |
| 20 | 5 | 行按 IPv4 在前稳定排序（与另两个平台的次序一致）；写出路径是纯函数 `path_written`；放不进 `i32` 的 PID 直接不问系统 | `321fa58` |
| 21 | 6 | 经引擎的用例的目标是回显服务器（`rurge_proto::testing::echo_server`），不是 `TestServer`：用例要“回显成功”；UDP 关联的用例用一条 `IP-CIDR … REJECT` 规则代替计划写的 `FINAL,REJECT`——测试夹具在末尾追加 `FINAL,DIRECT`，而策略取最后一条 `FINAL` 的（阶段 1 补遗（M5b）），`FINAL,REJECT` 证明不了什么 | `5769bee` |
| 22 | 6 | `ProcessSlot::settled_by` 在期限到了之后再看一次槽：与期限同时落定的算落定（比“期限先到是 `None`”宽一点，规则多等的界不变） | `5769bee` |
| 23 | 6 | 失败的假查找用 `std::panic::resume_unwind` 而不是 `panic!`：缺省的 panic 钩子在设了 `RUST_BACKTRACE` 时于阻塞线程上解析回溯，超过 200 毫秒的窗口，用例把“超时”误读成“许可没还”；用例里的 PID 取 `4_000_321`（端口放不下），查 INFO 行时 PID、名字与路径都查；记录的 JSON 与槽的 `Debug` 只读一次槽 | `038aaee` |
| 24 | 3 | 基准（Windows、debug 构建、跑一次，`#[ignore]` 的 `a_lookup_with_many_connections_open`）：开着 500 条回环连接时一次新的查找 3.39 毫秒（两张 TCP 表 2.49 毫秒，一条路径 59 微秒）；`interface_addresses()` 第一次 3.7 毫秒、第二次 1.5 毫秒，6 个地址 | `4f5294d` |
| 25 | 7 | 端到端用例 `a_process_rule_matches_the_test_process` 对每个目标至多重发 3 次，只在那个目标的记录因 200 毫秒里没知道进程而没命中（`notes` 里有 `rurge_engine::owner::NOT_IN_TIME` 那一句）时重发，别的不符当场失败、带出整条记录：全量门禁下机器忙时一次查找可能超过 200 毫秒（任务 6 的一条用例在设了 `RUST_BACKTRACE` 时就因 panic 钩子的耗时红过）。代价：一个让查找处处变慢的回归，在快的机器上要到第四次才红（一次性的 WARN 与 `notes` 照样说出来） | `baa3ba1` |
| 26 | 7 | 计划之外多一条单元用例 `the_owner_lookup_asks_as_it_was_asked_and_answers_with_the_pid`（`crates/rurge/src/cli/runtime.rs`），钉住引擎的问题到平台的问题、平台的回答到引擎的回答这两次翻译：M2 里引擎只发 TCP 的查询（P15），端到端的用例看不出 TCP 与 UDP 的对调 | `baa3ba1` |
| 27 | 7 | `rurge_command()` 与 `rurge_process()` 设 `RURGE_PROCESS_BACKEND=off`，只有端到端用例在构造函数之后去掉它；`external` 策略拉起的嵌套 `rurge run` 拿不到以 `RURGE_` 开头的变量，它的会话照真的查（只读系统的表；它的 Profile 里没有 `PROCESS-NAME` 规则，没有谁等它；设计 8.1 就是这样）；`an_unknown_process_backend_is_refused` 连标准输出一起读：`listening on` 写在标准输出上，只读标准错误的检查什么也证明不了 | `baa3ba1` |
| 28 | 5 | 整分支终审之后，macOS 的 `walk` 只把直接跟在 `XSO_INPCB` 项后面的 `XSO_SOCKET` 项配给它，其间别的种类的项让还开着的 pcb 项没有套接字项（`_ => open = None`）；计划任务 5 写的是"`XSO_SOCKET` 属于当前记录……其余种类跳过"，那样一个套接字项配给最近开始的 pcb 项，不管中间隔着什么。理由：某个版本若把套接字项挪到 pcb 项前面，每个 pcb 会配上下一条记录的套接字，每条记录看上去都完整，不报 `Layout`，也不 WARN，每个会话认成别的进程而没有任何东西说出来；XNU 的布局（pcb、套接字、两个缓冲项、统计，TCP 再加 tcpcb）下读出的行不变；sing-box 与 mihomo 也是在紧跟 pcb 项的固定位置读套接字项（只取事实）。代价：某个 macOS 版本若在两项之间放了别的项，每次查找都是"不知道"，`Layout` 的 WARN 说一次。用例：`a_socket_item_belongs_to_the_pcb_item_just_before_it`（原名 `a_socket_item_belongs_to_the_record_the_latest_pcb_item_began`）里"其间有别的项"一格不再给这条 pcb 项出行，`no_complete_record_among_the_items_is_a_layout_that_is_not_known` 加一格重排的布局（先套接字项、再 pcb 项、再缓冲项），`kinds_that_are_not_known_are_skipped` 把原来放在 pcb 项与套接字项之间的缓冲项挪到套接字项之后（XNU 就写在那里）；前两处在旧的 `walk` 上是红的（旧的把重排的布局读成 `Ok`、第一个 pcb 配上第二条记录的套接字），第三处只是跟着新读法挪了位置 | `d6cc8f6` |
| 29 | 3 | 整分支终审之后，名单扫描器（`crates/rurge-platform/tests/unsafe_functions.rs`）的规则改成：每一行在 `//` 之前的部分提到 `unsafe_code` 的，去掉首尾空白后必须恰好是 `#[allow(unsafe_code)]`，那一行给紧跟它的 `fn` 记名，别的一律报错（带行号，调用方加文件名）。原来只看以 `#[` 开头又含 `unsafe_code` 的行，rustfmt 拆成多行的属性（`#[allow(` / `unsafe_code,` / `dead_code,` / `)]`）整个逃掉：那个函数不在名单上，用例也不红，CLAUDE.md 里"多一个、少一个都让它变红"的保证不成立。新增一条自测 `a_line_that_names_the_lint_and_is_not_the_attribute_is_refused`；原来"没有提到这个 lint"的一条把字符串里的提及当作无害，新规则下这样的行被拒，那一条改成只含注释里的提及；共六条自测。在真实的树上给一个私有函数临时写多行属性：旧扫描器绿，新扫描器红，文件还原后工作区里没有它。代价：`rurge-platform` 的 `src/` 里块注释或字符串提到 `unsafe_code` 是假红，要提就写在 `//` 注释里 | `d6cc8f6` |
| 30 | 终审 | 整分支终审（只读代码与文档，什么也没运行）的结论是改了再合并：没有 Critical；三条 Important 都是 CLAUDE.md 里后来的会话会照着办的不实之辞——macOS 的两个 unsafe 函数与 Linux、macOS 的真读用例被说成"也只在 Windows 开发机上编译、运行过"（其实从没有编译过，只有 Windows 的真读用例与端到端用例在开发机上编译、运行过），Windows 读表的循环被说成"在每个宿主上编译、都测"（它和它的用例都在 `#[cfg(windows)]` 里），名单用例"多一个、少一个、标在 `fn` 之外的东西上都让它变红"的保证在扫描器漏掉被拆成多行的属性时不成立（扫描器同一波里改了，第 29 行）。三条与几处小项在合并之前改了：代码里是扫描器（第 29 行）、macOS 的 `walk`（第 28 行）、`an_ipv4_query_finds_the_mapped_row` 里两行换成两个 PID、macOS 的 `a_process_that_does_not_exist_is_not_described` 多问 `i32::MAX as u32`、`the_process_backend_off_looks_nothing_up` 多断言 `notes` 为空、三处注释（`parse_tcp6` 丢掉作用域号的理由、UDP 流水线里暂停时拒绝的那一句、两处不再数"第几个 unsafe"）；文档里是 CLAUDE.md 的三处与 API 参考、手工验收清单里同一句不实的话，兼容性清单的三行（macOS 的 `proc_pidpath` 没有观察过、逻辑规则里永不匹配的例子换成 `DEVICE-NAME`、`rurge run` 一行补 `RURGE_PROCESS_BACKEND`），阶段 1 的 API 参考里 `notes`、被拒会话的记录与新加的两个键，和 API 参考、手工验收清单里这一轮加的引号。终审说不必现在改的次要项多数在延后事项里（第 7 ～ 16 行，和合并之前补进表的第 23 行），终审另加的接在后面（第 17 ～ 22 行）；两条不进表：测试辅助函数重复 `ParseCtx` 的字面量（纯属整理），`RURGE_PROCESS_BACKEND=off` 的启动不说一句话（只供测试的变量）。分两个提交：代码是 `d6cc8f6`，文档与这张表是 `0f98d62`（复审之后的订正见第 33 行） | `d6cc8f6`、`0f98d62` |
| 31 | 终审 | 两条次要项不改：阶段 1 验收文档（`docs/acceptance/phase1-acceptance.md`）里说守护进程的会话没有进程的几句，记的是读 `a4d665c` 时的样子，M1 对它那一行 `SUBNET` 也是这样处理的，兼容性清单与 CLAUDE.md 说的是现在的样子；兼容性清单开头的"第二个里程碑 M2……也已合并"在紧接着的快进合并之后即为真，措辞同 M1。代价：读阶段 1 审计文档的人可能把"会话里没有进程"当成今天的状态；在合并之前读到清单开头那句的人早一步看到"已合并" | — |
| 32 | 终审 | 修正波的最后一个代码提交 `d6cc8f6` 上的全量运行：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 干净，`cargo test --workspace --no-fail-fast` 77 个结果行、3797 个通过、0 个失败、10 个忽略（比 `f6327f0` 上的 3796 个通过多的一个是扫描器的新自测），没有哪个测试二进制以 `STATUS_HEAP_CORRUPTION` 或 `STATUS_ACCESS_VIOLATION` 退出——一台 Windows 11 x86-64 的开发机上的一次运行，不是 CI。本轮改的 macOS 的 `a_process_that_does_not_exist_is_not_described` 只在 macOS 上编译，第一次 CI 运行时才第一次编译 | `d6cc8f6` |
| 33 | 终审 | 范围内的复审（只读，另跑了两条聚焦的用例）认定修正波把要改的每一项都改了、没有带出新的 Critical 或 Important；合并之前订正了它留下的两条文档上的次要项和主持这一轮的会话自己看出的一处：延后事项第 14 行的去向直接写明 M3 之前在真实的 Linux 机器上量（原来另写成一行"第 14 行的去向补一句"的第 23 行），第 23 行换成任务 7 留下而当时没有进表的两条次要项，第 30 行不再说不改的次要项都在表里，列举 M2 改了兼容性清单哪几行的三处（CLAUDE.md 的状态一段、清单开头的说明与附录）补上 3.1 节 `AND` / `OR` / `NOT` 与 10.3 节 `rurge run` | 本提交 |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | Linux 与 macOS 的真实读取（`/proc` 的内容与权限、`pcblist_n` 的真实布局）、macOS 的两个 unsafe 函数与三个平台的真读用例和端到端：Linux 与 macOS 的代码在开发机上编译不到，只有纯函数对着夹具测过。任务 7 的端到端用例 `a_process_rule_matches_the_test_process` 与单元用例 `the_process_backend_values` 里只在 Unix 上编译的一段也在其内；端到端在 Linux 或 macOS 上红时先查读取器（Linux 上名字取自 `exe` 链接，macOS 上 `/private/…` 的链接与 P7 的偏移），不是接线 | 第一次 CI 运行时核对 |
| 2 | macOS 上 `so_last_pid` 与 `so_e_pid` 的取舍（经 `nsurlsessiond` 等系统服务代建的连接，记录里是谁） | 手工验收（`docs/acceptance/phase3-manual.md` 的 macOS 一节） |
| 3 | M1 停放的第四件：五条起守护进程的端到端用例依赖第一次读网络在 1 秒的预算里完成 | 第一次 CI 运行时留意 |
| 4 | 查找的代价：任务 3 的基准只在 Windows 上跑；Linux 的 `fd` 扫描在进程多的机器上、几百条并发连接时查找是否在 200 毫秒里结束 | 手工验收 |
| 5 | 看不见或读不了的进程：虚拟机、WSL2、容器的客户端；Windows 上以普通用户运行时别的用户的服务进程；Linux 上不以 root 运行时别的用户的进程 | 手工验收 |
| 6 | 永不返回的平台查找：两个许可被占住，此后的每条会话排一个小任务、不设上限；`rurge run` 退出时 tokio 不设时限地等阻塞线程，进程不退。三个平台的读取都是有界的系统调用（Windows 的表与映像路径、`/proc` 的读、`sysctl` 与 `proc_pidpath`），不知道哪一个会挂住；只有至多 2 个在途的调用会被等，排队的是异步任务、关闭时丢掉 | 有用户报告再说 |
| 7 | Windows 上不分大小写与路径归一化的接线（`crates/rurge-rules/src/matcher.rs` 的 `cfg!(windows)`）没有经 `eval` 的用例钉住，只钉住了带平台参数的纯函数（`normalize_process_path_for`、`process_matches_for`）；UNC 与用 `/` 写的前缀没有在规则的用例里比较过 | 下次动到时 |
| 8 | 一条规则在一次求值里被评估两三次时，它的“不支持”提示进 `notes` 几次（`NeedsResolve` 早就如此，`NeedsProcess` 多一个触发） | 下次动到时 |
| 9 | `Finder::owner` 在新旧的规则排除旧快照之前先扫它（Linux 上可能做一次按 inode 的扫描）；只是性能 | 下次动到时 |
| 10 | 地址表的刷新不是单飞的，`listed_at` 记在列完之后；受引擎的 2 个许可与 2 秒的有效期所限 | 有用户报告再说 |
| 11 | 地址锁的纪律（问系统时不持锁）与锁中毒之后的恢复没有用例 | 下次动到时 |
| 12 | Windows 读表的缓冲按系统说的大小分配（至多 4 GiB，`as u32` 在那里截断），放弃之前多分配一块用不上的缓冲；真实的表到不了 | 下次动到时 |
| 13 | Windows 上“IPv4 在前”只凭写法、没有用例钉住；真读用例按 ASCII 不分大小写比 `current_exe()`，经目录联接或 8.3 短名启动时可能不同 | 有用例失败时再看 |
| 14 | Linux 上一次谁也认不出的未命中要在协议锁下扫一遍 `/proc`，没有“刚扫过、没有”的缓存（第 4 行的查找代价的一部分） | M3 之前在真实的 Linux 机器上量一次（手工验收，与第 4 行一起） |
| 15 | Linux 的读取：IPv6 的行进修剪集合没有用例；`RealFs::link` 只在 Linux 的用例里对真实的符号链接测过；`RealFs::list` 丢掉单个条目的错误而没有注释说是有意的；`parse_proc_net` 的签名并排三个布尔参数（计划写定的） | 下次动到时 |
| 16 | 三个平台的真读辅助函数是三份相近的拷贝（计划写的“同任务 4”）；macOS 的偏移是裸数字而读的长度有名字 | 下次动到时 |
| 17 | TCP 的查询只比较远端端口（设计 3.1 的 `Query.remote_port`）：本地端点相同、远端端口也相同、只有远端 IP 不同的两行都是候选，第一条有主人的行胜出。触发的情形：同一个本地 IP:端口同时连着另一台主机上与 rurge 监听端口同号的端口——Linux 会把一个本地端口同时用于不同的目的地，而 6152 是 Surge 的缺省端口。会话其实知道接受这条连接的本地地址（`crates/rurge-inbound/src/http.rs` 的 `ctx.local`，SOCKS5 同样），查询可以带上远端地址再比较 | 做掉（阶段 3 / M3b 任务 1，`af34beb`） |
| 18 | Linux：数据行全都解析不出的表读作空表、不写日志（任务 4 的评审留下的），布局上的一次意外会让每次查找悄悄是"不知道"。改成 `Layout` 加一次 WARN，与 macOS 一致，并改 `procinfo/mod.rs` 里 `ReadError::Layout` 的注释 | 做掉（阶段 3 / M3b 任务 1，`af34beb`） |
| 19 | macOS 记录的双栈读法（记录第 11 行）：来自 IPv6 地址、落在双栈套接字上的 UDP 查询可能对不上，UDP 的查询出现时再看一遍 | 做掉（阶段 3 / M3b 任务 1，`af34beb`） |
| 20 | 被拒与失败的会话的 INFO 行带规则的文字（自阶段 1 起）：命中 `PROCESS-NAME,x,REJECT` 的会话，INFO 行里写着这条规则，读日志的人由此推得出进程。要不要在 INFO 里隐去 `PROCESS-NAME` 规则的文字 | 项目所有者 |
| 21 | 设计接受的低概率认错：客户端的端口在 `TCP_GRACE` 之内被别的进程重用；表读与路径读之间 PID 被重用；Linux 上一个套接字的 inode 交到别的进程手里 | 有用户报告再说 |
| 22 | 写成另一个平台的路径形式的规则（Linux 或 macOS 上的 `C:\…`，Windows 上的 `/Applications/…`）永不匹配，而 `rurge check` 不再说什么（M2-D11、M2-D16：同一份 Profile 在各平台读法相同） | 有用户报告再说 |
| 23 | 任务 7 留下的两条次要项（当时没有进这张表，合并之前补上）：启动时拒绝的那一支（打印一行、`recover_without_a_log`、以 2 退出）在 `crates/rurge/src/cli/run.rs` 里出现了三次（计划要它照同样的写法），第四次出现时抽成一个函数；没有用例钉住 `check`、`rule match`、`dns lookup` 不读 `RURGE_PROCESS_BACKEND`、它不在 `--help` 里，也没有用例钉住它的取值读不懂时先恢复系统代理再退出（`an_unknown_process_backend_is_refused` 只看退出码、那一句话与没有监听） | 下次动到时 |

第 6 ～ 16 行与第 23 行是任务评审与实现者留下的次要项（第 23 行是合并之前补进表的），第 17 ～ 22 行是整分支终审另加的；终审要改的次要项已在合并之前改了，见执行期修正记录第 28 ～ 33 行。
