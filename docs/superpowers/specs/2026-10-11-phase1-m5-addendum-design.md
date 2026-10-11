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
14. **整分支的评审之后：第 5.2、5.3、6.2 节各改了一处（记录第 71 ～ 74 行）。** (a) **第 5.2 节：`external` 策略拉起的程序得不到 rurge 自己的设置。** 本文没有说。`--listen-http` 的口令可以放在 `RURGE_LISTEN_HTTP` 里（本里程碑加的建议），而 `external` 策略拉起的程序原先继承 rurge 的全部环境，程序里的一句 `env` 或 `set -x` 就会把口令写进 `<数据目录>/external/<策略名>.log`；拉起的程序本身是另一个 rurge 时，它还会照第一个的设置监听，不照自己 Profile 里的 `local-port`。现在名字以 `RURGE_` 开头的变量（不分大小写）都不交给程序，按前缀判断而不列名单：这些名字声明在可执行文件里，名单迟早过时（`is_own_setting`，`crates/rurge-proto/src/external.rs`；记录第 72 行）。(b) **第 5.3 节：`E0008` 在 `include-other-group` 的一处带自己的建议。** 缺省建议是"定义缺的策略或组"，而这个参数只收策略组的名字，名字是 `[Proxy]` 里已有的一条策略时照着做不成；这一处改说给参数写 `[Proxy Group]` 里定义的组的名字，单独一条策略作为组的成员列出。`E0008` 的其余各处仍用缺省建议，59 / 33 / 25 的数没有变（记录第 73 行）。(c) **第 6.2 节：内部 DNS 会话的 `remoteAddr` 与别的 TCP 会话同一条规则。** 文档写成"上游服务器的地址"，只在会话直连时成立；经以地址配置的代理时是代理服务器的地址，代理走复用的连接或隧道时是 `null`。两份 API 参考改了，经代理与因代理以域名配置而改走直连两种情形各补了一条断言；测速会话的三个键恒为空也补了断言（记录第 74 行）。
