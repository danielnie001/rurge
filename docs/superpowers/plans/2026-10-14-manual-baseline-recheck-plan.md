# 手册基线重对 实施计划

> **执行方式**：子代理驱动（superpowers:subagent-driven-development）——每个任务一个新的实现者，做完由新的评审者评审，全部做完后整分支评审一次。本计划写到"改哪里、接口是什么、要哪些用例、判据是什么"为止：实现者先写失败的用例、看到红，再实现到绿；与计划的出入记进末尾的「执行期修正记录」。

**目标**：兼容性清单与 Surge Mac 6.10.0 / iOS 5.23.0 的手册一致（报告第 5 节的 123 项逐行改动）；手册的新句子落到阶段 1、2 已经声称支持的条目上的十一处代码改动做完；项目各份文档的手册基线前移。

**做法**：十一处改动都落在已有的结构里。九处很小——一张表多一项、一个判断多一支、一句文本、一组用例；两处动接口：`GeoLookup` 的两个方法改为返回三种结果之一（"没有库"与"查不到"分开），解析器的依赖从一个连接器变成每种上游一个（内部会话的协议标签由此按方案定）。不新增 crate，不新增诊断码。文档放在最后一个任务：清单的行要引用前四个任务的用例名。

**依据**：设计 `docs/superpowers/specs/2026-10-14-manual-baseline-recheck-design.md`（第 3 节 MB-D1 ～ D22、第 4 节十一处改动、第 7 节文档；下面写"设计"指它）；调查结果 `docs/reports/2026-10-14-manual-baseline-recheck.md`（第 5 节 R1 ～ R123 逐行的改动、第 6 节附录的预期数字；下面写"报告"指它）。

## 全局约束

- 不新增 crate，`Cargo.lock` 不变；没有新的依赖边。全工作区 `forbid(unsafe_code)`，`rurge-platform` 的两个例外不增加；MSRV 1.89，edition 2024。
- 不新增诊断码：这一轮用到的 `I0001`、`W0001`、`W0002`、`W0008`、`W0013`、`W0016`、`W0026`、`W0028`、`E0018` 都是已有的码，它们在 `codes::hint` 里的决定都不变（`every_code_is_decided_once` 不该变红）。
- 凭据不进日志、错误文本、诊断、`Debug` 与 API 输出。诊断不引用配置里的取值：`custom` 行的 `I0001` 只带策略名（经 `rurge_config::policy::named`）；`tcp://` 的 `W0026` 照 `quic://` 现有的那一句引用写出来的那一项，带用户名口令的 URL 照旧先被拒、文本只有最后一个 `@` 之后的部分。新的带凭据的写法（`custom` 行位置上的口令）写进标记用例 `crates/rurge-config/tests/secrets.rs`。
- 解析器怎样把一行拆成字段这一轮不动（设计第 6 节）。脱敏照解析器的读法遮字段，所以解析器对一行的读法有任何变化，都在同一次改动里对着 `crates/rurge-config/src/redact.rs` 核对——这一轮唯一沾边的是 `custom` 成了认识的类型（任务 1）。
- 测试只用回环地址与端口 0（要先于主人绑定就写进配置的端口或必须关着的端口取自 `closed_tcp_port()` / `closed_udp_port()`），等待都有时限，不用固定的 sleep 当同步；不联网。测试在**失败**时也不得离开回环、不得写到自己的临时目录之外：Profile 里写了主机名或固定端口的用例另给第二道防线（一条按地址的规则、一行 `[Host]`，或一个会拒绝的连接器）。没有哪条用例把回环之外的地址（文档地址也算）写成可能被拨的目标；规则匹配的单元用例拿公网地址当查库的数据不在此限（它们不经任何连接器）。新的用例不把 `localhost` 之外的名字交给系统解析库。
- "改一行代码看用例变红"的验证之前，先写下这一改会让哪些会话、查找、下载与文件去哪里；可能离开回环或写到用例的临时目录之外的，不做。
- 新写的计时用例与靠随机数的用例，提交前单独循环跑二十次以上。
- 不改系统设置（不运行 `rurge run --system-proxy`，`rurge service install` / `uninstall` 只带 `--dry-run`）；不下载、不安装任何东西；只运行本仓库构建出来的二进制。
- Surge 语法就是 rurge 语法；与 Surge 的每一处行为差异都进兼容性清单（任务 5）。手册的文字不进仓库，除了设计与报告已经引用的句子。
- 注释、日志与错误文本用英文，照所在文件的写法；提交说明的标题用中文，说改了什么、为什么。
- 每一处改动一个提交：先写失败的用例、看到红，再实现到绿。设计 4.9 的八条用例本来就该是绿的：各做一次"改一行代码看它变红、再还原"的验证；哪一条写出来是红的，就是又查出一处缺口——停下来报告，不顺手改。
- 门禁（每个任务末尾一次，任务里的每个提交之前跑它自己碰到的测试目标）：`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test -j 8 --workspace --no-fail-fast`（本机 `RUSTFMT` 指向 gnu 工具链的 rustfmt）。本机上聚焦地跑用例也经整个工作区（`cargo test -j 8 --workspace --lib -- <过滤词>`，或 `--test <目标> -- <过滤词>`），不用 `-p`（它会另编一份依赖）。测试二进制无失败用例的异常退出（`STATUS_HEAP_CORRUPTION`）单独重跑那个二进制。门禁之前看一眼磁盘，剩余不足约 15 GiB 就停下。

## 评审要点

计划的用例之外最可能咬到使用者的五种情形，各自归到拥有那段代码的任务里：

1. 数据库在启动之后才到（rurge 不捆绑数据库，第一次下载之前没有）：`GEOIP,UNKNOWN` 在库载入之前不匹配，库载入之后**同一个**规则引擎开始匹配，不必重载（任务 2）。
2. 旧的 `custom` 行带着今天的命名参数与一个早就没有的参数（`obfs=http`、`udp-relay=true`、`ota=true`）：读成 `ss`，只多一条未知参数的 `W0001`；口令里带 `=` 的行是 `E0018`，不是悄悄连错（任务 1）。
3. `policy-path` 的列表里的 `custom` 行：同样读作 `ss`，不被跳过，也不为它说任何话（任务 1）。
4. 两个 `wireguard` 策略指向同一个节而 `tos` 不同：照旧共用一条隧道，谁也不顶掉谁的连接（任务 1）。
5. `tcp://` 的服务器写了路径：这一个被跳过并告警，Profile 照常加载，查找经其余的服务器照常完成（任务 1）。

---

## 文件结构

