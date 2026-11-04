# 阶段 4 / M6「收尾与验收」实施计划

> **执行方式**：交给 Pi，由协调的会话逐个任务派工、逐个任务评审（设计第 2、9 节，M6-D13、D14；做法同阶段 4 的 M1a ～ M5b）。
>
> - 协调的会话每个任务开一个 Pi 会话，交给它这个任务的全文（派工文件）。Pi 做完把报告写进派工文件指定的路径，在本地提交。
> - 协调者照设计第 9 节评审：名字、抽查、改坏验证的日志、覆盖、数。有问题回到同一个 Pi 会话修正，再评审；过了才派下一个任务。
> - 任务 0 交回之后，协调者先审切法（M6-D2），再派任务 1。
> - 当场补的（设计第 4.5 节）：先写会红的用例、在没有补的代码上看到它红、再补到绿；一处一个提交（**只在本地，不推送**）。
> - 与计划的出入记进末尾的「执行期修正记录」，**不改计划的正文**。
>
> 停点：任务 7 之后，协调者把 `phase4-m6` 快进合并进本地 `main`、请项目所有者推送（M6-D23）；任务 8 在推送之后做。

**目标**：

- 阶段 4 的需求逐句对照代码，结论写进新文件 `docs/acceptance/phase4-acceptance.md`（第一部分）；
- 总设计第 15 节七条验收标准，逐条写出证据（同一份文件的第二部分）；
- 手工清单 `docs/acceptance/phase4-manual.md` 的 68 项与兼容性清单里阶段栏含 4 的 68 行（今天的 67 行加任务 2 新加的一行），对着现在的代码核一遍；
- 重对报告（`docs/reports/2026-11-03-manual-release-notes-recheck.md`）交来的每一条有去处（设计第 3.5 节）；
- 对外的文档收尾；推送之后读 CI。

**不加功能，不做 M3c。**

**架构**：这是读的活，不是写的活。

- 验收文档只有一份（M6-D16）。任务 0 搭骨架：每一句一行，「结论」一格先写 `待读（任务 N）`。任务 1 ～ 5 各把落在自己那一片的行读完，把占位换成八种结论之一；任务 6 核两份清单；任务 7 写其余部分；任务 8 读 CI。
- 代码只在当场补的时候改（设计第 4.5 节）；另有两句过时的注释（评审要点第 3 条）。
- 名字的核对与结论的计数用工作区里的脚本（P12）。

**技术栈**：不加依赖。工作区的脚本是 Python，用 `python -I` 运行，只读仓库与 `git` 的输出。

**依据**：

- M6 设计（下面写"设计"指它）：`docs/superpowers/specs/2026-11-03-phase4-m6-acceptance-design.md`，全文；已决事项 M6-D1 ～ D29（D28、D29 是读设计的评审者提问之后补的）。**先读它第 15 节**：第 1、2 条是写本计划时改的。本计划不改设计的其余正文；「计划期决定」把设计的决定落到具体的行、命令与脚本上。设计与「计划期决定」不一致的，以计划为准。
- 先例：阶段 3 / M6 计划（`docs/superpowers/plans/2026-10-28-phase3-m6-acceptance-plan.md`）——本计划照它的结构写，它「执行期修正记录」里的判法照用，尤其第 9、10、12、16 行（`需求文字有问题` 与 `不属于阶段 3` 的分界、`有意不同` 与 `没有做完` 的分法、推过来的句子看兼容性清单那一行的阶段栏、阶段 N 做到了的推过来的句子不再记 `需求文字有问题`）；阶段 3 的验收文档 `docs/acceptance/phase3-acceptance.md`；阶段 1 的 `docs/acceptance/phase1-acceptance.md` 的「怎么读」与表的写法。
- 总设计 `docs/superpowers/specs/2026-10-29-phase4-http-engine-design.md` 第 1.2、1.4、1.5、9.1、14、15、17 节；M1 ～ M5 各自的设计与计划（任务 1 ～ 5 各自列出）。
- 手册的出处（设计第 4.7 节）：兼容性清单那一行的「Surge 行为」；`docs/reports/2026-10-29-phase4-manual-recheck.md`；`docs/reports/2026-10-28-manual-6.11-recheck.md`；重对报告；工作区的 `manual-1103/`。
- 写计划时对着 `7eed4b4` 核对过的事实（设计第 14 节十项）写在「计划期决定」里。`7eed4b4` 的代码树与 `bd981d2`、M5b 的 `b43278b` 相同。

## 全局约束

**不加东西**

- 不加外部包，不加工作区依赖边。`Cargo.lock` 的 `[[package]]` 今天 520 个，每个任务之后照旧；`cargo` 要下载任何东西就停下报告。
- `unsafe_code`：不加放开的函数。`crates/rurge-platform/tests/unsafe_functions.rs` 的 `LISTED` 仍是 23 项。
- `crates/rurge/tests/sockets.rs` 的 `LISTED` 仍是 26 行：补的生产代码不新建套接字、不问系统解析库、不建不带钩子的连接器。
- 不加功能、诊断码、命令行选项、API 端点、日志行。当场补的只能是设计第 4.5 节的尺度；要加这些的，登记。
- 不改已经登记的行为（M6-D5）：兼容性清单、`docs/api/phase4.md`、手工清单或某份设计的已决事项里写着的行为，要改它就是改登记——登记，交给项目所有者。
- 只补本机能编译、能运行的代码：要改的地方只在 Linux 或 macOS 上编译的（`cfg(unix)`、`cfg(target_os = "linux")`、`cfg(target_os = "macos")`；例如 `crates/rurge/src/cli/mitm.rs` 的 `restrict`），不改，登记。
- 不动 HTTP 引擎里 hyper 的设置：`crates/rurge-http/src/engine.rs` 的三处 `Builder::new()`（M5b 计划延后第 1 行：改不改归项目所有者）。不做 M3c（jq、`br`、`zstd`）。

**缺省的 `cargo test` 不碰系统**

- 不建网卡，不改系统的任何设置；不执行 `netsh`、`route`、`ipconfig`、`ifconfig`、`ip`、`resolvectl`、`networksetup`、`netstat`、`arp`、`sysctl`、`certutil`、`security`、`update-ca-certificates`、`trust`，不读真实的网络环境。
- **从不把任何 CA 装进信任库，也不读系统的信任库**：
  - 补的引擎用例一律给 `EngineShared.roots`（`crates/rurge-engine/src/shared.rs` 的 `roots`；写法照 `crates/rurge-engine/tests/mitm.rs` 的 `harness_mitm`）。不给的话 `crates/rurge-engine/src/runtime.rs` 的 `build_mitm` 会读系统的根。
  - 测试的客户端的 rustls 配置只信给它的根。
- 补的用例只用回环、各模块的假实现与用例自己的临时目录。

**测试与改坏验证**

- `CLAUDE.md`「测试约定」整节适用。写哪一块的用例之前，读 `docs/testing.md`「测试约定（全文）」里对应的一节：阶段 4 的是「阶段 4 / M1a 起：嗅探」到「阶段 4 / M5b 起：基准与遗留」十节，加「第 17 次 CI 运行起：同一个号上的 TCP 与 UDP、互操作用例失败时的说明」。
- 改坏验证照设计第 4.6 节：
  - 动手之前，在日志里写下这一改会让哪些会话、查找、下载与文件去哪里；可能离开回环或写到用例的临时目录之外的，不做。
  - 日志是工作区的 `task-N-mut-<序号>.log`：改了哪个文件的哪一行（改前、改后）、跑的命令、红的输出的摘录、改回之后 `git status --short` 为空。
  - 改回去用编辑工具，不用拷回旧的副本（它保留旧的修改时间，cargo 会接着跑旧的二进制）。
- 新写的带等待的用例，提交前单独循环跑二十次以上。

**拿真实的输入读**（设计第 4.8 节；P11）

- 只用三条不起守护进程的命令，二进制是 `target/debug/rurge.exe`（门禁会编好它；没有时 `cargo build --workspace --bins`）：
  - `rurge check -c <文件> --json --data-dir <工作区里的目录>`；
  - `rurge rule match -c <文件> <目标> --explain`，按需带 `--url`、`--user-agent`、`--sni`、`--http-host`、`--protocol`、`--udp`、`--listener`；每次都带 `--no-network`、`--data-dir <工作区里的目录>`、`--network-backend file:<工作区里一个不存在的文件>`，并带 `--resolve <地址>` 或 `--no-dns`；
  - `rurge mitm ca generate --out <工作区里已经建好的空目录>` 与 `rurge mitm ca export -c <文件> --data-dir <工作区里的目录> [--out <工作区里的文件> [--der]]`。`generate` 打印的装 CA 的命令**不执行**；生成的口令与私钥不进报告、不进仓库（报告里只写生成了哪几个文件、打印了哪几段，口令写成 `<口令>`）。
- Profile 片段里的名字只用保留的名字（`.test`、`.example`、`.invalid`），地址只用回环与文档地址；`dns-server` 写 `127.0.0.1`，不写 `system`、`syslib`。
- 不写远程的 `RULE-SET` / `DOMAIN-SET` / `#!include`、托管配置；`[Map Local]` 的 `data=` 只写工作区里的文件或 `.invalid` 下的 URL。
- 片段与输出放工作区的 `inputs/task-N/`，报告里摘要。

**门禁与这台开发机**（说"通过"之前必跑）

- 门禁：`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test -j 8 --workspace --no-fail-fast`。
- 本机 msvc 工具链没有 rustfmt。跑 `cargo fmt` 之前让环境变量 `RUSTFMT` 指向 gnu 工具链里同版本的 rustfmt：用户目录下的 `.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe`。不要为此安装或升级工具链。
- 聚焦地跑用例也经整个工作区：`cargo test -j 8 --workspace --lib -- <过滤词>`；可执行文件的单元用例 `--bins`；集成测试 `--test <名字>`。**不用 `-p`**（它另编一份依赖，写满过 D: 盘），不用 `--release`。
- 跑全量之前 `df -h /d`：剩余不足约 15 GiB 就停下报告。
- 测试二进制以 `STATUS_HEAP_CORRUPTION` 或 `STATUS_ACCESS_VIOLATION` 退出：单独重跑那一个二进制，报告里写明。rustc 在增量编译里报内部编译器错误：删掉出错的那个目标在 `target/debug/incremental/` 下的目录再编。
- 基线（P13）：94 个结果行、5453 通过、0 失败、33 忽略。补了用例的任务，通过数应当是基线加上新的用例，忽略数不变。
- 只改了文档的任务不跑测试：报告里写一行 `git diff --stat <任务开始的提交> HEAD`，只有 `docs/`、`CLAUDE.md`、`README.md`、`README_en.md` 下的文件。只改注释的提交（评审要点第 3 条的两句）跑 `cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings`，不跑测试。
- 结果照实记：本机的全量运行是"一台机器上的一次运行，不是 CI"；需要参照服务端的互操作用例在本机立刻返回、算在通过的数里，要说出来。

**仓库的规矩**（`CLAUDE.md`「必须遵守的规矩」整节适用；与本计划直接相关的）

- 照实记录：没有证据的写"没有"，不写成通过；没有在某个平台上编译、运行过的代码，照实说。
- 称齐备：照设计第 11 节，只在任务 7 写。照现在的代码，"阶段 4 的功能需求齐备"预期写不出来（FR-HTTP-07 的两句 P0 随 M3c）。
- 凭据：Profile 片段与文档的例子里不写真的凭据，口令写 `password` 一类；`generate` 生成的口令不进日志、报告与文档。
- 去向写着"项目所有者"的事（设计第 13 节，与读的时候查出来的）不定：登记，列出来。
- 记录：计划两张表里已写下的行不改写；阶段 1 ～ 3 的验收文档只在开头加一句（M6-D15、P17），正文不改；`docs/api/phase4.md` 各里程碑的历史段落不改。
- 文档用写文件 / 改文件的工具改。**永远不要用把一个文件的内容经管道写回同一个文件的命令**（`Get-Content x | Out-File x -Append` 一类；2026-10-27 这样写满过 D: 盘）。
- 行尾：新文件（验收文档、工作区的清单）用 LF。改已有的文档之前先看它在工作区里的行尾（数一数 `\r`），照它的：`CLAUDE.md`、`docs/status.md`、`docs/testing.md`、`docs/requirements.md`、兼容性清单、`docs/api/phase4.md` 与两份 README 在工作区里是 CRLF（仓库里是 LF，`core.autocrlf` 是 true），只在一行之内改；用脚本改它们时按字节读写、保住 CRLF。

**写法**

- 验收文档照 `phase3-acceptance.md` 的写法：
  - 一行一句，表的一格不换行；
  - 代码写 `路径::函数`；
  - 用例写"`路径`：`名字`（断言了什么）"，后面写 CI 的证据（P14）。
- 引号：验收文档、两份清单的改动与本计划两张表里的行只用直引号 `"…"` 与「」，不用弯引号。
- 文档里写里程碑带上阶段（"阶段 4 / M2a"），不写光秃秃的"M2a"。
- 不写本机路径；验收文档里不写流程用语（"派工""账本""Pi 会话"），读的人写"一个助手"，抽查的人写"协调者"。
- 补的代码：注释、日志与错误文本用英文，照所在文件的写法。注释里提到修正记录，写 `the phase 4 / M6 plan's execution record row N`。
- 提交说明照 `git log` 的样子：`docs: …`、`docs(<crate>): …`（只改注释）、`test(<crate>): …`、`fix(<crate>): …`，标题用中文；Pi 的提交不带署名行。只在本地提交：不 `push`、不 `pull`、不 `fetch` 远端、不 `amend`、不改写历史。

**不做的事**

- 只运行本仓库构建出来的二进制与 Pi 自己。不下载、不安装任何东西（含 `rustup target add` 与 h2spec）。
- 不手动跑 `rurge run`、`rurge dns lookup`；`rurge service` 只用 `--dry-run`。手工清单里的项一项也不执行（那是项目所有者在真机上做的）。
- 不跑 release 的构建与基准（M6-D22）：NFR-01 的数照 M5b 的记录引（P8）。
- 参考实现：GPL 的（sing-box、mihomo、Xray-core）只取事实，代码不抄。

**计划没有写到、要做决定的地方**

- 能按设计与总设计的原则定的：自己定，记进修正记录，写"执行者的裁定"。
- 去向写着"项目所有者"的，或会改变 M6-D1 ～ D29 的：停下来列出，交给协调者。
- 现有用例变红：停下来，记进修正记录，不顺手改期望值。

## 评审要点

设计没有写、而最可能让验收文档说错话的几种情形，与负责它们的任务：

1. **用例只证明了"跑通了"**。例如一条经三种入口解密的用例只看了应答的状态码，却被当成"按主机名缓存叶子证书"的证据；一条抓包的用例只数了条数，却被当成"单条上限可配"的证据。
   - 期望：结论凭断言下；读不出来的做改坏验证（设计第 4.6 节）。
   - 负责：任务 1 ～ 5；评审第 2、3 件。
2. **只在 Unix 上编译的代码与读系统信任库的那一层**。`crates/rurge/src/cli/mitm.rs` 的 `restrict` 与它的用例 `crates/rurge/tests/cli.rs` 的 `mitm_ca_generate_keeps_the_secrets_private` 都是 `#[cfg(unix)]`；解密的上游信"系统的根 ∪ `webpki-roots`"（`build_mitm`），缺省的用例从不走到读系统的根。
   - 期望：没有能红的用例的记 `做了，本机没有能红的用例`；有用例的写明平台与 CI 的证据（P14）。
   - 负责：任务 2。
3. **文档说的比代码多或少**。兼容性清单阶段栏含 4 的行里有 🟡 与写着"随 M3c""随 M5"一类的；手工清单 M1 ～ M4 的项写在阶段 4 / M5a 之前（之后 QUIC 流被读出 SNI、被 `auto-quic-block` 阻断）；两份 README 的阶段 4 一段；`crates/rurge-config/src/mitm.rs` 第 40 行（`h2`："read only until M2b"）与第 44 行（`auto_quic_block`："read only until M5"）两句注释已经过时。
   - 期望：清单是现行的登记，照现在的代码改；历史的段落不改；两句注释由任务 2、任务 5 各改一句（`docs(config):`，只改注释）。
   - 负责：任务 2、5、6a、6b、7。
4. **照着手工清单做会改坏真机**：把测试的 CA 装进系统或浏览器的信任库、开系统代理。
   - 期望：每一项改系统的都写着前提（一次性的虚拟机或能还原的机器）与怎样恢复（从信任库删掉那张 CA、关掉系统代理）；没有的补上。
   - 负责：任务 6b。
5. **数字对不上**：三个数与第四个数、表、验收文档与 README / 需求文档 / `CLAUDE.md` / `docs/status.md` 引的数；预期的两句 P0 `没有做完（已登记）`（jq 表达式；"解压"之 `br`、`zstd`）。
   - 期望：数由 `tools/count_rows.py` 从表里数出来，五处引的都是同一组数。
   - 负责：任务 7。

---

## 文件结构

| 文件 | 任务 | 职责 |
| --- | --- | --- |
| `docs/acceptance/phase4-acceptance.md`（新） | 0 ～ 8 | 对照表、七条标准与其余各节 |
| 补的用例与修复（读的时候才知道在哪里） | 1 ～ 6a | 设计第 4.5 节 |
| `crates/rurge-config/src/mitm.rs` 的两句注释 | 2、5 | 评审要点第 3 条 |
| `docs/surge-compatibility-matrix.md` | 2、4、5、6a | 任务 2 在 7.2 节加「叶证书的密钥」一行，任务 4 在 10.4 节加抓包过滤器一行，任务 5 在 `block-quic` 与 `auto-quic-block` 两行登记 UDP ASSOCIATE 上的不同（读出不同时）；任务 6a 核 68 行与附录 |
| `docs/acceptance/phase4-manual.md` | 6b | 就地订正、「M6」一节、开头的说明与索引表 |
| `README.md`、`README_en.md`、`docs/requirements.md`（第 7 节的状态）、`CLAUDE.md`（「现在在哪里」）、`docs/status.md`、`docs/README.md`、`docs/acceptance/phase1-acceptance.md`、`phase2-acceptance.md` 与 `phase3-acceptance.md`（开头各一句） | 7 | 收尾 |
| `docs/api/phase4.md`、`docs/testing.md` | 1 ～ 7 | 只在读的时候查出与代码不符时改（设计第 10 节） |
| 工作区的 `tools/`、`lists/`、`inputs/` | 0 ～ 7 | 脚本（P12）、四张清单、真实输入的片段与输出 |
| 本计划的两张表 | 全部 | 记录 |

---

## Task 0：骨架

**做什么**：

1. **核对**：
   - `git status --short` 干净；分支 `phase4-m6`，`git log --oneline bd981d2..HEAD` 只有 M6 的设计（`7eed4b4`）与本计划的提交；
   - `Cargo.lock` 520 个包，`unsafe_functions.rs` 的 `LISTED` 23 项，`sockets.rs` 的 `LISTED` 26 行；
   - `python -I tools/stage4_rows.py docs/surge-compatibility-matrix.md` 给 67 行，手工清单 68 项（P9）；
   - 不跑门禁（P13）。
   - 推过来的句子（M6-D19）：把 `docs/acceptance/phase1-acceptance.md` 与 `docs/acceptance/phase3-acceptance.md` 里结论是「不属于阶段 N」与「需求文字有问题」的每一行逐行看一遍（括号里写"HTTP 引擎""MITM""抓包""重写"而没有写阶段号的也算），再看 `docs/acceptance/phase2-acceptance.md` 的「需求逐条对照代码」一节，核对 P3 的名单。工作区的 `lists/pushed-grep.txt` 是协调者在 `bd981d2` 上 grep 的结果（43 行），逐行对上 P3 的两类（推了句子的、只提到的）。名单之外还有推到阶段 4 的句子的，照 P4 加进骨架，记一行修正记录。
2. **骨架**：新建 `docs/acceptance/phase4-acceptance.md`，照下面「骨架的样子」与「切法」写。
3. **四张清单**，放工作区的 `lists/`：
   - `matrix-rows.md`：兼容性清单阶段栏含 4 的 67 行——行号、节、项、阶段、状态标记（`tools/stage4_rows.py --list` 给这五栏）、读它的任务（6a 都读；第一部分里有对应的句子的，写那一句的编号）。任务 2 加了「叶证书的密钥」一行之后，由任务 2 在表末补第 68 行；任务 4 加的 10.4 节抓包过滤器一行（阶段栏"待定"）由任务 4 补在表末，标"不在 68 行里"。
   - `manual-items.md`：手工清单的 68 项——`节/编号`、所在的节与平台、行号、一句话的标题（`python -I tools/gen_manual_items.py lists/manual-items.md` 写表身，前面加表头）。
   - `pushed.md`：P3 列的每一行——出处（文件:行号）、需求、那一句、当时的结论、并进了骨架的哪一行（或"只提到"与理由）。
   - `golden.md`：手册的重写示例（P10）——`python -I tools/manual_examples.py manual-1103` 抽出的 18 个代码块，加 P10 列的 7 处正文里的写法，共 25 行；栏是 `| # | 页 | 行 | 示例（原文） | 用例 | 用例断言了什么 |`，后两栏空着，由任务 3b 填。
4. **记录**：「执行期修正记录」写一行，写行数与各任务的行数；切法与本计划不同的地方一项一行（应当没有）。

### 骨架的样子

章节照设计第 5 节：

1. `## 怎么读`：先放六列（设计第 4.2 节）、"真机上"一列（第 4.3 节）、八种结论与两者的分界、"用例一格里 CI 的证据"一段（第 4.4 节）的定义，照抄设计的字（弯引号换成直引号；"给以后能在那个平台上跑的会话（或所有者）补"一类说流程的话照阶段 3 的写法换成"助手"）；其余由任务 7 写。
2. `## 结论`：只写"（任务 7 写）"。
3. `## 第一部分：需求逐句对照代码`，下面依次是：
   - 会话入口与 HTTP 引擎（FR-HTTP-01、02）；MITM（FR-HTTP-03、04、14）；重写与 Map Local（FR-HTTP-05 ～ 09）；请求记录与抓包（FR-HTTP-10、11，FR-RULE-14）；功能开关（FR-HTTP-13）；配置与观测（FR-CFG-06、FR-CFG-18、FR-OBS-02）；
   - 总设计第 1.2 节另列的；总设计第 1.5 节的遗留；
   - 阶段 1 ～ 3 推过来的；
   - 非功能需求；
   - 数字（任务 7 写）；
   - 没有做完的、需求文字的问题、有意不同的、本机没有能红的用例的——四张表先只有表头（P18）。
4. `## 第二部分：总设计第 15 节七条验收标准`：只写"（任务 7 写）"，第 1 条之下先放「手册的重写示例与黄金用例」一张表（P10）：`| 页 | 示例（原文） | 用例 | 用例断言了什么 |`，25 行，后两栏空着（任务 3b 填）。
5. `## 靠手工验收的`、`## 靠 CI 的`、`## 留给项目所有者的`：只写"（任务 7 写）"。

对照的表都是六列：`| 需求 | 这一句 | 结论 | 代码 | 用例 | 真机上 |`。

- 「需求」一格：需求编号；推过来的那张表写编号与出处（例如 `FR-CFG-12（阶段 1 对照第 126 行）`）；总设计第 1.2、1.5 节的两张小表写 `总设计第 1.2 节（FR-RULE-01）` 一类——括号里是这一句算在哪条需求下（P6）；FR-CFG-06 的几行写 `FR-CFG-06（兼容性清单 2.1 节）`；非功能需求写 `NFR-01` 一类。下面切法的「需求」一栏写着出处的，照抄进「需求」一格。
- 「阶段 1 ～ 3 推过来的」一节，表之前写一句：这张表只列没有并进别的表的句子；并进去的，出处写在那一行的「需求」一格里（P3、P4）。
- 「这一句」一格：用需求自己的话引，长的用 `…` 缩短；总设计第 1.2 节的两行引兼容性清单 3.1 节那两行的话，括号里写清单的节；第 1.5 节的两行引总设计第 1.5 节那一行的话。
- 「结论」一格：`待读（任务 N）`。
- 别的三格空着。指向另一行的行（P4），「代码」一格先写"同「<表>」的「<那一句>」一行"，「用例」一格写"同上"。

### 切法

照设计第 4.1 节，下表是定下的切法：

- 「#」是骨架里的次序；编号不进文档。
- 「任务」一栏写 3a、3b 的，是任务 3 分两次派的哪一次（P20）；占位一律写 `待读（任务 3）`（任务号不变，脚本只认数字）。
- 「备注」里写"指向 #n"的行照 P4 处理；写"重对报告第 N 条"的，是设计第 3.5 节交给读这一行的任务对着读的那一条（重对报告第 3 节的编号）。
- "预期"是写计划时的判断（P19），读的时候以代码为准。

**会话入口与 HTTP 引擎（FR-HTTP-01、02）**

| # | 需求 | 这一句 | 任务 | 备注 |
| - | ---- | ------ | ---- | ---- |
| 1 | FR-HTTP-01 | 「HTTP 引擎：HTTP/1.1 请求 / 响应解析与转发」 | 1 | 重对报告第 5 条（`Proxy-Authorization` 不转发） |
| 2 | FR-HTTP-01 | 「keep-alive」 | 1 | 重对报告第 3 条（上游在 304 之后多送正文） |
| 3 | FR-HTTP-01 | 「chunked」 | 1 | 重对报告第 9 条（chunked 正文之后紧跟管线化的请求） |
| 4 | FR-HTTP-01 | 「WebSocket 升级透传」 | 1 | |
| 5 | FR-HTTP-01 | 「80 / 443 协议嗅探」 | 1 | |
| 6 | FR-HTTP-01 | 「`force-http-engine-hosts`」 | 1 | |
| 7 | FR-HTTP-01 | 「`always-raw-tcp-hosts`」 | 1 | |
| 8 | FR-HTTP-01 | 「`always-raw-tcp-keywords`」 | 1 | |
| 9 | FR-HTTP-02 | 「HTTP/2：MITM 后与客户端…使用 HTTP/2（`h2=true`）」 | 2 | 重对报告第 4、6 条 |
| 10 | FR-HTTP-02 | 「…/ 上游使用 HTTP/2…」 | 2 | 重对报告第 7 条 |
| 11 | FR-HTTP-02 | 「`:authority` 提取」 | 2 | 读两件：拼请求的 URL，规则与 `extended-matching` 用它；#139 指向它（P4） |

**MITM（FR-HTTP-03、04、14）**

| # | 需求 | 这一句 | 任务 | 备注 |
| - | ---- | ------ | ---- | ---- |
| 12 | FR-HTTP-03 | 「`[MITM]` 九个键」之 `hostname` | 2 | Host List 与别的节共用是 #131 |
| 13 | FR-HTTP-03 | 之 `hostname-disabled` | 2 | |
| 14 | FR-HTTP-03 | 之 `ca-p12` | 2 | |
| 15 | FR-HTTP-03 | 之 `ca-passphrase` | 2 | 凭据（`crates/rurge-config/tests/secrets.rs`） |
| 16 | FR-HTTP-03 | 之 `ca-keystore-name` | 2 | |
| 17 | FR-HTTP-03 | 之 `skip-server-cert-verify` | 2 | |
| 18 | FR-HTTP-03 | 之 `h2` | 2 | 指向 #9 |
| 19 | FR-HTTP-03 | 之 `client-source-address` | 2 | MAC 地址的写法在阶段 7（兼容性清单 7.2 节那一行） |
| 20 | FR-HTTP-03 | 之 `auto-quic-block` | 5 | 指向 #34 |
| 21 | FR-HTTP-03 | 「按主机名签发…叶子证书」 | 2 | 重对报告第 1 条：在兼容性清单 7.2 节加「叶证书的密钥」一行（P21），这一句的结论照常下 |
| 22 | FR-HTTP-03 | 「…并缓存叶子证书」 | 2 | |
| 23 | FR-HTTP-03 | 「靠嗅探到的 SNI 认出的 TLS…连接，不带端口的项不看端口」 | 2 | |
| 24 | FR-HTTP-03 | 「…与 QUIC 连接，不带端口的项不看端口」 | 5 | |
| 25 | FR-HTTP-03 | 「`client-source-address`」 | 2 | 指向 #19 |
| 26 | FR-HTTP-03 | 「`skip-server-cert-verify`」 | 2 | 指向 #17 |
| 27 | FR-HTTP-03 | 「证书固定提示日志」 | 2 | |
| 28 | FR-HTTP-03 | 「CA 通过 `ca-p12`…提供」 | 2 | 指向 #14 |
| 29 | FR-HTTP-03 | 「…/ `ca-keystore-name` 提供」 | 2 | 指向 #16 |
| 30 | FR-HTTP-03 | 「`rurge mitm ca generate` 生成 CA」 | 2 | |
| 31 | FR-HTTP-03 | 「…并给出各平台信任安装指引」 | 2 | |
| 32 | FR-HTTP-03 | 「`GET /v1/mitm/ca` 导出 DER」 | 2 | |
| 33 | FR-HTTP-04 | 「`auto-quic-block`：解析 QUIC Initial 的 SNI」 | 5 | 与 #129、#130 不是同一句（P4） |
| 34 | FR-HTTP-04 | 「命中 MITM 列表时阻断 UDP 使客户端回落」 | 5 | 重对报告第 2.3 节：经 SOCKS5 的 UDP ASSOCIATE 来的流 |
| 35 | FR-HTTP-14 | 「`http-api-tls`：API 使用 MITM CA 签发的证书提供 HTTPS」 | 2 | |

**重写与 Map Local（FR-HTTP-05 ～ 09）**

| # | 需求 | 这一句 | 任务 | 备注 |
| - | ---- | ------ | ---- | ---- |
| 36 | FR-HTTP-05 | 「URL Rewrite：`header`」 | 3a | |
| 37 | FR-HTTP-05 | 「…`302`」 | 3a | |
| 38 | FR-HTTP-05 | 「…`307`」 | 3a | |
| 39 | FR-HTTP-05 | 「…`reject`」 | 3a | |
| 40 | FR-HTTP-05 | 「首个 header 规则生效」 | 3a | |
| 41 | FR-HTTP-05 | 「结果 URL 合法性」 | 3a | |
| 42 | FR-HTTP-05 | 「`{{{GATEWAY_ADDRESS}}}`」 | 3a | |
| 43 | FR-HTTP-05 | 「对 Host…重建的 URL 也匹配」 | 3a | |
| 44 | FR-HTTP-05 | 「对…连接主机名重建的 URL 也匹配」 | 3a | |
| 45 | FR-HTTP-06 | 「Header Rewrite：方向」 | 3a | |
| 46 | FR-HTTP-06 | 「四种动作」之 `header-add` | 3a | |
| 47 | FR-HTTP-06 | 之 `header-del` | 3a | |
| 48 | FR-HTTP-06 | 之 `header-replace` | 3a | |
| 49 | FR-HTTP-06 | 之 `header-replace-regex` | 3a | |
| 50 | FR-HTTP-06 | 「顺序生效」 | 3a | |
| 51 | FR-HTTP-06 | 「改动消息框架头则拒绝…」 | 3a | |
| 52 | FR-HTTP-06 | 「…并使请求失败」 | 3a | |
| 53 | FR-HTTP-07 | 「Body Rewrite：正则多对替换」 | 3b | |
| 54 | FR-HTTP-07 | 「按行锚定」 | 3b | |
| 55 | FR-HTTP-07 | 「`^$` 生成正文」 | 3b | |
| 56 | FR-HTTP-07 | 「UTF-8 检查」 | 3b | |
| 57 | FR-HTTP-07 | 「jq 表达式」 | 3b | 预期 `没有做完（已登记）`（M3c，等总设计 Q4；今天那一行给 `W0042` 并跳过） |
| 58 | FR-HTTP-07 | 「大小限制（请求 32 MB 断开…）」 | 3b | |
| 59 | FR-HTTP-07 | 「（…响应 10 MB 直通）」 | 3b | |
| 60 | FR-HTTP-07 | 「chunked…限制」 | 3b | |
| 61 | FR-HTTP-07 | 「…`Expect` 限制」 | 3b | |
| 62 | FR-HTTP-07 | 「解压…」之 `gzip`、`x-gzip`、`deflate` | 3b | 设计第 3.1 节 |
| 63 | FR-HTTP-07 | 之 `br`、`zstd` | 3b | 预期 `没有做完（已登记）`（总设计 P4-D13 与 M3 设计把它们定给 M3c）；兼容性清单第 620 行的 ✅ 不动，任务 6a 在它的备注里补一句 |
| 64 | FR-HTTP-07 | 「…与 `Content-Length` 重算」 | 3b | 重对报告第 8 条 |
| 65 | FR-HTTP-07 | 「先于脚本」 | 3b | 预期 `需求文字有问题`（P19） |
| 66 | FR-HTTP-08 | 「Map Local：四种 `data-type`」之 `file` | 3b | |
| 67 | FR-HTTP-08 | 之 `text` | 3b | |
| 68 | FR-HTTP-08 | 之 `tiny-gif` | 3b | |
| 69 | FR-HTTP-08 | 之 `base64` | 3b | |
| 70 | FR-HTTP-08（总设计第 1.5 节） | 「`data` 路径…」 | 3b | 总设计第 1.5 节"Map Local 的 `data=` 路径先问 `Reach`"并进这一行（设计第 3.4 节） |
| 71 | FR-HTTP-08 | 「…/ URL…」 | 3b | |
| 72 | FR-HTTP-08 | 「…/ 文本…」 | 3b | 指向 #67 |
| 73 | FR-HTTP-08 | 「…/ Base64」 | 3b | 指向 #69 |
| 74 | FR-HTTP-08 | 「`header` 两种形式」之 `a:b\|c:d` | 3b | |
| 75 | FR-HTTP-08 | 之 Base64 编码的多行头 | 3b | |
| 76 | FR-HTTP-08 | 「`status-code` 200–999」 | 3b | |
| 77 | FR-HTTP-08 | 「`Content-Type` 补全」 | 3b | |
| 78 | FR-HTTP-08 | 「需 Rewrite 功能开启」 | 3b | |
| 79 | FR-HTTP-09 | 「流水线顺序 Header Rewrite → URL Rewrite → Body Rewrite…」 | 3b | |
| 80 | FR-HTTP-09 | 「…→ 脚本」 | 3b | 预期 `需求文字有问题`（P19） |
| 81 | FR-HTTP-09 | 「Map Local 短路」 | 3b | |
| 82 | FR-HTTP-09 | 「每请求 / 响应最多一个脚本」 | 3b | 预期 `需求文字有问题`（P19） |

