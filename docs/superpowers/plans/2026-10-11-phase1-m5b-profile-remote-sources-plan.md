# 阶段 1 / M5b（补遗：Profile 的远程来源与阶段 1 的收尾）实施计划

> **执行方式**：子代理驱动（superpowers:subagent-driven-development）——每个任务一个新的实现者，做完由新的评审者评审，全部做完后整分支评审一次。本计划写到"改哪里、接口是什么、要哪些用例、判据是什么"为止：实现者先写失败的用例、看到红，再实现到绿；与计划的出入记进末尾的「执行期修正记录」。

**目标**：阶段 1 两处从来没有做的 P0 落地——托管配置（`#!MANAGED-CONFIG`）的缓存、按间隔更新、`strict` 与更新后的自动重载（FR-CFG-05），远程 URL 的 `#!include`（FR-CFG-04）——，并给外部资源一个查看与手动更新的出口（FR-CFG-14：`GET /v1/external_resources`、`POST /v1/external_resources/update`）。连同阶段 1 / M5a 留给本计划的四件事（「承接事项」）：`[Rule]` 里几条 `FINAL` 的读法照手册改对（任务 0）、集合文件里没有用上的行说出来（任务 10）、API 不再让晚到的请求正文丢掉应答（任务 11）、称阶段 1"齐备"之前把它的需求对着代码再读一遍（任务 12）。

**做法**：下载、缓存、退避与条件请求沿用 `rurge-net` 的资源管理器，给它补上"采用之前先检查""可等待的一次更新""撤掉一个条目"三样；加载器自己从不联网，按 URL 指名的内容经一个 `RemoteSource` 从数据目录的缓存里取；Profile 的远程来源由一个随进程存续的 `ProfileSources`（`rurge-engine` 的新模块 `profile`）管，下载到的内容先当作候选把整份 Profile 加载一遍，加载得了才进缓存；`strict` 不满足时引擎进入"暂停"，拒绝新会话而不退出。rurge 不改写用户的文件：下载到的托管配置放在数据目录，本地文件始终是 Profile 的身份。不新增 crate。

**依据**：阶段 1 / M5 设计（`docs/superpowers/specs/2026-10-11-phase1-m5-addendum-design.md`）第 3 节与第 7 ～ 11 节，连同写本计划时加的第 13 节「M5b 计划期的订正」（与第 7 节不同的地方以第 13 节与本计划为准；第 12 节是 M5a 实施期的订正）。手册：`profile/managed-profile.html`、`profile/format.html`、`rules/final.html`、`tools/http-api.html`（2026-10-11 重读；设计引用的 31 句原话逐句对过页面的文本，一句不差；原话见设计与本计划的「计划期决定」）。

**基线**：阶段 1 / M5a 合并之后 `main` 上的全量门禁——71 行结果、2801 通过、0 失败、6 忽略（以合并后的那一次为准；一个测试二进制死于开发机的偶发故障时少一行结果，单独重跑它）。

## 全局约束

- 不新增 crate，`Cargo.lock` 里不出现新的包；本计划也不需要新的依赖边。
- 全工作区 `forbid(unsafe_code)`，`rurge-platform` 的两个例外不增加；MSRV 1.89，edition 2024。
- 凭据不进日志、错误文本、诊断、`Debug` 与 API 输出。托管配置与远程 include 的 URL 整个当作凭据（服务商发的链接带令牌）：除了 `scheme://host[:port]/…` 这个样子，它的用户名口令、路径与查询串不出现在任何输出里；诊断不引用配置里的取值。新的带凭据的写法写进 `crates/rurge-config/tests/secrets.rs` 的那份标记配置。
- 下载到的文本不能让 rurge 去动本机的东西：不启动本机的程序，远程 include 里的本地路径不读（任务 3；托管配置的副本里的本地 include 是登记过的例外）。
- 测试只用回环地址与端口 0，等待都有时限，不用固定的 sleep 当同步（"断言什么都没发生"的等待除外，注释写明等的是什么、余量多大）；不联网——**在线**跑起来的引擎或守护进程要把 GeoIP 的两个下载地址指到回环（引擎用例的 `geo_urls`，CLI 的 `--geoip-url` / `--geoip-asn-url`），否则会去取公网的库；不改系统设置（系统代理只经文件后端 `RURGE_SYSTEM_PROXY_BACKEND=file:<路径>`）。
- 拉起 `rurge` 的用例一律给 `--data-dir`（或 `RURGE_DATA_DIR`）指到临时目录：从任务 8 起，`check`、`rule match`、`dns lookup` 与三个控制命令会读数据目录里 Profile 来源的缓存，缺省的平台数据目录是开发机上真实的那一个。
- 拉起守护进程的用例先等它说出 `rurge <版本> running` 一行，再发信号、杀进程或做任何要它的处理器就位的事；"文件没有被写过"看的是字节与修改时间。
- 测试里手写的 HTTP 客户端（直接往 `TcpStream` 写请求的）每个请求一次写出请求头与正文；只有任务 11 那几条专门测"正文晚到"的用例分两次写。`crates/rurge/tests/cli.rs` 的 `api_call`、`http_get_with` 已经是一次写出，新用例用它们。
- 测试辅助用现成的，不另写一套：`crates/rurge/tests/cli.rs` 的 `spawn_command_with`、`spawn_command_reading_stderr`、`wait_for_stderr_line`、`free_tcp_port`、`forward_lines`、`standard_error_so_far`、`give_up`、`api_call`；`crates/rurge-engine/tests/common/` 的 `LogCapture`（`start()` 在用例自己的线程上，只用于当前线程的运行时）、`closed_tcp_port` / `closed_udp_port`、`UNTESTED`；`crates/rurge-api/tests/api.rs` 的 `api_seeded(rules, groups, seed)`（引擎建起来之前先往数据目录里放东西）。名字以合并后的代码为准，对不上的在报告里说。
- 诊断的建议（`Diagnostic.hint`，M5a）：每个新的诊断码必须落在 `diagnostic::tests` 的三张表之一（`WITH_DEFAULT`、`PER_SITE`、`WITHOUT`），`every_code_is_decided_once` 会因漏掉的而红；建议是 `&'static str`、一句不带句号的小写英文、不引用配置里的任何取值。给人看的打印（`print_diagnostics`）会多一行 `hint:`，断言整行 stderr 的用例要算上它。
- 不下载、不安装任何东西；只运行本仓库构建出来的二进制。
- Surge 语法就是 rurge 语法：本计划不给配置格式加任何键；rurge 专有的运行时选项只走命令行与环境变量（FR-CFG-17）；与 Surge 的每一处行为差异都进兼容性清单（任务 13）。rurge 不改写用户的配置文件（FR-CFG-02）。
- 从开发机上读到的东西不进仓库：设备名、硬件型号、用户名、家目录的路径都不写；说到这台机器只写"一台 Windows 11 x86-64 的开发机"。
- 注释、日志与错误文本用英文，照所在文件的写法；提交说明的标题用中文，不提任务编号、评审者与执行过程。
- 门禁：`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test -j 8 --workspace --no-fail-fast`（本机 `RUSTFMT` 指向 gnu 工具链的 rustfmt）。聚焦运行不用 `-p`（每种包的选法都另编一份依赖，写满过磁盘），用 `cargo test -j 8 --workspace --lib -- <过滤词>`、`--workspace --bin rurge -- <过滤词>`、`--workspace --test <名字> -- <过滤词>`。测试二进制无失败用例的异常退出（`STATUS_HEAP_CORRUPTION`）单独重跑那个二进制。

## 评审要点

计划的用例之外最可能咬到使用者的五种情形，各自归到拥有那段代码的任务里：

1. **服务端答了 200，内容却不是 Profile**——强制门户或报错页的 HTML、空的正文、截断的文件：不采用，缓存里上一份照旧生效，原因出现在 `/v1/external_resources` 的 `error` 与日志里。对托管配置，判据是"加载得了"（空文件与 HTML 都缺 `FINAL`）；对远程 include，另加一条"它被 include 的那个节它得有"——否则一页 HTML 会被当成一份空的规则表采用，规则悄悄全没了（任务 3 给出 `lacks_section`，任务 7 用它）。
2. **下载到的文本想动本机的东西**——远程 include 或托管配置的副本里写了 `external` 策略（启动本机程序）、远程 include 里写了本地路径的 `#!include`：前者不建出站、行为同 REJECT，后者不读，各一条 `W0033`；写在本地文件里的照旧（任务 3、4）。
3. **新版本提到一个从没下载过的来源**——服务端的新 Profile 多了一行远程 include，或把地址换到了别处：不能因此永远卡在旧版本，也不能半份半份地换（先换上缺着那一块的新版本，几秒后再补齐——其间规则是残缺的）。新版本先留着，新来源被跟踪并下载，试完第一次时一并采用；其间旧的一代照常运行，`strict` 不把这段等待当成失败（任务 7）。
4. **本地文件在上一次更新之后被改过**——它生效到下一次成功的检查为止；那次检查答的是"没有变化"（304）时同样结束本地文件的优先，并触发一次重载。只有一行 `#!MANAGED-CONFIG`、没有任何节的本地文件只是一个指针，从不优先。服务商把地址换过之后（副本里那一行指向别处），"上一次检查"看的是整条链上最新的那一次，本地文件那一行指着的旧地址失效了也不碍事（任务 4、6、8）。
5. **`strict` 的边界**——只有"到期"且"最近一次尝试失败（或离线）"才暂停，到期而还没来得及试的不算；系统时钟被拨回去不算到期；暂停时 API 照常、更新照常；一次成功的更新之后不重启就恢复，UDP 的目标在几秒内恢复而不必等客户端停下来；使用者把本地文件那一行的 `strict` 去掉就能脱身（任务 5、6、8、9）。

## 承接事项

阶段 1 / M5a 计划（`docs/superpowers/plans/2026-10-11-phase1-m5a-small-items-plan.md`）「延后事项」里去向写着本计划的六行，各自在这里的去处：

| M5a 延后事项 | 事项 | 在本计划里 |
| --- | --- | --- |
| 第 36 行 | 几条启用的 `FINAL` 之间写的规则仍被评估，手册的读法与此不同 | 任务 0（排在最前：远程 `#!include` 让"一个节里有两条 `FINAL`"从少见变成寻常，要在任务 3 之前改对） |
| 第 35 行 | 不读请求正文的端点与在读正文之前就被拒绝的请求，只凭请求头就答复并关掉连接；正文晚到的客户端可能读不到应答 | 任务 11 |
| 第 32 行 | 启动之后才载入或刷新的集合跳过了行，什么也不说 | 任务 10 |
| 第 31 行 | 集合文件的行上的未知参数没有人报告 | 任务 10 |
| 第 28 行 | 需求审计判成"做了"的条款没有人复核过；称"齐备"之前再对一遍需求，每条"做了"指到一条会因它而红的用例 | 任务 12 |
| 第 30 行 | 集合文件里的要求指令不是指令 | 不做：2026-10-11 重读手册 `profile/requirement.html` 与 `profile/managed-profile.html`，两页说的都是 Profile 的行（"a certain line of configuration"、"the beginning or end of a profile line"），对外部集合文件只字未提；去向仍是"有用户报告再说"（本计划延后事项第 19 行） |

M5a 另有一处读过手册之后已经登记、本计划只补一条用例的：通配命名节带来重名的节时 rurge 以 `W0020` 加载并取第一处定义（手册说 Surge 加载失败；兼容性清单 1.3 节已写明这是决定保留的差异）——任务 3 加一条"被 include 的文件带来重名的命名节"的用例，今天没有用例钉着这种情形。

---

## 文件结构

| 文件 | 任务 | 职责 |
| ---- | ---- | ---- |
| `crates/rurge-config/src/{config.rs,diagnostic.rs}`、`crates/rurge-rules/src/engine.rs` | 0 | `Config::first_final`；`W0019` 与 `W0021` 的新读法；规则表编译到第一条 `FINAL` 为止 |
| `crates/rurge-net/src/resource/{mod.rs,cache.rs}` | 1、2 | 种类、状态、磁盘上的副本；检查、可等待的更新、采用、撤销 |
| `crates/rurge-net/src/testing.rs` | 1 | 回环服务端不带校验器的路由；往缓存里放一份副本的测试辅助 |
| `crates/rurge-rules/src/{registry.rs,geoip_update.rs}`、`crates/rurge-engine/src/subscriptions.rs`、`crates/rurge-dns/src/resolver.rs` | 1 | 四个登记处各自填种类 |
| `crates/rurge-config/src/remote.rs`（新） | 3 | `RemoteSource`、`Remote`、`RemoteRef` |
| `crates/rurge-config/src/text/{mod.rs,include.rs}` | 3 | `Origin::Remote`；远程 include 的展开 |
| `crates/rurge-config/src/{config.rs,managed.rs}` | 3、4 | `LoadOptions.remote`、`SourceInfo`；托管配置的生效文本、间隔的下限 |
| `crates/rurge-config/src/{diagnostic.rs,spec/mod.rs}` | 3、4 | `W0011` 的新文本、`W0033`、`W0034`、`I0004`；`NotImplemented::DownloadedExternal` |
| `crates/rurge-engine/src/{engine.rs,udp.rs}` | 5 | 暂停；`config_text` 读生效文本 |
| `crates/rurge-engine/src/profile.rs`（新；长了就拆成 `profile/` 目录）、`src/stack.rs` | 6、7 | `ProfileSources`、磁盘视图；候选的判定与一并采用、启动时的引导；资源用的 HTTP 客户端抽成一个函数 |
| `crates/rurge/src/cli/{run.rs,check.rs,rule.rs,dns.rs,control.rs}` | 8 | 引导、主循环、各命令的磁盘视图 |
| `crates/rurge-api/src/{lib.rs,routes/resources.rs（新）}` | 9 | 两个端点 |
| `crates/rurge-rules/src/{set_format.rs,registry.rs}`、`crates/rurge-api/src/routes/resources.rs` | 10 | 集合文件里没有用上的行：计数、状态、诊断与日志；端点上的三项 |
| `crates/rurge-api/src/{lib.rs,auth.rs,linger.rs（新）}` | 11 | 连接的收尾：先关写端、把晚到的字节读掉、再关连接 |
| `docs/acceptance/phase1-acceptance.md`（新）；查出的缺口各自所在的文件 | 12 | 阶段 1 的需求逐句对照代码与用例 |
| 各 crate 的单元测试、`crates/rurge-engine/tests/{pipeline.rs,udp.rs,profile_sources.rs（新）,profile_updates.rs（新）}`、`crates/rurge-api/tests/api.rs`、`crates/rurge/tests/cli.rs`、`crates/rurge-config/tests/secrets.rs` | 0 ～ 12 | 用例 |
| 文档 | 13 | 兼容性清单、需求文档、API 参考、验收文档、手工清单、README、CLAUDE.md、设计的修正记录、本计划的两张表 |

---

## Task 0：`[Rule]` 里第一条 `FINAL` 之后的规则不再评估

**为什么在这份计划里、又为什么排在最前**：远程 `#!include` 让"一个节里有两条 `FINAL`"从少见变成寻常——手册的 Linked Profiles 就是把一整份托管配置的 `[Rule]` include 进本地的 `[Rule]`，而那份 `[Rule]` 以它自己的 `FINAL` 结尾。rurge 现在对这种写法的读法与手册不同（阶段 1 / M5a 重读手册时查出，登记在兼容性清单 3.1 节 `FINAL` 一行与 M5a 计划延后事项第 36 行），要在任务 3 之前改对。

**手册**（2026-10-11 读，原话）：

- `rules/final.html`："A FINAL rule always matches, so any rules written below it never take effect. If several enabled FINAL rules are present, the last one is the effective one. FINAL is not allowed inside rule set files or as a sub-rule of a logical rule."
- `profile/format.html`（Mixed Content and Includes）："In the `[Rule]` section, a `FINAL` rule immediately terminates rule matching. Rules included or defined after it are never evaluated, so check the position of `FINAL` in both the main profile and the included files."

