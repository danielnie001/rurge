# 阶段 1 / M5 设计：补遗（托管配置、远程 include 与十余处没有落地的条款）

- 日期：2026-10-11
- 状态：由实施者定稿，没有经项目所有者评审。项目所有者的授权（2026-10-06）：决定由实施者自己做，不逐项询问；第 3 节的每一条都可以事后改。
- 对应需求：FR-CFG-03、04、05、09、10、14、15（当前 Profile 记录）、17；FR-RULE-03、04、06、15；FR-HTTP-10（基础）；FR-OBS-01、10（补用例）
- 上游文档：`docs/requirements.md`（第 4 节、第 7 节阶段 1 的状态行）、`docs/acceptance/phase2-acceptance.md`「需求逐条对照代码」、M8c 计划「延后事项」第 100 ～ 103 行、阶段 1 设计 `2026-09-03-phase1-core-skeleton-design.md`、M2 设计 `2026-09-04-phase1-m2-rules-dns-design.md`
- 调研只读（2026-10-10 ～ 11）：Surge 手册的 `tools/http-api.html`、`rules/overview.html`、`rules/ruleset.html`、`profile/requirement.html`、`profile/managed-profile.html`、`profile/format.html` 六页（手册改过目录，旧的 `others/http-api.html`、`rule/ruleset.html` 已不存在）；本仓库的源码。本文引用的三个 HTTP API 端点是 Surge Mac 6.10 / iOS 5.23 新加的，晚于本项目的手册基线（Mac 6.9 / iOS 5.22）——只取这三个端点，基线的整体更新仍是阶段 3 开工前单独的一件事（M8 设计第 1 节）。没有下载或安装任何东西。

## 1. 背景与范围

2026-10-10 把阶段 1 的功能需求逐条对着代码读过一遍，查出十余处条款没有落地或只做到解析（清单见验收文档的「需求逐条对照代码」一节）。它们都属于阶段 1，其中两处是 P0 的整块功能（托管配置的缓存与更新、远程 URL 的 `#!include`），其余是零散的小项。本里程碑把它们补齐，排在阶段 3 开工之前。

| 条款 | 现状 | 本里程碑 |
| --- | --- | --- |
| FR-CFG-05 托管配置：缓存、按间隔更新、`strict`、更新后自动重载 | 只解析 `#!MANAGED-CONFIG` 一行 | 做（M5b） |
| FR-CFG-04 / 14 远程 URL 的 `#!include` | `W0011`，什么也不展开 | 做（M5b） |
| FR-CFG-14 外部资源的手动强制更新 | `ResourceManager::force_update` 没有调用方 | 做（M5b，HTTP API） |
| FR-RULE-06 GeoIP / ASN 数据库日期可查询 | `GeoDb::info()` 没有出口 | 做（M5a，HTTP API） |
| FR-RULE-03 `pre-matching` 规则优先 | 规则按书写顺序评估，`pre_match_*` 没有调用方 | 做（M5a） |
| FR-RULE-15 子规则命中日志 | `Decision.sub_rule` 算出来但引擎不读 | 做（M5a） |
| FR-RULE-04 同一资源不可同时作两种集 | 两种引用各自成为一个集合，没有检查 | 做（M5a） |
| FR-CFG-09 平台变量取自操作系统 | `SYSTEM_VERSION` 恒为 `unknown`、`DEVICE_MODEL` 是 CPU 架构、`LANGUAGE` / `DEVICE_NAME` 只看环境变量 | 做（M5a） |
| FR-CFG-10 `#!WINDOWS-ONLY` / `#!LINUX-ONLY` | 没有 | 做（M5a） |
| FR-CFG-17 监听覆盖 | 没有 `--listen-http` / `--listen-socks5` | 做（M5a） |
| FR-CFG-03 `rurge check` 的建议 | 57 个诊断码里只有 `E0010` 带建议 | 做（M5a） |
| FR-HTTP-10（基础）请求记录的监听端口、远端地址、DNS 耗时、备注 | 没有这四项 | 做（M5a；远端地址有登记的限制） |
| FR-CFG-15 当前 Profile 记录 | `State.current_profile` 从不写入 | 做（M5a） |
| 有实现而没有用例的几条（FR-CFG-02、16，FR-OBS-01、10，FR-RULE-05） | 见验收文档 | 补用例（M5a）；FR-RULE-05 补一次实测 |

不在本里程碑：

- 规则匹配之前识别 SNI / Host（FR-RULE-07、08）：已登记为随阶段 4 的 HTTP 引擎；需求文档与兼容性清单在阶段上的出入由项目所有者定（M8c 计划「延后事项」第 105 行）。
- DNS 查询与 TCP 握手两个阶段的预匹配：没有 TUN 与对外的 DNS 服务之前无处可做，随阶段 3。
- 多 Profile 管理的配置目录、列出与切换（FR-CFG-15 的其余部分）：阶段 6。
- Logbook（FR-OBS-06）与 `profile-reloaded` 事件脚本：阶段 6 与阶段 5；托管配置更新后的记录目前是一条日志。
- 外部资源的 CLI 子命令：阶段 6（本里程碑只给 HTTP API）。
- `rurge run --config-dir`：阶段 1 设计的命令行草图里有它，但 `-c` 是必填的，配置目录只在 rurge 要在几份 Profile 之间列出与切换时才有用——那是多 Profile 管理（FR-CFG-15 的完整部分），在阶段 6。本里程碑只做草图里的另外两个，`--listen-http` 与 `--listen-socks5`。
- 非本子网来源被监听端拒绝的端到端用例（FR-IN-03）：要绑定非回环地址，自动化测试里做不了，留在手工验收清单。
- 三平台 CI 转绿与手工验收：不是本里程碑能产出的证据。

## 2. 拆分

两份计划，各自独立可交付。

- **M5a（零散条款）**：规则引擎三处（`pre-matching` 优先、子规则日志、同一资源两种集）、平台变量、两个简写、监听覆盖、`check` 的建议、请求记录四项、当前 Profile 记录、`GET /v1/geoip`、补用例。互相之间没有依赖。
- **M5b（Profile 的远程来源）**：资源管理器的四处扩展、远程 `#!include`、托管配置（生效文本、更新、`strict`）、启动时的引导、两个外部资源端点。

两份计划各带自己的文档任务：一份计划合并时，它改变的行为已经登记在兼容性清单与 API 参考里。

顺序 M5a → M5b：M5a 的请求记录备注与诊断建议是 M5b 的拒绝说明与新诊断码要用的。

## 3. 已决事项

| 编号 | 事项 | 决定 |
| --- | --- | --- |
| M5-D1 | 下载到的托管配置放在哪里 | 数据目录的资源缓存里。rurge 不改写用户的配置文件（FR-CFG-02），`-c` 指的本地文件始终是 Profile 的身份与起点 |
| M5-D2 | 本地文件与下载副本谁生效 | 一次成功的更新检查之后，服务端的内容生效；本地文件在那之后被改过（修改时间晚于那次检查、且不在将来）时，本地文件生效到下一次成功的检查为止 |
| M5-D3 | 副本里的 `#!MANAGED-CONFIG` 换了 URL | 跟着走，至多 8 跳；只有链上最后一个 URL 定期更新 |
| M5-D4 | 下载到的内容加载不了 | 不采用：缓存里留着上一份能用的，这次算更新失败。几个来源要一起换才加载得了的，在都到齐时一起采用 |
| M5-D5 | `strict` | 过了间隔而最近一次更新没有成功时，进入"暂停"：拒绝新会话、API 照常；更新成功后自动恢复。进程不退出 |
| M5-D6 | `interval` 的下限 | 60 秒；小于 60 的按 60 并告警（`W0012`），`interval=0` 按缺省的 86400 |
| M5-D7 | 远程 `#!include` 的内容从哪里来 | 加载器不联网：只读数据目录里的缓存。缓存里没有时这一行不展开、告警 `W0022`，下载到之后自动重载 |
| M5-D8 | 远程内容里的本地 include | 不读（`W0033`）：远程文件不能让 rurge 去读本机的文件。托管配置的副本除外——它就是用户的 Profile，其中的本地 include 相对本地文件所在目录解析 |
| M5-D9 | 启动时缓存里缺远程内容 | 启动先等一轮下载（有上限），再加载；到时限仍缺的照 M5-D7 |
| M5-D10 | Profile 的远程来源由谁管 | 一个独立于"每代一个"的资源管理器，随进程存续，放在 `rurge-engine` 的新模块 `profile` |
| M5-D11 | 手动更新与状态的出口 | 照 Surge Mac 6.10 / iOS 5.23 的 `GET /v1/external_resources` 与 `POST /v1/external_resources/update`；`key` 是来源的 SHA-256，`path` 不带凭据。CLI 留给阶段 6 |
| M5-D12 | GeoIP 数据库日期的出口 | 照同一版手册的 `GET /v1/geoip?ip=` |
| M5-D13 | `pre-matching` 在哪里生效 | 规则模式下，规则引擎先把带 `pre-matching` 的规则不解析 DNS 地过一遍，再走正常顺序。直连与全局模式不看规则（手册原话） |
| M5-D14 | 子规则命中记在哪里 | 一条 DEBUG 日志（带会话 id）与请求记录的 `notes` |
| M5-D15 | 同一资源两种集 | `[Rule]`、`[Host]` 与内联集里直接写出的：加载错误 `E0024`；集合文件里嵌套出现的：运行期告警 `W0032`，后一种引用不匹配 |
| M5-D16 | 平台变量的取值 | macOS 与 Surge Mac 同源；Windows 与 Linux 取同样形状的值（第 5.4 节）。取不到时退回现在的值 |
| M5-D17 | 远端地址怎么取 | 这条会话自己开的 socket 连到的地址，经任务局部变量从连接器带回；复用已有连接或隧道的会话没有这一项（登记） |
| M5-D18 | 诊断的建议 | 按诊断码给缺省建议，诊断自己带了建议时用自己的；不设建议的码要写明理由（第 5.3 节） |
| M5-D19 | 新的诊断码 | `E0024`、`W0032`、`W0033`、`W0034`、`I0004`；`W0011` 只剩"没有数据目录"一种情形 |
| M5-D20 | 下载到的文本里的 `external` 策略（写 M5b 计划时加，见第 13 节） | 远程 include 与托管配置的副本里的 `external` 定义不建出站，行为同 REJECT（`W0033`）；写在本地文件里的照旧。`external` 启动本机的程序，一份会自己更新的文本不该有这个能力。Surge 的托管配置能带这种策略——登记为差异；要不要给一个放开它的命令行开关由项目所有者定 |
| M5-D21 | 新版本提到还没下载过的来源（写 M5b 计划时加，见第 13 节） | 不立刻采用：新来源先被跟踪并下载，试完第一次时与新版本一并采用；其间旧的一代照常运行，`strict` 不把这段等待算作失败 |
| M5-D22 | `[Rule]` 里几条 `FINAL` 的读法（M5a 重读手册时查出，见第 13 节） | 匹配在第一条启用的 `FINAL` 处结束，策略与选项（`dns-failed`）取最后一条启用的 `FINAL` 的——手册 `rules/final.html` 与 `profile/format.html` 的三句话同时成立的唯一读法。rurge 原先让夹在两条 `FINAL` 之间的规则照常评估。是推断，没有在 Surge 上观察过 |
| M5-D23 | API 关连接的方式（M5a 的一条偶发失败的用例查出，见第 13 节） | 分三步：关写端、读掉晚到的字节（至多 1 秒、256 KiB）、再关——RFC 9112 第 9.6 节说的做法，nginx 的 `lingering_close` 与 Go 的 `closeWriteAndWait` 同此。放在连接一层，对每条连接都做；不取"答复之前先读完正文"的中间件 |
| M5-D24 | 集合文件里没有用上的行（M5a 延后，见第 13 节） | 带未知参数的行按集合计数、建注册表时一条 `W0003`（不引用参数）；启动之后载入的集合有没用上的行、且数与上一次不同时一行 WARN（只有数与行号）；三个数进 `/v1/external_resources`。同一个注册表的重编译一次一个 |
| M5-D25 | 称阶段 1"齐备"之前的需求复读（CLAUDE.md 的规矩，见第 13 节） | 是 M5b 的一个任务，排在全部代码任务之后；每条"做了"指到一条会因它而红的用例；产出 `docs/acceptance/phase1-acceptance.md`；"齐备"写不写由它的结论定 |

## 4. 规则引擎（M5a）

### 4.1 `pre-matching` 优先（FR-RULE-03）

手册（`rules/overview.html`）："Rules tagged with `pre-matching` are extracted and checked before everything else"、"Pre-matched rules have the highest priority."、"When the outbound mode is set to Direct or Global instead of Rule-Based, the rule list is not consulted at all."

`RuleEngine::run` 在规则模式下多一步：正常顺序开始之前，按 `PreMatchingSet::positions()` 的顺序把带 `pre-matching` 的规则各评估一次，评估时一律当作写了 `no-resolve`（预匹配从不触发 DNS）。第一条命中的就是结果：`Decision.reason` 是新的 `Reason::PreMatching`（文本 `pre-matching`），`matched` 是那条规则，命中计数加一，`sub_rule` 照常填。都没命中时进入正常顺序，带 `pre-matching` 的规则在其中仍留在自己的位置上（一条要解析才能命中的 IP 规则，在预匹配里不命中，在自己的位置上照常命中——手册说预匹配是"additionally"的）。

`evaluate_traced` 的轨迹里，预匹配只为命中的那条规则加一步（`pre-match`）；没命中的规则会在正常顺序里出现，不重复列。

`pre_match_domain` / `pre_match_ip` 保留，与预匹配这一步共用同一段评估：阶段 3 在 DNS 应答与 SYN 两个阶段调用它们。

出站模式是直连或全局时 `Engine::choose_policy` 在调用规则引擎之前就返回，预匹配随之不生效，与手册一致。

### 4.2 子规则命中（FR-RULE-15）

手册（`rules/ruleset.html`）："When a sub-rule matches, the log records it as `Sub-rule matched: <rule> (in <set name>)`."

`Engine::choose_policy` 在 `decision.sub_rule` 有值时：

- 记一条 DEBUG 日志 `Sub-rule matched: <entry> (in <set>)`，带 `session` 字段（与"会话完成"那一行同级、同一个 id）；
- 往会话的备注里加同一句（第 6.2 节的 `notes`）。

`<set>` 是集合的显示名（`SetHandle::name()`）：URL 不带用户名口令。嵌套时沿用现在的取法（最外层集合与它命中的那一行）。

这句话要说对，先得修一处现有的错：一次评估里，集合一命中就记下"命中的子规则"，不管外面那条规则最后命中没有（逻辑规则里集合那一半命中、另一半不符时），而它在规则之间不清——后面命中的规则会把它当成自己的。两趟评估都在每条规则之前清掉它，`Decision.sub_rule` 只属于命中的那条规则。今天这个错只在离线的 `rurge rule match` 里看得到。

### 4.3 同一资源不可同时作两种集（FR-RULE-04）

手册（`rules/ruleset.html`）："The same URL or file path cannot be used as both a RULE-SET and a DOMAIN-SET in one profile."（没有说违反时怎样。）

- **加载期**（`rurge-config`）：收齐配置里直接写出的全部集合引用——`[Rule]` 的顶层规则与逻辑规则的子规则、`[Host]` 的 `RULE-SET:` / `DOMAIN-SET:` 键、内联 `[Ruleset]` 里的集合行——只看 `ResourceRef::Url` 与 `ResourceRef::File` 两种。同一个引用以两种身份出现时，在后出现的那一行报 `E0024`（"the same resource cannot be both a RULE-SET and a DOMAIN-SET"，指出先出现的那一行的位置；URL 不带凭据）。被 `#!REQUIREMENT` 关掉的行不算。
- **运行期**（`rurge-rules` 的 `SetRegistry::ensure`）：同样只看 URL 与文件两种引用——要登记 `(key, kind)` 而 `(key, 另一种)` 已在时，记 `W0032` 与一条告警日志，返回空集合（这个引用从不匹配），不为它建条目。加载期查不到的只有集合文件里嵌套出现的引用，这是它们的兜底；先登记的那种身份保留。

选错误而不是告警：两种身份里总有一种是按错的格式读文件，那个集合实际是空的，它后面的规则悄悄不生效——对一个分流工具，宁可启动时就说清楚。Surge 自己怎样处理没有证据，登记。

## 5. 配置层的小项（M5a）

