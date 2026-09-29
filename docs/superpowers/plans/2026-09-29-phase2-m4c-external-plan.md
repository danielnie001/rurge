# 阶段 2 / M4c「external 出站」Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 实现 `external` 出站（TCP）：`external` 策略行的读取与校验（重复的 `local-port` 是错误、`args` 脱敏、订阅导入的 `external` 一律跳过）；rurge 第一次用到这个策略时拉起外部程序，经它在 `127.0.0.1:<local-port>` 上的 SOCKS5 转发；程序退出后下次用到时再拉起（两次拉起至少隔 2 秒）；输出写进数据目录并轮转；停止时连同它启动的进程一起结束（Unix 进程组、Windows Job Object）；rurge 正常退出时最后停掉全部外部程序；能力表翻转 `external`。M4 至此完成。

**Architecture:** `rurge-config` 新增 `spec::external`（`ExternalSpec`：`args` 是 `Secret`）与 `ParamReader::all`；`rurge-platform` 新增 `process`（`prepare` 与 `ProcessTree`：Unix 上程序领一个新进程组、`killpg`；Windows 上程序进一个"关闭即结束全部"的 Job Object——工作区第二个 `#[allow(unsafe_code)]`）。`rurge-proto` 新增 `external`（`ExternalOutbound`；注入的 `ProcessHook` / `ProcessGroup` 两个 trait 与不分组的 `NoProcessGroups`；每个拉起的程序由一个任务看守，程序退出、被要求停止或任务被丢弃时连同它启动的进程一起结束），SOCKS5 握手抽成 `socks5::negotiate` 供它复用。`rurge-engine` 的工厂多一个分支，`EngineShared` 带上进程控制与"已构建的 external 出站"名单，`Engine::stop_external_programs` 给退出流程用；bin 注入 `PlatformProcesses` 并在退出流程最后调用它。只用于测试的新工作区成员 `tests/external` 带一个极小的 SOCKS5 辅助程序，真实拉起它的用例都在这里。

**Tech Stack:** Rust 1.89 / edition 2024；不新增第三方 crate（`Cargo.lock` 只多几条依赖边与一个工作区成员）：`nix` 0.31.3 早已因 boringtun、russh 进了依赖树，只在 Unix 上给 `rurge-platform` 开 `signal` 特性；`windows-sys` 0.61 多开 `Win32_Security`、`Win32_System_JobObjects`、`Win32_System_Threading` 三个特性；`tokio` 在 `rurge-proto` 里多开 `process` 特性。

**Spec:** `docs/superpowers/specs/2026-09-27-phase2-m4-wireguard-ssh-external-design.md`（第 2 节 M4-D1、D6、D8、D11、D12；第 4.4、4.6 ～ 4.8 节中 `external` 的部分；第 7 节；第 8.2 ～ 8.4 节；第 9 ～ 12 节中 external 的部分；第 13 节 `CLAUDE.md` 一行；第 15 节 V9 ～ V11、V14；第 16 节 M4c 草图）；M3a 计划 `docs/superpowers/plans/2026-09-23-phase2-m3a-subscriptions-plan.md` 末尾「延后事项」#25。与本计划「计划期决定」表不一致处，以该表为准；写计划时一并把这些写进设计文档新增的第 21 节。

## Global Constraints

- MSRV **1.89**，edition 2024。`unsafe_code`：全工作区 `forbid`；只有 `rurge-platform` 是 `deny`。**本计划只新增一处 unsafe**：`rurge-platform::process` 里的 `job_for`（Windows，M4-D6），整个函数标 `#[allow(unsafe_code)]`，每个 FFI 调用各自一个 `unsafe` 块并写 `SAFETY:` 说明；其余 crate（含新成员 `tests/external`）照旧 `forbid`。
- 依赖方向：`rurge-proto → rurge-net → rurge-config`；`rurge-engine → rurge-proto`；`rurge-platform` 不依赖内部 crate，平台代码只在 `rurge-platform`，经 bin 的适配器（`PlatformProcesses`，同 `PlatformSockets`）注入。**不新增第三方 crate**（P2）。
- **测试绝不碰公网**：只用回环 + 端口 0（或刚刚空闲的端口）+ 有界等待；不用固定 sleep 当同步手段（轮询 + 截止时间；只有"断言这段时间里什么也没发生"时才等一段固定时间）。外部程序只拉起测试自带的辅助程序 `socks-helper`（只监听 127.0.0.1）、`rurge` 自己，以及 `rurge-platform` 用例里的 `cmd /c ping -n 30 127.0.0.1`（Windows）/ `sh -c "sleep 30; sleep 30"`（Unix）；每个用例结束时拉起的进程都已结束。**任何带 `url-test` / `fallback` / `load-balance` / `smart` 组的测试配置，`proxy-test-url` 与 `internet-test-url` 都必须指向回环。**
- **测试与任何命令都不得修改本机的系统代理、注册表、网络设置，不得注册真实服务 / 计划任务**：不在 CLI 测试夹具之外运行 `rurge run --system-proxy`；不运行不带 `--dry-run` 的 `rurge service install | uninstall`。
- **不在本机下载或安装任何东西**（不 `rustup target add`、不 `cargo install`，不装 sshd 等工具）。本计划不需要从 crates.io 下载新 crate。
- **凭据永不外泄**：`args` 不进日志、错误文本、API 输出与 `Debug`（`ExternalSpec.args` 是 `Secret`；日志只写策略名、pid 与退出码，从不写 `exec` 与 `args`，设计 7.5）；`args` 在 `profiles/current`、`policies/detail` 与 `lineHash` 里整体为 `***`（M4-D12）。
- rurge 专有的运行时选项只经命令行与环境变量提供，不扩展 Surge 配置格式（FR-CFG-17）。与 Surge 的每一处行为差异都登记进 `docs/surge-compatibility-matrix.md`（Task 5）。
- 日志、错误文本、CLI 输出、代码注释用英文；文档与提交标题用中文。注释密度、命名、惯用法与相邻代码保持一致；注释里不写评审轮次的标签。
- 提交信息结尾两行：`Co-Authored-By: <执行者自己的署名>` 与 `Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th`。**不 push、不 merge**，不 amend。
- 每个任务结束的门禁（全部通过才算完成）：

  ```bash
  RUSTFMT="C:\Users\SZV01065\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe" cargo fmt --all --check \
    && cargo clippy --all-targets -- -D warnings \
    && timeout 1500 cargo test --workspace --no-fail-fast
  ```

  `timeout` 不能省：`rurge-dns` 的一个用例曾让测试进程以 100% CPU 空转数小时（M3a「延后事项」#20）。测试二进制异常退出而没有失败用例时（`STATUS_ACCESS_VIOLATION`、`STATUS_HEAP_CORRUPTION` / `0xc0000374`、段错误——本机已知的既有问题，M3b 计划 P21），或整轮被 `timeout` 杀掉时，重跑一次并保留两次的日志，**不要在任务里去修它**。已知偶发失败的计时类用例（`rurge-dns` 的 `a_partial_result_completes_aaaa_in_the_background` 与 `bootstrap::tests::stale_entries_are_served_and_refreshed_once`、`rurge` 的 `run::watch_reloads_rules_on_change`）同样重跑。
- 第三方 crate 的 API 以本机源码为准：`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`（`windows-sys-0.61.2`、`nix-0.31.3`、`tokio-1.53.1`）。**Unix 分支（`nix`、进程组）在本机（Windows，只装了 `x86_64-pc-windows-msvc` 目标）编译不到**，只在 CI 的 Linux / macOS 上编译与运行（P2）：照抄计划里的代码，不要为了让本机"看得见"而改写它。
- 本机的 bash 处理不了超过约 8 KB 或含反斜杠的 heredoc（`\\` 会被改写）：新文件与含反斜杠的改动一律用写文件的工具落盘，不用 heredoc。

## Review Focus

设计没有逐条写到、而最可能伤到使用者的五类输入或失败方式；每一条都在负责它的任务里配了用例。

1. **程序起得慢**（`ssh -D` 要先连上服务器才开始监听本机端口）：第一个请求要等它，而不是立即失败；Windows 上连一个没人监听的本机端口要约 2 秒才被拒（P5），不能因此把 6 次尝试拖成 15 秒、超过拨号时限。用例：Task 3 `the_first_use_starts_the_program`（辅助程序 300 ms 后才监听）、`a_port_that_never_opens_fails_the_request`（6 次尝试在 2.5 ～ 6 秒内结束）。
2. **程序一启动就崩、或根本启动不了**（路径写错、缺依赖）：不能每个请求都去拉起一次，请求要立即得到说得清的错误。用例：Task 3 `a_program_that_cannot_start_fails_the_request`（第二个请求不再拉起、得到同一个错误）、`a_program_that_exited_is_started_again`（再拉起不早于上一次拉起 2 秒）。
3. **rurge 退出、崩溃或重载后留下孤儿进程**，占着端口让下一次拉起失败：程序和它启动的子进程要一起结束。用例：Task 2 `terminating_the_tree_ends_the_program`、`dropping_the_tree_ends_the_program_on_windows`；Task 3 `stopping_ends_the_whole_tree`、`dropping_the_outbound_stops_the_program`（辅助程序再拉一个子进程）；Task 5 `run_starts_an_external_program_and_stops_it_on_exit`（`rurge stop` 之后外部程序的端口不再应答）。
4. **重载**：只改了无关内容时不能重启程序（会断掉它的会话，`ssh -D` 还要重新登录）；改了策略行时旧程序要在旧策略释放后停掉。用例：Task 4 `a_reload_keeps_an_unchanged_program_and_stops_a_replaced_one`。
5. **`args` 里的口令与订阅里的 `external`**：`args` 不能出现在 API 输出、`rurge check` 的输出与日志里；订阅作者不能让 rurge 启动任何程序；`rurge check` 与构建绝不拉起程序。用例：Task 1 `an_external_line_loses_its_args`、`the_manual_example`（`Debug` 不显示参数）、`a_subscription_never_brings_an_external_policy`；Task 4 `a_check_or_a_build_starts_nothing`；Task 5 `check_knows_external`（输出里没有口令）。

另有几条同样配了用例、但不那么常见的：两个策略写了同一个 `local-port`（Task 4 `two_external_policies_cannot_share_a_local_port`）；策略名里有文件名不能用的字符、或是 Windows 的设备名（Task 3 `a_log_is_named_after_its_policy`）；日志越写越大（Task 3 `a_log_is_rotated_once_it_is_over_the_limit`）；程序按环境变量里的代理设置绕回 rurge（Task 3 `the_program_gets_no_proxy_settings`、`the_first_use_starts_the_program`）。

## 计划期决定

写计划时对照设计、Surge 手册（`policies/external.html`）、windows-sys 0.61.2 / nix 0.31.3 / tokio 1.53.1 源码与本仓库源码核对后定下的事；与设计文档文字不同的，写进设计文档第 21 节。

**本计划里的代码不是凭空写的。** 全部 5 个任务的改动在仓库的一份副本上按任务顺序真实做了一遍（副本用自己的构建目录，不与本仓库的 `target/` 混用）：Task 4 与 Task 5 之后各跑了一次全工作区门禁，最后一次是 **51 个测试二进制，1165 通过 / 0 失败 / 2 忽略**（本计划开工前的 main 是 49 个测试二进制，1135 通过 / 2 忽略）；新的真实进程用例连跑五次都通过，跑完没有残留的 `socks-helper` 进程。计划里新文件的全文取自副本上该任务的提交，修改处的"把 … 换成 …"由脚本从相邻两个任务提交的差异生成，并在拼好之后按计划的顺序套到开工前的源码上逐字核对过——计划文本与验证过的代码一字不差。每个任务 Step 2 的"预期失败"是只把该任务的用例块（及写明的前置改动）套到上一个任务的状态上、真实跑出来的。做的过程中发现的问题（P5、P6、P8 与辅助程序的 `--serve`）已经改在了它们所属的任务里。

| # | 事项 | 决定与依据 |
| - | ---- | ---------- |
| P1 | V9：windows-sys 0.61 的 Job Object | `CreateJobObjectW`（参数类型 `SECURITY_ATTRIBUTES` 要 `Win32_Security` 特性）、`SetInformationJobObject` 设 `JOBOBJECT_EXTENDED_LIMIT_INFORMATION`（这个结构要 `Win32_System_Threading` 特性）的 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`、`OpenProcess(PROCESS_SET_QUOTA \| PROCESS_TERMINATE)` 取程序的句柄、`AssignProcessToJobObject`（`Win32_System_JobObjects`）。按 pid 打开进程，而不是取 tokio `Child` 的原始句柄：两个平台的接口因此都是 `ProcessTree::contain(pid)`；tokio 还没回收子进程时 pid 不会被复用。两个句柄都立即包进 `OwnedHandle`（关闭由它负责）；Job 的句柄一关（`ProcessTree` 被丢弃、或 rurge 进程死掉），系统就结束 Job 里的全部进程。**已知限制**：程序在被放进 Job 之前（拉起之后的几微秒里）启动的子进程不在 Job 里；stable Rust 没有"直接在 Job 里创建进程"的办法（`PROC_THREAD_ATTRIBUTE_JOB_LIST` 要不稳定的 `raw_attribute`）。测试辅助程序在开始监听之后才拉起自己的子进程 |
| P2 | V10：nix 与进程组 | `nix` 0.31.3 已在 `Cargo.lock` 里（boringtun、russh 的依赖）：工作区依赖加 `nix = { version = "0.31", default-features = false }`，`rurge-platform` 只在 `cfg(unix)` 上依赖它并开 `signal`（它带上 `process`）；`killpg(Pid, Signal)`，组已经没了（`ESRCH`）不算错。进程组用 std 的 `CommandExt::process_group(0)`（1.64 起稳定）：`rurge-platform` 只认 `std::process::Command`，`ExternalOutbound` 先配好 std 的命令再转成 tokio 的（`tokio::process::Command::from`）。本机编译不到 Unix 分支：代码保持最少，只在 CI 上验证 |
| P3 | V11：退出路径 | bin 里没有 `std::process::exit`；`rurge run` 的出口只有 `block_on` 里的几个 `return`：绑定监听或 API 失败时还没有拨号，外部程序不会已经拉起；Ctrl-C、SIGTERM、`rurge stop` 与 `POST /v1/stop` 都走主循环的 `break`，之后先恢复系统代理、停 API、等会话结束（或到宽限期），**最后** `engine.stop_external_programs()`，每个程序先被要求结束（Unix：整个组 SIGTERM），2 秒后仍在就强制结束；再按一次 Ctrl-C 的"立即退出"直接返回：运行时结束时看守任务被丢弃，它持有的进程组（`Tree` 的 `Drop`：Unix SIGKILL，Windows 关闭 Job）与程序本身（`kill_on_drop`）随之结束。服务停止：systemd / launchd 发 SIGTERM，同 Ctrl-C；Windows 计划任务结束进程时 Job 句柄随进程关闭，系统结束整棵树 |
| P4 | V14：测试辅助程序放在哪里 | 新的仅测试用工作区成员 `tests/external`（包名 `rurge-external-tests`，`publish = false`），只有一个二进制目标 `socks-helper`（只用 std，没有依赖）：`CARGO_BIN_EXE_socks-helper` 只对本包的集成测试可见，所以真实拉起它的用例——`ExternalOutbound` 的（Task 3）与经引擎的（Task 4）——都放在这个包里（像 `tests/interop` 一样以开发依赖用 `rurge-*`）。`cargo build -p rurge` 与发行的二进制不含它。CLI 测试手里只有 `rurge` 本身，就拿第二个 `rurge run`（它的 `socks5-listen` 就是 SOCKS5 服务端）当外部程序（Task 5） |
| P5 | 连接本机端口的重试（设计 7.1） | 手册：每 500 ms 一次、最多 6 次。**Windows 上连一个没人监听的回环端口要约 2 秒才被拒**（写计划时实测三次都是 2.03 秒：系统先重发 SYN）——照原样重试要约 15 秒，超过默认 10 秒的拨号时限。做法：每次尝试以它自己的 500 ms 为限（`timeout_at`），到时没连上与被拒一样处理，下一次在这 500 ms 结束时开始；全部 6 次最多约 3 秒。本机回环上真正在监听的端口，连接在微秒内完成，不受这个时限影响 |
| P6 | 程序与它启动的进程怎样结束 | 每个拉起的程序由一个任务（`watch`）持有：程序自己退出时，立即结束它的整个组（留下的子进程只会占着下一次拉起要用的端口）；被要求停止时（`stop()` 发来信号，或 `ExternalOutbound` 被丢弃、信号的发送端随之丢弃），先要求整个组结束，2 秒（`STOP_GRACE`）内没结束就强制结束；没有进程组时（`NoProcessGroups`）直接结束程序本身，不等。任务被丢弃（运行时结束）时，`Tree` 的 `Drop` 与 tokio 的 `kill_on_drop` 兜底。所以设计 7.4 的"旧进程在旧出站释放时停掉"不需要额外的代码：出站释放 → 信号发送端丢弃 → 看守任务停掉程序。Unix 上程序本身已被回收之后再给组发信号：组里还有进程时它的编号不会被复用；组已空时 `ESRCH`，被别的新进程组恰好占用同一编号的机会极小，接受（延后事项） |
| P7 | 拉起的间隔（M4-D11） | 间隔从上一次拉起（成功或失败）算起。间隔之内：上一次拉起失败的，请求立即得到同一个错误（不再尝试）；程序是在间隔内自己退出的，不拉起，交给请求自己的重试等过间隔（6 次 × 500 ms 足够覆盖 2 秒） |
| P8 | `ProtoSpec::External` 何时接上 | `read_external` 与它的用例在 Task 1；`ProtoSpec::External`、`to_spec` 的分支、重复 `local-port` 的检查与"Shadow TLS 不能叠在 `external` 上"在 Task 4 与引擎的工厂分支一起落地（同 M4b 的做法）：有了 spec 却没有工厂分支时，干构建会把每一条 `external` 行报成加载错误。Task 4 之前 `external` 行照旧没有 spec（`W0007`，按 REJECT） |
| P9 | 设计没写到的几处参数规则 | Shadow TLS 不能叠在 `external` 上（连的是本机上的程序）：`shadow_tls::allowed_on` 把 `External` 列进去，`E0018`，与 `wireguard` 同一句。`tfo` 只报 `W0028`（不适用），不再同时报 `W0029`（`read_common` 放进"暂不生效"的 `tfo` 被移除）。`addresses` 只在写了时报 `W0029`，`udp-relay=true` 同样。`local-port` 是 1 ～ 65535 的整数；`exec` 去掉首尾空白后不能为空 |
| P10 | 日志文件（设计 7.1） | `<数据目录>/external/<名字>.log`：策略名里的控制字符与 `/ \ : * ? " < > \|` 换成 `_`，去掉末尾的点与空格和开头的点；名字因此变了、变空了、或是 Windows 的设备名（`CON` `PRN` `AUX` `NUL` `COM0-9` `LPT0-9`，大小写不论、带不带扩展名）时，再加上原名 SHA-256 的前 8 个十六进制字符——两个策略永远不共用一个日志。目录在第一次拉起时创建；超过 1 MiB 时在拉起前改名为 `<名字>.log.1`（覆盖已有的那个）；每次拉起先写一行 `--- rurge: starting the program (unix time <秒>) ---`（工作区没有日期格式化的 crate）。`ExternalOutbound::new` 不碰文件系统 |
| P11 | 错误文本与日志 | `external: could not start <策略名> (<I/O 错误种类>)`（种类是 `io::ErrorKind` 的显示，如 `entity not found`）；`external: the local SOCKS5 port refused the connection`；SOCKS5 握手的错误照 `socks5` 出站的文本（`socks5: …`）；整次拨号超时是 `connect timed out`。日志：`info` `external: the program started`（`policy`、`pid`）、`external: the program exited`（`policy`、`pid`、`code`）、`external: the program stopped`（`policy`、`pid`）；`warn` `external: the program could not be started`（`policy`、`error` 为错误种类）。从不写 `exec` 与 `args` |
| P12 | 子进程的环境与标准流（M4-D11） | 去掉 `HTTP_PROXY` `HTTPS_PROXY` `ALL_PROXY` 与小写的三个，设 `NO_PROXY=*` 与 `no_proxy=*`（Windows 的环境变量不分大小写，两次设的是同一个值）；标准输入为空，标准输出与错误都写日志文件；工作目录沿用 rurge 的；`kill_on_drop(true)` 兜底 |
| P13 | 引擎的接法 | `EngineShared` 多两个字段：`processes: Arc<dyn ProcessHook>`（默认 `NoProcessGroups`，bin 设为 `PlatformProcesses`，同 `empty_group` 的设法）与 `externals: Arc<ExternalPrograms>`（每个构建过的 `ExternalOutbound` 的弱引用，跨代次）。工厂经 `with_externals(processes, <数据目录>/external, externals)` 拿到它们；干构建的工厂从不登记。`Engine::stop_external_programs()` 同时停掉名单里还活着的全部程序，等它们都结束。重载时 spec 没变的策略按指纹沿用原出站，程序随之沿用 |
| P14 | SOCKS5 的复用（设计 7.1） | `socks5.rs` 的握手抽成 `pub(crate) async fn negotiate(stream, request, credentials)`，`connect_request` 改为 `pub(crate)`；`Socks5Outbound` 的行为不变（它的用例照旧通过）。`ExternalOutbound` 直接用 `tokio::net::TcpStream` 连 `127.0.0.1:<local-port>`，不经连接器——`interface` 等参数对它不适用（`W0028`） |
| P15 | 任务的切分 | 设计第 16 节草图的 5 个任务；spec 的接入随 Task 4（P8）；辅助程序与真实进程的用例从 Task 3 起放在 `tests/external`（P4） |

## 承接事项

之前计划「延后事项」表里标给 M4c 的条目，及仍然有效的既有现象。

| # | 来源 | 事项 | 处理 | 任务 |
| - | ---- | ---- | ---- | ---- |
| C1 | M3a #25（设计 4.8、M4-D8） | 订阅导入的 `external` 行在实现 `external` 之前必须一律跳过，否则订阅作者（或 `http://` 链路上的中间人）能让 rurge 以任意参数启动本机程序 | 装配时先于其它检查跳过，`W0023`，说法固定；`external-policy-modifier` 设什么都不例外 | 1 |
| C2 | 设计第 13 节 | `CLAUDE.md` 的 unsafe 规则加上 `rurge-platform::process` 的 Job Object 例外 | Task 5 的文档 | 5 |
| C3 | M3b #7（P21） | 测试二进制偶发崩溃 | 照旧：门禁遇到就重跑 | — |

## File Structure

新建：