**现在的行为**（M2 设计 4.2 与 6.5，当时只读到"多条 FINAL 取最后一条"这一句）：`Config::effective_final()` 是最后一条 `FINAL`；`RuleEngine::build`（`crates/rurge-rules/src/engine.rs`）编译到最后一条 `FINAL` 为止、跳过此前的每一条 `FINAL`，夹在两条 `FINAL` 之间的规则照常评估；此前的 `FINAL` 各一条 `W0021`（`this FINAL is shadowed; the last FINAL rule takes effect`），`W0019` 只数最后一条 `FINAL` 之后的规则。`FINAL,X` / `DOMAIN,a.com,A` / `FINAL,Y` 下，`a.com` 走 `A`。

**改成**（三句原话同时成立的唯一读法）：**匹配在第一条启用的 `FINAL` 处结束；所用的策略与选项（`dns-failed`）是最后一条启用的 `FINAL` 的。** 上例里 `a.com` 走 `Y`。哪一条 `FINAL` 的策略生效，两页合起来并没有说死（format 页只说匹配何时结束）；"the last one is the effective one" 是手册对它仅有的一句，照它。没有对着真实的 Surge 验证过——登记为"按手册的三句话推断"。受影响的只有带两条或更多启用的 `FINAL`、且它们之间夹着规则的 Profile：夹着的规则从生效变为不生效，`rurge check` 用 `W0019` 指出有几条、在哪里。

**文件**：`crates/rurge-config/src/config.rs`（`effective_final` 旁加 `first_final`；给出 `E0010`、`W0019`、`W0021` 的那段诊断；四条现有用例）、`crates/rurge-config/src/diagnostic.rs`（`W0021` 常量上的注释；它的缺省建议不变）、`crates/rurge-rules/src/engine.rs`（`build` 与它的注释；两条现有用例）、`crates/rurge/tests/cli.rs`（`rule match` 的一条）、`crates/rurge-config/tests/corpus/`（有两条 `FINAL` 的语料若存在，快照会变——逐个读）。

**接口**：

```rust
// rurge-config::config
impl Config {
    /// 给出策略与选项的那条 FINAL：最后一条（不变）。
    pub fn effective_final(&self) -> Option<usize>;
    /// 匹配结束的地方：第一条 FINAL。只有一条 FINAL 时与 effective_final 相同。
    pub fn first_final(&self) -> Option<usize>;
}
```

`RuleEngine::build`：编译 `cfg.rules[..first_final]`（其中不会有 `FINAL`），再接上 `cfg.rules[effective_final]` 作最后一条；`CompiledRule.index` 仍是它在 `cfg.rules` 里的下标。`final_pos`、`final_rule()`、`dns-failed` 的去处、`PreMatchingSet::extract` 与 `Engine::rules_view`（`GET /v1/rules`）都读编译后的表，随之而变，不另改。`Decision.matched` 在落到 `FINAL` 时仍是给出策略的那一条的下标（最后一条）。

**诊断**（文本不引用配置里的任何取值）：

- `W0019`（`{n} rule(s) after FINAL never take effect`，文本与缺省建议不变）：数的是**第一条** `FINAL` 之后不是 `FINAL` 的规则，位置是其中的第一条。夹在两条 `FINAL` 之间的规则从此算在里面。
- `W0021`：最后一条之前的每条 `FINAL` 各一条，文本改为 `this FINAL does not give the policy; the last FINAL rule does`（它不再是"被遮住"的——第一条仍然结束匹配）。缺省建议不变（``keep a single `FINAL` rule``）。
- `E0010`（没有启用的 `FINAL`）不变；被要求表达式关掉的 `FINAL` 不算 `FINAL`（现有用例 `final_rules` 的第三段，不变）。

**用例**（先红后绿；标"现有"的是改期望，其余是新加）：

- 现有 `dead_rules_after_last_final_are_counted`（`config.rs`）：`FINAL,DIRECT` / `DOMAIN,a,DIRECT` / `FINAL,REJECT` 现在**有** `W0019`（数 1、位置在那条 `DOMAIN` 规则上）也有 `W0021`；`FINAL,DIRECT` / `FINAL,REJECT` 只有 `W0021`。用例改名（它的名字说的是旧读法），注释里那句旧引文换成上面的两句原话。
- 现有 `last_final_takes_effect_and_earlier_final_is_shadowed`（`config.rs`）：`effective_final()` 不变，新加 `first_final()` 的断言，`W0019` 现在有。改名。
- 现有 `last_final_wins_and_shadowed_final_is_dropped`（`engine.rs`）：编译后的表少了夹着的那条；`a.com` 现在落到 `FINAL`（`Reason::Final`、`matched` 是最后一条 `FINAL` 的下标、策略是它的）；其余主机不变。改名。
- 现有 `rules_after_the_last_final_are_excluded`（`engine.rs`）、`rules_after_last_final_are_dead`、`final_rules`（`config.rs`）：期望不变（只有一条 `FINAL`），照跑。
- 新：三条 `FINAL`、各段之间都有规则——只有第一条之前的规则被评估，策略与 `dns-failed` 取自第三条（第一条带 `dns-failed` 而第三条不带时，DNS 失败是 `Outcome::DnsFailed`；反过来时是第三条的策略、`Reason::DnsFailedFallback`）。
- 新：第一条 `FINAL` 之后写了 `pre-matching` 的规则不进预匹配表（`RuleEngine::pre_match_domain` 与评估时的预匹配那一趟都不会命中它）。
- 新：本地 include 的文件以 `FINAL` 结尾、include 之后还有本地规则与本地的 `FINAL`（手册警告的正是这种写法）：include 之后的本地规则不评估，策略是本地那条 `FINAL` 的；`W0019` 的位置指向主文件里那条本地规则，`W0021` 的位置指向被 include 的文件里那条 `FINAL`。
- 新：两条 `FINAL` 各带一个互斥的要求表达式——只有一条启用，没有 `W0019` 也没有 `W0021`，行为与只写一条相同。
- 新（`crates/rurge/tests/cli.rs`）：`rurge rule match --explain` 对 `FINAL,X` / `DOMAIN,a.com,A` / `FINAL,Y` 的 `a.com` 说的是最后一条 `FINAL` 与策略 `Y`，轨迹里没有那条 `DOMAIN` 规则；`rurge check` 打印 `W0019` 与 `W0021` 两行连同各自的建议。
- 新（`crates/rurge-api/tests/api.rs`，用 `api_with` 给规则）：`GET /v1/rules` 不列出第一条 `FINAL` 之后的规则，最后一项是给出策略的那条 `FINAL`。

**判据**：`cargo test -j 8 --workspace --lib -- final`、`--workspace --test cli -- final`、`--workspace --test corpus` 全绿（快照有变化时逐个读、在报告里列出）；全量门禁绿；`git grep -n 'shadowed' -- crates` 不再有说旧读法的注释或文本。

---

## Task 1：资源管理器——种类、状态与磁盘上的副本

**文件**：`crates/rurge-net/src/resource/{mod.rs,cache.rs}`、`crates/rurge-net/src/testing.rs`；四个登记处 `crates/rurge-rules/src/registry.rs`（`compile_external`）、`crates/rurge-rules/src/geoip_update.rs`（`GeoUpdater::spawn`）、`crates/rurge-engine/src/subscriptions.rs`（`Subscriptions::register`）、`crates/rurge-dns/src/resolver.rs`（`watch_etc_hosts`）；`ResourceSpec` 在测试里的各个构造处。

**接口**（后面的任务照这些名字用）：

```rust
// rurge-net::resource
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResourceKind { RuleSet, DomainSet, PolicyGroup, GeoIp, Profile, Include, System }

pub struct ResourceSpec { /* 原有的 */, pub kind: ResourceKind }
pub struct ResourceOptions { /* 原有的 */, pub never_early: bool }        // 缺省 false

pub struct ResourceStatus {
    /* 原有的：source、state、version、fetched_at、next_refresh、last_error */
    pub kind: ResourceKind,
    pub label: Option<String>,
    pub key: String,       // resource_key(&source)
    pub local: bool,       // 来源是本地文件
    pub ready: bool,       // 手里有内容：available，或 failed 而留着上一份
    pub updating: bool,    // 一次下载在途
}
impl ResourceStatus {
    /// 对外怎么称呼它：有标签的是标签，没有的是不带用户名口令的来源（日志里用的同一种样子）。
    pub fn shown(&self) -> String;
}

/// `source.key()` 的 SHA-256，64 位小写十六进制；对 URL 来源它就是缓存目录的名字。
pub fn resource_key(source: &ResourceSource) -> String;

pub struct CachedCopy { pub data: Bytes, pub path: PathBuf, pub fetched_at: SystemTime }
/// 不经管理器、不联网地读一个 URL 在 `root` 下的缓存：正文、正文所在的文件（`…/resources/<key>/data`）、
/// 最近一次成功检查的时刻。两个文件缺一个或 `meta.json` 读不了时是 `None`。
pub fn cached_copy(root: &Path, url: &Url) -> Option<CachedCopy>;

// rurge-net::resource::Meta
pub struct Meta { /* 原有的 */, #[serde(default)] pub fetched_subsec_nanos: u32 }

// rurge-net::testing（`testing` 特性）
impl TestServer { pub fn set_validators(&self, path: &str, on: bool); }   // 关掉：这条路由不发 ETag，也不理会 If-None-Match
pub fn seed_cache(root: &Path, url: &Url, data: &[u8], fetched_at: SystemTime);
```

**行为**：

- **种类**：登记时给，同一个来源被登记几次时先到的留下（与标签相同）。四个登记处：规则集 `RuleSet`、域名集 `DomainSet`（`compile_external` 的 `SetKind` 参数）、`policy-path` 是 `PolicyGroup`、GeoIP 两个库是 `GeoIp`、系统 hosts 是 `System`。
- **`fetched_at` 的含义放宽**：状态里的 `fetched_at` 是"最近一次成功的下载或'没有变化'的应答"，只要管理器知道就给——`failed` 而留着缓存时也给（今天这种状态下是 `None`，`strict` 与 `updatedAt` 都要它）。从没成功过才是 `None`。
- **时刻到纳秒**：`Meta.fetched_at`（秒）旁边多一个 `fetched_subsec_nanos`，200 与 304 都写；启动时读回来的时刻与 `cached_copy` 给的时刻都是两者相加。旧的 `meta.json` 没有这一项，读作 0。为什么要：任务 4 拿它与本地文件的修改时间比先后，只到秒的话，同一秒里"先存文件、后下载"会被判成本地文件更新。
- **内容没变的 200 不算新版本**：正文与手里的逐字节相同时，照"没有变化"办——时刻前移、状态回到 `available`、不升版本、不发布、不记 `resource updated`——，应答带的校验器换掉存着的。不带 ETag / Last-Modified 的服务端（生成出来的托管配置常常如此）因此不会每个间隔都触发一次重载；对规则集也省掉一次无谓的重编译。
- **`never_early`**：为真时间隔的抖动只往后（0 ～ +10 %），不往前。手册说托管配置的 `interval` 是"the shortest time before an update may be triggered"。抖动函数改成吃种子的纯函数以便测试（今天的种子是当前时刻的纳秒数对 21 取余）。
- **`updating`**：一次尝试从发出请求到记下结果之间为真。
- **失败原因不带 URL**：`last_error` 里不出现 URL 的用户名口令、路径与查询串；现有的各种失败文本（`http status N`、`connect: …`、`tls: …`、`timeout`、`body exceeds N bytes`、`too many redirects`、`protocol: …`、`invalid url: …`）逐个核对，带的改掉——重定向的 `Location` 也算（它可以是一个带签名的地址）。
- `set_validators` 与 `seed_cache` 只在 `testing` 特性下；`seed_cache` 经真实的缓存写入代码，不另写一套格式。

**用例**（`rurge-net`，回环 `TestServer` 与现有的 `fast()` 选项）：

- 状态的新字段：一个 URL 资源的 `kind`、`label`、`key`（等于 `source.key()` 的 SHA-256，也等于缓存目录名）、`local == false`；`shown()` 对有标签的给标签、对没有标签而 URL 带用户名口令的给 `***` 的样子（找不到标记）；`ready` 从假到真；服务端应答被拖住（`set_delay`）时 `updating` 为真，结果记下之后为假；本地文件资源 `local == true`。
- 同一个来源先后以两种种类登记：留下第一种。
- 成功一次之后服务端改答 500：状态 `failed`，`fetched_at` 仍是那次成功的时刻，`ready` 为真，`last_error` 有值；从没成功过的：`fetched_at` 为 `None`，`ready` 为假。
- 304 之后 `fetched_at` 前移，`cached_copy` 给的时刻与状态里的相同（到纳秒）；手写一份没有 `fetched_subsec_nanos` 的 `meta.json` 读得回来。
- 内容没变的 200（路由关掉校验器）：第二次下载之后版本不变、订阅者没有被通知、`fetched_at` 前移、服务端被请求了两次；内容变了的照常升版本。关掉校验器而用 `set_header` 给一个 `etag` 时，内容没变的 200 之后下一次请求带的是新的 `If-None-Match`。
- 抖动：对全部 21 种种子，`never_early` 时结果不小于输入、不大于输入的 110 %；缺省时在 90 % ～ 110 % 之间。
- `cached_copy`：下载过的给出正文、`…/resources/<key>/data`、时刻；没下载过的是 `None`；`seed_cache` 放进去的经 `cached_copy` 原样读回，经管理器登记后是版本 1。
- 失败原因：URL 取 `http://user:S3CR3T@127.0.0.1:<端口>/S3CR3T/x?token=S3CR3T`，分别遇上 404、500、超长的正文、超时、重定向过多、重定向到一个解析不了而带标记的 `Location`：`last_error` 与告警日志里都找不到 `S3CR3T`。
- 四个登记处各一条：`rurge-rules`（规则集与域名集各自的种类；GeoIP）、`rurge-engine`（订阅）、`rurge-dns`（系统 hosts）的管理器状态里是各自的种类。

**判据**：`rurge-net` 的 `resource` 用例与上面四个 crate 的相关用例全绿；工作区里没有不带 `kind` 的 `ResourceSpec`。

---

## Task 2：资源管理器——采用之前的检查、可等待的更新、采用与撤销

**文件**：`crates/rurge-net/src/resource/mod.rs`。

**接口**：

```rust
pub type Check = Arc<dyn Fn(&[u8]) -> Result<(), String> + Send + Sync>;

impl ResourceManager {
    pub fn get_checked(&self, spec: &ResourceSpec, label: &str, check: Check) -> ResourceHandle;
    pub async fn update_now(&self, key: &str) -> Option<Result<(), String>>;
    pub async fn attempted(&self, key: &str);
    pub fn adopt(&self, source: &ResourceSource, data: Bytes) -> std::io::Result<()>;
    pub fn retire(&self, source: &ResourceSource) -> bool;
}
```

**行为**：