| 文件 | 任务 | 职责 |
| ---- | ---- | ---- |
| `crates/rurge-config/src/text/mod.rs`、`src/deferred.rs` | 1 | `[IP Rewrite]` 是有序的、留到后面阶段的节 |
| `crates/rurge-config/src/policy.rs`、`src/spec/ss.rs`、`src/config.rs` | 1 | `custom` 关键字、位置上的方法与口令、每行一条 `I0001` |
| `crates/rurge-config/src/redact.rs`、`tests/secrets.rs` | 1 | `custom` 成了认识的类型之后脱敏的注释与用例；标记用例 |
| `crates/rurge-dns/src/upstream/mod.rs` | 1 | `tcp://` 只写主机与端口 |
| `crates/rurge-config/src/spec/wireguard.rs` | 1 | `tos` 不再是不适用的参数 |
| `crates/rurge-config/src/config.rs` | 1 | `W0013` 看进逻辑规则；`subnet` 组的 `W0008` 文本 |
| `crates/rurge-config/src/glob.rs`、`crates/rurge-rules/src/set_format.rs` | 1 | 手册的三个通配例子进用例；内置集的注释改称新基线 |
| `crates/rurge-config/src/rule.rs` | 2、3 | `GeoIpUnknown` / `IpAsnUnknown`；成对括号的用例 |
| `crates/rurge-rules/src/{matcher.rs,geoip.rs,set.rs,engine.rs}`、`benches/rules.rs` | 2 | `GeoResult`、两种 `UNKNOWN` 的匹配、集合里的 `UNKNOWN` 行 |
| `crates/rurge-policy/src/{registry.rs,lib.rs}`、`crates/rurge-engine/src/engine.rs` | 3 | 随机数公开；`use-local-host-item-for-proxy` 随机取一个地址 |
| `crates/rurge-engine/src/udp.rs` | 3 | `block-quic = all-proxy` 的那一格 |
| `crates/rurge-policy/src/{assemble.rs,cell.rs}`、`crates/rurge-rules/src/matcher.rs`、`crates/rurge-proto-wireguard/src/{stack.rs,testing/mod.rs}` | 3 | 钉住现有行为的用例与它们要的测试支撑 |
| `crates/rurge-dns/src/{resolver.rs,lib.rs}`、`benches/dns.rs`、`tests/resolver.rs` | 4 | `DnsConnectors`、`ResolverDeps.connectors` |
| `crates/rurge-engine/src/{dns_pipeline.rs,stack.rs,runtime.rs}` | 4 | 五个带标签的面、`StackOptions.dns_connector` |
| `crates/rurge-engine/tests/{pipeline.rs,outbounds.rs,outbounds_shadowsocks.rs,outbounds_wireguard.rs,common/mod.rs}`、`crates/rurge/tests/cli.rs` | 1 ～ 4 | 经引擎与命令行的用例 |
| 文档 | 5 | 兼容性清单、需求文档、两份验收文档、两份手工清单、两份 API 参考、README（中英）、CLAUDE.md、设计与本计划的两张表 |

---

## Task 1：配置层的六处小改动与两处小补充

**文件**：`crates/rurge-config/src/{text/mod.rs,deferred.rs,policy.rs,config.rs,redact.rs,glob.rs}`、`crates/rurge-config/src/spec/{ss.rs,wireguard.rs}`、`crates/rurge-dns/src/{upstream/mod.rs,hosts.rs}`、`crates/rurge-rules/src/set_format.rs`；用例在这些文件自己的单元测试、`crates/rurge-config/tests/secrets.rs`、`crates/rurge-policy/src/assemble.rs`、`crates/rurge-dns/tests/resolver.rs`、`crates/rurge-engine/tests/{outbounds_shadowsocks.rs,outbounds_wireguard.rs,common/mod.rs}`、`crates/rurge/tests/cli.rs`。七个提交，按下面的顺序，各自先红后绿。

**接口**：

```rust
// rurge-config::policy
// POLICY_KEYWORDS 里 ("custom", PolicyKind::Shadowsocks) 排在 ("ss", …) 之后：
// PolicyKind::parse("custom") 是 Shadowsocks，PolicyKind::Shadowsocks.keyword() 仍是 "ss"
pub struct ProxyPolicy { /* 原有的 */, pub legacy_custom: bool }   // 类型关键字写的是 `custom`

// rurge-config::spec::wireguard
pub const NOT_APPLICABLE: [&str; 3] = ["interface", "allow-other-interface", "tfo"];
```

`read_ss` 的签名不变：它从 `r.policy().legacy_custom` 得知这一行是不是旧写法。

**`[IP Rewrite]`**（设计 4.2）：`ORDERED_SECTIONS`（`text/mod.rs`）与 `DEFERRED`（`deferred.rs`）各加 `"IP Rewrite"`。行原样留着，动作与地址不校验。

- 用例 `ip_rewrite_is_a_known_section_kept_for_later`（`config.rs` 的单元测试）：Profile 带 `[IP Rewrite]` 与三行（`10.0.0.1 = reject`、`10.0.0.0/8 = drop`、`fd00::/8 = reflect`）——`W0016` 的那一条列着 `[IP Rewrite]`，没有 `W0002`，`cfg.deferred.get("IP Rewrite")` 的三行按书写顺序在；节名大小写不同（`[ip rewrite]`）同样。

**旧的 `custom` 类型**（设计 4.3）：

- `policy.rs`：关键字表加一项（见接口）；`parse_policy` 照 `PolicyKind::parse` 认关键字的方式（去空白、不分大小写）置 `legacy_custom`。`with_params` 与脱敏的 `takes_server_type` 都经 `PolicyKind::parse`，随之把 `custom` 当成带服务器与端口的类型。
- `spec/ss.rs`：`read_ss` 在旧写法的行上读 `r.positional(0)`（加密方法）、`r.positional(1)`（口令）与 `r.positional(2)`（模块的链接，读过即丢）；命名的 `encrypt-method=` / `password=` 胜过位置上的（写法同 `read_credentials`）。不是旧写法的 `ss` 行照旧不读位置字段（现有用例 `a_missing_password_is_an_error_and_a_positional_one_is_not_read` 不变）。再多的位置字段照旧由 `ParamReader::finish` 报成多余的位置值。
- `config.rs` 的策略循环：`p.legacy_custom` 的每一行各一条 `Diagnostic::info(codes::I_LEGACY_MIGRATED, …).at(p.span.clone())`，文本 ``format!("{}: legacy type `custom` read as `ss`; the module URL is ignored", named("policy", &p.name))``——这一行有没有别的错误都给（它正好解释了那条 `encrypt-method` / `password` 的错误是怎么来的）。订阅导入的行不经这个循环，不说话。
- `redact.rs`：`redact_positional_credentials` 上面的注释里"Surge's old `custom, …` is an unknown type to rurge"一句改成说今天的事（Loon 的 `Shadowsocks` 行仍是不认识的类型；旧的 `custom` 行是 `ss`）。两处拿 `custom` 当"不认识的类型"的用例行各换成一个仍然不认识的类型（Loon 的 `Shadowsocks`），保住"第三个字段是端口"那条路；`custom` 自己各留一行，结果与原来相同。
- `tests/secrets.rs`：`a_line_that_does_not_load_shows_no_secret_either` 里的 `Old = custom, …` 现在加载得了，换成一个加载不了的类型；带标记的那份配置加一行能加载的 `custom`，口令在位置上、带标记。

用例：