| 文件 | 职责 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-config/src/spec/external.rs` | `NOT_APPLICABLE`、`ExternalSpec`、`read_external`，与用例 | 1 |
| `crates/rurge-platform/src/process.rs` | `prepare`、`ProcessTree`（Unix 进程组、Windows Job Object），与用例 | 2 |
| `crates/rurge-proto/src/external.rs` | `ProcessHook`、`ProcessGroup`、`NoProcessGroups`、`log_file_name`、`ExternalOutbound`，与用例 | 3 |
| `tests/external/Cargo.toml` | 仅测试用的工作区成员 `rurge-external-tests` | 3、4 |
| `tests/external/src/bin/socks-helper.rs` | 测试辅助程序：极小的 SOCKS5 服务端 | 3 |
| `tests/external/tests/common/mod.rs` | 用例共用：辅助程序路径、`Platform` 钩子、回显服务端、等端口关闭 | 3 |
| `tests/external/tests/outbound.rs` | `ExternalOutbound` 拉起真实程序的用例 | 3 |
| `tests/external/tests/engine.rs` | 经引擎的用例 | 4 |
| `tests/external/README.md` | 这个成员是什么 | 5 |

修改：

| 文件 | 改动 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-config/src/{redact.rs, spec/reader.rs, spec/mod.rs}` | `args` 脱敏、`ParamReader::all`、模块导出（1）；`ProtoSpec::External` 与 `to_spec` 分支（4） | 1、4 |
| `crates/rurge-policy/src/assemble.rs` | 订阅导入的 `external` 跳过 | 1 |
| `Cargo.toml` | 工作区依赖 `nix`（2）；成员 `tests/external`（3） | 2、3 |
| `crates/rurge-platform/{Cargo.toml, src/lib.rs, src/sysproxy/windows.rs}` | 依赖与特性、`pub mod process`、注释 | 2 |
| `crates/rurge-proto/{Cargo.toml, src/lib.rs, src/socks5.rs}` | `tokio` 的 `process` 特性、`pub mod external`、`negotiate` | 3 |
| `crates/rurge-config/src/{config.rs, spec/shadow_tls.rs}`、快照 | 重复的 `local-port`；Shadow TLS 不能叠在 `external` 上；kitchen-sink 多一条 `W0029`（`addresses`） | 4 |
| `crates/rurge-engine/src/{shared.rs, outbounds.rs, runtime.rs, engine.rs, lib.rs}` | `ExternalPrograms`、`EngineShared` 的两个字段、工厂分支与 `with_externals`、`stop_external_programs` | 4 |
| `crates/rurge/{Cargo.toml, src/cli/runtime.rs, src/cli/run.rs, src/capabilities.rs, tests/cli.rs}` | `PlatformProcesses`、退出流程、能力表翻转与用例 | 5 |
| 文档（`CLAUDE.md`、两份 README、兼容性清单、`docs/api/phase2.md`、手工验收） | 见 Task 5 | 5 |

## 任务一览

| 任务 | 交付物 | 依赖 |
| ---- | ------ | ---- |
| 1 | 配置层：`ExternalSpec` 与 `read_external`、`ParamReader::all`、`args` 脱敏；订阅导入的 `external` 跳过（承接 C1） | — |
| 2 | `rurge-platform::process`：Unix 进程组、Windows Job Object（第二个 unsafe 例外） | — |
| 3 | `ExternalOutbound`：拉起、日志与轮转、环境、再拉起与间隔、连接的重试、停止；进程控制 trait；测试辅助程序与真实进程的用例 | 1、2 |
| 4 | 接入：`ProtoSpec::External`、重复 `local-port`、引擎工厂、`EngineShared` 的进程控制与名单、`stop_external_programs`、经引擎的用例 | 3 |
| 5 | bin：`PlatformProcesses`、退出流程停掉外部程序、能力表翻转 `external`；文档（承接 C2） | 4 |

---

### Task 1: 配置层——`ExternalSpec`、`args` 脱敏；订阅导入的 `external` 跳过（承接 C1）

`external` 策略行的参数读成 `ExternalSpec`（设计 4.4）：`exec` 必填，`args` 可重复、按出现顺序，`local-port` 必填（1 ～ 65535），`addresses` 可重复、只收 IP 地址；连本机端口用不上的通用参数报 `W0028` 并清掉。`args` 常带口令，spec 里是 `Secret`（`Debug` 不显示），并进内联参数的脱敏名单（M4-D12）。订阅导入的 `external` 行一律跳过（C1，设计 4.8）：订阅作者、或 `http://` 订阅链路上的中间人，不能让 rurge 启动任何程序。本任务只提供 `read_external`；`to_spec` 的分支与重复 `local-port` 的检查在 Task 4 与引擎的工厂分支一起接上（P8），之前 `external` 行照旧 `W0007`。

**Files:**
- Create: `crates/rurge-config/src/spec/external.rs`（`NOT_APPLICABLE`、`ExternalSpec`、`read_external`，与用例）
- Modify: `crates/rurge-config/src/spec/mod.rs`（`pub mod external;` 与导出）、`src/spec/reader.rs`（`ParamReader::all`，与用例）、`src/redact.rs`（`args`，与用例）
- Modify: `crates/rurge-policy/src/assemble.rs`（订阅导入的 `external` 跳过，与用例）

**Interfaces:**
- Consumes: 既有的 `rurge_config::spec::{Secret, ParamReader, CommonOpts}`、`spec::tls::refuse_tls`、`ParamMap::get_all`。
- Produces:
  - `rurge_config::spec::ParamReader::all(&mut self, key: &str) -> Vec<&'a str>`（按书写顺序的全部取值，并记为已读）
  - `rurge_config::spec::external::{NOT_APPLICABLE: [&str; 6], ExternalSpec, read_external(r: &mut ParamReader<'_>, common: &mut CommonOpts) -> ExternalSpec}`（报错后返回值无意义，调用方看 `r.has_errors()`）与导出 `rurge_config::spec::ExternalSpec`
  - `pub struct ExternalSpec { pub exec: String, pub args: Secret<Vec<String>>, pub local_port: u16, pub addresses: Vec<IpAddr> }`（`Clone + Debug + PartialEq + Eq`）

- [ ] **Step 1: 先写用例**

`crates/rurge-policy/src/assemble.rs`——把

```rust
            )]
        );
    }
}

```

换成

```rust
            )]
        );
    }

    /// An `external` line would have rurge start whatever program the
    /// subscription names: never imported, whatever the modifier sets
    /// (phase 2 M4 design 4.8, M3a deferred item #25).
    #[test]
    fn a_subscription_never_brings_an_external_policy() {
        let cfg = profile(
            "Corp = http, corp.test, 80",
            "G = select, policy-path=https://sub.test/g, external-policy-modifier=\"exec=/bin/true\"",
        );
        let a = assemble(
            &cfg,
            &snapshots(
                &cfg,
                &[(
                    "G",
                    "Run = external, exec=/bin/sh, args=-c, args=evil, local-port=1080\n\
Plain = http, p.test, 80",
                )],
            ),
        );
        assert_eq!(members(&a, "G"), ["Plain"]);
        let skipped: Vec<_> = warnings(&a)
            .into_iter()
            .filter(|(code, _)| *code == codes::W_SET_LINES_SKIPPED)
            .collect();
        assert_eq!(
            skipped,
            [(
                codes::W_SET_LINES_SKIPPED,
                "policy group `G`: `policy-path` line 1: `external` policies are not imported from subscriptions; skipped".to_string()
            )]
        );
    }
}

```

`crates/rurge-config/src/redact.rs`——把

```rust
            "[General]\nproxy-test-url = http://p.test/\n"
        );
    }
}

```

换成

```rust
            "[General]\nproxy-test-url = http://p.test/\n"
        );
    }

    /// Every `args` of an `external` line, quoted or not (M4-D12).
    #[test]
    fn an_external_line_loses_its_args() {
        assert_eq!(
            redact_definition(
                "external, exec = \"/usr/bin/sshpass\", args = \"-p\", args = \"hunter2, really\", args=ssh, local-port = 1080"
            ),
            "external, exec = \"/usr/bin/sshpass\", args = ***, args = ***, args=***, local-port = 1080"
        );
    }
}

```

`crates/rurge-config/src/spec/reader.rs`——把

```rust
    #[test]
    fn numbers_report_what_was_expected() {
```

换成

```rust
    #[test]
    fn a_repeated_parameter_is_read_whole_and_in_order() {
        let p = policy("external, exec=/bin/p, args=-D, args=1080, args=-N");
        let mut r = ParamReader::new(&p);
        assert_eq!(r.all("args"), ["-D", "1080", "-N"]);
        assert!(r.all("absent").is_empty());
        r.touch("exec");
        // read once, not reported as unknown
        assert!(r.finish().is_empty());
    }

    #[test]
    fn numbers_report_what_was_expected() {
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-policy a_subscription_never_brings_an_external_policy`
Expected: FAIL——订阅导入的 `external` 行还在组里（`rurge-config` 自己的用例此时编译不过：`ParamReader::all` 还没有，Step 3 之后才能跑）：

```text
test assemble::tests::a_subscription_never_brings_an_external_policy ... FAILED
thread 'assemble::tests::a_subscription_never_brings_an_external_policy' panicked at crates\rurge-policy\src\assemble.rs:1673:9:
assertion `left == right` failed
  left: ["Run", "Plain"]
 right: ["Plain"]
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 128 filtered out; finished in 0.00s
   Compiling thiserror v2.0.20
   Compiling thiserror-impl v2.0.20
error: test failed, to rerun pass `-p rurge-policy --lib`
exit 101
```

- [ ] **Step 3: 实现（新模块自带用例）**

新建 `crates/rurge-config/src/spec/external.rs`：

```rust
//! `external` policy parameters (manual: Policies › External Proxy Program):
//! a program rurge starts itself, reached as a SOCKS5 proxy on a local port.

use super::common::CommonOpts;
use super::reader::ParamReader;
use super::secret::Secret;
use super::tls::refuse_tls;
use crate::diagnostic::codes;
use std::net::IpAddr;

/// Common parameters that mean nothing for a connection to a local port.
/// Warned about (`W0028`) and cleared.
pub const NOT_APPLICABLE: [&str; 6] = [
    "interface",
    "allow-other-interface",
    "tfo",
    "tos",
    "ip-version",
    "underlying-proxy",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalSpec {
    /// The program, as the operating system finds it.
    pub exec: String,
    /// Its arguments, in the order written. They often carry a password
    /// (`sshpass -p …`), so they never print (M4-D12).
    pub args: Secret<Vec<String>>,
    /// The port its SOCKS5 server listens on, at `127.0.0.1`.
    pub local_port: u16,
    /// Addresses to keep out of the TUN routes (phase 3).
    pub addresses: Vec<IpAddr>,
}

/// Everything `external`-specific on the line, and the common parameters it
/// has no use for taken out of `common`. After an error was reported the
/// returned value is meaningless: the caller checks `r.has_errors()`.
pub fn read_external(r: &mut ParamReader<'_>, common: &mut CommonOpts) -> ExternalSpec {
    refuse_tls(r);
    for key in NOT_APPLICABLE {
        if r.has(key) {
            r.warn(
                codes::W_PARAM_NOT_APPLICABLE,
                format!("`{key}` does not apply to `external` policies; ignored"),
            );
        }
    }
    common.interface = None;
    common.allow_other_interface = false;
    common.tfo = false;
    common.tos = 0;
    common.ip_version = Default::default();
    common.underlying_proxy = None;
    let exec = r.str("exec").map(str::trim).unwrap_or_default();
    if exec.is_empty() {
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            "`exec` is required".to_string(),
        );
    }
    let args = r.all("args").into_iter().map(str::to_string).collect();
    let local_port = match r.str("local-port") {
        None => {
            r.error(
                codes::E_INVALID_POLICY_PARAM,
                "`local-port` is required".to_string(),
            );
            0
        }
        Some(v) => match v.trim().parse::<u16>() {
            Ok(port) if port > 0 => port,
            _ => {
                r.invalid("local-port", v, "a port, 1-65535");
                0
            }
        },
    };
    let mut addresses = Vec::new();
    for v in r.all("addresses") {
        match v.trim().parse::<IpAddr>() {
            Ok(ip) => addresses.push(ip),
            Err(_) => r.invalid("addresses", v, "an IP address"),
        }
    }
    ExternalSpec {
        exec: exec.to_string(),
        args: Secret::new(args),
        local_port,
        addresses,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::Diagnostic;
    use crate::policy::parse_policy;
    use crate::span::Span;
    use crate::spec::IpVersion;
    use crate::spec::common::{Applies, Notes, read_common};
    use std::path::Path;
    use std::sync::Arc;

    fn read(def: &str) -> (ExternalSpec, CommonOpts, bool, Vec<Diagnostic>) {
        let p = parse_policy("X", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        let mut common = read_common(&mut r, Applies::Proxy, &mut Notes::default());
        let spec = read_external(&mut r, &mut common);
        let failed = r.has_errors();
        (spec, common, failed, r.finish())
    }

    fn errors(def: &str) -> Vec<(&'static str, String)> {
        let (_, _, failed, diags) = read(def);
        assert!(failed, "{def}");
        diags.into_iter().map(|d| (d.code, d.message)).collect()
    }

    /// The manual's example: `args` repeat, in order.
    #[test]
    fn the_manual_example() {
        let (spec, _, failed, diags) = read(
            "external, exec = \"/usr/bin/ssh\", args = \"11.22.33.44\", args = \"-D\", args = \"127.0.0.1:1080\", local-port = 1080, addresses = 11.22.33.44",
        );
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.exec, "/usr/bin/ssh");
        assert_eq!(spec.args.expose(), &["11.22.33.44", "-D", "127.0.0.1:1080"]);
        assert_eq!(spec.local_port, 1080);
        assert_eq!(spec.addresses, ["11.22.33.44".parse::<IpAddr>().unwrap()]);
        // no argument ever prints
        assert!(!format!("{spec:?}").contains("127.0.0.1:1080"));
        assert!(format!("{spec:?}").contains("Secret(***)"));
    }

    #[test]
    fn exec_and_local_port_are_required() {
        assert_eq!(
            errors("external, local-port = 1080"),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `X`: `exec` is required".to_string()
            )]
        );
        assert_eq!(
            errors("external, exec = \"  \", local-port = 1080")[0].1,
            "policy `X`: `exec` is required"
        );
        assert_eq!(
            errors("external, exec = /bin/prog"),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `X`: `local-port` is required".to_string()
            )]
        );
        for port in ["0", "65536", "http"] {
            assert_eq!(
                errors(&format!("external, exec = /bin/prog, local-port = {port}")),
                [(
                    codes::E_INVALID_POLICY_PARAM,
                    format!(
                        "policy `X`: invalid value `{port}` for `local-port` (expected a port, 1-65535)"
                    )
                )]
            );
        }
    }

    #[test]
    fn addresses_are_ip_addresses() {
        let (spec, _, failed, _) = read(
            "external, exec = /bin/prog, local-port = 1080, addresses = 10.0.0.1, addresses = fd00::1",
        );
        assert!(!failed);
        assert_eq!(spec.addresses.len(), 2);
        assert_eq!(
            errors("external, exec = /bin/prog, local-port = 1080, addresses = vpn.test"),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `X`: invalid value `vpn.test` for `addresses` (expected an IP address)"
                    .to_string()
            )]
        );
    }

    /// A connection to a local port has no interface, no TCP options and no
    /// relay of its own.
    #[test]
    fn socket_parameters_do_not_apply() {
        let (_, common, failed, diags) = read(
            "external, exec = /bin/prog, local-port = 1080, interface = en0, allow-other-interface = true, tfo = true, tos = 0x10, ip-version = v4-only, underlying-proxy = Other",
        );
        assert!(!failed);
        let found: Vec<(&str, &str)> = diags.iter().map(|d| (d.code, d.message.as_str())).collect();
        let expected: Vec<String> = NOT_APPLICABLE
            .iter()
            .map(|key| {
                format!("policy `X`: `{key}` does not apply to `external` policies; ignored")
            })
            .collect();
        assert_eq!(
            found,
            expected
                .iter()
                .map(|m| (codes::W_PARAM_NOT_APPLICABLE, m.as_str()))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            (
                common.interface,
                common.allow_other_interface,
                common.tfo,
                common.tos,
                common.ip_version,
                common.underlying_proxy
            ),
            (None, false, false, 0, IpVersion::Dual, None)
        );
    }

    #[test]
    fn tls_parameters_do_not_apply() {
        let (_, _, failed, diags) =
            read("external, exec = /bin/prog, local-port = 1080, sni = x.test");
        assert!(!failed);
        assert_eq!(
            (diags[0].code, diags[0].message.as_str()),
            (
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `X`: `sni` does not apply to `external` policies; ignored"
            )
        );
    }
}
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
pub mod common;
```

换成

```rust
pub mod common;
pub mod external;
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
pub use common::{Applies, CommonOpts, IpVersion, Tristate};
```

换成

```rust
pub use common::{Applies, CommonOpts, IpVersion, Tristate};
pub use external::ExternalSpec;
```

`crates/rurge-config/src/spec/reader.rs`——把

```rust
        policy.params.get(key)
```

换成

```rust
        policy.params.get(key)
    }

    /// Every value of `key`, in the order written; marks it as read.
    pub fn all(&mut self, key: &str) -> Vec<&'a str> {
        self.touch(key);
        let policy = self.policy;
        policy.params.get_all(key)
```

`crates/rurge-config/src/redact.rs`——把

```rust
/// `pre-shared-key` stays for profiles written the other way. Over-redacting
/// is the safe side for an endpoint whose purpose is safe output.
const SECRET_PARAMS: [&str; 16] = [
```

换成

```rust
/// `pre-shared-key` stays for profiles written the other way. An `external`
/// policy's `args` often carry a password (`sshpass -p …`, M4-D12).
/// Over-redacting is the safe side for an endpoint whose purpose is safe
/// output.
const SECRET_PARAMS: [&str; 17] = [
```

`crates/rurge-config/src/redact.rs`——把

```rust
    "test-url",
```

换成

```rust
    "test-url",
    "args",
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
                };
                if let Some(why) = reaches_into_profile(cfg, &policy, modifier) {
```

换成

```rust
                };
                // a subscription must never start a program on this machine,
                // whatever the modifier says (M4-D8)
                if policy.kind == PolicyKind::External {
                    diags.push(warn(
                        g,
                        codes::W_SET_LINES_SKIPPED,
                        format!("`policy-path` line {line}: `external` policies are not imported from subscriptions; skipped"),
                    ));
                    continue;
                }
                if let Some(why) = reaches_into_profile(cfg, &policy, modifier) {
```

要点：
- `read_external`：TLS 参数不适用（`refuse_tls`）；`NOT_APPLICABLE` 的六个参数各报一条 `W0028` 并从 `common` 里清掉（含 `underlying-proxy`，于是 `to_spec` 之后的中继检查不会再看它）；`exec`、`local-port` 缺了是 `E0018`；`local-port` 与 `addresses` 的错误值照 `ParamReader::invalid` 引用取值（端口与 IP 地址不是凭据）；`args` 从不出现在任何诊断里。
- `args` 进 `SECRET_PARAMS` 之后，`profiles/current`（`sensitive=0`）、`policies/detail` 与 `lineHash` 里每个 `args=` 的值都是 `***`（`lineHash` 取的就是脱敏后的定义）。
- 订阅的检查放在 `reaches_into_profile` 之前、按解析后的类型判断（`PolicyKind::External`），修饰列表设什么都不例外；说法固定：`` `policy-path` line <n>: `external` policies are not imported from subscriptions; skipped ``。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-config --lib` → 205 passed（新增 `spec::external::tests` 5 条、`spec::reader` 1 条、`redact` 1 条）。
Run: `cargo test -p rurge-policy assemble` → 通过（新增 `a_subscription_never_brings_an_external_policy`）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-config/src/spec crates/rurge-config/src/redact.rs crates/rurge-policy/src/assemble.rs
git commit -m "feat(config): external 策略的 ExternalSpec 与 args 脱敏；订阅导入的 external 一律跳过"
```


### Task 2: `rurge-platform::process`——Unix 进程组、Windows Job Object

外部程序要能连同它启动的进程一起结束（设计 7.3、M4-D6）。Unix：程序在拉起时成为一个新进程组的组长（`prepare`），结束时给整个组发信号（`killpg`，P2）。Windows：程序拉起之后立即放进一个设了"关闭即结束全部"的 Job Object，结束时关闭 Job 的句柄；rurge 自己死掉时句柄随进程关闭，同样不留孤儿（P1）。这是 `rurge-platform` 里第二个 `#[allow(unsafe_code)]` 函数，只做四个 FFI 调用，句柄都立即交给 `OwnedHandle`。本任务只提供平台接口；`rurge-proto` 的 trait 与 bin 的适配器在 Task 3、Task 5。

**Files:**
- Create: `crates/rurge-platform/src/process.rs`（`prepare`、`ProcessTree`、Windows 的 `job_for`，与用例）
- Modify: `Cargo.toml`（工作区依赖 `nix`）、`crates/rurge-platform/Cargo.toml`（`windows-sys` 的三个特性、Unix 上的 `nix`、说明与 lint 注释）、`src/lib.rs`（`pub mod process;`）、`src/sysproxy/windows.rs`（"唯一的 unsafe"的注释改为"两个之一"）

**Interfaces:**
- Consumes: `windows-sys` 0.61（`Win32::System::JobObjects`、`Win32::System::Threading`）；`nix` 0.31（`sys::signal::{killpg, Signal}`、`unistd::Pid`、`errno::Errno`）。
- Produces:
  - `rurge_platform::process::prepare(command: &mut std::process::Command)`（Unix：`process_group(0)`；Windows：什么也不做）
  - `pub struct ProcessTree`，`ProcessTree::contain(pid: u32) -> io::Result<ProcessTree>`（程序刚拉起、还没被回收时调用）
  - `ProcessTree::terminate(&mut self) -> io::Result<()>`（Unix：组 SIGTERM；Windows：关闭 Job，全部结束）、`ProcessTree::kill(&mut self) -> io::Result<()>`（Unix：组 SIGKILL；Windows：同 `terminate`）；组已经没了不算错，重复调用无害
  - Windows 上丢弃 `ProcessTree` 即结束全部；Unix 上丢弃什么也不做（由 Task 3 的看守任务负责）

- [ ] **Step 1: 先写用例（连同模块）**

新模块的用例与实现在同一个文件里；先把模块与 `pub mod process;` 放进去（依赖与特性留到 Step 3）：

新建 `crates/rurge-platform/src/process.rs`：

