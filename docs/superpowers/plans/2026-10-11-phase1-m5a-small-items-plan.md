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

## 延后事项

| # | 事项 | 去向 |
| --- | --- | --- |
| 1 | SIGHUP 的 CLI 用例只在 Unix 上编译与运行，本机无法验证 | 第一次 CI 运行时核对（M8a 任务 6 同一次） |
| 2 | 平台变量在 macOS 与 Linux 上的读取在本机只对着假的命令与文件测过；真实系统上命令的输出与文件的格式是否如所料，要看那两个平台上 `cli.rs` 的 `SYSTEM_VERSION` 用例 | 同上 |
| 3 | `remoteAddr`：复用已有连接或隧道的会话与 UDP 流没有这一项 | 阶段 4（FR-HTTP-10 完整） |
| 4 | DNS 查询与 TCP 握手两个阶段的预匹配 | 阶段 3 |