- `the_custom_keyword_is_the_ss_type`（`policy.rs`）：`PolicyKind::parse("custom")` 与 `"CUSTOM"` 都是 `Shadowsocks`；`PolicyKind::Shadowsocks.keyword()` 是 `ss`；`parse_policy` 对 `custom, 127.0.0.1, 8388, m, p` 置 `legacy_custom`，对 `ss, …` 不置。
- `a_legacy_custom_line_is_read_as_shadowsocks`（`spec/ss.rs`）：`custom, 127.0.0.1, 8388, chacha20-ietf-poly1305, pw, https://m.test/SSEncrypt.module` 读出的方法、口令、`udp_relay`、`obfs` 与 `ss, 127.0.0.1, 8388, encrypt-method=chacha20-ietf-poly1305, password=pw` 相同，没有任何诊断（模块链接不是"多余的位置值"）；不带模块链接的同样；第四个位置字段是一条 `W0001`；命名的 `encrypt-method=aes-128-gcm` 胜过位置上的。
- `a_legacy_custom_line_keeps_its_named_parameters`（`spec/ss.rs`，评审要点 2）：`custom, 127.0.0.1, 8388, aes-256-gcm, pw, https://m.test/x.module, obfs=http, obfs-host=h.test, udp-relay=true, ota=true`——`obfs` 是 `http`、`udp_relay` 为真，诊断恰好一条 `W0001`（`ota`），没有错误。
- `a_legacy_custom_password_with_an_equals_sign_is_an_error`（`spec/ss.rs`，评审要点 2）：`custom, 127.0.0.1, 8388, aes-256-gcm, hunter=2secret`——有一条 `E0018`（`password` 必填），诊断的文本里找不到 `2secret`。
- `an_imported_custom_line_is_read_as_ss`（`crates/rurge-policy/src/assemble.rs` 的单元测试，评审要点 3）：`policy-path` 的列表里一行 `Old = custom, 127.0.0.1, 8388, aes-128-gcm, pw`——组的成员里有 `Old`，它的 spec 是 `ProtoSpec::Ss`，装配的诊断里没有关于这一行的任何一条。
- `with_params` 现有的表（`policy.rs`）加一行 `custom`：类型、服务器、端口三个字段不取键，追加的参数写在行尾。
- `a_legacy_custom_policy_connects_like_ss`（`crates/rurge-engine/tests/outbounds_shadowsocks.rs`）：回环的假 Shadowsocks 服务端，Profile 里一行 `custom`——经它回显成功（照这个文件里第一条 TCP 用例的写法）。
- `check_reads_a_legacy_custom_policy_as_ss`（`crates/rurge/tests/cli.rs`）：`rurge check` 对带一行 `custom` 的 Profile 以 0 退出，输出里有 `I0001` 与 `` legacy type `custom` read as `ss` ``，没有 `E0004`。
- 标记用例与脱敏的用例如上。

**`tcp://` 只写主机与端口**（设计 4.4）：`UpstreamSpec::parse` 的 `tcp://` 一支在拒绝用户名口令之后加与 `quic://` 相同的检查——去掉结尾的一个 `/` 之后还含 `/`、`?`、`#` 的是 ``format!("`{s}`: a tcp:// URL names a host and a port, nothing else")``。`tls://` 不动。

- 用例 `a_tcp_url_is_a_host_and_a_port`（`upstream/mod.rs`）：`tcp://127.0.0.1:5353/x`、`tcp://127.0.0.1?x`、`tcp://127.0.0.1#x`、`tcp://127.0.0.1/a/` 是那一句错误；`tcp://127.0.0.1:5353/`、`tcp://127.0.0.1`、`tcp://[::1]:53/` 照旧解析成主机与端口；`tls://127.0.0.1/x` 照旧接受；`tcp://u:p@127.0.0.1/x` 得到的仍是拒绝用户名口令的那一句，文本里没有 `u:p`。
- 用例 `a_tcp_url_with_a_path_is_skipped_and_the_profile_still_loads`（`crates/rurge-dns/tests/resolver.rs`，评审要点 5）：`encrypted-dns-server` 写两项，`tcp://<MockDns 的地址>/x` 与 `tcp://<MockDns 的地址>`——诊断里恰好一条 `W0026`、是那一句；对 MockDns 认识的名字的查找成功，MockDns 记下的问题都经 TCP 到达。
- `[Host]` 的 `server:tcp://127.0.0.1/x`（`crates/rurge-dns/src/hosts.rs` 现有的 `W0026` 用例旁加一行）同样是 `W0026`。

**`wireguard` 行上的 `tos`**（设计 4.5）：`NOT_APPLICABLE` 去掉 `tos`，`common.tos = 0;` 一句去掉。隧道的 socket 是起这条隧道的那个策略的连接器开的，握手之后 `set_tos(0)` 回到的就是它的 `tos`——这两处已有，不用改。载体键（`crates/rurge-engine/src/outbounds.rs` 里 `with_carrier` 的那一串，决定哪几个策略共用一条隧道）**不加** `tos`：加了的话，同一个节上 `tos` 不同的两个策略每拨一次就顶掉对方的隧道。

- 现有用例 `socket_parameters_do_not_apply`（`spec/wireguard.rs`）：`tos` 从被告警的参数里去掉，其余三个照旧。
- 用例 `the_tos_of_a_wireguard_line_is_kept`（`spec/wireguard.rs`）：`wireguard, section-name=wg, tos=0x28` 没有 `W0028`，`spec.common.tos` 是 `0x28`；`interface`、`allow-other-interface`、`tfo` 照旧各一条 `W0028`。
- `crates/rurge-engine/tests/common/mod.rs`：加一个记录 `(Family, u8)` 的 socket 钩子与一个吃钩子的夹具变体（照 `harness_edited` 的写法，多设 `stack.socket_hook`；钩子照 `tests/pipeline.rs` 里 `TosHook` 的写法）。
- 用例 `a_wireguard_tunnel_marks_its_datagrams_with_the_policys_tos`（`outbounds_wireguard.rs`）：`tos=0x28` 的 `wireguard` 策略经回环的假对端回显一次——钩子依次见到 `(V4, 0x28)`（建 socket 时）、`(V4, 0x88)`（握手）、`(V4, 0x28)`（握手发完）。
- 用例 `two_wireguard_policies_of_one_section_share_the_tunnel_whatever_their_tos`（`outbounds_wireguard.rs`，评审要点 4）：两个策略指向同一个节，一个 `tos=0x28`、一个不写；经第一个开一条连接并留着，经第二个再开一条并回显，然后在第一条上再回显一次——都通（第二个策略若另起隧道，第一条连接会被重置）。

**`W0013` 看进逻辑规则**（设计 4.6）：`config.rs` 里报 `W0013` 的那个循环对每条顶层规则除了它自己的类型，还走进 `RuleKind::And` / `Or` / `Not` 的子规则（任意层），对每个子规则的 `type_name()` 做同一个判断；报在这条顶层规则的位置上，每种类型仍只报一次，文本不变。`[Ruleset]` 与规则集文件里的行不看。

