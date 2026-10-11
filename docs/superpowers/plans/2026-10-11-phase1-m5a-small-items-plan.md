# 阶段 1 / M5a（补遗：零散条款）实施计划

> **执行方式**：子代理驱动（superpowers:subagent-driven-development）——每个任务一个新的实现者，做完由新的评审者评审，全部做完后整分支评审一次。本计划写到"改哪里、接口是什么、要哪些用例、判据是什么"为止：实现者先写失败的用例、看到红，再实现到绿；与计划的出入记进末尾的「执行期修正记录」。

**目标**：2026-10-10 逐条核对查出的阶段 1 零散缺口全部落地——`pre-matching` 规则优先、子规则命中的日志、同一资源不可同时作两种集、平台变量取自操作系统、`#!WINDOWS-ONLY` / `#!LINUX-ONLY`、监听覆盖、`rurge check` 的建议、请求记录的监听端口 / 远端地址 / DNS 耗时 / 备注、当前 Profile 记录、GeoIP 数据库日期可查询，并给有实现而没有用例的几条补上用例。

**做法**：每一项都落在已有的结构里，互相之间没有依赖。规则引擎多一趟不解析的预匹配；会话的远端地址与 DNS 耗时经一个任务局部的 `Trail` 从连接器与解析器带回，不改 `ConnectOpts` 一类到处构造的接口；平台变量的读取分成与平台无关的纯函数和各平台很薄的读取。不新增 crate。

**依据**：阶段 1 / M5 设计（`docs/superpowers/specs/2026-10-11-phase1-m5-addendum-design.md`）第 4 ～ 6 节；缺口的清单与证据在 `docs/acceptance/phase2-acceptance.md` 的「需求逐条对照代码」一节。

## 全局约束

- 不新增 crate，`Cargo.lock` 里不出现新的包。允许的新依赖边：`rurge-engine` 的 dev-dependency `tracing-subscriber`（任务 6 的日志捕获），`rurge` 在 Unix 上的 dev-dependency `nix`（特性 `signal`，任务 6 的 SIGHUP 用例）。
- 全工作区 `forbid(unsafe_code)`，`rurge-platform` 的两个例外不增加；MSRV 1.89，edition 2024。
- 凭据不进日志、错误文本、诊断、`Debug` 与 API 输出：诊断与建议不引用配置里的取值，集合与资源的 URL 只以不带用户名口令的样子出现，命令行上给的监听口令不被回显。
- 测试只用回环地址与端口 0，等待都有时限，不用固定的 sleep 当同步；不联网；不改系统设置（不运行 `rurge run --system-proxy`，`rurge service install` / `uninstall` 只带 `--dry-run`）。
- 不下载、不安装任何东西；只运行本仓库构建出来的二进制。
- Surge 语法就是 rurge 语法：rurge 专有的运行时选项只走命令行与环境变量（FR-CFG-17）；与 Surge 的每一处行为差异都进兼容性清单（任务 7）。
- 注释、日志与错误文本用英文，照所在文件的写法；提交说明的标题用中文。
- 门禁：`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test -j 8 --workspace --no-fail-fast`（本机 `RUSTFMT` 指向 gnu 工具链的 rustfmt）。测试二进制无失败用例的异常退出（`STATUS_HEAP_CORRUPTION`）单独重跑那个二进制。

## 评审要点

计划的用例之外最可能咬到使用者的五种情形，各自归到拥有那段代码的任务里：

1. 一条规则在预匹配里没命中、在自己的位置上命中：命中计数只加一次，`rurge rule match --explain` 的轨迹里它只出现一次；预匹配引用的集合还没下载到（`Missing`）时当作不匹配，不报错、不把所有会话都拒掉（任务 1）。
2. 同一个集合以**同一种**身份出现多次（`RULE-SET,x,A` 与 `RULE-SET,x,B`，或 `[Rule]` 的 `RULE-SET,x` 与 `[Host]` 的 `RULE-SET:x`）是合法的，不能报 `E0024`（任务 1）。
3. 给了 `--listen-http` 而配置里有几条 `http-listen`：全部被替换；之后配置自己改了 `http-listen` 再重载，监听端不重绑（任务 2）。
4. 平台变量读不到——注册表键不在、命令不存在、文件不可读、输出不是预期的样子——时静默退回原来的取值，启动与 `check` 不因此失败、也不多出告警（任务 3）。
5. 两条会话同时拨号时各记各的远端地址与 DNS 耗时；`smart` 组换成员后记的是最后连上的那个成员的地址（任务 5）。

---

## 文件结构

| 文件 | 任务 | 职责 |
| ---- | ---- | ---- |
| `crates/rurge-rules/src/engine.rs` | 1 | 预匹配这一趟、`Reason::PreMatching` |
| `crates/rurge-rules/src/registry.rs` | 1 | `W0032`：另一种身份已登记的引用 |
| `crates/rurge-config/src/{diagnostic.rs,config.rs}` | 1、4 | `E0024` / `W0032` 两个码与加载期的检查；缺省建议 |
| `crates/rurge-inbound/src/session.rs` | 1、5 | 会话的备注；会话持有 `Trail` |
| `crates/rurge-engine/src/{engine.rs,observe.rs}` | 1、5 | 子规则的日志与备注；请求记录的四项 |
| `crates/rurge-api/src/routes/requests.rs` | 1、5 | JSON 的四项 |
| `crates/rurge/src/cli/rule.rs` | 1 | `rule match` 显示预匹配 |
| `crates/rurge-config/src/requirement.rs` | 2 | 两个简写 |
| `crates/rurge-config/src/general.rs` | 2 | 监听地址取值的公开解析入口 |
| `crates/rurge/src/cli/run.rs` | 2 | `--listen-http` / `--listen-socks5`；启动时写 `current_profile` |
| `crates/rurge-platform/src/sysinfo.rs`（新）、`crates/rurge/src/cli/mod.rs` | 3 | 平台变量 |
| `crates/rurge-net/src/trail.rs`（新）、`src/connector.rs` | 5 | `Trail` 与连上之后的登记 |
| `crates/rurge-dns/src/resolver.rs` | 5 | 查找结束时的登记 |
| `crates/rurge-rules/src/geoip.rs`、`crates/rurge-engine/src/views.rs`、`crates/rurge-api/src/routes/geoip.rs`（新）、`crates/rurge-api/src/lib.rs` | 5 | `GET /v1/geoip` |
| `crates/rurge-engine/tests/`、`crates/rurge/tests/cli.rs`、`crates/rurge-api/tests/api.rs`、`crates/rurge-rules/tests/` | 1 ～ 6 | 用例 |
| 文档 | 7 | 兼容性清单、需求文档、API 参考、验收文档、README、CLAUDE.md、M2 设计 Q3、本计划的两张表 |

---

## Task 1：规则引擎——预匹配优先、子规则的日志、同一资源两种集

**文件**：`crates/rurge-rules/src/{engine.rs,registry.rs}`、`crates/rurge-config/src/{diagnostic.rs,config.rs}`、`crates/rurge-inbound/src/session.rs`、`crates/rurge-engine/src/{engine.rs,observe.rs}`、`crates/rurge-api/src/routes/requests.rs`、`crates/rurge/src/cli/rule.rs`；用例在各 crate 的单元测试、`crates/rurge-engine/tests/pipeline.rs`、`crates/rurge-api/tests/api.rs`、`crates/rurge/tests/cli.rs`。

**接口**（后面的任务照这些名字用）：

```rust
// rurge-rules::engine
pub enum Reason { /* 原有的 */, PreMatching }        // as_str() == "pre-matching"

// rurge-inbound::session
impl SessionHandle {
    pub fn add_note(&self, note: impl Into<String>);
    pub fn notes(&self) -> Vec<String>;
}

// rurge-engine::observe
pub struct RequestRecord { /* 原有的 */, pub notes: Vec<String> }   // JSON: "notes"，没有时是空数组

// rurge-config::diagnostic::codes
pub const E_SET_KIND_CONFLICT: &str = "E0024";
pub const W_SET_KIND_CONFLICT: &str = "W0032";
```

**预匹配**（设计 4.1）：`RuleEngine::run` 在规则模式下、正常顺序之前，按 `PreMatchingSet::positions()` 的顺序把每条带 `pre-matching` 的规则评估一次，`no_resolve` 一律按真。命中的第一条即结果：`reason = Reason::PreMatching`、`matched` 是它、命中计数加一、`sub_rule` 照常、`resolved` 为空。都不命中时走原来的循环，一行不改。`pre_match_domain` / `pre_match_ip` 与这一趟共用同一段评估，行为不变。`evaluate_traced` 只为预匹配里命中的那条加一步，`verdict` 是 `pre-match`。`rurge rule match` 的输出不用另改（`reason:` 一行打印 `as_str()`），`--explain` 的轨迹照现有格式多出那一步。

**子规则**（设计 4.2）：`Engine::choose_policy` 在 `decision.sub_rule` 有值时，`handle.add_note("Sub-rule matched: <entry> (in <set>)")`，并记一条同文的 `tracing::debug!`（字段 `session = handle.id()`）。`RequestRecord.notes` 取自 `handle.notes()`；API 的 JSON 加 `notes`。句柄上已有的 `set_error` / `error` 不动：今天写在 `error` 里的说明（组环、空组兜底、`policy does not support UDP; sent through DIRECT` 一类）是 API 参考里登记过的文本，留在原处，不搬进 `notes`。

先修一处现在就有的错，否则这句日志会说错话：`EvalCtx.sub_hit` 在一次评估的全部规则之间共用，`Matcher::Set` 一命中就写它，不管外面那条规则最后命中没有——`AND,((RULE-SET,x),(DEST-PORT,1)),A` 里集合命中而端口不符时，这条规则不命中，`sub_hit` 却留了下来，被后面命中的任何一条规则当成自己的交出去。`run` 的两趟（预匹配与正常顺序）都在评估每条规则之前把 `ctx.sub_hit` 清掉；`Decision.sub_rule` 因此只属于命中的那条规则。

**两种集**（设计 4.3）：

- `rurge-config`：`config.rs` 的 `validate`（规则引用的策略、缺 `FINAL` 这些跨节的检查所在的那个函数）里加一段，在规则、`[Host]` 与内联集都解析完之后跑。按配置里的出现顺序收集 `(引用, 身份, 位置)`：`[Rule]` 的顶层规则与逻辑规则的子规则（递归）、`[Host]` 的 `HostKey::RuleSet` / `HostKey::DomainSet`、内联 `[Ruleset]` 里的集合行；只收 `ResourceRef::Url` 与 `ResourceRef::File`，被 `#!REQUIREMENT` 关掉的条目不收。一个引用先以一种身份出现、后以另一种出现时，在后者的位置报 `E0024`：`the same resource cannot be both a RULE-SET and a DOMAIN-SET (first used as a <RULE-SET|DOMAIN-SET> at <先出现处的位置，照 Span 的显示>)`；同一个引用只报一次。文本里不出现引用本身。位置是 `Span`（显示为 `文件:行`）：规则与 `[Host]` 条目用自己的，逻辑规则的子规则用所在规则的，内联集里的行没有自己的位置（`SubRule` 不带 `Span`）、用所在集合的。两个引用是不是同一个，按 `ResourceRef` 的相等判断：路径中间的 `.`（`./a.list`）不算差别，大小写不同或带 `..` 的两种写法认不出来（登记在任务 7）。
- `rurge-rules`：`SetRegistry::ensure` 在 `key` 是 `Key::Url` / `Key::File`、要登记的身份不在而另一种身份已在时，`push_diag(W0032)`（`set `<显示名>` ignored: the same resource is already used as a <另一种>`）、记一条同文的告警日志，返回空集合；不为它建条目。

**用例**：

- `rurge-rules`（单元）：带 `pre-matching` 的 REJECT 规则写在一条同域名的 DIRECT 规则之后仍然先命中，`reason` 是 `PreMatching`，那条规则的命中计数是 1、DIRECT 那条是 0；`IP-CIDR,…,REJECT,pre-matching` 对一个域名目标——预匹配这一趟不调用解析器（用一个数调用次数的解析器），到它自己的位置解析后命中，`reason` 是 `Rule`，命中计数是 1；直连与全局模式不看预匹配；`RULE-SET,…,REJECT,pre-matching` 命中时 `sub_rule` 有值；预匹配引用的集合是空的（`Missing`）时不命中；轨迹里预匹配命中时只有一步 `pre-match`，不命中时与原来逐步相同。
- `rurge-rules`（单元）：`AND,((RULE-SET,x),(DEST-PORT,1)),A` 不命中（集合那一半命中）、其后的 `DOMAIN,…,B` 命中时 `sub_rule` 为空——修之前这条用例是红的；`NOT,((RULE-SET,x)),A` 因集合不命中而命中时 `sub_rule` 为空；一条 `RULE-SET` 规则要解析才命中（集合里是 IP 行、目标是域名）时 `sub_rule` 是解析之后命中的那一行。
- `rurge-rules`（单元，`registry`）：两个文件——`a.list` 先以顶层 `RULE-SET` 登记，`b.list` 也是顶层 `RULE-SET`、里面有一行 `DOMAIN-SET,a.list`——`W0032` 恰好一条，`b.list` 里嵌套的那一行不匹配、它别的行照常匹配，`a.list` 作为规则集照常匹配，`statuses()` 里 `a.list` 只有 `RULE-SET` 一项；反过来的顺序（先 `DOMAIN-SET` 后被嵌成 `RULE-SET`）同样一条；文件自己嵌自己仍是原来的 `W0025`（环在先判）；URL 带用户名口令时 `W0032` 的文本里只有 `***`。
- `rurge-config`（单元）：`[Rule]` 里 `RULE-SET,u` 与 `DOMAIN-SET,u`——`E0024` 在后一行；`[Host]` 的 `DOMAIN-SET:u` 与 `[Rule]` 的 `RULE-SET,u`；逻辑规则的子规则里的引用；内联集里的集合行；同一路径的两种写法（`a.list` 与 `./a.list`）算同一个；同一种身份出现两次不报（`RULE-SET,u,A` 与 `RULE-SET,u,B`；`[Rule]` 的 `RULE-SET,u` 与 `[Host]` 的 `RULE-SET:u`）；被关掉的行不算；`SYSTEM` / `LAN` 与内联集的名字不查；带用户名口令的 URL 的 `E0024` 文本里找不到口令。
- `rurge-engine`（`tests/pipeline.rs`）：会话的目标被一条写在后面的 `pre-matching` REJECT 规则命中——被拒绝，请求记录的 `rule` 是那一条；出站模式切到直连后同一个目标照常连通。经本地文件的 `RULE-SET` 命中的会话——请求记录的 `notes` 是 `["Sub-rule matched: DOMAIN-SUFFIX,… (in …)"]`。
- `rurge-api`：`requests/recent` 的 JSON 里有 `notes`。
- `rurge`（`tests/cli.rs`）：`rule match` 对预匹配命中的目标打印 `reason: pre-matching`；`check` 对两种身份的配置以 2 退出并打印 `error[E0024]`。

**判据**：`cargo test -p rurge-rules`、`-p rurge-config`、`-p rurge-engine --test pipeline`、`-p rurge-api`、`-p rurge --test cli` 全绿；`cargo clippy --all-targets -- -D warnings` 无告警。

---

## Task 2：配置层——两个简写、监听覆盖、当前 Profile 记录

**文件**：`crates/rurge-config/src/{requirement.rs,general.rs}`、`crates/rurge/src/cli/run.rs`；用例在 `requirement.rs` 的单元测试与 `crates/rurge/tests/cli.rs`。

**接口**：

```rust
// rurge-config::general —— 命令行取值与配置取值共用 parse_listener
/// The value of `http-listen` given outside a profile: one or more `[password@]address[:port]`, comma separated.
pub fn parse_http_listen(value: &str) -> Result<Vec<Listener>, &'static str>;      // 缺省端口 6152
/// The value of `socks5-listen` given outside a profile; a password is an error.
pub fn parse_socks5_listen(value: &str) -> Result<Vec<Listener>, &'static str>;    // 缺省端口 6153

// rurge::cli::run::RunArgs
#[arg(long, env = "RURGE_LISTEN_HTTP", value_name = "ADDR", hide_env_values = true)]
pub listen_http: Option<String>,
#[arg(long, env = "RURGE_LISTEN_SOCKS5", value_name = "ADDR", hide_env_values = true)]
pub listen_socks5: Option<String>,
```

**简写**（设计 5.1）：`SIMPLIFIED` 加 `("WINDOWS-ONLY", "SYSTEM == 'Windows'")` 与 `("LINUX-ONLY", "SYSTEM == 'Linux'")`。

**监听覆盖**（设计 5.2）：两个取值在 `run()` 里、加载 Profile 之前由上面两个函数解析，不交给 clap 的 `value_parser`（clap 的报错会把取值原样打印出来，而 `http-listen` 的取值可以带口令）；写得不对时打印 `error: --listen-http: <原因>`（或 `--listen-socks5`）并以 2 退出，原因里不含取值；这条提前退出同 Profile 加载不了的那两条一样，退出之前调用 `recover_without_a_log(&args.runtime)`（上一次崩溃留下的系统代理照样恢复）。一项也没有的取值（空串、只有逗号）与 `socks5` 的取值带了口令是同样的错误。解析结果存进一个 `Overrides { http: Option<Vec<Listener>>, socks5: Option<Vec<Listener>> }`（可以派生 `Debug`：`Listener` 手写的 `Debug` 已经把口令显示成 `<redacted>`）；`fn apply(cfg: &mut Config, o: &Overrides)` 在启动与 `reload` 里、`load_checked` 成功之后、`build_engine_runtime` 之前各调用一次：给了哪一种就整项替换 `cfg.general` 里的那一种。`Daemon` 多带一个 `overrides`。重载时监听端重不重绑照旧由 `Engine::swap_runtime` 比较前后两份 `general` 决定——两份都已套过覆盖，所以配置自己改 `http-listen` 不再引起重绑。