**请求记录与抓包（FR-HTTP-10、11，FR-RULE-14）**

| # | 需求 | 这一句 | 任务 | 备注 |
| - | ---- | ------ | ---- | ---- |
| 83 | FR-HTTP-10 | 「请求记录：最近请求…与活动请求」之经 HTTP 引擎的每个请求各一条 | 4 | 总设计第 9.1 节给"完整"定的 |
| 84 | FR-HTTP-10 | 「字段包含…目标…状态…」之经引擎的请求的方法、URL 与状态码 | 4 | 同上 |
| 85 | FR-HTTP-10（阶段 1 对照第 410 行；阶段 3 对照第 250 行） | 「…来源（…进程…）」之经引擎的每个请求 | 4 | |
| 86 | FR-HTTP-10（阶段 1 对照第 410 行；阶段 3 对照第 251 行） | 「…来源（…设备）」 | 4 | 预期 `不属于阶段 4`（P19） |
| 87 | FR-HTTP-10（阶段 1 对照第 417 行；阶段 2 验收第 527 行；总设计第 1.5 节） | 「…远端地址」之复用的连接、隧道与 UDP 流 | 4 | |
| 88 | FR-HTTP-10 | 「…上下行流量」之两个正文的字节数 | 4 | |
| 89 | FR-HTTP-10 | 「…各阶段耗时」之 `receiveMs` | 4 | |
| 90 | FR-HTTP-10（总设计第 1.5 节） | 之 `dnsMs` 在解析途中 | 4 | |
| 91 | FR-HTTP-10（阶段 1 对照第 421 行） | 「…备注（重写…）」 | 4 | |
| 92 | FR-HTTP-10（阶段 1 对照第 421 行） | 「…备注（…脚本…）」 | 4 | 预期 `不属于阶段 4`（P19） |
| 93 | FR-HTTP-10（阶段 1 对照第 421 行） | 「…备注（…直通原因）」 | 4 | 九种原因（阶段 4 / M4b） |
| 94 | FR-HTTP-10 | 「终止活动请求」之经引擎的请求 | 4 | |
| 95 | FR-HTTP-11 | 「抓包：请求 / 响应头…保存」 | 4 | |
| 96 | FR-HTTP-11 | 「…与正文保存」 | 4 | |
| 97 | FR-HTTP-11 | 「（总容量…可配）」 | 4 | |
| 98 | FR-HTTP-11 | 「（…单条上限可配）」 | 4 | |
| 99 | FR-HTTP-11 | 「命名会话」 | 4 | |
| 100 | FR-HTTP-11 | 「`always-capture`」 | 4 | 指向 #103 |
| 101 | FR-HTTP-11 | 「通过 API…查看」 | 4 | 抓包过滤器在兼容性清单 10.4 节加一行（P21），这一句的结论照常下 |
| 102 | FR-HTTP-11 | 「通过…Dashboard 查看」 | 4 | 预期 `需求文字有问题`（P19） |
| 103 | FR-RULE-14 | 「`always-capture=<session>`：强制抓包…」 | 4 | |
| 104 | FR-RULE-14 | 「…并归入命名会话」 | 4 | |

**功能开关（FR-HTTP-13）**（三行都看阶段 1 对照第 439 行：那时除 `system_proxy` 之外的 `POST` 回 501）

| # | 需求 | 这一句 | 任务 | 备注 |
| - | ---- | ------ | ---- | ---- |
| 105 | FR-HTTP-13 | 「功能开关 `mitm`…运行时切换并持久化」 | 2 | |
| 106 | FR-HTTP-13 | 「…`rewrite`…」 | 3a | |
| 107 | FR-HTTP-13 | 「…`capture`…」 | 4 | |

**配置与观测（FR-CFG-06、FR-CFG-18、FR-OBS-02）**

| # | 需求 | 这一句 | 任务 | 备注 |
| - | ---- | ------ | ---- | ---- |
| 108 | FR-CFG-06（兼容性清单 2.1 节；阶段 1 对照第 80 行） | 「…各键随所属功能阶段生效」之 `http-api-tls` | 2 | 指向 #35 |
| 109 | FR-CFG-06（兼容性清单 2.1 节） | 之 `force-http-engine-hosts` | 1 | 指向 #6 |
| 110 | FR-CFG-06（兼容性清单 2.1 节） | 之 `always-raw-tcp-hosts` | 1 | 指向 #7 |
| 111 | FR-CFG-06（兼容性清单 2.1 节） | 之 `always-raw-tcp-keywords` | 1 | 指向 #8 |
| 112 | FR-CFG-06（兼容性清单 2.1 节） | 之 `show-error-page`（经引擎的请求上） | 1 | 清单的阶段栏写 1 / 4 |
| 113 | FR-CFG-06（兼容性清单 2.1 节） | 之 `show-error-page-for-reject`（经引擎的请求上） | 1 | 阶段 1 对照第 387、388 行引的 `crates/rurge-inbound/src/http.rs::forward` 在阶段 4 / M1b 退役了；`REJECT` 与 `REJECT-TINYGIF` 在经引擎的请求上的样子读 `crates/rurge-engine/tests/pipeline.rs` 的 `reject_rules_close_serve_gifs_or_render_pages` |
| 114 | FR-CFG-18（阶段 1 对照第 162 行） | 「Header Rewrite 省略方向」 | 3a | |
| 115 | FR-OBS-02 | 「路径、方法、JSON 字段与 Surge 一致（清单 10.4）」之阶段 4 / M1a 加的（`docs/api/phase4.md` 的「M1a」一节） | 1 | P16 |
| 116 | FR-OBS-02 | 之阶段 4 / M1b 加的（同一份文件的「M1b」一节） | 1 | |
| 117 | FR-OBS-02 | 之阶段 4 / M2a 加的（「M2a」一节） | 2 | |
| 118 | FR-OBS-02 | 之阶段 4 / M2b 加的（「M2b」一节） | 2 | |
| 119 | FR-OBS-02 | 之阶段 4 / M2c 加的（「M2c」一节） | 2 | |
| 120 | FR-OBS-02 | 之阶段 4 / M3a 加的（「M3a」一节） | 3a | |
| 121 | FR-OBS-02 | 之阶段 4 / M3b 加的（「M3b」一节） | 3b | |
| 122 | FR-OBS-02 | 之阶段 4 / M4a 加的（「M4a」一节） | 4 | |
| 123 | FR-OBS-02 | 之阶段 4 / M4b 加的（「M4b」一节） | 4 | |
| 124 | FR-OBS-02 | 之阶段 4 / M5a 加的（「M5a」一节） | 5 | |

**总设计第 1.2 节另列的**（设计第 3.2 节；句子取自兼容性清单 3.1 节 `USER-AGENT`、`URL-REGEX` 两行，按 FR-RULE-01 算 P0）

| # | 需求 | 这一句 | 任务 | 备注 |
| - | ---- | ------ | ---- | ---- |
| 125 | 总设计第 1.2 节（FR-RULE-01） | 「`USER-AGENT`：HTTP User-Agent，`*` `?` 通配，区分大小写」（3.1 节）之引擎里的每个请求 | 1 | 阶段 1 对照第 181、182 行只对 HTTP 监听器转发的明文请求读过 |
| 126 | 总设计第 1.2 节（FR-RULE-01） | 「`URL-REGEX`：在完整 URL 里搜索的正则…」（3.1 节）之引擎里的每个请求 | 1 | 同上 |

**总设计第 1.5 节的遗留**（设计第 3.4 节；句子取自总设计第 1.5 节那一行）

| # | 需求 | 这一句 | 任务 | 备注 |
| - | ---- | ------ | ---- | ---- |
| 127 | 总设计第 1.5 节（FR-HTTP-01） | 「经监听器来的绝对形式明文请求里，主机是 fake 地址时保留着它」 | 1 | R27 |
| 128 | 总设计第 1.5 节（FR-OUT-01） | 「HTTP 一侧的 `REJECT-DROP` 看不到客户端提前关闭」 | 1 | 阶段 1 对照第 389 行（`有意不同（已登记）`，兼容性清单 4.1 节 `REJECT-DROP` 一行）引的 `forward` 已退役 |
| 129 | 总设计第 1.5 节（FR-RULE-07） | 「QUIC 里的 SNI、跨几个包的 ClientHello…」 | 5 | 重对报告第 2 条；阶段 3 / M5 计划延后第 6 行 |
| 130 | 总设计第 1.5 节（FR-RULE-07） | 「…按 SNI 匹配规则」 | 5 | 同上 |

**阶段 1 ～ 3 推过来的**（P3、P4；「需求」一格写编号与出处）

| # | 需求 | 这一句 | 任务 | 备注 |
| - | ---- | ------ | ---- | ---- |
| 131 | FR-CFG-12（阶段 1 对照第 126 行） | 「供…MITM `hostname` 等共用」 | 2 | |
| 132 | FR-CFG-14（阶段 1 对照第 131 行） | 「…Map Local 数据」 | 3b | |
| 133 | FR-RULE-02（阶段 1 对照第 204 行） | 「…参数」之 `extended-matching` | 1 | |
| 134 | FR-RULE-02（阶段 1 对照第 210 行） | 「…参数」之 `always-capture` | 4 | 指向 #103 |
| 135 | FR-RULE-04（阶段 1 对照第 232 行；总设计第 1.5 节） | 「…文件格式约束」之行内可带 `extended-matching` | 1 | 清单 L314 |
| 136 | FR-RULE-04（阶段 1 对照第 241 行） | 「…行级参数作用于整集」之 `extended-matching` | 1 | |
| 137 | FR-RULE-07（阶段 1 对照第 257 行；阶段 2 验收第 564 行；总设计第 1.5 节） | 「`extended-matching`：从 TLS ClientHello 提取 SNI」 | 1 | 拨号之前（M8c 计划延后第 105 行） |
| 138 | FR-RULE-07（阶段 1 对照第 258 行；阶段 3 对照第 247 行） | 「…从 HTTP/1.1 Host…提取主机名」 | 1 | 连经虚拟网卡来的明文 HTTP |
| 139 | FR-RULE-07（阶段 1 对照第 259 行） | 「…HTTP/2 `:authority`…提取主机名」 | 2 | 指向 #11 |
| 140 | FR-RULE-08（阶段 1 对照第 261 行；阶段 2 验收第 564 行） | 「协议识别：HTTP」之 CONNECT 隧道或 SOCKS5 连接里的明文 HTTP | 1 | |
| 141 | FR-RULE-08（阶段 1 对照第 262 行） | 「…HTTPS（TLS）」 | 1 | |
| 142 | FR-CFG-11（阶段 2 验收第 574 行） | 「…被…`ca-keystore-name`…引用」 | 2 | 指向 #16。阶段 2 验收第 574 行："`[MITM]` 的 `ca-keystore-name` 不对照 `[Keystore]` 检查（FR-CFG-11，阶段 4）" |

**非功能需求**（设计第 3.6 节）

| # | 需求 | 这一句 | 任务 | 备注 |
| - | ---- | ------ | ---- | ---- |
| 143 | NFR-01 | 「目标值（阶段 1 与阶段 3 建立基准后校准）」之经 HTTP 引擎的路径 | 5 | 预期 `没有做完（已登记）`（M6-D24；P8） |
| 144 | NFR-03 | 「MITM CA 私钥与 Keystore 材料文件权限仅属主可读」之 Unix | 2 | |
| 145 | NFR-03 | 之 Windows | 2 | |
| 146 | NFR-07 | 「平台特定代码隔离在 `rurge-platform` 与 `rurge-tun`」之阶段 4 加的 | 5 | P7 |
| 147 | NFR-07 | 「CI 在三平台构建与测试」之阶段 4 的代码 | 5 | P14 |
| 148 | NFR-11 | 「依赖许可证白名单（MIT / Apache-2.0 / BSD / ISC / MPL-2.0 等）」之阶段 4 用到的外部包 | 5 | 预期 `没有做完（已登记）`（M6-D24；P22） |

各任务的行数：任务 1 二十六行，任务 2 三十三行，任务 3 五十二行（3a 二十行、3b 三十二行），任务 4 二十六行，任务 5 十一行；共 148 行，其中指向别的行的十六行（#18、#20、#25、#26、#28、#29、#72、#73、#100、#108 ～ #111、#134、#139、#142）。被指的行都与指它的行在同一个任务里。

**判据**：

- 骨架照上面的样子：148 行，各任务的行数如上；
- `python -I tools/check_rows.py HEAD` 不报 `MISS`：结论一格都是占位，别的格是空的或指向；
- `python -I tools/count_rows.py HEAD` 的"优先级认不出的"是 0；
- 四张清单在工作区；修正记录里有任务 0 的行。

---

## 任务 1 ～ 5 共同的做法

每个任务读骨架里结论写着 `待读（任务 N）` 的那些行。

**每一行**

1. **手册的原句**（设计第 4.7 节）：照兼容性清单那一行的「Surge 行为」→ `docs/reports/2026-10-29-phase4-manual-recheck.md` → `docs/reports/2026-10-28-manual-6.11-recheck.md` → 重对报告 → 工作区 `manual-1103/` 里那一页的文本的次序找。都没有的，写"没读到"，接着读别的行；报告里列出这些行。不凭记忆，不联网。
2. **代码**：真正作用于这件事的地方，写 `路径::函数`。只有解析器接受它而没有东西用它的，不算"做了"。
3. **用例**：读它的断言。写"`路径`：`名字`（断言了什么）"。
   - 用例只在某个平台上编译的，写明平台。
   - CI 的证据照 P14 写（M6-D28）：先 `git grep -n "fn <名字>" 20efc16 -- <路径>`，再看那条用例在不在 `#[cfg(unix)]` 一类的门后面、带不带 `#[ignore]`。
4. **结论**：设计第 4.4 节的八种之一，别的写法都不用。`不属于阶段 4` 与 `需求文字有问题` 的后面括号里用一句话说为什么。
   - 两者的分界照设计第 4.4 节与 P19。
   - 第八种 `做了，本机没有能红的用例` 有两种情形：只在 Linux / macOS 上编译的；本机编译、但缺省的用例照规矩从不走到的（读系统信任库的那一层、只有真浏览器与真站点才走到的路径）。代码一格写明是哪一种（它的 `cfg`，或"读系统的信任库"一类）。
   - L1 的用例把逻辑一直钉到系统的边界（打印的装 CA 的命令逐字、写出的文件与权限位、发给客户端的字节），改坏那段逻辑会有用例红的，记 `做了`；系统真的照做没有，是"真机上"一格的事（设计第 4.4 节）。
5. **真机上**：设计第 4.3 节。
   - 写手工清单的项，一律写 `节/编号`（`M2/W1`、`M4/P13`），按平台写；
   - L1 已经证明了全部的写 `—`；
   - 要真机、但真机上也没有一项手工操作看得出的，写 `看不出（<为什么>）`；
   - 要真机而清单里没有的写 `缺`（任务 6b 补）。
   - 阶段 4 没有 L2：这一格不写用例名（`tools/check_rows.py` 把它当 `MISS`）。
6. **指向别的行的行**（P4）：结论与「真机上」照被指的那一行；「代码」一格写"同「<表>」的「<那一句>」一行"；「用例」一格写"同上"。

**`做了` 的每一行**：读断言读不出它会因这一句不成立而红的，做一次改坏验证（全局约束）。`做了，补了用例` 的每一行都要有改坏验证的日志。

**当场补**（设计第 4.5 节）：

- 一处代码、一条用例补得上的，当场补：先写用例；在没补的代码上跑，看到红；再补到绿。一处一个提交。
- 那一行记 `补上了`（代码原来就对、只缺用例的记 `做了，补了用例`），用例一格后面括号里写提交的短哈希。
- 要先有设计、要加依赖、要动只在 Linux / macOS 上编译的代码、要改登记、要动 `crates/rurge-http/src/engine.rs` 里 hyper 的设置、属于 M3c 的：不补，记 `没有做完（已登记）`。
- `rurge-platform` 与 `rurge-tun` 阶段 4 没有改过；读到它们时不改。

**重对报告交来的条目**（设计第 3.5 节）：每一条照"拿真实的输入或一条用例去试 rurge"读——一样的，记进对应那一句的用例格；不一样的，照上面「当场补」或「登记」处理。报告里一条一段：读了什么、结论、记在哪一句。

**登记**（P18）：

- `没有做完（已登记）`、`需求文字有问题`、`有意不同（已登记）`、`做了，本机没有能红的用例` 的每一行，同时在第一部分末尾对应的那张表里加一行。
- 去向写"项目所有者"，或写已有的登记（某份计划「延后事项」第几行、兼容性清单哪一节、某份设计的哪一条已决事项）。
- 读之前先查 `docs/status.md` 对应里程碑一节的「去向写着项目所有者的」与那份计划的「延后事项」：已经登记过的，引它，不另起。

**拿真实的输入读**（全局约束）：照手册的例子手写几份 Profile 片段，各任务列出要写的；看有没有被悄悄跳过、或读成别的意思的行。读出问题的，照上面「当场补」或「登记」处理。

**提交与报告**

- 提交至少一个 `docs:`，内容是这个任务的行与修正记录的行；补的代码与用例各自提交。
- 补了用例的任务跑全量门禁；只改了文档的照全局约束写一行。
- 交回之前跑 `python -I tools/check_rows.py HEAD --task N`：本任务的行里没有 `MISS`，没有 `待读（任务 N）` 剩下。
- 报告写清：读了几行；八种结论各几行；做了改坏验证的是哪几行（日志的文件名）；补了什么（提交）；登记了什么；重对报告的条目各自的去处；真实输入的片段与读出的东西。

**一个任务的行超过 30 行时**，协调者可以分两次派（P20），各一份报告与评审；任务号不变。

---

## Task 1：M1 那一片——会话入口与明文 HTTP 引擎

**读的行**（26 行）：

- FR-HTTP-01 的 #1 ～ #8；
- FR-CFG-06 的 #109 ～ #113；
- FR-OBS-02 的 #115、#116；
- 总设计第 1.2 节的 #125、#126；第 1.5 节的 #127、#128；
- 推过来的 #133、#135 ～ #138、#140、#141。

**先读**：

- M1 细化设计 `docs/superpowers/specs/2026-10-29-phase4-m1-session-entry-http-engine-design.md` 与它第 20 节的订正；
- M1a 计划 `docs/superpowers/plans/2026-10-29-phase4-m1a-sniffing-plan.md` 与 M1b 计划 `docs/superpowers/plans/2026-10-29-phase4-m1b-http-engine-plan.md` 的两张表；
- `docs/status.md` 阶段 4 的「M1a（嗅探与规则之前的 SNI）」「M1b（明文 HTTP 引擎）」两节；
- `docs/api/phase4.md` 的「M1a」「M1b」两节；
- `docs/testing.md` 的「阶段 4 / M1a 起：嗅探」「阶段 4 / M1b 起：明文 HTTP 引擎」两节；
- 手工清单的「M1　会话入口与明文 HTTP 引擎」一节（`M1/W1`、`M1/W2`、`M1/M1`、`M1/M2`、`M1/L1`、`M1/L2`、`M1/P1` ～ `M1/P3`）；
- 重对报告第 3 节第 3、5、9 条。

**代码从这里找起**：

- 嗅探与前缀流：`crates/rurge-http/src/sniff.rs`、`crates/rurge-http/src/prefix.rs`、`crates/rurge-http/src/tls.rs`（读 ClientHello）；先答后读的会话：`crates/rurge-engine/src/serve.rs`；
- 明文 HTTP 引擎：`crates/rurge-http/src/engine.rs`、`crates/rurge-http/src/pool.rs`、`crates/rurge-http/src/headers.rs`；引擎一侧的 `crates/rurge-engine/src/http.rs`；
- 三种入口：`crates/rurge-inbound/src/http.rs`、`crates/rurge-inbound/src/socks5.rs`；已经答了"连上"的会话上的 `REJECT-DROP`：`crates/rurge-inbound/src/hold.rs`；错误页：`crates/rurge-inbound/src/responses.rs`；fake 地址：`crates/rurge-engine/src/fake_address.rs`；
- 规则：`crates/rurge-rules/src/matcher.rs`（`USER-AGENT`、`URL-REGEX`、`extended-matching`）、`crates/rurge-rules/src/set.rs`（规则集一行上的标志作用于整集）；
- 用例：`crates/rurge-engine/tests/sniff.rs`、`http.rs`、`pipeline.rs`；`crates/rurge-http/src/` 各文件的测试模块。

**重对报告的三条**：

- 第 3 条：上游在 304（或别的不该带正文的应答）之后多送正文——下一个借这条连接的请求不失败（Surge 修的是这时请求以 503 失败）。
- 第 5 条：HTTP 监听器收到的请求去掉 `Proxy-Authorization`（重对报告读的是 `crates/rurge-inbound/src/http.rs` 的 `handle`）；嗅探到的连接上引擎照转，是总设计 P4-D5 定的。核实两边各有断言它的用例，没有的照「当场补」。
- 第 9 条：chunked 正文之后紧跟着管线化的第二个请求——两个请求各自得到应答、各有一条记录。

**拿真实的输入读**：

- `force-http-engine-hosts`、`always-raw-tcp-hosts`、`always-raw-tcp-keywords`（带端口与不带端口的项、通配、`-` 开头的排除）；`show-error-page`、`show-error-page-for-reject`；
- `USER-AGENT` 与 `URL-REGEX` 规则（`rule match --url`、`--user-agent`）；
- 规则行上与规则集文件里一行上的 `extended-matching`（规则集文件写在工作区，`RULE-SET` 指本地文件），`rule match --sni`、`--http-host`；
- `PROTOCOL,HTTP` 与 `PROTOCOL,HTTPS` 规则，`rule match --protocol`。

**判据**：

- 26 行都有八种结论之一；
- 共同做法的每一条都做了，重对报告的三条各有去处；
- 补了用例的，全量门禁的通过数是 5453 加新用例的个数。

---

## Task 2：M2 那一片——MITM、HTTP/2、CA 工具与 `http-api-tls`

**读的行**（33 行）：

- FR-HTTP-02 的 #9 ～ #11；
- FR-HTTP-03 的 #12 ～ #19、#21 ～ #23、#25 ～ #32；
- FR-HTTP-14 的 #35；
- FR-HTTP-13 的 #105；FR-CFG-06 的 #108；
- FR-OBS-02 的 #117 ～ #119；
- 推过来的 #131、#139、#142；
- NFR-03 的 #144、#145。

可以分两次派（P20）：2a 是 #12 ～ #17、#19、#21 ～ #23、#25 ～ #32、#131、#142、#144、#145（22 行：解密的决定、CA 与叶证书、CA 工具）；2b 是其余（11 行：HTTP/2、`h2` 键、`http-api-tls`、`mitm` 开关、M2 的三节 API）。

**先读**：

- M2 细化设计 `docs/superpowers/specs/2026-10-30-phase4-m2-mitm-design.md`：先读第 23 节的订正；第 22 节是留给项目所有者的；
- M2a、M2b、M2c 三份计划（`docs/superpowers/plans/2026-10-30-phase4-m2a-mitm-http1-plan.md`、`2026-10-30-phase4-m2b-mitm-http2-plan.md`、`2026-10-31-phase4-m2c-ca-tools-api-tls-plan.md`）的两张表；
- `docs/status.md` 的 M2a、M2b、M2c 三节；
- `docs/api/phase4.md` 的「M2a」「M2b」「M2c」三节；
- `docs/testing.md` 的「阶段 4 / M2a 起：MITM」「阶段 4 / M2b 起：MITM 的 HTTP/2」「阶段 4 / M2c 起：CA 工具、`http-api-tls` 与命令行的 HTTPS」三节；
- 手工清单的「M2　MITM」一节（`M2/W1` ～ `M2/W5`、`M2/M1` ～ `M2/M5`、`M2/L1` ～ `M2/L5`、`M2/P1` ～ `M2/P6`）；
- 重对报告第 3 节第 1 条（登记）与第 4、6、7 条。第 12 条（上游证书链靠操作系统的信任评估）并进总设计 Q4，不读（设计第 3.5 节）。

**代码从这里找起**：

- 配置：`crates/rurge-config/src/mitm.rs`（九个键；`W0039`、`W0040`）；
- 解密：`crates/rurge-http/src/mitm/`——`authority.rs`（CA、叶证书的签发与缓存）、`decrypt.rs`（解不解、与客户端握手、证书固定的提示）、`generate.rs`（新 CA 的四个文件）、`api.rs`（API 的 TLS）；两侧的 HTTP 版本：`crates/rurge-http/src/version.rs`、`crates/rurge-http/src/exec.rs`；
- 引擎：`crates/rurge-engine/src/mitm.rs`、`crates/rurge-engine/src/runtime.rs` 的 `build_mitm`；开关与 `state.json`：`crates/rurge-engine/src/state.rs`；
- API：`crates/rurge-api/src/routes/mitm.rs`、`routes/features.rs`、`crates/rurge-api/src/tls.rs`；
- 可执行文件：`crates/rurge/src/cli/mitm.rs`（`generate`、`export`、`restrict`）、`crates/rurge/src/cli/api_client.rs`；
- 用例：`crates/rurge-engine/tests/mitm.rs`、`crates/rurge/tests/run/mitm.rs`、`crates/rurge/tests/cli.rs`、`crates/rurge-api/tests/api.rs`，与上面各文件的测试模块。

**这一片要特别留意的**（评审要点第 2 条）：

- `restrict` 与 `mitm_ca_generate_keeps_the_secrets_private` 是 `#[cfg(unix)]`：#144 的用例格照 P14 写"只在 Linux 与 macOS 上编译"与 CI 的证据。#145（Windows 上不设权限、打印一句提醒，M2-D20）读那一句提醒的用例。
- 解密的上游信"系统的根 ∪ `webpki-roots`"（总设计 P4-D10）：读系统的根那一层缺省的用例走不到，属于第八种结论的第二种情形，写在用到它的那一句上。
- `crates/rurge-config/src/mitm.rs` 第 40 行 `h2` 的注释"read only until M2b"已经过时：改成照现在的代码说它做什么，单独一个 `docs(config):` 提交（评审要点第 3 条）。

**重对报告的四条**：

- 第 1 条（登记）：在兼容性清单 7.2 节加「叶证书的密钥」一行（P21），`lists/matrix-rows.md` 表末补第 68 行；#21 的结论照常下。
- 第 4 条：解密的 HTTP/2 客户端读得很慢时内存不涨——读 HTTP/2 一侧的流量控制与缓冲的上限。
- 第 6 条：一个格式不对的 HTTP/2 请求只重置它自己的流，同一条连接上别的流照常。
- 第 7 条：上游的 HTTP/2 连接在应答中途关闭——给客户端的那个流被截断（重置或不完整），不当成完整的应答。

**拿真实的输入读**：

- `[MITM]` 九个键的手册例子（`hostname` 的通配、`-` 开头的排除、带端口与不带端口的项、`hostname-disabled`、`client-source-address` 的 CIDR 与 MAC 写法、`h2`、`skip-server-cert-verify`、`auto-quic-block`），`ca-p12` 写 `generate` 生成在工作区的那个文件；
- `rurge mitm ca generate --out <工作区里的空目录>`：生成的四个文件、打印的各平台装 CA 的命令（不执行）；
- `rurge mitm ca export`：PEM 与 `--der` 两种；
- `http-api-tls = true` 而 `[MITM]` 没有可用的 CA 时 `rurge check --json` 给的 `W0040`。

**判据**：同任务 1；另有 `docs(config):` 的那一个提交。

---

## Task 3：M3 那一片——重写与 Map Local（分两次派：3a、3b）

**读的行**（52 行）：

- 3a（20 行）：FR-HTTP-05 的 #36 ～ #44；FR-HTTP-06 的 #45 ～ #52；FR-HTTP-13 的 #106；FR-CFG-18 的 #114；FR-OBS-02 的 #120。
- 3b（32 行）：FR-HTTP-07 的 #53 ～ #65；FR-HTTP-08 的 #66 ～ #78；FR-HTTP-09 的 #79 ～ #82；FR-OBS-02 的 #121；推过来的 #132；另填第二部分「手册的重写示例与黄金用例」那张表（P10）。

**先读**：

- M3 细化设计 `docs/superpowers/specs/2026-10-31-phase4-m3-rewrite-map-local-design.md`：先读第 22 节的订正；第 21 节是留给项目所有者的；
- M3a 计划 `docs/superpowers/plans/2026-10-31-phase4-m3a-url-header-rewrite-plan.md`（3a）与 M3b 计划 `docs/superpowers/plans/2026-10-31-phase4-m3b-body-rewrite-map-local-plan.md`（3b）的两张表；
- `docs/status.md` 的 M3a、M3b 两节；
- `docs/api/phase4.md` 的「M3a」「M3b」两节；
- `docs/testing.md` 的「阶段 4 / M3a 起：URL Rewrite 与 Header Rewrite」「阶段 4 / M3b 起：Body Rewrite 与 Map Local」两节；
- 手工清单的「M3　重写与 Map Local」一节（`M3/W6` ～ `M3/W8`、`M3/M6` ～ `M3/M8`、`M3/L6` ～ `M3/L8`、`M3/P7` ～ `M3/P10`）；
- 3b 另读重对报告第 3 节第 8 条。

**代码从这里找起**：

- 配置：`crates/rurge-config/src/rewrite.rs`（四个节的读法，`W0041`、`W0042`）；
- 重写：`crates/rurge-http/src/rewrite/`——`url.rs`、`header.rs`（3a），`body.rs`、`map_local.rs`（3b），`mod.rs`（次序与开关）；引擎里调它们的地方在 `crates/rurge-http/src/engine.rs`；
- Map Local 的 URL 数据：外部资源的 `data` 一类（`crates/rurge-net/src/resource/`）；
- 用例：`crates/rurge-engine/tests/rewrite.rs`（名字以 `the_manual_` 开头的 16 条是手册的例子），与上面各文件的测试模块。

**3b 的黄金用例表**（设计第 6 节第 1 条；P10）：照 `lists/golden.md` 的 25 行，逐行找对应的用例（多半是 `the_manual_` 开头的那 16 条），读它的断言，填进验收文档第二部分那张表与 `lists/golden.md`。一个用例对应几个示例的，每个示例一行都写它；没有用例的示例照实写（jq 的两块随 M3c，今天那一行给 `W0042` 并跳过）。

**重对报告的一条**（3b）：第 8 条——HTTP/2 上不带正文的应答（`HEAD`、204、304）同时命中正文改写与脚本时，Surge 修的是它出错；rurge 没有脚本，读的是不带正文的应答命中 `[Body Rewrite]` 时不出错、不凭空加正文与 `Content-Length`。

**拿真实的输入读**：

- 3a：`[URL Rewrite]` 的四种模式（含省略类型、`$1`、`{{{GATEWAY_ADDRESS}}}`、结果不是 `http` / `https` / `ws` 的）；`[Header Rewrite]` 的四种动作与两个方向，连省略方向的旧写法、改 `Content-Length` 与 `Transfer-Encoding` 的行。
- 3b：`[Body Rewrite]` 的多对替换、`^$`、带 `$1` 的、jq 的两种写法（应当给 `W0042` 并跳过）；`[Map Local]` 的四种 `data-type`、`data=` 写工作区的文件与相对路径、`.invalid` 下的 URL、`header` 的两种形式、`status-code` 的边界（199、200、999、1000）。

**判据**：同任务 1；3b 另有黄金用例表的 25 行都有了"用例"与"断言了什么"（没有的写"没有"与理由）。

---

## Task 4：M4 那一片——请求记录与抓包

**读的行**（26 行）：

