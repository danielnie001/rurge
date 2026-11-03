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