```rust
//! Stopping an external program together with every process it started
//! (phase 2 M4 design 7.3, M4-D6). On Unix the program leads a process group
//! of its own, and the group is signalled. On Windows it runs in a Job Object
//! that ends every process in it when the job's handle closes — when rurge
//! closes it, and also when rurge dies.

use std::io;
use std::process::Command;

/// Called on the command before it is spawned: on Unix the program becomes
/// the leader of a new process group. Nothing on Windows.
pub fn prepare(command: &mut Command) {
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(command, 0);
    #[cfg(not(unix))]
    let _ = command;
}

/// The program and whatever it starts.
pub struct ProcessTree {
    #[cfg(unix)]
    group: nix::unistd::Pid,
    /// `None` once closed: every process of the tree has been ended.
    #[cfg(windows)]
    job: Option<std::os::windows::io::OwnedHandle>,
}

impl ProcessTree {
    /// Takes in the program `pid`, spawned after `prepare` and not waited for
    /// yet (so the id is still its own). On Windows a process the program
    /// started before this call is not taken in.
    pub fn contain(pid: u32) -> io::Result<ProcessTree> {
        #[cfg(unix)]
        {
            let pid = i32::try_from(pid)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "no such process id"))?;
            Ok(ProcessTree {
                group: nix::unistd::Pid::from_raw(pid),
            })
        }
        #[cfg(windows)]
        {
            Ok(ProcessTree {
                job: Some(windows::job_for(pid)?),
            })
        }
    }

    /// Asks every process of the tree to end: SIGTERM to the group on Unix.
    /// Windows has no asking: every process ends at once.
    pub fn terminate(&mut self) -> io::Result<()> {
        #[cfg(unix)]
        {
            self.signal(nix::sys::signal::Signal::SIGTERM)
        }
        #[cfg(windows)]
        {
            self.job = None;
            Ok(())
        }
    }

    /// Ends every process of the tree now: SIGKILL to the group on Unix; on
    /// Windows the same as `terminate`.
    pub fn kill(&mut self) -> io::Result<()> {
        #[cfg(unix)]
        {
            self.signal(nix::sys::signal::Signal::SIGKILL)
        }
        #[cfg(windows)]
        {
            self.terminate()
        }
    }

    /// A group that has already ended is not an error.
    #[cfg(unix)]
    fn signal(&self, signal: nix::sys::signal::Signal) -> io::Result<()> {
        match nix::sys::signal::killpg(self.group, signal) {
            Ok(()) | Err(nix::errno::Errno::ESRCH) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(windows)]
mod windows {
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

    /// A Job Object that ends every process in it when its last handle
    /// closes, with process `pid` in it. The workspace's second `unsafe`
    /// (M4-D6): nothing but the four calls, on handles this function owns.
    #[allow(unsafe_code)]
    pub(super) fn job_for(pid: u32) -> io::Result<OwnedHandle> {
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
        };
        // SAFETY: no security attributes and no name are documented as
        // valid; a non-null result is a new handle nobody else owns.
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `job` is a valid handle owned by nothing else.
        let job = unsafe { OwnedHandle::from_raw_handle(job) };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: the buffer is the structure the class names, with its own
        // size, and lives across the call.
        let set = unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if set == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: plain call; a non-null result is a new handle nobody else owns.
        let process = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
        if process.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `process` is a valid handle owned by nothing else.
        let process = unsafe { OwnedHandle::from_raw_handle(process) };
        // SAFETY: both handles are valid and outlive the call.
        let assigned =
            unsafe { AssignProcessToJobObject(job.as_raw_handle(), process.as_raw_handle()) };
        if assigned == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Stdio};
    use std::time::{Duration, Instant};

    /// A program that runs for half a minute unless ended, started through
    /// a shell, so the shell's own child is a grandchild of ours.
    fn long_runner() -> Command {
        #[cfg(windows)]
        let mut command = {
            let mut c = Command::new("cmd");
            c.args(["/c", "ping -n 30 127.0.0.1"]);
            c
        };
        #[cfg(unix)]
        let mut command = {
            let mut c = Command::new("sh");
            c.args(["-c", "sleep 30; sleep 30"]);
            c
        };
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        prepare(&mut command);
        command
    }

    fn ended_within(child: &mut Child, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            if child.try_wait().unwrap().is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn terminating_the_tree_ends_the_program() {
        let mut child = long_runner().spawn().unwrap();
        let mut tree = ProcessTree::contain(child.id()).unwrap();
        assert!(!ended_within(&mut child, Duration::from_millis(200)));
        tree.terminate().unwrap();
        let ended = ended_within(&mut child, Duration::from_secs(10));
        if !ended {
            let _ = child.kill();
        }
        assert!(ended, "the program is still running");
        // ending an ended tree is no error
        tree.terminate().unwrap();
        tree.kill().unwrap();
    }

    #[test]
    fn killing_the_tree_ends_the_program() {
        let mut child = long_runner().spawn().unwrap();
        let mut tree = ProcessTree::contain(child.id()).unwrap();
        tree.kill().unwrap();
        let ended = ended_within(&mut child, Duration::from_secs(10));
        if !ended {
            let _ = child.kill();
        }
        assert!(ended, "the program is still running");
    }

    /// The job goes with its handle: a rurge that dies (or forgets the tree)
    /// leaves nothing behind (M4-D6).
    #[cfg(windows)]
    #[test]
    fn dropping_the_tree_ends_the_program_on_windows() {
        let mut child = long_runner().spawn().unwrap();
        let tree = ProcessTree::contain(child.id()).unwrap();
        drop(tree);
        let ended = ended_within(&mut child, Duration::from_secs(10));
        if !ended {
            let _ = child.kill();
        }
        assert!(ended, "the program is still running");
    }

    #[cfg(windows)]
    #[test]
    fn a_process_that_does_not_exist_cannot_be_taken_in() {
        // process ids are multiples of 4 on Windows: 3 is never one
        assert!(ProcessTree::contain(3).is_err());
    }
}
```

`crates/rurge-platform/src/lib.rs`——把

```rust
pub mod dns;
```

换成

```rust
pub mod dns;
pub mod process;
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-platform process`
Expected: FAIL——`windows-sys` 还没开 Job Object 的特性（Unix 上同样失败：还没有 `nix` 依赖）：

```text
error[E0432]: unresolved import `windows_sys::Win32::System::JobObjects`
  --> crates\rurge-platform\src\process.rs:96:41
  --> C:\Users\SZV01065\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\windows-sys-0.61.2\src\Windows\Win32\System\mod.rs:56:9
  --> C:\Users\SZV01065\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\windows-sys-0.61.2\src\Windows\Win32\System\mod.rs:55:7
For more information about this error, try `rustc --explain E0432`.
error: could not compile `rurge-platform` (lib test) due to 1 previous error
exit 101
```

- [ ] **Step 3: 实现（依赖与特性）**

`Cargo.toml`——把

```toml
windows-sys = { version = "0.61", features = ["Win32_Networking_WinInet"] }
```

换成

```toml
windows-sys = { version = "0.61", features = ["Win32_Networking_WinInet"] }
nix = { version = "0.31", default-features = false }
```

`crates/rurge-platform/Cargo.toml`——把

```toml
description = "Platform-specific helpers for rurge (directories, system DNS, system proxy, service install)"
```

换成

```toml
description = "Platform-specific helpers for rurge (directories, system DNS, system proxy, service install, external programs)"
```

`crates/rurge-platform/Cargo.toml`——把

```toml
windows-sys.workspace = true
```

换成

```toml
windows-sys = { workspace = true, features = [
    "Win32_Security",
    "Win32_System_JobObjects",
    "Win32_System_Threading",
] }

# killpg for an external program's process group (`process`)
[target.'cfg(unix)'.dependencies]
nix = { workspace = true, features = ["signal"] }
```

`crates/rurge-platform/Cargo.toml`——把

```toml
# settings changed (`sysproxy::windows`) is an FFI call. The lint stays `deny`
# here and only that one function opts out; every other crate keeps `forbid`.
```

换成

```toml
# settings changed (`sysproxy::windows`) and putting an external program in a
# Job Object (`process`) are FFI calls. The lint stays `deny` here and only
# those two functions opt out; every other crate keeps `forbid`.
```

`crates/rurge-platform/src/sysproxy/windows.rs`——把

```rust
/// The one `unsafe` in the workspace (plan decision P1).
```

换成

```rust
/// One of the workspace's two `unsafe` exceptions (plan decision P1; the
/// other is `process`).
```

要点：
- `Cargo.lock` 只多一条依赖边（`rurge-platform → nix`，它已在锁文件里）；不下载新 crate。
- `job_for` 每个 FFI 调用一个 `unsafe` 块、各带 `SAFETY:`；失败时返回 `io::Error::last_os_error()`，之前创建的句柄由 `OwnedHandle` 关闭。
- 用例拉起的是 `cmd /c ping -n 30 127.0.0.1`（Windows）或 `sh -c "sleep 30; sleep 30"`（Unix）——外壳的子进程是用例的孙进程；每条用例最后都确认程序已结束（没结束就 `kill` 再断言失败），不留进程。
- Unix 分支在本机编译不到（P2），照抄；CI 的 Linux / macOS 上它与 `terminating_the_tree_ends_the_program`、`killing_the_tree_ends_the_program` 一起验证。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-platform process` → 4 passed（Windows：`terminating_the_tree_ends_the_program`、`killing_the_tree_ends_the_program`、`dropping_the_tree_ends_the_program_on_windows`、`a_process_that_does_not_exist_cannot_be_taken_in`；Unix 上是前两条）。
Run: `tasklist | findstr /i ping` → 没有用例留下的 `ping`。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add Cargo.toml Cargo.lock crates/rurge-platform
git commit -m "feat(platform): 外部程序的进程树——Unix 进程组与 killpg、Windows Job Object（第二个 unsafe 例外）"
```


### Task 3: `ExternalOutbound`——拉起、日志、再拉起、重试与停止；测试辅助程序

`rurge-proto` 的 `external` 模块（设计第 7 节）：第一次用到时拉起程序（`exec` 加按原顺序的 `args`，标准输入为空，输出追加写进日志，环境里没有代理设置，P12），经 SOCKS5 连 `127.0.0.1:<local-port>`；连不上时每 500 ms 一次、最多 6 次，每次以 500 ms 为限（P5）；程序退出后下次用到时再拉起，两次拉起至少隔 2 秒（P7）；每个程序由一个看守任务持有，程序退出、被要求停止或出站被丢弃时连同它启动的进程一起结束（P6）。进程怎样分组由注入的 `ProcessHook` 决定：bin 注入 `rurge-platform::process`（Task 5），其余用 `NoProcessGroups`。SOCKS5 握手从 `socks5.rs` 抽出来复用（P14）。

真实拉起程序的用例要一个测试自带的程序：新的仅测试用工作区成员 `tests/external`（`rurge-external-tests`）只有一个二进制目标 `socks-helper`——只监听 127.0.0.1、不认证、只做 CONNECT 的 SOCKS5 服务端，另有几个开关（记下自己的参数与代理变量、晚一点才监听、服务几次就退出、再拉一个一直运行的子进程）；用例在这个包的 `tests/` 里（P4），用的进程分组是 `rurge-platform::process`（`common` 里一个与 bin 相同的小适配器）。

**Files:**
- Create: `crates/rurge-proto/src/external.rs`（常量、`ProcessHook`、`ProcessGroup`、`NoProcessGroups`、`log_file_name`、`ExternalOutbound`、看守任务，与用例）
- Create: `tests/external/Cargo.toml`、`tests/external/src/bin/socks-helper.rs`、`tests/external/tests/common/mod.rs`、`tests/external/tests/outbound.rs`
- Modify: `Cargo.toml`（成员 `tests/external`）、`crates/rurge-proto/Cargo.toml`（`tokio` 的 `process` 特性、开发依赖 `tempfile`）、`src/lib.rs`（`pub mod external;`）、`src/socks5.rs`（`connect_request` 改 `pub(crate)`，握手抽成 `negotiate`）

**Interfaces:**
- Consumes: Task 1 的 `rurge_config::spec::ExternalSpec`；既有的 `rurge_proto::{Outbound, OutboundError}`、`rurge_net::connector::{BoxedStream, ConnectOpts, Target}`。
- Produces:
  - `rurge_proto::external::{ATTEMPTS: u32 (6), ATTEMPT_EVERY: Duration (500 ms), START_GAP: Duration (2 s), STOP_GRACE: Duration (2 s), LOG_LIMIT: u64 (1 MiB)}`
  - `pub trait ProcessHook: Send + Sync { fn prepare(&self, command: &mut std::process::Command); fn contain(&self, pid: u32) -> io::Result<Option<Box<dyn ProcessGroup>>>; }`
  - `pub trait ProcessGroup: Send { fn terminate(&mut self) -> io::Result<()>; fn kill(&mut self) -> io::Result<()>; }`
  - `pub struct NoProcessGroups;`（`ProcessHook`：不做准备，`contain` 返回 `Ok(None)`）
  - `pub fn log_file_name(policy: &str) -> String`
  - `ExternalOutbound::new(name: &str, spec: &ExternalSpec, log_dir: &Path, hook: Arc<dyn ProcessHook>) -> ExternalOutbound`（不做任何 I/O）、`log_path(&self) -> &Path`、`async fn stop(&self)`（停掉正在运行的程序与它启动的进程，等它们结束）；`impl Outbound`
  - `rurge_proto::socks5::{connect_request, negotiate}`（`pub(crate)`）
  - 测试辅助程序 `socks-helper`：`--port <p> [--record <file>] [--delay-ms <n>] [--serve <n>] [--child <file>]`、`--hold <file>`（见文件头注释）；`tests/external/tests/common` 的 `helper()`、`free_port()`、`Platform`、`outbound(..)`、`args(..)`、`echo_server()`、`round_trip(..)`、`wait_closed(port)`、`wait_for_file(path)`

- [ ] **Step 1: 先写用例（测试成员与辅助程序）**

`Cargo.toml`——把

```toml
members = ["crates/*", "tests/interop"]
```

换成

```toml
members = ["crates/*", "tests/interop", "tests/external"]
```

新建 `tests/external/Cargo.toml`：

```toml
[package]
name = "rurge-external-tests"
description = "The test program of rurge's `external` outbound and the tests that start it (test-only, never published)"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish = false

[[bin]]
name = "socks-helper"
path = "src/bin/socks-helper.rs"
test = false

[dev-dependencies]
rurge-config.workspace = true
rurge-net.workspace = true
rurge-platform.workspace = true
rurge-proto.workspace = true
tempfile.workspace = true
tokio.workspace = true

[lints]
workspace = true
```

新建 `tests/external/src/bin/socks-helper.rs`：

```rust
//! A SOCKS5 server as small as a test needs (no authentication, CONNECT
//! only) that rurge starts as an `external` policy's program. It listens on
//! 127.0.0.1 only and connects nowhere but where its client asks.
//!
//! socks-helper --port <p> [--record <file>] [--delay-ms <n>] [--serve <n>]
//!              [--child <file>]
//! socks-helper --hold <file>
//!
//! `--record` appends its process id, its arguments and the proxy variables
//! it was given to `<file>`; `--delay-ms` waits before listening; `--serve`
//! serves one client at a time and exits after that many relayed sessions (a
//! connection given up before its request does not count); `--child` starts
//! a copy of itself in
//! `--hold` mode, which listens on a port of its own, writes the port to
//! `<file>` and runs until ended.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::process::{Command, Stdio};
use std::time::Duration;

fn value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(file) = value(&args, "--hold") {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        // written whole at once, so a reader never sees half a number
        let tmp = format!("{file}.tmp");
        std::fs::write(&tmp, port.to_string()).unwrap();
        std::fs::rename(&tmp, &file).unwrap();
        for stream in listener.incoming() {
            drop(stream);
        }
        return;
    }
    let port: u16 = value(&args, "--port")
        .and_then(|p| p.parse().ok())
        .expect("--port <port>");
    if let Some(file) = value(&args, "--record") {
        let mut out = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)
            .unwrap();
        let mut line = format!("pid={} args={}", std::process::id(), args.join(" "));
        for name in ["HTTP_PROXY", "ALL_PROXY", "NO_PROXY", "no_proxy"] {
            line.push_str(&format!(
                " {name}={}",
                std::env::var(name).unwrap_or_default()
            ));
        }
        writeln!(out, "{line}").unwrap();
    }
    if let Some(ms) = value(&args, "--delay-ms").and_then(|v| v.parse().ok()) {
        std::thread::sleep(Duration::from_millis(ms));
    }
    let serve: Option<usize> = value(&args, "--serve").and_then(|v| v.parse().ok());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).unwrap();
    println!("socks-helper listening on 127.0.0.1:{port}");
    // started once listening: by then rurge has long taken this process in
    // (a child started in the first instants of a program would escape a
    // Windows Job Object)
    if let Some(file) = value(&args, "--child") {
        start_child(&file);
    }
    if let Some(limit) = serve {
        let mut relayed = 0;
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            if serve_one(stream).unwrap_or(false) {
                relayed += 1;
                if relayed >= limit {
                    return;
                }
            }
        }
    }
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        std::thread::spawn(move || {
            let _ = serve_one(stream);
        });
    }
}

/// A copy of this program in `--hold` mode, left running: only the end of
/// the whole tree ends it, which is what the tests check.
#[allow(clippy::zombie_processes)]
fn start_child(file: &str) {
    let exe = std::env::current_exe().unwrap();
    Command::new(exe)
        .args(["--hold", file])
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
}

/// `true` once a session has been relayed to its end.
fn serve_one(mut client: TcpStream) -> std::io::Result<bool> {
    let mut head = [0u8; 2];
    client.read_exact(&mut head)?;
    let mut methods = vec![0u8; usize::from(head[1])];
    client.read_exact(&mut methods)?;
    client.write_all(&[5, 0])?;
    let mut request = [0u8; 4];
    client.read_exact(&mut request)?;
    let host: Vec<SocketAddr> = match request[3] {
        1 => {
            let mut ip = [0u8; 4];
            client.read_exact(&mut ip)?;
            vec![SocketAddr::from((Ipv4Addr::from(ip), port(&mut client)?))]
        }
        4 => {
            let mut ip = [0u8; 16];
            client.read_exact(&mut ip)?;
            vec![SocketAddr::from((Ipv6Addr::from(ip), port(&mut client)?))]
        }
        _ => {
            let mut len = [0u8; 1];
            client.read_exact(&mut len)?;
            let mut name = vec![0u8; usize::from(len[0])];
            client.read_exact(&mut name)?;
            let port = port(&mut client)?;
            (String::from_utf8_lossy(&name).as_ref(), port)
                .to_socket_addrs()?
                .collect()
        }
    };
    let Ok(upstream) = TcpStream::connect(&host[..]) else {
        client.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0])?;
        return Ok(false);
    };
    client.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0])?;
    let (mut c2, mut u2) = (client.try_clone()?, upstream.try_clone()?);
    let up = std::thread::spawn(move || {
        let _ = std::io::copy(&mut c2, &mut u2);
        let _ = u2.shutdown(Shutdown::Write);
    });
    let (mut client, mut upstream) = (client, upstream);
    let _ = std::io::copy(&mut upstream, &mut client);
    let _ = client.shutdown(Shutdown::Write);
    let _ = up.join();
    Ok(true)
}

fn port(stream: &mut TcpStream) -> std::io::Result<u16> {
    let mut port = [0u8; 2];
    stream.read_exact(&mut port)?;
    Ok(u16::from_be_bytes(port))
}
```

新建 `tests/external/tests/common/mod.rs`：

```rust
//! What the test files of this directory share. Each test file is a crate of
//! its own and uses a part of all this only: hence the two `allow`s.

#![allow(dead_code, unused_imports)]

pub use rurge_config::HostName;
pub use rurge_config::spec::{ExternalSpec, Secret};
pub use rurge_net::connector::{ConnectOpts, Target};
pub use rurge_proto::Outbound;
pub use rurge_proto::external::{ExternalOutbound, NoProcessGroups, ProcessGroup, ProcessHook};
pub use std::net::SocketAddr;
pub use std::path::{Path, PathBuf};
pub use std::sync::Arc;
pub use std::time::{Duration, Instant};
pub use tokio::io::{AsyncReadExt, AsyncWriteExt};
pub use tokio::net::TcpStream;

/// The test program (`src/bin/socks-helper.rs`).
pub fn helper() -> String {
    env!("CARGO_BIN_EXE_socks-helper").to_string()
}

/// A port that was free a moment ago.
pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// `rurge-platform::process` behind the hook, as the bin wires it.
pub struct Platform;

struct Tree(rurge_platform::process::ProcessTree);

impl ProcessGroup for Tree {
    fn terminate(&mut self) -> std::io::Result<()> {
        self.0.terminate()
    }
    fn kill(&mut self) -> std::io::Result<()> {
        self.0.kill()
    }
}

impl ProcessHook for Platform {
    fn prepare(&self, command: &mut std::process::Command) {
        rurge_platform::process::prepare(command);
    }
    fn contain(&self, pid: u32) -> std::io::Result<Option<Box<dyn ProcessGroup>>> {
        let tree = rurge_platform::process::ProcessTree::contain(pid)?;
        Ok(Some(Box::new(Tree(tree))))
    }
}

/// An outbound running the helper with `args`, its log in `dir`.
pub fn outbound(name: &str, args: &[String], port: u16, dir: &Path) -> ExternalOutbound {
    let spec = ExternalSpec {
        exec: helper(),
        args: Secret::new(args.to_vec()),
        local_port: port,
        addresses: Vec::new(),
    };
    ExternalOutbound::new(name, &spec, dir, Arc::new(Platform))
}

/// `--port <port>` and whatever else.
pub fn args(port: u16, more: &[&str]) -> Vec<String> {
    let mut out = vec!["--port".to_string(), port.to_string()];
    out.extend(more.iter().map(|s| s.to_string()));
    out
}

/// A loopback server that echoes every connection back.
pub async fn echo_server() -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    addr
}

pub fn target(addr: SocketAddr) -> Target {
    Target::new(HostName::parse(&addr.ip().to_string()), addr.port())
}

/// One round trip through `outbound` to the echo server at `echo`.
pub async fn round_trip(outbound: &dyn Outbound, echo: SocketAddr) -> Result<(), String> {
    let mut stream = outbound
        .connect_tcp(&target(echo), &ConnectOpts::default())
        .await
        .map_err(|e| e.to_string())?;
    stream.write_all(b"ping").await.unwrap();
    let mut buf = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut buf))
        .await
        .expect("the echo comes back")
        .unwrap();
    assert_eq!(&buf, b"ping");
    Ok(())
}

/// Waits (at most 10 seconds) until nothing listens on `port` any more.
pub async fn wait_closed(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let attempt = tokio::time::timeout(
            Duration::from_millis(500),
            TcpStream::connect(("127.0.0.1", port)),
        )
        .await;
        // refused, or no answer within the slot (Windows refuses slowly)
        if !matches!(attempt, Ok(Ok(_))) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "port {port} still accepts connections"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Waits (at most 10 seconds) for `path` to exist; returns its text.
pub async fn wait_for_file(path: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(text) = std::fs::read_to_string(path)
            && !text.is_empty()
        {
            return text;
        }
        assert!(
            Instant::now() < deadline,
            "{} never appeared",
            path.display()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
```

新建 `tests/external/tests/outbound.rs`：