**当前 Profile 记录**（设计 5.5）：`run()` 的主运行时里，在全部可能失败的启动步骤之后（监听端与 API 都已绑定、`--system-proxy` 已启用，打印 `rurge … running` 那一行的地方）`store.update(|s| s.current_profile = Some(<-c 的绝对路径>))`——一次没起来的启动不把它没跑起来的 Profile 记成当前的。绝对路径用 `std::path::absolute`（不解析符号链接，Windows 上不带 `\\?\` 前缀），它失败时用给的路径。`StateStore::update` 每次都重写文件，所以先同 `StateStore::open` 交回的 `State` 比较，值没变时不调用（`state.json` 不因一次启动而重写）。

**用例**：

- `requirement.rs`（单元）：`#!WINDOWS-ONLY x` 与行尾的 `x #!WINDOWS-ONLY`、`x //!WINDOWS-ONLY` 各自只在 `SYSTEM` 是 `Windows` 时生效，`LINUX-ONLY` 同理；`#!WINDOWS-ONLYX` 不是简写。
- `rurge-config`：`redact_profile` 对 `#!WINDOWS-ONLY http-api = key@127.0.0.1:6170` 照样抹掉口令（沿用 `#!MACOS-ONLY` 现有用例的写法）。
- `general.rs`（单元）：两个函数对 `127.0.0.1`、`127.0.0.1:0`、`[::1]:7000`、`pw@127.0.0.1:1`、逗号分开的两项、空串、`,`、`localhost:1`；`parse_socks5_listen("pw@127.0.0.1")` 是错误；错误文本里没有取值。
- `run.rs`（单元，`apply`）：配置里有两条 `http-listen` 与一条 `socks5-listen`——只给 HTTP 的覆盖时 HTTP 只剩覆盖给的那些、SOCKS5 不变；两种都给；都不给时配置原样。
- `cli.rs`：配置的 `http-listen` 不带口令，`--listen-http '<口令>@127.0.0.1:0'` 启动——不带凭据的代理请求得到 407，带上就通；经 API 重载之后仍是 407，重载的应答里 `listeners_rebound` 为假；把配置里的 `http-listen` 改成另一个地址再重载，仍然不重绑、原来的端口照常应答；`RURGE_LISTEN_SOCKS5='[::1]:0'` 时 SOCKS5 监听端在 `::1` 上（机器没有 IPv6 回环时跳过这一条，照仓库里现有的 IPv6 用例的写法）；`--listen-http '<口令>@not-an-address'` 以 2 退出，标准错误里没有那段口令；`--help` 的输出里没有环境变量的取值；只给 `--listen-http` 时 SOCKS5 监听端照配置；数据目录里留着上一次崩溃的系统代理备份（文件后端，照现有"坏配置时也恢复"那组用例的写法）时，`--listen-http` 写错的这次启动同样把它恢复。
- `cli.rs`：启动之后（有时限地等）数据目录的 `state.json` 里 `current_profile` 是 Profile 的绝对路径；用相对路径的 `-c` 启动时记下的仍是绝对路径；同一份 Profile 再启动一次，`state.json` 的字节不变；监听端绑不上的那次启动（端口被占，以 1 退出）不留下 `current_profile`；换一份 Profile 启动后它变成新的那份。

**判据**：`cargo test -p rurge-config`、`-p rurge --test cli` 全绿。

---

## Task 3：平台变量取自操作系统

**文件**：`crates/rurge-platform/src/sysinfo.rs`（新）、`crates/rurge-platform/src/lib.rs`、`crates/rurge/src/cli/mod.rs`；用例在 `sysinfo.rs` 与 `crates/rurge/tests/cli.rs`。

**接口**：

```rust
// rurge-platform::sysinfo
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SystemInfo {
    pub version: Option<String>,    // "System Version 14.6.1 (Build 23G93)"
    pub model: Option<String>,
    pub language: Option<String>,   // "en-US"
    pub name: Option<String>,
}
/// Best effort: whatever the system does not say is `None`. Never fails. Reads once per process.
pub fn read() -> SystemInfo;

/// Where a reader gets what it reads; `read` passes the real ones.
pub struct Sources<'a> {
    pub runner: &'a dyn CommandRunner,
    /// A file's text; `None` when it cannot be read.
    pub file: &'a dyn Fn(&str) -> Option<String>,
    pub env: EnvLookup<'a>,             // rurge_platform::dirs::EnvLookup
}
pub fn read_macos(from: &Sources<'_>) -> SystemInfo;     // 三个平台都编译、都测
pub fn read_linux(from: &Sources<'_>) -> SystemInfo;     // 同上
// #[cfg(windows)] fn read_windows() -> SystemInfo;      // 注册表，只在 Windows 上编译
```

照本 crate 现有的做法（`dirs` 的"每个平台的规则在每台机器上都测"、`sysproxy` 的三个后端都编译而只有选择带 `cfg`）：macOS 与 Linux 的读取吃 `Sources`，用例里给假的命令（`command::testing::FakeRunner`）、假的文件与假的环境变量，在本机就能跑；`read()` 按 `cfg` 选一个并给真的来源。

与平台无关的纯函数（私有，三平台都编译、都测）：`system_version(version, build) -> String`；`macos_version(plist: &str) -> Option<(String, String)>`（取 `ProductVersion` 与 `ProductBuildVersion` 两个 `<key>` 后面的 `<string>`）；`windows_version(major, minor, build: &str, ubr: Option<u32>) -> (String, String)`；`linux_version(osrelease: &str) -> Option<(String, String)>`（版本取开头的 `数字(.数字)*`，构建是整串）；`model(raw: &str) -> Option<String>`（去空白与 NUL；空的与厂商的占位值不算：`To Be Filled By O.E.M.`、`System Product Name`、`Default string`、`None`、`Not Specified`，不分大小写）；`posix_language(lc_all, lc_messages, lang) -> Option<String>`（第一个非空的；去掉 `.` 之后的编码与 `@` 之后的修饰，`_` 换 `-`；`C` 与 `POSIX` 不算）；`apple_languages(output: &str) -> Option<String>`（`defaults read -g AppleLanguages` 的输出里第一项，带不带引号都认）；`name(raw: &str) -> Option<String>`（去空白，空的不算）。

各平台的读取（设计 5.4 的表）：Windows 用 `windows_registry::LOCAL_MACHINE` / `CURRENT_USER` 的 `open(..)`（只读打开）与 `get_u32` / `get_string` / `get_multi_string`——版本取 `CurrentMajorVersionNumber`、`CurrentMinorVersionNumber`（两个 DWORD）、`CurrentBuildNumber`（字符串）与 `UBR`（DWORD，可以没有）；语言依次看 `Control Panel\Desktop` 的 `PreferredUILanguages`、`Control Panel\International\User Profile` 的 `Languages`（都是多字符串，取第一项）、`Control Panel\International` 的 `LocaleName`；macOS 读 plist 文件（`/System/Library/CoreServices/SystemVersion.plist`），命令是 `sysctl -n hw.model`、`defaults read -g AppleLanguages`、`scutil --get ComputerName`（没有时 `scutil --get LocalHostName`），语言读不到时照 Linux 的三个环境变量；Linux 只读文件与环境变量（`/proc/sys/kernel/osrelease`；`/sys/class/dmi/id/product_name`，没有时 `/proc/device-tree/model`；`/proc/sys/kernel/hostname`，没有时 `/etc/hostname`、`HOSTNAME`）。任何一步出错都只是那一项为 `None`，不记日志。macOS 的三条命令各自是一次进程创建：`read()` 每个进程只真的读一次（`OnceLock`），`rurge run` 的重载不重读。

`cli::environment`：四项各自 `sysinfo::read()` 的值，`None` 时退回原来的取法（`unknown`、`std::env::consts::ARCH`、`$LANG` 的旧逻辑直至 `en-US`、`COMPUTERNAME` / `HOSTNAME` 直至 `rurge`）。

**用例**：

- `sysinfo.rs`（单元，纯函数）：每个函数的正常样本与坏样本——macOS 的 plist（键的顺序不同、缺一个键、不是 plist）；Windows 的四个值（有无 `UBR`）；`osrelease` 的 `6.8.0-45-generic`、`5.15.153.1-microsoft-standard-WSL2`、空串；占位型号；`zh_CN.UTF-8`、`en_US`、`sr_RS@latin`、`C`、`POSIX`、`C.UTF-8` 与三个变量的先后；`AppleLanguages` 的多行输出（`(\n    "en-US",\n    "zh-Hans-US"\n)`）、单项、空表；带换行的主机名。
- `sysinfo.rs`（单元，两个读取）：`read_macos` 对一套完整的假来源给出四项；每条命令失败、输出为空或文件读不到时只有那一项是 `None`（`ComputerName` 失败而 `LocalHostName` 有、`AppleLanguages` 失败而 `LANG` 有的两种退路各一条）；它发出的命令恰好是上面列的那几条（`FakeRunner::calls()`），没有别的。`read_linux` 同样：`product_name` 是占位值而 `/proc/device-tree/model` 有（带结尾的 NUL）、三处主机名的先后、三个语言变量的先后、什么都读不到时四项全是 `None`。
- `sysinfo.rs`（本机）：`read()` 不 panic、两次调用结果相同；`version` 有值时以 `System Version ` 开头并含 ` (Build `；`#[cfg(windows)]` 的一条——本机的 `read().version` 有值（注册表读得到）。
- `cli/mod.rs`（单元）：`environment` 的取值部分抽成一个吃 `SystemInfo` 的函数——四项都是 `None` 时得到的与原来逐字相同（`unknown`、CPU 架构、原来的语言与设备名逻辑）；只有一项有值时只换那一项。
- `cli.rs`：一份只有 `#!REQUIREMENT "SYSTEM_VERSION BEGINSWITH 'System Version '" …` 那一行能提供 FINAL 的配置，`rurge check` 通过（三个平台上都应读得到版本；读不到就是这条用例要抓的缺陷）。

**判据**：`cargo test -p rurge-platform`、`-p rurge --test cli` 全绿；`rurge-platform` 里没有新的 `#[allow(unsafe_code)]`。

---

## Task 4：诊断的建议

**文件**：`crates/rurge-config/src/diagnostic.rs`，以及因此变化的快照（`crates/rurge-config/tests/snapshots/`）与断言诊断全文的用例。

**接口**：

```rust
// rurge-config::diagnostic::codes
/// What to do about a diagnostic of this code, when there is one thing to say for every place that emits it.
pub fn hint(code: &str) -> Option<&'static str>;
```

`Diagnostic::error` / `warning` / `info` 构造时把 `codes::hint(code)` 填进 `hint`；`with_hint` 照旧覆盖它。文本输出与 `--json` 不用改。

**建议的写法**：照 `E0010` 现有的那一句（``add `FINAL,DIRECT` as the last rule``）——一句英文、祈使、小写开头、不带句号，配置里的字面量用反引号；说改配置的哪里；不引用任何取值；对发出这个码的**每一处**都成立。同一个码在几处因不同的原因发出、改法不同时，缺省建议只说共同的部分，或者不设缺省、各处用 `with_hint` 说自己的。消息自己已经把改法说完的，不为了"有"而重复一遍。

**覆盖**：没有一个码被漏掉——码表里的每个常量要么有缺省建议，要么它的每个发出点自带建议，要么列在"不设"的名单里并在码表旁的注释里写明理由。`E0009` 自阶段 2 / M3a 起没有发出点，列在名单里。

**初稿**。下表是照码的含义写的初稿，不是定稿：实现者对每一个码用 `git grep -n 'codes::<常量名>'` 列出全部发出点、读过每一处的文本之后，再决定这一句留、改还是不设——对任何一处不成立的不能留。每一处改动（连同"读了几处、哪一处不成立"）交回，记进「执行期修正记录」。初稿里的数字（10 层、8 层、1,000,000）以代码里的常量为准，对不上就改建议。

| 码 | 初稿 |
| --- | --- |
| `E0001` | ``compare the line with the syntax the Surge manual gives for this section`` |
| `E0002` | ``check the path after `#!include`; a relative path starts at the profile's directory`` |
| `E0003` | ``write the condition as the Surge manual does, for example `#!REQUIREMENT CORE_VERSION>=22` `` |
| `E0004` | ``use a policy type the Surge manual lists, and check its spelling`` |
| `E0005` | ``pick another name: the built-in policies cannot be redefined`` |
| `E0006` | ``give every policy and policy group a name of its own`` |
| `E0007` | ``define the policy in `[Proxy]` or `[Proxy Group]`, or correct the name`` |
| `E0008` | ``define what the group names in `[Proxy]` or `[Proxy Group]`, or remove the reference`` |
| `E0010` | 现有的一句，保留 |
| `E0011` | ``check the value against what the Surge manual gives for this rule type`` |
| `E0012` | ``check the rule type's spelling against the Surge manual`` |
| `E0013` | ``give an IP address: `127.0.0.1` for this machine only, `0.0.0.0` for every interface`` |
| `E0014` | 不设缺省，四处各带自己的：``move `pre-matching` to a top-level rule``、``give the rule a REJECT policy, or remove `pre-matching` ``、``remove `pre-matching`: this rule type cannot be checked before a connection exists``（这一句以实现者核对到的原因为准）、``keep `FINAL` as a top-level rule in `[Rule]` `` |
| `E0015` | ``split the rule: logical rules nest at most 10 levels deep`` |
| `E0016` | ``simplify the `#!include` lines: a file cannot include itself, directly or through another, and both the depth and the number of included files are limited``（三处：成环、过深、展开的文件过多） |
| `E0018` | ``check the parameter against the Surge manual's page for this policy type`` |
| `E0019` | ``break the loop: a policy cannot reach its server through itself`` |
| `E0020` | ``name a `[Keystore]` item of the kind this parameter needs``（两处：没有这一项、种类不对） |
| `E0021` | ``encode the file again as Base64, without line breaks`` |
| `E0024`（任务 1 新加） | ``use the resource as one kind of set only; when both kinds are needed, keep one copy for each`` |
| `W0001` | ``check the key's spelling; a key rurge does not know is ignored`` |
| `W0002` | ``check the section name's spelling; a section rurge does not know is ignored`` |
| `W0003` | ``check the parameter's spelling; it is ignored`` |
| `W0005` | ``correct or remove the entry; the rest of the list is used`` |
| `W0006` | ``remove the key: Surge no longer has it`` |
| `W0013` | ``correct or remove the rule: as written it never matches`` |
| `W0014` | ``check the directive's spelling; the line is read as a comment`` |
| `W0015` | ``move the line under a `[Section]` header`` |
| `W0017` | ``check the section name after the file name in the `#!include` line`` |
| `W0019` | ``move `FINAL` to the end: the rules after it never match`` |
| `W0021` | ``keep one `FINAL` rule`` |
| `W0024` | ``split the set: at most 1,000,000 entries are read`` |
| `W0025` | ``remove the reference that leads back to this set, or flatten the sets: they nest at most 8 levels deep`` |
| `W0030` | ``take one of the groups out of the other: a group cannot contain itself`` |
| `W0031` | ``move the server and this policy to an AEAD method such as `aes-128-gcm` or `chacha20-ietf-poly1305` `` |
| `W0032`（任务 1 新加） | ``use the resource as one kind of set only; when both kinds are needed, keep one copy for each`` |

初稿里"不设"的，理由各写一句进注释：`E0009`（没有发出点）；`E0017`（三处的消息就是改法：`expected `Name = ...``、`expected `key = value``、`expected `<host> = <value>``，各不相同）；`E0022`（原因在消息里，是构建出站时的原话）；`E0023`（消息已经说了缺什么或哪里不对；`section-name` 那一处的改法因节在不在而不同）；`W0004`、`W0009`、`W0010`、`W0016`、`W0027`（平台或阶段不适用，使用者没有要改的）；`W0007`、`W0008`（消息的每一种文本已经带着去处）；`W0011`（M5b 改写它）；`W0012`、`W0018`、`W0020`、`W0022`、`W0023`、`W0026`、`W0028`、`W0029`（发出点多、原因各异，消息自己说了是什么、怎样读）；`I0001` ～ `I0003`。实现者读过发出点之后认为其中某个码有一句对每一处都成立、而消息没有说的建议，可以加，交回时说明。

**用例**：

- 一条表驱动的用例：码表里的每个常量（用例里逐个列出；新加常量而没有列进来的由一条数目的断言抓住）要么有缺省建议、要么在"不设"的名单里，两边不重叠；每条建议不为空、不以句号结尾、不以大写字母开头、不含 `{`（没有漏掉的格式占位）。
- `E0010` 的显示与原来逐字相同；一个带 `with_hint` 的诊断显示的是自己的建议；`E0014` 的四处各自带着建议（四条用例，或现有用例里加断言）。
- `cli.rs`：`rurge check --json` 的诊断里有 `hint`；文本输出里 `hint:` 一行跟在它的诊断之后。
- 快照：本机没有 `cargo-insta`——`cargo test -p rurge-config` 留下的每个 `.snap.new` 对着它的 `.snap` 逐行看，只接受多出 `hint` 一行的那些（别的差异是缺陷），接受之后不留 `.snap.new`。