- 用例 `rule_types_that_never_match_are_reported_inside_logical_rules`（`config.rs`）：`AND,((PROCESS-NAME,x),(DEST-PORT,443)),P` 得一条说 `PROCESS-NAME` 的 `W0013`，位置是这条规则；`NOT,((SCRIPT,s)),P` 得一条说 `SCRIPT` 的；嵌两层（`OR,((AND,((PROCESS-NAME,x),(DEST-PORT,1))),(DOMAIN,a.test)),P`）同样；顶层已经报过的类型在后面的子规则里再出现不重复报，反过来（先在子规则里、后在顶层）也只一条；能匹配的类型（`DEST-PORT`、`DOMAIN`）不报。

**`subnet` 组的 `W0008`**（设计 4.11）：`GroupKind` 是 `subnet` 时文本是 ``policy group type `subnet` is not implemented in this version; its `default` policy is used on every network``；别的类型（今天没有）照旧用原来的那一句。

- 用例 `the_subnet_warning_says_the_default_is_used`（`config.rs`）：带一个 `subnet` 组的 Profile 的 `W0008` 是新的那一句，里面没有 `first member`。语料库快照里有这句话的随之更新（`INSTA_UPDATE` 或手工改 `.snap`，改动逐行看过）。

**两处小补充**（一个提交）：`wildcard_semantics`（`glob.rs`）加手册的三个例子——`*google.com` 匹配 `google.com` 与 `foo.google.com`，`*.google.com` 不匹配 `google.com`（这三条今天就该是绿的）；`SYSTEM_SET` 上面的注释（`set_format.rs`）的版本改成 Surge Mac 6.10.0 / iOS 5.23.0（清单与手册今天的清单逐行比过，报告 R47）。

**判据**：上面点名的用例全绿；任务末尾的门禁全绿；`git grep -n "first member is used" -- crates` 只剩"别的类型"的那一句；标记用例找不到标记。

---

## Task 2：`GEOIP,UNKNOWN` 与 `IP-ASN,UNKNOWN`

**文件**：`crates/rurge-config/src/rule.rs`、`crates/rurge-rules/src/{matcher.rs,geoip.rs,set.rs,engine.rs,lib.rs}`、`crates/rurge-rules/benches/rules.rs`；用例在这些文件的单元测试与 `crates/rurge/tests/cli.rs`。一个提交。

**接口**（设计 4.1）：

```rust
// rurge-config::rule
pub enum RuleKind { /* 原有的 */, GeoIpUnknown, IpAsnUnknown }
// type_name()：GEOIP / IP-ASN；is_ip_based() 为真；supports_pre_matching() 同 GEOIP / IP-ASN

// rurge-rules::matcher（GeoResult 与 GeoLookup 一起从 crate 根公开）
/// What a database says of an address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeoResult<T> {
    /// The database is not loaded: nothing that depends on it matches.
    NotLoaded,
    /// The database is loaded and has no result for the address — what
    /// `GEOIP,UNKNOWN` and `IP-ASN,UNKNOWN` match.
    Unknown,
    Known(T),
}

pub trait GeoLookup: Send + Sync {
    fn country(&self, ip: IpAddr) -> GeoResult<[u8; 2]>;
    fn asn(&self, ip: IpAddr) -> GeoResult<u32>;
}
```

**解析**：`GEOIP` 的取值（去空白后）不分 ASCII 大小写等于 `UNKNOWN` 时是 `GeoIpUnknown`；`IP-ASN` 的取值整个等于 `UNKNOWN` 时是 `IpAsnUnknown`——这个判断在去掉 `AS` 前缀之前，`IP-ASN,ASUNKNOWN` 照旧是 `E0011`。`GEOIP,USA` 照旧是 `GeoIp("USA")`（能加载、永不匹配，登记在清单那一行）。`RuleKind` 上每一个带通配臂的 `match` 都看一遍，新的两个变体不许掉进别人的通配臂里（今天点名 `GeoIp` / `IpAsn` 的地方：`rule.rs` 的 `type_name`、`is_ip_based`、`supports_pre_matching` 与解析；`matcher.rs` 的编译与求值；`set.rs` 的集合编译）。

**匹配**：`Matcher` 多两个变体；求值与 `GeoIp` / `IpAsn` 同一条路取目标地址（`target_ip(s, ctx, no_resolve, Family::Any)`，"要先解析""跳过"照旧），然后只在 `ctx.country(ip)` / `ctx.asn(ip)` 是 `GeoResult::Unknown` 时命中。`Matcher::GeoIp` / `IpAsn` 只在 `Known(同一个值)` 时命中。`EvalCtx` 的两个缓存存 `GeoResult`。

**`GeoDb`**：句柄没有库时照旧 `warn_missing` 并答 `NotLoaded`；有库而查询出错、没有记录、记录里没有国家码、国家码不是两个字母、没有 AS 号，都答 `Unknown`。"有没有库"与结果出自同一次 `load()` 拿到的同一个句柄。`describe` 与 `GET /v1/geoip` 的输出不变。`NoGeo` 两个方法都答 `NotLoaded`；测试与基准里别的实现照各自的意思改：扮一份载入了的库的，查不到时答 `Unknown`。

**集合**（`set.rs`）：两种 `UNKNOWN` 行进逐条求值的那一列（`linear`），不进 AS 号的表；没写 `no-resolve` 时置 `needs_dns`（把它们加进那个 `matches!` 的清单）。AS 号的表查找只认 `Known`。

**用例**：

- `unknown_is_a_value_of_geoip_and_ip_asn`（`rule.rs`）：`GEOIP,UNKNOWN,P`、`GEOIP,unknown,P`、`GEOIP,Unknown,P,no-resolve` 是 `GeoIpUnknown`；`IP-ASN,UNKNOWN,P`、`IP-ASN,unknown,P` 是 `IpAsnUnknown`；`IP-ASN,ASUNKNOWN,P` 是 `E0011`；两种的 `type_name()`、`is_ip_based()`、`supports_pre_matching()` 与 `GEOIP,CN` / `IP-ASN,1` 相同；`AND,((GEOIP,UNKNOWN),(DEST-PORT,443)),P` 解析得了；`GEOIP,USA,P` 仍是 `GeoIp("USA")`。
- `a_loaded_database_tells_unknown_from_not_loaded`（`geoip.rs`，用 `tests/fixtures/` 的两个测试库）：载入之后 `country(127.0.0.1)` 是 `Unknown`、库里有的地址是 `Known`；`asn` 同理；没有载入时两者都是 `NotLoaded`。
- `geoip_unknown_matches_an_address_the_database_does_not_know`（`matcher.rs`）：库载入时，目标 `127.0.0.1` 命中 `GEOIP,UNKNOWN` 与 `IP-ASN,UNKNOWN`，库里有的地址两者都不命中；目标是域名、没写 `no-resolve` 时先要解析，解析到库里没有的地址后命中；写了 `no-resolve` 时对域名目标不命中。
- `unknown_does_not_match_while_the_database_is_not_loaded`（`matcher.rs`）：`NoGeo` 之下两种都不命中，`GEOIP,CN` 与 `IP-ASN,1` 也不命中（MB-D5）。
- `unknown_lines_in_a_set_are_evaluated_in_order`（`set.rs`）：规则集文本里有 `IP-ASN,64500`、`IP-ASN,UNKNOWN` 与 `GEOIP,UNKNOWN,no-resolve`——没有被跳过的行；`UNKNOWN` 的两行不在 AS 号的表里；集合因 `IP-ASN,UNKNOWN` 没写 `no-resolve` 而要先解析；库里没有的地址命中时，报成子规则的是文件里靠前的那一行。
- `unknown_starts_matching_once_the_database_is_loaded`（`engine.rs`，评审要点 1）：规则 `GEOIP,UNKNOWN,A` 与 `FINAL,B`，`GeoDb` 建在一个空的临时目录上——目标 `127.0.0.1` 得 B；把测试库复制进那个目录并 `load`，同一个 `RuleEngine` 对同一个目标得 A。
- `check_accepts_ip_asn_unknown`（`crates/rurge/tests/cli.rs`）：`rurge check` 对含 `IP-ASN,unknown,DIRECT` 与 `GEOIP,UNKNOWN,DIRECT` 的 Profile 以 0 退出，输出里没有 `E0011`。