### 5.1 `#!WINDOWS-ONLY` / `#!LINUX-ONLY`（FR-CFG-10）

手册（`profile/requirement.html`）："Three simplified notations are provided for convenience: `#!IOS-ONLY`, `#!MACOS-ONLY`, `#!TVOS-ONLY`."，唯一的例子写在行尾（`DOMAIN,reject.com,REJECT #!MACOS-ONLY`）。

`requirement::SIMPLIFIED` 加两项，分别等价于 `SYSTEM == 'Windows'` 与 `SYSTEM == 'Linux'`，写法与 rurge 现在认的 `#!MACOS-ONLY` 相同（行首接内容，或行尾）。这两个标记 Surge 不认识：行首写法的整行在 Surge 上是一行注释，这样的行只在 rurge 上生效；行尾写法在 Surge 上怎样没有证据——多半被当成行尾注释、内容照常生效，那就不是写的人要的。文档里只推荐行首写法。`redact_profile` 经同一个 `split_line_slice` 认得它们，不用另改。

### 5.2 监听覆盖（FR-CFG-17）

`rurge run` 加 `--listen-http <ADDR>` 与 `--listen-socks5 <ADDR>`（环境变量 `RURGE_LISTEN_HTTP` / `RURGE_LISTEN_SOCKS5`），取值的写法与 `[General]` 的 `http-listen` / `socks5-listen` 相同，由同一个解析函数读。给出时替换配置里对应的那一项（重载之后仍然生效）；没给的那一种照配置。写法不对是命令行错误，启动即以 2 退出。

`http-listen` 的取值可以带口令（`[口令@]地址[:端口]`），所以这两个取值不交给 clap 的取值解析器——它报错时会把取值原样打印出来——而是作为字符串收下、由 `run()` 自己解析并自己报错，错误里只有原因、没有取值；`--help` 不显示环境变量当前的取值。

实现是加载之后、构建运行时之前对 `Config.general` 的一次改写，启动与重载走同一个函数；`--system-proxy` 指向的地址取自实际绑定的监听端，不用另改。

### 5.3 `check` 的建议（FR-CFG-03）

`rurge_config::diagnostic::codes::hint(code) -> Option<&'static str>` 给每个诊断码一条缺省建议；`Diagnostic` 在构造时填进自己的 `hint`，诊断自己用 `with_hint` 带了建议时以自己的为准。文本输出与 `--json` 都已经会显示 `hint`，不用另改。判据是没有一个码被漏掉：码表里的每个码要么有缺省建议，要么它的每个发出点自带建议（同一个码几处的改法各不相同时），要么列在"不设"的名单里、理由写在码表旁边——没有发出点的（`E0009` 自阶段 2 / M3a 起）、消息自己已经把原因与改法说完的、平台或阶段不适用而使用者没有要改的、只是告知的。一条用例逐码核对这三类与码表一致、互不重叠。

一条建议必须对发出这个码的每一处都成立——同一个码在几处因不同的原因发出、改法不同时，缺省建议只说共同的部分，各处用 `with_hint` 说自己的。建议不得引用配置里的取值。语料库的快照里出现的诊断随之多出 `hint` 一行。

### 5.4 平台变量（FR-CFG-09）

手册（`profile/requirement.html`）给的取值样例：`SYSTEM_VERSION: System Version 17.4.1 (Build 21E236)`、`DEVICE_MODEL: iPhone16,1`、`LANGUAGE: en-US`、`DEVICE_NAME: Tim's iPhone`，并说 `DEVICE_NAME` 在 macOS 上是系统设置里的电脑名。

`rurge-platform` 新模块 `sysinfo`：`read() -> SystemInfo { version, model, language, name }`，四项都是 `Option<String>`，尽力而为、从不失败；`rurge` 的 `cli::environment` 用它，取不到的一项退回现在的值（`unknown`、CPU 架构、`en-US`、`rurge`）。读取分两层：与平台无关的纯函数（解析与拼接）与各平台的读取。照本 crate 现有的做法（`dirs`、`sysproxy`），macOS 与 Linux 的读取吃"命令、文件、环境变量"三种来源，三个平台都编译，用假的来源在每台机器上都测；只有 Windows 的注册表读取与"选哪一个"带 `cfg`。

| 变量 | macOS | Windows | Linux |
| --- | --- | --- | --- |
| `SYSTEM_VERSION` | `/System/Library/CoreServices/SystemVersion.plist` 的 `ProductVersion` 与 `ProductBuildVersion` → `System Version 14.6.1 (Build 23G93)` | 注册表 `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion` 的主、次版本号、`CurrentBuildNumber` 与 `UBR` → `System Version 10.0.22631 (Build 22631.4317)` | `/proc/sys/kernel/osrelease` → `System Version 6.8.0 (Build 6.8.0-45-generic)` |
| `DEVICE_MODEL` | `sysctl -n hw.model` → `Mac15,6` | 注册表 `HKLM\HARDWARE\DESCRIPTION\System\BIOS` 的 `SystemProductName` | `/sys/class/dmi/id/product_name`，没有时 `/proc/device-tree/model` |
| `LANGUAGE` | `defaults read -g AppleLanguages` 的第一项，没有时照 Linux | 显示语言：注册表 `HKCU\Control Panel\Desktop` 的 `PreferredUILanguages` 第一项（使用者指定过显示语言时才有），没有时 `HKCU\Control Panel\International\User Profile` 的 `Languages` 第一项（语言列表的首选），再没有时 `HKCU\Control Panel\International` 的 `LocaleName`（区域格式） | `LC_ALL`、`LC_MESSAGES`、`LANG` 里第一个非空的，去掉编码与修饰、`_` 换 `-`；`C` / `POSIX` 不算 |
| `DEVICE_NAME` | `scutil --get ComputerName`，没有时 `scutil --get LocalHostName` | 注册表里的计算机名，没有时 `COMPUTERNAME` | `/proc/sys/kernel/hostname`，没有时 `/etc/hostname`、`HOSTNAME` |

厂商留空时填的占位值（`To Be Filled By O.E.M.`、`System Product Name`、`Default string` 等）不算型号。macOS 上的三条命令经现有的 `CommandRunner`；Windows 用已有的 `windows-registry` 依赖；Linux 只读文件。不新增依赖，不新增 `unsafe`。

登记的差异：`SYSTEM_VERSION` 的样式照手册的示例，与真机逐字是否一致没有证据；Windows 与 Linux 上是 rurge 自己照同样形状定的。`SYSTEM_VERSION` 是字符串，`>=` 一类的比较按字符串比（现状），要比版本用 `BEGINSWITH` / `MATCHES`。

### 5.5 当前 Profile 记录（FR-CFG-15 的阶段 1 部分）

`rurge run` 启动时把 `-c` 的绝对路径写进 `state.json` 的 `current_profile`（`std::path::absolute`：不解析符号链接，Windows 上不带 `\\?\` 前缀；值没变时不重写文件）。读它的是阶段 6 的多 Profile 管理；本里程碑只保证它被写下。

## 6. 观测（M5a）

### 6.1 `GET /v1/geoip`（FR-RULE-06）

照手册（`tools/http-api.html`，Mac 6.10 / iOS 5.23）：`GET /v1/geoip?ip=<地址>` 返回 `address`、`country`（ISO 3166 代码）、`asn`（数字，示例 `13335`）、`organization`、`geoip-db-date`、`asn-db-date`。原话："using the same databases as the `GEOIP`, `IP-ASN` rules"、"Fields without a result are null."、"`geoip-db-date` and `asn-db-date` are the UNIX timestamps of the databases in use."、"If `ip` is not a valid IP address, a 400 error is returned."

`GeoDb` 加一个不触发"库不在"告警的查询（国家代码、ASN 编号与组织名——ASN 库里本来就有组织名，规则用不到所以没读）。两个日期取自 `GeoDb::info()`，即 MMDB 元数据里的构建时间——手册只说是"数据库的时间戳"，没有说是构建时间还是下载时间，rurge 取库自己带的那一个（登记）。

### 6.2 请求记录的四项（FR-HTTP-10 基础）

`RequestRecord` 与 API 的 JSON 各加四项：

| 字段（JSON） | 含义 | 来源 |
| --- | --- | --- |
| `listenPort` | 会话进来的监听端口 | `SessionInfo.in_port` |
| `remoteAddr` | 这条会话自己开的 socket 连到的地址；没有时是 `null` | 见下 |
| `dnsMs` | 这条会话拨号期间花在域名解析上的时间合计（规则要的解析与 DIRECT 自己的解析都算，命中缓存的接近 0）；没有做过解析时是 `null` | 见下 |
| `notes` | 备注，字符串数组；本里程碑只有子规则命中一句 | `SessionHandle::add_note` |

远端地址与 DNS 耗时都发生在离会话很远的地方（`DirectConnector`、`Resolver`），而 `ConnectOpts` 只有一个超时、在五十多个文件里各自构造。不改这些接口：`rurge-net` 加一个很小的模块 `trail`——

```rust
pub struct Trail { /* 连到的地址、解析耗时合计与次数 */ }
impl Trail {
    pub fn new() -> Trail;
    pub async fn follow<F: Future>(&self, f: F) -> F::Output;   // 任务局部变量的作用域
    pub fn peer(&self) -> Option<SocketAddr>;
    pub fn clear_peer(&self);                                   // 换一次尝试之前
    pub fn dns(&self) -> Option<Duration>;
}
pub fn connected(addr: SocketAddr);     // DirectConnector 连上一条 TCP 之后调用
pub fn looked_up(spent: Duration);      // Resolver 的一次查找结束时调用
```

`SessionHandle` 持有一个 `Trail`；引擎把拨号的整段 future 放进 `follow`，`choose_policy` 里的规则评估同样（UDP 流与内部 DNS 会话由此也有 `dnsMs`）。不在作用域里的调用什么也不做。地址字面量不是一次查找，目标是地址的会话 `dnsMs` 是 `null`。`smart` 组换成员时，每次尝试之前忘掉上一次记下的地址：上一个成员连上了 TCP 却在握手里失败、下一个成员走的是复用的连接时，记录里不能留着上一个成员的地址。链式策略（`underlying-proxy`）的会话记下的是第一跳。测速会话不记这三项。解析器向上游发的查询各在另起的任务里，查找自己开的连接（`tcp://`、`tls://`、`https://` 上游）因此不会被记成会话的远端地址——这靠的是解析器现在的写法，由一条用例钉住。

登记的限制（M5-D17）：任务局部变量只在会话自己的任务里可见，所以 `remoteAddr` 只在这条会话自己开了 socket 时有——DIRECT 与每条连接各自拨号的协议。复用已有连接或隧道的会话（`ssh`、`wireguard`、QUIC 族、HTTP/2 族、连接池里取出的 `anytls` / `snell` 连接）没有这一项——建起那条连接或隧道的那一条会话除外，它自己开了 socket。UDP 的会话（流与内部 UDP 会话）一律没有：它没有单一的远端，而经代理的载体只由第一条用到它的流打开，只让那一条带上地址反而误导。补全随阶段 4 的"完整"。

### 6.3 补用例

- FR-CFG-02：CLI 用例——带着一份 Profile 启动，做一次重载、一次组选择、一次停止，前后逐字节比较 Profile 文件。
- FR-CFG-16：只在 Unix 上的 CLI 用例——发 SIGHUP，等到重载生效。
- FR-OBS-01 / 10：抓会话日志的用例——日志行的 `session` 字段等于请求记录的 `id`，并带 `rule`、`policy`。
- FR-RULE-05：一条不进门禁的 `#[ignore]` 用例，各建一个 100 万条的域名集与规则集；在开发机上实测进程的峰值内存，把数字回填到 M2 设计的 Q3 与本文的修正记录。

## 7. Profile 的远程来源（M5b）

### 7.1 资源管理器的扩展（`rurge-net`）

1. **种类**：`ResourceSpec` 加 `kind: ResourceKind`（`RuleSet`、`DomainSet`、`PolicyGroup`、`GeoIp`、`Profile`、`Include`、`System`；最后一种是 `/etc/hosts` 这类不对外列出的）。四个登记处各自填。
2. **状态**：`ResourceStatus` 加 `kind`、`label`、`key`（`source.key()` 的 SHA-256，十六进制）、`local`、`updating`（一次下载在途）。
3. **可等待的更新**：`update_now(key) -> Result<(), String>`——踢一下并等这一次尝试结束，返回它的结果；本地文件直接返回。`force_update` 留着。
4. **采用之前的检查**：`get_checked(spec, label, check)`，`check: Arc<dyn Fn(&[u8]) -> Result<(), String> + Send + Sync>`。下载到新内容时先过 `check`，不过的不写缓存、不发布版本，这次尝试按失败处理（`Failed`，`last_error` 是 `check` 给的理由，照常退避）。`adopt(source, data)`：把一份内容当作刚下载到的写进缓存并发布（不带校验器，下次刷新完整重取）。

日志照旧只叫标签或不带凭据的来源。

### 7.2 加载器（`rurge-config`）

```rust
/// 加载器取 Profile 里按 URL 指名的内容的地方。加载器自己从不联网。
pub trait RemoteSource: Send + Sync {
    fn get(&self, url: &str) -> Remote;
}
pub enum Remote {
    Text { text: String, path: PathBuf, fetched_at: SystemTime },
    Missing { why: Option<String> },
}
```

`LoadOptions` 加 `remote: Option<Arc<dyn RemoteSource>>`（`Debug` 手写）。`path` 是缓存里那份文件的路径：诊断的位置指向它，使用者打得开。

**远程 `#!include`**（手册 `profile/format.html`："The included profile can be a local file or a remote URL."、"`#!include` can also reference a remote managed profile (a URL) directly."、"The local layer stores only your overrides, while the remote managed profile keeps receiving updates automatically."）：

- 目标是 `http(s)://` 时向 `RemoteSource` 要内容。拿到的按本地 include 的同一套规则展开（就地、通配命名节、缺节的 `W0017`、深度 8、总数 200、成环 `E0016`——环按 URL 判）。
- 没拿到：这一行不展开，告警 `W0022`（"remote include not downloaded yet"，有失败原因时带上；不引用 URL）。
- `LoadOptions.remote` 是 `None`（没有数据目录的调用方）：`W0011`，文本改为"remote include not read: no data directory"。
- 远程内容里再出现的 include：URL 照上面办；本地路径不读，告警 `W0033`（M5-D8）。
- 更新间隔：被 include 的内容自己带 `#!MANAGED-CONFIG` 时取它的 `interval`（下限同 M5-D6），否则 86400 秒；它的 `strict` 与 URL 不看（`strict` 只属于主 Profile；被 include 的地址由写 include 的那一行定）。

**托管配置的生效文本**。手册（`profile/managed-profile.html`）："Surge can automatically update a profile from a URL."、"Ensure the new remote profile also includes the `#!MANAGED-CONFIG` line."、"Without it, the profile reverts to a standard profile."。手册没有说的三件事：下载到的内容里那一行换了 URL 时怎样、本地文件是不是被改写、下载到的内容采用之前验不验——分别是 M5-D3、D1 / D2、D4 的决定。

`config::load` 读到本地文件 L 之后、解析正文之前，先定"主文本"：

1. L 没有 `#!MANAGED-CONFIG`，或 `remote` 是 `None`：主文本是 L。
2. 否则取 L 那一行的 URL U，向 `RemoteSource` 要：
   - 没有：主文本是 L，待更新的是 U。
   - 有，而 L 的修改时间晚于副本的 `fetched_at` 且不在将来：主文本是 L（`I0004` 说明本地文件比副本新、用本地文件到下一次更新为止），待更新的是 U。
   - 有，其余情形：主文本是副本。看副本自己的 `#!MANAGED-CONFIG`：没有——"reverts to a standard profile"，不再更新，`W0034`；URL 仍是 U——待更新的是 U，间隔与 `strict` 取副本这一行的；URL 换成了 U′——对 U′ 重复这一步（副本之间不再比修改时间），至多 8 跳，超过或成环时停在已有的最后一份并 `W0034`。
3. 主文本是副本时给一条 `I0004`（"managed profile: using the copy downloaded at …"，带缓存文件的路径）；它里面的本地 include 相对 L 所在目录解析。

`fetched_at` 在"没有变化"（304）的应答上同样前移——一次成功的检查就是一次成功的更新，这正是 M5-D2 要的。依据是一条推断，不是手册的原话：从"新内容里没有那一行，Profile 就变回普通 Profile"可以看出 Surge 是拿下载到的内容换掉 Profile 的，本地的改动因此留不过下一次更新；rurge 不改写文件，用"副本生效"得到同样的结果。