```rust
//! `ExternalOutbound` with a real program (phase 2 M4 design 7.1–7.3): the
//! test helper, started and stopped through `rurge-platform::process`.

mod common;
use common::*;

/// The first use starts the program with its arguments in order and no
/// proxy settings; its output goes to the log, after a separator.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_first_use_starts_the_program() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let record = dir.path().join("record");
    let o = outbound(
        "Ext",
        &args(
            port,
            &["--record", &record.to_string_lossy(), "--delay-ms", "300"],
        ),
        port,
        dir.path(),
    );
    assert!(!record.exists(), "nothing starts before the first use");
    let echo = echo_server().await;
    round_trip(&o, echo).await.unwrap();
    // one program serves every later connection
    round_trip(&o, echo).await.unwrap();
    let recorded = std::fs::read_to_string(&record).unwrap();
    assert_eq!(recorded.lines().count(), 1, "{recorded}");
    assert!(
        recorded.contains(&format!(
            "args=--port {port} --record {} --delay-ms 300 ",
            record.to_string_lossy()
        )),
        "{recorded}"
    );
    assert!(
        recorded.contains(" HTTP_PROXY= ALL_PROXY= NO_PROXY=* no_proxy=*"),
        "{recorded}"
    );
    assert_eq!(o.log_path(), dir.path().join("Ext.log"));
    let log = std::fs::read_to_string(o.log_path()).unwrap();
    assert!(log.starts_with("--- rurge: starting the program"), "{log}");
    assert!(
        log.contains(&format!("socks-helper listening on 127.0.0.1:{port}")),
        "{log}"
    );
    o.stop().await;
    wait_closed(port).await;
}

/// A program that exited is started again on the next use — not sooner
/// than two seconds after the last start, which the request waits out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_program_that_exited_is_started_again() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let record = dir.path().join("record");
    let o = outbound(
        "Ext",
        &args(
            port,
            &["--record", &record.to_string_lossy(), "--serve", "1"],
        ),
        port,
        dir.path(),
    );
    let echo = echo_server().await;
    let first = Instant::now();
    round_trip(&o, echo).await.unwrap();
    wait_closed(port).await;
    round_trip(&o, echo).await.unwrap();
    assert!(first.elapsed() >= external_gap(), "{:?}", first.elapsed());
    let recorded = std::fs::read_to_string(&record).unwrap();
    let pids: Vec<&str> = recorded
        .lines()
        .map(|l| l.split(' ').next().unwrap())
        .collect();
    assert_eq!(pids.len(), 2, "{recorded}");
    assert_ne!(pids[0], pids[1]);
    let log = std::fs::read_to_string(o.log_path()).unwrap();
    assert_eq!(
        log.matches("--- rurge: starting the program").count(),
        2,
        "{log}"
    );
    o.stop().await;
}

fn external_gap() -> Duration {
    rurge_proto::external::START_GAP
}

/// A program that never listens on its port: six attempts, half a second
/// apart, then the fixed text.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_port_that_never_opens_fails_the_request() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let elsewhere = free_port();
    let o = outbound("Ext", &args(elsewhere, &[]), port, dir.path());
    let started = Instant::now();
    let err = round_trip(&o, echo_server().await).await.unwrap_err();
    assert_eq!(
        err,
        "external: the local SOCKS5 port refused the connection"
    );
    let took = started.elapsed();
    assert!(
        took >= Duration::from_millis(2500) && took < Duration::from_secs(6),
        "{took:?}"
    );
    o.stop().await;
    wait_closed(elsewhere).await;
}

/// Stopping ends the program and what it started (M4-D6): the helper's
/// own child keeps a port open until it is ended.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopping_ends_the_whole_tree() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let child_port = dir.path().join("child-port");
    let o = outbound(
        "Ext",
        &args(port, &["--child", &child_port.to_string_lossy()]),
        port,
        dir.path(),
    );
    round_trip(&o, echo_server().await).await.unwrap();
    let grandchild: u16 = wait_for_file(&child_port).await.trim().parse().unwrap();
    TcpStream::connect(("127.0.0.1", grandchild))
        .await
        .expect("the program's child runs");
    o.stop().await;
    wait_closed(port).await;
    wait_closed(grandchild).await;
    // stopping twice is fine
    o.stop().await;
}

/// A released outbound stops its program (M4 design 7.4: a reload that
/// replaces the policy).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_the_outbound_stops_the_program() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let child_port = dir.path().join("child-port");
    let o = outbound(
        "Ext",
        &args(port, &["--child", &child_port.to_string_lossy()]),
        port,
        dir.path(),
    );
    round_trip(&o, echo_server().await).await.unwrap();
    let grandchild: u16 = wait_for_file(&child_port).await.trim().parse().unwrap();
    drop(o);
    wait_closed(port).await;
    wait_closed(grandchild).await;
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-external-tests`
Expected: FAIL——`rurge_proto::external` 还不存在：

```text
error[E0432]: unresolved import `rurge_proto::external`
  --> tests\external\tests\common\mod.rs:10:22
error[E0433]: failed to resolve: could not find `external` in `rurge_proto`
  --> tests\external\tests\outbound.rs:91:18
Some errors have detailed explanations: E0432, E0433.
For more information about an error, try `rustc --explain E0432`.
error: could not compile `rurge-external-tests` (test "outbound") due to 2 previous errors
exit 101
```

- [ ] **Step 3: 实现（新模块自带用例）**

新建 `crates/rurge-proto/src/external.rs`：

```rust
//! `external` outbound (phase 2 M4 design §7): a program rurge starts itself
//! the first time the policy is used, reached as a SOCKS5 proxy at
//! `127.0.0.1:<local-port>`. A program that exited is started again on the
//! next use; the program and whatever it started are stopped together.

use crate::socks5::{connect_request, negotiate};
use crate::{Outbound, OutboundError};
use rurge_config::spec::ExternalSpec;
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Target};
use std::fs::{File, OpenOptions};
use std::io::{self, Write as _};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, oneshot};
use tokio::task::JoinHandle;
use tokio::time::Instant;

/// Connection attempts per request, one every `ATTEMPT_EVERY` (manual).
pub const ATTEMPTS: u32 = 6;
pub const ATTEMPT_EVERY: Duration = Duration::from_millis(500);
/// Two starts of one policy's program are at least this far apart (M4-D11).
pub const START_GAP: Duration = Duration::from_secs(2);
/// How long a program asked to end may take before it is killed.
pub const STOP_GRACE: Duration = Duration::from_secs(2);
/// A log larger than this is rotated before the next start.
pub const LOG_LIMIT: u64 = 1024 * 1024;

/// The proxy settings a program would otherwise follow back into rurge.
const PROXY_VARIABLES: [&str; 6] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
];

/// How external programs are started and stopped: the bin injects
/// `rurge-platform::process` (a process group on Unix, a Job Object on
/// Windows), everything else uses `NoProcessGroups`.
pub trait ProcessHook: Send + Sync {
    /// Called on the command before it is spawned.
    fn prepare(&self, command: &mut Command);
    /// Takes in the program just spawned, with process id `pid`. `None`:
    /// only the program itself can be stopped, not what it started.
    fn contain(&self, pid: u32) -> io::Result<Option<Box<dyn ProcessGroup>>>;
}

/// A program and whatever it started.
pub trait ProcessGroup: Send {
    /// Asks every process to end.
    fn terminate(&mut self) -> io::Result<()>;
    /// Ends every process now.
    fn kill(&mut self) -> io::Result<()>;
}

/// No process groups: stopping a program kills the program alone.
pub struct NoProcessGroups;

impl ProcessHook for NoProcessGroups {
    fn prepare(&self, _command: &mut Command) {}

    fn contain(&self, _pid: u32) -> io::Result<Option<Box<dyn ProcessGroup>>> {
        Ok(None)
    }
}

/// `<policy>.log`: characters a file name cannot hold become `_`, and a
/// name that had to change gets a short hash of the original, so two
/// policies never share a log.
pub fn log_file_name(policy: &str) -> String {
    let mut stem: String = policy
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    // Windows drops trailing dots and spaces; a leading dot hides the file
    let trimmed = stem.trim_end_matches(['.', ' ']).trim_start_matches('.');
    if trimmed.len() != stem.len() {
        stem = trimmed.to_string();
    }
    if stem != policy || stem.is_empty() || reserved_on_windows(&stem) {
        use sha2::Digest;
        let hash = sha2::Sha256::digest(policy.as_bytes());
        stem = format!(
            "{stem}-{:02x}{:02x}{:02x}{:02x}",
            hash[0], hash[1], hash[2], hash[3]
        );
    }
    format!("{stem}.log")
}

/// `CON`, `NUL`, `COM1` and the rest stay device names with an extension.
fn reserved_on_windows(stem: &str) -> bool {
    let base = stem.split('.').next().unwrap_or("").to_ascii_uppercase();
    matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((base.starts_with("COM") || base.starts_with("LPT"))
            && base.len() == 4
            && base.as_bytes()[3].is_ascii_digit())
}

/// Opens the log for one more start: a log over `LOG_LIMIT` becomes
/// `<name>.log.1` first (one old file is kept), then a separator line goes in.
fn open_log(path: &Path) -> io::Result<File> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if std::fs::metadata(path).is_ok_and(|m| m.len() > LOG_LIMIT) {
        let mut old = path.as_os_str().to_owned();
        old.push(".1");
        std::fs::rename(path, old)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    writeln!(
        file,
        "--- rurge: starting the program (unix time {now}) ---"
    )?;
    Ok(file)
}

/// The command for one start: the arguments in order, no input, output to
/// the log, and no proxy settings to follow back into rurge (M4-D11).
fn command(exec: &str, args: &[String], log: &File) -> io::Result<Command> {
    let mut command = Command::new(exec);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log.try_clone()?);
    for name in PROXY_VARIABLES {
        command.env_remove(name);
    }
    command.env("NO_PROXY", "*").env("no_proxy", "*");
    Ok(command)
}

/// A started program, watched by its own task.
struct Running {
    /// Dropped or sent: the task stops the program.
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

#[derive(Default)]
struct Slot {
    running: Option<Running>,
    /// When the program was last started (or failed to start).
    started: Option<Instant>,
    /// Why the last start failed, while `started` is recent.
    failed: Option<io::ErrorKind>,
}

pub struct ExternalOutbound {
    name: String,
    exec: String,
    args: Vec<String>,
    port: u16,
    log: PathBuf,
    hook: Arc<dyn ProcessHook>,
    slot: Mutex<Slot>,
}

fn start_failed(policy: &str, kind: io::ErrorKind) -> OutboundError {
    OutboundError::Proxy(format!("external: could not start {policy} ({kind})"))
}

impl ExternalOutbound {
    /// Nothing starts here: a build only checks (M4 design 7.5). The log
    /// goes to `log_dir`.
    pub fn new(
        name: &str,
        spec: &ExternalSpec,
        log_dir: &Path,
        hook: Arc<dyn ProcessHook>,
    ) -> ExternalOutbound {
        ExternalOutbound {
            name: name.to_string(),
            exec: spec.exec.clone(),
            args: spec.args.expose().clone(),
            port: spec.local_port,
            log: log_dir.join(log_file_name(name)),
            hook,
            slot: Mutex::new(Slot::default()),
        }
    }

    /// Where the program's output goes.
    pub fn log_path(&self) -> &Path {
        &self.log
    }

    /// Stops the program and whatever it started, when it runs; returns
    /// when they are gone (at most about `STOP_GRACE` later).
    pub async fn stop(&self) {
        let running = self.slot.lock().await.running.take();
        if let Some(Running { stop, task }) = running {
            let _ = stop.send(());
            let _ = task.await;
        }
    }

    /// Starts the program unless it runs. Within `START_GAP` of the last
    /// start nothing is started: a failed start is failed again, and a
    /// program that has exited since is left to the caller's retries.
    async fn ensure_started(&self) -> Result<(), OutboundError> {
        let mut slot = self.slot.lock().await;
        if slot
            .running
            .as_ref()
            .is_some_and(|running| !running.task.is_finished())
        {
            return Ok(());
        }
        slot.running = None;
        if let Some(at) = slot.started
            && at.elapsed() < START_GAP
        {
            return match slot.failed {
                Some(kind) => Err(start_failed(&self.name, kind)),
                None => Ok(()),
            };
        }
        slot.started = Some(Instant::now());
        match self.start() {
            Ok(running) => {
                slot.running = Some(running);
                slot.failed = None;
                Ok(())
            }
            Err(e) => {
                tracing::warn!(policy = %self.name, error = %e.kind(), "external: the program could not be started");
                slot.failed = Some(e.kind());
                Err(start_failed(&self.name, e.kind()))
            }
        }
    }

    fn start(&self) -> io::Result<Running> {
        let log = open_log(&self.log)?;
        let mut command = command(&self.exec, &self.args, &log)?;
        self.hook.prepare(&mut command);
        let mut command = tokio::process::Command::from(command);
        // the last resort, should the watching task be dropped unfinished
        command.kill_on_drop(true);
        let child = command.spawn()?;
        let pid = child.id().unwrap_or(0);
        // an error drops `child`, which kills it
        let group = self.hook.contain(pid)?;
        tracing::info!(policy = %self.name, pid, "external: the program started");
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(watch(self.name.clone(), pid, child, group, stopped));
        Ok(Running { stop, task })
    }

    async fn dial(&self, target: &Target) -> Result<BoxedStream, OutboundError> {
        // checked first: no program is started for a request we cannot send
        let request = connect_request(target)?;
        for attempt in 1..=ATTEMPTS {
            self.ensure_started().await?;
            // Windows takes about two seconds to refuse a connection to a
            // port nobody listens on: an attempt ends with its slot either way
            let slot_end = Instant::now() + ATTEMPT_EVERY;
            let connected = tokio::time::timeout_at(
                slot_end,
                TcpStream::connect((Ipv4Addr::LOCALHOST, self.port)),
            )
            .await;
            match connected {
                Ok(Ok(stream)) => {
                    let _ = stream.set_nodelay(true);
                    return negotiate(Box::new(stream), &request, None).await;
                }
                Ok(Err(e)) if e.kind() == io::ErrorKind::ConnectionRefused => {}
                Ok(Err(e)) => return Err(e.into()),
                Err(_) => {}
            }
            if attempt < ATTEMPTS {
                tokio::time::sleep_until(slot_end).await;
            }
        }
        Err(OutboundError::Proxy(
            "external: the local SOCKS5 port refused the connection".to_string(),
        ))
    }
}

/// Ends the group once, however the watching task ends: when it is dropped
/// unfinished (the runtime shutting down) too.
struct Tree(Option<Box<dyn ProcessGroup>>);

impl Tree {
    fn terminate(&mut self) {
        if let Some(group) = self.0.as_mut() {
            let _ = group.terminate();
        }
    }

    fn end(&mut self) {
        if let Some(mut group) = self.0.take() {
            let _ = group.kill();
        }
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        self.end();
    }
}

/// Owns the program until it exits or is told to stop. A program that
/// exits takes what it started with it: whatever is left of the group
/// would only hold the port the next start needs.
async fn watch(
    policy: String,
    pid: u32,
    mut child: tokio::process::Child,
    group: Option<Box<dyn ProcessGroup>>,
    stop: oneshot::Receiver<()>,
) {
    let mut tree = Tree(group);
    tokio::select! {
        status = child.wait() => {
            tree.end();
            let code = status.ok().and_then(|s| s.code());
            tracing::info!(policy = %policy, pid, code, "external: the program exited");
        }
        _ = stop => {
            if tree.0.is_some() {
                tree.terminate();
                if tokio::time::timeout(STOP_GRACE, child.wait()).await.is_err() {
                    tree.end();
                }
            }
            tree.end();
            let _ = child.kill().await;
            tracing::info!(policy = %policy, pid, "external: the program stopped");
        }
    }
}

impl Outbound for ExternalOutbound {
    fn name(&self) -> &str {
        &self.name
    }

    fn connect_tcp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            match tokio::time::timeout(opts.timeout, self.dial(target)).await {
                Ok(result) => result,
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_log_is_named_after_its_policy() {
        assert_eq!(log_file_name("Home SSH"), "Home SSH.log");
        assert_eq!(log_file_name("香港 01"), "香港 01.log");
        // a changed name carries a hash of the original: never shared
        let a = log_file_name("a/b");
        let b = log_file_name("a:b");
        assert!(a.starts_with("a_b-") && a.ends_with(".log"), "{a}");
        assert!(b.starts_with("a_b-") && b != a, "{b}");
        assert_ne!(log_file_name("a_b"), a);
        for odd in [
            "..", "x.", " ", "CON", "nul", "com1", "LPT9", "con.x", "\u{7}",
        ] {
            let name = log_file_name(odd);
            assert!(name.len() > 4 + 8, "{odd:?} → {name}");
            assert!(!name.starts_with('.'), "{odd:?} → {name}");
        }
        assert_eq!(log_file_name("console"), "console.log");
        assert_eq!(log_file_name("COM10"), "COM10.log");
    }

    #[test]
    fn a_log_is_rotated_once_it_is_over_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("external").join("P.log");
        drop(open_log(&path).unwrap());
        let first = std::fs::read_to_string(&path).unwrap();
        assert!(
            first.starts_with("--- rurge: starting the program (unix time "),
            "{first}"
        );
        assert_eq!(first.lines().count(), 1);
        // appended while it is small
        drop(open_log(&path).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 2);
        std::fs::write(&path, vec![b'x'; LOG_LIMIT as usize + 1]).unwrap();
        drop(open_log(&path).unwrap());
        let rotated = dir.path().join("external").join("P.log.1");
        assert_eq!(std::fs::metadata(&rotated).unwrap().len(), LOG_LIMIT + 1);
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 1);
        // one old file only: the next rotation replaces it
        std::fs::write(&path, vec![b'y'; LOG_LIMIT as usize + 2]).unwrap();
        drop(open_log(&path).unwrap());
        assert_eq!(std::fs::metadata(&rotated).unwrap().len(), LOG_LIMIT + 2);
    }

    #[test]
    fn the_program_gets_no_proxy_settings() {
        let dir = tempfile::tempdir().unwrap();
        let log = File::create(dir.path().join("l")).unwrap();
        let command = command("prog", &["-D".into(), "1080".into()], &log).unwrap();
        assert_eq!(command.get_program(), "prog");
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args, ["-D", "1080"]);
        let envs: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        for name in PROXY_VARIABLES {
            assert!(
                envs.iter()
                    .any(|(k, v)| k.eq_ignore_ascii_case(name) && v.is_none()),
                "{name}: {envs:?}"
            );
        }
        assert!(
            envs.iter()
                .any(|(k, v)| k.eq_ignore_ascii_case("NO_PROXY") && v.as_deref() == Some("*")),
            "{envs:?}"
        );
    }

    fn outbound(exec: &str, port: u16, dir: &Path) -> ExternalOutbound {
        let spec = ExternalSpec {
            exec: exec.to_string(),
            args: rurge_config::spec::Secret::new(Vec::new()),
            local_port: port,
            addresses: Vec::new(),
        };
        ExternalOutbound::new("P", &spec, dir, Arc::new(NoProcessGroups))
    }

    /// A program that cannot be started fails the request at once, and
    /// within `START_GAP` again without another try; the log was written.
    #[tokio::test]
    async fn a_program_that_cannot_start_fails_the_request() {
        let dir = tempfile::tempdir().unwrap();
        let o = outbound("./no-such-program-for-rurge", 9, dir.path());
        let target = Target::new(rurge_config::HostName::parse("t.test"), 80);
        let err = o
            .connect_tcp(&target, &ConnectOpts::default())
            .await
            .err()
            .unwrap();
        assert_eq!(
            err.to_string(),
            format!("external: could not start P ({})", io::ErrorKind::NotFound)
        );
        let at = o.slot.lock().await.started;
        let again = o
            .connect_tcp(&target, &ConnectOpts::default())
            .await
            .err()
            .unwrap();
        assert_eq!(again.to_string(), err.to_string());
        assert_eq!(
            o.slot.lock().await.started,
            at,
            "no second start within the gap"
        );
        assert!(o.log_path().exists());
    }

    /// Nothing starts for a request that cannot be sent.
    #[tokio::test]
    async fn an_unsendable_target_starts_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let o = outbound("./no-such-program-for-rurge", 9, dir.path());
        let target = Target::new(rurge_config::HostName::parse(&"a".repeat(300)), 80);
        let err = o
            .connect_tcp(&target, &ConnectOpts::default())
            .await
            .err()
            .unwrap();
        assert_eq!(
            err.to_string(),
            "socks5: the host name is longer than 255 bytes"
        );
        assert!(o.slot.lock().await.started.is_none());
        assert!(!o.log_path().exists());
    }
}
```

`crates/rurge-proto/src/lib.rs`——把

```rust
pub mod direct;
```

换成

```rust
pub mod direct;
pub mod external;
```

`crates/rurge-proto/Cargo.toml`——把

```toml
tokio.workspace = true
```

换成

```toml
tokio = { workspace = true, features = ["process"] }
```

`crates/rurge-proto/Cargo.toml`——把

```toml
rcgen.workspace = true
```

换成

```toml
rcgen.workspace = true
tempfile.workspace = true
```

SOCKS5 握手抽成 `negotiate`，`Socks5Outbound` 的行为不变：

`crates/rurge-proto/src/socks5.rs`——把

```rust
fn connect_request(target: &Target) -> Result<Vec<u8>, OutboundError> {
```

换成

```rust
/// The CONNECT request for `target`, built before any connection is opened.
pub(crate) fn connect_request(target: &Target) -> Result<Vec<u8>, OutboundError> {
```

`crates/rurge-proto/src/socks5.rs`——把

```rust
        let mut stream = self.stack.open(opts).await?;
        let offered: &[u8] = if self.credentials.is_some() {
            &[NO_AUTH, USER_PASS]
        } else {
            &[NO_AUTH]
        };
        let mut greeting = vec![VERSION, offered.len() as u8];
        greeting.extend_from_slice(offered);
        stream.write_all(&greeting).await.map_err(handshake_io)?;
        let mut selected = [0u8; 2];
        stream
            .read_exact(&mut selected)
            .await
            .map_err(handshake_io)?;
        match (selected[1], &self.credentials) {
            (NO_ACCEPTABLE, _) => {
                return Err(proxy(
                    "the proxy accepts none of the offered authentication methods",
                ));
            }
            (method, _) if !offered.contains(&method) => {
                return Err(proxy(format!(
                    "the proxy selected authentication method {method}, which was not offered"
                )));
            }
            (USER_PASS, Some((user, password))) => {
                // lengths were checked in from_spec (<= 255 bytes each)
                let mut auth = vec![1, user.len() as u8];
                auth.extend_from_slice(user.as_bytes());
                auth.push(password.len() as u8);
                auth.extend_from_slice(password.as_bytes());
                stream.write_all(&auth).await.map_err(handshake_io)?;
                let mut status = [0u8; 2];
                stream.read_exact(&mut status).await.map_err(handshake_io)?;
                if status[1] != 0 {
                    return Err(proxy("authentication failed"));
                }
            }
            _ => {}
        }
        stream.write_all(&request).await.map_err(handshake_io)?;
        let mut reply = [0u8; 4];
        stream.read_exact(&mut reply).await.map_err(handshake_io)?;
        if reply[1] != 0 {
            return Err(proxy(reply_text(reply[1])));
        }
        // skip the bound address
        let remaining = match reply[3] {
            1 => 4 + 2,
            4 => 16 + 2,
            3 => {
                let mut len = [0u8; 1];
                stream.read_exact(&mut len).await.map_err(handshake_io)?;
                usize::from(len[0]) + 2
            }
            other => return Err(proxy(format!("unknown address type {other} in the reply"))),
        };
        let mut bound = vec![0u8; remaining];
        stream.read_exact(&mut bound).await.map_err(handshake_io)?;
        Ok(stream)
    }
```

换成

```rust
        let stream = self.stack.open(opts).await?;
        negotiate(stream, &request, self.credentials.as_ref()).await
    }
}

/// The SOCKS5 handshake on `stream`, a connection to the proxy: method
/// selection, the user name and password when there are any, then
/// `request` (from `connect_request`). The stream carries the tunnel after.
pub(crate) async fn negotiate(
    mut stream: BoxedStream,
    request: &[u8],
    credentials: Option<&(String, String)>,
) -> Result<BoxedStream, OutboundError> {
    let offered: &[u8] = if credentials.is_some() {
        &[NO_AUTH, USER_PASS]
    } else {
        &[NO_AUTH]
    };
    let mut greeting = vec![VERSION, offered.len() as u8];
    greeting.extend_from_slice(offered);
    stream.write_all(&greeting).await.map_err(handshake_io)?;
    let mut selected = [0u8; 2];
    stream
        .read_exact(&mut selected)
        .await
        .map_err(handshake_io)?;
    match (selected[1], credentials) {
        (NO_ACCEPTABLE, _) => {
            return Err(proxy(
                "the proxy accepts none of the offered authentication methods",
            ));
        }
        (method, _) if !offered.contains(&method) => {
            return Err(proxy(format!(
                "the proxy selected authentication method {method}, which was not offered"
            )));
        }
        (USER_PASS, Some((user, password))) => {
            // lengths were checked in from_spec (<= 255 bytes each)
            let mut auth = vec![1, user.len() as u8];
            auth.extend_from_slice(user.as_bytes());
            auth.push(password.len() as u8);
            auth.extend_from_slice(password.as_bytes());
            stream.write_all(&auth).await.map_err(handshake_io)?;
            let mut status = [0u8; 2];
            stream.read_exact(&mut status).await.map_err(handshake_io)?;
            if status[1] != 0 {
                return Err(proxy("authentication failed"));
            }
        }
        _ => {}
    }
    stream.write_all(request).await.map_err(handshake_io)?;
    let mut reply = [0u8; 4];
    stream.read_exact(&mut reply).await.map_err(handshake_io)?;
    if reply[1] != 0 {
        return Err(proxy(reply_text(reply[1])));
    }
    // skip the bound address
    let remaining = match reply[3] {
        1 => 4 + 2,
        4 => 16 + 2,
        3 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).await.map_err(handshake_io)?;
            usize::from(len[0]) + 2
        }
        other => return Err(proxy(format!("unknown address type {other} in the reply"))),
    };
    let mut bound = vec![0u8; remaining];
    stream.read_exact(&mut bound).await.map_err(handshake_io)?;
    Ok(stream)
```