- **`get_checked`**：与 `get_labelled` 相同，另给条目一个检查（一个条目的第一个检查留下）。下载到的新正文——在"内容没变"的判断之后——先交给检查，检查在阻塞线程池上跑（`spawn_blocking`：它要把整份 Profile 加载一遍），通过了才写缓存、升版本、发布。没通过：什么也不写、不发布，存着的校验器不动，状态是 `failed`、`last_error` 是检查给的理由、上一份内容留着，照常退避；下一次尝试重新下载、重新检查。启动时从缓存读回来的内容不再检查（它进缓存时检查过）。检查里可以调用管理器的同步接口（`adopt`、登记别的来源、`statuses`）：管理器不在自己的锁里调用检查。
- **`update_now(key)`**：`key` 是 `resource_key`。没有这个条目：`None`。本地文件：踢一下重读，立刻 `Some(Ok(()))`。离线的管理器：立刻 `Some(Err(..))`（文本说明是离线）。URL：要求一次尝试，等到**在这次调用之后开始**的第一次尝试结束——调用时正在途的那一次不算——，结果是新版本、内容没变或"没有变化"时 `Ok(())`，否则 `Err(原因)`，文本与 `last_error` 相同。等待期间条目被撤掉或管理器没了：`Err`。它自己不设时限（一次尝试以 `fetch_timeout` 加检查的耗时为界），调用方另加。
- **`attempted(key)`**：等到这个条目"此刻没有一次该做而没做完的尝试"——登记以来已有一次尝试结束，或者它一开始读到的缓存还没到期。没有这个条目、本地文件、离线、已撤掉：立刻返回。
- **`adopt(source, data)`**：把一份内容当作刚下载到的写进缓存（时刻取现在，不带校验器，下一次刷新完整重取）；来源登记着时同时成为它的内容：升版本、`available`、发布。不经检查（调用方检查过）。写缓存失败时返回错误，条目不动。它是同步的，可以在检查里（阻塞线程上）调用。条目正等着退避时不必打断它：它至多再取一次，内容相同，按"没有变化"记。
- **`retire(source)`**：没有这个条目是 `false`。撤掉之后：`statuses()` 不再列它，它的任务结束（正等着下一次刷新的也立刻结束），正在途的一次尝试不再写缓存、不再发布；别处还拿着的句柄读到的是最后的内容；之后再登记同一个来源是一个从缓存读起的新条目。管理器的文档注释里"没有撤掉单个条目的接口"那一段随之改写。

**用例**：

- 检查通过：进缓存、升版本。检查不通过：缓存目录与之前逐字节相同（第一次下载时则不存在）、版本不变、`failed`、`last_error` 是检查的理由、`ready` 与之前相同；退避之后再试，检查被调用第二次；服务端换成好的内容后被采用。
- 检查不被调用的三种情形（用计数器）：304、内容没变的 200、启动时读缓存。
- 检查不占住异步线程：单线程运行时上，检查里阻塞 200 ms 的期间，另一个任务的定时器照常到期。
- 检查里登记另一个来源：那个来源的任务确实启动并下载到（检查跑在阻塞线程上，登记要找得到运行时）；检查里对另一个来源 `adopt`：不死锁。
- `update_now`：成功；失败时的文本与 `last_error` 相同；调用时一次尝试正在途（应答被拖住）、其间服务端换了内容——它在第二次请求结束后才返回，手里是新内容；不认识的键是 `None`；本地文件立刻返回且文件被重读；离线立刻给错误；等待期间撤掉条目给错误。
- `attempted`：第一次尝试失败之后也返回；缓存没到期的条目立刻返回；缓存已到期的等到那次刷新结束。
- `adopt`：登记着的来源——版本加一、内容与状态、缓存里的 `meta.json` 没有校验器（下一次请求不带 `If-None-Match`）；没登记的来源——只写缓存，之后登记读到它；写不进去（根目录是个文件）时返回错误、条目不动。
- `retire`：不再列出；之后不再有请求（"什么都没发生"的等待，余量写明）；在途的尝试被撤掉后缓存与版本都不变；重新登记从缓存读起。
- `adopt` 遇上这个条目自己在途的尝试：下载到的与采用的相同时版本只升一次；不同时再升一次，内容是下载到的那份。

**判据**：`cargo test -j 8 --workspace --lib -- resource` 全绿。

---

## Task 3：加载器——`RemoteSource` 与远程 `#!include`

**文件**：`crates/rurge-config/src/remote.rs`（新）、`src/lib.rs`、`src/config.rs`、`src/diagnostic.rs`、`src/text/{mod.rs,include.rs}`、`src/spec/mod.rs`、`crates/rurge-policy/src/registry.rs`（一处文本）、`crates/rurge-config/tests/secrets.rs`；`LoadOptions` 的五个字面量构造处（`crates/rurge/src/cli/{check,control,dns,rule,run}.rs`）先填 `remote: None`，任务 8 接线。

**接口**：

```rust
// rurge-config::remote
/// 加载器取 Profile 里按 URL 指名的内容的地方。加载器自己从不联网。
pub trait RemoteSource: Send + Sync { fn get(&self, url: &Url) -> Remote; }
pub enum Remote {
    Text { text: String, path: PathBuf, fetched_at: SystemTime },   // path：这份文本所在的文件，诊断的位置指向它
    Missing { why: Option<String> },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteRole { Managed, ManagedHop, Include }
#[derive(Clone, PartialEq, Eq)]
pub struct RemoteRef {
    pub url: Url,                // Debug 只显示 scheme://host[:port]/…
    pub role: RemoteRole,
    pub path: Option<PathBuf>,   // 这次加载拿到了它的文本时，文本所在的文件
    pub interval: Duration,      // 多久更新一次（下限见任务 4）
    pub strict: bool,            // 只有 Managed 可能为真
    pub lacks_section: bool,     // Include：它被 include 的某个节它没有
}
impl RemoteRef { pub fn found(&self) -> bool; }   // path.is_some()

// rurge-config::config
pub struct LoadOptions { /* 原有的 */, pub remote: Option<Arc<dyn RemoteSource>> }   // Debug 手写：只说有没有
pub struct SourceInfo {
    pub main: PathBuf,           // 传给 load 的本地文件（不变）
    pub includes: Vec<PathBuf>,  // 本地的 include（不变：只有本地文件）
    pub main_text: PathBuf,      // 主文本读自哪个文件；本任务里恒等于 main
    pub remote: Vec<RemoteRef>,  // 这次加载碰到的每个远程来源，按第一次出现的顺序，一个 URL 一项
}

// rurge-config::text
pub enum Origin { Main, Include(Arc<Path>), Module(String), Remote(Arc<Path>) }   // Remote：这一行来自下载到的文本

// rurge-config::diagnostic::codes
pub const W_REMOTE_INCLUDE_NOT_READ: &str = "W0011";    // 改名自 W_REMOTE_INCLUDE_UNSUPPORTED
pub const W_REMOTE_LOCAL_ACCESS: &str = "W0033";

// rurge-config::spec
pub enum NotImplemented { /* 原有的 */, DownloadedExternal }
```

URL 一律先经 `Url::parse`，此后只用解析出来的那个值（缓存目录的名字取自它的规范写法，加载器与资源管理器必须说的是同一个）。同一个 URL 先后以两种身份出现时记第一种。

**远程 include**（设计 7.2；手册 `profile/format.html`："The included profile can be a local file or a remote URL."、"The referenced file must contain the corresponding section declaration."）：`#!include` 的目标是 `http(s)://` 时——

- 解析不了：`W0012`（`the target of a remote include is not a valid URL`，不引用它），这一行不展开，不产生 `RemoteRef`。
- `LoadOptions.remote` 是 `None`：`W0011`，文本改为 `remote include not read: no data directory`。
- `Remote::Missing { why }`：这一行不展开，`W0022`（`remote include not downloaded yet`，有原因时接 `: <原因>`；不引用 URL），`RemoteRef { path: None, interval: 86400 秒, .. }`。
- `Remote::Text`：以 `path` 作位置里的文件、`Origin::Remote(path)` 作来源解析——经 Profile 自己的读取器，与本地 include 的文件完全相同：开头的 UTF-8 BOM 不算进第一行、整行注释与行尾注释、行首与行尾的要求指令都照 Profile 的规则——，然后照本地 include 的同一套规则展开——就地、通配命名节、缺节的 `W0017`（并把这个来源的 `lacks_section` 置真；通配节一个也没匹配到不算）、深度 8、总数 200（远程的与本地的一起数）、成环 `E0016`。环按 URL 判，文本是 `include cycle through a remote include`，不带 URL。
- 远程文本里再出现的 include：目标是 URL 的照上面办；是本地路径的不读，`W0033`（`a downloaded file cannot include a local file; the line is ignored`，建议 `include the local file from the local profile instead`）。本地文件（主文件或本地 include）里出现的远程 include 照常展开。"是不是在远程文本里"随递归传下去，不从条目的来源推断（任务 4 的托管配置副本是下载到的，却允许本地 include）。
- 更新间隔：被 include 的文本自己的头部有 `#!MANAGED-CONFIG` 时取它的 `interval`（经 `parse_managed`，诊断丢弃——被 include 的文件的头部一向是丢弃的），否则 86400 秒；它的 `strict` 与 URL 不看。
- `SourceInfo.includes` 的收集不变（只认 `Origin::Include`），所以缓存里的文件不会进 `--watch` 的监视表。

**下载到的文本里的 `external` 策略**：`[Proxy]` 里一行来自 `Origin::Remote` 的 `external` 定义不建出站。它留在 `Config.policies` 里（组与规则对它的引用照常成立），没有 spec，`Config.not_implemented[名字] == NotImplemented::DownloadedExternal`，每次加载一条 `W0033`（`` `external` policies written in downloaded text are not started; they behave as REJECT ``，建议 `define the policy in the local profile to use it`）——走现有的"没有 spec 的策略"那条路：REJECT、不参加测速、会话日志 `policy protocol not implemented: external (downloaded text)`（`NotImplemented::note()`）。`definition_entries` 因此要把条目的来源带出来。写在主文件或本地 include 里的 `external` 一切照旧。理由与登记见「计划期决定」P7。

**诊断的建议**（M5a 的三张表，`crates/rurge-config/src/diagnostic.rs` 的 `tests`）：

- `W0011`（常量改名 `W_REMOTE_INCLUDE_NOT_READ`）留在 `WITHOUT`；`codes::hint` 上方注释里它的那一句改为：只有没给加载器"下载到的文本从哪里取"的调用方才见得到它，`rurge` 的每个命令都给（任务 8 之后）。
- `W0033` 进 `PER_SITE`：它的两处各带各的建议（上面两句），没有缺省。
- 本任务的另外两条文本（`W0012` 的 `the target of a remote include is not a valid URL`、`W0022` 的 `remote include not downloaded yet`）用的是已在 `WITHOUT` 里的码，不另设建议。

**用例**（单元，`RemoteSource` 用一个内存里的假实现；标记配置）：

- 展开：`[Rule]` 里就地展开、顺序不变；`[Proxy]`；通配的 `[Ruleset *]`。
- 没拿到：`W0022`（带与不带原因两种）、这一行不展开、`RemoteRef` 的 `found()` 为假；`remote` 为 `None`：`W0011` 的新文本（改掉现有两处钉着旧文本的断言——`include.rs` 一条、`secrets.rs` 一处——与 `include.rs` 文件头那句"远程 include 在以后的里程碑做"的注释）；`http://` 这样解析不了的目标：`W0012`、没有 `RemoteRef`。
- 嵌套：远程里再 include 远程；远程里 include 本地路径——`W0033`，那个本地文件存在、里面的规则不在结果里；A → B → A 成环是 `E0016`，文本里没有 URL；深度与总数的上限；本地 include 里的远程 include 照常展开。
- `lacks_section`：被 include 的文本没有那个节——`W0017` 且该项为真；通配节没有匹配的不为真；同一个 URL 在两个节里各 include 一次是一项 `RemoteRef`，任一处缺节即为真。
- 间隔：被 include 的文本带 `#!MANAGED-CONFIG … interval=7200` 是 7200 秒，没有那一行是 86400 秒，它写的 `strict=true` 不进 `RemoteRef`。
- 位置：远程文本里一行有错时，诊断的文件是 `Remote::Text` 给的 `path`、行号是那份文本里的；`RemoteRef.path` 是同一个文件。
- `SourceInfo`：`includes` 里没有缓存的路径；`main_text == main`；`remote` 的顺序。
- 要求表达式照常作用于远程文本里的行（`#!MACOS-ONLY` 一类）；远程文本以 UTF-8 BOM 开头、行尾带注释时与本地 include 相同（BOM 不进第一行，注释不进取值）。
- 与任务 0 合在一起（手册警告的写法）：`[Rule]` 里 include 的远程文本以 `FINAL` 结尾，include 之后还有本地规则与本地的 `FINAL`——include 之后的本地规则不评估，策略是本地那条 `FINAL` 的，`W0019` 的位置在本地文件里，`W0021` 的位置在 `Remote::Text` 给的那个文件里。
- 重名的命名节（今天没有用例钉着被 include 的文件带来的重名）：主文件写了 `[Ruleset x]`，通配 include 的文件（本地的一条、远程的一条）也带一个 `[Ruleset x]`——`W0020`，生效的是主文件的那一个（被 include 的命名节排在主文件自己的各节之后）；两个被 include 的文件带同名的节——`W0020`，生效的是排在前面的那一个（读代码得出的是先 include 的那个：用例先确认再钉住，与此不同就在报告里说，不为它改行为）。
- `external`：远程文本里两条 `external` 策略——都在 `policies` 里、都没有 spec、`not_implemented` 是 `DownloadedExternal`、`W0033` 恰一条且带那句建议、引用它们的组照常加载；同样的行写在主文件里与写在本地 include 里照常有 spec。
- 凭据：include 的 URL 的用户名口令、路径、查询串各带标记，假实现给的文本里有带标记的口令——`Loaded` 的 `Debug`、全部诊断、脱敏输出里都找不到标记；`LoadOptions` 的 `Debug` 不显示来源里的内容。

**判据**：`rurge-config` 的单元测试、`--test secrets`、`--test corpus`（快照不应变化；变了逐个读）全绿；新诊断码在 M5a 的建议表里各有一项（建议，或写明不设建议的理由）。

---

## Task 4：加载器——托管配置的生效文本

**文件**：`crates/rurge-config/src/{config.rs,managed.rs,diagnostic.rs}`、`crates/rurge-config/tests/secrets.rs`。

**接口**：

```rust
// rurge-config::diagnostic::codes
pub const W_MANAGED_NOT_UPDATED: &str = "W0034";
pub const I_MANAGED_SOURCE: &str = "I0004";
```

`load` 与 `from_text` 的签名不变。`from_text` 没有文件可言，不做下面的选择：给它什么文本就解析什么（远程 include 照常）。`ManagedConfig.url` 仍是写出来的文本。

**`parse_managed`**：`interval=0` 读作没写（86400 秒，不告警）；1 ～ 59 按 60 并告警（`W0012`：`interval below 60 seconds; 60 is used`）。URL 必须解析成 `http(s)://`，否则 `W0012`（`the #!MANAGED-CONFIG URL is not an http(s) URL; the line is ignored`，不引用它）并当作没有这一行。

**主文本**（设计 7.2 与第 13 节；手册 `profile/managed-profile.html`："Surge can automatically update a profile from a URL."、"Ensure the new remote profile also includes the `#!MANAGED-CONFIG` line. Without it, the profile reverts to a standard profile."）。`load` 读到本地文件 L 之后：