`SourceInfo` 加 `main_text: PathBuf`（主文本实际读自的文件）与 `remote: Vec<RemoteRef>`（这次加载碰到的每个远程来源：URL、是托管还是 include、拿到没有、间隔、`strict`；`Debug` 不显示 URL）。`Config.managed` 是"管着下一次更新的那一行"（链上最后一跳的）。`GET /v1/profiles/current` 返回主文本。

### 7.3 `rurge-engine::profile`

```rust
pub struct ProfileSources { /* 一个随进程存续的 ResourceManager、当前跟踪的来源、待采用的候选 */ }
impl ProfileSources {
    pub fn new(data_dir: &Path, offline: bool, check: Check) -> Arc<ProfileSources>;
    pub fn remote(&self) -> Arc<dyn RemoteSource>;       // 读磁盘缓存；拿不到时带上管理器记下的失败原因
    pub fn track(&self, sources: &SourceInfo);            // 登记这一代的来源；不再出现的不再跟踪
    pub async fn settle(&self, limit: Duration) -> bool;  // 等每个还没有内容的来源试完第一次
    pub async fn changed(&self);                          // 任何一个来源有了新版本（去抖 1 秒）
    pub fn strict(&self) -> Option<Suspended>;            // 见 7.4
    pub fn statuses(&self) -> Vec<ResourceStatus>;
    pub async fn update(&self, key: &str) -> Option<Result<(), String>>;
}
pub fn disk_remote(data_dir: &Path) -> Arc<dyn RemoteSource>;   // 没有管理器的调用方（check、离线命令）
```

`Check` 是"把这份候选当作这个 URL 的内容，Profile 加载得了吗"：`load_checked` 加一层覆盖（这个 URL 答候选，其余读磁盘），没有错误即通过。先只带这一份候选试；不过而另有待采用的候选时，带上全部待采用的再试一次，过了就把它们一并 `adopt`（M5-D4 后半句）；仍不过的记为待采用，等别的来源到齐时再试。待采用的候选只在内存里，每个 URL 至多留最新的一份。

来源集合变小时（重载后某个 URL 不再出现）重建管理器——管理器没有撤掉单个条目的接口；重建后以当时的版本为起点，不把"重新读到缓存"当成一次变化。

日志与状态里，托管配置叫 `managed profile`，include 叫 `remote include <序号> (<scheme>://<host>/…)`；URL 的路径与查询串不出现。

### 7.4 `strict` 与暂停

手册（`profile/managed-profile.html`）：`interval` 是 "Optional, in seconds, default: 86400"、"This is the shortest time before an update may be triggered"、"Surge does not necessarily update immediately after the interval passes."；`strict` 是 "Optional, Boolean, default: false"、"If `strict` is true, Surge requires a successful update after the interval arrives."、"Otherwise, if the update fails, the user may continue using the outdated profile."、"Even when `strict` is true, the user can still start Surge from a widget or the VPN switch in Settings."。手册没有说 `strict` 不满足时 Surge 具体不做什么；最后一句说明它不是一把彻底的锁。

`ProfileSources::strict()` 在下面三条同时成立时给出 `Suspended { since, why }`：

- 管着下一次更新的那一行写了 `strict=true`；
- 距上一次成功的检查已不少于间隔（没有副本时从本地文件的修改时间起算）；
- 最近一次尝试失败（管理器里这个来源是 `Failed`），或者运行在 `--no-network` 下。

`Engine::suspend(Option<String>)` 存下这个状态（跨重载）。暂停期间 `Dialer::dial`、UDP 流的打开与内部 DNS 会话都在最前面拒绝：会话以失败结束，请求记录的 `error` 是 `refused: the managed profile requires an update (strict)`；监听端与 API 照常。主循环每秒看一次 `strict()`，变化时改引擎的状态并记日志（进入是 WARN，恢复是 INFO）。

更新自己不经引擎的流水线（资源管理器用直连的 HTTP 客户端），暂停不妨碍它。下载到的内容没通过检查也是一次失败的更新。恢复的路径只有一条：一次成功的更新（随后自动重载）。

### 7.5 启动与运行（`rurge` 可执行文件）

启动：

1. 数据目录在加载 Profile 之前就定下（`RuntimeArgs` 不依赖 Profile），`LoadOptions.remote` 用磁盘视图，照常 `load_checked`。
2. 这次加载碰到了没拿到内容的远程来源、或者 `strict` 已到期，而又不是 `--no-network`：进入引导——临时的单线程运行时里建一个 `ProfileSources`、`track`、`settle`（每轮至多 30 秒），再加载一次；新一次加载又露出没见过的来源时再来一轮，至多 8 轮、合计至多 60 秒。引导期间还没有日志，下载失败的原因经 `Remote::Missing { why }` 进诊断。
3. 最后一次加载仍有错误：照现在的样子打印并以 2 退出。
4. 主运行时里另建一个 `ProfileSources`（引导用的随临时运行时结束；磁盘缓存把一切带过来），`track` 当前这一代。`strict` 到期而引导没更新成功时，引擎一起来就是暂停的。

运行：主循环的 `select!` 多一路 `sources.changed()`，与文件监视、SIGHUP、API 的重载走同一个 `reload_and_refresh`；每次重载成功后 `track` 新一代的来源。`--watch` 只看本地文件，远程来源由 `changed()` 管。重载失败保留旧配置（现有行为）。

其余读 Profile 的命令（`check`、`rule match`、`dns lookup`、`status` / `reload` / `stop -c`、`service install`）都用磁盘视图，看到的与运行中的实例一致；它们不联网。

### 7.6 外部资源的两个端点（FR-CFG-14）

照手册（`tools/http-api.html`，Mac 6.10 / iOS 5.23）：

- `GET /v1/external_resources` → `{"defines":[…]}`，每项 `path`、`type`、`key`、`local`、`ready`、`updatedAt`、`updating`、`fromModule`，上次更新或解析失败时另有 `error`。各字段的原话：`type` 是 "One of `ruleset`, `domainset`, `script`, `policy-group` and `data`."（没有解释 `script` 与 `data` 各指什么）；`key` 是 "The identifier of the resource, used by the update endpoint."（示例是 32 位十六进制）；`local` 是 "Whether the resource is a local file. Local resources have no `updatedAt` field."；`updatedAt` 是 "The UNIX timestamp of the last successful download, or 0 if the resource has never been downloaded."；`fromModule` 是 "The name of the module that references the resource, or null if it's from the profile."；`error` 是 "Present only if the last update or parsing failed."。`path` 没有说明，示例里是原样的 URL。
- `POST /v1/external_resources/update`，正文 `{"key":"<key>"}` 或 `{"key":"all"}`（"Use `"all"` as the key to update all remote resources."、"Local resources are skipped."）；应答 "maps each updated key to `"success"` or an error message"；"If the key is not found, a 400 error is returned."

rurge 这边：列表合并当前这一代的资源管理器与 `ProfileSources` 两处的状态（`System` 种类不列）。`type` 用手册的 `ruleset`、`domainset`、`policy-group`，另加 rurge 自己的 `geoip`、`profile`、`include`；手册的 `script` 随阶段 5 的脚本引擎，`data` 手册没有解释、rurge 不用它。`key` 是来源的 SHA-256（64 位十六进制），调用方只应把它当成不透明的标识。`path` 是标签或不带用户名口令的来源（手册的示例是原样的 URL——登记）。本地资源不带 `updatedAt`；远程而从没下载到的是 `0`。`fromModule` 恒为 `null`（模块在阶段 5）。托管配置那一项在暂停时另有 `suspended: true`。更新经 `update_now`，逐个等结果，整个请求有时限。

## 8. 需登记的差异（进兼容性清单）

1. 托管配置不改写本地文件：副本在数据目录；本地文件在一次成功的检查之后被改过时暂时生效（M5-D1、D2）。
2. 副本换了 URL 时跟着走至多 8 跳（M5-D3）；本地文件里那一行不变。
3. 下载到的内容加载不了时不采用（M5-D4）；Surge 的 CLI 文档只对手动更新说了"validate the result"。
4. `strict` 是暂停而不是不启动；`--no-network` 下到期即暂停（M5-D5）。
5. `interval` 下限 60 秒（M5-D6）。
6. 远程 include：缓存里没有时不展开并告警；远程内容里的本地 include 不读；被 include 的内容自己的 `strict` 不看（M5-D7、D8）。
7. `/v1/external_resources`：`key` 的算法是 rurge 自己的（来源的 SHA-256）、`path` 不带凭据、`type` 多三种（`geoip`、`profile`、`include`）而没有 `script` 与 `data`、`fromModule` 恒为 `null`；没有对应的 CLI（M5-D11）。`/v1/geoip` 的两个日期是库的构建时间。
8. `pre-matching`：DNS 查询与 TCP 握手两个阶段的预匹配随阶段 3（M5-D13）。
9. 子规则命中的日志在 DEBUG 级（M5-D14）。
10. 同一资源两种集：直接写出的是加载错误，嵌套的是运行期告警；Surge 的做法没有证据（M5-D15）。
11. 平台变量的取值样式（第 5.4 节）；`SYSTEM_VERSION` 按字符串比较。
12. `#!WINDOWS-ONLY` / `#!LINUX-ONLY` 是 rurge 的扩展，只推荐行首写法（第 5.1 节）。
13. 请求记录的 `remoteAddr` 只在会话自己开了 socket 时有（M5-D17）。
14. `--listen-http` / `--listen-socks5` 是 rurge 的运行时选项（FR-CFG-17）。
15. 下载到的文本（远程 include、托管配置的副本）里的 `external` 策略不启动，行为同 REJECT（M5-D20）。
16. 只有一行 `#!MANAGED-CONFIG`、没有任何节的本地文件只是一个指针：它被改动不会让它生效。服务商换过地址之后，本地文件与副本的先后看的是整条链上最新的那次检查。手册（`profile/format.html`）："A managed profile cannot be modified locally because it can be updated remotely. To make changes, first create a copy to turn it into a normal profile."——Surge 不让使用者改托管配置，"本地文件被改过"时怎么办没有 Surge 的行为可循，第 1 项与这一项都是 rurge 自己的规则。
17. 托管配置的新版本提到还没下载过的来源时，等它们试完第一次再一并采用（M5-D21）；几个来源要一起换的同样。
18. 远程 include 的新内容缺它被 include 的节时不采用（一页 HTML 不会被当成一份空的规则表）。
19. 内容没变的 200 不算新版本、不触发重载；对规则集、域名集与订阅同样。
20. Profile 的来源走直连下载，不经策略；`interval` 是下限，间隔的抖动只往后；只在 `rurge run` 运行时更新（手册："The profile can only be updated while the main Surge app is running."），`rurge check` 与其余命令只读缓存。
21. `strict`：等新版本提到的来源不算失败；本地文件生效时取本地那一行的 `strict`。
22. `reload` / `stop` / `status` 的 `--data-dir`；`/v1/external_resources` 的 `ready` 与 `updatedAt`（最近一次成功的检查，"没有变化"的应答也算）、`suspended`，更新请求的 60 秒时限与 `"timed out"`；规则集与域名集的项多 `skipped`、`truncated`、`unknownParams` 三个 rurge 自己的字段，`error` 不因它们出现。
23. `[Rule]` 里匹配在第一条启用的 `FINAL` 处结束、策略取最后一条的（M5-D22）；读法是从手册的三句话推断的。
24. 规则集文件里带未知参数的行：参数被忽略，按集合报一条 `W0003`，不引用参数（内联小节逐行报、引用参数）。
25. API 关连接之前先关写端并读掉晚到的字节（M5-D23）：不读正文的端点与在读正文之前被拒绝的请求，正文晚到的客户端也读得到应答。

## 9. 测试

- **单元**：预匹配的顺序与不解析；`E0024` 的各种写法与 `W0032`；`SIMPLIFIED` 的两项；平台变量的纯函数（三平台都跑）；建议表与码表一致；生效文本的每一条分支（本地新、副本新、将来的修改时间、换 URL、没有那一行、成环、8 跳）；远程 include 的展开、缺失、环与本地路径；`Trail` 在超时与并发分支下的作用域。
- **回环 HTTP 服务端**（沿用 `rurge-net` 资源测试的那一个）：采用之前的检查（不过的不写缓存、退避、`last_error`）、`update_now`、`updating`、`adopt`；`ProfileSources` 的跟踪、去抖、几个来源一起换、`strict` 的三条件。
- **引擎**：预匹配经流水线、请求记录的四项、暂停时三种入口都拒绝且 API 可用、`GET /v1/geoip`、两个外部资源端点。
- **CLI**（真实拉起 `rurge`，全部回环）：缓存为空时带远程 include 启动（引导）、托管配置更新后自动重载、更新到一份加载不了的内容时保留上一份、`strict` 到期而服务端不可达时会话被拒且恢复后放行、`--listen-http`、Profile 文件逐字节不变、`current_profile`、Unix 上的 SIGHUP。
- 凭据不外泄：托管配置与 include 的 URL（带标记）不出现在日志、诊断、API 输出与 `Debug` 里——并进 `crates/rurge-config/tests/secrets.rs` 的那份标记配置，引擎与 CLI 各一条。

不联网、端口取 0、等待都有上限、不用固定睡眠做同步。

## 10. 写计划时必须核对的事项

- V1 手册 `tools/http-api.html`：三个端点的应答逐字段再读一遍（`asn` 的类型、日期字段、`update` 应答的形状、400 的条件）。
- V2 `tokio::task_local!` 的作用域在 `tokio::time::timeout`、`select!` 与 `DirectConnector` 的并发连接里是否都可见（它竞速时是否 `spawn`）；`Resolver` 的查找是否都经过同一个入口。
- V3 所有加载 Profile 的地方（各 CLI 子命令、API 的 `profiles/reload` 与 `profiles/check`、`check_profile`）与它们各自有没有数据目录。
- V4 `Engine::config_text`、`profile_path()`、`profile_key(&cfg.source.main)` 的全部用处：状态的键仍用本地路径。
- V5 `--watch` 的路径表与 `SourceInfo.includes`：远程来源不是路径。
- V6 `SetRegistry::ensure` 在构建与热重载时的调用顺序（`W0032` 的"先登记的保留"）。
- V7 `windows-registry` 读 DWORD 与字符串的接口；四个注册表值在 Windows 10 / 11 上的类型。
- V8 macOS 三条命令的输出格式与 `SystemVersion.plist` 的格式（照公开文档与开源项目里的解析核对；本机没有 macOS）。
- V9 现有用例里依赖 `LANG`、`COMPUTERNAME` 或四个变量当前取值的。
- V10 `W0011` 的文本与新诊断码牵动的快照（`insta`）。
- V11 `ResourceSpec`、`LoadOptions`、`SourceInfo`、`RequestRecord`、`ResourceStatus` 的全部构造处。
- V12 监听端对 `DialError::Failed` 的应答（暂停时 HTTP 客户端看到的状态码）。
- V13 `http-listen` / `socks5-listen` 的解析函数能否从命令行取值复用。
- V14 资源测试里回环 HTTP 服务端的能力（条件请求、可编排的应答）。

## 11. 任务草图

M5a：

1. 规则引擎：预匹配优先；子规则日志与 `notes`；`E0024` / `W0032`。
2. 配置层：两个简写；监听覆盖；当前 Profile 记录。
3. 平台变量（`rurge-platform::sysinfo`）。
4. 诊断的建议。
5. 请求记录的四项（`rurge_net::trail`）与 `GET /v1/geoip`。
6. 补用例（Profile 不被改写、SIGHUP、日志与记录的 id、内存实测）。
7. 文档：M5a 改变的行为进兼容性清单、需求文档的状态行、API 参考、验收文档、README（中英）、CLAUDE.md。

M5b：

1. 资源管理器的扩展。
2. 加载器：`RemoteSource`、远程 include。
3. 加载器：托管配置的生效文本。
4. `rurge-engine::profile`（`ProfileSources`、检查与一并采用、`strict`）与引擎的暂停。
5. 可执行文件：引导、主循环、其余命令的磁盘视图。
6. 两个外部资源端点。
7. 文档：兼容性清单、需求文档的状态行、API 参考、验收文档、README（中英）、CLAUDE.md。

## 12. 实施期的订正（M5a）