**判据**：上面的用例全绿；现有的 GeoIP / ASN 用例与两个基准（`cargo bench --no-run`）照旧编译、通过；门禁全绿。

---

## Task 3：运行期的两处与七条钉住现有行为的用例

**文件**：`crates/rurge-policy/src/{registry.rs,lib.rs,assemble.rs,cell.rs,testing.rs}`、`crates/rurge-engine/src/{engine.rs,udp.rs}`、`crates/rurge-config/src/rule.rs`、`crates/rurge-rules/src/matcher.rs`、`crates/rurge-proto-wireguard/src/{stack.rs,testing/mod.rs}`、`crates/rurge-engine/tests/{outbounds.rs,pipeline.rs}`。产品代码只改两处，各一个提交；七条用例一个提交（连同它们要的测试支撑）。

**接口**：

```rust
// rurge-policy（从 registry 模块公开；load-balance 与引擎共用这一个取法）
/// A number below `n`, drawn the way `load-balance` draws.
pub fn random_below(n: usize) -> usize;
```

**随机取一个地址**（设计 4.7）：`Engine` 里 `use-local-host-item-for-proxy` 的那一支，`HostAction::Ips(ips)` 从 `ips.first().copied()` 改成 `ips.get(rurge_policy::random_below(ips.len())).copied()`。别的不动（不看 `ip-version`；别名、`server:`、`script:` 条目不受影响）。

- 用例 `a_local_host_item_with_several_addresses_gives_the_proxy_one_at_random`（`tests/outbounds.rs`，照 `a_local_host_item_reaches_the_proxy_as_an_ip` 的写法）：`[Host]` 把一个名字映射到 `127.0.0.2, 127.0.0.3, 127.0.0.4`；假 SOCKS5 上游的 `connect_to` 指到用例自己的回环源站（上游从不去拨那三个地址）；发 40 个会话——上游记下的目标都是 IPv4 地址类型、都在那三个之内、不止一种。提交前单独循环跑二十次以上。

**`block-quic = all-proxy`**（设计 4.10）：`quic_blocked` 的 `AllProxy` 一支改成：经代理的阻断；直连类的只在策略自己是 `on` 时阻断。签名不变。

- 现有的表用例 `which_quic_flows_are_blocked`（`udp.rs`）：`(all-proxy, on, 直连)` 从放行变成阻断，另加 `(all-proxy, auto, 直连)` 与 `(all-proxy, off, 直连)` 两行（都放行）；其余各行不变。

**七条钉住现有行为的用例**（设计 4.9；第八条在任务 4）。每条写出来应该是绿的；绿了之后各做一次"改一行产品代码看它变红、再还原"的验证，找不到一处只影响这条用例又不离开回环的改动时，写明没有做、为什么。

| 用例 | 在哪里 | 断言 |
| --- | --- | --- |
| `a_policy_name_with_balanced_parentheses_is_read_whole` | `crates/rurge-config/src/rule.rs` | `AND,((DOMAIN,a.example),(DEST-PORT,443)),Proxy (US)` 的策略名是 `Proxy (US)`，子规则两条；`OR`、`NOT` 各一行同样 |
| `an_ipv6_literal_is_reached_directly_with_ipv6_off` | `crates/rurge-engine/tests/pipeline.rs`（夹具的 Profile 本来就是 `ipv6 = false`） | 源站听在 `[::1]:0`；经 HTTP 监听的 `CONNECT [::1]:<端口>` 走 DIRECT 到达并回显。机器没有 IPv6 回环时跳过（`if std::net::TcpListener::bind("[::1]:0").is_err() { return; }`） |
| `the_groups_relay_wins_over_one_set_by_the_modifier` | `crates/rurge-policy/src/assemble.rs` | 组有 `policy-path` 的列表、`external-policy-modifier="underlying-proxy=A"` 与组自己的 `underlying-proxy=B`——导入的成员派生成 `M (via B)`，它的 `underlying_proxy` 是 `B` |
| `members_from_every_source_are_chained_through_the_groups_relay` | 同上 | 组写了 `include-all-proxies=true`、`include-other-group=G2`、`underlying-proxy=R`，并用 `policy-regex-filter` 把 `R` 自己滤在外面——两种来源带进来的代理成员都派生成 `M (via R)` |
| `url_regex_without_an_anchor_matches_anywhere_in_the_url` | `crates/rurge-rules/src/matcher.rs` | 模式 `/ads/` 匹配 `http://h.test/x/ads/y`；`^http://h\.test/ads/` 不匹配同一个 URL |
| `only_echo_requests_to_the_tunnels_ipv6_address_are_answered` | `crates/rurge-proto-wireguard/src/stack.rs`，支撑在 `testing/mod.rs` | 节带 `self-ip-v6`；发给隧道自己 IPv6 地址的 ICMPv6 回显请求得到应答，发给同一前缀里别的地址的没有。测试支撑要加：建 ICMPv6 请求的 `ping6`、`pong` 认 IPv6 的回显应答、带 `self_ip_v6` 与一段 IPv6 允许范围的节 |
| `a_udp_target_is_handed_over_unchanged` | `crates/rurge-policy/src/cell.rs`（`the_target_is_handed_over_unchanged_also_through_a_group` 旁） | `ChainConnector::connect_udp` 对主机名目标：底下的载体被问到的是那个主机名与端口，没有先在本机解析 |

**判据**：上面的用例全绿；七条钉住的用例各有一次变红的验证（或写明没有做的理由）；门禁全绿。哪一条钉住的用例写出来是红的：停下来报告，不改产品代码。

---

## Task 4：DNS 会话的协议标签按方案，与第八条用例