**判据**：`cargo test -p rurge-config`、`-p rurge --test cli`、`-p rurge-api`、`-p rurge-engine` 全绿（后两个里有断言诊断文本的用例）；`crates/rurge-config/tests/secrets.rs` 照旧通过（建议里没有取值）。

---

## Task 5：请求记录的四项与 `GET /v1/geoip`

**文件**：`crates/rurge-net/src/{lib.rs,trail.rs,connector.rs}`、`crates/rurge-dns/src/resolver.rs`、`crates/rurge-inbound/src/session.rs`、`crates/rurge-engine/src/{engine.rs,observe.rs,udp.rs,views.rs}`、`crates/rurge-rules/src/geoip.rs`、`crates/rurge-api/src/{lib.rs,routes/requests.rs,routes/geoip.rs}`；用例在各 crate 的单元测试、`crates/rurge-engine/tests/pipeline.rs`、`crates/rurge-api/tests/api.rs`。

**接口**：

```rust
// rurge-net::trail —— 一次拨号留给会话记录的东西
#[derive(Clone, Default)]
pub struct Trail { /* Arc：连到的地址、解析耗时合计、解析次数 */ }
impl Trail {
    pub fn new() -> Trail;
    /// Runs `f` with this trail as the one `connected` and `looked_up` write to.
    pub async fn follow<F: Future>(&self, f: F) -> F::Output;
    /// The address the last TCP connection made under this trail went to.
    pub fn peer(&self) -> Option<SocketAddr>;
    /// Forgets the address: what is noted next belongs to another attempt.
    pub fn clear_peer(&self);
    /// The time spent on name lookups; `None` when none was made.
    pub fn dns(&self) -> Option<Duration>;
}
pub fn connected(addr: SocketAddr);     // 不在任何 follow 里时什么也不做
pub fn looked_up(spent: Duration);

// rurge-inbound::session
impl SessionHandle { pub fn trail(&self) -> &Trail; }

// rurge-engine::observe::RequestRecord
pub listen_port: u16,                   // JSON: "listenPort"
pub remote_addr: Option<SocketAddr>,    // JSON: "remoteAddr"，没有时 null
pub dns_ms: Option<u64>,                // JSON: "dnsMs"，没有时 null

// rurge-rules::geoip
pub struct GeoAnswer { pub country: Option<String>, pub asn: Option<u32>, pub organization: Option<String> }
impl GeoDb { pub fn describe(&self, ip: IpAddr) -> GeoAnswer; }     // 不触发"库不在"的告警

// rurge-engine::views —— rurge-api 只依赖 rurge-engine 与 rurge-config，经这个视图拿
pub struct GeoIpView {
    pub country: Option<String>,
    pub asn: Option<u32>,
    pub organization: Option<String>,
    pub country_db_date: Option<u64>,   // GeoDb::info() 的构建时间，UNIX 秒
    pub asn_db_date: Option<u64>,
}
impl Engine { pub fn geoip_view(&self, ip: IpAddr) -> GeoIpView; }
```

**登记的位置**：`DirectConnector::connect` 在 `race(..)` 返回之后取这条流的对端地址调用 `trail::connected`（竞速里的每次尝试是 `JoinSet` 里另起的任务，看不到任务局部变量，所以不在尝试里登记；取不到对端地址时不登记）；`Resolver::lookup_inner`——`lookup` 与 `lookup_via` 都经它，解析器对外的三个 trait 入口又都经这两个——在返回之前以自己的耗时调用 `trail::looked_up`（成功失败都算，`[Host]` 与缓存命中的也算，数值接近 0）。地址字面量不是一次查找：`lookup_inner` 开头对字面量的那个提前返回（`Source::Literal`）不登记，目标是地址的会话 `dnsMs` 因此是 `null` 而不是 0。

**查找自己开的连接不算会话的**：解析器向上游发的每个查询都在 `fanout::resolve_name` 的 `JoinSet` 里另起任务，`tcp://`、`tls://`、`https://` 上游的 TCP 连接（连同它们自己要做的解析）因此不在会话的 `follow` 之内，不会被记成会话的远端地址，也不会重复计时。这一点现在靠的是那个 `JoinSet`，没有别的保证——由下面 `resolver.rs` 的一条用例钉住：哪天查询改成就地执行，那条用例变红，到时再加防护。跟随出站模式下，查询里建的内部 DNS 会话有自己的 `follow`，照常记自己的。

**作用域**：引擎里建会话句柄、随后为它开连接的四处，把建句柄之后的整段放进这条会话的 `Trail`（`let trail = handle.trail().clone(); trail.follow(async move { … }).await`）：`Dialer::dial`、`udp::run_flow` 里对 `open(..)` 的调用、`Engine::dial_internal`、`Engine::dial_internal_udp`。测速会话（`auto.rs`）不放，它的三项是空的。`Dialer::dial` 的换成员循环在每次尝试之前 `trail.clear_peer()`：上一个成员连上了 TCP 却在握手里失败、下一个成员走的是复用的连接时，记录里不能留着上一个成员的地址。UDP 的会话（`udp::run_flow` 的流与 `dial_internal_udp` 的内部会话）一律没有 `remoteAddr`：经代理的 UDP 载体由第一条用到它的流打开（`socks5` 的控制连接、`trojan` 的那条流），只有那一条会碰巧带上地址，后面共用载体的流没有，反而让人误读——这两处在打开之后 `trail.clear_peer()`；`dnsMs` 照记。`record_of` 从句柄取三项，`listen_port` 取 `handle.session().in_port`。

链式策略（`underlying-proxy`）的会话记下的是第一跳的地址——这台机器的 socket 实际连到的地方。

**`GET /v1/geoip?ip=<地址>`**（设计 6.1）：应答 `{"address","country","asn","organization","geoip-db-date","asn-db-date"}`；`asn` 是数字，两个日期是 `GeoDb::info()` 的构建时间（UNIX 秒），没有结果或库不在的项是 `null`；`ip` 缺失或不是地址时 400，正文照本 crate 其它 400 的形状。鉴权与其它端点相同。

**用例**：

- `trail.rs`（单元）：`follow` 之内 `connected` / `looked_up` 落到这个 `Trail`，之外不落；同一个 `Trail` 嵌套 `follow`；经 `tokio::time::timeout` 与 `select!` 的分支仍可见；两个并发的 `follow` 各落各的；`tokio::spawn` 出去的任务里不落（把这条限制钉成用例）；两次 `looked_up` 相加，`dns()` 在一次也没有时是 `None`、有一次耗时为 0 的查找时是 `Some(0)`；第二次 `connected` 盖掉第一次；`clear_peer` 之后 `peer()` 为空而 `dns()` 不变；`follow` 的 future 被丢弃（超时）之后已记下的仍读得到；`follow` 里再 `follow` 另一个 `Trail`，落到里面那个、外面那个不变，里面的结束之后又落到外面那个。
- `connector.rs`：`follow` 里经 `DirectConnector` 连一个回环监听端，`peer()` 是它的地址；连不上时 `peer()` 为空。
- `resolver.rs`：`follow` 里查一个 `[Host]` 里的名字，`dns()` 有值；同一个名字再查（命中缓存）仍有值；查一个地址字面量，`dns()` 仍是 `None`；上游是回环上的 `tcp://`（现有的 `MockDns`）时查一个名字——`dns()` 有值而 `peer()` 为空（查询要是在调用方的任务里就地执行，这条就红：记下的会是假 DNS 服务端的地址）。
- `pipeline.rs`：DIRECT 会话——`listen_port` 是监听端的端口、`remote_addr` 是回显服务端的地址、`dns_ms` 为空（目标是地址）；目标是 `[Host]` 里钉到回环的名字时 `dns_ms` 有值；经 `socks5` 假上游的会话 `remote_addr` 是上游的地址；被 REJECT 的会话 `remote_addr` 为空；两条并发会话去两个不同的回显服务端，各记各的；`smart` 组第一个成员连不上、第二个连上，记的是第二个的上游地址。`tests/udp.rs`：经 `socks5` 假上游的 UDP 流 `remote_addr` 为空（去掉那次 `clear_peer` 这条就红：记下的是假上游的 TCP 地址），`listen_port` 是 SOCKS5 监听端的端口。
- `api.rs`：`requests/recent` 的 JSON 里 `listenPort`、`remoteAddr`、`dnsMs`；`GET /v1/geoip`——把 `crates/rurge-rules/tests/fixtures/` 的两个测试库放进数据目录的 `geoip/`：库里有的地址给出国家、ASN 与组织，两个日期是库的构建时间；库里没有的地址三项为 `null`、日期仍在；没有库时五项都是 `null`；`ip=nonsense` 与不带 `ip` 是 400；不带 `X-Key` 是 401。

**判据**：`cargo test -p rurge-net trail`、`-p rurge-dns`、`-p rurge-engine --test pipeline`、`-p rurge-engine --test udp`、`-p rurge-api` 全绿。

---

## Task 6：补用例

**文件**：`crates/rurge/tests/cli.rs`、`crates/rurge/Cargo.toml`、`crates/rurge-engine/Cargo.toml`、`crates/rurge-engine/tests/common/`（日志捕获）、`crates/rurge-engine/tests/pipeline.rs`、`crates/rurge-rules/tests/memory.rs`（新）。本任务不改产品代码；用例暴露出缺陷时停下来报告。

- **FR-CFG-02**（`cli.rs`）：Profile 带一个本地 include 与一个 `select` 组；启动，经 API 重载一次、切一次组、停止；两个文件的字节与启动前逐字节相同。
- **FR-CFG-16**（`cli.rs`，`#[cfg(unix)]`）：启动，往 Profile 里加一条规则，发 SIGHUP（`nix::sys::signal::kill`），有时限地等到 `GET /v1/rules` 里出现那条规则。`crates/rurge/Cargo.toml` 加 `[target.'cfg(unix)'.dev-dependencies] nix = { workspace = true, features = ["signal"] }`。
- **FR-OBS-01 / 10**（`pipeline.rs`）：`tests/common` 里放一个抓日志的辅助，照 `crates/rurge-proto-quic/src/test_log.rs` 的做法——`tracing_subscriber::fmt()` 写进内存、`tracing::subscriber::set_default` 装在用例的线程上，连同它注释里说的第二个 `Dispatch`（只有一个 dispatcher 时，别的用例先到过的调用点在这个线程上会一直关着）；`rurge-engine` 的 dev-dependency 加 `tracing-subscriber`。用例跑在单线程的运行时上（`#[tokio::test]` 的缺省），会话的任务都在装了捕获的那个线程上。捕获的级别开到 DEBUG（`session completed` 是 DEBUG 级）。一条经本地 `RULE-SET` 命中、走 DIRECT 的会话结束后：抓到的文本里有 `log_session` 记的 `session completed` 那一行，它的 `session=` 等于请求记录的 `id`，并带 `rule=` 与 `policy=`、取值与记录的 `rule`、`policy` 相同；任务 1 的 `Sub-rule matched:` 那一行也在，`session=` 相同。一条被 REJECT 的会话有 `session rejected by …` 一行，`session=` 等于它的记录；一条连不上的会话有 `session failed` 一行，带 `error=`。
- **FR-RULE-05**（`memory.rs`，`#[ignore]`）：各建一个 100 万条的域名集与规则集（生成的名字，不读文件），打印条数。同一个文件里另有一条什么也不建的 `#[ignore]` 用例作基线。实现者在本机对 release 构建的这个测试二进制各量一次进程的峰值工作集——进程退出之后 .NET 读不到内存计数，所以在它运行期间轮询（PowerShell：`$p = Start-Process <二进制> -ArgumentList '--ignored','--exact','<用例名>' -PassThru -NoNewWindow`，循环里 `$p.Refresh()` 并记下最大的 `$p.PeakWorkingSet64`，直到 `$p.HasExited`）；建好的集合在用例末尾还要被查几次（真的查找，不是 sleep），让最后一次轮询落在它们还活着的时候。三个数字（两种集合各一个、基线一个）连同机器的说明与量法交回，任务 7 回填到 M2 设计的 Q3。

**判据**：`cargo test -p rurge --test cli`、`-p rurge-engine --test pipeline` 全绿；`cargo test -p rurge-rules --release --test memory -- --ignored --nocapture` 在本机跑通并给出条数。SIGHUP 那一条在本机不编译（Windows），由评审者逐行读过，第一次 CI 运行时核对——登记在「延后事项」。

---

## Task 7：文档

- **兼容性清单**：第 8 节列出的差异里属于本计划的各一行或并入现有行（`pre-matching`、子规则日志、两种集、平台变量、两个简写、请求记录、`/v1/geoip`、监听覆盖）；先前标着"目前只解析 / 目前没有…"的几行去掉标记、改写备注；统计附录照脚本重数。
- **需求文档**：阶段 1 的状态行里去掉本计划补上的条款，留下 M5b 的两处与仍然没有证据的几项。
- **验收文档** `docs/acceptance/phase2-acceptance.md` 的「需求逐条对照代码」：给每一处已补上的缺口注明补在哪个提交、凭哪条用例。
- **API 参考** `docs/api/phase1.md`：请求记录的四个字段（含 `remoteAddr` 的限制）、`GET /v1/geoip`。
- **README（中英）**：有提到这些缺口的地方同步。
- **CLAUDE.md**：状态段加 M5a 一段；索引加 M5 设计与本计划；常用命令加新的测试过滤词。
- **M2 设计**：Q3 回填任务 6 量到的数字。
- **本计划**：两张表。

**判据**：每一处文档里的名字（类型、函数、诊断码、测试名、命令）在代码里找得到；表格的列数与表头一致；没有本机路径与流程用语。

---

### Task 8（执行中新加）：集合里带 no-resolve 的 IP 行进前缀树

（这一节与下一节里的"第 N 行"，都指本文末尾「执行期修正记录」的行。）

**缘由**：任务 6 量大集合的内存时，实现者读出：集合里的 `IP-CIDR` / `IP-CIDR6` 行只有不带 `no-resolve` 的才进前缀树，带的留在逐条求值的行里。常用的列表每条 IP 行都带 `no-resolve`：blackmatrix7 的 `ChinaMax.list`（2026-10-11 读它自己的 README：124,509 行里 `IP-CIDR` 8,244 条、`IP-CIDR6` 4,281 条；它的 `_Resolve` 版本与它只差这些行不带 `no-resolve`）有 12,525 条 IP 行，到了集合而名字索引里没有的会话，每一条都要逐条比一遍。FR-RULE-05 的"IP 前缀树"因此只对一半的写法成立，而 2026-10-10 的需求审计把这一条判成了"做了"。控制者（调度这次执行的代理）裁定并进本计划，理由和代价见「执行期修正记录」第 51 行。

**做法**（`crates/rurge-rules/src/set.rs`，约十行产品代码）：`CompiledSet::compile` 把每条 `IP-CIDR` / `IP-CIDR6` 行放进集合的前缀树，不论带不带 `no-resolve`；私有标志 `ips_resolve` 记树里有没有哪条行没写 `no-resolve`；`CompiledSet::eval` 向树要地址时传 `no_resolve || !self.ips_resolve`，树仍在逐条求值的行之后查。一棵树加一个标志，不是带与不带各一棵树：`no-resolve` 写在行上只在地址未知时才有区别，而那时没有哪条 IP 行能命中，唯一的问题是集合要不要先问地址——那是集合的事实，不是行的。标志不是 `needs_dns`（那个更宽：嵌套集合、逻辑规则、`GEOIP`、`IP-ASN` 也置它，不论最后问不问）。每次求值的结论（命中 / 不命中 / 先要地址）与改动之前相同。

**用例**（三个提交：`00d97d3` 先钉住、`39817cf` 再加基准、`e4f0991` 最后改）：在没改的代码上绿的钉子——`set::tests` 的 `a_set_of_no_resolve_ip_lines_never_asks_for_a_lookup`、`the_same_network_with_and_without_no_resolve_still_asks`、`a_line_of_the_other_family_that_may_ask_makes_the_set_ask`、`a_nested_set_beside_no_resolve_lines_does_not_make_the_set_ask`，经规则引擎的 `a_rule_set_of_no_resolve_lines_never_asks_for_a_lookup`（`crates/rurge-rules/src/engine.rs`）；改动带来的、先红后绿的三条，在 `set::tests`（`crates/rurge-rules/src/set.rs`）——`no_resolve_ip_lines_go_into_the_index_not_the_linear_list`、`among_no_resolve_ip_lines_the_longest_prefix_is_reported`、`a_walked_line_is_reported_ahead_of_an_ip_line_however_it_is_written`。评审要的一条在任务 9 的第一个提交里补上：`a_walked_line_that_may_ask_makes_the_set_ask_beside_no_resolve_ip_lines`。实现者另以独立写的模型在新旧代码上各随机比了 10 万例，试了六种改错的写法，都没有提交；评审逐格推过等价性（「执行期修正记录」第 52 行）。