M5a 的执行（计划 `docs/superpowers/plans/2026-10-11-phase1-m5a-small-items-plan.md`，2026-10-11）改了本文几处的决定，也让几句不再成立。第 1 ～ 11 节不改，在这里逐条订正；每条的来龙去脉在计划末尾的「执行期修正记录」与「延后事项」两张表里（下面写"记录第 N 行""延后第 N 行"）。

1. **第 1 节末行、第 2 节、第 11 节：M5a 不止七个任务。** FR-RULE-05 在补用例时查出只做到一部分：集合里带 `no-resolve` 的 `IP-CIDR` / `IP-CIDR6` 行不进前缀树（常用的列表每条 IP 行都这样写）；集合里的 `IP-ASN` 行不是手册与需求说的常数时间，而常用的 ASN 列表每行都以注释结尾、规则集文件只认整行注释，整份被跳过。计划因此在执行中新加任务 8（带 `no-resolve` 的 IP 行进前缀树）与任务 9（规则集文件的行尾注释；集合里的 `IP-ASN` 行按号码查），并在任务 9 的修正里顺带修了两个早于本计划的缺陷：集合文件开头的 UTF-8 BOM 被算进第一行、域名集里读不了的行被告警原样引用（记录第 51、56、65、66 行）。第 1 节表里 FR-RULE-05 一行的"补一次实测"之外，M5a 还改了集合的求值，顺序成了：名字索引 → 逐条求值的行（文件顺序）→ `IP-ASN` 行 → IP 行（M2 设计第 17 节第 5 条）。
2. **第 3 节 M5-D15、第 4.3 节：`E0024` 与 `W0032` 的细节。** (a) `E0024` 报在 `Config` 里排在后面的那一处（顺序是规则、`[Host]` 的集合键、内联集合，不是文件里的先后），每个资源一次；文本是 `the same resource cannot be both a RULE-SET and a DOMAIN-SET (it is used as a <kind> at <file:line>)`，说出另一处的种类与位置而不说谁先谁后（两处可以在不同的文件里），不引用资源（记录第 5 行）。(b) 运行期"先登记的那种身份保留"只在两条都是嵌套引用时成立：Profile 自己的文本（规则——含逻辑规则的子规则——、`[Host]` 的集合键、内联集合的各行，也就是 `E0024` 看的那些用法）给的角色，总是压过集合文件里嵌套的另一种角色的引用，不论被占的条目建没建；否则同一份 Profile 在冷缓存与热缓存上分流不同（记录第 4 行）。(c) `W0032` 的文本带上写着那条引用的集合：``set `<name>` (referenced in `<parent>`) ignored: the same resource is used as a <RULE-SET|DOMAIN-SET>``（记录第 6 行）。
3. **第 4.2 节最后一段：清掉命中的子规则不够。** 同一条逻辑规则里没有参与命中的分支（`OR,((AND,((RULE-SET,x),(DEST-PORT,1))),(DOMAIN,a))`）仍会留下 x 的命中，新加的日志就会说假话；逻辑匹配器在没有命中时也恢复进入时的 `sub_hit`（`NOT` 一律恢复）（记录第 1 行）。预匹配那一趟遇到的运行期提示经 `collect_notes`，每条规则只记一次，哪一趟先遇到算哪一趟（记录第 2 行）。
4. **第 4.2 节：几行同时命中时报哪一行。** 本文没有说。现在报成子规则的是求值顺序里第一个命中的行（名字索引 → 逐条求值的行，文件顺序 → `IP-ASN` 行 → IP 行；IP 行之间先 IPv4 再 IPv6、同一地址族里取最长前缀），不一定是文件里最靠前的；报出的文字是去掉行尾注释的那一行（记录第 53、60 行）。Surge 报不报、报哪一行不知道。
5. **第 5.1 节："`requirement::SIMPLIFIED` 加两项""`redact_profile` ……不用另改"。** `SIMPLIFIED` 不是唯一的清单：`text::is_requirement_prefix` 是另一份写死的三个简写，节解析器与 `redact.rs` 先问它——只加进 `SIMPLIFIED` 的简写写在行尾有效、写在行首被当成未知指令丢掉。现在只留一份：`is_requirement_prefix` 搬进 `requirement.rs`，从 `SIMPLIFIED` 派生（记录第 9 行）。`#!WINDOWS-ONLYX` 这样粘着别的字母的写法不是简写，也一向不是"未知指令"——`is_requirement_prefix` 只看前缀、没有词边界，这样的行按它所在节的普通行读（`[Rule]` 里是 `E0012`），三个旧简写也一样（记录第 16 行）。
6. **第 5.2 节：监听覆盖的三个决定。** (a) "没给的那一种照配置"：只给 `--listen-http` 而 Profile 没写 `socks5-listen` 时没有缺省的 SOCKS5 监听——覆盖读起来就像 Profile 只写了 `http-listen`（"缺省只在两种都没写时才有"是原有的规则；记录第 11 行）。(b) 设了而为空的环境变量（`RURGE_LISTEN_HTTP=`）是错误，与 `--listen-http ''` 相同（`no address given`，退出 2；记录第 10 行，代项目所有者裁定）。(c) 覆盖进来的端口落在某条 `external` 策略的 `local-port` 上时 rurge 会自己连自己，加载期的检查看不到：覆盖就位之后再问一次（`Config::external_on_own_listener`），启动时报错、恢复上一次崩溃留下的系统代理、以 2 退出，重载则失败而保持运行中的配置（记录第 12 行）。
7. **第 5.3 节：`check` 的建议。** (a) "57 个诊断码"加上任务 1 的 `E0024`、`W0032` 是 59 个：33 个有缺省建议，`E0014` 四处各自带，25 个不设（记录第 24 行）。(b) "文本输出与 `--json` 都已经会显示 `hint`，不用另改"不全：`rurge check` 的两种输出是这样；写进日志的诊断仍是一行（`Diagnostic::headline()`），给人看的打印（`print_diagnostics`：`rurge run` 的启动与重载、`rule match`、`dns lookup`）要多打一行 `hint:`，`rule match --json` 与 `dns lookup --json` 的 `warnings` 手拼 `code` 与 `message`，现在也带 `hint`（记录第 27、32 行）。(c) `Diagnostic.hint` 与 `with_hint` 的参数都是 `&'static str`，"建议不得引用配置里的取值"因此由类型保证（记录第 29 行）。(d) "语料库的快照里出现的诊断随之多出 `hint` 一行"要先让 `tests/corpus.rs` 的 `render()` 带上建议（它自己拼文字、从不含建议），三份快照随之变（记录第 25 行）。
8. **第 5.4 节：平台变量。** (a) Windows 的 `DEVICE_NAME` 取注册表 `Tcpip\Parameters` 的 `Hostname`（DNS 主机名，保留使用者输入的大小写，也是"设置"里显示的那个），不取大写的 NetBIOS 名；`COMPUTERNAME` 仍是回落（记录第 18 行）。(b) 区域变量照 POSIX 读，`LC_ALL`、`LC_MESSAGES`、`LANG` 里第一个设了值的说了算，`C` / `POSIX` 时系统没有给出语言；取不到时的回落与它共用一条规则（`rurge_platform::sysinfo::language_of`），所以 `LANG=POSIX` 现在回落到 `en-US`、`sr_RS@latin` 是 `sr-RS`（记录第 19、20 行）。(c) `--platform` 指到别的系统时，`SYSTEM_VERSION` 只在所评估的平台就是本机的平台时报真实值，否则 `unknown`；其余三个照读到的报（记录第 17 行）。(d) 占位型号是六个前缀与十三个整串，不分 ASCII 大小写（记录第 21 行）。
9. **第 5.5 节：当前 Profile 记录。** 写在启动的最后一步（打印 `rurge … running` 之前）；`state.json` 写不进去（数据目录不可写）时启动照常，多一行 ERROR `cannot write state.json`（记录第 13 行）。启动因此在说出"在跑"之前多写一次 `state.json`，两条现有的进程用例的准备改为先等那一行（记录第 14 行）。
10. **第 6.1 节：`GET /v1/geoip` 的错误。** `ip` 不是地址时 400 的文本是 `ip is not a valid IP address`（不回显调用方给的取值）；缺 `ip` 时是 axum 查询串拒绝的原文（记录第 38 行）。IPv4 映射的地址（`::ffff:a.b.c.d`）按 IPv4 查，回显的 `address` 也是 IPv4（记录第 43 行）。
11. **第 6.2 节：请求记录的四项。** (a) `Trail` 的草图里"解析耗时合计与次数"：只存 `Option<Duration>`，次数没有人读（记录第 37 行）。(b) 耗时用 drop 守卫登记，拨号被外层超时丢弃时花在这次解析上的时间同样计入 `dnsMs`；同名查询已在途时，等它的时间也算（记录第 34、39 行）。(c) 引擎里三处拨号的函数体抽成私有方法（`connect_session`、`connect_internal`、`open_internal_udp`）再放进 `follow`（记录第 35 行）。(d) `listenPort` 在 JSON 里对 rurge 自己发起的会话（`listener: "internal"`）是 `null`，记录里仍是 `u16` 的 `0`（记录第 42 行）。(e) `remoteAddr` 的限制："建起那条连接或隧道的那一条会话除外，它自己开了 socket"说宽了：`wireguard` 的载体由隧道自己的任务拨号，是 `null`；QUIC 族是 `null`，只有经 `underlying-proxy` 且底层策略的 UDP 载体是当场打开的一条 TCP 连接时，碰巧打开 QUIC 连接的那一条会话带着底层策略服务器的地址（延后第 21 行）；`external` 的会话是 `127.0.0.1:<local-port>`（它确实建了一条到程序本机端口的 TCP 连接，记录第 41 行）；UDP 一律没有，这条规则放在拍记录的地方（`record_of`，`crates/rurge-engine/src/observe.rs`），而不是拨号之后的清除（记录第 40 行）；IPv4 映射的远端地址记成 IPv4（记录第 43 行）。
12. **第 6.3 节：补用例。** (a) FR-RULE-05 的实测数字回填在 M2 设计第 16 节的 Q3 一行：100 万条的 `DOMAIN-SET` 载入时的峰值比基线多约 403 MB，`RULE-SET` 约 456 MB，建好之后各约 137.5 MiB 与 156.4 MiB（Windows，发布构建，四轮）；"可控"没有判据，留给项目所有者（延后第 24 行）。内存实测是三条 `#[ignore]` 用例（域名集、规则集、基线），不是两条（记录第 45 行）。(b) `SIGHUP` 用例先等守护进程说出 `rurge … running` 再发信号——处理器是在那一行之前才装上的；同一文件里早先的 `SIGINT` 用例同样改过（记录第 46 行）。两条都只在 Unix 上编译，本机没有编译过，第一次 CI 运行时核对（延后第 1、23 行）。
13. **第 8 节：又多了这些差异。** 除第 8 节的 14 条之外，执行中又定了下面这些；兼容性清单（1.4、3.3、3.5、10.3、10.4 各节）已经登记的有：规则集文件的行可以带行尾注释（手册只对 Profile 写明了这条规则，说内联规则集与外部文件是同一种行语法，"Surge 读得了集合文件行尾的注释"是从两份常用的列表推断的，记录第 57 行）；值里有"空白加 `#`、`;` 或 `//`"的行现在在那里截断，单引号括起来也一样，要保留用双引号（记录第 58、64 行）；集合文件开头的 UTF-8 BOM 不算进第一行（记录第 65 行）；带 `no-resolve` 的 IP 行与 `IP-ASN` 行改变了报成子规则的是哪一行，以及运行期不求值的类型的那句提示出现的时机（记录第 53、60 行）；Windows 的 `DEVICE_NAME` 取 DNS 主机名、`--platform` 下的 `SYSTEM_VERSION`；设了而为空的环境变量是错误、只给 `--listen-http` 时没有缺省的 SOCKS5 监听；`#!WINDOWS-ONLYX` 这样的行不是简写。只记在计划里、没有进兼容性清单的一处是：没有载入 GeoIP / ASN 库时那句每个进程一次的告警，可能由这样的会话提前触发，也可能被推后（记录第 53、60 行）。
14. **整分支的评审之后：第 5.2、5.3、6.2 节各改了一处（记录第 71 ～ 74 行）。** (a) **第 5.2 节：`external` 策略拉起的程序得不到 rurge 自己的设置。** 本文没有说。`--listen-http` 的口令可以放在 `RURGE_LISTEN_HTTP` 里（本里程碑加的建议），而 `external` 策略拉起的程序原先继承 rurge 的全部环境，程序只要把自己的环境打印出来（脚本里的一句 `env`），口令就进了 `<数据目录>/external/<策略名>.log`；拉起的程序本身是另一个 rurge 时，它还会照第一个的设置监听，不照自己 Profile 里的 `local-port`。现在名字以 `RURGE_` 开头的变量（不分大小写）都不交给程序，按前缀判断而不列名单：这些名字声明在可执行文件里，名单迟早过时（`is_own_setting`，`crates/rurge-proto/src/external.rs`；记录第 72 行）。(b) **第 5.3 节：`E0008` 在 `include-other-group` 的一处带自己的建议。** 缺省建议是"定义缺的策略或组"，而这个参数只收策略组的名字，名字是 `[Proxy]` 里已有的一条策略时照着做不成；这一处改说给参数写 `[Proxy Group]` 里定义的组的名字，单独一条策略作为组的成员列出。`E0008` 的其余各处仍用缺省建议，59 / 33 / 25 的数没有变（记录第 73 行）。(c) **第 6.2 节：内部 DNS 会话的 `remoteAddr` 与别的 TCP 会话同一条规则。** 文档写成"上游服务器的地址"，只在会话直连时成立；经以地址配置的代理时是代理服务器的地址，代理走复用的连接或隧道时是 `null`。两份 API 参考改了，经代理与因代理以域名配置而改走直连两种情形各补了一条断言；测速会话的三个键恒为空也补了断言（记录第 74 行）。

## 13. M5b 计划期的订正

写 M5b 实施计划（`docs/superpowers/plans/2026-10-11-phase1-m5b-profile-remote-sources-plan.md`）时核对本仓库的源码（资源管理器的登记、抖动、缓存与状态；加载器的 include 展开与头部指令；`rurge run` 的启动顺序与崩溃恢复；监听端对拨号失败的应答；UDP 流的重试；M5a 合并之后又对了一遍：诊断的建议表、`RuleEngine::build`、集合注册表的重载任务、`rurge-api` 的 `serve` 与它依赖的 axum 0.8.9 的 `Listener`）与手册的四页（`profile/managed-profile.html`、`profile/format.html`、`rules/final.html`、`tools/http-api.html`，2026-10-11 重读；上文引用的 31 句原话逐句对过页面的文本，一句不差）后，与上文不一致或上文没写到的，以本节为准（括号里是计划「计划期决定」的编号）。第 3 节的表多出 M5-D20 ～ D25 六行，第 8 节多出第 15 ～ 25 项。

1. **资源管理器（细化与订正 7.1）**：
   - 状态里的 `fetched_at` 放宽为"最近一次成功的下载或'没有变化'的应答，只要知道就给"（今天在 `failed` 时是 `None`），另加 `ready`（手里有内容）。缓存的 `Meta` 多一个纳秒字段，时刻到纳秒：只到秒的话，同一秒里"先存文件、后下载"会被判成本地文件更新。公开的 `cached_copy(root, url)` 给出正文、正文所在的文件与时刻（P1、P2）。
   - 正文与手里的逐字节相同的 200 照"没有变化"办，对所有种类生效：不带校验器的服务端因此不会每个间隔触发一次重载（P3）。
   - 管理器加 `retire(source)`；7.3 的"来源集合变小时重建管理器"不做。间隔变了的来源撤掉重登——对同一个条目再登记只会把间隔往短里并（P4）。
   - `update_now` 返回 `Option<Result<(), String>>`（没有这个条目是 `None`），等的是调用之后才开始的那一次尝试，离线时立刻给错误（P5）；另加 `attempted(key)`：等到这个条目此刻没有一次该做而没做完的尝试。
   - `ResourceOptions.never_early`：Profile 的来源的间隔抖动只往后（0 ～ +10 %），手册说 `interval` 是最短的时间（P6）；它们的退避上限是 600 秒（P23）。
   - 失败原因（`last_error`）里不出现 URL 的任何部分，重定向的 `Location` 也算。