要点：
- 锁：`slot`（tokio 的 `Mutex`）只包住"看一眼程序还在不在、需要时拉起"（`ensure_started`），连接与握手都在锁外；拉起本身（开日志、`spawn`、`contain`）是同步的、很快。
- `dial` 先构造 CONNECT 请求（送不出去的目标名不拉起任何程序），再做最多 `ATTEMPTS` 次：确保程序在跑 → 以本次的 500 ms 为限连本机端口 → 连上就握手返回；被拒或到时就等到这 500 ms 结束再试；其它连接错误直接返回。整次拨号受 `opts.timeout` 约束。
- 拉起失败：记 `warn`（只有错误种类），在 `START_GAP` 内的请求直接得到同一个 `external: could not start <策略名> (<种类>)`；`contain` 失败时刚拉起的程序随 `Child` 被丢弃而结束（`kill_on_drop`）。
- 看守任务（`watch`）持有 `Child` 与进程组（包在 `Tree` 里，`Drop` 时强制结束组）；`stop()` 与丢弃出站都经 oneshot 通知它（P6）。
- 辅助程序的 `--serve <n>` 一次只服务一个客户端，只数"完成了 CONNECT 的会话"：Windows 上被 500 ms 时限放弃的连接尝试也可能在辅助程序开始监听的一瞬间被接下，不能把它算作一次服务（写计划时由此出现过偶发失败）。`--child` 在开始监听之后才拉起子进程（P1 的已知限制）。
- `tests/external/src/bin/socks-helper.rs` 里的 `start_child` 标了 `#[allow(clippy::zombie_processes)]`：那个子进程故意一直运行，只由结束整棵树来结束。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-proto` → 167 passed（新增 `external::tests` 5 条：`a_log_is_named_after_its_policy`、`a_log_is_rotated_once_it_is_over_the_limit`、`the_program_gets_no_proxy_settings`、`a_program_that_cannot_start_fails_the_request`、`an_unsendable_target_starts_nothing`；`socks5` 的既有用例照旧通过）。
Run: `cargo test -p rurge-external-tests` → 5 passed（`the_first_use_starts_the_program`、`a_program_that_exited_is_started_again`、`a_port_that_never_opens_fails_the_request`、`stopping_ends_the_whole_tree`、`dropping_the_outbound_stops_the_program`），约 4 秒；连跑几次都应通过。
Run: `tasklist | findstr /i socks-helper`（Unix：`pgrep socks-helper`）→ 没有残留。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add Cargo.toml Cargo.lock crates/rurge-proto tests/external
git commit -m "feat(proto): external 出站——按需拉起、日志轮转、再拉起与间隔、本机端口的重试、连同子进程一起停止；tests/external 测试辅助程序"
```


### Task 4: 接入——`ProtoSpec::External`、引擎工厂、外部程序的名单与停止

把 `external` 接进配置与引擎（P8、P13）：`ProtoSpec::External` 与 `to_spec` 的分支（`tfo` 只报不适用；`addresses`、`udp-relay` 写了才报 `W0029`，P9）；两个 `external` 策略写同一个 `local-port` 是第二个的 `E0018`（设计 4.4）；Shadow TLS 不能叠在 `external` 上（P9）。引擎：`EngineShared` 带上进程控制（`processes`，默认 `NoProcessGroups`）与"构建过的 external 出站"名单（`externals`，弱引用，跨代次）；工厂的 `External` 分支构建 `ExternalOutbound`（日志目录是数据目录下的 `external`），真实构建登记进名单，干构建从不登记也从不拉起；`Engine::stop_external_programs()` 给退出流程用（Task 5）。重载按指纹沿用出站，程序随之沿用；被替换的出站释放时它的程序停掉（P6）。

**Files:**
- Create: `tests/external/tests/engine.rs`（经引擎的用例）
- Modify: `crates/rurge-config/src/spec/mod.rs`（`ProtoSpec::External`、`to_spec` 分支，与用例）、`src/config.rs`（重复的 `local-port`，与用例）、`src/spec/shadow_tls.rs`（`allowed_on`，与用例）、`crates/rurge-config/tests/snapshots/corpus__corpus__kitchen-sink.snap`
- Modify: `crates/rurge-engine/src/shared.rs`（`ExternalPrograms`、`EngineShared.processes` / `externals`）、`src/outbounds.rs`（`with_externals`、`External` 分支）、`src/runtime.rs`（工厂拿到数据目录与名单）、`src/engine.rs`（`stop_external_programs`）、`src/lib.rs`（导出 `ExternalPrograms`）
- Modify: `tests/external/Cargo.toml`（开发依赖 `rurge-dns`、`rurge-engine`、`rurge-rules`）

**Interfaces:**
- Consumes: Task 1 的 `read_external`、`ExternalSpec`；Task 3 的 `ExternalOutbound`、`ProcessHook`、`NoProcessGroups`、`ExternalOutbound::stop`。
- Produces:
  - `rurge_config::spec::ProtoSpec::External(ExternalSpec)`（`tls()` 为 `None`，`keystore_item()` 为 `None`）
  - `rurge_engine::ExternalPrograms`（`Default`；`pub async fn stop_all(&self)`；登记是 `pub(crate) fn add`）
  - `EngineShared.processes: Arc<dyn rurge_proto::external::ProcessHook>`、`EngineShared.externals: Arc<ExternalPrograms>`
  - `EngineFactory::with_externals(self, processes: Arc<dyn ProcessHook>, logs: PathBuf, externals: Arc<ExternalPrograms>) -> EngineFactory`
  - `Engine::stop_external_programs(&self)`（async）

- [ ] **Step 1: 先写用例**

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            "policy `W`: Shadow TLS cannot be combined with a `wireguard` policy"
```

换成

```rust
            "policy `W`: Shadow TLS cannot be combined with a `wireguard` policy"
        );
    }

    /// An `external` line: no server, no port; what does not work yet is said
    /// once per load (`W0029`), and `tfo` only as not applicable (M4 design 4.4).
    #[test]
    fn an_external_line() {
        let o = outcome(
            "X",
            "external, exec=/usr/bin/ssh, args=-D, args=1080, local-port=1080, addresses=10.0.0.1, udp-relay=true, tfo=true, ecn=on",
        );
        let spec = o.spec.expect("an external spec");
        let ProtoSpec::External(external) = &spec.proto else {
            panic!("{:?}", spec.proto);
        };
        assert_eq!(external.args.expose(), &["-D", "1080"]);
        assert_eq!((spec.server, spec.port), (None, None));
        assert_eq!(o.inert, ["ecn", "addresses", "udp-relay"]);
        assert_eq!(spec.proto.keystore_item(), None);
        let o = outcome(
            "X",
            "external, exec=/bin/p, local-port=1080, shadow-tls-password=pw",
        );
        assert!(o.spec.is_none());
        assert_eq!(
            o.diagnostics[0].message,
            "policy `X`: Shadow TLS cannot be combined with a `external` policy"
```

`crates/rurge-config/src/config.rs`——把

```rust
        assert!(loaded.config.spec("Old1").is_none() && loaded.config.spec("Old2").is_none());
    }

    const WG_PRIVATE: &str = "yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=";
```

换成

```rust
        assert!(loaded.config.spec("Old1").is_none() && loaded.config.spec("Old2").is_none());
    }

    /// Two `external` policies on one `local-port`: the second is an error
    /// at its own line (M4 design 4.4).
    #[test]
    fn two_external_policies_cannot_share_a_local_port() {
        let loaded = load_text(
            "[Proxy]\nA = external, exec=/bin/a, local-port=1080\nB = external, exec=/bin/b, local-port=1081\n\
C = external, exec=/bin/c, local-port=1080\n[Rule]\nFINAL,DIRECT\n",
        );
        let errors: Vec<&Diagnostic> = loaded
            .diagnostics
            .iter()
            .filter(|d| d.code == codes::E_INVALID_POLICY_PARAM)
            .collect();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(
            errors[0].message,
            "policy `C`: `local-port` 1080 is also the `local-port` of policy `A`"
        );
        assert_eq!(errors[0].span.as_ref().map(|s| s.line), Some(4));
        assert!(loaded.config.spec("A").is_some() && loaded.config.spec("B").is_some());
        assert!(loaded.config.spec("C").is_none());
    }

    const WG_PRIVATE: &str = "yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=";
```

`crates/rurge-config/src/spec/shadow_tls.rs`——把

```rust
        for kind in [Tuic, TuicV5, Hysteria2, Masque, WireGuard, Tailscale] {
```

换成

```rust
        for kind in [
            Tuic, TuicV5, Hysteria2, Masque, WireGuard, Tailscale, External,
        ] {
```

`tests/external/Cargo.toml`——把

```toml
rurge-config.workspace = true
```

换成

```toml
rurge-config.workspace = true
rurge-dns.workspace = true
rurge-engine.workspace = true
```

`tests/external/Cargo.toml`——把

```toml
rurge-proto.workspace = true
```

换成

```toml
rurge-proto.workspace = true
rurge-rules.workspace = true
```

新建 `tests/external/tests/engine.rs`：

```rust
//! `external` policies through the engine (phase 2 M4 design 7.4, 7.5,
//! 8.2): the program starts on the first dial, never on a check or a build;
//! a reload keeps an unchanged program; the exit flow stops them all.

mod common;
use common::*;
use rurge_config::config::{LoadOptions, from_text};
use rurge_dns::system::StaticSystemDns;
use rurge_engine::stack::StackOptions;
use rurge_engine::{Engine, EngineShared, Runtime, RuntimeOptions};
use rurge_rules::{GeoUrls, OutboundMode};

/// `[General]` for every test: loopback listeners, nothing tested online.
const GENERAL: &str = "[General]\nhttp-listen = 127.0.0.1:0\nipv6 = false\n\
proxy-test-url = http://127.0.0.1:9/\ninternet-test-url = http://127.0.0.1:9/\n";

/// A profile sending everything to `Ext`, the helper with `extra` arguments
/// on `port`.
fn profile(port: u16, extra: &str) -> String {
    format!(
        "{GENERAL}[Proxy]\nExt = external, exec = \"{}\", args = --port, args = {port}{extra}, local-port = {port}\n\
[Rule]\nFINAL,Ext\n",
        helper().replace('\\', "\\\\")
    )
}

fn shared() -> EngineShared {
    EngineShared {
        processes: Arc::new(Platform),
        ..EngineShared::default()
    }
}

async fn runtime(dir: &Path, text: &str, shared: EngineShared) -> Runtime {
    let path = dir.join("t.conf");
    std::fs::write(&path, text).unwrap();
    let loaded = from_text(text, &path, &LoadOptions::for_tests());
    assert!(
        !loaded.diagnostics.has_errors(),
        "{:?}",
        loaded
            .diagnostics
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
    );
    Runtime::build(
        loaded.config,
        RuntimeOptions {
            stack: StackOptions {
                data_dir: dir.to_path_buf(),
                no_network: true,
                geo_urls: GeoUrls::default(),
                dns_cache_size: 100,
                system: Arc::new(StaticSystemDns::default()),
                wait: Duration::ZERO,
                dns_connector: None,
                socket_hook: Arc::new(rurge_net::socket::NoopSocketHook),
            },
            outbound_mode: OutboundMode::Rule,
            idle_timeout: Duration::from_secs(600),
            shared,
            request_log_size: 100,
        },
    )
    .await
    .unwrap()
}

struct Harness {
    dir: tempfile::TempDir,
    engine: Arc<Engine>,
    http: SocketAddr,
    _listeners: Vec<(rurge_engine::ListenerSpec, rurge_engine::Running)>,
}

async fn harness(text: &str) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new(runtime(dir.path(), text, shared()).await);
    let listeners = engine.bind_listeners().await.unwrap();
    let http = listeners[0].1.local_addr;
    Harness {
        dir,
        engine,
        http,
        _listeners: listeners,
    }
}

/// `CONNECT` through rurge's HTTP listener to `echo`, one round trip.
async fn echo_via(proxy: SocketAddr, echo: SocketAddr) {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    s.write_all(format!("CONNECT {echo} HTTP/1.1\r\nHost: {echo}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        let n = tokio::time::timeout(Duration::from_secs(10), s.read(&mut byte))
            .await
            .expect("the proxy answers")
            .unwrap();
        assert!(n > 0, "closed: {:?}", String::from_utf8_lossy(&head));
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    s.write_all(b"ping").await.unwrap();
    let mut buf = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut buf))
        .await
        .expect("the echo comes back")
        .unwrap();
    assert_eq!(&buf, b"ping");
}

/// The program starts on the first dial; its log is in the data directory.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_first_dial_starts_the_program() {
    let port = free_port();
    let h = harness(&profile(port, "")).await;
    echo_via(h.http, echo_server().await).await;
    let log = std::fs::read_to_string(h.dir.path().join("external").join("Ext.log")).unwrap();
    assert!(
        log.contains(&format!("socks-helper listening on 127.0.0.1:{port}")),
        "{log}"
    );
    h.engine.stop_external_programs().await;
    wait_closed(port).await;
}

/// A check and a build only check: nothing starts (M4 design 7.5).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_check_or_a_build_starts_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let record = dir.path().join("record");
    let text = profile(
        port,
        &format!(
            ", args = --record, args = \"{}\"",
            record.to_string_lossy().replace('\\', "\\\\")
        ),
    );
    let path = dir.path().join("t.conf");
    std::fs::write(&path, &text).unwrap();
    let loaded = rurge_engine::load_checked(&path, &LoadOptions::for_tests()).unwrap();
    assert!(!loaded.diagnostics.has_errors());
    let engine = Engine::new(runtime(dir.path(), &text, shared()).await);
    assert_eq!(engine.registry().names().len(), 1);
    // give a start that should not happen the time to show
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!record.exists(), "a program was started");
}

/// A reload that leaves the line alone keeps the program; one that
/// changes it starts the new program and stops the old one once the old
/// generation is gone (M4 design 7.4).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reload_keeps_an_unchanged_program_and_stops_a_replaced_one() {
    let port = free_port();
    let dir = tempfile::tempdir().unwrap();
    let record = dir.path().join("record");
    let extra = format!(
        ", args = --record, args = \"{}\"",
        record.to_string_lossy().replace('\\', "\\\\")
    );
    let h = harness(&profile(port, &extra)).await;
    let echo = echo_server().await;
    echo_via(h.http, echo).await;

    let unrelated = format!("{}\n[Host]\nx.test = 127.0.0.1\n", profile(port, &extra));
    h.engine
        .swap_runtime(runtime(h.dir.path(), &unrelated, h.engine.shared()).await);
    echo_via(h.http, echo).await;
    let recorded = std::fs::read_to_string(&record).unwrap();
    assert_eq!(recorded.lines().count(), 1, "the same program: {recorded}");

    let other = free_port();
    h.engine
        .swap_runtime(runtime(h.dir.path(), &profile(other, &extra), h.engine.shared()).await);
    echo_via(h.http, echo).await;
    wait_closed(port).await;
    let recorded = std::fs::read_to_string(&record).unwrap();
    assert_eq!(recorded.lines().count(), 2, "{recorded}");
    h.engine.stop_external_programs().await;
    wait_closed(other).await;
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-external-tests --test engine`
Expected: FAIL——`EngineShared` 还没有进程控制，引擎也还不会停外部程序（`rurge-config` 的新用例此时编译不过：`ProtoSpec::External` 还没有）：

```text
error[E0560]: struct `EngineShared` has no field named `processes`
  --> tests\external\tests\engine.rs:29:9
error[E0599]: no method named `stop_external_programs` found for struct `std::sync::Arc<Engine>` in the current scope
   --> tests\external\tests\engine.rs:128:14
error[E0599]: no method named `stop_external_programs` found for struct `std::sync::Arc<Engine>` in the current scope
   --> tests\external\tests\engine.rs:186:14
Some errors have detailed explanations: E0560, E0599.
For more information about an error, try `rustc --explain E0560`.
error: could not compile `rurge-external-tests` (test "engine") due to 3 previous errors
exit 101
```

- [ ] **Step 3: 实现**

配置层：

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    WireGuard(WireGuardSpec),
```

换成

```rust
    WireGuard(WireGuardSpec),
    External(ExternalSpec),
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            | ProtoSpec::WireGuard(_) => None,
```

换成

```rust
            | ProtoSpec::WireGuard(_)
            | ProtoSpec::External(_) => None,
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            (common, ProtoSpec::WireGuard(wireguard))
```

换成

```rust
            (common, ProtoSpec::WireGuard(wireguard))
        }
        PolicyKind::External => {
            let mut common = read_common(&mut r, Applies::Proxy, &mut notes);
            let external = external::read_external(&mut r, &mut common);
            // `tfo` does not apply at all (`W0028`): no second word on it
            notes.inert.retain(|name| *name != "tfo");
            if !external.addresses.is_empty() {
                notes.inert.push("addresses");
            }
            if r.bool("udp-relay").unwrap_or(false) {
                notes.inert.push("udp-relay");
            }
            (common, ProtoSpec::External(external))
```

`crates/rurge-config/src/config.rs`——把

```rust
use crate::spec::{GroupSpec, NameKind, PolicySpec, SpecEnv, to_group_spec, to_spec};
```

换成

```rust
use crate::spec::{GroupSpec, NameKind, PolicySpec, ProtoSpec, SpecEnv, to_group_spec, to_spec};
```

`crates/rurge-config/src/config.rs`——把

```rust
        for p in &cfg.policies {
            let outcome = to_spec(p, &env);
            for d in outcome.diagnostics {
                diags.push(d);
```

换成

```rust
        // `local-port` → the `external` policy that has it
        let mut local_ports: HashMap<u16, &str> = HashMap::new();
        for p in &cfg.policies {
            let mut outcome = to_spec(p, &env);
            for d in outcome.diagnostics {
                diags.push(d);
            }
            // two programs cannot listen on one port: one policy's
            // connections would reach the other's program (M4 design 4.4)
            if let Some(PolicySpec {
                proto: ProtoSpec::External(external),
                ..
            }) = &outcome.spec
            {
                match local_ports.get(&external.local_port) {
                    Some(first) => {
                        diags.push(
                            Diagnostic::error(
                                codes::E_INVALID_POLICY_PARAM,
                                format!(
                                    "policy `{}`: `local-port` {} is also the `local-port` of policy `{first}`",
                                    p.name, external.local_port
                                ),
                            )
                            .at(p.span.clone()),
                        );
                        outcome.spec = None;
                    }
                    None => {
                        local_ports.insert(external.local_port, p.name.as_str());
                    }
                }
```

`crates/rurge-config/src/spec/shadow_tls.rs`——把

```rust
/// (manual: a configuration error).
```

换成

```rust
/// (manual: a configuration error), and so is `external`, whose connection
/// goes to a program on this machine.
```

`crates/rurge-config/src/spec/shadow_tls.rs`——把

```rust
            | PolicyKind::Tailscale
```

换成

```rust
            | PolicyKind::Tailscale
            | PolicyKind::External
```

kitchen-sink 语料里的 `external` 行现在有了 spec，它的 `addresses` 多报一条 `W0029`：

`crates/rurge-config/tests/snapshots/corpus__corpus__kitchen-sink.snap`——把

```text
  - "warning[W0029] valid/kitchen-sink.conf:69: policy parameter `ecn` is parsed but has no effect in this version"
```

换成

```text
  - "warning[W0029] valid/kitchen-sink.conf:69: policy parameter `ecn` is parsed but has no effect in this version"
  - "warning[W0029] valid/kitchen-sink.conf:71: policy parameter `addresses` is parsed but has no effect in this version"
```

引擎：

`crates/rurge-engine/src/shared.rs`——把

```rust
use rurge_policy::{EmptyGroup, GroupSelections, RegistryCell, SelectionTable};
```

换成

```rust
use rurge_policy::{EmptyGroup, GroupSelections, RegistryCell, SelectionTable};
use rurge_proto::external::{ExternalOutbound, NoProcessGroups, ProcessHook};
```

`crates/rurge-engine/src/shared.rs`——把

```rust
use std::sync::Arc;
```

换成

```rust
use std::sync::{Arc, Mutex, Weak};
```

`crates/rurge-engine/src/shared.rs`——把

```rust
        })
    }
}

/// Created once per engine — before the first `Runtime::build`, because the
```

换成

```rust
        })
    }
}

/// Every `external` outbound built for the engine that still exists, in
/// whatever generation: the exit flow stops their programs (phase 2 M4
/// design 8.2).
#[derive(Default)]
pub struct ExternalPrograms(Mutex<Vec<Weak<ExternalOutbound>>>);

impl ExternalPrograms {
    pub(crate) fn add(&self, outbound: &Arc<ExternalOutbound>) {
        let mut list = self.0.lock().expect("external program list");
        list.retain(|o| o.strong_count() > 0);
        list.push(Arc::downgrade(outbound));
    }

    /// Stops every program still running, all at once; returns when they
    /// are gone.
    pub async fn stop_all(&self) {
        let live: Vec<Arc<ExternalOutbound>> = self
            .0
            .lock()
            .expect("external program list")
            .iter()
            .filter_map(Weak::upgrade)
            .collect();
        let stops: Vec<_> = live
            .iter()
            .map(|o| {
                tokio::spawn({
                    let o = o.clone();
                    async move { o.stop().await }
                })
            })
            .collect();
        for stop in stops {
            let _ = stop.await;
        }
    }
}

/// Created once per engine — before the first `Runtime::build`, because the
```

`crates/rurge-engine/src/shared.rs`——把

```rust
    pub auto: Arc<AutoGroups>,
```

换成

```rust
    pub auto: Arc<AutoGroups>,
    /// How `external` programs are started and stopped: the bin injects
    /// `rurge-platform::process`; everything else uses `NoProcessGroups`.
    pub processes: Arc<dyn ProcessHook>,
    /// The `external` outbounds built so far.
    pub externals: Arc<ExternalPrograms>,
```

`crates/rurge-engine/src/shared.rs`——把

```rust
            auto: Arc::new(AutoGroups::new(Arc::new(TestBook::new()))),
```

换成

```rust
            auto: Arc::new(AutoGroups::new(Arc::new(TestBook::new()))),
            processes: Arc::new(NoProcessGroups),
            externals: Arc::new(ExternalPrograms::default()),
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
//! policy which cannot be built into a load error (M1 design 6.1, 6.4).

use rurge_config::config::{LoadError, LoadOptions, Loaded, load};
```

换成

```rust
//! policy which cannot be built into a load error (M1 design 6.1, 6.4).

use crate::shared::ExternalPrograms;
use rurge_config::config::{LoadError, LoadOptions, Loaded, load};
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
use rurge_proto::build::server_of;
```

换成

```rust
use rurge_proto::build::server_of;
use rurge_proto::external::{ExternalOutbound, NoProcessGroups, ProcessHook};
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
use std::net::IpAddr;
use std::path::Path;
```

换成

```rust
use std::net::IpAddr;
use std::path::{Path, PathBuf};
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
    dry: bool,
```

换成

```rust
    dry: bool,
    /// How `external` programs are started, where they log, and the list
    /// the exit flow stops them from (`with_externals`).
    processes: Arc<dyn ProcessHook>,
    external_logs: PathBuf,
    externals: Arc<ExternalPrograms>,
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
            dry: false,
        }
```