- FR-HTTP-10 的 #83 ～ #94；
- FR-HTTP-11 的 #95 ～ #102；
- FR-RULE-14 的 #103、#104；
- FR-HTTP-13 的 #107；
- FR-OBS-02 的 #122、#123；
- 推过来的 #134。

**先读**：

- M4 细化设计 `docs/superpowers/specs/2026-11-01-phase4-m4-capture-records-design.md`：先读第 18 节的订正；第 17 节是留给项目所有者的；
- M4a 计划 `docs/superpowers/plans/2026-11-01-phase4-m4a-capture-plan.md` 与 M4b 计划 `docs/superpowers/plans/2026-11-02-phase4-m4b-records-plan.md` 的两张表；
- `docs/status.md` 的 M4a、M4b 两节，「M1 ～ M4 推送之后的 CI（2026-11-02）」与「第 17 次 CI 与 M4b 任务 11（2026-11-02）」两节；
- `docs/api/phase4.md` 的「M4a」「M4b」两节；
- `docs/testing.md` 的「阶段 4 / M4a 起：抓包」一节；
- 手工清单的「M4　抓包与请求记录的完整」一节（`M4/W9` ～ `M4/W12`、`M4/M9` ～ `M4/M12`、`M4/L9` ～ `M4/L12`、`M4/P11` ～ `M4/P17`）；
- 重对报告第 2.1、2.2 节（抓包过滤器）。

**代码从这里找起**：

- 抓包：`crates/rurge-http/src/capture/`（`store.rs`、`slot.rs`、`mod.rs`）；HAR：`crates/rurge-api/src/har.rs`；端点：`crates/rurge-api/src/routes/capture.rs`；两个上限的命令行参数在可执行文件里；
- 请求记录：`crates/rurge-engine/src/observe.rs`；直通的原因在 `crates/rurge-http/src/mitm/decrypt.rs` 的 `decides` 与 `crates/rurge-engine/src/engine.rs` 的 `dial_noted`（建句柄时写备注）；记录的端点 `crates/rurge-api/src/routes/requests.rs`；
- 规则的 `always-capture`：`crates/rurge-rules/` 里 `RuleParams` 的那个字段与引擎里用它的地方；
- 用例：`crates/rurge-engine/tests/capture.rs`、`http.rs`、`mitm.rs`、`udp.rs`；`crates/rurge-api/tests/api.rs`；`crates/rurge/tests/cli.rs`；两条带 `#[ignore]` 的量内存的用例（`crates/rurge-http/src/capture/memory.rs`、`crates/rurge-engine/src/observe/memory.rs`）照 P14 写"CI 编译、不运行"。

**登记**（设计第 3.5 节，P21）：在兼容性清单 10.4 节加抓包过滤器一行，`lists/matrix-rows.md` 表末补它（标"不在 68 行里"）；验收文档「留给项目所有者的」由任务 7 汇总，本任务在报告里写明加了哪一行。#101 的结论照常下：需求文档没有抓包过滤器这一句，它不进「没有做完的」。

**拿真实的输入读**：

- 带 `always-capture=<会话名>` 的规则（规则行上、`RULE-SET` 行上），`rule match --explain` 看它被读成什么；
- `--capture-size`、`--capture-body-size` 两个参数的边界：用 `rurge run --help` 看帮助文字（不运行 `rurge run` 本身），值的解析读代码与用例。

**判据**：同任务 1。

---

## Task 5：M5 那一片——QUIC 里的 SNI、`auto-quic-block` 与三条非功能需求

**读的行**（11 行）：

- FR-HTTP-03 的 #20、#24；
- FR-HTTP-04 的 #33、#34；
- FR-OBS-02 的 #124；
- 总设计第 1.5 节的 #129、#130；
- NFR-01 的 #143；NFR-07 的 #146、#147；NFR-11 的 #148。

**先读**：

- M5 细化设计 `docs/superpowers/specs/2026-11-02-phase4-m5-auto-quic-block-bench-design.md`：先读第 16 节的订正；第 15 节是留给项目所有者的；
- M5a 计划 `docs/superpowers/plans/2026-11-02-phase4-m5a-quic-hello-auto-quic-block-plan.md` 与 M5b 计划 `docs/superpowers/plans/2026-11-03-phase4-m5b-bench-legacy-plan.md` 的两张表（M5b 记录第 22、27 行是基准的读数）；
- `docs/status.md` 的 M5a、M5b 两节；
- `docs/api/phase4.md` 的「M5a」一节；
- `docs/testing.md` 的「阶段 4 / M5a 起：QUIC 的 ClientHello 与 `auto-quic-block`」「阶段 4 / M5b 起：基准与遗留」两节；
- 手工清单的「M5　QUIC 里的 SNI 与 `auto-quic-block`」一节（`M5/W13`、`M5/W14`、`M5/M13`、`M5/M14`、`M5/L13`、`M5/L14`）；
- 重对报告第 3 节第 2 条与第 2.3 节 QUIC 那一句。

**代码从这里找起**：

- `crates/rurge-net/src/quic/initial.rs`（Initial 包的解密、CRYPTO 帧按偏移拼 ClientHello、两个上限）；
- `crates/rurge-engine/src/udp.rs`（持住一条流的第一批数据报、读出的 SNI 写进会话、`auto-quic-block` 的阻断）；`crates/rurge-engine/src/serve.rs::auto_quic_blocks`；
- `crates/rurge-engine/src/quic_adaptive.rs`（`block-quic` 的 `adaptive` 与 `auto`）；
- 用例：`crates/rurge-engine/tests/udp.rs`、`mitm.rs`、`tun.rs`；`crates/rurge-net/src/quic/initial.rs` 的测试模块；基准 `crates/rurge-engine/tests/bench.rs`。

**QUIC 在 UDP ASSOCIATE 上**（设计第 3.5 节第 2.3 节那一句）：手册说 Surge 只对增强模式的流量识别与阻断 QUIC，不对 SOCKS5 的 UDP ASSOCIATE。用一条现有的用例核实 rurge 对 UDP ASSOCIATE 来的流（`block-quic` 与 `auto-quic-block` 各一）做什么；没有现成的用例就照「当场补」写一条只钉住现在的行为的（不改行为）。与手册不同的，在兼容性清单 `block-quic` 与 `auto-quic-block` 两行的备注里登记，不当场改（改行为要先有设计，设计第 4.5 节）。

**重对报告的第 2 条**：跨几个包的 ClientHello 读进 #129；同一帖子说 QUIC v2 与草案 29 也认了——重对报告读的是 `crates/rurge-net/src/quic/initial.rs` 的 `quic_version` 认 v1、v2 与草案 29 ～ 32，核实有断言它的用例，写进 #33 的用例格。

**NFR-01**（#143；P8）：记 `没有做完（已登记）`。代码一格写 B1、B5 的读数与出处（M5b 记录第 27 行），B1 那一句带着 M5b 延后第 1 行；"校准——经引擎的路径用哪条线——没有做，去向项目所有者（M5 设计第 15 节第 6 条）"。不重跑基准。

**NFR-07**（#146、#147）：#146 照 P7 的名单逐处读过、照 P7 的规则下结论；#147 照 P14，引 `.github/workflows/ci.yml` 的三个平台与两次运行。

**NFR-11**（#148；P22）：用 `cargo metadata --offline --format-version 1`（只读本机已有的元数据，不下载；要下载就停下报告）列出 P22 的 24 个包的版本与 `license` 字段，写进代码一格；记 `没有做完（已登记）`：白名单没有定（`webpki-roots` 的 CDLA-Permissive-2.0 不在需求的字面里），去向项目所有者（总设计 Q4）。

**过时的注释**：`crates/rurge-config/src/mitm.rs` 第 44 行 `auto_quic_block` 的"read only until M5"改成照现在的代码说它做什么，单独一个 `docs(config):` 提交（评审要点第 3 条）。

**拿真实的输入读**：

- `[MITM]` 的 `auto-quic-block` 与全局、策略上的 `block-quic` 各种值放在一起；
- `PROTOCOL,QUIC` 规则与带 SNI 的 QUIC：`rule match --protocol QUIC --udp --sni <名字>`，看域名规则能不能按 SNI 命中。

**判据**：同任务 1；另有 `docs(config):` 的那一个提交。

---

## Task 6：两份清单（分两次派：6a 兼容性清单，6b 手工清单）

### 6a：兼容性清单的 68 行（设计第 8 节，M6-D10）

**做什么**：照工作区 `lists/matrix-rows.md` 逐行读——今天的 67 行、任务 2 补的第 68 行（7.2 节「叶证书的密钥」），与任务 4 补的 10.4 节抓包过滤器一行（阶段栏"待定"，不在 68 行里）——对着验收文档里对应的句子与代码。

1. **状态标记与备注**：与代码一致；验收文档里同一件事的那一行，两边说同一件事。
   - 清单与代码对不上时（M6-D17）：清单的说法有设计的已决事项、计划的修正记录或一条用例撑着的，按代码的缺口处理——小的照「任务 1 ～ 5 共同的做法」当场补（补了代码就跑全量门禁），大的把状态标记降到与代码一致、在验收文档的「没有做完的」登记；没有东西撑着、只是过时或说多了的，改清单。两种都写一行修正记录。
   - 代码缺口碰到任务 1 ～ 5 已经下了结论的行的，那一行跟着改，修正记录写明是任务 6a 改的。
2. **过时的话**（P15）：P15 列的行逐行看。这份清单阶段 4 的行是分段写的——后写的段落以"**阶段 4 / M… 起**"开头，订正前面的话。同一格里已有后写的段落订正了它的，不动；没有的，在那一格末尾加一段"**阶段 4 / M6 核对**：…"照现在的代码写，不删前面的话。`docs/api/phase4.md` 各里程碑的历史段落不改。
3. **阶段栏含 4 而阶段 4 没有做的那部分**：
   - 某份设计已经把它留给 M3c 或推到后面阶段的（jq、`br`、`zstd` 等总设计 Q4；`client-source-address` 的 MAC 地址写法在阶段 7），照那份设计写，备注里写出处；
   - 没有设计推过它的，不改阶段栏：在验收文档的「没有做完的」里加一行，去向项目所有者（M6-D10）。
4. **第 620 行**（改写前按需要先解压）：✅ 不动；备注末尾补一句"`br`、`zstd` 随阶段 4 / M3c（等总设计 Q4）"，与验收文档 #63 说同一件事。
5. **任务 2、4、5 加的行与备注**（「叶证书的密钥」、抓包过滤器、`block-quic` 与 `auto-quic-block` 在 UDP ASSOCIATE 上的不同）：核对与代码、与 P21 一致。
6. **附录的统计**：任务 2、4 各加了一行，附录一定要重数——`python -I tools/count_matrix.py . HEAD` 整张重数，写进附录的那一段（数法是阶段 3 / M5 计划记录第 66、75 行的规则）；文件开头"最近一次"那一句照惯例改。

**判据**：

- 68 行与 10.4 节那一行都读过：`lists/matrix-rows.md` 每一行写了"不改"或改了什么；
- P15 的行都有了交代（不动的写哪一段已经订正了它）；
- 附录的数与脚本的输出相同；
- 补了代码的，全量门禁的通过数是当时的基线加新用例。

### 6b：手工清单（设计第 7 节，M6-D9）

**做什么**：

1. **能照着做**：每一项里反引号括起来的命令、选项、键、端点、日志原文、计数的名字，用 `python -I tools/manual_names.py HEAD` 逐个在代码里找；剩下的人读。写在阶段 4 / M5a 之前的项（M1 ～ M4 四节），照 M5 之后的代码看还对不对（例如浏览器开着 QUIC 时，M5a 之后 QUIC 流被读出 SNI、命中 `[MITM]` 的名单时被 `auto-quic-block` 阻断）。
2. **就地订正**：
   - 清单里的项都还没有执行（结果、日期、系统版本三栏是空的），过时的项就地改；已经填了结果的项（现在没有）不改。
   - 那一节开头加一行"阶段 4 / M6 订正了第几项"；改了什么记进修正记录。
3. **覆盖**：
   - 验收文档里「真机上」一格写 `缺` 的每一行，每个相关的平台至少一项；总设计第 15 节第 2 条（主流站点解密、证书固定站点给出提示）每个平台至少一项。
   - 没有的，在文件末尾新加「M6」一节补上（标题以 `## M6` 开头，`tools/gen_manual_items.py` 与 `tools/check_rows.py` 靠它认节）。编号接着往下排：W15、M15、L15、P18 起，新编号在整份清单里不重复。
   - 补完把验收文档里的 `缺` 换成 `M6/<编号>`。
4. **重号的说明**：M1 与 M2 两节已有的重号不改；清单开头「怎么用」里加一句：引用一律写 `节/编号`（`M2/W1`），光写编号分不出是哪一节。
5. **安全**（评审要点第 4 条）：每一项改系统的（把 CA 装进系统或浏览器的信任库、开系统代理）都写着前提（一次性的虚拟机或能还原的机器）与怎样恢复（从信任库删掉那张 CA、关掉系统代理）；没有的补上。开头的「改系统设置的项与恢复」一节随新项更新。
6. **索引表**：开头「第 7 节的五条验收标准各由哪几项证明」那张表随新项更新。表头照旧——它说的是需求文档第 7 节的五条；总设计第 15 节另两条（逐句对照、CI）没有手工项。

**判据**：

- `tools/manual_names.py` 的输出里每个查不到的名字都有交代（改了，或说明它不是代码里的名字）；
- 验收文档里没有 `缺`（`tools/count_rows.py` 的"真机上写着 缺"是 0）；
- 新编号在整份清单里不重复；`tools/gen_manual_items.py` 重跑之后 `lists/manual-items.md` 的项数等于 68 加新项；
- 修正记录写了订正的项与新加的项数。

---

## Task 7：验收文档的其余与文档收尾

**做什么**：

1. **验收文档**（设计第 5、6、11 节）：
   - 「怎么读」：读的是什么，谁读的（一个助手；协调者每个任务抽查五行），读的代码是哪个提交（`bd981d2`，补的在其后、逐行写提交），六列与八种结论，没有覆盖什么（手工验收一项也没有做；阶段 4 没有 L2；`a262d4c` 之后的提交 CI 没有见过，任务 8 改写；M3c 没有做）。
   - 「数字」：`python -I tools/count_rows.py HEAD` 的输出——八种结论 × 各张表；三个数与第四个数，按 P0 / P1 分开，非功能需求另列（P6）。
   - 第一部分末尾的四张表：编号、排序，与表里的行一一对上。
   - 「结论」：三个数与第四个数并排；"称不称齐备"照设计第 11 节（表里有 P0 或 P1 的 `没有做完（已登记）` 就照实写缺哪几句，"齐备"一词不出现；非功能需求的 `没有做完` 写在同一句里）。总设计第 15 节七条各一句。
   - 第二部分：七条，每条照设计第 6 节写 L1、L3、CI 三种证据与"还缺什么"；"满足"照 M6-D27——第 1、3 条缺 jq，第 2 条缺手工验收，第 7 条缺一次三个平台全绿、编译了阶段 4 全部代码的运行，第 4、5 条写"还缺 CI 对 `a262d4c` 之后的代码"（任务 8 改），第 6 条看本任务的结果。L1 的用例名用脚本核对；全量门禁的数是本任务那一次运行的。**没有一条能因为 L1 全绿就写成"通过"。**
   - 「靠手工验收的」：按平台数「真机上」一格点到的项（`节/编号`），再列清单里有而没有被任何一行点到的项。一项也没有做过。
   - 「靠 CI 的」：只在 Linux / macOS 上编译的用例（`tools/platform_tests.py` 列出、与表里写着"只在 Linux 与 macOS 上编译"的对上）；带 `#[ignore]` 的（CI 编译、不运行）；`a262d4c` 之后的提交（M4b 任务 11、M5、M6）CI 没有见过（任务 8 改写）。
   - 本任务在任何文档里写"CI 没有见过"一类的话，报告里逐处列出文件与行（M6-D23）：任务 8 照这张单子改。
   - 「留给项目所有者的」：设计第 13 节十一条，加上对照查出的；第 13 节第 8 条那张表照 `docs/status.md` 阶段 4 各节「去向写着项目所有者的」汇总。
2. **文档收尾**（设计第 10 节）：
   - `README.md` 与 `README_en.md`：阶段 4 的状态、特性表、路线图（P15 列的行），与验收文档一致，两份一致。
   - `docs/requirements.md` 第 7 节阶段 4 的"状态"一行：接着写 M6 的一句，不改前面的句子；三个数与"称不称齐备"照验收文档写。
   - `CLAUDE.md`：只改「现在在哪里」。
   - `docs/status.md`：阶段 4 下、「附：依赖方向一条的原文」之前新的「M6（收尾与验收）」一节——做了什么，没做的与不要当成已经有的，以后的会话必须知道的事，去向写着项目所有者的；细节指到本计划的两张表。
   - `docs/README.md`：验收文档 `docs/acceptance/phase4-acceptance.md` 加一项（M6 的设计、本计划与重对报告三项开工前已经加了）；手工清单一项的描述随项数改。
   - 阶段 1 ～ 3 的验收文档：开头各加一句（P17）。别处不改。
   - 本计划的「延后事项」补上执行期新出现的。
3. **全量门禁**：跑一次，数写进第二部分与报告（一台机器上的一次运行，不是 CI）。

**判据**：

- `tools/check_rows.py HEAD` 全文没有 `MISS`、没有占位；
- `tools/count_rows.py` 的数与「结论」「数字」两节、README 两份、需求文档、`CLAUDE.md`、`docs/status.md` 引的数相同；
- README 两份的状态一致；
- 门禁绿。

**之后**（M6-D23）：协调者做完整个分支的终审，把 `phase4-m6` 快进合并进本地 `main`（在 `phase4-m6` 上 `git fetch . phase4-m6:main`），请项目所有者推送。M6 的工作到这里做完；任务 8 在推送之后做。

---

## Task 8：读 CI（M6-D12、D23、D29）

**等**：项目所有者推送 M4b 任务 11、M5 与 M6 的提交之后才开始；之前不做。

**协调者先做**：

- `git rev-parse origin/main` 看推送到了哪里（只读本地已有的引用；不 `fetch`）。
- 用公开的 API 读那次运行：
  - `https://api.github.com/repos/danielnie001/rurge/actions/runs?head_sha=<完整哈希>` 取运行号；
  - `/actions/runs/<运行号>/jobs` 取各任务与各步的结论；
  - `/check-runs/<任务号>/annotations` 取注解。
- 结果写进工作区的 `ci-<运行号>.md`。

**Pi 做的**：照 `ci-<运行号>.md` 的原文改任务 7 报告里列的每一处"CI 没有见过"，至少：

- 验收文档的「靠 CI 的」、第二部分第 7 条，与第 1、3、4、5 条的 CI 证据（照 M6-D27、D28：看那几条用例在三个平台的 `check` 全绿的运行里通过了没有）；用例格里写着"CI 没有见过"的行改成读到的结果；
- `docs/status.md`：M6 一节之后另起「M6 推送之后的 CI」一小节（阶段 3 的「M4 推送之后的 CI」与阶段 4 的「M1 ～ M4 推送之后的 CI」是先例）；
- `CLAUDE.md`「现在在哪里」里 CI 的那一句；两份 README 与需求文档第 7 节里写到 CI 的那几句。

只写读到的，不推断。

**有失败**：在 M6 里修——协调者另写一个任务插在这里（阶段 3 / M6 的任务 8a 是先例）。修完由所有者再推送，回到本任务。**至多两轮**（M6-D29）：第二次读回仍有失败的，M6 照实收尾——第二部分第 7 条写"未满足"，失败的用例、读到的原因与没查明的写进验收文档「靠 CI 的」与 `docs/status.md`「M6 推送之后的 CI」，交给下一个会话照 `CLAUDE.md`"推送之后先读注解、修掉它列出的失败"接着修。

**所有者在任务 1 ～ 7 进行中先推送的**（M6-D23）：协调者当时就照上面的办法读那次运行。有失败就插一个修的任务；修动到的代码上、已经下了结论的行重读，写修正记录。读到的结果先记进工作区，任务 7 写文档时用。

**判据**：文档里写的运行号、提交、结论与 `ci-<运行号>.md` 相同。

---

## 计划期决定

| # | 事项 | 决定 |
| - | ---- | ---- |
| P1 | 引用的名字与行号 | 对着 `7eed4b4` 核对过（代码树与 `bd981d2`、`b43278b` 相同）。行号会随前面任务的改动漂移，以名字与内容为准 |
| P2 | 需求行（设计第 14 节第 1 项；M6-D1） | `docs/requirements.md` 第 4 节阶段栏含 4 的 14 行：FR-RULE-14（第 358 行）、FR-HTTP-01 ～ 11（第 381 ～ 391 行）、FR-HTTP-13、14（第 393、394 行）；其中 FR-HTTP-02、04、14 与 FR-RULE-14 是 P1，其余 P0。第 7 节阶段 4 的范围（第 528 行）正是这 14 行。阶段栏写"各键随所属功能阶段生效""各阶段""逐阶段补齐"的 FR-CFG-06（第 275 行）、FR-CFG-18（第 287 行）、FR-OBS-02（第 438 行）都是 P0。另引的都是 P0：FR-CFG-11（第 280 行，阶段 2）、FR-CFG-12、14（第 281、283 行）、FR-OUT-01（第 312 行）、FR-RULE-01、02、04（第 345、346、348 行）、FR-RULE-07、08（第 351、352 行）。非功能需求：NFR-01（第 455 行）、NFR-03（第 457 行）、NFR-07（第 461 行）、NFR-11（第 465 行）。FR-HTTP-12（错误页，阶段栏 1）不在表里，它在经引擎的请求上的样子由 #112、#113 读到 |
| P3 | 推过来的句子的出处（设计第 14 节第 2 项；M6-D19） | grep `阶段 4\|：4\|4 完整`，在 `bd981d2` 上阶段 1 的 34 行、阶段 2 的 4 行、阶段 3 的 5 行（工作区 `lists/pushed-grep.txt`）。<br>**推了句子的**：阶段 1（`docs/acceptance/phase1-acceptance.md`）第 80、126、131、162、204、210、232、241、257、258、259、261、262、410、417、421 行；阶段 2（`docs/acceptance/phase2-acceptance.md`）第 527、564、574 行（第 574 行一段里与阶段 4 有关的只有"`[MITM]` 的 `ca-keystore-name` 不对照 `[Keystore]` 检查（FR-CFG-11，阶段 4）"一件，成 #142）；阶段 3（`docs/acceptance/phase3-acceptance.md`）第 247、250、251 行。<br>**只提到、没有推的**：阶段 1 第 268 行（FR-RULE-08 的 DoH3，阶段 2 的事）、第 387 行（`REJECT` 一行，匹配到的是"403 页"；它引的 `forward` 已退役，#113 的备注引它）、第 439 行（功能开关的 `POST` 那时回 501，三行功能开关的表头引它）、第 516、517、521、525 行（结论与说明里的数）、第 539 行（阶段 4 / M1a 加的说明），与「需求文字的问题」表里与上面同一句的第 546、549、557、561 ～ 567 行；阶段 2 第 497 行（阶段 1 的数）；阶段 3 第 305、455 行（数字与"留给项目所有者的"）。<br>逐行看阶段 1 与阶段 3 的「不属于」「需求文字有问题」（写计划时按"HTTP 引擎、MITM、抓包、重写、Rewrite、Map Local、嗅探、SNI、Host、`:authority`、capture、解密、证书"粗看过一遍）没有多出的；任务 0 照 M6-D19 再逐行看一遍 |
| P4 | 同一句在两处 | 同一句话出现在两条需求里，或同一条需求里出现两次：只读一次。被指的那一行写全，别的行写"同「<表>」的「<那一句>」一行"，结论与「真机上」相同。十六行：#18 → #9、#20 → #34、#25 → #19、#26 → #17、#28 → #14、#29 → #16、#72 → #67、#73 → #69、#100 → #103、#108 → #35、#109 → #6、#110 → #7、#111 → #8、#134 → #103、#139 → #11、#142 → #16。<br>**并进别的行、不另起一行的推过来的句子**：阶段 1 对照第 80 行 → #108，第 162 行 → #114，第 410 行 → #85、#86，第 417 行 → #87，第 421 行 → #91 ～ #93；阶段 2 验收第 527 行 → #87，第 564 行 → #137、#140；阶段 3 对照第 247 行 → #138，第 250 行 → #85，第 251 行 → #86。出处写进那一行的「需求」一格。<br>**总设计第 1.5 节**：R27 与 `REJECT-DROP` 自成一行（#127、#128）；M8c 计划延后第 105 行（FR-RULE-07 / 08 的拨号之前部分）并进 #137、#138、#140、#141——这四句读的都是拨号之前（嗅探之后、求规则之前）有没有；清单 L314 并进 #135；`remoteAddr` 与 `dnsMs` 并进 #87、#90；`Reach` 并进 #70；QUIC 里的 SNI 自成两行（#129、#130）。<br>**判了不是同一句的**：#11（FR-HTTP-02 的"`:authority` 提取"）与 #139（FR-RULE-07 的"从 HTTP/2 `:authority` 提取主机名"）是同一句，#11 写全、读两件（拼 URL；规则与 `extended-matching` 用它）。#33（为 `auto-quic-block` 读 QUIC Initial 的 SNI）与 #129（跨几个包的 ClientHello）、#130（规则按 QUIC 的 SNI 匹配）不是同一句：代码多半是同一处，三句各自下结论。#12（`hostname` 键的效果）与 #131（Host List 这种类型供 `[MITM]` 共用）不是同一句 |
| P5 | 占位与覆盖 | 骨架的结论一格写 `待读（任务 N）`，N 是数字（分两次派的 2a / 2b、3a / 3b 仍写 2、3）。任务 N 交回之后，`git grep -c "待读（任务 N）" -- docs/acceptance/phase4-acceptance.md` 是 0 |
| P6 | 数的数法（设计第 11 节，M6-D18） | 三个数与第四个数数的是行：指向别的行的行也算。优先级取「需求」一格里第一个需求编号的（`tools/count_rows.py` 这样做）：总设计第 1.2 节两行按 FR-RULE-01 算 P0；第 1.5 节四行——R27 按 FR-HTTP-01、`REJECT-DROP` 按 FR-OUT-01、QUIC 两行按 FR-RULE-07，都是 P0（设计第 11 节写"自成一行的两行"，写本计划时 QUIC 一行也定为自成两行，订正在设计第 15 节第 1 条）；推过来的行按它自己的需求（都是 P0）。P1 的行是 #9 ～ #11、#33 ～ #35、#103、#104 八行。非功能需求没有优先级，另列一栏；它的 `没有做完` 不挡"功能需求齐备"，但写在同一句里 |
| P7 | NFR-07 的名单（设计第 3.6 节、第 14 节第 4 项） | `git diff f87012f 7eed4b4` 在 `crates/*/src` 下加的平台 `cfg`（`cfg(…)`、`cfg!(…)`、`cfg_attr(…)` 里有 `unix`、`windows`、`target_os`），只在三个文件：`crates/rurge/src/cli/mitm.rs`（`restrict` 的 `#[cfg(unix)]` 与 `#[cfg(not(unix))]` 两个版本、提示里的 `cfg!(unix)`）——可执行文件的选择处；`crates/rurge-engine/src/observe/memory.rs` 与 `crates/rurge-http/src/capture/memory.rs`（`#[cfg(windows)]`、`#[cfg(target_os = "linux")]`、`#[cfg(not(any(windows, target_os = "linux")))]` 三支；两个文件都由 `#[cfg(test)] mod memory;` 拉进来）——只在用例里。规则照阶段 3：可执行文件的选择处与只在用例里的不算违背；阶段 3 登记过的旧的几处（它的 #139）不重读。任务 5 在读的那个提交上重跑一次这个 diff，结果写进 #146 的代码格 |
| P8 | 基准的数（设计第 3.6 节、第 14 节第 7 项；M6-D22） | M5b 计划「执行期修正记录」第 22 行（B4：签一张叶证书的中位数 248.6 µs，第一遍减第二遍的中位数，含在阻塞线程上签发与两次交接）与第 27 行（B1：经 rurge 的明文下行 0.79 Gbit/s，是"单核 ≥ 1 Gbps"那条线的 0.79 倍；B5：每条空闲连接经 rurge 比直连多的内存，是"1 万并发 < 300 MB"折成每条 30 KB 的 1.36、1.02、2.58 倍——明文、不解密的 TLS、解密的）。一台 Windows 11 x86-64 开发机上 release 的一次运行，不是 CI。B1 那一句带着 M5b 延后第 1 行（明文一路比 TLS 一路慢，原因没查明）。M6 不重跑基准 |
| P9 | 两份清单的数（设计第 14 节第 3 项） | 兼容性清单阶段栏含 4 的行在 `7eed4b4` 上 67 行：1 节 6、2 节 6、3 节 6、7 节 44、10 节 5（`tools/stage4_rows.py`：每张表取表头叫"阶段"的那一列，单独的数字 4 算；附录不算）；状态标记 ✅ 51、🟡 15、🔁 1。任务 2 加「叶证书的密钥」之后 68 行。手工清单 68 项：M1 9（`W1`、`W2`、`M1`、`M2`、`L1`、`L2`、`P1` ～ `P3`）、M2 21（`W1` ～ `W5`、`M1` ～ `M5`、`L1` ～ `L5`、`P1` ～ `P6`）、M3 13（`W6` ～ `W8`、`M6` ～ `M8`、`L6` ～ `L8`、`P7` ～ `P10`）、M4 19（`W9` ～ `W12`、`M9` ～ `M12`、`L9` ～ `L12`、`P11` ～ `P17`）、M5 6（`W13`、`W14`、`M13`、`M14`、`L13`、`L14`）；下一个编号 W15、M15、L15、P18 |
| P10 | 黄金用例的表（设计第 6 节第 1 条、第 14 节第 5 项） | `tools/manual_examples.py manual-1103` 从四页抽出 18 个代码块：`http_url-rewrite` 第 17、28、34、39、45 行（5 块），`http_header-rewrite` 第 17、25、37、58、79、99、105 行（7 块），`http_body-rewrite` 第 17、24、26、36、40 行（5 块；第 36、40 行是 jq），`http_map-local` 第 16 行（1 块）。另有 7 处写在正文里的：`http_url-rewrite` 第 49 行（按 `Host` 头与连接的主机名重建的 URL 也匹配）；`http_header-rewrite` 第 125 行（改 `Content-Length` 或 `Transfer-Encoding` 被拒绝、请求失败）；`http_body-rewrite` 第 30 行之 `$1`、第 30 行之 `^` 与 `$` 按行锚定、第 32 行之 `^$`、第 33 行之 UTF-8；`http_map-local` 第 39 行之 `data-type=text data=""`。共 25 行。`http_overview.txt` 没有重写的示例（2026-11-03 看过）。`crates/rurge-engine/tests/rewrite.rs` 里名字以 `the_manual_` 开头的用例 16 条。行号指工作区 `manual-1103/` 里的文本 |
| P11 | 真实输入的命令（设计第 4.8 节，M6-D20） | `rurge rule match` 缺省会读本机真实的网络（`--network-backend` 是隐藏的选项）、会解析目标、会下载外部资源，所以每次都带 `--network-backend file:<…>`、`--no-network`、`--data-dir <…>`、`--resolve` 或 `--no-dns`（全局约束）。`rurge check` 不读网络。`rurge mitm ca export` 经 `crates/rurge/src/cli/control.rs` 的 `load_profile` 读 Profile 与数据目录里已下载的来源，不联网；`rurge mitm ca generate --out <目录>` 要那个目录已经存在，写四个文件。`--protocol` 的值写大写（`QUIC`、`HTTPS`），见阶段 3 / M6 计划延后第 7 行 |
| P12 | 工作区的脚本 | 协调者在派任务 0 之前写好，放工作区的 `tools/`：<br>`stage4_rows.py`：数兼容性清单阶段栏含 4 的行（`--list` 逐行列出）；<br>`check_rows.py <提交> [--task N] [--notes] [--file <路径>]`：读那个提交上的验收文档，「需求」「结论」「代码」「用例」「真机上」各格照它开头的说明核对，查不到的打 `MISS`；<br>`count_rows.py <提交> [--file <路径>]`：数八种结论，按表与优先级，三个数与第四个数；<br>`count_matrix.py <仓库> <提交>`：附录的统计；<br>`manual_names.py <提交> [--all] [--file <路径>]`：手工清单里反引号括起来的名字逐个查；<br>`gen_manual_items.py <输出>`：手工清单的项列成 `节/编号` 一行一项；<br>`manual_examples.py <手册目录>`：四页重写的示例；<br>`platform_tests.py [<路径前缀> …]`：只在某些平台上编译的用例；<br>`new_word.py`、`paren_check.py`、`pipes.py`：查两个提交之间新出现的词、全角与半角括号是否配对、表格行里没转义的 `\|`。<br>脚本有错，Pi 停下来告诉协调者，不自己改 |
| P13 | 基线 | 门禁在 `b43278b`（阶段 4 / M5b 最后一次全量，2026-11-03，一台机器上的一次运行，不是 CI）：`cargo fmt --all --check` 退出 0；clippy 0 个警告；94 个结果行、5453 通过、0 失败、33 忽略。`git diff --stat b43278b 7eed4b4 -- crates tests Cargo.toml Cargo.lock .github` 为空，任务 0 不重跑 |
| P14 | CI 的证据（M6-D28） | 两次运行各取 `check` 全绿的平台拼起来：运行 41402787066（第 16 次，`20efc16`）Linux 与 Windows 的 `check` 全绿、macOS 失败；运行 41411445901（第 17 次，`a262d4c`）macOS 的 `check` 全绿、Windows 与 Linux 失败。`git diff --stat 20efc16 a262d4c -- crates tests Cargo.toml Cargo.lock .github` 只有 `crates/rurge-tun/src/tests/dispatch.rs`（写计划时核过）。所以用例格写：<br>`git grep -n "fn <名字>" 20efc16 -- <路径>` 有结果的，"CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901）"；<br>其中只在 Unix 上编译的（`tools/platform_tests.py` 列出），"只在 Linux 与 macOS 上编译；CI：Linux 运行 41402787066、macOS 运行 41411445901 通过"；<br>`20efc16` 上没有的，"CI 没有见过"；带 `#[ignore]` 的，"CI 编译、不运行"。<br>这说的是那条用例在 `a262d4c` 的代码上通过过；`a262d4c` 之后改过的代码（M4b 任务 11、M5、M6）CI 没有见过，任务 8 改写。非功能需求的「用例」一格可以引 CI 的运行、基准的记录行、扫描的结果或报告的节，不只是用例。#147 的证据是 `.github/workflows/ci.yml` 里三个平台的 `check` 与这两次运行 |
| P15 | 过时的话 | 兼容性清单阶段栏含 4 的行里，在 `7eed4b4` 上写着某个里程碑之前的样子的（`随 M[1-5]\|写这一行时\|还没有编译过\|今天`）：第 54、172、173、563、569、570、582、584、585、616 行，任务 6a 照它第 2 步处理。`crates/rurge-config/src/mitm.rs` 第 40、44 行两句注释（评审要点第 3 条），任务 2、5 各改一句。`README.md` 第 36、52、60、65、66 行与 `README_en.md` 第 36、51、59、64、65 行（阶段 4 的状态、特性表的 HTTP 处理一行、"现在"一句、路线图的阶段 3 与阶段 4 两项），任务 7 改。`docs/requirements.md` 第 7 节阶段 4 的"状态"一行由任务 7 接着写，前面的句子不改。`docs/api/phase4.md` 的历史段落不改 |
| P16 | FR-OBS-02 的切法 | 按 `docs/api/phase4.md` 的里程碑各节切，一节一行（#115 ～ #124）：M1a（第 7 行）、M1b（第 65 行）、M2a（第 118 行）、M2b（第 233 行）、M2c（第 291 行）、M3a（第 366 行）、M3b（第 484 行）、M4a（第 620 行）、M4b（第 940 行）、M5a（第 1066 行），归那个里程碑的任务。M5b 没有改 API；「M1 ～ M4 推送之后的 CI（2026-11-02）」（第 1058 行）不是里程碑。读法：那一节写的每个端点、字段、计数在代码里、有断言它的形状的用例；`/v1/features/*`、`/v1/mitm/ca`、`/v1/requests/*` 是 Surge 的端点，对着手册读；`/v1/rurge/…` 是 rurge 自己的（兼容性清单 10.4 节），对着 API 参考读 |
| P17 | 阶段 1 ～ 3 文档的那一句（M6-D15） | 阶段 1、2 的文档加在阶段 3 / M6 加的那一句之后，阶段 3 的文档加在标题之后：「<日期> 起，这份文档里推到阶段 4 的句子在 `docs/acceptance/phase4-acceptance.md` 重读过（「阶段 1 ～ 3 推过来的」一节；并进别的表的，出处写在那一行的「需求」一格里）；这里的结论是当时的，不改。」三份文档措辞相同；日期是任务 7 的日期。阶段 1、2 的文档在工作区里是 CRLF，照它 |
| P18 | 四张登记表（M6-D17） | 「没有做完的」「需求文字的问题」「有意不同的」「本机没有能红的用例的」四张表，任务 1 ～ 6 读到时各自加行。表头：<br>没有做完的：`\| # \| 需求（优先级） \| 这一句 \| 缺什么、使用者今天看到什么 \| 去向 \|`；<br>需求文字的问题：`\| # \| 需求（优先级） \| 这一句 \| 问题 \| 去向 \|`；<br>有意不同的：`\| 需求 \| 这一句 \| 登记在 \|`；<br>本机没有能红的用例的：`\| 需求 \| 这一句 \| 代码与为什么本机没有能红的用例（它的 cfg，或缺省的用例走不到的那一层） \| 要补的用例 \|`。<br>`#` 一栏在任务 1 ～ 6 里先空着，任务 7 编号、排序 |
| P19 | 预先判的边界（设计第 4.4 节） | 阶段 3 / M6 执行期的判法照用（它的修正记录第 9、10、12、16 行）。照它们，写计划时的预期：<br>`需求文字有问题`——#65（FR-HTTP-07"先于脚本"）、#80（FR-HTTP-09"→ 脚本"）、#82（"每请求 / 响应最多一个脚本"）、#102（FR-HTTP-11"通过…Dashboard 查看"）：阶段栏只写 4，句子自己没有说别的阶段，脚本登记在阶段 5（FR-SCR）、Dashboard 在阶段 6；<br>`不属于阶段 4`——#86（FR-HTTP-10 来源之设备，网关模式在阶段 7）、#92（备注之脚本，阶段 5）：需求行的阶段栏写"1 基础 / 4 完整"，列了几个阶段；<br>`没有做完（已登记）`——#57（jq）、#63（`br`、`zstd`）：总设计 P4-D13 与 M3 设计定给 M3c，等总设计 Q4；#143（NFR-01）、#148（NFR-11）：M6-D24；<br>推过来的句子阶段 4 做到了的记 `做了`，阶段栏的事留在阶段 1 那张表里，不在阶段 4 再登记（阶段 3 / M6 记录第 16 行）；<br>`有意不同` 只用于 rurge 有意做成别的样子、并且登记过的；缺的是功能本身的记 `没有做完`（阶段 3 / M6 记录第 10 行）；<br>#21（按主机名签发叶证书）照常下结论，密钥的做法与 Surge 6.10.0 不同登记在兼容性清单 7.2 节新的一行（P21），不因此改 #21 的结论。<br>读的时候以代码为准；同一类的句子由协调者在评审时统一 |
| P20 | 分两次派（M6-D17） | 任务 3 分 3a、3b 两次派（第一部分切法表里写着），任务 6 分 6a、6b；任务 2 多于 30 行，协调者可以分 2a、2b（它那一节写着分法，被指的行与指它的行在同一次里）。各一份报告、各一次评审，任务号不变 |
| P21 | 两行新的登记（设计第 3.5 节，M6-D21） | **7.2 节「叶证书的密钥」**（任务 2；7.2 节的栏是 `\| 键 \| 取值 / 默认 \| rurge \| 阶段 \| 备注 \|`，加在「证书固定提示」一行之后）：「键」写 `叶证书的密钥`；「取值 / 默认」写 Surge Mac 6.10.0 起撤回了"每个域名一把密钥"（理由是对大量不同域名并发 MITM 时慢；发行说明帖子 1796，2026-10-13；手册没有写）；「rurge」写 🟡；「阶段」写 4；「备注」写 rurge 每张叶证书各自新生成一把 P-256（总设计 P4-D8），阶段 4 / M5b 的 B4 量到签一张的中位数 248.6 µs（P8 的口径），改不改归项目所有者。设计第 3.5 节与 M6-D21 写的 🔁 不对——图例里 🔁 是"解析并忽略…不产生行为"，而这一行是做了、语义不同，照图例是 🟡；订正在设计第 15 节第 2 条。<br>**10.4 节抓包过滤器**（任务 4；10.4 节的栏是 `\| 端点 \| 用途 \| Surge 平台 \| rurge \| 阶段 \| 备注 \|`）：「端点」写 `GET` / `POST /v1/capture/filters` 与命令行 `capture-filter`；「用途」照手册（重对报告第 2.1、2.2 节引的原文）；「Surge 平台」照手册那一页，没写的写"没读到"；「rurge」写 ❓；「阶段」写 `待定`；「备注」写手册的出处、"需求文档没有这一句"与"去向项目所有者：阶段 4 的抓包、阶段 6 的 API 与命令行，还是不做" |
| P22 | NFR-11 要列的包（设计第 3.6 节、第 14 节第 9 项） | 阶段 4 的代码直接用到的外部包，24 个：`crates/rurge-http/Cargo.toml` 的 20 个——`tokio`、`hyper`、`hyper-util`、`http`、`http-body-util`、`bytes`、`tokio-util`、`tracing`、`rcgen`、`x509-parser`、`time`、`lru`、`rustls`、`tokio-rustls`、`sha2`、`ring`、`h2`、`p12-keystore`、`base64`、`flate2`；阶段 4 在别的 crate 加的边上的 `futures-util`（`rurge-api`）与 `regex`（`rurge-config`）；阶段 4 多了用法的 `fancy-regex`（`crates/rurge-config/src/dialect.rs`，重写的正则）与 `webpki-roots`（`crates/rurge-net/src/tls.rs`，解密的上游信的根之一，总设计 P4-D10）。`rurge-engine`、`rurge-api`、`rurge` 在阶段 4 新加的别的边都在这 24 个里 |