2. **加载器的接口（订正 7.2）**：`RemoteSource::get(&Url)`，不是 `&str`——缓存目录的名字取自 `Url` 的规范写法，加载器与资源管理器必须说的是同一个值（P26）。`RemoteRef { url, role, path, interval, strict, lacks_section }`：`role` 是 `Managed`（定期更新的那一个）、`ManagedHop`（链上经过的，只读缓存）、`Include` 三种，一个 URL 一项、先出现的身份留下。`Origin` 加 `Remote`（这一行来自下载到的文本）。`#!MANAGED-CONFIG` 的 URL 与远程 include 的目标解析不成 `http(s)` 的 URL 时是 `W0012`（不引用取值），那一行当作没写。下载到的文本经 Profile 自己的读取器，与本地 include 的文件完全相同（BOM、注释、要求指令）。
3. **下载到的文本里的 `external` 策略（新增，M5-D20）**：远程 include 与托管配置的副本里的 `external` 定义不建出站——它留在 `Config.policies` 里（组与规则对它的引用照常成立）、没有 spec、`NotImplemented::DownloadedExternal`，行为同 REJECT，每次加载一条 `W0033`；写在主文件或本地 include 里的照旧。`W0033` 因此有两种用处（下载到的文本里的本地 include、`external` 策略），各带各的建议。订阅导入的 `external` 行自阶段 2 / M4c 起已经一律跳过，这是同一条理由（P7）。
4. **远程 include 的"不是 Profile"（新增）**：新内容缺它被 include 的节时不采用（`RemoteRef.lacks_section`）。一页 HTML 当作 include 展开时只有告警，没有这一条它会被当成一份空的规则表采用。已经在缓存里的照旧只是 `W0017`（P8）。
5. **谁生效（订正 7.2 的第 2 步，细化 M5-D2）**：
   - 本地文件一个节也没有时（只有一行 `#!MANAGED-CONFIG` 的指针）不参与先后的比较，副本在就用副本。这样的文件被动过之后按原来的规则会加载不了，而副本没到期、不会去更新（P9）。
   - 先沿缓存里的副本走到头，再拿本地文件的修改时间与链上各份副本里最新的那次检查比——7.2 只比第一跳的副本。第一跳换了地址之后不再刷新，它的时刻停在换地址的那一天：此后本地文件一被动过就永远"更新"，被跟踪的退回本地那一行的旧地址。被跟踪的总是链的头，哪怕本地文件生效；本地文件生效时间隔与 `strict` 取本地那一行的；走过不止一个地址时多一条 `I0004`，提醒本地那一行该改了（P10）。
   - 副本没有 `#!MANAGED-CONFIG` 而本地文件生效时，那份副本的 URL 仍被跟踪：本地那一行说这份 Profile 是托管的。
   - `Config.managed` 是生效的主文本自己的那一行；被跟踪的 URL、间隔与 `strict` 以 `SourceInfo.remote` 里 `Managed` 一项为准（本地文件生效而地址换过时，两者的 URL 不同）。
6. **诊断（订正 7.2）**：不带时间戳——同样的输入给出同样的诊断，时刻在 `/v1/external_resources` 里。托管配置没拿到时也给一条 `W0022`（`managed profile not downloaded yet`，带原因）：只有一行的本地文件第一次启动失败时，使用者靠它知道为什么。`I0004` 的三句与 `W0034` 的两句见计划任务 4。副本生效时，本地文件自己的文本的诊断不出现（P16）。新的码在 M5a 的建议表里各有去处：`W0033`、`W0034` 每一处各带建议，`I0004` 是告知，`W0011` 的理由改写为"只有没给加载器远程来源的调用方才见得到它"（P34）。
7. **采用之前的判定（订正 7.3，M5-D21）**：
   - 一次判定有三种结论：通过、不通过、等待。等待是"加载得了，但这次加载提到的来源里有没拿到内容、又还没试完第一次的"：它们随即被跟踪，候选留着；它们下载到时一并采用，下载失败时候选在自己那个来源的下一次重试时采用（那一行照 `W0022` 不展开）。没有这一条，多了一行远程 include 的新版本会先换上缺着那一块的一代，几秒后再补齐（P12）。
   - 顺序：另有留着的候选时先带上全部留着的一起判，再只带这一份——7.3 写的是反过来。新来源到达的那一刻就能把等它的候选一并采用（P11）。
   - 不通过另有两种：Profile 已不提这个地址（P14）；候选缺它被 include 的节（上面第 4 条）。
   - 留着的候选在它的来源回到 `available` 时作废：服务端又答了"没有变化"，留着的那份已不是它现在给的内容。
   - `ProfileSources::new(data_dir, offline, check)` 改为 `new(SourcesOptions)` 与 `production(..)`：判定是它自己的事，不由调用方给。
8. **`outdated()`（新增）**：本地文件生效的一代里，一次"没有变化"的应答不升版本，`changed()` 不会返回，而 M5-D2 要它结束本地文件的优先。主循环每秒看一次 `outdated()`，为真时重载；对同一个检查时刻只真一次（P15）。
9. **`strict`（细化与订正 7.4）**：
   - 上一次检查的时刻在将来（系统时钟被拨回）不算到期。
   - 候选因"等待"而留着的不算失败——下载是成功的；因"不通过"而留着的算（P13）。
   - 7.4 的"恢复的路径只有一条"订正为两条：一次成功的更新；或者使用者把本地文件（一份完整的 Profile）那一行的 `strict` 去掉并重载——本地文件生效时 `strict` 取它那一行的。只有一行的本地文件没有第二条路：`strict` 是副本那一行说的，使用者换一份 Profile 启动。
   - 被拒的入口是四个：`dial`、UDP 流的打开、`dial_internal`、`dial_internal_udp`。UDP 以"失败"而不是"拒绝"结束：失败的流 5 秒后重试，暂停一结束目标就恢复，被拒绝的流要等客户端停下来 60 秒才放手（P22）。
10. **`settle`（细化 7.3）**：除了没有内容的来源，还等到了期的 `strict` 来源（否则启动时判断不了要不要暂停）与等待期间新登记的来源；到期而不是 `strict` 的不等；返回之前把留着的候选再判一次（P17）。
11. **启动时的引导（订正 7.5）**：
    - 位置仍在日志之前，但先建数据目录、拿数据目录的实例锁——下载要写缓存，不能与另一个实例同时写——，这把锁沿用到进程结束；开始时往标准错误写一行。引导之后仍加载不了、与 `--log-file` 打不开的两条退出，共用"拿到锁之后"的那一半崩溃恢复：原来的函数自己去拿锁，同一个进程拿不到第二把（P18）。
    - 要不要引导由 `boot_need` 定。有没拿到内容的来源或到期的 `strict`：等一轮。有一条错误诊断的位置在下载到的文件里：把每个来源更新一遍——7.5 第 3 步对加载不了的 Profile 是直接退出，而生效的副本一旦加载不了（升级后校验变严、它引用的本地 include 被删），每次启动都死在同一份副本上，没有运行着的进程去刷新它。错误全在本地文件里的照旧以 2 退出：不拿一次更新去盖掉使用者自己的错误（P19）。
    - 一轮什么也没改变时即结束，不空转到上限；逻辑在 `rurge-engine::profile`（`boot_need`、`ProfileSources::bootstrap`），可执行文件只接线（P20）。
    - M5a 的监听覆盖（第 12 节第 6 条）：套覆盖的 `apply` 只对最后用来启动的那一次加载做，它失败与"加载有错误"同样处理。
12. **其余命令（订正 7.5 的末段）**：`reload` / `stop` / `status -c` 加 `--data-dir`——它们读 Profile 是为了 `http-api`，托管配置生效时这一项在副本里；`service install` 不加载 Profile，不动（P21）。
13. **两个端点（细化 7.6）**：`ready` 是"手里有内容"，`updatedAt` 是最近一次成功的检查（"没有变化"的应答也算）（P24）；`ApiContext.sources`（P25）；整个更新请求至多 60 秒，到时限还没结束的键是 `"timed out"`；新版本在等它提到的来源时，那个键的结果是等待的那一句；列表里 Profile 的来源排在最前；`all` 的下载同时发出，下载到的集合的重编译由注册表排队（P32）。
14. **标签（订正 7.3）**：`managed profile (<scheme>://<host>[:port]/…)`、`remote include <key 的前 8 位> (<scheme>://<host>[:port]/…)`。序号随 Profile 的改动而变，而一个条目的标签在它登记时就定了；`key` 的前 8 位不变，也对得上 `/v1/external_resources` 里的那一项。
15. **`[Rule]` 里几条 `FINAL`（新增，M5-D22；订正 M2 设计 4.2 与 6.5）**：手册 `rules/final.html`："A FINAL rule always matches, so any rules written below it never take effect. If several enabled FINAL rules are present, the last one is the effective one."；`profile/format.html`："In the `[Rule]` section, a `FINAL` rule immediately terminates rule matching. Rules included or defined after it are never evaluated, so check the position of `FINAL` in both the main profile and the included files."。rurge 原先只照"最后一条生效"读：编译到最后一条 `FINAL`、跳过此前的每一条、它们之间的规则照常评估。改为：匹配在第一条启用的 `FINAL` 处结束（`Config::first_final`），策略与选项取最后一条的（`effective_final`，不变）；`W0019` 从第一条 `FINAL` 起数，`W0021` 的文本改为 `this FINAL does not give the policy; the last FINAL rule does`。排在远程 include 之前做：把一整份托管配置的 `[Rule]` include 进本地的 `[Rule]`（手册的 Linked Profiles）正是带来第二条 `FINAL` 的写法。只有带两条或更多启用的 `FINAL`、且之间夹着规则的 Profile 受影响（P28）。
16. **集合文件里没有用上的行（新增，M5-D24）**：规则集文件里带未知参数的行按集合计数（`ParsedSet.unknown_params`、`SetStatus.unknown_params`），建注册表时一条 `W0003`、说集合与第一处的行号、不引用参数；启动之后载入的集合，三个数（`skipped`、`truncated`、`unknown_params`）不全为零且与上一次不同时一行 WARN；`SetStatus.key` 把集合的状态接到 `/v1/external_resources` 的那一项上。同一个注册表的重编译由一把异步锁排队（P30 ～ P32）。集合文件里的要求指令仍不是指令：手册的两页说的都是 Profile 的行。
17. **API 的连接收尾（新增，M5-D23）**：`rurge-api` 用自己的 `axum::serve::Listener`（`Lingering`），它交出的连接（`Closing`）被丢掉时由一个被跟踪的任务关写端、读掉晚到的字节（至多 1 秒、256 KiB；同时至多 64 条，满了的当场关）、再关；`serve` 的 future 等这些任务结束才完成，守护进程本来就在退出的宽限里等它。对端地址改由本 crate 的 `Peer` 带给鉴权一层（axum 只为 `TcpListener` 实现了 `ConnectInfo<SocketAddr>`）。请求怎样被读、鉴权与应答不变（P29）。
18. **需求复读（新增，M5-D25）**：阶段 1 的每一条功能需求逐句对着手册的原句、代码与一条会因它而红的用例读一遍，产出 `docs/acceptance/phase1-acceptance.md`；小的缺口当场补，要设计的登记；"阶段 1 功能齐备"写不写由它的结论定（P33）。
19. **任务（订正第 11 节）**：M5b 是 14 个任务（0 ～ 13）——`FINAL`；资源管理器两个（种类、状态与磁盘上的副本；检查、可等待的更新、采用与撤销）、加载器两个、引擎的暂停、`ProfileSources` 两个（跟踪；判定与引导）、可执行文件、两个端点；集合文件里没有用上的行；API 的连接收尾；需求复读；文档（P27）。

## 14. 实施期的订正（M5b）

M5b 的执行（计划 `docs/superpowers/plans/2026-10-11-phase1-m5b-profile-remote-sources-plan.md`，2026-10-11 ～ 13）改了本文几处的决定，也让几句不再成立。第 1 ～ 13 节不改，在这里逐条订正；以代码为准，仍然成立的句子不列。每条的来龙去脉在计划末尾的「执行期修正记录」与「延后事项」两张表里（下面写"记录第 N 行""延后第 N 行"）。

1. **第 1 节、第 2 节、第 11 节、第 13 节第 19 条：M5b 不止 14 个任务。** 14 个任务（任务 0 ～ 13）之外，执行中又新加了三项：任务 4b（下载到的文本能让 rurge 对本机做什么，第 2 条）、任务 14（只有系统知道的名字交系统解析库，第 22 条）、任务 15（`POST /v1/profiles/check` 照手册回答，第 23 条），另有一项不属任何任务的改动（URL 里的用户名口令作 Basic 认证发出，第 26 条）。后两个任务来自需求复读（M5-D25，第 20 条）：复读查出的缺口里，优先级 P0、一个 crate 里补得上的，当场补，不登记（记录第 1、111、115 行）。

2. **第 3 节 M5-D8、M5-D20，第 7.2 节：下载到的文本不因为被 include 而多出权力（新决定，代项目所有者作的裁定）。** 本文与计划把托管配置的副本 include 的本地文件当作使用者自己的文本：其中的 `external` 策略照建，本地的集合与 `policy-path` 照读。这不成立：rurge 把下载的内容放在 `<数据目录>/resources/<链接的 SHA-256>/data`，这个名字任何人都能从链接算出来；数据目录在 Profile 所在的目录之下时（Profile 放在数据目录里，或 `--data-dir ./data`），一份托管配置能让 rurge 先把服务商选定的文本下载下来（当集合或远程 include），在之后的某次更新里再用一条朴素的相对路径 include 它被存成的那个文件——服务商的文本被当成本地文本读，其中的 `external` 策略会启动程序。所以每一行的触及范围由它的来源决定（`rurge_config::Reach`）：`Anywhere`——本地 Profile 与它 include 的本地文件（`Origin::Main`、`Origin::Include`）——一切照旧；`BelowProfile`——托管配置的副本与它所 include 的每一个本地文件（`Origin::Managed`、`Origin::ManagedInclude`）——只能用朴素的相对路径 include 一个至多 64 MiB 的普通文件，不点名本地集合、不点名本地 `policy-path`、不启动 `external` 程序；`Nowhere`——其余下载到的文本（`Origin::Remote`、`Origin::Module`、下载到的集合文件）——不点名本地文件、不启动任何东西。被拒的引用是一条告警（`W0033`，或沿用内联集合与下载到的集合文件的 `W0018` / `W0023` 的原因），那一行丢掉，Profile 照常加载。M5-D8 因此收窄（副本的本地 include 只剩朴素的相对路径），M5-D20 放宽（不止远程 include，副本 include 的本地文件也不建 `external`）。代价：托管配置不能从它 include 的本地文件启动 `external` 策略，也不能点名本地的集合与 `policy-path` 文件（手册没有这样的限制，按手册的写法两样都行；没有在 Surge 上核对）；要用的人写一份不带 `#!MANAGED-CONFIG` 的本地 Profile，用 `#!include <URL>` 取下载的部分；被拒的 `policy-path` 让组没有导入的成员，组里没有别的成员时是空组，而空组缺省走 DIRECT（`--empty-group-reject` 把它改成拒绝），加载时的 `W0033` 是唯一的信号。放开它的命令行开关由项目所有者定（记录第 36、37 行；延后第 8、24 行）。

3. **第 7.2 节：`Reach` 在哪里问、怎样读文件、什么是朴素的相对路径。** 问的地方五处，每处都在碰文件系统之前：`#!include`（`Reach::include` 同时回答"读不读"与"当作什么读"——`TakenIn::AsOwnText` / `AsTheCopy`——，读文件与定行的来源都是对它的穷尽匹配，以后加一种触及范围而不做决定就编不过）；规则、`[Host]` 的集合键、内联集合与集合文件的行点名的集合（`ResourceRef::parse_external`，造出 `ResourceRef::File` 的唯一一处）；`policy-path`（`spec/group.rs`，造出 `PolicyPath::File` 的唯一一处）；`external`（`NotImplemented::external_in`）；集合注册表（给下载到的集合文件 `Nowhere`：第一次编译、陈旧的副本、每次重载）。副本 include 的文件是 `Origin::ManagedInclude`，先用 `metadata` 确认是普通文件，才经上限 64 MiB + 1 字节的读取器读，目录、设备、更大的文件是 `E0002`（`not a regular file` / `larger than 64 MiB`）。加载器里两张按名字取值的表（每个组的文本的触及范围、每条 `external` 策略的文本的触及范围）对没有的名字默认 `Nowhere`。"朴素的相对路径"按文本定义，各平台相同：没有空段、没有 `..`、没有以点或空格结尾的段（单独的 `.` 除外）、没有 `"` `*` `:` `<` `>` `?` `|` 与控制字符、没有 Windows 的设备名（`CON`、`PRN`、`AUX`、`NUL`、`CONIN$`、`CONOUT$`、`COM0` ～ `COM9`、`COM¹²³`、`LPT0` ～ `LPT9`、`LPT¹²³`，带不带扩展名都算）。没有验证到或没有限住的：只在 Unix 上编译的符号链接用例没有编译过；路径仍可能穿过目录里早已存在的符号链接；64 MiB 只限一个文件读多少，加载留着的是每行一项（记录第 38 ～ 41 行；延后第 31、32、36、37 行）。