**度量**（criterion 的点估计，不是门禁，也不是 p99；`cargo bench -p rurge-rules --bench rules -- no_resolve`；在一台 Windows 11 x86-64 的开发机上量了一次）：十万行 `IP-CIDR,<各不相同的 /24>,no-resolve` 的集合求值一次，改动前（`39817cf`）落空 897.71 µs、命中文件里最后一个网段 905.20 µs，改动后（`e4f0991`）20.098 ns 与 1.1633 µs。NFR-01 写的是"10 万条规则集下单次规则匹配 p99 < 50 µs"：改动之前这样的集合约是它的十八倍；这一项仍没有照它的写法（p99）验证过。**不同会话的数字不能互相比**：同一段产品代码，任务 9 的会话里命中量到 127.00 ns，收尾时第三次会话里 131.83 ns（落空 19.371 ns，紧挨最后一个网段的落空 46.766 ns）——三次里两次把命中量在约 130 ns，第一次的 1.1633 µs 偏高，原因没有查明（第 54、61 行）。改动后的落空是前缀树的最好情形（地址在树的顶部就离开），所以任务 9 补了一个紧挨最后一个网段的落空用例 `set_100k_no_resolve_cidr_near_miss`。

**使用者或读日志的人看得到的三处**（结论与策略都不变；代项目所有者裁定接受，「执行期修正记录」第 53 行）：一、同一个集合有多行命中同一条会话时，报成子规则的是哪一行——带 `no-resolve` 的 IP 行原先按文件顺序夹在逐条求值的行之间，现在与不带的一样排在那些行之后，IP 行之间先看 IPv4 再看 IPv6、同一地址族里取最长前缀；二、写在带 `no-resolve` 的 IP 行之后的、运行期不求值的规则类型（`SUBNET`、`DEVICE-NAME`、`MAC-ADDRESS`、`SCRIPT`）现在也给出那句每条规则一次的提示；三、没有载入 GeoIP / ASN 数据库时那句每个进程一次的告警，可能由这样一条会话提前触发。

**判据**：门禁 71 个结果行 / 2768 通过 / 0 失败 / 6 忽略（任务 6 之后是 2760，多了这 8 条用例）。

---

### Task 9（执行中新加）：规则集文件的行尾注释；集合里的 IP-ASN 行按号码查

**缘由**：核对任务 8 的事实时，把 FR-RULE-05 所依据的手册原句整段读了一遍（`rules/ruleset.html`，2026-10-11）："IP-ASN entries are checked in constant time."；同一段里还有"All other rule types are evaluated linearly."与"Inline rule sets use the same line syntax as external files"。又读了两份常用的、为 Surge 写的 ASN 列表：`missuo/ASN-China` 的 `ASN.China.list`（5,234 行，规则行 5,230）与 `VirgilClyne/GetSomeFries` 的 `ruleset/ASN.China.list`（5,270 行，规则行 5,076）。两份的每条规则行都是 `IP-ASN,<号码>`、空白、`//`、说明（如 `IP-ASN,4134 // 中国电信骨干网`），没有一条带 `no-resolve`。rurge 的规则集文件只认整行注释，行尾的说明成了取值的一部分，`4134 // 中国电信骨干网` 不是号码——**两份列表整份被跳过**，集合是空的，使用者只在加载时看到一条数着被跳过的行数的 `W0023`；另外集合里的 `IP-ASN` 行留在逐条求值的行里，不是手册与需求说的常数时间。这两处也是审计判成"做了"的 FR-RULE-05 的一部分。控制者裁定并进本计划（「执行期修正记录」第 56 行）。

**甲，行尾注释**（`crates/rurge-rules/src/set_format.rs`）：`RULE-SET` 文件的每一行先经 `rurge_config::text::strip_inline_comment` 再读——空白之后、双引号之外的 `#`、`;` 或 `//` 起是注释，与 Profile 里的行一样；`DOMAIN-SET` 文件不变（只认整行注释）。内联的 `[Ruleset …]` 小节本来就经 Profile 的读取器这样读，两种集合从此一致。依据：手册对 Profile 写明了这条规则（`profile/format.html`："Inline comments are also supported."、"there must be at least one space before the delimiter"），又说内联规则集与外部文件是同一种行语法；"Surge 读得了集合文件行尾的注释"**是从那两份列表推断的**（为 Surge 写的，每一行都这样写，第一个仓库的 issue 里没有人说读不了），手册没有写（「执行期修正记录」第 57 行）。改动之前另有两种不出声的写法：`DOMAIN-SUFFIX,example.com // note` 被收下，后缀是 `example.com // note`，什么也匹配不到；`IP-CIDR,10.0.0.0/8,no-resolve # lan` 被收下，而 `no-resolve # lan` 成了没人认识的参数，这一行丢了 `no-resolve`（第 58 行）。

**乙，`IP-ASN` 行按号码查**（`crates/rurge-rules/src/set.rs`）：`CompiledSet::compile` 把集合里（顶层）的 `IP-ASN` 行收进一张"号码 → 第一条点名它的行"的表 `asns`，另有标志 `asns_resolve` 记有没有哪条 `IP-ASN` 行没写 `no-resolve`；`eval` 的顺序成了：名字索引 → 逐条求值的行（文件顺序）→ `IP-ASN` 行（对地址的号码查一次表）→ IP 行。标志既不是 `needs_dns`，也不是 IP 行的 `ips_resolve`：两类行各凭自己要地址。写在 `AND` / `OR` / `NOT` 里的 `IP-ASN` 随那条逻辑规则逐条求值。每次求值的结论与改动之前相同，一个地址的 AS 号被问的次数只会少不会多（第 59 行）。

**用例**（五个提交：`5b03f3d` 任务 8 评审留下的四件小事、`92aa656` 甲、`96b4cf7` 钉住乙之前的匹配结果、`3d964a9` 基准、`ee5b458` 乙；共 21 条用例）：甲在 `set_format.rs`——`a_rule_line_may_end_in_a_note_that_is_not_part_of_the_rule`（上面的三种真实写法）、`a_note_may_follow_a_tab_and_start_with_any_of_the_three_markers`、`a_marker_without_whitespace_before_it_or_inside_quotes_starts_no_note`、`a_whole_line_comment_is_still_one_with_any_of_the_three_markers`、`a_file_of_the_usual_shape_yields_its_rule_lines_and_nothing_is_skipped`、`a_domain_set_line_with_a_note_is_still_a_skipped_line`，与 `set.rs` 的 `a_name_line_with_a_note_after_it_matches_its_name`；乙在改动之前就绿的钉子（`set.rs`）——`a_set_of_no_resolve_asn_lines_never_asks_for_a_lookup`、`the_same_number_with_and_without_no_resolve_still_asks`、`a_name_is_judged_by_its_first_ipv4_address_else_its_first_ipv6_one`、`asn_lines_that_say_no_resolve_are_not_made_to_ask_by_a_nested_set`、`asn_lines_and_ip_lines_each_make_the_set_ask_on_their_own_account`、`without_a_number_from_the_database_no_asn_line_matches`、`an_asn_line_is_reported_ahead_of_an_ip_line_that_also_matches`、`the_database_is_asked_once_for_an_address_and_not_at_all_without_one`，经规则引擎的 `a_rule_set_of_asn_lines_resolves_the_name_once_and_names_the_line` 与 `a_rule_set_of_asn_lines_that_all_say_no_resolve_never_asks_for_a_lookup`；乙带来的三条——`asn_lines_are_not_in_the_list_of_lines_walked_one_by_one`、`a_number_named_twice_reports_the_first_of_the_two_lines`、`a_walked_line_that_matches_is_reported_ahead_of_an_asn_line_that_also_matches`。其中 `a_number_named_twice_reports_the_first_of_the_two_lines` 在改动之前也绿（第 62 行）。

**度量**（criterion 的点估计，不是门禁，也不是 p99；`cargo bench -p rurge-rules --bench rules -- asn`；每一对前后数字都是同一次会话里背靠背量的，在一台 Windows 11 x86-64 的开发机上）：五千行照列表写法（`IP-ASN,<号码> // 说明`）的 `RULE-SET` 求值一次——`set_5k_asn_miss`（号码没有哪一行点名）改动前（`3d964a9`）30.239 µs、改动后（`ee5b458`）22.123 ns，`set_5k_asn_hit`（最后一行点名的号码）43.296 µs 与 94.030 ns，`set_5k_asn_asks`（没解析的名字，集合答"先要地址"）24.260 µs 与 11.949 ns。改动前的三个数字不同，criterion 自己给的区间互不重叠（`set_5k_asn_miss` 26.773 ～ 33.176 µs、`set_5k_asn_hit` 42.477 ～ 44.054 µs、`set_5k_asn_asks` 23.907 ～ 24.678 µs），原因没有查；改动前，到了这样一个集合而名字索引不认识的会话，要为这一个集合花 5,000 次比较，约 24 ～ 43 µs，已近 NFR-01 的 50 µs。在甲之前这个问题不存在：集合是空的。第三次会话（收尾时，没有别的东西运行）：`IP-ASN` 的三个用例 20.990 ns / 94.927 ns / 11.932 ns（第 61 行）。

**使用者或读日志的人看得到的**：乙的三处与任务 8 相同，根子是一件事——逐条求值的行现在全部先走完（「执行期修正记录」第 60 行）；甲的三处——取值里本来就有"空白加 `#`、`;` 或 `//`"的行现在在那里截断（用**双**引号括起来保持完整），每条 IP 行都写成 `…,no-resolve # 说明` 的集合从"会先要地址"变成"从不要地址"，取值用**单**引号括起来也同样被截断（第 58、64 行）。反过来，甲顺带修好一种写法：嵌套引用后面跟注释（`RULE-SET,other.list // 说明`）原先把 `other.list // 说明` 当成文件名，悄悄成了空集合。

**两轮修正**（评审通过，五条小项里做了三条，并修了两个早于本计划的缺陷；「执行期修正记录」第 63 ～ 66 行）：第一轮（`ffb6477`、`3034ccb`）——评审要的两条用例 `single_quotes_do_not_keep_a_marker_from_starting_a_note`（`set_format.rs`）与 `a_file_written_the_way_the_lists_write_it_loads_whole`（`registry.rs`），和集合文件开头的 UTF-8 BOM 不再算进第一行（`parse_set_with_limit` 去掉一个 U+FEFF；先红后绿的 `a_byte_order_mark_at_the_head_of_a_rule_set_is_no_part_of_its_first_rule`、`a_comment_after_a_byte_order_mark_is_still_a_comment_in_a_rule_set`、`a_comment_after_a_byte_order_mark_is_still_a_comment_in_a_domain_set`、`a_byte_order_mark_at_the_head_of_a_domain_set_is_no_part_of_its_first_name`、`a_local_file_saved_with_a_byte_order_mark_loads_whole`，钉住范围的 `only_one_byte_order_mark_at_the_very_start_of_the_text_is_dropped`）；第二轮（`09f1a7a`）——域名集里不是主机名的行不再被告警原样引用，原因改为 `invalid DOMAIN-SET line`、告警照旧给行号（`a_domain_set_line_that_is_not_a_name_is_skipped_without_being_quoted` 与 `a_domain_set_given_the_wrong_file_warns_without_quoting_its_lines` 改之前是红的，`a_rule_set_line_without_a_rule_type_is_skipped_without_being_quoted` 是钉子）。

**判据**：门禁 71 个结果行 / 2800 通过 / 0 失败 / 6 忽略（任务 8 之后是 2768：五个提交 +21，第一轮 +8，第二轮 +3）。

---

### Task 10（执行中新加）：两条偶发失败的进程用例

（这一节里写"记录第 N 行"的，指本文末尾「执行期修正记录」的行；写"延后事项第 N 行"的，指「延后事项」的行。两个提交 `bbf220d`、`6ceeba8` 都在 `09f1a7a` 之后，只动 `crates/rurge/tests/cli.rs`。）

**为什么加**：写文档时把 CLAUDE.md 里列的每个测试过滤词各跑了一遍；其中一次，本里程碑的一个用例红了：`a_start_with_the_recorded_profile_leaves_state_json_as_it_is` 里 `POST /v1/stop` 的状态读成了 0。一个 N 次里失败一次的用例，会让门禁 N 次里失败一次，所以并进本计划查清（记录第 67 行）。同一轮反复跑里另有一个用例也失败了一次，见最后一段。

**查明的原因**：辅助函数 `api_call` 用 `write!` 把请求直接写到套接字上，格式串的每一段是一次 `send`，请求头和正文（`{}`）可能分在几个 TCP 段里，正文晚于请求头到达。`POST /v1/stop`、`/v1/dns/flush`、`/v1/profiles/reload`、`/v1/profiles/check` 的处理函数不读正文，服务端凭请求头就答复：axum-core 0.5.6 在跑一个只取 `State` 的处理函数之前就把请求的正文丢掉；hyper 1.11.1 发现正文的接收端在正文到齐之前被丢掉，就调 `poll_drain_or_close_read`（`src/proto/h1/dispatch.rs`、`src/proto/h1/conn.rs`）：试读一次正文，没读到就 `close_read`，它同时结束 keep-alive，所以这与请求带不带 `Connection: close` 无关（两个版本取自 `Cargo.lock`，源码是读的）。连接在应答之后关掉，晚到的正文落在关了的连接上，对端回一个 RST；在 Windows 上，RST 让客户端的系统把已经收到而还没读的应答丢掉（RFC 9112 第 9.6 节 Tear-down 描述的正是这件事），`read_to_string` 什么也没读到就以 `ConnectionAborted`（os error 10053）失败，`api_call` 又把这个错误丢掉，状态成了 0。

**证据**，按取得的次序。一、在负载下复现：给失败的那一刻加了临时的记录之后，这一组用例（过滤词 `its_profile recorded cannot_write_state_json` 选中的五个）单独跑 150 次没有失败；旁边另有一到两组用例反复跑着时跑了 665 次，失败一次，是第一个有负载的系列的第 115 次：状态 0，应答一个字节也没读到，`ConnectionAborted`（os error 10053），出现在写完请求之后 13 µs；那次写请求用了 8.19 ms，平时约 1 ms（550 次有负载而通过的运行：中位数 1.01 ms）。二、临时探针（对着真实的 API 服务端，回环；先发请求头，300 ms 后发正文，其间客户端不读；每一行重复十次，十次完全一样，密钥不对的那一行两次）：

| 请求 | 正文晚到 | 请求头与正文一次写出 |
| --- | --- | --- |
| `POST /v1/dns/flush`、`/v1/stop`、`/v1/profiles/reload`、`/v1/profiles/check`（处理函数不读正文） | 正文的写成功；读失败，`ConnectionAborted`（10053），0 字节 | `200`，然后流结束 |
| `POST /v1/log/level`（处理函数读正文） | `200`，然后流结束 | `200`，然后流结束 |
| `POST /v1/dns/flush`，密钥不对（在读正文之前就被拒绝） | 读同样失败，0 字节 | `401`，然后流结束 |

在发正文之前先读的客户端读到 `200`（服务端凭请求头就答复了），在本该读到流结束的地方遇到中止。三、不强迫晚到：一个像 `api_call` 那样写（`write!`，分几段）的客户端，在另外几组用例反复跑着的负载下，对进程内的真实服务端各发 1500 次：`POST /v1/dns/flush` 丢了 12 次应答（每一次都是状态 0、10053、什么也没读到，写出请求用了 5.9 ～ 19.8 ms）；`POST /v1/log/level` 一次也没丢；`POST /v1/dns/flush` 一次写出，一次也没丢。四、不是守护进程死了：一个在连接开着时被杀掉的守护进程，客户端读到的是 `ConnectionReset`（os error 10054），五次都是，不是 10053。第一次失败那一次，守护进程自己的退出码没有记下（记退出码的改动是事后加的，之后 550 次有负载的运行没有再失败，每次 `POST /v1/stop` 都是 200、守护进程退出码 0）：对那一次，这个解释有证据支持，没有证明。探针和上面这些记录都是临时的，提交之前删掉了。

**改了什么**（`bbf220d`）：`api_call` 与 `http_get_with` 把整个请求拼成一个 `String`，用一次 `write_all` 写出；`http_get_with` 没有正文，丢不了应答，但一个请求一次写出是同一条规则。测试代码里手写的 HTTP 客户端只有 `api_call` 把正文分几次写，其余的本来就是一次 `write_all` 写完整个请求。改动之后，同一组用例在三组用例同时反复跑着的负载下跑了 400 次，没有失败。这个数的分量要说清：改动之前负载下 665 次里才失败一次，单看 400 次干净的运行，没有这次改动也有约 55% 的机会看到；一次写出去掉了原因，靠的是探针（正文晚到的请求每次都丢应答，一次写出的每次都得到应答）和 1500 次的对照（分几段写丢 12 次，一次写出 0 次）。

**没有改的**：服务端这一侧是 API 自己的性质，早于本计划：带着正确密钥、处理函数不读正文的请求，或在读正文之前就被拒绝的请求，在请求头与正文分开到达时，可能让客户端读不到应答（只在 Windows 上量过）。谁分开发：CPython 的 `http.client`——读 v3.13.0 的 `Lib/http/client.py`，`_send_output` 用一次 `send` 发请求头、对正文的每一块各用一次——，所以 `urllib.request` 也是；`requests` 经 urllib3 也是（读 urllib3 2.7.0 的 `HTTPConnection.request`：`endheaders()` 之后逐块 `send`）。谁不：rurge 自己的客户端（`crates/rurge/src/cli/api_client.rs`，hyper-util 的客户端加 `Full` 正文）——hyper 1.11.1 把请求头和已经备好的正文放进同一个写缓冲，`Buffered::poll_flush`（`src/proto/h1/io.rs`）一次向量化写出——这是读源码得出的，没有量过。有名的服务端怎么做：Go 的 `net/http`（读 go1.23.0 的 `src/net/http/server.go`）在应答之前替处理函数读掉最多 `maxPostHandlerReadBytes`（256 KiB）没读的正文，读不完的，应答带 `Connection: close`，关连接之前先 `closeWriteAndWait`：只关写的一半，等 `rstAvoidanceDelay`（500 ms）再关整条连接。登记给阶段 1 补遗（M5b），它本来就要动 API（延后事项第 35 行；写给使用者的版本在 `docs/api/phase1.md` 的「鉴权与错误」）。