换成

```rust
            dry: false,
            processes: Arc::new(NoProcessGroups),
            external_logs: PathBuf::from("external"),
            externals: Arc::new(ExternalPrograms::default()),
        }
    }

    /// `external` programs start through `processes`, log into `logs` (the
    /// data directory's `external`) and are listed in `externals`.
    pub fn with_externals(
        mut self,
        processes: Arc<dyn ProcessHook>,
        logs: PathBuf,
        externals: Arc<ExternalPrograms>,
    ) -> EngineFactory {
        self.processes = processes;
        self.external_logs = logs;
        self.externals = externals;
        self
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
            dry: true,
```

换成

```rust
            dry: true,
            // a build starts nothing (M4 design 7.5)
            processes: Arc::new(NoProcessGroups),
            external_logs: PathBuf::from("external"),
            externals: Arc::new(ExternalPrograms::default()),
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
            ),
```

换成

```rust
            ),
            // nothing starts here: the program starts on the first dial
            ProtoSpec::External(external) => {
                let outbound = Arc::new(ExternalOutbound::new(
                    &spec.name,
                    external,
                    &self.external_logs,
                    self.processes.clone(),
                ));
                if !self.dry {
                    self.externals.add(&outbound);
                }
                outbound
            }
```

`crates/rurge-engine/src/runtime.rs`——把

```rust
        let factory = Arc::new(match &opts.shared.roots {
```

换成

```rust
        let factory = match &opts.shared.roots {
```

`crates/rurge-engine/src/runtime.rs`——把

```rust
        });
```

换成

```rust
        };
        let factory = Arc::new(factory.with_externals(
            opts.shared.processes.clone(),
            opts.stack.data_dir.join("external"),
            opts.shared.externals.clone(),
        ));
```

`crates/rurge-engine/src/engine.rs`——把

```rust
        self.shared.clone()
```

换成

```rust
        self.shared.clone()
    }

    /// Stops every `external` program still running, whatever generation
    /// started it (phase 2 M4 design 8.2): the exit flow calls this before
    /// the runtime ends, rather than trust what exiting would drop.
    pub async fn stop_external_programs(&self) {
        self.shared.externals.stop_all().await;
```

`crates/rurge-engine/src/lib.rs`——把

```rust
pub use shared::{EngineShared, ResolverCell};
```

换成

```rust
pub use shared::{EngineShared, ExternalPrograms, ResolverCell};
```

要点：
- 重复端口的检查在 `config.rs` 逐条读 spec 的循环里：先到的策略占住端口，后来的那条报 `E0018`（`` policy `C`: `local-port` 1080 is also the `local-port` of policy `A` ``）并且没有 spec（按 REJECT）。
- 名单只存弱引用：旧代次的出站在它的会话都结束后照常释放（程序随之停掉），名单里留下的空引用在下一次登记时清掉。`stop_all` 同时停掉全部还活着的程序（每个最多约 `STOP_GRACE`）。
- `EngineFactory::new` / `with_roots` / 干构建的工厂都用 `NoProcessGroups` 与一个不会被用到的日志目录；只有 `Runtime::build` 经 `with_externals` 给出真的。
- 经引擎的用例用 `Platform`（`rurge-platform::process`）做进程分组；`a_check_or_a_build_starts_nothing` 在构建之后等 300 ms 只为断言"什么也没发生"。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-config` → 通过（`--lib` 207 条，新增 `an_external_line`、`two_external_policies_cannot_share_a_local_port`；`--test corpus` 2 条）。
Run: `cargo test -p rurge-external-tests` → `engine` 3 passed（`the_first_dial_starts_the_program`、`a_check_or_a_build_starts_nothing`、`a_reload_keeps_an_unchanged_program_and_stops_a_replaced_one`），`outbound` 5 passed。

- [ ] **Step 5: 门禁与提交**

跑门禁（51 个测试二进制，1162 通过 / 2 忽略）。

```bash
git add crates/rurge-config crates/rurge-engine tests/external
git commit -m "feat(engine): external 策略接入——ProtoSpec::External、重复 local-port、工厂分支、外部程序的名单与 stop_external_programs"
```


### Task 5: bin——`PlatformProcesses`、退出时停掉外部程序、能力表翻转 `external`；文档（承接 C2）

bin 把 `rurge-platform::process` 接到 `ProcessHook` 上（`PlatformProcesses`，同 `PlatformSockets`），`rurge run` 把它放进 `EngineShared`；退出流程在会话都结束（或宽限期过去）之后，最后调用 `engine.stop_external_programs()`（P3，设计 7.3、8.2）。翻转前核对设计承诺的行为都已存在（设计 8.4）：按需拉起与转发、再拉起与间隔、日志、环境、进程树的停止、重载、脱敏与订阅安全门——Task 1 ～ 4 的用例都覆盖了。然后能力表翻转 `external`，`W0007` 不再因它出现。文档：兼容性清单登记差异（设计第 12 节）、两份 README、`CLAUDE.md`（含 unsafe 规则的第二个例外，C2）、API 文档的 `args` 脱敏、手工验收的 M4c 一节、`tests/external` 的说明。

**Files:**
- Create: `tests/external/README.md`
- Modify: `crates/rurge/Cargo.toml`（依赖 `rurge-proto`）、`src/cli/runtime.rs`（`PlatformProcesses`，与用例）、`src/cli/run.rs`（注入与退出流程）、`src/capabilities.rs`（`External`）、`tests/cli.rs`（两条用例）
- Modify: `CLAUDE.md`、`README.md`、`README_en.md`、`docs/surge-compatibility-matrix.md`、`docs/api/phase2.md`、`docs/acceptance/phase2-manual.md`

**Interfaces:**
- Consumes: Task 2 的 `rurge_platform::process::{prepare, ProcessTree}`；Task 3 的 `ProcessHook`、`ProcessGroup`；Task 4 的 `EngineShared.processes`、`Engine::stop_external_programs`。
- Produces: `crate::cli::runtime::PlatformProcesses`（bin 内部）；能力表含 `PolicyKind::External`。

- [ ] **Step 1: 先写用例**

`crates/rurge/tests/cli.rs`——把

```rust
            "bad.conf:3: policy `W`: `section-name` names `[WireGuard home]`, which does not exist or has errors",
        ))
        .stdout(predicate::str::contains("c2VjcmV0").not());
}

const SUBSCRIBED: &str = "[General]\n[Proxy Group]\nLocal = select, DIRECT, policy-path=nodes.txt\n\
```

换成

```rust
            "bad.conf:3: policy `W`: `section-name` names `[WireGuard home]`, which does not exist or has errors",
        ))
        .stdout(predicate::str::contains("c2VjcmV0").not());
}

const EXTERNAL: &str = "[General]\n[Proxy]\n\
X = external, exec = \"/usr/bin/sshpass\", args = -p, args = hunter2, args = ssh, local-port = 1080\n\
Old = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\n[Rule]\nFINAL,DIRECT\n";
const EXTERNAL_SAME_PORT: &str = "[General]\n[Proxy]\n\
X = external, exec = /bin/x, local-port = 1080\nY = external, exec = /bin/y, local-port = 1080\n\
[Rule]\nFINAL,DIRECT\n";

/// `rurge check` knows `external` policies and starts nothing; two on one
/// `local-port` are an error at the second (M4 design 4.4, 7.5).
#[test]
fn check_knows_external() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "x.conf", EXTERNAL))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    // `ss` is still a later milestone; `external` is not
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(out.contains("`ss`") && !out.contains("`external`"), "{out}");
    assert!(!out.contains("hunter2"), "{out}");

    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "same.conf", EXTERNAL_SAME_PORT))
        .assert()
        .code(2)
        .stdout(predicate::str::contains(
            "same.conf:4: policy `Y`: `local-port` 1080 is also the `local-port` of policy `X`",
        ));
}

const SUBSCRIBED: &str = "[General]\n[Proxy Group]\nLocal = select, DIRECT, policy-path=nodes.txt\n\
```

`crates/rurge/tests/cli.rs`——把

```rust
            "stop must not wait out the grace period: {lines:?}"
        );
    }

    #[test]
    fn run_exits_1_when_the_api_port_is_taken() {
```

换成

```rust
            "stop must not wait out the grace period: {lines:?}"
        );
    }

    /// `rurge run` starts an `external` policy's program on first use and
    /// stops it on the way out (phase 2 M4 design 7.3, 8.2). The program is
    /// a second rurge: a SOCKS5 proxy on the policy's `local-port`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_starts_an_external_program_and_stops_it_on_exit() {
        let target = TestServer::spawn().await;
        target.set("/hello", "hi from target");
        let port = target.url("/").port().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let inner_port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let inner = dir.path().join("inner.conf");
        std::fs::write(
            &inner,
            format!("[General]\nsocks5-listen = 127.0.0.1:{inner_port}\nloglevel = warning\n[Rule]\nFINAL,DIRECT\n"),
        )
        .unwrap();
        let quoted = |p: &Path| format!("\"{}\"", p.to_string_lossy().replace('\\', "\\\\"));
        let conf = dir.path().join("t.conf");
        std::fs::write(
            &conf,
            format!(
                "[General]\n{API_GENERAL}\n[Proxy]\n\
Inner = external, exec = {}, args = run, args = -c, args = {}, args = --no-network, args = --data-dir, args = {}, local-port = {inner_port}\n\
[Rule]\nFINAL,Inner\n",
                quoted(&assert_cmd::cargo::cargo_bin("rurge")),
                quoted(&inner),
                quoted(&dir.path().join("inner-data")),
            ),
        )
        .unwrap();
        let data = dir.path().join("data");
        let mut daemon = tokio::task::spawn_blocking({
            let (conf, data) = (conf.clone(), data.clone());
            move || spawn_daemon(&conf, &data)
        })
        .await
        .unwrap();
        let api = api_port(&daemon);
        let http_port = daemon.http;
        let ok = tokio::task::spawn_blocking(move || {
            http_get(http_port, &format!("http://127.0.0.1:{port}/hello"))
        })
        .await
        .unwrap();
        assert!(
            ok.starts_with("HTTP/1.1 200") && ok.ends_with("hi from target"),
            "{ok}"
        );
        let log = std::fs::read_to_string(data.join("external").join("Inner.log")).unwrap();
        assert!(
            log.contains(&format!("listening on socks5://127.0.0.1:{inner_port}")),
            "{log}"
        );
        assert_eq!(api_call(api, "POST", "/v1/stop", "k", Some("{}")).0, 200);
        assert_eq!(wait_for_exit(&mut daemon, 10), Some(0), "stop exits 0");
        // the program is gone with rurge: its port no longer answers
        wait_until("the inner rurge to end", || {
            TcpStream::connect_timeout(
                &std::net::SocketAddr::from(([127, 0, 0, 1], inner_port)),
                Duration::from_millis(500),
            )
            .is_err()
        });
    }

    #[test]
    fn run_exits_1_when_the_api_port_is_taken() {
```

`crates/rurge/src/cli/runtime.rs`——把

```rust
    use rurge_net::socket::{Family, SocketHook};
```

换成

```rust
    use rurge_net::socket::{Family, SocketHook};
    use rurge_proto::external::ProcessHook;
```

`crates/rurge/src/cli/runtime.rs`——把

```rust
        assert_eq!(socket.tos_v4().unwrap(), 0x28);
    }
}
```

换成

```rust
        assert_eq!(socket.tos_v4().unwrap(), 0x28);
    }

    /// Nothing but this test's own child: a process that is not ours (no
    /// such process) cannot be taken in.
    #[test]
    fn platform_processes_delegates_to_rurge_platform() {
        let hook = PlatformProcesses;
        #[cfg(windows)]
        let mut command = {
            let mut c = std::process::Command::new("cmd");
            c.args(["/c", "ping -n 30 127.0.0.1"]);
            c
        };
        #[cfg(unix)]
        let mut command = std::process::Command::new("sleep");
        #[cfg(unix)]
        command.arg("30");
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null());
        hook.prepare(&mut command);
        let mut child = command.spawn().unwrap();
        let mut group = hook.contain(child.id()).unwrap().expect("a group");
        group.kill().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while child.try_wait().unwrap().is_none() {
            if std::time::Instant::now() > deadline {
                let _ = child.kill();
                panic!("the program is still running");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge --test cli check_knows_external`
Expected: FAIL——`external` 还在能力表之外，`W0007` 多出一条（`runtime.rs` 的新用例此时编译不过：`PlatformProcesses` 还没有）：

```text
test check_knows_external ... FAILED
thread 'check_knows_external' panicked at crates\rurge\tests\cli.rs:378:5:
assertion `left == right` failed: warning[W0007] C:\Users\SZV01065\AppData\Local\Temp\.tmpGzUC6w\x.conf:3: policy type `external` is not implemented in this version; such policies behave as REJECT
C:\Users\SZV01065\AppData\Local\Temp\.tmpGzUC6w\x.conf: 0 error(s), 2 warning(s), 0 note(s)
  left: 2
 right: 1
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 49 filtered out; finished in 1.10s
   Compiling serde_path_to_error v0.1.20
error: test failed, to rerun pass `-p rurge --test cli`
exit 101
```

- [ ] **Step 3: 实现**

`crates/rurge/Cargo.toml`——把

```toml
rurge-platform.workspace = true
```

换成

```toml
rurge-platform.workspace = true
rurge-proto.workspace = true
```

`crates/rurge/src/cli/runtime.rs`——把

```rust
        rurge_platform::socket::set_tos(socket, platform_family(family), tos)
    }
}

#[cfg(test)]
```

换成

```rust
        rurge_platform::socket::set_tos(socket, platform_family(family), tos)
    }
}

/// `rurge-platform::process` behind the `ProcessHook` trait (AR-02): an
/// `external` program leads a process group of its own on Unix and runs in
/// a Job Object on Windows, so it is stopped with whatever it started.
pub struct PlatformProcesses;

struct PlatformTree(rurge_platform::process::ProcessTree);

impl rurge_proto::external::ProcessGroup for PlatformTree {
    fn terminate(&mut self) -> std::io::Result<()> {
        self.0.terminate()
    }

    fn kill(&mut self) -> std::io::Result<()> {
        self.0.kill()
    }
}

impl rurge_proto::external::ProcessHook for PlatformProcesses {
    fn prepare(&self, command: &mut std::process::Command) {
        rurge_platform::process::prepare(command);
    }

    fn contain(
        &self,
        pid: u32,
    ) -> std::io::Result<Option<Box<dyn rurge_proto::external::ProcessGroup>>> {
        let tree = rurge_platform::process::ProcessTree::contain(pid)?;
        Ok(Some(Box::new(PlatformTree(tree))))
    }
}

#[cfg(test)]
```

`crates/rurge/src/cli/run.rs`——把

```rust
use super::runtime::RuntimeArgs;
```

换成

```rust
use super::runtime::{PlatformProcesses, RuntimeArgs};
```

`crates/rurge/src/cli/run.rs`——把

```rust
            shared.empty_group = EmptyGroup::Reject;
        }
        let engine_rt =
```

换成

```rust
            shared.empty_group = EmptyGroup::Reject;
        }
        shared.processes = Arc::new(PlatformProcesses);
        let engine_rt =
```

`crates/rurge/src/cli/run.rs`——把

```rust
                println!("forced shutdown");
```

换成

```rust
                println!("forced shutdown");
                // no grace for the external programs either: ending the
                // runtime ends the tasks that hold them, which kill them
```

`crates/rurge/src/cli/run.rs`——把

```rust
                let _ = tokio::time::timeout(Duration::from_secs(1), &mut drain).await;
            }
        }
        Ok(ExitCode::SUCCESS)
```

换成

```rust
                let _ = tokio::time::timeout(Duration::from_secs(1), &mut drain).await;
            }
        }
        // the sessions are over: the programs behind `external` policies go
        // last, each with what it started (phase 2 M4 design 7.3)
        engine.stop_external_programs().await;
        Ok(ExitCode::SUCCESS)
```

`crates/rurge/src/capabilities.rs`——把

```rust
//! M2b), `ssh` (phase 2 M4a), `select` groups, `url-test` / `fallback` /
```

换成

```rust
//! M2b), `ssh` (phase 2 M4a), `wireguard` (phase 2 M4b), `external`
//! (phase 2 M4c), `select` groups, `url-test` / `fallback` /
```

`crates/rurge/src/capabilities.rs`——把

```rust
            PolicyKind::WireGuard,
```

换成

```rust
            PolicyKind::WireGuard,
            PolicyKind::External,
```

文档：

`CLAUDE.md`——把

```markdown
阶段 1 功能已齐备（M1 ～ M4b）；阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`。M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。M3b（测速与自动组）已完成——`[General] test-timeout` 区分未设置（`General::test_target`；策略的 `test-url` / `test-timeout` 生效，`W0029` 对它们退役）；`rurge-policy` 的 `probe`（经策略自己的出站在一条连接上两次 HEAD，HTTPS 在已建立的 TLS 连接上测第二次，根证书取 `OutboundFactory::roots`）、`testbook`（`TestBook`：结果按策略与定义保存、同一策略同时只测一次、最多 8 个并发、`TestObserver`、一次性的 `test_once`）与 `auto`（`url-test` / `fallback` / `load-balance` 三种选法、`SelectCtx`、`AutoGroups`：临时覆盖、`url-test` 保持的成员、每组上一轮测试的时间与测试请求）；注册表按测试结果选成员（只有拨号触发测试、`evaluate-before-use` 的 `Resolution.pending`、嵌套组的分数、`round_timeout`）、`resolve_relay`（空组当中继一律拒绝）、订阅行不能动用主配置的 `client-cert` 与策略（`W0023`）；`rurge-engine` 的测试调度任务、测试会话进请求记录（规则 `policy test`）、`evaluate-before-use` 的有界等待（`policy group evaluation failed`）、重载时保留未变组的覆盖、`Engine::snapshot`（运行时与注册表在代际锁下成对读取）与 DNS 会话在链底为 REJECT 时的旁路；`rurge-api` 的 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 与 `POST /v1/policy_groups/select` 对自动组的临时覆盖；能力表翻转 `url-test` / `fallback` / `load-balance`（`W0008` 只剩 `smart` 与 `subnet`）。M3c（`smart`）已完成——`rurge-policy::smart`（`SmartBook`：按策略的时间加权首字节分数与失败罚分、健康 / 未知 / 失败三种状态、站点记忆、使用计数；`rank` 给出选中的成员与重试列表；`sample` 给大组的常规轮抽样）、注册表的 `smart` 成员过滤与 `policy-priority` 因子、`Resolution.smart`（`SmartPick`）、`resolve_member`、`test_round`，以及每代预存的测试查表（`TestBook::outcome`，M3b 延后事项 #14）；`rurge-inbound` 的 `SessionHandle` 两个时刻（出站就绪、首字节）、可挂多个的结束钩子与首字节钩子、`mark_upstream_failed`；`rurge-engine` 的 `smart` 模块（拨号时按重试列表换成员，单次时限"剩余时间 ÷ 剩余尝试次数"；质量回报：首字节、3 秒无响应、上游先断）、请求记录的 `connectMs` / `firstByteMs`、`automatic()` 纳入 `smart`、每次发布注册表时 `SmartBook::retain`；能力表翻转 `smart`（`W0008` 只剩 `subnet`）。M4（WireGuard / SSH / external）按三份计划推进（M4a SSH → M4b WireGuard → M4c external）：M4a（SSH）已完成——`rurge-config::spec::SshSpec`（`server-fingerprint` 解析为 `HostKeyPin`、`idle-timeout`、`private-key` 的 `E0020`）与 `ProtoSpec::keystore_item`（重载的指纹含 SSH 私钥）；新 crate `rurge-proto-ssh`（russh 0.63.3，`ring` 后端）：Keystore 私钥解码（Ed25519 / ECDSA / RSA；带口令与 DSA 为 `E0022`）、主机密钥比对、`SshOutbound`（每个策略一条会话、单飞握手、认证失败等持久失败后按 60 秒起翻倍至 10 分钟退避、钉住的主机密钥算法优先协商、`direct-tcpip` 通道、会话断了重建一次、按通道计的空闲断开、30 秒 × 3 的保活、没配指纹的一次性告警、RSA 只用 SHA-2 签名）与 `testing::FakeSsh`；`rurge-engine` 的工厂分支；订阅行自己的 `private-key=` 进订阅安全门（`W0023`）；`tests/interop` 的临时 `sshd`（`RURGE_TEST_SSHD`，只在 Unix）；能力表翻转 `ssh`。M4b（WireGuard）已完成——`rurge-config` 的 `[WireGuard <name>]` 类型化（`WireGuardSection`，`E0023`，重名的节 `W0020`）与 `spec::WireGuardSpec`（spec 带着节的内容；`underlying-proxy` 为 `W0029` 且策略 REJECT；订阅行自己的 `section-name=` 进订阅安全门）；`rurge-net` 的 `Datagram`、`Connector::connect_udp` 与 `DirectConnector::connect_udp`（按 `ip-version`、`set_tos`、7 MiB 收发缓冲）；新 crate `rurge-proto-wireguard`（boringtun 0.7.1 的 sans-IO `Tunn` 加 smoltcp 0.12：`routes` 最长前缀选 peer、`wire` 的 `client-id`、`stack`（锁内只做内存操作，每次推进都 poll 到发完，Reno 拥塞控制）、`device`（每条隧道一个任务、批量收、握手 TOS 0x88、5 分钟重拨与 endpoint 跟随、发送时本机地址或路由失效或握手无回应时换新载体（网络变化后自愈）、网络变化入口、同一私钥与 peer 只留一条隧道，节与载体设置都相同的策略共用）、`stream`、`dns`（隧道内 DNS 与缓存；每个问题 2 秒内重发、先等隧道第一次握手）、`outbound`（`WireGuardOutbound`：没有 `dns-server` 时在本机解析、含 `[Host]`；原生握手测速）与 `testing::FakeWgPeer`）；`rurge-proto` 的 `Outbound::native_test`；`rurge-policy` 的 `TestMode`（URL 与原生两种测速，`wireguard` 另加 10 秒）；`rurge-engine` 的工厂分支、拨号时 `Unsupported` 带说明、DNS 会话防环扩展到 endpoint 写成域名的 `wireguard`；bin 不输出 boringtun 自己的日志；`tests/interop` 对 sing-box WireGuard 端点（带保留字节）的互操作用例；能力表翻转 `wireguard`。M4c（external）尚未开始。
```

换成

```markdown
阶段 1 功能已齐备（M1 ～ M4b）；阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`。M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。M3b（测速与自动组）已完成——`[General] test-timeout` 区分未设置（`General::test_target`；策略的 `test-url` / `test-timeout` 生效，`W0029` 对它们退役）；`rurge-policy` 的 `probe`（经策略自己的出站在一条连接上两次 HEAD，HTTPS 在已建立的 TLS 连接上测第二次，根证书取 `OutboundFactory::roots`）、`testbook`（`TestBook`：结果按策略与定义保存、同一策略同时只测一次、最多 8 个并发、`TestObserver`、一次性的 `test_once`）与 `auto`（`url-test` / `fallback` / `load-balance` 三种选法、`SelectCtx`、`AutoGroups`：临时覆盖、`url-test` 保持的成员、每组上一轮测试的时间与测试请求）；注册表按测试结果选成员（只有拨号触发测试、`evaluate-before-use` 的 `Resolution.pending`、嵌套组的分数、`round_timeout`）、`resolve_relay`（空组当中继一律拒绝）、订阅行不能动用主配置的 `client-cert` 与策略（`W0023`）；`rurge-engine` 的测试调度任务、测试会话进请求记录（规则 `policy test`）、`evaluate-before-use` 的有界等待（`policy group evaluation failed`）、重载时保留未变组的覆盖、`Engine::snapshot`（运行时与注册表在代际锁下成对读取）与 DNS 会话在链底为 REJECT 时的旁路；`rurge-api` 的 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 与 `POST /v1/policy_groups/select` 对自动组的临时覆盖；能力表翻转 `url-test` / `fallback` / `load-balance`（`W0008` 只剩 `smart` 与 `subnet`）。M3c（`smart`）已完成——`rurge-policy::smart`（`SmartBook`：按策略的时间加权首字节分数与失败罚分、健康 / 未知 / 失败三种状态、站点记忆、使用计数；`rank` 给出选中的成员与重试列表；`sample` 给大组的常规轮抽样）、注册表的 `smart` 成员过滤与 `policy-priority` 因子、`Resolution.smart`（`SmartPick`）、`resolve_member`、`test_round`，以及每代预存的测试查表（`TestBook::outcome`，M3b 延后事项 #14）；`rurge-inbound` 的 `SessionHandle` 两个时刻（出站就绪、首字节）、可挂多个的结束钩子与首字节钩子、`mark_upstream_failed`；`rurge-engine` 的 `smart` 模块（拨号时按重试列表换成员，单次时限"剩余时间 ÷ 剩余尝试次数"；质量回报：首字节、3 秒无响应、上游先断）、请求记录的 `connectMs` / `firstByteMs`、`automatic()` 纳入 `smart`、每次发布注册表时 `SmartBook::retain`；能力表翻转 `smart`（`W0008` 只剩 `subnet`）。M4（WireGuard / SSH / external）按三份计划推进（M4a SSH → M4b WireGuard → M4c external）：M4a（SSH）已完成——`rurge-config::spec::SshSpec`（`server-fingerprint` 解析为 `HostKeyPin`、`idle-timeout`、`private-key` 的 `E0020`）与 `ProtoSpec::keystore_item`（重载的指纹含 SSH 私钥）；新 crate `rurge-proto-ssh`（russh 0.63.3，`ring` 后端）：Keystore 私钥解码（Ed25519 / ECDSA / RSA；带口令与 DSA 为 `E0022`）、主机密钥比对、`SshOutbound`（每个策略一条会话、单飞握手、认证失败等持久失败后按 60 秒起翻倍至 10 分钟退避、钉住的主机密钥算法优先协商、`direct-tcpip` 通道、会话断了重建一次、按通道计的空闲断开、30 秒 × 3 的保活、没配指纹的一次性告警、RSA 只用 SHA-2 签名）与 `testing::FakeSsh`；`rurge-engine` 的工厂分支；订阅行自己的 `private-key=` 进订阅安全门（`W0023`）；`tests/interop` 的临时 `sshd`（`RURGE_TEST_SSHD`，只在 Unix）；能力表翻转 `ssh`。M4b（WireGuard）已完成——`rurge-config` 的 `[WireGuard <name>]` 类型化（`WireGuardSection`，`E0023`，重名的节 `W0020`）与 `spec::WireGuardSpec`（spec 带着节的内容；`underlying-proxy` 为 `W0029` 且策略 REJECT；订阅行自己的 `section-name=` 进订阅安全门）；`rurge-net` 的 `Datagram`、`Connector::connect_udp` 与 `DirectConnector::connect_udp`（按 `ip-version`、`set_tos`、7 MiB 收发缓冲）；新 crate `rurge-proto-wireguard`（boringtun 0.7.1 的 sans-IO `Tunn` 加 smoltcp 0.12：`routes` 最长前缀选 peer、`wire` 的 `client-id`、`stack`（锁内只做内存操作，每次推进都 poll 到发完，Reno 拥塞控制）、`device`（每条隧道一个任务、批量收、握手 TOS 0x88、5 分钟重拨与 endpoint 跟随、发送时本机地址或路由失效或握手无回应时换新载体（网络变化后自愈）、网络变化入口、同一私钥与 peer 只留一条隧道，节与载体设置都相同的策略共用）、`stream`、`dns`（隧道内 DNS 与缓存；每个问题 2 秒内重发、先等隧道第一次握手）、`outbound`（`WireGuardOutbound`：没有 `dns-server` 时在本机解析、含 `[Host]`；原生握手测速）与 `testing::FakeWgPeer`）；`rurge-proto` 的 `Outbound::native_test`；`rurge-policy` 的 `TestMode`（URL 与原生两种测速，`wireguard` 另加 10 秒）；`rurge-engine` 的工厂分支、拨号时 `Unsupported` 带说明、DNS 会话防环扩展到 endpoint 写成域名的 `wireguard`；bin 不输出 boringtun 自己的日志；`tests/interop` 对 sing-box WireGuard 端点（带保留字节）的互操作用例；能力表翻转 `wireguard`。M4c（external）已完成——`rurge-config::spec::ExternalSpec`（`args` 是 `Secret`；两个 `external` 策略写了同一个 `local-port` 是 `E0018`；`args` 进内联参数的脱敏名单）、订阅导入的 `external` 行一律跳过（`W0023`）；`rurge-platform::process`（Unix 进程组与 `killpg`，Windows Job Object）；`rurge_proto::external`（`ExternalOutbound`：第一次用到时拉起、输出写 `<数据目录>/external/<策略名>.log` 且超过 1 MiB 时在拉起前轮转、子进程环境去掉代理变量并设 `NO_PROXY=*`、程序退出后下次用到时再拉起且两次拉起至少隔 2 秒、每 500 ms 连一次本机端口最多 6 次；`ProcessHook` / `ProcessGroup` 与 `NoProcessGroups`；程序自己退出时连同它启动的进程一起结束）；`rurge-engine` 的工厂分支、`EngineShared.processes` / `externals` 与 `Engine::stop_external_programs`；bin 的 `PlatformProcesses` 与退出流程最后停掉全部外部程序；只用于测试的工作区成员 `tests/external`（`rurge-external-tests`：极小的 SOCKS5 辅助程序 `socks-helper` 与真实拉起它的用例）；能力表翻转 `external`（`W0007` 不再因 `external` 出现）。M4 至此完成。
```