4. **第 7.2 节、第 13 节第 5 条：副本怎样生效，诊断放哪里。** 副本的行的来源是 `Origin::Managed`，不是 `Origin::Remote`（上一条：两种下载到的文本权力不同）。"至多 8 跳"是跟着走至多 8 次换地址、读至多 9 份副本（十个地址 U0 ～ U9：读 U0 ～ U8 的副本，U9 从不问）。本地文件的修改时间读不出来时副本生效（本地文件只有确实更新时才生效），副本的检查时刻在将来（时钟被拨回过）时本地文件不生效，直到时钟走过那个时刻；本地文件读不了而缓存里有副本时是 `LoadError::Io`，不退到副本——本地文件是 Profile 的锚（它的那一行、它的目录、它的时间）。链上换地址之前经过的地址（`RemoteRole::ManagedHop`）取缺省间隔 86 400 秒、`strict` 为假。诊断都放在本地文件的 `#!MANAGED-CONFIG` 那一行；两句 `W0034` 只在副本生效时给，第二条 `I0004` 在副本生效而链走过不止一个地址时给，`I0004` 里的缓存文件路径是数据目录加哈希出来的名字，不含链接的任何部分。副本生效时本地文件自己的文本的诊断（包括错误）都不出现，代价是副本生效期间本地文件里的笔误看不到。这一条里计划没有说的几处——修改时间读不出来、检查时刻在将来、本地文件读不了、诊断放在哪里——是代项目所有者作的裁定，各自的代价写在记录的那几行（记录第 28 ～ 32、34 行）。

5. **第 7.2 节：链接的解析、`ConfigSummary.managed`、`E0010` 的建议。** 加载器解析链接时去掉片段（片段从不发给服务端），同一个链接的两种写法是一个来源，经片段的环按来源判；一次加载里每个链接只有一个答案（`remote.rs` 的 `Memo` 包住来源，托管配置的那一步与 include 的展开共用它）；`rurge_config::remote::origin`（给出 `scheme://host[:port]/…`）与 `DEFAULT_INTERVAL` 各只有一份，`rurge-engine` 与 `rurge-net` 都用。`W0011` 只剩"没有数据目录"一种，既读不成链接也读不成路径的 include 目标是 `W0012`；缺节的 `W0017` 在远程 include 一处有自己的建议。下载到的 `external` 行不调 `to_spec`、不参加两处 `local-port` 的检查，代价是这样一行里的参数错误在它被挪进本地 Profile 之前不报。`ConfigSummary.managed` 只显示 `scheme://host[:port]/…`（服务商发的链接整个是凭据），`kitchen-sink` 的快照因此变了一行（代项目所有者作的裁定）。只有一行 `#!MANAGED-CONFIG` 的本地文件在副本还没下载时得到 `W0022` 加 `E0010`，而 `E0010` 的通用建议（加一条 `FINAL,DIRECT`）会把人带错——照做本地文件就生效、一切走 DIRECT——，所以这一处的建议是 ``the rules come with the managed profile, which has not been downloaded yet``——代项目所有者作的裁定，代价是这一句说的是原因而不是做法（记录第 19 ～ 24、33、35 行）。

6. **第 13 节第 15 条（M5-D22）；M2 设计 4.2、6.5：`FINAL` 的落地。** 匹配在第一条启用的 `FINAL` 处结束（`Config::first_final`），策略与选项取最后一条的（`Config::effective_final`），读法是从手册三句话推断的，没有在 Surge 上观察过（延后第 22 行）。`RuleEngine::build` 编译第一条 `FINAL` 之前的规则，再接上最后一条 `FINAL` 作最后一条规则；`GET /v1/rules` 与启动行 `N policies, M rules` 数的是编译后的表。`W0019` 数第一条 `FINAL` 之后不是 `FINAL` 的规则，夹在两条 `FINAL` 之间的也算；`W0021` 的文本是 `this FINAL does not give the policy; the last FINAL rule does`。集合注册表只登记第一条 `FINAL` 之前的规则引用的集合：之后的不读、不下载、不给 `W0022`、不被启动时等待（`919ee16`）；一个 URL 或文件的角色（`RULE-SET` 还是 `DOMAIN-SET`，`claims_of`）仍读全部规则，与加载器的 `E0024` 一致；加载器的检查（`E0007`、`W0009`、`W0010`、`W0013`、`E0024`）与 `Config::summary()` 仍读写下的每一条规则，所以第一条 `FINAL` 之后一条规则里的未知策略仍是 `E0007`；一个 URL 既被只在第一条 `FINAL` 之后的规则带着 `update-interval` 引用、又被 `[Host]` 的集合键引用时，取 `[Host]` 项的缺省间隔。M2 设计第 17 节有对应的订正（记录第 3 ～ 9 行；延后第 38 行）。

7. **第 7.1 节、第 13 节第 1 条：资源管理器的行为（与本文和计划的出入）。** (a) 一个带检查的条目，检查通过而缓存写不进去（数据目录写满、只读）：这次尝试按失败算（`cannot write the cache: <io 错误>`），什么也不发布，版本与校验器不动，照常退避——加载器读 Profile 的来源读的是磁盘上的缓存，不是内存——；代价是这样的使用者永远等不到托管配置的更新（状态里写着原因）。不带检查的条目缓存写不进去（早于本计划的缺陷：内存里是新校验器、磁盘上是旧正文，重启之后旧正文对服务端永远是"没有变化"）：新正文仍从内存里用，条目记住"正文不在磁盘上"，下一次成功的检查（304 或没变的 200）把正文和元数据一并写下，规则集、订阅等已有的资源因此也变好了；`meta.json` 写失败与正文写失败一样记下、一样记住。(b) `adopt()` 对本地文件来源是 `InvalidInput`、什么也不变；`get_checked` 对本地文件来源等同 `get_labelled`；与条目手里相同的正文不升版本。(c) `retire()` 返回"这次调用是不是撤掉了它"，也结束本地文件条目的任务，在途的尝试只被 `retire` 打断（管理器没了不打断）；7.3 的"来源集合变小时重建管理器"不做，用 `retire`（计划期决定 P4）。(d) `updating` 在宣布新版本之前、在一次尝试算结束之前放下。(e) URL 条目的任务与等待者的状态放在一条 `watch` 通道里（`Progress`：计数与标志，用 `wait_for` 的谓词读），不再用 `Notify` 的"踢一下"；每个条目一把锁，护住该条目的缓存读写、元数据与状态的更新，`statuses()`、`register`、`force_update`、`wait_initial` 不拿跨越读写的锁；订阅者不得握着从 `ResourceHandle::subscribe()` 借出的值。(f) 检查没通过时的日志是 WARN `downloaded resource not adopted`，不是 `resource fetch failed`（那句把读的人引向网络）。(g) HTTP 客户端的 `unsupported scheme` 失败文本不再引用方案名（重定向的 `Location` 由服务端说了算）：失败文本不引用 URL 的任何部分，这条规矩不留例外；失败的本地文件资源的 `fetched_at` 仍是 `None`；没有标签登记的 URL 来源在日志里带路径与查询串（对规则集是有意的），Profile 的来源一律给标签（记录第 10 ～ 18 行）。

8. **第 7.1 节（新）：缓存条目带正文的摘要——缓存可以丢，不许撒谎。** 有校验器的条目，每个请求（包括强制的、引导的 `Refresh`）都是条件请求，304 只写 `meta.json`，而什么也不 fsync：一份正文若已不是那个校验器当初对应的正文，只要服务端不换版本，就没有请求会修它；对托管配置，那个条目就是 Profile。所以 `meta.json` 多 `sha256`（正文的 SHA-256，小写十六进制；旧缓存里没有或为 `null` 的照读，在它第一次"没有变化"的应答时补上；任何别的写法——大写、被截短、空——都不是这份正文的）；`CacheDir::store(data, &mut Meta)` 记它，`CacheDir::load` 在正文对不上时给 `None`：这样的一对文件不是条目，登记时不带校验器、服务端送整份正文，`cached` / `cached_copy` 找不到；"没有变化"的应答，或手里这份正文又被送来一次，除非磁盘上完好地存着元数据所说的正文（`CacheDir::holds`），否则从内存里把正文再写一遍；对不上时一行 DEBUG，按缓存键称呼而不是 URL；`Meta` 的 `Debug` 只说有没有摘要。不做 fsync：断电之后在系统写回窗口内的下载会丢，认得出来、重新下载，只有指针的 Profile 因此那次启动需要网络。同一个条目被两方同时写（`write_atomic` 里每个文件固定的临时名）认得出来，没有防住。代项目所有者作的裁定；代价：每个缓存的正文在登记、每次离线读、每次没有新东西的应答时多算一次 SHA-256，没有对一份大的 GeoIP 库量过；手工改过的缓存正文被重新下载（记录第 67 行；延后第 30、52 ～ 54 行）。

9. **第 7.3 节：来源的登记与 `strict()` 的边角。** 本文说一个来源登记时从管理器手里有的版本算起；实际从这一代读到的版本算起（`built_from`）：加载拿到文本时是第一个版本，没拿到时是"没有"——加载与登记之间到的副本是一次要报告的变化——，再次登记时刚发布的版本带过去；代价是偶有的竞态里多一次内容相同的重载。`strict()` 对"到期而没试过"给 `None`、对离线给 `offline mode: the update cannot be attempted`；`interval` 写成任意 u64 秒都能加载，加到时刻上只用 `checked_add`，加不上的永远不到期；`outdated()` 只看链头的那次检查，可能晚、不会早（同一时刻副本胜出，与加载器相同）。再次登记会忘掉旧条目失败过：暂停中的 `strict` Profile 要到新条目的第一次尝试失败才再暂停（记录第 47、48 行；延后第 42 行）。

10. **第 7.3 节、第 13 节第 4 条与第 7 条：采用之前的判定怎样做。** 判定是对"本地 Profile 加一层覆盖"的一次 `load_checked`：覆盖里有手里这份正文、留着的各版本、以及放行了而还没写盘的正文；三种结论——采用、不采用（来源显示 `failed` 与理由）、等待；"还没试过"在覆盖层里判，不从加载之后的状态判。两把锁，次序是判定锁（只在阻塞线程池上持有，跨越整个加载）、来源的状态、管理器的锁；`track`、`changed`、`strict`、`outdated`、`statuses`、`update`、`remote().get` 从不取判定锁，检查只持有来源的 `Weak`。一个版本提到从没下载过的文件时，那个文件被登记为临时的来源并等它试完第一次；`track` 在还留着版本时不撤它，一次读了 Profile 的判定把没有人再提的放掉。每个地址至多留最新的一份、只在内存里；缺它被 include 的节的正文不留（它不是任何一次变化的一半）；一起才加载得了的几份一起采用，`changed()` 只报一次。因等待而留着的版本，在它等的来源试完第一次时就再判（`judge_again_when_tried`），不等那个来源的下一次重试；`settle` 末尾把留着的一起判，不过再扣掉被拒的、只判等着的那些。不通过的理由一律不含路径、链接至多 `scheme://host[:port]/…`——本地文件读不了时是 `does not load: the local profile cannot be read (<io::ErrorKind>)`。引导的"一轮什么也没改变"把来源的增减也算进去，否则一轮 `Refresh` 把副本修好、而修好的副本又提到一个新文件时，会带着旧的错误结束。一个地址的副本放在哪里只有一个定义（`rurge_net::resource::cache_file`）。判定的几个已知界限写在 `Verdicts` 上（延后第 48 行）（记录第 54 ～ 59、61、62 行；延后第 45 ～ 50 行）。

11. **第 7.3 节（设计没有）：向其余来源问更新。** 一份留着的版本加载不了时，向它那几次加载提到的其他被跟踪的来源各问一次更新（`ResourceManager::force_update`）：全部地址合起来至多一分钟一次（`ASK_EVERY`），为同一个地址至多五次（`ASK_TIMES`，从第一次有这个地址的东西被留着起数，换一份别的坏正文不重新数），加上管理器的退避，五次落在第 0、1、3、7、15 分钟；因等待而留着的不问，那几次加载没提到别的来源时（该是托管配置的地方来了一页登录页）也不问。为什么：被 include 的文件缺省一天才问一次，先到的那一半可能要等一天——托管来源 `failed`，`strict` 的 Profile 一直拒绝会话。封顶用一个次数而不是一个时间：次数只在该地址没有东西留着时重新开始，一个一直坏着的地址只被问这五次；跨地址的一分钟一次，免得几个地址同时坏时一分钟里问出一串。代项目所有者作的裁定；代价：一个在"能加载"与"不能加载"之间来回的服务端，可以让其余来源一分钟被问一次，只要它这样做；使用者自己的本地文件里的错误——没有下载修得好它——也会让其余来源被问（记录第 60 行；延后第 47 行）。

12. **第 7.4 节：暂停。** 暂停只在内存里（不进 `state.json`），重载不动它，重启之后由 `strict` 的判断重新定；`Engine::suspend` 自己不记日志（决定的那一方记）；会话被拒的原因 `why` 只进日志与 API，不进任何会话的记录、错误页或日志行。被拒的四个入口都在会话的句柄建好之后的第一个语句里问，在规则、任何查找、任何组之前；UDP 以"失败"而不是"拒绝"结束：失败的流 5 秒后重试，暂停一结束目标就恢复，代价是暂停期间每个活跃的 UDP 目标每 5 秒一条失败记录。内部 DNS 会话与任何会话一样被拒（代项目所有者作的裁定），于是 7.4 的"测速不受影响"只对机制成立：开了 `encrypted-dns-follow-outbound-mode` 又没有明文 UDP 的 DNS 服务器时，策略的服务器**名**的解析是内部会话，暂停期间被拒，以域名配置服务器的策略的测速在暂停结束前失败；一次所有服务器都被拒的查找要走完解析器的整个时间表（缺省 5 秒）才结束，并留下每轮重发每台服务器一条失败的内部记录；DoH 服务器的错误文本是 HTTP 客户端的（`… client error (Connect)`），那句话只在内部会话的记录与日志里。结束暂停的更新本身不受影响（资源客户端用系统的解析器、直连）；不受影响的还有订阅、规则集、GeoIP 与 Profile 自己的下载；自动组的测试轮次由拨号触发，暂停期间没有拨号，所以没有新的一轮开始。`Engine::config_text` 读 `SourceInfo.main_text`，副本生效时 `GET /v1/profiles/current` 返回副本；缓存里的副本被删掉之后它答 500 ``cannot read the profile: <系统的错误文本>``（文本里没有路径），直到下一次成功的更新把副本写回（记录第 42 ～ 46 行；延后第 13、39 ～ 41 行）。

13. **第 7.4 节、第 7.5 节：启动时的规则与主循环。** 一份到期而还没试过的 `strict` 来源，`strict()` 给 `None`；只看它，引擎会在这样的 Profile 上一开始就放行会话。所以监听端绑定之前，`strict()` 给出原因**或** `strict_due()`（`strict` 的托管配置被跟踪、它的更新已到期，算法同 `strict()`，不管尝试怎么样了）为真时，引擎是暂停的：原因取等待结束时的那一个，没有就是 `the update has not finished`。代价：服务端挂着不答时，一份到期的 `strict` Profile 从第一个会话起就被拒，直到一次尝试有了结果（代项目所有者作的裁定）。主循环每秒一眼，判断在纯函数 `what_the_sources_ask(strict, due, refusing)` 里（`follow_the_sources` 用它）：有原因→拒绝（只在引擎本来在放行时记 WARN，原因变了只悄悄换）；没有原因而到期→什么也不做；没有原因也没到期而引擎在拒绝→放行并记 INFO。结束拒绝的那一行是 INFO `managed profile: new sessions are accepted again`，不是 7.4 与计划写的 `updated; …`：拒绝也会因别的原因结束（使用者把本地文件那一行的 `strict` 去掉或把间隔拉长再重载、时钟被拨回、没有检查记录而本地文件的时间读不出来），那时什么也没更新。这一眼读管理器在内存里的状态和 Profile 文件的修改时间（三次 `stat`），从不读缓存里的正文，是在主循环的线程上做的；没有远程来源的守护进程也这样看（记录第 63 ～ 65 行；延后第 55、57 行）。