**第二条**：同一轮反复跑里，`run::a_listen_value_that_cannot_be_used_ends_the_start_without_quoting_it` 失败了一次：辅助函数 `give_up` 在 20 秒期限后 panic，"rurge run did not give up within 20 s"，那时有三组用例同时反复跑着。信息既没有说是这个用例里九次启动中的哪一次，也没有说子进程打印过什么；读代码，那次启动的路径上没有东西在等（`Overrides::parse`、`error:` 一行、`recover_without_a_log`、退出码 2——`crates/rurge/src/cli/run.rs`）。`6ceeba8` 让 `give_up` 在期限到时说出这次启动（设置的环境变量、程序与参数）、把子进程已经写到标准错误的内容读完附上、`try_wait` 返回过错误时附上最后一个错误。改动之前这一组在同样的负载下跑了 398 次、失败一次（第 183 次），旁边一组跑了 405 次没有失败，更早单独一组的 347 次也没有；改动之后同样的负载下 810 次（两组各 405 次）没有再出现，所以没有信息可引，原因不知道，也没有查下去（延后事项第 37 行）。

**判据**：门禁 71 个结果行 / 2800 通过 / 0 失败 / 6 忽略（`6ceeba8` 上，与 `09f1a7a` 的数字相同；第一遍里一个测试二进制死于本机已知的偶发崩溃，没有用例失败，单独重跑通过）。

---

## 计划期决定

| # | 事项 | 核对到的事实 | 决定 |
| --- | --- | --- | --- |
| P1 | 预匹配放在哪一层 | 规则只在 `Engine::choose_policy` 一处被评估，出站模式的旁路在它调用规则引擎之前；`pre_match_*` 在 crate 之外没有调用方 | 放进 `RuleEngine::run`，三种调用方（TCP、UDP 流、内部 DNS 会话）与 `rurge rule match` 一起得到它 |
| P1a | 子规则命中属于哪条规则 | `EvalCtx.sub_hit` 由 `Matcher::Set` 在集合命中时写下，`run` 只在规则命中时取走，中间不清：一条没命中的逻辑规则里命中过的集合，会被后面命中的规则带走。今天只有离线的 `rurge rule match` 显示它 | 两趟都在每条规则之前清掉；任务 1 的第一条用例先把它钉红 |
| P2 | 同一路径的两种写法 | `ResourceRef::File` 是与 Profile 目录拼接后的 `PathBuf`；`PathBuf` 的相等按路径分量比较，中间的 `.` 不算 | 加载期直接比较 `ResourceRef`，不另做规范化；注册表的键与它一致 |
| P3 | `E0024` 是错误 | 现有的同类规定（`pre-matching` 配了非 REJECT 的策略）是规则行的错误 | 照此；嵌套的引用在运行期才看得到，只能是告警 |
| P4 | 监听取值不经 clap 解析 | clap 对不合法的取值会在错误里原样引用它；`http-listen` 的取值是 `[口令@]地址[:端口]` | 取 `String`，`run()` 自己解析并自己报错 |
| P5 | 远端地址在哪里登记 | `DirectConnector::connect` 用 `race`，每次尝试在 `JoinSet` 里另起任务；`race` 返回的是连上的那条流 | 在 `race` 返回之后登记 |
| P6 | DNS 耗时在哪里登记 | `Resolver` 对外的三个 trait 入口（`LazyResolver::resolve`、`Resolve::resolve`、`Resolve::resolve_via`）都经 `lookup` / `lookup_via`，这两个又都只是调用 `lookup_inner` | 在 `lookup_inner` 一处计时；它在调用方自己的任务里被轮询，内部另起的查询任务不影响 |
| P6c | 查找时连上游的连接 | `Resolver::query_coalesced` 就地 `await` `fanout::resolve_name`，而 `resolve_name` 把每个上游的每个查询放进 `JoinSet` 另起任务（`set.spawn(one_query(..))`）；DoQ / DoH3 的拨号也在会话的后台任务里：`tcp://` / `tls://` / `https://` 上游的连接都不在调用方的任务里 | 不另加"查找期间没有 `Trail`"的作用域（今天用不上）；用一条用例把"查找开的连接不记到会话上"钉住 |
| P6a | 换成员时的远端地址 | `Dialer::dial` 的循环里，一个成员可以连上 TCP 而在协议握手里失败；下一个成员可能走复用的连接、不再登记 | 每次尝试之前 `clear_peer()` |
| P6b | 谁建会话句柄 | `Engine::new_handle` 有五个调用处：`Dialer::dial`、`udp::run_flow`、`dial_internal`、`dial_internal_udp` 与测速（`auto.rs`） | 前四处放进 `follow`，测速不放 |
| P7 | 请求记录怎么取这几项 | `record_of` 在快照时从句柄读，不在会话结束时另存 | 句柄持有 `Trail` 与备注，记录按需读 |
| P7a | 备注与 `error` | 句柄的 `set_error` 今天兼作"为什么这样走"的说明（`fail` 把它与失败原因拼在一起），这些文本登记在 `docs/api/phase2.md` | `notes` 是另一项，只放新的备注；现有的说明不搬 |
| P8 | JSON 里没有值的写法 | `RequestJson` 现有的可选项序列化为 `null` | 新的三项照此；`notes` 是数组 |
| P9 | 子规则日志的级别 | "会话完成"一行是 DEBUG；Surge 的 `loglevel = info` 对应 DEBUG | DEBUG |
| P10 | 建议在哪里填 | `Diagnostic` 派生了序列化，`hint` 是它的一个字段 | 构造时填缺省值，输出的代码不动；快照随之变化 |
| P11 | `SYSTEM_VERSION` 的样式 | 手册的示例是 `System Version 17.4.1 (Build 21E236)`；比较按字符串 | 三个平台都给这个形状 |
| P12 | Windows 的读取 | `windows-registry` 0.6.1 已是依赖，`sysproxy::windows` 用着 `get_u32` / `get_string` | 只读注册表，不开进程 |
| P12a | macOS 与 Linux 的读取在本机怎么测 | `rurge-platform` 的惯例：`dirs` 的规则吃环境变量查找与显式的 `Os`，`sysproxy` 的三个后端都编译、命令经 `CommandRunner`，`command::testing::FakeRunner` 记下并应答命令 | 两个读取吃 `Sources`（命令、文件、环境变量），三平台都编译，用假来源测；只有注册表那一个带 `cfg(windows)` |
| P13 | GeoIP 的测试库 | `crates/rurge-rules/tests/fixtures/` 有国家与 ASN 两个测试库 | API 用例把它们放进数据目录 |
| P13a | API 怎么拿到 GeoIP | `rurge-api` 只依赖 `rurge-engine` 与 `rurge-config`；引擎的 `Stack.geo` 是 `Arc<GeoDb>` | 引擎加一个视图方法 `geoip_view`，API 不直接依赖 `rurge-rules` |
| P14 | SIGHUP 的用例 | 本机是 Windows，`cfg(unix)` 的代码不编译 | 照写，评审者逐行读，CI 首跑核对 |

## 执行期修正记录