## 执行期修正记录

下面写"执行者的裁定"的，是 Pi 在设计与计划的原则下定的；写"协调者的裁定"的，是协调的会话代项目所有者定的。项目所有者都可以改。计划的正文不改：正文里说得不对的地方，以这张表的行为准。

| # | 任务 | 修正 | 提交 |
| - | ---- | ---- | ---- |
| 1 | 0 | 骨架 148 行：会话入口与 HTTP 引擎 11、MITM 24、重写与 Map Local 47、请求记录与抓包 22、功能开关 3、配置与观测 17、总设计第 1.2 节另列的 2、总设计第 1.5 节的遗留 4、阶段 1 ～ 3 推过来的 12、非功能需求 6；任务 1 26、任务 2 33、任务 3 52（3a 20、3b 32）、任务 4 26、任务 5 11；指向 16 行（#18、#20、#25、#26、#28、#29、#72、#73、#100、#108 ～ #111、#134、#139、#142）。四张清单在工作区的 `lists/`：`matrix-rows.md` 67 行、`manual-items.md` 68 行、`pushed.md` 43 行、`golden.md` 25 行 | 本提交 |
| 2 | 0 | 推过来的句子的核对（M6-D19）：阶段 1 的「不属于阶段 1」37 行与「需求文字有问题」40 行、阶段 3 的「不属于阶段 3」2 行与「需求文字有问题」4 行，共 83 行逐行看过；阶段 2 的「需求逐条对照代码」一节也核过。工作区 `lists/pushed-grep.txt` 的 43 行与 P3 的两类（推了句子 22 行、只提到 21 行）完全对上；名单之外没有推到阶段 4 的句子，骨架没有加行 | 本提交 |
| 3 | 0 | 评审 `0415387` 之后的修正第 1 轮，改了五处：①验收文档「八种结论」开头那句的 `（M6 设计第 15 节）` 改回 `（阶段 3 / M6 设计第 15 节）`（那是阶段 3 / M6 的设计）；②三张小表的表前一段去掉开头的表名（`总设计第 1.2 节另列的：`、`总设计第 1.5 节的遗留：`、`阶段 1 ～ 3 推过来的：`）；③验收文档第二部分黄金用例表第 19 ～ 25 行与工作区 `lists/golden.md` 的第 19 ～ 25 行，英文原句外面改成直引号、句内的反引号照旧；④`lists/matrix-rows.md` 第 860 行（10.3 节「rurge 专有」）的「对应的句子」由 `—` 改成 `#30、#31`；⑤`lists/pushed.md` 里阶段 1 第 546 ～ 567 行那十行照阶段 1「需求文字的问题」表的栏重排（需求、那一句、当时的结论） | 本提交 |
| 4 | 0 | 协调者核过任务 0（`0415387`，第 1、2 行）与它的修正第 1 轮（`5f1f89a`，第 3 行）。只改文档、没有跑门禁，照派工单。**第一轮**：148 行与计划「切法」逐行比过（需求一格、待读的任务号、指向与否），没有一行不合；十六行指向的代码一格都写成「同「…」…的「…」一行」、用例一格「同上」；这一句展开成「「…」之 …」一类写法的 32 行，展开前后的意思不变；「怎么读」与设计第 4.2 ～ 4.4 节逐行比过，不同的只有计划指定的几处（需求一列的写法、"跑的助手"、末尾的「（其余由任务 7 写）」）与第 3 行改回的那一处；`tools/check_rows.py` 是 MISS 0、AMBIG 0、待读 148（任务 1 26、任务 2 33、任务 3 52、任务 4 26、任务 5 11），`tools/count_rows.py` 优先级认不出的 0，`tools/pipes.py` 对验收文档与工作区的四份清单都没有输出；没有流程用语；带本机账户名的文件 26 个。推过来的句子（第 2 行）协调者另按关键词把阶段 1、3 那 83 行筛过一遍，名单之外没有推到阶段 4 的。要改的五处写成修正第 1 轮（第 3 行）。另：`0415387` 的提交说明写"阶段 1 的 83 行"，实际是阶段 1 的 77 行与阶段 3 的 6 行（第 2 行写对了），提交说明不改。**修正第 1 轮**：五处都对了；工作区的 `lists/golden.md` 第 19 ～ 25 行没有跟着改（报告说改了），协调者照验收文档的写法把那七行外面的反引号换成直引号（工作区文件，不进仓库）。**协调者的裁定**：任务 7 照 P17 给阶段 1 ～ 3 的验收文档加那一句时不改动它们的行号——阶段 1、2 的接在第 2 行（阶段 3 / M6 加的那一句）的同一行末尾，阶段 3 的换掉标题下面那一个空行——所以验收文档与清单里"阶段 1 对照第 N 行"一类的行号在任务 7 之后照旧有效。 | 本提交 |
| 5 | 1 | 读 M1 那一片的 26 行（#1 ～ #8、#109 ～ #111、#112、#113、#115、#116、#125 ～ #128、#133、#135 ～ #138、#140、#141）：结论 `做了` 23、`做了，补了用例` 2（#2 keep-alive、#3 chunked）、`有意不同（已登记）` 1（#128 HTTP 一侧的 `REJECT-DROP`）。补了两条用例（`cc8216b`）：`a_body_after_a_304_does_not_poison_the_next_request`、`a_pipelined_request_after_a_chunked_body_is_its_own_request`；两条的改坏验证在 `task-1-mut-1.log`、`task-1-mut-2.log` | `cc8216b`、本提交 |
| 6 | 1 | 重对报告第 3、5、9 条：第 5 条（`Proxy-Authorization`）两边都已有断言（`crates/rurge-http/src/engine.rs` 的 `proxy_authorization_is_forwarded_only_on_a_sniffed_connection`：监听器一侧上游收不到、嗅探一侧照转；`a_retargeted_sniffed_request_drops_proxy_authorization`：换了目标不转），不补；第 3 条与第 9 条各补一条用例（`cc8216b`，见第 5 行）。**执行者的裁定**：第 3 条的改坏验证先试了池的 `is_closed` / `is_ready` 恒假 / 恒真与 `Sent::NotWritten` 不重发，都改不红——读代码与临时打印确认：没有正文的应答在下一个请求到来之前不会被放回池里（`Tail::new` 的 `empty` 为假、`complete` 要等正文被轮询到末尾），第二个请求总是新拨一条连接；记 `做了，补了用例`，红的改动用“让 `Tail::new` 对每条应答都立刻 `complete()`” | 本提交 |
| 7 | 1 | 拿真实的输入读（P11）：工作区 `inputs/task-1/` 的 `real.conf`（`force-http-engine-hosts`、`always-raw-tcp-hosts` / `-keywords`、`show-error-page` / `-for-reject`）、`proto.conf`、`local-set.list`、`bad.conf`；`rurge check` 对 real.conf 无告警，bad.conf 把 `-`、`:80`、`10.0.0.0/99` 各报一条 `W0005`；`rule match --sni`（大小写不同都命中 `DOMAIN,…,extended-matching`）、`--http-host`、`--user-agent`（`Mozilla*` / `*Safari*`）、`--url`（`/a` 与 `/b` 各走一条）、`--protocol HTTP` / `HTTPS`（各命中对应行）、本地规则集文件里一行上的 `extended-matching` 作用于整集（`--sni spread.test` 命中没有标志的那一行） | 本提交 |
| 8 | 1 | 读到过时、不归本任务改的：兼容性清单第 327 行（`REJECT-DROP`）与「空闲超时」一行的备注还写着阶段 1 / M3b 的“HTTP 侧固定保持到超时”“阶段 4 自有 HTTP 引擎后统一”，与现在的代码不符；它们的阶段栏是 1，不在任务 6a 的 68 行里，写进报告的「交给后面任务的」，本任务不改清单 | 本提交 |
| 9 | 1 | 修正第 1 轮，订正第 5 行的结论与计数：**协调者的裁定**——重对报告的条目补的用例，若那一句原来就有会因它不成立而红的用例，结论照旧 `做了`（补的用例照写、括号写提交）。#2「keep-alive」（原有 `two_requests_on_one_connection_share_an_upstream_connection`）与 #3「chunked」（原有 `chunked_bodies_go_both_ways`）因此从 `做了，补了用例` 改成 `做了`。新的计数（`tools/count_rows.py HEAD`）：`做了` 25、`有意不同（已登记）` 1，共 26 行 | 本提交 |
| 10 | 1 | 订正第 6 行的那一句话：#2 的改坏验证（`Tail::new` 的 `if empty` → `if true`）的红不是在记录上——`task-1-mut-1-run.log` 的 panic 在 `assert!(second.ends_with("ok"))`，第二个应答是 `HTTP/1.1 200 OK` 加 `Content-Length: 0`：这一改把**有正文的**应答也提前结束了、正文 `ok` 被丢掉，证明的是"第二个应答的正文要完整"，与 304 之后的多余字节无关。日志与报告已照实改 | 本提交 |
| 11 | 1 | 修正第 1 轮的其余四件：**第 3 件**——只让没有正文的应答一构造出来就 `complete()` 的实验（`task-1-mut-3.log`）：单独跑 20 次红 0、绿 20；PROBE 看出放回得早也没用——自己的应答走 `complete()` 时上游连接已经是 `closed=true`（hyper 把带多余字节的连接结束了），`!sender.is_closed()` 那一问把连接丢掉，第二条请求照旧新拨；**第 4 件**——补 `crates/rurge-http/src/engine.rs` 的 `a_dropped_request_ends_early_when_the_client_closes`（`3fc8347`，改坏验证 `task-1-mut-4.log`：`.half_close(true)` 让它变红），#128 的用例格加上；**第 5 件**——`docs/api/phase4.md` 的 M1a / M1b 两节逐项对过代码与用例（表在报告里），补 `statusCode=502`、借来的 `firstByteMs`、被停止的嗅探不计、经监听器来的未知 fake 地址的 `requests` 四条断言（`f9fc0ba`、`d40ddcf`），#115、#116 的用例格照表补；**第 6 件**——#138 的用例格加上 `every_entry_brings_its_requests_into_the_engine`（经虚拟网卡来的请求，记录的 `url` 由 `Host` 拼出） | `3fc8347`、`f9fc0ba`、`d40ddcf`、本提交 |
| 12 | 1 | 全量门禁的两件事（不改期望值、不改代码）：一、第一遍 `a_leaf_says_what_the_design_says`（`crates/rurge-http/src/mitm/authority.rs:1459`，M2a 的叶证书序列号 16 字节）以 `left: 15, right: 16` 红一次——随机序列号的首字节为 0（约 1/256）时 DER 丢掉它；单独循环 50 次全过，与本次改动无关；二、第二遍 `rurge-dns` 的 lib 与 `rurge-engine --test pipeline` 两个二进制以 `STATUS_HEAP_CORRUPTION`（0xc0000374）退出（`CLAUDE.md`「这台开发机」那种），单独重跑分别 288 / 80 条全过。汇总：94 个结果行、5456 通过、0 失败、33 忽略 | 本提交 |
| 13 | 1 | 订正第 11 行里“第 5 件”“第 6 件”的说法：①#115 的用例格里 `a_server_first_protocol_passes_after_the_wait` 不是“15 秒”——它用的是 300 毫秒的 `sniff_wait`，缺省的 15 秒没有用例等满（`docs/api/phase4.md`「M1a」的「没有覆盖的」第 2 条），句子照实改了；②#138 的用例格里经虚拟网卡那一段证明的是“两个请求各一条记录、`listener` / `method` / `url` / `statusCode`”，分不出 `url` 取自 `Host` 还是取自换回名字的连接目标——虚拟网卡的连接目标是 fake 地址、引擎先换回名字，两种取法都得 `http://target.test/a`；“`url` 与 `http_host` 取自 `Host`”改由 `the_session_of_a_request_says_what_the_rules_see` 证明；③#116 的用例格补上 `a_request_without_host_is_named_by_its_target`（`20efc16` 上已有，CI 三平台通过） | 本提交 |
| 14 | 1 | 订正**延后第 12 行**：1xx 不算——hyper 1.11.1 的 `hyper-1.11.1/src/proto/h1/role.rs` 的 `Client::decoder`（第 1262 ～ 1285 行）跳过 100 与 102 ～ 199、不当最终应答交回，101 是升级、连接不回池；204、304 与 `HEAD` 是 `DecodedLength::ZERO`，所以与 304 走同一条路的只有 204 与 `HEAD`（到 `Tail::new` 也是同一个 `empty`），没有用例单独核实它们 | 本提交 |
| 15 | 1 | 订正第 12 行：①概率写错了——序列号是 16 个随机字节、最高位清零（`crates/rurge-http/src/mitm/authority.rs` 第 469 ～ 475 行，M2 设计第 214 行的表），首字节为 0 的概率是 1/128，最短编码只在第二个字节最高位也为 0（1/2）时丢掉它，合起来约 1/256；产品照设计做了，是用例断言编码之后的长度断言得太死。**去向**：那条用例属于 M2 那一片，交任务 2a（它读叶证书的那几句），只改用例、不改产品。②`f9fc0ba` 的提交说明坏了：它是用 PowerShell 的双引号字符串写的，`` `a `` 成 BEL、`` `f `` 成 FF、别的反引号被吃掉（`git log -1 --format=%B f9fc0ba` 能看到）；照规矩不改写历史，这一轮的提交说明、报告与文档改用写文件工具 | 本提交 |
| 16 | 1 | 订正第 13 行的第 ③ 处：#116 的用例格把“嗅探到的 `CONNECT` 没有 URL”记在了 `the_session_of_a_request_says_what_the_rules_see` 名下，可那条用例里没有 `CONNECT`——断言它的是同一个文件的 `a_connect_on_a_sniffed_connection_goes_to_its_target`（`crates/rurge-http/src/engine.rs` 第 6448 行 `assert_eq!(s.session.url, None)`）；用例格与报告第 5 件 M1b 表的 `url` 一项都改了 | 本提交 |
| 17 | 1 | 协调者核过任务 1：第一遍（`cc8216b`、`b52894e`、`5d97a69`，第 5 ～ 8 行）与修正第 1 ～ 3 轮（`3fc8347`、`f9fc0ba`、`d40ddcf`、`9563ec6`、`554a33c`，第 9 ～ 12 行；`c6e1200`，第 13 ～ 15 行；`26d782b`，第 16 行）。读了 26 行的结论、代码与用例格，用例格引的每一条用例逐条读过断言；补的三条用例（`cc8216b` 两条、`3fc8347` 一条）与四个断言（`f9fc0ba` 三个、`d40ddcf` 一个），四份改坏验证的日志，三遍全量门禁，真实输入与报告。**协调者的裁定**（以后的任务照做）：重对报告的条目补的用例，若那一句原来就有会因它不成立而红的用例，结论照旧 `做了`（第 9 行）。三轮改的：#2、#3 的结论（第 9 行）；#2 那一改坏验证的说法（第 10 行）；304 之后的实验（`task-1-mut-3.log`：只让没有正文的应答立刻放回连接，红 0 / 绿 20——hyper 也已把那条连接标成关闭，`Tail::complete` 的 `is_closed` 一问把它丢掉，今天有两道防线；延后第 12 行，第 14 行订正它的 1xx）；#128 已经做到的那一半补了用例（`half_close(true)` 让它红）；`docs/api/phase4.md`「M1a」「M1b」两节逐项对过，#115、#116 的用例格照表补；#115 的"15 秒"（那条用例把时限调到 300 毫秒，缺省的 15 秒没有用例等满）、#138 说过头的一句（经虚拟网卡那一段分不出 `url` 取自 `Host` 还是换回名字的目标）、#116 漏的与记错名下的用例（第 13、16 行）。门禁：第一遍 94 个结果行、5455 过、0 失败、33 忽略；修正第 1 轮第一遍 94 行、5455 过、1 失败（`a_leaf_says_what_the_design_says`，序列号的最短编码偶发 15 字节，约 1/256，第 15 行交任务 2a 改用例），第二遍 92 行 5088 过、`rurge-dns` 的 lib 与 `--test pipeline` 以 `STATUS_HEAP_CORRUPTION` 退出、单独重跑 288 / 80 过，合计 5456 = 5455 + 1；都是一台机器上的一次运行，不是 CI。另记两件：一、修正第 1 轮时 `cargo test -j 8` 同时链接几个测试二进制，内存吃紧，系统停掉了协调者在后台的包装与观察进程（Pi 与门禁照常跑完），被停的命令没有重起；**协调者的决定**：以后的派工里门禁与聚焦的用例写 `-j 4`（这台机器 64 GiB 内存；订正全局约束里的 `-j 8`）。二、`f9fc0ba` 的提交说明里有三个 BEL 与一个 FF、反引号都没了（第 15 行；评审第 2 轮的意见误写成两个 BEL），不改写历史。三、第 6、8、13、16 行用了弯引号（计划第 105 行只许直引号与「」），照「已写下的行不改写」留着；以后的派工在判据里加一条弯引号的检查。交给任务 6a：兼容性清单第 327 行（`REJECT-DROP`）与「空闲超时」一行的备注过时（第 8 行）。结论：`做了` 25、`有意不同（已登记）` 1；`check_rows.py` 是 `MISS 0`、`AMBIG 0`，待读 122；任务 1 通过 | 本提交 |
| 18 | 2a | 读 M2 那一片的前一半 22 行（#12 ～ #17、#19、#21 ～ #23、#25 ～ #32、#131、#142、#144、#145）：结论 `做了` 19、`做了，本机没有能红的用例` 2（#17、#26 的 `skip-server-cert-verify` 缺省校验读系统的根的那一层）、`有意不同（已登记）` 1（#145 Windows 不另设 ACL）。改了一条用例（`fc972b8`）：`crates/rurge-http/src/mitm/authority.rs` 的 `a_leaf_says_what_the_design_says` 的序列号断言按 DER 编码之后的样子写（原来断 `len() == 16`，首字节为 0 时偶发 15 字节）；改坏验证 `task-2a-mut-1.log`（去掉 `serial[0] &= 0x7f;`，第 1 次循环红在 17 字节）；#21 的用例格照 P14 写「改过、CI 没有见过」。重对报告第 1 条：兼容性清单 7.2 节加「叶证书的密钥」一行（`lists/matrix-rows.md` 补一行）。`docs(config):` 一处（`5441fb0`）：`crates/rurge-config/src/mitm.rs` 的 `h2` 注释照 M2b 之后的样子写。门禁（一台机器上的一次运行，不是 CI）：fmt 与 clippy 退出 0、94 个结果行 5456 通过 0 失败 33 忽略 | `fc972b8`、`5441fb0`、本提交 |
| 19 | 2a | 修正第 1 轮，订正第 18 行的四处：①**协调者的裁定**——#17、#26 从 `做了，本机没有能红的用例` 改成 `做了`：派工提示 5 说的「读系统的根那一层缺省的用例走不到」不对，`an_untrusted_origin_is_a_502_and_skip_server_cert_verify_lets_it_through` 不给 roots、走过 `build_mitm` 读 `union_root_store` 的那一支，`webpki-roots` 那一半还有 `the_union_holds_every_webpki_root_and_is_shared` 钉住；照设计第 4.4 节末段「逻辑钉住了、系统的效果没有钉住的，记 `做了`」记 `做了`。这一半的分布改成 `做了` 21、`有意不同（已登记）` 1。②序列号那一条断言（`fc972b8` 的「15 或 16」）：DER 丢掉开头每一个为 0 的字节，首两个字节都为 0 时是 14 字节（约 1/65536），照样红；`c5f33e8` 改成「至多 16 字节」，改坏验证 `task-2a-mut-2.log`（去掉 `serial[0] &= 0x7f;`，第 2 次红在 17 字节，改回后 20 / 20 绿）。③#31 的「逐字断言九段安装文字」说过了头，改成「各有它的那一段，不是逐字断言整行」。④#145 补了那一句提醒的断言（`f485306`，`task-2a-mut-3.log`：`announce` 的 Windows 那句去掉 `its own` 时红；Unix 那一支本机没有运行过），#30、#31 的用例格各加一句附注；那两行从「本机没有能红的用例的」表里删掉。门禁（一台机器上的一次运行，不是 CI）：fmt 与 clippy 退出 0、94 个结果行 5456 通过 0 失败 33 忽略 | `c5f33e8`、`f485306`、本提交 |
| 20 | 2a | 协调者核过任务 2a：第一遍（`fc972b8`、`5441fb0`、`fa455da`、`46dd115`，第 18 行）与修正第 1 轮（`c5f33e8`、`f485306`、`e6e0010`，第 19 行）。读了 22 行的结论、代码与用例格，用例格引的每一个用例名在 `HEAD` 与 `20efc16` 上都核过；改过的两条用例的断言，`5441fb0` 的注释（对着代码核过，`h2` 缺省为假），三份改坏验证的日志（`task-2a-mut-1.log` ～ `task-2a-mut-3.log`），两遍全量门禁（都重数过：94 个结果行、5456 过、0 失败、33 忽略，一台机器上的一次运行，不是 CI），真实输入（`inputs/task-2a/`）与报告。这一半的分布：`做了` 21、`有意不同（已登记）` 1；全文 `做了` 46、`有意不同（已登记）` 2、`待读` 100。**协调者的裁定**（以后的任务照做）：①设计第 4.4 节第八种的第二种情形（缺省的用例照规矩走不到）只用于作用于这一句的代码整段没有缺省用例走到的；缺省用例走到了、逻辑有会红的用例、只有系统的效果（读本机的信任库一类）没有断言的，记 `做了`，代码格写清哪一半没有断言（#17 的先例；派工提示 5 写"缺省的用例照规矩走不到"，是协调者的前提错了）。②一条用例为别的行加了断言、这一行引的断言没有改的，这一行的 CI 说法照旧，括号里加一句附注；靠新断言的那一行写"这一条改过，CI 没有见过"（#30、#31 与 #145 的先例）。③用例格写"逐字"只用于断言整段文字的用例（#31 的先例）。订正第 19 行一处：④末尾"那两行从「本机没有能红的用例的」表里删掉"说的是①的 #17、#26（#30、#31 从来不在那张表里）。另记：`a_leaf_says_what_the_design_says` 的 `serial[0] & 0x80 == 0` 对 yasna 以正数写出的任何序列号都成立，抓住"最高位清零"的是长度那一条（两次改坏验证都红在 17 字节）；`task-2a-mut-2.log` 改回之后的 20 次只记了一行计数，同一条用例在门禁里也通过。#31 打印的装 CA 命令，协调者对着三处平台文档核过：Fedora 的文档（锚点目录与 `update-ca-trust`）、Android 11 起用户装的 CA 只给声明信任它的应用（那一行写了）、微软的 `certutil -user -addstore` 写当前用户的存储；这是读文档，不是真机上的证据，「真机上」一格照旧。工作区（不提交）：`lists/matrix-rows.md` 照第 588 行加的一行重排了行号（之后的 28 行各挪一行），3b 的派工与任务 6 的简报改了第 620 → 621 行；新工具 `tools/ctrl.py`（提交说明里的控制字符）进以后各任务的判据；2b 的派工去掉了 `5441fb0` 已做的那一条。交给后面任务的照报告：6a 重数兼容性清单 7.2 节的附录（阶段栏含 4 的行 67 → 68），「CA 生成」一行"写这一行时 CI 也还没有编译过它"过时；6b 可在 M2/W1 的期望里加 Windows 那一句提醒 | 本提交 |
| 21 | 2b | 读 M2 那一片的后一半 11 行（#9 ～ #11、#18、#35、#105、#108、#117 ～ #119、#139）：结论 `做了` 11。补了一条用例（`5a99dc9`）：`crates/rurge-http/src/engine.rs` 的 `an_upstream_h2_connection_that_ends_mid_body_truncates_the_client`（重对报告第 7 条：解密的连接上，上游 h2 连接在应答头与一部分正文之后关掉，客户端正文出错、记录是 `Failed`；`OriginScript` 加 `CloseMidBody`），改坏验证 `task-2b-mut-1.log`（`PolledUpstream::Failed` 那一支改成 `this.complete()`，红在 `expected a failure, got Some(Completed)`），20 次循环全绿（`task-2b-green.log`）。重对报告第 4 条（客户端读得慢时内存不涨）与第 6 条（格式不对的 h2 请求只重置自己的流）在现在的代码上写不出会红的用例，报告里各一条一段：第 4 条读 hyper 与 h2 的流量控制与发送缓冲缺省（rurge 只把 `max_concurrent_streams` 设成 100，其余是 hyper 的缺省：服务端窗口 1 MiB、发送缓冲 400 KiB；客户端窗口 2 MiB、连接窗口 5 MiB、发送缓冲 1 MiB），第 6 条读 h2 把连接专用的头当流级错误（`h2-0.4.19/src/frame/headers.rs` 的 `load_hpack`、`src/codec/framed_read.rs` 的 `MalformedMessage` → `Error::library_reset`）。`docs/api/phase4.md` 的「M2a」「M2b」「M2c」三节的端点、字段、计数与日志行逐个核过，都有断言形状的用例；`W0040` 拿真实的输入读过（`inputs/task-2b/` 的两份 Profile：有 `[MITM] hostname` 时 `W0040` 在 `W0039` 之前，只有 `http-api-tls` 时只有 `W0040`）。门禁（一台机器上的一次运行，不是 CI）：fmt 与 clippy 退出 0、94 个结果行 5457 通过 0 失败 33 忽略 | `5a99dc9`、本提交 |
| 22 | 2b | 修正第 1 轮，订正第 21 行的四处：①第 4 条那一段把 hyper 服务端的接收窗口（1 MiB）当成了应答一侧的上限：应答一侧在途的是客户端给的窗口与 h2 每个流约 400 KiB 的发送缓冲（`hyper-1.11.1/src/proto/h2/server.rs` 第 39 行；`h2-0.4.19/src/server.rs` 的 `max_send_buffer_size` 文档说它是每个流的），hyper 服务端的 1 MiB 窗口是 rurge 给客户端的接收窗口、管请求正文；②#9 的用例格不再指向工作区的报告，第 4、6 条读到的写进了那一格，第 4 条的去向是「延后事项」第 13 行；③#9 里 `a_client_is_served_http1_or_nothing` 的说法（它试的是什么都不给与两种都给，没有试只给 `h2` 的；只给 `h2` 的由 `a_client_offering_only_h2_is_passed_through`）；④#11 补了 `url_regex_with_extended_matching_rewrites_host`，`:authority` 的手册原句在 `rules/domain.html`、`rules/http.html`、`rules/overview.html`（协调者取的，见报告「修正第 1 轮」） | 本提交 |
| 23 | 2b | 协调者核过任务 2b：第一遍（`5a99dc9`、`952b8f8`、`70859fc`，第 21 行）与修正第 1 轮（`abf4aec`，第 22 行与延后第 13 行）。读了 11 行的结论、代码与用例格，用例格引的每一个用例名在 `HEAD` 与 `20efc16` 上都核过（`70859fc` 之前引过一个不存在的名字，`check_rows.py` 的 MISS 抓到，Pi 自己订正了；换上的 `a_client_is_served_http1_or_nothing` 的说法第 22 行③又订正了一次）；三处"逐字"（`W0040` 的正文与 `hint`、`GET /v1/mitm/ca` 的两个头、`/v1/rurge/http` 的原始字节）都是整段断言，成立。补的用例 `an_upstream_h2_connection_that_ends_mid_body_truncates_the_client` 读过：两处等待都有 5 秒的上界，改坏验证 `task-2b-mut-1.log` 有红有绿，改回之后的 20 次只记了一行计数（同一条用例在门禁里也通过）。门禁重数过（一台机器上的一次运行，不是 CI）：93 个结果行 5441 过，加 `outbounds_http2` 以 `STATUS_HEAP_CORRUPTION` 退出之后单独重跑的 16 过，合计 94 个结果行、5457 过（5456 + 1）、0 失败、33 忽略。这一半 `做了` 11；全文 `做了` 57、`有意不同（已登记）` 2、`待读` 89。第 22 行①写进验收文档的 hyper 与 h2 的事实，协调者对着源码核过（`hyper-1.11.1/src/proto/h2/mod.rs` 的 `PipeToSendStream` 等到发送容量才交出手里那一块；`src/proto/h2/server.rs` 第 39 行与 `client.rs` 第 48、49 行的缺省；`h2-0.4.19/src/codec/framed_read.rs` 第 173 ～ 177 行与 `src/server.rs` 的 `convert_poll_message` 都是流级的 `PROTOCOL_ERROR`）。**协调者的裁定**（以后的任务照做）：①验收文档不指向工作区里的文件（报告、日志）：读到的事实写进那一格，要所有者定的进计划的「延后事项」、格子里写"延后第 N 行"（#9 的先例；派工提示 2、3 与判据写"报告里写清"，是协调者的话没说清）。②用例格里给一条用例写的括号只写它断言了的情形，它没有试的不写（#9 的先例）。`:authority` 的手册原句由协调者从 2026-11-03 的手册文本里取（`rules/domain.html`、`rules/http.html`、`rules/overview.html` 的 `extended-matching`），交给了 #11 | 本提交 |
| 24 | 3a | 读 M3 那一片的前一半 20 行（#36 ～ #52、#106、#114、#120）：结论 `做了` 19、`做了，补了用例` 1（#45）。补了一条用例（`5679d6c`）：`crates/rurge-http/src/rewrite/header.rs` 的 `a_rule_of_one_direction_leaves_the_other_alone`（Header Rewrite 的方向：请求规则在 Response 方向不改一个头、应答规则在 Request 方向也不改；这以前没有一条会因这条按方向跳过的分支不成立而红的用例——把它改成恒不跳过，`rewrite::header::` 六条与 `--test rewrite` 五十条全过）。改坏验证 `task-3a-mut-1.log`（红在 `values_of(&headers, "X-Resp").is_empty()`，改回后 `git status --short` 只剩那一个新用例的那个文件）。拿真实的输入读了两个节：`inputs/task-3a/rewrite.conf`（四种模式、省略类型、`$1`、`{{{GATEWAY_ADDRESS}}}`、结果不是 `http` / `https` / `ws` 的一行、四种动作、两个方向、省略方向、改框架头的两行）给 2 条 `W0041`（两行框架头，行保留）、没有别的诊断；`inputs/task-3a/rewrite-bad.conf`（六行读不了的）给 6 条 `W0012`，URL 的三条是同一句、Header 的三条各说各的。手册原句在 `manual-1103/http_url-rewrite.txt` 与 `manual-1103/http_header-rewrite.txt`。`#120` 里 `POST /v1/features/rewrite` 之后那行 INFO `rewrite switched via http-api` 的代码在 `crates/rurge-api/src/routes/features.rs::set_feature`，没有断言它形状的用例（`rurge-api` 的用例没有抓日志的装置，补一条要加依赖，按「一条用例补得上」不成立，不补） | 本提交 |
| 25 | 3a | 修正第 1 轮，订正第 24 行的八处：①改坏验证补跑（`task-3a-mut-2.log`）：同一处方向分支改成恒不跳过，`--lib -- engine::tests:: rewrite:: --skip a_rule_of_one_direction_leaves_the_other_alone`（`rurge-http` 的 lib 182 条）与 `--test rewrite`（51 条）全绿，所以 #45 照旧 `做了，补了用例`；②#44 补一条经引擎的用例 `a_rule_matches_the_url_rebuilt_from_the_connection_target`（`crates/rurge-engine/tests/rewrite.rs`，`06d0806`）：经 SOCKS5 连 `ad.com`、`Host` 写 `other.test`，只有第三个候选（连接目标）命中，改坏验证 `task-3a-mut-3.log`、20 次循环 `task-3a-green.log`；#44 的用例格去掉 `the_manual_matching_details_example`、换成这一条；③「真机上」#36、#38、#39、#45、#47、#48、#49、#50 由 `M3/W6、M3/M6、M3/L6` 改成 `—`（W6 只做到 `302` 与请求方向的 `header-add`），#37、#46 不动；④四行补 🟡 说明：#41、#42（7.3 节）与 #47、#48（7.4 节）；⑤#36 的用例换成 `header_mode_sends_the_request_to_the_new_target`，#48 加 `a_changed_host_is_written_back`；⑥第 6 件的括号订正（#40、#42、#43、#46、#47、#49、#50 与 #114）；⑦#120 的两处：`rurge_http_counts_start_at_zero` 改过的附注、日志行那一句改成「这一节写的日志行没有断言它们原文的用例」；⑧真实输入 `inputs/task-3a/rewrite.conf` 补一行不写类型的、把 `$1` 那一行改成真有捕获组的，重跑 `check` 仍是 2 条 `W0041` | 本提交 |
| 26 | 3a | 修正第 2 轮，订正第 25 行第⑥处的 #50（理由从括号里挪到括号外，另起一句），另改 #45 的 `the_manual_header_rewrite_examples` 括号（照它断言的写：请求方向的三条、应答方向的一条）与 #48 的代码格（7.4 节那一行登记的「讲 HTTP/2 的上游上写不成 authority 时也不写回」） | 本提交 |
| 27 | 3a | 协调者核过任务 3a：第一遍（`5679d6c`、`6d0ad8d`、`0383974`，第 24 行）、修正第 1 轮（`06d0806`、`8e7c590`，第 25 行）与修正第 2 轮（`e571a3b`，第 26 行）。读了 20 行的结论、代码、用例与「真机上」四格，用例格引的每一个用例名在 `HEAD` 与 `20efc16` 上都核过，被引的用例逐条读过：第 1 轮订正了八处括号（#40、#42、#43、#46、#47、#49、#50、#114）写了用例没有断言的情形或写错了值、#36 引错的一条用例（`a_changed_host_is_written_back` 讲的是 Header Rewrite 写回 `Host`，挪到 #48）、#120 不补日志用例的理由（`crates/rurge/tests/cli.rs` 读得到守护进程的标准错误与日志文件，补得上；不补是因为 P16 只要端点、字段与计数）；第 2 轮订正了 #50 括号里一句用例没有的"倒过来先加后删"（第 1 轮的派工写了放在括号外，没有照做）、#45 一处读来像是断言了方向隔离的括号，并补上 #48 漏写的 7.4 节那一处（讲 HTTP/2 的上游上写不成 authority 时也不写回；是协调者第 1 轮的派工漏了）。补的两条用例读过：`a_rule_of_one_direction_leaves_the_other_alone`（`5679d6c`）的改坏验证第一遍只跑了 `rewrite::header` 一个模块，第 1 轮把配 `[Header Rewrite]` 的用例全跑了（`git grep` 过：只在 `rurge-http` 的 `rewrite/`、`engine.rs` 与 `--test rewrite` 里），方向的判断改坏时 `rurge-http` 的 lib 182 条与 `--test rewrite` 51 条全绿，所以 #45 记 `做了，补了用例`；`a_rule_matches_the_url_rebuilt_from_the_connection_target`（`06d0806`）经 SOCKS5 让三个候选里只有第三个配得上，`candidates` 不加第三个候选时红在 403 那一条断言（`rewrite.rs` 第 675 行；原始输出被复原之后那一次盖掉了，红的那一次协调者在 Pi 的会话记录里核过），单独跑 20 次全过；#44 原来就有会红的单元用例，照第 9 行记 `做了`。门禁重数过（一台机器上的一次运行，不是 CI）：93 个结果行 5445 过，加 `outbounds_hysteria2` 以 `STATUS_HEAP_CORRUPTION` 退出之后单独重跑的 14 过，合计 94 个结果行、5459 过（5457 + 2）、0 失败、33 忽略。这一半 `做了` 19、`做了，补了用例` 1；全文 `做了` 76、`做了，补了用例` 1、`有意不同（已登记）` 2、`待读` 69。交给任务 6b 的两处：手工清单 M3 一节的 W6 只做到 `302` 与请求一侧的 `header-add`；W6 的期望引了 `header_mode_sends_the_request_to_the_new_target`，步骤里却没有 `header` 模式。**协调者的裁定**（以后的任务照做）：①兼容性清单标 🟡 的行，若与手册不同的只是边角、这一句说的那件事 rurge 照做，结论记 `做了`，代码格写出那处差异与登记它的清单行（#41、#42、#47、#48，同任务 2a 的 `hostname`、叶证书与 CA 两行）；`有意不同（已登记）` 只用于那件事本身做成了别的样子的。②「真机上」只列步骤真的做了这一句说的主要的事的手工项；只走到一部分的，括号里写走到哪一部分；没有手工项走到的写 `—`（#36、#38、#39、#45、#47 ～ #50）。③改坏验证先 `git grep` 那段代码的节名或函数名，跑用到它的全部用例，不只跑那一个模块；红的那一次与复原之后那一次的原始输出各存一个文件，后一次不盖掉前一次（#44、#45）。④用例格的括号只写断言了的；"这条断言为什么钉住了这一句"一类的理由（例如"倒过来会怎样"）写在括号外、另起一句（#50） | 本提交 |
| 28 | 3b | 读 M3 那一片的后一半 32 行（#53 ～ #82、#121、#132）：结论 `做了` 26、`做了，补了用例` 1（#79）、`没有做完（已登记）` 2（#57 jq、#63 `br` 与 `zstd`）、`需求文字有问题` 3（#65、#80、#82）。补了两条用例（`c5a37ba`）：`crates/rurge-engine/tests/rewrite.rs` 的 `the_pipeline_order_is_header_then_url_then_body`（Header Rewrite → URL Rewrite → Body Rewrite 的次序，原来没有一条会因次序不对而红的用例）、`crates/rurge-http/src/engine.rs` 的 `a_bodyless_answer_is_not_rewritten`（重对报告第 8 条的 rurge 一半：HTTP/1.1 的 304 与 HTTP/2 的 304、204、`HEAD` 命中正文规则时不缓冲、不凭空加正文与 `Content-Length`）。改坏验证：`task-3b-mut-1.log`（URL Rewrite 用 Header Rewrite 之前的 `Host` 时 pipeline 用例红在 403 那一条）、`task-3b-mut-2.log`（`response_buffer` 一律交回 `true` 时 bodyless 用例红在 304 那一条），各有红绿两份原始输出；两条各自单独循环跑 20 次全过。登记：`没有做完的` 两行（#57、#63，去向总设计 Q4），`需求文字的问题` 三行（#65、#80、#82，去向项目所有者）。重对报告第 8 条写进 #64 的用例格。黄金用例表 25 行填进第二部分的表与工作区的 `lists/golden.md`（jq 两行写「没有」）。真实输入 `inputs/task-3b/`：`[Body Rewrite]` 的多对替换、`^$`、带捕获组的、jq 两种写法、`[Map Local]` 四种 `data-type`、相对路径与 `.invalid` 的 URL、两种 `header`、`status-code` 的四个边界，过 `rurge check`。**执行者的裁定**：①#57 与 #63 的「真机上」括号里不写反引号包着的 `W0042` / `br` / `zstd`（`check_rows.py` 把反引号里的裸码与裸名字当手工项或 L2 用例），码写在代码格；②没有用例的黄金示例（#10 `header-replace DNT 1`）引 `the_four_actions` 并在括号里写出它用的是 `User-Agent`；③手工清单这一半的真机项照协调者的表填，读到不同的照读到的（这一半没有读到不同）。门禁（一台机器上的一次运行，不是 CI）：fmt 与 clippy 退出 0，`rurge-proto` 的测试二进制以 `STATUS_HEAP_CORRUPTION` 退出之后单独重跑 503 全过，合计 94 个结果行、5461 过（5459 + 2）、0 失败、33 忽略 | 本提交 |
| 29 | 3b | 修正第 1 轮（评审意见六件）。①改坏验证第 27 行裁定③补跑：`git grep` 找到配三节的用例在 `--test rewrite`、`--test bench` 与 `--lib`；三次改坏各跑 `--test rewrite`（全部）与 `--lib -- engine:: rewrite::`，日志 `task-3b-mut-1.log` 的第 2 次一节（改坏 1，URL Rewrite 用 Header Rewrite 之前的 `Host`：只有补的 `the_pipeline_order_is_header_then_url_then_body` 红，红在 403 那一条；日志 `task-3b-mut-1b-red.log` / `-green.log`）、`task-3b-mut-3.log`（改坏 3，`request_body_rules` 用 URL Rewrite 之前的 `url`：同样只有那一条红，红在正文那一条；`task-3b-mut-3-red.log` / `-green.log`）、`task-3b-mut-2.log` 的第 2 次一节（改坏 2，`response_buffer` 一律交回 `true`：另有既有的 `head_and_204_answers_are_not_buffered` 与 `which_answers_are_buffered` 红；`task-3b-mut-2b-red.log` / `-green.log`）。结论照读到的：#79 两半各只有补的那一条红，照旧 `做了，补了用例`（用例格里写出两半各自没有会红的用例）；#64 另有既有用例红，照旧 `做了`（用例格照第 4 件改括号）。②#121 补两条用例（`c2b9ae1`）：`a_request_body_that_fails_while_buffered_closes`（`request body failed: …`）、`a_response_that_fails_while_buffered_answers_502`（`upstream body failed: …` 与页里 `The upstream server's response body failed.`），各改了那一句的格式串做改坏验证（`task-3b-mut-4.log`、`task-3b-mut-5.log`，都只有补的那一条红）、各循环 20 次（`task-3b-loop-4.log`、`task-3b-loop-5.log`）；#121 改记 `做了，补了用例`，用例格按小节分组，写清每一项的用例（`Map Local: reading … failed` 与 `reading … stalled` 两种没有断言原文的用例，各带代码的位置与为什么补不出；`POST /v1/external_resources/update` 引 `external_resources_are_updated_one_key_at_a_time`，写明没有用例经它更新一项 `data`）。③「真机上」#66、#70、#71、#132 的 `M3/P8` 各加括号写出走到的那一支（括号里不写反引号）。④#64 补的那条用例的括号写出它是重对报告第 8 条，并写明客户端讲 HTTP/2 时的 304、204、`HEAD` 与 HTTP/1.1 的 304。⑤黄金用例表第 11、14、15、18、20 行：删去没有任何用例断言的一句（第 11 行的先加后删）、第 14 行换成各有一对的用例并写明这一行没有逐字用例、第 15 行写清两行并加 `Body Rewrite: response by {式子}` 与 `:\d+` 的备注、第 18 行的 1×1 改成以 `GIF89a` 开头、第 20 行补 `framing_changes_are_found` 与 `framing_headers_are_kept_with_w0041` 的断言、`在拨号之前` 改成 `失败`；验收文档与 `lists/golden.md` 两处同改，`golden-cmp.py` 得 `rows 25 25`、`bad []`。门禁（一台机器上的一次运行，不是 CI）：fmt、clippy 退出 0，全量 94 个结果行、5463 过（5461 + 2）、0 失败、33 忽略；这一次没有撞上 `STATUS_HEAP_CORRUPTION`，不需要单独重跑 | 本提交 |
| 30 | 3b | 修正第 2 轮（评审意见三件）。①#121 的用例格五处：`Skipped` 那一组去掉 `the_request_limit_is_thirty_two_mib`（它断言的是 `Failed`）；解压之后越过上限的两句改引补的 `a_decoded_response_over_ten_mib_is_not_rewritten` 与 `a_decoded_request_over_thirty_two_mib_is_not_rewritten`，`a_decompression_bomb_stops_at_the_limit` 留着并写「同一个格式，用例自己的上限 `1000 B`」；`a_file_that_is_a_directory_answers_502` 的括号照它断言的写（记录与页里的 `Connection failed`，没有断言页里的说明与备注）；记录 `Body Rewrite: the request body is over 32 MiB` 加进记录那一组；「外部资源的 data」那一组每条括号照它断言的写，`POST /v1/external_resources/update` 写明没有用例经它更新一项 `data`、按键/`"all"`/列不列出是同一个判断（`type_name` 交回的是不是 `Some`）、`data` 列得出来由 `map_local_urls_are_listed_as_data` 断言。②补两条用例（`0b01ebf`）：`a_decoded_response_over_ten_mib_is_not_rewritten`（11 MiB 解压、备注 `the decoded body is over 10 MiB`、正文原样、`counted(0,0,1,0)`；用时 0.11s）、`a_decoded_request_over_thirty_two_mib_is_not_rewritten`（33 MiB 解压、备注 `the decoded body is over 32 MiB`、`Completed`、`counted(0,0,1,0)`；用时 0.26s）；各做一次改坏验证（改坏 6 应答的 `RESPONSE` 改成 `REQUEST`、改坏 7 请求的 `REQUEST` 改成 `RESPONSE`，都只有补的那一条红，`--test rewrite` 都 52 条全过；`task-3b-mut-6.log`、`task-3b-mut-7.log`），各循环 20 次全过（`task-3b-loop-6.log`、`task-3b-loop-7.log`）。③#136、#137 引上补的两条，括号写它断言的、后面写提交与「CI 没有见过」；#137 的 `a_decompression_bomb_stops_at_the_limit` 括号补上「用例自己的上限」。门禁（一台机器上的一次运行，不是 CI）：fmt、clippy 退出 0，全量 94 个结果行、5465 过（5463 + 2）、0 失败、33 忽略；没有撞上 `STATUS_HEAP_CORRUPTION` | 本提交 |
| 31 | 3b | 修正第 3 轮（评审意见三处，只改文字、不跑门禁）。①#137 的 `a_decompression_bomb_stops_at_the_limit` 括号照它断言的重写：`decode` 解到越过上限就停，交回 `the decoded body is over 1000 B`，上限是用例自己的（#121 里那一处不动）；订正第 30 行③：那一处是把括号换掉了、不是补上。②#121 末尾的 CI 说法改成「`c2b9ae1` 与 `0b01ebf` 各补的两条：CI 没有见过」（格子中间「后两条 `0b01ebf` 补，CI 没有见过」那一句留着）；两处 `data` 加上反引号，并把「端点的形状」那一句移到那一段的末尾，好让两个 `data` 的名字检查落在 `crates/rurge-net/src/resource/mod.rs`（它有 `fn data`）上——不然 `check_rows.py` 会把反引号里的 `data` 当成 `crates/rurge-api/tests/api.rs` 的用例名而报 MISS。只改文档，没有跑门禁 | 本提交 |
| 32 | 3b | 协调者核过任务 3b：第一遍（`c5a37ba`、`81821be`，第 28 行）、修正第 1 轮（`c2b9ae1`、`79a3018`，第 29 行）、修正第 2 轮（`0b01ebf`、`aa86355`，第 30 行）与修正第 3 轮（`0d74ec1`，第 31 行）。读了 32 行的结论、代码、用例与「真机上」四格；用例格引的每一个用例名在 `HEAD` 上都核过，不在 `20efc16` 上的六条正好是这几轮补的，所在的行都写了"CI 没有见过"；被引的用例逐条读过，#121 引的每一条按括号里写的原文对过它的断言。第 1 轮：改坏验证照第 27 行裁定③补跑，#79 的两半（Header → URL、URL → Body）各改坏一次、各只有补的那一条红，记 `做了，补了用例`；#64 的改坏另有两条既有的用例红，照第 9 行记 `做了`；#121 原来只对上了计数、`data` 一类与日志行，备注、记录与错误页的句式一句也没有对上，第 1 轮按 API 参考的小节逐项对上用例，补了缓冲时请求正文出错与缓冲应答时上游出错两条（`c2b9ae1`），结论改记 `做了，补了用例`；另改了 #66、#70、#71、#132 的 `M3/P8` 走到的那一支、#64 那条用例的括号与黄金用例表的五行。第 2 轮：#121 订正五处——`Skipped` 那一组多列了一条 `Failed` 的、目录那一条写成"同一种"（它没有断言页里的说明与备注）、漏了记录 `Body Rewrite: the request body is over 32 MiB`、外部资源 `data` 那一组的括号写多了、一句"走资源管理器自己的用例"说多了（资源管理器的用例没有一条用 `MapLocal`）；`the decoded body is over 32 MiB` 与 `the decoded body is over 10 MiB` 两句原来只有一条用例以自己的上限 `1000 B` 断言同一个格式，请求、应答两侧各传哪个上限也没有用例钉住，补两条（`0b01ebf`，正文由多个 gzip 成员拼成，单独跑 0.11 秒与 0.26 秒），两侧的上限各改坏一次、各只有补的那一条红，#136、#137 引上它们，结论照第 9 行不变。第 3 轮：#137 那条解压炸弹用例的括号是从 #121 照搬的，"同一个格式"在那一行没有所指，照它断言的重写；#121 末尾的 CI 说法漏了 `0b01ebf`；两处 `data` 加反引号。改坏 2（第 2 次）与改坏 4 ～ 7 红的那一次没有加 `--no-fail-fast`，`rurge-http` 的库红了之后，排在它后面的十二个库没有跑：其中只有 `rurge-interop` 经 `rurge-engine` 依赖 `rurge-http`，而它的库没有配上过滤词的用例，其余的都不依赖它，所以"只有补的那一条红"站得住。门禁重数过（一台机器上的一次运行，不是 CI）：第一遍 93 个结果行 4958 过，加 `rurge-proto` 的库以 `STATUS_HEAP_CORRUPTION` 退出之后单独重跑的 503 过，合计 94 个结果行、5461 过；第 1 轮 94 个结果行、5463 过；第 2 轮 94 个结果行、5465 过（5463 + 2）；都是 0 失败、33 忽略。第 3 轮只改文字，没有跑门禁。这一半 `做了` 25、`做了，补了用例` 2（#79、#121）、`没有做完（已登记）` 2（#57 jq、#63 `br` 与 `zstd`，去向总设计 Q4）、`需求文字有问题` 3（#143、#158、#160，去向项目所有者）；全文 `做了` 101、`做了，补了用例` 3、`没有做完（已登记）` 2、`需求文字有问题` 3、`有意不同（已登记）` 2、`待读` 37（任务 4 26、任务 5 11）。交给任务 7 的：API 参考「M3b」一节「外部资源的 `data`」引的两条用例说多了（`the_same_map_local_url_is_one_resource` 没有断言"第一次登记定下种类与标签"，`offline_mode_uses_the_cache_only` 是资源管理器的通用用例、用的是 `RuleSet`），"可以按键更新它，`"all"` 也更新它"没有经端点的用例。**协调者的裁定**（以后的任务照做）：①订正第 27 行裁定②的末一句"没有手工项走到的写 `—`"：照设计第 4.3 节分三种——L1 的用例证明了这一句的全部、不必上真机的写 `—`；要真机、却没有一种手工操作看得出来的写 `看不出（…）`；要真机、清单里缺这一项的写 `缺`（任务 6b 补项）；需求文字有问题的行照阶段 3 的先例写 `—`。3a 与 3b 写 `—` 的行，除了三行需求文字有问题的，都属第一种。②一条补的用例分几段、各钉住这一句的一部分的，每一段各做一次改坏验证、各记哪几条红（#79）。③API 参考一节写的备注、记录的 `error` 与错误页说明的每一种句式都算字段的形状，各要对上一条断言它原文的用例（`format!` 拼出原文再整句比的也算）；断言"没有什么"的用例不算，断言同一个格式、参数换成用例自己的值的也不算，原文在回环上补得出的当场补（#121）。④括号不写"同一种""同上"一类的省略，也不从别的行照搬（#121、#137）。⑤改坏验证红的那一次也加 `--no-fail-fast`：不加时一个测试二进制红了，排在它后面的不再跑，"只有补的那一条红"就要另外说明没跑的那些碰不到改坏的代码 | 本提交 |
| 33 | 4 | 读 M4 那一片的 26 行（#83 ～ #94、#95 ～ #104、#107、#122、#123、#134）：结论 `做了` 22、`做了，补了用例` 1（#122）、`不属于阶段 4` 2（#86 设备、#92 脚本）、`需求文字有问题` 1（#102 Dashboard）。#87「远端地址」的三种 UDP 载体（`vmess`、`h2-connect`、MASQUE）照旧 `null`，照 M4 细化设计第 10.3 节的登记记 `做了`、代码格写出那处（登记进 `docs/api/phase4.md`）；#84、#85 的「真机上」照手工清单 M1 一节的 W1、W2（W1 断记录的方法 / URL / 状态码，W2 断 `pid` / `processPath`）。补一条用例（`3d4702b`）：`crates/rurge-api/tests/api.rs` 的 `the_outcome_upgraded_and_the_budget_truncation_are_named`（P16：M4a 一节里 `outcome` 的 `upgraded`、`truncated` 的 `budget`、没有拨号的 `remoteAddr` 是 `null` 三处没有断言形状的用例），#122 因此记 `做了，补了用例`；改坏验证 `task-4-mut-1.log`、`task-4-mut-2.log`（各红绿两份原始输出）。登记：兼容性清单 10.4 节在 `GET /v1/requests/recent` 那一行之后加抓包过滤器一行（`GET` / `POST /v1/capture/filters` 与 `capture-filter`，❓，阶段 `待定`），工作区 `lists/matrix-rows.md` 表末补一行（标"不在 68 行里"）；验收文档「需求文字的问题」加 #102 一行。真实输入（工作区 `inputs/task-4/`）：带 `always-capture=<名字>` 的规则行与 `RULE-SET` 行经 `rule match --explain` 照读出来，空值与不带 `=` 的分别给 `W0012` / `W0003`；`rurge run --help` 的两个参数帮助文字照代码。门禁（一台机器上的一次运行，不是 CI）：fmt、clippy 退出 0，94 个结果行、5466 过（5465 + 1）、0 失败、33 忽略 | `3d4702b`、本提交 |
| 34 | 4 | **执行者的裁定**：（1）#87 的三种 UDP 载体照旧 `null` 记 `做了` 不记 `有意不同（已登记）`——照协调者第 27 行裁定①（与手册不同的只是边角、这一句说的那件事 rurge 照做，记 `做了`、代码格写出差异）与阶段 1 验收第 417 行（同一处的旧口径记 `做了`、限制写进代码格并在兼容性清单登记）的先例。（2）#84、#85 的「真机上」照手工清单 M1 一节的项（W1 断记录的 `method` / `url` / `statusCode`，W2 断 `pid` / `processPath`）；派工单第 14 条的映射没有列它们（那一节只读了 M4 一节的步骤），照第 14 条末句"读到的照读到的写"。（3）#94「终止活动请求」没有手工项、L1 的用例证明全部，记 `—`；#86、#92、#102 照设计第 4.3 节与阶段 3 的先例记 `—`。 | 本提交 |
| 35 | 4 | 修正第 1 轮（评审意见九件）。①改坏验证的范围（第 27 行裁定③、第 32 行裁定②）：六次改坏红的那一次都跑全量 `cargo test -j 4 --workspace --no-fail-fast`，从 `failures:` 抄出红了哪几条。改坏 1（`outcome_name` 的 `Upgraded` → `"completed"`，`task-4-mut-1-red-2.log`）：只有补的 `the_outcome_upgraded_and_the_budget_truncation_are_named` 红（`har.rs` 与 `--test cli` 都没有断言 `_outcome`）。改坏 2（`body_json` 的 `Budget` → `"limit"`，`task-4-mut-2-red-2.log`）：只有补的那一条红，`--test engine` 以 `STATUS_HEAP_CORRUPTION` 退出（`task-4-mut-2-rerun.log` 8 过）。改坏 3（`entry_json` 的 `remoteAddr` 加 `.unwrap_or_default()`，`task-4-mut-3-red.log`）：只有补的那一条红。改坏 4（`ConnUpstreams::open` 的 `self.process.clone()` → `ProcessSlot::unknown()`，`task-4-mut-4-red.log`）：补的 `the_requests_on_an_engine_connection_carry_its_process` 与既有的 `run::processes::a_process_rule_matches_the_test_process` 两条红，`--test outbounds` 以 `STATUS_HEAP_CORRUPTION` 退出（`task-4-mut-4-rerun.log` 19 过）。改坏 5（`entry` 的 `store.limits().body` → `u64::MAX`，`task-4-mut-5-red.log`）：只有补的 `the_decode_limit_is_the_body_limit_of_this_process` 红。改坏 6（`har` 的 `store.limits().body` → `u64::MAX`，`task-4-mut-6-red.log`）：只有补的那一条红，`--test resolver` 以 `STATUS_HEAP_CORRUPTION` 退出（`task-4-mut-6-rerun.log` 11 过）。②两条补的用例一个提交（`c24a278`，`test: 补经引擎的请求带着进程与解开之后越过单条上限两条用例`）：`the_requests_on_an_engine_connection_carry_its_process`（`crates/rurge-engine/tests/process.rs`：一条引擎连接上两个请求各一条记录、`method` / `url` / `rule` 与 `process` 都对上，一条连接只查一次）与 `the_decode_limit_is_the_body_limit_of_this_process`（`crates/rurge-api/tests/api.rs`：`?decode=true` 与 HAR 的 `comment` 的上限就是进程的单条上限）；各单独循环 20 次全过（`task-4-loop-1.log`、`task-4-loop-2.log`），用时各约 0.03 秒与 0.2 秒。③九件：#85 的结论**照旧 `做了`**——改坏 4 另有既有的 `run::processes::a_process_rule_matches_the_test_process` 红，照第 9 行记 `做了`，用例格引上补的那一条与红了的那一条、并写明原来引的两条是 `CONNECT`、不经引擎；#122 照旧 `做了，补了用例`，用例格引上补的解码上限那一条、HAR 的 `comment` 那一句改成「两句都有时」；#93 的用例格重新分组（每一句只列断言它的用例，`always-raw-tcp-hosts` 那两句列三条，不写备注的五条另成一组、`pre_matching_writes_no_reason_unless_a_list_came_first` 的括号写它两半各断言什么）；#94 的用例格引上经引擎的请求被终止的八条（`crates/rurge-engine/tests/http.rs` 三条、`crates/rurge-http/src/engine.rs` 五条），`recent_and_active_requests_and_kill` 的括号写明终止的是不经引擎的 `CONNECT`；#102 的「Dashboard 登记在阶段 6」改成需求文档第 2.2 节「产品形态」的 Web Dashboard 一行、「M4b」一节没有那条「没有覆盖的」的话删掉（验收文档与「需求文字的问题」表两处）；#83 的「真机上」把 `M4/P17` 拿掉、`M4/W9…` 加括号说明只证明同一个请求的 `id` 相同；#87 的代码格补兼容性清单 10.4 节请求记录那一行、四种真机项各加括号写走到的那一部分；#84 的括号「两个键」改成「三个键」。④结论的新分布（全文 148 行）：`做了` 123、`做了，补了用例` 4、`没有做完（已登记）` 2、`需求文字有问题` 4、`不属于阶段 4` 2、`有意不同（已登记）` 2、待读 11（任务 5）；本任务 26 行仍是 `做了` 22、`做了，补了用例` 1、`不属于阶段 4` 2、`需求文字有问题` 1——订正第 33 行的「补的用例」那一处：它把 #122 标成 `做了，补了用例` 没错，但改坏验证的范围写成了只跑那一条，照第 27 行裁定③改成全量；#85 的结论照旧 `做了`（第 33 行原来就写的 `做了`，不改）。⑤门禁（一台机器上的一次运行，不是 CI）：fmt、clippy 退出 0；全量 `task-4-fix1-gate.log` 94 个结果行、5468 过（5466 + 2）、0 失败、33 忽略，两条新用例都在里面、都过，没有崩溃。 | 本提交 |
| 36 | 4 | 修正第 2 轮（评审意见八件，只改验收文档与计划，不改代码、不跑门禁）。①#94 的 `a_request_killed_through_the_engine_closes_its_connection` 括号去掉「同样的结束」，照它断言的写。②#87 的 `a_borrowed_connection_records_where_it_went` 后面写明它改过（`f9fc0ba` 加了 `firstByteMs` 一句，`remoteAddr` 那一句没有改，CI 说法照旧），照第 213、214 行的先例。③#122 格子末尾的 CI 说法：补上第一遍漏写的「CI 三平台通过」（这一格除两条补的与两条 `#[ignore]` 外都在 `20efc16` 上、之后没有改过），并把「本轮」改成提交号（「本轮」是协调者第 1 轮派工单里的说法，验收文档是留下来的文字）。④#123 同③。⑤第 143 行的 `rules_and_pairs_act_in_sequence` 补上「CI 三平台通过」。⑥第 208、209、210 行的 `rurge_http_counts_start_at_zero` 各写「这一条改过」（`6c4bded` 在正文与原始字节两处整段断言里加了 `quic` 四个数，各自引的那一段没有改）；这三处不在任务 4 那一片，是协调者这一次扫全文找到的，前几次评审漏看了，一并改。⑦第 214 行的「后两条 `c2b9ae1` 补」改成点名（`a_request_body_that_fails_while_buffered_closes` 与 `a_response_that_fails_while_buffered_answers_502`；另两条在 `20efc16` 上）；不在任务 4 那一片。⑧第 70、100、141、245 行四处第 32 行裁定④之前写下的省略照断言改写（`a_sniffed_http_connection_has_a_record_per_request`、`an_untrusting_client_is_warned_once_an_hour_and_counted_each_time`、`unsupported_and_broken_encodings`、`force_http_engine_hosts_examples`）；不在任务 4 那一片。结论的分布不变（`做了` 123、`做了，补了用例` 4、`没有做完（已登记）` 2、`需求文字有问题` 4、`不属于阶段 4` 2、`有意不同（已登记）` 2、待读 11）。判据：`changed_cited.py ... 20efc16` 的 `NOT NOTED 0`；`ci_wording.py` 的 `LINES 1`（只剩第 157 行）；`shorthand_scan.py` 的 `HITS 0`；`check_rows.py HEAD` 得 `MISS 0`、`AMBIG 0`、待读 `任务 5 11`；三个 `pipes.py` 都空；`CURLY 0`、`CTRL 0`。 | 本提交 |
| 37 | 4 | 协调者核过任务 4：第一遍（`3d4702b`、`6115ce1`，第 33、34 行）、修正第 1 轮（`c24a278`、`0f06ee9`，第 35 行）与修正第 2 轮（`d253a2f`，第 36 行）。读了 26 行的结论、代码、用例与「真机上」四格；用例格引的每一个用例名在 `HEAD` 上都核过，不在 `20efc16` 上的三条正好是这几轮补的（`3d4702b` 一条、`c24a278` 两条）；被引的用例逐条读过断言，#93 的九句与 #94 引的十一条逐条对过原文。第一遍退回九件：改坏验证红的那一次只跑了补的那一条（`--test api` 加用例名），而 `outcome_name` 还被 HAR 与 `--test cli` 用，补的那条用例钉住的三段（`upgraded`、`budget`、拒绝那一条的 `remoteAddr` 是 `null`）里第三段没有改坏；#85 没有一条走经引擎的请求的用例；#122 第五句 `the decoded body is over {上限} bytes` 没有经 API 的用例；#93 的分组错了（「sniffing timed out」那一组多列六条、「no certificate」那一组多列一条）；#94 只引了一条不经引擎的 `CONNECT` 隧道与 HTTP/2 的那一条；#102 写"「M4a」与「M4b」两节的「没有覆盖的」都写着"，「M4b」那一节没有这一句；#83 的「真机上」多了 `M4/P17`、`M4/W9` 一组没有括号；#87 的代码格少了兼容性清单 10.4 节那一行、四种真机项没有括号；#84 的括号把三个键写成两个。修正第 1 轮：六次改坏红的那一次都跑全量 `cargo test -j 4 --workspace --no-fail-fast`，改坏 1、2、3、5、6 各只有补的那一条红，改坏 4（`ConnUpstreams::open` 的进程槽换成 `ProcessSlot::unknown()`）补的那一条与既有的 `run::processes::a_process_rule_matches_the_test_process` 两条红，所以 #85 照第 9 行记 `做了`；改坏 5 红在 `api.rs` 第 4184 行（条目）、改坏 6 红在第 4198 行（HAR），两段各自钉住；改坏 2、4、6 各有一个测试二进制以 `STATUS_HEAP_CORRUPTION` 退出，同一个改坏下单独重跑都过；补的两条读过、各循环 20 次全过；门禁重数过（一台机器上的一次运行，不是 CI）：fmt、clippy 退出 0，94 个结果行、5468 过（5466 + 2）、0 失败、33 忽略。修正第 2 轮只改文字、没有跑门禁：#94 还有一处"同样的结束"；#87 引的 `a_borrowed_connection_records_where_it_went` 在 `f9fc0ba` 改过、格子没有写；#122、#123 格子末尾漏了 `20efc16` 上用例的"CI 三平台通过"（第一遍就漏了，协调者第 1 轮也没有看出来），#122 的"本轮"是协调者第 1 轮派工单的说法；另有协调者扫全文找到的、不在任务 4 那一片的五件——第 143 行没有 CI 说法，第 208、209、210 行引的 `rurge_http_counts_start_at_zero` 在 M5a（`6c4bded`）改过而没有写（第 213、214 行写了），第 214 行"后两条 `c2b9ae1` 补"实为四条里的第二条与第四条，第 70、100、141、245 行四处第 32 行裁定④之前写下的省略；这五件是协调者评审任务 1 ～ 3b 时漏看的。十三处替换与派工单逐字相同，别的行没有动。订正第 36 行：⑤（第 143 行）也不在任务 4 那一片，它在任务 3 那一片，第 36 行与报告都只给⑥ ～ ⑧写了。判据都过（`d253a2f`）：`check_rows.py` 得 `MISS 0`、`AMBIG 0`；`changed_cited.py … 20efc16` 得 `NOT NOTED 0`；`ci_wording.py` 只列第 157 行；`shorthand_scan.py` 得 `HITS 0`；三个文件的 `pipes.py` 都空；工作区的 grep 没有输出；`CURLY 0`、`CTRL 0`；账户名 26；`golden-cmp.py` 得 `rows 25 25`、`bad []`。这一片 `做了` 22、`做了，补了用例` 1（#122）、`不属于阶段 4` 2（#86、#92）、`需求文字有问题` 1（#102，去向项目所有者）；全文 `做了` 123、`做了，补了用例` 4、`没有做完（已登记）` 2、`需求文字有问题` 4、`不属于阶段 4` 2、`有意不同（已登记）` 2、`待读` 11（任务 5）。交给后面任务的：任务 6a——兼容性清单 10.4 节多了抓包过滤器一行（阶段栏 `待定`，不在 68 行里），它之后的行号都往后挪一行；任务 6b——#94 不加手工项（端点到 `Engine::kill` 与引擎里的终止两段合起来是这一句的全部），#83 不引 `M4/P17`；任务 7——`需求文字有问题` 现在 4 行（加了 #102）。**协调者的裁定**（以后的任务照做）：①改坏验证红的那一次：用到那段代码的用例分在几个 crate 或几个测试二进制里时，跑全量 `cargo test -j 4 --workspace --no-fail-fast`，不加过滤词（`outcome_name` 的先例）。②一句话把范围限定在一类会话上的（例如"之经引擎的请求"），用例格引的用例要走那一类会话；走别的会话的只作旁证，括号写明它走的是哪一类；那一类一条用例也没有的当场补（#85、#94）。③格子里写"某份文档的某一节写着……"的，逐处打开那一节核过原文才写（#102）。④用例格的 CI 说法管到它引的每一条用例：`20efc16` 上的写"CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901）"，补的写"CI 没有见过"，带 `#[ignore]` 的写"CI 编译、不运行"；`20efc16` 之后改过的，照第 20 行的裁定在它后面写"这一条改过：……"，这一行靠的那一段没有改的写"CI 说法照旧"，靠新断言的写"CI 没有见过"。工作区的 `tools/changed_cited.py`（逐条比引的用例在 `HEAD` 与 `20efc16` 上的函数体）最后一行要是 `NOT NOTED 0`；`tools/ci_wording.py` 列出的只能是引的用例全是补的或全带 `#[ignore]` 的行。⑤"后两条 `<提交>` 补"一类按位置的说法，先用 `git log -S` 逐条查过，能写名字就写名字（第 214 行）。⑥验收文档不写"本轮""这一轮"，写提交号；`tools/shorthand_scan.py` 最后一行要是 `HITS 0`（也查第 32 行裁定④的省略） | 本提交 |
| 38 | 4 | 修正第 3 轮（评审意见两件，只改验收文档与计划，不改代码、不跑门禁）。两处都是第 32 行裁定④一类的省略（括号往前指、自己没有说断言了什么），`shorthand_scan.py` 认不出（「同一个」「同一条」在别的格子里多是在说内容），都不在任务 4 那一片，是协调者扫全文找到的。①第 117 行的 `a_url_rewrite_reject_answers_like_reject`：括号「同一个答复」改成照它断言的写（页开着答 403、页里有 `The request was rejected by a URL Rewrite rule.`，页关着读不到字节；两次都不拨号，结局是 `Rejected(Reject)`，`url` 计 1）。②第 214 行的 `a_decompression_bomb_stops_at_the_limit`：括号「同一个格式，用例自己的上限 `1000 B`」改成「`the decoded body is over 1000 B`，上限是用例自己的」（1 MiB 零字节压成 gzip、上限 1000 字节，`decode` 交回那一句）。结论的分布不变。判据：`changed_cited.py ... 20efc16` 得 `NOT NOTED 0`；`ci_wording.py` 得 `LINES 1`；`shorthand_scan.py` 得 `HITS 0`；`grep -c "同一个答复\|同一个格式"` 为 0；`check_rows.py HEAD` 得 `MISS 0`、`AMBIG 0`、待读 `任务 5 11`；三个 `pipes.py` 都空；`CURLY 0`、`CTRL 0`。 | 本提交 |
| 39 | 4 | 协调者核过修正第 3 轮（`93ddc7d`，第 38 行）：两处替换与派工单逐字相同，别的行没有动，`git diff --stat a3a2ca7 93ddc7d` 只有验收文档与计划两个文件；两处新括号与用例的断言逐处对过——第 117 行对 `crates/rurge-http/src/engine.rs` 的 `a_url_rewrite_reject_answers_like_reject`（页开着答 403、页里有那一句，页关着读到空串，两次 `dials()` 都是 0，结局都是 `Rejected(Reject)`，`counted(1, 0, 0, 0)` 的第一个数是 `url`），第 214 行对 `crates/rurge-http/src/rewrite/body.rs` 的 `a_decompression_bomb_stops_at_the_limit`（`decode` 交回 `the decoded body is over 1000 B`，`1000 B` 是用例自己给的上限）。判据都过（`93ddc7d`）：`changed_cited.py … 20efc16` 得 `NOT NOTED 0`；`ci_wording.py` 只列第 157 行；`shorthand_scan.py` 得 `HITS 0`；`grep -c "同一个答复\|同一个格式"` 为 0；`check_rows.py` 得 `MISS 0`、`AMBIG 0`，结论的分布与第 37 行写的一样；三个文件的 `pipes.py` 都空；工作区的 grep 没有输出；`CURLY 0`、`CTRL 0`；账户名 26；`golden-cmp.py` 得 `rows 25 25`、`bad []`；提交说明没有控制字符与弯引号。任务 4 至此评审完。**订正第 30、31、32 行的编号**：这三行有几处把验收文档的行号写成了 `#`。第 32 行的"`需求文字有问题` 3（#143、#158、#160，去向项目所有者）"说的是第 143、158、160 行，计划编号是 #65、#80、#82。第 30 行③、第 31 行①与第 32 行里的"#136""#137"（第 32 行是"#136、#137 引上它们""#137 那条解压炸弹用例"与裁定④的"（#121、#137）"）说的是第 136、137 行，计划编号是 #58、#59；这几处里的 #121 是计划编号，不改。第 30、31 行的写法照的是协调者给任务 3b 修正第 2、3 轮的派工单，错在协调者。第 28 行②的"#10"是黄金用例表（`lists/golden.md`）的第 10 行（`header-replace DNT 1`），不是计划第 10 号。协调者把这张表里的每一个 `#N` 拿切法表的那一号与验收文档同一行号的需求对过上下文，别处的都是计划编号。以后这张表里 `#N` 只指切法表的编号，验收文档的行写"第 N 行"，黄金用例表的行写"黄金用例表第 N 行"。 | 本提交 |
| 40 | 5 | 读了 #20、#24、#33、#34、#124、#129、#130、#143、#146、#147、#148 十一行：结论 `做了` 9（#20 照 #34，#24、#33、#34、#124、#129、#130、#146、#147）、`没有做完（已登记）` 2（#143 NFR-01、#148 NFR-11）；#143、#148 各加进「没有做完的」一行（#143 去向阶段 4 / M5 设计第 15 节第 6 条与 M5b 延后第 1 行，#148 去向总设计 Q4），没有新增「有意不同」「需求文字有问题」「本机没有能红的用例」的行。手册的原句：#24、#34 用 `manual-1103/http_mitm.txt` 第 35、59 行，#24 另有总设计第 1.2 节表里那一行的引文；#33、#129、#130 用需求文档第 4 节 FR-HTTP-04 与总设计第 1.5 节、`manual-1103/rules_protocol-and-network.txt` 第 22、32 行；四条非功能需求照需求文档第 5 节，没有要手册的。真实输入在 `inputs/task-5/`：`[MITM]` 的 `auto-quic-block` 与全局、策略上的 `block-quic` 各种值一起过 `rurge check`（两份片段各一条 `W0039`，开关与 `always-allow` 都读进来了）；`rurge rule match --protocol QUIC --udp --sni <名字>` 在目标是地址时命中带 `extended-matching` 的 `DOMAIN` 行、去掉它就不命中。改坏验证两条，各存了红与绿的原始输出：`task-5-mut-1.log`（`auto_quic_blocks` 交出的 `sni` 改成 `None`，全量里只有补的 `a_listed_hello_is_blocked_at_an_address_it_names_it_to` 红）与 `task-5-mut-2.log`（`swap_runtime` 里清 `quic` 的计数器，全量里只有补的 `the_quic_counts_survive_a_reload` 红）。CI 说法照 M6-D28 与任务 1 ～ 4 的裁定：M5 的用例写「CI 没有见过」，`20efc16` 上就有而之后没有改过的写「CI 三平台通过（Linux、Windows：运行 41402787066；macOS：运行 41411445901）」，`rurge_http_counts_start_at_zero` 写「这一条改过：……`6c4bded`」。`tools/check_rows.py HEAD --file` 为 `MISS 0, AMBIG 0`、`tools/changed_cited.py` 为 `NOT NOTED 0`、`tools/ci_wording.py` 列出 4 行（第 157 行属任务 3b；第 217、236 行引的全是 M5 的用例；第 262 行引的是十六个 `#[ignore]` 的基准）、`tools/shorthand_scan.py` 为 `HITS 0`。 | 本提交 |
| 41 | 5 | 补了两条用例（`test(engine)`，没有改行为）：`crates/rurge-engine/tests/mitm.rs` 的 `a_listed_hello_is_blocked_at_an_address_it_names_it_to`（已有的 `auto-quic-block` 用例里 ClientHello 的名字与目标是同一个，名单按 SNI 比与按目标比分不开；这一条把目标写成 `127.0.0.1:443`）、`crates/rurge-engine/tests/udp.rs` 的 `the_quic_counts_survive_a_reload`（「M5a」一节写着 `quic` 的四个数重载不清零，没有用例钉住）。两条都是等一件事发生的用例，各单独循环跑 20 次全过；全量门禁 94 个结果行、5470 过、0 失败、33 忽略（一台机器上的一次运行，不是 CI）。 | `84f5843` |
| 42 | 5 | 兼容性清单 2.1 节 `block-quic` 与 7.2 节 `auto-quic-block` 两行的备注末尾各加一段「**阶段 4 / M6 核对**：……」（`policy/udp` 页的「QUIC detection and blocking apply only to Enhanced Mode traffic, not to UDP ASSOCIATE associations.」；rurge 对 SOCKS5 的 UDP 关联来的流同样识别、同样按两个键阻断，用例 `block_quic_drops_quic_and_nothing_else` 与 `auto_quic_block_blocks_quic_to_a_listed_host`）；状态标记、阶段栏与附录的五个数都没动。7.2 节那一行原有的一个弯引号对改成直引号，使改过的行没有弯引号。执行者的裁定：`block-quic` 的登记加在 2.1 节的全局键那一行，4.3 节的策略参数那一行已经写着 M5a 起经 SOCKS5 的 UDP 生效，不另加。 | `7618e46`、本提交 |
| 43 | 5 | 过时的注释：`crates/rurge-config/src/mitm.rs` 里 `auto_quic_block` 的注释从 「read only until M5」 改成照 M5 之后的代码说它做什么，单独一个 `docs(config):` 提交，只跑 `cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings`。执行者的裁定：#34 的「真机上」不列 `M5/W14`、`M5/M14`、`M5/L14` 的 (a) 那一半（它只走到开关关掉、QUIC 照通，不是这一句说的阻断）；#24 的真机项写出只走到不带端口的项在 443 上命中这一部分，#129 的写出跨没跨包在真浏览器上分不出来。 | `41b1066` |
| 44 | 5 | 修正第 1 轮，十五件：① `crates/rurge-config/src/mitm.rs` 里 `auto_quic_block` 的注释折成两行、字没有改（`0b16d63`）。② `a_listed_hello_is_blocked_at_an_address_it_names_it_to` 改名成 `a_flow_to_an_address_is_blocked_by_its_sni`（只改 `fn` 的名字），验收文档第 97、107 行与兼容性清单第 584 行三处旧名字一起改；计划第 40、41 行里的旧名字照「记录」的规矩不改，由这一行订正。③ `the_quic_counts_survive_a_reload` 的文档注释改成英文、指到 `phase 4 M5 design 5.4`。④ #23 与 #24 各补一条用例（`a_bare_entry_decrypts_on_a_widened_port`、`a_bare_entry_blocks_quic_on_a_widened_port`，`88d8149`）：名单里另一项写下的端口把嗅探放宽，不带端口的项在那里也按 SNI 命中；两条单独跑一次约 0.3 秒（`finished in 0.31s` 与 `0.32s`），各单独循环跑 20 次全过（`task-5-fix1-loop.log`）。⑤ 改坏 3（`decrypting` 的 `matches_sni` 换成 `matches`，`task-5-mut-3.log`）：全量里只有补的这两条红，没有现有的用例红——所以第 96 行（#23）也改记 `做了，补了用例`；第 97 行（#24）与第 217 行（#124）也改记 `做了，补了用例`（改坏 1 与改坏 2 各只让它们补的那一条红，订正第 40 行写的三处）。⑥ 第 97 行的「真机上」拿掉 W14 的三项（W14 的 (a) 设了 `auto-quic-block = false`，名单根本不问；(b) 说的是规则）。⑦ 第 106 行的「真机上」写成 W14 两半都读出了 sni。⑧ 第 107 行一处括号（`assert_blocked` 不断言 `rule`，改「策略链不空」）、一个数（mitm.rs 里是 M5a 的十二条加 `84f5843` 的一条）与改名。⑨ 第 217 行两处括号（`rurge_http_counts_start_at_zero` 只有原始字节那一处逐字节、键的次序只由它钉住；九种读不成的原因：八种是 `initial.rs` 的常数、第九种写在 `udp.rs` 的 `run_flow`，原文只有两种有用例断言，其余七种不补）。⑩ 第 262 行的 `bench.rs` 是 M5b 加的、`a262d4c` 之后，CI 没有见过，不是「CI 编译、不运行」。⑪ 第 266 行三处：`check` 作业不止三条命令、到 `a262d4c` 为止的代码与那时的用例、最近一次运行 41411445901 没有全绿且失败的都在测试一侧。⑫ 第 267 行的 `sha2` 是 `crates/rurge-http` 用的 0.10.9。⑬ 兼容性清单第 584 行的改名。⑭ `task-5-mut-1.log`、`task-5-mut-2.log` 各加「结果」一节（`task-5-mut-2.log` 另加「订正」：那一次实际动了两处——`QuicCounts::reset` 与调用它）。⑮ 这一节与报告。门禁（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI）：fmt 与 clippy 退出码 0；全量 93 个结果行、5441 通过、0 失败、33 忽略，`rurge-proto-ssh` 的单元测试二进制以 `STATUS_HEAP_CORRUPTION` 退出，单独重跑 31 通过（`task-5-fix1-rerun.log`），合计 **94 / 5472 / 0 / 33**（基线 5470 加两条）。`check_rows.py` 的结论：`做了 129, 做了，补了用例 7, 没有做完（已登记） 4, 需求文字有问题 4, 不属于阶段 4 2, 有意不同（已登记） 2`。 | `0b16d63`、`88d8149`、本提交 |
| 45 | 5 | 修正第 2 轮，四件，只改文档与工作区的记录，没有改代码、没有跑门禁。① 第 97 行（#24）格末那一句把一条用例算成了两条：`84f5843` 补的那一条与 `88d8149` 改名的那一条是同一条（`a_flow_to_an_address_is_blocked_by_its_sni`），这一格引的新用例是五条——M5a 的三条、`84f5843` 补（`88d8149` 改名）的一条与 `88d8149` 补的一条（上一轮那句「补的三条」写得含糊，不是执行者数错）。② 报告的「# 修正第 1 轮」三处与事实不符，在末尾新加「# 修正第 2 轮」一节照实写明：QUIC 那一条实际在`an_incomplete_hello_is_not_blocked` 之后、`a_port_entry_blocks_only_its_port` 之前（派工单写的是`a_bare_entry_does_not_block_quic_on_8443` 之后，协调者接受这个位置）；`task-5-mut-3-green.log` 不存在，改坏 3 还原之后的绿是 `task-5-fix1-gate.log`；第 44 行写了 #23 补的用例与结论，没有写它是任务 2 那一片、也没有写它的代码格加了 `crates/rurge-engine/src/serve.rs::decrypting`——第 44 行不改，由协调者在评审行里补。③ 两份改坏日志各在末尾加「## 修正第 2 轮的订正」：`task-5-mut-2.log` 照会话记录订正「两处都用编辑工具改了回去」一句（施加与还原里各有一步是 `python -c` 脚本在原文件上改，改空行那一步才是编辑工具；跑绿之前`git status --short` 与 `git diff --stat` 只列着两个用例文件）；`task-5-mut-3.log` 订正开头写的`task-5-mut-3-green.log`（不存在），绿的那一次是 `task-5-fix1-gate.log`。④ 这一节与报告的判据。结论的分布不变（`做了 129, 做了，补了用例 7, 没有做完（已登记） 4, 需求文字有问题 4, 不属于阶段 4 2, 有意不同（已登记） 2`）。判据：`check_rows.py HEAD` 为 `MISS 0`、`AMBIG 0`、没有「待读」；`changed_cited.py … 20efc16` 为 `NOT NOTED 0`；`ci_wording.py` 为 `LINES 4`（157、217、236、262）；`shorthand_scan.py` 为 `HITS 0`；`grep -c "里改名的一条与补的一条"` 为 0；三个 `pipes.py` 的输出都空；`curly.py 9da3059` 为 `CURLY 0`、`ctrl.py 9da3059` 为 `CTRL 0`。 | 本提交 |
| 46 | 5 | 协调者评审任务 5（第 40 ～ 43 行，`41b1066`、`84f5843`、`7618e46`、`b24f21c`）、修正第 1 轮（第 44 行，`0b16d63`、`88d8149`、`9da3059`）与修正第 2 轮（第 45 行，`781449b`）。核过的：门禁是修正第 1 轮之后的全量一次，94 个结果行、5472 通过、0 失败、33 忽略（`rurge-proto-ssh` 的单元测试二进制以 `STATUS_HEAP_CORRUPTION` 退出，单独重跑 31 通过；一台机器上的一次运行，不是 CI）。三次改坏各只让补的用例红，协调者读过六份日志的结果行与 `failures:` 一段：改坏 1（`auto_quic_blocks` 交给 `decrypting` 的 `sni` 换成 `None`）只红 `a_flow_to_an_address_is_blocked_by_its_sni`（当时叫 `a_listed_hello_is_blocked_at_an_address_it_names_it_to`）；改坏 2（重载时把 `quic` 的四个数清零）只红 `the_quic_counts_survive_a_reload`；改坏 3（`decrypting` 有 SNI 时 `matches_sni` 换成 `matches`）只红 `88d8149` 补的两条，所以 #23 与 #24 在补之前没有引擎一层能红的用例。`84f5843` 与 `88d8149` 补的四条各单独循环跑过 20 次，全过。修正第 1 轮在兼容性清单里只改了第 584 行的用例名（`9da3059`），行尾仍是 CRLF；修正第 2 轮在验收文档里只换了第 97 行那一句，与派工单逐字相同。判据都过（`781449b`）：`check_rows.py` 得 `MISS 0`、`AMBIG 0`、没有「待读」；`changed_cited.py … 20efc16` 得 `NOT NOTED 0`；`ci_wording.py` 只列第 157、217、236、262 行；`shorthand_scan.py` 得 `HITS 0`；三个文件的 `pipes.py` 都空；工作区的 grep 没有输出；`curly.py 9da3059` 与 `ctrl.py 9da3059` 都是 0；账户名 26；`golden-cmp.py` 得 `rows 25 25`、`bad []`；`781449b` 的提交说明没有控制字符与弯引号。裁定：① 改了的行，行里原有的弯引号换成直引号（只换引号，别的字不动），修正记录那一行写明换了哪几处；这是第 42 行的做法，任务 6a、6b、7 照做。② 兼容性清单 4.3 节策略参数 `block-quic` 一行（第 375 行）写着 M5a 起经 SOCKS5 进来的 UDP 生效，与 2.1 节 `block-quic` 一行（第 177 行）末尾「阶段 4 / M6 核对」那一段登记的是同一处不同：任务 6a 在第 375 行加一句指到那一段，这一条覆盖第 42 行的「不另加」；第 375 行原有两个弯引号，照①换。③ 接受 `a_bare_entry_blocks_quic_on_a_widened_port` 放在 `an_incomplete_hello_is_not_blocked` 之后（派工单写的是 `a_bare_entry_does_not_block_quic_on_8443` 之后），也接受验收文档第 106 行「真机上」一格括号里的 sni 不带反引号。④ `88d8149` 的提交说明说 `the_quic_counts_survive_a_reload` 的注释"与两个用例文件里别的 M5a 用例的写法一致"：与 `crates/rurge-engine/tests/mitm.rs` 的一致，`crates/rurge-engine/tests/udp.rs` 里 M5a 的用例写的是 `(design 5.1)` 这样的；注释照派工单写，接受，提交说明不改。⑤ 补第 44 行漏的一句：#23 是任务 2 的那一片（切法表的任务一栏是 2），修正第 1 轮除了改它的用例格与结论，还在它的代码格开头加了 `crates/rurge-engine/src/serve.rs::decrypting`。⑥ 验收文档第 97 行把一条用例数成两条，起因是协调者给修正第 1 轮的派工单里"补的三条"一句写得含糊（第 45 行①）。⑦ 第 45 行③与报告的「# 修正第 2 轮」说"改空行那一步才是编辑工具"，说的是还原；施加的第一步（加 `self.quic_counts = Default::default();` 那一行）也是编辑工具，`task-5-mut-2.log` 的订正写全了。⑧ 任务 5 报告第 12 节的四条顾虑都接受：改坏 1 红在等记录结束超时、不在一条断言上，不改写法；"没有什么"的断言与没有原文断言的日志行不补（第 32 行③与 P16 的口径）；#33 引的 `the_rfc_9369_client_initial_is_one` 与 `what_the_builder_makes_is_one` 在 `20efc16` 上就有，照设计第 4.4 节写"CI 三平台通过"，说的是在 `a262d4c` 的代码上通过过，前一条用的 RFC 9369 样例由 `b2bcb4a` 挪进 `quic::testing`，协调者逐字节比过、相同；#146 只扫阶段 4 新增的行。⑨ 手工清单不为 M5/W14 的 (a) 单列一项：报告第 13 节说没有一句的「真机上」列它，不对，#33（验收文档第 106 行）与 #129（第 236 行）都列了 M5/W14、M5/M14、M5/L14，第 106 行写明了 (a)。任务 5 至此评审完，切法表的 148 行都读过：`做了` 129、`做了，补了用例` 7（#23、#24、#45、#79、#121、#122、#124）、`没有做完（已登记）` 4（#57、#63、#143、#148）、`需求文字有问题` 4（#65、#80、#82、#102）、`不属于阶段 4` 2（#86、#92）、`有意不同（已登记）` 2（#128、#145）。之后是任务 6a、6b 与 7。 | 本提交 |
| 47 | 6a | 核对兼容性清单：阶段栏含 4 的 68 行（`lists/matrix-rows.md` 的行号改成 `f91ed5a` 上的两个 10.4 节行 903、906）、10.4 节抓包过滤器一行、阶段栏不含 4 的 2.1 节空闲超时 / 4.1 节 `REJECT-DROP` / 4.3 节 `block-quic` 三行，逐行对过代码与验收文档；**状态标记一个也没有改**，也没有「没有做完」的缺口要向验收文档加行。改了文字的 11 行，各在格末加一段照现在代码写的「阶段 4 / M6 核对」（前一段不删）：2.1 节 `force-http-engine-hosts`（`MITM 随 M2` 已不成立）、`always-raw-tcp-hosts`（`今天` 那一句仍在）、4.1 节 `REJECT-DROP`（M1b 起请求正文读完后客户端先关会提前结束停留、没读完时停到 `drop_hold`）、2.1 节空闲超时（嗅探到的连接上读请求头的时限是 `idle_timeout`、HTTP 监听器的是 `handshake_timeout`）、7.1 节「80 / 443 端口自动协议嗅探…」（`今天` 那一句仍在）、「`force-http-engine-hosts` 强制明文连接进入引擎…」与「HTTPS 默认不透明…」（判为 TLS 的自 M2a 起先由 `[MITM]` 判；`h2` 为真时只给 `h2` 的也解密）、「引擎处理的请求在抓包视图中逐条展示」（`随 M3c` 仍是待办）、「明文 HTTP 代理转发」（`解密随 M2` 已不成立）、7.2 节「CA 生成」（`0600` 那一支与用例在 Linux 与 macOS 上编译，CI 运行 41402787066 与 41411445901 通过）、7.5 节 jq 形式与「改写前按需要先解压」（`br`、`zstd` 随 M3c，与验收文档的「没有做完」说同一件事）；另在 7.2 节 `client-source-address` 补上阶段 7 的出处（M2 设计第 4 节、M2-D3）、4.3 节 `block-quic` 加一句指到 2.1 节 `block-quic` 一行末尾那一段。P15 的十行：54、582、584 由同一格里后写的段落订正，其余七行按上面各加了一段。**执行者的裁定**：3.2、3.3、3.5 节另有 13 行少了末尾的空备注一格（`pipes.py` 报 6 与 5 条竖线对 7 与 6 条），补上空的一格，显示不变；这是 `pipes.py` 的判据要的空输出，不加行、不改标记。换了弯引号的行：本文开头（4 对）、4.3 节 `block-quic`（1 对）、附录那一段（1 对）。附录从 577 行（✅ 388 / 🟡 138 / 🔁 32 / ⛔ 14 / ❓ 5）变成 579 行（✅ 388 / 🟡 139 / 🔁 32 / ⛔ 14 / ❓ 6），第 7 节 46 行变成 47 行、第 10 节 58 行变成 59 行；开头「最近一次」那一句照惯例改写，并在「状态标记说的是兼容程度」一句之前补上 M5 做完、验收文档 148 行都读过、手工清单六十八项三件事 | `eec37b8` |
| 48 | 6a | 验收文档第 235 行（#128）的代码格与「有意不同的」表（第 295 行）把 HTTP 一侧 `REJECT-DROP` 的登记指到「M1 细化设计第 20 节第 8 条」。执行者打开三处核过（第 20 节第 8 条说的是 `hold_dropped` 与会话根令牌竞速——计划 P10；「请求正文读完时客户端先关会结束停留、没读完时停到时限」写在第 17 节第 8 条，第 8.8 节的表里 `REJECT-DROP` 一行也这样写）：两处改成指到第 17 节第 8 条与第 8.8 节，结论与别的格不动 | 本提交 |
| 49 | 6a | 修正第 1 轮（协调者的评审意见十件，只改文档、不改代码、不跑门禁）：① 照协调者的裁定还原第 264、267 ～ 270、279、281 ～ 285、305、313 行——`pipes.py` 在 `f91ed5a` 上本来就列出这 13 行，第 47 行的「执行者的裁定」作废，补的格去掉了。② 第 172 行把「这份名单本身不决定解不解密，它只让命中的非 80、443 端口也被嗅探」收窄到判为 TLS 的会话，补上「判为别的或关闭的非 TLS 连接命中时进引擎」。③ 第 180 行把 `head_wait` 的来源改成 `crates/rurge-engine/src/serve.rs::serve_requests`，两处「设计第 8.10 节」改成「M1 细化设计第 8.10 节」。④ 第 563 行照代码改写：可嗅探的条件在 M2a 多了一条（`crates/rurge-engine/src/serve.rs::widened_by_mitm`、M2 设计第 7.6 节 / M2-D9、7.2 节 `hostname` 一行），15 秒的等待也落到这些端口上。⑤ 第 565 行的段名改成「阶段 4 / M6 核对」，段里写「M2b 起」。⑥ 第 569 行去掉「今天」，出处说准：抓包的 `?decode` 与 HAR 是 FR-HTTP-11「…与正文保存」一句（`做了`，用例 `what_cannot_be_decoded_says_why`），Body Rewrite 的 `br`、`zstd` 是 FR-HTTP-07「解压…」一句（`没有做完（已登记）`）。⑦ 第 570 行指到 1.2 节 `[MITM]` 一行（7.2 节的九个键见那里）。⑧ 第 5 行与第 936 行把「前面两个任务」改成「M6 在 7.2 节与 10.4 节各加了一行」、去掉「这一轮」、并把「M6 改了文字的行有」补全（加 2.1 节 `block-quic` 与 7.2 节 `auto-quic-block`，与附录的表三行）。⑨ 订正第 47 行的数：这一任务改了文字的是 14 行（172、173、180、327、375、563、564、565、569、570、583、585、617、621），其中 11 行在 68 行里、另 3 行是阶段栏不含 4 的那三行；另有本文开头、附录那一段与附录表的三行（第 47 行写「11 行」没有说清是哪一个数；连同第 5 行里补进去的 177、584 两行是 16 行正文行）。⑩ `lists/matrix-rows.md` 的「6a」一栏照这一轮更新，第 283 行改回「不改」 | 本提交 |
| 50 | 6a | 修正第 2 轮（两件，只改文档）：① 第 569 行的「阶段 4 / M6 核对」不再说「上面"随 M3c"说的两处」——这一格前面的「随 M3c」只说了读一条（`?decode`）与 HAR 不解 `br`、`zstd`，`[Body Rewrite]` 不在这一格里；改写成一个在前、一个在后的两句：那一处在验收文档里是 FR-HTTP-11「…与正文保存」一句（`做了`，用例 `what_cannot_be_decoded_says_why`），同样等 M3c 的 `[Body Rewrite]` 不解那两种是 FR-HTTP-07「解压…」之 `br`、`zstd` 一句（`没有做完（已登记）`）。② 报告里 68 行的状态标记数错了：照 `stage4_rows.py --list` 的第五栏是 ✅ 51、🟡 16、🔁 1（合计 68），报告写成 🟡 15；订正在报告末尾新加的「修正第 2 轮」一节，前面的字不改。`lists/matrix-rows.md` 第 569 行的「6a」一栏补上这一轮的提交 | 本提交 |
| 51 | 6a | 协调者评审任务 6a（第 47、48 行，`eec37b8`、`13cc32f`、`282da80`）、修正第 1 轮（第 49 行，`58be197`）与修正第 2 轮（第 50 行，`24631bc`）。核过的：兼容性清单在 `f91ed5a` 与 `24631bc` 之间改了字的正好 19 行（第 5、172、173、180、327、375、563、564、565、569、570、583、585、617、621、936、946、949、950 行），没有增删行，状态标记一个也没有变；第 264、267 ～ 270、279、281 ～ 285、305、313 行与 `f91ed5a` 上逐字相同。改了的各行逐条对过代码：第 172 行对 `crates/rurge-engine/src/serve.rs` 的 `sniffable_target`、`serve_sniffed`（判为 TLS 的交给 `decrypting`）与 `forces_http_engine`（只把判为别的或关闭的连接送进引擎）；第 173 行对 `sniffable_target` 里 `raw_listed` 那一支与 `auto_quic_blocks`（要 `sniffable_target` 有结果、`decrypting` 答应）；第 180 行对 `serve_requests` 的 `head_wait: runtime.idle_timeout`、`crates/rurge-http/src/engine.rs::serve_h1` 的 `header_read_timeout(head_wait)`、`crates/rurge-engine/src/engine.rs` 里 HTTP 监听器那一处的 `head_wait: HANDSHAKE_TIMEOUT`（30 秒）与 `crates/rurge-inbound/src/http.rs::serve_connection`；第 327 行对 `crates/rurge-http/src/engine.rs::answer` 的 `RejectKind::Drop` 一支（`drop_hold` 与会话的令牌竞速，`DROP_HOLD` 30 秒）与三条用例 `a_dropped_request_is_held_then_closed`、`a_dropped_request_ends_early_when_the_client_closes`、`a_dropped_h2_request_is_held_then_reset`；第 563 行对 `widened_by_mitm`（经 `crates/rurge-http/src/mitm/decrypt.rs::Mitm::widens`）、`crates/rurge-http/src/sniff.rs` 的 `SNIFF_WAIT`（15 秒）与 `crates/rurge-tun/src/session.rs::hold_in_silence`；第 564、565 行对 `Mitm::decides`（名单、来源、证书、ALPN 依次判）与 `server_config`（`h2` 为真时给 `h2` 与 `http/1.1`）；第 583 行对 M2 设计第 4.3 节与 M2-D3（阶段 7 之前 MAC 一类的条目什么也不匹配，`W0029`）；第 585 行对 `crates/rurge/src/cli/mitm.rs`（自 `20efc16` 起没有改过）与那条用例；第 569、617、621 行对验收文档 FR-HTTP-11「…与正文保存」与 FR-HTTP-07「解压…」两句的结论。验收文档第 235、295 行改指的两处协调者打开读过：M1 细化设计第 17 节第 8 条与第 8.8 节的表里 `REJECT-DROP` 一行都写着请求正文读完时客户端先关会结束停留。修正第 2 轮之后协调者重跑的判据：`matrix_diff.py f91ed5a HEAD` 列出的是那 19 行；`added_words.py` 对兼容性清单（任务、这一轮、本轮、今天、报告、派工）得 `HITS 0`；兼容性清单 CRLF 950、LF 950；`curly.py f91ed5a` 与 `ctrl.py f91ed5a` 都是 0；计划的 `pipes.py` 为空；`hash_suspects.py` 对第 47 ～ 50 行都是 0；账户名 26。裁定：① 6a 的派工单要 `pipes.py` 对兼容性清单的输出"为空"，是协调者的错：`f91ed5a` 上它本来就列出那 13 行，执行者为了凑这一条补了格（第 47 行的「执行者的裁定」），修正第 1 轮还原了（第 49 行①）；之后的派工单把这一条写成"与基线上那一份的输出相同"。② 68 行的状态标记是 ✅ 51、🟡 16、🔁 1（第 47 行没有写这个数，报告写的 🟡 15 由第 50 行②订正）。③ 接受第 563 行「15 秒的等待因此也落到这些端口上」与第 936 行的末句（标记一个也没有动，因为标记说的是兼容程度、不是实现进度）。④ 第 49、50 行里的"这一轮"说的是那一行记的那一轮，与第 15 行的写法相同，不改；第 37 行⑥不写这两个词的规矩管的是验收文档，6a 的派工单把它也用在了兼容性清单上。⑤ 交给任务 7 的：附录的 579 行（✅ 388 / 🟡 139 / 🔁 32 / ⛔ 14 / ❓ 6）、第 7 节 47 行与第 10 节 59 行，别的文档引这几个数时照这一次的写；兼容性清单第 5 行的"六十八项"要跟任务 6b 之后手工清单的项数走。任务 6a 至此评审完，之后是任务 6b 与 7。 | 本提交 |
| 52 | 6b | 核对手工清单的 68 项、五节前置条件与开头两段（`c5aba68` 上 376 行，照阶段 4 / M5 之后的代码）。订正 8 项：`M1/W2`（增强模式下浏览器后台的 HTTP/3 流量另有 `transport` 为 `udp`、`protocol` 为 `quic` 的记录，不经引擎、`requests` 不涨）、`M2/W1`（`generate` 在 Windows 上那句不另设权限的提醒，原文照 `crates/rurge/src/cli/mitm.rs::announce`）、`M2/W2`（浏览器先试 HTTP/3 时另有一条 `error` 为 `QUIC blocked by auto-quic-block` 的 `rejected` 记录，页面是回落到 TCP 之后解密的）、`M2/W3`（`h2 = false` 时只给 `h2` 的客户端那一条记录是 `https`、没有 `url` 与 `method`，备注 `Not decrypted: the client offers no protocol rurge decrypts`）、`M2/P3`（`client-source-address` 拒掉那两次的备注 `Not decrypted: the client is not in client-source-address`，名单没有收的那两条没有备注）、`M3/W6`（期望里去掉 `header_mode_sends_the_request_to_the_new_target`：步骤里没有 `header` 模式的一步）、`M4/W12`（那条 DIRECT 的 QUIC 流要的 HTTP/3 站点不写在 `[MITM]` 的 `hostname` 里，前置条件补了这一句）、M5 前置条件（「其余三个对象照旧」改成四项）。非项的文字就地改的：「怎么用」的引擎计数由三项改五项（`HttpJson` 是五项）、「先做哪一节」补最后做 W12 一组、「引用怎么写」一段、「改系统设置的项与恢复」补全 M3 ～ M5 的项、索引表一律写 `节/编号` 并在第 2 条补 `M5/W13`、`M5/M13`、`M5/L13`、五节标题下各一句订正说明、开头一段末尾记下这一次。新加项 0（清单仍是 68 项，`gen_manual_items.py` 输出 68 行；验收文档「真机上」写着「缺」的 0 行）；总设计第 15 节第 2 条两半都有项：「主流站点解密」每个平台 `M2/W2`、`M2/M2`、`M2/L2`（加 M5 的那三号），「证书固定站点给出提示」只有与平台无关的 `M2/P1`——给提示的路径上没有按平台分的 `cfg`（`serve.rs`、`mitm.rs` 与 `rurge-http/src/mitm/` 下只有 `#[cfg(test)]`）。`manual_names.py` 的 53 段：甲 21（别的程序与系统的名字：`certutil`、`security`、`update-ca-certificates`、`trust`、`openssl s_client`、curl 的 `--cacert` 与 `CURLE_…` / `CURLOPT_…`、`SSL_VERIFY_NONE`、浏览器的程序名与 `--user-data-dir`、badssl.com 的两个站点、`<NUL` 与 `</dev/null`）、乙 11（清单自己取的名字，定义在各节前置条件或那一项里：`X-M3`、`/m3-redirect`、`/m3-file`、`/m3-url`、`m3-big`、`s3cr3t-m3`、`h3ad3r-m3`、`qu3ry-m3`、`<服务器>:8080/echo`、`capture.har`、`X-M3: yes`）、丙 18（rurge 打印或答复的原文：诊断行与 `error: …`、`check` 的末行、`W0039` / `W0040`、两个应答头、不信任与证书固定两句、`mitm.noRequest`、`host:0`、三句改写备注、`rewrite.mapLocal`、`W0042` 那一句与 `/sessions/{name}`）、丁 3（`Vary`、`Accept-Encoding`、`BND.ADDR:BND.PORT`），逐段的交代在报告里；改动带进的 32 段全是 `节/编号`。判据：`check_rows.py` 的 `缺` 0 行、`MISS 0`、`AMBIG 0`；`pipes.py` 对清单没有输出；`curly.py` 是 `CURLY 0`、`ctrl.py` 是 `CTRL 0`；`added_words.py`（`任务`、`这一轮`、`本轮`、`今天`、`报告`、`派工`、`现在还`）是 `HITS 0`；清单的 CRLF 数是 0。 | `8d0eeb9`、本提交 |
| 53 | 6b | 修正第 1 轮（评审意见五件，只改文档）：① 清单第 5 行的末句换成写明「七项与 M4、M5 两节的前置条件」与三处非项改动的那一句（不再写「几项」）。② 第 11 行括号里的「另各有 `P` 开头的项」改成「两节都有 `W1`、`W2`、`M1`、`M2`、`L1`、`L2` 与 `P1` ～ `P3`」（M1 一节是 `P1` ～ `P3`、M2 一节是 `P1` ～ `P6`，重号的是 `P1` ～ `P3`）。③ 第 137 行 `M2/W2` 的括号改成「增强模式那一次浏览器先试 HTTP/3 时」，第 166 行那一句改成「第三次（增强模式）浏览器先试了 HTTP/3 的话」并补上「前两次经代理，浏览器不发 QUIC，没有这一条，见 M5 一节的前置条件」——依据是 M5 前置条件里「浏览器自己的 QUIC 只经虚拟网卡才到 rurge；HTTP 监听器上 rurge 只有 TCP；浏览器不用 SOCKS5 的 UDP 关联」那一条。④ 验收文档第 254 行（FR-RULE-08「协议识别：HTTP」之 CONNECT 隧道或 SOCKS5 连接里的明文 HTTP）的「真机上」换成「M1/W1、M1/M1、M1/L1（只走到 SOCKS5 连接里的那一半：经 HTTP 代理打开明文站点时浏览器发的是绝对形式的请求，不经 CONNECT 隧道；CONNECT 隧道里的明文 HTTP，一项手工项也没有走到）」；`M1/W2`、`M1/M2`、`M1/L2` 从这一格去掉（经虚拟网卡既不是 CONNECT 隧道，也不是 SOCKS5；它们在第 69、168、209、252 行照旧）；`a_plain_http_session_is_marked_http` 调 `connect_via_http`，走的是 CONNECT 隧道；第 255 行照裁定不改（这一句说的是协议识别，规则命中由用例断言）。⑤ 订正第 52 行的数：「订正 8 项」把 M5 一节的前置条件算成了一项；订正的项是七项（`M1/W2`、`M2/W1`、`M2/W2`、`M2/W3`、`M2/P3`、`M3/W6`、`M4/W12`），另改了 M4、M5 两节的前置条件。报告的两处订正写在它的「# 修正第 1 轮」。判据（这一台机器上的运行）：`check_rows.py` 的「缺」0 行、`MISS 0`、`AMBIG 0`；`count_rows.py` 与 `40f5975` 上相同（129 / 7 / 4 / 4 / 2 / 2）；`gen_manual_items.py` 68 行、与 `lists/manual-items.md` 逐字相同；`pipes.py` 与 `paren_check.py` 对清单、验收文档、计划三份都没有输出；`added_words.py` 两条都是 `HITS 0`；`changed_cited.py` 是 `NOT NOTED 0`；`ci_wording.py` 只列第 157、217、236、262 行；`shorthand_scan.py` 是 `HITS 0`；`grep -n -E` 那一条（四个模式：`报告里`、`task-[0-9]`、`inputs/`、`superpowers/sdd`）没有输出；三份的 CR 数是 0；`curly.py` 是 `CURLY 0`、`ctrl.py` 是 `CTRL 0`。 | 本提交 |
| 54 | 6b | 协调者评审任务 6b（第 52 行，`8d0eeb9`、`40f5975`）与修正第 1 轮（第 53 行，`f42803b`）。核过的：清单订正的七项与 M4、M5 两节的前置条件逐条对过代码——`M1/W2` 与 `M2/W2` 的 QUIC 记录对 `crates/rurge-engine/src/udp.rs`（`AUTO_QUIC_BLOCKED` 的原文；`set_error(AUTO_QUIC_BLOCKED)` 那一处只在读出 ClientHello、`auto_quic_blocks` 答应之后，记 `RejectKind::Reject`，所以 `blocked` 都在 `hello` 里）；`M2/W1` 那一句对 `crates/rurge/src/cli/mitm.rs::announce` 的非 Unix 一支；`M2/W3` 与 `M2/P3` 的备注对 `crates/rurge-engine/src/serve.rs` 的 `NO_PROTOCOL` 与 `SOURCE`（名单不收时 `decrypting` 不写备注）；`M3/W6` 的步骤里没有 `header` 模式的一步；`M4/W12` 与 M4 的前置条件对 `auto_quic_blocks`；「看引擎的计数」的五项与 M5 的前置条件对 `crates/rurge-api/src/routes/rurge_http.rs` 的 `HttpJson`。名字表的 53 段都在报告的表里，四类是甲 21、乙 11、丙 18、丁 3；报告有两处说法不对，修正第 1 轮订正了（`W0039` 的常量是 `W_MITM_CA_UNUSABLE`；两处 `cannot reach rurge at …` 的断言不含 `error: ` 前缀），订正里又说 `W_MITM_OFF` 是 `serve.rs` 第 48 行的备注常量，也不对：那一行是 `MITM_OFF`，代码里没有 `W_MITM_OFF`。这三处只在报告里，清单与验收文档没有引。修正第 1 轮之后协调者重跑的判据：`check_rows.py` 的「缺」0 行、`MISS 0`、`AMBIG 0`；`count_rows.py` 的三张表与 `40f5975` 上相同；`manual_refs.py` 是 68 项、被「真机上」点到 65 项，没有被点到的是 `M2/W5`、`M2/M5`、`M2/L5`；`pipes.py` 与 `paren_check.py` 对清单、验收文档、计划三份的输出与 `40f5975` 上那一份相同（都没有输出）；`added_words.py` 对清单与验收文档都是 `HITS 0`，对计划只命中第 52 行把判据要查的词逐个列出的那几处；`changed_cited.py` 是 `NOT NOTED 0`，`ci_wording.py` 只列验收文档第 157、217、236、262 行，`shorthand_scan.py` 是 `HITS 0`；三份的 CR 数是 0；`curly.py 40f5975` 与 `ctrl.py 40f5975` 都是 0；`hash_suspects.py` 对第 52、53 行都是 0；账户名 26。裁定：① 6b 派工单提示第 3 条里"订正了几项"一类的话是协调者留下的占位，执行者照抄进了清单第 5 行，是协调者的错，修正第 1 轮换成了实数；之后的派工单里要执行者填的数写明"照实写数"，示例句里不留占位的字。② 验收文档第 254 行照第 27 行的裁定改了「真机上」（第 53 行④）；验收文档第 255 行不改：这一句说的是协议识别，识别的结果是记录里的 `protocol`，`PROTOCOL` 规则怎样用它由用例断言。③ 接受索引表第 2 条加的 `M5/W13`、`M5/M13`、`M5/L13`：HTTP/3 的站点被阻断、回落到 TCP 之后被解密，也是"主流站点解密"的一支。④ 接受 `M2/P1` 一项算三个平台：给证书固定提示的路径上（`serve.rs`、`crates/rurge-engine/src/mitm.rs` 与 `crates/rurge-http/src/mitm/` 下）除了 `#[cfg(test)]` 没有别的 `cfg` 属性，也没有 `cfg!`，协调者用 `git grep` 核过；macOS 与 Linux 上那条路径没有运行过。⑤ `M2/W5`、`M2/M5`、`M2/L5`（卸掉 CA 之后不信任的客户端：一行 WARN、`mitm.untrusted` 加一、承载连接一条失败记录）不进验收文档第 100 行：那一句是「证书固定提示日志」，它的用例格引 `an_untrusting_client_is_warned_once_an_hour_and_counted_each_time`，是因为两种提示共用 `crates/rurge-engine/src/mitm.rs::MitmShared::warn_now` 的限频（`Hint::Untrusted` 与 `Hint::Pinning`），不是因为这一句说到不信任的客户端；这三项在「靠手工验收的」里列为没有被点到的项，写明它们验的是什么。⑥ 交给任务 7 的：清单仍是 68 项，引"六十八项"的地方照旧；被点到的 65 项与没有被点到的三项照上面写。任务 6b 至此评审完，之后是任务 7a 与 7b。 | 本提交 |
| 55 | 7a | 验收文档的其余（只改 `docs/acceptance/phase4-acceptance.md` 与这一行；文档改动在 `aa41f00`）。「怎么读」补四小节：读的是什么（148 行，十张表各 11 / 24 / 47 / 22 / 3 / 17 / 2 / 4 / 12 / 6 行，句子的来处，计划 P4 的十六行）、谁读的（一个助手，协调者逐个任务评审，评审行是第 4、17、20、23、27、32、37、39、46、51、54 行）、读的代码（`bd981d2`，代码树与 `b43278b` 相同；补的用例与改的注释在 `cc8216b` ～ `88d8149` 的 21 个提交里）、没有覆盖的（手工验收一项也没有做、阶段 4 没有 L2、`a262d4c` 之后的提交 CI 没有见过、M3c 没有做、基准没有重跑）。「结论」：三个数与第四个数并排写成表（4 / 4 / 2 / 0），点名没有做完的两句 P0（随 M3c）与两句非功能需求、另 4 句「需求文字有问题」与 2 句「有意不同」；七条验收标准各一句（第 6 条满足，第 1 ～ 5、7 条未满足）。「数字」：照 `count_rows.py HEAD` 抄三张表，前面写数法。第一部分末尾四张登记表：前两张按那一句在第一部分出现的先后编号 1 ～ 4，第三张按同一次序排，第四张是空表、写明没有这样的句子（两个例子为什么记 `做了`）。第二部分：三种证据的读法与 M6-D27 的满足条件，第 1 条的四段写在黄金用例表之前、第 2 ～ 7 条各四段。「靠手工验收的」：清单 68 项（Windows 16、macOS 16、Linux 16、与平台无关 20）、被「真机上」点到 65 项、没有被点到的 `M2/W5`、`M2/M5`、`M2/L5` 各写一句。「靠 CI 的」：只在 Linux 与 macOS 上编译的一条、带 `#[ignore]` 的 20 条（两份 memory 的各两条、基准的十六个）、`a262d4c` 之后 CI 没有见过的 27 行（第一部分 26 行、黄金用例表 1 行）。「留给项目所有者的」：M6 设计第 13 节的十一条加 M6 计划「延后事项」第 12、13 行，第 8 条是一张按 `docs/status.md` 阶段 4 各节汇总的表。门禁（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI）：fmt 与 clippy 退出 0；测试 93 个结果行、5011 通过、0 失败、29 忽略，`rurge-platform` 的单元测试二进制两次挂在 `tun::windows::wintun` 那一组上（整份一起跑时），分两半单独跑都过（454 与 7 通过、4 忽略），合计 94 个结果行、5472 通过、0 失败、33 忽略，与基线相同。**执行者的裁定**：① 数字那张表照 `count_rows.py` 的 7 列写（没有「做了，本机没有能红的用例」一列，那一种 0 行）；② 第 1 ～ 5 条的 L1 里各写一次全量门禁的数，不只写「同第 1 条」；③ 验收文档第 55 行「任务 8 读回…改写」是前面任务写下的字，这一任务不动它。 | `aa41f00`、`5d58b65`、本提交 |
| 56 | 9 | **插入的任务**：任务 7a 的全量门禁（2026-11-04）在 `rurge-platform` 的单元测试二进制里三次停在 `tun::windows::wintun::tests::a_file_that_is_not_a_dll_cannot_be_loaded`，系统日志（System，来源 `Application Popup`，事件 26）在 05:42:40、05:47:20、05:47:51 各一条，协调者在 06:12:57 以 `CREATE_DEFAULT_ERROR_MODE` 起子进程复现一条（只记时刻，不抄全文）。原因：`load` 直接调 `LoadLibraryExW`，文件不是 PE 映像时装载器报硬错误，线程与进程的错误模式里没有 `SEM_FAILCRITICALERRORS` 时 Windows 弹出模态的「损坏的映像」对话框、调用要等人点掉才返回；错误模式从父进程继承，所以取决于门禁从哪里起。**协调者的裁定**（代项目所有者作，所有者可以改）：①修在 `load` 里，`LoadLibraryExW` 期间线程的错误模式是 `SEM_FAILCRITICALERRORS`、一返回就还原，照 libloading 的三条（第一次 `SetThreadErrorMode` 返回 0 也照装、之后不还原；原来就是 `SEM_FAILCRITICALERRORS` 不还原；装载失败的系统错误在还原之前读出），不用进程级的 `SetErrorMode`；②改在现有的 `load` 里、不加放开的函数，`LISTED` 仍是 23 项（这一条订正派工共同规矩里"`rurge-platform` 与 `rurge-tun` 阶段 4 没有改过"一句，这个任务只改 `load`、它的文档注释与同一个文件的 `mod tests`）；③`windows-sys` 的 feature 加 `Win32_System_Diagnostics_Debug`，按字母序插，不加新包；④新加一条用例 `a_file_that_is_not_a_dll_is_refused_with_the_default_error_mode`（以 `CREATE_DEFAULT_ERROR_MODE` 再起测试二进制、只跑那一条、至多等 60 秒、到时不退出就 kill 并写明多半有一个 "Bad Image" 对话框在等人点，只用安全的 Rust，注释写明为什么另起进程与在无交互桌面的地方可能红不了）；⑤`load` 的文档注释写明装载期间线程的错误模式与为什么这样设；⑥`CLAUDE.md` 的 `unsafe_code` 那一条里 `load` 的括号也写出前后两次 `SetThreadErrorMode`、并写明这两次是阶段 4 / M6 加的（这一行原有弯引号 0 处，没有可换的）；⑦阶段 3 的文档与手工清单 `W35` 不改，`docs/status.md` 由任务 7b 写。**红与绿**：`cargo test -j 4 --workspace --lib -- a_file_that_is_not_a_dll_is_refused_with_the_default_error_mode` 在只加了用例时 06:21:13 起跑、06:22:27 失败（60.53 秒，信息是子进程 60 秒还没结束、多半有一个 "Bad Image" 对话框，子进程 stdout 只有 `running 1 test`），系统日志 06:21:27 多一条；补了 `load` 之后同一条命令 0.62 秒通过，`-- tun::windows::wintun` 八条全过（7 条原有加 1 条新的）。**二十次循环**：直接跑测试二进制 `--exact` 那一条用例 20 次，全过（每条 0.06 ～ 0.12 秒）。**缺省错误模式下的整个二进制**：照任务 7a 停住的那种起法（`cmd.exe /c` 经不带重定向的 `Start-Process`）在 06:27:13 起跑，1.57 秒结束，466 条（通过 462、忽略 4、失败 0）；系统日志从绿那一步起一条 `Application Popup` 也没有。**门禁**三步：`cargo fmt --all --check` 退出 0，clippy 0 个警告，`cargo test -j 4 --workspace --no-fail-fast` 94 个结果行、5473 通过、0 失败、33 忽略（基线 5472 加 1）；一台 Windows 11 x86-64 开发机上的一次运行，不是 CI。`Cargo.lock` 没有变，`LISTED` 仍是 23 项。新的用例 CI 没有见过。 | 本提交 |
| 57 | 9 | 修正第 1 轮（协调者的评审意见三件；只改一条用例的注释与这张表，没有改行为）：① `crates/rurge-platform/src/tun/windows/wintun.rs` 里 `a_file_that_is_not_a_dll_is_refused_with_the_default_error_mode` 的文档注释第二段整段换掉——原来写 "`0x0` from the Explorer" 与 "which is what a process started from the Explorer … looks like"，而资源管理器从来没有量过（这台机器上量过的只有 Git Bash、PowerShell、带与不带重定向的 `Start-Process`、经 `cmd.exe /c`），另外 PowerShell 直接起的进程也是 `0x8003`、不只是带重定向的时候；现在的七行只写量过的：`0x3` 来自 Git Bash，`0x8003` 来自 PowerShell 与带重定向的 `Start-Process`，`0x0` 来自不带重定向的 `Start-Process`，`CREATE_DEFAULT_ERROR_MODE` 让子进程不论父进程怎样都用缺省的模式。第一段、第三段与代码一字未改。验证照 M4a 计划修正记录第 31、46 行只跑三步：`cargo fmt --all --check` 退出 0，`cargo clippy --all-targets -- -D warnings` 0 个警告，`cargo test -j 4 --workspace --lib -- tun::windows::wintun` 一条 `test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 458 filtered out; finished in 0.40s`；一台 Windows 11 x86-64 开发机上的一次运行，不是 CI。② 第 55 行的「`rurge-platform` 的单元测试二进制两次挂在 `tun::windows::wintun` 那一组上（整份一起跑时），分两半单独跑都过」订正为：停了三次——05:42:40 那一次在任务 7a 的全量门禁里（门禁 05:39:45 由不带重定向的 `Start-Process … cmd.exe /c` 起，05:45:44 结束那个停住的二进制之后跑完，93 个结果行、5011 通过），05:47:20 与 05:47:51 两次是之后单独重跑那个测试二进制（05:47:15 的 `--test-threads=1` 与 05:47:49 的 `--test-threads=4`，同样经不带重定向的 `Start-Process` 起的 `cmd.exe /c`，重定向写在 cmd 的命令行里）；此后四次由 `Start-Process` 带 `-RedirectStandardOutput` 起的运行（05:47:42、05:49:58、05:50:19、05:50:25）都过，分两半的两次也是；原因在起法（带重定向的 `Start-Process` 起的进程错误模式是 `0x8003`，里面有 `SEM_FAILCRITICALERRORS`），不在分开。③ 第 56 行的「任务 7a 的全量门禁（2026-11-04）在 `rurge-platform` 的单元测试二进制里三次停在 …」订正为：在全量门禁里的只有第一次（05:42:40），后两次是之后单独重跑那个测试二进制（05:47:20、05:47:51）。`e85bcfa` 的提交说明不能改（不 amend），其中两句以这一行为准：「线程与进程的错误模式从父进程继承」——从父进程继承的是进程的错误模式（`CREATE_DEFAULT_ERROR_MODE` 管的也是进程的），线程的错误模式是每个线程自己的；「Git Bash 是 0x3，PowerShell 带重定向是 0x8003，资源管理器与不带重定向的 `Start-Process` 是 0x0」——资源管理器没有量过，PowerShell 直接起的也是 `0x8003`。② 里订正第 56 行的那一句同样覆盖 `e85bcfa` 的提交说明里同样的那一句。 | `223c492`、本提交 |
| 58 | 9 | 协调者评审任务 9（第 56 行，`e85bcfa`、`936de7b`）与修正第 1 轮（第 57 行，`223c492`、`b50e384`）。**原因是怎样查到的**：任务 7a 的门禁停住之后，协调者照系统日志（System，来源 `Application Popup`，事件 26）的三条认出停在 `tun::windows::wintun::tests::a_file_that_is_not_a_dll_cannot_be_loaded`，读任务 7a 的会话记录对出三次各在哪一次运行（第 57 行 ②③）；用一个只读 `GetErrorMode()` 与 `GetThreadErrorMode()` 的子进程量了这台机器上七种起法的错误模式：Git Bash 直接起与经 `cmd` 起都是进程 `0x3`，PowerShell 直接起 `0x8003`，`Start-Process` 带重定向 `0x8003`（经 `cmd.exe /c` 也是），不带重定向 `0x0`（经 `cmd.exe /c` 也是），七种的线程错误模式都是 `0x0`；06:12:57 以 `CREATE_DEFAULT_ERROR_MODE` 起那个测试二进制、只跑那一条，8 秒之后还停在 `running 1 test`，系统日志多一条。**核过的**：`load` 的改法与 libloading（`src/os/windows/mod.rs` 的 `ErrorModeGuard`，只取事实）的三条一致——第一次 `SetThreadErrorMode` 返回 0 就照装、不还原，原来就是 `SEM_FAILCRITICALERRORS` 不还原，装载失败的系统错误在还原之前读出；两个新的 `unsafe` 块各有一行 `// SAFETY:`，`#[allow(unsafe_code)]` 没有多（`LISTED` 仍是 23 项）；`Cargo.lock` 没有变，`windows-sys` 多的只是 `Win32_System_Diagnostics_Debug` 一个 feature；`CLAUDE.md` 改的只是 `unsafe_code` 那一条里 `load` 的括号，工作树里 CR 与 LF 各 114。新用例照裁定写：以 `CREATE_DEFAULT_ERROR_MODE` 再起这个测试二进制、`--exact` 那一条、按系统时钟至多等 60 秒、到时 kill 并说多半有一个对话框在等人点；子进程只跑一条用例、输出很短，等待时不读管道不会把它卡住。红（06:21:13 起，60.53 秒失败，系统日志 06:21:27 一条）与绿（0.62 秒）、`tun::windows::wintun` 八条、二十次循环、缺省错误模式下的整个二进制（462 通过、4 忽略、0 失败）与门禁的日志协调者都读过，数与第 56 行相同；门禁 94 个结果行、5473 通过、0 失败、33 忽略，没有以 `STATUS_` 退出的测试二进制（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI）。修正第 1 轮只改那一段注释（`git diff 936de7b HEAD` 在 `wintun.rs` 里只有那六行换成五行）；之后协调者自己跑了一次 `cargo test -j 4 --workspace --lib -- tun::windows::wintun`，8 条全过（同上，一台机器上的一次运行，不是 CI）；`curly.py` 与 `ctrl.py` 以 `936de7b` 为基线都是 0，计划 CR 0，两个提交说明没有控制字符，带本机账户名的文件 26。**要知道的**：① 用例注释里原来"Explorer"那两处出自协调者的派工单，资源管理器没有量过，修正第 1 轮改掉了；② 第 57 行末句「② 里订正第 56 行的那一句」应为「③ 里」；③ 第 57 行 ② 的「此后四次」里 05:47:42 那一次在第三次停住（05:47:51）之前，次序以时刻为准；④ 磁盘：`windows-sys` 多一个 feature 让它在整个工作区换了哈希，任务 9 几乎重编了全部依赖，D: 盘一度只剩 2.2 GiB；协调者删了可再生的缓存（`target/release`、`target/debug/incremental` 下一天以前的目录、`target/debug/deps` 下两天以前的 exe 与 pdb），回到 71 GiB，门禁里没有因此失败的步骤；⑤ 执行者的三个顾虑照收：新用例在没有交互桌面的地方（例如 CI）多半红不了（注释写明了）；签名对而装不上的文件（阶段 3 手工清单 `W35` 的 (d)）没有能红的用例；第一次 `SetThreadErrorMode` 失败时这次装载仍可能弹出对话框（与 libloading 相同）。新的用例与改过的 `load` CI 没有见过。 | 本提交 |
| 59 | 7a | 修正第 1 轮（协调者的评审意见十九件；只改验收文档与这张表，没有改代码、没有跑门禁）：① 第 65 行的「协调的会话逐个任务评审」改成「协调者逐个任务评审」，同一行末尾的评审行名单加第 58 行与「之后以「协调者评审任务」开头的各行」。② 第 70 行的「`88d8149` 之后的提交只改文档」改成写明 `88d8149` 之后改了代码的只有 `e85bcfa`（阶段 3 的 `crates/rurge-platform/src/tun/windows/wintun.rs` 里的 `load`、同文件一条用例与 `windows-sys` 的一个 feature）与 `223c492`（只改那条用例的注释），两者都不是本文哪一句的代码；不写新用例的名字（名字核对的基准仍是代码与 `88d8149` 相同的提交）。③ 门禁的数：第 396 行那一段（第二部分开头的 L1）换成 `e85bcfa` 的代码上的一次——fmt 与 clippy 退出 0，`cargo test -j 4 --workspace --no-fail-fast` **94 个结果行、5473 通过、0 失败、33 忽略**，比第 46 行那一次多的一条是 `e85bcfa` 加的那条用例（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI）；第 404、439、446、453、460 行的 5472 各改成 5473；不再写 93 / 5011 / 29、停住、分两半与「合计」。第 55 行门禁那一段由第 57 行 ② 与这一行订正。④ 第 102 行改对示例数：25 行示例里 22 行由 16 条 the_manual_ 用例对上，`header-replace DNT 1` 那一行由 `the_four_actions` 对上，jq 的两行没有。⑤ 第 103 行与第 439 行的「握手之后不读」改成「握手之后不发请求就关」（照那条用例的文档注释）。⑥ 第 104 行的 9 条改成 7 条；第 446 行只列 Body Rewrite 的 7 条，另起一句写 M3b 另两条 the_manual_ 用例是 Map Local 的、在第 1 条那张表里；第 448 行的「九条」改成「七条」。⑦ 第 102、104 行的「还缺 jq」改成「还缺 jq 与 CI 对 `a262d4c` 之后的代码」；第 407、449 行「还缺什么」各加一句另缺 CI 对 `a262d4c` 之后的代码（第 400 行那一段）。⑧ 第 94 行去掉「与功能需求那两句写在同一句里」；第 96 行拆开：前两种逐句见「需求文字的问题」与「有意不同的」两张表，另起一句写 2 句「不属于阶段 4」（第 216、222 行各自的去向）。⑨ 第 390 行改写：产品代码里本机编译不到的只有 `restrict` 的 `#[cfg(unix)]` 那一支（`#[cfg(not(unix))]` 那一支在这台机器上编译、在用）；测试代码里两份量内存模块的 Linux 与其余平台两支本机也编译不到，只供带 `#[ignore]` 的量法用例。⑩ 第 493 行改写：第一部分没有一句需求说的是不信任的客户端，引那条用例的两行（第 147、257 行）各有自己的理由，说的都不是不信任的客户端；三项照旧按清单做。⑪ 第 511 行的黄金用例表行号 418 改成 429。⑫ 第 106 行的「API 的六条与命令行的一条」照第 460 行改成「API 的九条（M6 补的两条在内）与命令行的两条」。⑬ 第 105 行的「两段」改成「两条」。⑭ 第 61 行改成「十六行的代码一格写着「同…」」，并加一个括号说 FR-CFG-14 的「…Map Local 数据」一行（第 293 行）只在用例一格借了另一行的一组用例、不算在内。⑮ 第 519 行只留「Q5——把 CA 自动装进系统与 Firefox 的信任库」；「经虚拟网卡来的会话的名字在 INFO 及以上」进第 8 条那张表新增的一行（M2，出处 M2 细化设计第 22 节、阶段 3 / M3c 计划延后第 6 行那一项）。⑯ 第 532 行加一句：304 是用例里看到的，204、1xx 与 `HEAD` 照代码读是同一条路，没有单独核实。⑰ 第 8 条那张表逐行：M1b 第一行的出处加括号（那一行的去向写的是以后的会话，`docs/status.md` 把它列在归所有者的一段）；M1b 之后加 M2 两行（M2-D2、D4、D9、D11、D18、D21 与 INFO 及以上那件）；M2b 之后加一行（`RST_STREAM(NO_ERROR)`）；M3a 照 M3 设计第 21 节写九项；M3b 的出处改成 M3b 计划修正记录第 22 行的测量与 `docs/status.md` 的阶段 4 / M3b 一节（1 亿是 M3-D8 定的）；M4a 的事项补 M4-D11 与 M4-D13，出处加 `docs/status.md` 的阶段 4 / M4a 一节；M5b 的出处改成「延后事项」第 1 ～ 3 行；M4b 与 M5a 两行不改。⑱ 第 524 行写明表里列的是那几段里点了名的事项与设计里「尤其」列出的裁定，各节另有一句总括（设计与计划里代所有者作的裁定都可以改），那些不逐条进表。⑲ 第 552 行的「任务 9」写成「M3b 任务 9」。这一轮没改的：第 309、362 行的「今天只有读数」是 `1b62ff0` 上就有的；结论、各表的数不变。 | 本提交 |
| 60 | 7a | 修正第 2 轮（协调者的评审意见四件；只改验收文档与这张表，没有改代码、没有跑门禁）：① 第 102 行（结论第 1 条）的「jq 的两行没有；」改回「jq 的两行没有（随 M3c）；」（这一句的「（随 M3c）」在修正第 1 轮那①件里丢了，出自协调者第 1 轮说明的措辞）。② 第 493 行（「靠手工验收的」）的「引了那条用例的两行」改成「引了 `an_untrusting_client_is_warned_once_an_hour_and_counted_each_time` 的两行」（这一行里已经没有用例名，「那条」没有所指；同样出自第 1 轮说明的措辞），其余不改。③ 第 542 行（第 8 条那张表新加的第二个 M2 行）：「照 M3-D26 写」在阶段 4 的文档里会被读成阶段 4 / M3 设计的 M3-D26，而 M2 细化设计第 22 节那一行说的是阶段 3 / M3 设计的 M3-D26（「名字与地址只在 DEBUG 与请求记录里」，已开 M2 细化设计第 594 行与阶段 3 / M3 设计那一行核过）；事项一格改成「M2 新加的握手失败与证书固定提示的行，对经虚拟网卡来的会话在 WARN / INFO 里不带名字、名字只在 DEBUG 与记录里（阶段 3 / M3 设计的 M3-D26）；所有者若决定 INFO 及以上可以带，这几行随之改」。④ 第 548 行（M3a）：「设计里可以改的九项（…）」把意思改窄了（M3 设计第 21 节说的是 M3-D1 ～ D31 都可以改、尤其这九项），改成「设计里可以改的裁定，尤其九项（M3-D2、M3-D3、M3-D4、M3-D7、M3-D9、M3-D11、M3-D16、M3-D27、M3-D30）」，写法与 M1a、M2 那两行一致（也出自第 1 轮说明的措辞）。 | 本提交 |
| 61 | 7a | 协调者评审任务 7a（第 55 行，`aa41f00`、`5d58b65`、`f6998f1`）、修正第 1 轮（第 59 行，`7352a88`）与修正第 2 轮（第 60 行，`a962029`）。**核过的**：「结论」「数字」里的数与 `check_rows.py` 的输出相同（148 行、十张表 11 / 24 / 47 / 22 / 3 / 17 / 2 / 4 / 12 / 6；做了 129、做了，补了用例 7、没有做完（已登记）4、需求文字有问题 4、不属于阶段 4 2、有意不同（已登记）2；`MISS 0`、`AMBIG 0`）；第一部分写着「CI 没有见过」的 26 行与「靠 CI 的」那一段列的行号逐个相同，写着「只在 Linux 与 macOS 上编译」的只有第 310 行；第二部分引的用例名逐个在 `20efc16` 与 HEAD 上 `git grep` 过，`20efc16` 上没有的正好是写着「CI 没有见过」的五条（`06d0806` 的一条、`0b01ebf` 的两条、`3d4702b` 与 `c24a278` 各一条）；代码一格以「同「」开头的十六行与计划 P4 的十六行逐个对上，FR-CFG-14 的「…Map Local 数据」一行只在用例一格借了别一行的一组用例；手工清单 68 项、「真机上」点到 65 项、没有点到的三项是 `M2/W5`、`M2/M5`、`M2/L5`。「留给项目所有者的」第 1 ～ 12 条与第 8 条那张表由一个子代理逐条对过出处（`docs/status.md` 阶段 4 各节、各设计的开放问题与"尤其"、各计划的延后事项与修正记录、重对报告、兼容性清单与手工清单），它查出的几处协调者都打开原文核过：第 3 条把 M2 细化设计第 22 节单列的一件写进了 Q5；第 12 条少了 M6 计划延后第 12 行"304 是用例里看到的"那一句限定；表里 M1b 第一行的去向、M3a 的"尤其"少三项、M3b 与 M4a 的出处、M5b 的出处与事项对不上；`docs/status.md` 各节点了名而表里没有的 M2 设计的"尤其"、M2b 延后第 2 行、M4-D11 与 M4-D13；第 8 条说得比表大。这些与协调者自己读出的十几处一起进了修正第 1 轮（十九件，第 59 行），修正第 2 轮四件（第 60 行，其中三件出自协调者第 1 轮说明的措辞）。**协调者的裁定**：第 15 节第 1、3 条的「还缺」与第 4、5 条一样写上 CI 对 `a262d4c` 之后的代码（M6-D27，验收文档第 400 行那一段）；「谁读的」里的评审行名单写到第 58 行，之后的写成「之后以「协调者评审任务」开头的各行」；第 309、362 行的「今天只有读数」在 `1b62ff0` 上就有，不改。**门禁**：7a 的那一次停住了三次，原因与修法见第 56 ～ 58 行，第 55 行门禁那一段由第 57 行 ② 订正；验收文档里的门禁数现在取自任务 9 在 `e85bcfa` 上的那一次（94 / 5473 / 0 / 33，一台 Windows 11 x86-64 开发机上的一次运行，不是 CI）；两轮修正只改文档，没有跑门禁。**判据**：两轮之后 `curly.py` 与 `ctrl.py` 以 `97d917b` 为基线都是 0，验收文档与计划的 `pipes.py` 输出为空、与基线上那一份的输出相同，两个文件 CR 0，两个提交说明没有控制字符，带本机账户名的文件 26；新写进的 `the_four_actions`、`MitmShared::warn_now`、`an_untrusting_client_is_warned_once_an_hour_and_counted_each_time`、两份 `memory.rs` 与 `crates/rurge-platform/Cargo.toml` 在 HEAD 上都在。 | 本提交 |
| 62 | 7b | 文档收尾（任务 7 的后一半；只改文档，没有跑门禁；新写的「CI 没有见过」逐处列在报告的「CI 没有见过的单子」一节，接在 7a 那张后面）。改了十份文件：两份 README 各五行（`README.md` 第 36、52、60、65、66 行与 `README_en.md` 第 36、51、59、64、65 行：阶段 4 的状态、特性表的 HTTP 处理一行、「现在」一句、路线图的阶段 3 与阶段 4 两项；两份内容一致，不写「齐备」与流程用语）、`docs/requirements.md` 第 7 节阶段 4 的「状态」一行（接着写 M6 的一句，前面的句子不改；同一次把这一行原有的九对弯引号换成直引号，好让 `curly.py` 为 0：换的是「关掉整条连接」「重置这个流」「M4 还剩 M4b」「下一步是 M4b 的计划」「验收没有做」「CI 没有见过」「只有 CI 会编译」「下一步是 M5 的细化设计」「下一步是 M5b（基准与遗留）的计划」九处）、`CLAUDE.md`（「现在在哪里」的阶段 4 与 CI 两条；阶段 4 那一条原有的两对弯引号也换成直引号（「关掉整条连接」「重置这个流」）；「这台开发机」加一条错误模式与模态对话框、并在 `target/` 那一条末尾加依赖 feature 与换哈希的一句）、`docs/status.md`（阶段 4 下新的「M6（收尾与验收）」一节）、`docs/README.md`（`docs/acceptance/phase4-acceptance.md` 一项；手工清单一项的描述随 68 项改）、阶段 1 ～ 3 的验收文档各一句（P17，日期 2026-11-04；阶段 1、2 加在第 3 行，阶段 3 加在标题之后）、`docs/api/phase4.md` 末尾新的「M6 的核对（2026-11-04）」一节（五处：M3b 的两条用例说多了、端点更新没有用例、M5a 的入口一步与重载之后的读数）。「延后事项」加第 14 行（Map Local 的 URL 数据没有断言「第一次登记定下种类与标签」的用例、也没有经端点更新一项 `data` 的用例）；任务 9 的三个顾虑不加行，理由在报告里（分别写进了注释、做阶段 3 手工清单 `W35` 时自然看得见、与 libloading 相同而没有可补的用例）。「执行期修正记录」第 1 ～ 61 行逐行看过，结论在报告里。 | `991988e`、`eb536ca`、`a6302c3`、本提交 |
| 63 | 7b | 修正第 1 轮（协调者的评审意见十三件；只改文档与这张表，没有改代码、没有跑门禁，验收文档没有改）：① 阶段 1 ～ 3 验收文档的行号——派工单第 6 条的写法与第 4 行的裁定相反，照第 4 行做：`docs/acceptance/phase1-acceptance.md` 与 `docs/acceptance/phase2-acceptance.md` 把第 3 行那一句接到第 2 行末尾（中间不加空格）、删掉第 3 行，两份回到 657、667 行；`docs/acceptance/phase3-acceptance.md` 删掉第 3 行的空行、那一句占住原来空行的位置，回到 461 行；三份对 `1682b3e` 各只有一个改第 2 行的块，后面各行的行号照旧。② `docs/api/phase4.md`：把「## M6 的核对（2026-11-04）」整节挪到 M5a 一节「没有覆盖的」的最后一条（「没有观察过 Surge…」）之后、文件末尾，那一条回到原处、一个字不改（派工单第 2 条的写法所致：原来插在它前面）。③ 两份 README 的 68 行：`README.md` 第 60 行与 `README_en.md` 第 59 行去掉「仍是」/ "still"，写成阶段栏含 4 的 68 行、其中「叶证书的密钥」一行是 M6 加的（`bd981d2` 上 67 行）；`docs/status.md` 第 1265 行补一句：M6 里改兼容性清单的还有任务 2a（7.2 节「叶证书的密钥」一行，`fa455da`）、任务 4（10.4 节 `GET` / `POST /v1/capture/filters` 与 `capture-filter` 一行，阶段栏「待定」，`6115ce1`）、任务 5（2.1 节 `block-quic` 与 7.2 节 `auto-quic-block` 两行，`7618e46`，之后 `b24f21c`、`9da3059` 又改过），附录 577 → 579 行就是那两行、67 → 68 行是「叶证书的密钥」那一行（附录 🟡 138 → 139、❓ 5 → 6）。④ 「CI 没有见过」：`README_en.md` 第 36、51、59、65 行四处改成以 "CI has not seen" 起的说法；`docs/status.md` 第 1287 行的 `git grep` 模式加上 `CI 既没有编译\|CI 还没有编译过`；用新模式对七份跑一遍。⑤ `docs/status.md` 第 1267 行：标题改成「补的用例与改的注释」（派工单的措辞所致），补上 `5441fb0`、`41b1066`、`0b16d63`（只改 `crates/rurge-config/src/mitm.rs` 的注释）与 `5d97a69`（`crates/rurge-engine/tests/http.rs` 里的类型名），「每个各有一行」改成几个提交共用一行，细节的行号改成每个提交第一次出现在记录里的那一行（第 5、11、17、18、19、21、24、25、28、29、30、33、35、41、43、44 行）。⑥ 「CPU 不走」：`CLAUDE.md` 第 109 行与 `docs/status.md` 第 1284 行改成「门禁停在一个测试二进制上、久久没有新的输出时（先）看系统日志」，不提 CPU（派工单的写法所致：与同一节「CPU 时间不可信」一条相抵）。⑦ `docs/status.md` 第 1274 行：照验收文档改成第 6 条满足、第 1、3 条还缺 jq 与 CI 对 `a262d4c` 之后的代码、第 2 条还缺手工验收、第 4、5 条还缺 CI 对 `a262d4c` 之后的代码、第 7 条未满足。⑧ `docs/status.md` 第 1286 行：「数字」一节是两张表与「三个数与第四个数」的四条列表；四张登记表里前两张的 `#` 是表内编号 1 ～ 4、「有意不同的」没有 `#` 一栏、「本机没有能红的用例的」是空表（打开验收文档第 316 ～ 392 行对过）。⑨ `docs/api/phase4.md` 新一节 M5a 的第一条：写进预匹配（`sniffable_session` 在 `sniffable_target` 之后、代理端口上问一次 `runtime.rules.pre_refuses`），后半句改成传输、暂停与预匹配不共用（核过 `crates/rurge-engine/src/serve.rs` 两个函数的原文）。⑩ `docs/status.md` 第 1268 行：「错误模式从父进程继承」改成「进程的错误模式从父进程继承」；「照 libloading 的三条」改成照 libloading 的 `ErrorModeGuard` 并写出三条。⑪ `docs/status.md`：第 1259 行的 82 个提交写明到 `1682b3e`；第 1275 行的「文中」改成「验收文档里引的」；第 1266 行照 M6 记录第 52 行写出「怎么用」的就地改动。⑫ `README_en.md` 第 36 行第四个数照同一份阶段 3 的写法写成 "0 sentences this machine has no test that can turn red for"（派工单的写法所致）。⑬ **执行者的裁定**：第 62 行的提交一栏在 `b6a129d`、`0c4f97c` 改过两次，按「记录」的规矩本不该改写已写下的行（自己刚写的也一样）；不回退，写明第 62 行在 `d39a09a` 写下、那一格的「本提交」指 `d39a09a`、它的提交一栏后来写成 `991988e`、`eb536ca`、`a6302c3`、本提交，以及第 62 行「阶段 1、2 加在第 3 行，阶段 3 加在标题之后」由本行①订正。 | 本提交 |
| 64 | 7b | 修正第 2 轮（协调者的评审意见三件；只改文档与这张表，没有改代码、没有跑门禁，验收文档没有改）：① `docs/status.md` 第 1267 行的「与一处类型名（`5d97a69`，`crates/rurge-engine/tests/http.rs`）」改成「与一处类型别名（`5d97a69`，`crates/rurge-engine/tests/http.rs` 里的 `SeenPath`）」（`5d97a69` 加的是 `type SeenPath = (String, Vec<u8>);`，`path_echo` 读到的一个请求的路径与正文），订正第 63 行 ⑤ 的「类型名」。② `docs/api/phase4.md` 第 1187 行那个括号改成「（UDP 流也问它，`auto_quic_blocks` 就是问它的；`sniffable_target` 的文档注释写着 `The transport is not asked`，`auto_quic_blocks` 的写着 `Neither the pause nor pre-matching is asked`）」：原来「它的文档注释」读起来是 `sniffable_target` 的，可那几个词在 `auto_quic_blocks` 的注释里（`crates/rurge-engine/src/serve.rs` 第 330 ～ 331 行），`sniffable_target` 自己那一段（第 279 ～ 281 行）写的是 `The transport is not asked`。③ `README.md` 第 36 行原有的两对弯引号换成直引号，围着「关掉整条连接」与「重置这个流」这两处（照第 46 行裁定①；同第 62 行里 `CLAUDE.md` 第 16 行与 `docs/requirements.md` 第 531 行的做法），只换那四个字符。 | 本提交 |
| 65 | 7b | 协调者评审任务 7b（第 62 行，`991988e`、`d39a09a`、`eb536ca`、`a6302c3`，第 62 行的提交一栏在 `b6a129d`、`0c4f97c` 订正）、修正第 1 轮（第 63 行，`c17cafd`、`c623c0b`）与修正第 2 轮（第 64 行，`9c8d355`）。**核过的**：两份 README、`docs/requirements.md` 第 531 行、`CLAUDE.md` 第 16 行与 `docs/status.md` 新一节写的三个数与第四个数，与 `count_rows.py HEAD` 的输出相同（没有做完（已登记）4：P0 2、非功能需求 2；需求文字有问题 4：P0 4；有意不同（已登记）2：P0 1、非功能需求 1；本机没有能红的用例 0）；兼容性清单阶段栏含 4 的在 `bd981d2` 上 67 行、HEAD 上 68 行，多的一行是任务 2a 在 7.2 节加的「叶证书的密钥」（第 18 行，`fa455da`），附录 577 → 579 行是这一行与任务 4 在 10.4 节加的一行（第 33 行，`6115ce1`，阶段栏「待定」），🟡 138 → 139、❓ 5 → 6；第 18、33、42 行由 `git blame` 对到 `fa455da`、`6115ce1`、`b24f21c`（第 42 行记的清单改动在 `7618e46`）；手工清单 68 项（M1 9、M2 21、M3 13、M4 19、M5 6）；`docs/status.md` 新一节的数：`bd981d2` 到 `1682b3e` 82 个提交，`cc8216b` ～ `88d8149` 改 `crates/` 的 21 个提交，其中只改注释的三个（`5441fb0`、`41b1066`、`0b16d63`）与加类型别名 `SeenPath` 的一个（`5d97a69`），以及 21 个提交第一次写进记录的那十六行；`CLAUDE.md` 只改了第 16、17 行（「现在在哪里」）与第 109、114 行（「这台开发机」，派工单定的两处）；阶段 1 ～ 3 的三份验收文档对 `1682b3e` 各只有一个 `@@ -2 +2 @@` 的块，行数 657、667、461 照旧；`docs/api/phase4.md` 新一节全在文件末尾（`@@ -1172,0 +1173,16 @@`），第 1187 行对过 `crates/rurge-engine/src/serve.rs` 的 `sniffable_session`、`sniffable_target` 与 `auto_quic_blocks`；`docs/status.md` 第 1287 行的 `git grep` 模式列得出 `a262d4c` 之后的各处，它不列的几种说法（「CI 一个也没有见过」「CI 没有跑过」与英文的 "CI has never"）都在写阶段 2、3 的历史段落里。**协调者自己的错**：派工单第 6 条（阶段 1 ～ 3 的验收文档另起一行）与第 4 行的裁定相反，「CPU 不走」与 `CLAUDE.md`「这台开发机」里 CPU 时间不可信的一条相抵，21 个提交写成「计划记录里各有一行」，三处由第 63 行 ①⑥⑤ 订正；修正第 1 轮第 13 件说「自己刚写的行也不改」也错了：派工的规矩允许一个任务交回之前另起 `docs:` 提交订正它自己写的行、提交说明写清改了什么，所以 `b6a129d`、`0c4f97c`、`c623c0b` 都合规矩（`c623c0b` 也不算违反修正第 1 轮「提交之后不再改第 63 行」那一句，那一句出自同一个错），这一条覆盖第 63 行 ⑬「本不该改写已写下的行（自己刚写的也一样）」那半句，⑬ 写明的第 62 行在 `d39a09a` 写下、那一格的「本提交」指 `d39a09a` 照旧有效。另外第 63 行 ② 与 ⑫ 括号里的「派工单（第 2 条）的写法所致」不对：任务 7b 的派工单写的是「在文件末尾加新的一节」，第四个数的英文写法派工单没有给，出自派工单写法的只有 ①⑤⑥。教训：写派工单的一条之前，先在记录里查同一件事以前的裁定（第 4 行早就定了阶段 1 ～ 3 的验收文档不改行号）。**协调者的裁定**：① `docs/requirements.md` 第 531 行的九对弯引号换成直引号，接受（第 46 行裁定①，任务 7b 的报告问过），`README.md` 第 36 行的两对由第 64 行照同一条换了；② 两份 README 不逐条列总设计第 15 节的七条，只写第 6 条满足、其余六条没有一条能写成通过、第 7 条要一次三个平台全绿且编译了阶段 4 全部代码的运行，接受；③ 延后第 14 行的去向写项目所有者，照第 23 行裁定①与延后第 13 行的先例，接受，`docs/status.md` 第 1289 行点了它的名，验收文档不改。**门禁**：任务 7b 与两轮修正只改文档，没有跑门禁；门禁的数仍是任务 9 在 `e85bcfa` 上的那一次（94 / 5473 / 0 / 33，一台 Windows 11 x86-64 开发机上的一次运行，不是 CI）。**判据**（修正第 2 轮之后，以 `1682b3e` 为基线）：`curly.py` 与 `ctrl.py` 都是 0，`docs/` 之外改了的行（`CLAUDE.md`、两份 README）也没有弯引号；改过的十一份 `pipes.py` 的输出都是空的；`added_words.py` 只在 `docs/status.md` 第 1259 行碰到「Pi」，那是「由协调的会话逐个任务调 Pi 做」（阶段 4 前面各节同样的写法）；CRLF 的八份 CRLF 数等于 LF 数，LF 的三份 CR 0；九个提交说明没有控制字符；带本机账户名的文件 26。 | 本提交 |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | **手工验收**：阶段 1 ～ 4 四份清单，一项也没有做过 | 项目所有者（真机上） |
| 2 | **推送与读 CI**：M4b 任务 11、M5（本地 `main` `bd981d2` 比 `origin/main` 多 48 个提交）与 M6 的提交 | 项目所有者推送；任务 8 读回 |
| 3 | 总设计 Q1 ～ Q6：尤其 Q4——M3c（jq、`br`、`zstd`）等它；`p12-keystore` 的升级；`webpki-roots` 的许可（NFR-11）；要不要改用 `rustls-platform-verifier`（重对报告第 3 节第 12 条：Surge 说它的上游证书链靠操作系统的信任评估）；Q6——需求文档的阶段栏 | 项目所有者 |
| 4 | 对照查出的"需求文字有问题"各句怎么改；"没有做完"的 P0 / P1 句子放进哪个阶段 | 项目所有者（任务 7 汇总） |
| 5 | M1a ～ M5b 归所有者的延后项与可以改的裁定 | 项目所有者（验收文档的「留给项目所有者的」汇总，不在 M6 里改） |
| 6 | M5b 计划延后第 1 ～ 3 行。第 1、2 行写着"由阶段 4 / M6 的设计决定查不查"：M6-D22 答**不查**（理由见设计第 12 节）；第 3 行本来就归所有者。三行：B1 的明文一路慢要不要查、要不要为它改引擎里 hyper 的写法；B4 的握手长尾要不要查（第一步是加一个直连的握手基线）；改写正文的峰值约多 3.94 份正文，要不要做得更省、归哪个里程碑 | 项目所有者 |
| 7 | h2spec（M6-D25）：hyperium/h2 的 CI 拿 h2spec v2.1.1 测它自己的示例服务端；对 rurge 解密之后对客户端讲 HTTP/2 的那一侧跑它要下载它的二进制，M6 不做 | 项目所有者：要不要在 CI 里跑 |
| 8 | 叶证书的密钥：Surge Mac 6.10.0 撤回了"每个域名一把密钥"；rurge 每张叶证书一把 P-256（兼容性清单 7.2 节新的一行，P21） | 项目所有者：改不改 |
| 9 | 抓包过滤器：手册新写的 `GET` / `POST /v1/capture/filters` 与 `capture-filter`（兼容性清单 10.4 节新的一行，P21） | 项目所有者：要不要做、在哪个阶段 |
| 10 | 重对报告第 4 节：与阶段 4 无关的手册与发行说明的变化 | 项目所有者：要不要、在哪个阶段处理，要不要另开一轮重对把它们落进兼容性清单 |
| 11 | M3c 做完之后：它的收尾重读 FR-HTTP-07 的"jq 表达式"与"解压"之 `br`、`zstd` 两句、总设计第 15 节第 1、3 条，新的结论写在验收文档末尾新的一段，并改「结论」一节的数（M6-D26） | 阶段 4 / M3c 的收尾 |
| 12 | 304、204、1xx 或 `HEAD` 这类没有正文的应答之后，同一条客户端连接上的下一个请求在现在的代码里会多拨一次上游连接（那条连接要等下一个请求来了才走 `complete` / `put`）；`task-1-mut-3.log` 的实验里，即使把这类应答提前 `complete`，连接也已经因为上游多写的字节被 hyper 标成关闭、借不出去（红 0 / 绿 20）。304 是用例里看到的；204、1xx 与 `HEAD` 的应答照代码读是同一条路（`Tail::new` 的 `empty` 与应答的状态码），没有单独核实 | 项目所有者：改不改、怎样改，要先有设计 |
| 13 | 重对报告第 4 条：MITM 的 HTTP/2 连接上，读得慢的客户端让 rurge 在途攒多少，没有量过、没有能红的用例（照读到的停在窗口与缓冲的量级，见验收文档 FR-HTTP-02 "HTTP/2：MITM 后与客户端…" 一行）。要量就要一条新用例：源站送远大于回环套接字缓冲的正文（几十 MiB），客户端只给很小的窗口、不读，量源站写出去多少；界要远大于三个平台回环的套接字缓冲，否则会偶发红 | 项目所有者：要不要补这条用例 |
| 14 | Map Local 的 URL 数据：`the_same_map_local_url_is_one_resource` 只断言列出一项、只下载一次，「第一次登记定下种类与标签」没有断言；没有用例经 `POST /v1/external_resources/update` 按键或以 `"all"` 更新一项 `data`（按键能不能更新与列不列出是同一个判断，`type_name`；`docs/api/phase4.md` 的「M6 的核对」一节照实写了） | 项目所有者：要不要补这两条用例 |