**文件**：`crates/rurge-dns/src/{resolver.rs,lib.rs}`、`crates/rurge-dns/benches/dns.rs`、`crates/rurge-dns/tests/resolver.rs`、`crates/rurge-engine/src/{dns_pipeline.rs,stack.rs,runtime.rs}`、`crates/rurge-engine/tests/pipeline.rs`。一个提交（接口与标签），第八条用例另一个提交。

**接口**（设计 4.8）：

```rust
// rurge-dns
/// Who dials for the upstreams that go through a connector, one per kind of
/// upstream: whoever stands behind them knows what a connection carries.
#[derive(Clone)]
pub struct DnsConnectors {
    pub tcp: Arc<dyn Connector>,
    pub tls: Arc<dyn Connector>,
    pub https: Arc<dyn Connector>,
    pub quic: Arc<dyn Connector>,
    pub h3: Arc<dyn Connector>,
}

impl DnsConnectors {
    /// The same connector for every kind.
    pub fn all(connector: Arc<dyn Connector>) -> DnsConnectors;
}

pub struct ResolverDeps {
    pub connectors: DnsConnectors,   // 原来是 connector: Arc<dyn Connector>
    /* 其余不变 */
}

// rurge-engine::dns_pipeline
impl PipelineConnector {
    /// Five faces of this pipeline, each labelling what it dials.
    pub fn connectors(self: &Arc<Self>) -> DnsConnectors;
}

// rurge-engine::stack
pub struct StackOptions { /* 原有的 */, pub dns_connector: Option<DnsConnectors> }
```

**解析器**：`Resolver` 握着的从一个引导连接器变成五个（每个是 `BootstrapConnector::new(deps.connectors.<种类>, bootstrap.clone())`，共用一个 `Bootstrap`）；`build_upstream` 里 `tcp://` 用 `tcp`、`tls://` 用 `tls`、`quic://` 用 `quic`、`h3://` 用 `h3`，DoH 的内部 HTTP 客户端建在 `https` 上。经策略网卡的普通查询（`via_set`）不动。字面量 `ResolverDeps { connector: … }` 的五处（`benches/dns.rs`、`resolver.rs` 两处、`tests/resolver.rs`、`crates/rurge-engine/src/stack.rs`）改成 `connectors: DnsConnectors::all(…)`。

**引擎**：`PipelineConnector` 仍是握着引擎与回落连接器的那一个对象；它不再自己猜标签——`protocol_for` 与 `quic_protocol_for` 连同它们的两条用例去掉，建内部会话的两段逻辑改成吃一个 `ProtocolKind`。`connectors()` 给出五个薄连接器，各带固定的标签：`tcp` 是 `Dns`、`tls` 是 `Dot`、`https` 是 `Doh`、`quic` 是 `Doq`、`h3` 是 `Doh3`；`PipelineConnector` 今天实现的每一个 `Connector` 方法，每个面照样实现（目标是域名、或没有引擎时照旧交给回落连接器）。`stack.rs` 里没给 `dns_connector` 时是 `DnsConnectors::all(connector)`；`runtime.rs` 里是 `Some(pc.connectors())`。`ConnectOpts` 不加字段。写 `dns_connector: None` 的那些字面量只有类型变了，不用改。

**用例**：

- `each_kind_of_upstream_dials_through_its_own_connector`（`crates/rurge-dns/src/resolver.rs`）：五个只记录、一律拒绝的连接器；对五种上游各建一个解析器（`tcp://127.0.0.1:1`、`tls://127.0.0.1:2`、`https://127.0.0.1:3/dns-query`、`quic://127.0.0.1:4`、`h3://127.0.0.1:5/dns-query`——端口只是标识，连接器拒绝，什么也不拨），各查一次（失败）——被问到的只有对应的那一个连接器，问的是那个端口，TCP 的三种经 `connect`、QUIC 的两种经 `connect_udp`。
- `the_five_faces_carry_the_five_labels`（`dns_pipeline.rs`）：`connectors()` 的五个面各带上面的标签；原来两条"没有引擎时交给回落连接器"的用例改经一个面。
- `a_dns_session_is_labelled_by_its_scheme_whatever_the_port`（`tests/pipeline.rs`，取代 `a_dns_connection_is_called_dot_at_853_and_doh_at_443`，照它的写法：服务器只被点名、从不被拨——`PROTOCOL,<标签>,UpIp` 与兜底的 `IP-CIDR,127.0.0.1/32,UpIp,no-resolve` 都把会话交给用例自己的、拒绝一切的回环 `http` 代理，它也不载 UDP）：逐行各建一个引擎——`tcp://127.0.0.1:853` 是 `DNS`；`tls://127.0.0.1:8853` 与 `tls://127.0.0.1`（853）是 `DOT`；`https://127.0.0.1:8443/dns-query` 与 `https://127.0.0.1/dns-query`（443）是 `DOH`；`h3://127.0.0.1:8443/dns-query` 是 `DOH3`；`quic://127.0.0.1:443` 是 `DOQ`；另一行把加密服务器写在 `[Host]` 的 `server:` 里（`tls://127.0.0.1:8853`，`DOT`）。每行断言：有一条内部会话的协议是这个标签、目标是那个地址与端口、命中的规则是 `PROTOCOL,<标签>`。
- 现有用例 `a_doh3_carrier_follows_an_ip_rule_through_a_socks5_proxy`（`tests/pipeline.rs`）：它的服务器不在 443，断言从 `ProtocolKind::Doq` 改成 `Doh3`，上面的注释随之改。
- `a_dns_session_follows_the_outbound_mode`（`tests/pipeline.rs`，设计 4.9 的第八条，应该是绿的）：DNS 服务器是回环上的 `MockDns`（`tcp://<它的地址>`）；三种出站模式各建一个引擎——规则模式下一条 `IP-CIDR,127.0.0.1/32,ByRule,no-resolve` 把 DNS 会话交给一个拒绝的回环代理 `ByRule`，内部会话的策略是 `ByRule`；直连模式下内部会话的策略是 `DIRECT`、查找成功；全局模式（`OutboundMode::Proxy`，另一个拒绝的回环代理 `Glob`）下内部会话的策略是 `Glob`。不用 REJECT 当全局策略（链底是 REJECT 的 DNS 会话会被旁路成直连）。

**判据**：上面的用例全绿；`git grep -n "protocol_for\|quic_protocol_for" -- crates` 没有结果；门禁全绿（基准目标由 clippy 的 `--all-targets` 一并检查）。

---

## Task 5：文档

**文件**：`docs/surge-compatibility-matrix.md`、`docs/requirements.md`、`docs/acceptance/{phase1-acceptance.md,phase2-acceptance.md,phase1-manual.md,phase2-manual.md}`、`docs/api/{phase1.md,phase2.md}`、`README.md`、`README_en.md`、`CLAUDE.md`、设计与本计划。不改代码。两个提交：清单一个，其余一个。

**兼容性清单**（设计第 7 节第一条）：

- 报告第 5 节的 R1 ～ R123 逐项落到清单上。报告里的行号是提交 `30b082a` 上的，按第一格的文字找行；每一项照报告"改动"一格写的做，整格替换的文字与新增的十行在各小节的表后面。改完一项核对一项：那一行的标记、阶段两格与报告说的一致，表格的列数没有变。
- 报告里写"引用这一轮的用例"的地方，用例名取自下表（写进清单之前各用 `git grep` 核对一次）：