| # | 任务 | 与计划的出入 | 处理 |
| --- | --- | --- | --- |
| 1 | 1 | 计划只在两条规则之间清掉 `sub_hit`。同一条逻辑规则里没有参与命中的分支（`OR,((AND,((RULE-SET,x),(DEST-PORT,1))),(DOMAIN,a))`）仍会留下 x 的命中，新加的日志就会说假话 | `Matcher::eval` 在逻辑匹配器没有命中时也恢复 `sub_hit`（`Not` 一律恢复）；多两条用例 |
| 2 | 1 | 预匹配那一趟遇到的运行期提示，计划没说怎么办 | 经 `collect_notes`，每条规则只记一次，哪一趟先遇到算哪一趟；`pre_match` 照旧丢弃 |
| 3 | 1 | 计划是一个提交 | 三个提交（预匹配；子规则日志、备注与陈旧的命中；`E0024` / `W0032`），末尾一次门禁——三件不相干的改动分开好审、好回退 |
| 4 | 1 | 评审发现 `W0032` 取决于缓存状态：同一份 Profile 在冷缓存与热缓存上分流不同 | Profile 自己的文本给一个资源的角色（规则——含逻辑规则的子规则——、`[Host]` 的集合键、内联集合的各行，也就是 `E0024` 看的那些用法）总是压过集合文件里另一种角色的引用，不论被占的条目建没建（`claims_of`，`crates/rurge-rules/src/registry.rs`）；两个都是嵌套引用时仍是先登记的留下 |
| 5 | 1 | `E0024` 的文本说"first used"，而两处用法可以在不同文件里，没有先后 | 文本改为 `the same resource cannot be both a RULE-SET and a DOMAIN-SET (it is used as a <kind> at <file:line>)` |
| 6 | 1 | `W0032` 不说被忽略的引用写在哪个集合里 | 文本带上写着那条引用的集合：``set `<name>` (referenced in `<parent>`) ignored: the same resource is used as a <RULE-SET\|DOMAIN-SET>``；没有父集合时不带括号（这一句只有 `E0024` 拒绝的 Profile 才出得来，而每个命令都在建注册表之前拒绝这样的 Profile，所以没有用例） |
| 7 | 1 | 修正之后的复审：`claim_kind` 的 `And` / `Or` / `Not` 三个分支在注册表一层没有用例（删掉它们全部用例仍绿） | 补一条 `the_role_a_logical_rule_gives_a_file_wins_too`（`AND`、`OR`、`NOT` 与嵌在 `OR` 里的 `AND`） |
| 8 | 1 | 聚焦的 `cargo test -p <crate>` 每换一种包的选择就另编一份依赖（cargo 按所选的包统一依赖特性）：`target/` 涨到 141 GiB，磁盘只剩 5.3 GiB，全量门禁没法跑 | 清掉 debug 构建缓存（`cargo clean --profile dev`），此后一律不用 `-p`：聚焦运行用 `--workspace --lib -- <过滤词>`、`--workspace --bin rurge -- <过滤词>`、`--workspace --test <名字> -- <过滤词>`，与全量门禁共用同一份构建。CLAUDE.md「常用命令」里的 `-p` 写法没有改（邻近各行都是那样写的）；本里程碑新加的过滤词每个都在 `--workspace` 的形式下跑过，确认选中了用例 |
| 9 | 2 | 计划说两个简写"只是往 `SIMPLIFIED` 里加两项"。`text::is_requirement_prefix` 是另一份写死的清单，`parse_str` 与 `redact.rs` 先问它；只加进 `SIMPLIFIED` 的简写写在行尾有效、写在行首被当成未知指令丢掉 | 只留一份清单（别处从 `SIMPLIFIED` 派生），加一条行首写法经 `from_text` 的用例 |
| 10 | 2 | 设置了而为空的环境变量（`RURGE_LISTEN_HTTP=`）计划没说 | 是错误（退出 2，`no address given`），与 `--listen-http ''` 相同——决定代理在哪里监听的设置，拒绝启动比悄悄照 Profile 监听稳妥。代项目所有者裁定；另一种读法是当作没设 |
| 11 | 2 | 只给了 `--listen-http`、Profile 没写 `socks5-listen` 时有没有缺省的 SOCKS5 监听，计划没说 | 没有：覆盖读起来就像 Profile 只写了 `http-listen`（"缺省只在两种都没写时才有"是原有的规则） |
| 12 | 2 | 评审发现覆盖进来的监听端口，加载期对 `external` 策略 `local-port` 的检查看不到：`--listen-socks5` 落在某条 `external` 策略的端口上，rurge 会自己连自己 | 覆盖就位之后再问一次（`Config::external_on_own_listener`，`apply` 返回 `Result`）：启动时报 ``error: --listen-socks5: port <P> is the `local-port` of an `external` policy``，恢复上一次崩溃留下的系统代理，以 2 退出；重载失败而保持运行中的配置。反过来的情形没有变：Profile 自己的监听在那个端口上时，即使被覆盖走仍是 `E0018` |
| 13 | 2 | `current_profile` 写不进 `state.json`（数据目录不可写）时怎么办，计划没说 | 启动照常，多一行 ERROR（`cannot write state.json`）；有用例 |
| 14 | 2 | 启动在说出 `rurge … running` 之前多写一次 `state.json`，两条现有的进程用例受了影响：`a_profile_that_does_not_load_leaves_a_live_instances_system_proxy_alone` 在看到 `system proxy enabled:` 之后立即读 `state.json`，现在稳定地红；七个用例共用的辅助函数 `crashed_with_the_system_proxy_on` 在同一时刻杀进程，杀在写 `state.json` 的窗口里会留下半个 `state.json.tmp`，而 `run_says_so_when_it_restored_but_cannot_update_the_state` 要在原处建同名目录（反复运行那一个用例 30 次里 20 次红，读过其中一个失败日志，是 `AlreadyExists`） | 准备步骤改为先等 `rurge … running` 一行再读 `state.json` 或杀进程；断言没动 |
| 15 | 2 | 两个选项的帮助文字 | 写明取值整项替换 Profile 里该种类的每个地址，以及口令写在命令行上会出现在进程列表里（环境变量不会） |
| 16 | 2 | 评审指示里的一个前提是错的：`#!WINDOWS-ONLYX` 这样粘着别的字母的写法从来不是"未知指令" | `is_requirement_prefix` 一向没有词边界，旧的三个简写也一样：这样的行按它所在节的普通行读（`[Rule]` 里是 `E0012`）。没有改代码，登记进兼容性清单 |
| 17 | 3 | `--platform` 指到别的系统时四个变量怎么报，设计 5.4 没说 | `SYSTEM_VERSION` 只在所评估的平台就是本机的平台时报真实值，否则 `unknown`（一个系统的版本顶着另一个系统的名字，版本条件就没有意义）；`DEVICE_MODEL`、`LANGUAGE`、`DEVICE_NAME` 照读到的报 |
| 18 | 3 | Windows 的 `DEVICE_NAME` 取哪个名字 | 取注册表 `Tcpip\Parameters` 的 `Hostname`（DNS 主机名，保留使用者输入的大小写，也是"设置"里显示的那个），不取大写的 NetBIOS 名；`COMPUTERNAME` 仍是回落。登记为差异 |
| 19 | 3 | 区域变量的读法 | 照 POSIX：`LC_ALL`、`LC_MESSAGES`、`LANG` 里第一个设了值的说了算；它是 `C` / `POSIX` 时系统就没有给出语言 |
| 20 | 3 | 计划要求"系统没给出的值回落到与旧值逐字相同"。旧的 `$LANG` 回落另有一套读法：`LANG=POSIX` 报成 `POSIX`，修饰符留着（`sr-RS@latin`） | 回落与读区域变量共用一条规则（`rurge_platform::sysinfo::language_of`）：`POSIX` 与 `@euro` 现在回落到 `en-US`，`sr_RS@latin` 是 `sr-RS`，其余取值不变。有意保留并有断言钉着：系统因 `LC_ALL=C` 没给出语言时，回落仍然单独问 `$LANG`（`LANG=de_DE.UTF-8` 照旧报 `de-DE`） |
| 21 | 3 | 计划只列了五个固件占位型号 | 改为六个前缀与十三个整串（不分 ASCII 大小写），取自 fastfetch 的 `ffIsSmbiosValueSet`（`src/common/impl/smbios.c`，MIT，2026-10-11 读，只取事实）里适用于产品名的那些；`Standard`、`All Series` 与全零的规则没有取。占位值照旧回落到 CPU 架构 |
| 22 | 3 | 评审：三条用例与机器或名字不符（一条要求注册表的 `BootExecute` 存在且非空；一条的名字说"只读一次"而没有检验；`cli.rs` 的版本用例没有 `cfg`） | 三处都改了 |
| 23 | 3 | 开发机编不了 Linux / macOS 目标，而 CI 在三个平台上跑 `clippy -D warnings` | 实现者把 `cfg` 谓词在本机临时改写（Unix 开、Windows 关；两者都关）各跑一遍 clippy 与单元用例，文件按字节恢复。这只证明类型与 `dead_code`；真实系统上的第一次执行是 CI 首跑 |
| 24 | 4 | 计划的建议初稿表 | 实现者逐个发出点读过后的结果：33 个码有缺省建议、`E0014` 四处各自带、25 个不设。与初稿不同的：`W0013`、`W0024`、`W0031` 不设（初稿的句子对其中一处不成立）；`W0020` 加了；`W0017` 的初稿是错的（`#!include` 行里没有节名）；`E0002`、`E0004`、`E0008`、`E0011`、`E0013`、`E0019`、`E0021`、`W0001`、`W0030` 与 `E0014` 第四句改写；`E0015`、`W0025` 去掉了数字。逐码的句子在 `codes::hint`（`crates/rurge-config/src/diagnostic.rs`），不设建议的码的理由写在它上面的注释里 |
| 25 | 4 | 计划说语料库快照里的诊断会多出 `hint` 一行，而 `tests/corpus.rs` 的 `render()` 自己拼文字、从不含建议 | `render()` 照 `Display` 带上建议，三份快照（`format-examples`、`group-cycle`、`legacy-keys`）随之各多出建议的文字，别无变化 |
| 26 | 4 | 计划的"数目断言" | 改为从 `diagnostic.rs` 的源码里读出 `codes` 模块声明的全部码，与三张名单（有缺省 / 每处自带 / 不设）的并集比对；新加一个码而不做决定，`diagnostic::tests::every_code_is_decided_once` 就红 |
| 27 | 4 | 写进日志的诊断会因为建议多出第二行 | 一条日志记录仍是一行：`Diagnostic::headline()`（`crates/rurge-engine/src/subscriptions.rs`）；给人看的打印（`print_diagnostics`：`rurge run` 的启动与重载、`rule match`、`dns lookup`）在诊断后多一行 `hint:` |
| 28 | 4 | `E0010` 原有的那一句 | 从发出点挪进表，显示逐字不变（两条用例钉着） |
| 29 | 4 | 评审：`Diagnostic::with_hint` 收 `impl Into<String>`，"建议不引用 Profile 里的东西"只是约定 | `Diagnostic.hint` 与 `with_hint` 的参数都改为 `&'static str`（比评审要求的多走一步：字段是公开的，只收紧参数挡不住直接赋值）。以后哪条诊断真的需要拼出来的建议，要把类型再放宽 |
| 30 | 4 | 评审：`E0014` 没有缺省建议，新增一处而忘了带，没有用例会红 | 四处改经 `rule.rs` 里一个必须带建议的私有构造（`not_allowed_here`）；绕过它直接写 `ParseError::new` 的第五处仍然编得过（接受） |
| 31 | 4 | 评审：五句缺省建议在某些发出点上不贴切 | 改写：`E0002`（相对路径从**主** Profile 的目录算起，嵌套的 `#!include` 也是）、`E0015`（"用更少的层数重写"——拆成几条只对 `OR` 等价）、`E0020`（也涵盖"没有这一项"）、`W0006`（组的旧参数 `url` 不是键："remove it"）、`W0030`（对自己引用自己的组、对多于两个组的环都成立）。快照 `legacy-keys`、`group-cycle` 只变那一句 |
| 32 | 4 | 评审问：`rule match --json` 与 `dns lookup --json` 的 `warnings` 是手拼的，不带 `hint` | 加上（只增键；字符串或 `null`，与 `check --json` 经 derive 得到的形状相同） |
| 33 | 4 | 评审：划分用例失败时只打印两个 59 项的数组 | 失败信息点名是哪个码、该进哪张表；`declared()` 读源码的方式没有改（声明形状特殊的常量会被漏掉而用例不说，接受） |
| 34 | 5 | 计划：解析器在 `lookup_inner` 返回之前登记耗时 | 改用 drop 守卫登记：拨号被外层超时丢弃时，花在这次解析上的时间同样计入 `dnsMs`（超时的解析正是人去看 `dnsMs` 的情形；多一条用例钉着） |
| 35 | 5 | 计划：四处把建句柄之后的函数体放进 `trail.follow(async move { … })` | 引擎里三处的函数体抽成私有方法（`connect_session`、`connect_internal`、`open_internal_udp`）再放进 `follow`：内联的 async 块里有超长的字符串字面量时 rustfmt 整块不格式化。行为等价，`engine.rs` 的 diff 按 `git diff -w` 是 +60 −9 |
| 36 | 5 | 计划没有写 UDP 的记录怎样不带远端地址 | 实现者先是在两处拨号之后各清一次（`dial_internal_udp` 的 `follow` 之后、`run_flow` 的 `select!` 之后）；评审之后改成一条规则，见第 40 行 |
| 37 | 5 | 计划的注释写 `Trail` 记"耗时合计、解析次数" | 只存 `Option<Duration>`：次数没有人读，`None` 与 `Some(合计)` 已经分得开"没有做过解析"与"做过、很快" |
| 38 | 5 | 计划只说 400 "照本 crate 其它 400 的形状" | `ip` 不是地址时是 `ip is not a valid IP address`（不回显调用方给的取值）；缺 `ip` 时是 axum 查询串拒绝的原文 |
| 39 | 5 | `dnsMs` 在同名查询已在途时算不算等它的时间，计划没有说 | 算：它是这条会话等域名解析的时间（裁定） |
| 40 | 5 | 评审：UDP 的记录"从不带远端地址"在一条路径上不成立——内部 UDP 载体打不开时，会话在拨号里面就结束、记录当场拍下，拨号之后才清地址已经来不及（代理拒绝登录的 DoQ 载体，记录带着代理那条 TCP 连接的地址）；流的载体正在打开时，进行中的记录也看得到那个地址 | 规则挪到拍记录的地方：`record_of`（`crates/rurge-engine/src/observe.rs`）对 UDP 会话一律不给远端地址，拨号之后的两处清除去掉。`a_doq_carrier_the_proxy_will_not_open_fails_naming_the_policy` 多一句断言（改之前是红的），另加一条单元用例 |
| 41 | 5 | 评审：实现者交给文档的"哪些会话有远端地址"的清单有一行不属实——`external` 的会话连本机端口不经连接器，记录是 `null`；QUIC 类经底层策略的例外也写窄了 | `external` 改代码使它属实：`ExternalOutbound::connect_local` 连上之后记下地址，这样的会话是 `127.0.0.1:<local-port>`（字段的含义是"会话自己的拨号建的 TCP 连接去了哪里"，它确实建了一条）。QUIC 类的例外照评审核对过的范围写进文档（延后第 21 行） |
| 42 | 5 | 计划（P8）说新键照 `null` 的惯例，而交给实现者的说明把 `listenPort` 写成了"数字"：rurge 自己发起的会话是 `0` | JSON 里是 `null`；记录里仍是 `u16` 的 `0` |
| 43 | 5 | 评审：远端地址不把 IPv4 映射的地址（`::ffff:a.b.c.d`）归一；`GET /v1/geoip` 对这种写法查出什么，从代码判断不了 | 两处都按 IPv4：`rurge_net::trail::connected` 记下归一后的地址（带作用域的 IPv6 地址原样，作用域不丢）；`/v1/geoip` 先归一再查，回显的 `address` 也是 IPv4（改之前两个测试数据库对映射写法查出的结果相同，不同的只有回显的 `address`） |
| 44 | 5 | 评审的三个小项 | `Trail` 的限制靠 `fanout` 里每个查询各在自己的任务里运行，那一行补了说明；`UNTESTED`（让成员不参加测试轮次的参数）在两个测试文件里各写了一遍，收进 `crates/rurge-engine/tests/common/mod.rs`；补一条用例：规则触发的解析同样计入 UDP 流的 `dnsMs` |
| 45 | 6 | 计划说内存实测是"两条 `#[ignore]` 用例" | 三条：域名集、规则集、什么也不建的基线（要交回的是三个数字）。普通的 `cargo test --workspace` 编译它们而不运行，门禁里的 ignored 从 3 变成 6 |
| 46 | 6 | 计划的 `SIGHUP` 用例没有写"先等守护进程准备好" | 先等 `rurge <版本> running` 一行再发信号：处理器是在这一行之前才装上的，早到的 `SIGHUP` 会按缺省动作结束进程。评审指出同一文件里早先的 `SIGINT` 用例有同样的竞态（两条 `listening on` 一读到就发信号），照同样的办法改了（一行；`#[cfg(unix)]`，本机编不了，靠读） |
| 47 | 6 | 计划没有说新的日志捕获与已有的手写订阅器的关系 | `crates/rurge-engine/tests/common/log_capture.rs` 的 `LogCapture` 取代 `pipeline.rs` 里任务 1 手写的订阅器，那条用例迁过来（捕获保留级别，它的 `starts_with` 因此更严）。`rurge-rules` 单元用例里手写的那一份属于另一个 crate，留着（延后第 8 行） |
| 48 | 6 | 计划之外多一个提交 | `cli.rs` 两个辅助函数的修正，任务 2 修正之后的复审提的、当时裁定并进本任务：守护进程在打印 `listening on` 之前就退出时，测试的 panic 带上它已经写到标准错误的内容（标准错误被测试接管时）；两份相同的读行线程合成 `forward_lines`。附一条 `#[should_panic(expected = "error: cannot read")]` 的永久用例 |
| 49 | 6 | 评审的三条小项 | 日志行的字段按整段比较（原来的 `has_field` 看不到取值的右边界：`policy=A > B` 能满足 `A`），辅助函数自己有一条用例；"会话完成"那条用例里被测会话之前先走一条别的会话，使它的 id 不是引擎的第一个；"Profile 不被改写"除了字节也比两个文件的修改时间（写回同样的字节也是写了使用者的文件：可能写到一半失败，`--watch` 也会把它当成一次修改）。两条用例重复的准备步骤没有收（不值得为它再动一次） |
| 50 | 6 | FR-RULE-05"大集合内存可控"没有数字，计划要求"量出来、对着需求说是否满足" | 量了，判不了：需求没有判据。数字照实写进验收文档与 M2 设计的 Q3（100 万条的集合，Windows 上发布构建的峰值工作集：载入时比基线多约 403 MB（`DOMAIN-SET`）/ 456 MB（`RULE-SET`），建好之后多约 130 / 150 MB，编译后的集合本身约 101 / 111 MiB——设计估的"约 60 MB"说的只是排序数组）。是否算"可控"、NFR-01 的 300 MB 是否要在载了满额集合的前提下成立，留给项目所有者（延后第 24 行） |
| 51 | 8 | 计划没有这个任务。任务 6 量大集合的内存时，实现者读出集合里带 `no-resolve` 的 `IP-CIDR` / `IP-CIDR6` 行不进前缀树、留在逐条求值的行里；而常用的列表每条 IP 行都带 `no-resolve`（blackmatrix7 的 `ChinaMax.list`，2026-10-11 读它自己的 README：124,509 行里 `IP-CIDR` 8,244 条、`IP-CIDR6` 4,281 条，`_Resolve` 版本只差这些行不带 `no-resolve`）。FR-RULE-05 的"IP 前缀树"只对一半的写法成立，而 2026-10-10 的需求审计把这一条判成了"做了" | 新加任务 8（控制者（调度这次执行的代理）裁定并进本计划：这正是本计划要补的那一类缺口，产品改动十来行）：集合里每条 `IP-CIDR` / `IP-CIDR6` 行都进前缀树，不论带不带 `no-resolve`。三个提交：先钉住现有的匹配结果，再加基准，再改 |
| 52 | 8 | 做法 | 一棵树加一个标志，不是带与不带各一棵树。`no-resolve` 写在行上只在地址未知时才有区别，而那时没有哪条 IP 行能命中，唯一的问题是集合要不要先问地址——那是集合的事实（有没有哪条 IP 行没写 `no-resolve`），不是行的。`CompiledSet` 的私有标志 `ips_resolve` 记它；它不是 `needs_dns`（那个更宽：嵌套集合、逻辑规则、`GEOIP`、`IP-ASN` 也置它，不论最后问不问）。每次求值的结论（命中 / 不命中 / 先要地址）与改动之前相同：评审逐格推过（目标 × 是否已解析 × 调用方的 `no-resolve` × 行的四种组合），实现者另以独立写的模型在新旧代码上各随机比了 10 万例、试了六种改错的写法（都没有提交；每种错法都有已提交的用例变红） |
| 53 | 8 | 使用者或读日志的人看得到的三处变化（结论与策略都不变） | 一、同一个集合有多行命中同一条会话时报成子规则的是哪一行：带 `no-resolve` 的 IP 行原先算在逐条求值的行里、按文件顺序，现在与不带的一样排在那些行之后；IP 行之间先看 IPv4 地址再看 IPv6 地址，同一地址族里取最长前缀，同一网段写两遍取前面一行。二、（实现者发现）集合里 rurge 运行期不求值的规则类型（`SUBNET`、`DEVICE-NAME`、`MAC-ADDRESS`、`SCRIPT`）写在带 `no-resolve` 的 IP 行之后时，命中那条 IP 行的会话现在也得到"`<类型>` rules are not supported in this version; treated as no match"的提示（每条规则一次）。三、（评审发现）没有载入 GeoIP / ASN 数据库时那句每个进程一次的告警，可能由这样一条会话提前触发。三处都是不带 `no-resolve` 的 IP 行一直有的行为；代项目所有者裁定接受，前两处登记进兼容性清单 |
| 54 | 8 | 度量（不是门禁，也不是 p99） | criterion 基准，十万行 `IP-CIDR,<各不相同的 /24>,no-resolve` 的集合求值一次，点估计，量了一次：改动前落空 897.71 µs、命中最后一个网段 905.20 µs；改动后 20.098 ns 与 1.1633 µs。NFR-01 写的是"10 万条规则集下单次规则匹配 p99 < 50 µs"，改动之前这样的集合约是它的十八倍。改动后的落空是前缀树的最好情形（任务 9 补了一个紧挨最后一个网段的落空用例）。这一对数字是同一次会话量的；同一段代码在另一次会话里"命中"量到 127.00 ns，绝对值不要单独引用（第 61 行）。NFR-01 这一项仍没有照它的写法（p99）验证过 |
| 55 | 8 | 评审的七条小项 | 四条（`eval` 注释里"最长前缀"只在同一地址族内成立；缺一条"逐条求值的行自己要地址、IP 行全带 `no-resolve`"的用例；基准里一句描述旧求值方式的注释；落空用例只量了最好情形）并进任务 9 的第一个提交；两条不改（一句今天不可能失败的断言，是任务说明里要求的，留着防将来；两个分支各有三行相同的置位）；一条是给文档的（提交说明只列了两处可见变化，实际三处——见第 53 行） |
| 56 | 9 | 计划没有这个任务。核对任务 8 的事实时，把 FR-RULE-05 所依据的手册原句整段读了一遍（`rules/ruleset.html`，2026-10-11："IP-ASN entries are checked in constant time."），又在 GitHub 上读了两份常用的、为 Surge 写的 ASN 列表（`missuo/ASN-China` 的 `ASN.China.list`：5,234 行、其中规则行 5,230；`VirgilClyne/GetSomeFries` 的 `ruleset/ASN.China.list`：5,270 行、规则行 5,076）：每条规则行都写成 `IP-ASN,<号码> // 说明`。rurge 的规则集文件只认整行注释，行尾的说明成了取值的一部分，号码读不出来——**两份列表整份被跳过**，集合是空的，使用者只在加载时看到一条 `W0023`（数着被跳过的行数，引用第一条的取值）；另外，集合里的 `IP-ASN` 行留在逐条求值的行里，不是手册与需求说的常数时间 | 新加任务 9（控制者裁定并进本计划，理由同第 51 行：需求审计判成"做了"的同一条需求的另外两处）。五个提交：任务 8 评审留下的四件小事；甲、规则集文件的行可以带行尾注释；钉住 `IP-ASN` 行现有的匹配结果；基准；乙、集合里的 `IP-ASN` 行按号码查 |
| 57 | 9 | 甲的读法，与它的依据有多硬 | `RULE-SET` 文件的每一行照 Profile 的行尾注释规则读（`rurge_config::text::strip_inline_comment`：空白之后、双引号之外的 `#`、`;`、`//` 起是注释；` #!` 与 ` //!` 不算）；`DOMAIN-SET` 文件不变（只认整行注释）。内联的 `[Ruleset …]` 小节一向经 Profile 的读取器、本来就这样读，两种集合从此一致。依据：手册对 Profile 写明了这条规则（`profile/format.html`："Inline comments are also supported."、"there must be at least one space before the delimiter"），又说内联规则集与外部文件是同一种行语法（`rules/ruleset.html`）；"Surge 读得了集合文件行尾的注释"**是从那两份列表推断的**（为 Surge 写的，每一行都这样写，第一个仓库的 issue 里没有人说读不了），手册没有写。代项目所有者裁定 |
| 58 | 9 | 甲之前另有两种不出声的写法（实现者的红用例读出来的），以及使用者可能遇到的两处变化 | `DOMAIN-SUFFIX,example.com // note` 原先被收下，后缀是 `example.com // note`，什么也匹配不到，没有告警；`IP-CIDR,10.0.0.0/8,no-resolve # lan` 原先被收下，而 `no-resolve # lan` 成了一个没人认识的参数，这一行丢了 `no-resolve`。现在两行都照作者写的读。变化一：取值里本来就有"空白加 `#`、`;` 或 `//`"的行（`USER-AGENT`、`PROCESS-NAME` 的模式，`URL-REGEX`）现在在那里截断，与同一行写在 `[Rule]` 或内联集合里一向的读法相同，用双引号括起来保持完整。变化二：每条 IP 行都写成 `…,no-resolve # 说明` 的集合，从"会先要地址"变成"从不要地址"——文件写的本来就是后者。两处都登记进兼容性清单 |
| 59 | 9 | 乙的做法 | 一张"号码 → 第一条点名它的行"的表加一个标志（有没有哪条 `IP-ASN` 行没写 `no-resolve`），理由与第 52 行相同：一个集合里所有 `IP-ASN` 行判断的是同一个地址（目标本身；名字则取解析到的第一个 IPv4 地址，没有才取第一个 IPv6 地址），`no-resolve` 只在地址未知时才有区别。标志既不是 `needs_dns`，也不是 IP 行的那个（`ips_resolve`）：两类行各凭自己要地址。只收集合里顶层的 `IP-ASN` 行；写在 `AND` / `OR` / `NOT` 里的随那条逻辑规则逐条求值。求值顺序成了：域名索引 → 逐条求值的行（文件顺序）→ `IP-ASN` 行 → IP 行。每次求值的结论与改动之前相同；一个地址的 AS 号被问的次数只会少不会多（评审逐格推过；实现者另以独立写的"旧的逐条比较"模型随机比了 3 万个集合、36 万次求值，试了 17 种改错的写法，都没有提交，每种错法都有已提交的用例变红；评审核过那个模型确是旧的逐条比较——它在旧代码上与旧代码逐次相同） |
| 60 | 9 | 读日志的人看得到的变化 | 与第 53 行同样的三处，这次是对 `IP-ASN` 行，根子是一件事：**逐条求值的行现在全部先走完**，所以写在一条命中的 `IP-ASN` 行之后的那些行也被求值了。一、报成子规则的是哪一行——`IP-ASN` 行原先按文件顺序夹在逐条求值的行之间，现在排在它们全部之后、IP 行之前（同时命中的 `DEST-PORT` 行不论写在哪里都先被报告；同时命中的 IP 行照旧排在 `IP-ASN` 行之后；一个号码写了两遍取前面一行）；二、运行期不求值的规则类型写在命中的 `IP-ASN` 行之后时那句提示现在也出现；三、写在它之后的 `GEOIP` 行现在会去问国家库（提交 `ee5b458` 的说明写"数据库被问的次数只会少"，那句话只对 AS 号成立——评审指出，提交不改写，以这一行为准），没有数据库时每个进程一次的告警因此可能提前（国家库）或推后（号码库：先被 `DEST-PORT` 一类的行命中的会话不再触发它）。没有第四处（差分检验没有找到，评审也没有）。任务 8 的 IP 行同理。代项目所有者裁定接受，前两处登记进兼容性清单 |
| 61 | 9 | 度量（不是门禁，也不是 p99；每一对前后数字都是同一次会话里背靠背量的） | criterion 基准，五千行照列表的写法（`IP-ASN,<号码> // 说明`）的集合求值一次，点估计：改动前落空 30.239 µs、命中最后一行 43.296 µs、名字未解析 24.260 µs（三个数字不同，criterion 自己给的区间互不重叠——落空 26.773 ～ 33.176 µs、命中 42.477 ～ 44.054 µs、名字未解析 23.907 ～ 24.678 µs——，原因没有查）；改动后 22.123 ns、94.030 ns、11.949 ns。**不同会话的数字不能互相比**：第 54 行那个"命中"的用例，产品代码一字未动，这一次会话量到 127.00 ns（上一次 1.1633 µs，相差九倍，原因没有查明；落空两次是 20.098 ns 与 17.726 ns）；这一次另量了紧挨最后一个网段的落空：43.917 ns。任务 9 做完之后在没有别的东西运行时又量了一次（第三次会话）：十万行的集合落空 19.371 ns、命中 131.83 ns、紧挨的落空 46.766 ns，五千行 `IP-ASN` 的集合 20.990 ns / 94.927 ns / 11.932 ns——三次会话里两次把"命中"量在约 130 ns，第一次的 1.16 µs 偏高 |
| 62 | 9 | 与任务说明的出入 | "先红后绿"的三条用例里有一条（一个号码写两遍报前面一行）在改动之前就是绿的——旧的逐条比较同样报前面一行，它守的是表里"记第一次"而不是"最后一次覆盖"这一个选择（改成覆盖它就红）；照说明应当停下来报告，实现者做完才报告，裁定接受。基准里的数据库替身按地址回答（说明里的那一句自相矛盾，执行中更正）。用例比说明列的多（21 条） |
| 63 | 9 | 评审（通过，五条小项） | 三条在修正里做了（`set.rs` 模块说明少了第四部分；单引号的读法没有用例——见下一行；没有一条经注册表的用例钉住"照常见写法写的文件加载后没有告警"）；一条是给文档的（第 60 行里国家库与告警的那半句）；一条不改（没写 `no-resolve` 时置两个标志的三行现在有三份） |
| 64 | 9 | 评审指出甲带来的第三处变化，实现者的报告没有登记 | 取值用**单引号**括起来、里面有"空白加 `#`、`;` 或 `//`"的行：拆字段的函数认单引号，切行尾注释的函数只认双引号，所以这样的行原先在规则集文件里读得完整，现在在引号里面被截断（行照收、没有告警、取值不是作者写的；`URL-REGEX,'^http://a // b'` 的取值成了 `'^http://a`）。这是 Profile 的读取器一向的读法，规则集文件现在与它一致；裁定不改（让切注释的函数也认单引号会改变 Profile 每一行的读法——取值里一个撇号就会吞掉后面的注释——，而 Surge 怎么读没有证据），用一条用例钉住，登记进兼容性清单：要保留就用双引号。另：甲顺带修好了一种写法——嵌套引用后面跟注释（`RULE-SET,other.list // 说明`）原先把 `other.list // 说明` 当成文件名（找不到，悄悄成了空集合） |
| 65 | 9 | 计划之外、早于本计划的一个缺陷，在这一轮修正里修了：规则集与域名集文件开头的 UTF-8 BOM 被算进第一行 | 实现者读出来、评审对着代码核实：集合文件的字节经 `String::from_utf8_lossy` 成为文本，沿途没有任何一处去掉 BOM（Profile 的读取器与订阅解析器一向去掉）。带 BOM 保存的 `RULE-SET` 丢第一条规则、`DOMAIN-SET` 丢第一个名字，第一行是注释时多出一条被跳过的行；文本在启动时已在缓存里才有一条 `W0023`，启动之后才下载到的列表什么也不说。裁定在这一轮修（控制者；与任务 9 是同一个函数、同一类问题——人们实际在用的列表文件 rurge 读不对——，一行）：`parse_set_with_limit` 在文本开头去掉一个 U+FEFF，别处的不动、行号不变。五条用例先红后绿（规则集与域名集各两条、经注册表一条），另一条钉住范围（只去开头的一个） |
| 66 | 9 | 计划之外、早于本计划的第二个缺陷，第二轮修正里修了：**域名集里读不了的行被告警原样引用** | 第一轮修正的实现者从红日志里读出来：`DOMAIN-SET` 文件里不是主机名的行，原因写成 ``invalid DOMAIN-SET line `<整行>` ``，而第一条被跳过的行的原因是 `W0023` 文字的一部分（每个加载 Profile 的命令的标准错误与日志）。域名集文件的一行是某个文件或 URL 正文里的任意文字：把订阅、一份代理行的列表误当成域名集时，它的第一行——分享链接、带 `password=` 的行——就进了告警。项目所有者的规矩是凭据不进日志与错误文本；规则行的读取器在 M8c 已经不引用"不像规则"的行，域名集是剩下的唯一一处引用整行的地方。裁定当场修（控制者）：原因改为 `invalid DOMAIN-SET line`，告警照旧给出行号（提交 `09f1a7a`；两条用例改之前是红的，另一条钉住"同样的三行当规则集读，原因里也没有口令"）。代价：域名集第一条被跳过的行的文字不再显示，只有行号。规则集的行的原因没有变（类型认得而取值不对时仍引用取值，与 Profile 里相同） |
| 67 | 10 | 计划里没有这一项。写文档时把 CLAUDE.md 里列的每个测试过滤词各跑了一遍，本里程碑的 `a_start_with_the_recorded_profile_leaves_state_json_as_it_is` 红了一次（`POST /v1/stop` 的状态读成 0）；N 次里失败一次的用例会让门禁 N 次里失败一次 | 查明并修了，详见「Task 10」一节。原因：辅助函数 `api_call` 用 `write!` 一段一段地写请求，正文可能晚于请求头到达；`/v1/stop` 一类的处理函数不读正文，服务端凭请求头就答复并关掉连接，晚到的正文引来一个 RST，Windows 上的客户端因此丢掉已收到而没读的应答（`ConnectionAborted`，os error 10053，状态 0）。证据各一句：有负载的 665 次运行里失败一次，失败的那次读到 10053、一个字节没读到，出在写完请求之后 13 µs；探针里正文晚到的请求每次都丢应答、一次写出的每次都得到应答；同样的写法对进程内的真实服务端发 1500 次丢了 12 次，一次写出 0 次。修法：`api_call` 与 `http_get_with` 把整个请求一次 `write_all` 写出（提交 `bbf220d`，在 `09f1a7a` 之后，只动 `crates/rurge/tests/cli.rs`）；之后同一组用例在三组用例同时反复跑着的负载下跑了 400 次，没有失败。服务端这一侧没有改，登记为延后事项第 35 行 |
| 68 | 10 | `run::a_listen_value_that_cannot_be_used_ends_the_start_without_quoting_it` 在同一轮反复跑里失败了一次：本该立刻放弃的启动 20 秒后还没有结束（辅助函数 `give_up` 的 panic："rurge run did not give up within 20 s"），那时有三组用例同时反复跑着；信息没有说是这个用例里九次启动中的哪一次，也没有说子进程打印过什么 | 读代码，那次启动的路径上没有东西在等（`Overrides::parse`、`error:` 一行、`recover_without_a_log`、退出码 2——`crates/rurge/src/cli/run.rs`）。提交 `6ceeba8` 让 `give_up` 在期限到时说出这次启动（设置的环境变量、程序、参数）、子进程已经写到标准错误的内容，和等待子进程是否失败过。改动之前这一组在同样的负载下跑了 398 次、失败一次，旁边一组 405 次没有失败，更早单独一组的 347 次也没有；改动之后同样的负载下 810 次（两组各 405 次）没有再出现，所以没有信息可引。这样合并：原因不知道，登记为延后事项第 37 行 |
| 69 | 7 | 文档任务的评审：通过，十四条小项 | 十三条在这一轮订正了：CLAUDE.md 里两条测试命令选中的用例比它们的说明少（`pre_match`；E0024 一行补四个过滤词与两句说明）；验收文档里十八处没带阶段的里程碑名；四处把证据说得比实际硬——子规则日志行里相对路径的集合名、`rurge check` 缺省建议的"每一处都成立"、`remoteAddr` 清单里哪些情形有用例、平台变量在 macOS 与 Linux 上的读取；一处没有依据的解释（三个不同的基准数字不是"测量噪声"，改为原因没有查；清单里的"三次会话"改为"三次测量"）；范围与位置——需求文档状态行里的行号范围、验收文档里已决事项的范围，FR-CFG-15 一行漏记的提交，测试库 README 少的一句，M2 设计第 17 节漏的一条和开头一句，CLAUDE.md 状态段开头一个子句的位置，Task 8 里三条用例所在的文件。一条改不了：提交 `83e3932` 的说明正文提到了工作过程，与 `fa75f2d` 的说明里那处裸写的 M5b 同一类；历史不改写 |
| 70 | — | 2026-10-11 准备阶段 1 补遗（M5b）时，手册的 `rules/final.html`、`profile/format.html`、`profile/managed-profile.html`、`rules/ruleset.html` 和 `tools/http-api.html` 又读了一遍：设计与清单引用它们的句子逐字相符；读出两处早于本计划、与手册不同的地方 | 一、几条启用的 `FINAL`：手册的几句话合起来是"匹配在第一条启用的 `FINAL` 处结束，策略与选项取最后一条的"，rurge 让写在两条 `FINAL` 之间的规则仍被评估；登记在兼容性清单 3.1 节的 `FINAL` 一行（第一格与备注带"目前"，状态标记没有动）和清单开头一句、验收文档「要定的事」、CLAUDE.md；改动排在阶段 1 补遗（M5b）的第一项（延后事项第 36 行）。二、重名的具名节：手册说两个被 include 的文件提供同名的节、或被 include 的节与 Profile 里已有的同名时加载失败，rurge 不拒绝，给 `W0020`、用第一段（比手册说的 Surge 宽松）；登记在清单 1.3 节通配命名节一行的备注里，决定保持现状，没有延后事项 |
| 71 | — | 整分支的评审（2026-10-11，范围 `e347044..2c9762c`，即本分支的全部改动与文档）：可以合并；没有 Critical 与 Important；六条小项——两份 API 参考把内部 DNS 会话的 `remoteAddr` 说成"上游服务器的地址"；`external` 策略拉起的程序继承 `RURGE_LISTEN_HTTP`；`crates/rurge/tests/cli.rs` 里取端口的助手是第三份、测试约定里没有它；测速会话的三个键恒为空没有断言；`parse_http_listen` 的注释把错误说成告警；`E0008` 的缺省建议在 `include-other-group` 这一处只对一半 | 六条这一轮都改了，见记录第 72 ～ 74 行与延后事项第 39 行。评审没有运行 cargo；它证明不了只有三平台 CI 与真机才能证明的部分（`sighup_reloads_the_profile` 与平台变量的读取在 Linux、macOS 上的编译与运行）和 Surge 自己的行为（行尾注释、同一资源作两种集、几条 `FINAL`、行尾的平台标记）：这些仍是延后事项里原有的各行，不因这次评审而变 |
| 72 | — | `external` 策略拉起的程序继承 rurge 的全部环境，只去掉六个代理变量：`RURGE_LISTEN_HTTP`（本里程碑加的建议是用它放 HTTP 监听的口令）连口令一起交给程序，程序里的一句 `env` 或 `set -x` 就会把它写进 `<数据目录>/external/<策略名>.log`；拉起的程序本身是另一个 rurge 时（`run_starts_an_external_program_and_stops_it_on_exit` 里就是），它还会照第一个的设置监听，不照自己 Profile 里的 `local-port` | 名字以 `RURGE_` 开头的变量（不分大小写，Windows 的变量名也不分）都不交给程序：`is_own_setting`（`crates/rurge-proto/src/external.rs`）判断，`command()` 对当前进程环境里每一个这样的变量 `env_remove`。按前缀而不列名单：这些名字声明在可执行文件里，这里的名单迟早过时；不调用 `std::env::set_var`（edition 2024 里它是 `unsafe`，工作区禁用 `unsafe`）。两条用例：单元用例 `rurges_own_settings_are_told_by_their_prefix`（名字表，含一个多字节字符跨过第六个字节的名字，不 panic），和进程级的 `run_starts_an_external_program_and_stops_it_on_exit`——外层 rurge 带着 `RURGE_LISTEN_SOCKS5=127.0.0.1:0` 启动，改动之前内层 rurge 照这个值监听、不听在 Profile 里的 `local-port` 上，经 `external` 策略的请求得到 502（`external: the local SOCKS5 port refused the connection`），改动之后得到 200。兼容性清单 `external` 一行与 `rurge run` 一行、CLAUDE.md、设计第 12 节第 14 条同步 |
| 73 | — | `include-other-group` 里不是策略组的名字报 `E0008`，缺省建议是在 `[Proxy]` 或 `[Proxy Group]` 里定义缺的策略或组、或改正名字；名字是 `[Proxy]` 里已有的一条策略时（这个参数只收组），照建议做不成。兼容性清单自己说"缺省建议在发出这个码的每一处是否贴切，是读那些发出点得出的"，这一处读漏了 | 这一处带自己的建议：``give `include-other-group` the names of groups defined in `[Proxy Group]`, and list a single policy as a member of the group instead``。策略组的读取器（`crates/rurge-config/src/spec/group.rs` 里的 `Reader`）多一个 `error_with_hint`，与 `error` 共用 `report` 一个函数体；`E0008` 的其余各处仍用缺省建议，仍在 `diagnostic::tests` 的 `WITH_DEFAULT` 里，59 / 33 / 25 的数没有变。用例：`names_are_checked_against_the_profile` 里加了两条 `E0008` 各带这一处的建议（改动之前拿到的是缺省建议），`values_that_cannot_be_used_are_errors` 里加了组上别的错误仍带各自代码的缺省建议。兼容性清单 `--check` 一行同步 |
| 74 | — | 四处小事：(一) 内部 DNS 会话的 `remoteAddr`：两份 API 参考写成"上游服务器的地址"，读代码（`Engine::dial_internal` 把整段拨号放进这条会话的 `Trail`）得出这只在会话直连时成立；(二) 测速会话的 `listenPort`、`remoteAddr`、`dnsMs` 恒为空，设计与 API 参考都这么写，没有断言；(三) `run_starts_an_external_program_and_stops_it_on_exit` 里内层 rurge 的端口是 `bind(0)` 再放掉的，放掉的号可能被并行的用例拿走；(四) `parse_http_listen` 的注释说 Profile 对读不了的一项告警并继续，实际是错误（`E0013`）并继续读 | (一) 先钉后改：`a_dns_session_goes_through_a_proxy_configured_by_ip` 加断言，经以地址配置的 `http` 代理的会话 `remoteAddr` 是代理的地址（把期望换成 DNS 服务器的地址试过一次，红，实际值是代理的地址，之后改回）；`a_dns_session_bypasses_a_proxy_configured_by_host_name` 加断言，因代理以域名配置而改走直连的会话是 DNS 服务器的地址。读代码得出的与断言看到的一致，两份 API 参考照改；因 REJECT 改走直连的内部会话与 DoT / DoH 仍是读代码得出、没有用例。(二) `every_test_is_a_session_of_the_request_log` 对 `A` 与 `Dead` 各加 `(listen_port, remote_addr, dns_ms) == (0, None, None)`：钉住已有的行为，改动之前也过。(三) 换成 `free_tcp_port()`，不动另两份助手（延后事项第 39 行）。(四) 注释改成与代码一致的说法，只改注释 |