1. L 的头部没有 `#!MANAGED-CONFIG`，或 `LoadOptions.remote` 是 `None`：主文本是 L，与今天相同。
2. 否则从 L 那一行的 URL 起**沿着缓存里的副本走到头**：向 `RemoteSource` 要当前的 URL——没有，停（这个 URL 是"头"，没拿到）；有，看这份副本自己的 `#!MANAGED-CONFIG`：没有那一行，停（"没有那一行"）；URL 就是当前这个，停（这个 URL 是"头"，拿到了）；换成了别的 URL，接着走。至多 8 跳；超过、或回到走过的 URL，停在手里的最后一份（"绕不出去"）。
3. **谁生效**：一份副本也没拿到时是 L。否则，L 自己的文本里至少有一个节、并且 L 的修改时间晚于链上各份副本里最新的那个 `fetched_at`、又不晚于现在——L 生效；其余情形（含 L 一个节也没有——它只是指向托管配置的一行——与修改时间在将来）——链上最后拿到的那份副本生效。
4. **被跟踪的是谁**（`RemoteRef`）：链上"头"之前的每个 URL 是 `ManagedHop`；"头"是 `Managed`。停在"没有那一行"时：副本生效的话没有 `Managed` 一项（那份副本的 URL 也是 `ManagedHop`）——Profile 变回了普通的 Profile，不再更新；L 生效的话那份副本的 URL 是 `Managed`（L 那一行说这份 Profile 是托管的，缓存说它现在住在那里）。停在"绕不出去"时最后那份的 URL 是 `Managed`。
5. **间隔与 `strict`**：L 生效时取 L 那一行的；副本生效时取指名"头"的那一行的（"头"拿到了，就是它自己那份副本里的那一行；没拿到，是指向它的那份副本里的那一行）。
6. **诊断**：
   - "头"没拿到：`W0022`（`managed profile not downloaded yet`，有原因时接 `: <原因>`）。
   - L 生效而有副本：`I0004`（`managed profile: the local file was changed after the last update; it is in effect until the next one`）。
   - 副本生效：`I0004`（``managed profile: the downloaded copy is in effect (`<缓存文件>`)``）；走过不止一个 URL 时另一条 `I0004`（``managed profile: the address in the local file leads to another one (see the #!MANAGED-CONFIG line of `<第一跳的缓存文件>`); the local file still names the old address``）。
   - 副本生效而它没有那一行：`W0034`（`the downloaded managed profile has no #!MANAGED-CONFIG line; it is no longer updated`，建议 `to keep it updated, the profile the address serves has to carry a #!MANAGED-CONFIG line`）。
   - 绕不出去：`W0034`（`the #!MANAGED-CONFIG lines of the downloaded profiles lead through too many addresses; the last copy found is in effect`，建议 `write the address the profile is finally served from into the #!MANAGED-CONFIG line of the local file`）。
   - 副本生效时，L 自己的文本的诊断不出现（它不是生效的 Profile）。
   - 建议表：`W0034` 进 `PER_SITE`（两处各带各的建议）；`I0004` 进 `WITHOUT`，归到注释里"they inform"那一句（第三句自己已经说出本地文件该改的是哪一行）；`parse_managed` 的两条 `W0012` 与没拿到时的 `W0022` 用的是已在 `WITHOUT` 里的码。
7. **副本怎样解析**：以缓存文件作位置里的文件、`Origin::Remote` 作来源；`Profile.main` 仍是 L（`SourceInfo.main`、include 的环检查与相对路径都按 L），`SourceInfo.main_text` 是缓存文件。副本里的本地 include 相对 L 所在目录解析、照常读（它们进 `SourceInfo.includes`），远程 include 照任务 3；副本里的 `external` 策略照任务 3 不建。
8. `Config.managed` 是生效的主文本自己的那一行（副本没有那一行时是 `None`）。

诊断不带时间戳（同一份输入每次给出同样的诊断；时刻在 `/v1/external_resources` 里）。

**用例**（单元；假的 `RemoteSource`；文件的修改时间用 `File::set_modified` 设）：

- `remote` 为 `None`：与今天逐项相同，没有 `RemoteRef`。
- 没拿到：L 生效、`W0022`（带原因）、`RemoteRef` 的各项（间隔与 `strict` 取 L 那一行的）、`main_text == main`。
- 副本比 L 新：只在副本里的一条规则在结果里；`I0004` 带缓存文件的路径；`main_text` 是缓存文件而 `source.main` 是 L；副本里一行有错时诊断的位置在缓存文件里；`Config.managed` 与 `RemoteRef` 的间隔、`strict` 取自副本那一行；L 里一行有错时不报。
- L 比副本新：L 生效、`I0004` 的另一句、`RemoteRef` 的 `found()` 为真而间隔与 `strict` 取 L 那一行的（副本那一行写 `strict=true`、L 那一行不写——结果是假）。L 的修改时间在将来：副本生效。L 没有任何节而比副本新：副本生效。
- 副本没有那一行：副本生效、`W0034`、只有 `ManagedHop`、`Config.managed` 为空；这时 L（有节）比副本新：L 生效、那个 URL 是 `Managed`。
- 换地址：U → U′，U′ 没拿到（主文本是 U 的副本、`W0022`、U′ 是 `Managed` 而没拿到、间隔取 U 的副本那一行的）与拿到（U′ 的副本生效、两条 `I0004`、U 是 `ManagedHop`）；A → B → A；十个地址的链停在第 8 跳。
- 评审要点 4（换过地址之后的先后）：L 比 U 的副本新、比 U′ 的副本旧——副本生效；L 比两份都新——L 生效，被跟踪的仍是 U′（`Managed`），间隔与 `strict` 取 L 那一行的，U 是 `ManagedHop`。
- 副本里的本地 include 相对 L 的目录读到、在 `SourceInfo.includes` 里、没有 `W0033`；副本里的远程 include 展开；副本里的 `external` 不建（`W0033`），副本的本地 include 里的 `external` 照建。
- 副本是一页 HTML：加载结果有错误（缺 `FINAL`）——采用与否不是加载器的事，这里只钉住"它加载不了"。
- `parse_managed`：`interval=0`、`interval=59`、`interval=60`；不是 `http(s)` 的 URL。
- `from_text`：文本带那一行、`remote` 里有副本——解析的仍是给它的文本，没有 `Managed` 一项。
- 凭据：L 与副本的 `#!MANAGED-CONFIG` 的 URL 带标记、副本里有带标记的口令——`Debug`、诊断、脱敏输出里都找不到。

**判据**：`rurge-config` 的单元测试、`--test secrets`、`--test corpus` 全绿（`kitchen-sink` 的快照里托管那一行 `interval=3600`，不应变化）；新诊断码在建议表里各有一项。

---

## Task 5：引擎——暂停与生效文本

**文件**：`crates/rurge-engine/src/{engine.rs,udp.rs,lib.rs}`；用例在 `crates/rurge-engine/tests/{pipeline.rs,udp.rs}`。

**接口**：

```rust
// rurge-engine
pub const REFUSED_STRICT: &str = "refused: the managed profile requires an update (strict)";
impl Engine {
    pub fn suspend(&self, why: Option<String>);     // Some：进入暂停；None：结束
    pub fn suspended(&self) -> Option<String>;
}
```

**行为**（设计 7.4）：暂停的状态放在 `Engine` 上、跨重载（与 `mode`、`global_policy` 同处）。暂停期间四个入口——`Dialer::dial`、UDP 流的 `open`、`dial_internal`、`dial_internal_udp`——在会话的句柄建好之后、选策略之前拒绝：会话以失败结束，请求记录的 `error` 是 `REFUSED_STRICT`，没有 `rule`。TCP 用现有的 `fail(handle, FailKind::Other, …)`（HTTP 客户端看到 502 的错误页、文本里有这句话，SOCKS5 是 `0x05`）；UDP 用 `failed(handle, …)` 而不是 `Rejected`——失败的流 5 秒后重试，暂停一结束目标就恢复，被拒绝的流要等客户端停下来 60 秒才放手。监听端、API 与测速不受影响。`why` 只进日志与 API，不进会话的错误文本。

`Engine::config_text` 改读 `source.main_text`（托管配置的副本生效时，`GET /v1/profiles/current` 返回的是副本）；`profile_path()` 与 `profile_key` 不动，仍是本地文件。

**用例**：

- `pipeline.rs`：暂停时 HTTP `CONNECT` 得 502、正文里有 `REFUSED_STRICT`；明文 HTTP 同；SOCKS5 的应答码是 `0x05`；目标从未被访问；请求记录的 `error` 是那句话、没有 `rule`。`suspend(None)` 之后同样的请求照常通过。暂停跨过一次 `swap_runtime`。跟随出站模式下的内部 DNS 会话（TCP 载体与 QUIC 载体各一）在暂停时以这句话失败。
- `udp.rs`：暂停时数据报被丢、记录的 `error` 是那句话；客户端每 500 ms 发一次不停，`suspend(None)` 之后有时限地（8 秒）等到回包。
- `config_text`：一代的 `main_text` 与 `main` 不同时返回前者的文本（经假的 `RemoteSource` 加载一份有副本的 Profile），脱敏照旧。

**判据**：`--test pipeline`、`--test udp` 全绿。

---

## Task 6：`rurge-engine::profile`——来源的跟踪、等待与 `strict`

**文件**：`crates/rurge-engine/src/profile.rs`（新）、`src/lib.rs`、`src/stack.rs`；用例在 `crates/rurge-engine/tests/profile_sources.rs`（新）与模块的单元测试。

**接口**：

```rust
// rurge-engine::profile
pub struct SourcesOptions {
    pub profile: PathBuf,              // -c 给的本地文件
    pub load: LoadOptions,             // 环境、平台、能力（任务 7 的判定用；它的 remote 不看）
    pub data_dir: PathBuf,
    pub offline: bool,
    pub client: Arc<HttpClient>,
    pub resources: ResourceOptions,    // 退避、超时；offline 与 never_early 由 ProfileSources 自己定
}
pub struct ProfileSources { /* 一个随进程存续的 ResourceManager、当前跟踪的来源 */ }
pub struct Suspended { pub since: SystemTime, pub why: String }

impl ProfileSources {
    pub fn new(opts: SourcesOptions) -> Arc<ProfileSources>;
    pub fn production(profile: &Path, load: &LoadOptions, data_dir: &Path, offline: bool) -> anyhow::Result<Arc<ProfileSources>>;
    pub fn remote(self: &Arc<Self>) -> Arc<dyn RemoteSource>;
    pub fn track(&self, sources: &SourceInfo);
    pub async fn settle(&self, limit: Duration) -> bool;
    pub async fn changed(&self);
    pub fn outdated(&self) -> bool;
    pub fn strict(&self) -> Option<Suspended>;
    pub fn statuses(&self) -> Vec<ResourceStatus>;
    pub async fn update(&self, key: &str) -> Option<Result<(), String>>;
}
pub fn disk_remote(data_dir: &Path) -> Arc<dyn RemoteSource>;

// rurge-engine::stack
pub fn resource_client() -> anyhow::Result<Arc<HttpClient>>;   // build_stack_with 里建的那个客户端，抽出来两处共用
```

**行为**：

- **`disk_remote`**：`rurge_net::resource::cached_copy` 的一层皮。正文必须是 UTF-8（本地文件一向如此），不是时 `Missing { why: Some("the downloaded file is not UTF-8 text") }`；没有缓存 `Missing { why: None }`。**`remote()`** 相同，另在没有内容时带上管理器记下的失败原因。
- **`production`**：`resource_client()`（它的 User-Agent 带 `Surge-compatible`，服务商的面板凭它给 Surge 的格式）、退避上限 600 秒（缺省的 3600 秒下，`strict` 暂停之后网络恢复了可能还要等一小时）、`never_early`。
- **`track`**：把这一代的来源（`SourceInfo.remote` 里 `Managed` 与 `Include` 两种；`ManagedHop` 只读缓存、不刷新）登记进管理器（`get_checked`，种类 `Profile` / `Include`，间隔取 `RemoteRef.interval`），不再出现的 `retire`；已跟踪而间隔变了的撤掉重登（管理器对同一个条目只会把间隔往短里并）。本任务里的检查只要求正文是 UTF-8；任务 7 换成真正的判定。标签：`managed profile (<scheme>://<host>[:port]/…)`、`remote include <key 的前 8 位> (<scheme>://<host>[:port]/…)`——括号里的样子与 `ResourceSource` 的 `Debug` 相同（`rurge_net::resource` 里现成的那个函数，按需公开）；`key` 是 `resource_key`，它的前 8 位让同一台主机上的几个 include 在日志里分得开，也对得上 `/v1/external_resources` 里的那一项（设计 7.3 写的是序号：序号随 Profile 的改动而变，而一个条目的标签在它登记时就定了）。记下这一代是不是"本地文件生效而副本存在"（`main_text == main` 且 `Managed` 一项拿到了），供 `outdated` 用。
- **`settle(limit)`**：等到每个还没有内容的来源、以及到了期的 `strict` 来源试完一次（`attempted`），至多 `limit`；都等到了返回真。到期而不是 `strict` 的不等（后台刷新）。等待期间新登记的来源也等（任务 7 的判定会登记新的）。
- **`changed()`**：被跟踪的任一来源发布了新版本之后返回，从最后一次变化起静默 1 秒才返回（一并采用的几个来源只触发一次）。登记时读到缓存不算变化，撤掉重登也不算。两条要求来自它的用法——主循环的 `select!` 里另有每秒一次的定时器，这个 future 每秒都会被丢掉重建：（一）可取消，去抖的截止时刻记在 `ProfileSources` 里而不在 future 里，否则静默期永远数不满；（二）不丢变化，它返回之后、下一次调用之前发生的变化（重载进行时来的新版本），下一次调用照样报告——记的是"上一次报告时各来源的版本"，不是一个通知。
- **`outdated()`**：这一代是"本地文件生效而副本存在"、而 `Managed` 那个来源最近一次成功的检查已经晚于本地文件的修改时间（一次"没有变化"的应答就会这样）时为真——对同一个检查时刻只真一次（重载失败时不会每秒再来）。
- **`strict()`**（设计 7.4；手册："If `strict` is true, Surge requires a successful update after the interval arrives."）：三条同时成立时给出 `Suspended`——这一代 `Managed` 一项的 `strict` 为真；距它上一次成功的检查已不少于间隔（从没成功过时从本地文件的修改时间起算；上一次检查的时刻在将来——时钟被拨回——时不算到期）；最近一次尝试失败（管理器里这个来源是 `failed`）或运行在离线下。`since` 是到期的时刻，`why` 是失败的原因或离线的说明。
- 日志与状态里不出现 URL 的路径、查询串与用户名口令。

**用例**（`profile_sources.rs`：回环 `TestServer`、临时目录、缩短了退避的 `ResourceOptions`、`seed_cache`；`SourceInfo` 由 `rurge_config::config::load` 加载真实的文件得到）：

- 磁盘视图：读到 `seed_cache` 放进去的；不是 UTF-8 的是 `Missing` 带原因；没有的是 `Missing { why: None }`。`remote()`：被跟踪而服务端答 404 的来源带 `http status 404`。
- 第一次下载：`track` 一个没有内容的托管来源，`settle` 之后缓存里有它，`changed()` 返回。
- `changed()`：静默期内的两次变化只返回一次；登记有缓存的来源不返回；间隔变了而撤掉重登不返回；每 200 ms 把它丢掉重建一次，它仍在最后一次变化之后约 1 秒返回（有时限地等，3 秒）；它返回之后、再次调用之前来的新版本，再次调用时报告。
- `track`：`ManagedHop` 不被登记（没有请求）；下一代不再出现的来源不在 `statuses()` 里、不再有请求；间隔从 86400 改成 60 之后，过期的副本随即被刷新。
- 评审要点 4：本地文件比副本新的一代被 `track` 之后，一次答 304 的更新让 `outdated()` 为真恰一次；`track` 下一代之后回到假。
- 评审要点 5（`strict()` 的每一条）：没写 `strict`；写了、到期、失败（`since` 与 `why`）；写了、到期、成功；写了、没到期、失败；离线而到期；从没成功过时从本地文件的修改时间起算；副本的检查时刻在将来。到期用 `seed_cache` 放一份两小时前的副本、间隔 60 秒来造。
- `settle`：没有可等的立刻返回；等到缺内容的来源第一次尝试结束（成功与失败各一）；到期而不是 `strict` 的不等（服务端拖 2 秒，`settle` 先返回）；到期的 `strict` 来源等；到时限返回假。
- `update`：不认识的键是 `None`；成功与失败照管理器的结果。
- 凭据：URL 的用户名口令、路径、查询串带标记——`statuses()` 的 `Debug`、标签、捕获到的日志里都找不到。

**判据**：`--test profile_sources` 与模块的单元测试全绿；`build_stack_with` 换用 `resource_client()` 之后现有用例不变。

---

## Task 7：`rurge-engine::profile`——采用之前的判定与启动时的引导

**文件**：`crates/rurge-engine/src/profile.rs`；用例在 `crates/rurge-engine/tests/profile_updates.rs`（新）与模块的单元测试。

**接口**：

```rust
// rurge-engine::profile
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boot { No, Wait, Refresh }
/// 启动时的第一次加载之后要不要先等一轮下载。
pub fn boot_need(loaded: &Loaded, data_dir: &Path) -> Boot;
pub struct BootLimits { pub round: Duration, pub total: Duration, pub rounds: usize }   // 缺省 30 秒、60 秒、8 轮
impl ProfileSources {
    pub async fn bootstrap(self: &Arc<Self>, first: Loaded, need: Boot, limits: BootLimits) -> Result<Loaded, LoadError>;
}
```

**判定**（设计 7.3、M5-D4，订正见第 13 节）。"加载"指 `load_checked(本地文件, 选项)`，选项的 `remote` 是磁盘视图上盖一层：候选的 URL 答候选的文本（`fetched_at` 取现在——采用之后正是如此——，`path` 取它将来所在的缓存文件）。对一组候选的一次加载有三种结论：

- **不通过**：有错误诊断——理由 `does not load: <第一条错误的文本>`（错误的位置在某个候选里时接 ` (line <行号>)`）；本地文件此刻读不了（`LoadError`）同样，理由取它的文本；或者某个候选的 URL 不在这次加载的 `source.remote` 里——`the profile no longer refers to this address`；或者在而 `lacks_section`——`lacks the section it is included into`。候选不是 UTF-8 时不必加载，直接不通过。
- **等待**：加载得了，但这次加载里有没拿到内容的来源还没试完第一次——还没被跟踪的随即登记（到下一次 `track` 为止；`track` 在还留着候选时不撤它们）。理由 `waiting for the sources this version refers to`。
- **通过**：其余。没拿到而已经试过（失败了）的来源不拦着——那一行照 `W0022` 不展开。

`get_checked` 的检查（串行，一把锁）对 URL u 的候选 c：

1. 另有留着的候选时，先带上全部留着的与 c 一起加载；通过：把其余的逐个 `adopt`，清空留着的，接受 c。
2. 否则只带 c 加载；通过：接受 c。
3. 都没通过：c 留着（每个 URL 只留最新的一份，只在内存里），返回失败，理由是最后那次加载的。

留着的候选只在它的来源仍是"上一次尝试没被采用"（管理器里是 `failed`）时有效：那个来源后来又答了"没有变化"（304，或与缓存里那份相同的 200——服务端退回了旧版本）而回到 `available` 时，留着的那份已不是服务端现在给的内容，作废。每次判定之前、`settle` 返回之前先把这样的清掉。

`settle` 在返回之前，手里有留着的候选时把它们作为一组再判一次（它们等的来源这时都试过了），通过就全部 `adopt`。平时不另设定时的重判：留着的候选在它自己那个来源的下一次重试（退避之后）时再判。

`strict()` 多一条：`Managed` 那个来源的候选正因"等待"而留着时，不算"最近一次尝试失败"——内容是下载到了的，没有采用是在等它提到的来源。

**启动时的引导**（设计 7.5、M5-D9）：

- `boot_need`：没有远程来源——`No`。有一条错误诊断的位置在下载到的文件里（某个 `RemoteRef.path`）——`Refresh`（生效的副本或某个远程 include 的缓存加载不了：一次更新也许换得来能用的版本；错误全在本地文件里的不算，那是使用者自己要改的）。有没拿到内容的来源，或者 `Managed` 一项是 `strict` 而已到期（照 `strict()` 的算法，时刻读磁盘缓存）——`Wait`。其余——`No`。
- `bootstrap`：每轮先 `track` 手里这次加载的来源；第一轮在 `Refresh` 时把每个来源 `update` 一遍，其余时候 `settle`（每轮至多 `round`）；然后重新加载（在阻塞线程池上，`remote` 用 `self.remote()`）。重新加载的结果没有错误、也没有还没被跟踪的来源时结束；一轮什么也没改变（没有新登记的来源，也没有哪个来源换了内容）时也结束，不空转到上限；至多 `rounds` 轮、合计至多 `total`。返回最后一次加载的结果（可以带着错误——由调用方决定怎么办）。

**用例**（`profile_updates.rs`）：

- 评审要点 1：服务端给 HTML / 空正文 / 缺 `FINAL` 的 Profile——不进缓存，`statuses()` 里是 `failed` 与理由，`changed()` 不返回（"什么都没发生"的等待），`remote()` 给的仍是上一份；远程 include 被换成 HTML——`lacks the section it is included into`，上一份照旧。
- Profile 已不提这个地址（本地文件改掉了那一行、还没重载）：候选不采用，理由是那一句。
- 评审要点 3：
  - 两个已跟踪的来源必须一起换（托管配置 v2 用到只有 include v2 才定义的策略，而 include v2 不再定义 v1 的组用到的策略）——各自更新时都不通过，后一个到达时两个一并采用，`changed()` 只返回一次，之后加载的结果里是 v2 的策略。
  - 托管配置 v2 多了一行从没下载过的远程 include，没有它 v2 也加载得了——v2 不立刻采用（理由是等待的那一句），新来源出现在 `statuses()` 里，它下载到时两个一并采用，`changed()` 只返回一次；其间 `remote()` 给的一直是 v1。
  - 同上而新来源的服务端答 404——托管来源下一次重试时 v2 被采用，加载的结果里那一行带 `W0022`。
  - 托管配置换地址（v2 的那一行指向 U′）——U′ 被登记并下载，两份一并采用；之后的一代里 U 是 `ManagedHop`、不再被请求。
  - 等待期间一次不相干的 `track`（同一代的来源）不撤掉为候选登记的来源。
  - 留着的候选作废：托管配置 v2 因等待而留着（新来源的服务端拖着不答），服务端退回 v1（下一次重试得 304），之后新来源到达——v2 不被采用，`remote()` 给的仍是 v1，那个来源的状态是 `available`。
- 本地文件在判定时读不了（用例里把它换成一个目录）：候选不采用，理由里是读文件的错误；本地文件恢复后的下一次重试照常采用。
- 评审要点 5：到期的 `strict` 来源，新版本因等待而留着——`strict()` 是 `None`；新版本加载不了——`strict()` 给出 `Suspended`，`why` 是不通过的理由。
- `boot_need` 的每一条：没有远程来源；缺内容；`strict` 到期与没到期；错误在本地文件里（`No`）；错误在副本里（`Refresh`）；错误在远程 include 的缓存里（`Refresh`）。
- `bootstrap`：
  - 缓存为空、只有一行的本地文件、副本提到两个远程 include、其中一个再 include 第三个——一次 `bootstrap` 之后加载没有错误、四份都在缓存里。
  - 服务端答 500——返回的加载带着错误与 `W0022`（原因 `http status 500`），一轮之后就返回（限时给得很长，断言耗时远小于它）。
  - `Refresh`：副本加载不了、服务端换了能用的版本——之后加载通过；服务端仍是那份——带着错误返回，不空转。
  - 被 include 的文件拖 2 秒才答——`bootstrap` 等到它。
  - 新来源答 404 的那种——`bootstrap` 结束时新版本已被采用（`settle` 末尾的重判）。
- 凭据：不通过与等待的理由、日志里都找不到 URL 里的标记。

**判据**：`--test profile_updates`、`--test profile_sources` 全绿。

---

## Task 8：可执行文件——引导、主循环、其余命令

**文件**：`crates/rurge/src/cli/{run.rs,check.rs,rule.rs,dns.rs,control.rs}`；用例在 `crates/rurge/tests/cli.rs`。

**各命令的 `LoadOptions.remote`**（设计 7.5，订正见第 13 节）：

- `check`、`rule match`、`dns lookup`：`disk_remote(数据目录)`——看到的与运行中的实例一致，自己不为 Profile 联网。
- `reload` / `stop` / `status -c`：`ControlArgs` 加 `--data-dir`（环境变量 `RURGE_DATA_DIR`，缺省是平台的数据目录），同样用磁盘视图。它们读 Profile 是为了 `[General] http-api`，托管配置生效时这一项在副本里。
- `service install` 不加载 Profile，不动。
- `run`：第一次加载用磁盘视图；`ProfileSources` 建好之后换成 `sources.remote()`（重载与 API 的 `profiles/check` 都用它，`W0022` 因此带得上失败的原因）。

**启动**（`run()`；今天的顺序是 读监听覆盖 → 加载 → 套监听覆盖 → 日志 → 数据目录 → 实例锁）：

0. `Overrides::parse`（M5a 的 `--listen-http` / `--listen-socks5`）照旧在最前，读不出来的那条退出不变。套覆盖的 `apply(&mut cfg, &overrides)` 会失败（覆盖落在某条 `external` 策略的 `local-port` 上）：它只对**最后用来启动的那一次加载**做——引导之前的那一次加载只用来判断要不要引导，不套覆盖（托管配置生效时策略在副本里，那一次看到的可能还不是最后的策略表）。它失败与"加载有错误"同样处理（下面第 2、4 步各自的退出方式）。重载里的 `apply` 不变。
1. 用磁盘视图加载一次（数据目录在加载之前就定得下来：`RuntimeArgs::data_dir()`）。
2. `--no-network`，或 `boot_need` 是 `No`：与今天完全相同（有错误，或套覆盖失败：打印、恢复崩溃留下的系统代理、以 2 退出，不创建数据目录）。
3. 否则引导，位置在这里——日志之前：建数据目录、拿数据目录的实例锁（下载要写缓存，不能与另一个实例同时写；被别的实例占着时以 1 退出，与今天相同），这把锁一直拿到进程结束，后面不再拿第二次；往标准错误写一行 `waiting for the profile's remote sources (at most 60 s)`；在一个临时的运行时里建 `ProfileSources::production` 并 `bootstrap`，然后把这个运行时连同它的任务收掉。引导期间没有日志：下载失败的原因经 `W0022` 的文本给出。
4. 引导之后的加载仍有错误，或加载通过而套覆盖失败：打印诊断（或那一行 `error: --listen-…`），恢复崩溃留下的系统代理，以 2 退出。这时锁已经在手里，不能再经 `recover_without_a_log`（它自己去拿锁，会拿不到）：把那个函数拆成"拿锁"与"拿到锁之后做的事"两半，后一半两处共用。之后 `--log-file` 打不开的那条退出同样处理。
5. 主运行时里另建一个 `ProfileSources`（引导用的已经结束，磁盘缓存把一切带过来），`opts.remote = sources.remote()` 在 API 的上下文建起来之前，`track` 当前这一代，`engine.suspend(sources.strict() 的原因)` 在监听端绑定之前。

**运行**：主循环的 `select!` 多两路——`sources.changed()`：与文件监视、SIGHUP、API 的重载走同一个 `reload_and_refresh`；每秒一次的定时器（错过的不补）：`sources.strict()` 与引擎的状态不同时改引擎的状态并记日志（进入是 WARN：`managed profile: the update that strict requires has not succeeded; new sessions are refused until it does`，带 `error`；恢复是 INFO：`managed profile: updated; new sessions are accepted again`），`sources.outdated()` 为真时重载。每次重载成功之后 `track` 新一代的来源（在配置被移进新的运行时之前取出 `source`）。重载失败保留旧配置（现有行为）。`--watch` 只看本地文件（`SourceInfo.includes` 里没有缓存的路径）。

**用例**（`cli.rs`，真实拉起 `rurge`，全部回环；新的辅助 `rurge_run_online`：不带 `--no-network`，带 `--data-dir`、文件后端的系统代理、`--geoip-url` 与 `--geoip-asn-url` 指到回环服务端）：

- 缓存为空、Profile 的 `[Proxy]` 与 `[Rule]` 各有一行远程 include：启动成功，标准错误里有等待的那一行，被 include 的策略与规则生效，缓存里有两份；Profile 文件与启动前逐字节相同。
- 只有一行 `#!MANAGED-CONFIG` 的本地文件：启动成功，`GET /v1/profiles/current` 是副本的文本；本地文件逐字节不变。之后 `rurge check -c <本地文件> --data-dir <目录>` 以 0 退出并打印 `I0004`；`rurge status -c <本地文件> --data-dir <目录>` 找得到 API（`http-api` 只写在副本里）。
- 同样的本地文件、服务端答 500：以 2 退出，stderr 里有 `W0022` 与 `http status 500`；URL 的用户名口令、路径、查询串带标记，stdout 与 stderr 里都找不到。
- 完整的本地 Profile 带那一行、服务端答 500：照常启动，本地文件生效。
- `--no-network` 而远程 include 不在缓存里：照常启动，`W0022`，没有等待的那一行。
- 评审要点 5：缓存里一份两小时前的副本（`seed_cache`），它的那一行是 `interval=60 strict=true`，服务端答 500——启动成功，经代理的请求得 502、正文里有 `REFUSED_STRICT`，`GET /v1/outbound` 照常。
- 被 include 的文件拖 3 秒才答：启动等到它（有时限地等到 `running` 一行），不是带着 `W0022` 起来。
- 引导之后仍加载不了的启动：崩溃留下的系统代理被恢复（文件后端），以 2 退出；另一个实例占着数据目录时以 1 退出、不下载（服务端没有被请求）。
- 使用者改坏了本地文件（本地文件比副本新、有一行错误）：以 2 退出、报的是本地文件的错误，服务端没有被请求——不拿一次更新去盖掉使用者自己的错误。

**判据**：`--test cli` 全绿；`rurge run` 在没有远程来源的 Profile 上的启动顺序与输出不变（现有用例不改）。

---

## Task 9：外部资源的两个端点与经它们的端到端用例

**文件**：`crates/rurge-api/src/lib.rs`、`src/routes/resources.rs`（新）、`crates/rurge/src/cli/run.rs`（`ApiContext` 多一项）；用例在 `crates/rurge-api/tests/api.rs`、`crates/rurge/tests/cli.rs`。

**接口**：

```rust
// rurge-api
pub struct ApiContext { /* 原有的 */, pub sources: Option<Arc<ProfileSources>> }
```

**端点**（手册 `tools/http-api.html`，Mac 6.10 / iOS 5.23；原话见设计 7.6）：

- `GET /v1/external_resources` → `{"defines":[…]}`。先是 `ProfileSources` 的（托管配置、各个远程 include），再是当前这一代资源管理器的（按 `path` 排序）；`System` 种类不列。每项：`path`——`ResourceStatus::shown()`：有标签的用标签（订阅、GeoIP、Profile 的来源都有，URL 不外露），没有的是不带用户名口令的来源；`type`——`ruleset`、`domainset`、`policy-group`，另加 rurge 自己的 `geoip`、`profile`、`include`；`key`；`local`；`ready`；`updatedAt`——最近一次成功检查的 UNIX 秒数，从没成功过是 `0`，本地资源没有这一项；`updating`；`fromModule` 恒为 `null`；`error`——只在最近一次更新失败时有；托管配置那一项在引擎暂停时另有 `suspended: true`。
- `POST /v1/external_resources/update`，正文 `{"key":"<key>"}` 或 `{"key":"all"}`。应答把每个更新过的键映射到 `"success"` 或错误文本。指名的键两处都找不到：400。`all`：两处全部远程资源一起更新（并发），本地的跳过、不出现在应答里；指名一个本地资源：踢一下重读，`"success"`。整个请求至多 60 秒，到时限还没结束的键是 `"timed out"`。正文不是 JSON 或没有 `key`：400。处理请求期间拿着当时那一代（其间发生的重载不会让等着的键以"管理器没了"收场）。

托管配置或远程 include 更新成功后的重载是主循环的事（`changed()`），不在这个请求里等。新版本因为等它提到的来源而没有立刻采用时，那个键的结果是等待的那一句；来源到齐后自动采用并重载。

路由照 M5a 的 `GET /v1/geoip`（`crates/rurge-api/src/routes/geoip.rs`）的样子写：坏的正文是 400 而文本不回显调用方给的东西（`ApiError::bad_request`）。`all` 把每个远程资源的下载同时发出（与冷启动时相同）；下载到的集合的重编译由注册表排队，一次一个（任务 10）——一份满员的集合编译时的峰值约是建好之后的三倍（M5a 的实测），几十份同时编译会把峰值叠起来。

**用例**：

- `api.rs`（在线的一代：`no_network = false`，`geo_urls` 指到回环——现有的夹具 `api_seeded` 建的是离线的一代，给它加一个开关或在它上面包一层，不复制整个夹具；要先放进缓存的东西经它的 `seed`）：一个 URL 规则集、一个本地文件规则集（`local: true`、没有 `updatedAt`）、一个订阅（`path` 是标签，URL 里的令牌不在应答里）、GeoIP 两项；从没下载到的 `updatedAt: 0`、`ready: false`、带 `error`；`System` 不在；给了 `sources` 时托管配置与 include 两项在最前；暂停时的 `suspended`。更新一个键：成功，规则集的新内容随后生效（有时限地等）；服务端答 500：错误文本；不认识的键、坏的正文：400；`all`：应答里有每个远程键、没有本地的；离线的一代：每个 URL 资源都是错误；不带 `X-Key`：401。暂停期间这两个端点与 `GET /v1/outbound` 照常。
- `cli.rs`（`rurge_run_online`）：
  - 托管配置更新后自动重载：服务端换成多一条规则的 v2，`POST update` 答 `success`，有时限地等到 `profile reloaded` 一行，那条规则在 `GET /v1/rules` 里；本地文件逐字节不变。
  - 评审要点 1：服务端换成加载不了的内容——应答里是错误文本，v1 的规则还在，`GET /v1/external_resources` 那一项带 `error`、`ready` 仍为真。
  - 评审要点 3：v2 多了一行从没下载过的远程 include——`POST update` 答等待的那一句，有时限地等到恰好一次 `profile reloaded`，之后 v2 的规则与 include 里的规则都在。
  - 评审要点 5：缓存里一份两小时前的 `strict` 副本、服务端答 500 地启动（任务 8 的那个场面）——服务端恢复后 `POST update`，经代理的请求通过，日志里有恢复的那一行。同一个场面的另一条出路：把本地文件（完整的 Profile）那一行的 `strict` 去掉并经 API 重载——会话随即放行。
  - 评审要点 4：往本地文件加一条规则并经 API 重载——那条规则生效（本地文件比副本新）；再 `POST update`（服务端答 304）——有时限地等到又一次 `profile reloaded`，那条规则不在了。

**判据**：`--test api`、`--test cli` 全绿。

---

## Task 10：集合文件里没有用上的行说出来；重编译一次一个

**为什么**（M5a 延后事项第 31、32 行；项目自己的原则是"解析并忽略 + 记录日志"，这两处缺了后一半）：

- 规则集文件的一行带着 rurge 不认识的参数时——`no-resolve` 拼成 `no-resolv`，写成 `no-resolve#说明`（标记前没有空白，不算注释），或者整行是从 `[Rule]` 里抄来的、末尾带着策略名——，参数被丢掉而什么也不说（`SubRule.unknown` 没有人读；内联的 `[Ruleset X]` 小节有 `W0003`）。丢掉的是 `no-resolve` 时，这一行从此会触发 DNS 解析。
- 数被跳过的行的那条 `W0023` 只在建注册表时发（`SetRegistry::push_diag` 在 `building` 之外什么也不做）：启动之后才第一次下载到的集合，和之后的每一次刷新，只悄悄改状态里的计数，而今天没有任何出口显示它。任务 9 的端点是第一个出口。

**文件**：`crates/rurge-rules/src/set_format.rs`（`ParsedSet`、`parse_set_with_limit`）、`crates/rurge-rules/src/registry.rs`（`SetStatus`、`Entry`、`finish`、`spawn_reloader`）、`crates/rurge-api/src/routes/resources.rs`；用例在这两个 crate 的单元测试、`crates/rurge-engine/tests/`（要捕获日志的那几条，用 `common::LogCapture`）与 `crates/rurge-api/tests/api.rs`。

**接口**：

```rust
// rurge-rules::set_format
pub struct ParsedSet { /* 原有的 */, pub unknown_params: usize, pub first_unknown: Option<usize> }   // 带未知参数的行数；第一处的行号（从 1 起）
// rurge-rules::registry
pub struct SetStatus { /* 原有的 */, pub key: Option<String>, pub unknown_params: usize }
```

`SetStatus.key` 是 `rurge_net::resource::resource_key`（任务 1）——URL 与本地文件的集合有，内联与内置的是 `None`；端点凭它把集合的状态接到资源的那一项上（一个资源只有一种集合身份，M5a 的 `E0024` / `W0032`）。

**行为**：

- **数**：规则集文件里 `SubRule.unknown` 不为空的行各算一行，记第一处的行号；域名集的行没有参数，不数。内联的 `[Ruleset X]` 不经这里（加载器已经逐行给了 `W0003`），不重复报。
- **建注册表时**（文本当时已在缓存里或是本地文件）：有这样的行时一条 `W0003`——``set `<name>`: <n> line(s) carry a parameter that is not known and is ignored (first: line <行号>)``；说集合的名字与行号，**不引用参数**（集合文件是下载到的任意文本，给错的可能是一条带口令的代理行；M5a 对域名集读不了的行已经这样办）。`W0003` 的缺省建议照旧适用。
- **状态**：`SetStatus.unknown_params`；`skipped`、`truncated` 原有。
- **启动之后**（第一次下载到，或刷新；诊断这时无处可去）：这一次载入的三个数（`skipped`、`truncated`、`unknown_params`）不全为零、并且与这个集合上一次载入的不同时，一行 WARN——`set loaded with lines that are not used`，字段 `set`、`skipped`、`truncated`、`unknown_params`，以及有的话 `first_skipped_line`、`first_unknown_line`；只有数与行号，不带任何一行的文字，也不带跳过的原因（原因的文本可能引用那一行的开头）。数没变的刷新不再说一遍：一份每天刷新、总有三行读不了的列表不该每天一行 WARN。建注册表时已经报过的集合，"上一次"就是那一次。
- **重编译一次一个**：`spawn_reloader` 里"解析并编译"那一段由注册表的一把异步锁护着，同一个注册表同时只有一份集合在编译（等锁的任务让出线程）。一份满员的集合编译时的峰值约是建好之后的三倍（M5a 的实测，M2 设计第 16 节 Q3）；冷启动时与任务 9 的 `all` 之后，几十份集合会同时拿到新内容。编译仍在运行时的线程上（与今天相同），不搬到阻塞线程池。
- **端点**（任务 9 的 `GET /v1/external_resources`）：`type` 是 `ruleset` 或 `domainset` 的项多三个 rurge 自己的字段 `skipped`、`truncated`、`unknownParams`（数；没有这样的行时是 `0`），别的种类没有。`error` 不因它们而出现：手册的 `error` 是"Present only if the last update or parsing failed."，一份有几行读不了的集合是载入了的。

**用例**：

- `set_format`：`DOMAIN,a.com,no-resolv`（算）；`IP-CIDR,10.0.0.0/8,no-resolve`（不算）；`DOMAIN,a.com,Proxy`（抄来的整行，算）；`IP-CIDR,10.0.0.0/8,no-resolve#note`（粘着的标记，算）；`DOMAIN,a.com,bogus // note`（行尾注释不进参数，算一次）；第一处的行号数的是文件里的行（前面有注释行与空行）；域名集恒为 0。
- 注册表（建的时候）：缓存里一份带两行这样的规则集——恰一条 `W0003`，文本里有集合的名字与第一处的行号、没有参数（参数取带标记的 `S3CR3T`，诊断与它的建议里找不到）；`statuses()` 里 `unknown_params == 2`、`key` 等于那个来源的 `resource_key`；内联与内置集合的 `key` 是 `None`；内联小节里的未知参数照旧只有加载器的那条 `W0003`，注册表不另报。
- 启动之后的日志（引擎一层，回环 `TestServer` 给一份 URL 规则集，在线的一代、GeoIP 的地址指到回环，`LogCapture`）：缓存为空地启动，下载到的集合有 3 行读不了、1 行带未知参数——恰一行 WARN，四个数与两个行号都对，那几行里的标记不在捕获到的日志里；服务端换成数相同的另一份内容，等到集合的版本前进——WARN 仍是一行；再换成数不同的——第二行。缓存里已经有那份内容地启动（建的时候报了 `W0023`），之后一次数相同的刷新——没有 WARN。决定"说不说"的那个判断另抽成纯函数、单元测试覆盖它的每一条（全零、与上一次相同、任何一个数变了、从非零回到全零——不说）。
- 一次一个：多线程运行时上两份各几十万行的集合同时发布新版本——用例专用的计数器看到的"同时在编译"的最大值是 1（没有锁时先看到 2，把集合给大到这一步稳定地红）；两份都编译完，内容都是新的。
- 端点：带这样几行的规则集那一项的三个字段是对的数；干净的那一项是三个 `0`；订阅、GeoIP 与 Profile 来源的项没有这三个字段；`error` 不因它们出现。

**判据**：`cargo test -j 8 --workspace --lib -- set_format`、`--workspace --lib -- registry`、`--workspace --test api -- external_resources` 与那几条引擎用例全绿；`crates/rurge-config/tests/secrets.rs` 照旧绿（新文本不引用取值）。

---

## Task 11：API 的连接收尾——晚到的请求正文不再让客户端丢掉应答

**为什么**（M5a 延后事项第 35 行；来由与证据在 M5a 计划的「Task 10」一节，写给使用者的版本在 `docs/api/phase1.md` 的「鉴权与错误」）：不读请求正文的端点（`POST /v1/dns/flush`、`/v1/profiles/reload`、`/v1/profiles/check`、`/v1/stop`），和在读正文之前就被拒绝的请求（密钥不对的 `401`、被封禁的 `403`、`404`、`405`），只凭请求头就答复；hyper 写完应答后试着读一次正文，没读全就关掉连接。正文晚于请求头到达时落在一条已经关掉的连接上，服务端的协议栈回一个 reset，客户端还没读的应答可能就此丢掉——在一台 Windows 11 x86-64 的开发机上量到的是：正文的写成功，接着的读以 `ConnectionAborted`（os error 10053）失败、一个字节也读不到，而请求已经执行。请求头与正文分两次写的客户端并不少见：CPython 的 `http.client` 就是这样（`urllib.request`、`requests` 都建在它上面）。

**业界的做法**（2026-10-11 读）：

- RFC 9112 第 9.6 节："If a server performs an immediate close of a TCP connection, there is a significant risk that the client will not be able to read the last HTTP response. … To avoid the TCP reset problem, servers typically close a connection in stages. First, the server performs a half-close by closing only the write side of the read/write connection. The server then continues to read from the connection until it receives a corresponding close by the client, or until the server is reasonably certain that its own TCP stack has received the client's acknowledgement of the packet(s) containing the server's last response. Finally, the server fully closes the connection."
- nginx 的 `lingering_close`（缺省 `on`）："instructs nginx to wait for and process additional data from a client before fully closing a connection"；关掉它 "breaks the protocol and should not be used under normal circumstances"；读掉并忽略的总时间由 `lingering_time`（缺省 30 秒）限制，每一次等待由 `lingering_timeout`（缺省 5 秒）限制。
- Go 的 `net/http`（1.23.0 的 `server.go`）：处理函数没读完正文时先替它读掉至多 256 KiB（`maxPostHandlerReadBytes`）；不得不带着没读的数据关连接时，先关写端、等 500 毫秒（`rstAvoidanceDelay`）再全关（`closeWriteAndWait`）。

**做法**：照 RFC 说的分三步关——关写端、读掉、全关——，放在连接这一层，对每一条被 hyper 放手的连接都做：一个机制盖住四个不读正文的端点、四种拒绝与以后新加的任何端点，请求怎样被读、鉴权与应答一概不动。（另一条路——在路由里加一层中间件、答复之前先把正文读完——盖不住在鉴权那一层就被拒绝的请求，除非让没通过鉴权的对端也能叫服务端替它读正文；而且它把"答复"拖到"正文到齐"之后，对一个永远不发正文的客户端要另设时限。不取。）

**文件**：`crates/rurge-api/src/linger.rs`（新）、`src/lib.rs`（`serve`、`router` 的可见性与注释）、`src/auth.rs`（`require_key` 取对端地址的那个参数）；用例在 `crates/rurge-api/tests/api.rs`、`crates/rurge/tests/cli.rs` 与 `linger.rs` 的单元测试。不新增依赖（`tokio-util` 的 `TaskTracker` 工作区里已经在用）。

**接口**（都在 crate 之内）：

```rust
// rurge-api::linger
pub(crate) const LINGER_TIME: Duration = Duration::from_secs(1);   // 从连接交到收尾任务手里起，至多等这么久
pub(crate) const LINGER_BYTES: usize = 256 * 1024;                 // 至多读掉这么多
pub(crate) const LINGER_MAX: usize = 64;                           // 同时在收尾的连接数；满了的直接关

/// 接受连接的一层：交出的每条连接在被丢掉时先收尾再关。
pub(crate) struct Lingering { /* TcpListener 与收尾的状态 */ }
impl axum::serve::Listener for Lingering { type Io = Closing; type Addr = SocketAddr; /* accept、local_addr */ }

/// 一条连接：读写原样转给 TcpStream（含向量写与 `is_write_vectored`）；被丢掉时把它交给收尾的任务。
pub(crate) struct Closing { /* Option<TcpStream> 与收尾的状态 */ }

/// 对端地址。axum 只给 `TcpListener` 实现了 `ConnectInfo<SocketAddr>`，自己的监听类型要一个本 crate 的类型（孤儿规则）。
#[derive(Clone, Copy, Debug)]
pub(crate) struct Peer(pub SocketAddr);
impl Connected<IncomingStream<'_, Lingering>> for Peer { /* 取 remote_addr */ }
```

`auth::require_key` 的参数从 `ConnectInfo<SocketAddr>` 改成 `ConnectInfo<Peer>`；`serve` 用 `Lingering` 接受连接、以 `into_make_service_with_connect_info::<Peer>()` 起服务。`router` 只有 `serve` 一个调用方，改成 crate 之内可见（它的注释原来教调用方用 `SocketAddr` 起服务，那样做从此取不到对端地址）。`serve` 的签名与返回值不变。

**行为**：

- `Closing` 被丢掉（hyper 用完了这条连接：答完并决定关、对端先关、出错、优雅退出）时：当前线程在一个 tokio 运行时里、并且同时在收尾的连接不到 `LINGER_MAX` 条——把 `TcpStream` 交给一个被跟踪的任务：关写端（应答到此完整，对端读得到文件尾），然后读掉并丢弃收到的字节，直到对端关闭、读出错、丢弃满 `LINGER_BYTES`、或从交到任务手里起过了 `LINGER_TIME`，四者先到的为准；然后丢掉 `TcpStream`。否则（没有运行时，或已满）当场丢掉，与今天相同。
- `serve` 返回的 future：`axum::serve` 结束（退出信号已到、在途的应答写完）之后，再等收尾的任务全部结束才完成——`POST /v1/stop` 的应答正是在进程退出前的这一刻最容易丢。每个任务以 `LINGER_TIME` 为界，守护进程本来就在退出的宽限（5 秒）里等这个 future，所以退出至多因此晚 `LINGER_TIME`；对端读完应答就关连接的客户端（`rurge stop` 自己、`curl`）不让它等。
- 不记日志（一条连接怎样结束不是使用者要看的事）；收尾期间读到的字节不解析、不保存。
- 数字的来由：`LINGER_BYTES` 取 Go 的 256 KiB；`LINGER_TIME` 取 1 秒——API 的正文都是小的 JSON，晚到的那一段在回环与局域网上是毫秒的事，1 秒是 Go 的 500 毫秒的两倍、比 nginx 的 5 秒短得多，因为它同时是退出可能多等的时间；`LINGER_MAX` 让一个不停连进来又不关的对端最多占住 64 个描述符各 1 秒。

**用例**：

- `api.rs`（手写的客户端，直接用 `TcpStream`；这几条是全局约束里"一次写出"的例外）。每条的步骤：写请求头（带 `Content-Length`，不带正文）→ 用 `peek` 有时限地等到应答的第一个字节已经在本端的接收缓冲里（不把它读走——要测的正是"还没读应答的客户端"）→ 再等 200 毫秒 → 写正文 → 读到文件尾。断言：读到的是完整的应答（状态行、`Content-Length` 与正文相符），之后是干净的文件尾而不是错误。那 200 毫秒不是同步：有了收尾，正文什么时候到断言都成立；它只让没有收尾的服务端来得及把连接关掉，用例才会红（注释里这样写明）。
  - 四个不读正文的端点各一条（`/v1/stop` 用夹具里的 `FakeControl`，断言它被调用了一次）。
  - 四种拒绝各一条：密钥不对的 `401`、被封禁的 `403`（同一台服务端上先用错的密钥失败 `BAN_FAILURES` 次）、带对的密钥访问没有的路径 `404`、带对的密钥而方法不对的 `405`（鉴权在路由之前，不带密钥时这两种都是 `401`）。
  - 读正文的端点（`POST /v1/log/level`）同样的步骤：照旧 `200`（它等正文，今天就是好的——钉住没有被改坏）。
  - 一次写出请求头与正文、不发正文的 `GET`：照旧。
  - 应答之后客户端先发 300 KiB 的垃圾再读：应答仍读得到（超过 `LINGER_BYTES` 之后服务端关连接，客户端此前已经收到应答与文件尾；断言只到"应答完整"为止，之后是文件尾还是 reset 不断言）。
- `linger.rs` 的单元测试（回环上的一对 `TcpStream`，暂停不了的真实时钟，等待都有时限）：对端关了，任务立刻结束；对端什么也不做，任务在 `LINGER_TIME` 上下结束（断言不早于它的九成、不晚于它的三倍）；对端不停地写，任务在读满 `LINGER_BYTES` 后结束；同时收尾的连接到了上限（用例里把上限作为参数给成 2）时第三条当场关掉；没有运行时的线程上丢掉 `Closing` 不 panic。
- `serve` 的 future：取消令牌之后，还有一条连接在收尾时它不完成，那条连接的对端一关它就完成；对端一直不关时它在 `LINGER_TIME` 上下完成。
- `cli.rs`（真实拉起 `rurge`）：
  - 手写的客户端对 `POST /v1/stop` 走上面那套"请求头、等、正文、读"——读到 `200` 与 `{}`，进程以 0 退出。
  - 一个客户端发完 `GET /v1/outbound`、读完应答而**不关连接**，另一个客户端 `POST /v1/stop`：进程有时限地（10 秒）以 0 退出——一个不肯关连接的客户端拖不住退出。
  - `rurge stop -c <配置>` 照旧打印成功并以 0 退出（rurge 自己的客户端）。

**判据**：`cargo test -j 8 --workspace --lib -- linger`、`--workspace --test api`、`--workspace --test cli -- stop` 全绿；新用例在本机（Windows）上对改动之前的代码是红的——报告里给出红的那一次的输出；Linux 与 macOS 上它们"改动之前是不是红的"没有量，写进报告与文档（那两个系统上客户端丢不丢应答本来就没有量过）。

---

## Task 12：阶段 1 的需求逐句对照代码——称"齐备"之前的那一遍

**为什么**（CLAUDE.md 的规矩；M5a 延后事项第 28 行）：阶段 1 曾经被写成"功能已齐备"而缺着两项 P0；2026-10-10 的那次审计逐句读过阶段 1、2 的功能需求，查出的缺口由 M5a 与本计划的前面各任务补上，但那一次**判为"做了"的条款没有人复核**——M5a 补用例时就在其中查出一条只做了一半的（FR-RULE-05）。这个任务把阶段 1 的需求再对一遍，这一次每条"做了"都要指到证据；它的结论决定任务 13 能不能写"阶段 1 功能齐备"。

**读什么**：`docs/requirements.md` 第 4 节里所属阶段含阶段 1 的每一条功能需求——2026-10-11 数是 48 行（CFG 16、IN 5、OUT 2、RULE 10、DNS 7、HTTP 2、OBS 6；其中 12 行的阶段一栏写着"1 基础 / 4 完整""1（UDP：2）"一类，只读它的阶段 1 部分），另加阶段一栏写"各阶段"的一行里属于阶段 1 的部分——，逐句：一条需求里有几句话（分号、顿号隔开的几件事）就是几行。每一句对着三样东西读：手册里说这件事的原句（`https://manual.nssurge.com/`，页面路径见 CLAUDE.md；读不到的页面写"没读到"，不凭记忆）、实现它的代码、一条会因为它不成立而红的用例。

**每一句的结论是四种之一**：

1. **做了，有用例**：给出代码所在（文件与函数）与用例（文件与名字），并读过那条用例的断言——它断言的是这一句说的行为，不是"跑通了"。读不出来的，做一次改坏代码的验证：改一行让这一句不成立、跑那条用例、看到它红、改回去（改坏的那一行与红的输出记进报告；验证完工作区必须干净）。
2. **做了，没有用例**：补一条用例（先看到它在改坏的代码上红），随本任务提交。
3. **只做了一部分 / 只解析 / 没有做**：一处代码、一条用例补得上的（M5a 那些小项的尺度），当场补——先红后绿，每处一个提交——；要先有设计才能做的，不做，登记进本计划的「延后事项」与验收文档，写明缺什么、使用者会看到什么。
4. **不属于阶段 1 / 需求文字本身有问题**（与手册不符、与另一条需求矛盾、所属阶段标错）：登记，不改需求文档的正文（任务 13 只改状态行）。

**拿真实的输入读**（M5a 的两处缺口都是这样查出来的——常用的列表每条 IP 行都带 `no-resolve`、每行都以注释结尾，而用例里的列表不是这样）：规则、规则集与域名集、`[Host]`、DNS 上游、`[General]` 的键这几组需求，各拿一份照手册的示例与公开的常用列表的写法**手写**出来的输入过一遍（不下载任何东西；公开列表的写法从网页上读），看有没有被悄悄跳过或读成别的意思的行。

**产出**：

- `docs/acceptance/phase1-acceptance.md`（新；阶段 1 此前只有手工清单，没有逐条的证据）：一张表——需求编号、那一句话、结论、代码所在、用例；表后是「没有做完的」（第 3 种里登记的）、「需求文字的问题」（第 4 种）、「靠手工验收的」（指向 `docs/acceptance/phase1-manual.md` 的行）、「靠第一次 CI 运行的」（只在 Unix 上编译的用例、macOS 与 Linux 的读取）。用例名与路径写完之后用 `git grep` 逐个核对。
- 补的用例与补的代码，各自的提交。
- 报告里：一共多少句、四种结论各多少、做了改坏验证的是哪几句、补了什么、登记了什么。

**结论怎么用**：表里没有第 3 种里"登记而没补"的 P0 / P1 时，任务 13 才可以写"阶段 1 的功能需求逐句对过代码，齐备"；有的话照实写缺哪几句，"齐备"一词不出现。无论哪一种，文档都要写明这一遍是谁读的（一个只读加补用例的助手，每条结论由评审者抽查）、没有覆盖什么（手工验收的 32 行、CI）。

**规模与分工**：这是读的活，不是写的活；一个实现者读不完时照模块分成两三份先后做（配置与规则；DNS、入站与引擎；API、CLI 与平台），每份各自提交自己那一段表，最后合成一张——不并行（同一时刻只有一个写者）。评审者抽查：每个模块随机取五句，照同样的三样东西重读，另把全部"做了改坏验证"的条目的记录读一遍。

**判据**：表覆盖需求文档里阶段 1 的每一条功能需求（评审者拿需求文档的编号清单对表的编号清单，一个不少）；表里每个用例名 `git grep` 得到；补的用例全绿、全量门禁绿；工作区干净（没有留下改坏的代码）。

---

## Task 13：文档

- **兼容性清单**：`#!MANAGED-CONFIG`（三个参数）与远程 `#!include` 两行从"只解析 / 不支持"改为实现的状态与差异；设计第 8 节的 1 ～ 7 项与第 13 节新增的几项各一行或并入现有行——副本在数据目录、本地文件被改过时暂时生效与"没有节的本地文件不优先"（并引手册 `profile/format.html` 的那一句："A managed profile cannot be modified locally because it can be updated remotely. To make changes, first create a copy to turn it into a normal profile."——Surge 根本不让使用者改托管配置，所以"本地文件被改过"时怎么办没有 Surge 的行为可循，是 rurge 自己的规则）、换地址至多 8 跳、加载不了的与不是 Profile 的内容不采用、新版本等它提到的来源、`strict` 是暂停、`interval` 下限 60 秒、远程 include 缺缓存时不展开、下载到的文本里的本地 include 与 `external` 策略、更新走直连、只在 `rurge run` 运行时更新（手册："The profile can only be updated while the main Surge app is running."）、两个端点的字段差异（含任务 10 的三个字段）；3.1 节 `FINAL` 一行照任务 0 改写（第一格去掉"目前……仍被评估"，备注写两句都成立的读法、"按手册的三句话推断，没有在 Surge 上观察过"、只有哪一种 Profile 受影响），开头一段里 M5a 登记的那一项随之去掉；3.5 节加一行或并入：集合文件里带未知参数的行（`W0003`、状态与端点里的数）；10.4 节：`docs/api/phase1.md` 里那条"已知限制"所说的事不再发生。统计附录照脚本重数。
- **需求文档**：第 7 节阶段 1 的状态行去掉 FR-CFG-04、05、14 三处，并照任务 12 的结论写（齐备，或缺哪几句）。
- **验收文档**：`docs/acceptance/phase2-acceptance.md` 的「需求逐条对照代码」一节——两处 P0 与手动更新的出口注明补在哪些提交、凭哪些用例；「要定的事」里 `FINAL` 的读法一项改成"已照手册改，读法是推断的"；「知悉」里 API 的那一件事改成已修。`docs/acceptance/phase1-acceptance.md`（任务 12 的产出）在这里只核对名字、不重写。
- **API 参考** `docs/api/phase1.md`：两个端点的请求与应答、各字段与手册的差异、等待的那一句、任务 10 的三个字段；`GET /v1/profiles/current` 返回的是生效的主文本；暂停时会话的 `error` 文本与 HTTP 客户端看到的 502；`GET /v1/rules` 不列出第一条 `FINAL` 之后的规则；「鉴权与错误」里"已知限制"那一条改写为现在的行为（服务端关连接之前先关写端并读掉晚到的字节，至多 1 秒、256 KiB；在 Windows 上量过，别的系统没有量）。
- **手工清单** `docs/acceptance/phase1-manual.md`：对真实服务商的托管配置与远程 include 各一项（自动化测试只用回环）、真实的 `strict`（断网到期、恢复）、用 Python 的 `requests` 对一个不读正文的端点发一次带正文的 `POST`（Windows、macOS、Linux 各一行——后两个系统上自动化用例"改动之前是不是红的"没有量过）。
- **M2 设计**（`docs/superpowers/specs/2026-09-04-phase1-m2-rules-dns-design.md`）第 17 节加一条订正：4.2 的"`W0019` 以最后一条 `FINAL` 为锚点"与 6.5 的"其前的其他 `FINAL` 在构建时被移除"不再成立，现在的读法与来由。
- **README（中英）**、**CLAUDE.md**（状态段、索引、常用命令的新过滤词、"阶段 1 功能并不齐备"那一段照任务 12 的结论改写、M5a 那句"三件登记而没有改的事"里已经做掉的两件）。
- **设计文档**：加「M5b 实施期的订正」一节；**本计划**：两张表。
- **M5a 计划**的「延后事项」不回头改（它是那一次执行的记录）；它的第 28、31、32、35、36 行在本计划的「承接事项」里各有去处，文档里引用时指向本计划。

**判据**：每一处文档里的名字（类型、函数、诊断码、测试名、命令）在代码里找得到；表格的列数与表头一致；没有本机路径与流程用语；文档里写到里程碑时带上阶段（"阶段 1 / M5b"），不写光秃秃的"M5b"（阶段 2 也有 M5a、M5b）。

---

## 计划期决定

| # | 事项 | 核对到的事实 | 决定 |
| --- | --- | --- | --- |
| P1 | 最近一次成功检查的时刻从哪里来 | `ResourceStatus.fetched_at` 在 `failed` 时是 `None`，那个时刻只在私有的 `Entry.meta` 与 `meta.json` 里；缓存没有带路径与时刻的公开读取入口（`cached` 只给正文），`CacheDir::path()` 是目录 | 状态里的 `fetched_at` 放宽为"只要知道就给"；加公开的 `cached_copy`（正文、文件路径、时刻） |
| P2 | 时刻的精度 | `Meta.fetched_at` 只到秒；本地文件的修改时间到 100 纳秒。只到秒的话，"先存文件、同一秒内下载到"会被判成本地文件更新——CLI 用例与真实使用里都会遇到 | `Meta` 加 `fetched_subsec_nanos`（`serde(default)`，旧缓存读作 0） |
| P3 | 内容没变的 200 | URL 资源每个 200 都升版本、不比较内容（本地文件是比较的）；回环的 `TestServer` 总带 ETag，所以现有用例看不到 | 逐字节相同的正文照"没有变化"办，对所有种类生效；`TestServer` 加一个关掉校验器的开关来测 |
| P4 | 撤掉一个来源 | 管理器没有撤掉单个条目的接口，设计 7.3 写的是"来源集合变小时重建管理器"；对已有的条目再登记只会把间隔往短里并（`merge_interval`） | 给管理器加 `retire`，不重建：订阅者、留着的候选与状态都不必搬；间隔变了的来源撤掉重登 |
| P5 | "这一次更新"的结果 | 踢一下是 `Notify::notify_one`，没有完成的信号；在途的尝试之后会再跑一次；离线的条目没有任务，`force_update` 却返回真 | `update_now` 等"调用之后才开始的那一次"结束；离线立刻给错误；`force_update` 不动 |
| P6 | 间隔是下限 | 抖动是 ±10 %；手册："This is the shortest time before an update may be triggered" | `never_early`：Profile 的来源只往后抖 |
| P7 | 下载到的文本里的 `external` 策略 | `external` 策略启动本机的程序、参数取自配置；订阅导入的 `external` 行已经一律跳过（`W0023`，M4c）。Surge 的托管配置能带这种策略——那边整份 Profile 都被当作使用者自己的 | 来自下载文本（远程 include、托管配置的副本）的 `external` 定义不建出站，行为同 REJECT，`W0033`；写在本地文件里的照旧。这是与 Surge 的一处差异（设计 M5-D20），登记在兼容性清单；要不要给一个放开它的命令行开关由项目所有者定（延后事项 8） |
| P8 | 远程 include 的"不是 Profile" | 一页 HTML 当作 include 展开时只有告警（`W0015`、`W0017`），加载照样通过；手册："The referenced file must contain the corresponding section declaration." | `RemoteRef.lacks_section`；候选缺它被 include 的节时不采用。已经在缓存里的照旧只是 `W0017` |
| P9 | 没有节的本地文件 | 常见的起步写法是本地文件里只有一行 `#!MANAGED-CONFIG`。这样的文件被动过（改间隔、`touch`）之后按"本地文件更新则本地生效"会加载不了，而副本没到期、不会去更新；`#!include` 写在第一个节之前是 `W0015`，所以没有节的文件不可能靠 include 有内容 | 本地文件一个节也没有时不参与先后的比较，副本在就用副本 |
| P10 | 换过地址之后本地文件与副本的先后 | 设计 7.2 只拿第一跳的副本与本地文件比。第一跳换了地址之后不再刷新，它的时刻停在换地址的那一天：此后本地文件一被动过（存一次、`touch`）就永远"更新"，被跟踪的退回本地文件那一行的旧地址——服务商迁走后旧地址多半已经关掉，于是本地那份旧内容永久生效，带 `strict` 时永久暂停 | 先沿缓存走到头，再拿本地文件与链上最新的那次检查比；被跟踪的总是链的头；本地文件生效时间隔与 `strict` 取本地那一行的（使用者改掉 `strict` 就能从暂停里出来）；走过不止一个地址时多一条 `I0004` 提醒本地那一行该改了 |
| P11 | 候选的检查顺序 | 设计 7.3 写的是"先只带这一份试，不过再带上全部留着的" | 另有留着的候选时先一起试：新来源到达的那一刻就能把等它的候选一并采用，不必等那个候选自己的下一次退避重试 |
| P12 | 新版本提到没下载过的来源 | 没有新来源也加载得了的新版本（多了一行远程 include，那一行照 `W0022` 不展开）若立刻采用，会先换上缺着一块的一代、几秒后再补齐——规则在这几秒里是残缺的；检查那次加载里看得到它碰了哪些来源（`SourceInfo.remote`） | 三种结论：通过、不通过、等待。没拿到而还没试完第一次的来源先登记、候选留着，它试完（成功则一并采用，失败则候选在自己的下一次重试时采用，那一行带 `W0022`）。设计 M5-D21 |
| P13 | 等待算不算 `strict` 的失败 | 候选留着时那个来源在管理器里是 `failed`；手册要的是"a successful update after the interval arrives"——这时下载是成功的 | 因等待而留着的不算；因不通过而留着的算（设计 7.4："下载到的内容没通过检查也是一次失败的更新"） |
| P14 | Profile 已不提的地址 | 本地文件可以在两次重载之间被改掉那一行 | 候选的 URL 不在这次加载的 `SourceInfo.remote` 里时不采用 |
| P15 | "没有变化"也结束本地文件的优先 | 304 不升版本，`changed()` 不会返回；设计 M5-D2："本地文件生效到下一次成功的检查为止" | `outdated()`，主循环每秒看一次；对同一个检查时刻只真一次 |
| P16 | 诊断里的时刻与没下载到时说什么 | 设计 7.2 的 `I0004` 写的是"using the copy downloaded at …"，只给远程 include 定了 `W0022` | 诊断不带时刻：同样的输入给出同样的诊断（快照、`check` 的输出可比）；托管配置没拿到时也给一条 `W0022`，带原因——只有一行的本地文件第一次启动失败时，使用者靠它知道为什么 |
| P17 | `settle` 等谁 | 设计："等每个还没有内容的来源试完第一次" | 另加到了期的 `strict` 来源（否则启动时判断不了要不要暂停）与等待期间新登记的来源；到期而不是 `strict` 的不等，后台刷新；返回之前把留着的候选再判一次 |
| P18 | 引导在启动顺序里的位置 | 今天的顺序是 加载 → 日志 → 建数据目录 → 实例锁；设计 7.5 把引导放在第一次加载之后——那里还没有锁、没有数据目录；`recover_without_a_log` 自己去拿锁，同一个进程拿不到第二把 | 引导仍在日志之前（级别要取自最后加载到的 Profile），但先建数据目录、拿锁，这把锁沿用到底；开始时往标准错误写一行；之后的两条失败退出共用"拿到锁之后"的那一半恢复 |
| P19 | 加载不了时要不要引导 | 生效的副本可能因为升级后校验变严、或它引用的本地 include 被删而加载不了；rurge 不在运行时没有东西去刷新缓存，于是每次启动都死在同一份副本上。反过来，使用者刚改坏了本地文件时，一次"没有变化"的更新会让副本重新生效、把他的错误盖掉而不报 | 有一条错误的位置在下载到的文件里时才引导并把每个来源更新一遍（`Boot::Refresh`）；错误全在本地文件里的照今天的样子以 2 退出 |
| P20 | 引导的逻辑放在哪里 | 可执行文件里的代码只能靠拉起二进制来测 | `boot_need` 与 `bootstrap` 放在 `rurge-engine::profile`，引擎一层用回环服务端测；可执行文件只接线 |
| P21 | 控制命令的数据目录 | `ControlArgs` 没有 `--data-dir`；`service install` 不加载 Profile | 三个控制命令加 `--data-dir`（缺省平台目录）；`service install` 不动 |
| P22 | UDP 的拒绝用哪种形式 | `run_flow`：失败的流 5 秒后重试（每次一条新记录），被拒绝的流留到空闲 60 秒 | 用失败：暂停结束后目标自己恢复。代价是暂停期间每个活跃的 UDP 目标每 5 秒一条失败记录 |
| P23 | 退避的上限 | 缺省 60 秒起、至多 3600 秒 | Profile 的来源至多 600 秒 |
| P24 | `ready` 与 `updatedAt` | 手册：`ready` 是 "Whether the resource content is available."；`updatedAt` 是 "The UNIX timestamp of the last successful download, or 0 if the resource has never been downloaded." | `ready`：手里有内容；`updatedAt`：最近一次成功的检查（"没有变化"的应答也算）——登记 |
| P25 | API 怎么够到 `ProfileSources` | `ApiContext` 只有引擎、`Control` 与加载选项，两处构造（`run.rs`、API 的测试辅助） | `ApiContext.sources: Option<Arc<ProfileSources>>`；`Control` 不动 |
| P26 | 加载器的接口用什么表示 URL | 设计 7.2 的 `RemoteSource::get(&str)`；缓存目录的名字是 `Url` 规范写法的 SHA-256，写出来的文本与它可以不同（主机的大小写、缺省端口、空路径） | `get(&Url)`、`RemoteRef.url: Url`：加载器与资源管理器说的是同一个值 |
| P27 | 任务的划分 | 设计第 11 节是 7 个任务 | 14 个（任务 0 ～ 13）。Profile 的远程来源 9 个（任务 1 ～ 9）：资源管理器拆成两个（并发的那一半单独评审），引擎的暂停单独一个，`ProfileSources` 拆成"跟踪"与"判定和引导"两个。M5a 留下的 4 个：`FINAL`（任务 0）、集合文件里没有用上的行（任务 10）、API 的连接收尾（任务 11）、需求复读（任务 12）。文档 1 个（任务 13） |
| P28 | 第一条 `FINAL` 之后的规则不评估，策略取最后一条 `FINAL` | 手册两页各有一句说 `FINAL` 之后的规则不生效（`rules/final.html`、`profile/format.html`），一句说多条 `FINAL` 时最后一条生效；rurge 原先只照后一句读，夹在两条 `FINAL` 之间的规则照常评估（`RuleEngine::build` 跳过此前的 `FINAL`、留下它们之间的规则） | 三句同时成立的读法只有这一种，照它改（任务 0）。错了的代价：Surge 其实会评估的一条夹在两条 `FINAL` 之间的规则在 rurge 里不生效，`W0019` 会说有几条、在哪里；不改的代价是 Surge 不评估的规则在 rurge 里生效，提示只有一条说前一条 `FINAL` 被遮住的 `W0021`。读法是推断的，没有在 Surge 上观察过（延后事项 22） |
| P29 | API 怎样不让晚到的正文丢掉应答 | hyper 写完应答后读一次正文，没读全就关读端并结束连接（锁定的 hyper 1.11.1，`proto/h1/conn.rs` 的 `poll_drain_or_close_read`）；`axum::serve` 不把连接交出来，但它的 `Listener` 是一个可以自己实现的 trait（axum 0.8.9：`type Io`、`type Addr`、`accept`、`local_addr`），连接结束就是 `Io` 被丢掉；`ConnectInfo<SocketAddr>` 只为 `TcpListener` 实现。RFC 9112 第 9.6 节、nginx 的 `lingering_close`、Go 的 `closeWriteAndWait` 说的是同一种做法 | 连接一层的分步关闭（任务 11）：自己的 `Listener`，`Io` 被丢掉时关写端、读掉晚到的字节、再关；`serve` 等这些任务结束。不取"答复之前先读完正文"的中间件：盖不住鉴权一层的拒绝，又把答复拖到正文到齐之后。代价：一条对端不肯关的连接多留至多 1 秒，退出至多因此晚 1 秒 |
| P30 | 集合文件里带未知参数的行怎么报 | `parse_subrule` 把值之后不认识的每一项收进 `SubRule.unknown`；`parse_rule_line` 原样交出 `SubRule`，之后没有人读这一项。内联小节由加载器逐行给 `W0003`、文本引用参数。集合文件是下载到的任意文本，M5a 已经定了域名集读不了的行不引用 | 每个集合数行数、记第一处的行号，建注册表时一条 `W0003`（不引用参数），进状态与端点（任务 10） |
| P31 | 启动之后集合跳过了行怎么说 | `SetRegistry::push_diag` 在 `building` 之外什么也不做；刷新由每个集合自己的任务在 `spawn_reloader` 里做，`finish` 只更新计数 | 三个数不全为零、且与上一次载入不同时一行 WARN，只带数与行号；数没变的刷新不重复说。端点上的三个字段是随时看得到的出口 |
| P32 | 几十份集合同时拿到新内容 | 每个集合的重载任务各自在运行时的线程上解析并编译；冷启动时全部资源同时下载，任务 9 的 `all` 随时可以再来一次；一份满员的集合编译时的峰值约是建好之后的三倍（M5a 实测） | 同一个注册表的重编译由一把异步锁排队，一次一个；`all` 的下载不设并发上限（与冷启动相同，每份至多 64 MiB 的正文）。编译不搬到阻塞线程池：与今天相比不多占线程，改动最小（延后事项 21） |
| P33 | 需求复读放在哪里、产出什么 | CLAUDE.md："一个阶段要在它的需求逐句对照过代码之后才称齐备"；2026-10-10 的审计判为"做了"的没有人复核，M5a 在其中查出一条只做了一半的；阶段 1 没有逐条的验收文档，只有手工清单 | 作为本计划的一个任务（任务 12），排在全部代码任务之后、文档之前；产出 `docs/acceptance/phase1-acceptance.md`；小的缺口当场补，要设计的登记；"齐备"写不写由它的结论定 |
| P34 | 新诊断码的建议 | M5a 的三张表（`WITH_DEFAULT`、`PER_SITE`、`WITHOUT`）与 `every_code_is_decided_once`；`W0011` 今天在 `WITHOUT`，理由是"这个版本不读远程 include" | `W0033`、`W0034` 各有两处、各带各的建议（`PER_SITE`）；`I0004` 是告知（`WITHOUT`）；`W0011` 留在 `WITHOUT`、理由改写；`external` 那条 `W0033` 文本末尾括号里的做法移到建议里 |

## 执行期修正记录

| # | 任务 | 与计划的出入 | 处理 |
| --- | --- | --- | --- |

## 延后事项

| # | 事项 | 去向 |
| --- | --- | --- |
| 1 | Profile 来源的下载走直连，不经策略（与订阅、规则集相同）：只有经代理才够得到的服务端取不到 | 项目所有者；阶段 3 之后再看 |
| 2 | `meta.json` 里存着完整的 URL（带令牌）；数据目录的权限是使用者自己的 | 项目所有者（早于本计划，订阅也是这样） |
| 3 | 同一个 URL 既是 Profile 的来源又是规则集时，两个管理器写同一个缓存目录 | 有用户报告再说 |
| 4 | 修改时间只到秒（HFS+）或 2 秒（FAT）的文件系统上，"检查之后的同一秒内改了本地文件"会被判成没改 | 有用户报告再说 |
| 5 | `--watch` 的路径表在启动时算一次；托管配置的副本生效时，本地 include 的集合可能随更新而变 | 下次动到 `--watch` 时 |
| 6 | `#!include` 的目标表按逗号分，带逗号的 URL 要另想写法 | 有用户报告再说 |
| 7 | 手册："Loading fails if two included files provide a section with the same name, or if an included section duplicates one already declared in the profile."——rurge 以 `W0020` 加载并取第一处定义，比手册说的 Surge 宽（Surge 拒绝的 Profile 在这里带着告警加载，Surge 接受的没有一份被拒绝） | 已核对并登记（阶段 1 / M5a，兼容性清单 1.3 节，决定保留）；任务 3 补一条被 include 的文件带来重名的用例 |
| 8 | 放开"下载到的文本里的 `external` 策略"的命令行开关 | 项目所有者 |
| 9 | 托管配置能改 `http-api` 与监听地址（与 Surge 相同）；监听地址有 `--listen-http` / `--listen-socks5` 可以钉住，`http-api` 没有对应的选项 | 项目所有者 |
| 10 | 外部资源的 CLI 子命令；`rurge status` 不显示暂停 | 阶段 6 |
| 11 | Logbook 与 `profile-reloaded` 事件脚本；托管配置更新后的记录目前是日志 | 阶段 6 与阶段 5 |
| 12 | 对真实服务商的托管配置、远程 include 与 `strict` | 手工验收清单 |
| 13 | 暂停不拦没开跟随出站模式时的 DNS 上游连接，也不拦测速 | 下次动到时 |
| 14 | 换了地址之后第一跳不再刷新：服务商日后把旧地址改指第三处时不会跟过去，缓存被清掉而旧地址已关时要使用者自己改本地那一行（有一条 `I0004` 提醒） | 有用户报告再说 |
| 15 | 互相要一起换的两个来源里有一个长期下载不到新版本时，另一个的新版本也一直留着（每次重试再判） | 有用户报告再说 |
| 16 | 新版本提到的新来源下载失败时，新版本要等到托管来源自己的下一次重试（至少 60 秒）才采用 | 有用户报告再说 |
| 17 | 一并采用不是原子的：恰在两次写缓存之间开始的一次重载（文件监视、SIGHUP、API 触发的）会读到混合的一组——要么加载不了（旧的一代照常运行，约一秒后 `changed()` 再来一次），要么成为存续约一秒的一代；别的进程（`rurge check`）同样可能读到 | 有用户报告再说 |
| 18 | 本地文件生效期间采用的远程 include 是照本地文件判定的；一次"没有变化"的应答让副本重新生效时不再判定，两者不相容时那次重载失败（旧的一代照常运行），下一次启动报错——使用者动一下本地文件即可脱身 | 有用户报告再说 |
| 19 | 集合文件里的要求指令不是指令：写在行首（`#!MACOS-ONLY DOMAIN,a.com`）时整行在所有平台上都被当成注释丢掉，写在行尾时这一行被收下、取值里带着指令的文字、永远匹配不到；内联的 `[Ruleset X]` 小节由 Profile 的读取器拆出指令。手册的两页说的都是 Profile 的行，对外部集合文件只字未提，Surge 认不认不知道（M5a 延后事项第 30 行，早于两份计划） | 有用户报告再说 |
| 20 | 连接收尾的三个数（1 秒、256 KiB、64 条）是写死的 | 有用户报告再说 |
| 21 | 集合的编译仍在运行时的线程上：一份大集合编译期间占住一个工作线程（时间没有量过），现在一次只占一个 | 下次动到时 |
| 22 | `[Rule]` 里几条 `FINAL` 的读法（匹配在第一条处结束、策略取最后一条）是从手册的三句话推断的，没有在 Surge 上观察过 | 项目所有者（手边有 Surge 时核对） |
| 23 | 正文晚到的客户端在 Linux 与 macOS 上丢不丢应答，改动之前与之后都没有量过（自动化用例在那两个系统上断言的是同一件事，但它们在改动之前红不红不知道） | 第一次 CI 运行；手工清单 |