`CLAUDE.md`——把

```markdown
- `docs/superpowers/plans/2026-09-28-phase2-m4b-wireguard-plan.md`：阶段 2 / M4b（WireGuard）实施计划（10 个任务）。开头「计划期决定」表（P1–P21）记录核对 boringtun / smoltcp / hickory-proto 源码与手册得出的结论和与设计文字不同的决定（smoltcp 0.12 每次 `poll` 每条连接只发一个报文段、它的 Cubic 把窗口单位算错而改用 Reno、设备批量收与载体 7 MiB 缓冲、同一私钥与 peer 只留一条隧道、连接持有隧道、握手日志只在状态变化时记、引擎装配排在测速之前等）与「承接事项」；末尾「执行期修正记录」与「延后事项」两张表。
```

换成

```markdown
- `docs/superpowers/plans/2026-09-28-phase2-m4b-wireguard-plan.md`：阶段 2 / M4b（WireGuard）实施计划（10 个任务）。开头「计划期决定」表（P1–P21）记录核对 boringtun / smoltcp / hickory-proto 源码与手册得出的结论和与设计文字不同的决定（smoltcp 0.12 每次 `poll` 每条连接只发一个报文段、它的 Cubic 把窗口单位算错而改用 Reno、设备批量收与载体 7 MiB 缓冲、同一私钥与 peer 只留一条隧道、连接持有隧道、握手日志只在状态变化时记、引擎装配排在测速之前等）与「承接事项」；末尾「执行期修正记录」与「延后事项」两张表。
- `docs/superpowers/plans/2026-09-29-phase2-m4c-external-plan.md`：阶段 2 / M4c（external）实施计划（5 个任务）。开头「计划期决定」表记录核对 windows-sys / nix / tokio 源码与本仓库得出的结论和与设计文字不同的决定（每次连接本机端口限时 500 ms——Windows 要约 2 秒才拒绝连接、`ProtoSpec::External` 与重复端口检查随引擎任务落地、Shadow TLS 不能与 `external` 组合、程序自己退出时连同它启动的进程一起结束、测试辅助程序放在只用于测试的工作区成员 `tests/external` 里等）与「承接事项」；末尾「执行期修正记录」与「延后事项」两张表。
```

`CLAUDE.md`——把