14. **第 7.5 节：启动的次序、锁与其余命令。** 引导在日志之前（日志的级别要取自最后加载到的 Profile），但先建数据目录、拿数据目录的实例锁——下载要写缓存，不能与另一个实例同时写——，这把锁一直拿到进程结束；被别的实例占着时以 1 退出、什么也不下载，也不碰占着它的实例的系统代理备份与 `state.json`。等待在一个自己的临时单线程运行时里做，开始时往标准错误写一行 `waiting for the profile's remote sources (at most 60 s)`，主运行时建起之前收掉；只打印最后一次加载的诊断，只有它套监听覆盖。引导之后仍加载不了、套监听覆盖失败、`--log-file` 打不开：各自恢复崩溃留下的系统代理（一次），以 2 退出；锁已经拿到，所以共用"拿到锁之后"的那一半（`restore_what_a_crash_left`）。数据目录根本锁不了（`Lock::Taken(None)`）时启动照常、什么也不恢复，日志就绪后一条 WARN。是否引导由 `boot_need` 定，逻辑在 `rurge-engine::profile`，可执行文件只接线。`check`、`rule match`、`dns lookup` 与 `reload` / `stop` / `status -c` 都读磁盘视图（`disk_remote(数据目录)`），从不为 Profile 联网，读 Profile 时不创建、不写数据目录（`rule match` 与 `dns lookup` 读完之后建栈，`RuntimeArgs::resolve` 那时照旧创建数据目录）；`reload` / `stop` / `status -c` 为此加了 `--data-dir`（环境变量 `RURGE_DATA_DIR`，缺省是平台的数据目录）；守护进程用 `sources.remote()`，没有副本的来源另带上最近一次尝试失败的原因，所以 `W0022` 说得出原因，重载与 API 的 `profiles/check` 读到的与守护进程一致。第一次加载的诊断在等待的路径上不打印；启动时的 `Refresh` 失败，使用者只看到副本的错误（记录第 66 行；延后第 51、56 行）。

15. **第 7.6 节：两个端点与本文的出入。** 视图与更新在 `rurge-engine`（`Engine::external_resources`、`Engine::update_external_resources`，类型 `ExternalResource`），路由只负责序列化——`rurge-api` 够不到 `ResourceManager`，除非新依赖 `rurge-net`。应答的形状有几处是手册没有的（代项目所有者作的裁定）：`type` 多三种（`geoip`、`profile`、`include`）；托管配置那一项在引擎暂停时有 `suspended: true`；`error` 里除了自己失败的更新，还说会话为什么被拒（第一次尝试结束之前是等待结束时的原因或 `the update has not finished`，离线时是 `offline mode: the update cannot be attempted`，自己失败的更新排在前面；更新成功之后至多一秒内这一项仍带 `suspended` 与原因，直到主循环的下一眼）；规则集的链接以路径与查询串列出（与 `GET /v1/rules`、日志里一样，只去掉用户名口令）；列表里 Profile 的来源排在最前，再是当前这一代的资源按 `path`、再按 `key`；`ready` 是手里有内容，`updatedAt` 是最近一次成功的检查（"没有变化"的应答也算）；`key` 就是缓存键（`resource_key`，来源的 SHA-256），在列表里不唯一（一个地址既是 Profile 的来源又是集合给出两项、一个键）。更新：不认识的键是 400，文字固定、不引用请求；整个请求至多 60 秒，到时还没结束的键是 `"timed out"`，别的键保留各自的答案；`all` 包括托管配置与两个 GeoIP 库，每个地址只更新一次，下载同时发出（不设并发上限，与冷启动相同）；指名一个本地资源：重读，`success`；离线：每个 URL 资源都是错误；新版本因等它提到的来源而没有立刻采用时，那个键的结果是等待的那一句。请求开始时取下当时那一代的管理器，其间的重载不让等着的键以"管理器没了"收场；它不等集合的重新编译，也不等托管配置更新之后的重载。暂停时两个端点照常。主机文件不是外部资源，不列、不能按键更新（记录第 70、71 行；延后第 59 ～ 63 行）。

16. **第 7.6 节（设计没有）：换代时补问。** 一个管理器只在建条目时读一次缓存；一个更新请求等着时来了重载（托管配置变了、一秒后重载，而 `all` 里的集合还在下载），旧一代之后落下的下载就在缓存里而不在用，请求却已答 `success`，要到资源自己的下一次刷新（缺省一天）。所以 `Engine::swap_runtime` 以 `catch_up(旧的管理器, 新的管理器)` 收尾：新一代里每个不是本地文件又没有在下载的资源，只要旧一代在新一代读缓存之后成功检查过它（`fetched_at` 更晚）、或此刻正在检查它（`updating`），就再问一次（`ResourceManager::force_update`，不等）；比的是时刻不是内容，旧一代的检查什么也没带来时多一次条件请求；换代中间什么也没检查过的，什么也不问。第二眼：更新请求在另一代下结束时，用同一个 `catch_up(它问的那个管理器, 在用的)`——请求开始时取管理器、过一会儿才去问，其间换代，旧管理器上还什么都没问，键却答 `success` 而在用的一代是旧内容。补问放在换代处，不放在请求里的第二遍，因为请求里的第二遍仍会错过"请求在换代前结束、而下载恰在下一代建起来时落下"的情形。代项目所有者作的裁定（每次重载都多这一步，为的是关上一个错的 `success`）。够不到的（写在 `catch_up` 与 `update_external_resources` 的文档注释里）：新一代已经在下载的资源，留给那次尝试，它可能在服务端变之前就发出了；被丢掉的请求（客户端走了）结束时不看；比较走墙上时钟，时钟被拨回后的检查可能错过；没有人在等的尝试（旧一代的定时器触发的）在换代处有两个微秒级的窗口（记录第 72、73 行；延后第 30、59 行）。

17. **第 3 节 M5-D24，第 13 节第 16 条：集合文件里没有用上的行的落地。** 数的是带着不认识的参数的行，按集合计数，一行里有几个参数也只算一行，嵌套的子规则里的同样算（`ParsedSet.unknown_params`、`first_unknown`；行号从 1 起，注释行与空行也算行，BOM 不算）；被跳过的行算被跳过的，不再算它带着什么，超过一百万条上限的行只算被截掉的；在下载到的文本里，点名本地文件的行被跳过。建注册表时每个集合一条 `W0003`——``set `<name>`: <n> line(s) carry a parameter that is not known and is ignored (first: line <N>)``——，只有个数与行号，从不引用参数：集合文件是下载到的任意文本，给错的可能是一条带口令的代理行。启动之后载入的集合，三个数（`skipped`、`truncated`、`unknown_params`）不全为零且与上一次载入的不同时才一行 WARN `set loaded with lines that are not used`（字段 `set`、三个数，有的话还有 `first_skipped_line`、`first_unknown_line`；`worth_a_warning` 是纯函数）：数没变的刷新不再说一遍——一份每天刷新、总有三行读不了的列表不该每天一行 WARN——，数先落到零又回来时再说；这一行写在集合入库之前，所以等集合版本前进的人要么看到它、要么知道没有。三个数经 `UnusedLines` 进 `GET /v1/external_resources` 里 `ruleset` 与 `domainset` 的项（`skipped`、`truncated`、`unknownParams`），别的种类没有，`error` 不因它们出现——一份有几行读不了的集合是载入了的；它们是集合上一次编译所依据的内容的数，更新之后到集合轮到编译之前仍是更新之前内容的数；`SetStatus.key` 把集合的状态接到资源的那一项上。同一个注册表的重编译由一把异步锁（`compile_lock`）排队，一次一个：重载任务等锁，再看自己的注册表是否还在，再读资源管理器手里的（可用状态的副本，以及失败状态留着的那份），跳过编译过的版本，编译并存入，锁里没有 `await`；`SetRegistry::build` 持锁直到每个集合都有了条目，建的时候到的集合在建完之后编译。锁在读状态之前取，免得排队期间一个新版本被一次失败的刷新盖过而漏掉。代项目所有者作的裁定；代价：一起变的集合一个接一个编译，内存的峰值是一次编译而不是同时变的那么多份，多核机器上时间是各份之和而不是最长的一份。加载器的三处 `W0003`（未知的规则参数、子规则参数、`[Ruleset <name>]` 的参数）只点名 `=` 之前的部分，且只在它是 1 ～ 32 个小写 ASCII 字母和连字符时，否则是 `unknown rule parameter ignored`；缺省建议改为 ``check the parameter's spelling and its value against the Surge manual``，因为一个已知的参数而取值用不了（`update-interval=1d`）报的也是这个码（记录第 75、76、80 行；延后第 21、64 ～ 71 行）。

18. **第 3 节 M5-D24、第 7.5 节：带 `--wait` 的命令等集合编译完。** `rurge rule match`、`rurge dns lookup`、`rurge dns cache` 建栈时带一个等待（`--wait`，缺省 30 秒）：先等下载，再多等两秒等可能不来的东西（还没到的集合、正在装的库）；之后只要还有集合没编译它资源手里的版本（`sets_behind`：键相同、资源的版本高于集合的），就最多再等一个 `--wait`，从下载结束算起；最坏是两倍的 `--wait`，`--wait 1` 时第二段可以到两秒（一个根本没载入的集合也按"可能不来"等）。时间到了而仍有集合落在后面时，栈的诊断里多一条 `W0022`（没有建议）：``<n> set(s) that arrived were still waiting to be compiled when the wait ended: until they are, they match as they did before, and a set that is new matches nothing``，三个命令像别的诊断一样把它打在标准错误上、并进 `--json` 的 `warnings`，答案照常、退出码不变；只有个数，没有名字与链接。它是诊断而不是日志行，因为三个命令都没有显示日志行的东西（`rule match` 与 `dns cache` 没有装订阅器，`dns lookup --trace` 只放过解析器自己的 debug 行），而会写日志的守护进程传零等待、不跑这一段、从不说这句话。代项目所有者作的裁定（宁可晚回答，不回答仍是空的集合）；代价：这几个命令最慢要多等一倍（记录第 77 ～ 79 行；延后第 66、67 行）。

19. **第 3 节 M5-D23，第 13 节第 17 条：API 连接收尾的落地。** `crates/rurge-api/src/linger.rs`：API 的监听器是 `Lingering`（包住 `TcpListener`），它交出的连接是 `Closing`；hyper 放手一条连接时（答完并决定关、对端先关、出错、优雅退出），连接交给一个被跟踪的任务：关写端（结果忽略：hyper 在它按常规结束的连接上已经关过，第二次关在有的系统上会失败），再读掉并丢弃对端还在发的字节，直到对端关闭、读出错、丢弃满 `LINGER_BYTES`（256 KiB）、或从任务拿到连接起过了 `LINGER_TIME`（1 秒；一个期限管整件事，不是每次读一个），然后关；同时在这个任务里的至多 `LINGER_MAX`（64）条，满了的、以及在没有运行时的线程上被丢掉的，当场关。放在监听器一层，因为每条连接都从这里过——不读正文的端点、四种在读正文之前就被拒绝的请求（`401`、`403`、`404`、`405`）和以后新加的端点——，请求怎样被读、鉴权与应答一概不动；先关写端，因为这样不论后面怎样，对端都读得到它的应答的结尾。三个数的来由：256 KiB 是 Go 的 `net/http` 替处理函数读掉的上限；1 秒是 Go 的 500 毫秒的两倍、nginx 的 `lingering_timeout`（5 秒）的五分之一，因为它同时是一次停止可能多等的时间；64 让一个不停连进来又不关的对端最多占住 64 个描述符各 1 秒。一条连接从 `accept` 起就被计数，不是从被放手起：axum 0.8.9 里连接的任务在丢掉连接前一条语句就发"我完了"的信号，而优雅退出只等那个信号，从放手起算的话服务端可能在 `POST /v1/stop` 那条连接还没交出去时就看到计数为零，进程带着那个客户端没读的应答结束；`serve` 的 future 等这些任务都结束才完成，守护进程本来就在退出的 5 秒宽限里等它，所以退出至多因此晚 `LINGER_TIME`。`router` 现在只在 crate 里可见；对端地址改由本 crate 的 `Peer` 带给鉴权一层。没有做的：不设 socket 选项、不记每条连接的日志（唯一新增的一行是 `error` 级的 `http-api: cannot accept a connection; trying again in a second`）、收尾期间读到的字节不解析不保存。没有覆盖的：超过 `LINGER_BYTES` 之后才读应答的客户端仍可能丢应答；64 个位置由所有对端共用；一个不关连接的空闲客户端使每次停止多等至多一秒；Linux 与 macOS 没有量过（一台 Windows 11 x86-64 的开发机上：边发 300 KiB 边读，50 次里 50 次得到应答；发 8 MiB 之后 1.5 秒才读，10 次里 10 次一个 reset）。`POST /v1/profiles/check` 在任务 15 之后读正文，不读正文的端点从四个变成三个（记录第 81 ～ 87 行；延后第 20、23、72 ～ 74 行）。

20. **第 3 节 M5-D25，第 13 节第 18 条：需求复读的落地。** 读的是 `docs/requirements.md` 第 4 节里阶段栏含 1 的 49 行需求（其中一行写"各阶段"，阶段栏写着几个阶段的只读它的阶段 1 部分），逐句一行，共 416 句，产出 `docs/acceptance/phase1-acceptance.md`。结论有七种，比计划写的四种多：做了（238）、做了而补了用例（75）、补上了（9）、没有做完而已登记（3）、有意不同而已登记（14）、不属于阶段 1（37）、需求文字有问题（40）。"补上了"与"没有做完"拆自"只做了一部分 / 只解析 / 没有做"，"有意不同"是本文没有预设的一类，"不属于阶段 1"与"需求文字有问题"拆自同一种，因为后者要项目所有者定：一句话的字面、或它所在的需求行的阶段栏把它放进后面的阶段、或它说的是项目怎样做事的，是前者；阶段栏给了阶段 1 而兼容性清单或设计把它登记得更晚、或它的话与手册或另一条需求不符的，是后者。FR-OBS-09 的两句（Windows 服务；崩溃后重启）照字面可以读成前者，仍记作后者，进项目所有者要读的那一张表：Windows 上 `rurge service install` 登记的是登录触发的计划任务，不是服务，崩溃的进程没有人重启，而阶段栏的"1 基础"没说这算不算基础（代项目所有者作的裁定）。说结果要把三个数一起说，缺一不可：3 句没有做完，都在优先级 P0 的需求行上（手册 IP 页的 `GEOIP,UNKNOWN` 与 `IP-ASN,UNKNOWN`、规则集文件行上的 `extended-matching`、大集合内存可控的判据）；37 句阶段栏给了阶段 1 而清单或设计登记在后面的阶段（优先级 P0 的需求行上 32 句、P1 的 5 句；其中 34 句今天不按需求的字面做，等阶段 3 ～ 6，另 3 句登记在阶段 2 而今天有代码）；14 句有意不同而已登记。托管配置的更新（FR-CFG-05）、远程 `#!include`（FR-CFG-04）与外部资源的手动更新（FR-CFG-14）这三处是本里程碑开工的原因，现在都做了。读的方法：一个读代码、读用例的断言、补用例的助手逐句读，每条结论另由一位评审者抽查（验收文档里也这么写）；判为"做了"的行指到一条断言了这一句的用例，这不等于别处没有缺口，有几行在自己的格子里写着一处限制；补的每条用例都做过改一行代码看它变红的验证。没有覆盖的：阶段 1 的手工验收清单（没有一行做过）、CI（没有绿过，所有用例只在一台 Windows 11 x86-64 的开发机上跑过）、阶段 2 的验收（没有完成）。需求文档没有改，改阶段栏还是改登记由项目所有者定（记录第 88 ～ 103、109、110 行；延后第 75 ～ 92 行）。

