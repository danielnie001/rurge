# 项目状态与交接

这里记每个阶段、里程碑做了什么，没做的与不要当成已经有的，以后的会话必须知道的事，去向写着项目所有者的事。现行的规矩以根目录的 `CLAUDE.md` 为准；细节以各计划末尾的「执行期修正记录」「延后事项」两张表为准。

以后收尾一个里程碑，在它的阶段下另起一节写这四样，细节指到计划的两张表，不写进 `CLAUDE.md`。

## 关于下文

下文是 `CLAUDE.md` 原来的「当前状态（2026-10）」一段，2026-10-26 挪到这里：只加了小标题、按句换行，文字一字未改（挪之前的样子是 `git show 521abd8:CLAUDE.md`）。读的时候：

- 文中的"这个文件""这份文件"指当时的 `CLAUDE.md`；"上面""下面"指本文的前后文。
- 文中说的"下面「常用命令」之后的测试约定""命令一节"今天在 `docs/testing.md`（要点在 `CLAUDE.md` 的「测试约定」）；"「先读这些文档」"今天是 `docs/README.md`。
- 各计划、设计、报告里写"CLAUDE.md 的状态一段""CLAUDE.md M8c 任务 9"一类的，指的就是本文。
- 后写的段落订正前面的（文中写着"以这一段为准"的地方），前面的段落不逐处改。
- 文末附 `CLAUDE.md`「计划中的架构」一节里依赖方向一条的原文：那一条在 `CLAUDE.md` 里改写成了现在的规矩，哪个里程碑加了哪条依赖边的记录留在这里。

## 阶段 1

### 概况与阶段 1 补遗（M5a）

阶段 1 的里程碑 M1 ～ M4b 已合并，代理端到端可用。
2026-10-10 把需求文档里阶段 1、2 的功能需求逐句对着代码读过一遍（两个只读的助手，什么也没运行；缺口在代码里核对过，判为"做了"的没有人复核，所以不证明别处没有缺口），阶段 1 查出的只解析或没有做的条款分两批补上。
第一批是 2026-10-11 的阶段 1 补遗（M5a）（一句一项，M8c 计划「延后事项」第 101 ～ 102 行），括号里是现在的代码所在：`pre-matching` 规则优先（`RuleEngine::run` 先把带 `pre-matching` 的规则不解析地评估一遍，`crates/rurge-rules/src/engine.rs`）、子规则命中日志（`Engine::choose_policy` 记一条 DEBUG 日志并写进请求记录的 `notes`，`crates/rurge-engine/src/engine.rs`）、同一资源不可同时作两种集（加载期 `E0024`，`crates/rurge-config/src/config.rs`；运行期 `W0032`，`crates/rurge-rules/src/registry.rs`）、GeoIP / ASN 数据库日期可查询（`GET /v1/geoip`，`crates/rurge-api/src/routes/geoip.rs`，经 `GeoDb::describe` 与 `Engine::geoip_view`）、`SYSTEM_VERSION` 等四个平台变量取自操作系统（`rurge_platform::sysinfo`，`crates/rurge/src/cli/mod.rs` 的 `environment_from` 把读不到的退回原来的取值；macOS 与 Linux 的读取没有在这两个平台上编译、运行过，第一次 CI 运行时核对）、`rurge run` 的监听覆盖（`--listen-http` / `--listen-socks5`，`Overrides` 与 `apply`，`crates/rurge/src/cli/run.rs`；`external` 策略拉起的程序得不到名字以 `RURGE_` 开头的变量，`is_own_setting`，`crates/rurge-proto/src/external.rs`）、`rurge check` 的建议（`codes::hint`，当时 59 个诊断码里 33 个有缺省建议；阶段 1 补遗（M5b）加了 `W0033`、`W0034`、`I0004`，现在 62 个：33 个有缺省建议，`E0014`、`W0033`、`W0034` 的每一处各带各的，26 个不设；`crates/rurge-config/src/diagnostic.rs`）、请求记录的监听端口 / 远端地址 / DNS 耗时（`listenPort`、`remoteAddr`、`dnsMs`，经任务局部的 `Trail`，`crates/rurge-net/src/trail.rs`；远端地址只在会话自己开了 TCP 连接时有，复用已有连接或隧道的会话与所有 UDP 会话没有）、`State.current_profile`（`rurge run` 启动完成时写下 `-c` 的绝对路径，`crates/rurge/src/cli/run.rs`）、`#!WINDOWS-ONLY` / `#!LINUX-ONLY`（`SIMPLIFIED`，`crates/rurge-config/src/requirement.rs`）；做了而没找到用例的几句（第 103 行）补了用例（FR-CFG-02、FR-OBS-01 / 10，FR-RULE-05 的内存实测，以及只在 Unix 上编译、本机没有编译过的 FR-CFG-16 的 `SIGHUP` 用例）。
FR-RULE-05 另补了两处审计读成"做了"的缺口：带 `no-resolve` 的 IP 行进集合的前缀树（`CompiledSet::compile`，`crates/rurge-rules/src/set.rs`），规则集文件的行尾注释（`crates/rurge-rules/src/set_format.rs`）与集合里的 `IP-ASN` 行按号码查——这是"判为做了的没有人复核"第一次查出只做了一部分的一条。
阶段 2 查出一项没登记的：组参数 `hidden` 被读而没有作用（`GET /v1/policy_groups` 不带它，第 104 行），仍然如此。
清单与证据见 `docs/acceptance/phase2-acceptance.md` 的「需求逐条对照代码」一节；阶段 1 / M5a 的设计与计划见下面「先读这些文档」。
阶段 1 / M5a 收尾时另登记了几件没有改的事：其中两件已由阶段 1 补遗（M5b）做掉——API 连接的收尾（阶段 1 / M5a 计划「延后事项」第 35 行；见下面「API 的连接分步关」）和几条启用的 `FINAL` 的读法（第 36 行；见下面「`FINAL`」）——，第三件是一个测试里的启动有一次没有在期限内结束，原因没有人知道（第 37 行），照旧。

### 阶段 1 补遗（M5b）

第二批是 2026-10-11 ～ 13 的阶段 1 补遗（M5b）（设计 `docs/superpowers/specs/2026-10-11-phase1-m5-addendum-design.md` 第 7 节，对它的订正在第 13 节（计划期）与第 14 节（实施期），其中的已决事项 M5-D1 ～ D25 是在项目所有者"自己决定、继续做"的常设授权下定的，所有者可以改；计划 `docs/superpowers/plans/2026-10-11-phase1-m5b-profile-remote-sources-plan.md`，下面写"记录第 N 行""延后第 N 行"指它末尾的「执行期修正记录」与「延后事项」两张表），做了过去从来没有做的三项，都是需求文档里阶段 1 的 P0：托管配置的更新（FR-CFG-05）、远程 `#!include`（FR-CFG-04 的"远程 URL"）与外部资源的手动强制更新（FR-CFG-14）。
做了什么（括号里是类型或函数与文件）：托管配置（`#!MANAGED-CONFIG <URL> interval= strict=`）在 `rurge run` 运行期间按 `interval` 下载（缺省 86400 秒，不足 60 秒按 60 秒，间隔只往后抖，失败后 60 秒起翻倍重试至 600 秒，`MAX_BACKOFF`，`crates/rurge-engine/src/profile.rs`），链接整个当作凭据：日志、标签、诊断、`Debug` 与 API 里最多显示 `scheme://host[:port]/…`（`rurge_config::remote::origin`）；rurge 不改写用户的文件，所以下载到的是数据目录里的**副本**（`<数据目录>/resources/<链接的 SHA-256>/data`，旁边的 `meta.json` 记校验器与正文的摘要；`rurge_net::resource::cache_file`、`CacheDir`），本地文件原样不动。
每次加载在二者之间选生效的文本（`SourceInfo.main_text`，`GET /v1/profiles/current` 返回的也是它，`I0004` 说是哪一份）：没有副本时是本地文件，有副本时是副本，除非本地文件自己有节、修改时间又晚于链上各份副本最近一次检查的时刻——使用者在上一次成功的检查之后改过它——，那时它生效到下一次成功的检查为止（"没有变化"的应答也算，`ProfileSources::outdated` 让主循环重载）；只有一行 `#!MANAGED-CONFIG` 的本地文件只是个指针，从不优先；副本里的 `#!MANAGED-CONFIG` 指向别处时沿着缓存走（至多换 8 次地址），被更新的是走到头的那个地址。
下载到的内容先判定再采用（`ProfileSources`，判定在 `crates/rurge-engine/src/profile/verdict.rs` 的 `Verdicts`）：整份 Profile 带着它也加载得了才采用，强制门户的页面、空的正文、截断的文件、不是 UTF-8 的内容、加载不了的新版本都不采用，缓存里的上一份照旧生效，原因在 `GET /v1/external_resources` 的 `error` 与一条 WARN `downloaded resource not adopted` 里；结论有三种——采用、不采用、等待（新版本提到一个从没下载过的来源时先留着，那个来源试完第一次时一并采用，其间旧的一代照常运行）；一份留着的版本加载不了时，向其余被跟踪的来源各问一次更新（`ASK_EVERY`、`ASK_TIMES`）。
`strict` 是暂停，不是不启动：生效的那一行写了 `strict=true`、更新已到期、最近一次尝试失败（或离线）三者同时成立时，引擎拒绝新会话（`Engine::suspend`；入口是 `dial`、UDP 流的打开、`dial_internal` 与 `dial_internal_udp`；会话的 `error` 是 `refused: the managed profile requires an update (strict)`，HTTP 客户端看到 502，SOCKS5 客户端得到 `0x05`，UDP 流以失败收场、5 秒后重试），API、监听器、已经打开的会话、订阅、规则集、GeoIP 与 Profile 自己的下载照常；一次成功的更新（不必重启），或使用者把本地文件——一份完整的 Profile——那一行的 `strict` 去掉再重载，就恢复；暂停只在内存里，不进 `state.json`。
启动（`wait_for_the_sources`，`crates/rurge/src/cli/run.rs`；逻辑在 `rurge-engine::profile` 的 `boot_need` 与 `ProfileSources::bootstrap`）：`rurge run` 先照磁盘上的样子加载 Profile（`disk_remote`）；缺着内容的来源或到期的 `strict` 来源让它等第一次尝试——先建数据目录、拿实例锁（一直拿到进程结束），往标准错误写一行 `waiting for the profile's remote sources (at most 60 s)`，至多 60 秒（`BootLimits`：每轮 30 秒、至多 8 轮），再加载一遍；到期而还没试过的 `strict` 来源，监听端绑定之前引擎就是暂停的；错误出在下载到的文件里时把每个来源更新一遍（`Boot::Refresh`），错误全在本地文件里的照旧以 2 退出；`--no-network` 时不等；主循环每秒看一眼（`follow_the_sources`，判断在纯函数 `what_the_sources_ask` 里）。
重载失败时也去下载它点名而从没下载过的来源（`09fa962`，代项目所有者作的裁定）：`reload` 在"有错误"的分支里调 `ProfileSources::follow_missing`，把那次加载点名、没有副本、也没被跟踪的来源临时跟上（一律不带 `strict`，`ManagedHop` 除外，排在在用一代的来源之后；不联网的进程什么也不跟），日志里一行 INFO（只有个数），第一份副本到了由 `changed()` 让主循环再重载——只有一行指针的 Profile 换了链接、覆盖层换了 `#!include` 的链接，不必重启；一次失败的重载不暂停任何东西，运行中的一代照旧；点名它的那一代把它变成普通的来源，不点名的把它放掉（延后第 107 行）。
指针换了链接之后、新副本到来之前，磁盘上的 Profile 没有 `http-api`，`rurge reload` / `stop` / `status -c` 找不到运行着的实例（读代码得出）：重载由 `--watch`、SIGHUP 或带 `--remote` 与 `--key` 的命令触发。
远程 `#!include`：目标是 http(s) URL 的 `#!include` 由数据目录里的副本在原位展开，下载与更新同托管配置（`interval` 取被 include 的文本自己那一行 `#!MANAGED-CONFIG`，没有就是 86400 秒）；缓存里还没有时不展开并给 `W0022`；被 include 的内容缺它被 include 进的那个节时不采用（`RemoteRef.lacks_section`）——通配命名节（`[Ruleset *]` 等）下，下载到的、写下来连一个节都没有的正文也算缺节（`W0017`：`` `<文件>` has no section ``；看的是展开它自己的 include 之前的文本，`Included.sectionless`，自己只有通配节的文本不算），有别的节而没有这一种节的仍被采用（延后第 106 行）——；加载器自己从不联网，经 `RemoteSource::get(&Url)` 向调用方要（`crates/rurge-config/src/remote.rs`），`W0011` 现在只剩 `remote include not read: no data directory`。**下载到的文本不因为被 include 而多出权力**（代项目所有者作的裁定；`rurge_config::Reach`，`crates/rurge-config/src/reach.rs`）：本地 Profile 与它 include 的本地文件是 `Anywhere`，一切照旧；托管配置的副本是 `ManagedCopy`，不 include 任何本地文件（`Reach::include_refusal`），不点名本地的集合与 `policy-path`，`external` 策略不启动（不建出站，行为同 REJECT）；远程 `#!include` 的文本与下载到的集合文件是 `Nowhere`，不点名任何本地文件、不启动任何东西；被拒的是一条 `W0033`，那一行丢掉，Profile 照常加载。**以后任何把 Profile 文本变成本机路径或程序的东西，先问 `Reach`**（延后第 31 行）；这条规矩的代价与放开它的命令行开关留给项目所有者（记录第 36、37、119 行，延后第 24、8 行）。**托管配置的副本原来可以用朴素的相对路径 include 一个本地文件（那时这一种叫 `BelowProfile`），整分支评审之后关上了**（`e8bfacd`，代项目所有者作的裁定）：读进来的行成了副本自己的行，另一份本地文件里的代理于是成了副本的策略，副本用自己的 `dns-server` 或中继就能让它们把口令发到服务商选的地址；要把托管配置和本地文件一起用，写一份不带 `#!MANAGED-CONFIG` 的本地 Profile、用 `#!include <URL>` 取下载的部分；**后来的会话不要不问项目所有者就把这个例外加回来**（去掉的代码在 `a5dd842` 的历史里）。
资源管理器的扩展（`crates/rurge-net/src/resource/mod.rs`，约 5000 行、大半是测试，延后第 28 行）：公开的 `cached_copy`、可等待的更新（`update_now`、`attempted`）、带检查的条目（`get_checked`）、采用与撤销（`adopt`、`retire`）、`ResourceOptions.never_early`（Profile 的来源的间隔只往后抖）；与手里逐字节相同的 200 当作"没有变化"，不升版本；缓存条目带正文的摘要（`meta.json` 的 `sha256`），对不上的一对文件不是条目，会被重新下载；不做 fsync；`ResourceManager::force_update` 现在有非测试的调用者（换代时的补问、向其余来源问更新）。
两个端点与命令：`GET /v1/external_resources`（`Engine::external_resources`，类型 `ExternalResource`；`type` 比手册多 `geoip`、`profile`、`include`，托管配置在暂停时带 `suspended: true`）与 `POST /v1/external_resources/update`（`Engine::update_external_resources`；键是地址的 SHA-256 或 `all`，整个请求至多 60 秒，到时还没结束的键是 `"timed out"`；换代时 `catch_up` 补问，免得键已答 `success` 而在用的一代仍是旧内容），`ApiContext.sources` 把 `ProfileSources` 交给 API，形状与同手册的差异见 `docs/api/phase1.md` 的「外部资源」与兼容性清单 10.4 节；`rurge check`、`rule match`、`dns lookup` 与 `reload` / `stop` / `status -c` 都读数据目录里已有的副本，读 Profile 时从不联网，也从不创建或写数据目录（`rule match` 与 `dns lookup` 读完之后建栈，那时照旧创建数据目录），`reload` / `stop` / `status -c` 为此多一个 `--data-dir`（环境变量 `RURGE_DATA_DIR`，缺省是平台的数据目录）。
`FINAL`：匹配在第一条启用的 `FINAL` 处结束（`Config::first_final`），策略与选项取最后一条的（`Config::effective_final`）；`RuleEngine::build` 只编译第一条 `FINAL` 之前的规则再接上最后一条，`W0019` 数第一条之后的规则，`W0021` 的文本是 `this FINAL does not give the policy; the last FINAL rule does`，`GET /v1/rules` 与启动行的规则数按编译后的表，集合注册表只登记第一条 `FINAL` 之前的规则引用的集合；这个读法是从手册的三句话推断的，没有在 Surge 上观察过（延后第 22 行，项目所有者）。
集合文件里没用上的行：带不认识的参数的行按集合计数（`ParsedSet.unknown_params`），建注册表时一条 `W0003`（只有个数与第一处的行号，从不引用参数），启动之后载入的集合三个数不全为零且与上一次不同时一行 WARN `set loaded with lines that are not used`，三个数（`skipped`、`truncated`、`unknownParams`）也在 `GET /v1/external_resources` 里 `ruleset` 与 `domainset` 的项上；同一个注册表的重编译由 `compile_lock` 排队，一次一个；`rurge rule match`、`dns lookup`、`dns cache` 的 `--wait` 最坏等两倍（`sets_behind`），时间到了而集合还落在后面时多一条 `W0022`。
API 的连接分步关（`crates/rurge-api/src/linger.rs` 的 `Lingering` 与 `Closing`）：服务端放手一条连接时先关写端，再读掉并丢弃对端还在发的字节，从放手起至多 `LINGER_TIME`（1 秒）、`LINGER_BYTES`（256 KiB），同时在收尾里的至多 `LINGER_MAX`（64）条，然后才关，`serve` 等这些收尾结束；为的是不读正文的端点（现在是三个：`POST /v1/dns/flush`、`POST /v1/profiles/reload`、`POST /v1/stop`；`POST /v1/profiles/check` 读正文）和在读正文之前就被拒绝的请求（401、403、404、405）只凭请求头就答复并关连接，正文晚于请求头到达的客户端此前读不到应答——在一台 Windows 11 x86-64 的开发机上量过，Linux 与 macOS 没有量过（延后第 23 行）；代价是一个得到应答却不关连接的空闲客户端使每次停止多等至多 1 秒。
URL 里的用户名口令作 Basic 认证发出（代项目所有者作的裁定，2026-10-12，所有者可以改）：`rurge_net::http::take_basic_authorization(&mut Url)` 把用户名与口令（百分号解码之后）拿出来，作 `Authorization: Basic …`（标为敏感）发出，`HttpClient` 对它建的每个请求这样做——规则集、域名集、订阅、GeoIP 库、Profile 的来源，DoH 早就如此——，调用方自己带了 `Authorization` 头的以它为准，重定向时只带到同一来源（方案、主机、端口都相同）且目标自己没有写用户名口令的；这些凭据此前从不作为认证发出，对 `http://` 的 URL 现在是明文；改回去只是 `HttpClient::build` 里的一个条件（记录第 49 ～ 51 行，延后第 25 行）；连通性测试不经这个客户端（`crates/rurge-policy/src/probe.rs` 在被测策略自己的出站上发两次 `HEAD`，只带 `Host` 与 `User-Agent`，读代码得出，没有用例）。
同一次改动把依赖库的日志收紧成允许名单（`quiet_dependencies()`：rurge 自己的目标按使用者要的级别，其余只留 WARN 及以上）。

### 阶段 1 的需求逐句对照代码、合并之前的评审与留下的事

随后（2026-10-13）把阶段 1 的需求逐句对着代码读了一遍（结果见下），读的时候当场补了五处小缺口，各先写用例看它红：`include-device-communication` 是手册 `[General]` 页现在列的 58 个键里多出来的那一个（需求文档当时写 57 个，手册基线重对时改成 58）、iOS 专属（`0863e52`；代项目所有者作的裁定：当时它属于比项目基线更晚的手册版本（手册基线重对之后在基线之内），加它是因为"手册里的键解析并忽略、带正确的说明"是项目自己的规矩）、`dns-server` 里同一个服务器写两遍只问一次（`ec80158`）、`encrypted-dns-server = off` 关掉加密 DNS 而不告警（`bffa339`）、系统 hosts 文件开头的字节顺序标记不算进第一个地址（`ce9a5e9`）、`HttpAuth` 的 `Debug` 不再带出口令（`fcb4abc`，没有哪一行日志打过它，是个潜伏的泄漏）；读完之后又各用一个独立的改动补上四处，都是代项目所有者作的裁定：只有系统知道的名字交系统解析库（`90d91cc`；`SystemDns::library_lookup` 必须实现、没有缺省函数体，`crates/rurge-dns/src/system.rs`，可执行文件里的 `PlatformSystemDns::library_lookup` 调 `tokio::net::lookup_host`，`crates/rurge/src/cli/runtime.rs`；`.local` 名字与 `[Host]` 的 `server:system`、`server:syslib`、`server:force-syslib`——增强模式（阶段 3）之前是同一件事——走它；限制：时限是重发间隔乘轮数（缺省 5 秒），应答报 60 秒、不进 rurge 的缓存、同时的同名查找不合并，带作用域号的 IPv6 地址不要；**调真正的系统解析库的用例数到五条（怎样数的、没数到哪里，见后面的测试约定），都只问 `localhost`——经解析器的一条，和 `rurge-proto` 里把代理服务器写成 `localhost` 的四条（经 `rurge_net::connector::SystemResolve`）——，新的用例不许把别的名字交给它**；真的解析库在真实网络上对 `.local` 名字做什么，在任何平台上都没有观察过，尤其是由单播 DNS 服务器回答的 `.local` 区域，碰到的使用者在 `[Host]` 里写 `<区域> = server:<地址>`；记录第 111 ～ 114 行，延后第 98 ～ 102 行）；`POST /v1/profiles/check` 照手册回答（`76992aa`：应答多第五个键 `error`——Profile 没有严重程度为 error 的诊断时是 `null`，否则是第一条错误的一行文字，还有别的错误时接 ` (and N more)`，措辞是 rurge 自己的——，请求里可以带 `name`，是正在运行的 Profile 的文件名或去掉最后一个扩展名的文件名才检查，别的名字是 404，不是 JSON 对象、`name` 不是字符串或正文超过 2 MiB 是 400，两句都固定、不重复请求里的任何东西、都在加载 Profile 之前给出；这个端点因此读正文；记录第 115 ～ 117 行）；AAAA 抑制按查找计数（`a72b5e9`、`4a0e0a9`；计数的写法见下面 M8c 一段里的"AAAA 抑制的计数"一句）；`[Host]` 的键末尾的点不算进模式（`9ac62b9`：被查的名字已规范化，原来 `Example.COM. = 127.0.0.2` 被接受却什么名字也匹配不上；这是 rurge 自己的读法，手册没有说键末尾的点是什么意思）。
阶段 1 的需求逐句对着代码读的结果在 `docs/acceptance/phase1-acceptance.md`：需求文档第 4 节里所属阶段含阶段 1 的 49 行（48 行的阶段栏里有 1，另一行 FR-CFG-18 写的是"各阶段"），一句一行，共 416 句，每一句有结论、指到的代码和断言了这一句的用例（一个读代码和用例断言的助手逐句读、补用例、补小缺口，每条结论另由一位评审者抽查；起点是提交 `1f1938a`，读的代码是 `a4d665c`，几行按后来补上它们的提交改写，文档的 `## 数字` 一节写明）。
结果是三个数，**要一起说，缺一不可**：**2 句没有做完**，都在优先级 P0 的需求行上——规则集文件里一行上的 `extended-matching`（只对逐条求值的行有效）、"大集合内存可控"没有判据（需求没有给数字）——，兼容性清单各有登记（延后第 76、77 行；逐句读的时候是 3 句，第三句是手册 IP 页的 `GEOIP,UNKNOWN` 与 `IP-ASN,UNKNOWN`，手册基线重对时做了，`9b0bcfe`）；**37 句的阶段栏写着阶段 1，而兼容性清单或某份设计把它们登记在后面的阶段**（P0 的需求行上 32 句，P1 的 5 句），其中 34 句今天不按需求的字面做、等阶段 3 ～ 6（有的没有代码，有的只解析，有的只做了登记的那一部分），另 3 句登记在阶段 2、今天有代码，改需求文档第 4 节的阶段栏还是改登记由项目所有者定，阶段栏没有改（延后第 78 行）；**10 句有意做成了别的样子，已登记**（逐句读的时候是 14 句：DNS 连接的 DoH、DoT、DNS 三个协议标签与 FR-DNS-04 的 `PROTOCOL` 规则匹配那 4 句，手册基线重对时随标签改按 URL 的方案补上了，`42c2294`）。
判为"做了"的行指到一条断言了这一句的用例，这不等于别处没有缺口，有几行在自己的格子里写着一处限制或一种推断的读法（`## 没有做完的` 末尾列了，不是全的，延后第 91 行）。
这一遍**没有覆盖**：阶段 1 的手工验收清单（一行也没有做过）、CI（没有绿过，所有用例只在一台 Windows 11 x86-64 的开发机上跑过）、阶段 2 的验收（功能做出来了，验收没有完成）；也不能由它推出：真实的系统解析库对 `.local` 名字做什么，在任何平台的真实网络上都没有观察过。
合并之前的整分支评审（2026-10-13，只读代码与文档，什么也没运行）的结论是改了再合并：没有 Critical，五条 Important 都在合并之前处理了（记录第 118 ～ 124 行）——托管配置的副本不再 include 本地文件（上面 `Reach` 一句），通配命名节下不是 Profile 的正文不采用（这一处的第一个提交把自己只有通配节的下载文本也当成了那样的正文，同一轮里订正，记录第 120 行），重载失败时也去下载它点名而从没下载过的来源，资源管理器的测试建在一个拒绝一切名字的解析器上，文档里几句说得不对的话订正了——；十三条小项里登记而没有改的在延后第 105 行与第 109 ～ 112 行（第 106 ～ 108 行是那五条里三条留下的事）。
修正之后把这一轮又对着评审的各条读了一遍（只读，什么也没运行，记录第 125 行）：没有新的 Critical 或 Important，读出的几处小的在合并之前改了——会话因 `strict` 被拒时，`GET /v1/external_resources` 只把 `strict` 所问的那一份托管配置标成 `suspended`、只在它上面说被拒的原因（`ProfileSources::strict_key`，`50d77a0`；此前一次失败的重载之后临时跟上的另一份托管配置也被标上），几份文档里几句不对的话订正了——。
`50d77a0` 上的那一次全量运行（自那以后只有文档变过）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 干净，`cargo test --workspace --no-fail-fast` 74 个结果行、3383 个通过、0 个失败、6 个忽略——一台机器的一次运行，不是 CI；需要参照服务端二进制的互操作用例在这台机器上立刻返回（那些服务端本机一个也没有），算在通过的里面。
没有做的，和不要当成已经有的：外部资源没有 CLI 子命令，`rurge status` 不显示暂停，托管配置更新与拒绝的记录目前是日志而不是 Logbook（延后第 10、11 行，阶段 6）；下载走直连、不经策略，`meta.json` 里存着完整的链接（延后第 1、2 行）；托管配置能改 `http-api` 与监听地址，监听地址有 `--listen-http` / `--listen-socks5` 可以钉住，`http-api` 没有对应的选项（延后第 9 行）；集合文件里的要求指令、`#!include` 行自己上的要求指令都不是指令（延后第 19、26 行）。**去向写着"项目所有者"的，后来的会话不要自己定**：计划「延后事项」第 1、2、8、9、22、24、25、28、30 行、第 77 ～ 81 行与第 105 行（其中第 24 行的 `Reach`——连同整分支评审之后关上的副本的本地 include，记录第 119 行——与第 25 行的 Basic 认证是代项目所有者作的裁定，所有者可以改；重载失败时下载新来源也是，记录第 121 行）。
第一次 CI 运行要核对的：延后第 23、27、41、58、63、71、97、98 行（含 Linux 与 macOS 上 API 连接分步关的行为）。**这个文件里的一句状态不是某条需求已经做好的证据**：一个阶段要在它的需求逐句对照过代码之后才称齐备，不是在它的里程碑都合并之后；以后收尾一个阶段的会话，先拿这个阶段的需求对一遍代码，再写"齐备"（阶段 1 补遗（M5b）收尾时已把阶段 1 的需求逐句对了一遍，判为"做了"的行指到一条断言了这一句的用例——补的用例都做过"改一行代码看它变红"的验证，原有的用例读的是断言——；结果与三个数见上面和 `docs/acceptance/phase1-acceptance.md`）。
阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`（阶段 1 补遗（M5b）又加了对真实服务商的托管配置、真实服务器上的远程 `#!include`、真实的 `strict`、真实的 `.local` 名字和 API 连接收尾的几项，也都没有做过）。

### 阶段 1 的 M1 ～ M4b

M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。
M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。

## 阶段 2

### 概况与 M1（出站地基与 HTTP / SOCKS5 上游）

阶段 2（出站协议与策略组）功能上已完成、验收没有完成（三平台 CI 没有绿过，互操作用例没有对真实的二进制跑过，手工清单一项没做；逐条的证据与缺口在 `docs/acceptance/phase2-acceptance.md`）：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。

### M2（TLS 族）

M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。
M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。
M2（TLS 族）至此完成。

### M3（策略组、订阅与连通性测试）

M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。
M3b（测速与自动组）已完成——`[General] test-timeout` 区分未设置（`General::test_target`；策略的 `test-url` / `test-timeout` 生效，`W0029` 对它们退役）；`rurge-policy` 的 `probe`（经策略自己的出站在一条连接上两次 HEAD，HTTPS 在已建立的 TLS 连接上测第二次，根证书取 `OutboundFactory::roots`）、`testbook`（`TestBook`：结果按策略与定义保存、同一策略同时只测一次、最多 8 个并发、`TestObserver`、一次性的 `test_once`）与 `auto`（`url-test` / `fallback` / `load-balance` 三种选法、`SelectCtx`、`AutoGroups`：临时覆盖、`url-test` 保持的成员、每组上一轮测试的时间与测试请求）；注册表按测试结果选成员（只有拨号触发测试、`evaluate-before-use` 的 `Resolution.pending`、嵌套组的分数、`round_timeout`）、`resolve_relay`（空组当中继一律拒绝）、订阅行不能动用主配置的 `client-cert` 与策略（`W0023`）；`rurge-engine` 的测试调度任务、测试会话进请求记录（规则 `policy test`）、`evaluate-before-use` 的有界等待（`policy group evaluation failed`）、重载时保留未变组的覆盖、`Engine::snapshot`（运行时与注册表在代际锁下成对读取）与 DNS 会话在链底为 REJECT 时的旁路；`rurge-api` 的 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 与 `POST /v1/policy_groups/select` 对自动组的临时覆盖；能力表翻转 `url-test` / `fallback` / `load-balance`（`W0008` 只剩 `smart` 与 `subnet`）。
M3c（`smart`）已完成——`rurge-policy::smart`（`SmartBook`：按策略的时间加权首字节分数与失败罚分、健康 / 未知 / 失败三种状态、站点记忆、使用计数；`rank` 给出选中的成员与重试列表；`sample` 给大组的常规轮抽样）、注册表的 `smart` 成员过滤与 `policy-priority` 因子、`Resolution.smart`（`SmartPick`）、`resolve_member`、`test_round`，以及每代预存的测试查表（`TestBook::outcome`，M3b 延后事项 #14）；`rurge-inbound` 的 `SessionHandle` 两个时刻（出站就绪、首字节）、可挂多个的结束钩子与首字节钩子、`mark_upstream_failed`；`rurge-engine` 的 `smart` 模块（拨号时按重试列表换成员，单次时限"剩余时间 ÷ 剩余尝试次数"；质量回报：首字节、3 秒无响应、上游先断）、请求记录的 `connectMs` / `firstByteMs`、`automatic()` 纳入 `smart`、每次发布注册表时 `SmartBook::retain`；能力表翻转 `smart`（`W0008` 只剩 `subnet`）。

### M4（WireGuard / SSH / external）

M4（WireGuard / SSH / external）按三份计划推进（M4a SSH → M4b WireGuard → M4c external）：M4a（SSH）已完成——`rurge-config::spec::SshSpec`（`server-fingerprint` 解析为 `HostKeyPin`、`idle-timeout`、`private-key` 的 `E0020`）与 `ProtoSpec::keystore_item`（重载的指纹含 SSH 私钥）；新 crate `rurge-proto-ssh`（russh 0.63.3，`ring` 后端）：Keystore 私钥解码（Ed25519 / ECDSA / RSA；带口令与 DSA 为 `E0022`）、主机密钥比对、`SshOutbound`（每个策略一条会话、单飞握手、认证失败等持久失败后按 60 秒起翻倍至 10 分钟退避、钉住的主机密钥算法优先协商、`direct-tcpip` 通道、会话断了重建一次、按通道计的空闲断开、30 秒 × 3 的保活、没配指纹的一次性告警、RSA 只用 SHA-2 签名）与 `testing::FakeSsh`；`rurge-engine` 的工厂分支；订阅行自己的 `private-key=` 进订阅安全门（`W0023`）；`tests/interop` 的临时 `sshd`（`RURGE_TEST_SSHD`，只在 Unix）；能力表翻转 `ssh`。
M4b（WireGuard）已完成——`rurge-config` 的 `[WireGuard <name>]` 类型化（`WireGuardSection`，`E0023`，重名的节 `W0020`）与 `spec::WireGuardSpec`（spec 带着节的内容；`underlying-proxy` 为 `W0029` 且策略 REJECT；订阅行自己的 `section-name=` 进订阅安全门）；`rurge-net` 的 `Datagram`、`Connector::connect_udp` 与 `DirectConnector::connect_udp`（按 `ip-version`、`set_tos`、7 MiB 收发缓冲）；新 crate `rurge-proto-wireguard`（boringtun 0.7.1 的 sans-IO `Tunn` 加 smoltcp 0.12：`routes` 最长前缀选 peer、`wire` 的 `client-id`、`stack`（锁内只做内存操作，每次推进都 poll 到发完，Reno 拥塞控制）、`device`（每条隧道一个任务、批量收、握手 TOS 0x88、5 分钟重拨与 endpoint 跟随、发送时本机地址或路由失效或握手无回应时换新载体（网络变化后自愈）、网络变化入口、同一私钥与 peer 只留一条隧道，节与载体设置都相同的策略共用）、`stream`、`dns`（隧道内 DNS 与缓存；每个问题 2 秒内重发、先等隧道第一次握手）、`outbound`（`WireGuardOutbound`：没有 `dns-server` 时在本机解析、含 `[Host]`；原生握手测速）与 `testing::FakeWgPeer`）；`rurge-proto` 的 `Outbound::native_test`；`rurge-policy` 的 `TestMode`（URL 与原生两种测速，`wireguard` 另加 10 秒）；`rurge-engine` 的工厂分支、拨号时 `Unsupported` 带说明、DNS 会话防环扩展到 endpoint 写成域名的 `wireguard`；bin 不输出 boringtun 自己的日志；`tests/interop` 对 sing-box WireGuard 端点（带保留字节）的互操作用例；能力表翻转 `wireguard`。
M4c（external）已完成——`rurge-config::spec::ExternalSpec`（`args` 是 `Secret`；两个 `external` 策略写了同一个 `local-port` 是 `E0018`；`args` 进内联参数的脱敏名单）、订阅导入的 `external` 行一律跳过（`W0023`）；`rurge-platform::process`（Unix 进程组与 `killpg`，Windows Job Object）；`rurge_proto::external`（`ExternalOutbound`：第一次用到时拉起、输出写 `<数据目录>/external/<策略名>.log` 且超过 1 MiB 时在拉起前轮转、子进程环境去掉代理变量并设 `NO_PROXY=*`、程序退出后下次用到时再拉起且两次拉起至少隔 2 秒、每 500 ms 连一次本机端口最多 6 次；`ProcessHook` / `ProcessGroup` 与 `NoProcessGroups`；程序自己退出时连同它启动的进程一起结束）；`rurge-engine` 的工厂分支、`EngineShared.processes` / `externals` 与 `Engine::stop_external_programs`；bin 的 `PlatformProcesses` 与退出流程最后停掉全部外部程序；只用于测试的工作区成员 `tests/external`（`rurge-external-tests`：极小的 SOCKS5 辅助程序 `socks-helper` 与真实拉起它的用例）；能力表翻转 `external`（`W0007` 不再因 `external` 出现）。
M4 至此完成。

### M5（UDP 路径）

M5（UDP 路径）按三份计划推进（M5a 地基 → M5b TLS 族 → M5c WireGuard 与其余）：M5a 已完成——`rurge_net::connector::PacketSocket`（按包收发、带地址的 UDP 载体）与 `Connector::open_udp`（`DirectConnector`：每个地址族一个未连接的 socket，忽略 Windows 的 ICMP 不可达报错）；`Outbound::udp()` / `open_udp()` 与 `UdpSupport`；DIRECT 与 `socks5` / `socks5-tls` / `external` 的 UDP（`udp-relay`，`W0029` 退役）；`ChainConnector::open_udp`（链式 UDP 载体）；`rurge-inbound` 的 SOCKS5 UDP ASSOCIATE（`UdpClient`、`UdpAdmission`、`Dialer::admit_udp` / `associate`）；`rurge-engine` 的 UDP 流水线（`udp` 模块：一条流 = 关联 + 目标、按"关联 × 出站"共用载体的全锥、60 秒 / DNS 10 秒回收、1024 流 / 4096 关联的上限）、请求记录与 API 的 `transport`、`block-quic`（`W0029` 退役）与 `udp-policy-not-supported-behaviour`、QUIC Initial 识别、`PROTOCOL` 规则按传输层匹配 `TCP` / `UDP`；`FakeSocks5` 与 `tests/external` 辅助程序的 UDP ASSOCIATE；对 sing-box `socks` 入站的 UDP 互操作用例。
M5b（TLS 族的 UDP）已完成——`rurge-proto` 的 `stream_udp`（一条字节流上按包收发：请求头随第一个包发出、写那个包的目标；`trojan` 的 UDP ASSOCIATE 与 `anytls` 的 UDP over TCP v2 两种封装）、`trojan` / `anytls` 的 `open_udp`（全锥）、`vmess` 的命令 2（`vmess::udp`：每个目标一条 VMess 连接、随它的第一个包建立、每个数据报一个分块，对称型）；`FakeTrojan` / `FakeAnyTls` / `FakeVmess` 的 UDP 与 `rurge_proto::testing::udp_echo_server`；经引擎的端到端用例（`tests/udp_tls_family.rs`）；对 sing-box（三种）与 xray（vmess）的 UDP 互操作用例。
M5c（WireGuard 的 UDP 与其余）已完成——`rurge-proto-wireguard` 的 `TunnelUdp`（隧道里每个地址族一个 UDP socket、第一次发往该族时绑定、全锥，目标名经隧道 DNS 或本机解析）与 `Stack::udp_bind` / `check`；`rurge_net::packet_datagram`（把 `PacketSocket` 变成一条到固定目标的 `Datagram`）与 `ChainConnector::connect_udp`，`wireguard` 的载体经 `underlying-proxy`（底层策略不载 UDP 时拨号失败，`W0029` 退役）；启动时 peer 连不上的告警每个策略的每个 peer 5 分钟至多一次（M4b 延后事项 #15）；`rurge_policy::udp_probe`（经策略的 UDP 向 `hostname@ipv4` 问一次 A 记录）、`Engine::test_udp` 与 `POST /v1/policies/test` 结果里的 `udp` 键（`test-udp` / `proxy-test-udp`，不保存、不参与组的选择）；`smart` 计入 UDP（载体打不开算失败、第一个回包算首字节、3 秒无回包只在 53 / 443 端口算失败、UDP 不换成员）；`dns-follow-interface`（`rurge_net::connector::Via` / `ResolveVia`、`Resolver::lookup_via`：策略自己的解析经它的 `interface` 问普通 DNS 服务器、答案另存，配了加密 DNS 时不跟随；没有 `interface` 时 `W0028`）；对 sing-box WireGuard 端点的 UDP 互操作。
M5 至此完成。
M5 合并后的复查（2026-10-02，M5 设计第 23 节）改了：Windows 上写了 `interface` 的策略能开 UDP 载体；发给客户端失败只丢那个包；发往上游时出站不收或系统拒发的包只丢那一个（`lost_alone`），其余发送失败的流记为失败并歇 5 秒，一次发送至多 15 秒（`SEND_TIMEOUT`，超时作废载体）；TCP keepalive（`rurge_net::socket::keep_alive`，监听与 `DirectConnector` 都开）；名字目标的回包按端口归流，`smart` 计入 UDP 的发送失败、3 秒规则只看发往地址的流；重建注册表时沿用内置 DIRECT；`wireguard` 跟随底层策略的变化（`Connector::route` / `rurge_net::connector::Route`、`PolicyRegistry::relay_now`）；开 UDP 流时的解析限时 10 秒；每个关联排队至多 1 MiB（`Queued`）、一次打开失败等着它的流一起失败；双栈监听上 IPv4 客户端的回包发往映射地址。

### M6（Shadowsocks / Snell / HTTP/2 族）

M6（Shadowsocks / Snell / HTTP/2 族）按三份计划推进（M6a Shadowsocks → M6b Snell → M6c HTTP/2 族）：M6a（Shadowsocks）已完成——`rurge-config::spec` 的 `SsSpec`（`SsMethod`：AEAD 五种、`none`、SS 2022 两种；2022 的 `password` 按冒号拆成逐层的 Base64 密钥、最后一段是用户密钥，不合法是 `E0018` 且不引用取值；`udp-relay`、`udp-port`）与 `ObfsOpts`（与 Snell 共用；`obfs-host` 缺省服务器主机名、`obfs-uri` 缺省 `/`）、`NotImplemented`（取代 `legacy_vmess` 标记：vmess 旧握手与 `ss` 流式旧方法都是 `W0007` + REJECT，流式方法每种每次加载一条，会话日志 `policy protocol not implemented: ss (<method>)`）、`obfs-host` 进内联参数的脱敏名单；`rurge_proto::transport::obfs`（simple-obfs 的 `http` / `tls`，自写模板，`Stack` 的一层：connect → shadow-tls → obfs → tls → ws）；`rurge_proto::shadowsocks`（`ShadowsocksOutbound`：AEAD 分块流与 `none`、SS 2022（BLAKE3 子密钥、请求头块与填充、应答的类型 / 时间戳 / 回显 salt 校验、SIP023 多用户身份头）、UDP（`udp-relay=true` 发往 `udp-port`，全锥；2022 的分离头、按服务端 session 的防重放窗口））；新依赖只有 `blake3`；`rurge_proto::testing` 的 `FakeShadowsocks`（AEAD、2022 含身份头、UDP、两种 obfs）与 `accept_obfs`；`rurge-engine` 的工厂分支与经引擎的端到端用例（`tests/outbounds_shadowsocks.rs`）；能力表翻转 `ss`（流式旧方法除外）；测试里"未实现的协议"的例子从 `ss` 换成 `hysteria2`；`tests/interop` 对 shadowsocks-rust `ssserver`（固定版本 v1.25.0，`RURGE_TEST_SSSERVER`）与 sing-box `shadowsocks` 入站的互操作用例。
M6b（Snell）已完成——`rurge-config::spec` 的 `SnellSpec`（`SnellVersion` 只有 V4 / V5；`version` 1–6、缺省 1，1–3 与 6 是 `NotImplemented::SnellVersion`：`W0007` + REJECT，每个版本每次加载一条，会话日志 `policy protocol not implemented: snell v<n>`；`psk` 是 `Secret`；v4 / v5 的 `obfs` 只有 `http`；`mode` 只对 v6 校验）；`rurge_proto::snell`（`SnellOutbound`：`kdf`（Argon2id 取前 16 字节，在阻塞线程上算；工作区依赖 `argon2` 0.6，已在依赖树里）、`record`（`SnellStream`：每方向一个 salt、7 字节加密头、首帧 256–511 字节填充与负载密文交错、每帧至多 0x3FFF、空帧结束一个方向、复用时计数器延续）、`tunnel`（请求头随首段负载写出，`reuse=false` 发 Connect `0x01`、`reuse=true` 发 ConnectV2 `0x05`；应答 `00` / `02`；干净结束的连接在 `Drop` 时回池，没结束的在后台补完再回池；池里取出的失效连接换新连接重发一次）、`pool`（每个出站至多 8 条空闲连接、空闲 60 秒回收）、`udp`（命令 `0x06`：每个载体一条自己的连接、从不进池，每个数据报一帧，全锥；`udp-port` 只解析（`W0029`），UDP 一律走主端口））；`LazyHead` 改为泛型（`LazyHead<S = BoxedStream>`，加 `get_mut` / `into_inner`）；`rurge_proto::testing` 的 `FakeSnell`（`SnellScript`：TCP、ConnectV2 复用、UDP、obfs http、拒绝、每条连接的请求数上限）；`rurge-engine` 的工厂分支与经引擎的端到端用例（`tests/outbounds_snell.rs`）；能力表翻转 `snell`（v4 / v5）；`tests/interop` 对 sing-box `snell` 入站（`version: 5`，同时接受 v4 客户端；sing-box 固定版本从 1.14.1 升到 1.14.2）与 Surge 官方 snell-server v5.0.1（只有 Linux 版，`RURGE_TEST_SNELL_SERVER`，只在 Linux CI 上装）的互操作用例。
M6c（HTTP/2 族）已完成——`rurge-config::spec::h2`（`H2ConnectSpec`：凭据写在端口之后或命名、命名的优先，`headers`、`max-streams` 缺省 3 且至少 1、`udp-relay`；`TrustTunnelSpec`：`username` / `password` 只读命名写法、必填（`E0018`）；两者 `alpn` 固定 `h2`（写了别的 `W0028`），`headers` 里 HTTP/2 禁止的连接专用字段与 `host` 各 `W0028` 并去掉；`trust-tunnel` 的 `h3=true` 是 `W0029`、M7 之前按 HTTP/2 连，`udp-relay` 是 `W0028`）；`rurge_proto::h2pool`（`H2Pool`：每个出站一个池，每条 TLS + HTTP/2 连接至多 `max-streams` 条流且不超过服务端的上限、流数自己计，新连接收到服务端 SETTINGS 之前只承载一条流（得知 SETTINGS 的 PING 5 秒内没有回应即不再分配新流），在途的每次拨号算作 `max-streams` 个位置、放不下的请求再拨（至多 8 条在途），连接窗口为 `max-streams` × 1 MiB（至少 4 MiB），没有流的连接空闲 60 秒关闭，收到 GOAWAY 的连接不再分配新流，请求没得到应答时用 PING 检查连接是否还活着（5 秒），死了就换新连接；`H2Stream`：按流控窗口分段写、读即释放容量、END_STREAM 半关闭；`StackDial`：ALPN 不是 `h2` 时 `<协议>: the server does not speak HTTP/2`）；`rurge_proto::h2connect`（`H2ConnectOutbound`：CONNECT + Basic，`headers` 替换同名的自生成字段、`<random-string>` 按请求生成；407 是 `h2-connect: proxy authentication required`；`capsule`（RFC 9297 的 capsule 与 varint）与 `udp`（`udp-relay=true` 时的 CONNECT-UDP：每个目标一条 extended CONNECT 流、对称型，masque 默认模板的路径，DATAGRAM capsule，其它 capsule 与 context id 丢弃，超过 65527 字节的数据报不发，服务端没通告 extended CONNECT 时发往每个目标的第一个包失败，空闲 60 秒关流））；`rurge_proto::trust_tunnel`（`TrustTunnelOutbound`：同一套 CONNECT，`user-agent: rurge`，407 是 `trust-tunnel: authentication failed`，不载 UDP）；`TlsClient::wrap_negotiated` / `Stack::open_negotiated`（取协商出的 ALPN）；`rurge_proto::testing` 的 `FakeH2Proxy`（`H2ProxyScript`：Basic 与 407、拒绝、服务端流数上限、GOAWAY、extended CONNECT 与 CONNECT-UDP、多余的 capsule、`user-agent` 必填、不参与 ALPN、双向 TLS）；`rurge-engine` 的两个工厂分支与经引擎的端到端用例（`tests/outbounds_http2.rs`）；能力表翻转 `h2-connect` 与 `trust-tunnel`；`tests/interop` 对 TrustTunnel endpoint v1.1.0（只有 Linux / macOS 版，`RURGE_TEST_TRUSTTUNNEL`；sing-box 1.14.2 的 `http` 入站只说 HTTP/1，所以 endpoint 同时充当 `h2-connect` 普通 CONNECT 的参照）的互操作用例；CONNECT-UDP 没有可用的参照服务端。
M6 至此完成；合并后的复查（M6 设计第 18 节）又修了会话池的串行拨号与 SETTINGS 到达前静默断掉的连接、Snell 的 `udp-port`（只解析）与重拨时只唤醒最后一个轮询者、CONNECT-UDP 的 `:authority`（取 TLS 的名字）、`headers` 的值进 `Debug`、ss 首字节前被重置时的提示、`obfs-uri` 脱敏，并把 CI 钉在 1.89（`RUSTUP_TOOLCHAIN=1.89.0`；此前 CI 每次装最新 stable，三个平台都在 clippy 一步失败，测试与互操作从未执行过）。

### M7（QUIC 族）

M7（QUIC 族）按三份计划推进（M7a QUIC 地基 + Hysteria 2 → M7b TUIC → M7c HTTP/3 族与 DoQ / DoH3）：M7a 已完成——新依赖只有 `quinn` 0.11.12（`default-features = false`，`runtime-tokio` + `rustls-ring`，随之引入 quinn-proto / quinn-udp；不用 crates.io 的 `h3`，M7-D5），`blake2` 0.10 沿用依赖树里的；`rurge-config::spec` 的 `QuicOpts`（只有 `port_hopping: Option<PortHopping>`；`alpn` 留在 `TlsOpts`、QUIC 类缺省 `h3`，`ecn` 留在 `CommonOpts`）、`Hysteria2Spec`（`password` 必填、`Secret`；`download-bandwidth` 整数 Mbps；`Hysteria2Obfs` 的 Salamander / Gecko 互斥，都是 `Secret`）与 `ProtoSpec::quic()`：`port-hopping` 写法错误是 `E0018`，`port-hopping-interval` 小于 5 是 `W0012` 并按 5、没有 `port-hopping` 时 `W0028`，`tfo` / `udp-relay` 在 `hysteria2` 上是 `W0028`，`ecn` 在它上面不再 `W0029`，`port-hopping` × `underlying-proxy` 与 Shadow TLS × `hysteria2` 两条 `E0018` 真正报出，组级 `underlying-proxy` 的组里带 `port-hopping` 的成员（配置本身给出的）加载时报 `W0023`，`salamander-password` / `gecko-password` 进内联参数的脱敏名单；`rurge-net` 的 `Connector::as_direct()`（只有 `DirectConnector` 覆盖：直连时 QUIC 自己建 socket，其余经 `connect_udp` 的载体）与 `quic` 模块（`Path::open`：直连 socket 复用 `DirectConnector::bind_udp`，`tos ≠ 0` 时发送不经 quinn-udp，`ecn=off` 去掉两侧的标记，链式载体（占位地址 `192.0.2.1:443`、不做 PMTU 探测、放不下就丢），端口跳跃层（每次换到另一个端口、上一个 socket 留到下一跳）；socket 层的发送错误一律丢包——非 `WouldBlock` 的错误会让 quinn 的连接挂死；`dial`：每条连接一个 quinn endpoint、`Transport::STANDARD`（空闲 30 秒、保活 10 秒、窗口每流 8 MiB、每连接 20 MiB——M8c 起每连接 256 MiB，见 M8c 一段——、Cubic）、`connection_error` 的 `quic: …` / `tls: …` 文本；`testing` 的 `TestCert` / `EchoServer` / `MultiPort`）与 `quic::h3`（自写的最小 HTTP/3 客户端：只开控制流、SETTINGS 与等待、QPACK 静态表 + 字面量 + Huffman 解码（照 RFC 9204 附录 A 与 RFC 7541 附录 B 录入并独立核对）、HEADERS / DATA、可取消安全的写、GOAWAY；`testing` 的 `H3Server`）；`rurge-proto` 的 `QuicTls` / `build::quic_tls`（与 `TlsClient` 共用构造，只用 TLS 1.3）；新 crate `rurge-proto-quic`：`session`（`Dialer`（`with_layer`；`port-hopping` 遇上非直连的连接器在构建时失败，组中继派生的 `M (via R)` 因此被略去）、`Protocol`、`Session`：在请求自己的 future 里单飞拨号与握手、只关还开着的连接、按租约计的 60 秒空闲关闭、失效连接重试一次、跟随底层策略的去处（`Connector::route()`）；`testing::ServerPort`：能重启并回 stateless reset 的服务端端口）与 `hysteria2`（`Hysteria2Outbound`：HTTP/3 认证（失败以 0x101 关闭，`Hysteria-UDP` 按 Go 的 `ParseBool` 读）、TCP（`0x401`，任何非 0 状态都是拒绝）、UDP（会话号按出站递增、分片与重组、`Hysteria-UDP: false` 时打开载体即失败、换连接沿用会话号）、`obfs`（Salamander：BLAKE2b-256 用 RFC 7693 的自测核对；Gecko：长头包拆 2–8 片、512–1200 字节、至多 8 组在途、8 秒过期；混淆口令不对只表现为超时）；`testing::FakeHysteria2`（线上编解码与混淆都独立实现））；`rurge-engine` 的工厂分支与经引擎的端到端用例（`tests/outbounds_hysteria2.rs`；`common::harness_edited` 缩短跳跃间隔）与不作门禁的吞吐基准（Windows 回环、release：每方向明文约 70–80 MiB/s、Salamander 约 56–66 MiB/s）；bin 不输出 `quinn` / `quinn_proto` 的日志，`quinn_udp` 留 WARN；能力表翻转 `hysteria2`；测试里"未实现的协议"的例子从 `hysteria2` 换成 `tailscale`；`tests/interop` 对 sing-box 1.14.2 `hysteria2` 入站（无混淆、Salamander、Gecko）的互操作用例（CI 已在三平台装 sing-box，不用改）。
M7b（TUIC）已完成——`rurge-config::spec::TuicSpec`（`tuic` 与 `tuic-v5` 共用一份：`TuicVersion::V4 { token }` / `V5 { uuid, password }`，都是 `Secret`；`uuid` 与 `vmess` 共用 `read_uuid`；缺凭据或 UUID 不对是 `E0018` 且不引用取值，另一个版本的凭据与 `udp-relay` / `tfo` 是 `W0028`，`ecn` 生效；`port-hopping` × `underlying-proxy` 与 Shadow TLS × `tuic` / `tuic-v5` 两条 `E0018` 真正报出；格式不对的这两种行从只报 `W0007` 变成加载错误；`required` 移到 `spec/mod.rs` 与 `trust-tunnel` 共用）；`rurge-proto-quic` 的 `tuic`（`TuicOutbound`：一份实现，按版本分支；认证在连接建好后写进一条单向流、不等回应——v5 的 token 是 TLS 导出的 32 字节（label 为 UUID 的原始字节、context 为口令；导出失败时拨号失败，不 panic），v4 是 BLAKE3(token)（只加 `blake3` 的依赖边）；TCP 开流即写 Connect，v5 立即交出隧道、v4 等 `04 ff REP`；v5 没有错误码，"码 0、空原因、服务端还没转发过任何东西"的关闭读作 `tuic: authentication failed`，v4 的 0xfffffff0 ～ 0xfffffff3 四个码三条文本（认证失败、认证超时、协议错误）；UDP：服务端接受 QUIC 数据报时 `native`，否则每个包一条单向流、经每条连接一个的写任务（至多 64 个排队、满了丢包、从不截断一个包），v5 分片与重组（每个关联至多 8 个包在重组）、v4 不分片（放不下的包 `InvalidInput`），关联号按连接分配（v5 从 0 递增、v4 随机 32 位，换连接时取新号），载体走时经同一个写任务的队列发 `Dissociate`（排在已交出的包之后，满了不发），认证被拒时一次发送失败，IPv4 映射的来源按 IPv4 读；心跳只在有转发时发（v5 每 3 秒一个数据报、服务端不收数据报时不发，v4 每 10 秒一条单向流）；`testing::FakeTuic`（v4 / v5、两种 UDP 模式、认证失败的关闭、流的扣留与放行，线上编解码独立实现）；测试用的日志捕获移到共用的 `test_log`）；`rurge-engine` 的工厂分支（两种关键字一个分支）与经引擎的端到端用例（`tests/outbounds_tuic.rs`；`tests/common` 收了四个与协议无关的辅助函数）；能力表翻转 `tuic` / `tuic-v5`；`tests/interop` 对 sing-box 1.14.2 `tuic` 入站（v5，只能覆盖 `native` 模式）与官方 tuic 0.8.5 服务端（v4；GPL，只作为外部进程运行；`RURGE_TEST_TUIC_SERVER`，只在 Linux CI 上装）的互操作用例——这个发布没有 GitHub 的 `digest`，CI 那一步的 SHA-256 取自发布自带的校验文件 `tuic-server-0.8.5-x86_64-linux-musl.sha256sum`（M7c 收尾时钉住，Linux 的 CI 任务不再卡在这一步；此前是占位值）。
M7c（HTTP/3 族与 DoQ / DoH3）已完成——没有新依赖（`rurge-dns` 多了 `quinn` 的依赖边，`rurge-engine` 的测试多了 `socket2` 的依赖边）；`rurge-config::spec` 的 `MasqueSpec`（`tls`、`quic`、`credentials: Option<MasqueCredentials>`，凭据是 `Secret`；`username` / `password` 只读命名写法、要么都写要么都不写，缺一个或写了却为空是 `E0018` 且不引用取值；`headers` / `max-streams` 是未知参数，`udp-relay` / `tfo` 是 `W0028`，`ecn` 生效，端口跳跃同 `hysteria2`）与 `TrustTunnelSpec.h3`（`read_trust_tunnel` 先读 `h3`：ALPN 按传输钉成 `h2` / `h3`，`h3=true` 时 `tfo` 是 `W0028`、`headers` 里 HTTP/3 禁止的字段按 HTTP/3 的文本告警并去掉、Shadow TLS 是 `E0018`（`shadow_tls::refuse`）；两种传输上 `port-hopping` / `port-hopping-interval` 都是 `W0028`；`h3` 的 `W0029` 退役）；`rurge-net::quic::h3` 的 extended CONNECT（`Protocol::CONNECT_UDP`，先等服务端的 SETTINGS）与 HTTP Datagram（`Client::with_datagrams` 宣告 `H3_DATAGRAM`、`RequestStream::datagrams` 按流分发、只给 CONNECT 请求；`Client::new` 照旧不宣告，Hysteria 2 的 UDP 不受影响）、更严的应答检查（`content-length` 比对、SETTINGS 的 0x08 / 0x33 只能是 0 或 1、CONNECT 的 2xx 之后再来 HEADERS 是连接错误）、服务端对一个请求说了什么带在错误里（`h3::rejected` / `h3::going_away` / `Client::unprocessed`：被以 H3_REQUEST_REJECTED 拒绝、因 GOAWAY 没有发出、流号不低于 GOAWAY 的界限，不解析文本），`Transport::keep_alive` 改为 `Option`、新增不保活的 `Transport::DNS` 与 `Transport::apply`，`ServerPort` 搬到 `rurge_net::quic::testing`；QUIC 经载体时载体此刻发不了就等而不丢（`Datagram::poll_send_ready`：`Carried` 返回 `WouldBlock`、轮询器在载体可发时唤醒，新 socket 上连接的第一个包不再丢失），`packet_datagram` 把载体发送失败的原因交给下一次接收（只丢这一个包的失败——`rurge_net::connector::lost_alone`——除外），握手期间拨号以载体的原话失败，已建成的连接在载体报出这样的失败时随即结束、文本仍是 `quic: the UDP path to the server failed`（`ConnectionReset` 一类的错误 quinn 不理会，载体自己因它而结束时另有交法；`wireguard` 的隧道不变；M8c 任务 2 又改了这一带的几处，见 M8c 一段）；`rurge-proto` 的 `connect_udp`（CONNECT-UDP 的路径与 capsule 编解码，`h2-connect` 与 `masque` 共用）、`http::basic_authorization` 与 `trust_tunnel::{connect_request, refused}`（两种传输共用）；`rurge-proto-quic` 的 `masque`（`MasqueOutbound`：TCP 是经典 CONNECT，每个 CONNECT 都主动带 `proxy-authorization: Basic`（手册没写字段名，rurge 的选择）；UDP 是 CONNECT-UDP——每个目标一条 extended CONNECT 流（对称型）、随发往它的第一个包打开、2xx 之后才发、包走 HTTP/3 数据报（收的时候也接受流上的 DATAGRAM capsule），放不下的包丢弃而不退回 capsule（能发的负载就是 QUIC 数据报的余量：路径 MTU 探测抬高之前不到 1200 字节；经 `underlying-proxy` 时 QUIC 不做探测，M7c 时一直停在 1200 字节的包，内层 QUIC 的 Initial 发不出去，M8c 起这样的连接从 1350 字节的包起步，见 M8c 一段），服务端不支持 CONNECT-UDP 时只让 UDP 失败（`masque: the server does not support CONNECT-UDP`），每个目标的流空闲 60 秒以 FIN 关闭（它们与 TCP 隧道共用服务端给的流额），GOAWAY、连接关闭或底层策略变了时在当前连接上重开）、`trust_tunnel`（`TrustTunnelH3Outbound`：与 HTTP/2 版同样的请求头与错误文本，不宣告 HTTP Datagram，不载 UDP，没有 HTTP/2 回落）与两者共用的 `connect`（`Proxy` / `Http3` / `tunnel`：经典 CONNECT 的隧道，读的时候也推进写了一半的帧；`failure` 按发生了什么判失效——应答到来之前连接已结束、因 GOAWAY 没有发出、被以 H3_REQUEST_REJECTED 拒绝或流号不低于 GOAWAY 界限的请求在新连接上重试一次，界限之下在自己流上的失败是终局），`Session::with_stream_cap`（`trust-tunnel` 的 `max-streams` 按每条 QUIC 连接计，满了照 HTTP/2 池的规则另拨；失效重试只落在失效那条之后拨的连接上；连接的接收窗口取 ⌈`max-streams` × 8 MiB × 8/7⌉、至少 20 MiB（M7c 写的是 max(`max-streams` × 8 MiB, 20 MiB)，M8c 因 quinn 一次只公布窗口的八分之一而改，见 M8c 一段），读端停住的流占不光它，一条连接最坏缓冲 `max-streams` × 8 MiB 的未读数据；不看服务端的 QUIC 流数上限），`carrier`（`hysteria2`、`tuic` 与 `masque` 的 UDP 载体共用的路由时钟、收件箱与限时打开），`testing::FakeH3Proxy`（MASQUE 与 TrustTunnel endpoint 两种服务端、quiche 式的 GREASE、CONNECT-UDP 的各种开关）；不设流数上限的 QUIC 出站（`masque`、`hysteria2`、`tuic`）在 M7c 有一处已知限制留给 M8：三条读端停住的隧道占满 20 MiB 的连接窗口时，这个策略其余 TCP 隧道的下行随之停住（M8c 把连接窗口改成 256 MiB，现在要 29 条读端停住的隧道才拖住其余的，见 M8c 一段）；`rurge-dns` 的 `quic://`（`upstream::doq`，RFC 9250：ALPN `doq`、报文 ID 0、EDNS(0) padding 到 128 字节的倍数（`message::doq_query`）、写完即 FIN，违规的应答以 DOQ_PROTOCOL_ERROR 关连接，放弃的查询以 DOQ_REQUEST_CANCELLED 停流）与 `h3://`（`upstream::doh3`：POST，没写路径时 `/dns-query`，应答的判据与 DoH 共用）两种上游，带用户名口令的 `quic://` / `h3://` / `tls://` / `tcp://`（任何位置出现 `@`）是 `W0026` 且文本不含口令，`https://` 的上游名与告警 / 错误文本同样不带用户名口令、它的 `@` 只能在主机之前（路径、查询串或片段里出现 `@` 的也是 `W0026`），共用 `upstream::session`（每个上游一条 QUIC 连接；单飞拨号由会话的后台任务完成，不随发起它的查询被放弃而中止，以那个查询的截止时间为限、到时限不算失败；域名的多个地址各占一份时间；不保活，连接一关闭即放手（连同载体）；握手后立刻被关的连接让等它的查询一并失败、不各自重拨；查询中途失效的连接（DoH3 的请求被以 H3_REQUEST_REJECTED 拒绝也算）换新连接重问一次；悄悄失效的连接——查询写出后没有答案地结束，而连接过了 max(3 × 往返时间 + 50 ms, 100 ms) 仍什么也没收到（`Progress`）——不再收新查询），`BootstrapConnector::connect_udp`，`testing::{FakeDoq, FakeDoh3}`；`rurge-engine` 的 `PipelineConnector` 的 UDP 一面（手册基线重对起是 `PipelineConnector::connectors()` 的 `quic` 与 `h3` 两个面，逻辑在 `connect_udp_as`）与 `Engine::dial_internal_udp`（`encrypted-dns-follow-outbound-mode` 下 DoQ / DoH3 的 QUIC 载体作为内部 UDP 会话走规则：协议标签当时按端口（443 → `DOH3`，其余 → `DOQ`），手册基线重对起按 URL 的方案（`h3://` 是 `DOH3`、`quic://` 是 `DOQ`），DIRECT 用 DIRECT 策略自己的 UDP，REJECT 与以域名配置的代理照 TCP 告警并直连，不载 UDP 的代理照 `udp-policy-not-supported-behaviour` 办——`REJECT`（缺省）时该上游这次失败（`via <策略>: policy does not support UDP`），`DIRECT` 时载体改经 DIRECT 的 UDP 发出、记录写 `policy does not support UDP; sent through DIRECT`；载体打开之后报错（发不出去、自己断了）时这条内部会话以失败结束（`ConnectionReset` 除外：M8c 起它不结束会话、不进记录、不计失败），记录写载体的原话、解析器拿到 `via <策略>: <原话>`、算作 `smart` 成员的一次失败（它是唯一上游唯一的去处时，每秒一轮的重发各拨一次，一次查找至多 5 条失败的会话与 5 次失败回报，M8c 起其中只有第一条计入 `smart` 成员——同一个地址在站点记忆里一小时内只罚一次，读 `report_internal` / `report_failure` 所得，没有用例钉住这个数），改走直连的载体报的是系统自己的错误文本；路由与 `smart` 的回报与 TCP 共用 `route_internal` / `report_internal`）、`masque` 与 `h3=true` 的 `trust-tunnel` 两个工厂分支与经引擎的端到端用例（`tests/outbounds_masque.rs`；`trust-tunnel` 的 HTTP/3 在 `tests/outbounds_http2.rs`）；能力表翻转 `masque`（`trust-tunnel` 的 `h3=true` 不再是 `W0029`），`W0026` 不再仅因 scheme 是 `h3://` / `quic://` 而出现（只剩解析不了的 URL）；`tests/interop` 对 masque-server v0.13.0（`RURGE_TEST_MASQUE_SERVER`，只在 Linux）、TrustTunnel endpoint v1.1.0 的 QUIC 监听（`[listen_protocols.quic]`）、AdGuard dnsproxy v0.86.0 与 CoreDNS v1.14.7（`RURGE_TEST_DNSPROXY` / `RURGE_TEST_COREDNS`，三平台；解析器不收测试 CA，用 `encrypted-dns-skip-cert-verification`）的互操作用例——这四个参照服务端的命令行、配置与就绪日志读自源码，还没有对真实的二进制跑过，第一次 CI 运行时核对。
M7 至此完成。

### M8（收尾与验收）与 M8a

M8（收尾与验收）按三份计划推进（M8a CI 转绿与凭据不外泄 → M8b P2 协议项：手册列出的 11 种 `ss` 流式方法与 Snell v1 ～ v3 实现、VMess 的旧握手不实现 → M8c 各份计划的遗留事项、可行性报告与阶段验收）；`W0007` 与 `OutboundError::Unsupported` 不退役，改指 rurge 不实现的协议与变体（M8-D10）。
M8a 进行中：已完成的是 CI 的失败摘要（测试一步 `--no-fail-fast`、限时 60 分钟，失败时 `.github/scripts/report-test-failures.py` 把失败用例输出的开头、异常退出的二进制、编译错误与没跑完的二进制（其中还在跑的用例，`e499061` 起连同其中已经失败的用例）发成注解并写进任务摘要——任务日志要登录才能读，注解与摘要公开可读），配置层的凭据不外泄（`rurge_config::redact::{without_credentials, url_without_credentials}`：`[General]` 里 DNS 服务器 URL、`http-api` / `external-controller-access`、监听地址与 `wifi-access-http-auth` 的诊断不再引用凭据，`redact_profile` 抹掉 DNS 服务器 URL 里的用户名口令与 `[Tailscale]` 的 `auth-key`、`key@address` 按最后一个 `@` 切；`EncryptedDns`、`ControllerAccess`、`ProxyPolicy`、`KeystoreItem`、`WsOpts`、`ObfsOpts`、`CommonOpts`、原样保留的行 `Entry` 手写 `Debug`；判据是标记用例 `crates/rurge-config/tests/secrets.rs`），DNS 上游 URL（`https://` 的用户名口令百分号解码后以 `Authorization: Basic` 发出并从请求目标里去掉；全角 `＠` 与 `@` 同样对待；端口错误不引用取值），四个开关的环境变量接受 `1` / `0`、`yes` / `no`、`on` / `off`（clap 的 `BoolishValueParser`），`rurge-net` 资源的 `Debug` 不显示 URL 的路径与正文；余下的是 CI 的失败：首跑（`b41930a`）三个平台都在 `cargo test` 一步失败而失败清单读不到，第二次运行（`7bd6589`）读到了，六类失败里五类已修、第 6 类换了找端口的做法并补了诊断（原因仍不知道），管不管用要再推送一次才知道，见下面"CI 还没有绿过"一句。

### CI

CI 现在的样子（`.github/workflows/ci.yml`，M8b 终审改过）：推送到 `main` 与拉取请求时触发；`check` 任务在 ubuntu / windows / macos 上各跑一遍，工具链固定 1.89.0；`RURGE_INTEROP_REQUIRED=1` 在工作流一级的 `env` 里设置，不由哪一个安装步骤设置（否则那一步一失败变量就没有了，全部互操作用例会不声不响地跳过）；参照服务端的每个安装步骤（连同把 `RURGE_TEST_SSHD` 指向 `sshd` 的那一步）都带 `continue-on-error: true`，装不上的服务端只让要用它的用例各自以"找不到二进制"失败，其余用例照常运行；没有单独的互操作任务，互操作用例随 `cargo test --workspace --no-fail-fast` 那一步（限时 60 分钟）运行；另有一个只做 `cargo bench --no-run` 的 `bench` 任务。
CI 还没有绿过：最近一次运行在 `7bd6589`（2026-10-17，项目所有者推送的，运行号 39022550224；它是阶段 3 / M1 的合并，阶段 3 / M2 的提交不在里面）——`bench` 通过，`check` 任务里 fmt、clippy 与全部参照服务端的安装步骤三个平台都通过，三个平台都在 `cargo test` 一步失败。
Linux 6 分半跑完了全部测试二进制，以 101 退出，5 条失败。
macOS 与 Windows 又用满了测试一步的 60 分钟，都停在 `rurge-proto` 的单元测试二进制里（各有三条用例没有结束）；测试二进制按包名的次序一个接一个地跑，所以只跑到排在它前面的各包（`rurge`、`rurge-api`、`rurge-config`、`rurge-dns`、`rurge-engine`、`rurge-external-tests`、`rurge-inbound`、`rurge-interop`、`rurge-net`、`rurge-platform`、`rurge-policy`），macOS 上 6 条失败、Windows 上 7 条失败；`rurge-proto-quic`、`rurge-proto-ssh`、`rurge-proto-wireguard`、`rurge-rules` 这四个包的测试在这两个平台上一个也没有跑到，`rurge-proto` 单元测试里别的用例在这两个平台上的结果也不知道（失败用例的输出要到二进制结束时才打印，那时的失败摘要也不列没跑完的二进制里已经失败的用例——`e499061` 起列出——，任务日志又要登录才能读）。
失败摘要这一次认出了跑完的二进制里的全部失败，所以这是第一次读得到互操作用例对真实二进制的结果：装了参照服务端的平台上，除下面第 2 ～ 4 类的五条之外都通过（各平台实际跑了多少个没有读到：任务日志要登录才能读）。
六类失败（根因读自代码、参照服务端的源码与注解，本机能复现的只有第 1 类）与修它们的提交（M8a 计划「执行期修正记录」第 7 ～ 12 行）：一、`rurge-proto` 里 `shadowsocks` 与 `snell` 的四条单元用例，macOS 与 Windows 上每个平台有三条没有结束，`LazyHead` 的两半交给两个任务轮询时丢了写端的唤醒——`f8d0389`（在类型里修）、`4fc1bad`（在别的任务里写、在用例的任务里等它写完的 18 处限时 30 秒）、`cae06c3`（四条用例，其中一条用真实的管道在两个任务里复现了这一类，旧代码上红；CI 上挂住的那四条用例本身在本机用旧代码也过）、`26be5e2`，整分支终审之后又加了 `199e584`（写端的 `poll_flush` 或 `poll_shutdown` 完成时也了结读端欠着的 flush）与 `f0e41fd`（VMess 的 UDP 在两个任务里轮询两半的用例，旧代码上要到时限才完成而失败、现在约 0.2 秒），生产里也有这样轮询它的：VMess 的 UDP 载体在拨号时起一个任务读、由 `send_to` 的任务写，旧代码下第一个数据报的写过了 100 毫秒的宽限仍是 `Pending` 时，`send_to` 要等到引擎 15 秒的 `SEND_TIMEOUT` 到时才完成（数据报早已由读端推了出去，那条流后面的数据报都等着；引擎这一段读代码得出），计划原先写生产碰不到这一类，不对，计划「执行期修正记录」第 15 行订正；二、三个平台上 SS 2022 对 sing-box 与对 `ssserver` 的两个互操作用例，收 UDP 回包的缓冲只有 64 字节——`c048315`；三、Linux 上 `snell` 对 snell-server 的 UDP 与 obfs 两个用例，snell-server 的双栈 socket 把 IPv4 的来源写成映射的形式而用例比较时没有归一——`2519b3f`；四、Linux 上 `masque` 对 masque-server 的 TCP 用例，masque-server 每条 TCP 隧道只排 1 MiB 客户端的字节（参照服务端的限制，用例节制上行，登记在兼容性清单 4.2 节）——`0deba9f`；五、macOS 上 `udp` 的三条与 `udp_tls_family` 的一条，macOS 上 UDP socket 的缺省发送缓冲 9216 字节装不下测试发的大数据报——`f9377a8`（rurge 自己的 SOCKS5 UDP 中继 socket 同样如此，没有改，登记在计划「延后事项」第 7 行）；六、Windows 上 `rurge-api` 的五条用例，`MockDns::start` 找不到同时空闲的 TCP 与 UDP 端口（原因没有证据，现在两种绑定次序轮流试，找不到时说出两种绑定各自最后的错误）——`94cfd9e`。**还不知道的**有四项：这些修正有没有用（本机无从验证第 2 ～ 6 类）；macOS 与 Windows 上没有跑到的四个包的测试和 `rurge-proto` 单元测试其余用例的结果（这两项要再推送一次才知道）；Windows 上 `MockDns` 为什么找不到端口（再失败时 panic 的文本里有两种绑定各自最后的错误）；以及通过的互操作用例定不下来的几处（PSK 或版本不符时 sing-box、mihomo 与 snell-server 是关闭还是重置连接，mihomo 启动时联不联网，masque-server 与 TrustTunnel endpoint 在 CONNECT 的 2xx 之后会不会再发 HEADERS；任务日志要登录才能读，公开的只有注解与摘要，见 `tests/interop/README.md` 各服务端一节的更新）。**此前各段里"第一次 CI 运行时核对""还没有对真实的二进制跑过"的说法，凡这次运行跑到了的——Linux 上全部测试二进制，macOS 与 Windows 上排在 `rurge-proto` 单元测试之前的各包，阶段 3 / M2 不在里面——，以这一句为准**，那些历史的段落不逐处改。
M8a 任务 6 最后一个代码提交 `e499061`（失败摘要的脚本；改 Rust 代码的最后一个是 `f0e41fd`）上的全量运行（自那以后只有文档变过）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 干净，`cargo test --workspace --no-fail-fast` 77 个结果行、3810 个通过、0 个失败、10 个忽略，没有哪个测试二进制以 `STATUS_HEAP_CORRUPTION` 或 `STATUS_ACCESS_VIOLATION` 退出——一台 Windows 11 x86-64 的开发机上的一次运行，不是 CI；需要参照服务端二进制的互操作用例在这台机器上立刻返回，算在通过的里面。
写这段时 `origin/main` 是 `7bd6589`；阶段 3 / M2 的提交和这一轮的修正都在它之后，没有一个被 CI 见过——读到这里先 `git rev-parse origin/main`，推送之后这句话就过时了。

### 这台开发机

本机测试二进制偶发的 `STATUS_HEAP_CORRUPTION` 已查明来自开发机的终端管控软件注入的钩子 DLL，不是 rurge 或其依赖的缺陷（M8 设计 3.5），遇到就重跑那个二进制。
rustc 1.89 在增量编译里出过内部编译器错误（阶段 1 补遗（M5b）里一个任务把 API 监听器的类型来回改之后，在依赖图里对一个异步函数做 `Send` 检查的类型处；那个任务里遇到两次，此外没有再见过）：删掉出错的那一个编译目标（crate 或测试目标）在 `target/debug/incremental/` 下的目录（可再生的缓存）再编译就过去了（两次都停在同一处，两次都是删掉缓存就过去；只在这台机器上见过，原因没有查）。
开发机的其它事实：回环的 TCP / UDP 7、9、13、17、19 号端口开着（Windows 的 Simple TCP/IP Services）而 CI 上关着，所以测试不能假定任何固定端口的状态，关着的回环端口在 Windows 上约 2 秒才拒绝连接（实测 2.03 ～ 2.05 秒）、在 Linux / macOS 上立刻拒绝；`target/` 长得很快（聚焦构建的 `-p` 会另编一份依赖，两天攒到过 145 GiB，写满过 D: 盘），跑全量门禁前先 `df -h /d`，剩余不足约 15 GiB 就停下；Windows 记在进程名下的 CPU 时间在这台机器上不可信（一个计算 8 秒的线程只被记约 0.1 秒），别凭任务管理器判断是不是在空转。

### M8b

M8b（P2 协议项与 `W0007` 的归宿）已完成——`rurge-config`：`spec::Remark` 与 `SpecOutcome.remarks`（"策略照建、每次加载按种类说一次"的提示，与"不建策略"的 `NotImplemented` 并列：`VmessAead` 是 `W0029`、`SsStreamCipher` 是新码 `W0031`；主配置里每种每次加载一条，订阅导入的行每个组、每种一条，行有错误时不给）；没写 `vmess-aead=true` 的 `vmess` 行照样用 AEAD 握手（`NotImplemented::LegacyVmess` 去掉，旧握手不实现）；`SsMethod` 的 11 个流式取值（`iv_len` / `is_stream`；手册不再列出的 9 个方法名是 `NotImplemented::SsCipher`，未知方法的错误列出全部 19 个名字）；`SnellVersion` 的 V1 ～ V3（`is_chunked` / `carries_udp`；不写 `version` 是 V1，`reuse` 存生效的值——v2 恒真、v1 / v3 恒假，写反了 `W0028`——，v1 ～ v3 接受 `obfs=tls`，`NotImplemented::SnellVersion` 只剩 6）；`W0007` 的三句文本（`tailscale` 的一句指出经 tailscaled 的 SOCKS5 的写法，`snell` 的 `version=6`，`ss` 的旧方法名；导入行是 `imported policies of type … are not supported; they behave as REJECT`）；`mode` 写在低于 6 的 `snell` 版本上是 `E0018`（原来是告警并忽略），`ecn` 写在不走 QUIC 也不是 `wireguard` 的策略上是每行一条的 `W0028`（取值置回 `auto`），Hysteria 2 的混淆口令短于 4 字节是 `W0012`，`ss` 的 `udp-port` 没有 `udp-relay=true` 时是 `W0028`；`rurge_proto::shadowsocks::stream`（`Keystream`：手写的 AES-CFB128、AES-CTR（128 位大端计数）与 RC4，`chacha20` 0.9 与 `salsa20` 0.10 两条新的依赖边、没有新的包，以 NIST SP 800-38A、RFC 6229、RFC 8439 与 ECRYPT 的向量钉住；`StreamKey`；`CipherStream`：我方的 IV 与首段密文同一次写出、一次写至多 16 KiB、读时先收齐对端的 IV，`rc4` 没有 IV；`seal_packet` / `open_packet` 与 UDP 的 `Packets::Stream`；`chacha20` 与 `chacha20-ietf` 的密钥流在每个方向 256 GiB 少 64 字节处到头，以 `ss: the cipher's keystream is used up` 失败而不 panic；服务端一个字节没回就结束或重置的两句文本与 AEAD 方法共用）；`rurge_proto::snell` 的 `chunked`（`ChunkedStream`：Shadowsocks AEAD 式的分块，v1 用 Argon2id 的全部 32 字节作 ChaCha20-Poly1305 的密钥、v2 / v3 取前 16 字节作 AES-128-GCM 的；零长度块只有密封的长度那一段）、`record` 的 `Format` 与外壳 `SnellStream`（原来的实现改名 `V4Stream`）、`kdf::derive`；v1 / v3 发 Connect、不复用、以 TCP 的写关闭结束，v2 发 ConnectV2、总是复用，UDP 自 v3 起（v1 / v2 的 `udp()` 是 `Unsupported`）；"没有应答"的文本对所有版本改为三句——服务端一个字节没发就关闭或重置时带 `(wrong psk or version?)` 的提示（`poll_salt`；重置是新的一句），它的 salt 已到时是不带提示的那句；`rurge_proto::testing` 的 `FakeShadowsocks`（流式方法，密钥流与出站共用 `Keystream`）与 `FakeSnell`（`version` 1 ～ 5、`obfs_tls`、`reset`；v1 ～ v3 上目标一个字节没回就结束时回 `Remote EOF`），这两个假服务端只连、只发往 127.0.0.1 与 ::1 本身（`is_localhost` / `connect_upstream`）；经引擎的端到端用例（`tests/outbounds_shadowsocks.rs`、`tests/outbounds_snell.rs`、`tests/outbounds_tls_family.rs`）与 `rurge check` 的诊断用例；`tests/interop` 的 `rurge_interop::mihomo`（mihomo v1.19.32，GPL、只作为外部进程运行，`RURGE_TEST_MIHOMO`，三平台 CI 新装；就绪要经一个额外的 `socks` 监听端真的转发成一次）与 `tests/mihomo.rs`（Snell v1 ～ v3、v2 的复用由 mihomo 的日志证明、v3 的 UDP、PSK 或版本不符、`ss` 的 `chacha20`、两种协议前面的 simple-obfs）、`tests/shadowsocks.rs` 对 `ssserver` 的九种流式方法、`rurge_interop::free_ports`（一次拉起的端口一次取齐）——这些互操作用例在本机跳过，还没有对真实的二进制跑过，第一次 CI 运行时核对；`salsa20` 没有参照服务端；注释里把 `W0007` / `Unsupported` 说成"还没实现"的地方改为"不实现"。

### M8c

M8c（遗留事项、报告与验收）已完成——计划 `docs/superpowers/plans/2026-10-09-phase2-m8c-leftovers-acceptance-plan.md`（任务 1 ～ 12，执行中新加任务 13、14、15；执行期修正记录 96 行，延后事项 105 行）：任务 1（QUIC 的接收窗口与 `masque` 经载体的起步包长）：不设流数上限的 QUIC 会话（`masque`、`hysteria2`、`tuic`）每条连接的接收窗口 256 MiB（`Transport::STANDARD`：每流 8 MiB 的 32 倍），`Transport::DNS` 与带上限的会话（`capped_transport`）各用自己的 20 MiB 常数；256 MiB 是上界不是预留（一条连接占 min(256 MiB, 8 MiB × 有未读数据的隧道数)，一个 QUIC 策略除此之外没有自己的上界，19 条停住的隧道就超过 NFR-01 的 300 MB 在 1 万个会话的转发缓冲之后剩下的，由项目所有者决定）；quinn（quinn-proto 0.11.19）攒到窗口的八分之一才公布一次连接级额度，所以要 29 条读端停住的隧道（各攒满 8 MiB）才拖住其余的；`trust-tunnel` 的 `h3=true`（带上限）的窗口因此改为 ⌈`max-streams` × 8 MiB × 8/7⌉、至少 20 MiB（缺省 `max-streams=3` 时 28,760,942 字节；窗口是 `u32`，`max-streams` 448 起饱和在 `u32::MAX`，"其余隧道都停住时最后一条仍在读的照旧收到数据"这条保证到 449 为止成立）；`masque` 经 `underlying-proxy` 的连接（链式载体上 QUIC 不做 PMTU 探测）从 1350 字节的包起步（`Transport::carried_initial_mtu` / `Transport::apply_carried`、`Path::is_carried()`；`rurge-proto-quic` 的 `Dialer` 留直连与经载体两份 `quinn::ClientConfig`，TLS 部分只构造一次），quinn 的黑洞检测要四批只丢大包（不是三批）才把 MTU 退回 1200，回落是每条连接各自的、此后不再升回（窄路径上每建一条连接先丢一轮满包，回环上量到 0.19 ～ 0.23 秒）；缺省 `mtu`（1280）的 WireGuard 带不动 1350 字节的数据报，分片、整包送出还是被丢弃只读了 smoltcp 0.12 的源码，没有实测，内层 QUIC 经 `masque` 的真实握手也没跑过（用例只量了一个 1250 字节的 UDP 包往返），手工清单 M8c 一节有这一项。
任务 2（载体的错误，M7c 延后事项 #28 ～ #32）：`Carried::try_send` 第一次收不下时问一次 `poll_send_ready` 再试，第二次之后载体仍说发不了的留给 quinn 等（不丢）；接收一侧载体的结束原因同样交出，载体有自己的话（自己的文本或系统错误）才留作原因，载体自己因 `ConnectionReset` 而结束时（跑在一条流上的 UDP 载体 `trojan`、`anytls` 被服务端重置连接就是这样）以 `ConnectionAborted` 加同样的文本交出；握手丢了 endpoint 的两种 quinn 错误（`DRIVER_LOST` 的传输错误与连接驱动退出时改写出来的 `LocallyClosed`）读作同一句 `quic: the UDP path to the server failed`；链上载体打开之后报出的错误带 `via <策略>: ` 前缀（`rurge-policy` 的 `Relayed`，策略是 `underlying-proxy` 写的那个）；载体报出 `ConnectionReset`（Windows 上改走直连的载体收到 ICMP 端口不可达）不再结束内部 UDP 会话、不进请求记录、不给 `smart` 的成员记失败。
任务 3（DNS）：没写路径的 `https://` 上游发往 `/dns-query`（判据取自写出来的文本：`https://` 之后直到结尾没有 `/`、`?`、`#`，也数 `\`；`UpstreamSpec::Https { url, pathless }`、`DohUpstream::new(url, pathless, http)`；名字是规范形式，没写路径的显示 `https://dns.example`；自己托管、在 `/` 上应答的服务端从此必须明写 `/`），`Age`（RFC 8484 5.1）从应答每条记录的 TTL 里减去（按 RFC 9111 5.1 取列表的第一个成员，OPT 不动，`message::lower_ttls` 在报文原处改写，`doh::answer(headers, body)` 是 DoH 与 DoH3 收下应答的一步；减到 0 的应答这一次照用、不进缓存）。
任务 5（AnyTLS 的 SYNACK 看门狗）：复用的会话（流号 ≥ 2）上、对端版本 ≥ 2（取自服务端设置里的 `v=<n>`，没有按 1）时，一条新流 3 秒内没有得到自己的 `cmdSYNACK` 就关会话：读写以 `anytls: the session is closed` 结束，池丢掉这条会话，一行 `debug`：`anytls: no answer to a new stream on a reused session; the session is closed`；请求不重试，3 秒的失败出现在转发阶段（请求记录的 `error`），不是 502；与参考客户端相同的后果：服务端拨通目标之后才回 `cmdSYNACK`，目标 3 秒拨不通同样触发，UDP 流的看门狗在 `open` 时起表，"开了、没发"的载体会让所在的会话在 3 秒后被关；**没有对真实服务端验证过，第一次 CI 运行也验证不了**（互操作用例在几毫秒内放掉每条流）。
任务 6（WireGuard 的收尾）：隧道只有一种结束方式 `Ending::{Superseded, Closed}` 与 `Shared::end`，三个入口（被更晚的配置接替；设备任务以任何方式结束，panic 也算；协议栈的锁中毒），等待者以 `wireguard: a newer configuration of this tunnel is in use` 或 `wireguard: the tunnel has closed` 结束，下一个连接起一条新隧道，隧道上已有的 TCP 连接读到 reset、写得 broken pipe，UDP 载体（含还没发过数据报的）当场结束；意外结束的隧道在下一个连接时记一条 warn（`wireguard: the tunnel ended unexpectedly; the next connection starts a new one`）；载体的两把小锁中毒不再 panic；假对端沉默时不再空转。
任务 7（指向构建失败成员的中继；出站一侧的 IDN）：成员全都挂在被略去的中继上的组成了空组，告警字段 `group`、`count`、`members`（至多列 20 个名字）；以 Unicode 写的 `server` 转 A-label（`rurge_config::hostname::to_ascii(name: &str) -> Option<String>`，`rurge-proto` 的同名函数转调它；ASCII 的 `server` 不经它）：没有 ASCII 形式的 Unicode 名（含全是点的）是 `E0018`，ASCII 字母表之外的名字要用作 TLS 名字或 WebSocket `Host` 时是构建错误 `E0022`、否则到第一次拨号才失败；`[WireGuard]` peer 的 `endpoint` 不转换；`Config::proxy_hostnames()` 两种写法都在。
任务 8（坏配置时的系统代理恢复）：`rurge run` 在配置读不了、配置有错误、配置能加载但 `--log-file` 打不开三种情形下，先恢复上一次崩溃留下的系统代理设置（`recover_without_a_log`；`SystemProxyManager::recover()` 返回 `Option<Result<(), String>>`；`RuntimeArgs::data_dir()` 取数据目录而不创建）再以 2 退出；必须拿到数据目录的实例锁，拿不到（别的实例活着）不碰备份；`state.json` 只读着看（`StateStore::open` 会把解析不了的文件悄悄改名成 `.broken`）；真实的系统代理后端从没在这条路径上跑过，阶段 1 手工清单有对应项。
任务 4（`smart` 的失败计分）：一个失效的站点一小时内只罚一次，不论经哪个成员、同时来多少条会话——记下一次失败的那一刻，站点记忆里没有任何未过期的失败记录（别的成员的，或回报者自己上一次在这里的）才另算成员自己的失败，"看"与"记"是同一次调用、同一次加锁，一小时从最后一次失败算——；N 个不同的失效地址仍罚 N 次，站点记忆按策略名记（站点数超过 4096、一个站点超过 16 个策略或重载去掉了策略时，同一站点一小时内可能再罚一次）；目标发不出去（域名经 `to_ascii` 转不了，或转出来超过 255 字节）时引擎把这条会话整个当作不经这个组（`unsendable`：不回报、不换成员、不记首字节与使用计数，TCP 与 UDP 相同）。
任务 9（转发循环的 flush 与缓冲）：`http` 与 `socks5` 出站的握手每一步写完先 flush 再等应答；`http` 把代理在 CONNECT 期间关了或重置连接统一说成一句 `http proxy closed the connection during CONNECT`，下一跳自己带话的失败原样通过，`response header larger than N bytes` 只在响应头真的超长时说（`transport::head::is_too_long`）；转发循环保持每次写后 flush、缓冲 8 KiB（`BUF`，私有的 `pump_sized`）：基准 `relay_throughput`（忽略项）量出 flush 的时机没有差别，缓冲越大越快，按两遍 16 轮合计 64 KiB 过了设计的判据线而缺省没有改——每条会话从建立到结束拿着两块缓冲，1 万条会话 8 KiB 是 156 MB、64 KiB 是 1.22 GiB，对着 NFR-01 的 300 MB 不行——，自适应的缓冲（从 8 KiB 起、读满时才换大）移交阶段 3。
任务 10（小项）：IPv4 映射地址（`::ffff:a.b.c.d`）在入站每道门（SOCKS5 的目标、HTTP 的 `CONNECT` 与绝对 URI、双栈监听上 TCP 客户端自己的地址）与引擎 UDP 流水线的 `receive()` 比较出站报回的来源时按 IPv4 处理（比较来源的另外三处——`packet_datagram` 的接收任务、`udp_probe`、SOCKS5 UDP 与 Shadowsocks UDP 共用的 `from_relay`——是最终修正波补的），目标地址与来源归一的唯一实现是 `HostName::canonical`；`CORE_VERSION` 报告 22（`check_reports_core_version_22`）；已经报错的参数不再多一条告警（`ParamReader::{error_on, rejected, has_unrejected}`，加新的"不适用 / 没有效果"告警的人先问 `has_unrejected`；`wireguard` 上合法的 `tfo=true` 现在只得 `W0028`）。
任务 11 与 14（测试稳固；不依赖本机的 9 号端口）：要先于主人绑定就写进配置的端口，或必须一直关着的端口，取自 20000–29999 并现场试绑（`closed_tcp_port()` / `closed_udp_port()`、`tests/external` 的 `free_port()`），不用 `bind(0)` 再放掉，也不用"绑了不听的 TCP socket"（macOS 默默丢掉 SYN）；数问题的用例用不重发的 `FanoutOpts`；对守护进程做事之前先等它说准备好了；一次拉起多个端口的互操作夹具一次取齐（`free_ports`）；`Failures` 有 Drop 守卫（记过失败而没有 `finish` 就被丢掉时当场让用例失败）；`rebind_failure_leaves_no_listeners_and_a_later_rebind_recovers` 偶发的 `os error 10048` 的原因没能证明，再出现时最可能是临时端口耗尽、是机器的状态。
任务 15（解析器空转，一个自阶段 1 起的产品缺陷）：`rurge_dns::fanout::resolve_name` 在"还有轮次没发、时钟已过了查询的截止时间、又没有有效应答"时没有出口，不停空转（一个核烧着，这次查询永不返回，同名的后续查询挂在它的 in-flight 条目上，只有重启才停；进程被停住再继续、机器睡眠而平台的单调时钟把睡眠算进去（Windows）、运行时被饿住时触发），一个条件修复；把握：与 M3a 延后事项 #20 记下的空转相符，但那次运行没有留下日志，不能证明就是它，也没有证据表明还有第二个；`rurge-dns` 里依赖墙钟的用例改走暂停的时钟与内存里的上游（`MockDns::upstream()`，不带截断与 TCP 重试；`resolver_in_memory` 只换 `primary`）；AAAA 抑制的计数在阶段 1 补遗（M5b）里改成了手册说的"连续 5 次查找"（`a72b5e9`、`4a0e0a9`；代项目所有者作的裁定，所有者仍可改回）：一个超时按发出的那次查询数一次、由发出它的调用方数——为同一次查找补问 AAAA 的后台查询再超时不计，同一时刻同名的查找共用一次查询、只计一次——；名字一个接一个地查（前一个的补问也结束之后再查下一个）时，第五个名字的查找把 AAAA 关掉，从第六个名字起不再问 AAAA，DNS 缓存刷新（`POST /v1/dns/flush`）、Profile 的重载（建一个新的解析器）或重启让它回来（用例 `aaaa_is_turned_off_by_the_fifth_lookup_in_a_row_and_a_flush_turns_it_on`、`five_lookups_of_one_name_at_once_count_one_aaaa_timeout`；记录第 104 ～ 106 行，延后第 95、96 行）。
任务 13（十个小项）：`rurge_proto::keystore::decode_base64`（`rurge-proto-ssh` 与 Keystore 共用，不带填充的 Base64 也收）；`with_params` 按解析器读这一行的方式取键（类型、服务器与端口三个字段不取键，行尾留着未闭合的引号时追加的修饰写在它前面；整项带引号的 `"k=v"` 当时算同名参数、没配上的括号算未闭合，一行怎样拆成各项那一轮订正了这两处：整项带引号的是值、没有键，括号不分组）；旧资源管理器的 URL 任务随管理器结束（`gone`）；VMess 与 `h2-connect` 的 UDP 连接被服务端一建成就关时一个数据报至多再拨一次；`General` 手写 `Debug`；`russh` 的日志只留 WARN 及以上（`quiet_dependencies`）；`test-timeout` 写得再大也不再 panic；外部命令失败的报错文本里命令行参数中最后一个 `@` 之前换成 `***`（`rurge_platform::command::failure`）；六处凭据曾进输出而封上了：`[General] geoip-maxmind-url`（MaxMind 的下载链接带账号的授权密钥）、`proxy-test-url` / `internet-test-url`、`#!MANAGED-CONFIG` 的 URL、组的旧参数 `url=`、`--help` 打印三个环境变量的当前值、GeoIP 更新器的日志（现在按数据库的文件名称呼）。
任务 12 的文档：兼容性清单（开头一句与 1.3、1.4、3.2、3.3、3.5、5.2 节各行的"目前"备注）、需求文档第 7 节的状态行、两份 API 参考、两份手工清单（阶段 2 的 M8c 一节 6 项；阶段 1 加两项共 4 行）、阶段 2 遗留事项总表 `docs/superpowers/plans/2026-10-09-phase2-leftovers.md`（20 份计划的 403 行逐行标去向：已做掉 80 / M8 做掉 64 / 关闭 73 / 移交 186）、M8 设计第 13 节（25 条订正）、验收文档 `docs/acceptance/phase2-acceptance.md`，以及 Snell 的 `udp-port`（手册只说它是进行 UDP 转发时用的另一个服务端端口号，"v5 的 QUIC Proxy Mode 的端口"是 rurge 自己的推断、没有来源，已从清单和四处代码注释里去掉；行为不变：只解析，`W0029`，UDP 走主端口）。
终审与最终修正波：整分支终审（只读代码、什么也没运行）的结论是"可以合并，要修"，没有 Critical，两项 Important 都早于本分支——脱敏认秘密参数的边界比解析器窄（自 M8a）、`update-interval=0` 与没有缓存的 304 让 URL 任务不歇地重取（自阶段 1）——；随后九个提交（`ab91157` ～ `087f399`）分两部分。
第 2 部分：规则集与域名集的 `update-interval=0` 读作没写，按缺省的 86 400 秒刷新（`ResourceManager::register` 把 0 滤掉；**策略组上的 `update-interval=0` 是加载错误 `E0018`**——`crates/rurge-config/src/spec/group.rs` 的 `read_import` 要求正数——，两种读法不一致，由项目所有者决定），没有缓存时服务器答 304 算一次失败的抓取；自动刷新关闭的 URL 任务每分钟醒一次、只看间隔而不抓取；每秒一次的流量采样的定时器取 `MissedTickBehavior::Delay`；一代要么整个发布要么不发布（`publish_generation` 里会失败的都在第一次 store 之前）；比较出站报回的来源的四处都先归一成 IPv4（任务 10 只做了第一处）；`rurge-dns` 里剩下的计时用例走暂停的时钟。
第 1 部分（`rurge-config`，项目所有者的规矩"凭据不进日志、错误文本、API 输出与 `Debug`"）三轮加三处窄化：脱敏（`crates/rurge-config/src/redact.rs`）先照节解析器的顺序剥去注释与指令再读一行——被注释掉的行和行尾的注释照"没有被注释"那样脱敏，注释里的链接只显示 scheme 与 authority——，整行键、位置凭据、URL 里的用户信息、冒号作分隔符、键前面另有文字的行都照各自解析器的读法认；清单是 `SECRET_KEYS`（14 个）、`SECRET_PARAMS`（24 个）、`CONTROLLER_KEYS`、`LISTENER_KEYS`、`DNS_KEYS`、`READ_SCHEMES`（6 个 URL scheme），判断是 `is_secret_key`；URL 里的用户信息在脱敏后的配置之外也隐去（`ResourceRef` 的 `Debug` 与 `ResourceRef::shown_url`、规则的显示文字 `Rule.raw` / `SubRule.raw`、`HostEntry.raw_key`、`Entry::log_name` 经 `ResourceSource::shown()`、规则集注册表的 `Key::display()`）；不是名字的名字（`is_name`：含 `://`、`=`、全角 `＝` 或 `，`；`=` 是一行怎样拆成各项那一轮加的）不在诊断里引用；这些诊断不再引用用户写的东西：不像键名的未知 `[General]` 键、不像规则类型的规则类型、规则与组里的未知策略、不是名字的名字、`[Keystore]` 与 `[WireGuard]` 引用里的无名者、滑进别的参数里的取值；`[Host]` 下含 `://` 又不是集合键的键成了 `E0017`（行为变化，代项目所有者裁定）。**三件以后的会话必须知道的事**：一、**脱敏后的视图是一张"藏什么"的清单，尽力而为，不是没有秘密会出现的保证**——两次对抗性复审各在普通配置里找到了仍然露出秘密的行，第二位复审者的判断是对这种格式这样的清单做不成可靠的；第三轮之后代项目所有者裁定不再追，没有第三次复审；已知没有覆盖的写在计划「延后事项」第 58 ～ 77 行，要不要换成只显示已知无害内容的视图是项目所有者的未决问题（第 51 行）——以后的会话不要自己去追更多的拼法，也不要把这个视图说成可靠的；二、**新的带凭据的字段或写法要写进 `crates/rurge-config/tests/secrets.rs`**（命令一节里也这么说，两处保持一致）；三、**节头后面跟注释不是节头**（`[MITM] # note`：下面的行归给上一节，作者看到一串不相干的错误；Surge 怎么读不知道，建议读成节头并给告警，由项目所有者决定，第 52 行）。
M8 余下的是看 M8a 的任务 6 对第二次运行的修正管不管用（读出的六类失败里五类已修，第 6 类换了找端口的做法并补了诊断、原因仍不知道，项目所有者再推送一次才知道）；阶段 2 的验收没有完成（CI 没绿过；互操作用例对真实二进制的结果，读得到的只有运行 39022550224（`7bd6589`）这一次；手工清单——阶段 2 的 187 项与阶段 1 的 46 行——没做），逐条的证据见 `docs/acceptance/phase2-acceptance.md`。

## 手册基线重对（2026-10-14）

手册基线重对（2026-10-14）：把那天取的整本手册（87 页；Surge Mac 6.10.0 / iOS 5.23.0，2026-10-13 发布）逐句对着兼容性清单读了一遍——报告 `docs/reports/2026-10-14-manual-baseline-recheck.md`，设计 `docs/superpowers/specs/2026-10-14-manual-baseline-recheck-design.md`（已决事项 MB-D1 ～ D22 是在项目所有者"自己决定、继续做"的常设授权下定的，所有者可以改），计划 `docs/superpowers/plans/2026-10-14-manual-baseline-recheck-plan.md`（下面写"重对计划延后第 N 行"指它末尾的「延后事项」）。
读的结果：2,757 条里 2,439 条与清单一致（没有人逐条复核），318 条不一致——128 条不引出改动（版本标记、界面、iOS 专属项的细节、误报、清单已经隐含的），190 条引出了改动——；清单写着"不支持"的 46 行逐行复核过，第二遍对着代码读了 47 处，清单里"手册没有写"的 69 行重查过；十一个只读的助手读的，什么也没有运行。
代码改了十一处（括号里是提交与代码所在）：`GEOIP,UNKNOWN` 与 `IP-ASN,UNKNOWN` 匹配数据库里查不到的地址（`9b0bcfe`；`RuleKind::GeoIpUnknown` / `IpAsnUnknown`，`crates/rurge-config/src/rule.rs`；查库的答案是 `rurge_rules::GeoResult` 的 `NotLoaded` / `Unknown` / `Known` 三种之一，`crates/rurge-rules/src/matcher.rs`；库没有载入时两种规则都不命中，库已载入时 `GEOIP,UNKNOWN` 也命中回环与私有网段这些库里没有条目的地址；集合里与别的逐条求值的行按顺序判）；`[IP Rewrite]` 是已知的节（`436ee74`；行原样留着、`W0016`，语义随阶段 3）；旧的 `custom` 类型读作 `ss`（`b809298`、`9a01a32`；端口之后的位置依次是加密方法、口令、模块的链接（读了就丢），命名参数压过位置上的，主配置里每行一条 `I0001`，位置上写错的方法名不被引用——那个位置上也可能是口令）；`tcp://` 的 DNS 服务器只写主机与端口（`24e1841`；多写了别的是 `W0026`、那个服务器不用）；`wireguard` 行上的 `tos` 生效（`31c4245`；隧道的 UDP socket 带策略的 `tos`，握手的发起包照旧 `0x88`；`tos` 不进隧道的键，指向同一个节的几个策略共用一条隧道，标记是起它的那个策略的）；永不匹配的规则类型（`W0013`）在逻辑规则的子规则里也报（`05d2e1c`）；`subnet` 组的 `W0008` 说它在每个网络上都用 `default`（`bd6f61d`）；`use-local-host-item-for-proxy` 从 `[Host]` 的几个地址里每个会话随机交一个给代理（`a0dba4b`；`rurge_policy::random_below`），此前总是第一个；`block-quic = all-proxy` 只盖过代理策略的参数，直连类策略照自己的（`9027daf`；`quic_blocked`，`crates/rurge-engine/src/udp.rs`）；跟随出站模式时 DNS 会话的协议标签按 URL 的方案、不看端口（`42c2294`；`rurge_dns::DnsConnectors`，`PipelineConnector::connectors()` 的五个面各带一个固定的标签：`tcp://` 是 `DNS`、`tls://` 是 `DOT`、`https://` 是 `DOH`、`quic://` 是 `DOQ`、`h3://` 是 `DOH3`，`[Host]` 的 `server:` 同样；此前按端口猜，不在缺省端口上的加密服务器被标错）；手册写了、rurge 本来就做到而没有用例断言的八处行为各补了一条用例，写出来都是绿的（七条在 `5c5fe22`，第八条 `a_dns_session_follows_the_outbound_mode` 在 `745beda`）。
兼容性清单改了 123 处（报告第 5 节的 R1 ～ R123，新增十行，当时 565 行；报告之后定的标签改动另改了 3.1 节 `PROTOCOL` 一行）；阶段 1 的三个数现在是 **2 句没有做完、37 句阶段栏与登记不一致、10 句有意不同**（`docs/acceptance/phase1-acceptance.md`：`GEOIP,UNKNOWN` 那一句与 DNS 连接标签的四句——FR-RULE-08 的 DoH、DoT、DNS 连接与 FR-DNS-04 的 `PROTOCOL` 规则匹配——改记"补上了"）；两份手工清单各加了回环上验不了的项（阶段 2 新的一节两项：`wireguard` 的 `tos` 在线上的 DSCP 位、一行真实的旧 `custom` 策略对真实的服务端；阶段 1 一行：真实的 GeoLite2 库之下的 `GEOIP,UNKNOWN` 与 `IP-ASN,UNKNOWN`）。
没有改、要知道的：`CORE_VERSION` 仍报 22（新的编码归项目所有者，MB-D22）；`GET /v1/profiles/current` 仍回纯文本（手册说应答总是 JSON，随阶段 6 对齐，MB-D21）；一个值中间没有配对的引号或括号会吃掉一行的后半（随后由一行怎样拆成各项那一轮改掉，见下）；组的中继自己也在成员里时派生出 `R (via R)`、没写 `default` 的 `subnet` 组、跟随出站模式时给 DNS 服务器的主机名写规则，都登记在清单与重对计划延后第 2、3、5 行。
这一轮**没有覆盖**：Surge 的实际行为（一处也没有观察，清单里"与 Surge 一致"说的是与手册的文字一致）；2,439 条"一致"；代码只对着读了 47 处；阶段 3 及以后的行上手册多说的细节（重对计划延后第 7 行）；取文本时丢掉的图、链接与表格的行列；知识库与博客（发行说明只读了最近的一页）；两件事实取自第三方（旧 `custom` 行里字段的位置，`GET /v1/profiles/current` 的三个键）；CI 与手工验收照旧。
合并之前的整分支评审（2026-10-14，只读代码与文档，什么也没运行）的结论是改了再合并：没有 Critical，也没有 Important；要改的几处小的在合并之前改了（重对计划「执行期修正记录」第 30 ～ 35 行）——两条新的 `wireguard` 用例加了第二道防线，一条新用例钉住以名字写的 `tls://` / `https://` DNS 服务器交给连接器的是引导解析出的地址（`e2e0a8e`），几份文档里几句过时或说不准的话订正了——，留着没改的登记在重对计划延后第 14 ～ 17 行。
`5446065`（一行怎样拆成各项那一轮的最后一个代码提交之后）上的全量运行（自那以后只有文档变过）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 干净，`cargo test --workspace --no-fail-fast` 74 个结果行、3438 个通过、0 个失败、6 个忽略（两个单元测试二进制在这次全量里以 `STATUS_HEAP_CORRUPTION` 退出，单独重跑通过，计入这个数）——一台机器的一次运行，不是 CI。

## 一行怎样拆成各项（2026-10-14 ～ 16）

一行怎样拆成各项（2026-10-14 ～ 16；设计 `docs/superpowers/specs/2026-10-14-line-splitting-design.md`，已决事项 LS-D1 ～ D13 是在项目所有者"自己决定、继续做"的常设授权下定的，所有者可以改；计划 `docs/superpowers/plans/2026-10-14-line-splitting-plan.md`，下面写"拆分计划延后第 N 行"指它末尾的「延后事项」）：把手册基线重对留下的那件事做掉了——带撇号的名字不再吃掉一行的后半——，解析器与脱敏对"一项在哪里结束"的读法在同一次改动里一起改（共用的扫描与脱敏在 `7c31ab6`，每一项读成什么在 `303a7f1`，`fc5e30e` 与 `34ff473` 是它们各自的修正，`rurge check` 的端到端用例在 `0c7632a`）。
读法（都在 `rurge-config`，没有新的依赖、没有新的诊断码，LS-D12）：引号（`"` 或 `'`）只在**一个值开始的地方**开启引用——一项的开头（这段文字的开头、引用之外的逗号与左括号之后），或自上一个这样的位置以来第一个 `=` 之后，中间可以有空白（LS-D1）——，别处的引号是普通字符，没有伴的引用到这段文字的末尾（LS-D2），伴的规则同从前（LS-D4）；圆括号只在配对时分组，配不上的 `(` 与 `)` 是普通字符，认引号不看括号配不配对（LS-D3）；整个在引号里的一项是**值**，不是参数，`"pa=ss"` 是值 `pa=ss`（LS-D6）；引起来的值只去一次引号、原样保留，里面的引号字符与两头的空白都在，`with_params` 写回时也只引一次（LS-D7）；`=` 左边定义的名字照原样读、不去引号（LS-D11）；行内注释（`text::strip_inline_comment`）与行尾的要求指令（`requirement::find_suffix_marker`）认 `"` 的方式不变（LS-D13）。
代码所在：共用的扫描是 `value::item_spans`（两遍、线性；`wide` 时全角逗号也切但不开启一个值，LS-D5、LS-D10）与 `split_items`，`value::Field` 与 `split_fields` 给出每一项是值还是参数（`Field` 的 `Debug` 是 `Field(***)`），`split_list` 是 `split_fields` 的投影，`unquote` 只去整个在引号里的一项的引号；读 `key=value` 的各处（`policy::parse_policy`、`parse_group`、`with_params`，`keystore`，`wireguard` 的 peer，`spec::group` 的 `external-policy-modifier`，`rule::RuleParams::parse`）改用 `split_fields`，`ParamMap::from_fields` 收 `&[Field]`、`insert` 不再去引号与空白，`policy::is_name` 不认带 `=` 的文字；LS-D8：读位置凭据的类型（`http`、`https`、`socks5`、`socks5-tls`、`h2-connect` 与旧的 `custom`）上，端口之后头两个位置里读成未知参数的一项，`W0001` 不点出它的名字，带一句固定的提示（`spec::reader::CREDENTIAL_HINT`，经 `ParamReader::warn_with_hint`：含 `=` 的用户名或口令要整个用引号包起来）；一个键本身不是普通参数名（小写字母、数字、`-`、`_`，至多 32 个）时从不点名（`written_as_name`；整个加引号的 `key=value` 丢了后面的逗号、粘着下一个参数时，键里带着引号里的那一段）。
脱敏（`redact.rs`）改看**区域**（`regions`，LS-D9）：解析器的一项，连同旧的读法（`next_comma`）从这一项的开头读下去会带进来的后续各项，区域的末尾取两种读法里较晚的一处、再对齐到解析器的下一个切分处（穷举短字符串时找到了新读法比旧读法结束得晚的写法）；旧的读法在脱敏里不只用来量区域：`key_line` 判断 `=` 前面的文字是不是键时只用它（换成两种读法任何一种，`a'b, http-listen = 口令@地址` 就不再是监听地址的键行、口令留在原处），`redact_entry` 判断 `=` 前面有没有逗号时两种读法任何一种看到逗号就算（把这一行当作定义的两种读法都脱敏一遍，`redact_both_ways`）；两遍的次序是先逐项（在原文上：区域的第一项是秘密参数，就把它的值连同并进来的各项一起遮掉，否则在整个区域里按名字找），再位置凭据（在逐项写出的文字上，按解析器的字段数数到第 4 个字段起，不是命名参数的整个遮掉），`password=ab"c,d"` 一类写法脱敏的输出与此前逐字相同（同一行前面没有漏了收尾引号的值时；有的话见下面"还查出几件"的一）。
以后改一行怎样拆分的，解析器与脱敏的读法要在同一个提交里一起改。
使用者看得到的变化（设计第 7 节）：加载得了、以前加载不了的有名字与值里的撇号（`G = select, Tom's HK, Tom's US`、`USER-AGENT,Bob's App*,DIRECT`、`#!include` 的文件名）、名字与正则里不配对的括号（`DOMAIN,b.example,Proxy (US,extended-matching`、`URL-REGEX,^http://a\.example/[^(]*$,REJECT`）、位置上含 `=` 的用户名或口令整个加引号（`http, 127.0.0.1, 8080, alice, "pa=ss"`，旧的 `custom` 行的口令同样）；读法变了的有一个值中间成对的引号不再护住逗号（`password=ab"c,d"`，要护住就把整个值用引号包起来）、整个加引号的 `key=value` 是值（策略行上是多余的位置值 `W0001`，组行上是找不到的成员 `E0008`，peer 与 `[Keystore]` 上少一个字段，规则的参数是一个不认识的参数）、引起来的值原样（`sni="\"a.example\""` 是 `"a.example"`）、不是整个在引号里的一项保留引号字符（`"P1" "x"` 是名字 `"P1" "x"`）、位置凭据处的未知参数不点名而带提示、`external-policy-modifier` 遇到整个加引号的 `"k=v"` 时把它当作一个值留着、把修饰的参数追加在行尾（此前替换它）、单引号里没有转义，作为一行最后一项的 `password='it's'` 此前读成 `it's`、现在读成 `'it's'`（这样写的口令升级之后会变，要写成 `password="it's"`；这两处是终审查出、设计第 12 节补记的）。
凭据：位置上整个加引号、含 `=` 的口令（`http` 与 `custom` 各一）写进了 `crates/rurge-config/tests/secrets.rs`。**没有覆盖的**（设计第 10 节与拆分计划「延后事项」）：行内注释与行尾指令认 `"` 的方式（LS-D13，第 1 行）、`[General]` 一类键值行整个取值带引号的不去引号（`loglevel = "notify"` 得 `W0012`，阶段 1 验收文档已登记，第 2 行）、定义处带引号的名字（LS-D11，第 3 行）、引起来的空项照旧丢掉（`http, h, 80, "", pw` 把口令读成用户名，第 4 行）、位置上不带引号、含 `=` 的口令在脱敏后的视图里照旧显示（设计 6.4；脱敏后的视图本来就是一张尽力而为的清单，换成只显示已知无害内容的视图是项目所有者的未决问题，第 5 行）、Surge 怎样读这些行没有观察过（清单照旧 🟡，第 6 行）；第 7 ～ 11 行另有五处小的。
做这一轮时还查出几件。
一、脱敏在一类写坏了的行上比此前藏得少（第 12 行）：同一行前面某个值在值开始的地方开了引号而漏了收尾（服务器字段、`sni=` 一类参数，或误打的一个引号），后面又有一个含逗号、加了引号的秘密值时（整个加引号，或值中间带一对引号，如 `password=ab"p1,p2"`），逗号之后那一段留在脱敏后的视图里（`GET /v1/profiles/current?sensitive=0`、`GET /v1/policies/detail`，`lineHash` 随它变）——`trojan, h.example, 443, sni="a.example, password="p1,p2", ws=true` 留下 `p2"`，`trojan, "h, 443, password="ZQ1,ZQ2", ws=true` 留下 `ZQ2"`，一行里三个以上多余的引号字符时另有几种；成因在设计层面（位置凭据那一遍读的是逐项那一遍写出的文字，秘密值换成 `***` 时带走了一个引号，引号的配对随之变了），整分支终审的对照里各插一处误打的 40 万行中这样少藏的 147 行、多藏的 47,429 行；代项目所有者裁定这一轮不改代码，要不要收口（在原文上按解析器的字段数决定位置凭据，再落到逐项输出里对应的区域上）由项目所有者定。
二、读位置凭据的类型上，整个加引号、以秘密参数名加 `=` 开头的口令（`"token=abc123"`）脱敏后显示成 `token=***`，露出这个前缀（第 18 行）。
三、值中间成对的引号不再护住逗号之后，被切开的前半可能落进不是凭据的位置：`http-listen = ab"p1,p2"@127.0.0.1:6152` 的地址错误引用 `ab"p1`，`dns-server = https://u:"p1,p2"@…` 的 `Debug` 显示 `https://u:"p1`（第 19 行）。
四、一个此前就有的泄漏——`[General]` 里漏了名字后面的 `=` 的策略行被当作未知键存成小写，它的 `Debug` 显示口令（前半是小写的），标记用例大小写敏感，没有看到它（第 13 行）。

## 阶段 3（增强模式 / TUN）

### 接下来的工作、阶段 3 的总设计与 M1（网络环境与子网）

接下来的工作依次是：一、项目所有者再推送一次 `main`（带上第二次运行读出的六类失败的修正，和阶段 3 / M2）：那一次运行会显示这些修正有没有用，并给出 macOS 与 Windows 上排在 `rurge-proto` 单元测试之后的 `rurge-proto-quic`、`rurge-proto-ssh`、`rurge-proto-wireguard`、`rurge-rules` 四个包的测试的结果，读它的失败清单并修掉（M8a 的任务 6 的后续）；二、阶段 3（增强模式 / TUN）：总设计已写好（2026-10-16，`docs/superpowers/specs/2026-10-16-phase3-enhanced-mode-design.md`，已决事项 P3-D1 ～ D24 是代项目所有者作的裁定，所有者可以改；开始时把阶段 3 那几节对应的手册页对着清单重读了一遍——重对计划延后第 7 行——，报告 `docs/reports/2026-10-16-phase3-manual-recheck.md`；开源实现与平台接口的调研 `docs/reports/2026-10-16-phase3-tun-survey.md`），六个里程碑依次是网络环境与子网（M1）、进程识别（M2）、内存设备上的 TUN 内核（M3）、平台接入（M4）、识别与阻断与 `[IP Rewrite]`（M5）、收尾与验收（M6），M1（网络环境与子网）已做完（2026-10-17；设计 `docs/superpowers/specs/2026-10-16-phase3-m1-network-subnet-design.md`，已决事项 M1-D1 ～ D16 是代项目所有者作的裁定，所有者可以改；计划 `docs/superpowers/plans/2026-10-16-phase3-m1-network-subnet-plan.md`，下面写"M1 记录第 N 行""M1 延后第 N 行"指它末尾的「执行期修正记录」与「延后事项」两张表）。
做了什么（括号里是类型或函数与文件）：`rurge_config::network`（`NetworkFacts`、`Primary`、`WifiLink`、`NetKind`，`NetworkCell` 是一把 `RwLock<Arc<NetworkFacts>>`，`crates/rurge-config/src/network.rs`）与 `SubnetExpr::matches`（`crates/rurge-config/src/policy.rs`）；`rurge_platform::netenv`（`crates/rurge-platform/src/netenv/`：`Snapshot`、`read`、`read_quick`、`hints`；Windows 经 `ipconfig` 包与 `netsh wlan show interfaces`，Linux 读 `/proc/net/route`、`/proc/net/ipv6_route`、`/sys/class/net` 与 `nmcli` / `iw`，macOS 经 `route -n get`、`networksetup -listallhardwareports`、`ipconfig getsummary` 和一个 `PF_ROUTE` 套接字；三个平台的读取器都在每个宿主上编译、运行时按 `dirs::Os` 选，对着假机器（`FakeHost`）和按公开格式写的文本夹具测，M1 记录第 15 行；Windows 与 Linux 每 2 秒比一次不起子进程的指纹，问 Wi-Fi 的名字要起子进程，只在指纹变了时与有无线网卡开着时每 30 秒问一次，每个子进程限时 2 秒（`run_with_timeout`，Windows 上超时连子进程一起结束）；`rurge-platform` 不依赖别的 crate，所以快照用平台自己的类型、由可执行文件映射成 `NetworkFacts`，M1 记录第 1 行）；可执行文件的网络任务（`crates/rurge/src/cli/network.rs`：建第一代之前先读一次，最多等 1 秒——读得更慢就先不知道网络、读完再当作变化告知——，之后读取至多每秒一次、一阵提示至多花两次读取，变化经 1 秒的首发加尾发去抖，什么都读不出来的网络（没有主网卡也没有 Wi-Fi）不算变化、沿用上一个网络，变化经主循环的 `Command::NetworkChanged` 与重载、API 的命令一次一个地处理）；`Engine::network_changed`（存下新的网络、对正在用的这一代的解析器 `on_network_change`、通知注册表里每个出站——只有 `wireguard` 做事——、作废 `url-test` / `fallback` / `load-balance` 的测试轮次、清 `subnet` 组的临时覆盖；`smart` 不动）；`subnet` 组按当前网络选成员（临时覆盖、第一个命中的条件、`default`），**没写 `default` 的 `subnet` 组现在是加载错误 `E0018`**（此前按空组兜底，行为变化），`POST /v1/policy_groups/select` 对它设置与清除临时覆盖，换网络时清除；`SUBNET` 规则按当前网络匹配（含预匹配与逻辑规则的子规则），`INACTIVE_RULE_TYPES` 去掉了它；`[SSID Setting]` 类型化（`Config.ssid_settings`、`Config::effective_ssid_setting`：一行的表达式写到第一个前面是空白、写成 `<认识的键>=` 的地方为止，几行同时命中取第一行），`dns-server` / `encrypted-dns-server` 按网络替换解析器的两张表（`StackOptions.dns_override`；网络变化让覆盖变了，就用内存里的配置重建一代，不重读磁盘上的 Profile，换上之前像重载那样打印新一代的诊断，M1 记录第 3、13、24 行），`suspend=true` 暂停对系统代理的接管（`Takeover`，`crates/rurge/src/cli/run.rs`；只在内存里，不进 `state.json`；暂停期间 `GET /v1/features/system_proxy` 是 `false`，`POST` 只记下想要的状态，暂停结束时系统代理的实际状态朝两个方向都对到想要的状态；增强模式的那一半随 M4）；`rurge rule match`、`dns lookup`、`dns cache` 各读一次当前网络；只供测试的 `RURGE_NETWORK_BACKEND=file:<path>` 与隐藏的 `--network-backend`（见下面「常用命令」之后的一段）。
API 与日志行见 `docs/api/phase3.md`（没有新增端点；没有任何端点给出本机所在的网络，SSID、BSSID 与网关只在 DEBUG 日志里；读网络的日志行不带本机网卡的地址和系统 DNS 服务器的取值，但这些地址不是在任何级别的日志里都没有：DNS 服务器的地址出现在解析器自己的行里——DEBUG 的并发查询行带 `upstream=udp://<地址>:53` 一类的名字，`ipv6` 关着时 INFO 有一行 `IPv6 DNS server ignored because ipv6 is off`——，会话的日志行带来源地址 `src`；M1 收尾时写的“本机地址与 DNS 服务器的取值任何级别的日志里都没有”说得过了头，阶段 3 / M2 的文档订正了）。
兼容性清单改了 2.3、2.4、3.1、3.2、3.4、3.5、4.1、4.2、4.6、5.1、5.2、6.1、9.1、9.4、10.3、10.4 节的几行（现在 566 行：`SSID:`、`BSSID:`、裸值与 `suspend` 四行从 ✅ 变成 🟡，新增"当前网络指哪块网卡"一行），其中四处清单把自己的读法写得比手册的话更肯定的措辞订正了（`pre-matching` 的"手册原话"、UDP 上 `REJECT-NO-DROP` 的"没有限频"、增强模式的"开关不写在 Profile 中"、`suspend` 的"释放系统代理 / TUN，保留 API"）。
整分支终审之后的修正波（M1 记录第 21 ～ 30 行）的最后一个代码提交 `bc3e43a` 上的全量运行（自那以后只有文档变过）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 干净，`cargo test --workspace --no-fail-fast` 75 个结果行、3635 个通过、0 个失败、9 个忽略，没有哪个测试二进制以 `STATUS_HEAP_CORRUPTION` 或 `STATUS_ACCESS_VIOLATION` 退出——一台 Windows 11 x86-64 的开发机上的一次运行，不是 CI。
此前最后一个任务提交 `ea9353c` 上的全量是同样的 75 个结果行、3620 个通过（`rurge-dns` 的 `resolver` 测试二进制那一次以 `STATUS_HEAP_CORRUPTION` 退出，单独重跑 11 个都过，计入这个数）；更早的一次全量里 `rurge-api` 的测试二进制以 `STATUS_ACCESS_VIOLATION` 退出，单独重跑五次与下一次全量都过，原因没有查明（M1 延后第 12 行）。**没有做的，和不要当成已经有的**：Linux 与 macOS 的读取只对着假的机器测过——读取器的逻辑在开发机上跑过，没有跑过的是它们与真实系统的接触（真实的 `/proc`、`/sys`、`nmcli` / `iw` / `route` / `networksetup` / `ipconfig getsummary` 的输出与 `PF_ROUTE` 套接字），第一次 CI 运行时核对（M1 延后第 1、3 行）；手工验收清单 `docs/acceptance/phase3-manual.md`（Windows 10 项、macOS 8 项、Linux 9 项、与平台无关 10 项，共 37 项）一项也没有做过；SSID 与 BSSID 在 Windows 没有位置权限时（据调研以 SYSTEM 运行时也一样，没有核对）、macOS 14 起的守护进程里、Linux 上 NetworkManager 与 `iw` 都说不出时读不到，那时 `SSID:` / `BSSID:` 条件不命中、不报错，在 macOS 上要拿到名字只能另做一个带位置授权的 App 包，是留给项目所有者的问题（总设计 Q6，M1 延后第 7 行）；含非 ASCII 字母的真实 SSID 没有量过；非英文系统上 `netsh wlan show interfaces` 的 `SSID` 与 `BSSID` 这两个键本身没有见过——开发机（系统语言是中文）上量的时候 Wi-Fi 是断开的，输出里没有这两行，见过的只是断开时就有的键（管道里是 GBK、经 `chcp 65001` 之后是英文；M1 延后第 4 行）；主网卡是默认路由所在的网卡，不分物理与虚拟，VPN 接管默认路由后 `TYPE:` 与 `ROUTER:` 看的是 VPN 的网卡（M1 延后第 16 行）；DNS 覆盖今天只管解析器的两张表，DNS 应答器的转发随 M3，接管暂停的增强模式一半与排除 rurge 自己的虚拟网卡随 M4（M1 延后第 13、14 行）；整分支终审已经做了（只读代码与文档，什么也没运行；没有 Critical；两条 Important——掉线被当成换了网络、完整读取没有限速——和几条次要项在合并之前改了，M1 记录第 21 ～ 30 行），任务评审留下的其余次要项仍登记在 M1 延后第 8 ～ 11 行（那里已做掉的各项标明了）。
M1 记录第 1 ～ 14 行与第 21、22、23、25、27 行是主持这一轮的会话代项目所有者作的裁定，所有者可以改。**三件以后的会话必须知道的事**：一、`GET /v1/policy_groups` 不带当前选择，要看一个组选了谁用 `GET /v1/policy_groups/select`（设计 6.3 节写成了"给出当前选择"，M1 记录第 20 行；以 `docs/api/phase3.md` 为准）；二、起 `rurge` 的测试辅助函数缺省把网络指到一个不存在的文件，嵌套在 `external` 策略里的 `rurge run` 拿不到变量、要带 `--network-backend`（见测试约定）；三、`Engine::network_changed` 总是清缓存、不比较前后的网络，只该在网络真的变了时调，调用方是可执行文件的网络任务。

### M2（进程识别）

M2（进程识别）已做完（2026-10-18；设计 `docs/superpowers/specs/2026-10-17-phase3-m2-process-design.md`，已决事项 M2-D1 ～ D19 是在项目所有者"自己决定、继续做"的常设授权下定的（代项目所有者作的裁定），所有者可以改；计划 `docs/superpowers/plans/2026-10-17-phase3-m2-process-plan.md`，下面写"M2 记录第 N 行""M2 延后第 N 行"指它末尾的「执行期修正记录」与「延后事项」两张表）。
它不需要虚拟网卡：今天经系统代理与代理端口从本机进来的连接就有进程可查。
做了什么（括号里是类型或函数与文件）：`rurge_config::session::ProcessInfo` 多了 `pid: Option<u32>`（`SessionInfo.process` 现在是调用方事先知道的进程：离线的 `rurge rule match --process` 填它，PID 是 `None`；经监听器进来的会话不填）；`PROCESS-NAME` 值的分类（`process_pattern`，`crates/rurge-config/src/rule.rs`）：以 `/` 开头、以盘符加 `:\` 或 `:/` 开头、以 `\\` 开头（UNC）的是路径，同时以 `/` 或 `\` 结尾的是前缀，其余是文件名，与平台无关——**读法的变化**：此前只认以 `/` 开头的路径，写进规则的 Windows 路径一律当作文件名、永不匹配；规则引擎（`crates/rurge-rules/src/`）：`Verdict::NeedsProcess` 与 `EvalCtx` 的进程状态（`ProcessState`），`LazyProcess`、`NoProcess` 与 `RuleEngine::evaluate_with_process`（`evaluate` 与 `evaluate_traced` 照旧传 `NoProcess`）——匹配器回 `NeedsProcess`，引擎等进程、记进 `EvalCtx`、重评这一条规则，一次评估至多问一次进程；`AND` / `OR` / `NOT` 与集合里逐条求值的行的组合规矩同 `NeedsResolve`，**要进程优先于要解析**，预匹配那一遍不等进程——，Windows 的比较（`compile_process_pattern` 与 `process_matches_for` 带平台参数，是纯函数，在每个宿主上测）：文件名与路径都按 ASCII 不分大小写比、路径里的 `/` 当作 `\`、不去掉 `.exe`；内部集 `SYSTEM` 的两条 `PROCESS-NAME` 只在 macOS 上编进集合（`internal_set_source`，M2-D17，集合的文字照手册不变）；`rurge_platform::procinfo`（`crates/rurge-platform/src/procinfo/`）：`Finder`（`owner` 阻塞，`is_local` 从不问系统）、`Query`、`Owner`、`Protocol`，每种协议一份系统表的快照（含 IPv4 与 IPv6 两个地址族的行，记读取开始的时刻；TCP 命中且快照不早于会话开始之前 2 秒就用它，`TCP_GRACE`，UDP 与"找不到"要快照不早于会话开始，其余重读一次），每种协议一把锁、一次读取服务所有在它开始之前开始的会话，同一种表的两次读取至少隔 20 毫秒（`READ_GAP`），PID → 名字与路径每份快照问一次，来源既不是回环也不是本机网卡上的地址（取自 `if-addrs`，至多 2 秒旧，`ADDRESS_TTL`）时不读表，读取失败是"不知道"、每种失败在进程的一生里 WARN 一次；读取器写在 `pub(crate)` 的 `Source` 上，解析的纯函数在每个宿主上编译、测试：Windows（`windows.rs`：`GetExtendedTcpTable` / `GetExtendedUdpTable` 各按两个地址族读一次，先问大小再至多读 4 次，四种行的解析是纯函数，PID 4 是 `System`，路径经 `OpenProcess` 与 `QueryFullProcessImageNameW`），Linux（`linux.rs`：写在很小的文件系统接口 `Fs` 上，`/proc/net/tcp` 一类的解析 `parse_proc_net` 带字节序参数，跨快照的 inode → PID 表，先扫最近找到过的至多 16 个进程的 `fd` 再扫全部，`/proc/<pid>/exe` 去掉 ` (deleted)`、读不了取 `comm`，不用 FFI 也不用 netlink），macOS（`macos.rs`：`sysctlbyname` 读 `pcblist_n`，纯函数 `walk` 不按记录大小切而按每一项自己的长度与种类走，偏移取自 XNU 的 `xinpcb_n` 与 `xsocket_n`，取 `so_last_pid`，路径经 `proc_pidpath`）；**`rurge-platform` 里放开 `unsafe_code` 的函数现在是七个**（见下面「计划中的架构」一节的 `unsafe_code` 条），名单由 `crates/rurge-platform/tests/unsafe_functions.rs` 钉住；`rurge_inbound::ProcessSlot`（`crates/rurge-inbound/src/process.rs`：`OnceLock` 加 `Notify`，只落定一次，`settled_by(期限)` 先登记等待再看值、不丢唤醒）与 `SessionHandle::new_with_slot` / `process()`（`new` 与 `new_with_token` 的签名不变，得到"不知道"的槽）；`rurge_engine::{OwnerLookup, OwnerQuery}` 与 `EngineShared.owners`（`None` 什么也不查，缺省与现有的全部用例都是它）；`Engine::look_up_owner`（在 `Dialer::dial` 里、暂停的检查之前：没有 `owners`、监听器不是 HTTP 或 SOCKS5、`is_local` 答"不是本机"的会话得到"不知道"的槽，其余起一个任务 `owner::find`——先拿查找许可（`LOOKUPS` 个，限制同时占用的阻塞线程）再 `spawn_blocking` 调平台的查找，结果落进槽，不论用了多久；M2 里引擎只发 TCP 的查询，SOCKS5 的 UDP 关联按它的控制连接查一次，`Association.process`，流共用）；规则等进程：`choose_policy` 把槽包成 `SlotProcess`（期限是查找发起之后 `PROCESS_WAIT` = 200 毫秒），到期时这次评估按"不知道"判，会话的 `notes` 里加 `NOT_IN_TIME`，一行 DEBUG（带会话号），引擎的一生里第一次到期时 WARN 一次（`SLOW`）；`RequestRecord.process`（读记录时再看槽，会话结束之后才查到的进程也在）与 API 记录的 `pid`、`processPath`（紧跟 `listenPort`，不知道时 `null`）；可执行文件的 `PlatformOwners`（`crates/rurge/src/cli/runtime.rs`）、`owners_backend` 与只供测试的 `RURGE_PROCESS_BACKEND=off`（见下面「常用命令」之后的一段），`INACTIVE_RULE_TYPES` 去掉了 `PROCESS-NAME`（现在 5 个，`rurge check` 不再说它永不匹配，`W0013` 不再因它出现）。
进程的名字、路径与 PID 只在 DEBUG 的查找行（`process lookup found the process`，字段 `src`、`listen_port`（M3b 起是 `remote`）、`elapsed_ms`、`pid`、`name`、`path`）、请求记录与 API 里，INFO 及以上的行里没有；不知道进程从不让会话失败。
API 与日志行见 `docs/api/phase3.md` 的「M2　进程识别」一节（没有新增端点；同一次改动订正了 M1 的那一句，并补写了 `--system-proxy` 遇上慢的首读又落在 `suspend` 的网络上时系统代理翻转一次）。
兼容性清单改了 3.1 节 `PROCESS-NAME` 与 `AND` / `OR` / `NOT`（永不匹配的例子换成 `DEVICE-NAME`）、3.5 节内部集 `SYSTEM`、9.4 节 `suspend`（补"掉线不算离开"）、10.3 节 `rurge run`（补只供测试的 `RURGE_PROCESS_BACKEND`）与 10.4 节请求记录六行的文字，没有增删行，也没有改状态标记（仍是 566 行，附录重数过）；手工验收清单 `docs/acceptance/phase3-manual.md` 加了 M2 一节。
整分支终审之后的修正波（M2 记录第 28 ～ 33 行）的最后一个代码提交 `d6cc8f6` 上的全量运行（此后改代码的是 M8a 的任务 6，那一轮的全量见上面讲第二次 CI 运行的一段）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 干净，`cargo test --workspace --no-fail-fast` 77 个结果行、3797 个通过、0 个失败、10 个忽略（比此前多的一个是名单扫描器的新自测），没有哪个测试二进制以 `STATUS_HEAP_CORRUPTION` 或 `STATUS_ACCESS_VIOLATION` 退出——一台 Windows 11 x86-64 的开发机上的一次运行，不是 CI；此前最后一个代码提交 `f6327f0` 与 M2 最后的功能提交 `baa3ba1` 上都是 77 个结果行、3796 个通过、0 个失败、10 个忽略。**没有做的，和不要当成已经有的**：Linux 与 macOS 的真实读取只对着夹具测过——读取器的逻辑在开发机上跑过，没有跑过的是它们与真实系统的接触（真实的 `/proc` 的内容与权限、`pcblist_n` 的真实布局、`proc_pidpath`）；macOS 的两个 unsafe 函数与 Linux、macOS 的真读用例 `this_process_owns_its_loopback_sockets` 从没有在任何地方编译过（它们在只为本平台编译的模块里）；Windows 的那一条真读用例和端到端用例 `a_process_rule_matches_the_test_process` 只在 Windows 开发机上编译、运行过；Linux 与 macOS 上端到端红了先查读取器，不是接线——第一次 CI 运行时核对（M2 延后第 1 行）；M1 停放的第四件：五条起守护进程的端到端用例依赖第一次读网络在 1 秒的预算里完成，第一次 CI 运行时留意（M2 延后第 3 行）；手工验收清单的 M2 一节（Windows 5 项、macOS 5 项、Linux 5 项、与平台无关 8 项，共 23 项）一项也没有做过；macOS 取 `so_last_pid`，不取 `so_e_pid`，经 `nsurlsessiond` 等系统服务代建的连接记录里是谁没有看过，留给手工验收和项目所有者（M2 延后第 2 行）；查找的代价只在 Windows 上量过一次（debug 构建，开着 500 条回环连接时一次新的查找 3.39 毫秒，M2 记录第 24 行），Linux 的 `fd` 扫描在进程很多的机器上、几百条并发连接时是否在 200 毫秒里结束没有量过（M2 延后第 4、14 行）；虚拟机、WSL2、容器的客户端和读不了的进程应当是"不知道"，没有真的试过（M2 延后第 5 行）；一个永不返回的平台查找会占住两个许可、此后的会话各排一个小任务、`rurge run` 退出时 tokio 不设时限地等阻塞线程（M2 延后第 6 行）；经虚拟网卡来的会话随 M3，`$request.processPath` 随阶段 5，外部程序的进程树判断随 M4，没有给使用者的"关掉查找"的开关（M2-D3）；Windows 上不分大小写、不去掉 `.exe` 与盘符路径的读法是 rurge 自己的，没有在 Surge 上观察过；M2 延后第 7 ～ 13、15、16、23 行是次要项（下次动到时、有用户报告再说或有用例失败时再看；第 23 行是任务 7 留下、合并之前才补进表的两条）；第 17 ～ 22 行是整分支终审另加的：第 17、19 行留给 M3（TCP 的查询只比较远端端口；macOS 双栈记录的读法），第 18 行在 M3 让 UDP 也读 `/proc/net/udp*` 之前改（Linux 上整张表一行也解析不出时报 `Layout`），第 20 行归项目所有者（INFO 里要不要隐去 `PROCESS-NAME` 规则的文字，后来的会话不要自己定），第 21、22 行有用户报告再说；第 14 行的去向是 M3 之前在真实的 Linux 上量一次扫描的代价（与第 4 行一起）。
整分支终审已经做了（只读代码与文档，什么也没运行；结论是改了再合并；没有 Critical；三条 Important 都是这份文件里后来的会话会照着办的不实之辞——编译、运行过什么，Windows 读表的循环在哪里测，名单用例的保证（它的扫描器漏掉被拆成多行的属性，同一波里改了扫描器）——，连同代码、测试、注释与文档里的几处次要项在合并之前改了，M2 记录第 28 ～ 33 行）。
M2 记录第 1 ～ 16 行、第 25 行与第 28、29、31 行是主持这一轮的会话代项目所有者作的裁定，所有者可以改。**M2 里三件以后的会话必须知道的事**：一、会话的进程放在 `SessionHandle` 的进程槽里（`process()`），查找在会话开始时起、规则只在走到 `PROCESS-NAME` 时才等、至多 200 毫秒；以后任何要看进程的东西（M3 的虚拟网卡会话、阶段 5 的 `$request.processPath`、M4 防环用的进程树）读同一个槽，不另起查找（M2 记录第 15 行：查找在暂停的检查之前起，被拒的会话照样说出进程）；二、`rurge-platform` 里放开 `unsafe_code` 的函数由 `unsafe_functions.rs` 的 `LISTED` 钉住，加一个要改这张名单，并在提交说明与这份文件的 `unsafe_code` 一条里说（M2 记录第 1 行：名单用例接受任何可见性前缀，只认普通的 `fn`）；三、起 `rurge` 的测试辅助函数缺省设 `RURGE_PROCESS_BACKEND=off`，查真实进程的用例在构造之后去掉它，碰到进程查找的用例不用暂停的时钟（见测试约定；M2 记录第 27 行）。

### M3（内存设备上的 TUN 内核）的设计

M3（内存设备上的 TUN 内核）的细化设计已写好（2026-10-18；`docs/superpowers/specs/2026-10-18-phase3-m3-tun-core-design.md`，已决事项 M3-D1 ～ D26 是代项目所有者作的裁定，所有者可以改），分三份计划：M3a 设备、包的去向与 TCP → M3b UDP、ICMP、池与引擎接入 → M3c 应答器与预匹配。

### M3a（设备、包的去向与 TCP）

M3a 已做完（2026-10-19；计划 `docs/superpowers/plans/2026-10-18-phase3-m3a-tun-tcp-plan.md`，下面写"M3a 记录第 N 行""M3a 延后第 N 行"指它末尾的「执行期修正记录」与「延后事项」两张表，写"设计第 18 节第 N 条"指 M3 设计里对它自己正文的订正）。
它是新 crate `rurge-tun`（平台无关，`forbid(unsafe_code)`，依赖 `rurge-config`、`rurge-net`、`rurge-inbound`、`rurge-proto`、`smoltcp`、`getrandom`、`tokio`、`tokio-util`、`tracing`，没有新的外部包，`Cargo.lock` 只多它自己这一项、现在 518 个包）：在一块内存里的设备上把应用发来的 IP 包分给 TCP 的用户态协议栈（smoltcp 0.12，按四元组分片），建成的连接以 `BoxedStream` 交给引擎现有的 `Dialer`（`dial`，拨通再 `relay`）。**还没有谁依赖它**：`rurge-engine`、`rurge` 可执行文件、`rurge run` 与全部 API 不变，使用者看不到任何行为，兼容性清单、`docs/api/phase3.md`、手工清单与 README 随 M3c（设计第 16 节）。
做了什么（括号里是类型或函数与文件，都在 `crates/rurge-tun/src/` 下）：`Device` trait、`MemoryDevice::pair` 与 `AppEnd`（`device.rs`）；收包的任务（`dispatch::run`）先让 `packet::classify` 把每个包过一遍（`malformed`、`fragment`、`martian`、`unsupported` 四种丢弃，错误带着读到的两端），TCP 按四元组的散列分到片，队列满了丢并计 `queue_full`，UDP 与 ICMP 过了过滤之后计 `unsupported`（M3b 起分发）；丢弃按原因计数（`TunHandle::counters()` 的 `Counters`：十种原因加 `stack_panics`），每种原因第一次一行 WARN、其余 DEBUG，`martian` 什么也不说，名字与地址不进 INFO 及以上；写任务（`writer::run`）；片（`tcp::shard`）：每片一套 smoltcp `Interface` 与 `SocketSet` 在一把锁里、一个任务逐包推进（每次拿锁先推进一次发送，再至多 256 个包，再反复推进发送到没有出口或做满 64 遍），新的 SYN 依次过停止、上限（`tcp::admission::Admission`：16384 个套接字、2048 个半开、rurge 自己的 RST 3 秒 100 个）、目的地（`space::place`：rurge 自己的地址与 fake 地址一律回 RST，M3a 没有池，fake 地址计 `unknown_fake`）才建一个只听那个目的端点的套接字，缓冲按一个总预算分配（`tcp::budget`）；`TunStream`（`tcp::stream`）；smoltcp 0.12 的隐患的对策（设计第 5.3 节）：过滤、SYN 里 SACK-permitted 的改写（`packet::strip_sack_permitted`）、窗口边缘的守卫（`tcp::guard`）、接收缓冲不超过 65535 字节（`RX_MAX`，窗口缩放为 0）、片的 `catch_unwind` 与锁中毒时整片重建（`tcp::rebuild`；两个只在测试构建里的 panic 触发点，`PANIC_MARKER`）；会话（`session::serve`）：握手之后拨号，拒绝与拨不通回 RST，`REJECT-DROP` 拿着再关；停止与设备出错（`Ending`）。
应用端的夹具在 `rurge_tun::testing`（`testing` 特性，M3b 起引擎的用例经它）：`AppStack` 与 `AppTcp`（应用一侧的另一个 smoltcp 栈）、`FakeDialer` 与 `Script`（假 `Dialer`，只拨用例写给它的回环地址）、回环上的 `Upstream`、`ScriptedTcp`（B2 用的最小 TCP 客户端）。
最后一个改代码的提交 `06593e6`（整分支终审之后的修正波）上的全量运行（此后 `34377e7` 只改了 `rurge-tun` 里一处文档注释，`cargo fmt --all --check` 与 clippy 仍干净，其余只有文档变过）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 干净，`cargo test --workspace --no-fail-fast` 79 个结果行、3973 个通过、0 个失败、12 个忽略（`rurge-tun` 的 `--lib` 是 163 个通过、2 个忽略，那两个忽略的是基准；`rurge-engine` 的 `pipeline` 测试二进制那一次以 `STATUS_HEAP_CORRUPTION` 退出，单独重跑通过，计入这个数）——一台 Windows 11 x86-64 的开发机上的一次运行，不是 CI。
基准门槛的结论与选定的缺省值（设计第 6 节、第 18 节第 11 条；M3a 记录第 8 ～ 11、17、21、30 行；两个基准是忽略的用例，release 下手动跑，不作门禁，命令见「常用命令」）：B1（单流吞吐）看远端在内存里的行（`TUN_BENCH_FAR=memory`）：第一次（旧缺省，还没有守卫与片的重建）80 行全部过 0.7 Gbit/s、最低 4.53，MTU 1500、`tx_max` 256 KiB 的中位数上行 7.36、下行 5.67；加了守卫与重建之后的最近一次（各 40 次，M3a 记录第 21 行）中位数上行 7.175、下行 5.460，八个设置的 320 行里最低 3.959；远端是回环 TCP 的行只作那一跳的上限、不下判语（引擎的转发循环接一段回环 TCP，不经 TUN 就只有 0.7 ～ 0.9 Gbit/s）；B2（一万条空闲加一百条活跃连接）看 rurge 自己的内存，即峰值减去测试一侧自己的内存，对着 429,000,000 字节（NFR-01 的 300 MB 除以 0.7），缺省设置下最近一次 390.4 MB，在线以内（余量 38.6 MB）；**不转向自写的 TCP 被动一端**。
选定的缺省值（`TunOptions::default()`）：`shards` 256、`buffer_min` 4 KiB、`tx_max` 256 KiB、`budget` 64 MiB、MTU 1500（M4 的设备取它）；代价是 64 MiB 的预算被最先建的约 250 个套接字用完，此后建的套接字一辈子只有 `buffer_min`（4 KiB），这样的一条高吞吐连接没有量过（M3a 延后第 6 行，M6 的验收）。**B2 的保守读法**（只减去准的应用端套接字缓冲）不在线以内：M3a 记录第 17 行的守卫加进来之后的 11 次里有 5 次超过、都不到 0.6 MB，全部做完之后的一次超过 3.1 MB（432.1 MB），原因没有查；这个读法本来就是一次测量的结果、不是保证，要不要把它当判据、要不要因此把预算调到 48 MiB 由项目所有者定（M3a 延后第 15 行）。
B2 里最大的一项是引擎的转发循环（每条会话两块 8 KiB，一万条 164 MB）：不改它验不到 NFR-01 本身，自适应的转发缓冲移交阶段 3 / M6（M3a 记录第 11 行、延后第 5 行）。**没有做的，和不要当成已经有的**：UDP、ICMP 回显、fake-IP 池、DNS 应答器、预匹配在 DNS 与 SYN 层、`SessionInfo.local`、引擎接入（`Engine::start_tun`）、经虚拟网卡来的会话查进程、请求记录里的虚拟网卡会话都还没有（M3b、M3c）；`Counters` 还没有出口，`GET /v1/features/enhanced_mode` 随 M4；真实的设备、地址、路由、系统 DNS 与防环是 M4，`Device` 只有内存里的实现；基准与全部用例只在一台 Windows 11 x86-64 的开发机上量过、跑过，没有在 Linux 或 macOS 上编译或运行过（`peak_resident_bytes` 读 `VmHWM` 的 Linux 分支更是从没编译过），CI 没有跑过 `rurge-tun`（M3a 延后第 4、14 行）；整分支终审做过了（2026-10-19，只读代码与文档，什么也没运行；`3b20b09..785080c` 的 38 个提交）：结论是改了再合并，没有 Critical，两条 Important 在 M3a 里都够不到（只有内存设备、没有引擎），是 M3b 与 M4 要靠的东西——下面第三、四件事写的就是它们——；修正波在 `rurge-tun` 里只动了说明、注释、几处不改行为的守卫（同一条连接第二次被丢不再多登记一项、到期函数遇上孤项不再 panic、`panic = "unwind"` 的编译期断言、`#[must_use]`）、一条新用例和一个用例的改名，其余是计划、设计、这份文件里的说明与去向；两条 Important、七条新的次要项和任务评审留下的次要项都有去向，做了什么、留下什么逐件写在 M3a 延后第 16、17 行；各任务的评审没有提出过 Critical，提出过的 Important——任务 1 的两条、任务 3 的一条（M3a 记录第 1、2、8、9 行）、任务 9 的一条（文档里"没有一条"的话不实）——都在下一个任务之前的修正轮里改了；M3a 的提交写这段时在分支 `phase3-m3a` 上，没有合并、没有推送，没有一个被 CI 见过。**M3a 里五件以后的会话必须知道的事**：一、TUN 的套接字开保活（30 秒）、超时 600 秒（`TunOptions::keep_alive`、`timeout`），不是设计原来写的"不开保活、超时 120 秒"——smoltcp 0.12 的超时在应用沉默 `timeout` 之后就断开，不管有没有在途的数据，照原写法空闲的长连接两分钟就断（M3a 记录第 3 行，设计第 18 节第 12 条）。
二、窗口边缘的守卫只在这条连接的接收缓冲里还有任何空位时丢越过边缘的段，缓冲满时交给协议栈去答（零窗口探测因此得到确认）；它靠协议栈的窗口缩放为 0，所以接收缓冲不能超过 `RX_MAX` = 65535（编译期断言，谁要调大先读 `RX_MAX` 的文档）；真实的 Windows、macOS 与 Linux 发送方在长停顿里怎样探测没有观察过（M3a 记录第 18、19 行，延后第 11 行）。
三、停止与 `REJECT-DROP`：停止时每片走最后一轮，把复位交给写任务，写任务在停止之后只给设备 `STOP_GRACE`（1 秒）；`REJECT-DROP` 拿着 `drop_hold`、FIN、再读掉应用还在发的字节至多 `linger`，代价是这样的连接多占一个套接字一阵；M3b 起读掉丢弃用 512 字节的数组，拿着与连接的结束赛跑（应用把连接复位时拿着随之结束，`TunStream::closed`）。**M3b 起停止取消正在转发的会话**（任务 5，`7828455`）：`TunHandle::stop` 只发出停止、立刻返回，每个已拨通的会话的任务看到停止就取消自己的会话令牌，再等它的转发结束，`join` 等所有的任务，停止与转发赛跑时停止居先，转发中的会话等的是停止令牌的子令牌（`Life::session_stop`，各有自己的 `Notify`，M3b 延后第 26 行）；此前会话只在转发循环去碰 `TunStream` 的时候才发现连接被复位，两个方向都在等上游的转发（上行卡在写一个不收的上游，下行卡在读一个不出声的上游）一次也不碰，要等到上游动一下、转发循环的空闲超时（`--idle-timeout`，缺省 600 秒）到点或有人取消会话的令牌；引擎在停止接受时停 TUN（`Engine::start_tun`）。**拨号仍不与停止赛跑**（P29）：拨号进行中会话的令牌还在引擎手里，TUN 够不到，丢掉拨号的 future 会留下开着的请求记录；握手在停止之后才结束的会话不再拨号（`established()` 之后先看停止，已停止就返回，那条连接正被片的最后一轮复位）；`Engine::dial` 每一条会等待的路都有自己的时限（M3b 记录第 15 行），拨号回来时流已被复位、转发当场结束，`join` 等着它；一个永不回来的拨号仍让 `join` 一直等，所以 **M4 的可执行文件停 TUN 时要给等 `join` 的那一步一个自己的时限**（M3b 延后第 8 行）；不要在 TUN 里把转发或拨号的 future 丢掉来结束它（`relay` 要结束 `SessionHandle`，丢掉它请求记录会一直开着）。
`STOP_GRACE` 在真实的设备上够不够没有量过，最后一轮的复位在真实的设备上会不会因为设备满了被丢，要等 M4（M3a 记录第 25 ～ 28 行，延后第 12、13 行；设计第 18 节第 15、22 条）。
四、应用发来、会话还没读的数据，在 TIME-WAIT 里 10 秒之后被 smoltcp 自己的 `reset()` 清掉，读成 `ConnectionReset`——应用的 TCP 看到这些字节都被确认了，所以这是应用看不见的丢数据——：**M3b 的任务 4（`03f01c7`）按这一条要的形状做掉了**。
套接字到 TIME-WAIT（或 `Closed`）而流还在、`recv_queue` 里有没读的字节时，片把它们拷进连接与流共用的记录（`ConnShared` 的 `Leftover`，`crates/rurge-tun/src/tcp/mod.rs`），接收缓冲的预算份额留到读完或流被丢掉再还，套接字和四元组的表项立刻放掉；读先从记录里答，答完是结局（收到过 FIN 的是 `Ok(0)`，没有的仍是复位）。**不要只修结局**（比如计时器关掉套接字时记成 `Closed`），那会把看得见的丢失（一个复位）变成悄悄的截断；片一级的用例 `unread_bytes_outlive_the_time_wait`（M3a 延后第 8 行，设计第 18 节第 21 条）。
五、**一个 M3a 之外的风险，留给项目所有者，后来的会话不要自己动**：`rurge-proto-wireguard` 的 smoltcp 栈（`stack.rs` 的 `TCP_BUFFER`）每个 TCP 套接字的收发缓冲各 256 KiB，窗口缩放位数是 3，不为 0，那个栈没有守卫（TUN 靠接收缓冲不超过 65535 字节避开这件事）；按设计第 5.3 节 H2 的读法，经隧道下行而本机一侧读得慢时，对端在途的数据可能落到 smoltcp 最近一次告知的边缘之外，`last_scaled_window` 的减法下溢而 panic（`socket/tcp.rs` 第 693 ～ 701 行），隧道随之结束、下一个连接时重建（阶段 2 / M8c 任务 6）。
读源码得出，没有观察过；修法二选一：接收缓冲不超过 65535（经隧道的往返 50 毫秒时下行至多约 10 Mbit/s），或照 M3 加窗口边缘的守卫（M3a 延后第 1 行）。
M3a 记录里写着"主持这一轮的会话代项目所有者作的裁定"的各行，所有者可以改；测试方面新的约定在下面「常用命令」之后的测试约定里。

### M3b（UDP、ICMP、fake-IP 池与引擎接入）

M3b 的十个任务做完了（2026-10-23；计划 `docs/superpowers/plans/2026-10-22-phase3-m3b-udp-icmp-pool-engine-plan.md`，下面写"M3b 记录第 N 行""M3b 延后第 N 行"指它末尾的「执行期修正记录」与「延后事项」两张表，写"P N"指它的「计划期决定」，设计第 18 节第 16 ～ 22 条是它对 M3 设计正文的订正）。**上面 M3a 一段里"没有做的"那张单子，被 M3b 做掉的以这一段为准**：UDP、ICMP 回显、fake-IP 池、`SessionInfo.local`、`Engine::start_tun`、虚拟网卡会话的进程查找与请求记录都有了，应答器、预匹配与经监听器来的 fake 地址仍是 M3c；M3a 一段"五件事"里的第三、四件，已改写成做掉之后的样子。**更早的段落里被 M3b 改了的几句，同样以这一段为准**，那些历史的段落不逐处改：M3a 一段里 `rurge-tun` 的依赖列表（多一个 `rurge-dns`）与"**还没有谁依赖它**"一句（引擎现在依赖 `rurge-tun`，`GET /v1/traffic` 多了 `listener.tun`，进程查找的 DEBUG 行换了一个字段，`docs/api/phase3.md` 已有「M3b」一节，不是"随 M3c"；兼容性清单、手工清单与 README 仍随 M3c），M2 一段里 `Engine::look_up_owner` 的"监听器不是 HTTP 或 SOCKS5"（现在是 `query_for` 按监听器与传输定查不查、怎样查，虚拟网卡的会话也查）、"经虚拟网卡来的会话随 M3"（引擎一侧的查找 M3b 做了，真实的设备随 M4）和把延后第 17 ～ 19 行"留给 M3"的话（M3b 任务 1 做掉了）。
做了什么（括号里是类型或函数与文件）：`SessionInfo.local`（`crates/rurge-config/src/session.rs`：监听器的会话是被接受的那条连接自己的本地地址，TUN 的 TCP 会话是应用连的端点）与进程查询按会话的形状问（`owner::query_for`，`crates/rurge-engine/src/owner.rs`；`OwnerQuery.remote` 与 `procinfo::Query.remote`，TCP 的行连远端地址一起比；Linux 一张有内容而一行也解析不出的表是 `Layout`，macOS 双栈的记录出两行；DEBUG 查找行的字段 `listen_port` 换成 `remote`；M2 延后第 17 ～ 19 行关掉了）；fake-IP 池（`rurge_dns::fakeip`，`crates/rurge-dns/src/fakeip/{mod,file}.rs`：编号 *i* 同时是 v4 `198.18.1.1 + i` 与 v6 `fd00:6152::` 加同一个 v4 地址，130,814 个，名字规范化，先游标后最久没用到的，有租约的编号从不回收；`fake-ip.txt` 在数据目录里，至多每 10 秒与停下时写，`FakeIpPool::keep_saved`）；引擎的 UDP（`UdpClient::send` 多一个 `answers`——目标恰好相同、或同端口的域名流只有一条时给出，`Routes::flow_for`，`crates/rurge-engine/src/udp.rs`；新的 `UdpClient::refused`，`REJECT` 一族的流对每个被丢的包调，`Failed` 的不调：拨不通与暂停的流 5 秒后重试，回 ICMP 会让应用放弃一个也许马上就通的目的地，P8）；`rurge-tun`（`crates/rurge-tun/src/`）：`hooks.rs`（`Hooks`、只有解析器与 `icmp_forwarding` 的 `TunSettings`、`Pinger`，P2），发往 fake 地址的 TCP 按名字拨号，UDP 会话（`udp/{mod,client,targets}.rs`：每个应用源端点一个，实现 `UdpClient`、交给引擎现有的 UDP 流水线；目标表至多 1024 个，两个方向都空闲 `udp_idle`（60 秒）的目标早放租约；回包大于 MTU 时分片，`fragment.rs`），被拒的 UDP 流回 ICMP"管理上禁止"（`icmp.rs`：`REJECT` 与 `REJECT-TINYGIF` 合起来 3 秒 100 个，`REJECT-NO-DROP` 不限，`REJECT-DROP` 不回；发往 rurge 自己地址的 UDP 同样回、不限频），ICMP 回显（`icmp.rs`：rurge 自己的地址本地应答，fake 地址先解析再转发，其余经 `Pinger`；`icmp-forwarding = false` 时一律丢弃，rurge 自己的地址也是），`classify` 验 UDP 与回显请求的校验和（`packet.rs`），`TunOptions::check`（P27），和 M3a 交下来的两件：TIME-WAIT 里没读的数据交给流（`Leftover`，P26）、停止取消正在转发的会话与 `REJECT-DROP` 的拿着随连接结束（`session.rs`，P29、P30）；`Engine::start_tun`（`crates/rurge-engine/src/tun.rs`，`async`：在数据目录里打开池、起 TUN、保存池直到 TUN 结束、引擎停止接受时停 TUN；`Runtime.tun` 是每一代的 `TunSettings`）；`GET /v1/traffic` 的 `listener` 多一个 `tun`（`docs/api/phase3.md` 多了「M3b」一节：`listener.tun`、`remote`、池的文件与它的三行日志；`docs/api/phase1.md` 的 Traffic 一节多一句注）。
夹具在 `rurge_tun::testing`（`FakeHooks`、`FakeResolve`、`FakePinger`、`packets`、`AppStack::divert_udp` / `divert_icmp`）。
最终修正波的最后一个改代码的提交 `880482e` 上的全量运行（此后只有文档变过）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 干净，`cargo test --workspace --no-fail-fast` 80 个结果行、4125 个通过、0 个失败、12 个忽略，没有 `STATUS_*` 退出、没有挂住——一台 Windows 11 x86-64 的开发机上的一次运行，不是 CI；此前最后一个任务提交 `ec93297` 上的全量同样是 80 个结果行、4125 个通过（更早的一次运行里 `outbounds_masque` 以 `STATUS_HEAP_CORRUPTION` 退出，单独重跑 11 个通过）。**没有做的，和不要当成已经有的**：DNS 应答器、预匹配在 DNS 与 SYN 层、经监听器来的 fake 地址（M3c）；真实的 `Pinger`（ICMP 套接字）、真实的设备、地址、路由、系统 DNS 与防环（M4）；`rurge run` 不起 TUN（可执行文件里没有谁调 `Engine::start_tun`，调它的只有 `crates/rurge-engine/tests/tun.rs`；可执行文件只多了一处接线，`crates/rurge/src/cli/runtime.rs` 的 `platform_query` 把 `remote` 交给平台层，所以 `rurge run` 的 TCP 进程查找也连远端地址一起比），池的文件与它的三行日志、经虚拟网卡来的会话，使用者要到 M4 才看得到，`GET /v1/traffic` 的 `listener.tun` 在那之前恒为 0；兼容性清单、`docs/requirements.md`、README 与手工清单（除三处写 `listen_port` 的）这一轮没有动，随 M3c；`TunSettings` 的其余字段与 `Hooks::pre_match`、每次换源端口的 UDP 应用占关联、拆 `bench.rs` 的一遍（M3b 延后第 5、7、9 行）；没有在 Linux 或 macOS 上编译或运行过任何东西（代码平台无关；任务 1 的平台层解析器与夹具用例写成在每个宿主上都编译、都跑，但只在这台 Windows 机器上编译、跑过；只给那两个平台的两处真读用例里的 `Query` 字面量，从没有在任何地方编译过），CI 没有跑过 `rurge-tun` 与引擎的 TUN 用例，第一次 CI 运行时核对的在 M3b 延后第 27 行，任务 5 的门禁里挂住过一次的 `rurge-dns` 库测试在第 28 行（`MockDns` 的接收与接受循环自最终修正波起遇到错误停 10 毫秒（接收一侧的 `ConnectionReset` 除外：那是发给已经走了的一方的应答在 Windows 上回来的错误，照旧立刻接着收），那是不是挂住的原因没有证明）；整分支终审做过了（2026-10-23，一位新的评审者，只读代码与文档，什么也没运行）：没有 Critical，没有 Important，结论是改了再合并；它的五条次要项里四条与各任务评审留下的小项里十条在最终修正波里改了，一条登记了（TUN 里排队的 UDP 没有字节上限，延后第 29 行），其余各有去向（M3b 记录第 19、20 行与延后事项）。
各任务的评审是新的评审者：规格全部通过、质量没有 Critical，两条 Important（任务 2 的空洞抵扣，任务 8 的测试重复，M3b 记录第 17 行）在下一个任务之前的修正轮里改了，复审认定已改；M3b 的提交写这段时在分支 `phase3-m3b` 上（计划 `34c477c`，任务 1 ～ 9 到 `ec93297`，最终修正波的代码到 `880482e`），从 `main` 的 `f3c09d2` 切出，没有推送，没有一个被 CI 见过。**M3b 里五件以后的会话必须知道的事**：一、**停止取消正在转发的会话，不取消正在进行的拨号**（P29）：拨号进行中会话的令牌还在引擎手里，TUN 够不到，不要在 TUN 里把拨号的 future 丢掉（请求记录会一直开着）；握手在停止之后才结束的会话不拨号（`session::serve` 在握手之后先看停止）；`Engine::dial` 每一条会等待的路都有时限（M3b 记录第 15 行），其中一个由 Profile 定（`evaluate-before-use` 的 `round_timeout`，`test-timeout` 可以写任意多秒），所以停止之后 `join` 最长等多久不是常数，**M4 的可执行文件停 TUN 时要给等 `join` 的那一步一个自己的时限**（M3b 延后第 8 行）。
二、**池的隔离是一条所有者可以改的裁定，有一笔代价**（M3b 记录第 3 行）：打开时，文件里没有名字的编号（上一次运行隔离着、没来得及交出的）再隔离 120 秒，**叠加**在接下来的 2048 个之上、不抵扣；代价是连续崩溃时每次不干净的打开都多隔离 2048 个（上一次隔离的编号要等 120 秒之后被交出去才有名字，在那之前每次打开都再隔离它们），大约 64 次之后分配器绕过一整圈：此后每次打开——干净的也一样——都把没有名字的编号再隔离 120 秒，这 120 秒里 `assign` 只能接手最久没用到的名字的编号、一个也没有时答 `Exhausted`，直到活过 120 秒的运行把那些编号交出去（`QUARANTINE_COUNT` 的说明，M3b 记录第 21 行）；`5546384` 的提交说明还写着抵扣，没有改历史；`open` 把 `clean 1` 立刻改写成 `clean 0`，读不了的文件留着不动、第一次保存时覆盖。
三、**一条 TUN 会话在引擎眼里的样子**：TCP 的 `listener` 是 `Tun`、`in_port` 0、`local` 是应用连的端点（fake 地址或真实地址）、`dst_host` 是 fake 地址背后的**名字**（会话拿着租约到结束）或目的 IP；UDP 是每个应用源端点一个会话（引擎的一个关联），模板的目的是协议栈自己的地址（`198.18.0.253` / `fd00:6152::fd`）加端口 0、没有 `local`，引擎的每条流换成自己的目标，请求记录一个目的地一条；进程查找按这个形状（TCP 按 `src` 与 `local`，UDP 只按应用自己的端点）；fake 地址只在 `rurge-tun` 里换回名字，引擎看到的已经是名字。
四、**起 TUN 的调用者（M4 的可执行文件）要先调 `TunOptions::check`** 并把错误交给使用者：`Tun::start` 对不合格的选项与小于 1280 的 MTU panic，而池的文件那时已被改成 `clean 0`（下次启动多隔离 120 秒，M3b 延后第 24 行）；一个数据目录一次只起一个 TUN，代码不拦（第 25 行）。
五、**测试**：`TunHandle::session_tasks()` 是 `cfg(test)` 的，TUN 停止之后不能调（panic 或少算），停止之后读 `live_tasks()`；写 TUN 用例的规矩（P17 的两道防线、校验和、`writer_queue`）在下面「常用命令」之后的测试约定里。
M3b 记录里写着"主持这一轮的会话代项目所有者作的裁定"的各行，所有者可以改。

### M3c（DNS 应答器、预匹配与经监听器来的 fake 地址）

M3c（DNS 应答器、预匹配与经监听器来的 fake 地址）的任务 0 ～ 6 做完了（2026-10-23；计划 `docs/superpowers/plans/2026-10-23-phase3-m3c-dns-responder-prematch-plan.md`，下面写"M3c 记录第 N 行""M3c 延后第 N 行"指它末尾的「执行期修正记录」与「延后事项」两张表，写"P N"指它的「计划期决定」，写"R N"指执行期的裁定，见记录各行；设计第 18 节第 23 ～ 34 条是它对 M3 设计正文的订正）。**至此 M3（内存设备上的 TUN 内核）整个做完**：虚拟网卡的内核在内存里的设备上做完、测完，使用者今天看不到它，真实的设备、地址、路由、系统 DNS 与防环是 M4；今天看得到的只有几处小的，M3c 的两处：`rurge dns lookup --trace` 一类输出里 FORMERR 与 NOTIMP 的文字（原来写 `RCODE1`、`RCODE4`），和 UDP 上游给在途的同 ID 的第二个查询换一个空着的 ID（`Pending::claim`；原来第二个顶掉第一个的等待者）；M3b 的三处：`GET /v1/traffic` 的 `listener.tun`（恒为 0）、进程查找 DEBUG 行的 `remote` 字段、`rurge run` 的 TCP 进程查找连远端地址一起比（`platform_query`）；细节见 `docs/api/phase3.md`。**上面 M3b 一段里"没有做的"那张单子，被 M3c 做掉的以这一段为准**：DNS 应答器、预匹配在 DNS 与 SYN 两层、经监听器来的 fake 地址、`TunSettings` 的其余字段与 `Hooks::pre_match` 都有了；M3b 一段说兼容性清单、`docs/requirements.md`、README 与手工清单（除三处写 `listen_port` 的）这一轮没有动、随 M3c，M3c 的任务 7 动了（兼容性清单现在 567 行，手工清单加了 M3 一节，需求文档加了阶段 3 的状态行）。**更早的段落里被 M3c 改了的几句，同样以这一段为准**，那些历史的段落不逐处改：M3b 一段里"只有解析器与 `icmp_forwarding` 的 `TunSettings`"（现在多了 `dns`、`always_real_ip`、`allow_dns_svcb`、`hijack_dns`）；M3a 与 M3b 两段里说发往 rurge 自己的地址的 TCP 一律回 RST、UDP 一律回 ICMP（应答器地址的 53 端口现在去应答器，用例 `syns_to_rurge_addresses_are_reset` 与 `udp_to_rurge_addresses_is_prohibited_unlimited` 改发 54 端口，R4、R5）；常用命令里 `tests::udp::`、`tests::syn::`、`tests::options::`、`tests::bench::`、`--test tun` 与 `pre_match` 的几行已按 M3c 改过。
做了什么（括号里是类型或函数与文件）：任务 0 只挪代码（`rurge-tun` 的大测试文件与基准拆成 `src/tests/{bench,udp,fixture,lifecycle,outcomes,icmp}/`，用例的路径前缀不变；`Context` 挪进 `src/context.rs`，`udp` 与 `icmp` 不再引用 `dispatch`；造 UDP 包的函数挪进 `packet.rs`；`Session::ending()` 改名 `mark_ending()`；`TunHandle::session_tasks()` 开头加 `debug_assert!`：停止之后读它在调试构建里 panic）。
`rurge-dns`（任务 1）：`Resolver::forward(query)`（`crates/rurge-dns/src/resolver.rs`，经 `fanout::forward_raw`；上游照 `[Host]`：`server:` 的服务器、`server:system` / `syslib` / `force-syslib` 的系统 DNS 服务器、其余主上游，别名不跟随，`.local` 与单标签名字照主上游；同时问每个上游、每秒重发、至多 5 轮，每次发出换随机 ID，只收问题相同的应答，第一个 NOERROR 或 NXDOMAIN 胜出；不进缓存、不计 AAAA 抑制）、`message` 里只读报文头与问题的函数和应答器读请求造应答的函数（`read_request`、`answer`、`answer_truncated`、`answer_header_only`，`Rcode::{FormErr, NotImp}`）、UDP 上游给每个在途的查询一个不冲突的 ID（`upstream/udp.rs` 的 `Pending` 与 `Claim`）、`MockDns` 回答任何类型。
`rurge-config` 与 `rurge-rules`（任务 2）：`PreMatchLayer { Dns, Syn }`（`rurge_config::session`）、`Verdict::NeedsConnection` 与三值的组合（`matcher.rs`、`set.rs`）、`RuleEngine::pre_match(session, layer)`（`engine.rs`，取代 `pre_match_domain` 与 `pre_match_ip`；`PreMatch` 多 `sub_rule`）。
`rurge-tun` 的应答器（任务 3 ～ 5，`src/dns/`）：`Responder::answer`（`mod.rs`：每个 TUN 一个，UDP 与 TCP 共用 256 个转发许可；判定的次序是读请求的三种拒绝 → class → canary → SVCB 与 HTTPS → 对 A 与 AAAA 的预匹配 → 地址族 → `always-real-ip` → 没有池 → fake 地址，其余类型转发；UDP 的限度夹在 512 与 1232 之间，TCP 超过 65,535 字节答 SERVFAIL）；UDP 的去处（`src/udp/mod.rs` 的 `destination`；应答器的队列与每个查询的任务在 `src/dns/udp.rs`，`dns_queue`）；只问 53 端口的会话早结束（`udp_dns_idle`）；TUN 里排队的 UDP 共用的字节预算（`src/udp/bytes.rs`，`udp_bytes`，M3b 延后第 29 行的 TUN 一侧）；DNS over TCP（`src/dns/tcp.rs` 的 `serve_dns`，`dns_tcp_idle`；同时服务的连接至多 `dns_tcp_max` 条，`src/tcp/admission.rs` 的 `DnsPlace`，整分支终审之后加的；`TunStream::close_and_wait_for_the_app`，`src/tcp/stream.rs`，与 `REJECT-DROP` 共用）；SYN 层的预匹配（`src/tcp/shard.rs` 的 `refused_by_rules`，在片的锁里、建套接字之前；`src/session.rs` 的 `Serving` 与 `Purpose`）；`src/hooks.rs` 的 `TunDns`（`rurge_dns::Resolver` 实现它）、`TunSettings` 的 `dns`、`always_real_ip`、`allow_dns_svcb`、`hijack_dns` 与 `Hooks::pre_match`；`space::REFUSED_V4`（`198.18.0.244`）；`rurge_tun::testing` 的 `FakeDns`；b3。
`rurge-engine`（任务 6）：`Engine::pre_match_refusal` 与 `Recorded`（`src/prematch.rs`：该记时才查进程、建 handle，同一代同一条规则 5 分钟记一次；自动升级与 TCP 会话共用一份计数）、`impl Hooks for Engine` 的 `pre_match`（`src/tun.rs`）、经监听器来的 fake 地址（`src/fake_address.rs` 的 `Turned` 与 `Engine::turn_back`，`Engine.fake_pool`，`FAKE_ADDRESS_NOT_KNOWN`；SOCKS5 的 UDP 流在 `src/udp.rs` 的 `run_flow`，`Flow` 的 `asked` 与 `swapped`）、`rurge_dns::fakeip::in_space`、记录与会话日志的 `dst` 在端口为 0 时只写主机（`observe::destination`）；`tests/tun_dns.rs`（23 个）与 `tests/common/tun.rs`。
最后一个改代码的提交 `dc5f9bc`（最终修正波）上的全量运行（此后 `5137937` 只改了 `rurge-tun` 测试里的几处注释，`cargo fmt --all --check` 与 clippy 仍干净、`tests::dns::` 70 个通过，其余只有文档变过）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 干净，`cargo test --workspace --no-fail-fast` 81 个结果行、4304 个通过、0 个失败、13 个忽略（没有哪个测试二进制以 `STATUS_HEAP_CORRUPTION` 或 `STATUS_ACCESS_VIOLATION` 退出）——一台 Windows 11 x86-64 的开发机上的一次运行，不是 CI。
基准（release，这台开发机，内存里的设备；不作门禁，M3c 记录第 26 行）：**b3**（`Responder::answer` 加造应答包，十万个不同的名字，不含设备、写任务和引擎的规则评估，所以是整条路的下限）中位数 0.9 微秒、p99 3.2 微秒、最大 5.8 毫秒（NFR-01 的线 0.2 毫秒；最大值在打印了下标的四次里都是第 57,345 个名字，那是池的 `HashMap` 翻倍的地方——读代码得出的解释，没有改池去验证）；**B2**（缺省设置）三次——第一次在 `dbe4f80` 上，后两次在 `e5c4b0d` 上，只有后两次可以互相比（门槛读数差 0.06 MB）——：进程的峰值 472.1 / 476.8 / 476.9 MB，其中门槛读数（扣掉测试一侧自己的内存）397.7 / 402.4 / 402.5 MB，对着 429,000,000 字节的线余量 31.3 / 26.6 / 26.5 MB（M3a 最后一次 390.4 MB、余量 38.6 MB），保守读数超线 10.4 / 15.1 / 15.1 MB（M3a 最后一次 3.1 MB）；自 M3a 起的涨幅按同一个提交上的那两次算，门槛读数两次都高了约 12.0 MB（按字节是 11.97 与 12.03 MB），保守读数超线同样多了约 12.0 MB（两个读数减去的是同一个测试一侧的常数，所以涨的是同一个量），原因没有查；保守读法要不要当判据、预算要不要调到 48 MiB 仍归项目所有者（M3a 延后第 15 行，M3c 延后第 13 行）。
整分支终审做过了（2026-10-24，一位新的评审者，只读代码与文档，什么也没运行；`4cf60ba..5cacdee`）：没有 Critical，没有 Important，四条次要项，结论是改了再合并；最终修正波（`03b809a`、`dc5f9bc`）都处理了——一条是 SOCKS5 的 UDP 回包来源的限制登记得比代码窄（只改文字，R32）、一条是应答器的 TCP 连接没有数量上限（加了 `TunOptions::dns_tcp_max`，缺省 128，R33）、一条是 R27 的延后事项没有让 M4 看见（只改去向，R34）、一条是 `assert_no_address_given` 只看编号 0（加了 `FakeIpPool::named()`，R35）——，连同各任务评审留下的小项，在 M3c 记录第 28 ～ 36 行与延后第 25 ～ 28 行。
修正波之后的复审（一位新的评审者，只看修正波的改动，什么也没运行）：没有新的 Critical 与 Important，五条次要项——SOCKS5 的 UDP 回包来源的限制写宽了（每个目标一条连接的载体没有这个限制）、TCP 上的应答其实经写任务的队列送出、计划两行的提交格与先红、一条与半开超时赛跑的用例没写余量、测试夹具的说明里留着旧名——由主持者直接改了（R36，`5137937` 与随后的文档提交，M3c 记录第 37 行）。**没有做的，和不要当成已经有的**：`rurge run` 不起 TUN（可执行文件里没有谁调 `Engine::start_tun`，调它的只有 `crates/rurge-engine/tests/common/tun.rs`（`tun.rs` 与 `tun_dns.rs` 共用它）与 `tests/tun.rs`），所以应答器、预匹配、池的文件与经监听器来的 fake 地址使用者要到 M4 才看得到——连经监听器来的那一半也要有 TUN 的池（`Engine.fake_pool` 只由 `start_tun` 放）；`Counters` 没有出口，`GET /v1/features/enhanced_mode` 随 M4；系统的 DNS 服务器名单要排除应答器与虚拟网卡的地址，随 M4（M3c 延后第 4 行）；真实的设备、地址、路由、真实的 `Pinger` 与防环（M4）；明文 HTTP 请求经监听器带着 fake 地址来，换回名字之后请求行、`Host` 与 `session.url` 仍是 fake 地址（R27，阶段 4）；**M3c 的提交没有一个被 CI 见过**，`rurge-tun` 的全部用例与引擎的 TUN 用例只在一台 Windows 11 x86-64 的开发机上跑过，计时用例的余量在慢的机器上没有量过（M3c 延后第 23 行）；手工验收清单的 M3 一节（Windows 3 项、macOS 3 项、Linux 3 项、与平台无关 10 项，共 19 项，除 P27 的监听器那一半外要等 M4）一项也没有做过；去向写着"项目所有者"的，后来的会话不要自己定：M3c 延后第 6 行（被预匹配拒绝的名字进 INFO，P18）与第 13 行（B2 的读数与预算）。**M3c 里八件以后的会话必须知道的事**：一、DNS 层的预匹配是**三值**的（P2）：要有连接才知道的条件——端口、对名字的 IP 类规则、`PROTOCOL`、`USER-AGENT`、`URL-REGEX`——答 `NeedsConnection`，逻辑规则按"能定就定"组合，`NOT` 只互换成立与不成立、"不知道"原样传出，不是设计原来写的跳过含端口条件的规则；它比"跳过"多出的两种命中（`OR` 的另一支不靠连接就成立，和 `NOT` 套在名字使之不成立的 `AND` 上，R17）结论都与端口无关；二、**一个 TUN 一个应答器**（R3），UDP 与 TCP 共用它的 256 个转发许可、池耗尽的 WARN 一个 TUN 一次；应答器在 UDP 上的应答是本地应答，`try_send` 给写任务、满了丢并计 `queue_full`、从不等空位（R12；TCP 上的应答写进连接，以 `dns_tcp_idle` 为限），连发一阵查询的用例要把 `writer_queue` 设得比这一阵大；三、**DNS over TCP 的关法**（R25）：先 `shutdown`、再读掉丢弃应用还在发的字节直到应用的 FIN 或 `dns_tcp_idle`、然后才丢流，不是"先 `shutdown` 再丢"（那会在 FIN-WAIT 里把 FIN 变成 RST，最后一个应答丢了）；应答的写以 `dns_tcp_idle` 为限；TCP 的应答长过 65,535 字节答 SERVFAIL（R19）；同时服务的连接至多 `dns_tcp_max` 条（缺省 128，R33）：分片在 `new_syn` 里、锁下不等待地占位（`Admission::claim_dns_tcp`，守卫 `DnsPlace` 随 `Purpose::Dns` 交给会话，到 `session::serve` 返回才还），没有位置的 SYN 回 RST 并计 `busy`；四、`TunOptions::udp_dns_idle` 不得大于 `udp_idle`，`TunOptions::check` 拒绝，把 `udp_idle` 调到 10 秒以下的调用者要同时调它；它的标志是粘住的（发过任何不是 53 端口的目标，此后永远按 `udp_idle`）；五、**被预匹配拒绝的名字进 INFO**（P18）：引擎的会话日志行 `session rejected by …` 带 `dst`，DNS 层的名字因此出现在 INFO 里（每条规则每代 5 分钟至多一行），与设计第 14 节要的名字只在 DEBUG 不一致，是项目所有者的问题；应答器自己的行只在 DEBUG；六、**SOCKS5 的 UDP 流的回包来源在引擎里换回 fake 地址**（R16），`rurge-inbound` 不依赖 `rurge-dns`、没有改；只在引擎知道回包应答哪一条流时才换：经 DIRECT 或 `wireguard`（它们的载体自己把名字解析成地址）时两条流发往同一个地址与端口，经载体是全锥的代理策略时（流按名字记；回包带着远端自己的地址，如 `socks5`、`ss`、`trojan` 的 UDP）同一个端口上发往名字的流不止一条，回包都保留真实的来源，这样的载体上只有一条流的话，别的主机从这个端口发来的包会被说成 fake 地址——与兼容性清单 9.1 节 TUN 一侧"同一个端口上同时对两个名字收发、出站又是全锥的载体时"那一行是同一个限制（`Routes::flow_for`，M3c 延后第 21 行）；每个目标一条连接的载体（`vmess`、`h2-connect`、`masque` 的 UDP）把回包的来源报成流写下的那个目标，没有这个限制；七、**经监听器来的明文 HTTP 请求保留 fake 地址**（R27）：换回名字的只是 `dst_host`，请求行、`Host` 与 `session.url` 仍是 fake 地址，经 `http` 策略时上游代理被要求连 fake 地址，`URL-REGEX` 与扩展匹配看到的也是，阶段 4 的 HTTP 引擎改，M4 起可见，M4 的设计决定要不要先做窄修（`turn_back` 换了 `dst_host` 的绝对形式请求，把请求行里的 authority 与 `Host` 照样换成名字）；"rurge 地址空间"是整个 `198.18.0.0/15` 与 `fd00:6152::/64`，经代理端口去 rurge 自己的地址得 `fake address not known`；八、预匹配的钩子**只在规则模式下答**（取活的出站模式）、**不看 `strict` 暂停**；出站模式是全局而全局策略不可用时引擎在拨号处按规则走，这样的拒绝落在拨号；自动升级按每个被拒的 A 或 AAAA 查询数。
M3c 记录里写着"主持这一轮的会话代项目所有者作的裁定"的各行，所有者可以改。
M3c 的提交写这段时在分支 `phase3-m3c` 上（计划 `b96093b`，任务 0 ～ 6 到 `e5c4b0d`，最终修正波的代码到 `dc5f9bc`，复审之后改注释的 `5137937`），从 `main` 的 `4cf60ba` 切出，没有推送。

### 下一步：M4（平台接入）

下一步是 M4（平台接入，总设计第 9 节：三个平台的设备、地址与路由、系统 DNS 的接管与恢复、防环、开关与生命周期、权限、接管暂停的 TUN 一半；它的设计还没有写）：写设计与计划时要接起来的有 M3a 延后第 2、11、13 行（真实设备上的数字、零窗口探测、`STOP_GRACE` 与最后一轮的复位，都是手工验收）与第 17 行的第二 ～ 四条（进片的队列按字节界、`shard_of` 的固定键散列改随机键、片重建的 WARN 限频）、M3b 延后第 4 行（真实的 `Pinger`）、第 6 行（未指定地址上的监听器的 `local`，手工验收）、第 8 行（停止不取消拨号：可执行文件停 TUN 时给等 `join` 的那一步一个自己的时限）、第 14 行的一条（"引擎放开客户端"的证据以 1 秒为界，与第 8 行一起）、第 23 ～ 25 行（`fake-ip.txt` 的权限、`start_tun` 的选项检查、同一个数据目录一次只起一个 TUN）、M3c 延后第 4 行（系统的 DNS 服务器名单要排除应答器与虚拟网卡的地址）与第 5、14、24 行（真实设备上的数字、一阵查询、整个 M3 一节的手工验收）、第 19 行（经监听器来的明文 HTTP 请求保留 fake 地址：M4 起可见，M4 的设计决定要不要先做窄修——`turn_back` 换了 `dst_host` 的绝对形式请求，把请求行里的 authority 与 `Host` 照样换成名字）、第 27 行（`dns_tcp_max` 的缺省 128 在真实设备上没有试过）；再推送一次 `main`、看第二次运行读出的六类失败修好没有，仍排在最前（上面的"一"）；发行说明里 2026-10-13 之后的测试版条目手册还没有写到，下一次重对从那里开始（重对计划延后第 8 行）。

### CI 在 `ac0a98a` 上第一次全绿（2026-10-26）

项目所有者把 `main` 推送到 `ac0a98a`：阶段 3 的 M1 ～ M3、`7bd6589` 那次运行读出的六类失败的修正与 `CLAUDE.md` 的整理都在里面。CI 的运行 40283632986（运行号 12，2026-10-26）**三个平台的 `check` 与 `bench` 都通过，这是 CI 第一次全绿**：运行号 1 ～ 11 都是失败。运行与各任务的结论读自公开的 API；任务日志要登录才能读，所以各平台跑了多少个用例没有读到。
**上面阶段 2「CI」一节里"CI 还没有绿过"与"还不知道的有四项"里的前两项（修正有没有用；macOS 与 Windows 上没跑到的四个包），以及各段里"第一次 CI 运行时核对""只在一台 Windows 开发机上跑过"的说法，凡这次运行跑到了的，以这一段为准**：M3 的全部用例、阶段 3 / M2 与 M8a 的修正第一次在三个平台上跑过并通过；`CLAUDE.md` 的 `unsafe_code` 一段说 macOS 的两个函数"第一次 CI 运行时核对"，这次运行在 macOS 上编译并通过了名单的用例。那些历史的段落不逐处改。
仍不知道的：第六类失败（Windows 上 `MockDns` 找不到同时空闲的端口）这一次没有出现，原因仍没有查明；一次全绿不说明计时用例在慢的机器上不会偶发失败（它们的余量没有在 CI 上量过）。此后的提交 CI 都没有见过。

### M4 的设计与计划（2026-10-26）

M4（平台接入）的细化设计与四份实施计划写好了，**代码一行也没有写**。执行交给 Pi（另一个代理工具），从 M4a 开始。
做了什么：设计 `docs/superpowers/specs/2026-10-26-phase3-m4-platform-design.md`（已决事项 M4-D1 ～ D30，代项目所有者作的裁定）；四份计划：`docs/superpowers/plans/2026-10-26-phase3-m4a-skeleton-plan.md`（平台无关的骨架，平台后端在每个平台上都答"这一版还不能"，端到端经测试后端）、`…-m4b-loop-dns-plan.md`（防环与接管期间的 DNS）、`…-m4c-linux-macos-plan.md`（Linux 与 macOS）、`…-m4d-windows-wrapup-plan.md`（Windows 与 M4 的文档收尾）。写计划时对着文档与源码核对，改了设计正文的地方列在设计第 21 节"写计划时的订正"（25 条），例如：Windows 先探后建、不复用残留的 Wintun 适配器；Linux 用 `route add`，与另一个全隧道 VPN 的相同前缀冲突时打开失败；`netsh` 的成败只由读回定；macOS 的 IPv6 地址用 `/128 alias`。写法的出处记在各计划「计划期决定」的最后一行（man7 的 ip(8) 与 resolvectl(1)、苹果的 ipconfig 源码、ss64 的 networksetup、微软的 `netsh interface` 与 Winsock、`WINTRUST_DATA`、`LoadLibraryExW`、`SwDeviceSetLifetime` 各页、Wintun 的 `wintun.h` 与 `adapter.c`）。上面「下一步：M4（平台接入）」一段列的、去向是 M4 的 M3 延后事项，各自的去处在设计第 15 节。
没做的，与不要当成已经有的：M4 的代码一行也没有；`rurge run` 照旧不起虚拟网卡；macOS `networksetup -listnetworkserviceorder` 的输出格式、中文系统上 `netsh` 的输出、Wintun 适配器的缺省 MTU、真的 `wintun.dll` 的签名者，写计划时都没有在机器上看过（计划里的夹具是手写的，标明了），由 CI、手工清单或 `RURGE_TEST_WINTUN_DLL` 的用例核对。
以后的会话必须知道的事：读 M4 设计正文之前先读它第 21 节。Pi 一份一份地做，每份的任务 0 是开工前核对，出入记进计划末尾的「执行期修正记录」，计划的正文不改；每份做完停下交项目所有者；M4c（任务 10）与 M4d（任务 8）各有一个等 CI 的停点，Pi 不推送。计划之间以前一份的修正记录为准。没有一条缺省的用例开设备或执行改网络的命令（M4c 计划 P19、M4d 计划 P19）。计划新增 16 个放开 `unsafe_code` 的函数（M4c 两个、M4d 十四个），名单与 `CLAUDE.md` 的那一段随加随改；Windows 的句柄存成 `usize`，不写 `unsafe impl`（M4d 计划 P4）。
去向写着项目所有者的：总设计 Q2（改用现成的包）、Q3（随附 `wintun.dll`）、Q4（L2 特权用例在 CI 上启用、macOS 与 Windows 的冒烟）、Q5（Windows 的 WFP）；推送；手工验收（M3、M4 两节都要真机）。
这些文档的提交在本地 `main` 上，没有推送。

### M4a（平台无关的骨架）

M4a 做完了（2026-10-26；计划 `docs/superpowers/plans/2026-10-26-phase3-m4a-skeleton-plan.md`，下面写"M4a 记录第 N 行""M4a 延后第 N 行"指它末尾的「执行期修正记录」与「延后事项」两张表，写"P N"指它的「计划期决定」）。**上一节「M4 的设计与计划」里"代码一行也没有写"那句以这一段为准**（那是当时的事实，规矩是历史的段落不逐处改、由后写的段落订正）：M4 的代码从 M4a 起开始写，M4a 这一份写完了，M4b ～ M4d 一行也还没有。M4a 由协调的会话逐个任务调 Pi 做、逐个任务评审，做完之后经整分支终审与一轮修正（2026-10-27），接着做 M4b，不停下等所有者评审（M4a 记录第 104 行）。

做了什么：一句话说，增强模式的生命周期在不碰系统的前提下整个做出来了——路由集的计算与三个平台的步骤（都是数据）、`state.json` 的备份与崩溃之后的恢复、可执行文件里的管理器与三个后端、`--enhanced-mode` 与 `GET` / `POST /v1/features/enhanced_mode`、退出的次序、接管暂停的增强模式一半、重载与网络变化的增量、`rurge check` 的四个新诊断、`external` 的 `addresses` 进路由的排除。**使用者仍然开不了增强模式**：平台后端对每个动作都答 `the enhanced mode is not available on this platform in this version`（见下面"没做的"）。细节在 `docs/api/phase3.md` 的「M4」一节；代码在 `crates/rurge-platform/src/route/`（`RouteSet` 与 `linux` / `macos` / `windows` 三个翻译）、`crates/rurge/src/cli/enhanced/`（`EnhancedMode`、`Backend`、`FileBackend`、`Undo`、`Wanted`、生产的 `Starter`）与 `crates/rurge/src/cli/run.rs` 的接线。
设计第 15 节归 M4a 的 M3 延后事项，各做在：**M3a 延后第 17 行的第二、三、四条 → 任务 1**（进片的队列按字节计，`TunOptions::queue_bytes` 缺省 32 MiB、超出按 `queue_full` 丢；`shard_of` 用每个 TUN 一个 `RandomState`，`DefaultHasher` 去掉了；片重建的 WARN 每片每分钟至多一行，其余 DEBUG）；**M3b 延后第 8 行与第 14 行的一条 → 任务 7 与任务 2**（可执行文件关闭时等 `join` 至多 3 秒，`JOIN_LIMIT`，用例 `closing_waits_for_the_tun_three_seconds_at_most`；"引擎放开客户端"的证据从 5 秒收到 `STOP_GRACE` 加余量，`an_association_returns_when_closed_and_lets_go_of_its_client`）；**M3b 延后第 23 ～ 25 行 → 任务 2**（`fake-ip.txt` 在 Unix 上以 `0600` 建出；`Engine::start_tun` 返回 `Result`、先 `TunOptions::check` 再查 MTU 下限 1280，不再 panic 着把池的文件留在 `clean 0`；一个引擎一个 TUN，`Engine::tun_ended()`）。
门禁：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过，`cargo test -j 8 --workspace --no-fail-fast` **81 个结果行、4430 通过、0 失败、13 忽略**（`rurge-proto-wireguard` 的 lib 二进制以 `STATUS_HEAP_CORRUPTION` 退出，单独重跑通过、数字算进去了；M4a 记录第 105 行）——一台 Windows 11 x86-64 的开发机上的一次运行，不是 CI。

没做的，与不要当成已经有的：

- **这一版 `rurge run --enhanced-mode` 在每个平台上都以 1 退出**（标准错误 `error: cannot turn the enhanced mode on: …`），`POST /v1/features/enhanced_mode` 带 `true` 回 500；真的能开是 M4c（Linux 与 macOS）与 M4d（Windows）之后。所有行为只经只供测试的文件后端端到端测过。
- **没有真实的设备**：这一版没有任何代码执行系统命令、建虚拟网卡、改路由或系统 DNS。三个平台的步骤只生成为数据，任何宿主上都编译、都测；执行它们、建真的设备、拿权限都在 M4c / M4d。`netsh` 与 `ip` 的写法按文档与手册页核对过，一条也没有在真机上执行过。
- **出口表没有读的人**：M4a 写出口表（`cli/enhanced` 的 `EgressSlot`），判定的那一半（`PlatformSockets` 的 `egress`）是 M4b。所以这一版没有防环。
- **ICMP 回显不转发**：TUN 的 `Pinger` 是 `NoPinger`（每个回显请求都立刻失败），M4b 换成真的套接字。
- **Linux 与 macOS 上 `0600` 的用例只在 CI 上编译运行**：`crates/rurge-dns/src/fakeip/file.rs` 的 `the_pool_file_is_its_owners_alone` 是 `#[cfg(unix)]` 的，这台 Windows 开发机上没有编译过（M4a 记录第 22 行说的同一件事）。
- 手工验收一项也没有做（`docs/acceptance/phase3-manual.md` 里还没有 M4 一节，只有 M1、M2、M3；M4d 写它，要验的东西列在设计第 16 节）；兼容性清单的 `ipv6-vif`、`tun-excluded-routes`、`tun-included-routes` 与 `external` 的 `addresses` 四行写的是"目前（阶段 3 / M4a）"，其余行随 M4d。
- **M4a 的提交 CI 都没有见过**（最后一次被 CI 见过的运行是 `ac0a98a` 上的 40283632986，此后没有推送）。

以后的会话必须知道的事：

- **`EnhancedMode::follow` 的次序与失败策略**（`crates/rurge/src/cli/enhanced/mod.rs`）：IPv6 从关到开是"网卡的地址 → 路由 → 系统 DNS → 系统自己的调整（只在换网络时）"，从开到关是"DNS → 路由 → 地址"（协调者的裁定，M4a 记录第 79 行）。一次 `follow` 至多接管一次系统 DNS（IPv6 变了与换网络合并成一次）。每一步失败只记一行 ERROR（`cannot follow the change; the enhanced mode goes on with what it has`，带 `step` 与 `error`），后面的照做，增强模式照开、已做的保留，`GET` 照实是 `true`。备份写不进去时紧接着的那一步不做。路由的 IPv6 一半失败（`routes` 回 `Ok(false)`）时别当空集：后端只撤它新加的那一半、照常减掉要减的，所以记下的是旧集减去要减的（与新的交集）；这一次之前 IPv6 没开着的这一次按没开往下做（DNS 以没有 IPv6 接管，与打开同一条规矩，M4a 记录第 95 行），本来就开着的照开着。
- **出口表始终是装进网卡的路由的超集**（协调者的裁定 R25，M4a 记录第 75 行）：路由那一步之前先写"旧的与新的并起来"的集合，成功之后写新的，失败就留着并集。M4b 的读者靠这一条不把 rurge 自己的连接引回虚拟网卡，改这一段之前先读它。
- **`Takeover` 的两半**（`run.rs`）：系统代理与增强模式各有一个"想要的"，启动参数与每一次得到成功应答的 `POST` 都能改它；进入暂停时两样都给出、记下想要，离开暂停时照想要的办，且**增强模式先于系统代理**。增强模式的关从不半途停下，所以关闭回 500 时它已经关了：`POST false` 不论成败都把"想要的"记成关（协调者的裁定 R35，M4a 记录第 97 行），离开暂停时只有"想要开着而关着"这一支要开，没有"想要关着而仍开着"那一支（那只是系统代理的）。启动时就在暂停的网络上两样都不开。设备自己坏了（`Ending::DeviceFailed`）时管理器把增强模式整个关掉、不重试，但 `Takeover` 记的"想要的"不变（P24）——设备坏一次不该改变使用者的意图。
- `StateStore::try_update` 与 `update` 各用在哪里：**备份的"加"用 `try_update`**（写不进就不做那一步，打开整个回退），**备份的"去掉"与 `features.enhanced_mode` 用 `update`**（写不进只记 ERROR，下次启动再撤销一遍，撤销对已经撤销过的东西无害）。备份只记会在崩溃之后留在系统里的动作（macOS 的 DNS、Linux 的 `rp_filter` 与文件后端的 `test`），设备、地址、路由不进；`features.enhanced_mode` 没有读者，它只是记录。
- `Starter` / `RunningTun` 是起 TUN 的接缝（P6）：生产里那个包 `Engine::start_tun`（`cli/enhanced/engine.rs`），用例里是假的。`RunningTun` 有两句话——`ended()`（关闭等它，含引擎最后保存池，关闭给这一步 3 秒的时限）与 `ending()`（等设备坏了的那个任务只看 TUN 被告诉结束，不占引擎那一个 TUN 的位置，也不等正在进行的拨号——`TunHandle::ending()` 在停止令牌被取消时回，M4a 记录第 62、100 行）。
- `Command::TunEnded(序号)`：序号归管理器（每次打开成功加一），主循环用 `is_current_open` 判断"是不是正开着的那一次"，早先那一次或关闭之后的失败不再处理。队列满时用一个任务把消息送进去，不丢。
- 起 `rurge` 二进制的辅助函数（`rurge_command()`、`rurge_process()`）缺省设 `RURGE_ENHANCED_BACKEND=off`：没有一条起 `rurge` 二进制的缺省用例能走到平台后端。要用文件后端的用例在构造函数之后自己设 `file:<路径>`；那个文件的写不是原子的，等的办法见 `docs/testing.md` 的「阶段 3 / M4a 起」。
- 崩溃之后的恢复（`EnhancedMode::recover`）在拿到实例锁之后、任何新改动之前，与系统代理的恢复同一个位置；配置读不了、有错误、日志文件打不开的那三个出口也做（标准错误四句之一，不改变退出码）。读不懂的备份三处都原样留着。备份为空而 `features.enhanced_mode` 为真时（Windows，或 Linux 不用改 `rp_filter`），恢复也把它写回 `false`（M4a 记录第 99 行）。撤销失败的 ERROR 行的 `step` 只写种类（`mac_dns` 不带服务名），完整的名字在紧跟的 DEBUG 行（M4a 记录第 98 行）。

去向写着项目所有者的（后来的会话不要自己定）：设计第 20 节的 Q2（设备、路由、网络环境改用现成的包，或开 `nix` 的 `socket` / `net` 特性）、Q3（发行包随附官方签名的 `wintun.dll`）、Q4（L2 特权用例在 CI 上启用、macOS 与 Windows 的冒烟）、Q5（Windows 防 DNS 泄漏的严格模式，WFP）；**推送**（M4a 的提交都在本地 `phase3-m4a` 分支上，`origin/main` 仍是 `ac0a98a`）；**手工验收**（M3、M4 两节都要真机，M4 一节要等 M4c / M4d）；P24 对设计第 9.4 节的订正（设备自己坏了的情形）**M4d 收尾时**写进设计的订正，M4a 不改设计正文。

### M4b（防环与接管期间的 DNS）

M4b 做完了（2026-10-27；计划 `docs/superpowers/plans/2026-10-26-phase3-m4b-loop-dns-plan.md`，下面写“M4b 记录第 N 行”“M4b 延后第 N 行”指它末尾的两张表，写“P N”指它的「计划期决定」）。**上一节 M4a 的两条以这一段为准**（那是当时的事实；规矩是历史的段落不逐处改、由后写的段落订正）：“出口表没有读的人”不成立了——出口判定的读者在这一份里（`PlatformSockets::egress`）；“ICMP 回显不转发”也不成立了——TUN 的 `Pinger` 现在是真的 `PlatformPinger`。M4b 由协调的会话逐个任务调 Pi 做、逐个任务评审，做完任务 10 停下，整分支终审与收尾由协调者做（与 M4a 记录第 104 行同一个做法）。

做了什么：一句话说，增强模式开着的时候 rurge 自己建的每一个出站套接字都不回到虚拟网卡，接管系统 DNS 的那段时间 `server:system` 一类的名字去问物理网卡的服务器：

- **出口的判定**（`crates/rurge-platform/src/socket/egress.rs`，只有数据）：`EgressTable` 加了直连子网（`subnets`），`decide` 的五条次序、`subnets_of`（每个子网一次、最长前缀先、同一个子网挂在两块网卡上留输入里先出现的那一块）、`subnets`（读 `if_addrs`）。
- **一处建法**：`SocketHook::egress`（缺省 `Ok(None)`）；`open_socket` 在策略没有 `interface`（或绑不上而 `allow-other-interface` 退回）时问出口、按回答绑网卡；`DirectPacket` 按（地址族，出口）各一个套接字、一个数据报只问一次；QUIC 直连的套接字按服务端地址问。
- **DNS**：纯 UDP 上游一律经 `ResolverDeps::direct`（带平台钩子的直连连接器，不随出站模式）；`SystemDns::enhanced_mode()` 与接管期间的四种去处（`server:system` / `syslib` 与单标签问物理网卡，`force-syslib` 与 `.local` 照旧问系统解析库）；没有服务器时以 `all upstreams failed (system DNS servers: no DNS server outside the enhanced mode)` 失败、不退回库；`Resolver::reread_system_servers` 与 `Engine::system_dns_changed`；系统的 DNS 名单（`without_rurge_space`）与网络环境的快照都去掉 rurge 的地址空间。
- **rurge 自己的名字**：`OwnResolve`（平时系统解析库，接管期间问这一代的解析器）、`resource_client_with`；守护进程的两个下载客户端（外部资源与 Profile 来源）都改成了带钩子与 `OwnResolve` 的那一个。
- **可执行文件**：有状态的 `PlatformSockets`（读出口表的槽）与 `PlatformSystemDns`（增强模式的标志与 `physical`），`Runtime` 建一次、每一代克隆同一份 `Arc`；管理器的 `Links` 写出口表、标志与网络任务的排除名单，并在标志变了、接管、交还之后调 `system_dns_changed`；后端接口多了 `subnets`（文件后端从文件里读）。
- **真实的 `Pinger`**：`rurge_platform::icmp`（建套接字、报文与应答的解读）与 `crates/rurge/src/cli/pinger.rs` 的 `PlatformPinger`（按出口绑、按序号分派）；M4a 的 `NoPinger` 与它的那一条用例删掉，换成 `pings_the_loopback`。
- **外部程序一律 DIRECT**：`ProcessGroup::contains` / `ProcessTree::contains`（Unix 是进程组，Windows 在 M4d 之前恒假）、`ExternalOutbound::runs` / `running`、`ExternalPrograms::any_running` / `runs`，引擎的 `choose_policy` 在看出站模式之前先看外部程序。
- **漏网的检查**：`crates/rurge/tests/sockets.rs` 的源码扫描（`LISTED` 现在 24 行、逐处比对，见下面的修正一轮）与 `crates/rurge-engine/tests/egress.rs` 的五个运行时用例。

**使用者仍然开不了增强模式**：平台后端照旧对每个动作答 `the enhanced mode is not available on this platform in this version`，上面这些只经单元用例、运行时用例、源码扫描与只供测试的文件后端看得见。细节在 `docs/api/phase3.md` 的「M4b」一节。

门禁：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过，`cargo test -j 8 --workspace --no-fail-fast` **83 个结果行、4501 通过、0 失败、13 忽略**（一台 Windows 11 x86-64 的开发机上的一次运行，不是 CI；没有二进制以 `STATUS_HEAP_CORRUPTION` 退出、没有重跑；与任务 9 的基线相同，M4b 记录第 66 行）。

**整分支终审之后的一轮修正**（2026-10-27，同一个做法；M4b 记录第 67 ～ 77 行与延后第 13 ～ 17 行）：`reread_system_servers` 也重建 `[Host]` 的上游；活得比一次会话长的套接字（WireGuard 的载体、QUIC 的连接）靠 `Connector::route()` 跟上出口表，`set_egress` 只在表的内容变了时才换 `Arc`，跟随写了另一张表而不重新接管 DNS 时解析器的上游按新表重建；`decide` 先把 IPv4 映射地址规范化；Profile 来源问出口的运行时用例；扫描改成逐处比对、多认几种写法（名单 24 行）；外部程序的 PID 只进 DEBUG。门禁：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过，`cargo test -j 8 --workspace --no-fail-fast` **83 个结果行、4513 通过、0 失败、13 忽略**（同一台开发机上的一次运行，不是 CI；没有二进制以 `STATUS_HEAP_CORRUPTION` 或 `STATUS_ACCESS_VIOLATION` 退出、没有重跑；比上一次多 12 条，就是这一轮新加的那十二条）。

没做的，与不要当成已经有的：

- **这一版 `rurge run --enhanced-mode` 在每个平台上仍然以 1 退出**（M4a 那一条照旧）；真的能开是 M4c（Linux 与 macOS）与 M4d（Windows）之后。
- **`physical` 没有人写**：`PlatformSystemDns::servers()` 里“接管期间给物理网卡的服务器”那一半是 M4c 的（macOS 写备份里的原值与 `ipconfig getpacket` 的，Linux 写 resolved 的上游名单；Windows 不需要它），所以今天两边给出的是同一份名单。
- **Windows 的 `ProcessTree::contains` 恒假**（P15）：`runs(pid)` 只认程序自己的 PID，程序拉起的子进程的会话要等 M4d 的 `in_job`；`crates/rurge-platform/src/process.rs` 的 `#[cfg(windows)] contains_is_false_until_m4d` 断言的就是“程序本身不在树里”，M4d 接上 `in_job` 时要换掉（M4b 延后第 3 行）。
- **本机没有编译过的 Unix 代码**（这台 Windows 上编不到、也跑不到，第一次 CI 核对）：`rurge_platform::icmp::open` 的 Linux / Android 一份（先数据报、不行退原始）、macOS 一份（数据报）与“其余平台恒失败”一份（后一份 CI 的三个平台也编不到，所以“第一次 CI 核对”对它不成立，M4b 延后第 17 行）；`ProcessTree::contains` 的 `#[cfg(unix)]` 一段（`nix::unistd::getpgid`）与它的用例 `a_process_tree_contains_its_leader_and_what_it_started_and_nothing_else`；`crates/rurge-proto/src/external.rs` 的测试助手 `long_runner` 的 `#[cfg(unix)]` 一支（起 `sleep`）；`tests/external/tests/engine.rs` 的用例 `what_the_program_started_goes_direct_too`。
- **`pings_the_loopback` 只在本机那一条路上跑过**：这台 Windows 上原始套接字开得起来，`127.0.0.1` 与 `::1` 都真的来回了一次、载荷相等（M4b 记录第 45 行）；Linux / Android 的数据报套接字与它退到原始套接字的那一路、macOS 的数据报套接字、“开不了套接字所以跳过”这条路没有任何平台跑过；macOS 的数据报套接字按标识符与序号对号（协调者的裁定 R22，M4b 记录第 46 行）也没有在 macOS 上观察过。
- **防环没有在真机上验过**：`rurge_platform::socket::egress::subnets` 读本机网卡的那一步没有用例走到（要真的网卡与出口表，随 M4c 接上平台后端）；绑上网卡这一事只用“记下不真绑”的假钩子与数据用例测过；`PlatformPinger` 的 `Some(网卡)` 那一支与收包退避也没有用例。设计第 14 节的 L2 特权用例还没有写。
- 手工验收一项也没有做（`docs/acceptance/phase3-manual.md` 里还没有 M4 一节，只有 M1、M2、M3；M4d 写它，要验的东西列在设计第 16 节），M4b 把 `ping`、防环、非直连的 VPN 路由三项加进了那张单子要看的东西里。
- **M4b 的提交 CI 都没有见过**（最后一次被 CI 见过的运行是 `ac0a98a` 上的 40283632986，此后没有推送）。
- **表变之前建的每个会话的套接字不重建**：已经建立的 TCP 连接（含 DoH、HTTP/2、SSH 的连接池）与 UDP 中继、旧 QUIC 连接上已经在走的流，在表变的那一刻（增强模式打开、或者换了网络）经虚拟网卡绕一次或断一次；WireGuard 的包最晚到下一个 `TICK`（至多 250 毫秒）才按新表发出。去向：手工验收与用户报告（M4b 延后第 16 行）。
- **直连子网只在打开与跟随时读**（设计第 7.1 节）：打开之后才出现的次网卡（`docker compose up` 建的网桥、后接的第二块网卡、USB 共享）的子网要到下一次跟随（重载或网络变化）才进表，在那之前 rurge 自己到那里的连接绑在主网卡上、到不了（M4b 延后第 14 行）。

以后的会话必须知道的事：

- **出口判定的次序与两句话**（`crates/rurge-platform/src/socket/egress.rs`）：表是空的 -> 不绑；回环、未指定、组播、链路本地与广播 -> 不绑；直连子网（取最长前缀）-> 那块网卡；路由集（含 rurge 的空间）-> 主网卡，没有主网卡时这次连接失败；其余 -> 不绑。**直连子网那一条在路由集那一条之前**：`tun-included-routes` 把一段局域网送进虚拟网卡时，rurge 自己到局域网的连接仍走局域网的网卡。绑不上出口的网卡时连接失败、**不退回不绑**（退回就进虚拟网卡）；进请求记录与 API 的错误是 `cannot bind to the network outside the enhanced mode: <错误种类>`，网卡名与系统的原话只在 DEBUG（P3，M4b 记录第 16 行）。
- **活得比一次会话长的套接字靠 `route()` 跟上出口表**（M4b 记录第 68、69 行）：直连的连接器的 `route()` 回 `SocketHook::egress_route()`，增强模式开着时它是 `Some(Route::egress(&表))`、没有表时是 `None`；WireGuard 的载体每个 `TICK`（250 毫秒）、QUIC 的连接每次取用都比一次，不同就重拨（INFO 换成“the egress of rurge’s own sockets changed”那一句）。管理器只在**表的内容变了**时才往槽里写另一张表（`set_egress` 内容相同时留着同一个 `Arc`、回 `false`），否则每一次重载都会让每个隧道与 QUIC 连接重拨；跟随写了另一张表而这一次不重新接管 DNS 时，仍调一次 `system_dns_changed` 让解析器的上游按新表重建。新加**活得长的套接字**时，要么同样跟着 `route()`、要么在表变时自己重建。
- **`SocketHook::egress` 的缺省是 `Ok(None)`**：所以没有平台钩子的地方（`rurge-net` 的其余用例、`DirectConnector::new`）行为一字不变。改名或改签名时 `crates/rurge-net/src/connector.rs` 的 `open_socket` 的三个调用者（`connect_one`、`bind_udp`、`connect_udp`）与 `DirectPacket` 一起看。
- **`ResolverDeps::direct` 不随出站模式**：纯 UDP 上游（配置的、系统的、引导的、`[Host]` 的 `server:`）一律经它——带平台钩子的直连连接器；`encrypted-dns-follow-outbound-mode` 只管加密上游，别把 `direct` 换成流水线的连接器。
- **`OwnResolve`**：`enhanced_mode()` 且这一代的解析器已发布时问它，否则问系统解析库。守护进程的两个下载客户端用 `resource_client_with`；`resource_client()` 只有一个生产调用者——`crates/rurge/src/cli/run.rs` 的 `wait_for_the_sources`：`rurge run` 启动时、引擎建起来之前下载 Profile 的远程来源（托管配置、远程 include），与用例（P10）。那一次下载可以不带钩子：那时引擎还没有、增强模式还开不了，出口表是空的，带了钩子也只会答“不绑”（`rurge check` 一类不起引擎的命令不下载，也不调它）。扫描用例的名单里写着它。
- **`system_dns_changed` 与 `network_changed` 是两件事**：前者是接管 / 交还（重读系统的服务器、**不清缓存**，经接口的上游随之清掉），在标志变了、接管、交还之后各调一次；后者是网络真的变了（清缓存、读 IPv6、重读服务器、清 `[Host]` 的上游），照旧只在网络真的变了时调。
- **扫描用例的名单**（`crates/rurge/tests/sockets.rs` 的 `LISTED`）：加一处建套接字、问系统解析库或建不带钩子的连接器（`NoopSocketHook)`）的地方，就加一行（文件、写法、理由写它是哪一个函数）；M4c / M4d 加设备的套接字时同样（macOS 的 utun 控制套接字）。名单与找到的写法**逐处**比对（不按（文件，写法）去重），同一个文件里同一种写法的第二处也要一行；一行只读 `//` 之前的部分（`docs/testing.md` 的「阶段 3 / M4b 起」一段，M4b 记录第 73 行）。
- **`DirectPacket` 一个数据报只问一次出口**：那次回答决定新套接字绑哪块网卡，也是它在列表里的键（M4b 记录第 17 行）；建它时不经 `open_socket`，所以不会问两次。
- **外部程序的那一段在 `choose_policy` 的最前面**（在看出站模式之前）：所以程序在跑时，**直连与全局模式的会话也等一次进程**（上限 `PROCESS_WAIT`，P16、协调者的裁定 R17）；`notes` 的 `Requested by an external program: always DIRECT` 在 `crates/rurge-engine/src/owner.rs`，PID 只在 `choose_policy` 的那一行 DEBUG（M4b 记录第 50 行）。Windows 在 M4d 之前只认程序本身。
- **ICMP 的两种对号规则**：`Kind::keeps_ident`（原始套接字恒真；数据报套接字只在 Linux / Android 上为假）；新增平台时看它的内核改不改 `icmp_id`。`PlatformPinger` 的 WARN 一个实例只报一次；载体（`Carrier`）按（地址族，出口）只增不减（M4b 延后第 4 行）。
- 测试的规矩照 M4a 那一条：`rurge_command()` / `rurge_process()` 缺省 `RURGE_ENHANCED_BACKEND=off`；`tests/egress.rs` 的 `BOUND` 2 秒、`QUIET` 300 毫秒的来由在 `docs/testing.md`。

去向写着项目所有者的（后来的会话不要自己定）：**推送**（M4b 的提交都在本地 `phase3-m4b` 分支上，`origin/main` 仍是 `ac0a98a`）；推送之后第一次 CI 的 Linux / macOS 运行要核对上面“本机没有编译过的 Unix 代码”与 ICMP 的两份（M4b 延后第 2、5 行）；**手工验收**（M4 一节要 M4c / M4d 之后，M3、M4 两节都还没做）；设计第 20 节的 Q2（改用现成的包或开 `nix` 的特性）、Q3（发行包随附 `wintun.dll`）、Q4（L2 特权用例在 CI 上启用、macOS 与 Windows 的冒烟）、Q5（Windows 防 DNS 泄漏的严格模式）照旧。

### M4c（Linux 与 macOS）

M4c 做完了（2026-10-27；计划 `docs/superpowers/plans/2026-10-26-phase3-m4c-linux-macos-plan.md`，下面写“M4c 记录第 N 行”“M4c 延后第 N 行”指它末尾的两张表，写“P N”指它的「计划期决定」）。**上一节 M4b 的四条以这一段为准**（那四条是当时的事实；规矩是历史的段落不逐处改、由后写的段落订正）：“这一版 `rurge run --enhanced-mode` 在每个平台上仍然以 1 退出”不成立了——**Linux 与 macOS 上有了平台后端的代码（不再答“不可用”），照代码会开增强模式**；其中开设备的代码与挑平台的 `cfg` 一次也没有编译过（这台开发机编译不到，CI 还没有见过），动系统的那一半在这台开发机上编译、只用假的执行器测过，真机上都没有跑过；“`physical` 没有人写”不成立了——两个平台各有一段（见下面）；“防环……随 M4c 接上平台后端”里的那半句也不成立了——`PlatformBackend::subnets` 现在真的读本机网卡，三条 L2 用例就是为它写的（但没有运行过，所以 M4b 延后第 11 行仍然开着）；“设计第 14 节的 L2 特权用例还没有写”不成立了——写好了，没有运行过。M4c 由协调的会话逐个任务调 Pi 做、逐个任务评审；任务 10（等 CI）与 M4d 的任务 8 挪到最后一起做（协调者的裁定 R3，M4c 记录第 9 行），所以做完任务 9 接着做任务 11，不在任务 11 之后停下。

做了什么：一句话说，这两个平台的增强模式从“只生成为数据”变成有了动系统的代码（照代码会建虚拟网卡、配地址、装路由、接管系统 DNS 并交还、需要时改 Linux 的 `rp_filter`，以及接管期间问物理网卡的哪些服务器），但开设备与挑平台的代码一次也没有编译过、动系统的一半只用假的执行器测过，真机上都没有跑过（一次也没有编译过的是哪些，见下面「本机没有编译过的代码」一条）：

- **设备的接口与帧**（`crates/rurge-platform/src/tun/`）：`mod.rs` 定接口约定（`open` / `read` / `try_read` / `try_write` / `no_room`）与平台无关的三块——`drain`（先把等到的那一个放进去，再把已经在了的搬出来）、`write_all`（写不进就丢、计数，第一次 WARN、之后 DEBUG）、`RX_MAX` 64 KiB；`frame.rs` 不带 `cfg`（`ifname`、`name_of`、`packet_len`、`utun_header`、`utun_packet`、`utun_to_front`、`explained`），在本机能测；`unsupported.rs` 给没有设备的平台。
- **两个平台真的设备**：Linux 的 `/dev/net/tun`（`tun/linux.rs`，`set_iff` 包 `ioctl(TUNSETIFF)`、`IFF_TUN | IFF_NO_PI`）、macOS 的 utun（`tun/macos.rs`，`utun_connect` 包 `ioctl(CTLIOCGINFO)`、`connect` 与 `getsockopt(UTUN_OPT_IFNAME)`）；两个 `unsafe` 函数进 `LISTED`（现在 9 个），`CLAUDE.md` 同步。
- **系统一侧不带 `cfg`**（P2）：`enhanced/linux.rs` 的 `LinuxSystem`（`ip -batch` 配地址与路由、`resolvectl` 逐链路接管系统 DNS、`/proc` 里的 `rp_filter`、resolved 的上游名单）与 `enhanced/macos.rs` 的 `MacSystem`（`ifconfig` / `route` / `networksetup` / `dscacheutil`、`-listallnetworkservices` / `-getdnsservers` / `-listnetworkserviceorder` / `ipconfig getpacket` 的解析、每个网络服务的备份与写回）在这台 Windows 上也编译、也测；`enhanced/mod.rs` 的 `run_routes`（加、IPv6 一半失败时撤那一半、再减）两边共用（R15，M4c 记录第 29 行）。
- **执行**：`route/exec.rs`（`TimedRunner` 每个程序 30 秒，P11；脚本写进数据目录的 `enhanced/`、`create_new`、用完删掉、崩溃留下的下一次 `open` 删，P12），`route/macos.rs` 的 IPv6 地址用 `/128 … alias`（P5）、`resolvectl default-route` 失败不算错（P4）、Linux 用 `route add` 不 `replace`（P3）。
- **可执行文件接上**：`backend(data_dir)` 建 `rurge_platform::enhanced::system(data_dir.join("enhanced"))`，`PlatformBackend` 每个方法转给它，`PlatformDevice` 把 `TunDevice` 的 `recv` / `send` 包成 `BoxFuture`；`Undo` ↔ `Saved` 的映射是 `saved_of` / `undo_of`，平台后端不认识的撤销动作 WARN 一行丢掉（P9）。`Backend::physical_dns` 加了，管理器在三处写它的结果（R6，M4c 记录第 31 行），读的那一步没回来时写空列表而不是 `None`（R19，第 33 行）。`wait_for_the_sources` 拿到实例锁之后、下载 Profile 的远程来源之前先恢复一次（R10，第 32 行）。M4a 的 `the_platform_backend_is_not_available_in_this_version` 删掉（P19：从 M4c 起它会走到平台后端的设备代码，不再是一条缺省能跑的用例）。
- **L2 特权用例**：新的工作区成员 `tests/privileged`（`rurge-privileged-tests`，`publish = false`）——两个网络命名空间、`l2-helper`、三条用例（TCP 往返与 `ping` 经虚拟网卡出去、rurge 自己的 DNS 查询不进虚拟网卡、`kill -9` 之后下一次启动撤销）（P17，协调者的裁定 R17、R20、R21）。**写好之后没有在任何地方运行过。**
- **CI 的 clippy 日志**（任务 1，P18）：clippy 一步有名字、输出写进文件，失败时报告脚本读它、发成注解——M4c 的 Linux / macOS 编译错误因此公开可读（M4c 记录第 10 行）。
- **文档**（任务 11）：`docs/api/phase3.md` 的 M4c 一节、`docs/testing.md` 的「阶段 3 / M4c 起」、`CLAUDE.md` 的「现在在哪里」、兼容性清单 9.1、本节的附录。

门禁：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过，`cargo test -j 8 --workspace --no-fail-fast` **86 个结果行、4583 通过、0 失败、13 忽略**（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI；工作区的 `gate-task-11.log`；与 M4c 任务 9 的基线相同，这一次只改文档与 `steps.rs` 里的一处注释）。

**整分支终审之后的一轮修正**（2026-10-27，同一个做法；M4c 记录第 40 ～ 49 行与延后第 6 ～ 10 行）：`rp_filter` 只在 `adjust_undo` 要或备份里已有时才放宽（F1）；L2 的崩溃恢复用例在主路径写在标准输出的那一句上等、对 `all` 有界轮询（F2，没有编译过、没有运行过）；做了一半的接管也当接管过、`give_dns_back` 照撤（F3）；关闭时设备先关、再写回 `rp_filter`（F4）；带脚本的步骤的错误不带脚本的路径（F5）；接管时关掉 rurge 链路上的 DoT、DNSSEC、LLMNR、mDNS，`flush-caches` 失败不算错（F6）；Linux 的权限提示只说 root（F7）；macOS 接管之前的空名单当作不知道（F8）。门禁：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过，`cargo test -j 8 --workspace --no-fail-fast` **86 个结果行、4590 通过、0 失败、13 忽略**（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI；工作区的 `gate-final-fix.log`；比上一次多 7 条，就是这一轮新加的：F1 三条、F3 ～ F5 与 F8 各一条；F6 改的是两条现有用例的期望，F7 改的是一条现有用例的期望）。

没做的，与不要当成已经有的：

- **两个平台的设备与系统改动没有在真机上跑过**：Linux 与 macOS 开设备与挑平台的代码在这台 Windows 开发机上编译不到，CI 还没有见过 M4c 的提交（最后一次被 CI 见过的运行是 `ac0a98a` 上的 40283632986）；系统一侧只在开发机上用假的执行器与临时目录测过，`ifconfig` / `route` / `networksetup` / `ip` / `resolvectl` 一条也没有在真机上执行过。
- **L2 的用例没有运行过**：`tests/privileged` 写好、在本机只编译到 `#![cfg(target_os = "linux")]` 之后是一个空文件；在虚拟机上跑要项目所有者决定（总设计 Q4）。
- **`PlatformDevice` 的两个 future 在 Linux / macOS 上是不是 `Send` 没有编译过**（任务 8 的顾虑）：`rurge_net::BoxFuture` 要求 `Send`，而 `crates/rurge/src/cli/enhanced/device.rs` 的 `Box::pin(self.0.recv(…))` / `send(…)` 要等 `TunDevice` 的那两个 `async fn` 在带 `cfg` 的平台上也是 `Send`；只有 CI 的 Linux 与 macOS 编得到那一段（`recv` 里那把 `Mutex` 的守卫在 `await` 两侧各自取、各自放，正是为此）。
- **`physical` 交给管理器的那一行没有端到端用例**：`run.rs` 里把它写进 `Links` 的那一处没有用例走到（`server:system` 在测试里会去问这台机器真的系统 DNS，失败时就离开了回环，`CLAUDE.md`「测试约定」）。
- **M4b 延后第 11 行仍然开着**：`rurge_platform::socket::egress::subnets` 读本机网卡的那一步现在有 `PlatformBackend::subnets` 的调用者，但只有 L2 的用例会真的走到它，而那些用例没有运行过；防环没有在真机上验过。
- **macOS 的 `-listnetworkserviceorder` 的输出格式与公开的样本对得上**（两个出处：Apple Support Communities 的帖子贴的原样输出，每个服务两行 `(N) 名字` 与下一行 `(Hardware Port: …, Device: …)`，VPN 服务的 `Device:` 可以是空的、服务名里可以有括号；Jamf Nation 的帖子是另一种排版的同一份），`numbered_service` 与它对得上；服务与网卡的配对与 `ipconfig getpacket` 周围的行走仍要真机核（M4d 的手工清单；M4c 记录第 49 行、延后第 2 行仍然开着）。
- **终审的四处存疑在手工清单上**：`networksetup` 失败时的退出码、一个服务读得了原值却改不了、运行期间网络服务改名、NetworkManager 会不会盖掉 rurge 链路上的逐链路设置（Q1 ～ Q3、Q6；M4c 延后第 6 ～ 9 行），都要在 M4 一节的手工验收里核。
- **Windows 照旧不能开**：平台后端在 Windows 上仍是 `Unsupported`，`rurge run --enhanced-mode` 以 1 退出、`POST /v1/features/enhanced_mode` 回 500（M4d）。
- **手工验收没有做**：`docs/acceptance/phase3-manual.md` 还没有 M4 一节（只有 M1、M2、M3），M4d 写它。
- **本机没有编译过的代码**（第一次 CI 核对）：`tun/linux.rs` 与 `tun/macos.rs` 的全部，`tun/mod.rs` 与 `enhanced/mod.rs` 里挑平台的 `cfg`（`system()` 的两支与 `mod` / `use` 的三支），`tests/privileged/tests/linux_netns.rs` 的 `#![cfg(target_os = "linux")]` 之后那一大段（它在本机是一个空文件，0 条用例）。

以后的会话必须知道的事：

- **设备的约定**（`crates/rurge-platform/src/tun/mod.rs` 开头）：平台模块只给 `open` / `read` / `try_read` / `try_write` / `no_room` 五样，包在缓冲开头、长度是包的自己的。读的都经 `frame::packet_len`——**读到 0 个字节是设备没了**（`UnexpectedEof`），不是“跳过再读”：内核只在缓冲长度为 0 时回 0，就绪一直置着的话跳过就是空转（协调者的裁定 R12、R14，M4c 记录第 27、28 行）。**写不经 tokio 的就绪**：描述符只注册 `READABLE`，`try_write` 直接在非阻塞描述符上写；经 `AsyncFd::try_io(WRITABLE)` 时缓存的就绪没置上就根本不写、直接回 `WouldBlock`，`write_all` 会把好包当“没有地方”成批丢掉。macOS 的写是 `send_vectored(&[头, 包])`，读到的东西经 `frame::utun_to_front` 挪到缓冲开头。
- **IPv6 配不上的告警只有一处**（R15，M4c 记录第 29 行）：地址那一半由管理器写 `IPv6 could not be configured; the enhanced mode goes on without it`，路由那一半由 `run_routes` 写 `the IPv6 routes could not be added to the virtual adapter; they are left out`；`route::exec::run` 对 `Ipv6Off` 的失败只写 DEBUG。加新的“IPv6 没配上”的地方照这个分工。
- **`resolv.conf` 尽力读**（协调者的裁定 R13，M4c 记录第 19 行）：`dns::parse_resolv_conf` 走 `resolv_conf::Config::parse_with_errors`，解析得到的行照用，解析不到的行合成一句进 DEBUG——一个不认识的选项不让整份名单变空。两个平台的存根判断、`physical` 与网络环境都经它。
- **`physical` 的写入点与 `None` 的意思**（R6、R19，M4c 记录第 31、33 行）：管理器在三处读它——打开的第 7 步（`take_dns` 回来之后、回调之前）、网络变化时的 `take_the_dns_again`（重载不读）、关闭的 DNS 那一段末尾（写 `None`）。`None` 只来自后端自己该答它的几种（Linux 还没接管、文件后端没有那个键、`RURGE_ENHANCED_BACKEND=off`、以后的 Windows），意思是“机器自己的列表是对的”；**读的那一步没回来（阻塞任务没了）是 `Some(空)`**，查找失败、不退回机器自己的列表。
- **拿到实例锁先恢复的两处**（R10，M4c 记录第 32 行）：`recover_without_a_log` / `Lock::recover`（启动时的三处出口与持有锁的那条路）与 `wait_for_the_sources`（下载 Profile 的远程来源之前）。`Lock::Taken(None)` 什么也不做，恢复两次无害。
- **系统一侧不带 `cfg`，设备与 `system()` 带**（P2）：新加的系统一侧代码经 `CommandRunner` 与根目录（或 `resolv.conf` 的路径）进来，用例用 `crate::command::testing::FakeRunner` 与 `tempfile::TempDir`；只在一个平台上用到、又不带 `cfg` 的项做成 `pub`，否则别的平台 `dead_code` 在 `-D warnings` 下变红。
- **`TimedRunner` 与脚本**（P11、P12）：程序一律绝对路径、每个 30 秒为限；脚本在数据目录的 `enhanced/` 下、`rurge-steps-<pid>-<n>.txt`、`create_new`、用完删掉、崩溃留下的下一次 `open` 删。`networksetup` 的命令行里有使用者自己起的服务名，所以带服务名的失败只回固定的一句（P7）。
- **Linux 用 `route add`，不用 `replace`**（P3）：`replace` 会悄悄换掉另一个程序装、前缀相同的路由，关闭时再删掉它；`add` 遇到相同前缀报 `File exists`，打开失败、倒着撤销，使用者看得到冲突。macOS 的 IPv6 地址是 `/128` 加 `alias`（P5），范围路由只由 rurge 显式装。
- **没有一条缺省的用例开设备**（P19）：`TunDevice::open` 与真实平台的 `System::open` 不被任何缺省用例调用（Linux / macOS 的 CI 上它们是真的，以 root 跑就真的建网卡）；要测打开之后的事用文件后端或 `Unsupported`。
- **撤销的两种例外**（P9）：服务已经被删掉算做完（INFO 一行）；别的平台的撤销动作与平台后端不认识的 `Test` WARN 一行丢掉——留着的话每次启动都失败一次，永远去不掉。`Saved` / `Undo` 的映射在 `crates/rurge/src/cli/enhanced/backend.rs` 的 `saved_of` / `undo_of`，加新的 `Saved` 时三处一起改（`Undo`、平台一侧、管理器的备份）。
- **没有 resolved 的 Linux 与读不了原值的 macOS 服务不接管**（P4、P8）：`take_dns` 回 `Ok(false)` 是“这一次没有系统 DNS”，不是失败；一次打开只 WARN 一次。`resolvectl default-route` 失败不算错。
- **CI 的 clippy 错误在注解里**（P18，M4c 记录第 10 行）：报告脚本读 `clippy.log` 或 `test.log`，`error[E….]:` 与 `error: ` 两类行发成 `::error title=compile error::`。任务日志要登录才能读，注解不用。
- **`rp_filter` 只在需要或备份里已有时放宽**（F1，M4c 记录第 40 行）：`adjust` 由管理器在打开的第 8 步与 `adjust_again` 里按“这一次 `adjust_undo` 回了 `Some`，或者备份里已有 `Stage::Adjust` 的一项”调；`on.adjust = true` 照旧说这一阶段到过。不需要放宽的机器（有效值不是 1、打开时还没有主网卡）既不改全机的设置、也不留下没有备份的改动。
- **带脚本的步骤的错误不带脚本路径**（F5，M4c 记录第 44 行）：`run_step` 把错误里脚本路径的每一处换成 `…`（命令行里的与 `ip -batch` 原文里的都换），路径只在 DEBUG。
- **接管时关掉 rurge 链路上的四项**（F6，M4c 记录第 45 行）：`take_dns` 先 `llmnr`、`mdns`、`dnssec`、`dnsovertls` 各设 `no`（都是 `Ignore`）再设服务器；`flush-caches` 也是 `Ignore`，失败只是缓存里已有的名字到期之前照旧是真地址。
- **Linux 的权限提示**（F7，M4c 记录第 46 行）：`the enhanced mode needs root (in a container, root with CAP_NET_ADMIN)`——`ip` 与 `resolvectl` 是外部程序，`setcap` 给 rurge 的文件能力到不了它们。
- **两份名单**：`crates/rurge/tests/sockets.rs` 的 `LISTED` 现在 25 行（macOS 的 utun 控制套接字那一行），`crates/rurge-platform/tests/unsafe_functions.rs` 的 `LISTED` 现在 9 个函数（加了 `set_iff` 与 `utun_connect`）——加建套接字的地方、加放开的函数时照 `docs/testing.md` 的规矩改。

去向写着项目所有者的（后来的会话不要自己定）：**推送**（M4a、M4b、M4c 的提交都在本地，`origin/main` 仍是 `ac0a98a`；推送之后 M4c 的任务 10 与 M4d 的任务 8 一起读回 CI，先修掉它列出的失败）；**手工验收**（M4 一节要 M4c / M4d 之后，M3、M4 两节都还没做；macOS 的 `-listnetworkserviceorder` 与 `ipconfig getpacket` 的实际输出也在那一份单子里核对）；**错误里 rurge 自己的网卡名、地址与前缀**（含 `tun-excluded-routes` 的前缀；非 `networksetup` 步骤失败时它们进 ERROR、`rurge run` 的标准错误与 API 的 500）：要不要照 P7 改成固定的一句、原文进 DEBUG（M4c 延后第 10 行）；**L2 特权用例在 CI 上启用**（总设计 Q4）与设计第 20 节的 Q2（改用现成的包或开 `nix` 的特性）、Q3（发行包随附 `wintun.dll`）、Q5（Windows 防 DNS 泄漏的严格模式）照旧。

### M4d（Windows 与收尾）

M4d 做完了（2026-10-27；计划 `docs/superpowers/plans/2026-10-26-phase3-m4d-windows-wrapup-plan.md`，下面写“M4d 记录第 N 行”指它末尾的「执行期修正记录」，写“P N”指它的「计划期决定」）。**上一节 M4c 的三条以这一段为准**（那是当时的事实；规矩是历史的段落不逐处改、由后写的段落订正）：“Windows 照旧不能开”不成立了——Windows 的平台后端不再是 `Unsupported`，它建 Wintun 的虚拟网卡；“三个平台都没有在真机上验证”照旧成立；“M4c 的提交 CI 都没有见过”的范围扩到 M4d。M4d 由协调的会话逐个任务调 Pi 做、逐个任务评审；这一轮做的是任务 9（M4 的文档收尾），任务 8（推送之后读 CI）不在这一轮，与 M4c 的任务 10 一起等所有者推送。

做了什么：一句话说，Windows 上 `rurge run --enhanced-mode` 有了完整的代码（照代码会建名叫 `rurge` 的 Wintun 适配器、配地址与 MTU、装路由、把系统 DNS 指向应答器），M4 的文档也收尾了；**设备与提示没有在任何地方运行过**。逐项：

- **防环的 Windows 一半**（任务 1）：`crates/rurge-platform/src/socket/mod.rs` 的 `unicast_if`（`IP_UNICAST_IF` 用网络字节序、`IPV6_UNICAST_IF` 用主机字节序；设不上只记一行 DEBUG、不把连接打掉，R11），`bind_interface` 在 Windows 上接上它；`process.rs` 的 `in_job`（`OpenProcess` + 零等待的 `WaitForSingleObject` + `IsProcessInJob`），`ProcessTree::contains` 在 Windows 上从此真的回答，`contains_is_false_until_m4d` 删掉，换成三条用例；`ICMP` 的载体发送失败时不再留着（R12，`PlatformPinger::forget`）。
- **读回的解析**（任务 2）：`crates/rurge-platform/src/route/windows.rs` 的 `routes_in` / `addresses_in` / `mtu_in` 与四条读回命令，夹具手写（注明不是从机器上取的）；`set interface` 的度量拆成单独一行（P9）。
- **`wintun.dll`**（任务 3）：`crates/rurge-platform/src/tun/dll.rs` 的查找（`--wintun-dll` 的相对路径补成绝对路径，否则 `rurge.exe` 旁边），`tun/windows/wintun.rs` 的签名校验（以只许读的共享方式开着那个文件、校验的就是它、签名者恰好是 `WireGuard LLC`、不联网）与加载（`LoadLibraryExW` 只查那个绝对路径，依赖只从 System32 找），每一句错误都不带路径。
- **系统一侧**（任务 4）：`enhanced/windows.rs` 的 `WindowsSystem`（不带 `cfg`）：`netsh -f` 的脚本、每一步之后读回（地址、MTU、路由、DNS）、`ipconfig /flushdns`；`netsh` 的退出码不看、成败只由读回定（P10）；路由那一半经 `run_routes`，IPv6 的两行 WARN 与 M4c 的分工一致；Windows 从不写备份（P12）。
- **设备**（任务 5）：`tun/windows/mod.rs` 与 `tun/windows/wintun.rs` 的设备与十二个放开的函数（其中 `verify_signer` 与 `load` 是任务 3 的）：先探后建、不复用（P8），接口序号每 100 毫秒问一次、至多 5 秒，通道 1,024 个包，环 8 MiB，读线程的循环是 `tun/reader.rs`（不带 `cfg`）；关闭在最后一个 `Arc` 放掉时；`enhanced::system` 把 `wintun_dll` 一路传下去。
- **命令行**（任务 6）：`--wintun-dll` / `RURGE_WINTUN_DLL`（说明 `Windows: where wintun.dll is (default: next to rurge.exe)`），别的平台上是用法错误（`--wintun-dll is only for Windows`，退出 2，在 `Overrides::parse` 与读配置之前）；`--help` 与别的路径选项一样连取值一起显示（R21）。
- **L1 的核对**（任务 7）：`docs/acceptance/phase3-m4-l1-checklist.md`——280 条（本机跑过 276、只在非 Windows 运行 1、L2 3、缺 0），`CLAUDE.md` 与本节引用它，数照它写。
- **文档**（任务 9，这一轮）：兼容性清单 9.1、6.3、10.3、10.4 节与两行新登记；`docs/api/phase3.md` 的「M4d」一节；`docs/acceptance/phase3-manual.md` 的 M4 一节（W19 ～ W41、M17 ～ M30、L18 ～ L32、P29 ～ P31）；`docs/testing.md` 的「阶段 3 / M4d 起」与 `RURGE_ENHANCED_BACKEND` 一段；两份 README；需求文档第 7 节阶段 3 的状态行；`CLAUDE.md` 的「现在在哪里」；设计末尾的「执行期的订正」。

门禁：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过，`cargo test -j 8 --workspace --no-fail-fast` **86 个结果行、4641 通过、0 失败、13 忽略**（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI；工作区的 `gate-task-9-fix-1.log`，`rurge-engine` 的 `udp_tls_family` 以 `STATUS_HEAP_CORRUPTION` 退出、单独重跑 7 条通过，重跑在 `gate-task-9-fix-1-rerun.log`；任务 9 的第一次门禁是 4637，这一轮加了 4 条用例）。

**终审之后的两处代码修正**（2026-10-27，记录第 87、88 行）：一、Windows 的路由集另减 `240.0.0.0/4`（Windows 缺省挡这一段，`netsh` 加不加得进没有人核实过；`RouteInputs` 加 `class_e`，可执行文件给 `!cfg!(windows)`），缺省的 Profile 下 Windows 上 IPv4 是 27 条、Linux 与 macOS 仍是 55 条；二、发送失败的 ICMP 载体退休而不是立刻停掉（`PlatformPinger::forget`，再读一个回显时限、至多 4 个退休着）。两处都先看到红再变绿。

**依赖与特性**：不加新的工作区依赖边，也不加外部包（`Cargo.lock` 的包数与 M4c 做完时相同）；`rurge-platform` 的 `windows-sys` 0.61.2 多开七个特性：`Win32_Networking_WinSock`（任务 1）、`Win32_NetworkManagement_Ndis`、`Win32_Security_Cryptography`、`Win32_Security_Cryptography_Catalog`、`Win32_Security_Cryptography_Sip`、`Win32_Security_WinTrust`、`Win32_System_LibraryLoader`（任务 3）。`tests/unsafe_functions.rs` 的 `LISTED` 现在 23 个函数（新加 `unicast_if`、`in_job` 与 `tun/windows/wintun.rs` 的十二个，其中 `verify_signer` 与 `load` 是任务 3 的）。

没做的，与不要当成已经有的：

- **三个平台的设备与系统改动都没有在真机上跑过**：Windows 的设备与它的十个放开的函数在这台开发机上编译、`clippy` 过，**没有在任何地方运行过**（没有一条缺省的用例开设备，`RURGE_TEST_WINTUN_DLL` 不设，P19，M4d 记录第 70 行）；`verify_signer` 与 `load`（任务 3）在本机的单元用例里跑过、真的 `wintun.dll` 上没有（手工清单 W36）；Linux 与 macOS 专有的代码 CI 还没有见过。
- **`a_real_wintun_dll_passes` 在本机没有真正跑过**（没有 `wintun.dll`）；真的 `wintun.dll` 的签名者是 `WireGuard LLC`、只读共享开着文件时 `LoadLibraryExW` 照样能载入，都要手工清单核（W36）。
- **`the_wintun_dll_option_is_a_usage_error_elsewhere` 在本机只编译、不运行**，第一次运行是 Linux / macOS 的 CI（M4d 记录第 75、76 行）。
- **L2 的用例没有运行过**：`tests/privileged` 写好、在这台机器上只编译到 `#![cfg(target_os = "linux")]` 之后是一个空文件；手工清单的 L32 是在一次性虚拟机上跑它。
- **手工验收一项也没有做**：三份清单（阶段 1、2、3）都是；M4 一节（`docs/acceptance/phase3-manual.md`）是这一轮写的，一项也没有做。
- **阶段 3 不称齐备**：它的需求还没有逐句对过代码。
- **`TunDevice::dropped()` 今天没有读者**：设备写不进的丢包只记在设备自己的计数与那一行 WARN、之后的 DEBUG 里，不进 `GET /v1/features/enhanced_mode` 的 `dropped`（那是 `rurge-tun` 的原因表）；要不要把它发给 API 归项目所有者。
- **启动时的第一次下载在增强模式打开之前**（M4b 延后第 10 行；M4d 记录第 22 行）：`rurge run` 在拿到实例锁之后先做一次崩溃恢复（M4c 的 R10 把它挪到了 `wait_for_the_sources` 里、下载之前），等完 Profile 的远程来源才建引擎、才可能开增强模式，所以“增强模式开着的时候启动一次下载”这条路没有用例走到；那一次下载用 `resource_client()`（不带钩子）、问系统解析库——那时出口表是空的，带了钩子也只会答“不绑”，而崩溃留下的 macOS DNS 已经先恢复了。要改的是次序（先开增强模式还是先等下载），归项目所有者。
- **`filled` 不再带 `#[cfg_attr(not(test), allow(dead_code))]`**（M4d 记录第 79 行）：核过它仍只有用例读（`git grep -n "filled(" crates/rurge/src`），所以换成 `#[cfg(test)]`、说明照实写；它说明里原来那句“wired up by …”已经不在。
- **网卡名固定当作 `rurge`**（R25）：Wintun 把新的网卡叫 `rurge 1`、`rurge 2`…… 时（`rurge` 被一块它改不了名的网卡占着），网络任务的排除名单与出口表认不出它；本轮的延后事项第 10 行（修法与去向）。
- **M4b 那一节里“Windows 的 `ProcessTree::contains` 恒假”与 `docs/api/phase3.md` 里同一件事的两处旧说法以本节为准**：Windows 上它经 `in_job` 回答，`crates/rurge-platform/src/process.rs` 的三条用例与端到端用例 `what_the_program_started_goes_direct_too`（三个平台都编译、在这台开发机上跑过）是证据。

以后的会话必须知道的事：

- **读回定成败**：`netsh -f` 的脚本每一步都当作“可能失败”执行，退出码与输出只进 DEBUG；成没成只看之后的读回（地址、MTU、路由按序号筛出的行、DNS 服务器）。读回只认前缀、序号、地址与数字，不认任何文字，所以系统的语言不影响结论。改 Windows 的步骤时把读回一起改，否则一步默默失败、打开却报成功。
- **句柄存成 `usize`，不写 `unsafe impl`**（P4）：Wintun 的适配器、会话、事件与模块句柄都是整数，只在放开的函数里转回指针；`every_unsafe_exception_is_on_the_list` 不认函数之外的放开。入口点在 `Arc<Wintun>` 里，所以设备自然 `Send` / `Sync`。
- **先探后建，不复用**（P8）：`WintunOpenAdapter("rurge")` 找得到就答“正在被使用”，不去复用——复用的适配器永远不会被移除，它上面留着的路由会让下一次 `add route` 撞上。崩溃之后网卡随进程消失，所以下一次启动不会误报“正在被使用”（读源码与邮件列表得出的推论，待手工清单 W22）。
- **`--wintun-dll`**：只在 Windows 上可用，别的平台上是用法错误（退出 2，在 `Overrides::parse` 之前）；路径在 DEBUG、错误里不带路径（错误会进 API 的 500）；起 `rurge` 二进制的测试助手都 `env_remove("RURGE_WINTUN_DLL")`（M4d 记录第 17 行）。
- **没有一条缺省的用例开设备**（P19）：Windows 的 CI 机器以管理员运行，谁调了 `TunDevice::open`，就真的装了驱动、建了网卡。
- **Windows 的设备在最后一个 `Arc` 放掉时关**（M4d 记录第 29 行）：正常的关闭经 `call`（`spawn_blocking`）交出管理器那一份，`Drop` 的等读线程、`end_session`、`close_adapter` 不在 tokio 的工作线程上；TUN 3 秒内没结束的那一种，最后一份在 TUN 的任务里放掉，那时 `Drop` 会占住一个工作线程。
- **`Drop` 的次序是置停止标志 → 关通道的接收端 → 等读线程 → `end_session` → `close_adapter`**（M4d 记录第 72 行）：读线程在会话结束之前一直用它；先关接收端是为了让卡在满通道上的 `blocking_send` 醒来。一个两枚停止标志的缺陷（`start` 自己又造了一枚，`Drop` 置的那一枚读线程从不看，关闭会挂住）没有用例抓得到，是读代码发现的。
- **发出去的包不超过 `wintun.h` 的 `WINTUN_MAX_IP_PACKET_SIZE`**（`MAX_PACKET`，0xFFFF，M4d 记录第 73 行）：超长的包会让驱动把整个发送环当成坏的，之后每次都是“环满”；`packet_size` 在放开的函数之前就拒它。
- **要 Wintun 0.14 或更新**：0.13 的 `wintun.dll` 没有 `WintunCloseAdapter`（那时是 `WintunFreeAdapter` / `WintunDeleteAdapter`），`load` 因缺入口拒绝它，文字是 `wintun.dll has no WintunCloseAdapter: is it wintun.dll?`。它所在的目录应当只有管理员能写（P7）。
- **`process` 的 Windows 一支用 `in_job`**：`contains` 的 `job` 是 `None` 回 `false`，`Err` 回 `false` 并只写一行 DEBUG；持着 Job Object 的那个进程退出时整棵进程树一起结束。
- **Windows 的路由集不进 `240.0.0.0/4`**（记录第 87 行）：Windows 所有版本缺省挡这一段（草案 draft-schoen-intarea-unicast-240 的 Windows 一段、`mstcpip.h` 的 `NlatInvalid`），`netsh` 加不加得进没有人核实过；`class_e` 为假时 `RouteSet::compute` 在最后把它挖掉（包含项也带不回来），缺省的 Profile 下是 27 条。Linux 与 macOS 把它当普通单播（55 条），一行也不变。改路由集或读回时记着它。
- **ICMP 载体退休，而不是立刻停**（记录第 88 行）：`forget` 把它移出表，接收任务再读一个回显时限（TUN 的 `echo_timeout`）才停，至多 4 个退休着；Linux 的数据报套接字只收得到自己那些请求的回复，所以已经发出的回显要靠它收。

去向写着项目所有者的（后来的会话不要自己定）：设计第 20 节的 Q2（设备、路由、网络环境改用现成的包，或开 `nix` 的 `socket` / `net` 特性）、Q3（发行包随附官方签名的 `wintun.dll`）、Q4（L2 特权用例在 CI 上启用、macOS 与 Windows 的冒烟）、Q5（Windows 防 DNS 泄漏的严格模式，WFP）；**推送与读回 CI**（M4a ～ M4d 的提交都在本地，`origin/main` 仍是 `ac0a98a`；推送之后 M4c 的任务 10 与 M4d 的任务 8 一起做）；**三份手工验收清单**（阶段 1、2、3）一项也没有做；M4a 延后第 9 行（`queue_bytes` 要不要把每个包的 16 字节也算进预算）与第 13 行（`state.json` 的写不 `fsync`，掉电时备份可能没了）；M3a 计划延后第 1 行（`rurge-proto-wireguard` 的 `stack.rs` 里 `TCP_BUFFER` 那个读源码得出、没有观察过的 panic 风险，修法归所有者选）；M4b 延后第 1 行（`the_copy_takes_over_from_a_local_file_once_a_check_has_succeeded` 偶发 `ConnectionRefused`，原因不明，再见到时查）；M4c 延后第 1 行（L2 在 CI 上跑，总设计 Q4）与第 10 行（错误文本里 rurge 自己的网卡名、地址与前缀：要不要照 P7 改成固定的一句）；仓库里 26 份旧文件写着这台开发机带 Windows 账户名的本机路径（`git grep -l` 数得出，新写的内容里没有那个名字与那些路径），改不改归所有者；`TunDevice::dropped()` 要不要发给 API（本轮延后第 5 行）；启动时第一次下载与增强模式打开的次序（M4b 延后第 10 行、本轮延后第 6 行）。

协调者在 M4d 代项目所有者作的裁定（所有者可以改，记在 M4d 记录的行里）：R16（`Wintun::module` 的生产读者是 `loaded` 的那一行 DEBUG，记录第 69 行）、R17（任务 4 漏的那条用例只断言 WARN 行，记录第 66 行）、R18（崩溃之后网卡随进程消失、探名字不会误报，待手工清单 W22，记录第 68 行）、R19（任务 5 修正第 1 轮的两条顾虑接受，记录第 73 行）、R20（第 75 行的理由由第 76 行订正）、R21（`--help` 显示 `RURGE_WINTUN_DLL` 的值，记录第 74 行）、R22（`holds_a_backup`——代码里是 `left_to_undo` / `Left::any`——不补单元用例，行为有端到端用例覆盖）、R23（Windows 的路由集不进 `240.0.0.0/4`，记录第 87 行、设计第 22 节第 32 条）、R24（发送失败的 ICMP 载体退休，记录第 88 行）、R25（网卡名可能是 `rurge 1`，这一轮不修，延后第 10 行）。

### M4 推送之后的 CI（2026-10-28）

项目所有者 2026-10-28 推送了 `3a76b80`（M4a ～ M4d 的 116 个提交）。运行 40611702392（第 13 次运行）的四个任务——三个平台的 `check` 与 `bench`——都成功，是 CI 第二次全绿（第一次见上面「CI 在 `ac0a98a` 上第一次全绿」一节）。M4c 的任务 10 与 M4d 的任务 8 一起做完，没有要修的（M4c 记录第 51 行、M4d 记录第 91 行）。**本节订正 M4a ～ M4d 各节里“CI 还没有见过”“推送与读回 CI”一类的说法**（规矩是历史的段落不逐处改、由后写的段落订正）：

- **第一次被编译、运行的**：Linux 与 macOS 专有的代码——`tun/linux.rs`（`set_iff`）、`tun/macos.rs`（`utun_connect`）、挑平台的 `cfg`、`icmp::open` 的 Linux / Android 与 macOS 两份、`ProcessTree::contains` 的 Unix 一段——在 CI 的 Linux 与 macOS 上过了 `cargo clippy --all-targets -- -D warnings`（工具链 1.89.0），缺省用例通过；M4d 不带 `cfg` 的改动（P18）第一次在这两个平台上编译、运行；只在非 Windows 运行的 `the_wintun_dll_option_is_a_usage_error_elsewhere` 与 `#[cfg(unix)]` 的 `0600` 用例在两个平台上通过。这些由测试一步的成功推出：任务日志要登录才能读，各平台跑了多少条用例、哪些打印了跳过，没有读到。
- **没有变的**：缺省的 `cargo test` 不建网卡、不改系统设置，三个平台的设备与系统改动都没有真的运行过，**没有在真机上验证**；CI 没设 `RURGE_TEST_PRIVILEGED`，L2 的三条在 Linux 上编译了、打印原因就返回（M4c 延后第 1 行，总设计 Q4）；没设 `RURGE_TEST_WINTUN_DLL`，Wintun 设备的十个放开的函数没有在任何地方运行过；`icmp::open` 的“其余平台恒失败”一份 CI 的三个平台也编不到（M4b 延后第 17 行）；三份手工验收清单一项也没有做；阶段 3 的需求还没有逐句对过代码，**阶段 3 不称齐备**。
- **注解**只有平台的：四个任务各一条 Node.js 20 弃用的警告（`actions/checkout@v4` 被强制跑在 Node.js 24 上），跑在 ubuntu-latest 上的两个任务各一条“`ubuntu-latest` 将从 2026-10-19 起迁到 Ubuntu 26”的通知，`check (macos-latest)` 一条 arm64 排队的通知。协调者代项目所有者作的裁定（所有者可以改）：R26，这一轮不换 `actions/checkout`（M4d 延后第 11 行）；R27，不钉 `ubuntu-24.04`，下一次 Linux 任务失败时先查当时的镜像（M4d 延后第 12 行）。
- 测试一步的时长：Linux 8 分 15 秒、macOS 10 分 36 秒、Windows 14 分 8 秒（工作流给这一步 60 分钟）。M4d 一节「去向写着项目所有者的」里的“推送与读回 CI”做完了，其余各项照旧。

### M5（识别、阻断与 `[IP Rewrite]`）

M5 做完了（2026-10-28；计划 `docs/superpowers/plans/2026-10-28-phase3-m5-recognition-blocking-ip-rewrite-plan.md`，下面写“M5 记录第 N 行”与“M5 延后第 N 行”指它末尾的两张表，写“P N”指它的「计划期决定」）。M5 由协调的会话逐个任务调 Pi 做、逐个任务评审（做法同 M4d）。**M5 的提交（`e6666c1` 之后）都在本地，CI 没有见过**：`origin/main` 仍是 `3a76b80`，CI 最近一次运行是 40611702392。

做了什么：一句话说，阶段 3 剩下的识别与阻断做完了——UDP 流在所有端口上认 QUIC 与 STUN，`block-quic` 有了 6.11 的 `adaptive` 与 `auto`，`[IP Rewrite]` 第一次生效，写回虚拟网卡的 UDP 不排在 TCP 后面，内部 DNS 会话带上主机名，手册 6.11 的 WireGuard 服务端写法能加载；三个基准重跑、文档收尾。逐任务（提交是短哈希）：

- **任务 0**（`eb66d73`）：开工前核对；设计第 20 节的五处订正与 P1 ～ P20 对着 `9ee270f` 的代码核过。
- **任务 1**（`b50f277`）：`crates/rurge-net/src/quic/initial.rs` 的 `is_client_initial`（包头筛不分配、不派生密钥，过了筛用 Initial 密钥解密确认），`quic/testing.rs` 的客户端 Initial 构造器（`testing` 特性）。RFC 9001 / 9369 附录 A.2 的两个 Initial、0 / 1 / 21 字节的 DCID、前缀、位翻转与一条 quinn 的真客户端 Initial 都有用例。
- **任务 2**（`7350ed2`）：`crates/rurge-engine/src/udp.rs` 在**所有端口**上打 QUIC 与 STUN 标签（`sniff.rs` 的 `is_stun`），端口 443 的旧判断与 `sniff.rs` 的 `is_quic_initial` 删掉；引擎用例 `quic_on_any_port_is_quic`、`block_quic_blocks_quic_on_any_port`、`a_lookalike_of_a_quic_initial_is_plain_udp`、`stun_flows_are_labelled_stun`。
- **任务 3**（`ed1e993`）：`BlockQuic` 的第四个取值 `adaptive`（`spec/common.rs`）与 `udp::quic_verdict` 的判定表（4 全局 × 4 参数 × 直连与否 × 载不载 UDP，`every_cell_of_the_quic_verdict` 逐格比对）；`quic_adaptive.rs` 的表（每引擎一张、200 个名字、递增的序号淘汰、1 秒的窗口、两行 DEBUG），`Dialer::dial` 里通知表；`rurge-policy/src/registry.rs` 的 `needs_an_outbound` 让写了 `block-quic` 的 `direct` 别名建自己的出站（第 19 行）。
- **任务 4**（`5280e58`）：`crates/rurge-config/src/ip_rewrite.rs` 的 `parse` / `IpRewrite` / `action`（按族分存、线性查、主机位照 `IP-CIDR` 的读法、`W0012` 跳过写错的行），`Config.ip_rewrite`；`[IP Rewrite]` 从 `deferred.rs` 拿掉。
- **任务 5**（`953a566`）：`rurge-tun` 的 `packet.rs`（`Classified::Rewrite`、`Hit`、`Refusal`、`refusal`、`rst_for`、`Reason::IpRewrite` 与 `is_wanted`）、`rewrite.rs`（`reflect` / `reject` / `drop`）、`dispatch.rs` 里每批读一次设置、`stats.rs` 的 `reflected`；引擎的 `TunSettings.ip_rewrite`（`2011f13`）与 API 的两个数（`6ef0334`）。
- **任务 6**（`fe7f414`）：`writer.rs` 的两条有界队列与 `Class` / `Queue` / `Outlet`，`Context::send` / `try_send` 每次发送读一次 `udp_priority`；评审第 1 轮的四处（第 41 ～ 44 行）在 `349c6c5`、`9c9763d`。
- **任务 7**（`7c026e1`、`d83b95f`、`2569ba2`）：`Connector` 的两个 `*_resolved` 与 `BootstrapConnector` 的转交（第 46 行）、`quic::Path::open_resolved`；引擎的 `choose_policy_with` / `FixedResolve` 让内部 DNS 会话按服务器的名字匹配规则、拨号一律拨地址；引擎未接上时回落只收到地址（第 50 行）。
- **任务 8**（`36bf294`）：`crates/rurge-config/src/wireguard.rs` 认得 `listen-port`、`accept-incoming`（`W0029`）与有 `listen-port` 时不带 `endpoint` 的 peer（`W0029`，留在节外）；`peers[0]` 的两处用法只靠“没有 peer 的节没有 spec”挡住（P17；`spec/mod.rs` 的 `NotImplemented::WireGuardNoPeers`）。评审第 1 轮的排版（第 59 行）在 `fb15c79`。
- **任务 9**（`76ff44c`）：三个基准重跑，读数在 M5 记录第 61 ～ 63 行（下一段）。
- **任务 10**（`6729fd8`）：文档收尾——兼容性清单、`docs/api/phase3.md` 的「M5」、`docs/testing.md`、手工清单的 M5 一节、两份 README、需求文档第 7 节的状态、`CLAUDE.md`、本节与计划的两张表。
- **任务 10 的修正第 1 轮**（`698c958`、`335e692`、`337570a`、`e920e1d`）：终审的四条（扩展首部与 ICMPv6 重定向、IPv6 的引用、`accept-incoming` 的 `W0029`、QUIC 的 QUIC 位）与任务 10 文档的十四条；四次变异核对与二十次循环在 M5 记录第 81、82 行。
- **任务 10 的修正第 2 轮**（`ca46f58`）：评审第 2 轮的文档与注释订正（C1 ～ C11），M5 记录第 83 行起；只改文档与注释，没有重跑全量。
- **任务 10 的修正第 3 轮**（这一个提交）：两段文档注释重排到 80 列，字不改（M5 记录第 90 行）。

门禁（任务 8，一台 Windows 11 x86-64 开发机上的一次运行，**不是 CI**）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过（0 个警告）；`cargo test -j 8 --workspace --no-fail-fast` **86 个结果行、4717 通过、0 失败、13 忽略**。任务 8 修正第 1 轮之后的第一次全量是 4716 通过、1 失败——`rurge-proto-quic` 的 `trust_tunnel::tests::the_last_tunnel_goes_on_beside_all_the_others_unread` 在它自己的时限上以 “the windows never filled” 失败（与本轮改动无关的计时用例），单独重跑通过，这个 4717 是第二次全量的数（M5 记录第 60 行）；任务 9 与任务 10 的第一部分不改代码，任务 10 的修正第 1 轮改了代码、全量重跑：**86 个结果行、4720 通过、0 失败、13 忽略**（4717 加本轮的 3 条：`refusal_reads_past_no_ipv6_extension_header`、`reject_quotes_an_ipv6_packet_up_to_the_minimum_mtu`、`a_cleared_quic_bit_is_still_one`），仍是同一台机器上的一次运行，不是 CI（M5 记录第 82 行）。

基准（任务 9，同一台机器、release、`TUN_BENCH_FAR=memory`，**不是 CI**）：B1 八个量出来的行都过 0.7 Gbit/s 的线，四个计数都是 0，中位数上行 8.03、下行 4.84 Gbit/s（两个 MTU 混在一起，只作量级）；同设置（MTU 1500、`tx_max` 256 KiB）的 5.717 / 5.172 是上一次的 79.7% 与 94.7%。B2 门槛读数 403,671,140 字节（403.7 MB），在线 429,000,000 字节以内、余量 25.3 MB；保守读数 445,333,604 字节（445.3 MB）超线 16.3 MB，照旧归项目所有者（M5 延后第 4 行）。b3 中位数 0.9 µs、p99 2.9 µs，最大 4542.2 µs（4.5 ms，落在第 28,673 个查询上，是线的 22.7 倍）。细节与上一次的对比在 M5 记录第 61 ～ 63 行。

**终审**：一位只读的评审者静态读 `e6666c1..76ff44c` 的全部代码，没有跑 cargo：严重 0、重要 0、次要 3、存疑 1。四条都在任务 10 的修正第 1 轮改了（M5 记录第 70 ～ 73 行）：`refusal_v6` 不读扩展首部之后的差错、也不回 ICMPv6 重定向 137；IPv6 的 `reject` 至多引用 1232 字节；`accept-incoming` 的 `W0029` 说对了理由；QUIC 首个 Initial 的 QUIC 位可以是 0（RFC 9287）。

**依赖与特性**：**不加新的工作区依赖边，也不加外部包**（`e6666c1..HEAD` 里没有 `Cargo.toml` 与 `Cargo.lock` 的改动；按 `[[package]]` 数，`Cargo.lock` 仍是 **519** 个包，与 M5 记录第 10 行相同）；`crates/rurge-platform/tests/unsafe_functions.rs` 的 `LISTED` 仍是 **23** 项（M5 没有加放开的函数）；`crates/rurge/tests/sockets.rs` 的 `LISTED` 仍是 25 行（M5 的生产代码不新建套接字，QUIC 的识别只读数据报）。

没做的，与不要当成已经有的：

- **M5 的提交都在本地**：`phase3-m5` 上的提交（`e6666c1` 之后）都没有推送，`origin/main` 仍是 `3a76b80`；CI 一个也没有见过（最近一次被 CI 见过的运行是 40611702392）。
- **手工验收一项也没有做**：M5 一节（`docs/acceptance/phase3-manual.md` 的 W42 ～ W47、M31 ～ M36、L33 ～ L38、P32）是这一轮写的，连同阶段 1、2、3 的其余各节；`[IP Rewrite]` 的 `reflect` 只在内存设备上证明过，真机上送回的包怎样被操作系统回应（应答又被送回、靠 TTL 与“不对 RST 回 RST、不对差错回差错”终止）要在 P32 看。
- **`udp-priority` 的效果没有观察过**：两条队列的逻辑有单元用例与二十次循环，真实应用上看不出延迟差别，不设手工项（兼容性清单 2.1 节把差异写成“尽力而为”，本节开头也说了）。
- **阶段 3 不称齐备**：它的需求还没有逐句对过代码；M5 做完了不等于阶段 3 的验收标准验过了（真实设备、真实网络、手工验收都还没有）。
- **没有用例覆盖的**：TLS 之后的 QUIC SNI、跨几个包的 ClientHello、`auto-quic-block` 全都留给阶段 4（M5 延后第 5、6 行）；gQUIC 与草案 28 以前不认、Surge 那边是否同样不认没有核对过（M5 延后第 16 行）。
- **release 测试构建的一条 `unused_imports` 警告**：`crates/rurge-tun/src/tests/filter.rs` 第 8 行导入的 `ingress` 只在一条 `#[cfg(debug_assertions)]` 的用例里用，任务 9 的 release 构建因此有一条警告；M5 之前（`e6666c1`）就如此，CI 不编这个构建（M5 延后第 14 行）。
- **`trust_tunnel` 的计时用例在负载下失败过一次**：`the_last_tunnel_goes_on_beside_all_the_others_unread` 在 `-j 8` 的全量里以 “the windows never filled” 失败过（单独重跑通过，M5 记录第 60 行），它的时限够不够是以后的会话的事（M5 延后第 13 行）。

以后的会话必须知道的事（改那一块代码之前读对应的一行与 `docs/api/phase3.md` 的「M5」）：

- **旧名字**（M5 记录第 74 行）：任务 7 把 `connect_as` / `connect_udp_as` 并成 `connect_with` / `connect_udp_with`；任务 3 用 `quic_verdict` 取代 `quic_blocked`、把 `which_quic_flows_are_blocked` 改写成 `every_cell_of_the_quic_verdict`、把 `has_socket_opts` 改名 `needs_an_outbound`。前面各节（M3b、M7c 等）里的旧名字以此为准，历史段落不逐处改。
- **包的去向的次序**（设计 8.3、P10）：首部 → 怪地址 → `[IP Rewrite]` → 分片 → 传输层（`packet::classify`）。命中的包只读到 IP 首部，`Classified::Rewrite(Hit)` 不填 `Parsed`；`#[cfg(test)]` 的 `checks` 给空的表，`tests/filter.rs` 的几十处调用走它。把 `[IP Rewrite]` 挪到分片之后或怪地址之前会同时改计数（用例 `martians_come_before_ip_rewrite`、`fragments_come_after_ip_rewrite`）。
- **`Reason::IpRewrite` 的 `is_wanted`**（P12、M5 记录第 26、27 行）：它是 profile 要的，`Stats::drop` 从不为它写 WARN、每次 DEBUG（`ip_rewrite_drops_are_said_at_debug_never_warned`）；`reflected` 不写日志；`queue_full` 为 `reflect` 的包记的是**要发出去**的包的两个地址（第 27 行）。新增会自动发生的丢弃原因不要往 `is_wanted` 里塞。
- **两个限频是两份**（第 28 行；评审第 1 轮的订正见第 76 行）：RST 与 ICMP 各是每 3 秒 100 个。`reject` 的 RST 与虚拟网卡自己回的 RST（停止中、忙、发往 rurge 自己的地址、不认识的 fake IP、预匹配拒绝，都经 `tcp/shard.rs` 的 `turn_away`）共用一份，`reject` 的 ICMP 与被拒 UDP 流的 ICMP 共用另一份——同一类共用一份，RST 与 ICMP 之间不共用。
- **`[IP Rewrite]` 每批读一次设置**（设计 8.2、8.8）：`dispatch.rs` 的 `run` 从 `hooks.settings()` 取一次 `Arc<IpRewrite>`，这一批的每个包都用它经 `route` / `packet::classify` 判（没有“每条流”）；重载之后下一个包就用新表，已经建立的连接也受它管。表按代，每代 `TunSettings.ip_rewrite` 是 `Arc`。
- **写任务的两条队列**（设计 7.2、P13）：`Class::Udp` 进优先队列（`udp-priority` 开着时），其余一律普通；一条队列关了之后不再等它，两条都关了才结束；`TunOptions.writer_queue` 是**各自**的容量（第 43 行），最坏能攒下两倍的批（设计 7.3，任务 6 的执行者提过）；设备正在写的那一次仍要等完，所以是“尽力而为”。
- **内部 DNS 会话拨地址而规则与组看名字**（M5-D15、设计 9.3 ～ 9.5）：流水线拿到的 `dst` 是名字与端口，`IP-CIDR` 用跟着来的地址（`FixedResolve`），**绝不交给引擎的解析器**（自己解析自己）；终端出站（以及经代理时的代理）收到的是引导解析出的地址（M5-D16）；`BootstrapConnector` 实现 `connect_resolved` / `connect_udp_resolved` 并把名字原样交给内层（第 46 行），DoQ / DoH3 经 `Path::open_resolved`；引擎还没接上时**拨地址**、名字从不当拨号目标（第 50 行，用例 `without_an_engine_a_named_server_is_dialled_at_its_address`）；请求记录 `dst` 是名字与端口，`remoteAddr` 照旧，`dnsMs` 是 `null`。
- **`needs_an_outbound` 让写了 `block-quic` 的 `direct` 别名建自己的出站**（第 19 行，超出了任务 3 列出的 crate）：只写 `block-quic=adaptive` 的别名在注册表里不再是 `Entry::Alias`，否则它的 spec 被丢掉、引擎读不到这个参数（用例 `adaptive_on_a_direct_alias_blocks_the_first_quic_flow`）。
- **WireGuard 的 `peers[0]` 只靠“没有 peer 的节没有 spec”挡住**（P17）：`crates/rurge-policy/src/registry.rs` 与 `crates/rurge-policy/src/testing.rs` 的 `peers[0]` 在 `WireGuardNoPeers` 的节上走不到；去掉 `to_spec` 里那一处设置，任务 8 的引擎用例正是在那里越界。`listen-port` / `accept-incoming` 的两句 `W0029`（前者的理由是这一版不接受连入，后者是这一版不接对端经隧道发起的连接）与“不带 `endpoint` 的 peer 留在节外”的第三句见 `wireguard.rs`；没有 `listen-port` 而缺 `endpoint` 照旧是 `E0023`。
- **自适应的表**（设计 6.3、P8）：每引擎一张，键是去掉末尾点、小写的主机名；跨重载保留、换网络不清；1 秒的窗口懒判定，`Internal` 的会话不通知；**阻断之前已经在的 TCP 不算回落**（M5-D6，Surge 那边是照手册的字面读、没有观察过）；单元用例用显式的时刻，不靠墙钟（P8）。
- **QUIC 的识别**（设计 4.2、M5-D3；终审的订正见第 73 行）：只看流的第一个数据报的第一个包；不读帧、不取 SNI；包头只要求长包头位（QUIC 位可以是 0，RFC 9287 §3.1），认 v1 / v2 / 草案 29 ～ 32，gQUIC 与草案 28 以前不认（草案 28 以前的 salt 不同，gQUIC 根本不是 IETF 的包保护）；包头筛不过的数据报不分配、不派生密钥。STUN 只看第一个数据报，判据是 RFC 8489 的魔数、首两位与长度（M5-D21）。两者都不写日志。
- **`reject` 的 IPv6 引用与扩展首部**（第 70、71 行）：IPv4 引用首部 + 8 字节；IPv6 引用整个包、至多 1232 字节（1280 的最小 MTU 减 IPv6 与 ICMPv6 首部，RFC 4443 2.4(c)），因为分片首部让传输层首部不在第 48 字节之前；带 IPv6 扩展首部的包（一个分片首部之外）与 ICMPv6 重定向 137 都不回。
- **日志与凭据**：`[IP Rewrite]` 的地址只在 DEBUG；自适应的两行 DEBUG 带名字；WireGuard 的新诊断只引可以引的键名，不引取值；API 的键 `dropped.ipRewrite` 与顶层 `reflected`（`crates/rurge-api/src/routes/features.rs` 的 `enhanced_json`）。

去向写着项目所有者的（后来的会话不要自己定）：M5 延后事项里去向是项目所有者的各行——**手工验收**（第 1 行）、**推送与读回 CI**（第 2 行）、总设计 Q8（需求文档要不要为 `[IP Rewrite]` 另立一行、FR-OUT-11 的“UDP 443”改不改字面，第 3 行）、B2 的保守读数超线（第 4 行）、gQUIC 与草案 28 以前要不要在 Surge 上核对（第 16 行）；其余延后项的去向是阶段 4 / 5 / 6 / 7 / 远期，照那张表（`[MITM]` 的 `auto-quic-block` 与 QUIC SNI 到阶段 4，`[IP Rewrite]` 的模块叠加到阶段 5，`include_external` 到阶段 6，WireGuard / Tailscale 接受连入到阶段 7 / 远期，`.local` 的作用域地址到远期）；另有 `trust_tunnel` 计时用例（M5 记录第 60 行，去向是以后的会话，M5 延后第 13 行），不在这一段里。

协调者在 M5 代项目所有者作的裁定（所有者可以改，记在 M5 记录的行里）：第 8 行（`rurge-tun` 的用例从文字造 `[IP Rewrite]` 的表，不为用例加公开的构造函数）；第 9 行（`QuicAdaptive` 用递增的序号记“最近用到”，不比较时刻，淘汰序号最小的）；第 18、19 行（执行者的裁定，协调者评审时认可：`udp.rs` 的 `open()` 取链上最后一个有 spec 的名字的 `block-quic`；`has_socket_opts` 改名 `needs_an_outbound`，写了 `block-quic` 的 `direct` 别名建自己的出站）；第 20 行（执行者记下、协调者认可：`E0018` 的消息随取值表多出 `adaptive`，`config.rs` 里那条既有用例的期望随之改）；第 27 行（`reflect` 的包遇到写任务队列满时，`queue_full` 记要写出去的包的两个地址，不计 `reflected`）；第 28 行（两条“共用限频”的用例先用掉虚拟网卡自己的配额、再发往 `reject` 的地址，真的证明是共用的）；第 36 行（`a_full_writer_queue_drops_the_icmp_and_counts_it` 改用关着 `udp-priority` 的 `gated_without_priority()`）；第 37 行（`reflect_sends_a_syn_back_with_the_addresses_swapped` 查“什么也没拨”改看 `dialed()`）；第 41 ～ 44 行（写任务的用例在起写任务之前放好几批；`reply_to_the_app` 自带五秒的时限；`TunOptions.writer_queue` 写成两条队列各自的容量；`writer::run` 用私有的 `Queue` 标记一批取自哪条队列）；第 46 行（补 P15 的缺口：DoQ / DoH3 也经 `BootstrapConnector`，所以它另实现 `connect_resolved` / `connect_udp_resolved`）；第 50 ～ 52 行（引擎还没接上时也拨引导给的地址；`boot` 被问的次数改成“被问过”；四处文档注释照实改）；第 55 行（`unknown_keys_and_fields_are_warnings` 里“不认识的键”的例子从 `listen-port` 换成 `fwmark`）；第 59 行（`NotImplemented::diagnostic()` 的一支手工排版）；第 70 ～ 73 行（终审的四条）；第 74 ～ 80 行（评审第 1 轮的 B1 ～ B14，含 B2 对第 66 行那一组数的订正）；第 83 ～ 88 行（评审第 2 轮的 C1 ～ C11）。

### M6（收尾与验收）

M6 做完了（2026-10-29；计划 `docs/superpowers/plans/2026-10-28-phase3-m6-acceptance-plan.md`，下面写“M6 记录第 N 行”与“M6 延后第 N 行”指它末尾的两张表，写“P N”指它的「计划期决定」）。M6 由协调的会话逐个任务调 Pi 做、逐个任务评审（做法同 M4d、M5）。**`3a76b80` 之后的提交都在本地，CI 没有见过**（读 M4 的 CI 的两个文档提交、M5 与 M6 的全部）：`origin/main` 仍是 `3a76b80`，最近一次被 CI 见过的运行是 40611702392。

做了什么：阶段 3 的需求逐句对照过代码（新文件 `docs/acceptance/phase3-acceptance.md`：141 句、十张表、四张登记表、第 14 节八条的证据、靠手工验收的、靠 CI 的、留给项目所有者的）；两份清单对着现在的代码核了一遍（手工清单 172 项、兼容性清单阶段栏含 3 的 81 行）；对外的文档对齐（两份 README、需求文档第 7 节的状态、`CLAUDE.md`「现在在哪里」、本节、`docs/README.md`、阶段 1 与阶段 2 的验收文档各一句）。逐任务（提交是短哈希）：

- **任务 0**（`e17e45b`、`269c738`、`c72b00f`）：验收文档的骨架（141 句切成句子）。
- **任务 1**（`5e00564`、`ead834e`、`c91e5cd`、`54a47ee`、`9f07c4d`、`433cd2b`）：M1 那一片（网络环境与子网、`[SSID Setting]`、NFR-05）；补了一条用例（`5e00564`：`rurge-api` 的 `subnet` 组在 `/v1/policy_groups` 里的成员表）。
- **任务 2**（`257945a`、`a2b91fd`、`e21d0af`、`a778b57`）：M2 那一片（进程识别与 `PROCESS-NAME`）。
- **任务 3**（`6ace5e1`、`31326e1`、`50683bd`、`39f7a41`、`a38ce30`、`99640f2`）：M3 那一片（虚拟网卡的内核、DNS 应答器、预匹配）。
- **任务 4**（`35d4551`、`d4fec84`、`8582636`）：M4 那一片（平台接入、NFR-07、NFR-11）。
- **任务 5**（`04ccf2d`、`0ca460d`、`35a8883`、`fcf9464`、`37616a0`、`f97f420`、`3306945`）：M5 那一片（识别、阻断、`[IP Rewrite]`、基准）；补了两条用例（`fcf9464` 的 `ip_rewrite_leaves_requests_to_the_proxy_ports_alone`、`37616a0` 的 `quic_and_stun_flows_are_named_quic_and_stun`）。
- **任务 6a**（`2b0b76d`、`25277c3`、`bf108ff`）：兼容性清单阶段栏含 3 的行逐行核（核对时把 2.1 节 `block-quic` 的阶段栏改成 `2 / 3`，所以从开工时的 80 行变成 81 行），附录的统计重数（577 行，✅ 395、🟡 131、🔁 32、⛔ 14、❓ 5）。
- **任务 6b**（`47eed2c`、`a1858aa`、`fcca7b6`、`befce97`、`b3ac83e`、`39d5ab4`、`f050653`）：手工清单核一遍、就地订正、M6 一节补齐（新加 19 项，从 153 项到 172 项：M1 37、M2 23、M3 19、M4 55、M5 19、M6 19），文件开头加「怎么用」与第 14 节八条的索引表。
- **任务 7**（`ea92912`、`6bc9cc1`、`e61a6c9`、`4afab7e`、`6c4aa57`、`ed37552`，与修正第 3 轮）：验收文档的结论、数字、四张登记表的编号与排序、第 14 节八条的证据、靠手工验收的、靠 CI 的、留给项目所有者的；对外的文档收尾。

门禁（任务 7，一台 Windows 11 x86-64 开发机上的一次运行，**不是 CI**）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过（0 个警告）；`cargo test -j 8 --workspace --no-fail-fast` **86 个结果行、4723 通过、0 失败、13 忽略**。第一次跑时 `rurge-engine` 的 `outbounds_tuic` 二进制以 `STATUS_HEAP_CORRUPTION` 退出（`CLAUDE.md`「这台开发机」里那种，与本轮改动无关），单独重跑 10 条全过；4723 与 `f97f420` 上的基线相同，任务 1a 与任务 5 补的三条在那条基线里已经算进去了（M6 记录第 92 行）。

**结果（三个数与第四个数，要一起说）**：没有做完（已登记）5 句（P0 没有，P1 一句——#37 的 `tfo-behaviour`；非功能需求 4 句：#132、#133 的 NFR-01、#139 的 NFR-07、#141 的 NFR-11）；需求文字有问题 4 句（P0：#20、#32、#99、#104）；有意不同（已登记）3 句（#35、#101、#120）；本机没有能红的用例 3 句（#1、#2、#3，三个平台的 TUN 设备那一层）。阶段 3 的功能需求有一句没有做完（P1 的 #37）；非功能需求另有 4 句没有做完（#132、#133、#139、#141）。需求逐句对过代码不等于验收通过。

没做的，与不要当成已经有的：

- **手工验收一项也没有做**：阶段 1、2、3 三份清单；阶段 3 那份 172 项（W 50、M 39、L 42、P 41），验收文档「真机上」一列点到 133 项，另 39 项没有任何一行点到（其中 8 项由清单开头的索引表点到）。L2 的四条特权用例（`tests/privileged/tests/linux_netns.rs` 的三条与 `a_real_wintun_dll_passes`）写好没有运行过。
- **三个平台的设备与系统改动没有在真机上验证**：照代码会建网卡、配地址与路由、接管系统 DNS、还回去，但没有一次真的在 Windows / Linux / macOS 上跑过。
- **第 14 节八条没有一条能写成「通过」**：L1 全绿不等于验收通过；第 3、4、6 条与真机有关的部分只有内存设备上的证据。
- **M5 与 M6 的提交 CI 没有见过**：连 `3a76b80` 之后加的用例（`5e00564`、`fcf9464`、`37616a0` 三条与 M5 自己的用例）都没跑过；任务 8 在推送之后读回 CI，改掉写着“CI 没有见过”的每一处。
- **`[IP Rewrite]` 只在内存设备上证明过**、`udp-priority` 的效果没有观察过（M5 一节就写着）。
- **`tfo-behaviour` 没有实现**（#37）：`socket2` 0.6 没有安全封装，登记在兼容性清单 9.4 节。
- **兼容性清单 5.2 节的 `MCCMNC:` 与旧键 `cellular` 两行不记日志**：阶段栏含 3、标 🔁，与 `CLAUDE.md` 兼容性原则“解析并忽略 + 记录日志”不一致；要不要加日志归项目所有者（M6 记录第 54、55 行）。

以后的会话必须知道的事（改那一块之前读对应的一条、`docs/acceptance/phase3-acceptance.md` 与 M6 计划的两张表）：

- **`rurge dns lookup` 是另一个进程，永远不在增强模式里**：`PlatformSystemDns` 的 `enhanced` 标志只有 `rurge run` 的增强模式管理器设。要证明增强模式下 rurge 的 DNS 行为，看运行中的 `rurge run`（M6 记录第 81 行）。
- **增强模式下向应答器问 A / AAAA 拿到 fake 地址、不问上游**；要让 rurge 向上游问一次，用 TXT，应答器把它转发出去（`crates/rurge-tun/src/dns/mod.rs` 的 `forward`）。
- **手工验收时局域网里的 `<服务器>` 在直连网段里**，要写进 `tun-included-routes` 才进虚拟网卡（M4 设计「执行期的订正」第 31 条；M6 记录第 83 行）。
- **验收文档的数**：141 行、十张表；数的是行，指向别的行的行也算（M6-D18）。四张登记表的 `#` 是这一句在第一部分的序号，四张表都按它升序排；「有意不同的」与「本机没有能红的用例的」两张表的 `#` 一栏是 M6 才补上的（M6 记录第 88 行）。
- **兼容性清单阶段栏含 3 的行是 81 行**（开工时 80，第 64 行把 2.1 节 `block-quic` 改成 `2 / 3`）；附录 577 行，✅ 395、🟡 131、🔁 32、⛔ 14、❓ 5（第 65 行）。
- **手工清单 172 项**（W 50、M 39、L 42、P 41；按节 M1 37、M2 23、M3 19、M4 55、M5 19、M6 19），编号整份不重复，文件开头有「怎么用」（建议次序、前置条件、恢复命令、第 14 节八条的索引表）。
- **`docs/api/phase3.md` 的 `Takeover::follow` 订正**（2026-10-29 那一段）：进入暂停时只在增强模式开着时把“想要”记成开着，关着时不改；每一次得到成功应答的 `POST` 都记下它要的（暂停里外都一样，关闭回 500 也算成关）；暂停结束时那一次打开失败之后与设备坏了之后都是“模式关着而想要仍是开着”（使用者的 `POST` 打开失败不改“想要”），下一次暂停结束时再试（M6 记录第 94 行、第 104 行与第 108 行）。
- **任务 8 在推送之后怎么做**（M6-D22）：先 `git rev-parse origin/main`；读那次运行的注解与任务摘要（办法见 `CLAUDE.md`「现在在哪里」）；用 `git grep -n -E "CI 没有见过|CI 还没有见过|CI 一个也没有见过|CI 都没有见过|CI has not seen"` 列出要订正的地方；说 M5、M6 的提交现在没有被见过的那些（现在时）照读到的结果订正，跑在 `3a76b80` 上那次运行之后的提交与用例也一样；历史的段落由后写的段落订正，不逐处改——「M4 推送之后的 CI」一节是先例；M4 以前写的那些已经由那一节订正过，不再动。查出的失败在 M6 里插任务修。
- **`docs/acceptance/phase3-m4-l1-checklist.md` 的两处过时**（同一行）：路由集那一组是 14 条用例（`8538125` 加的 `an_included_class_e_route_stays_out_where_the_platform_blocks_it` 与 `windows_leaves_class_e_out`）；`forget_drops_only_the_carrier_that_failed` 从 `f49ee24` 起叫 `forget_retires_only_the_carrier_that_failed`。
- **M6 改的代码只有三条用例**（`5e00564`、`fcf9464`、`37616a0` 里的那三条）：没有改生产代码、没有加依赖、没有加放开的函数（`LISTED` 仍是 23 项）。

去向写着项目所有者的（后来的会话不要自己定；验收文档的「留给项目所有者的」一节是汇总）：

- **手工验收**（三份清单）、**推送与读回 CI**（M5 与 M6 的提交）。
- 总设计 Q3（Wintun 再分发的书面确认，#141）、Q4（L2 在 CI 上启用）、Q7（NFR-01 在真实设备上怎样量、由谁量，#132、#133）、Q8（`[IP Rewrite]` 要不要另立需求行；`suspend` 与“UDP 443”的字面，#99、#104 与「有意不同的」里 `suspend` 那一行）。
- 「需求文字的问题」4 句怎么改；「没有做完的」5 句放进哪个阶段；阶段 1 推过来、阶段栏仍写 1 的六句（#120、#121、#123、#124、#125、#126）；`MCCMNC:` 与 `cellular` 两行要不要加日志；INFO 里的规则文字带着 `PROCESS-NAME` 的值（M2 延后第 20 行）；M3b 计划「延后事项」第 29 行的引擎一侧（UDP 的字节上限，原来写“阶段 3 / M6”）；B2 的保守读数超线（M5 延后第 4 行）与基准最大值算不算没达到（M3c 延后第 12 行）；「本机没有能红的用例的」3 句要补的用例与手工项；M1 ～ M5 各节「去向写着项目所有者的」那些。
- 与计划对不上的地方照 M6 计划的「执行期修正记录」；任务 7 自己定的事在第 88 ～ 99 行与修正加的第 100 ～ 110 行（四张表的编号与排序、手工 39 项的数法、一句总的 CI 的话、门禁的重跑、两处订正段的写法、README 的改法、81 行与 172 项、阶段 1 与 2 文档的那一句、六条阶段栏交给所有者；修正第 1 轮又订正了 89、90、99 行，补充了 94、97 行；修正第 2 轮补充了 104 ～ 106 行）。

### M6 推送之后的 CI（2026-10-29）

项目所有者 2026-10-29 推送了 `32238be`（`3a76b80` 之后的 82 个提交：读 M4 的 CI 的两个文档提交、M5 与 M6 的全部）。运行 40798658216（第 14 次运行）**失败**：`bench` 与 Linux、Windows 的 `check` 都成功；macOS 的 `check` 里 `cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 都过，测试一步只失败一条用例——`-p rurge-interop --test tuic :: v5_tcp_against_sing_box`，sing-box 起来就退出：`listen udp 127.0.0.1:55875: bind: address already in use`。**本节订正 M5、M6 两节里“CI 没有见过”“推送与读回 CI”一类的说法**（规矩是历史的段落不逐处改、由后写的段落订正）：

- **第一次被编译、运行的**：M5 与 M6 的代码（M6 的只有三条用例，在 `5e00564`、`fcf9464`、`37616a0`）在三个平台上过了 fmt 与 clippy（工具链 1.89.0）；缺省用例在 Linux 与 Windows 上全部通过，在 macOS 上除上面那一条之外都通过（失败摘要只列出它；测试一步带 `--no-fail-fast`，别的测试二进制都跑完了）。这些由各步的结论与失败摘要推出：任务日志要登录才能读，各平台跑了多少条用例、哪些打印了跳过，没有读到。
- **失败的那一条**：照源码读出的原因在测试夹具，不在产品代码——`tests/interop/src/lib.rs` 取参照服务端的端口时先绑 0 号端口、读号、放掉，再把号写进参照服务端的配置；号来自系统分临时端口的区间（macOS 缺省 49152–65535，55875 在里面），放掉之后、sing-box 绑上之前被别的套接字拿走了。测试约定早就不许这样取号，这个 crate 一直没有照做；这次推送的提交没有动 `tests/`，竞争从阶段 2 起就在，第 14 次运行第一次撞上。M6 插了任务 8a 修（`41c35d4`：四个取号函数改为从 20000–29999 取号、整个进程共用一趟遍历、交出去之前现场试绑；不改 CI、不加 `#[ignore]`、不重试；M6 记录第 111 行）。`41c35d4` 上的门禁（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI）：fmt 与 clippy 过，86 个结果行、4725 通过、0 失败、13 忽略，比任务 7 多的两条是 8a 的新用例；互操作用例在这台机器上立刻返回（没有参照服务端），算在通过的数里。所以修正管不管用**要看 `41c35d4` 推送之后的那一次运行**；`32238be` 之后的提交（`41c35d4` 与本节的文档提交）CI 还没有见过。
- **没有变的**：缺省的 `cargo test` 不建网卡、不改系统设置，三个平台的设备与系统改动都没有真的运行过，**没有在真机上验证**；CI 没设 `RURGE_TEST_PRIVILEGED` 与 `RURGE_TEST_WINTUN_DLL`，L2 的四条没有运行过；`[IP Rewrite]` 只在内存设备上证明过，`udp-priority` 的效果没有观察过；三份手工验收清单一项也没有做；第 14 节八条没有一条能写成「通过」，第 8 条（三平台 CI 绿）这一次也没有满足。
- **注解**只有平台的，与第 13 次运行相同：四个任务各一条 Node.js 20 弃用的警告，跑在 ubuntu-latest 上的两个任务各一条迁到 Ubuntu 26 的通知，`check (macos-latest)` 一条 arm64 排队的通知；照 R26、R27 不改 CI。
- 测试一步的时长：Linux 10 分 14 秒、macOS 11 分 0 秒、Windows 11 分 6 秒（工作流给这一步 60 分钟）。M6 一节「去向写着项目所有者的」里的“推送与读回 CI”做了一半：M5 与 M6 推送、读回了，`41c35d4` 与其后的提交还要推送一次（M6 延后第 2 行）。
- **文档怎样订正的**（M6 记录第 112 行）：所有者回「继续」而没有推送 `41c35d4`，任务 8 的文档部分照这一次运行先做。对外与参考的文档（两份 README、需求文档第 7 节、兼容性清单开头、`docs/testing.md`、手工清单、`docs/api/phase3.md` 末尾「M5 推送之后的 CI」一节）只写这一次运行的事实，修正与其后的运行指到本节；会随下一次运行变的只写在三处：本节、`CLAUDE.md`「现在在哪里」、验收文档（第 8 条两处、「靠 CI 的」、「留给项目所有者的」第 2 条）。
- **`41c35d4` 推送之后怎么做**（任务 8 的后一半）：先看推送到了哪里——`git rev-parse origin/main`（推送不是从这份检出做的时候它不会动，就读公开 API 的 `https://api.github.com/repos/danielnie001/rurge/branches/main`）；照 `CLAUDE.md`「现在在哪里」的办法读那次运行的注解与任务摘要；在本节后面接一段写读到的（不改上面几条）；改 `CLAUDE.md`「现在在哪里」、验收文档上面那几处与 `tests/interop/README.md` 里“下一次 CI 运行时核对”那一句；macOS 上那一条还失败，就在 M6 里再插任务修。

**`41c35d4` 推送之后（任务 8 的后一半）**：项目所有者 2026-10-29 推送了 `41c35d4`（只这一个提交；任务 8 文档部分的 `87342ff`、`db5ac0a` 那时还在本地）。运行 40824893108（第 15 次运行）的四个任务都成功，**三个平台全绿**，是 CI 第三次全绿（第一次见「CI 在 `ac0a98a` 上第一次全绿」一节，第二次见「M4 推送之后的 CI」一节）。上面几条写于这次运行之前，不改；与下面几条不同的，以下面为准：

- **各任务**：`bench` 的两步（`cargo bench -p rurge-rules --no-run` 与 `-p rurge-dns --no-run`）都成功；三个 `check` 的 fmt、clippy 与测试一步都成功，`Report the failed tests` 都跳过（它只在测试一步失败时跑）。测试一步的时长：Linux 9 分 6 秒、macOS 9 分 3 秒、Windows 13 分 2 秒。安装参照服务端的各步与第 14 次相同（Linux 十一步都成功；macOS 跳过 snell-server、tuic 0.8.5、masque-server 三步；Windows 跳过五步：这三步，加上 TrustTunnel 的安装与指向 `sshd` 的那一步）。
- **修正这一次管用**：第 14 次在 macOS 上失败的 `v5_tcp_against_sing_box` 这一次对真实的 sing-box 通过；取号改到 20000–29999 之后，装了参照服务端的平台上用到它们的互操作用例都通过（由测试一步的结论推出，测试一步带 `--no-fail-fast`；逐条的结果在要登录才能读的任务日志里）。一次全绿不证明竞争不会再出现：它是偶发的，旧的夹具在第 12、13 次运行里也没有撞上；修正的依据是源码——号不再来自系统分临时端口的区间（M6 记录第 111 行）。
- **第 14 节第 8 条（三平台 CI 绿）满足了**，以 `41c35d4` 为准（它之后的提交只改文档，CI 没有见过）；上面「没有变的」一条里“第 8 条（三平台 CI 绿）这一次也没有满足”以这一句为准，其余几句照旧：三个平台的设备与系统改动都没有真的运行过、没有在真机上验证，L2 的四条没有运行过，`[IP Rewrite]` 只在内存设备上证明过，`udp-priority` 的效果没有观察过，三份手工验收清单一项也没有做，其余七条没有一条能写成「通过」。
- **注解**只有平台的，与第 13、14 次相同（四个任务各一条 Node.js 20 弃用的警告，`bench` 与 `check (ubuntu-latest)` 各一条迁到 Ubuntu 26 的通知，`check (macos-latest)` 一条 arm64 排队的通知）；照 R26、R27 不改 CI。
- **文档怎样订正的**（M6 记录第 114 行）：`CLAUDE.md`「现在在哪里」的两条；验收文档第二部分开头讲 CI 的那一句、第 8 条两处、「靠 CI 的」、「留给项目所有者的」第 2 条；两份 README 写到第 14 次运行的三处各接半句；需求文档第 7 节与 `tests/interop/README.md` 各按日期接一句。兼容性清单开头、`docs/testing.md`、手工清单与 `docs/api/phase3.md` 只写第 14 次运行的事实、指到本节，不改。
- **M6 延后第 2 行（推送与读 CI）做完了**：M5、M6 与 `41c35d4` 都推送、读回了。**M6 做完了**；下一步照路线图是阶段 4（HTTP 引擎），先写它的设计。推送其后的文档提交、验收文档「留给项目所有者的」那几件都归项目所有者。

## 阶段 4（HTTP 引擎）

### M1a（嗅探与规则之前的 SNI）

M1a 做完了（2026-10-29；计划 `docs/superpowers/plans/2026-10-29-phase4-m1a-sniffing-plan.md`，下面写“M1a 记录第 N 行”与“M1a 延后第 N 行”指它末尾的两张表，写“P N”指它的「计划期决定」）。M1a 由协调的会话逐个任务调 Pi 做、逐个任务评审（做法同阶段 3 的 M4d ～ M6）。**M1a 的提交都在本地，CI 没有见过**（推送归项目所有者，M1b 做完一起推）。

做了什么：80、443 与 `force-http-engine-hosts` 命中的 TCP 会话经 SOCKS5、`CONNECT` 与虚拟网卡进来时先答“成功”，再读头一批字节、判出 TLS 或 HTTP，带着 SNI 与协议求规则、拨号，前缀一个字节不丢地转发；新 crate `rurge-http`（前缀流、ClientHello 的读取、HTTP 开头的判断、带时限的嗅探）；规则集文件里一行上的 `extended-matching` 作用于整集（兼容性清单 3.5 节那一行从 🟡 变成 ✅）；嗅探的五种结局计数，`GET /v1/rurge/http` 读出；文档收尾（兼容性清单、阶段 1 验收文档的三个数、`docs/api/phase4.md`、`docs/testing.md`、`CLAUDE.md`、`docs/README.md`、设计第 21 节）。逐任务（提交是短哈希）：

- **任务 0**（`e50599f`）：开工前核对（工作树、三份 crate 的名字与签名、`Cargo.lock` 的包数 519、`LISTED` 的 23 项、P9 的名单逐条读过），只往修正记录加第 1 ～ 14 行，0 条用例。
- **任务 1**（`061fee1`）：新 crate `rurge-http` 与前缀流 `Prefixed`；+4 条、+2 个结果行（88 / 4729）；`Cargo.lock` 519 → 520。
- **任务 2**（`d0f7914`、修正第 1 轮 `2b5e2c8`）：ClientHello、协议开头的判断与带时限的嗅探；+23 条，修正轮 +1 条（判次数的用例），共 +24（88 / 4753）。
- **任务 3**（`c3a3e12`）：规则集里一行的 `extended-matching` 及于整集；预匹配不计数的探询（`pre_refuses`）；+5 条（88 / 4758）。
- **任务 4**（`c485266`）：`Dialer` 的 `sniffable` 与 `serve`、`ServeOpts` 与 `hold_dropped`；SOCKS5 与 `CONNECT` 先答后交；+10 条（88 / 4768）。
- **任务 5**（`b9d1cd7`、修正第 1 轮 `3667af5`，只改注释）：虚拟网卡上可嗅探的会话整个交给 `Dialer::serve`；+3 条（88 / 4771）。
- **任务 6**（`894d78e`、文档订正 `64e5c9d`、修正第 1 轮 `d15c5a6`）：引擎先答后读、嗅探之后照 SNI 与协议求规则、记录与计数；+5 条、+1 个结果行（新的 `tests/sniff.rs`，89 / 4776）。
- **任务 7**（`624a2e9`、修正第 1 轮 `5201660`）：嗅探的端到端用例（`crates/rurge-engine/tests/sniff.rs` 从 5 条到 21 条）；+16 条（89 / 4792）。
- **任务 8**（`5b8ee14`）：`GET /v1/rurge/http`（`crates/rurge-api/src/routes/rurge_http.rs`，用例 `rurge_http_counts_start_at_zero`）；+1 条（89 / 4793）。
- **任务 9**（`6293faf`、`562e42c`、`38baa64`，修正第 1 轮的提交）：文档收尾，代码一行也没有改；兼容性清单改了 16 行、状态标记只改了 3.5 节那一行，附录整张重数（577 行，✅ 396、🟡 130、🔁 32、⛔ 14、❓ 5，只比上一版多一个 ✅）；阶段 1 的三个数从“2 句没有做完”变成“1 句没有做完”；新写 `docs/api/phase4.md`；`docs/testing.md`、`CLAUDE.md`、`docs/README.md`、设计第 21 节、计划的两张表。

一共 68 条用例、3 个结果行、1 个包（`Cargo.lock` 519 → 520，多的是 `rurge-http` 自己；`LISTED` 仍是 23 项）。门禁（任务 8 与本任务各一次，一台 Windows 11 x86-64 开发机上的一次运行，**不是 CI**）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过（0 个警告）；`cargo test -j 8 --workspace --no-fail-fast` **89 个结果行、4793 通过、0 失败、13 忽略**（M1a 之前 `41c35d4` 上是 86 / 4725 / 0 / 13）。重跑过的偶发写在下一条「以后的会话必须知道的事」里。

没做的，与不要当成已经有的：

- **M1b 的引擎没有做**：判为 HTTP 的会话在 M1a 照原样转发、协议记 `http`，进 HTTP 引擎（重写、Map Local、抓包、每个请求一条记录、上游连接池、`requests` 计数）是 M1b。
- **MITM 没有做**（M2）：判为 TLS 的会话照原样转发，只是带着 SNI 与协议 `https` 求规则。
- **HTTP/2 的 `:authority` 没有做**（随 M2 的引擎），**QUIC 的 SNI 没有做**（M5）。
- **M1a 的提交 CI 没有见过**：三个平台一次也没有编译、运行过它们。
- **手工验收没有做**：阶段 4 的手工清单从 M2 起建，M1 的行为（浏览器经三种入口上网、443 上按 SNI 分流）随它写进去。
- **阶段 1 的“37 句……其中 34 句今天不按需求的字面做”没有重数**：M1a 让其中几句对可嗅探的会话不再那样（`docs/acceptance/phase1-acceptance.md` 的「需求文字的问题」开头写了哪几行），这个数由阶段 4 的验收（M6）对着那时的代码重读。
- **`force-http-engine-hosts` 命中的明文在 M1a 照原样转发**，没有进引擎；**TLS 在 M1a 不解密**。

以后的会话必须知道的事（改那一块之前读对应的一条、设计与计划的两张表）：

- **嗅探之后会话走哪条路，哪些地方竞速、哪些不竞速**：嗅探与 `opts.stop`、会话根令牌竞速；拨号不与任何东西竞速（拨号握着会话的句柄，半途丢掉会让记录一直开着）；转发与服务结束与停止竞速；引擎里 `hold_dropped` 另与会话根令牌竞速（守护进程退出时不再停到 `drop_hold`）。设计第 7.1 节与 P10。
- **计数只在嗅探有结局时加**：被停止或守护进程退出打断的不计（没有一个结局），读出错的算 `closed`；读数自进程启动起累计，重载不清零。
- **讲“拨号之后才答”的现有用例用不嗅探的端口**（P9：443 → 8443、80 → 8080，断言不改、加一行英文注释）：M1a 改了六处（M1a 记录第 12 行与第 29 行）；新写的这类用例照做，否则它在 15 秒的等待上变红或变慢。
- **`give_up` 那一条偶发**：`crates/rurge/tests/cli.rs` 的 `run::enhanced_mode::an_unknown_enhanced_backend_ends_the_start_with_2` 在任务 3 的第一遍全量里以 "rurge run did not give up within 20 s" 失败，标准错误里已经有期望的 `error: unknown RURGE_ENHANCED_BACKEND value …` 一行；读代码，那一行之后只有立刻返回的 `recover_without_a_log` 与退出码 2，卡在进程退出的路上，原因不知道。这是阶段 1 / M5a 计划延后第 37 行那种卡住的再现，那一行把“有 `error:` 一行”算成产品的缺陷，这次的读法不支持那个归类，也没有否定它。单独重跑通过；任务 4 ～ 8 的全量里没有再出现（M1a 记录第 23 行）。
- **同一天 `STATUS_HEAP_CORRUPTION` 出现过五次**（`CLAUDE.md`「这台开发机」里那种、与本轮改动无关），都单独重跑通过：任务 3 的第二、三次全量（一次是 `outbounds_masque`，一次是 `rurge-api` 的单元测试二进制），任务 5 的全量（`outbounds_masque` 与 `udp`），任务 6 的全量（`udp`）。任务 0、1、2、4、7、8 的全量里没有（M1a 记录第 23、27、30 行）。
- **重跑的数怎么算**：修正记录里把第一次全量的数与单独重跑的数合起来记（例如任务 6 的 88 / 4738 加 `udp` 的 38 条得 89 / 4776）；上面的 68 条是各任务通过数之差的和。

去向写着项目所有者的（后来的会话不要自己定）：

- **推送**：M1a 与 M1b 的提交都在本地，M1b 做完推一次（M1a 延后第 2 行），之后读回那次 CI。
- **手工验收**（M1a 延后第 3 行）。
- **所有者可以改的裁定**：设计第 19 节（M1-D3、D5、D7、D11、D14、D23 等）与 M1a 里协调者作的 R1（探询看不到会话的进程）、R2（`NeedMore` 取握手消息最早能齐的位置）两个裁定（M1a 记录第 10、20 行；设计第 21 节）。
- **总设计 Q1、Q6**（设计第 19 节）：用户流量的内容算不算 API 输出；需求文档的阶段栏。

### M1b（明文 HTTP 引擎）

M1b 做完了（2026-10-30；计划 `docs/superpowers/plans/2026-10-29-phase4-m1b-http-engine-plan.md`，下面写“M1b 记录第 N 行”与“M1b 延后第 N 行”指它末尾的两张表，写“P N”指它的「计划期决定」）。M1b 同样由协调的会话逐个任务调 Pi 做、逐个任务评审。**M1b 的提交都在本地，CI 没有见过。**

做了什么：嗅探判为 HTTP 的连接（与 `force-http-engine-hosts` 强制进来的非 TLS 连接）进 `rurge-http` 的 hyper HTTP/1.1 引擎：每个请求各自求规则、选策略、借或拨上游连接，各有一条记录；上游连接在同一个策略、同一代配置、同一出口表时复用（池属于一条客户端连接）；HTTP 监听器的绝对形式请求也交给同一个引擎，`forward()` 退役；记录多 `method`、`url`、`statusCode`，借来的连接有 `remoteAddr` / `connectMs` / `firstByteMs`，每个请求一行会话日志；`GET /v1/rurge/http` 多 `requests`；承载请求的连接在一个请求也没走到选路时补一条记录。逐任务（提交是短哈希，用例数是那个任务之后的通过数）：

- **任务 0**（`0011f2b`、修正第 1 轮 `0b3cdf9`）：开工前核对与 P11 两张表的逐条读（第 1 ～ 21 行），只改文档，0 条用例（89 / 4793）。
- **任务 1**（`469e586`、修正第 1 轮 `3857f5a`）：`Trail::set_peer`、`SessionHandle` 的方法与状态码侧槽、`ResponseBody` 的错误类型、`rurge-http` 的四条依赖边与池（`Pool` / `Reusable` / `Meter` / `Metered`）、数接受次数的 `TestServer`；+9 条（89 / 4802；`Cargo.lock` 仍 520 个包）。
- **任务 2**（`b2f24fe`、修正第 1 轮 `b046c37`）：HTTP/1.1 引擎的每个请求流程、守卫、应答正文的包装、逐跳的头、拒绝与失败、504、`Conn::abort` 看门狗与 `Progress`；+25 条（89 / 4827）。
- **任务 3**（`058e3ea`、修正第 1 轮 `55b859f`）：升级（WebSocket）与隧道里的 `CONNECT`、借来的连接上的两种重发；+8 条（89 / 4835）。
- **任务 4**（`dc5c846`、修正第 1 轮 `590c1b9`）：`connect_session` 拆成选路与拨号、`ConnUpstreams`、嗅探到的 HTTP 进引擎、承载连接的记录；新的 `crates/rurge-engine/tests/http.rs`（多一个结果行），+9 条（90 / 4844）。
- **任务 5**（`7a88d1c`）：记录的 `method` / `url` / `statusCode` 与每个请求的会话日志行；+4 条（90 / 4848）。
- **任务 6**（`fafe89e`、修正第 1 轮 `2a5129b`）：`Dialer::plain_requests`、监听器的绝对形式交给引擎、fake 地址反查；删 `rurge-inbound` 的七条转发用例、加一条，引擎一侧 +9 条，净 +3 条（90 / 4851）。
- **任务 7**（`09226ac`、修正第 1 轮 `d9edc3c`）：端到端的 16 条用例（四种入口、池与借来的连接、重发的两种情形、WebSocket、chunked、`Expect`、终止与停止、承载连接的记录、504），净 +15 条（90 / 4866）。
- **任务 8**（`2efc298` 与本节的提交）：`requests` 的计数与用例、API 的一个键，加上文档收尾（兼容性清单、`docs/api/phase4.md`、`docs/api/phase1.md`、`docs/testing.md`、本文、`CLAUDE.md`、两份 README、需求文档、设计第 21 节、`docs/README.md`）；+1 条（90 / 4867）。

一共 74 条用例（基线 89 / 4793），1 个结果行（新的 `tests/http.rs`），没有新包（`Cargo.lock` 仍 520 个，`LISTED` 仍 23 项）。门禁：每个任务各跑了一次全量，任务 2、3、4、6、7 的修正轮又各跑了一次，任务 7 改了一处注释之后还有一次（`gate-task-7-rerun.log`）；都是一台 Windows 11 x86-64 开发机上的一次运行，**不是 CI**。`cargo fmt --all --check` 每次 0，`cargo clippy --all-targets -- -D warnings` 每次 0 个警告，全量最后一次是 90 个结果行 / 4867 通过 / 0 失败 / 13 忽略（任务 8 之前是 90 / 4866 / 0 / 13）。崩溃与偶发见下面「必须知道的事」的“偶发”一条。需要参照服务端的互操作用例在本机立刻返回、算在通过的数里。

没做的，与不要当成已经有的：

- **上游连接不跨客户端连接共享**（总设计 P4-D6）：池属于一条客户端连接，另一条客户端连接的请求一定另拨。
- **明文 HTTP/2 的先验照原样转发、不进引擎**（总设计 P4-D11）：引擎只说 HTTP/1.1。
- **HTTPS 仍不透明**：解密与 HTTP/2 随 M2。
- **四种重写与 Map Local 没有做**（M3）；**抓包视图与记录里的各段耗时没有做**（M4）；**逐请求求值的开销基准没有做**（M5）。
- **不改请求头**：不加 `Via`、`X-Forwarded-For`。
- **两处没有能红的用例**（M1b 记录第 33 行）：`Idle` 第一次 `poll_frame` 才起表、`Progress::restart`——去掉它们，现有的用例照样绿（延后第 4 行）。
- **M1a 与 M1b 的提交 CI 都没有见过**：三个平台一次也没有编译、运行过 M1b 的代码；也没有手工验收（阶段 4 的手工清单从 M2 起建）。`requests` 与 `sniffed` 只在用例里证明过，没有在生产环境里看过。

以后的会话必须知道的事（改那一块之前读设计第 8 ～ 12 节与计划的两张表）：

- **客户端不读时只有 `Conn::abort` 能结束那条连接**（M1b 记录第 50、44 行）：`rurge-http` 的看门狗（`watch_the_client`）在两件事之一到时 `abort.cancel()`：请求的令牌取消；或应答正文没有在等上游（上一次 poll 交出了一帧，或读到了末尾）而客户端满 `idle_timeout` 没有再取一帧——上一次 poll 时源头是空的、在等上游，看门狗就只睡、不判，那段等待归 `Tail` 计时。与它竞速（`biased`，`abort` 在前）的是两个 `serve_connection`：`rurge_http::serve_connection`（`crates/rurge-http/src/engine.rs`，`serve_http` 调的那一个）与监听器自己的那个（`crates/rurge-inbound/src/http.rs`）；`serve_http` 本身不竞速。`abort` 先到就丢掉连接、不记错误。hyper 1.11.1 里 `graceful_shutdown` 只在读缓冲空着时当场关，所以停止时 `routed() == 0` 的连接要当场结束，不等 `header_read_timeout`（新连接从 `KA::Busy` 开始，见记录第 44 行与 `crates/rurge-http/src/engine.rs`）。
- **测试上学到的（`docs/testing.md`「阶段 4 / M1b 起」一节有全文）**：断言钩子结果的用例用单线程运行时（`SessionHandle::finish` 照挂上的先后跑钩子）；“客户端不读”的用例写 256 MiB；kill 与停止之前等源站收到请求（`TestServer` 先记请求再按 `set_delay` 等）；代理端口上的 fake 地址用 `listener_rules`；规则里回环之外的地址段只写 `REJECT`（任务 7 修正第 1 轮）。
- **偶发**：两条 DoQ 用例——`a_doq_carrier_the_proxy_will_not_open_fails_naming_the_policy`（记录第 20、21 行与延后第 1 行）与 `a_doq_server_routed_to_a_proxy_without_udp_fails_while_another_answers`（记录第 43 行）——都单独重跑过、也随全量过。测试二进制没有正常跑完的（以 `STATUS_` 开头的退出码结束，或者没有结果行、退出码也没有记下），在这一台机器上 M1b 期间有七回，有的一回里不止一个二进制、不止一次（计划的修正记录只写了一部分，其余出自工作区的日志与各任务的报告）：一、任务 2 的 `engine::tests` 循环（`loop-task-2-engine.log`）36 次里第 7 次没有结果行，退出码没有记下；二、任务 2 修正轮的全量（`gate-task-2-fix-1.log`）`rurge-policy` 与 `rurge-rules` 两个二进制以 `STATUS_HEAP_CORRUPTION`（0xc0000374）退出，单独重跑两个都全过（`gate-task-2-fix-1-rerun.log`，406 条）；三、任务 3 的全量（`gate-task-3.log`）`rurge-policy` 一次 `STATUS_HEAP_CORRUPTION`，单独重跑 181 条全过（记录第 34 行）；四、任务 4 修正轮的全量（`gate-task-4-fix-1.log`）`rurge-api` 的单元测试二进制一次 `STATUS_HEAP_CORRUPTION`，单独重跑 31 条全过（`gate-task-4-fix-1-rerun.log`）；五、任务 6 修正轮的 `--test http` 循环 21 次里第 11、14 次没有结果行、退出码没有记下，直接跑那个 `.exe` 40 次里 39 次全过、1 次 0xc0000374（合起来 61 次里 58 次全过、3 次崩溃，没有一次是用例失败；`task-6-report.md` 的「修正第 1 轮」一节）；六、任务 7 改了一处注释之后的一次 `--test http` 以 0xc0000374 退出、没有结果行，直接跑同一个 `.exe` 35 条全过（记录第 78 行）；七、任务 8 的 `--test http` 循环（`loop-task-8-http.log`，记录第 81 行）第 17 次以 `STATUS_ACCESS_VIOLATION`（0xc0000005）退出、没有结果行，已打印的用例都是 `ok`，之后四次全过。`STATUS_HEAP_CORRUPTION` 那几次照 `CLAUDE.md`「这台开发机」归本机的管控软件；`STATUS_ACCESS_VIOLATION` 这是第二次见到（第一次在阶段 3 / M1 的 `rurge-api` 测试二进制上，阶段 3 / M1 计划延后第 12 行），两次的原因都没有查明。
- **`requests` 只在 `ConnUpstreams::route` 里加一**：嗅探到的连接、监听器的绝对形式与虚拟网卡共用这一个实现；新加一条进引擎的路要跟着它，否则那个入口的请求不计数。

去向写着项目所有者的（后来的会话不要自己定）：

- **推送**：M1a 与 M1b 的提交都在本地，推送之后读回那次 CI（M1a 延后第 2 行）。
- **设计第 17 节的 Q1 ～ Q6**（设计第 19 节）；**代所有者作的裁定可以改**：设计第 18 节的 M1-D1 ～ D27 与本计划的 P1 ～ P23。
- **升级之后转发的任务握着 `Shared`，池里别的空闲上游连接会活到转发结束**（由上游的空闲时限关；任务 3 评审）：以后可以考虑升级时清池（M1b 延后第 2 行）。
- **反查不到名字的 fake 地址两路次序不同**：`rurge-http` 先 `route` 再拒（规则命中数加一、记录带规则），`CONNECT` 那一路先拒（`tun_dns.rs` 断言记录没有规则）；要不要统一（M1b 延后第 3 行）。
- **记录里的 `url` 带着查询串出 API** 是总设计 Q1 的临时做法。
- **手工验收**（M1a 延后第 3 行）；**M1b 延后第 4 行那两条没有能红的用例**（以后的会话补）。

### M2a（MITM：HTTP/1.1 上的解密）

M2a 做完了（2026-10-30；计划 `docs/superpowers/plans/2026-10-30-phase4-m2a-mitm-http1-plan.md`，下面写“M2a 记录第 N 行”与“M2a 延后第 N 行”指它末尾的两张表；设计 `docs/superpowers/specs/2026-10-30-phase4-m2-mitm-design.md`，改设计正文的地方见它的第 23 节第 10 ～ 14 条）。照项目所有者 2026-10-26 的指示，同样由协调的会话逐个任务调 Pi 做、逐个任务评审。**M2a 的提交都在本地，CI 没有见过**（M1a 与 M1b 的也一样）。

做了什么：`[MITM]` 生效了。判为 TLS 的可嗅探会话在开关开着、有可用的 CA、来源允许、名单命中、ALPN 合得上时，rurge 用 `[MITM]` 的 CA 当场签一张叶证书、与客户端完成 TLS 握手，把解密之后的 HTTP/1.1 请求交给 M1b 的引擎：每个请求照旧各自求规则、选策略、借或拨上游连接、各有一条记录，只是上游一侧多一层 TLS、URL 以 `https://` 开头、协议是 `HTTPS`。另有 CA 的载入与六条检查（`W0039`）、`mitm` 开关与 `state.json` 第 2 版、握手失败与证书固定的提示与限频、计数、`GET /v1/mitm/ca`。

- **任务 0**（`0fb1ca4`）：开工前核对（第 1 ～ 18 行），只改文档，0 条用例（90 / 4867，本计划的基线）。
- **任务 1**（`015a29e`）：`[MITM]` 九个键类型化、两张名单与来源名单、`W0039` 的常量（`crates/rurge-config`）；+13 条（90 / 4880）。
- **任务 2**（`7011613`、修正第 1 轮 `f9819c2`）：`rurge-http` 的依赖与 `mitm::Authority`（六条检查与 `W0039` 的各种坏法、材料的指纹）；PKCS#12 的解码拆出来共用、对上游的客户端配置与两种根；`load_checked` 加 CA 的检查、重载的解码挪进 `spawn_blocking`；+20 条（90 / 4900）。
- **任务 3**（`149e7af`、修正第 1 轮 `e285c5a`）：叶证书与缓存、`Mitm`（何时解密、服务端配置、握手、`Watch`）；+17 条（90 / 4917）；延后第 1 行（运行中 CA 到期）。
- **任务 4**（`ad62336`）：解密的连接上的请求——上游先做 TLS、URL 是 `https://`、协议 `HTTPS`（`crates/rurge-http/src/engine.rs`）；+7 条（90 / 4924）。
- **任务 5**（`7fdd95b`、修正第 1 轮 `5097cb9`）：每一代的 `Mitm` 与沿用的 `Authority`、`mitm` 开关与 `state.json` 第 2 版、可嗅探的会话随名单扩大、解密的分派、`tests/mitm.rs`；+14 条（91 / 4938）；延后第 2 行（解密的流上对端不发 `close_notify`）。
- **任务 6**（`c80dd15`、修正第 1 轮 `b8506ec`、协调者的 `ced0838`）：握手失败与证书固定的提示、限频、`MitmCounts` 计数、承载连接的记录；+10 条（91 / 4948）。
- **任务 7**（`a9f00c7`）：`/v1/features/mitm` 生效、`GET /v1/mitm/ca`、`/v1/rurge/http` 的 `mitm`，启动时还原开关的用例；+4 条（91 / 4952）。
- **任务 8**（`16f2978`、修正第 1 轮 `03d033f`）：十条端到端用例（三种入口、同一条连接上的两个请求、不受信的源站与 `skip-server-cert-verify`、排除项与来源、`127.0.0.1` 的比较、重载的三种、WebSocket、明文不受影响、经 HTTP 代理的隧道）；+10 条（91 / 4962）。
- **任务 9**（本节的提交）：文档收尾（设计第 23 节第 10 ～ 14 条、兼容性清单、`docs/api/phase4.md`、`docs/testing.md`、本节与依赖附录、`CLAUDE.md`、`docs/README.md`）与一处注释里的节号（M2a 记录第 57 行），0 条用例（91 / 4962）。

一共 95 条用例（基线 90 / 4867），1 个结果行（新的 `tests/mitm.rs`），没有新包（`Cargo.lock` 仍 520 个，`LISTED` 仍 23 项）。门禁：每个任务各跑了一次全量，任务 2、3、5、6 的修正轮又各一次；任务 1 的全量里一个二进制崩溃、单独重跑那一个，任务 5 的第一次全量里一个二进制崩溃（单独重跑、再整份重跑、改了两处注释之后又跑一次），任务 6 修正轮的第一份全量里一个二进制崩溃（单独重跑、再整份重跑），任务 8 的全量与修正轮两次都干净、`--test mitm` 的循环两次各二十遍全过。`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 每一次都是 0（`-D warnings`）。这些都是一台 Windows 11 x86-64 开发机上的一次运行，不是 CI；需要参照服务端的互操作用例在本机立刻返回、算在通过的数里。

没做的，与不要当成已经有的：

- **HTTP/2 的解密没有做**（M2b）：只给 `h2` 的客户端照原样转发，`h2 = true` 读进来但不生效。
- **CA 的命令行工具**（`rurge mitm ca generate` / `export`）、**`http-api-tls`**、**阶段 4 的手工验收清单**都没有（M2c）；`README.md`、`README_en.md` 与 `docs/requirements.md` 也没有改（M2c 收尾时改，设计第 18 节）。
- **从来没有把 CA 装进任何信任库、没有解密过真实的站点、没有经真浏览器试过**：全部证据是回环上的用例（总设计 P4-D26、M2-D24）。
- **`auto-quic-block` 读进来、不生效**（M5）。
- **证书固定判断里两项“没有被停止”的条件没有能红的用例**（M2a 记录第 50 行、M2a 延后第 3 行）。
- **经虚拟网卡的“不信任”用例里，警报是用例手写的明文**（M2a 记录第 47 行）：真的 rustls 客户端经虚拟网卡发来的警报这条路没有用例（M2a 延后第 4 行）。
- **限频的两个边界只有单元用例**（M2a 记录第 45 行、M2a 延后第 5 行）。
- **运行中 CA 到期没有人管**（M2a 延后第 1 行）；**解密的流上对端不发 `close_notify` 就关闭的三种后果**照 rustls 的缺省（M2a 延后第 2 行）。
- **M1a、M1b 与 M2a 的提交 CI 都没有见过**：三个平台一次也没有编译、运行过 M2a 的代码（rcgen、ring 的那些特性在 Linux 与 macOS 上也没有编译过）；也没有手工验收。

以后的会话必须知道的事（改那一块之前读设计第 5 ～ 9 节与计划的两张表）：

- **解密的条件与次序**（设计第 7.1 节，计划 P18）：先问开关，再问名单、来源、ALPN、名字；反查不到名字的 fake 地址照 M1 原样转发。可嗅探的会话随名单扩大（第 7.6 节）：`hostname` 里写明的端口与 `:0` 的目标让那些端口也可嗅探，不带端口的项不扩。
- **每一代的 `Mitm` 与沿用的 `Authority`**（计划 P16）：材料相同且现在可用才沿用（叶证书与会话缓存跟着它）；第一代不经过 `swap_runtime`，由 `Engine::new` 自己记（M2a 记录第 41 行）；PKCS#12 的解码在阻塞线程上（重载的 `load_checked` 挪进了 `spawn_blocking`，计划 P12）。
- **`Watch` 数的是明文字节**（计划 P11）；证书固定的判断是计划 P21 的五条（握手完成、`routed() == 0`、读到 0 字节、客户端关的、没有被停止），`serve_connection` 的 `select!` 是 `biased` 的，`abort`、连接、令牌依序看，`routed() == 0` 时令牌一到就丢掉连接（M2a 记录第 50 行）。
- **测试上学到的，全文在 `docs/testing.md`「阶段 4 / M2a 起」一节**，最容易踩的三条：引擎用例的根不能是空的存储（空存储建不出校验器，那一代没有 `Mitm`，用例会在 MITM 没生效时绿）；要“不信任”就得让信的 CA 与签发的 CA 的 subject 不同（同名时是 `BadSignature`）；与 rustls 客户端握手的用例用真实时钟（钉在 2027-01-01 会 `CertificateExpired`）。
- **偶发**（都在这一台机器上，出处是工作区的日志与报告）：① 任务 1 的全量（`gate-task-1.log`）：`outbounds_snell` 以 `STATUS_HEAP_CORRUPTION`（0xc0000374）退出，单独重跑 17 条全过（`gate-task-1-rerun.log`）；② 任务 4 的 `engine::tests` 循环（`task-4-loop.log`）：20 次里第 19 次 `rurge_http` 的测试二进制以 0xc0000374 退出，已打印的用例都是 ok，之后又跑 5 次全过；③ 任务 5 的第一次全量（`task-5-gate.log`）：`rurge-proto` 的单元测试二进制以 0xc0000374 退出，单独重跑 502 条全过，整份重跑干净（`task-5-gate-rerun.log`）；④ 任务 6 修正轮的第一次全量：`rurge-http` 的单元测试二进制以 0xc0000374 退出，单独重跑 105 条全过，整份重跑干净（`task-6-fix-1-gate.log`）。都是 `STATUS_HEAP_CORRUPTION`，照本文件上面「这台开发机」归本机管控软件注入的钩子 DLL；**M2a 期间没有见到 `STATUS_ACCESS_VIOLATION`，也没有“没有结果行、退出码没记下”的一回**。另：任务 8 跑红时两次撞上链接错误 `LNK1104`（`cannot open file …\mitm-….exe`：上一个测试二进制的文件还被占着），等约 5 秒重跑同一条命令就过；**原因没有查明**，不要写成已知的原因。

去向写着项目所有者的（后来的会话不要自己定）：

- **推送与读回那次 CI**：M1a、M1b 与 M2a 的提交都在本地；设计第 22 节写的是 M1 与 M2 一起、M2c 做完之后推（M2-D1）。
- **设计第 22 节表里的各件**：Q4 的 `p12-keystore` 升级与 `webpki-roots`、Q1 的 URL（查询串）、Q3 的 `[MITM] # note` 这样的节头、Q5 的自动装 CA、经虚拟网卡的名字在 INFO 及以上；照设计第 22 节写的逐件去看，不要自己改写它们的意思。
- **手工验收**：阶段 4 的清单在 M2c 新建（`docs/acceptance/phase4-manual.md`）。
- **代所有者作的裁定可以改**：设计的 M2-D1 ～ D25、计划的 P1 ～ P27，与本文件、兼容性清单、计划表里写“协调者的裁定”的行。
- **M2a 延后第 1、2 行（去向写着所有者的）**：运行中 CA 到期要不要按时查、要不要告警或停止解密（第 1 行）；解密的流上对端不发 `close_notify` 就关闭要不要当成流的结束（第 2 行）。延后第 3 ～ 5 行去向后来的会话（没有能红的用例、真的客户端经虚拟网卡的那一条路、限频的两个边界）。

### M2b（MITM：HTTP/2）

M2b 做完了（2026-10-31；计划 `docs/superpowers/plans/2026-10-30-phase4-m2b-mitm-http2-plan.md`，下面写“M2b 记录第 N 行”与“M2b 延后第 N 行”指它末尾的两张表；设计 `docs/superpowers/specs/2026-10-30-phase4-m2-mitm-design.md`，改设计正文的地方见它的第 23 节第 20、21 条）。照项目所有者 2026-10-26 的指示，同样由协调的会话逐个任务调 Pi 做、逐个任务评审。**M2b 的提交都在本地，CI 没有见过**（M1a、M1b 与 M2a 的也一样）。

做了什么：`[MITM] h2 = true` 生效了。解密的连接上，客户端用 ALPN 选了 `h2` 的，rurge 用 hyper 的 HTTP/2 服务端讲它的请求；对上游的 TLS 同时给 `h2` 与 `http/1.1`，选了 `h2` 的上游连接在同一条客户端连接的同一个键下共享给并发的请求；两侧的版本可以不同，请求在两种形式之间转换（`:authority` 与 `Host`、拆开的 `cookie`、连接专用的头、`TE: trailers`）。HTTP/1.1 上“关掉整条连接”的终止、看门狗、拒绝与停止，在 HTTP/2 上改成“重置这个流”。另有：HTTP/2 上的 `CONNECT` 答 501、空闲之后 GOAWAY 与之后的一秒、被源站拒绝的流再发一次、七条端到端用例。

- **任务 0**（`a9a9ac3`、修正 `93a05ed`、协调者的 `b4f0051`）：开工前核对（第 1 ～ 23 行），只改文档，0 条用例（91 / 4962，本计划的基线）。
- **任务 1**（`3748a18`、修正第 1 轮 `faaefee`）：对客户端的 HTTP/2——执行器（`crates/rurge-http/src/exec.rs`）、版本转换（`version.rs`）、`serve_h2`（100 个流、不发 PING、空闲之后 GOAWAY 与之后的 `GOAWAY_GRACE`）、`CONNECT` 的 501（`crates/rurge-inbound/src/responses.rs` 的 `not_implemented`）、守卫的原因 `client cancelled the request`；+15 条（91 / 4977）。
- **任务 2**（`7745507`、修正第 1 轮 `ab853e0`）：对上游的 HTTP/2——`Sender`、`start_h2`、共享的 `Shared.h2`、按请求的字节、升级与隧道只拨 HTTP/1.1、被拒的流再发一次（`h2` 从开发依赖挪到正式依赖）；+8 条（91 / 4985）；延后第 1 行（`pipeline.rs` 那一条在并行负载下偶发红）。
- **任务 3**（`3806b64`、修正第 1 轮 `6609400`）：HTTP/2 上的终止、看门狗与拒绝——`Tail` 的上游收进与看门狗共享的槽、只重置这一个流、上游正文出错以 `UpstreamBodyFailed` 交出去；+7 条（91 / 4992）。
- **任务 4**（`21e1398`）：`Mitm` 按 `h2` 生效（服务端与对上游的 ALPN）、`Accepted.h2`、引擎把协议交给 `Conn`；+3 条（91 / 4995）。
- **任务 5**（`4cd2e6a`）：七条端到端（四种版本组合、`h2 = true` 下只给 `h2` 的客户端、每条流各自己的规则、并发的流共用一条上游连接、`cookie` 到 HTTP/1.1 源站、HTTP/1.1 客户端的 WebSocket 对讲 h2 的源站、`CONNECT` 的 501 与一条记录）；+7 条（91 / 5002）。
- **任务 6**（本节的提交）：文档收尾（设计第 23 节第 20、21 条、兼容性清单、`docs/api/phase4.md`、`docs/testing.md`、本节与依赖附录、`CLAUDE.md`、`docs/README.md`）与一处注释（`crates/rurge-http/src/version.rs` 里 `TE` 那一句，M2b 记录第 57 行），0 条用例（91 / 5002）。

一共 40 条用例（基线 91 / 4962），没有新包（`Cargo.lock` 仍 520 个，`LISTED` 仍 23 项）。门禁：每个任务各跑了一次全量，任务 1 ～ 3 的修正轮又各一次；任务 2 的第一次全量里 `rurge-dns` 的单元测试二进制以 `STATUS_HEAP_CORRUPTION` 退出，同一次里 `pipeline.rs` 那一条在并行负载下红，整份重跑干净；任务 5 的全量里 `outbounds_http2` 的测试二进制以 `STATUS_HEAP_CORRUPTION` 退出、没有结果行，单独重跑 15 条全过（`gate-task-5-rerun.log`）。`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 每一次都是 0。这些都是一台 Windows 11 x86-64 开发机上的一次运行，不是 CI；需要参照服务端的互操作用例在本机立刻返回、算在通过的数里。

没做的，与不要当成已经有的：

- **h2c**（明文的 HTTP/2，不论升级还是先验知识）：明文的连接只讲 HTTP/1.1；**扩展 `CONNECT`**（RFC 8441）不宣告；**服务端推送与优先级**、**对上游的 PING 保活**、**跨客户端连接共享上游的 HTTP/2 连接**都没有做。
- **上游在 END_STREAM 之前以 `RST_STREAM(NO_ERROR)` 结束一个流时，rurge 记的是完成**（M2b 延后第 2 行）：应答不带 `content-length` 时客户端看不出截断，M2b 不识别它。
- **客户端 HTTP/2 而上游只讲 HTTP/1.1 时，并发的请求各占一条上游连接、数目不设上限**（设计第 10.2 节的登记）；**拨共享 HTTP/2 连接的那一个请求的字节不是准确的数**，另带着握手与计量关掉之前走过的字节（设计第 23 节第 21 条）。
- **`pipeline.rs` 的 `a_dns_session_is_labelled_by_its_scheme_whatever_the_port` 在并行负载下偶发红**（M2b 延后第 1 行，M2b 记录第 46 行）：用例只等 `protocol` 出现就断言 `rule`，与 M2b 的改动无关；去向是 M2c 的计划，M2b 没有改它。
- **`rurge mitm ca generate` / `export`、`http-api-tls`、阶段 4 的手工验收清单**都没有（M2c）；`README.md`、`README_en.md` 与 `docs/requirements.md` 也没有改（M2c 收尾时改，设计第 18 节）。
- **没有手工验收**；**M1a、M1b、M2a 与 M2b 的提交 CI 都没有见过**：三个平台一次也没有编译、运行过 M2b 的代码（M2b 没有平台代码，用的 `h2` 与 hyper 的 HTTP/2 在别的平台上也编译过——阶段 2 的 HTTP/2 族用例早就在用 `h2`）。

以后的会话必须知道的事（改那一块之前读设计第 10 节与计划的两张表）：

- **“先有数据、再重置”的用例要用信号排先后，不能靠固定等待**（M2b 记录第 50、52 行）：h2 0.4.19 的 `send_reset` 会把这条流还排在队列里的帧丢掉（`h2-0.4.19/src/proto/streams/send.rs:254-268`）；hyper 的 `PipeToSendStream` 交给一段 DATA 之后会在同一次 poll 里接着读正文（`hyper-1.11.1/src/proto/h2/mod.rs:142-258`），上游的重置这时如果已经到了，hyper 随即重置客户端一侧的流、把还排在队列里没写出去的那段也丢掉。`OriginScript::ResetMidBody` 因此驱动到用例告诉它“那一段已经读到了”才重置。
- **交给 hyper 的错误不要在错误链里带着 `h2::Error`**（M2b 记录第 48 行）：hyper 按链里的原因码重置客户端的流（`Error::h2_reason`），`REFUSED_STREAM` 会让客户端把一个非幂等的请求再发一次。上游正文出错用 `UpstreamBodyFailed`。
- **HTTP/2 上“客户端不读”与“终止一个客户端不读的流”之后，在途数要等 hyper 下一次 poll 那个流、或者连接关掉才减**（任务 3 报告的「在途」一节，计划 P14，设计第 10.5 节的登记）。这不是缺陷，不要去“修”：连接因此在那一小段里不会被判为空闲。
- **`Tail` 的上游槽是 `std::sync::Mutex`**（M2b 记录第 49 行）：锁着它时不调 `complete` / `fail` / `stop`（不可重入），丢掉上游在锁外。
- **共享的 HTTP/2 上游上，拨号的那个请求的字节不准**（M2b 记录第 42 行，设计第 23 节第 21 条）：计量在 `start_h2` 返回之后才设成 `None`，之前走过的都算在它头上。
- **测试里**（全部在 `docs/testing.md`「阶段 4 / M2b 起」一节）：`TestServer::set_delay` 只对 `set` 过的路径起作用（M2b 记录第 29、45 行）；丢掉 hyper 客户端一个请求的应答 future 会发 `RST_STREAM(CANCEL)`（第 30 行）；判原因码用 `h2_reason_of`，没有原因码的 `h2::Error`（I/O）给 `None`（第 52 行）。
- **明文的会话只讲 HTTP/1.1**；`Accepted.h2` 在 `Watched` 包住之前读（M2b 记录第 53 行）。

去向写着项目所有者的（后来的会话不要自己定）：

- **推送与读回那次 CI**：M1a、M1b、M2a 与 M2b 的提交都在本地；设计第 2 节写的是 M1 与 M2 一起、M2c 做完之后推（M2-D1）。
- **代所有者作的裁定可以改**：设计与计划的已决事项，与本文件、兼容性清单、计划表里写“协调者的裁定”的行；M2b 的有：修正记录第 23 行（流控窗口的推理）、第 42 行（拨号请求的字节不追到准确）、第 48 行（上游正文出错一律以 `INTERNAL_ERROR` 重置），以及派工单里定的几件：任务 3 的 `Close` / `Stalled` 怎样选、`Tail` 的锁与 `Drop` 的规则、任务 3 修正轮把六条用例钉在原因码上。
- **M2b 延后第 2 行（去向就是 M2b 的任务 6）**：上游以 `RST_STREAM(NO_ERROR)` 提前结束要不要识别（M2b 不识别，是协调者的裁定；要识别，可以在上游讲 HTTP/2、`Incoming` 给了结尾而 `is_end_stream()` 仍是假时当作失败）。延后第 1 行去向 M2c 的计划。
- **设计第 22 节的开放问题**照旧归所有者（Q1 的 URL 与查询串、Q3 的节头、Q4 的 `p12-keystore` 与 `webpki-roots`、Q5 的自动装 CA、经虚拟网卡的名字在 INFO 及以上）。


### M2c（CA 工具、`http-api-tls` 与收尾）

M2c 做完了（2026-10-31；计划 `docs/superpowers/plans/2026-10-31-phase4-m2c-ca-tools-api-tls-plan.md`，下面写“M2c 记录第 N 行”与“M2c 延后第 N 行”指它末尾的两张表；设计 `docs/superpowers/specs/2026-10-30-phase4-m2-mitm-design.md`，改设计正文的地方见它的第 22 节与第 23 节第 28、29 条）。照项目所有者 2026-10-26 的指示，同样由协调的会话逐个任务调 Pi 做、逐个任务评审。**M2c 的提交都在本地，CI 没有见过**（M1a、M1b、M2a 与 M2b 的也一样），M2 的三份计划至此都做完了。

做了什么：三件事。

- **CA 的命令行工具**：`rurge mitm ca generate --out <目录>` 生成一张新的 CA（ECDSA P-256、10 年、回拨 2 天、`BasicConstraints CA:TRUE pathLen:0` 与 `KeyUsage keyCertSign,cRLSign` 都关键、带 SKI）并写四个文件（`rurge-ca.pem`、`rurge-ca.cer`、`rurge-ca.p12`（3DES + HMAC-SHA1）、`rurge-mitm.conf`）；目录要已存在、不覆盖、失败时删掉这一次写出的；Unix 上两个私密文件是 `0600`，Windows 上不设权限。`rurge mitm ca export -c <配置> [--out <文件>] [--der]` 读出 Profile 里那张 CA 的证书，只读 `[MITM]` 与 `[Keystore]`。
- **`http-api-tls`**：API 讲 HTTPS，叶证书由 `[MITM]` 的 CA 按客户端的 SNI 或连接的本地地址签，与解密的连接共用叶证书缓存；只给 `http/1.1`、不收 0-RTT；握手在各自的任务里、同时至多 64 个、每个 30 秒；没有可用的 CA 时 API 不启动、WARN 一行、**从不退回明文**；重载时关于 API 的告警有六句（P13）。`rurge status` / `reload` / `stop` 带 `-c` 时走 HTTPS、根只有那张 CA、按 IP 核证书。
- **`W0040`**：`http-api-tls` 开着而没有可用的 CA。`rurge check` 打在标准输出（按位置排序，没有位置的它排在最前），`rurge run` 打在标准错误（不带位置），重载时也一样。

另有：M2b 延后事项第 1 行（`crates/rurge-engine/tests/pipeline.rs` 一条用例的等待）、阶段 4 的手工验收清单 `docs/acceptance/phase4-manual.md`（30 项：M1 一节 9、M2 一节 21）与 M2 的收尾文档。

- **任务 0**（`3848a9a`）：开工前核对（记录第 1 ～ 18 行），只改文档，0 条用例（91 / 5002，本计划的基线）。
- **任务 1**（`cba22ab`、`6f63ddc`）：`W0040` 与 `pipeline.rs` 的等待；+2 条（91 / 5004）。
- **任务 2**（`3f9442d`）：`mitm::generate` 的四个文件与读回；+7 条（91 / 5011）。
- **任务 3**（`0da8262`）：`rurge mitm ca generate` / `export`；+5 条（91 / 5016）。
- **任务 4**（`52c8dc7`）：`ApiTls` 与 `Engine::api_tls`；+6 条（91 / 5022）。
- **任务 5**（`8198345`、修正轮 1 `9d4f05a`）：TLS 的监听器与 `serve_tls`；+10 条（91 / 5032）。
- **任务 6**（`6cd1d89`、`0dabdff`、修正轮 1 `852e483`）：起 API、重载的告警、命令行的 HTTPS 与两条端到端；+4 条（91 / 5036）。
- **任务 7**（`c8c1fca`、修正轮 1 `113dc44`、修正轮 2 `eedd9b1`）：阶段 4 的手工验收清单，只改文档。
- **任务 8**（本节的提交）：文档收尾（设计第 22 节与第 23 节第 28、29 条、兼容性清单、`docs/api/phase4.md`、`docs/testing.md`、本节与依赖附录、`requirements.md`、两份 README、`CLAUDE.md`、`docs/README.md`），只改文档，0 条用例（91 / 5036）。

门禁：除任务 7 外，每个任务各跑了一次全量（任务 7 与它的两轮修正只改文档，没有跑，修正记录第 41、43、45 行）。任务 4 的第一次全量里 `--test http` 的测试二进制以 `STATUS_HEAP_CORRUPTION` 退出，整个门禁重跑全绿；任务 1 与任务 2 各有一次测试二进制同样退出，单独重跑那一个全过。带等待的用例单独循环跑了 20 次（四圈是新写的用例，任务 1 那一圈是改了等待的旧用例），全过，五圈各属于谁：任务 1 改了等待之后的 `crates/rurge-engine/tests/pipeline.rs` 那一条（`a_dns_session_is_labelled_by_its_scheme_whatever_the_port`，M2b 延后第 1 行，`task-1-loop-pipeline.log`）、任务 5 的五条握手用例（`task-5-loop-tls.log`）、任务 5 的 `--test api` 两条（`task-5-loop-api.log`）、任务 5 修正轮 1 那五条又一圈（`task-5-fix-1-loop-tls.log`）、任务 6 的 `run::mitm` 两条（`task-6-loop-mitm.log`）。这些都是一台 Windows 11 x86-64 开发机上的一次运行，不是 CI；需要参照服务端的互操作用例在本机立刻返回、算在通过的数里。

没做的，与不要当成已经有的：

- **`#[cfg(unix)]` 的两处本机没有编译过**：`crates/rurge/src/cli/mitm.rs` 的 `restrict`（两个私密文件的 `0600`）与 `crates/rurge/tests/cli.rs` 的 `mitm_ca_generate_keeps_the_secrets_private`。第一次编译与运行它们的是项目所有者推送之后的 CI；在那之前，“Unix 上私密文件是 `0600`”只有读代码与照 `fakeip/file.rs` 写这两条依据。
- **没有手工验收**：`docs/acceptance/phase4-manual.md` 30 项一项也没有做；没有把 CA 装进任何信任库、没有经真浏览器打开过解密的站点、没有观察过 `generate` 打印的安装命令在任何平台上跑起来是什么样。全部证据在回环上。
- **M1a、M1b、M2a、M2b 与 M2c 的提交 CI 都没有见过**：**CI 的**三个平台一次也没有编译、运行过阶段 4 的代码；这台 Windows 开发机编译、运行过它们，只有 `#[cfg(unix)]` 的那两处（`restrict` 与 `mitm_ca_generate_keeps_the_secrets_private`）在这里编译不到。
- **`--remote` 对开了 `http-api-tls` 的实例连不上**（M2-D21）：没有加指定 CA 文件的参数；只有带 `-c` 的本机能用。
- **API 讲 HTTPS 时只讲 `http/1.1`**：没有 HTTP/2，也不在 ALPN 里给 `h2`。
- **Windows 上 `generate` 不给两个私密文件设 ACL**（M2-D20）：它们随所在目录的权限。
- **运行中不按时查 CA 的有效期**：CA 到期之后签出的叶证书本身就过期（M2a 延后第 1 行，已列进设计第 22 节，归项目所有者）。
- **证书固定的提示有两个误报来源**：浏览器的预连接，与握手完成之后才核证书的客户端（OpenSSL / LibreSSL 版的 curl 等）不信 CA；`curl --pinnedpubkey` 在 rurge 看来同样是证书固定（M2c 延后第 1 行，措辞归项目所有者）。
- **`M2b` 的其余没有覆盖的事照旧**（h2c、扩展 `CONNECT`、服务端推送、跨客户端连接共享上游的 HTTP/2 连接等），见 M2b 一节。

以后的会话必须知道的事（改那一块之前读设计第 13 ～ 15 节与计划的两张表）：

- **`rurge-http::mitm::testing::NOW` 是 2027-01-01T00:00:00Z，在今天之后**（M2c 记录第 10 行）：`generate_at(NOW)` 生成的 CA 用真时钟读回是“还没生效”，所以**用真时钟读回的用例用 `generate()`**，钉住日期的才用 `generate_at`。
- **两张同名的 CA 之间是 `BadSignature`，不是 `UnknownIssuer`**：webpki 先按 subject 找信任锚、再验签名，所以信的 CA 与签发的 CA 同名时，客户端发的是 `decrypt_error`。`generate` 的 CA 叫 `rurge MITM CA <日期>`，同一天生成的两张同名；要“另一个 CA”的用例给它另一个名字（`crates/rurge-http/src/mitm/api.rs` 的 `a_client_trusting_another_ca_is_refused` 用 `rurge test other CA`，`TlsFixture` 用 `TlsFixture::with_ca_name`）。
- **改一个 crate 的依赖，它下游的测试二进制都换哈希整套重编，旧的留在 `target/`**（M2c 记录第 25 行）：任务 2 的全量把 D: 从 31 GiB 吃到 9 GiB（`target/debug` 151 GiB），照 M4d 的先例 `cargo clean --profile dev` 删了 151.2 GiB（35 秒），再重编测试二进制约 13 分钟、clippy 约 2 分钟。改 `Cargo.toml` 的任务跑完要看一次 `df -h /d`。
- **rurge 的日志写在标准输出**（`crates/rurge/src/cli/run.rs` 的 `init_logging`，`fmt::layer()` 的缺省），诊断写在标准错误（`rurge check` 的那一份是标准输出）；端到端里抓 WARN 的 `output_until_it_is_running` 读标准输出、把标准错误丢掉。`rurge check` 的诊断有位置的带 `<文件>:<行>`、有 `hint` 的另起一行 `  hint: …`，按位置排序、没有位置的排在最前（所以 `W0040` 在带位置的 `W0039` 之前）；`rurge run` 的诊断不带位置、同样排过序（M2c 记录第 44 行）。
- **`rurge status` / `reload` / `stop` 带 `-c` 时从 Profile 里读 API 的地址**：端到端的 Profile 要写 `free_tcp_port()` 取的端口，`crates/rurge/tests/cli.rs` 的 `API_GENERAL` 是 `127.0.0.1:0`，那条命令连不上它（M2c 记录第 35 行 ②）。
- **`serve_tls` 不查有没有 CA**：没有也照样绑上，之后每次握手都失败。起 API 之前问 `Engine::api_tls()` 的是可执行文件（P12）；以后另起 HTTPS 的 API 的调用方也要先问。
- **`Engine::api_tls()` 与 `Engine::mitm_ca_der()` 不同**：前者是**最后一个生效过的** CA（`MitmShared::last_live`），后者是**当前这一代**的；重载让 CA 不可用之后，前者仍有、后者没有。API 签证书用前者（P9），重载时的 `api_ca_note` 问后者（P13）。
- **`crates/rurge-api/src/linger.rs` 的跟踪器对一条在握手中的连接数两次**（`accept` 给的 token 与握手任务）：所以 `TLS_HANDSHAKES + LINGER_MAX`（128）只是监听器这一侧占的连接，交给 `axum` 的不在其中。写“一个客户端最多让 rurge 占多少描述符”时不要把 128 说成整个 rurge 的上限（M2c 记录第 33 行）。
- **`crates/rurge/src/cli/run.rs` 有测试模块**（文件末尾），任务 0 的报告说没有，错了（M2c 记录第 36、38 行）。
- **两张表的写法**：M2c 记录第 36 行曾被原地改写（`0dabdff`），那违反「已写下的行不改写」；第 38 行记了这件事，后来的会话照第 38 行办，不要再去改第 36 行。

去向写着项目所有者的（后来的会话不要自己定）：

- **推送与读回那次 CI**：M1a、M1b、M2a、M2b 与 M2c 的提交都在本地；设计第 2 节写的是 M1 与 M2 一起、M2c 做完之后推（M2-D1）。推送之后先读那次运行的注解与任务摘要；`#[cfg(unix)]` 的两处第一次在那里编译与运行。
- **代所有者作的裁定可以改**：设计与计划的已决事项（M2-D1 ～ D25、P1 ～ P18），与本文件、兼容性清单、计划表里写“协调者的裁定”的各行——M2c 的有记录第 10、19、23、26、35 行与第 38 行 ③，以及派工单里定的几件（`W0040` 只写一处常量、`base64` 进开发依赖、`generate` ⑥ 的 Unix / Windows 两句用 `cfg!(unix)`、`sockets.rs` 新行按字节序放、`Endpoint` 不实现 `Debug`）；执行者的裁定同样可以改。
- **M2a 计划「延后事项」第 1、2 行**（去向都是项目所有者）：① 运行中要不要按时查 CA 的有效期、到期时告警或停止解密——M2c 已把它列进设计第 22 节；② 解密的流上对端不发 `close_notify` 就关闭要不要当作结束（M2a 照 rustls 的缺省报 `UnexpectedEof`）。
- **M2c 计划「延后事项」第 1 行**（去向是项目所有者）：证书固定提示会把“握手完成之后才核证书、因而不信 CA”的客户端也报成证书固定；改不改提示的措辞（“exclude it with `-<名字>`”）由所有者定——对这些客户端，正确的做法是装上并信任 CA。
- **设计第 22 节的开放问题**照旧归所有者：Q1（解密的请求带完整 URL 与查询串算不算 API 输出）、Q3（`[MITM] # note` 这样的节头）、Q4（`p12-keystore` 升到 0.4.x 与 `webpki-roots` 的许可）、Q5（把 CA 自动装进信任库）、经虚拟网卡来的会话名字在 INFO 及以上。
- **手工验收清单 `docs/acceptance/phase4-manual.md` 一项也没有做**（M1 与 M2 的行为，30 项；阶段 1、2、3 的清单也一项没有做）。

### M3a（URL Rewrite 与 Header Rewrite）

M3a 做完了（2026-10-31；计划 `docs/superpowers/plans/2026-10-31-phase4-m3a-url-header-rewrite-plan.md`，下面写“M3a 记录第 N 行”与“M3a 延后第 N 行”指它末尾的两张表；设计 `docs/superpowers/specs/2026-10-31-phase4-m3-rewrite-map-local-design.md`，改设计正文的地方见它的第 22 节）。照项目所有者 2026-10-26 的指示，同样由协调的会话逐个任务调 Pi 做、逐个任务评审。**M3a 的提交都在本地，CI 没有见过**（M1a、M1b、M2a、M2b 与 M2c 的也一样）。

做了什么：

- **一行的切法与正则**：`rurge-config` 的 `split_words`（空白分项、`"…"` 与 `'…'` 两种引号、`"` 与 `\`、没有配对的引号到行末、记着每项的起止）；`translate` 在编译之前按 ICU 的意思改写 `\h`、`\H`、`\v`、`\V`、`\R`、`\<`、`\>`、`\Q…\E`、`\Z` 九种写法（类里的三种拒绝）；替换模板照 `NSRegularExpression` 的读法；`Pattern::new` 先改写再编译，所以 `URL-REGEX` 规则、策略组的 `policy-regex-filter` / `policy-priority` 与四个重写节的正则同一套读法。脱敏一行没改，另加一条用例钉住输出不变。
- **两个节的读法**：`[URL Rewrite]`（四项、`gateway`）与 `[Header Rewrite]`（方向、四种动作、值是这一行余下的部分）类型化；读不了的行 `W0012`；写 `Content-Length` / `Transfer-Encoding` 的行保留并给 `W0041`；两个节从 `W0016` 的名单拿掉；`[Replica]` 认得并忽略，给 `I0005`。
- **`rurge-http` 的 `rewrite` 模块**：URL Rewrite 的依次求值（三个候选、`header` 只第一条生效、不合法的不算第一条、`302` / `307` 的网关与 `Location`、`reject`）、Header Rewrite 的四种动作与框架头的比较、新 URL 的合法性与换目标之后的 URI 与 `Host`、备注与计数。
- **每个请求自己的对上游 TLS**：池、共享的 HTTP/2 连接、`Outing` 与 `Resend` 的键换成 `(U::Key, OriginKey)`（任务 4 的 `type UpstreamKey<U>`），同一个目标的明文与 TLS 请求不共用连接；这一步行为不变，是给任务 5 的换目标铺路。
- **引擎里的各步**：请求一侧的 Header Rewrite → URL Rewrite（答复或换目标）→ 按请求的 `OriginTls` → 借或拨 → 写回改过的 `Host`；应答一侧在逐跳头之后改写；两侧的框架头失败都 502（或关连接 / 重置流）；嗅探到的连接上的 `CONNECT` 与绝对形式请求不过重写；选定 REJECT 一族的请求不过重写。
- **一代的 `Rewrite`、开关、计数与 API**：每一代建一次（两个节都空时不读根），建不成只关这一代；`rewrite` 开关（缺省开、`state.json` 第 2 版、启动时还原、在 `ConnUpstreams::route` 里按请求读）；计数在 `EngineShared.rewrite`（`Arc<RewriteCounters>`），自进程启动起累计、重载不清零；`GET/POST /v1/features/rewrite` 与 `GET /v1/rurge/http` 的 `rewrite` 六个数。
- **端到端**：手册两页的黄金用例与其余十二条。

各任务（括号里是全量的结果行 / 通过 / 失败 / 忽略，都是一台 Windows 11 x86-64 开发机上的一次运行，不是 CI；需要参照服务端的互操作用例在本机立刻返回、算在通过的数里）：

- **任务 0**（`677e7e0`，协调者 `80638d8`）：开工前核对，只改文档，0 条用例（91 / 5036，本计划的基线；记录第 1 ～ 14 行）。
- **任务 1**（`8137d86`、`bc2088a`、`e9ef947`，协调者 `3c32a3b`，修正轮 1 `7737a1a` 只改注释）：`split_words`、`translate`、`Template`；+16 条（91 / 5052）。
- **任务 2**（`ddb01b8`）：两个节的读法与 `[Replica]`；+9 条（91 / 5061）。
- **任务 3**（`ef152f9`、`475459a`、`e0250ec`，修正轮 1 `d83e507`、修正轮 2 `663ef06` 只改注释）：`rurge_http::rewrite`；+19 条（91 / 5080）。
- **任务 4**（`f844e1d`，`835383d` 订正它自己那一行的行号）：池与共享连接的键带上对上游的 TLS；+1 条（91 / 5081）。
- **任务 5**（`88e8f1d`、`508607a`、`dfe8ba1`，修正轮 1 `6a2c485`，协调者 `dd326ce`）：引擎里的各步；+17 条（91 / 5098）。
- **任务 6**（`ab9e181`、`6495d70`，修正轮 1 `e06e3cc` 只改注释，协调者 `a8a64d5`）：一代的 `Rewrite`、开关、计数与 API；+6 条、1 个新测试二进制（92 / 5104）。
- **任务 7**（`ae1922f`、`719e39e`，修正轮 1 `bd41f8e`）：端到端 19 条（92 / 5123）。
- **任务 8**（本节的提交）：文档收尾（设计第 22 节第 9 ～ 12 条、兼容性清单、`docs/api/phase4.md`、`docs/testing.md`、本节与依赖附录、`CLAUDE.md`、`docs/README.md`），只改文档，0 条用例（92 / 5123）。

门禁：任务 0 ～ 7 各跑了门禁，任务 8 只改文档、没有跑（M3a 记录第 68 行起）。任务 1、3（修正轮 1）、5、6 与 7 各有测试二进制以 `STATUS_HEAP_CORRUPTION`（本机已知的注入 DLL）退出，都单独重跑了那一个、全过；任务 5 三次门禁引的三份 `cargo fmt --all --check` 日志从来没有写出来（`Tee-Object` 在 fmt 什么也不输出时不建文件），协调者在 `6a2c485` 上自己跑过一次 `cargo fmt --all --check`，exit=0，此后每一步改用重定向写日志再追加退出码（记录第 54 行）。带等待的用例各自单独循环跑了二十次以上，全过（任务 5 与它的修正轮、任务 6、任务 7 的各批）；日志在工作区（git 忽略），记录里引的是第 47、50、53、62、64、65 行。

没做的，与不要当成已经有的：

- **`[Body Rewrite]` 与 `[Map Local]` 照旧不生效**：两个节仍在 `W0016` 的名单里，`rewrite` 的 `body` 与 `mapLocal` 恒为 0；它们随 M3b。
- **jq、`br`、`zstd` 与落到 `regex` 一侧的 Body Rewrite 随 M3c**，等项目所有者回答总设计 Q4（设计第 19 节第 3 条、第 21 节）。
- **两份 README 与 `docs/requirements.md` 没有改**：M3 的收尾文档在 M3b 收尾时一起改（设计第 17 节）。
- **没有手工验收**：`docs/acceptance/phase4-manual.md` 一项也没有做过，M3a 没有往里加条目（M3b 收尾时加）。全部证据是回环上的用例：没有经真浏览器、没有对真实的站点试过；`[IP Rewrite]` 那样在真设备上观察效果的证据一条也没有。
- **M1a ～ M2c 与 M3a 的提交 CI 都没有见过**：CI 的三个平台一次也没有编译、运行过它们；这台 Windows 开发机编译、运行过全部改动（M3a 没有平台代码，也没有只在 CI 上编译的代码）。
- **Surge 的实际行为没有观察过**：换目标之后的策略、新的主机与 fake 地址池、`header` 模式的 `{{{GATEWAY_ADDRESS}}}`、`wss` 算不算合法这几处照设计写，兼容性清单 7.3、7.4 节的备注登记的差异也没有对着 Surge 验证过。

以后的会话必须知道的事（改那一块之前读设计第 6 ～ 8、11、12 节与计划的两张表）：

- **替换只换配上的那一段、尾巴照接**（M3a 记录第 45 行）：`^http://a\.test/a https://b.test/secure` 配 `http://a.test/a` 得 `https://b.test/secure`；配某条只覆盖到主机的规则（`^http://a\.test https://b.test/`）同一个请求得 `https://b.test//a`——没配上的 `/a` 接在后面。
- **换到 `https` 而目标不讲 TLS 时，握手要等 `REWRITE_HANDSHAKE` 的 30 秒**（记录第 46 行）：用例里不要这样换；非要换的话用 accept 之后立刻关的源站。
- **建 `Rewrite` 的用例给不空的根**（记录第 20、32 行）：空的根是 rustls 的 `NoRootAnchors`，`Rewrite::build` 交 `Err`，这一代就没有重写，用例会在重写没生效的情况下绿。引擎用例经 `harness_with` / `harness_trusting` 给 `TlsFixture` 的根，不用 `harness(p)`（它的根是 `None`，`build_rewrite` 会去读系统的信任库）。
- **池与共享连接的键是 `(U::Key, OriginKey)`**（任务 4，`rurge-http` 的 `type UpstreamKey<U>`）：同一个目标的明文与 TLS 两种请求不共用连接；`OriginKey` 只放名字、不放配置（解密的连接用握手时那一代的配置，别的连接用这一代 `Rewrite` 的，M3-D31）。
- **应答一侧只在 `has_response_rules()` 为真时做**（记录第 48 行），101 那一支与普通那一支都是；应答一侧失败时那条上游连接不回池（HTTP/1.1 的随 `sender` 丢掉而关，HTTP/2 的流随正文丢掉而重置）。
- **`rurge-http` 引擎的测试模块里 `Fake` 比任务 5 之前多了几个开关**（记录第 42 行）：`rewrite`、`retargets`、`gateway`、`go_pages` 与 `forward`，写引擎用例时先看它们，不要另起夹具。
- **门禁的日志用重定向写、再追加退出码，不用 `Tee-Object`**（记录第 54 行）；门禁里单独重跑的结果也写进日志再引，引之前先看它在、看它写的是什么（记录第 54、63 行）。
- **`rewrite` 开关不属于一代**：在 `ConnUpstreams::route` 里读（`Engine::rewrite_on()`），切换对下一个请求生效、不结束开着的请求（设计第 11 节）。同一处看 `first.terminal`：选定 REJECT 一族的请求不过重写（M3-D27）。
- **`build_rewrite` 建不成只关这一代**（设计第 8.2 节），下一代自己再建；除了那一行日志（WARN `rewrites are off: cannot set up TLS: {e}`，或根读不出时的 DEBUG `the roots of the origin side cannot be read; rewrites are off for this generation`）没有别的痕迹。
- **计数在 `EngineShared.rewrite`（`Arc<RewriteCounters>`），不在 `Runtime`**：自进程启动起累计，重载不清零（设计第 12 节）；`body` 与 `mapLocal` M3a 就定下、恒为 0。
- **`crates/rurge-engine/tests/sniff.rs` 的那一处 `HttpCounts { … }` 每个字段写全、没有 `..`**（记录第 55 行）：给 `HttpCounts` 加字段时它编不过，要一并加。
- **改框架头而失败的请求，`header` 与 `failed` 各数一次**（P11 与设计第 12 节）：它的头确实被 Header Rewrite 改过；`notes_and_counts_reach_the_api_shape` 起初把 `header` 期望成 1、实际是 2，改的是用例（记录第 65 行）。
- **删掉 `Host` 不生效，但那一条规则照旧留 `Done` 与 `header`**，之后另留一条 `Skipped(Host cannot be removed)`（记录第 36 行、设计第 22 节第 12 条）；一条 `header-replace-regex` 规则至多一条 `Skipped`（第 35 行）。
- **`rurge-net` 的 `TestServer` 不记查询串**（`RecordedRequest.path` 是 `uri().path()`）：要断言查询串到了源站用 `tests/rewrite.rs` 的 `RecordingOrigin`；解密的端到端用例把源站的端口写进 `[MITM]` 的 `hostname`（记录第 64、65 行）。
- **`tests/rewrite.rs` 里照 `tests/mitm.rs` 抄来的夹具**（`mitm_section` ～ `h2_send`，注释写着出处）不在 `common` 里（P19）：改 `tests/mitm.rs` 的夹具时它们不跟着变，要一起改。

去向写着项目所有者的（后来的会话不要自己定）：

- **推送与读回那次 CI**：M1a、M1b、M2a、M2b、M2c 与 M3a 的提交都在本地。照设计第 2 节，M3a 之后不单独停下来等推送——M3b 做完之后由所有者一起推送，推送之后先读那次运行的注解与任务摘要。
- **代所有者作的裁定可以改**：设计与计划的已决事项（M3-D1 ～ D31、P1 ～ P21），与记录里写“协调者的裁定”的各行——M3a 的有第 13、14、20、36、37、41、42、43、48、51、52、55、61、66 行（第 19、54、63 行写的是“协调者的评审”，里面也定了事）；写“执行者的裁定”的同样可以改。
- **M3a 计划「延后事项」里去向是项目所有者的行**：今天一行也没有（那张表还是空的）。
- **设计第 21 节**：总设计 Q4（新包与升级，M3c 与 jq、`br`、`zstd` 等它）、Q2（重写节的头值要不要藏）、Q1（记录里的完整 URL 算不算 API 输出），以及设计里列的可以改的几项（尤其 M3-D2、M3-D4、M3-D7、M3-D16、M3-D27、M3-D30）。

### M3b（Body Rewrite 的正则形式与 Map Local）

M3b 做完了（2026-11-01；计划 `docs/superpowers/plans/2026-10-31-phase4-m3b-body-rewrite-map-local-plan.md`，下面写“M3b 记录第 N 行”与“M3b 延后第 N 行”指它末尾的两张表；设计 `docs/superpowers/specs/2026-10-31-phase4-m3-rewrite-map-local-design.md`，改设计正文的地方见它的第 22 节——执行期只改了一处意思，就是第 22 条：空的 `gzip` / `x-gzip` / `deflate` 那一层解成空的）。照项目所有者 2026-10-26 的指示，同样由协调的会话逐个任务调 Pi 做、逐个任务评审。**M3b 的提交都在本地，CI 没有见过**（M1a、M1b、M2a、M2b、M2c 与 M3a 的也一样）。**M3 至此做完**：M3a 与 M3b 都做完了；M3c（jq、`br` 与 `zstd`）等所有者回答总设计 Q4。

做了什么：

- **两个节的读法**：`[Body Rewrite]` 的式子 `BodyPattern`（先 `translate`、再用 `regex` 的多行与 CRLF 模式编，编不了再用 fancy-regex，回溯上限 1 亿），`[Map Local]` 的四种数据与 `header` 的两种写法、`status-code`；jq 的行 `W0042` 并跳过，Map Local 的本地路径写在下载到的文本里时 `W0033`（URL 的数据照收），两个节从 `W0016` 的名单拿掉，`CORE_VERSION` 的注释改成“已经做出来了”。
- **正文这一侧**：`ReadAhead` / `read_ahead`（每帧以 `idle` 为界、与请求的令牌赛跑、按上限停在上限加一个字节处），请求 32 MiB 与应答 10 MiB 才缓冲，`gzip`、`x-gzip` 与 `deflate` 逐层解，严格的 UTF-8 才改，改写过就去掉 `Content-Encoding` 与 `Transfer-Encoding`、重算 `Content-Length`、丢掉 trailer。
- **改写的求值与 Map Local 的回答**：规则按写的次序、一条规则里的各对依次作用，空的正文也能生成；Map Local 的五种数据、`Content-Type` 的表、文件的流式读与看门狗、打不开的 502。
- **引擎里的三步**：每个请求在 URL Rewrite 之后先过 Map Local（命中就答、不拨号），再过请求一侧的 Body Rewrite；应答在 Header Rewrite 之后过 Body Rewrite。
- **Map Local 的 URL 与资源管理器**：URL 的数据是外部资源的新一类 `data`（`ResourceKind::MapLocal`，标签 `map local data <键的前 8 位> (…)`），一个 URL 一项，下载、缓存与更新间隔照资源管理器。
- **计数与文档**：`body` 与 `mapLocal` 开始计；`docs/api/phase4.md` 的「M3b」一节、`docs/testing.md` 的十六条与手工清单的 M3 十三项（任务 8）。
- **端到端**：手册两页的九个黄金用例与其余十五条（32 MiB 与 10 MiB 的边界、chunked 与 `Expect`、HTTP/2、`br`、URL 的数据、解密的 `https` 请求、开关、正文不进备注与日志）。

各任务（括号里是全量的结果行 / 通过 / 失败 / 忽略，都是一台 Windows 11 x86-64 开发机上的一次运行，不是 CI；需要参照服务端的互操作用例在本机立刻返回、算在通过的数里）：

- **任务 0**（`64fdf00`，协调者 `307d539`）：开工前核对，只改文档，0 条用例（92 / 5123，本计划的基线；记录第 1 ～ 18 行）。
- **任务 1**（`b048276`、`c8d63aa`、`da1160a`，修正轮 1 `81215a5`、修正轮 2 `62401d0` 只改注释，协调者 `55ad675`）：`BodyPattern` 与 `[Body Rewrite]` 的读法、`W0042`；+12 条（92 / 5135）。
- **任务 2**（`031affc`，修正轮 1 `3ff8ad9` 只改注释，协调者 `55ad675`）：`[Map Local]` 的读法、`map_local_refusal`、`W0033`；+13 条（92 / 5148）。
- **任务 3**（`9646748`，修正轮 1 `44f7d9d`，协调者 `785bb88`、`8378416`）：`ReadAhead`、`read_ahead`、何时缓冲与解压；+13 条（92 / 5161）。
- **任务 4**（`3cce492`、`a33cb21`，修正轮 1 `2a1129d` 只改注释，协调者 `15332dd`、`3049efe`）：改写的求值与 Map Local 的规则；+15 条（92 / 5176）。
- **任务 5**（`5ea04a5`、`8070549`、`b7900d8`、`67ba2c4`，修正轮 1 `337e64b`，协调者 `c4ecd2e`、`bae0fea`）：引擎里的三步与流式读文件的正文；+24 条（92 / 5200）。
- **任务 6**（`7515fac`、`3a6bd8e` 只改它自己那一行的行号，协调者 `5333873`）：Map Local 的 URL 与资源管理器；+3 条（92 / 5203）。
- **任务 6a**（`84977e9`、`74dc9ec` 只改行号，协调者 `02abb16`）：`rurge-net` 两条用例改用 `rurge_net::testing` 的 `closed_udp_port` / `closed_tcp_port`，0 条新用例；`rurge_net` 的测试二进制循环二十次、每次 336 条全过（92 / 5203）。
- **任务 7**（`abc8da1`，修正轮 1 `64f7d05`，协调者 `16046c0`、`e500033`）：端到端 24 条（92 / 5227）。
- **任务 8**（`394b034`，修正轮 1 `f16eab9`，协调者 `2af46f2`、`733ccbf`）：手工清单、API 与测试的文档，只改文档，0 条用例。
- **任务 9**（本节的提交）：M3 的收尾文档（设计第 22 节与 M1、M2 两份设计的订正、兼容性清单、本节、两份 README、需求文档、`CLAUDE.md`、`docs/README.md`、`docs/api/phase1.md` 的两格），只改文档，0 条用例。

门禁：任务 1 ～ 7 各跑了门禁（每一次的全量数与退出码见记录第 33 ～ 78 行），任务 8、9 只改文档、没有跑。有几轮撞上开发机已知的两件事：`STATUS_HEAP_CORRUPTION`（任务 2、5、7 各一次，都单独重跑了那一个二进制、全过；记录第 34、64、77 行）、`LNK1104`（任务 2 一次（记录第 34 行）、任务 5 两次（第 55、65 行），等几秒重跑）与 `cli` 的等日志超时一次（任务 7，单独重跑过、原因没有查明，见下面“以后的会话必须知道的事”）。带等待的用例各自单独循环跑了二十次以上，全过（任务 5、6、6a、7 的各批；记录第 64、67、70、72 行）。日志在工作区（git 忽略）。

没做的，与不要当成已经有的：

- **jq、`br` 与 `zstd` 没有做**：jq 的行只给 `W0042`；`br` 与 `zstd` 的正文不改写、只留一条 `content-encoding br is not supported` 一类的备注。都在 M3c，等所有者回答总设计 Q4（设计第 9.1、9.4、21 节）。
- **没有手工验收**：`docs/acceptance/phase4-manual.md` 的 M3 十三项一项也没有做（整份清单四十三项全是 0）。全部证据是回环上的用例。
- **M3b 的提交 CI 都没有见过**：三个平台一次也没有编译、运行过它们；这台 Windows 开发机编译、运行过全部改动（M3b 没有平台代码，也没有只在 CI 上编译的代码）。
- **真站点上 Body Rewrite 常常没有效果**：真浏览器的 `Accept-Encoding` 带 `br`（这个构建不对上游收窄），真站点的应答常是 `br` 或 `zstd`，这样的应答 Body Rewrite 不改、只留备注（手工清单 W7 写了两种期望）。
- **带 `Expect: 100-continue` 的请求正文不缓冲**：curl 对超过约 1 MB 的 POST 缺省带它，大上传上请求一侧的 Body Rewrite 常常不起作用（手工清单 P9 因此加 `-H "Expect:"`）。
- **Map Local 的 URL 数据只从回环上的 `TestServer` 下载过**：没有对真实的可下载地址试过。

以后的会话必须知道的事（改那一块之前读设计第 9、10、12、14 节与计划的两张表）：

- **Map Local 的 URL 挂在这一代的资源管理器上**（记录第 67 行）：`rewrite` 开关关着照样下载；同一个 URL 只有一项，第一次登记定下种类与标签；更新间隔是资源管理器缺省的一天；不联网的一代不下载，数据目录里有副本时照副本答（这两句由任务 8 修正轮 1 订正过，记录第 85、86 行）。
- **写 M3 一带的用例之前先读 `docs/testing.md` 的「阶段 4 / M3b 起」十六条**（任务 8）：大正文在用例里生成、Map Local 的文件只在临时目录、`LogCapture` 断言的写法（字段的值是 `Debug`）、头名集合排序之后比、`ScriptedOrigin`、trailer 只在两端都讲 HTTP/2 时断言、取关着的端口用 `rurge_net::testing`。
- **在文档或清单里写 Surge 的示例行之前先用 `rurge check` 读一遍**（记录第 85 行 ⑦、⑧）：任务 8 把 `[URL Rewrite]` 的模式写在替换之前、`[Header Rewrite]` 漏了 URL 式子，两行照原样都是 `W0012`、整行作废。
- **到 HTTP/1.1 客户端的 trailer 一个也不转**（记录第 20 行）：`Trailer` 是逐跳的头，引擎从 M1b 起就把它去掉；两端都讲 HTTP/2 时照转。兼容性清单 7.1 节那一行与 M1、M2 两份设计的订正节都写了。
- **回溯上限 1 亿的代价量过一次**（记录第 22 行）：调试构建、一台机器上的一次运行——`(?=a)b` 对 32 MiB 用了 15.0 s，`(a|b|ab)*(?=c)` 对 80 字节用了 21.2 s 之后交回 `None`。一个写在 Profile 里的坏式子可以让一个请求慢二十秒；上限要不要改归所有者。
- **测试里取关着的端口的写法**（记录第 69 行的裁定与任务 6a）：`rurge_net::testing` 从任务 6a 起有 `closed_udp_port` 与 `closed_tcp_port`；全工作区另有五处“取 0 号端口、放掉、再用”的写法，在延后事项第 1 行，去向是协调者在 M4 实施之前派一个任务，其中 `crates/rurge-engine/tests/pipeline.rs:2738` 与红过的那一处同是 Windows 上的 UDP，最该先改。
- **`crates/rurge/tests/cli.rs` 的 `run::remote_sources::the_link_to_the_managed_profile_is_shown_as_where_it_goes_and_nothing_more` 在任务 7 的全量里等日志超时一次**：单独重跑过；原因没有查明（那一次的日志随用例的临时目录删掉了），读源码的一种可能写在记录第 76 行、没有证实。让下一次能查的改动是延后事项第 2 行，去向是协调者、与第 1 行一起派。再见到它，先读那一次的日志。

去向写着项目所有者的（后来的会话不要自己定）：

- **推送与读回那次 CI**：M1a、M1b、M2a、M2b、M2c、M3a 与 M3b 的提交都在本地。由所有者一起推送（历史里有一版计划带着本机账户名，见 M1b 计划记录第 84 行），推送之后先读那次运行的注解与任务摘要。
- **代所有者作的裁定可以改**：记录里写“协调者的裁定”的各行——第 20（trailer）、21（过时的“until M3b”）、36（`#[allow(dead_code)]`，写的是“协调者的裁定照做”）、40（空的正文与解压）、69（取号、加任务 6a）、94（第③条：7.5 节“请求体缓冲上限 32 MB”一行改回 ✅）行；写“协调者的评审”的第 64、68、76、85、87 行里也定了事（第 64 行定了 Map Local 的答复不数上下行字节，第 68 行定了 `online_harness` 留在 `crates/rurge-engine/tests/rewrite.rs`，第 76 行定了 `cli` 那一次失败的写法与去向，第 85 行定了任务 8 修正轮 1 的十七处，第 87 行定了手工清单的 `[Header Rewrite]` 写成两行、各配一个主机）。写“执行者的裁定”的同样可以改。
- **M3b 计划「延后事项」里去向是项目所有者的行**：今天一行也没有（三行分别是协调者两行与任务 9 一行）。
- **设计第 21 节**：总设计 Q4（新包与升级，jq、`br`、`zstd` 等它）、Q2（重写节的头值要不要藏）、Q1（记录里的完整 URL 算不算 API 输出），以及设计里列的可以改的几项。
- **回溯上限的代价**（记录第 22 行的测量）：1 亿要不要改、要不要按正文大小变。

依赖附录：M3b 没有加 crate 之间的边（`rurge-config` 的正式依赖多 `regex`，`rurge-http` 的正式依赖多 `flate2`、开发依赖多 `tempfile`，都是工作区已有的外部包，`Cargo.lock` 仍 520 个包；`unsafe_code` 的名单仍是 23 项，`crates/rurge/tests/sockets.rs` 的名单仍是 26 行）。

**任务 10（M3 收尾之后补派，2026-11-01）**：照延后事项第 1、2 行的去向，协调者在写 M4 的设计之前派了一个只改测试代码的任务（M3b 记录第 97 ～ 100 行；提交 `4a49d1d`、`594d93f`，协调者 `dfa22fc` 与本段的提交）。八处“取 0 号端口、放掉、再用”的写法改成取自 20000–29999、现场试绑一次的 `closed_tcp_port()` / `closed_udp_port()`：延后事项第 1 行的五处，加上协调者补扫出的三处在块里绑、出块就放掉的（`crates/rurge-proto/src/trust_tunnel.rs`、`crates/rurge-proto-quic/src/trust_tunnel.rs` 与 `crates/rurge-proto-quic/src/masque/mod.rs` 各一条用例）；`crates/rurge/tests/cli.rs` 那条偶发超时的用例等不到日志时，先结束守护进程，再把日志全文与它的标准错误、标准输出一起放进失败的信息。0 条新用例，全量 92 / 5227 / 0 / 13（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI；需要参照服务端的互操作用例在本机立刻返回、算在通过的数里）；改过的九条用例各循环二十次、`cli` 那一条十次，全过——竞争造不稳，循环没有红只说明没有见到。上面“以后的会话必须知道的事”里“测试里取关着的端口”与 `cli` 那两条说的“另有五处”“去向是协调者”到此做完：再见到 `cli` 那一条超时，失败的信息里就有那一次的日志，先读它。依赖：`rurge-proto` 的开发依赖多一条带 `testing` 特性的 `rurge-net`（工作区已有的 crate），生产依赖不变，`Cargo.lock` 仍 520 个包。代所有者作的裁定多一行：记录第 97 行的四条（snell 那一处取关着的 TCP 端口、删掉 `free_port()`、`rurge-proto` 的开发依赖、失败的信息先结束守护进程再读到底），所有者可以改。

### M4a（抓包）

M4a 做完了（2026-11-01；计划 `docs/superpowers/plans/2026-11-01-phase4-m4a-capture-plan.md`，下面写“M4a 记录第 N 行”与“M4a 延后第 N 行”指它末尾的两张表；设计 `docs/superpowers/specs/2026-11-01-phase4-m4-capture-records-design.md`，改设计正文的地方见它的第 18 节——执行 M4a 时算得上一条意思的是第 7 条（`ENTRY_OVERHEAD` 512 → 7168），第 8 条是正文少了的限定（全局代理只在全局策略设了而且认得时不求规则），不是执行期改的）。照项目所有者 2026-10-26 的指示，同样由协调的会话逐个任务调 Pi 做、逐个任务评审。**M4a 的提交都在本地，CI 没有见过**（M1a、M1b、M2a、M2b、M2c、M3a 与 M3b 的也一样）。**M4 还剩 M4b（请求记录的完整）**，M5 与 M6 仍是计划。

做了什么：

- **存储与记账**（`rurge_http::capture`）：只在内存里的一份 `CaptureStore`，总量 100 MiB 与单条正文 5 MiB 两个上限（`Limits`），一条的账是头按 RFC 7541 的「名字 + 值 + 32」、URL 与方法与规则与策略与备注的字节、两个正文存下的字节、`ENTRY_OVERHEAD`；正文按块向存储预留，装不下先挤掉最早的，单条超限只留开头（`Truncated::Limit`）、总量挤不出时从那一块起不再存（`Truncated::Budget`），之后的字节只计数；存进去、被丢掉与进行中的条目被丢下时预留都还回去，存储记着的字节在任何时刻不超过总量；会话就是条目上的名字（`default` 与 `always-capture` 写的都可以列出来）。
- **什么请求被抓**：`capture` 开关开着、或这个请求求规则时命中的那一行带 `always-capture=<名字>`（开关关着也抓；`FINAL`、`RULE-SET`、`DOMAIN-SET` 与逻辑规则都算）；只在求规则时，直连、`external`、全局代理而全局策略设了且认得的这三处早退没有命中的那一行；`always-capture=` 的值是空的给 `W0012`（规则照常、这一行不抓），不带 `=` 的照旧 `W0003`。
- **引擎里的 `CaptureSlot`**（`rurge-http`）：每个请求一个，`Requests::handle` 外面包一层；两个取样点（求规则之后 arm、第 5 步 `headers::strip` 之后取离开 rurge 时的请求）、`Idle` 里抄请求正文、包住交给客户端的应答正文（`CaptureBody`）、在 HTTP 部分结束的那一刻存进去；`REJECT-DROP` 由 `when_finished` 的回调认出（`answer` 一行不改）。
- **`rurge-engine`**：`EngineShared.capture`（一个引擎一份、重载之后条目还在）、`capture` 开关与 `state.json` 第 2 版、`choose_policy_with` 填槽、`ConnUpstreams::route` 的两个分支给 `capture`、`lib.rs` 的 `pub use rurge_http::capture`。
- **API**：`/v1/features/capture` 生效；`/v1/rurge/capture`、`/sessions`、`/sessions/{name}`、`/entries/{id}`、`/clear` 与 `/sessions/{name}/har` 六个端点，HAR 1.2 逐条写出（流式，不先在内存里拼出整个文件）。
- **命令行**：`--capture-size` / `RURGE_CAPTURE_SIZE` 与 `--capture-body-size` / `RURGE_CAPTURE_BODY_SIZE`（字节数，或带 `k` / `m` / `g`），单条上限大于总量时降到总量并打一行 WARN。
- **量一次内存**：调试构建的库内用例，装满与基线各跑一次——基线 17473536 字节，装满（每侧 14 行头、4 KiB 正文）的峰值 131170304，比基线多出的与总量之比 1.0843（`ENTRY_OVERHEAD` 512 时是 1.5207，改成 7168 之后是 1.0266；数照修正记录第 76、81 行）。
- **文档**：`docs/api/phase4.md` 的「M4a　抓包」一节、`docs/testing.md` 的十条命令与「阶段 4 / M4a 起」二十一条、手工清单的 M4 一节九项（任务 9a），M4a 的收尾（任务 9b）。

各任务（括号里是全量的结果行 / 通过 / 失败 / 忽略，都是一台 Windows 11 x86-64 开发机上的一次运行，不是 CI；需要参照服务端的互操作用例在本机立刻返回、算在通过的数里）：

- **任务 0**（`85db664`，协调者 `ecd7695`）：开工前核对，只改文档，0 条用例（92 / 5227，本计划的基线；记录第 1 ～ 20 行）。
- **任务 1**（`fb2c1f8`、`808fb56`、`49dafa3` 只改它自己那一行里一个名字的写法，协调者 `e44e11f`）：`SessionHandle::when_finished`、抓包会话的槽与空的 `always-capture=` 的 `W0012`；+7 条（92 / 5234）。
- **任务 2**（`b43c83d`、`9d6dac0`、`bbbd40f`、`014cd9a` 只改它自己两行里各一个错字，修正轮 1 `6c57466` 只改注释，协调者 `3a20a65`、`10bdb5d`）：`capture` 模块——存储、按块记账、挤掉、两种截断与丢掉、命名会话、读出时解开（`decode` 拆出 `layers` 与 `undo`）；+18 条（92 / 5252；修正轮只改注释，没有跑全量）。
- **任务 3**（`2c706b7`、`78a4fb6`、`4effc06` 只改它自己那一行里一句，修正轮 1 `551728e`，协调者 `fc04dcb`、`3ffab14`）：`CaptureSlot`、两个取样点、`Idle` 里抄请求正文、包住应答正文的一层；+18 条（92 / 5270；修正轮 1 之后）。
- **任务 4**（`665657a`，修正轮 1 `3e76e78` 只改注释，协调者 `7816399`、`6518861`）：`rurge-engine` 的存储、开关、`always-capture` 填槽与 `capture_for`，端到端的前一半；+9 条（93 / 5279；全量里 `rewrite` 一次 `STATUS_HEAP_CORRUPTION`，单独重跑 50 条全过、合起来数）。
- **任务 5**（`398014c`、`d184717`，修正轮 1 `c09717b`，协调者 `d491a43`、`f94b7b1`）：`capture` 开关与五个端点；+9 条（93 / 5288；`outbounds_wireguard` 一次 `STATUS_HEAP_CORRUPTION`，单独重跑 16 条全过）。
- **任务 6**（`5fb6ff2`、`5c6af62`、`1d67bc4` 只订正它自己两行的行数，修正轮 1 `7cab029`，协调者 `569e17e`、`608e172`）：HAR 1.2；+7 条（93 / 5295；`pipeline` 一次 `STATUS_HEAP_CORRUPTION`，单独重跑 78 条全过）。
- **任务 7**（`a74a044`，修正轮 1 `212853a`，协调者 `b04f733`、`54f8da7`）：两个参数与命令行的端到端；+6 条（93 / 5301；`rurge-dns` 的单元测试二进制一次 `STATUS_HEAP_CORRUPTION`，单独重跑 287 条全过）。
- **任务 8**（`6bfba7f`、`40f36c9`，修正轮 1 `c5993d8`，协调者 `39cb465`、`ca9170b`）：端到端的后一半与量内存；+9 条，其中 2 条是忽略的量内存用例（93 / 5308，忽略的从 13 条变成 15 条），修正轮 1 又加 1 条（93 / 5309；`rurge-dns` 的 `resolver` 一次 `STATUS_HEAP_CORRUPTION`，单独重跑 11 条全过）。
- **任务 9a**（`ce9087f`，修正轮 1 `8c8f64a`，协调者 `fe17ebc`、`f83942e`）：手工清单、API 与测试的文档，只改文档，0 条用例（记录第 83 ～ 90 行）。
- **任务 9b**（本节的提交）：M4a 的收尾文档，只改文档，0 条用例。

门禁：任务 0 ～ 8 各跑了一次全量（`cargo fmt --all --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test -j 8 --workspace --no-fail-fast`；每一次的数见各任务的括号与记录），任务 9a 与 9b 只改文档、没有跑（修正轮只改注释或只跑聚焦用例的，见记录第 31、40、46、53、58、70、81 行）。七次撞上开发机已知的 `STATUS_HEAP_CORRUPTION`（任务 1 的聚焦跑、任务 3、4、5、6、7 与任务 8 的修正轮各一次），都单独重跑了那一个二进制、全过（记录第 23、39、45、52、59、67、81 行）；`LNK1104` 与 `cli` 的等日志超时一次也没有见到。带等待或并发的用例各自单独循环跑了二十次以上、全过（任务 1、3、4、7、8；记录第 21、36、43、62、71、75 行）。日志在工作区（git 忽略）。

没做的，与不要当成已经有的：

- **M4b 的事都没有做**：记录的 `receiveMs`、两个正文的字节数、九种直通的原因、`remoteAddr` 在复用的连接与隧道与 UDP 流上的新口径、`dnsMs` 在途（设计第 10 节）——记录照旧，只有抓包这一半。
- **读一条（`?decode`）与 HAR 不解 `br` 与 `zstd`**：随 M3c、等所有者回答总设计 Q4；`notDecoded` 与 HAR 的 `comment` 说 `content-encoding br is not supported` 一类，手工清单 W9 写了它是登记的不同。
- **没有 Dashboard**：抓包视图只有 API 与 HAR（`/v1/rurge/capture/…`），界面在阶段 6。
- **没有手工验收**：`docs/acceptance/phase4-manual.md` 的 M4 九项（W9、W10、M9、M10、L9、L10、P11 ～ P13）一项也没有做（整份清单五十二项全是 0）。全部证据是回环上的用例。
- **M4a 的提交 CI 都没有见过**：三个平台一次也没有编译、运行过它们；这台 Windows 开发机编译、运行过全部改动（M4a 没有平台代码，也没有只在 CI 上编译的代码）。
- **导出的 HAR 没有被任何浏览器导入过**（手工清单 W10、M10、L10 就是它），也没有对真实站点抓过包：没有把 CA 装进任何信任库、没有经真浏览器试过。
- **`always-capture` 不让连接进 HTTP 引擎**（M4-D8）：不经 HTTP 引擎的连接不会因为它被抓，要抓原样转发的连接得另写 `force-http-engine-hosts` 或 `[MITM]`（清单 L271 的备注）。
- **抓包没有落盘**：存储只在内存里，进程退出就没了；不导入 HAR、不拆 `cookies`。

以后的会话必须知道的事（改那一块之前读设计第 4 ～ 8 节与计划的两张表）：

- **P3 的锁的次序**（P3，记录第 24 行）：`when_finished` 先锁 `on_finish`、在这把锁里读一次 `outcome`（另一把锁，读完就放），读到 `Some` 就放掉 `on_finish` 的锁再调 `f`；`finish` 从不同时握两把锁，`f` 在任何锁之外调，`f` 里再调 `when_finished` 或 `on_finish` 不死锁。
- **P8 的状态机与锁的次序**（记录第 39、40、41 行）：`Armed` 里的 `pending`、`request`、`answer`、`finished` 各自什么时候写；`answer` 三态（`Waiting` / `Nothing` / `Given { head, done }`）；`try_commit` 只在 `pending` 还在、`finished` 是 `Some`，而且 `answer` 是 `Nothing`（应答之前就被丢下、升级、客户端重置流那几种）或 `Given { done: Some }` 时存进去；arm 之后在任何锁之外调 `when_finished`（句柄可能已经结束，那时回调就在那一句里跑）；锁的次序是 `Armed` → 存储。
- **不许握着槽的锁 poll 正文**（P9，记录第 40 行）：包住应答正文的 `CaptureBody` 先 poll `inner`、再把数据交给 `tap`，`Idle` 的 `RequestTap` 也一样（先取数据、再锁住记账）；握着 `Armed` 的锁时不调 `finish`、不调 `when_finished`。
- **`REJECT-DROP` 的认法**（P8，记录第 39、40 行）：由 `when_finished` 的回调认出——结局是 `Rejected(Drop)` 而 `answer` 还是 `Waiting` 的就是没有应答（引擎里只有它先结束记录、停留之后才 `Err(Close)`）；以后改 `answer` 里 `REJECT-DROP` 那一支要同时看 `CaptureSlot`。
- **`rurge-http` 的 `testing` 特性**（P2，记录第 26、49 行）：只给别的 crate 的用例存进自己造的条目（`CaptureStore::put`），生产代码不开；`rurge-api` 的开发依赖开着它，`cargo test --workspace` 与 `cargo clippy --all-targets` 带着它编、`cargo build` / `cargo run` 不带，两种各一份产物；开它的那一次全量 D: 从 70G 掉到 46G（记录第 52 行）。
- **量到的内存**（记录第 76、81、82 行，数也写在 `docs/api/phase4.md`）：调试构建、一台机器上的一次运行——基线 17473536 字节，装满（每侧 14 行头、4 KiB 正文、每 1024 条里一条 6 MiB 的）峰值 131170304，比值 1.0843；`ENTRY_OVERHEAD` 的 7168 是这么定的（512 + (多出的 − 总量) / 存着的条数，取整到 64 的倍数），缺省的上限不因此改（M4-D7）。
- **存进去的条目复制自己的头**（`crates/rurge-http/src/capture/store.rs` 的 `owned`，`Pending::commit` 在拿存储的锁之前调；记录第 80、81 行）：hyper 1.11.1 的 HTTP/1 头值是那条连接读缓冲的切片，克隆只加引用计数，不复制的话一条条目会把 8 KiB ～ 约 408 KiB 的读缓冲留着而账上不计；h2 0.4.19 解出的值本来就是复制的。以后往条目里放从 hyper 来的 `Bytes`，同样先复制。
- **`ENTRY_OVERHEAD` 的 7168 只对量的那种形状成立**（记录第 76、80 行）：头的行数多得多的条目实际的占用仍比账多；一条最小的条目的账也在 7168 字节以上，`--capture-size` 小于它时什么也存不下、不告警。
- **`arm` 在 `token` 的检查之前**（P9 ②，记录第 39 行 ③、第 40、41 行）：在 `route` 与检查之间被 `kill` 或随连接停下的请求也有条目；没有用例钉住这个次序（`Fake::route` 没有能插手的点，协调者裁定不加钩子）。
- **HTTP/2 上正文中途被重置的流**（记录第 39 行 ④、第 40 行）：hyper 1.11.1 与 h2 0.4.19 当场丢下正文，条目当场存进去、带着已经过去的那一段（`an_http2_stream_reset_mid_body_is_captured_with_what_went_by`）。
- **`ConnUpstreams::route` 的 `Refused` 那一支**只接住 `route_session` 的三种 `Refused::Fail`，REJECT 一族经 `Go` 那一支、拨号时才被拒（`M3-D27`）；改这两个分支时两处都要给 `capture`（记录第 45 行 ②）。三处早退（`external`、直连、全局策略设了而且认得的全局代理）不填槽：直连那一处有用例，全局代理与 `external` 两处没有（记录第 45 行顾虑 ③）；全局策略没设或不认得的全局代理照规则走、照填（第 88 行 ③）。
- **监听器的口令在三处被拿掉**（记录第 73、80 行）：监听器的入站（`crates/rurge-inbound/src/http.rs:156`）、`arm` 的头、`leaving` 的头——`leaving` 那一处的 `Keep` 同时决定发出去的请求与条目的头。
- **四个凭据头的遮住在一处**（记录第 53、57 行）：`crates/rurge-api/src/routes/capture.rs` 的 `headers_json`，HAR 也经它；HAR 的两侧不对称是设计第 8.2 节定的（应答一侧解开再判断文字或 Base64，请求一侧不解；`postData` 的 `_encoding` 是 rurge 自己的键）。
- **抓到的内容从不进日志、错误文本、诊断或 `Debug`**（P5，设计第 6.6、9 节）：`Entry`、`Head`、`ResponseHead`、`Body`、`Capture`、`CaptureStore`、`Pending`、`CaptureSlot` 都不实现 `Debug`，`assert!` 的失败信息里也不打印正文（打印长度、ID、状态码可以）；任务 8 的 `captured_content_never_reaches_the_log` 在 TRACE 一级钉住它。
- **没有一处不记账地缓冲正文**（P6）：正文的块是抄进存储的（`Bytes::copy_from_slice`），不是从 hyper 的缓冲切出来的——切出来的 `Bytes` 握着整块缓冲，账就不对了。
- **`docs/api/phase4.md` 的「M4a　抓包」一节照代码写**（记录第 83 行）：状态、会话列表、摘要与 `timings` 是结构体声明的次序，一条的详情与 HAR 的一条是 `json!` 的字母序；以后改这些键时照代码改那一节的例子。
- **`?decode` 的五句由 `Body::decoded` 的次序定**（记录第 88、89 行）：先读层（那一行不是可见的 ASCII 就是第三句），没有层是 `Ok(None)`，有层而正文被截断的不管层叫什么都是 `the body was truncated`，其余的从最后一层往前解。
- **`docs/testing.md` 的十行命令的条数是 `cargo test … -- --list` 数的**（记录第 84 行，第 88 行核过）：`--test capture` 不写过滤词（`-- capture` 只选中 12 条），API 的 14 条拆成三条过滤词。
- **写抓包一带的用例之前先读 `docs/testing.md` 的「阶段 4 / M4a 起」各条**（任务 9a），另有 P14：端到端的用例在新文件 `crates/rurge-engine/tests/capture.rs`，夹具从 `tests/rewrite.rs` 照抄（每份前面一句注释写来处与为什么不挪进 `common`）。
- **手工清单 M1 一节的 Profile 里 `loglevel = debug` 得 `W0012`**（rurge 只认 `verbose`、`info`、`notify`、`warning`）：第 26 行末尾接了订正，第 20 行那一行没改；往清单里加新的一节、抄 Profile 时别抄它（记录第 87 ～ 90 行）。

去向写着项目所有者的（后来的会话不要自己定）：

- **推送与读回那次 CI**：M1、M2、M3 与 M4a 的提交都在本地（历史里有一版计划带着本机账户名，见 M1b 计划记录第 84 行），由所有者一起推送，推送之后先读那次运行的注解与任务摘要。
- **代所有者作的裁定可以改**：记录里写“协调者的裁定”的五处——第 22（`W0012` 那一条的四行怎么排，`FINAL` 放最后）、第 31 与第 46（只改注释的修正轮不跑全量门禁）、第 41（不给 `Fake` 加钩子：`arm` 在 `token` 的检查之前那一次序没有用例钉住）、第 49（`rurge-api` 的开发依赖加 `flate2`，只供测试）行；写“协调者的评审”的第 80 行里也定了事（产品：存进去的条目复制自己的头），第 24 行定了此后循环每一次记下 `test result:` 那一行。
- **M4a 计划「延后事项」里去向是项目所有者的行**：今天一行也没有（延后事项是一张空表）。
- **设计第 17 节**：Q1（抓到的内容算不算 API 输出、四个头遮住的做法）、M4-D4 的两个缺省上限（100 MiB 与 5 MiB）、M4-D8 的 `always-capture` 范围（不让连接进引擎、不让 TLS 被解密）、Q4 的 `br` 与 `zstd`（随 M3c）；M4b 的两项写明是「M4b 的事」（`remoteAddr` 的新口径、`dnsMs` 在途）。
- **任务 8 留下的两件**（记录第 80 行，协调者没有改、照设计第 4.3 节的固定开销）：头的行数多得多的条目实际的占用仍比账多，要不要按行数记账；`--capture-size` 小于一条最小的条目时什么也存不下（而且不告警），要不要告警或拒绝。

依赖附录：M4a 没有加生产代码里 crate 之间的边（`rurge-api` 的正式依赖多 `base64` 与 `futures-util`，都是工作区已有的外部包，正式依赖里工作区的 crate 仍只有 `rurge-engine` 与 `rurge-config`；`rurge-api` 的开发依赖多 `rurge-http`（开 `testing` 特性）与 `flate2`——开发依赖不在「生产代码的依赖方向」里；`rurge-engine` 以 `pub use rurge_http::capture;` 把类型交给 `rurge-api`；`Cargo.lock` 仍是 520 个包，只有 `rurge-api` 的依赖列表多 `base64`、`flate2`、`futures-util` 三行；`unsafe_code` 的名单仍是 23 项，`crates/rurge/tests/sockets.rs` 的名单仍是 26 行）。

M4a 做完，下一步是 M4b（请求记录的完整）的计划——不等推送，对着那时的代码写；推送归所有者。

### M4b（请求记录的完整）

M4b 做完了（2026-11-02；计划 `docs/superpowers/plans/2026-11-02-phase4-m4b-records-plan.md`，下面写“M4b 记录第 N 行”与“M4b 延后第 N 行”指它末尾的两张表；设计 `docs/superpowers/specs/2026-11-01-phase4-m4-capture-records-design.md`，改设计正文的地方见它的第 18 节——执行 M4b 时没有改过正文的意思，第 18 节末尾另记了正文没写的两处限定，都不是执行期改的）。照项目所有者 2026-10-26 的指示，同样由协调的会话逐个任务调 Pi 做、逐个任务评审。**M4 至此做完**（M4a 抓包与 M4b 请求记录的完整）。**M4b 的提交都在本地，CI 没有见过**（M1a ～ M3b 与 M4a 的也一样）。

做了什么：

- **记录多三个键**（`rurge-inbound`、`rurge-engine`、`rurge-api`）：`SessionHandle` 多两个 `AtomicU64`（`request_body`、`response_body`，`new_with_slot` 里初始化为 0）与 `add_request_body`、`add_response_body`、`body_bytes`、`first_byte_at` 四个方法；`RequestRecord` 与 `RequestJson` 多 `receiveMs`（紧跟 `dns_ms`）、`requestBodyBytes` 与 `responseBodyBytes`（紧跟 `status_code`；`RequestJson` 的字段次序就是 JSON 里键的次序）。`receive_ms` 是会话的时长（`h.elapsed()`）减去 `first_byte_at()`（从会话开始到上游的第一个字节，`crates/rurge-inbound/src/session.rs` 的 `first_byte_at`），即从上游的第一个字节到会话结束（`crates/rurge-engine/src/observe.rs` 的 `record_of`：`outcome.and(h.first_byte_at()).map(|f| elapsed.saturating_sub(f) ..)`）；进行中的会话与从没有收到上游字节的是 `null`；不是 HTTP 请求的会话（`method` 是 `null`）两个正文也是 `null`（记录第 7、8 行）。
- **两个正文在哪里数**（`rurge-http` 的引擎，M4-D30）：请求正文在 `Idle` 交出每一帧时数（两种版本都数，与 `Meter` 各记各的）；应答正文在 `Tail` 与 `LocalFile` 交出每一帧时、`complete()` 之前数（不看 `counted`）；rurge 自己的答复由私有的 `own_body` 按应答正文的长度记，它在这十处 `finish` 之前调（`TinyGif`、HTTP/2 上 `CONNECT` 的 501、三处 `bad_request` 与 `map_local` 的 `(None, data)` 是换了次序的——原来是先 `finish` 再建应答）（记录第 12、17 行）。
- **九种直通的原因**（`rurge-engine` 的 `serve.rs` 与 `engine.rs`、`rurge-http` 的 `mitm/decrypt.rs`）：九句是 `serve.rs` 的私有常量；`Mitm::decides` 交回五种（`Decrypt`、`NotListed`、`SourceBlocked`、`NoCertificate`、`AlpnMismatch`），次序是名单 → 来源 → 名字 → ALPN；`decrypting` 的次序是名单 → 开关 → CA → 来源 → 名字 → ALPN，解不解密与 M4b 之前相同；原因在拨号、建句柄时由 `dial_noted` 写（`PassedBy::Sniff(..)` 与 `PassedBy::Lists`），`Dialer::dial` 只是 `PassedBy::Lists` 那一条路（记录第 22 行）。没有加日志行。
- **`remoteAddr` 的新口径**（M4-D23；`rurge-net` 的 `connector.rs`、`packet_datagram.rs` 与 `quic/`、`rurge-proto` 与 `rurge-proto-quic` 的各个载体、`rurge-proto-wireguard`）：`Datagram::far_end` 与 `PacketSocket::far_end` 两个有缺省的新方法；UDP 流与内部的 UDP 会话由 `note_far_end` 与 `wrap_internal_udp` 写（开不成载体的两处 `clear_peer`）；复用、共用的 TCP 载体拨号时记着远端、借出时 `trail::connected`（snell 连同失效重拨、anytls、`h2pool`、ssh）；QUIC 一族的远端在 `Path::far_end`（`rurge_net::quic::FarEnd`，跳端口的是 `hop::Current`、经承载的是承载的远端，不用 quinn 的 `remote_address()`）；WireGuard 的 `Shared.far_ends` 随载体更新、`Device::far_end_for` 按协议栈给这个目标选中的那个对端答；`record_of` 读 `h.trail().peer()`，不再分 TCP 与 UDP（记录第 36 ～ 39、44 ～ 48、52 ～ 55 行）。
- **`dnsMs` 在途**（`rurge-net` 的 `trail.rs`、`rurge-dns` 的 `resolver.rs`）：`Left` 多一份在途查找的表与票据的号，新的 `Lookup`（不派生 `Clone`、不实现 `Debug`；`begin` 登记，`Drop` 加上花的时间并注销），`dns()` 给已结束的合计加上在途的到此刻为止，`trail::current()` 是 `CURRENT.try_with(Trail::clone).ok()`；`resolver.rs` 的 `Noted` 换成 `Lookup::begin()`（记录第 30、33 行）。
- **量一次一条记录的大小**（`crates/rurge-engine/src/observe/memory.rs`，新文件；两条带 `#[ignore]`）：任务 8 的 1085.3 字节一条、协调者评审那一次的 1081.8；1 000 条（`--request-log-size` 的缺省）约 1.03 MiB、10 000 条约 10.3 MiB、100 000 条约 103 MiB；1088 是取整到 8 的结果，不是量到的数（记录第 62、67 行；数也写在 `docs/api/phase4.md`）。
- **文档**：`docs/api/phase4.md` 的「M4b　请求记录的完整」一节、手工清单 M4 一节的十项（W11、W12、M11、M12、L11、L12 与 P14 ～ P17）、`docs/testing.md` 的两条命令（任务 9a），M4b 的收尾（任务 9b）。

各任务（括号里是全量的结果行 / 通过 / 失败 / 忽略，都是一台 Windows 11 x86-64 开发机上的一次运行，不是 CI；需要参照服务端的互操作用例在本机立刻返回、算在通过的数里）：

- **任务 0**（`9205dfe`，协调者 `9a0d165`）：开工前核对（名字与签名、P15 的现有用例、外部库的事实——hyper 1.11.1 的 HTTP/1.1 服务端不写 `HEAD` 的正文、`tokio::time::Instant` 与 `try_with`、记录在 `on_finish` 的钩子里拍下），只改文档，0 条用例（93 / 5309，本计划的基线；记录第 1 ～ 6 行）。
- **任务 1**（`a535147`，修正轮 1 `8f7623f` 只改注释，协调者 `d3088c1`）：句柄的两个计数与 `first_byte_at`、记录与 API 的三个键；+6 条（93 / 5315）。
- **任务 2**（`b4ae895`、`f22c72e`，修正轮 1 `3470e06` 只改断言，协调者 `3411874`）：`Idle`、`Tail`、`LocalFile` 与 `own_body` 数正文；+10 条（93 / 5325）。
- **任务 3**（`51e5886`、`30eeed7`、`40f31be`，修正轮 1 `0af3451`，协调者 `1e8143a`）：九种原因；+15 条（93 / 5340）。
- **任务 4**（`793f371`、`3c6ccdb`、`448a4e2`、`9457f8a`，协调者 `ba4a686`）：`Lookup`、`trail::current()` 与 `dnsMs` 在途；+7 条（93 / 5347）。
- **任务 5**（`95a4abc`、`e85d60d`、`4323335`、`73e3552`、`1e6cc8a`、`d129367`，协调者 `7d06251`）：两个 `far_end`、UDP 流与内部 UDP 会话的远端；+7 条（93 / 5354）。
- **任务 6**（`43fcb45`、`b7f9a53`、`8a11377`、`ca87b0b`、`e783ccd`、`0ec7cd9`、`8360a13`、`3845ad0`，协调者 `339e141`）：复用、共用的 TCP 载体（snell 连同失效重拨、anytls、`h2pool`、ssh）；+7 条（93 / 5361）。
- **任务 7**（`e9b99d4`、`9235156`、`4cd3e86`、`7fe4998`、`f0aa553`、`e960894`，修正轮 1 `c5689c1`，协调者 `97f02a8`）：QUIC 一族与 WireGuard；+13 条（93 / 5374）。
- **任务 8**（`91ec9f1`、`97b5fe4`、`005daa1`、`2e6abfe`，修正轮 1 `af84a16`、修正轮 2 `f5723fd`，协调者 `fcb6aa1`）：经 API 的端到端（`the_request_records_carry_the_three_new_keys`）与量一条记录的大小；+2 条，另加 2 条带 `#[ignore]` 的（93 / 5376，忽略的从 15 变成 17）。
- **任务 9a**（`223a83c`，修正轮 1 `11fbd9d`，协调者 `032a187`）：API、手工清单与测试的文档，只改文档，0 条用例（记录第 68 ～ 70 行）。
- **任务 9b**（本节的提交）：M4b 的收尾文档，只改文档，0 条用例。

门禁：任务 0 ～ 8 都跑了全量（`cargo fmt --all --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test -j 8 --workspace --no-fail-fast`），有几个不止一次（任务 4、5 各两次，任务 6 五次，任务 7 三次，任务 8 五次；记录第 35、41、51、60、67 行），任务 9a 与 9b 只改文档、没有跑。M4b 最后一棵代码树（`f5723fd`）上的全量是 93 / 5376 / 0 / 17、退出码 0（记录第 67 行；一台机器上的一次运行，不是 CI）。五次撞上开发机已知的 `STATUS_HEAP_CORRUPTION`（任务 2、4、6 的两次、8，记录第 13、34 与 35、51、67 行），每次单独重跑那一个二进制、全过；`LNK1102: out of memory` 两次、rustc 一次 `0xc0000409`、`LNK1104` 一次（记录第 40、57 行，换并行度或等几秒重跑即过）；`cli` 的 `give_up` 超时一次也没有见到。带等待或并发的用例各自单独循环跑了二十次以上、全过（任务 2 ～ 8；记录第 13、23、28、40、41、49、56、59、60、67 行；任务 4 的那一次只记在它的报告里，修正记录没有写）。日志在工作区（git 忽略）。

没做的，与不要当成已经有的：

- **没有手工验收**：`docs/acceptance/phase4-manual.md` 现在六十二项，M4b 的十项（W11、W12、M11、M12、L11、L12 与 P14 ～ P17）一项也没有做；M4b 的全部证据是回环上的用例与假的对端。
- **M4b 的提交 CI 都没有见过**：三个平台一次也没有编译、运行过它们；这台 Windows 开发机编译、运行过其余的全部改动——M4b 的生产代码没有平台代码，只在 CI 上编译的是两份量内存的 `memory.rs` 里 Linux 与其余平台的两支：M4b 的 `crates/rurge-engine/src/observe/memory.rs`（挂在 `crates/rurge-engine/src/observe.rs` 的 `#[cfg(test)] mod memory;` 下）的 `peak_resident_bytes` 有 `#[cfg(windows)]`、`#[cfg(target_os = "linux")]` 与 `#[cfg(not(any(windows, target_os = "linux")))]` 三支，后两支本机没有编译过，CI 上也只编译、不运行（调用 `peak_resident_bytes` 的只有带 `#[ignore]` 的那两条用例，CI 的 `cargo test --workspace --no-fail-fast` 不跑它们）；M4b 这一份照抄 M4a 的 `crates/rurge-http/src/capture/memory.rs`（文件里第 56、82、143 行的注释写明抄自那里），那一份也有这三支、后两支同样只由 CI 编译。**M4a 一节那一句（第 1008 行）不对**：`capture/memory.rs` 同样有这三支，Linux 与其余平台的两支只有 CI 编译（历史段落不改，由这一句订正）。
- **照旧是 `null` 的三种 UDP**（P8 ②，设计第 10.3 节）：`vmess`、`h2-connect` 与 MASQUE 的 UDP 按目标在另一个任务里懒开，记录里仍是 `null`；被拒绝的会话与 rurge 不实现的协议照旧。
- **几处没有用例**：三处 `bad_request`（记录第 18 行）；HTTP/2 上 rurge 自己的页答 `HEAD` 时的页长（P5 ③，设计第 18 节第 15 条）；源站提前作答、请求正文还在上传（设计第 11 节第 11 条）；跳端口的 hysteria2 在引擎那一层（记录第 60 行）；`external` 的 UDP 只断言到「回环上、不是目标」（记录第 39 行）。
- **没有真的代理服务端与浏览器**：snell / anytls / hysteria2 / tuic / MASQUE / WireGuard 的对端都是测试自己的假服务端与 `FakeWgPeer`，没有连过真的；没有经真浏览器看过三个新键与 `remoteAddr`。
- **WireGuard 的 `far_end_for` 与换载体不在同一把锁里**：碰上正在换载体的会话可能记下前一个载体的远端，这仍是「记下时的那一跳」，不改（记录第 60 行）。
- **`--request-log-size` 的缺省不改**（M4-D25）：量的数交给所有者；1088 字节一条不是量到的数。
- **M4b 不加日志行、不设诊断码**：九种原因只进备注。
- **没有观察过 Surge 在这些地方的实际行为**：九句备注的文字、`receiveMs` 与两个正文的口径、`remoteAddr` 在新口径下的取值都是照设计与手册定的。
- **没有 Dashboard**：记录只有 API 与抓包看到，界面在阶段 6。

以后的会话必须知道的事（改那一块之前读设计第 10、11 节与本计划的两张表）：

- **直通的原因在建句柄时写**（`crates/rurge-engine/src/engine.rs` 的 `dial_noted` 与 `PassedBy`，记录第 22 行；`serve.rs` 里是九句备注的常量与嗅探那一条路的 `PassedBy::Sniff(..)`）：新的入口要么经 `Dialer::dial`（`PassedBy::Lists`），要么自己给 `PassedBy`；原因在 `match turned` 之后、`quic_adaptive.on_tcp` 之前写，`judged_at_the_entry` 管的是 `PassedBy::Lists` 只判哪些会话（`Transport::Tcp`、监听器是 `Http` / `Socks5` / `Tun`、引擎没有暂停）。
- **`Decision` 的五种，名单先看**（`crates/rurge-http/src/mitm/decrypt.rs`）：`Decrypt`、`NotListed`、`SourceBlocked`、`NoCertificate`、`AlpnMismatch`；`decides` 的次序是名单 → 来源 → 名字 → ALPN，`decrypting` 的是名单 → 开关 → CA → 来源 → 名字 → ALPN。加判定时要同时改这两处的次序与 `decrypting` 的两行 DEBUG（这两行的文字不变）。
- **rurge 自己的答复先 `own_body` 再 `finish`**（`crates/rurge-http/src/engine.rs` 的 `own_body`，记录第 12 行）：新加的自答要在 `finish` 之前调它；新的应答正文类型要在交出每一帧时、`complete()` 之前数——不能靠 M4a 包住应答正文的那一层（那里总漏掉最后一帧、自己的答复总是 0）；两个正文的计数是 `u64`、`fetch_add` 累加，记录在 `finish` 那一刻拍下，之后加的进不了记录。
- **新的复用或共用的载体要记着自己的远端**（记录第 44 ～ 48、52 ～ 55 行）：拨号之后读 `trail::current().and_then(|t| t.peer())` 记在连接上，借出时在会话的 `follow` 里 `trail::connected`；新的 UDP 载体要答 `far_end`（`Datagram` 与 `PacketSocket` 各一个，缺省是 `None`），开不成时清 `trail`（`clear_peer`）。
- **QUIC 的远端是 `Path::far_end`，不是 quinn 的 `remote_address()`**（`crates/rurge-net/src/quic`）：跳端口时 quinn 给的是第一个端口、经承载时是替身 `192.0.2.1:443`。WireGuard 换载体的地方（`Driver::land` 的 `Some` 与载体结束的 `None`）要同时更新 `Shared.far_ends`，用 `Driver::note_far_end`。
- **`Trail::dns()` 含在途的查找**（记录第 30、33 行）：新的查找用 `rurge_net::trail::Lookup::begin()`（开始时登记、被丢下时加上花的时间并注销）；`trail::current()` 只在 `follow` 里有，`tokio::spawn` 出去的任务看不到——在 `follow` 之外要写远端的（snell 的重拨、UDP 流），握一份会话的 `Trail`。
- **`docs/api/phase4.md` 的「M4b」一节照代码写**（记录第 68、69 行）：以后改这些键、备注或 `remoteAddr` 的各载体时照代码改那一节与兼容性清单 10.4 的那一行；量的两条用例带 `#[ignore]`，缺省的 `cargo test` 不跑它们。

去向写着项目所有者的（后来的会话不要自己定）：

- **推送与读回那次 CI**：M1、M2、M3、M4a 与 M4b 的提交都在本地，由所有者一起推送（M1 ～ M4），推送之后先读那次运行的注解与任务摘要；`#[cfg(unix)]` 的两处（M2c 的）第一次在那里编译与运行；两份量内存的 `memory.rs`（M4a 的 `crates/rurge-http/src/capture/memory.rs`、M4b 的 `crates/rurge-engine/src/observe/memory.rs`）里 Linux 与其余平台的两支，也是第一次在那里编译（见上面「M4b 的提交 CI 都没有见过」那一条）。
- **设计第 17 节的两行（M4b 的）**：`remoteAddr` 的新口径（M4-D23：从「会话自己开的那条 TCP 连接」改成「所经的连接或载体的远端」，`docs/api/phase1.md` 的说法随之订正（照计划，`docs/api/phase1.md` 的历史段落不改，订正写在 `docs/api/phase4.md` 的「M4b」一节），照旧 `null` 的几种登记）与 `--request-log-size` 的缺省（M4-D25：1 000 不改，量到的一条记录的大小供所有者决定）。
- **手工验收**：`docs/acceptance/phase4-manual.md`（六十二项），连 M4b 的十项一项也没有做。
- **修正记录与延后事项里去向写着项目所有者的行**：延后事项一行也没有（空表）；修正记录里只有第 62 行（量到的数交给所有者，M4-D25）。第 41 行提到的「所有者让先合并到 `main`」是所有者已经下过的指示，不是留给所有者的事。

依赖附录：M4b 没有加生产代码里 crate 之间的边，也没有改任何 `Cargo.toml`（`Cargo.lock` 仍是 520 个包）；用 `rurge_net::trail` 的是 `rurge-proto`、`rurge-proto-quic`、`rurge-proto-ssh`、`rurge-proto-wireguard`、`rurge-dns`、`rurge-engine` 与 `rurge-inbound`，它们本来就正式依赖 `rurge-net`；`rurge-policy` 不用那个模块、只是给 `Relayed` 实现 `Datagram::far_end`，`rurge-http` 也不 `use` 它、经句柄读会话的 `Trail`（`handle.trail()`），两者也本来就正式依赖 `rurge-net`；`unsafe_code` 的名单仍是 23 项，`crates/rurge/tests/sockets.rs` 的 `LISTED` 仍是 26 行。

M4b 做完，M4 至此做完，下一步是项目所有者推送（M1 ～ M4 一起），推送之后读回那次 CI；再往后是 M5（`auto-quic-block`、基准与遗留）的细化设计。

### M1 ～ M4 推送之后的 CI（2026-11-02）

项目所有者 2026-11-02 推送了 `20efc16`（`41c35d4` 之后的 295 个提交：阶段 3 / M6 收尾的四个文档提交，与阶段 4 的设计和 M1a ～ M4b 的全部）。运行 41402787066（第 16 次运行）**没有全绿**：`bench` 与 Linux、Windows 的 `check` 都成功；macOS 的 `check` 里 `cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 都过，测试一步只失败一条用例——`-p rurge-tun --lib :: tests::dispatch::queued_bytes_are_given_back_when_the_shard_takes_the_packets`（阶段 3 / M4a 写的），在 `crates/rurge-tun/src/tests/support.rs:314:9` panic：`timed out waiting for the shards' queue to take up its bytes`。**本节订正阶段 4 各节（M1a ～ M4b）里“CI 没有见过”“推送与读回 CI”一类的说法**，`docs/testing.md` 阶段 4 各节开头与 `docs/README.md` 各项里的同样的说法也以本节为准（规矩是历史的段落不逐处改、由后写的段落订正）：

- **第一次被编译、运行的**：阶段 4 的代码在三个平台上过了 fmt 与 clippy（工具链 1.89.0）。缺省用例在 Linux 与 Windows 上全部通过，在 macOS 上除上面那一条之外都通过（失败摘要只列出它；测试一步带 `--no-fail-fast`，别的测试二进制都跑完了），所以阶段 4 的用例在三个平台上都通过。M2c 带 `#[cfg(unix)]` 的两处——`crates/rurge/src/cli/mitm.rs` 的 `restrict`（两个私密文件的 `0600`）与 `crates/rurge/tests/cli.rs` 的 `mitm_ca_generate_keeps_the_secrets_private`——第一次被编译，在 Linux 与 macOS 上过了 clippy，那条用例在两个平台上通过；M2c 一节“在那之前，‘Unix 上私密文件是 `0600`’只有读代码与照 `fakeip/file.rs` 写这两条依据”以这一句为准。两份量内存的 `memory.rs`（M4a 的 `crates/rurge-http/src/capture/memory.rs`、M4b 的 `crates/rurge-engine/src/observe/memory.rs`）里，Linux 的一支在 Linux 上、其余平台的一支在 macOS 上第一次被编译、过了 clippy；调用它们的只有带 `#[ignore]` 的两条用例，CI 不跑，所以它们照旧没有运行过。这些由各步的结论与失败摘要推出：任务日志要登录才能读，各平台跑了多少条用例、哪些打印了跳过，没有读到。
- **失败的那一条**（M4b 记录第 75 ～ 80 行）：照源码读出的原因在用例的时序假设，不在产品代码。8 KiB 的预算按容量记，只装得下 128 个 SYN；读设备的任务从不等队列，装不下的当场丢；片一轮至多取 256 个，最晚在第三批交给写者时被挡住（一批在写者手里、挡在闸门上，一批在长度为 1 的写者队列里），而且要先有三轮有段可发。片落在读设备的任务后面时，它可能一两轮就取完队列里剩下的、不被挡住，队列里什么也不剩，用例等的“队列占上字节”就一直不来。这一条从阶段 3 / M4a 起就在：第 13、15 次运行全绿，第 14 次的失败摘要只列 tuic 那一条，所以它在 macOS 上通过过三次；`41c35d4` 之后改过 `rurge-tun` 的两个提交（`b9d1cd7`、`3667af5`）不碰读设备的任务、片的一轮、写者与预算。M4b 插了任务 10 修（`76f7017`；修正轮 1 的 `1261c71` 只把一句注释说准）：只改用例，预算装得下 `3 * INGRESS_PER_WAKE + 128` 个 SYN，在任何先后之下片被挡住时都还有 SYN 在等；另加一条同样的用例，跑在当前线程的运行时上，让读设备的任务跑在片前面——它在旧的预算上三次都红在与 CI 相同的那一行。不改生产代码、不改 CI、不加 `#[ignore]`、不重试、不放宽时限。`76f7017` 上的门禁（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI）：`cargo fmt --all --check`、`cargo clippy --all-targets -- -D warnings` 与 `cargo test -j 8 --workspace --no-fail-fast` 的退出码都是 0，93 个结果行、5377 通过、0 失败、17 忽略（`f5723fd` 上是 93 / 5376 / 0 / 17，多的是新加的那一条）；两条用例各直接跑测试二进制 25 次、每次套 60 秒时限，全过。`1261c71` 只改注释，跑了 fmt、clippy 与这两条用例各一次。所以修正在 macOS 的 CI 上管不管用，**要看它们推送之后的那一次运行**；`20efc16` 之后的提交 CI 都还没有见过。
- **没有变的**：阶段 4 的手工验收一项也没有做（`docs/acceptance/phase4-manual.md` 六十二项），没有把 CA 装进任何信任库、没有经真浏览器试过，全部证据在回环上；阶段 4 的五条验收标准一条也没有验过；M5 与 M6 还没有做，M3c 等总设计 Q4。阶段 3 的照旧：三个平台的设备与系统改动没有真的运行过、没有在真机上验证，L2 的四条没有运行过，三份手工清单一项也没有做。
- **注解**只有平台的，与第 13 ～ 15 次相同：四个任务各一条 Node.js 20 弃用的警告，`bench` 与 `check (ubuntu-latest)` 各一条迁到 Ubuntu 26 的通知，`check (macos-latest)` 一条 arm64 排队的通知；照 R26、R27 不改 CI。
- **各任务**：`bench` 的两步都成功。测试一步的时长：Linux 9 分 41 秒、macOS 11 分 47 秒、Windows 14 分 44 秒（工作流给这一步 60 分钟）。`Report the failed tests` 在 macOS 上跑了（它只在测试一步失败时跑），在 Linux 与 Windows 上跳过。安装参照服务端的各步与第 14、15 次相同：Linux 都成功；macOS 跳过 snell-server、tuic 0.8.5、masque-server 三步；Windows 跳过五步，即这三步，加上 TrustTunnel 的安装与指向 `sshd` 的那一步。
- **文档怎样订正的**（M4b 记录第 81 行）：`CLAUDE.md`「现在在哪里」三条；两份 README 的状态、特性表与路线图里阶段 4 的四处；需求文档第 7 节阶段 4 的状态，按日期接一句（M4b 收尾时没有写这一行，M4b 的那一句也在里面）；兼容性清单开头，加新的「最近一次」，改阶段 4 的状态；`docs/api/phase4.md` 末尾加「M1 ～ M4 推送之后的 CI」一节；`docs/testing.md` 的「阶段 3 / M3a 起：`rurge-tun`」加一句，说要片或写者被挡住的用例不能靠各任务的先后。
- **下一步**：项目所有者推送 `f0d1d35` 及其后的提交（第 75 行、任务 10 的修正、本节），推送之后读回那一次运行。M5（`auto-quic-block`、基准与遗留）的细化设计不等推送。

### 第 17 次 CI 与 M4b 任务 11（2026-11-02）

项目所有者 2026-11-02 推送了 `f0d1d35` ～ `a262d4c`（上一节的第 75 行、任务 10 的修正与读回第 16 次的文档）。运行 41411445901（第 17 次运行）**没有全绿**（M4b 记录第 82 行）：`bench` 与 macOS 的 `check` 都成功——上一节那一条 `rurge-tun` 用例这一次在 macOS 上通过，任务 10 的修正第一次在 CI 上运行；Linux 与 Windows 的 `check` 里 fmt 与 clippy 都过，测试一步各有失败，注解列出三条，都在测试一侧，都不是阶段 4 的用例，所以阶段 4 的用例照旧在三个平台上都通过（由各步的结论与失败摘要推出，任务日志要登录才能读）。**本节订正上一节“修正在 macOS 的 CI 上管不管用，要看它们推送之后的那一次运行”一句**：

- **Windows 两条**：`crates/rurge-engine/tests/egress.rs` 的 `every_kind_of_dns_upstream_asks_the_hook_before_it_dials` 与 `an_egress_error_stops_each_kind_before_anything_is_sent`（阶段 3 / M4b 任务 9 写的）。两处都先在 TCP 上 bind `127.0.0.1:0`、再在同一个号上 bind UDP 并 `unwrap`，这一步得到 `WSAEACCES`（10013）：系统给 TCP 的临时端口，同一个号在 UDP 上不保证能用；这一次是哪一种原因（别的账户的套接字占着通配地址上的这个号，或号落在系统保留的号段里）没有证据。两条在第 13 ～ 16 次都通过，是偶发。
- **Linux 一条**：`-p rurge-interop --test snell :: obfs_http_against_sing_box`（阶段 2 / M6b 写的），客户端自己连 sing-box 时得到 `ConnectionRefused`：那一刻 sing-box 的号上没有人在听，而 `Reference::wait_ready` 之前连上过这个号。是 sing-box 就绪之后自己退出了、还是就绪的探测连上的是别人，照注解分不出：用例失败时什么也没有打印。这一条在第 13 ～ 16 次都通过。
- **任务 11**（M4b 插入的，记录第 82 ～ 86 行）只改测试的代码。甲（`707decf`）把 `MockDns::start` 里找号的循环（至多 64 轮、两种次序轮流，2026-10-18 为同一件事写的）原样抽成 `rurge_dns::testing::tcp_and_udp_on_one_port()`，`egress.rs` 的那两处与 `crates/rurge-dns/src/upstream/udp.rs` 里同样写法的两条单元测试都改用它。乙（`a9ef85b`）让 `tests/interop/src/lib.rs` 的 `Reference` 在用例失败时由 `Drop` 说出参照服务端是不是自己退出了、日志的最后至多二十行（两千个字符以内，连同 panic 的那一句放得进一条注解）：Linux 那一条的原因留给它下一次失败时的注解。两处都没有能在本机变红的用例；乙在 `Drop` 里新加的几行本机运行不到（这台机器上没有参照服务端），只编译过，`post_mortem` 由三条新用例钉住。门禁（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI）：fmt 与 clippy 的退出码都是 0；`cargo test -j 8 --workspace --no-fail-fast` 里 `outbounds_http2` 与 `rewrite` 两个测试二进制以 `0xc0000374`（`STATUS_HEAP_CORRUPTION`）退出，单独重跑全过，合起来 93 个结果行、5380 通过、0 失败、17 忽略（`76f7017` 上是 93 / 5377 / 0 / 17，多的是乙的三条）；甲改过的四条用例各直接跑测试二进制 25 次、每次套 60 秒时限，全过。不改生产代码、不改 CI、不加 `#[ignore]`、不重试、不放宽时限。
- **文档**：`docs/testing.md` 末尾加「第 17 次 CI 运行起：同一个号上的 TCP 与 UDP、互操作用例失败时的说明」一节，`CLAUDE.md` 的测试约定加一句（同一个号上的 TCP 与 UDP）；`CLAUDE.md`「现在在哪里」的阶段 4 一条的末尾与 CI 一条；两份 README 与需求文档第 7 节阶段 4 的状态里指到上一节的那一处，改成也指到本节、接一句这一次运行。兼容性清单与 `docs/api/phase4.md` 没有改：这一次运行没有改变它们说的事。
- **没有变的**：上一节「没有变的」一条照旧。
- **下一步**：项目所有者推送 `f9994db` 及其后的提交（任务 11 与本节），推送之后读回那一次运行；甲在 Windows 上管不管用、乙说不说得出原因，要看那之后的运行。M5（`auto-quic-block`、基准与遗留）的细化设计不等推送。

### M5a（读 QUIC 里的 ClientHello 与 `auto-quic-block`）

M5a 做完了（2026-11-02，最后一个文档提交在 2026-11-03 凌晨；计划 `docs/superpowers/plans/2026-11-02-phase4-m5a-quic-hello-auto-quic-block-plan.md`，下面写“M5a 记录第 N 行”指它末尾的「执行期修正记录」，它的「延后事项」是空表；设计 `docs/superpowers/specs/2026-11-02-phase4-m5-auto-quic-block-bench-design.md`，执行期改设计正文的地方见它的第 16 节，只有一处——第 4 条）。照项目所有者 2026-10-26 的指示，同样由协调的会话逐个任务调 Pi 做、逐个任务评审。**M5 至此还剩 M5b（基准与遗留）**。**M5a 的提交都在本地，CI 没有见过**（M1 ～ M4 的推送与两次运行见上面两节）。最后一棵代码树（`52e3590`）上的全量是 93 / 5449 / 0 / 17（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI）。

做了什么：

- **读 QUIC 里的 ClientHello**（`rurge-net` 的 `quic/initial.rs` 与新的 `quic/testing.rs`，M5-D2 ～ D6）：`read_first(数据报)` 先用今天那个便宜判据过一遍，再按这个数据报的目的连接 ID 与版本导出服务端一侧的 Initial 密钥打开第一个包，读它的帧（PADDING、PING、ACK、CRYPTO、CONNECTION_CLOSE），然后从这个包的末尾起走完数据报的其余部分——长包头位不在、版本不同、Retry 就停，0-RTT 与 Handshake 按 Length 跳过，打不开的 Initial 也按 Length 跳过，这些“停”都不算读不成；`HelloReader::read(数据报)` 从开头接着走。CRYPTO 的段按偏移排，重叠处逐字节比（不同就读不成）、相接的合并，`MAX_HELLO` 64 KiB 与 32 段两个上限在加之前就挡住；偏移 0 起的四个字节到了就看握手消息的头，`[0, 4 + 长度)` 都到了答 `Whole`，读不成答 `GaveUp`（九种原因之一），其余答 `Partial`（同一个数据报里读不成优先于拼齐）。`is_client_initial` 改成 `read_first(..).is_some()`，`initial.rs` 的十一条旧用例逐字未改、原样通过——分类没有变（M5-D3）。`quic::testing` 加了造 Initial 包、数据报与 rustls 写出的真 ClientHello 的构造器（含 Chrome 单包打散与跨两个数据报的两种排法，随机处用固定种子的 SplitMix64），两份 RFC 样例连同 `unhex` 挪过去。
- **持住、SNI、计数与备注**（`rurge-engine` 的 `udp.rs`，M5-D7 ～ D10）：`run_flow` 在 `turn_back` 与 `destination` 之后先 `read_first(第一个数据报)`，答 `Some(Partial)` 就持住——从这条流的队列接着收数据报，`HELLO_WAIT`（300 毫秒，从第一个数据报起）或 `HELLO_DATAGRAMS`（16 个）或拼齐或读不成，任一条到了就放手，关联在这期间结束则不留记录、不计数。读成的 SNI 在 `new_handle_with` 之前写进会话，带 `extended-matching` 的域名规则从此对 QUIC 流按 SNI 命中（`crates/rurge-rules/src/matcher.rs` 的 `domain_targets` 本来就按会话的 SNI，不改；记录第 15 行）。`forward` 的 `first` 换成 `VecDeque<Queued>`，先按原来的次序发完持着的再收队列；`open` 拒绝时对每个持着的数据报各告诉客户端一次。计数在 `serve.rs` 的 `QuicCounts`（`hello`、`incomplete`、`unreadable`、`blocked`，`Relaxed`，重载不清），两种备注与两行 DEBUG 照「表后的原文」，一条 QUIC 流至多一句；读不成的原因只进 DEBUG，不进备注、记录或 API。
- **`auto-quic-block`**（`serve.rs`、`udp.rs`，M5-D11 ～ D14）：`sniffable_session` 里入口、名字、`always-raw-tcp-hosts` 与 `always-raw-tcp-keywords` 两张名单、端口那几步抽成 `sniffable_target`，与新的 `auto_quic_blocks` 共用同一段代码，后者再调 `decrypting`（ALPN 给空）；`open` 在 `REJECT` 那一处之后、`block-quic` 之前判——读到了 ClientHello、开关为真、回落之后的 TCP 会被嗅探并解密时 `set_error("QUIC blocked by auto-quic-block")`、计数 `blocked`、`Rejected(RejectKind::Reject)`；规则与策略照旧记在记录上，不碰自适应的表，`always-allow` 不放过它。没有 SNI 的 ClientHello 按目标比、不带端口的项只对 443（M5-D14）；凑不齐与读不成都不阻断。
- **API**（`rurge-api`，M5-D9）：`/v1/rurge/http` 在 `rewrite` 之后多 `quic`，四个 `u64`（`hello`、`incomplete`、`unreadable`、`blocked`），`HttpCounts` 与引擎里多一份 `QuicCounts`。
- **文档**（任务 6a、6b）：`docs/api/phase4.md` 末尾的「M5a　QUIC 里的 SNI 与 `auto-quic-block`」一节、手工清单的 M5 一节（W13、W14、M13、M14、L13、L14）与开头的两句、`docs/testing.md` 的阶段 4 / M5a 一节、兼容性清单的四行（`block-quic`、`PROTOCOL`、`extended-matching`、`auto-quic-block`）与开头「最近一次」的括号；本节、两份 README、需求文档第 7 节与 `CLAUDE.md`「现在在哪里」。

各任务（括号里是全量的结果行 / 通过 / 失败 / 忽略，都是一台 Windows 11 x86-64 开发机上的一次运行，不是 CI；需要参照服务端的互操作用例在本机立刻返回、算在通过的数里）：

- **任务 0**（`af6fd9a`，协调者 `62ce590`）：开工前核对（工作树与基线、名字与签名、rustls 与 tokio 的几处事实、P14 表里的现有用例、四个数），只改文档，0 条用例（93 / 5380，本计划的基线；记录第 1 ～ 8 行）。
- **任务 1**（`b2bcb4a`、`66aea6b`，协调者 `9a50a3c`）：`quic::testing` 的九项构造器与末尾的测试模块，两份 RFC 样例挪过来；+9 条（93 / 5389；记录第 10、11 行）。
- **任务 2**（`748ed1b`、`7836738`，修正轮 1 `6423a3a`，协调者 `19ed701`）：`read_first`、`HelloReader` 与三段式的答复；+30 条（93 / 5419），修正轮 1 把段的缓冲换成 `VecDeque<u8>`、看头等四个字节、整段比的断言只打印长度，再 +2 条（93 / 5421；记录第 12 ～ 14 行）。
- **任务 3**（`02a0332`，修正轮 1 `7de770d`，协调者 `fa36ea1`）：持住、SNI、计数与备注；+12 条（93 / 5433；记录第 15 ～ 18 行）。
- **任务 4**（`2f623f7`、`a17dd22`，协调者 `bc74f81`）：`sniffable_target` 与 `auto-quic-block`；+15 条（93 / 5448；记录第 19 ～ 21 行）。
- **任务 5**（`6c4bded`，修正轮 1 `52e3590` 只改注释，协调者 `c705e82`）：`/v1/rurge/http` 的 `quic`；+1 条（93 / 5449；记录第 22 ～ 24 行）。
- **任务 6a**（`df2ee18`、修正轮 1 `a8b1024`，协调者 `7f71d53`）：四份行为的文档，只改文档，0 条用例（记录第 25 ～ 27 行）。
- **任务 6b**（本节的提交）：进度的文档，只改文档，0 条用例。

门禁：任务 0 ～ 5 每个任务都跑了全量（`cargo fmt --all --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test -j 8 --workspace --no-fail-fast`），有的不止一次（任务 2、3 各两次，任务 5 两次；记录第 8、10、12 ～ 15、17、20、22、23 行），一共九次，退出码都是 0；任务 6a 与 6b 只改文档、没有跑。九次里有四次撞上开发机已知的 `STATUS_HEAP_CORRUPTION`（任务 0 的 `rurge-http` 单元测试二进制；任务 1 的 `outbounds_http2` 与 `smart`；任务 5 的两次分别是 `outbounds_ssh` 与 `rewrite`、`resolver` 与 `rurge_inbound` 的单元测试二进制；记录第 8、10、22、23 行），每次单独重跑那些二进制、全过，重跑的是与全量里崩溃的同一份，哈希对过。带等待或并发的用例各自单独循环跑过：任务 2 的 quinn 那一条 22 次、任务 3 的 `--test udp` 45 条与 6 条暂停时钟的单元用例各 20 次（记录第 12、15 行）、任务 4 的 `mitm` 与 `tun` 两个测试二进制各 20 次（记录第 20 行）；协调者另跑过任务 3 的六条端到端并行 100 次、每条单独 50 次与 `--test-threads=1` 60 次（记录第 18 行）、任务 4 的十四条并行 40 次与四条各 30 次（记录第 21 行），全过。日志在工作区（git 忽略）。`cli` 的 `give_up` 超时、`LNK1104` 与 rustc 的内部编译器错误这一次都没有见到。

没做的，与不要当成已经有的：

- **没有手工验收**：`docs/acceptance/phase4-manual.md` 现在六十八项，M5 的六项（W13、W14、M13、M14、L13、L14）一项也没有做；M5a 的全部证据是回环上的用例与用例自己造的 Initial 包。
- **M5a 的提交 CI 都没有见过**：三个平台一次也没有编译、运行过它们；这台 Windows 开发机编译、运行过全部改动——M5a 没有平台代码，也没有只在 CI 上编译的代码。
- **没有经真浏览器、真站点，也没有完成过一次 QUIC 握手**：QUIC 的用例只喂数据报（总设计第 14 节第 5 条、设计第 10 节），quinn 的真客户端只在一条用例里连一个只收不答的回环套接字；真浏览器的 HTTP/3 长什么样、被阻断之后多久回落、回落的那条 TCP 会话是不是同一个站点、浏览器的信任库怎么看待 M2 那份 CA，都没有看过。
- **没有观察过 Surge 在 QUIC 上的实际行为**：说明、两种备注、九种读不成的原因与阻断的三个条件都是照设计与手册定的。设计第 4.7 节不做的四件照旧不做：从不完整的 ClientHello 里取 SNI、gQUIC、Retry 之后的第二把密钥、ECH。
- **没有 Dashboard**：`quic` 四个数只有 `GET /v1/rurge/http` 看得到，界面在阶段 6。
- **与手册不同的两处已登记**：不带端口的项只在 TCP 一侧本来就嗅探的端口上阻断、`always-raw-tcp-hosts` 的主机不阻断（设计第 6.1 节、M5-D11），`block-quic = always-allow` 不放过它（第 6.3 节、M5-D13）；兼容性清单的 `auto-quic-block` 一行状态留 🟡（记录第 25 行）。
- **本机的一个环境问题**：任务 3 的六条端到端一起并行循环 100 次里有 4 次以 `STATUS_HEAP_CORRUPTION`（0xC0000374）退出、没有失败的用例，与 `CLAUDE.md`「这台开发机」第一条同一个征状（注入的网络钩子），六条各单独 50 次（共 300 次）与 `--test-threads=1` 60 次都不出现，八条旧的 QUIC 与拒绝用例并行 60 次也不出现，记作环境、不为它改代码（记录第 18 行）；CI 的 Windows 上若也出现，再另查。

以后的会话必须知道的事（改那一块之前读设计第 4 ～ 7 节与 M5a 计划的两张表）：

- **持住的两个常数与不设旋钮**（`crates/rurge-engine/src/udp.rs` 的 `HELLO_WAIT` 与 `HELLO_DATAGRAMS`，记录第 15、17 行）：300 毫秒从第一个数据报起、用 `udp.rs` 的 `now()`（tokio 的钟）加 `sleep_until`；16 个含第一个，第 17 个起照旧排在队列里；两个边界只在暂停时钟的单元用例里量，端到端只在“只发第一个数据报”的几例里等它过去（有界）。
- **`quic_initial()` 现在是完整的 ClientHello**（`crates/rurge-engine/tests/common/mod.rs`，记录第 15、16 行）：`hello_initial(RFC_DCID, &client_hello(None, &[b"h3"]))`，不带 SNI、一个数据报装得下，所以用它的八条 `tests/udp.rs` 用例与 `tests/tun.rs` 的那一条不持、不写备注；`rurge-net` 的 `client_initial` 照旧只有 PADDING，给分类的用例用。
- **`sniffable_target` 两处共用**（`crates/rurge-engine/src/serve.rs`，P9、M5-D11）：`sniffable_session`（TCP）与 `auto_quic_blocks` 问同一段代码，加条件只改这一处，不要在两处各写一份；`sniffable_session` 另看传输与暂停，`auto_quic_blocks` 不看暂停与预匹配。
- **阻断的位置**（`udp.rs` 的 `open`，P10、M5-D12）：在 `REJECT` 那一处之后、`block-quic` 之前；被它阻断的流不调 `on_quic`、不碰自适应的表，规则与策略照旧记在记录上。
- **备注、计数与 DEBUG 的口径**（设计第 5.4 节）：`hello`／`incomplete`／`unreadable` 三选一，`blocked` 是 `hello` 里的一个子集；读成的不写备注（M5-D10）；两行 DEBUG 的名字与端口只进 DEBUG；读不成的原因只进 DEBUG。
- **放行的 QUIC 流的说明不断言**（P12）：任务 4 的数据报发往钉到 `127.0.0.1` 的名字的 443 或 8443，那上面没有人收，Windows 上记录可能写上一个“连接被重置”一类的说明；任务 3 的用例发往 `udp_sink()` 自己的端口。
- **`Pieces` 的段缓冲是 `VecDeque<u8>`，按倍数增长**（记录第 13、14 行）：一段的容量至多是它的字节数的两倍，设计第 4.5 节末段已按这个订正。拼接的开销用 `adding_costs_what_arrives` 钉住。
- **`udp.rs` 里的 `Opening` 是原来的 `struct Held`**（记录第 15 行）：M5a 的 `enum Held` 占了这个名字，改名的只是类型名与一行注释。
- **QUIC 的用例只喂数据报**（`docs/testing.md` 的阶段 4 / M5a 一节）：不完成任何 QUIC 握手；quinn 只在 `crates/rurge-net/src/quic/initial.rs` 的一条用例里当客户端、只连只收不答的回环套接字。

去向写着项目所有者的（后来的会话不要自己定）：

- **设计第 15 节的第 1 ～ 5 条**：gQUIC 要不要认（多了一条 Surge iOS 5.14.6 的事实）；从不完整的 ClientHello 里取 SNI 要不要做；`block-quic = always-allow` 不放过 `auto-quic-block`（M5-D13，代所有者作的裁定）；没有 SNI 的 ClientHello 按目标（M5-D14，代所有者作的裁定）；`auto-quic-block` 跟着 TCP 一侧（M5-D11，代所有者作的裁定）——要不要让 TCP 与 QUIC 两侧都照手册（不带端口的项在任何端口上解密）。第 6 条（NFR-01 的线怎样用在经引擎的路径上）与第 7 条照旧。
- **推送与读回 CI**：M1 ～ M4 已推送（`20efc16`）并读过两次运行；M5a 的提交都在本地，由所有者推送（`main` 上的 `802a634` 领先 `origin/main`（`a262d4c`）5 个提交，那 5 个也在等推送），推送之后先读那次运行的注解与任务摘要。
- **手工验收**：`docs/acceptance/phase4-manual.md`（六十八项），M5 的六项一项也没有做；需要真浏览器、真站点与真信任库。
- **修正记录里代项目所有者作的裁定**：第 9、11、14、25、26、27 行（写“协调者的裁定”的那几行），所有者可以改；它们不是等所有者回答的事。延后事项一行也没有（空表）。

依赖附录：M5a 没有改任何 `Cargo.toml`，也没有加生产代码里 crate 之间的边（`git diff --stat 2ca12b2 HEAD -- '*.toml' Cargo.lock` 是空的，提交之前 2026-11-03 又跑过一遍；`Cargo.lock` 仍是 520 个包）；`quic::testing` 用的 rustls 与 quinn 本来就是 `rurge-net` 的正式依赖，引擎与 API 的用例用它是靠它们开发依赖里本来开着的 `rurge-net` 的 `testing` 特性（`crates/rurge-engine/Cargo.toml` 第 35 行、`crates/rurge-api/Cargo.toml` 第 28 行）；`unsafe_code` 的名单仍是 23 项，`crates/rurge/tests/sockets.rs` 的 `LISTED` 仍是 26 行（M5a 的生产代码与 `quic::testing` 的构造器都不建套接字）。

M5a 做完，下一步是 M5b（基准与遗留）的计划——不等推送；M1 ～ M4 与 M5a 的推送归所有者。

### M5b（基准与遗留）

M5b 做完了（2026-11-03；计划 `docs/superpowers/plans/2026-11-03-phase4-m5b-bench-legacy-plan.md`，下面写“M5b 记录第 N 行”指它末尾的「执行期修正记录」，它的「延后事项」现在三行；设计 `docs/superpowers/specs/2026-11-02-phase4-m5-auto-quic-block-bench-design.md`，执行期改设计正文的地方见它的第 16 节「执行 M5b 时」一段，没有改正文意思的地方）。照项目所有者 2026-10-26 的指示，同样由协调的会话逐个任务调 Pi 做、逐个任务评审。**M5 至此做完，阶段 4 的六个里程碑还剩 M6（收尾与验收）。** **M5b 的提交都在本地，CI 没有见过**（M5a 的提交也在本地；M1 ～ M4 的推送与两次运行见上面两节）。最后一棵代码树上的全量是 94 / 5453 / 0 / 33（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI；任务 5、6 只改文档，没有跑门禁）。

做了什么：

- **遗留的三条用例**（任务 1，M1b 计划延后第 4 行）：`crates/rurge-http/src/engine.rs` 测试模块末尾的两条暂停时钟的单元用例 `idle_starts_its_clock_at_the_first_poll` 与 `restart_gives_the_resent_request_its_whole_wait`（`Never` 是不出帧的正文；前者钉住 `Idle` 第一次 `poll_frame` 才起表，后者钉住 `Progress::restart` 把表重置），`crates/rurge-engine/tests/http.rs` 的一条端到端 `a_request_sent_again_waits_its_whole_idle_on_the_new_connection`（`IDLE` 2,500 毫秒、`HOLD`/`DELAY` 1,500 毫秒；借来的连接被断之后重发的请求，在新连接上有整个时限）；`crates/rurge-http/Cargo.toml` 的开发依赖给 tokio 开 `test-util`。三处变异各去掉被测的那一处之后对应用例红（记录第 11 行）。
- **基准文件**（任务 2 ～ 4，设计第 8 节）：新建 `crates/rurge-engine/tests/bench.rs`（`mod common;`），里面有夹具（`Scale`、`latency`、`median`、`gbit_per_s`、逐帧发正文的 `Repeat`、hyper 写的源站 `origin`、连接级的客户端 `h1_client` / `h2_client`、`thousand_rules`）与十六个忽略的用例——B1（`b1_plain_throughput`、`b1_plain_latency`）、B2（`b2_decrypted_throughput`、`b2_decrypted_latency`）、B3（`b3_a_thousand_rules_latency`）、B4（`b4_leaf_certificates`）、B5（空闲连接五个、正文三个、叶证书一个）、Q1（`q1_reading_a_quic_client_hello`）——另有一条不忽略、守护读数解析的用例 `the_readings_are_parsed_from_what_the_system_prints`。基准各自断言走的路（计数增量、源站按版本数的请求、`/sanity` 的记录、`handshake_kind`、叶证书的 DER），路错了就 panic，不打印一个错的数；忽略的理由、跑法与内存读法的原文在计划的「表后的原文」，跑法也在 `bench.rs` 开头的模块注释里。
- **release 下的一次读数**（任务 5）：十六个基准与 `relay_throughput` 各一个进程、十七次运行都退出码 0；一台 Windows 11 x86-64 开发机上的一次运行，不是 CI（记录第 19 ～ 26 行）。
- **文档**（任务 6）：本节、`docs/testing.md` 的命令与「测试约定（全文）」、设计第 16 节的「执行 M5b 时」、`docs/requirements.md` 第 7 节的阶段 4 状态行、两份 README、`CLAUDE.md`「现在在哪里」的阶段 4 一条。

各任务（括号里是全量的结果行 / 通过 / 失败 / 忽略，都是一台 Windows 11 x86-64 开发机上的一次运行，不是 CI；需要参照服务端的互操作用例在本机立刻返回、算在通过的数里）：

- **任务 0**（`b8073ea`，协调者 `c7a48aa`）：开工前核对，只改文档，0 条用例（93 / 5449 / 0 / 17，本计划的基线；记录第 1 ～ 10 行）。
- **任务 1**（`f4fecda`、`f2955f8`，协调者 `b3d98d4`）：遗留的三条用例与开发依赖的一行，+3 条（93 / 5452 / 0 / 17；记录第 11、12 行）。
- **任务 2**（`dba1bc5`、`b2a88d5`，协调者 `eb5533e`）：夹具、B1 与 B3，三个忽略的用例，`bench` 是新的测试二进制（94 / 5452 / 0 / 20；记录第 13、14 行）。
- **任务 3**（`0c9f5d5`，协调者 `76f30f7`）：B2 与 B4，三个忽略的（94 / 5452 / 0 / 23；记录第 15、16 行）。
- **任务 4**（`b43278b`，协调者 `c5ad079`）：B5 与 Q1，一条守护用例与十个忽略的（94 / 5453 / 0 / 33；记录第 17、18 行）。
- **任务 5**（`331ff4a`，协调者 `7dc4dff`）：release 下的读数，只改计划（记录第 19 ～ 26 行）。
- **任务 6**（本节的提交）：收尾文档，只改文档。

门禁：任务 0 ～ 4 各跑了一次全量（`cargo fmt --all --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test -j 8 --workspace --no-fail-fast`）；任务 5、6 只改文档，没有跑。四次里各撞上开发机已知的 `STATUS_HEAP_CORRUPTION`（0xc0000374）：任务 0 的 `outbounds_shadow_tls` 与 `outbounds_snell`，任务 2 的 `outbounds_masque`，任务 4 的 `http`（跑到第 27 条）；任务 3 没有撞上。每次单独重跑崩溃的那一个二进制，都全过、重跑的是与全量里崩溃的同一份。一次是 `STATUS_ACCESS_VIOLATION`（0xc0000005）：任务 1 的 `http` 二进制（37 条里跑了 23 条）退出，单独重跑该二进制 37 通过，协调者把它循环 40 次——39 次过、1 次 heap corruption、没有再见访问违规，原因没有查明（这台机器上 `http` 二进制以访问违规退出这是第二次，第一次是 M1b 任务 8；记录第 12 行，同 `CLAUDE.md`「这台开发机」）。`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 每一步的退出码都是 0；`bench` 的结果行是 `0 passed; 0 failed; 3 ignored`（任务 2）、`0 passed; 0 failed; 6 ignored`（任务 3）、`1 passed; 0 failed; 16 ignored`（任务 4）。任务 0 的两次重跑是 `outbounds_shadow_tls`（6 通过）与 `outbounds_snell`（20 通过），任务 2 的是 `outbounds_masque`（13 通过），任务 4 的是 `http`（37 通过）。

读数（一台 Windows 11 x86-64 开发机上 release 的一次运行，不是 CI；处理器 `13th Gen Intel(R) Core(TM) i9-13900HX`，24 核、32 逻辑处理器；`rustc 1.89.0 (29483883e 2025-08-04)`、`cargo 1.89.0 (c24e10642 2025-06-23)`；处理器的名字与核数由任务 5 用 PowerShell 的 `Get-CimInstance Win32_Processor` 读一次，只读；十六个基准与 `relay_throughput` 各一个进程、十七次运行都退出码 0，顺序照记录第 19 行，日志在工作区 `release-task-5/`）。读的是数字，不写成“通过”“达标”“不通过”：

- **B1 与 B3**（`b1_plain_throughput`、`b1_plain_latency`、`b3_a_thousand_rules_latency`）：B1 经 rurge 的下行 0.79 Gbit/s、上行 0.92，直连下行 1.38、上行 1.37（直连是经 rurge 的 1.76 / 1.50 倍）；经 rurge 的每请求中位数 192.4 µs、p99 539.8 µs，直连中位数 91.9 µs，引擎多出的中位数 100.5 µs、p99 170.4 µs。对照 NFR-01 的“本地回环 TCP 直连转发单核 ≥ 1 Gbps”：B1 经 rurge 的下行 0.79 Gbit/s 是那条 1 Gbit/s 的 0.79 倍，直连 1.38 是 1.38 倍。B3 的 `plain`（引擎里一个只带 `FINAL` 的引擎）中位数 208.1 µs、`ruled`（一千条不命中的规则）336.8 µs，一千条不命中的规则在中位数上多 128.7 µs、在 p99 上多 79.3 µs（两者都是两个分布的分位数之差，不是逐个请求之差，p99 的差小于中位数的差并不矛盾）。NFR-01 那条“10 万条规则集下单次规则匹配 p99 < 50 µs”量的是 `rurge-rules` 的单次匹配；这里是一个请求在引擎里实际付的（设计第 8.4 节），不写成对那条线的比。
- **B2**（`b2_decrypted_throughput`、`b2_decrypted_latency`）：解密之后 HTTP/1.1 经 rurge 的下行 1.57 Gbit/s、上行 1.60，直连下行 3.57、上行 4.02（直连 2.28 / 2.51 倍）；HTTP/2 经 rurge 的下行 1.17、上行 1.12，直连 2.60 / 2.63（2.22 / 2.35 倍），两行都在 NFR-01 那条 1 Gbit/s 之上（h1 下行是它的 1.57 倍、h2 是 1.17 倍）。每请求中位数：h1 经 rurge 160.7 µs、直连 70.1，解密多 90.6 µs（p99 多 264.8）；h2 经 rurge 200.0 µs、直连 79.5，多 120.5 µs（p99 多 311.2）。
- **B4**（`b4_leaf_certificates`）：签一张叶证书的中位数 248.6 µs（第一遍中位数 1341.9 µs、第二遍 1093.3 µs，两遍之差含阻塞线程上的签发加两次交接，设计第 16 节第 6 条）；两遍的 p99 与最大都在 25 ～ 27 ms，比中位数（1.1 ～ 1.3 ms）高一个量级。
- **B5 的空闲连接**（`b5_idle_connections_plain_direct`、`b5_idle_connections_plain`、`b5_idle_connections_tls_direct`、`b5_idle_connections_relayed_tls`、`b5_idle_connections_decrypted`）：每条空闲连接的工作集（峰值与当前相同，差不到 4 字节）——直连明文 45.9 KiB、经 rurge 明文 85.8、直连 TLS 63.2、不解密的 TLS 隧道 93.1、解密的 138.6。经 rurge 减直连：明文多 40,837 字节（39.9 KiB）、不解密的 TLS 多 30,691 字节（30.0 KiB）、解密的多 77,271 字节（75.5 KiB）。对照 NFR-01 的“1 万并发连接常驻内存 < 300 MB”（折合每条 30 KB、30,000 字节）：三项分别是它的 1.36、1.02、2.58 倍，照这样的每条算，一万条约 408、307、773 MB。差里含 rurge 接入与上游两侧（HTTP 引擎的上游连接池属于一条客户端连接，`crates/rurge-http/src/pool.rs` 的模块注释说 “A pool belongs to one client connection”，记录第 18 行），也含每条连接的一条请求记录（上限 1,000 条，M4b 量过一条约 1.06 KiB）。NFR-01 那条线原来量的是虚拟网卡的协议栈（设计第 8.6 节），它管不管经引擎的连接归所有者（设计第 15 节第 6 条）。
- **B5 的正文与叶证书**（`b5_body_direct`、`b5_body_streamed`、`b5_body_rewritten`、`b5_leaf_cache`）：正文是 32 MiB 减 1 字节。经 rurge 而不改写比直连多 0.05 份（峰值）与 0.04 份（当前），即不缓冲；改写的比不改写的多 3.94 份（约 126 MiB），之后当前的工作集仍多 1.04 份（约 33 MiB）。叶证书每张 1,970 字节（约 1.9 KiB），峰值与当前一样。
- **Q1**（`q1_reading_a_quic_client_hello`）：读一个跨两个数据报的 ClientHello，中位数 7.0 µs、p99 10.0 µs（最大 563.1 µs）；对照阶段 3 / M5 设计第 4.2 节估的“十微秒”量级——中位数落在它里面。
- **`relay_throughput`**（`RELAY_BENCH_MIB=64`，每轮每向 64 MiB，不是缺省的 256 MiB；用时 197.21 秒）：`pump` 经 TCP 在 `BUF`（8 KiB，`crates/rurge-engine/src/relay.rs:30`）上的中位数 112.6 MiB/s（约 0.94 Gbit/s）；B1 经 rurge 的下行 0.79 Gbit/s 是它的 0.84 倍、上行 0.92 是 0.97 倍（B1 的引擎用 hyper、不经 `pump`，这一比只作量级，设计第 8.2 节）。B1 的明文一路比 B2 的 TLS 一路慢，原因只查明一部分（延后第 1 行）。

没做的，与不要当成已经有的：

- **基准不是门**：十六个都是忽略的用例，CI 的 `cargo test` 与门禁编译它们、不跑；读数是一台 Windows 11 x86-64 开发机上 release 的一次运行，没有在 Linux、macOS 上跑过；`HTTP_BENCH_QUICK=1` 的 quick 数不当读数，只看跑不跑得通。
- **macOS 没有内存的读数**（`resident()` 那一支是 `None`）；Linux 的读法只在守护用例里对过写好的文本，真的 `/proc/self/status` 没有读过；`resident()` 的 Linux 与别处两支只有 CI 编译。
- **B5 读的是整个进程的工作集**：客户端、rurge 与源站在同一个进程里；“经 rurge 减直连”是两次运行之差，不是 rurge 自己的分配。
- **`relay_throughput` 是用 `RELAY_BENCH_MIB=64` 跑的**（每轮每向 64 MiB，不是缺省的 256 MiB）。
- **没有 Surge 的读数可比**；NFR-01 的两条线管不管经引擎的路径归所有者（设计第 15 节第 6 条）。
- **没有手工验收**：M5b 不加手工条目（没有新的 API、日志或配置行为），`docs/acceptance/phase4-manual.md` 仍六十八项、一项也没有做。
- **M5（M5a 与 M5b）的提交 CI 都没有见过**：三个平台一次也没有编译、运行过它们；这台 Windows 开发机编译、运行过全部改动——`resident()` 的 Linux 与别处两支只有 CI 编译。

以后的会话必须知道的事（改那一块之前读 `bench.rs` 开头的模块注释与 M5b 计划的两张表）：

- **基准怎么跑**：一个基准一个进程、`--exact`、`--test-threads=1`，命令是 `cargo test -p rurge-engine --release --test bench -- --ignored --exact <名字> --nocapture --test-threads=1`（`--test bench` 是引擎的第二处 `-p` 例外，同 `relay_throughput`；跑之前先看放 `target/` 的盘剩 20 GiB 以上）；`HTTP_BENCH_QUICK=1` 在调试构建里把规模缩小、只看跑不跑得通（调试构建里不跑全规模，全规模只在任务 5、release 下跑过）。任务 5 直接起编好的测试二进制（工作目录 `crates\rurge-engine`）、外面套 GNU 的 `timeout`，与 `cargo test` 里 `--` 之后那些参数等价；理由是套在 `cargo` 外面的 `timeout` 只结束 `cargo`、测试二进制会留下来接着跑（记录第 19 行）。
- **B4 与会话恢复**（记录第 16 行）：MITM 的服务端在 TLS 1.3 上照样发票据（rustls 没有 `ticketer` 时走有状态的一支，把会话放进 `session_storage`）；B4 量的是完整握手，靠客户端关着恢复（`Resumption::disabled()`，设计第 16 节第 7 条照旧对）。rustls 客户端缺省的存储至多留 31 个服务器、挤掉最早的，握完不读也收不进票据——要让 B4 的 `handshake_kind` 断言变红，变异得同时开恢复、让存储装得下一遍的全部名字、握完读一下。记录第 15 行与 `0c9f5d5` 的提交说明里“服务端不发票据”的说法不对，以记录第 16 行为准。
- **上游连接池属于一条客户端连接**（记录第 18 行）：B5 经 rurge 的每条空闲连接各自留着上游连接，“经 rurge 减直连”含 rurge 接入与上游两侧（不解密的 TLS 隧道本来就一条对一条）。
- **B5 的差用有符号的数**（Windows 会修剪工作集，`now` 会变小）；一个进程只量一项（`static MEASURED: AtomicBool`，同一个进程里跑到第二个 B5 就 panic），峰值只涨不落；`before` 在预热之后读。
- **遗留三条用例钉住的行为**（记录第 11、12 行）：改 `crates/rurge-http/src/engine.rs` 的 `Idle`、`Progress::restart` 与 `retry` 里重发之前那一处 `progress.restart();` 之前读它们；两条单元用例用暂停的时钟（`rurge-http` 的开发依赖开了 `test-util`），端到端那一条用真实的时钟、约 3 秒、绿的一侧每处至少 1 秒余量、提交前单独循环 25 次全过。
- **延后的事项**：B1 的明文一路比 TLS 一路慢（延后第 1 行）与 B4 的握手长尾（延后第 2 行）查不查由 M6 的设计决定（延后第 2 行的第一步是加一个直连的握手基线）；改写正文的峰值（延后第 3 行）归所有者。

去向写着项目所有者的（后来的会话不要自己定）：

- **设计第 15 节第 6 条**：NFR-01 的线怎样用在经引擎的路径上——读数现在有了（B1 经 rurge 的下行 0.79 Gbit/s 是那条 1 Gbit/s 的 0.79 倍，B2 解密之后 h1 1.57、h2 1.17；B5 每条空闲连接经 rurge 减直连：明文 39.9 KiB、不解密的 TLS 30.0 KiB、解密的 75.5 KiB，分别是“每条 30 KB”的 1.36、1.02、2.58 倍），它管不管经引擎的连接与经引擎的明文、解密路径，归所有者。第 1 ～ 5 条与第 7 条照旧（M5a 一节已列，不重抄）。
- **推送与读回 CI**：M5（M5a 与 M5b）的提交都在本地；评审过后由协调者把 `phase4-m5` 快进合并进本地 `main`（`main` 上原来领先 `origin/main`（`a262d4c`）的 5 个提交也一起），由所有者推送，推送之后先读那次运行的注解与任务摘要。
- **手工验收**：`docs/acceptance/phase4-manual.md`（六十八项）一项也没有做。
- **延后事项第 3 行**（改写正文的峰值约多 3.94 份正文，总设计第 16 节风险 F）与**第 1 行的后半**（要不要为 B1 的明文一路慢改引擎）归所有者。
- **修正记录里代项目所有者作的裁定**：第 13、15、19 行（写“协调者的裁定”的那三行），所有者可以改；它们不是等所有者回答的事。

依赖附录：`rurge-http` 的开发依赖给 tokio 开 `test-util`（`f4fecda`，`crates/rurge-http/Cargo.toml` 第 45 行），别的 `Cargo.toml` 不改；生产代码（`crates/*/src` 里测试模块之外的部分）一行不改；`Cargo.lock` 不变（`git diff --stat 802a634 HEAD -- Cargo.lock` 是空的，520 个包）；生产代码里没有新的 crate 之间的边；`unsafe_code` 的名单仍是 23 项（`crates/rurge-platform/tests/unsafe_functions.rs` 的 `LISTED`），`crates/rurge/tests/sockets.rs` 的 `LISTED` 仍是 26 行。

M5 做完，下一步是项目所有者推送、读回那次 CI，之后是 M6（收尾与验收）的细化设计。

## 附：依赖方向一条的原文（原在 `CLAUDE.md`「计划中的架构」）

- 依赖方向（M2 设计文档确认）：`rurge-dns → rurge-rules → rurge-net → rurge-config`；`[Host]` 集合键与 `LazyResolver` 都由 `rurge-dns` 依赖 `rurge-rules` 提供，而非并列关系；`rurge (bin) → rurge-engine → { rurge-inbound → rurge-proto, rurge-policy → rurge-proto, rurge-dns }`。M4 设计文档确认：`rurge (bin) → rurge-api → rurge-engine`；`rurge-api` 依赖 `rurge-engine` / `rurge-config`（`rurge-dns` 的类型经 `rurge-engine` 间接可达，不需要直接依赖），不依赖 `rurge-platform`，也不认识 bin。`tests/interop`（`rurge-interop`，`publish = false`）与 `tests/external`（`rurge-external-tests`，`publish = false`，M4c）是仅测试用的工作区成员。阶段 1 / M5a 的 dev-dependencies 多两条边——`rurge-engine` 的 `tracing-subscriber`（抓会话日志的 `LogCapture`）与 `rurge` 在 Unix 上的 `nix`（`signal` 特性，`SIGHUP` 用例）——，都是工作区已有的依赖，`Cargo.lock` 里没有新的包，生产代码的依赖方向没有变。M7 设计文档确认（M7-D8）：`rurge-engine → rurge-proto-quic → { rurge-proto, rurge-net, rurge-config }`；QUIC 的共用部分（UDP 层、拨号与 TLS、自写的 HTTP/3 客户端）在 `rurge-net::quic`，M7c 的 DoQ / DoH3 经它（`rurge-dns → rurge-rules → rurge-net`），`rurge-dns` 不依赖协议 crate。阶段 1 补遗（M5b）：Profile 的远程来源在 `rurge-engine` 的 `profile` 模块（`ProfileSources`，随进程存续），`rurge-api` 经 `ApiContext.sources` 够到它；加载器（`rurge-config`）自己从不联网，下载到的文本经调用方给的 `RemoteSource` 取，被 include 的文本能做什么由 `Reach` 定；Basic 认证的 `take_basic_authorization` 在 `rurge_net::http`（DoH 经它），`percent-encoding` 与 `base64` 两条依赖边因此从 `rurge-dns` 挪到 `rurge-net`，`Cargo.lock` 仍是 517 个包。阶段 2 / M8a 的任务 6：`rurge-proto` 多一条到 `socket2` 的可选依赖边（作 `testing` 特性带进来，另加进 `[dev-dependencies]`），给要发大于 9216 字节数据报的测试 socket 调高发送缓冲，`Cargo.lock` 仍是 517 个包。阶段 3 / M3a 与 M3b（M3 设计 M3-D2）：新 crate `rurge-tun → { rurge-inbound, rurge-proto, rurge-net, rurge-config, rurge-dns }`（`rurge-dns` 是 M3b 加的边：`FakeIpPool`；dev-dependencies 另开它的 `testing` 特性），再加 `smoltcp`、`getrandom`、`tokio`、`tokio-util`、`tracing` 这些工作区已有的依赖，不依赖 `rurge-engine`、`rurge-platform`；`rurge-engine → rurge-tun`（M3b 加的边：`Engine::start_tun`、`impl Hooks for Engine`；根 `Cargo.toml` 的 `[workspace.dependencies]` 有了 `rurge-tun`，dev-dependencies 另开它的 `testing` 特性），可执行文件（`rurge`）与 `rurge-api` 照旧不依赖它，M3 里可执行文件不起 TUN（只多了 `platform_query` 把 `remote` 交给平台层这一处接线）；`Cargo.lock` 在 M3a 多了 `rurge-tun` 这一项，现在是 518 个包，M3b 只多了这两条依赖边，没有新的包，也没有新的外部包。`rurge-tun` 的 `testing` 特性（应用端夹具、假 `Dialer`、假 `Hooks` 与 `Pinger`、假的转发 `FakeDns`、造包与读包的 `packets`、回环上的服务器）由依赖它的 crate 的 dev-dependencies 打开：`rurge-engine` 的端到端用例（`crates/rurge-engine/tests/tun.rs`、`tun_dns.rs`）用它，`rurge-tun` 自己的用例走 `cfg(test)`。阶段 3 / M3c 没有加依赖边：`rurge-tun` 要的报文辅助、池和转发本来就在它依赖的 `rurge-dns` 里，`PreMatchLayer` 放在 `rurge-config`（所以 `rurge-tun` 不直接依赖 `rurge-rules`），引擎的预匹配钩子读 `rurge-rules`、引擎本来就依赖它；`Cargo.lock` 仍是 518 个包。阶段 3 / M4a 加了两条依赖边：`rurge-platform → ipnet`（外部包，工作区已有，`route::set` 的路由集计算用）与 `rurge`（可执行文件）`→ rurge-tun`（`cli/enhanced` 用它的 `Device`、`Ending`、`TunOptions`、`Pinger`）；计划里设想过的 `rurge → ipnet` 没有加，`ipv6_usable` 直接比地址的首段、为这个布尔值不值得（M4a 记录第 73 行）；`rurge-api` 照旧只依赖 `rurge-engine` 与 `rurge-config`，`rurge-platform` 照旧不依赖任何工作区 crate；`Cargo.lock` 仍是 518 个包（数法：`Cargo.lock` 里以 `name = ` 开头的行数）。阶段 3 / M4b 加了两条依赖边，都是工作区已有的包、都由可执行文件（`rurge`）加：`rurge → ipnet`（管理器的 `Backend::subnets` 签名写 `ipnet::IpNet`，M4b 记录第 36 行）与 `rurge → getrandom`（`PlatformPinger` 的标识符，第 44 行）；M4a 那一句里写的“计划里设想过的 `rurge → ipnet` 没有加”在 M4b 加了；`rurge-platform` 照旧不依赖任何工作区 crate，`rurge-net` 照旧不认识平台（出口经 `SocketHook` 问），`rurge-dns` 照旧不依赖协议 crate；`Cargo.lock` 仍是 518 个包（同一种数法）。阶段 3 / M4c 加了三条边的账：生产依赖 `rurge-platform → tokio`（`AsyncFd`，工作区已有的包，M4c 记录第 16 行）；`rurge-platform` 的 dev-dependencies 多 `tracing-subscriber`（`LogCapture`，`enhanced` 的用例抓日志用，协调者的裁定 R5，第 20 行）；新的仅测试用的工作区成员 `tests/privileged`（`rurge-privileged-tests`，`publish = false`，不是任何 crate 的依赖，第 36 行）。`Cargo.lock` 的包数按 `[[package]]` 的个数数：M4b 收尾时 518，任务 9 之后 519——多的是 `rurge-privileged-tests` 自己，没有新的第三方包（`1eb68db` 的提交说明写的“包数不变”由 M4c 记录第 37 行订正）。阶段 4 / M1a 加了新 crate `rurge-http`（生产依赖只有 `tokio`，dev-dependencies 有 `rustls`）与一条边 `rurge-engine → rurge-http`；`rurge-tun`、`rurge-inbound` 与 `rurge-api` 不依赖它（`rurge-api` 经引擎的 `http_counts()` 读）；`Cargo.lock` 520 个包（多的是 `rurge-http` 自己，按 `[[package]]` 数）。阶段 4 / M1b 给 `rurge-http` 加了四条工作区的边 `rurge-http → { rurge-config, rurge-net, rurge-inbound, rurge-proto }`（`rurge-proto` 只为 `RejectKind`）与七个外部的生产依赖 `hyper`、`hyper-util`、`http`、`http-body-util`、`bytes`、`tokio-util`、`tracing`（都是工作区已有的依赖，`Cargo.lock` 里没有新的包；上面 M1a 那句“生产依赖只有 `tokio`”到 M1b 不再成立），dev-dependencies 里给 `rurge-http` 加了一条带 `testing` 特性的 `rurge-net`（`TestServer`；`rurge-inbound` 的同样一条在阶段 1 / M3a（`0742dd3`）就有，M1b 没有动它的 `Cargo.toml`）；`rurge-engine → rurge-http` 在 M1a 已有，`rurge-tun` 与 `rurge-api` 仍不依赖它，`Cargo.lock` 仍 520 个包（只有 `rurge-http` 那一项的依赖列表变长）。阶段 4 / M2a（MITM）给 `rurge-http` 的正式依赖加了八个外部包：`rcgen`（开 `x509-parser` 特性）、`x509-parser`、`time`、`lru`、`rustls`（从 dev-dependencies 挪到正式依赖）、`tokio-rustls`、`sha2`、`ring`，开发依赖加了带 `testing` 特性的 `rurge-proto`（`TlsFixture` 与 `p12_base64`）与 `base64`；工作区的依赖表加了两行 `x509-parser = "0.18"` 与 `time = "0.3"`（都是原本的传递依赖，锁到 0.18.1 与 0.3.45）；`rurge-api` 只在开发依赖里加了带 `testing` 特性的 `rurge-proto` 与 `x509-parser`，它的正式依赖仍只有 `rurge-engine` 与 `rurge-config`（`CLAUDE.md`「架构」那一条仍成立）。没有新的工作区边（`rurge-http → rurge-proto` 与 `rurge-engine → rurge-http` 在 M1 就有）；`Cargo.lock` 仍 520 个包，只有 `rurge-http` 与 `rurge-api` 两项的依赖列表变长。阶段 4 / M2b（MITM：HTTP/2）给 `rurge-http` 的正式依赖加了一个外部包 `h2`（任务 1 先进开发依赖、任务 2 挪到正式依赖；锁文件里已有，包数不变），`rurge-engine` 的开发依赖加了 `hyper`、`hyper-util`、`http-body-util`（任务 5，工作区已有的依赖，`hyper` 开着 `client`、`server`、`http1`、`http2`）；没有新的工作区边，正式依赖的方向一条也没动（`rurge-tun`、`rurge-inbound`、`rurge-api`、`rurge-config` 仍不依赖 `rurge-http`，`rurge-http` 仍不依赖 `rurge-engine`）；`Cargo.lock` 仍 520 个包（只有 `rurge-http` 与 `rurge-engine` 两项的依赖列表变长）。阶段 4 / M2c（CA 工具与 `http-api-tls`）加了三条边的账，都是工作区里已有的 crate、没有新的外部包，`Cargo.lock` 仍 520 个包：`rurge`（可执行文件）→ `rurge-http`（任务 3：`rurge mitm ca generate` / `export` 用它的 `Generated`、`generate`、`Authority`、`certificate_pem`；`CLAUDE.md`「架构」里那条“只有可执行文件依赖 `rurge-platform`”与此无关，这是可执行文件到 HTTP 引擎的边，`rurge-api` → `rurge-http` 仍只在开发依赖里），`rurge-http` 的正式依赖加 `p12-keystore` 并把 `base64` 从开发依赖挪进正式依赖（任务 2），`rurge-api` 的正式依赖加 `rustls` 与 `tokio-rustls`（任务 5）、其开发依赖加 `rurge-http`（任务 5：单元用例要造 `Authority`，所以 `rurge-api` 的**正式**依赖仍只有 `rurge-engine` 与 `rurge-config` 两条工作区边），可执行文件的正式依赖加 `rurge-http`（任务 3）、`rustls` 与 `tokio-rustls`（任务 6）。`unsafe_code` 的名单仍是 23 项；`crates/rurge/tests/sockets.rs` 的名单从 25 项变成 26 项（任务 6 加的那一行：`crates/rurge/src/cli/api_client.rs` 的 `TcpStream::connect`，命令行的 HTTPS 客户端）。阶段 4 / M3a（URL Rewrite 与 Header Rewrite）没有加依赖边：`rurge-http` 的 `rewrite` 模块只用它已有的 `rurge-config`、`rurge-net`、`rurge-proto` 与 `http`、`rustls`、`fancy-regex`（经 `rurge-config` 的 `Pattern`），没有改任何 `Cargo.toml`，`Cargo.lock` 仍 520 个包（`[[package]]` 数）；`unsafe_code` 的名单仍是 23 项，`crates/rurge/tests/sockets.rs` 的名单仍是 26 行（重写不建套接字，换目标之后的拨号仍经 `dial_route`）。阶段 4 / M3b 任务 10（M3 收尾之后）：`rurge-proto` 的开发依赖加一条带 `testing` 特性的 `rurge-net`（单元测试用它的 `closed_tcp_port`，同 `crates/rurge-inbound/Cargo.toml` 的写法），只在开发依赖里，生产代码的依赖方向不变；`Cargo.lock` 仍 520 个包。