```markdown
- 依赖方向（M2 设计文档确认）：`rurge-dns → rurge-rules → rurge-net → rurge-config`；`[Host]` 集合键与 `LazyResolver` 都由 `rurge-dns` 依赖 `rurge-rules` 提供，而非并列关系；`rurge (bin) → rurge-engine → { rurge-inbound → rurge-proto, rurge-policy → rurge-proto, rurge-dns }`。M4 设计文档确认：`rurge (bin) → rurge-api → rurge-engine`；`rurge-api` 依赖 `rurge-engine` / `rurge-config`（`rurge-dns` 的类型经 `rurge-engine` 间接可达，不需要直接依赖），不依赖 `rurge-platform`，也不认识 bin。`tests/interop`（`rurge-interop`，`publish = false`）是仅测试用的工作区成员。
- 连接处理流水线（PRD 3.3）：入站 → 协议嗅探（SNI / Host / QUIC / STUN）→ 预匹配 → 出站模式判断 → 规则匹配（域名规则不触发 DNS，IP 规则按需解析）→ 策略解析（组 / 链式 / 别名）→ 出站建立 → HTTP 引擎（MITM → Header Rewrite → URL Rewrite → Body Rewrite → 脚本 → Map Local 短路）→ 观测。
- 配置对象不可变，重载时原子切换（AR-04）；每个连接是独立 tokio 任务（AR-03）。
- `unsafe_code`：全工作区 `forbid`；只有 `rurge-platform` 用 crate 自己的 `[lints] unsafe_code = "deny"` 放宽，且仅 `sysproxy::windows` 里调用 `InternetSetOptionW` 通知 WinINet 的那一个函数标 `#[allow(unsafe_code)]`（M4b）。
```

换成

```markdown
- 依赖方向（M2 设计文档确认）：`rurge-dns → rurge-rules → rurge-net → rurge-config`；`[Host]` 集合键与 `LazyResolver` 都由 `rurge-dns` 依赖 `rurge-rules` 提供，而非并列关系；`rurge (bin) → rurge-engine → { rurge-inbound → rurge-proto, rurge-policy → rurge-proto, rurge-dns }`。M4 设计文档确认：`rurge (bin) → rurge-api → rurge-engine`；`rurge-api` 依赖 `rurge-engine` / `rurge-config`（`rurge-dns` 的类型经 `rurge-engine` 间接可达，不需要直接依赖），不依赖 `rurge-platform`，也不认识 bin。`tests/interop`（`rurge-interop`，`publish = false`）与 `tests/external`（`rurge-external-tests`，`publish = false`，M4c）是仅测试用的工作区成员。
- 连接处理流水线（PRD 3.3）：入站 → 协议嗅探（SNI / Host / QUIC / STUN）→ 预匹配 → 出站模式判断 → 规则匹配（域名规则不触发 DNS，IP 规则按需解析）→ 策略解析（组 / 链式 / 别名）→ 出站建立 → HTTP 引擎（MITM → Header Rewrite → URL Rewrite → Body Rewrite → 脚本 → Map Local 短路）→ 观测。
- 配置对象不可变，重载时原子切换（AR-04）；每个连接是独立 tokio 任务（AR-03）。
- `unsafe_code`：全工作区 `forbid`；只有 `rurge-platform` 用 crate 自己的 `[lints] unsafe_code = "deny"` 放宽，且只有两个函数标 `#[allow(unsafe_code)]`：`sysproxy::windows` 里调用 `InternetSetOptionW` 通知 WinINet 的那一个（阶段 1 / M4b），与 `process` 里创建 Job Object、设"关闭即结束全部进程"、把外部程序放进去的 `job_for`（阶段 2 / M4c，M4-D6）。
```

`CLAUDE.md`——把

```markdown
cargo test -p rurge-proto-wireguard --release throughput -- --ignored --nocapture   # WireGuard 吞吐基准（经回环假对端双向回显 64 MiB；不作门禁）
```

换成

```markdown
cargo test -p rurge-proto-wireguard --release throughput -- --ignored --nocapture   # WireGuard 吞吐基准（经回环假对端双向回显 64 MiB；不作门禁）
cargo test -p rurge-external-tests              # external：真实拉起测试辅助程序（socks-helper）——按需拉起、参数顺序与代理变量、再拉起与 2 秒间隔、连接被拒的重试、日志、整棵进程树的停止、经引擎的端到端、检查不拉起、重载沿用与替换
```

`README.md`——把

```markdown
> **阶段 1 功能齐备：M1 ～ M4b**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）进行中：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；M3b（测速与自动组）已完成——`url-test` / `fallback` / `load-balance` 组按连通性测试（经各节点在同一条连接上两次 `HEAD`，HTTPS 测试 URL 在已建立的 TLS 连接上测第二次）自动选择，`interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` 与策略的 `test-url` / `test-timeout` 生效，自动组可经 `POST /v1/policy_groups/select` 临时覆盖，新增 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 三个端点；M3c（`smart`）已完成——`smart` 组按真实会话的首字节耗时与测速结果打分（失败罚分、`policy-priority` 因子、约 1 小时的站点记忆），选中的成员连不上时自动换下一个（最多再试 2 个），请求记录增加 `connectMs` / `firstByteMs`；M4a（SSH）已完成——`ssh` 策略（TCP；每个策略一条 SSH 会话、每个连接一个通道；口令或 `[Keystore]` 里的 OpenSSH 私钥登录；`server-fingerprint` 校验主机密钥；按 `idle-timeout` 断开空闲会话、30 秒一次保活）已可用；M4b（WireGuard）已完成——`wireguard` 策略（TCP；用户态隧道，不建虚拟网卡；多个 peer 按 `allowed-ips` 选路；`client-id`（WARP 的保留字节）；配了 `dns-server` 时目标域名经隧道查询，否则在本机解析；没有 `dns-server` 也没写 `test-url` 时以握手测速）已可用；其余出站协议仍在阶段 2 后续里程碑，`subnet` 策略组在阶段 3。
```

换成

```markdown
> **阶段 1 功能齐备：M1 ～ M4b**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）进行中：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；M3b（测速与自动组）已完成——`url-test` / `fallback` / `load-balance` 组按连通性测试（经各节点在同一条连接上两次 `HEAD`，HTTPS 测试 URL 在已建立的 TLS 连接上测第二次）自动选择，`interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` 与策略的 `test-url` / `test-timeout` 生效，自动组可经 `POST /v1/policy_groups/select` 临时覆盖，新增 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 三个端点；M3c（`smart`）已完成——`smart` 组按真实会话的首字节耗时与测速结果打分（失败罚分、`policy-priority` 因子、约 1 小时的站点记忆），选中的成员连不上时自动换下一个（最多再试 2 个），请求记录增加 `connectMs` / `firstByteMs`；M4a（SSH）已完成——`ssh` 策略（TCP；每个策略一条 SSH 会话、每个连接一个通道；口令或 `[Keystore]` 里的 OpenSSH 私钥登录；`server-fingerprint` 校验主机密钥；按 `idle-timeout` 断开空闲会话、30 秒一次保活）已可用；M4b（WireGuard）已完成——`wireguard` 策略（TCP；用户态隧道，不建虚拟网卡；多个 peer 按 `allowed-ips` 选路；`client-id`（WARP 的保留字节）；配了 `dns-server` 时目标域名经隧道查询，否则在本机解析；没有 `dns-server` 也没写 `test-url` 时以握手测速）已可用；M4c（external）已完成——`external` 策略（TCP；第一次用到时拉起外部程序、经它在本机端口上的 SOCKS5 转发；程序退出后下次用到时再拉起；输出写进数据目录；rurge 退出时连同它启动的进程一起停掉——Unix 用进程组，Windows 用 Job Object；三平台都支持，Surge 只在 Mac 上有）已可用，订阅导入的 `external` 一律跳过；其余出站协议仍在阶段 2 后续里程碑，`subnet` 策略组在阶段 3。
```

`README.md`——把

```markdown
| 出站协议       | DIRECT / REJECT 系列 / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / 外部程序（DIRECT 与 REJECT 系四种已实现，M3a；HTTP / SOCKS5（含 `socks5-tls`）TCP 上游与 `underlying-proxy` 链已实现，阶段 2 / M1；Trojan（TCP；WebSocket 传输）已实现，阶段 2 / M2a；VMess（AEAD 握手，TCP；可叠加 TLS / WebSocket）与 AnyTLS（TCP，会话复用）已实现，阶段 2 / M2b；SSH（TCP，会话复用）已实现，阶段 2 / M4a；WireGuard（TCP，用户态隧道）已实现，阶段 2 / M4b） | 2     |
```

换成

```markdown
| 出站协议       | DIRECT / REJECT 系列 / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / 外部程序（DIRECT 与 REJECT 系四种已实现，M3a；HTTP / SOCKS5（含 `socks5-tls`）TCP 上游与 `underlying-proxy` 链已实现，阶段 2 / M1；Trojan（TCP；WebSocket 传输）已实现，阶段 2 / M2a；VMess（AEAD 握手，TCP；可叠加 TLS / WebSocket）与 AnyTLS（TCP，会话复用）已实现，阶段 2 / M2b；SSH（TCP，会话复用）已实现，阶段 2 / M4a；WireGuard（TCP，用户态隧道）已实现，阶段 2 / M4b；外部程序（TCP，三平台）已实现，阶段 2 / M4c） | 2     |
```

`README_en.md`——把

```markdown
> **Phase 1 feature-complete: M1 through M4b** (profile parsing, rule engine, rule sets, GeoIP, external resource management, DNS client, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`; M3b added the request log and traffic stats, SNI recording, idle timeout, REJECT auto-escalation, a 502 for failed CONNECT dials, graceful shutdown, hot reload (SIGHUP / `--watch`), `--log-file`, and `encrypted-dns-follow-outbound-mode`; M4a added a Surge-compatible HTTP API (phase-1 endpoints, `X-Key` auth with banning), outbound mode / global policy persisted to `state.json`, and `rurge reload` / `stop` / `status`; M4b added the system proxy (Windows registry + a WinINet notification, macOS `networksetup`, Linux GNOME / KDE, `--system-proxy` and `POST /v1/features/system_proxy`, automatic recovery on exit or crash, and following listener changes across a reload) and `rurge service install / uninstall [--dry-run]` (systemd / launchd / a Windows scheduled task)) — the phase-1 acceptance criteria that need a real desktop environment (the three-platform system proxy, service install) have not been through manual acceptance yet; see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md) for the checklist. The dashboard is phase 6. Phase 2 (outbound protocols and policy groups) is in progress: M1 (outbound foundation and HTTP / SOCKS5 upstreams) is done — `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections by rule through DIRECT / REJECT or `http` / `https` / `socks5` / `socks5-tls` upstreams (TCP, including two-or-more-hop `underlying-proxy` chains), and a `select` group's choice can be read and switched over the HTTP API (see [docs/api/phase2.md](docs/api/phase2.md)); M2a (the TLS family, Trojan first) is done — the `trojan` policy (TCP, TLS always on, optionally over WebSocket) is usable; M2b (VMess / AnyTLS) is done — the `vmess` (AEAD handshake, TCP, optionally over TLS / WebSocket) and `anytls` (TCP, session reuse) policies are usable, and a reload reuses outbounds by fingerprint without disturbing connection pools still in use; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); M3a (member assembly and subscriptions) is done — `policy-path` subscriptions (a local file or a URL, in Surge format; an update rebuilds only the policy table, without disturbing unrelated connections), `include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`, the group-level `underlying-proxy` (derived `Name (via Relay)` members), group cycle warnings (`W0030`) and the empty-group fallback (DIRECT by default, REJECT with `--empty-group-reject`) are usable; M3b (connectivity tests and automatic groups) is done — `url-test` / `fallback` / `load-balance` groups choose by connectivity tests (two `HEAD`s over one connection through each member, the second one on the established TLS connection for an HTTPS test URL), `interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` and the policies' `test-url` / `test-timeout` take effect, an automatic group takes a temporary override through `POST /v1/policy_groups/select`, and `POST /v1/policies/test`, `GET /v1/policy_groups/test_results` and `POST /v1/policy_groups/test` are available; M3c (`smart`) is done — a `smart` group scores its members by the first-byte time of real sessions and by the tests (failure penalties, `policy-priority` factors, per-site memory for about an hour) and moves on to the next member when the one it picked does not connect (up to two more), and the request log gains `connectMs` / `firstByteMs`; M4a (SSH) is done — the `ssh` policy (TCP; one SSH session per policy and a channel per connection; login with a password or an OpenSSH private key from `[Keystore]`; host keys checked against `server-fingerprint`; idle sessions closed after `idle-timeout`, a keepalive every 30 seconds) is usable; M4b (WireGuard) is done — the `wireguard` policy (TCP; a user-space tunnel, no virtual interface; several peers chosen by `allowed-ips`; `client-id` (WARP's reserved bytes); destination names looked up through the tunnel when the section has a `dns-server`, on this machine otherwise; tested by a handshake when there is neither a `dns-server` nor a `test-url`) is usable; the remaining outbound protocols are later phase-2 milestones, and the `subnet` group comes in phase 3.
```

换成

```markdown
> **Phase 1 feature-complete: M1 through M4b** (profile parsing, rule engine, rule sets, GeoIP, external resource management, DNS client, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`; M3b added the request log and traffic stats, SNI recording, idle timeout, REJECT auto-escalation, a 502 for failed CONNECT dials, graceful shutdown, hot reload (SIGHUP / `--watch`), `--log-file`, and `encrypted-dns-follow-outbound-mode`; M4a added a Surge-compatible HTTP API (phase-1 endpoints, `X-Key` auth with banning), outbound mode / global policy persisted to `state.json`, and `rurge reload` / `stop` / `status`; M4b added the system proxy (Windows registry + a WinINet notification, macOS `networksetup`, Linux GNOME / KDE, `--system-proxy` and `POST /v1/features/system_proxy`, automatic recovery on exit or crash, and following listener changes across a reload) and `rurge service install / uninstall [--dry-run]` (systemd / launchd / a Windows scheduled task)) — the phase-1 acceptance criteria that need a real desktop environment (the three-platform system proxy, service install) have not been through manual acceptance yet; see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md) for the checklist. The dashboard is phase 6. Phase 2 (outbound protocols and policy groups) is in progress: M1 (outbound foundation and HTTP / SOCKS5 upstreams) is done — `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections by rule through DIRECT / REJECT or `http` / `https` / `socks5` / `socks5-tls` upstreams (TCP, including two-or-more-hop `underlying-proxy` chains), and a `select` group's choice can be read and switched over the HTTP API (see [docs/api/phase2.md](docs/api/phase2.md)); M2a (the TLS family, Trojan first) is done — the `trojan` policy (TCP, TLS always on, optionally over WebSocket) is usable; M2b (VMess / AnyTLS) is done — the `vmess` (AEAD handshake, TCP, optionally over TLS / WebSocket) and `anytls` (TCP, session reuse) policies are usable, and a reload reuses outbounds by fingerprint without disturbing connection pools still in use; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); M3a (member assembly and subscriptions) is done — `policy-path` subscriptions (a local file or a URL, in Surge format; an update rebuilds only the policy table, without disturbing unrelated connections), `include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`, the group-level `underlying-proxy` (derived `Name (via Relay)` members), group cycle warnings (`W0030`) and the empty-group fallback (DIRECT by default, REJECT with `--empty-group-reject`) are usable; M3b (connectivity tests and automatic groups) is done — `url-test` / `fallback` / `load-balance` groups choose by connectivity tests (two `HEAD`s over one connection through each member, the second one on the established TLS connection for an HTTPS test URL), `interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` and the policies' `test-url` / `test-timeout` take effect, an automatic group takes a temporary override through `POST /v1/policy_groups/select`, and `POST /v1/policies/test`, `GET /v1/policy_groups/test_results` and `POST /v1/policy_groups/test` are available; M3c (`smart`) is done — a `smart` group scores its members by the first-byte time of real sessions and by the tests (failure penalties, `policy-priority` factors, per-site memory for about an hour) and moves on to the next member when the one it picked does not connect (up to two more), and the request log gains `connectMs` / `firstByteMs`; M4a (SSH) is done — the `ssh` policy (TCP; one SSH session per policy and a channel per connection; login with a password or an OpenSSH private key from `[Keystore]`; host keys checked against `server-fingerprint`; idle sessions closed after `idle-timeout`, a keepalive every 30 seconds) is usable; M4b (WireGuard) is done — the `wireguard` policy (TCP; a user-space tunnel, no virtual interface; several peers chosen by `allowed-ips`; `client-id` (WARP's reserved bytes); destination names looked up through the tunnel when the section has a `dns-server`, on this machine otherwise; tested by a handshake when there is neither a `dns-server` nor a `test-url`) is usable; M4c (external) is done — the `external` policy (TCP; the program is started on first use and reached as a SOCKS5 proxy on its local port; a program that exited is started again on the next use; its output goes to the data directory; when rurge exits, the program is stopped together with whatever it started — a process group on Unix, a Job Object on Windows; all three platforms, where Surge has it on the Mac only) is usable, and subscriptions never bring `external` policies in; the remaining outbound protocols are later phase-2 milestones, and the `subnet` group comes in phase 3.
```

`README_en.md`——把

```markdown
| Outbound            | DIRECT / REJECT family / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / external program (DIRECT and the four REJECT flavours implemented, M3a; HTTP / SOCKS5 (including `socks5-tls`) TCP upstreams and `underlying-proxy` chains implemented, phase 2 / M1; Trojan (TCP; WebSocket transport) implemented, phase 2 / M2a; VMess (AEAD handshake, TCP; optionally TLS / WebSocket) and AnyTLS (TCP, session reuse) implemented, phase 2 / M2b; SSH (TCP, session reuse) implemented, phase 2 / M4a; WireGuard (TCP, user-space tunnel) implemented, phase 2 / M4b) | 2     |
```

换成

```markdown
| Outbound            | DIRECT / REJECT family / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / external program (DIRECT and the four REJECT flavours implemented, M3a; HTTP / SOCKS5 (including `socks5-tls`) TCP upstreams and `underlying-proxy` chains implemented, phase 2 / M1; Trojan (TCP; WebSocket transport) implemented, phase 2 / M2a; VMess (AEAD handshake, TCP; optionally TLS / WebSocket) and AnyTLS (TCP, session reuse) implemented, phase 2 / M2b; SSH (TCP, session reuse) implemented, phase 2 / M4a; WireGuard (TCP, user-space tunnel) implemented, phase 2 / M4b; external program (TCP, all three platforms) implemented, phase 2 / M4c) | 2     |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `external` | 外部代理程序（本地 SOCKS5） | Mac only（iOS 视为 REJECT） | ✅ | 2 | rurge 在 Win/Lin/mac 均支持 |
| 策略指向 rurge 尚未实现的协议类型 | Surge 原生支持全部协议 | 全部 | 🟡 | 1 / 2 | 加载时告警 `W0007`；运行时该策略按 `REJECT` 处理，会话日志 `error = policy protocol not implemented: <type>`；随阶段 2 各里程碑逐协议移除：M1 已移除 `http` `https` `socks5` `socks5-tls`；M2a 已移除 `trojan`；M2b 已移除 `vmess`（写了 `vmess-aead=true` 的行）与 `anytls`；M4a 已移除 `ssh`；M4b 已移除 `wireguard`。没写 `vmess-aead=true` 的 `vmess` 行是唯一例外：仍按 `W0007` 处理，但走专门的诊断文本 `` `vmess` without `vmess-aead=true` uses the legacy handshake, which is not implemented yet ``（每次加载一条，不是每行一条）与专门的会话日志文本 `policy protocol not implemented: vmess (legacy handshake)`，不是这里的通用 `<type>` 模板 |
```

换成

```markdown
| `external` | 外部代理程序（本地 SOCKS5） | Mac only（iOS 视为 REJECT） | ✅ | 2 | rurge 在 Win/Lin/mac 均支持；M4c 已实现（TCP） |
| 策略指向 rurge 尚未实现的协议类型 | Surge 原生支持全部协议 | 全部 | 🟡 | 1 / 2 | 加载时告警 `W0007`；运行时该策略按 `REJECT` 处理，会话日志 `error = policy protocol not implemented: <type>`；随阶段 2 各里程碑逐协议移除：M1 已移除 `http` `https` `socks5` `socks5-tls`；M2a 已移除 `trojan`；M2b 已移除 `vmess`（写了 `vmess-aead=true` 的行）与 `anytls`；M4a 已移除 `ssh`；M4b 已移除 `wireguard`；M4c 已移除 `external`。没写 `vmess-aead=true` 的 `vmess` 行是唯一例外：仍按 `W0007` 处理，但走专门的诊断文本 `` `vmess` without `vmess-aead=true` uses the legacy handshake, which is not implemented yet ``（每次加载一条，不是每行一条）与专门的会话日志文本 `policy protocol not implemented: vmess (legacy handshake)`，不是这里的通用 `<type>` 模板 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `external` | `exec` `local-port` `args`（可重复）`addresses`（可重复）`udp-relay` | ✅ | 2 | 进程退出自动重启；`addresses` 从 VIF 路由排除（阶段 3）；外部进程流量走 DIRECT；退出时清理；日志写入 rurge 数据目录 |
```

换成

```markdown
| `external` | `exec` `local-port` `args`（可重复）`addresses`（可重复）`udp-relay` | 🟡 | 2 | M4c 已实现（TCP）。第一次用到时拉起（`exec` 加按原顺序的 `args`，标准输入为空），经 SOCKS5 连 `127.0.0.1:<local-port>`；连不上时每 500 ms 一次、一个请求最多 6 次（每次连接限时 500 ms：Windows 上要约 2 秒才拒绝连接），仍不行是 `external: the local SOCKS5 port refused the connection`；程序退出后下次用到时再拉起（照手册）。差异：三平台都支持（Surge 仅 Mac）；输出追加写入 `<数据目录>/external/<策略名>.log`（名字里文件名不能用的字符换成 `_` 并加一段哈希），每次拉起先写一行分隔，超过 1 MiB 时在拉起前轮转、只留一个旧文件；同一策略两次拉起至少间隔 2 秒，拉起失败时间隔内的请求直接得到同一个 `external: could not start <策略名> (<错误种类>)`；子进程环境去掉 `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY`（大小写两种）并设 `NO_PROXY=*`；程序自己退出时它启动的进程一并结束；rurge 正常退出时最后停掉全部外部程序连同它们启动的进程（Unix：整个进程组先 SIGTERM、2 秒后 SIGKILL；Windows：Job Object 关闭即结束全部，rurge 崩溃时同样不留下进程；Unix 上 rurge 崩溃后留下的进程不处理）；重载时 `exec` / `args` / `local-port` 都没变的策略沿用原程序，变了的在旧策略释放（进行中的连接结束）后停掉；`args` 在 `profiles/current`、`policies/detail` 与 `lineHash` 里整体脱敏，日志只写策略名、pid 与退出码；两个策略写同一个 `local-port` 是错误（`E0018`）；`interface` `allow-other-interface` `tfo` `tos` `ip-version` `underlying-proxy` 不适用（`W0028`），不能叠 Shadow TLS（`E0018`）；`addresses`（阶段 3）与 `udp-relay`（M5）解析但暂不生效（`W0029`），`addresses` 只收 IP 地址；"外部进程的流量走 DIRECT"到阶段 3（有 TUN 才有意义）；订阅导入的 `external` 一律跳过（`W0023`）；干构建、`rurge check` 与 `POST /v1/profiles/check` 从不拉起程序 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `GET /v1/profiles/current?sensitive=0` | 当前配置文本（可脱敏） | 全部 | ✅ | 1 | M4a 已实现；`sensitive=0`（默认）脱敏：独立密钥行 `password` / `ca-passphrase` / `ca-p12` / `private-key` / `psk` / `pre-shared-key` / `token`；内联参数 `password` / `psk` / `private-key` / `pre-shared-key` / `base64` / `token` / `uuid` / `username` / `headers` / `ws-headers` / `ws-path` / `shadow-tls-password` / `policy-path` / `external-policy-modifier`（后两个自 M3a 起：订阅链接常带 token，修饰列表能设任何参数）/ `test-url`（自 M3b 起：订阅行设的测试 URL 可能带 token）；`http-api` / `external-controller-access` / `http-listen` / `socks5-listen` 的 `key@` 前缀与 `wifi-access-http-auth` 口令；所有写作 `type, server, port` 的代理类型（`http` `https` `h2-connect` `socks5` `socks5-tls` `ss` `snell` `vmess` `trojan` `tuic` `tuic-v5` `hysteria2` `masque` `anytls` `trust-tunnel` `ssh`）策略行第 4 个起、凡不是 `name=value` 具名参数的 token（最常见的是根本不含 `=` 的裸 token，即位置传递的凭据；含 `=` 但值为空或全是 `=` 的也算，例如带填充的 base64。除前四种外这个位置本就是多余参数 `W0001`，按偏安全一侧抹掉）。取值的结尾按解析器自己的规则找**第一个顶层逗号**：`"` 或 `'` 在值里任何位置都会开启一段引号（`"` 内 `\` 转义下一个字符），`(` / `)` 分组，引号内与括号内的逗号都属于值（`password="p,w"`、`password=ab"c,d"`、`password=a(b,c)d` 各是一整个值）；引号或括号未闭合时抹到行尾。名单以外不脱敏；其余内容与行数、行尾 CRLF 原样保留 |
```

换成

```markdown
| `GET /v1/profiles/current?sensitive=0` | 当前配置文本（可脱敏） | 全部 | ✅ | 1 | M4a 已实现；`sensitive=0`（默认）脱敏：独立密钥行 `password` / `ca-passphrase` / `ca-p12` / `private-key` / `psk` / `pre-shared-key` / `token`；内联参数 `password` / `psk` / `private-key` / `pre-shared-key` / `base64` / `token` / `uuid` / `username` / `headers` / `ws-headers` / `ws-path` / `shadow-tls-password` / `policy-path` / `external-policy-modifier`（后两个自 M3a 起：订阅链接常带 token，修饰列表能设任何参数）/ `test-url`（自 M3b 起：订阅行设的测试 URL 可能带 token）/ `args`（自 M4c 起：`external` 的参数里常带口令）；`http-api` / `external-controller-access` / `http-listen` / `socks5-listen` 的 `key@` 前缀与 `wifi-access-http-auth` 口令；所有写作 `type, server, port` 的代理类型（`http` `https` `h2-connect` `socks5` `socks5-tls` `ss` `snell` `vmess` `trojan` `tuic` `tuic-v5` `hysteria2` `masque` `anytls` `trust-tunnel` `ssh`）策略行第 4 个起、凡不是 `name=value` 具名参数的 token（最常见的是根本不含 `=` 的裸 token，即位置传递的凭据；含 `=` 但值为空或全是 `=` 的也算，例如带填充的 base64。除前四种外这个位置本就是多余参数 `W0001`，按偏安全一侧抹掉）。取值的结尾按解析器自己的规则找**第一个顶层逗号**：`"` 或 `'` 在值里任何位置都会开启一段引号（`"` 内 `\` 转义下一个字符），`(` / `)` 分组，引号内与括号内的逗号都属于值（`password="p,w"`、`password=ab"c,d"`、`password=a(b,c)d` 各是一整个值）；引号或括号未闭合时抹到行尾。名单以外不脱敏；其余内容与行数、行尾 CRLF 原样保留 |
```

`docs/api/phase2.md`——把

```markdown
- 哈希对象是脱敏之后的文本，不是配置文件里的原始行。**只改动凭据（密码、`base64`、`psk`、`headers=`、`ws-headers=`、`ws-path=`、`shadow-tls-password=`、`test-url=` 等被脱敏的字段）不会改变 `lineHash`**（含 `password="p,w"`、`password=ab"c,d"`、`password=a(b,c)d` 这些带引号或带括号的值：取值按解析器的顶层逗号规则整体被抹掉，不会有尾巴漏进哈希）——因为脱敏后两行文本相同——这是有意的：`lineHash` 经这个公开的、无需鉴权之外任何权限的端点暴露，如果它是对原始定义取哈希，持有 API key 的人就能对着猜测的凭据反复计算哈希、离线核对是否猜中，等于把凭据的验证能力带出了进程。任何由凭据派生的东西都不允许离开 rurge 进程（`global-constraints.md`），`lineHash` 因此必须建立在脱敏后的文本上。
```

换成

```markdown
- 哈希对象是脱敏之后的文本，不是配置文件里的原始行。**只改动凭据（密码、`base64`、`psk`、`headers=`、`ws-headers=`、`ws-path=`、`shadow-tls-password=`、`test-url=`、`args=`（`external`，M4c 起）等被脱敏的字段）不会改变 `lineHash`**（含 `password="p,w"`、`password=ab"c,d"`、`password=a(b,c)d` 这些带引号或带括号的值：取值按解析器的顶层逗号规则整体被抹掉，不会有尾巴漏进哈希）——因为脱敏后两行文本相同——这是有意的：`lineHash` 经这个公开的、无需鉴权之外任何权限的端点暴露，如果它是对原始定义取哈希，持有 API key 的人就能对着猜测的凭据反复计算哈希、离线核对是否猜中，等于把凭据的验证能力带出了进程。任何由凭据派生的东西都不允许离开 rurge 进程（`global-constraints.md`），`lineHash` 因此必须建立在脱敏后的文本上。
```

`docs/api/phase2.md`——把

```markdown
- `GET /v1/policies/detail` 能查导入与派生的策略：导入的是订阅里那一行（前缀与 `external-policy-modifier` 已应用），派生的是其来源策略的定义加上 `underlying-proxy=<中继>`；脱敏规则同上，`policy-path`、`external-policy-modifier` 与 `test-url`（M3b 起）也在脱敏名单里。
```

换成

```markdown
- `GET /v1/policies/detail` 能查导入与派生的策略：导入的是订阅里那一行（前缀与 `external-policy-modifier` 已应用），派生的是其来源策略的定义加上 `underlying-proxy=<中继>`；脱敏规则同上，`policy-path`、`external-policy-modifier` 与 `test-url`（M3b 起）也在脱敏名单里；`external` 策略的每个 `args` 整体变成 `***`（M4c 起）。
```

`docs/acceptance/phase2-manual.md`——把

```markdown
- [ ] 日志（含 `--log-level verbose`）里搜不到私钥与 `preshared-key` 的内容；`GET /v1/profiles/current?sensitive=0` 里二者都是 `***`。

```

换成

```markdown
- [ ] 日志（含 `--log-level verbose`）里搜不到私钥与 `preshared-key` 的内容；`GET /v1/profiles/current?sensitive=0` 里二者都是 `***`。

## M4c　external

前置：本机能用 `ssh` 登录一台自己的服务器（密钥登录，不需要输入口令）。配置里写 `[Proxy]` 一条 `Ext = external, exec = "<ssh 的完整路径>", args = "-N", args = "-D", args = "127.0.0.1:1080", args = "<用户>@<服务器>", local-port = 1080`，`[Rule]` 里 `FINAL,Ext`；`rurge check -c <配置>` 零错误（没有 `W0007`）。三个平台各验一遍。

- [ ] 第一次用到时拉起：`rurge run -c <配置> --log-level info` 启动后没有 `ssh` 进程；`curl -x http://127.0.0.1:<http-listen 端口> https://example.com/ -I` 返回 200，此时有了 `ssh` 进程，日志里有 `external: the program started`（策略名与 pid，没有参数）。
- [ ] 日志文件：`<数据目录>/external/Ext.log` 里有一行 `--- rurge: starting the program …`，`ssh` 自己的输出（如有）在它后面。
- [ ] 再拉起：手动结束这个 `ssh` 进程，日志里有 `external: the program exited`；再发一个请求，2 秒左右后返回 200，出现了新的 `ssh` 进程。
- [ ] 停止：`rurge stop`（或 Ctrl-C）之后没有 `ssh` 进程残留（Windows：任务管理器；Unix：`ps`）；日志里有 `external: the program stopped`。
- [ ] Windows 上结束 rurge 进程（任务管理器里"结束任务"）：`ssh` 进程随之消失。
- [ ] 脱敏：`GET /v1/policies/detail?policy_name=Ext` 与 `GET /v1/profiles/current` 里每个 `args` 都是 `***`；日志（含 `--log-level verbose`）里搜不到服务器地址与用户名。
- [ ] 订阅：把一行 `external` 放进自己的订阅文件，重载后该行被跳过，`rurge check` 报 `` `external` policies are not imported from subscriptions ``。

```

新建 `tests/external/README.md`：

````markdown
# rurge-external-tests

`rurge-external-tests` 是仅供测试使用的工作区成员（`publish = false`），不对外发布，也不是任何其它 crate 的依赖。它带一个极小的 SOCKS5 服务端 `socks-helper`（`src/bin/socks-helper.rs`，只监听 127.0.0.1、不认证、只做 CONNECT），用例把它当作 `external` 策略的外部程序真实拉起：按需拉起与转发、参数顺序与代理变量、程序退出后的再拉起与 2 秒间隔、连接被拒时的重试、日志、停止时连同它启动的子进程一起结束（Unix 进程组、Windows Job Object）、经引擎的端到端、检查与构建不拉起、重载时沿用或替换程序。

辅助程序只存在于测试构建里：`cargo build -p rurge` 与发行的二进制不含它。用例不访问公网，不改本机的网络与代理设置。

```bash
cargo test -p rurge-external-tests
```
````

要点：
- `run_starts_an_external_program_and_stops_it_on_exit` 的外部程序是第二个 `rurge run`（`socks5-listen` 在策略的 `local-port` 上，自己的数据目录，`--no-network`），它继承外层的环境——包括测试夹具设的系统代理文件后端，所以它同样碰不到真实的系统代理；外层经 `POST /v1/stop` 退出后，断言内层的端口不再应答。
- 能力表的模块注释顺带补上漏写的 `wireguard`（M4b）。
- 兼容性清单里 `external` 一行改为 🟡：三平台都支持是超集，日志位置、拉起间隔、环境变量、重复端口是错误、进程树的清理方式、Windows 上每次连接限时 500 ms 与"Unix 上 rurge 崩溃后的孤儿不处理"都是差异。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge --test cli` → 50 passed（新增 `check_knows_external`、`run::run_starts_an_external_program_and_stops_it_on_exit`）。
Run: `cargo test -p rurge --bin rurge platform_` → 2 passed（新增 `platform_processes_delegates_to_rurge_platform`）。
Run: `tasklist | findstr /i rurge.exe`（Unix：`pgrep -x rurge`）→ 没有用例留下的 `rurge`。

- [ ] **Step 5: 门禁与提交**

跑门禁（51 个测试二进制，1165 通过 / 2 忽略）。分两次提交：

```bash
git add crates/rurge
git commit -m "feat(rurge): external 的进程控制适配器、退出时停掉外部程序；能力表翻转 external（W0007 不再因 external 出现）"
git add CLAUDE.md README.md README_en.md docs tests/external/README.md
git commit -m "docs: M4c external——兼容性清单、README、CLAUDE.md（第二个 unsafe 例外）、API 文档、手工验收与 tests/external 说明"
```


---

## 验收对照（设计第 11 节，external 部分）

| # | 验收项 | 由谁保证 |
| - | ------ | -------- |
| 1 | SSH 经 `FakeSsh` 动态转发 | 不在本计划（M4a） |
| 2 | WireGuard 握手并转发 TCP | 不在本计划（M4b） |
| 3 | `external` 进程退出后下次使用时自动再拉起；rurge 退出时整个进程树被清理（Unix 进程组、Windows Job Object） | Task 3：`a_program_that_exited_is_started_again`、`stopping_ends_the_whole_tree`、`dropping_the_outbound_stops_the_program`；Task 2：`terminating_the_tree_ends_the_program`、`killing_the_tree_ends_the_program`、`dropping_the_tree_ends_the_program_on_windows`；Task 5：`run_starts_an_external_program_and_stops_it_on_exit`（Unix 分支在 CI 上验证，P2） |
| 4 | `W0007` 不再因 `external` 出现；订阅安全门有用例 | Task 5：`check_knows_external`；Task 1：`a_subscription_never_brings_an_external_policy` |
| 5 | 门禁全绿（fmt / clippy 零警告 / `cargo test --workspace`） | 各任务的门禁 |
| 6 | 需要真实环境的项目进手工验收清单 | Task 5：`docs/acceptance/phase2-manual.md` 的 M4c 一节（用 `ssh -D` 当外部程序，三个平台），由项目所有者验收 |
| — | 第 10 节 external 第 1 层（日志路径与轮转、拉起间隔的判定） | Task 3：`a_log_is_named_after_its_policy`、`a_log_is_rotated_once_it_is_over_the_limit`、`a_program_that_cannot_start_fails_the_request` |
| — | 第 10 节 external 第 2 层（按需拉起与转发、`args` 顺序、退出后再拉起、连接被拒时的重试、日志内容、出站释放时整个进程树被停掉、干构建不拉起） | Task 3：`the_first_use_starts_the_program`、`a_program_that_exited_is_started_again`、`a_port_that_never_opens_fails_the_request`、`stopping_ends_the_whole_tree`、`dropping_the_outbound_stops_the_program`；Task 4：`the_first_dial_starts_the_program`、`a_check_or_a_build_starts_nothing`、`a_reload_keeps_an_unchanged_program_and_stops_a_replaced_one` |

## 执行期修正记录

| 任务 | 计划原文 | 实际做法 | 原因 | 提交 |
| ---- | -------- | -------- | ---- | ---- |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | Unix 上 rurge 崩溃（或被 SIGKILL）后留下的外部程序不处理：下次启动时同一端口被占，拉起的新程序监听失败（设计 7.3） | 有用户报告再说（已登记为差异）；可在数据目录记下进程组号，启动时清理 |
| 2 | Windows：程序在被放进 Job Object 之前（拉起后的几微秒内）启动的子进程不在 Job 里，停止时不会随之结束（P1） | 等 std 的 `PROC_THREAD_ATTRIBUTE_JOB_LIST`（`raw_attribute`）稳定后改为在 Job 里直接创建 |
| 3 | Unix：程序本身已被回收、组也已空时再给组号发信号，极小概率落到恰好复用了这个编号的别的进程组上（P6） | 有用户报告再说 |
| 4 | `rurge-platform::process` 与 `ExternalOutbound` 的 Unix 分支只在 CI（Linux / macOS）上编译与运行，本机验证不了（P2） | 首次推送后看 CI |
| 5 | `external` 的 `udp-relay`（M5）与 `addresses`、"外部进程的流量走 DIRECT"（阶段 3，要有 TUN） | M5 / 阶段 3 |
| 6 | 外部程序自己退出时，只在日志里记一条 `external: the program exited`；持续崩溃的程序每 2 秒被一个请求拉起一次，没有更长的退避 | 有用户报告再说 |