## 延后事项

| # | 事项 | 去向 |
| --- | --- | --- |
| 1 | SIGHUP 的 CLI 用例只在 Unix 上编译与运行，本机无法验证 | 第一次 CI 运行时核对（M8a 任务 6 同一次） |
| 2 | 平台变量在 macOS 与 Linux 上的读取在本机只对着假的命令与文件测过；真实系统上命令的输出与文件的格式是否如所料，要看那两个平台上 `cli.rs` 的 `SYSTEM_VERSION` 用例 | 同上 |
| 3 | `remoteAddr`：复用已有连接或隧道的会话与 UDP 流没有这一项 | 阶段 4（FR-HTTP-10 完整） |
| 4 | DNS 查询与 TCP 握手两个阶段的预匹配 | 阶段 3 |
| 5 | `pre_match_domain` / `pre_match_ip`（`crates/rurge-rules/src/engine.rs`）仍然没有调用方：DNS 查询与 TCP 握手两个阶段的预匹配要等有那两个阶段的入站（与第 4 行是同一件事的代码一侧） | 阶段 3 |
| 6 | 集合文件 8 层嵌套的上限取决于登记的先后：先在深处遇到的集合留着被截断的链，Profile 之后直接引用它时也是。早于本计划，只影响超过 8 层的链（任务 1 修正的复审读出来的，没有复现） | 有用户报告再说 |
| 7 | `W0032` 没有父集合的那一句没有用例 | 下次动到时 |
| 8 | 手写的日志捕获有三处（`rurge-proto-quic` 的 `test_log`、`rurge-rules` 的单元用例、`rurge-engine/tests/common`），可以并成一个 | 下次动到时 |
| 9 | Surge 自己怎么处理"同一个资源既当 `RULE-SET` 又当 `DOMAIN-SET`"不知道（没有证据）；rurge 的做法（Profile 里直接冲突是 `E0024`，嵌套的冲突是 `W0032` 并忽略那条引用）是自己定的 | 项目所有者（手边有 Surge 时核对） |
| 10 | **`external` 策略 `local-port` 的检查看不到 rurge 的缺省监听。** `own_listener`（`crates/rurge-config/src/config.rs`）只读 Profile 写出来的监听。一份不写任何监听键的 Profile 监听 `127.0.0.1:6152` 与 `:6153`（开了 `allow-wifi-access` 时是 `0.0.0.0` 上的两个 `wifi-access-*-port`），这时 `external` 策略写 `local-port=6153` 能加载、没有诊断，启动之后经这条策略的每个连接都是 rurge 连自己。早于本计划（阶段 2 / M4c），是从代码读出来的，没有起这样一个进程复现。改法：把"Profile 什么都没写时在哪里监听"的规则从 `Engine::listener_specs`（`crates/rurge-engine/src/engine.rs`）搬进 `rurge-config`，作 `General` 的一个方法，引擎与 `own_listener` 共用，`E0018` 的文本说明那是缺省的监听 | 项目所有者（要不要在阶段 3 之前做）；不做则阶段 3 |
| 11 | `rurge service install` 不把 `--listen-http` / `--listen-socks5` 带进安装的服务 | 项目所有者 |
| 12 | `own_listener` 不认 IPv4 映射的 IPv6 写法（`[::ffff:127.0.0.1]:P`）是回环，加载期与覆盖之后都一样 | 下次动到时 |
| 13 | `rurge_platform::sysinfo` 没有在 macOS 或 Linux 上编译、运行过；macOS 的三种格式（`SystemVersion.plist`、`defaults read -g AppleLanguages`、`scutil --get`）取自公开描述（与第 2 行是同一件事，这里补上范围） | CI 首跑；手工清单（真实的 Mac 与 Linux 机器各一行） |
| 14 | macOS 给链接旧 SDK（早于 macOS 11）的二进制看兼容版本号 `10.16`，没有处理 | 有用户报告再说 |
| 15 | macOS 上每个加载 Profile 的命令起至多四个短进程（`sysctl`、`defaults`、`scutil` 一到两次），开销没有量过；三个工具经 `PATH` 找（与现有的 `networksetup` 一样），都没有时限——`cfprefsd` / `configd` 卡住的 Mac 上，每个加载 Profile 的命令都会挂住 | 手工验收时量；时限等下次动到 `CommandRunner` |
| 16 | macOS 上 root 身份的守护进程读到的是 root 的 `AppleLanguages`，多半没设，`LANGUAGE` 于是落到区域变量再到 `en-US`（评审的推断，没有在 Mac 上见过）；Windows 容器镜像上的 `DEVICE_NAME` 没有核对 | 手工验收 / CI 首跑 |
| 17 | `C` 与 `POSIX` 在 `language_of` 里区分大小写（小写的 `c`、`posix` 会被当成语言名），与改动之前相同；一个只是以 `OEM` 开头的真实型号会被当成占位值 | 有用户报告再说 |
| 18 | `E0018` 的缺省建议指向手册里这种策略或组的页面；在 rurge 自己的限制上（`server` 写不成 ASCII 的 IDNA、SOCKS5 凭据的长度、`obfs-host` / `obfs-uri` / `ws-path` 的限制）手册那一页帮不上。这些发出点要各自的建议，得先给 `ParamReader::error` 加一个带建议的变体，再逐处（44 处直接发出点）定 | 下次动到时 |
| 19 | 内联 `[Ruleset X]` 的一行上出现 `E0014` 的情形时，报出来的是 `W0018`（`line skipped: …`），不带那一处的建议 | 下次动到时 |
| 20 | Snell 的"池里取出的连接已失效，换新连接重发"在拨号返回之后才开 socket（`crates/rurge-proto/src/snell/tunnel.rs`），这样的会话自己开了 socket 而 `remoteAddr` 是 `null`（与第 3 行同一件事的一个边角） | 阶段 4（FR-HTTP-10 完整） |
| 21 | QUIC 类策略经 `underlying-proxy` 时，底层策略的 UDP 载体若是当场打开的一条 TCP 连接（带 `udp-relay=true` 的 `socks5` / `socks5-tls`、`external`、`trojan`、v3 起的 `snell`、没有空闲会话的 `anytls`；`wireguard` 与 `vmess` 不是），碰巧打开 QUIC 连接的那一条 TCP 会话的 `remoteAddr` 是底层策略服务器的地址（"链的第一跳"），复用这条连接的其余会话是 `null`。读代码得出，没有用例 | 阶段 4（FR-HTTP-10 完整） |
| 22 | `dnsMs` 在一次解析结束时才记下：卡在一次慢解析里的进行中会话显示 `null`，解析结束（或拨号超时）后才有数 | 阶段 4（FR-HTTP-10 完整） |
| 23 | 与第 1 行是同一件事，这里补上范围：`sighup_reloads_the_profile`（`crates/rurge/tests/cli.rs`，`#[cfg(unix)]`）没有编译、运行过，它用到的 nix 0.31.3 的每个签名对着锁定版本的源码核过，函数体另在本机以同签名的替身通过了类型检查；同一文件里 `run_shuts_down_gracefully_on_sigint` 新加的一行等待同样没有照提交的样子编过（去掉 `cfg` 属性做过一次类型检查） | CI 首跑 |
| 24 | **满额的集合占多少内存算"可控"没有判据**（FR-RULE-05 没有数字）。实测（Windows，发布构建，峰值工作集，四轮）：100 万条的 `DOMAIN-SET` 载入时比基线多约 403 MB、建好之后多约 130 MB；100 万条的 `RULE-SET` 载入时多约 456 MB、建好之后多约 150 MB；两个满额集合建好之后约 280 MB，已到 NFR-01"1 万并发连接常驻内存 < 300 MB"的九成，载入一个时的瞬时量单独就超过它。读代码的推断、没有量：刷新时旧的集合要到新的编译完才放手，所以刷新的瞬时量比第一次载入再高约一个稳定水位。只量了 Windows。Surge 自己的做法（手册 `rules/ruleset.html`，2026-10-11 读）："sets with more than 1000 domain entries use an on-disk database instead of an in-memory tree"，"If a set contains more than 50 IP-CIDR (or IP-CIDR6) entries, they are compiled into a binary IP database"——大集合不放在内存里；rurge 的索引全在内存。要把内存压下来，方向是同一个（索引放在磁盘上按需读），那是一项设计，不是小改 | 项目所有者（"可控"的判据；NFR-01 的前提；要不要做磁盘上的索引） |
| 25 | 日志捕获里的第二个 `Dispatch`（照 `rurge-proto-quic` 的 `test_log` 写的，防"别的线程先到过的调用点一直关着"）的必要性没能复现：去掉它把相关用例反复跑也不失败。留着（无害，与那一份一致） | 下次动到时 |
| 26 | 集合里除 `DOMAIN`、`DOMAIN-SUFFIX`（域名索引）与 `IP-CIDR`、`IP-CIDR6`（前缀树）、`IP-ASN`（任务 9 起按号码查）之外的行仍按文件顺序逐条求值：`DOMAIN-KEYWORD`、`DOMAIN-WILDCARD`、`GEOIP`、`USER-AGENT`、`URL-REGEX`、`PROCESS-NAME`、三种端口、`SRC-IP`、`PROTOCOL`、`HOSTNAME-TYPE`、逻辑规则、嵌套集合，以及运行期不求值的 `SUBNET`、`DEVICE-NAME`、`MAC-ADDRESS`、`SCRIPT` 与从不命中的 `CELLULAR-RADIO`、`CELLULAR-CARRIER`。与手册对 Surge 的说法一致（`rules/ruleset.html`："All other rule types are evaluated linearly."）。看过的一份常用列表里这样的行不多（2026-10-11 读 blackmatrix7 的 `ChinaMax.list` 自己的 README 里的计数：124,509 行里 `DOMAIN-KEYWORD` 13、`PROCESS-NAME` 12、`USER-AGENT` 65，共 90 行）；几百行的逐条求值没有量过，一份有几万行这类行的列表仍然慢 | 有用户报告再说 |
| 27 | `CompiledSet::needs_dns` 是公开字段而没有生产代码读它（只有集合自己的用例读） | 下次动到时 |
| 28 | **需求审计判成"做了"的条款没有人复核过，这一次查出一条只做了一部分**（FR-RULE-05：IP 行只有一半的写法进前缀树，`IP-ASN` 行不是常数时间，常用的 ASN 列表整份被跳过——任务 8、9）。阶段 1 补遗收尾、要写"齐备"之前照 CLAUDE.md 的规矩再对一遍需求；这一次每条"做了"要指到一条会因它而红的用例，并对着手册的原句、拿一份真实的列表或配置读 | M5b 收尾（阶段 3 开始之前） |
| 29 | `DOMAIN` / `DOMAIN-SUFFIX` / `DOMAIN-KEYWORD` 的取值不校验（`parse_kind`，`crates/rurge-config/src/rule.rs`）：`DOMAIN-SUFFIX,exa mple.com` 写在 `[Rule]` 或集合里都能加载、没有诊断、永远匹配不到；`DOMAIN-SUFFIX,.` 这样去掉点后为空的取值在集合里被悄悄丢掉而仍计入条目数（后一句只读了代码）。早于本计划 | 下次动到时 |
| 30 | 集合文件里的要求指令不是指令：写在行首（`#!MACOS-ONLY DOMAIN,a.com`）时整行在所有平台上都被当成注释丢掉；写在行尾（`DOMAIN,a.com #!MACOS-ONLY`）时这一行被收下、取值里带着指令的文字、永远匹配不到。两种都没有提示。同样的行写在内联的 `[Ruleset X]` 小节里，由 Profile 的读取器拆出指令、按平台生效——两种集合在这一点上不一样。Surge 认不认外部集合文件里的要求指令不知道。早于本计划 | 有用户报告再说（M5b 重读计划时先查手册 `profile/requirement.html`） |
| 31 | **集合文件的行上的未知参数没有人报告**（`parse_rule_line` 把 `SubRule.unknown` 丢了；内联小节有 `W0003`）：`no-resolve` 拼成 `no-resolv`，或写成 `no-resolve#说明`（标记前没有空白，不算注释），这一行就丢了 `no-resolve` 而什么也不说。项目自己的原则是"解析并忽略 + 记录日志"，这里缺了后一半。早于本计划。改法：每个集合数一下带未知参数的行，建注册表时发一条（第一处的行号，不引用参数），并进集合的状态 | M5b（它给外部资源加接口、要动集合的状态；写计划时定） |
| 32 | 启动之后才载入或刷新的集合跳过了行，什么也不说：数被跳过的行的那条 `W0023` 只在建注册表时发（文本当时已在缓存里才有），之后只更新集合状态里的计数，而阶段 1 没有接口显示这个计数。早于本计划 | M5b（外部资源的接口显示每个资源的 `skipped`） |
| 33 | 切行尾注释的函数只认双引号，拆字段的函数另认单引号（记录第 64 行）；这是 Profile 读取器早有的不一致，规则集文件现在继承了它 | 有用户报告再说 |
| 34 | `--listen-socks5` 的取值写得不对时，报错的原因是 `address must be an IP literal like 0.0.0.0:6152`：这句话在 `parse_listener`（`crates/rurge-config/src/general.rs`）里，与 `E0013` 共用，示例端口 6152 是 HTTP 的缺省端口，对 SOCKS5（缺省 6153）不贴切；没有改 | 项目所有者（改不改；要改就是给两处各一句） |
| 35 | **API 的一处性质：不读请求正文的端点，和在读正文之前就被拒绝的请求，只凭请求头就答复并关掉连接；正文晚于请求头到达时，客户端可能读不到那个应答。** 客户端看到的（只在 Windows 上量过，别的系统没有量）：正文的写成功，接着的读失败——`ConnectionAborted`（os error 10053），一个字节也读不到——，尽管请求已经执行。量过的是 `POST /v1/dns/flush`、`/v1/stop`、`/v1/profiles/reload`、`/v1/profiles/check` 和密钥不对的 `401`；`403`、`404`、`405` 的应答同样不读正文（读代码得出），没有量过。请求头与正文一次写出的客户端、不发正文的客户端不受影响。早于本计划；来由与证据在「Task 10」一节，写给使用者的版本在 `docs/api/phase1.md` 的「鉴权与错误」 | 阶段 1 补遗（M5b）（它本来就要动 API） |
| 36 | **几条启用的 `FINAL` 之间写的规则仍被评估，手册的读法与此不同。** 手册（`rules/final.html`）："If several enabled FINAL rules are present, the last one is the effective one."；`profile/format.html`："In the `[Rule]` section, a `FINAL` rule immediately terminates rule matching. Rules included or defined after it are never evaluated, …"。rurge 今天：策略取最后一条启用的 `FINAL` 的（与手册一致），但写在两条 `FINAL` 之间的规则仍被评估——`FINAL,DIRECT` / `DOMAIN,a.com,P` / `FINAL,P` 里，`a.com` 被中间的 `DOMAIN` 规则命中；最后一条之前的每条 `FINAL` 得 `W0021`，`W0019` 只数最后一条之后的规则（钉着这一点的用例：`last_final_wins_and_shadowed_final_is_dropped`、`rules_after_the_last_final_are_excluded`，`crates/rurge-rules/src/engine.rs`；`last_final_takes_effect_and_earlier_final_is_shadowed`、`dead_rules_after_last_final_are_counted`，`crates/rurge-config/src/config.rs`）。决定要改成的读法，是能让手册那几句话同时成立的唯一一种：匹配在第一条启用的 `FINAL` 处结束，策略与选项（`dns-failed`）取最后一条启用的 `FINAL` 的；它是从手册的几句话推断的，没有在 Surge 上观察过。受影响的只有带两条或更多启用的 `FINAL`、且它们之间还有规则的 Profile——`#!include` 一个自己以 `FINAL` 结尾的规则文件、又在 include 之后接着写规则，就是这样。早于本计划，2026-10-11 重读手册时发现；登记在兼容性清单 3.1 节的 `FINAL` 一行 | 阶段 1 补遗（M5b）的第一项 |
| 37 | `run::a_listen_value_that_cannot_be_used_ends_the_start_without_quoting_it` 里有一次启动没有在 20 秒内结束，**原因不知道**（记录第 68 行）。再出现时先看辅助函数 `give_up` 现在带的信息：它说出这次启动（环境变量、程序、参数）和子进程已写到标准错误的内容。标准错误里已经有 `error: --listen-…` 一行，说明子进程把该做的做完了、卡在退出的路上——那是产品的缺陷；标准错误是空的，说明它没有走到 `run`——那是机器 | 再出现时 |
| 38 | `api_call` 与 `http_get_with`（`crates/rurge/tests/cli.rs`）仍然丢掉读应答时的错误（`let _ = s.read_to_string(…)`），所以第一次失败只显示成"状态 0"，看不出是 10053；记录第 67 行的那次修改没有动它 | 下次动到时 |
| 39 | "取一个先写进配置的端口"的助手有三份：`closed_tcp_port`（`crates/rurge-engine/tests/common/mod.rs`）、`free_port`（`tests/external/tests/common/mod.rs`）、`free_tcp_port`（`crates/rurge/tests/cli.rs`）。三个测试 crate 各一份，没有共用的测试支撑 crate，改一份不会改到另两份 | 下次动到时 |