21. **第 3 节 M5-D25：读需求时当场补的修复，和测试不再够到本机的缺省目录。** 当场补的五处，每处先写用例看它红：`0863e52`（`include-device-communication` 是手册 `[General]` 页的第 58 个键，iOS 专属，原来当成未知键；代项目所有者作的裁定——它属于比项目基线更晚的手册版本，加它是因为"手册里的键解析并忽略、带正确的说明"是项目自己的规矩）、`ec80158`（`dns-server` 里同一个服务器写两遍或被 `system` 再带进来，只问一次）、`bffa339`（`encrypted-dns-server = off` 关掉加密 DNS，不再告警；代项目所有者作的裁定：与别的取值混写的几种形式手册没有说）、`ce9a5e9`（系统 hosts 文件开头的字节顺序标记不算进第一个地址）、`fcb4abc`（`HttpAuth` 的 `Debug` 不再带出口令：没有哪一行日志打过它，是一个潜伏的泄漏）。读完之后各用一个独立的改动补上的四处是第 22 ～ 25 条。为了可测的整理（`hosts_path_for`、`serve_checking`）行为不变。读的过程中有两次测试越出自己的范围：一次拨过一个文档地址（没有回应，没有发出或取回任何东西），一次建出了本机的缺省数据目录（一个空的锁文件和一个缺省值的 `state.json`，没有别的）。之后 `crates/rurge/tests/cli.rs` 里每一次起二进制都经 `rurge_command()` 与 `rurge_process()`，它们把缺省数据目录与配置目录所读的四个变量（`LOCALAPPDATA`、`APPDATA`、`XDG_DATA_HOME`、`XDG_CONFIG_HOME`）指到构建自己的临时目录下的一个草稿目录，并去掉 `RURGE_DATA_DIR` 与 `RURGE_LOG_FILE`（`HOME` 不动，所以在 macOS 上忘了 `--data-dir` 的用例仍够得到缺省目录）；夹具的 Profile 把规则里用到的名字在 `[Host]` 里钉到 127.0.0.1（记录第 90 ～ 98、108 行；延后第 93、94 行）。

22. **第 3 节 M5-D25（补）；M2 设计 7.2、7.5：只有系统知道的名字交系统解析库（任务 14）。** FR-DNS-02 的"以 `.local` 结尾的名字交系统解析库"与 FR-DNS-05 的 `[Host]` 值 `server:system`、`server:syslib`（以及等同的 `server:force-syslib`）一直写在各份设计与兼容性清单里，代码却先问系统的 DNS 服务器，只在一个服务器也不知道时才调库。手册（DNS Overview 的 Special Hostnames，Local DNS Mapping 的 System Resolution）：普通模式下这些都由系统解析库解析；只有增强模式（阶段 3）下 `system` / `syslib` 才是"转发到系统当前配置的 DNS 服务器"，`force-syslib` 仍用库。现在：`SystemDns` 多第五个方法 `library_lookup`（必须实现，没有缺省函数体，忘了实现的不会悄悄得到真的库；`NoSystemDns` 与测试里的 `SwapSystemDns` 答 `Unsupported`，`StaticSystemDns` 有 `library` 与 `library_asked`）；可执行文件一侧的 `PlatformSystemDns::library_lookup` 调 `tokio::net::lookup_host((name, 0))`，再经纯函数 `usable`（保持次序、每个地址一次、带作用域号的 IPv6 地址不要——`IpAddr` 带不了作用域，没有它拨不通）；解析器的私有方法 `Resolver::library_lookup` 的时限是重发间隔 × 轮数（缺省 5 秒），到时 `DnsError::Timeout`，库报错是 `DnsError::AllFailed`，地址按要的地址族裁剪、裁完没有是 `DnsError::EmptyAnswer`，应答生存期 60 秒、不进 rurge 的缓存、同一时刻同名的查找不合并（系统自己有缓存）。`[Host]` 的三个值（增强模式之前是同一件事）走库，来源 `Host(System)`；`.local` 名字走库，来源 `System`，系统的 DNS 服务器已知时、`dns-follow-interface` 下也一样；单标签名（追加第一个搜索域）与没有主上游时的普通名字，系统的 DNS 服务器已知则问它们，一个也不知道才调库。因为经 trait，M2 设计里写到 `tokio::net::lookup_host` 的句子只对可执行文件成立（M2 设计第 17 节有订正）。每次库查找结束一条 DEBUG 事件（目标 `rurge_dns::resolver`，`upstream = "system resolver library"`，不带名字与地址）。代项目所有者作的裁定：当场做而不登记；代价：这个任务的工作，增量的，可回退。真的解析库在真实网络上对 `.local` 名字做什么，在任何平台上都没有见过，尤其是由单播 DNS 服务器回答的 `.local` 区域；碰到的使用者在 `[Host]` 里写 `<区域> = server:<地址>`（记录第 111 ～ 114 行；延后第 98 ～ 102 行）。

23. **第 3 节 M5-D25（补）：`POST /v1/profiles/check` 照手册回答（任务 15）。** 手册（`tools/http-api.html`，Mac 4.0.6+）：请求 `{"name": "Profile2"}`，"If the profile is invalid, an error is returned. Otherwise, the `error` field is null."。应答原来没有 `error`，也不读请求的正文。现在：`CheckJson` 多第五个键 `error`，总是写出——Profile 没有严重程度为 error 的诊断时是 `null`（告警不算），否则是一个字符串：应答自己的 `diagnostics` 里第一条错误的 `Diagnostic::headline()`，还有别的错误时接 ` (and N more)`；这个措辞是 rurge 自己的，手册只说"an error is returned"。原有的四个键不变，读了且检查了的 Profile 一律 200，读不了的仍是 500。正文由处理函数自己读（`requested_name`，不用会自己作答的提取器——它的 400 会引用收到的东西）：没有正文、只有 JSON 空白、或没有 `name` 的对象，检查正在运行的那一份，别的键忽略；`name` 是字符串时，与正在运行的 Profile 的文件名、以及去掉最后一个扩展名的文件名比较（`names`，完全相等、不折叠大小写、从不当路径用），相符则检查，否则 404 `{"error":"profile not found"}` 且什么也没加载；不是 JSON、不是对象、`name` 不是字符串、或超过 2 MiB 的正文：400，一句固定的话 `the body must be a JSON object; "name", when given, must be a string`；两句都不重复请求里的任何东西，Content-Type 不看，400 与 404 都在加载之前给出。rurge 只有一份 Profile，所以一个不是它的名字被拒绝，而不是被忽略（忽略的话应答说的是一份没被问到的 Profile）；托管配置时名字是本地文件的；按 `name` 在几份 Profile 里选一份随 `GET /v1/profiles` 与 `POST /v1/profiles/switch` 在阶段 6。代项目所有者作的裁定：当场做而不登记；代价：同上一条。超过 2 MiB 一大截的发送者在 Windows 上可能丢掉 400，那是第 19 条的 256 KiB 界限，不是这个端点新有的（记录第 115 ～ 117 行；延后第 98、103、104 行）。

24. **第 3 节 M5-D25（补）；M2 设计 7.3：AAAA 抑制按查找计数。** 手册（DNS Overview，"A and AAAA Queries"）："If 5 consecutive lookups receive an A answer while the AAAA answer times out"，之后不再问 AAAA。M2 设计 7.3 的旁注记着现行计数与此不同：一次查找数了两次——它自己的查询一次，为同一次查找补问 AAAA 的后台查询再超时又一次——，AAAA 在第三个名字时就停了，并把它标作待定行为、留给项目所有者。现在（代项目所有者作的裁定）：一次查找数一次（私有的 `enum AaaaTimeout { Count, Ignore }`，交给 `record` 与 `spawn_refresh`）——这次查找自己的查询计数、过期条目的刷新计数（那是这次查找唯一的查询）、部分结果之后的补全不计，AAAA 应答不论在哪里到都清零——；并且一个超时按发出的那次查询数一次、由发出它的调用方数：同一时刻同名的查找共用一次查询（`query_coalesced`），原来每个都把那一次共享的超时数了一遍，一个浏览器对一台没查过的主机同时开五条连接、AAAA 应答慢过一秒一次，就让 AAAA 对整个进程停掉；现在 `query_coalesced` 返回结果和"是谁的查询"（私有的 `enum Query { Own, Another }`），只是等着的调用方仍把结果写进缓存、结果里有 AAAA 应答时仍清零，等的人在第一个调用方消失后接手、绕过缓存的调用方，发的是自己的查询，数。新鲜的缓存命中既不计也不清（TTL 内一个名字算一次）；`[Host]` 的 `server:` 行、`dns-follow-interface`、系统及库的回落走的查找根本不过这个计数。WARN 的文字改为 ``AAAA answers timed out in {n} lookups in a row; AAAA queries suppressed until the DNS cache is flushed``：原来的 `…until the next flush or network change` 许诺了不会发生的事——`Resolver::on_network_change` 也会解除抑制，但网络变化的检测（FR-IN-09，阶段 3）到来之前它没有调用方——，今天能让 AAAA 回来的是 DNS 缓存刷新（`POST /v1/dns/flush`）、Profile 的重载（重载建一个新解析器）和重启。为什么现在做：手册说的是连续五次查找，没有人论证过把一次查找数两次，而一处代码一条用例补得上的缺口是当场补的。代价：Surge 若数每个调用方，rurge 在一个从不回答 AAAA 的网络上、许多连接同时去一个新主机时比 Surge 晚停（五个不同的查询之后照样停）；Surge 怎样数同时发出的同名查找不知道。M2 设计第 17 节有对应的订正（记录第 104 ～ 106 行；延后第 87、95、96 行）。

25. **M2 设计 7.5（`[Host]` 映射）：键末尾的点。** 被查的名字已规范化（小写、去掉末尾的点），而键原来按写下的样子编译，所以 `Example.COM. = 127.0.0.2` 被接受、没有诊断，却什么名字也匹配不上。现在键最后一个标签之后的点不算进模式（`host.rs` 的私有 `pattern_text(key)`：去掉末尾的点——但剩下的是空、或以 `*`、`?` 结尾时按写下的原样）；`HostEntry.raw_key` 仍是使用者写的；集合键不动；没有诊断。键以通配符加点结尾（`*.`、`a?.`、`example.*.`）或只有点的，仍无诊断地加载并且什么也匹配不上（要告警得有新的诊断码）。**这是 rurge 自己的读法**：手册没有说键末尾的点是什么意思，Surge 怎样做不知道。第一条匹配仍然胜出，所以一条原来因末尾的点而作废的行，现在会在同名的后一行之前生效。代项目所有者作的裁定；代价：一份在同名的另一行之上写了带点的键的 Profile，现在得到上面那行的答案（记录第 107 行；延后第 90 行）。

26. **第 7.1 节（HTTP 客户端）：URL 里的用户名口令作为 Basic 认证发出（新决定，代项目所有者作的裁定，2026-10-12）。** 测"日志里找不到链接的标记"的用例查出，HTTP 库的连接池在 debug 级别记下一个 URL 的 authority，用户名与口令也在里面；再往下查，库从来不把 URL 里的用户信息当认证发出（HTTP/1 一概不发，HTTP/2 把整个 authority 连用户信息当 `:authority` 发），要 Basic 认证的服务端答 401。现在 `rurge_net::http::take_basic_authorization(&mut Url)` 把用户名与口令（百分号解码之后）拿出来，作 `Authorization: Basic …`（标为敏感）发出，`HttpClient` 对它建的每个请求这样做（外部资源、Profile 的来源；DoH 的客户端用同一个函数；连通性测试不经它——在被测策略自己的出站上发两次 `HEAD`——，测试 URL 里的用户名口令不发出），调用方自己带了 `Authorization` 头的以它为准。这件事早先的记录（阶段 2 的验收文档、M8c 计划延后事项第 54 行、M8 设计里对应的一条）都把"发不发"写成项目所有者的问题；另一种是把用户名口令从 URL 里拿掉而什么也不发，改回只需改 `HttpClient::build` 里的一个条件（拿掉用户信息的那一步留着）。代价：这些用户名口令此前从不作为认证发出（`http://` 的 URL 此前根本不把它们送出本机），现在随每个请求发给 URL 指名的服务器，对 `http://` 的 URL 是明文。重定向时只带到同一来源（方案、主机、端口都相同）、且目标自己没有写用户名口令时；相对的重定向保留，别的来源不给；从 `https` 到 `http` 的重定向仍被拒（`redirected` 是一个函数，每一种 `Location` 的写法逐种钉着）。`Response` 的 `Debug` 手写，只显示状态、头字段数、正文长度与来处（方案、主机、端口）。同一次改动收紧了依赖库的日志：`quiet_dependencies()` 成了允许名单——rurge 自己的目标按使用者要的级别，其余一律只留 WARN 及以上，`boringtun` 与 `quinn` 仍关，`quinn_udp` 留 WARN——；代项目所有者作的裁定，代价是要看某个依赖的 debug 日志得重新编译或加一个新选项（记录第 49 ～ 53 行；延后第 25、43、44 行）。

27. **第 3 节 M5-D19，第 5.3 节：诊断码与建议。** 本里程碑用到的码：`W0033`（下载到的文本里的本地 include、本地集合、本地 `policy-path`，与下载到的 `external` 策略，各处各带建议，在 `diagnostic::tests` 的 `PER_SITE` 里）、`W0034`（两句：副本没有 `#!MANAGED-CONFIG` 一行；链绕不出去，各带建议，`PER_SITE`）、`I0004`（三句，告知，`WITHOUT`）；`W0011` 留在 `WITHOUT`，理由改写为"只有没给加载器远程来源的调用方才见得到它"；`W0022` 多三种用处（托管配置没下载到、远程 include 没下载到、带 `--wait` 的命令时间到了而集合还落在后面），`W0012` 多"`#!MANAGED-CONFIG` 的链接不是 `http(s)`"与"`interval` 小于 60 按 60"，`W0003` 多集合文件里带未知参数的行，`W0017` 在远程 include 一处有自己的建议。诊断不带时间戳（同一份输入每次给出同样的诊断），不引用配置里的任何取值（记录第 20、21、31、35、78、80 行）。

28. **第 8 节：又多了这些差异。** 除第 8 节的 25 条之外，执行中又定了下面这些，都进兼容性清单：托管配置的副本与它 include 的每个本地文件不能启动 `external` 策略、不能点名本地的集合与 `policy-path`，include 本地文件只能用朴素的相对路径（第 2 条）；被拒的 `policy-path` 让组变空、空组缺省走 DIRECT；下载到的集合文件里的本地引用被跳过（`W0023` 带原因）；嵌套的远程 include 让 rurge 去 GET 更多的 `http(s)` 地址，回环与局域网地址也在内（与下载到的文本里的 `RULE-SET`、`policy-path` 的 URL 自阶段 1、2 起一样）；暂停时会话的 `error` 文本 `refused: the managed profile requires an update (strict)`、HTTP 客户端看到的 502、SOCKS5 的 `0x05`；`GET /v1/profiles/current` 返回生效的主文本（副本生效时是副本）；`GET /v1/external_resources` 的 `suspended`、`error` 里拒绝的原因、`ready` 与 `updatedAt` 的含义，规则集与域名集的项多 `skipped`、`truncated`、`unknownParams`；`POST /v1/profiles/check` 的 `error` 的措辞与 `name` 只认正在运行的 Profile；`GET /v1/rules` 与启动行的规则数不含第一条 `FINAL` 之后的规则；URL 里的用户名口令作 Basic 认证；`.local` 与 `server:system` 走系统解析库；`[Host]` 键末尾的点；AAAA 的计数；规则集文件里带未知参数的行（第 17 条）；API 关连接的三个数（第 19 条）。

29. **第 9 节：测试——执行中定下的几条规矩。** 会让任务停住一个工作线程的用例（比如握着版本通道的一个值，使条目的任务停在一次发送里）在用例自己的线程上按系统时钟等，不等运行时的定时器——tokio 1.53.1 的多线程调度器里握着驱动的那个工作线程不把驱动交出去，单独跑 40 次里挂过 23 次——；新写的计时用例提交前单独循环跑二十次以上；在线跑起来的引擎或守护进程要把 GeoIP 的两个下载地址指到回环（引擎用例的 `geo_urls`，命令行的 `--geoip-url` / `--geoip-asn-url`）；拉起 `rurge` 的用例一律给 `--data-dir`，命令行用例经 `rurge_command()` / `rurge_process()` 起二进制；"什么也没发生"的等待是仅有允许的固定等待；手写的 HTTP 客户端每个请求一次写出请求头与正文，只有专门测"正文晚到"的用例分两次写（记录第 17、94、95 行）。