| 清单的行 | 用例 |
| --- | --- |
| `use-local-host-item-for-proxy`（2.1 节与 6.3 节各一行） | `a_local_host_item_with_several_addresses_gives_the_proxy_one_at_random` |
| `ipv6` | `an_ipv6_literal_is_reached_directly_with_ipv6_off` |
| `block-quic` | `which_quic_flows_are_blocked` |
| `GEOIP` | `geoip_unknown_matches_an_address_the_database_does_not_know`、`unknown_does_not_match_while_the_database_is_not_loaded`、`unknown_lines_in_a_set_are_evaluated_in_order` |
| `IP-ASN` | 同上三条，与 `check_accepts_ip_asn_unknown` |
| `URL-REGEX` | `url_regex_without_an_anchor_matches_anywhere_in_the_url` |
| `AND` / `OR` / `NOT` | `rule_types_that_never_match_are_reported_inside_logical_rules`、`a_policy_name_with_balanced_parentheses_is_read_whole` |
| `wireguard`（经 `underlying-proxy` 时 endpoint 的解析） | `a_udp_target_is_handed_over_unchanged` |
| `tos` | `the_tos_of_a_wireguard_line_is_kept`、`a_wireguard_tunnel_marks_its_datagrams_with_the_policys_tos` |
| WireGuard 生命周期（ICMPv6） | `only_echo_requests_to_the_tunnels_ipv6_address_are_answered` |
| `custom`（新行） | `a_legacy_custom_line_is_read_as_shadowsocks`、`a_legacy_custom_policy_connects_like_ss`、`a_legacy_custom_password_with_an_equals_sign_is_an_error` |
| 组的 `underlying-proxy` | `members_from_every_source_are_chained_through_the_groups_relay`、`the_groups_relay_wins_over_one_set_by_the_modifier` |
| `subnet` 组 | `the_subnet_warning_says_the_default_is_used` |
| `https://` `tls://` `tcp://` | `a_tcp_url_is_a_host_and_a_port` |
| 跟随出站模式（6.2 节） | `a_dns_session_is_labelled_by_its_scheme_whatever_the_port`、`a_dns_session_follows_the_outbound_mode` |
| `[IP Rewrite]`（新行） | `ip_rewrite_is_a_known_section_kept_for_later` |

- `tos` 那一行另写一句任务 1 定下的事：指向同一个节的几个 `wireguard` 策略共用一条隧道，隧道的标记是起它的那个策略的 `tos`；重载只改了 `tos` 时，旧隧道还被连接拿着就继续用旧的标记（用例 `two_wireguard_policies_of_one_section_share_the_tunnel_whatever_their_tos` 钉住的是"谁也不顶掉谁"）。线上的 DSCP 位没有抓包看过（手工清单）。
- 表头：基线一句改成 Surge Mac 6.10.0 / iOS 5.23.0，"最近一次"改成这一轮；"三个数"一段随阶段 1 验收文档改（见下）。写着"比手册基线新"的几行去掉这句话（报告已逐行给出）。
- 附录整张重数：数的办法照附录自己写的口径（每个表格行按它的状态标记计一次），用脚本数，不用手数。预期 565 行（✅ 398、🟡 118、🔁 30、⛔ 14、❓ 5）；与预期不同时以脚本数出来的为准，并说明差在哪几行。

**需求文档**：参考基线一行；FR-CFG-06 的"57 个选项"改成 58；第 7 节的状态行加这一轮。阶段栏不动。

**阶段 1 验收文档**：FR-RULE-01 的 `GEOIP,UNKNOWN` / `IP-ASN,UNKNOWN` 那一句从"没有做完"改成"补上了"，指到任务 2 的代码与用例；`## 数字` 一节改成 补上了 10 / 没有做完 2（其余不变，合计仍是 416）；`## 没有做完的` 去掉这一条。"三个数"从此是 **2 句没有做完、37 句阶段栏与登记不一致、14 句有意不同**——引用它的每一处一起改：清单表头、`docs/acceptance/phase2-acceptance.md` 的那一小节、CLAUDE.md、README（中英）。

**两份手工清单**：`docs/acceptance/phase2-manual.md` 加一节「手册基线重对」两项——`wireguard` 的 `tos`（抓包看隧道的 UDP 包的 DSCP：握手的发起包是 `0x88`，其余是策略的 `tos`；Windows 上系统是否照办另记）、一行真实的旧 `custom` 策略对真实的 Shadowsocks 服务端。`docs/acceptance/phase1-manual.md` 的「与平台无关」加一行——真实的 GeoLite2 库之下 `GEOIP,UNKNOWN` 与 `IP-ASN,UNKNOWN`（一个保留地址命中，一个公网地址不命中）。两份清单的项数随之是 22 节 187 项与 46 行；引用旧数字（21 节 185 项、其余 165 项、45 行、与平台无关 5）的每一处一起改，历史记录（各份计划、写明日期的更新说明）不改。

**API 参考**：`docs/api/phase2.md` 的「跟随出站模式下 DNS 的内部会话」一节——标签按 URL 的方案（去掉"按端口"的说法，写明 `[Host]` 的 `server:` 同样）；`docs/api/phase1.md` 两处——「外部资源」一节里说这两个端点晚于手册基线的话去掉，`GET /v1/profiles/current` 的 `text/plain` 旁加一句"手册说应答总是 JSON，rurge 的形状随阶段 6 对齐"（MB-D21）。

**README（中英）**：状态一段——手册基线重对做完了、基线是哪一版、下一步是什么；两份保持一致。

**CLAUDE.md**：事实来源一句的基线；状态一段加这一轮（做了什么、三个数的新值、没有覆盖的照设计第 8 节）；文档清单加设计、本计划、报告三份；"接下来的工作"里"对一遍手册的基线"换成"一行怎样拆分的设计"；常用命令加这一轮新的测试过滤词。

**设计与本计划**：设计末尾加一节实施期的订正（与第 4 ～ 7 节不同的做法，没有就写没有）；本计划的两张表填实。

**判据**：每一处文档里的名字（类型、函数、诊断码、测试名、命令）在代码里找得到；表格的列数与表头一致；附录的数字是脚本数出来的；`git grep` 旧基线的说法（"6.9 / iOS 5.22"）只剩历史记录与写着版本的清单行；没有本机路径与流程用语；`cargo test -j 8 --workspace --no-fail-fast` 照旧全绿（文档不该动到它）。

---

## 计划期决定

写计划时对着设计、报告与本仓库源码核对后定下的事；与设计文字不同的，写进设计的实施期订正。

| # | 事项 | 核对到的事实 | 决定 |
| --- | --- | --- | --- |
| P1 | `custom` 行在哪里被认出来 | `PolicyKind::parse` 在关键字表里取第一项相等的，`keyword()` 取第一项种类相等的；`ProxyPolicy` 只有 `parse_policy` 一处字面量；`ParamReader::policy()` 交得出策略 | 关键字表加在 `ss` 之后，`ProxyPolicy` 多一个 `legacy_custom`，`read_ss` 经读取器得知，签名不变（`rurge-proto` 的测试也在调它） |
| P2 | `I0001` 由谁给 | `Remark` 是"每种每次加载一条"，订阅导入的行每组一条；`I0001` 在 `codes::hint` 里不设建议 | 主配置的策略循环里每行一条，有没有别的错误都给；导入的行不说 |
| P3 | 位置上的口令带 `=` | 字段里有 `=` 的被 `ParamMap::from_fields` 读成命名参数，`http` 的位置凭据今天就是这样；脱敏照同一种读法 | 不为 `custom` 另开读法：得到 `password` 必填的 `E0018`。未知参数的告警会点出 `=` 之前那一段，同 `http` 行，登记（延后第 9 行） |
| P4 | 脱敏随 `custom` 变什么 | `redact_positional_credentials` 对"带服务器的类型"与"第三个字段是端口的行"都遮第四个字段起的位置字段 | 遮的范围不变，只多遮"端口位置上不是数字的那一项"；注释与两行用例改 |
| P5 | `tcp://` 的告警引用什么 | `quic://` 今天的那一句引用写出来的整项，带用户名口令的在它之前被拒 | 同 `quic://`；`host_port` 不动 |
| P6 | `tos` 进不进 WireGuard 的载体键 | 载体键是 `"<环境> ip-version=… via=…"`；同一私钥同一 peer 只留一条隧道，节与载体键都相同的策略共用，不同的互相顶掉 | 不进：进了的话，同一个节上 `tos` 不同的两个策略每拨一次就顶掉对方的隧道。代价：隧道的标记是起它的那个策略的；重载只改 `tos` 时旧隧道还被拿着就不换。登记在清单 |
| P7 | 经引擎看 `tos` | `tests/common/mod.rs` 的夹具不吃 socket 钩子，`tests/pipeline.rs` 自己的夹具有 `TosHook` | `common` 里加同样的钩子与一个吃钩子的夹具变体 |
| P8 | `UNKNOWN` 与 `AS` 前缀 | `IP-ASN` 先去 `AS` / `as` 再读十进制数 | 整个取值等于 `UNKNOWN` 才是；`ASUNKNOWN` 照旧 `E0011` |
| P9 | `GeoLookup` 的实现者 | 八处：`GeoDb`、`NoGeo`，与测试、基准里的六个 | 都改；扮载入了的库的查不到时答 `Unknown` |
| P10 | 随机数 | `rurge-policy` 的 `random_below`（标准库的随机哈希）是 `load-balance` 的取法，私有 | 公开它，引擎用同一个，不引入第二种随机 |
| P11 | 随机取地址的用例怎样不离开回环 | 假 SOCKS5 的 `connect_to` 让它把每个请求都连到用例给的地址；macOS 上没有 `127.0.0.2` | 上游从不拨那三个地址，只记下被要求的目标 |
| P12 | 解析器里连接器的去处 | `Resolver` 握着一个引导连接器，`build_upstream` 把它交给 TCP / DoT / DoQ / DoH3，DoH 的 HTTP 客户端也建在它上面 | 五个都包引导解析、共用一个 `Bootstrap` |
| P13 | 标签的用例怎样不拨固定端口 | 现有的 `a_dns_connection_is_called_dot_at_853_and_doh_at_443`：两条规则都把会话交给拒绝的回环代理 | 新的表用例照它；QUIC 的行靠"`http` 代理不载 UDP"，同样什么也不拨 |
| P14 | 第七条用例里的中继 | `include-all-proxies` 会把中继自己也带进来，派生出 `R (via R)`（设计第 5 节登记、这一轮不改） | 用 `policy-regex-filter` 把它滤在外面，不把这个怪样钉成要的行为 |
| P15 | 第八条用例的全局策略 | 链底是 REJECT 的 DNS 会话被旁路成直连 | 三种模式各一个引擎，全局策略用一个拒绝的回环代理 |
| P16 | 聚焦地跑用例 | `-p` 会按另一组特性把依赖再编一份，`target/` 涨得很快 | 一律经整个工作区，用 `--lib` / `--test <目标>` 与过滤词 |
| P17 | 手工清单 | `tos` 到 socket 的调用有用例，线上的位没有；`custom` 的字段位置没有对真实的旧配置核对；`UNKNOWN` 只对测试库验过 | 两份清单各加（2 项与 1 行），数字随之改 |

## 执行期修正记录

| # | 任务 | 与计划的出入 | 处理 |
| --- | --- | --- | --- |

## 延后事项

| # | 事项 | 去向 |
| --- | --- | --- |
| 1 | 一行怎样拆分（引号只在一个值的开头开启引用；没有配对的引号与括号是普通字符）：解析器与脱敏一起改 | 它自己的设计，排在阶段 3 的设计之前（设计第 6 节） |
| 2 | 组的中继自己也在成员里时派生出 `R (via R)`：读代码得出，没有用例，这样的成员拨号时怎样没有试过 | 有用户报告再说；清单登记 |
| 3 | 没写 `default` 的 `subnet` 组：加载时不报，运行时按空组告警并兜底 | 阶段 3（那时这个组才真的求值） |
| 4 | `GET /v1/profiles/current` 的 JSON 形状（MB-D21） | 阶段 6 |
| 5 | 跟随出站模式时给 DNS 服务器的主机名写规则：内部会话带的是引导解析之后的地址 | 阶段 3（会话模型） |
| 6 | `CORE_VERSION` 的新编码（MB-D22） | 项目所有者 |
| 7 | 阶段 3 ～ 7 的行上手册多说的细节这一遍没有收全 | 各阶段的设计开始时，把它那几节对应的手册页对着清单重读一遍 |
| 8 | 发行说明里 2026-10-13 之后的测试版条目（自适应的 `block-quic`、`GET /v1/policies?include_external=true`、跨多个包的 QUIC 主机名识别、QUIC v2 / draft-29）：手册还没有写到 | 下一次重对从这里开始 |
| 9 | 旧的 `custom` 行位置上的口令带 `=` 时读不出口令（`E0018`），未知参数的告警会点出 `=` 之前那一段；同 `http` 行的位置凭据 | 随第 1 行的设计 |
| 10 | 指向同一个节的 `wireguard` 策略 `tos` 不同时，隧道的标记是起它的那个策略的；重载只改 `tos` 时旧隧道还被拿着就不换 | 有用户报告再说；清单登记 |
| 11 | `tos` 在线上的 DSCP 位、真实的旧 `custom` 配置、真实的 GeoLite2 库之下的 `UNKNOWN`：都没有在真实环境里看过 | 手工验收 |
| 12 | 新的用例只在一台 Windows 11 x86-64 的开发机上跑过；`an_ipv6_literal_is_reached_directly_with_ipv6_off` 在没有 IPv6 回环的机器上跳过（CI 的三种机器有没有，没有看过） | 第一次 CI 运行时核对 |
