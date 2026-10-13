# 手册基线重对报告：Surge Mac 6.10.0 / iOS 5.23.0 的手册对兼容性清单

| 项       | 内容 |
| -------- | ---- |
| 日期     | 2026-10-14 |
| 对应     | 阶段 3 开工之前登记着的"对一遍手册的基线"（阶段 1 补遗（M5b）计划「延后事项」第 75 行）。设计：`docs/superpowers/specs/2026-10-14-manual-baseline-recheck-design.md`（下面写"设计"指它）；计划：`docs/superpowers/plans/2026-10-14-manual-baseline-recheck-plan.md` |
| 结论     | 手册今天写的是 Surge Mac 6.10.0 / iOS 5.23.0（2026-10-13 发布），兼容性清单对照的是 Mac 6.9 / iOS 5.22。87 页手册逐句对着清单读了一遍，2,757 条里有 318 条与清单不一样，逐条裁定：十一处动代码（十处改行为或诊断的文本，一处是给照做了而没有用例的八句话补用例；这一轮做），清单 123 项改动（第 5 节逐行列出：改写、补手册的句子、新增十行、五行的标记从 ✅ 改成 🟡），其中 31 项带着留给后面的阶段或项目所有者的话（第 6 节）；其余不必改。没有哪一条推翻阶段 1、2 已有的结论 |

调研只读（2026-10-13 ～ 14）：Surge 手册 <https://manual.nssurge.com/> 的全部 87 页、Surge 官方频道公开的发行说明（只用来确认手册对应的版本）、本仓库的源码，以及两处第三方的开源代码，各只取一个事实：一个订阅转换器读旧式 `custom` 行的写法（GPL，只取"第几个字段是什么"），一个 Surge 面板读 `GET /v1/profiles/current` 的应答时用的类型定义（MIT，只取"应答有哪几个键"）。没有下载、安装、构建或运行本仓库之外的任何东西；**没有在 Surge 上观察过任何行为**——下面说"与手册一致"，说的是与手册的文字一致。

手册的文字不进仓库：这份报告只引用被对照的那一句（带页面路径），每页文本的 SHA-256 在附录 A，取文本的脚本在附录 B，下一次重对可以从摘要看出哪几页改过。

## 1. 结论

1. **基线前移。** 手册今天描述的是 Surge Mac 6.10.0（12500）、iOS 5.23.0 与 tvOS 5.23.0，三者 2026-10-13 发布（第 3 节）。项目的各份文档改称这一版。
2. **清单的绝大部分没有变。** 第一遍 2,757 条里 2,439 条被读成"清单有这一句、说法一致"（这个数没有人逐条复核，见第 9 节）；其余 318 条逐条对着页面、清单的行与代码核对：128 条不引出任何改动——41 条只是版本标记，8 条是界面与应用自己的东西，44 条是 iOS 专属项的细节，4 条是误报（手册的另一页支持清单的那一行），31 条是清单的行已经隐含的——，190 条引出了改动或要先对着代码读，落在第 4 ～ 6 节。助手另外点出清单里 46 行"读的这几页不支持"的：31 行是 rurge 自己的行或自己写明手册没有说的，11 行是误报，2 行改了，2 行进了"手册没有写"的复核（第 7、8 节）。
3. **最重的三件：**
   - 旧式的 `custom` 策略行今天是未知类型（`E0004`），带着它的 Profile 整份加载不了；手册说它读作 Shadowsocks 策略。这一轮改（第 4 节第 3 项）。
   - `GEOIP,UNKNOWN` 与 `IP-ASN,UNKNOWN`：阶段 1 的需求复读已经登记为"没有做完"的三句之一，这一轮做掉（第 4 节第 1 项）。
   - 一行怎样拆分：rurge 的拆分函数让引号在一项的任何位置都开启一段引用，带撇号的名字会吃掉一行的后半（`G = select, Tom's HK, Tom's US` 把两个成员读成一个）。这一轮**不改**——解析器与脱敏要一起改——，清单登记它，下一步单独设计（设计第 6 节）。
4. **手册自己有六处前后不一**（`SYSTEM_VERSION` 的示例、`test-timeout` 的缺省、`block-quic = always-allow` 管不管直连类策略、两条 `CELLULAR-*` 规则的平台、Shadow TLS 能不能与 QUIC 类协议组合、代理服务器的主机名进不进 `[Host]`），清单把两边的话都写出来（第 5 节的 R14、R28、R29、R40、R62、R92）。
5. 这份报告**不能**说明的：Surge 的实际行为；被读成"一致"的 2,439 条；阶段 3 及以后的行上手册多说的细节（这一遍没有收集）；阶段 2 的代码是否处处合手册（这一轮只对着代码核对了 47 处）。见第 9 节。

## 2. 做法

### 2.1 手册的副本

手册是 HonKit 生成的静态站点，没有公开的源。页面清单取自首页目录里的 `data-path` 属性，共 87 页：`dns/` 5 页、`features/` 9 页、`getting-started/` 3 页、`http/` 6 页、`policies/` 21 页、`policy-groups/` 9 页、`profile/` 7 页、`rules/` 11 页、`scripting/` 9 页、`tools/` 7 页。每页取成一份文本：只留正文，去掉导航与脚本，`<code>` 换成反引号，标题换成 `#`，一个块一行（附录 B 的脚本）。2026-10-13 与 2026-10-14 各取一次，87 页逐页相同。

### 2.2 第一遍：十一个只读的助手逐句读

清单 919 行，按节分给十一个助手。每个助手拿到手册的几页与清单里对应的几节，把页面的每一句当作一条，给出四种结论之一：`same`（清单有这一句、说法一致）、`missing`（手册有、清单没有）、`differs`（两边说法不同）、`more`（清单有这一行，而手册对这一项多说了那一行没有写的：缺省、上限、取值、单位、版本、平台、与别的项的关系）。`more` 只用于阶段 1、2 的行；阶段 3 及以后的行，除非 `missing` 或 `differs`，一律记 `same`——手册比那些行多说的细节，这一遍没有收集（第 9 节）。

| 助手读的页 | 页数 | 条数 | `same` | `missing` | `differs` | `more` |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `profile/general`、`getting-started/platform-differences` | 2 | 159 | 124 | 5 | 3 | 27 |
| `dns/` 五页 | 5 | 171 | 153 | 1 | 2 | 15 |
| `tools/` 七页（Dashboard、Logbook、测试、CLI、HTTP API、URL Scheme、面板） | 7 | 295 | 254 | 21 | 2 | 18 |
| `policies/` 前十一页（总览、内置、REJECT、通用参数、TLS、UDP、HTTP、SOCKS5、Shadowsocks、Snell、VMess） | 11 | 268 | 226 | 3 | 5 | 34 |
| `getting-started/` 两页与 `profile/` 六页（格式、托管 Profile、模块、Requirement、Keystore、Host List） | 8 | 251 | 224 | 8 | 5 | 14 |
| `rules/` 十一页 | 11 | 329 | 276 | 1 | 2 | 50 |
| `policies/` 后十页（Trojan、TUIC、Hysteria 2、MASQUE、AnyTLS、TrustTunnel、SSH、WireGuard、Tailscale、external） | 10 | 312 | 274 | 5 | 5 | 28 |
| `policy-groups/` 九页 | 9 | 204 | 178 | 2 | 3 | 21 |
| `http/` 六页 | 6 | 161 | 160 | 0 | 1 | 0 |
| `scripting/` 九页 | 9 | 389 | 377 | 10 | 2 | 0 |
| `features/` 九页 | 9 | 218 | 193 | 19 | 1 | 5 |
| 合计 | 87 | 2,757 | 2,439 | 75 | 31 | 212 |

`more` 的 212 条是手册比清单的行多说的话，其中不少只是版本标记或 iOS 专属项的细节（第 7 节）；`http/` 与 `scripting/` 两组是 0，因为清单的第 7、8 两节是阶段 4、5 的行。另外，每个助手在自己报告的末尾列出它那几节里"读的这几页不支持"的清单行，共 46 行；它们不在上表里，去向在 7.2 节。

### 2.3 第二遍：对着代码，对着"手册没有写"

- **代码**：两个只读的助手对着代码核对第一遍里挑出来的 47 处——阶段 1、2 的条目里手册的句子是新的、或与清单不同的。配置与规则 27 处：17 处照手册，5 处不同，5 处没有；运行期 20 处：10 处照手册，8 处不同，1 处没有，1 处从仓库里判断不了（WireGuard 隧道里的 ICMPv6 回显，靠协议栈库的行为）。每一处给出读到的代码、断言它的用例名（或"没有找到用例，搜过什么"）。
- **"手册没有写"**：清单里有 69 行（85 处）说"手册没有写 / 未说明 / 未定义"。一个助手拿今天的手册逐处再查：78 处手册仍然没有写，2 处手册现在写了，4 处其实不是在说手册，1 处拿不准。清单里引用的手册原句没有一句从手册里消失。

### 2.4 三条规矩

- **`same` 不是证据。** 助手说一致的，没有人逐条复核；它只表示"读的人没有看出不同"。
- **每一条 `missing` / `differs` / `more` 与每一处代码不符，都由主持这一轮的会话自己对着页面文本、清单的行与代码核对之后才裁定。** 助手的结论只是线索。
- **清单的一行写进关于 rurge 的新说法时，引用的用例名先用 `git grep` 核对。** 下面出现的用例名都核对过存在；这一轮要新写的用例不点名，写作"引用这一轮的用例"，名字在计划里。

### 2.5 裁定的去向

每一条落到下面之一：

- **改代码**（十一处，第 4 节）：阶段 1、2 已经声称支持的条目上手册的一句话，小，在回环上验证得了。
- **改清单**（第 5 节）：补手册的句子、订正过时或不对的说法、改标记、加行。
- **留给后面的阶段**（第 6 节）：阶段 3 ～ 7 的条目，手册的句子写进那一行，供那个阶段的设计从它出发；两件不设阶段的归项目所有者。
- **不改**（第 7 节）：版本标记、界面、iOS 专属项的细节、清单的行已经隐含的、误报。

## 3. 基线

### 3.1 手册今天对应哪一版

Surge 官方频道（Telegram 的 SurgeTestFlight，公开可读）2026-10-13 的三条：iOS 版的发行说明"Version: 5.23.0 / Release Date: 2026-10-13"；"Surge Mac 6.10.0, Surge iOS 5.23.0, and Surge tvOS 5.23.0 are now available."；Mac 版的发行说明"Version 6.10.0-12500"。手册里标着这两个版本号的小节（下一小节）与这三条说的新功能一一对得上，所以今天的手册就是这一版的手册。

### 3.2 手册里比旧基线新的十四处

在 87 页文本里找比 Mac 6.9 / iOS 5.22 新的版本标记，共十四处：

| 页 | 手册标的版本 | 是什么 | rurge |
| --- | --- | --- | --- |
| `features/ip-rewrite.html` | iOS 5.23.0+ / Mac 6.10.0+ | `[IP Rewrite]`：按目标地址处理进入虚拟网卡的包 | 这一轮起解析器认识这个节；语义随阶段 3（R7、R109） |
| `features/snell-server.html` | iOS 5.23.0+ / Mac 6.10.0+ | 模块可以提供整个 `[Snell Server]` 节 | 阶段 5 / 7（R16、R108） |
| `policies/parameters.html` | iOS 5.23.0+ / Mac 6.10.0+ | 策略的 `test-url` 可以是 HTTPS | 阶段 2 / M3b 起就是这样（R88） |
| `policy-groups/parameters.html` | iOS 5.23.0+ / Mac 6.10.0+ | 组参数 `category`（界面分类） | 清单已有一行：接受、不生效 |
| `profile/general.html` | iOS 5.23.0+ / Mac 6.10.0+ | `internet-test-url` 可以是 HTTPS，延迟仍取已建立的连接上的第二次 HEAD | 同上（R26、R88） |
| `profile/general.html` | iOS 5.23.0+ / Mac 6.10.0+ | `proxy-test-url` 同样 | 同上（R27） |
| `profile/general.html` | iOS 5.23.0+ | `include-device-communication` | 清单已有一行：解析并忽略（R30） |
| `profile/module.html` | iOS 5.23.0+ / Mac 6.10.0+ | 模块里的 `[Snell Server]` 节整个替换 Profile 的节 | 阶段 5（R16） |
| `profile/module.html` | iOS 5.23.0+ / Mac 6.10.0+ | 同名的内联规则集在 Profile 与各模块之间合并 | 阶段 5（R16）；今天一份 Profile 里两段同名的 `[Ruleset <名字>]` 是 `W0020`、用第一段 |
| `rules/ip.html` | iOS 5.102.0+ / Mac 6.9.1+（手册原文如此；官方频道上 5.102.0 是 TestFlight 测试版的版本号） | `GEOIP,UNKNOWN` / `IP-ASN,UNKNOWN` | 这一轮做（第 4 节第 1 项） |
| `tools/cli.html` | Mac 6.10.0+ | `surge-cli lsp`：给编辑器用的离线语言服务器 | 没有；清单加一行 ❓（R112） |
| `tools/http-api.html` | iOS 5.23.0+ / Mac 6.10.0+ | 外部资源的两个端点 | 阶段 1 补遗（M5b）已做（R118、R119） |
| `tools/http-api.html` | iOS 5.23.0+ / Mac 6.10.0+ | `GET /v1/geoip` | 阶段 1 补遗（M5a）已做（R120） |
| `tools/testing.html` | iOS 5.23.0+ / Mac 6.10.0+ | 测试 URL 可以是 HTTPS | 同 `test-url` |

十四处里八处 rurge 今天已经有，两处这一轮做（`[IP Rewrite]` 只做到解析器认识这个节），四处属于后面的阶段或归项目所有者定（语言服务器）。

### 3.3 发行说明里、手册没有写到的

官方频道 2026-10-13 发布之后的测试版说明（Mac 6.10.0-12510）里有下面几件，87 页手册没有一页写到，所以不在这一轮；下一次重对从这里开始：

- `block-quic=adaptive`（先挡 QUIC、应用一秒内不回落就对那个主机放行），`block-quic=auto` 对 DIRECT 之外的策略改用它；
- QUIC 的主机名识别在 ClientHello 跨几个包时也行，另加 QUIC v2 与 draft-29；
- `GET /v1/policies?include_external=true`（应答多一个字段，列出经 `policy-path` 载入的策略）。

正式版的发行说明里另有三条与 rurge 的现状有关，已经落在下面：

- "Fixed logical rules incorrectly parsing policy names containing parentheses."——手册没有对应的句子。rurge 今天把带成对括号的策略名读得对，这一轮用一条用例钉住（设计 4.9，R41）；不成对的括号归"一行怎样拆分"的设计（设计第 6 节）。
- "Improved UDP test diagnostics for peer-to-peer Tailscale and WireGuard policies without Internet egress, reporting unsupported tests instead of waiting for a timeout."——手册的 WireGuard 页有对应的一句（点对点的策略只提供往返时间的测试）。rurge 没有"点对点策略"这个概念，对这样的策略照样去测（R60）。
- "URL-based linked profiles can now be updated manually."——手册的 Linked Profiles 一节没有写怎样手动更新。rurge 里对应的是远程 `#!include`：它是一项外部资源，经 `POST /v1/external_resources/update` 更新（R119）。

## 4. 引出代码改动的十一处

每一处的语义、接口与判据在设计第 4 节；这里只说手册的话、今天的行为与为什么这一轮做。

| # | 手册（页）与原话 | 今天 | 这一轮 |
| --- | --- | --- | --- |
| 1 | `rules/ip.html`："Use `UNKNOWN` (case-insensitive) to match a destination IP for which the database lookup returns no country or ASN" | `GEOIP,UNKNOWN` 能加载、永不匹配、没有提示；`IP-ASN,UNKNOWN` 是加载错误 `E0011` | 两种写法都匹配"库已经载入而查不到"的地址；库没有载入时不匹配（设计 4.1） |
| 2 | `profile/format.html` 的节清单有 `[IP Rewrite]` | 未知的节，`W0002` | 已知的节，行留给阶段 3，`W0016`（设计 4.2） |
| 3 | `policies/shadowsocks.html`："The legacy `custom` policy type from very old configurations is parsed as a Shadowsocks policy; the external module URL in its declaration is ignored. Use the `ss` type instead." | 未知类型 `E0004`，整份 Profile 加载不了 | 读作 `ss`，每行一条 `I0001`（设计 4.3） |
| 4 | `dns/encrypted-dns.html`："A `tcp://` URL may only contain a host and an optional port." | `tcp://host/anything` 被接受，`/` 之后的被丢掉 | 写了路径或查询串的是 `W0026`，同 `quic://`（设计 4.4） |
| 5 | `policies/wireguard.html`："Common policy parameters apply, with L3-specific exceptions: `interface` binding and the `shadow-tls-*` parameters are not supported for WireGuard policies." | `tos` 写在 `wireguard` 行上是 `W0028`，被清成 0 | `tos` 到达隧道自己的 UDP socket（设计 4.5） |
| 6 | `rules/logical.html`："Any rule type that is valid inside a rule set can be used as a sub-rule" | "永不匹配"的告警 `W0013` 只看顶层规则：`AND,((PROCESS-NAME,x),(DEST-PORT,443)),P` 不声不响地永不匹配，`NOT,((SCRIPT,s)),P` 不声不响地总是匹配 | 检查走进 `AND` / `OR` / `NOT`（设计 4.6） |
| 7 | `dns/local-dns-mapping.html`："If the record has multiple addresses, one is chosen at random." | 总是取第一个 | 随机取一个（设计 4.7） |
| 8 | `dns/encrypted-dns.html` 按 URL 的方案列出 `PROTOCOL` 规则可用的标签（`https://` 是 `DOH`，`h3://` 是 `DOH3`，`quic://` 是 `DOQ`，`tls://` 是 `DOT`，`tcp://` 是 `DNS`） | 按端口猜：不在 443 的 `h3://` 服务器被标成 `DOQ`，不在 853 的 `tls://` 被标成 `DNS` | 按方案（设计 4.8） |
| 9 | 八句手册的话，代码照做了而没有用例 | 靠读代码 | 八条用例钉住（设计 4.9） |
| 10 | `profile/general.html`（`block-quic`）："`all-proxy`: Overrides the proxy policies' `block-quic` parameter, blocking all QUIC traffic through proxies." | `all-proxy` 下直连类策略一律放行，写了 `block-quic=on` 的 `direct` 别名也不阻断 | 这个值只盖过代理策略的参数，直连类策略照它自己的参数（设计 4.10） |
| 11 | `policy-groups/subnet.html`（`default` 一项）："The policy used when no subnet expression matches." | `subnet` 组的告警 `W0008` 说"the first member is used"；这样的组其实在每个网络上都代表它的 `default`（阶段 2 / M3a 起），它也没有成员 | 告警改成说实际发生的事的一句（设计 4.11）；组的语义随阶段 3 |

第 6 项的另一半——`SCRIPT` 作子规则时真的去问脚本——属于阶段 5。第 8 项手册同一段的另一句"可以给 DNS 服务器的主机名写规则"这一轮不做：rurge 的内部会话带的是引导解析之后的地址，随阶段 3 的会话模型一起（R92）。

## 5. 清单逐行的改动

这一节是 `docs/surge-compatibility-matrix.md` 要改的每一处，共 123 项（R1 ～ R123）。读法：

- **行**：`L<n>` 是清单在提交 `30b082a` 上的行号，后面是那一行的第一格。改的时候按第一格找行，行号只是帮着找。
- **手册**：页的路径（相对 <https://manual.nssurge.com/>）与原话。原话照页面的文本逐字抄，不翻译；一格里有几句时，后面几句与前一句同页，换页时另写路径。
- **改动**：这一行改成什么。没有提到标记的行标记不变；"备注"指那一行的最后一格，"Surge 一栏"指说 Surge 的行为（或取值、说明）的那一格。新增的行全文写在各小节的表后面。
- 提到的用例名都是今天代码里已有的，用 `git grep` 核对过。这一轮新加的用例不在这里点名（名字在计划里）：写着"引用这一轮的用例"的地方，改清单的人从计划里取名字。说"这一轮改了"的，改动本身见第 4 节与设计。
- 清单的行里称这一轮为"手册基线重对"。

### 5.1 文首与第 1 节（R1 ～ R19）

| 号 | 行 | 手册（页）与原话 | 改动 |
| --- | --- | --- | --- |
| R1 | L3 文首第一段 | — | "macOS 6.9 / iOS 5.22 时期的官方手册"改成"Mac 6.10.0 / iOS 5.23.0 的官方手册，2026-10-14 取的文本；怎样对的、每页文本的摘要见 `docs/reports/2026-10-14-manual-baseline-recheck.md`" |
| R2 | L5 文首第二段 | — | "最近一次：2026-10-13，阶段 1 补遗（M5b）"改成"最近一次：<文档提交那天的日期>，手册基线重对" |
| R3 | L7 文首第三段 | — | "要一起看：3 句没有做完"改成"要一起看：2 句没有做完"；破折号后面的三项去掉第一项（"手册 IP 页的 `GEOIP,UNKNOWN` 与 `IP-ASN,UNKNOWN`（3.1 节 `GEOIP` 与 `IP-ASN` 两行）、"）；在"（需求里没有给，这份清单没有对应的行）"后面加一个括号："（逐句读的时候是 3 句：第三句是手册 IP 页的 `GEOIP,UNKNOWN` 与 `IP-ASN,UNKNOWN`，手册基线重对时做了，见 3.1 节 `GEOIP` 与 `IP-ASN` 两行）" |
| R4 | L34 有序节 | `profile/format.html`："For line-based sections — `[Rule]`, `[Host]`, `[URL Rewrite]`, `[Header Rewrite]`, `[Body Rewrite]`, `[Map Local]`, `[Panel]`, `[Port Forwarding]`, `[IP Rewrite]`, `[Script]`, `[SSID Setting]`, and `[Ruleset <name>]` —" | 第二格在 `[Port Forwarding]` 后面加 `[IP Rewrite]`（十二个节，顺序同手册）；备注末尾加"；`[IP Rewrite]` 自手册基线重对起是有序节（1.2 节）" |
| R5 | L38 之后新增一行 | `rules/overview.html`："If a value contains commas (e.g. a regex), wrap it in double or single quotes." | 新行"一行怎样拆成各项（引号与括号）"，🟡，阶段 1；全文见表后。L38 自己不动 |
| R6 | L40 小节标题 | `profile/format.html` 的节清单里有 `[IP Rewrite]` | "（23 个）"改成"（24 个）" |
| R7 | L59 之后新增一行 | `features/ip-rewrite.html`："The `[IP Rewrite]` section decides what happens to a packet entering Surge VIF based on its destination address." | 新行 `[IP Rewrite]`，✅，阶段 3；全文见表后 |
| R8 | L44 `[General]` | `profile/general.html`："All keys are optional `key = value` lines; keys are case-insensitive." | 备注改成："每个键都可以不写；键名不分大小写（用例 `every_key_the_manual_lists_is_read`）；逐键见第 2 节" |
| R9 | L46 `[Proxy Group]` | `policy-groups/overview.html`："Each line follows the pattern `Name = type, member1, member2, ..., key=value, ...`. Components containing `=` are parameters; all other components are member policy names." | 备注（原来是空的）写："第一项是组的类型，其余各项里带 `=` 的是参数、不带的是成员（用例 `manual_group_examples`）；`subnet` 组上，键不是组参数的 `key=value` 是"网络条件 = 策略"（5.1 节 `subnet` 一行）；逐个参数见第 5 节" |
| R10 | L78 启动时等 Profile 的来源 | `profile/managed-profile.html`："Even when `strict` is true, the user can still start Surge from a widget or the VPN switch in Settings." | Surge 一栏"手册没有写启动时的行为"改成："手册关于启动只有一句：`strict` 拦不住使用者启动 Surge（原话在上一行）；启动时等不等来源，没有写" |
| R11 | L79 下载到的文本能让 rurge 对本机做什么 | `profile/format.html`："A managed profile cannot be modified locally because it can be updated remotely."；`profile/module.html`："Modules cannot adjust the content of `[Proxy]` and `[Proxy Group]` sections. Rule lines may only be inserted at the top of the rule list and are restricted to internal policies." | Surge 一栏开头的"手册没有把托管 Profile 与别的 Profile 区别对待；"改成："手册没有说托管 Profile 在本机能做的事比别的 Profile 少：它把托管 Profile 分开说的只有"不能在本地编辑"，按来源设限的只有模块（不能动 `[Proxy]` 与 `[Proxy Group]`，规则只能用内置策略）；"。后面的不动 |
| R12 | L84 通配命名节 | `profile/format.html`："Loading fails if two included files provide a section with the same name, or if an included section duplicates one already declared in the profile."（这一行已经引着） | 标记 ✅ 改成 🟡：这一行自己写着"与手册不同（登记的差异，决定保持现状）"，而图例里 ✅ 是"语义与 Surge 一致"。文字不动 |
| R13 | L96 变量 `CORE_VERSION` | `profile/requirement.html`："Starting with Surge Mac 6.8.0 and Surge iOS 5.21.0, Core Version is derived from the corresponding Surge Mac version instead of being maintained separately. The value is encoded as `major * 1000000 + minor * 1000 + patch`. For example, Surge Mac 6.8.0 and Surge iOS 5.21.0 use Core Version `6008000`."；"22: Surge Mac 5.7.0, Surge iOS 5.11.0, Smart Group"；"20: Surge Mac 5.6.0, Surge iOS 5.10.0, Body Rewrite, Inline Map Local" | Surge 一栏改成："Mac 6.8.0 / iOS 5.21.0 起由 Surge Mac 的版本号算出：`major * 1000000 + minor * 1000 + patch`（这两版是 `6008000`）；此前是单独指定的值：22（Mac 5.7.0 / iOS 5.11.0，Smart Group）、20（Mac 5.6.0 / iOS 5.10.0，Body Rewrite、Inline Map Local）"。备注末尾加："；照新编码写的条件（如 `CORE_VERSION>=6008000`）在 rurge 报 22 的期间不成立，那样的行不生效" |
| R14 | L98 变量 `SYSTEM_VERSION` 等 | `profile/requirement.html`："SYSTEM_VERSION: System Version 17.4.1 (Build 21E236)"；`profile/module.html`："SYSTEM_VERSION: String, such as `Version 17.4.1 (Build 21E236)`" | 备注里"手册的示例是 `System Version 17.4.1 (Build 21E236)`"后面加："（`profile/requirement.html`；`profile/module.html` 的示例是 `Version 17.4.1 (Build 21E236)`，没有开头的 `System `。手册两页不一样，rurge 拼的是前一种，Surge 实际报哪一种没有核对；照后一种写的 `SYSTEM_VERSION BEGINSWITH 'Version '` 在 rurge 里不成立，用例 `check_reads_the_system_version_from_the_operating_system` 钉的是 `'System Version '`）" |
| R15 | L100 逻辑运算符 | `profile/module.html`："It also supports logical expressions, such as `CORE_VERSION>=20 && (SYSTEM = 'iOS' \|\| SYSTEM = 'tvOS')`." | 备注（原来是空的）写："可以用括号分组，运算符两侧可以有空格（手册的例子；用例 `manual_examples_evaluate`、`every_operator_the_manual_lists_decides_both_ways`）；写在行上的 `#!REQUIREMENT` 里，带空格的表达式要整个用双引号包起来" |
| R16 | L111 可覆盖节 | `profile/module.html`："`[MTProto]` iOS 5.21.0+ Mac 6.8.0+ and `[Snell Server]` iOS 5.23.0+ Mac 6.10.0+ sections"、"A module can enable the built-in MTProto proxy server or Snell server. The section must be complete: it replaces the corresponding section of the profile as a whole instead of patching individual keys."；"A module may add lines to an inline rule set. If the profile, or another enabled module, already defines a rule set with the same name, the lines are merged into it; otherwise a new rule set is created. Since the order inside a rule set has no effect, a module cannot remove or reorder existing lines. iOS 5.23.0+ Mac 6.10.0+"；"Rule, Script, URL Rewrite, Header Rewrite, Host, IP Rewrite"、"New lines are inserted at the top of the original content." | 第二格加 `[IP Rewrite]` `[MTProto]` `[Snell Server]`。备注改成："行式节（手册今天列的是 Rule、Script、URL Rewrite、Header Rewrite、Host、IP Rewrite）的新行插入到原内容顶部；`[MTProto]`（iOS 5.21.0+ / Mac 6.8.0+）与 `[Snell Server]`（iOS 5.23.0+ / Mac 6.10.0+）要整节写全，整节替换 Profile 里的那一节，不是逐键覆盖；`[Ruleset *]`：模块给一个内联规则集加行，Profile 或别的启用的模块已有同名的集合时并进去，否则新建（iOS 5.23.0+ / Mac 6.10.0+）——今天同一份 Profile 里两段同名的 `[Ruleset X]` 是 `W0020`、用第一段（1.3 节），模块落地时模块带来的同名集合要合并而不是忽略；手册今天的这张单子里没有 `[Body Rewrite]` 与 `[Map Local]`，清单照旧把它们算作可覆盖（模块页自己拿 Body Rewrite 举例："if the module uses the newly added Body Rewrite feature"），Surge 是否接受没有核对" |
| R17 | L117 `#!system=` | `profile/module.html` 的两处示例都是 `#!system=mac` | 备注末尾加："；手册的模块页今天只出现 `#!system=mac`（两处示例），`iOS`、`tvOS` 两个取值不在今天的页上，rurge 的读法不变" |
| R18 | L118 `#!arguments=` | `profile/module.html`："The query-string form (`hostname=example.com&enable_mitm=true`) and `%PARAMETER%` placeholders are not supported for module parameter tables." | 备注末尾加："；查询串的写法（`hostname=example.com&enable_mitm=true`）与 `%PARAMETER%` 占位符手册明说不支持，rurge 同样不认" |
| R19 | L126 `[Keystore]` 条目 | `profile/keystore.html`："The item type. If omitted, an item with a `password` is treated as `p12`; an item without one is treated as `openssh-private-key`."；"Required, Base64 string"；"You must Base64-encode the entire private key file again, even though the private key file uses Base64 encoding itself. RSA, ECDSA, ED25519, and DSA keys are supported; see the SSH policy page for details."；`policies/ssh.html`："All four types of private keys, RSA/ECDSA/ED25519/DSA, are supported." | 标记 ✅ 改成 🟡：手册把 DSA 私钥列为支持，rurge 不支持（备注里早就写着，标记却是 ✅）。Surge 一栏改成："`type` 为 `p12` 或 `openssh-private-key`；省略时带 `password` 的条目是 `p12`、不带的是 `openssh-private-key`；`base64` 必填；私钥支持 RSA、ECDSA、ED25519、DSA"。备注开头加："**与手册不同的一处：DSA 私钥**（手册列为支持；rurge 在干构建期 `E0022`，文本只点名条目，用例 `keys_rurge_cannot_use_are_named_not_quoted`）。`type` 的推断同手册，空的 `password=` 也算带口令；没有 `base64` 是 `E0001`（用例 `items`）；" |

R5 的新行（接在"值含逗号"一行之后）：

```
| 一行怎样拆成各项（引号与括号） | 手册只讲"整个值用引号包起来"这一种写法（`rules/overview.html`："If a value contains commas (e.g. a regex), wrap it in double or single quotes."），没有说一个值中间的引号或括号算什么 | 🟡 | 1 | **登记的差异（手册基线重对；怎样改是下一份设计的题目，解析器与脱敏要一起改）**。rurge 的拆分（`rurge_config::value::split_list`，规则、策略、组、`[Host]` 都经它）比手册说的宽：`'` 或 `"` 在一项的任何位置都开启一段引用，直到下一个同样的引号；圆括号在任何位置都计层；没有配对的引号或左括号于是把这一行余下的部分全读进同一项。用 `rurge check` 看到的：`USER-AGENT,Bob's App*,Proxy` 是 `E0001`（`USER-AGENT: missing policy`）；`G = select, Tom's HK, Tom's US` 是 `E0008`，两个成员被读成一个名字；撇号在一行的最后一项里时没有事（`DOMAIN,b.example,Tom's HK` 加载得了）；成对的括号没有事（`AND,((DOMAIN,a.example),(DEST-PORT,443)),Proxy (US)` 加载得了）。**怎样绕开**：引用这个名字的地方把整项用双引号包起来（`G = select, "Tom's HK", "Tom's US"`、`USER-AGENT,"Bob's App*",G`、`DOMAIN,b.example,"Tom's HK"`）；`[Proxy]` 里 `=` 左边定义名字的地方照原样写、不加引号（`Tom's HK = direct`——写成 `"Tom's HK" = direct` 时 `Tom's HK` 这个名字没有被定义，引用它的组得到 `E0008`）。`[Proxy Group]` 里 `=` 左边的名字没有试过。Surge 怎样读这样的行没有观察过 |
```

R7 的新行（接在 `[Port Forwarding]` 一行之后）：

```
| `[IP Rewrite]` | 按目的地址处理进入虚拟网卡的 IP 包：`reflect` / `reject` / `drop`（iOS 5.23.0+ / Mac 6.10.0+） | ✅ | 3 | 解析器自手册基线重对起认得这个节：有序节，行原样留着，进"解析了、这一版不生效"的那一条 `W0016`（`sections parsed but inactive in this version: …`），此前是未知的节（`W0002`）。地址与动作今天不校验，诊断由阶段 3 的设计定；语义见 9.10 节 |
```

### 5.2 第 2 节 `[General]`（R20 ～ R32）

| 号 | 行 | 手册（页）与原话 | 改动 |
| --- | --- | --- | --- |
| R20 | L145 `encrypted-dns-follow-outbound-mode` | `dns/encrypted-dns.html`："When enabled, encrypted DNS connections follow the outbound mode settings and are matched against the rules like normal requests. You can then configure a rule for the DNS server's hostname to use a proxy, or match the connections with a PROTOCOL rule:" | "协议标签按端口启发（853→DoT，443→DoH，其余→DNS）"改成："协议标签按上游 URL 的方案（`tls://` 是 `DOT`，`https://` 是 `DOH`，`tcp://` 是 `DNS`，与端口无关；手册基线重对起——此前按端口猜，853 是 `DOT`、443 是 `DOH`、其余 `DNS`，不在缺省端口上的服务器被标错）"。"故域名规则不匹配上游主机名"后面加："（手册说可以给 DNS 服务器的主机名写规则，rurge 今天做不到，见 6.2 节同名一行）"。备注开头加："跟随出站模式的意思：直连模式下这些会话走 DIRECT，全局模式下走全局策略，规则模式下走规则，与普通会话是同一个判断（读代码得出）；" |
| R21 | L148 `use-local-host-item-for-proxy` | `dns/local-dns-mapping.html`："If the record has multiple addresses, one is chosen at random." | "只对 `[Host]` 里指向 IP 的条目生效，取第一个 IP；"改成"只对 `[Host]` 里指向 IP 的条目生效；"；末尾的"`[Host]` 给同一个名字列了多个地址时取第一个，不看策略的 `ip-version`（未与真实 Surge 核对）"改成："`[Host]` 给同一个名字列了几个地址时，每个会话随机取一个（同手册；手册基线重对起，此前总取第一个；引用这一轮的用例），不看策略的 `ip-version`（手册没有说）" |
| R22 | L151 `geoip-maxmind-url` | `rules/ip.html`："`GEOIP` rules use a MaxMind GeoLite2 country database. A copy is bundled with Surge, and Surge periodically downloads updates." | 备注开头加："Surge 随包带一份国家库并定期下载更新；rurge 不捆绑，第一次下载之前没有库（`GEOIP` 规则不匹配，`I0003`，见 3.1 节 `GEOIP` 一行）；" |
| R23 | L152 `disable-geoip-db-auto-update` | `rules/ip.html`："`IP-ASN` rules use a bundled GeoLite2 ASN database, which is updated together with app updates. There is no configuration key for it and no separate download mechanism." | 备注末尾加："；这个键在 rurge 里管两个库：Surge 的 ASN 库随应用更新、不单独下载，rurge 两个库都下载（ASN 库的地址是 `--geoip-asn-url` / `RURGE_GEOIP_ASN_URL`）；关掉自动更新之后，缺着的库仍会下载一次（用例 `disabled_auto_update_only_fetches_missing_files`）" |
| R24 | L153 `ipv6` | `profile/general.html`："Enable full IPv6 support. After enabling this option, Surge queries AAAA records when accessing domain names. Even if this option is not enabled, you can still access IPv6 sites by using IPv6 addresses directly." | 备注（原来是空的）写："它只管解析器问不问 AAAA（与留不留 IPv6 的 DNS 服务器），和已解析出的地址里哪一族在前；目标写成 IPv6 字面量的请求在 `ipv6 = false` 时照常拨出（同手册；引用这一轮的用例）" |
| R25 | L158 `skip-proxy` | `profile/general.html`："This option is used to fix compatibility problems with some apps; it is not a way to keep requests off a proxy policy."；"Notice: If you enter an IP address or address range, you are only able to bypass the proxy when you connect to that host using that address, not when you connect to the host by a domain name that resolves to that address." | 备注开头加："手册的两句提醒对 rurge 同样成立：这个键是让某些应用绕开系统代理用的，不是让请求不走代理策略的办法（那是规则的事）；写成 IP 地址或网段的项只在应用按那个地址连接时绕开，按解析到那个地址的域名连接时不绕开；" |
| R26 | L166 `internet-test-url` | `tools/testing.html`："`internet-test-url` in `[General]` sets the URL used for internet connectivity checks; it is also the test URL for the DIRECT policy." | 备注末尾加："；手册说它还是"互联网连通性检查"用的 URL，rurge 没有这样一项检查，它在 rurge 里只是直连类策略的测试 URL；HTTPS 的 URL 见 5.2 节"测试 URL / 超时解析顺序"一行" |
| R27 | L167 `proxy-test-url` | `profile/general.html`："The default testing URL for proxy policies. HTTPS URLs are supported the same way as `internet-test-url`. iOS 5.23.0+ Mac 6.10.0+" | 备注末尾加："；HTTPS 的 URL 见 5.2 节"测试 URL / 超时解析顺序"一行" |
| R28 | L168 `test-timeout` | `profile/general.html`："The default connectivity testing timeout for proxy policies. (Tests for the DIRECT policy use a 10-second timeout.)"；`policy-groups/parameters.html`："The per-test timeout is resolved similarly: the policy's `test-timeout` parameter, then the global `test-timeout` option, then 5 seconds by default (10 seconds for direct-type policies)." | 备注开头加："**手册两页说法不同**：`[General]` 页说 DIRECT 的测试用 10 秒，读起来与这个键无关；策略组参数页说先看策略自己的 `test-timeout`、再看全局的、最后才是缺省的 5 秒（直连类 10 秒）。rurge 照后一句（用例 `a_test_goes_where_the_policy_or_the_profile_says`、`the_defaults_are_the_manuals`）：写了全局的 `test-timeout` 时，DIRECT 与 `direct` 别名也用它，不再是 10 秒。" |
| R29 | L175 `block-quic` | `profile/general.html`："`per-policy`: Determined by each policy's `block-quic` parameter. This is the default value."；"`all-proxy`: Overrides the proxy policies' `block-quic` parameter, blocking all QUIC traffic through proxies."；"`all`: Overrides all policies' `block-quic` parameters, blocking everything including DIRECT policies."；"`always-allow`: Overrides the proxy policies' `block-quic` parameter, allowing everything." | "四个值的含义按名字理解、未与 Surge 核对：`per-policy` 用终端策略自己的 `block-quic`，`all-proxy` 凡经代理一律阻断，`all` 连 DIRECT 也阻断，`always-allow` 一律不阻断。"改成："四个值照手册：`per-policy` 用终端策略自己的 `block-quic`；`all-proxy` 盖过代理策略的参数，经代理的一律阻断，直连类策略（`DIRECT` 与 `direct` 别名）仍照它自己的参数——写了 `block-quic=on` 的 `direct` 别名照样阻断（手册基线重对起，此前这个值下直连类一律放行；引用这一轮的用例）；`all` 连直连也阻断；`always-allow` 一律不阻断——手册这一句的前半说它盖过的是代理策略的参数，后半说"allowing everything"，两半对写了 `block-quic=on` 的直连类策略说法不一，rurge 取后半。" |
| R30 | L195 `include-device-communication` | `profile/general.html` 的键表（58 个键） | 去掉"它属于晚于本清单的手册基线（Mac 6.9 / iOS 5.22）的版本；"；"需求文档写的是 57 个，多出来的就是它"改成"需求文档原来写 57 个，多出来的就是它（手册基线重对时需求文档改成了 58 个）" |
| R31 | L201 `http-listen` | `profile/general.html`："The address must be an IP literal (wrap an IPv6 address in `[...]` when a port is present). The port may be omitted, in which case 6152 is used. Multiple comma-separated listeners are allowed. The proxy service is off when neither `http-listen` nor `socks5-listen` is set." | "（手册只给出 `[password@]address[:port]`）"改成："（手册今天写明了地址必须是 IP 字面量、端口缺省 6152、可以写多个监听、两个键都不写时代理服务关闭；口令怎样比较、用户名起什么作用，没有写）" |
| R32 | L219 已从手册消失的键 | `features/enhanced-mode.html`："Existing configuration parameters such as `vif-mode` stay in the profile for backward compatibility but no longer affect the runtime behavior." | 第一格"已从手册消失的键："改成"手册的 `[General]` 页不再列出的键："；备注末尾加："；其中 `vif-mode` 手册的增强模式页还提到（留在 Profile 里只为兼容，不再影响运行），9.1 节另有它的一行；其余六个名字今天的手册里一页也没有" |

### 5.3 第 3 节 规则（R33 ～ R49）

| 号 | 行 | 手册（页）与原话 | 改动 |
| --- | --- | --- | --- |
| R33 | L229 ～ L231 `DOMAIN` `DOMAIN-SUFFIX` `DOMAIN-KEYWORD` | `rules/domain.html`："All domain rule types test the hostname of the request. Matching is case-insensitive, and a trailing root dot in the hostname (`example.com.`) is ignored." | L229 的备注"大小写不敏感，忽略尾部 `.`"后面加"（手册对全部域名类规则这样说；大小写只折叠 ASCII 字母）"；L230、L231 的备注（原来是空的）各写"大小写与尾部的 `.` 同 `DOMAIN`" |
| R34 | L232 `DOMAIN-WILDCARD` | `rules/domain.html`："`*` matches any number of characters, including none. It also crosses dots, so `*.example.com` matches `a.b.example.com`." | 备注（原来是空的）写："`*` 匹配任意多个字符（零个也算）并且跨点：`*.example.com` 匹配 `a.b.example.com`、不匹配 `example.com`（用例 `domain_wildcard_takes_star_question_mark_and_classes`、`wildcard_semantics`）；大小写与尾部的 `.` 同 `DOMAIN`" |
| R35 | L236 `GEOIP` | `rules/ip.html`："`GEOIP` rules use a MaxMind GeoLite2 country database. A copy is bundled with Surge, and Surge periodically downloads updates."；"Use `UNKNOWN` (case-insensitive) to match a destination IP for which the database lookup returns no country or ASN:"；"`GEOIP,UNKNOWN` matches an empty country result; `IP-ASN,UNKNOWN` matches an empty ASN result. Both forms also work in rule sets, where the policy is omitted."；"These rules require a destination IP. An unresolved domain is not an unknown database result: normal DNS resolution and `no-resolve` behavior still apply." | 标记 ✅ 改成 🟡（库的来源与 Surge 不同，同 `IP-ASN` 一行）。备注整格换成表后的那一段 |
| R36 | L237 `IP-ASN` | `rules/ip.html`："`IP-ASN` rules use a bundled GeoLite2 ASN database, which is updated together with app updates. There is no configuration key for it and no separate download mechanism." | 备注整格换成表后的那一段 |
| R37 | L239 `URL-REGEX` | `rules/http.html`："The rule matches when the pattern is found anywhere in the URL; anchor with `^` when you want a prefix match. The regular expression is case-sensitive." | 第二格"完整 URL 正则，区分大小写"改成"在完整 URL 里搜索的正则（任何位置找到就匹配，要前缀匹配用 `^`），区分大小写"；备注"同上"后面加"；没写锚点的模式匹配 URL 的任何位置（引用这一轮的用例）" |
| R38 | L243 `IN-PORT` | `rules/source-and-port.html`："`DEST-PORT`, `SRC-PORT`, and `IN-PORT` share the same value grammar:" | 备注（原来是空的）写："取值的写法同 `DEST-PORT` / `SRC-PORT`：单个端口、闭区间 `a-b`、`>` `<` `>=` `<=`（用例 `every_port_expression_the_manual_lists_decides_both_ways`）" |
| R39 | L248 `HOSTNAME-TYPE` | `rules/protocol-and-network.html`："`DOMAIN`: hostname contains dots and is a regular domain name."；"`SIMPLE`: hostname without a dot, such as `localhost`." | 备注（原来是空的）写："目标是 IPv4 / IPv6 字面量时是 `IPv4` / `IPv6`；名字里没有 `.` 的是 `SIMPLE`，其余的名字都是 `DOMAIN`——rurge 不另判什么算"regular"的域名；末尾的 `.` 先去掉，所以 `localhost.` 是 `SIMPLE`（这一句读代码得出）（用例 `hostname_type_classifies_every_form`）" |
| R40 | L250、L251 `CELLULAR-RADIO` `CELLULAR-CARRIER` | `getting-started/platform-differences.html`（"CELLULAR-RADIO / CELLULAR-CARRIER rules" 一项）："Match by radio access technology or carrier (also available on tvOS)."；规则页（`rules/protocol-and-network.html`）自己的两个标题仍写着 iOS Only | 平台一格"iOS only"改成"iOS / tvOS"（两行；依据是平台差异页，规则页的标题没有跟上） |
| R41 | L252 `AND` / `OR` / `NOT` | `rules/logical.html`："`OR` matches if any sub-rule matches. Evaluation short-circuits in both cases."；"Any rule type that is valid inside a rule set can be used as a sub-rule, including `RULE-SET` and `SCRIPT`. `FINAL` cannot be a sub-rule."；"Sub-rules may carry their own `no-resolve` and `extended-matching` flags, with the same meaning as on top-level rules." | 备注（原来是空的）写："`AND` / `OR` 短路求值（用例 `logical_rules_short_circuit_and_propagate_needs_resolve`）；能写在规则集里的类型都能作子规则，`RULE-SET` 与 `SCRIPT` 也在内（`SCRIPT` 作子规则时同顶层：脚本引擎就绪前不匹配）；子规则带各自的 `no-resolve` / `extended-matching`，写在逻辑规则自己那一行上的这两个参数不传给子规则；"永不匹配"的告警 `W0013` 自手册基线重对起也看子规则里的类型（此前只看顶层：`AND,((PROCESS-NAME,x),(DEST-PORT,443)),P` 不声不响地永不匹配）；策略名里成对的括号读得对（`…,Proxy (US)`）（后两句引用这一轮的用例）" |
| R42 | L253 `SCRIPT` | `scripting/rule.html`："Profile verification fails if a SCRIPT rule references a script name that does not exist." | 备注末尾加："；手册：引用了不存在的脚本名的 `SCRIPT` 规则让 Profile 校验失败——rurge 今天没有这项检查，随阶段 5" |
| R43 | L261 `no-resolve` | `rules/ip.html`："The rule still matches requests that target IP literals." | 加一格备注（这一行原来没有）："目标本来就是 IP 字面量的请求照样匹配，写不写 `no-resolve` 都一样（用例 `ip_rules_on_ip_targets_respect_family`）" |
| R44 | L264 `pre-matching` | `rules/logical.html`："The `pre-matching` flag cannot be placed on a sub-rule, but the whole logical rule may be flagged `pre-matching` if every sub-rule type supports pre-matching and the policy is a REJECT-family policy." | 备注开头加："写在子规则上是 `E0014`；写在逻辑规则上时，策略要是 REJECT 系、每个子规则的类型（逐层往里）都要支持预匹配，否则 `E0014`（同手册；用例 `pre_matching_respects_recursive_support`）。" |
| R45 | L270 未知参数 | `rules/overview.html`："Unknown parameters are silently ignored." | 备注开头的"rurge 照样忽略它，另给一条 `W0003`（Surge 静默）"改成："rurge 照样忽略它，另给一条 `W0003`（手册：静默忽略；多出来的只是一条告警，规则怎样匹配不变，所以标记是 ✅）"。标记不变 |
| R46 | L278 域名类规则不触发 DNS… | `rules/ip.html`："If the request targets an IP literal, the rule matches it directly; `GEOIP` and `IP-ASN` perform their database lookup without a DNS query." | 加一格备注（这一行原来没有）："目标是 IP 字面量时 IP 类规则直接匹配、不查 DNS，`GEOIP` / `IP-ASN` 直接查库（同手册；用例 `ip_rules_on_ip_targets_respect_family`）" |
| R47 | L303 内部集 `SYSTEM` 与 `LAN` | `rules/ruleset.html`："The internal rule set contents may change between Surge versions. The listings below reflect the current version; check the rule set details in the app for the authoritative list."；"Note that this rule set triggers a DNS lookup for domain hostnames (unless `no-resolve` is added to the RULE-SET line)." | 备注改成："rurge 内置的两份清单与手册今天的清单逐行相同、顺序相同（用例 `the_built_in_sets_are_the_lists_the_manual_gives`；手册基线重对时又逐行比过一次）；`LAN` 对域名目标会先解析，`RULE-SET,LAN` 那一行写了 `no-resolve` 就不解析（同手册；golden 用例 "RULE-SET,LAN,no-resolve does not resolve domains"）；`SYSTEM` 在非 Apple 平台可能匹配不到任何流量，其中两条 `PROCESS-NAME` 行在运行着的 rurge 里匹配不上（进程名今天只有 `rurge rule match --process` 给得出）" |
| R48 | L304 外部规则集文件格式 | `rules/overview.html`："Inline comments starting with `//`, `#`, or `;` are allowed at the end of a rule line." | 在"而它对规则集文件只写了整行注释（"Comment lines start with #, // or ;"）"后面加："；手册对 `[Rule]` 的行另有一句——"Inline comments starting with `//`, `#`, or `;` are allowed at the end of a rule line."（`rules/overview.html`）——，说的不是集合文件，但集合文件的一行就是不带策略的规则行，它加强了下面的推断" |
| R49 | L305 DOMAIN-SET 文件格式 | `rules/domain.html`："Lines starting with `#` or `//` are comments; blank lines are ignored." | 备注开头的"注释只认整行的"改成："注释只认整行的，以 `#` 或 `//` 开头（同手册；以 `;` 开头的行在 DOMAIN-SET 文件里不是注释，是一条被跳过的非法行，用例 `semicolon_is_a_comment_only_in_rule_sets`）" |

R35 的备注（`GEOIP` 一行）：

```
MaxMind GeoLite2 Country。**与手册不同的一处：库的来源**——Surge 随包带一份国家库并定期下载更新（`rules/ip.html`："A copy is bundled with Surge, and Surge periodically downloads updates."），rurge 不捆绑，经外部资源管理器下载并热更新；第一次下载之前没有库，`GEOIP` 规则不匹配，`I0003` 说库还没有（来源与更新周期见 `geoip-maxmind-url` / `disable-geoip-db-auto-update`）。**`GEOIP,UNKNOWN`**（手册基线重对起支持；手册 IP 页，Unknown Database Results，iOS 5.102.0+ / Mac 6.9.1+："`GEOIP,UNKNOWN` matches an empty country result"，"Both forms also work in rule sets, where the policy is omitted."）：取值是 `UNKNOWN`（不分大小写）时，匹配这样的目标地址——国家库**已经载入**，而它对这个地址没有给出国家。库没有载入时不匹配：这种情形只在 rurge 里有，这时别的 `GEOIP` 规则也都不匹配，一条 `GEOIP,UNKNOWN,REJECT` 不该在第一次下载完成之前拒掉所有会话。目标是域名时照 `GEOIP` 的老规矩（没写 `no-resolve` 就先解析；手册："An unresolved domain is not an unknown database result"）。规则集文件与内联集里也能写（引用这一轮的用例）。此前这一行能加载，`UNKNOWN` 被读成国家码，规则永不匹配，没有任何提示。**仍然如此的**：`GEOIP` 后面不是两个字母、也不是 `UNKNOWN` 的词（`GEOIP,USA`）照旧能加载、永不匹配、没有提示
```

R36 的备注（`IP-ASN` 一行）：

```
GeoLite2 ASN 库经外部资源管理器下载并热更新，来源与更新周期见 `geoip-maxmind-url` / `disable-geoip-db-auto-update`。Surge 的 ASN 库随包、随应用更新，不单独下载（`rules/ip.html`："`IP-ASN` rules use a bundled GeoLite2 ASN database, which is updated together with app updates. There is no configuration key for it and no separate download mechanism."）；rurge 不捆绑，第一次下载之前 `IP-ASN` 规则不匹配。**`IP-ASN,UNKNOWN`**（手册基线重对起支持；手册 IP 页，同 `GEOIP` 一行："`IP-ASN,UNKNOWN` matches an empty ASN result"）：匹配 ASN 库已经载入、而它对这个地址没有给出 AS 号的目标；库没有载入时不匹配；规则集文件里也能写，按文件里的顺序逐条求值（不进 AS 号的表）（引用这一轮的用例）。此前它是加载错误 `E0011`，含它的 Profile 加载不了，写在规则集文件里是一条被跳过的行
```

### 5.4 第 4 节 策略（R50 ～ R75）

| 号 | 行 | 手册（页）与原话 | 改动 |
| --- | --- | --- | --- |
| R50 | L322 `REJECT` | `policies/reject.html`："If a large number of requests to a hostname trigger the `REJECT`/`REJECT-TINYGIF` policy within a short period of time (the threshold is 50 times within 30 seconds in the current version), Surge automatically upgrades the policy to `REJECT-DROP`" | 备注末尾加："；只有 `REJECT` 与 `REJECT-TINYGIF` 的拒绝被计数、被升级，`REJECT-DROP` 本来就是丢弃，`REJECT-NO-DROP` 从不升级（同手册；用例 `only_reject_and_reject_tinygif_are_upgraded_to_drop`）；按会话的目标主机计（不含端口），30 秒是滑动的窗口；只计 TCP 会话（UDP 流与内部 DNS 会话不计，见本节"UDP 行为"一行）" |
| R51 | L323 `REJECT-TINYGIF` | 同上 | Surge 一栏末尾加"；与 `REJECT` 一样计入自动升级"；备注（原来是空的）写："同 `REJECT` 一行：同一主机 30 秒内第 50 次起按 `REJECT-DROP` 处理（用例同上）" |
| R52 | L332 UDP 行为 | `policies/reject.html`："If a REJECT policy is matched, an ICMP Administratively Prohibited response is generated. If the frequency limit built into the REJECT policy is triggered, packets are discarded."；"If a REJECT-NO-DROP policy is matched, an ICMP Administratively Prohibited response is generated." | 第一格改成："UDP 行为：`REJECT` 回 ICMP Administratively Prohibited（触发它自带的限频后丢包），`REJECT-NO-DROP` 回 ICMP Administratively Prohibited（没有限频），`REJECT-DROP` 直接丢包"。其余不动 |
| R53 | L340 `h2-connect`；L335 小节标题 | `policies/http.html`："Credentials for proxy authentication. They may be given positionally after the port, or as named parameters:"；"The server must advertise extended CONNECT support in its HTTP/2 settings, which Surge verifies when the connection is established." | "（手册的 `h2-connect` 语法没列凭据，兼容性清单与 `http` 一致按支持处理）"改成："（同手册 `policies/http.html`："Credentials for proxy authentication. They may be given positionally after the port, or as named parameters:"，页上的例子就是一行 `h2-connect`；都写时谁优先手册没有说）"。"没有通告时**错误出现在发往每个目标的第一个包上**"后面的括号里，在 `h2-connect: the server does not support extended CONNECT` 之前加一句："手册说 Surge 在连接建立时就核对——"which Surge verifies when the connection is established"——，rurge 打开 UDP 载体时不拨号，所以到第一个包才知道；"。L335 的标题"（16 种）"改成"（16 种，另有一行说旧的 `custom` 写法）" |
| R54 | L342 `ss` 之后新增一行 | `policies/shadowsocks.html`："The legacy `custom` policy type from very old configurations is parsed as a Shadowsocks policy; the external module URL in its declaration is ignored. Use the `ss` type instead." | 新行 `custom`，✅，阶段 2；全文见表后 |
| R55 | L352 `wireguard` | `policies/wireguard.html`："Surge maintains an internal IP stack for each WireGuard policy, encrypts each packet for the peer selected by `allowed-ips`, and sends the encrypted UDP datagrams either directly or through another policy."；"Endpoint lookup uses normal Surge DNS when transport is direct, or the underlying connector when `underlying-proxy` is configured." | 标记 ✅ 改成 🟡。"指向同一个节、载体设置（`ip-version`、`underlying-proxy`、`[General] ipv6`）也相同的策略共用一条隧道"后面加："（**与手册不同的一处**：手册说每个 WireGuard 策略各有自己的协议栈——"Surge maintains an internal IP stack for each WireGuard policy"——，rurge 让这样的策略共用一条隧道，因为同一私钥连着同一 peer 的两条隧道会互相抢 peer 记住的地址；使用者看得到的差别：这些策略的连接出自同一条隧道、同一个握手状态）"。"带 `underlying-proxy` 时（M5c 起）载体经底层策略的 UDP："后面加："endpoint 写成域名时，名字原样交给底层的载体，由它那一端解析（底层最终落到 DIRECT 或另一条 WireGuard 隧道时在本机解析；同手册："Endpoint lookup uses normal Surge DNS when transport is direct, or the underlying connector when `underlying-proxy` is configured."；引用这一轮的用例）；" |
| R56 | L364 `dns-follow-interface` | `policies/parameters.html`："Make the `interface` parameter of the policy also take effect for DNS queries; DNS requests that match the policy will use this interface for queries. (If DNS is triggered during the rule matching stage, a specific interface will not be used.)" | 末尾的"（手册只说"匹配该策略的 DNS 请求使用这个网卡查询"）"改成："（同手册："(If DNS is triggered during the rule matching stage, a specific interface will not be used.)"）" |
| R57 | L367 `hybrid` | `policies/parameters.html`："When omitted (`auto`), the connection follows the global All Hybrid setting; `on` and `off` force the behavior for this policy. `true`/`false` are also accepted." | 取值一格改成"`auto` `on` `off`（`true` / `false` 也收；省略即 `auto`，跟随全局的 All Hybrid）"；备注末尾加"（用例 `hybrid_is_ios_only_so_its_value_is_never_checked`）" |
| R58 | L369 `tos` | `policies/wireguard.html`："Common policy parameters apply, with L3-specific exceptions: `interface` binding and the `shadow-tls-*` parameters are not supported for WireGuard policies." | 备注末尾加："；`wireguard` 行上的 `tos` 自手册基线重对起生效：直连时到达隧道自己的 UDP socket，握手的发起包照旧标 `0x88`、发完回到策略自己的 `tos`；经 `underlying-proxy` 时没有作用（同别的协议）；此前是 `W0028` 并清成 0（引用这一轮的用例）" |
| R59 | L370 `ecn` | `policies/parameters.html`："Optional, `auto` / `on` / `off`, default: `auto`. Proxy policies only."；"Use `ecn=true` or `ecn=false` to override the applicable default explicitly."；`policies/wireguard.html`："On supported OS versions, Surge preserves ECN information for tunneled traffic and applies RFC 6040-style merging on received markings." | 取值一格开头的"`auto` `on` `off`；"改成"`auto` `on` `off`（`true` / `false` 也收，等同 `on` / `off`）；只用于代理策略；"。备注末尾加："；写在 `direct` 别名上同样是 `W0028`（手册：只用于代理策略）；手册说 WireGuard 上 Surge 保留隧道内流量的 ECN 信息、对收到的标记按 RFC 6040 的方式合并，rurge 不做" |
| R60 | L374 `test-udp` | `policies/wireguard.html`："For a peer-to-peer policy, only the RTT test is offered (the comprehensive network diagnostics likewise reports its UDP relay test as skipped); a general proxy policy keeps the standard latency, DNS, UDP, and external-address tests." | 备注末尾加："；手册把 WireGuard（与 Tailscale）的策略分成点对点的与一般代理的两种，点对点的不提供 UDP 测试；rurge 没有"点对点策略"这个概念，对这样的 `wireguard` 策略照样去测，得到的是那一次测试自己的结果" |
| R61 | L387 `alpn` | `policies/tls.html`："Optional, comma-separated protocol list (quote the value if it contains multiple entries)." | 备注开头加："取值是逗号分隔的列表，有几项时整个值加引号（`alpn="h2, http/1.1"`；用例 `all_six_parameters`）；" |
| R62 | L392 约束：Shadow TLS 不能与… | `policies/tls.html`："Shadow TLS cannot be combined with TUIC, WireGuard, or Tailscale policies; attempting to do so is a configuration error. As a TCP-based wrapper, it is also not meaningful for other QUIC-based protocols."；`policies/masque.html`："Shadow TLS cannot be combined with QUIC-based protocols." | Surge 一栏"配置错误"改成："TUIC / WireGuard / Tailscale：配置错误；其余 QUIC 类协议：TLS 参数页说"没有意义"，MASQUE 页说"不能组合""。备注末尾加："；手册对其余 QUIC 类协议的两句说法不一，rurge 一律 `E0018`（合 MASQUE 页的那一句）"。标记不变 |
| R63 | L409、L411 `http` `https` `h2-connect` 的凭据与 `headers` | `policies/http.html`："Credentials for proxy authentication. They may be given positionally after the port, or as named parameters:"；"For HTTP and HTTPS proxy policies, a configured header replaces an original field with the same name, including the `Host` field." | L409："手册的 `h2-connect` 语法没列凭据，rurge 与 `http` 一样两种写法都收、命名的优先"改成"两种写法都收（同手册，页上的例子是一行 `h2-connect`），都写时命名的优先（手册没有说，rurge 自定）"。L411 备注末尾加："；`http` / `https` 上写的字段替换同名的原字段，`Host` 也在内（同手册；用例 `custom_headers_replace_and_random_strings_are_rendered_per_connection`）；`h2-connect` 上 `host` 与连接专用字段不让写（见 4.2）" |
| R64 | L413 `h2-connect` 的 `udp-relay` | `policies/http.html`："The server must advertise extended CONNECT support in its HTTP/2 settings, which Surge verifies when the connection is established." | "服务端没有通告 extended CONNECT 时错误出现在发往每个目标的第一个包上"后面加："（Surge 在连接建立时核对；rurge 的 UDP 载体打开时不拨号，所以到第一个包才知道）" |
| R65 | L414 `socks5` `socks5-tls` | `policies/socks5.html`："Credentials for SOCKS5 username/password authentication. They may be given positionally after the port, or as named parameters." | 备注（原来是空的）写："`username` / `password` 可以写在端口之后（位置）或写成命名参数（同手册），都写时命名的优先（用例 `named_credentials_win_over_positional_ones`）" |
| R66 | L421 `trojan` | `policies/trojan.html` 的 `password`："Required." | 备注开头加："`password` 必填（没有或为空是 `E0018`），只读命名写法——写在端口之后的位置值是 `W0001`、不引用（用例 `a_missing_password_is_an_error_and_a_positional_one_is_not_read`）；" |
| R67 | L426 `anytls` | `policies/anytls.html`："Required. The authentication password, which must match the server's setting." | "`password` 只接受命名写法"改成"`password` 必填（`E0018`），只接受命名写法（用例 `a_missing_password_is_an_error_and_a_positional_one_is_not_read`）" |
| R68 | L427 `trust-tunnel` | `policies/trust-tunnel.html` 的 `h3`："Cannot be combined with the `ws` (WebSocket) parameter." | 备注末尾加："；手册在 `h3` 下写着它不能与 `ws` 合用，而这一页的参数表里没有 `ws`：rurge 把 `trust-tunnel` 行上的 `ws` / `ws-path` / `ws-headers` 当未知参数（`W0001`，忽略），写没写 `h3` 都一样（读代码得出，没有用例）" |
| R69 | L428 `ssh` | `policies/ssh.html`："Starting with Surge iOS 5.21.0 and Surge Mac 6.8.0, Surge emits a one-time security warning when connecting without a configured fingerprint."；"All four types of private keys, RSA/ECDSA/ED25519/DSA, are supported." | "没配时照 Surge 接受，每个出站在第一次建会话时告警一次"后面加"（Surge 自 iOS 5.21.0 / Mac 6.8.0 起同样告警一次）"；"`private-key` 的条目见 1.6 节 Keystore 行"后面加"（手册说 DSA 私钥也支持，rurge 不支持，这处差异登记在那一行）"。标记不变 |
| R70 | L429 `wireguard` 策略行 | `policies/wireguard.html`："Common policy parameters apply, with L3-specific exceptions: `interface` binding and the `shadow-tls-*` parameters are not supported for WireGuard policies." | "`interface` / `allow-other-interface` / `tfo` / `tos` 不适用（`W0028`，忽略）"改成："`interface` / `allow-other-interface` / `tfo` 不适用（`W0028`，忽略；`interface` 是手册点了名的例外，另两个在这里没有东西可作用）；`tos` 自手册基线重对起生效（见 4.3 节 `tos` 一行）；`no-error-alert` 读了而没有作用（错误通知随阶段 6）" |
| R71 | L430 `[WireGuard <name>]` | `policies/wireguard.html` 的 `private-key`："Required."；"Each peer is a parenthesized field list; multiple peers are separated by commas, and multiple `peer =` lines accumulate:" | 备注末尾加："；`private-key` 必填、`self-ip` / `self-ip-v6` 至少一个、至少一个 `peer`，缺了各是 `E0023`（用例 `what_a_section_must_have`）；一行 `peer =` 可以写几个括号，几行 `peer =` 累加（同手册；用例 `peers_accumulate_over_lines_and_within_one`）" |
| R72 | L431 `peer` 字段 | `policies/wireguard.html`："Endpoint lookup uses normal Surge DNS when transport is direct, or the underlying connector when `underlying-proxy` is configured." | "主机名不受 `[Host]` 影响（与代理服务器的主机名相同）"后面加："（用例 `wireguard_endpoints_are_proxy_hostnames`）；谁来解析这个名字：直连时是 rurge 自己的解析器，带 `underlying-proxy` 时名字交给底层的载体（同手册，见 4.2 节 `wireguard` 一行）" |
| R73 | L432 WireGuard 生命周期 | `policies/wireguard.html`："Surge only answers ICMP/ICMPv6 echo requests sent to the configured local tunnel address." | Surge 一栏"仅回应发往本地隧道地址的 ICMP echo"改成"仅回应发往本地隧道地址的 ICMP / ICMPv6 echo"；备注"只回应发往本端隧道地址的 ICMP echo"改成"只回应发往本端隧道地址的 ICMP 与 ICMPv6 echo（IPv4：用例 `only_echo_requests_to_the_tunnel_address_are_answered`；IPv6：引用这一轮的用例）" |
| R74 | L433 WireGuard 测试 | `policies/wireguard.html`："completes when the first peer returns a valid WireGuard packet, reporting the round-trip time and responding endpoint" | "测试会话的目标记为第一个 peer 的 endpoint"改成："手册说测试报告往返时间与作答的 endpoint；rurge 只报时间，测试会话的目标总是第一个 peer 写的 endpoint，哪个 peer 先答都一样（用例 `the_native_test_is_a_handshake_with_the_peers` 只有一个 peer）"。标记不变：差的是显示什么，不是测试怎样判定 |
| R75 | L435 `external` | `policies/external.html`："Required. The path of the external program's executable."；"Required. The SOCKS5 port that the external program listens on at 127.0.0.1. Surge forwards requests to this port." | 备注开头加："`exec` 与 `local-port` 必填，缺了各是 `E0018`（用例 `exec_and_local_port_are_required`）；" |

R54 的新行（接在 `ss` 一行之后）：

```
| `custom` | Shadowsocks 的旧写法（不是另一种协议） | 很旧的配置 | ✅ | 2 | 手册（`policies/shadowsocks.html`）："The legacy `custom` policy type from very old configurations is parsed as a Shadowsocks policy; the external module URL in its declaration is ignored. Use the `ss` type instead."。手册基线重对起支持（此前是未知类型 `E0004`，含它的 Profile 加载不了）：`Name = custom, <服务器>, <端口>, <加密方法>, <口令>[, <模块链接>][, 命名参数…]` 读作同一台服务器、同一种方法与口令的 `ss` 策略——端口之后的位置字段第一个是加密方法、第二个是口令、第三个（模块的链接）读过即丢；同一行上命名的 `encrypt-method=` / `password=` 胜过位置上的；其余一切同 `ss`（方法名的校验、`obfs`、`udp-relay`、通用参数）。主配置里每一行一条 `I0001`（``policy `<名字>`: legacy type `custom` read as `ss`; the module URL is ignored``），订阅导入的 `custom` 行同样读作 `ss`、不另说什么。这种策略在诊断、API 与日志里的类型是 `ss`，`GET /v1/policies/detail` 显示的那一行是使用者写的原文（脱敏之后）。**字段的位置手册没有写**：它是 Surge 2 / 3 时期的写法，一个开源的订阅转换器也这样读；位置读错的行得到 `encrypt-method` 的错误，不会悄悄连错。口令里带 `=` 的旧行读不出口令（字段里有 `=` 的被读成命名参数，同 `http` 的位置凭据；脱敏照同一种读法遮字段）（引用这一轮的用例） |
```

### 5.5 第 5 节 策略组（R76 ～ R88）

5.2 节的表没有备注一栏，rurge 的说明写在"取值 / 默认"一格里（"成员装配顺序"与"测试 URL / 超时解析顺序"两行写在第三格）；下面说"取值一格"的就是它。改动里带引号的长句放在表后，各自标着号。

| 号 | 行 | 手册（页）与原话 | 改动 |
| --- | --- | --- | --- |
| R76 | L446 `url-test` | `policy-groups/url-test.html`："The group selects the member with the lowest score among those that passed the test."（没有说一个都没有通过时怎样）；`policy-groups/fallback.html`："If no member qualifies, the first member is used regardless, so traffic is never left without a policy."；`policy-groups/load-balance.html`："If no member is currently available, all members are used as candidates." | 备注"没有可用成员时用第一个成员（手册未说明）"改成"没有可用成员时用第一个成员（手册对 `url-test` 没有说这种情形；对 `fallback` 说的是照样用第一个成员，对 `load-balance` 说的是全部成员都作候选；用例 `url_test_holds_its_member_within_the_tolerance` 的最后一段）" |
| R77 | L449 `smart` | `policy-groups/smart.html`："The base of the score is a time-weighted moving average of the first-response latency of real connections — the time from connecting until the first response byte arrives. URL test results also feed into the same score."；"A packet-loss penalty is added based on the observed TCP retransmission ratio (roughly 50 ms per 1% of loss)."；"The remaining healthy policies and then the failed ones form an ordered retry list, so if the chosen policy fails, the connection can fall back to the next candidate."；"a policy whose average delay grows too large is marked as failed until later tests or traffic recover it."；"When the group has many members (more than 12), regular rounds test only a subset — the most-used policies plus the least-recently-tested ones — while a manually triggered test always tests every member." | Surge 一栏两处："接近最优者构成优选集，其余为重试列表"改成"接近最优者构成优选集，其余健康的、然后是失败的依次构成重试列表；平均延迟过大的算失败"；">12 成员只测子集"改成">12 成员时常规轮只测子集（用得最多的加上最久没测的），手动触发的测试总是测全部成员"。备注开头的"M3c 已实现，算法细节手册未公开，rurge 按手册描述近似实现（常数见 M3c 设计第 9 节）。差异：用失败罚分（每次 800 ms，5 分钟减半）近似重传率"换成表后 R77 的那一段 |
| R78 | L450 `subnet` | `policy-groups/subnet.html`："Subnet groups do not support the other common group parameters (`policy-path`, `include-*`, `policy-regex-filter`, `no-alert`, ...); only `hidden`, `icon-url` and `category` are available besides the entries below."；"A bare value without a prefix matches SSID/BSSID/router IP, for legacy compatibility."；`default` 一项："Required" | 备注末尾加表后 R78 的那一段 |
| R79 | L451 嵌套与循环 | `policy-groups/overview.html`："If a loop is detected (for example, `A` includes `B` and `B` includes `A`), Surge logs a warning and the affected group temporarily behaves as a reject policy (shown as `FAILED` in logs). If a group ends up with no usable member at all, Surge falls back to `DIRECT` (shown as `SUBSTITUTE` in logs) and raises a one-time warning." | Surge 一栏末尾加"（日志里分别显示为 `FAILED` 与 `SUBSTITUTE`；后者另有一次性的告警）"；备注末尾加"；手册的两个日志标记 `FAILED` / `SUBSTITUTE` rurge 不用，用自己的话（`policy group cycle: …`、`policy group has no members; DIRECT substituted`）" |
| R80 | L452 临时覆盖 | `policy-groups/parameters.html`（Temporary Override）："The automatic group types — `url-test`, `fallback`, `load-balance`, `subnet`, and `smart` — can be temporarily overridden by manually selecting a policy:"；"While an override is active, the group always uses the selected policy and automatic testing for the group is no longer triggered by use. Overrides for groups that no longer exist are cleared when the profile reloads." | 备注两处："组定义（不计行位置）不变的重载保留覆盖，组消失或定义变了就清除"后面加"（手册只说到不再存在的组：它们的覆盖在重载时清除；定义变了的组手册没有说，清除是 rurge 自定）"；末尾加"；手册把 `subnet` 也列在可以临时覆盖的自动组里：rurge 对 `subnet` 组答 400（`` `<组>` does not take a selection ``；用例 `only_a_member_can_be_selected`），随 `subnet` 组一起在阶段 3 做"。"（Surge 未定义自动组上的这个端点，属 rurge 的扩展）"不动：手册给的入口是界面与 CLI，HTTP API 页只说 select 组 |
| R81 | L459 `tolerance` | `policy-groups/url-test.html`："The selected policy changes only when it failed the test, or when the new best policy is faster than the currently selected one by more than the tolerance." | 取值一格"M3b 已实现（显式 0 生效）"改成"M3b 已实现：只在当前选中的成员测试失败、或最快的成员比它快出超过 `tolerance` 时才换（同手册；用例 `url_test_holds_its_member_within_the_tolerance`）；显式 0 生效" |
| R82 | L466 `no-alert` | `policy-groups/parameters.html`："so this parameter has no effect on other group types — `load-balance`, `smart`, and `select` groups never notify." | 取值一格末尾加"；写在别的类型的组上没有作用（同手册：`load-balance`、`smart` 与 `select` 组从不通知，`subnet` 组不支持它），rurge 照收、不为此告警" |
| R83 | L471 `underlying-proxy` | `policy-groups/parameters.html`："Optional, the name of another proxy policy or policy group. Not available on subnet groups."；"It applies to members from every source — explicitly listed members, `policy-path`, `include-all-proxies`, and `include-other-group` — and can also be configured in the group editor UI."；"If both are present, the group-level `underlying-proxy` takes precedence over an `underlying-proxy` set through `external-policy-modifier`." | 适用一格"全部（iOS 5.22 / Mac 6.9+）"改成"除 subnet 外（iOS 5.22 / Mac 6.9+）"。取值一格三处："M3a 已实现：组上的中继覆盖成员自己的"改成"M3a 已实现：作用于每一种来源的成员（显式写的、`policy-path`、`include-all-proxies`、`include-other-group`；同手册，用例 `every_proxy_member_is_chained_through_the_group_relay` 与这一轮的用例）；组上的中继覆盖成员自己的，也胜过 `external-policy-modifier` 设的 `underlying-proxy`（同手册；引用这一轮的用例）"；"派生名已被占用时略去该成员并告警（不绕过中继）"后面加"；中继自己也在成员里时（如 `include-all-proxies` 把它带进来）同样派生出 `R (via R)`（读代码得出，没有用例；手册没有说这种情形，用 `policy-regex-filter` 可以把它滤掉）"；末尾加"；写在 `subnet` 组上是 `W0028`（手册：subnet 组不能用）" |
| R84 | L472 `policy-path` | `policy-groups/policy-including.html`："A policy list: one policy line per line, in the same format as the `[Proxy]` section. Blank lines and lines starting with `#` or `//` are ignored. Invalid lines are logged and skipped; policies whose names duplicate an existing policy are skipped with a warning." | 取值一格"坏行、重名、与配置同名的行跳过并告警（只报行号与原因）"后面加表后 R84 的那一句 |
| R85 | L473 `update-interval` | `policy-groups/policy-including.html`："Optional, in seconds, default: 86400"；"The update interval for a remote `policy-path`. Only meaningful when the path is a URL." | 取值一格里对手册的引述换成今天的两句，见表后 R85 |
| R86 | L474 `policy-regex-filter` | `policy-groups/policy-including.html`："Only use the policies whose name matches the regex. It applies to members from `policy-path`, `include-all-proxies`, and `include-other-group`, but not to explicitly listed members. The value must be a valid regular expression." | 取值一格"作用于导入成员，不作用于显式成员"改成"作用于 `policy-path`、`include-all-proxies` 与 `include-other-group` 取来的成员，不作用于显式写的（同手册；用例 `the_filter_and_the_prefix_act_in_the_manual_order`、`members_taken_from_another_group_pass_the_filter`）"；末尾加"；编译不了的正则是加载错误 `E0018`（手册要求取值是合法的正则；用例 `values_that_cannot_be_used_are_errors`）" |
| R87 | L475 `external-policy-modifier` | `policy-groups/policy-including.html`："This parameter only affects members imported via `policy-path`." | 取值一格"M3a 已实现（在文本层改写导入行：同名参数原地替换、没有的追加）"后面加"；只改这个组自己经 `policy-path` 导入的行，显式写的与 `include-*` 取来的成员不动（同手册；用例 `the_modifier_rewrites_the_imported_lines_only` 钉住显式成员不动）" |
| R88 | L480 测试 URL / 超时解析顺序 | `profile/general.html`（`internet-test-url`）："HTTPS URLs are supported. iOS 5.23.0+ Mac 6.10.0+ The TLS handshake is performed before the HEAD request, and the server certificate is verified; the reported latency is still measured on a second HEAD request over the established connection, so it excludes the TLS handshake." | 第三格"（手册 Mac 6.10 的行为）"改成"（同手册，iOS 5.23.0+ / Mac 6.10.0+：TLS 握手在 HEAD 之前完成并校验服务端证书，报告的延迟仍是已建立的连接上第二次 HEAD 的，不含握手）" |

R77（换掉 L449 备注开头的那一段）：

```
M3c 已实现。手册给了算法的轮廓与几个数（见 Surge 一栏），没有给的——"接近最优"是多近、平均延迟多大算失败、时间加权怎样衰减——rurge 自定（常数见 M3c 设计第 9 节；钉在用例上的三个：分数到 3 秒算失败 `a_score_of_three_seconds_fails_a_member`，连续三次失败算失败、一次成功恢复 `three_failures_in_a_row_fail_a_member_and_a_success_restores_it`，站点记忆一小时 `a_site_is_remembered_for_an_hour`）。差异：用失败罚分（每次 800 ms，5 分钟减半）近似重传率；手册的重试列表只有健康的与失败的两类，rurge 另有第三类"还没有样本的"，排在健康的之后、失败的之前（用例 `the_unknown_come_after_the_healthy_and_the_failed_last`）；大组常规轮的抽样同手册（用得最多的加上最久没测的；rurge 各取一半，没测过的排在最前）
```

R78（加在 L450 备注末尾）：

```
；手册：`subnet` 组在自己的条件与 `default` / `cellular` 之外只有 `hidden`、`icon-url`、`category` 三个参数——rurge 对写在它上面的八个成员来源参数（`policy-path`、`update-interval`、`policy-regex-filter`、`external-policy-name-prefix`、`external-policy-modifier`、`include-all-proxies`、`include-other-group`、`underlying-proxy`）与测试参数（`interval`、`tolerance`、`timeout`、`evaluate-before-use`、`persistent`、`policy-priority`）各给一条 `W0028`（`` `<键>` has no effect on a `subnet` group; ignored ``；用例 `parameters_a_group_type_ignores_are_warned_about` 钉住其中的 `policy-path` 与 `underlying-proxy`），`no-alert` 不告警地收下；rurge 不认识的 `key=value`（如写错的参数名）被读成一个网络条件：取值不是已有的策略或组时是 `E0008`，恰好是一个策略名时没有任何诊断——手册保留了不带前缀的旧写法（"A bare value without a prefix matches SSID/BSSID/router IP, for legacy compatibility."），所以分不出写错的参数与旧写法的条件；手册说 `default` 必填：没写 `default` 的 `subnet` 组 rurge 加载时不报（运行时按空组告警并兜底），是不是加载错误随阶段 3 定；`W0008` 对 `subnet` 组说的话自手册基线重对起改了（``policy group type `subnet` is not implemented in this version; its `default` policy is used on every network``；此前说的是 `the first member is used`，而 `subnet` 组没有成员表，用的一直是它的 `default`；引用这一轮的用例）
```

R84（接在"坏行、重名、与配置同名的行跳过并告警（只报行号与原因）"之后）：

```
（同手册："Invalid lines are logged and skipped; policies whose names duplicate an existing policy are skipped with a warning."）；策略行列表里空行与以 `#` 或 `//` 开头的行不算（同手册），rurge 另把以 `;` 开头的行也当注释（Profile 的注释写法；用例 `a_plain_list_is_read_line_by_line`）
```

R85（L473 里"手册在策略组这里只写 "The update interval for a remote `policy-path`."，没有说 0 与负数"一句换成）：

```
手册在策略组这里写的是 "Optional, in seconds, default: 86400" 与 "The update interval for a remote `policy-path`. Only meaningful when the path is a URL."，没有说 0 与负数
```

### 5.6 第 6 节 DNS 与 `[Host]`（R89 ～ R97）

| 号 | 行 | 手册（页）与原话 | 改动 |
| --- | --- | --- | --- |
| R89 | L494 连续 5 次 AAAA 超时 | `dns/overview.html`（Caching）："Identical in-flight lookups are coalesced into a single query." | 备注里"Surge 怎样数同时发出的同名查找，手册没有写，不知道：若它数每个调用方，"换成表后 R89 的那一句（后面的"rurge 在一个从不回答 AAAA 的网络上、许多连接同时去一个新主机时比它晚停（五个不同的查询之后照样停）。"不动） |
| R90 | L496 缓存 | `dns/overview.html`（Caching）："The cache holds up to 200 entries on iOS and 2000 entries on macOS, with least-recently-used eviction. The cache is flushed automatically on network changes, and can be flushed manually from the UI, the CLI, or the HTTP API. Identical in-flight lookups are coalesced into a single query." | 第一格"LRU 上限 2000（macOS）"改成"LRU 上限 200（iOS）/ 2000（macOS）"，末尾加"；可以从界面、CLI 或 HTTP API 手动清空；同时在途的相同查找合并成一次查询"；备注"容量可配置，默认 2000。"后面加"同时在途的相同查找合并成一次查询（同手册；用例 `cache_fresh_stale_negative_and_coalescing`）；不合并的两种：绕过缓存的查找（`rurge dns lookup --no-cache`）与交给系统解析库的查找（`.local` 与 `server:system`）。" |
| R91 | L510 `https://` `tls://` `tcp://` | `dns/encrypted-dns.html`："A `tcp://` URL may only contain a host and an optional port." | 备注末尾加"；**`tcp://` 只收主机与可选的端口**（手册基线重对起）：主机（与端口）之后还写着路径、查询串或片段的 `tcp://` 是 `W0026`（`` `<写的那一段>`: a tcp:// URL names a host and a port, nothing else ``，同下一行 `quic://` 的检查），这个上游不用；结尾单独一个 `/` 照旧接受；此前多出来的部分被悄悄丢掉（引用这一轮的用例）；`tls://` 不变（手册没有对它说这句话）" |
| R92 | L516 `encrypted-dns-follow-outbound-mode` | `dns/encrypted-dns.html`："When enabled, encrypted DNS connections follow the outbound mode settings and are matched against the rules like normal requests. You can then configure a rule for the DNS server's hostname to use a proxy, or match the connections with a PROTOCOL rule:"；"- `PROTOCOL,DOH` — DNS over HTTPS (`https://`)"、"- `PROTOCOL,DOH3` — DNS over HTTP/3 (`h3://`)"、"- `PROTOCOL,DOQ` — DNS over QUIC (`quic://`)"、"- `PROTOCOL,DOT` — DNS over TLS (`tls://`)"、"- `PROTOCOL,DNS` — DNS over TCP (`tcp://`)"；"To avoid the fallback, use an IP address as the proxy's server address, or add a `[Host]` IP mapping for the proxy's domain."；`dns/local-dns-mapping.html`："The hostnames of your proxy servers are never matched against `[Host]` items, to avoid resolution loops." | 备注五处，标记不变（🟡）。（一）"协议标签按端口启发（853→DoT，443→DoH，其余→DNS）"改成"协议标签按上游的方案（手册基线重对起）：`https://` → `DOH`，`tls://` → `DOT`，`tcp://` → `DNS`，端口是多少都一样（同手册；此前按端口猜——853 → `DOT`、443 → `DOH`、其余 → `DNS`——，不在 443 的 DoH 服务器被标成 `DNS`，`PROTOCOL,DOH` 匹配不上它）"；（二）"协议标签按端口启发（443→`DOH3`，其余→`DOQ`，不在 443 的 DoH3 服务器因此标成 `DOQ`；`PROTOCOL,UDP` 也匹配它们）"改成"协议标签同样按方案（手册基线重对起）：`h3://` → `DOH3`，`quic://` → `DOQ`（此前按端口猜：443 → `DOH3`、其余 → `DOQ`，不在 443 的 DoH3 服务器被标成 `DOQ`，开在 443 的 DoQ 服务器被标成 `DOH3`）；`PROTOCOL,UDP` 也匹配它们"；（三）"M3b：TCP/DoT/DoH 上游走流水线（Internal 会话，`PROTOCOL` 可匹配）；"后面加表后 R92（三）；（四）"上游主机名先由 Bootstrap 解析，流水线只见 IP 目标，域名规则不匹配上游主机名"后面加表后 R92（四）；（五）"以 IP 配置的代理照常承载 DNS 会话"后面加表后 R92（五） |
| R93 | L517 `[Host]` 中 `server:<加密 URL>` | `dns/encrypted-dns.html`："You can assign an encrypted DNS server to specific domains with a `server:` mapping in the `[Host]` section, while other domains keep using the normal servers."（没有版本标记） | Surge 一栏的"iOS 5.21 / Mac 6.8+"删掉（留空）：手册今天不给这种写法标版本，那个版本标记标的是"逗号分隔的多个服务器"（R95） |
| R94 | L525 键中的 `*` `?` 通配 | `dns/local-dns-mapping.html`："For example, `*google.com` matches `google.com`, `foo.google.com`, and `bargoogle.com`. `*.google.com` does not match `google.com`." | 第一格括号里的话改成"（整串匹配：`*google.com` 匹配 `google.com`、`foo.google.com` 与 `bargoogle.com`，`*.google.com` 不匹配 `google.com`）"；备注开头加"手册的这四个例子都在用例 `wildcard_semantics` 里（`google.com`、`foo.google.com` 与 `*.google.com` 三处是手册基线重对时补进去的）。" |
| R95 | L527 `server:` | `dns/local-dns-mapping.html`："Multiple DNS servers can be specified as a comma-separated list. iOS 5.21.0+ Mac 6.8.0+" | Surge 一栏"指定上游"改成"指定上游；逗号分隔的多个服务器自 iOS 5.21.0 / Mac 6.8.0 起" |
| R96 | L530 `script:` | `scripting/dns.html`："Profile verification fails if a [Host] entry references a script name that does not exist." | 备注末尾加"；手册：引用了不存在的脚本名的 `[Host]` 条目让 Profile 校验失败——rurge 今天没有这项检查（脚本还没有，整条跳过并告警），随阶段 5" |
| R97 | L533 `use-local-host-item-for-proxy` | `dns/local-dns-mapping.html`："After enabling this option, for requests whose target domain matches a local DNS mapping record, Surge sends the proxy request with the mapped IP address instead of the domain. If the record has multiple addresses, one is chosen at random."；"It only works for local DNS mapping records using IP addresses; `server:` and `script:` items are not affected." | 备注（原来是空的）写"有几个地址的记录每条会话随机取一个（同手册；手册基线重对起，此前总是取第一个；引用这一轮的用例），不看策略的 `ip-version`；别名、`server:` 与 `script:` 条目不受影响（同手册）；逐键的说明见 2.1 节同名一行" |

R89（换掉 L494 里那半句）：

```
手册今天写了同时在途的相同查找合并成一次查询（同页 Caching 一节："Identical in-flight lookups are coalesced into a single query."），与 rurge 相同；合并之后那"连续 5 次查找"按调用方数还是按查询数，手册仍没有写，不知道：若 Surge 数每个调用方，
```

R92（三）（接在"M3b：TCP/DoT/DoH 上游走流水线（Internal 会话，`PROTOCOL` 可匹配）；"之后）：

```
跟随的是出站模式（手册："encrypted DNS connections follow the outbound mode settings and are matched against the rules like normal requests"）：直连模式下 DNS 会话走 DIRECT，全局模式下走全局策略，规则模式下才过规则（引用这一轮的用例）；
```

R92（四）（接在"上游主机名先由 Bootstrap 解析，流水线只见 IP 目标，域名规则不匹配上游主机名"之后）：

```
（**与手册不同**：手册说可以 "configure a rule for the DNS server's hostname to use a proxy"；rurge 的 DNS 会话带的是 Bootstrap 解析出的地址，按 DNS 服务器主机名写的域名规则匹配不上它，要用 `PROTOCOL` 或 `IP-CIDR` 规则；让会话带上主机名随阶段 3 的会话模型）
```

R92（五）（接在"以 IP 配置的代理照常承载 DNS 会话"之后）：

```
；手册给了两个避免回退的办法——"use an IP address as the proxy's server address, or add a `[Host]` IP mapping for the proxy's domain"——，而它的 `[Host]` 页说代理服务器的主机名从不对照 `[Host]`（"The hostnames of your proxy servers are never matched against `[Host]` items, to avoid resolution loops."），两句对不上；rurge 照后一句（见 6.3 节与 4.6 节 `peer` 一行），所以在 rurge 里只有第一个办法管用：把代理的 `server` 写成 IP 地址
```

### 5.7 第 7、8 节 HTTP 处理与脚本（R98 ～ R104）

这两节都是后面阶段（4、5）的条目，今天没有代码；改动只是把手册今天的句子写到行上，留给那个阶段的设计。

| 号 | 行 | 手册（页）与原话 | 改动 |
| --- | --- | --- | --- |
| R98 | L558 协议嗅探 | `profile/general.html`（`always-raw-tcp-hosts`）："Surge will automatically sniff the protocol for TCP requests sent to ports 80 and 443, enabling advanced HTTP/HTTPS features while optimizing performance."；`http/overview.html`："When a new TCP connection arrives, Surge sniffs the first bytes; if they look like an HTTP request, the connection is passed to the HTTP engine."；"Cleartext HTTP on non-standard ports may not be detected by sniffing." | 第一格不动。备注（原来是空的）写"手册两处：`[General]` 页说对发往 80 与 443 端口的 TCP 请求嗅探协议，HTTP 概览页说新的 TCP 连接先看开头的字节、像 HTTP 请求就交给 HTTP 引擎，非标准端口上的明文 HTTP 可能嗅探不到、要用 `force-http-engine-hosts`；阶段 4 的设计两句都要对" |
| R99 | L615 Body Rewrite 之后的头 | `http/body-rewrite.html`："When the body is modified, Surge normalizes the related headers automatically: the body is decompressed first if necessary, and `Content-Length` is recalculated." | 第一格"改写后自动解压并重算 `Content-Length`"改成"改写前按需要先解压，改写后重算 `Content-Length`（相关的头自动规整）" |
| R100 | L642 `timeout` | `scripting/overview.html`："With the JSC engine, the timeout also terminates runaway synchronous JavaScript; the WebView engine enforces the timeout with a wall-clock timer only." | 取值一格"可中断同步代码"改成"JSC 引擎下超时也终止跑飞的同步代码，WebView 引擎只按墙钟计时"；备注（原来是空的）写"rurge 只有一个引擎，阶段 5 的设计定它照哪一种（建议：能中断同步代码）" |
| R101 | L676 `$surge.…` | `scripting/api.html`："Returns an object describing all select groups: `{groups: {groupName: [subPolicyNames]}, decisions: {groupName: selectedPolicy}}`."；"Forces a retest of an automatic testing group. The callback result contains `availablePolicyNames`."；"Sets the outbound mode: `"direct"`, `"global-proxy"`, or `"rule"`." | 备注（原来是空的）写表后 R101 的那一段 |
| R102 | L680 `setTimeout` / `clearTimeout` | `scripting/api.html`："`clearTimeout` is available only under the WebView engine." | Surge 语义一格末尾加"；`clearTimeout` 只在 WebView 引擎下有"；备注"`clearTimeout` 始终可用"后面加"（rurge 只有一个引擎，总是提供——比手册宽，照手册写的脚本不受影响）" |
| R103 | L687 `http-request` | `scripting/http-request.html`："An http-request script inspects and modifies an HTTP request before Surge performs rule matching and sends it upstream." | 备注末尾加"；手册：脚本在规则匹配之前运行，阶段 5 设计流水线的顺序时照它" |
| R104 | L693 `generic` | `scripting/api.html`（`$intent`）："Present when the script is triggered from the system Shortcuts app. `$intent.parameter` contains the parameter passed by the shortcut." | 备注（原来是空的）写"`$intent` 不提供（8.2 节 `$intent.parameter` 一行，⛔），`$trigger` 不会是 `editor` / `intent`（同节 `$trigger` 一行）" |

R101（L676 的备注）：

```
手册给了形状：`selectGroupDetails()` 返回 `{groups: {组名: [成员名]}, decisions: {组名: 选中的策略}}`；`retestGroup` 的回调结果带 `availablePolicyNames`；`setOutboundMode` 的取值是 `"direct"`、`"global-proxy"`、`"rule"`（HTTP API 的 `POST /v1/outbound` 用的是 `proxy`，两处不同名）；阶段 5 照这些实现
```

### 5.8 第 9 节 高级网络功能（R105 ～ R109）

| 号 | 行 | 手册（页）与原话 | 改动 |
| --- | --- | --- | --- |
| R105 | L705 ICMP | `features/enhanced-mode.html`："ICMP traffic cannot be proxied. Surge forwards ICMP packets directly and the VIF returns responses itself, so tools like ping keep working."；`profile/general.html`（`icmp-forwarding`）："Disable this option to stop ICMP forwarding; ping will no longer work through Surge." | 第一格"ICMP 由 VIF 本地应答（ping 可用）；`icmp-forwarding=false` 关闭"改成"ICMP 不能经代理：Surge 直接转发 ICMP 包，应答由 VIF 交回，所以 ping 照常可用；`icmp-forwarding=false` 关掉转发，此后 ping 经 Surge 不通"；备注（原来是空的）写"手册的两句（左栏）说的是转发出去、再把应答交回，不是在本机伪造应答；阶段 3 的设计照这两句" |
| R106 | L742 `dns-server` / `encrypted-dns-server` | `features/subnet-settings.html`："Each line consists of a subnet expression followed by comma-separated `key=value` parameters."；`dns-server`："Optional, comma-separated IP addresses or `system`"；`encrypted-dns-server`："Optional, comma-separated URLs or `off`" | 备注（原来是空的）写表后 R106 的那一段 |
| R107 | L759 9.7 节的标题 | `features/snell-server.html`："Besides Surge Mac, the server is available on Surge iOS and Surge tvOS, which makes an always-on Apple TV a convenient home access point. iOS 5.21.0+" | 标题里的"（Mac only）"改成"（Mac；iOS 5.21.0+ 与 tvOS 也有）" |
| R108 | L763 `interface` `port` `psk` | `features/snell-server.html`（Using a Module，iOS 5.23.0+ / Mac 6.10.0+）："The module's section replaces the one in the profile as a whole." | 备注末尾加"；手册（iOS 5.23.0+ / Mac 6.10.0+）：这个节也可以由模块整个提供，模块的节整个替换 Profile 里的那一个；随模块（阶段 5，见 1.5 节）" |
| R109 | 新的小节，接在 9.9 的最后一行（L785）之后、`---`（L787）之前 | `features/ip-rewrite.html`（整页，iOS 5.23.0+ / Mac 6.10.0+）："The lines are evaluated from top to bottom, and the first matching line wins. IPv4 and IPv6 addresses are both supported."；"The packet is sent straight back to its sender with the source and destination addresses swapped."；"The packet is silently discarded."；"It works at the IP layer: a matching packet is handled as soon as it arrives, before it reaches Surge's TCP/UDP stacks, rules or policies, and it never shows up as a request."；"Requests sent to Surge's proxy server are not affected."；"The packets must be routed to Surge VIF in the first place. If an address is covered by `tun-excluded-routes` or a local network route, add it to `tun-included-routes`." | 新增 9.10 节，见表后 R109；六行都是 ✅ / 阶段 3 |

R106（L742 的备注）：

```
手册：`dns-server` 的取值是 "comma-separated IP addresses or `system`"，`encrypted-dns-server` 是 "comma-separated URLs or `off`"；而一行的各个参数之间也用逗号分开（"Each line consists of a subnet expression followed by comma-separated `key=value` parameters."，例子 `SSID:MyHome dns-server=8.8.8.8,encrypted-dns-server=https://1.1.1.1/`）。列表的下一项与下一个参数靠什么分开，手册没有说——阶段 3 的设计要定（一种读法：逗号后面的一段若是 `<已知的键>=…` 就是下一个参数，否则是上一个列表的下一项）
```

R109（新的 9.10 节）：

```
### 9.10 `[IP Rewrite]` IP 层的包处理（iOS 5.23.0+ / Mac 6.10.0+）

手册 `features/ip-rewrite.html`。rurge 今天只认识这个节（解析、留着、`W0016`，见 1.2 节）；语义随阶段 3 的 TUN。

| 项 | Surge 行为 | rurge | 阶段 | 备注 |
| --- | --- | --- | --- | --- |
| 行的写法与求值 | 每行 `<IP 地址或 CIDR> = <动作>`；IPv4 与 IPv6 都行；自上而下，第一条命中的生效 | ✅ | 3 | 手册："The lines are evaluated from top to bottom, and the first matching line wins. IPv4 and IPv6 addresses are both supported."。写错的地址与动作是哪一个诊断码，阶段 3 的设计定（今天不校验这个节的内容） |
| `reflect` | 把包送回发送者，源地址与目的地址对调；网关模式下局域网设备发来的包送回那台设备 | ✅ | 3 | 手册："The packet is sent straight back to its sender with the source and destination addresses swapped." |
| `reject` | TCP 的连接请求回 RST，其余的包回 ICMP administratively prohibited，发送者立刻失败而不是等到超时 | ✅ | 3 | |
| `drop` | 悄悄丢弃 | ✅ | 3 | 手册："The packet is silently discarded." |
| 作用的范围与时机 | 只作用于进了 Surge VIF 的流量（增强模式与网关模式）；在 TCP / UDP 协议栈、规则与策略之前，命中的包不成为一条请求；发给代理监听端口的请求不受影响 | ✅ | 3 | 手册："It works at the IP layer: a matching packet is handled as soon as it arrives, before it reaches Surge's TCP/UDP stacks, rules or policies, and it never shows up as a request."、"Requests sent to Surge's proxy server are not affected." |
| 路由 | 包要先被路由进 VIF：地址落在 `tun-excluded-routes` 或本地网络的路由里时要加进 `tun-included-routes` | ✅ | 3 | 手册："The packets must be routed to Surge VIF in the first place. If an address is covered by `tun-excluded-routes` or a local network route, add it to `tun-included-routes`."。模块也可以带这个节（1.5 节） |
```

### 5.9 第 10 节 工具与可观测性（R110 ～ R122）

10.3 节（CLI）的表没有状态一栏，标记只在少数几行的"rurge 对应"一格里；新增的 `lsp` 一行带 ❓，按附录的规则计入第 10 节。

| 号 | 行 | 手册（页）与原话 | 改动 |
| --- | --- | --- | --- |
| R110 | L826 `script list/run` … `test-policy-bandwidth` | `policies/tailscale.html`："a peer-to-peer policy is covered by the RTT probe and reported as skipped in the UDP relay step, and `test-policy-udp` fails immediately with a not-applicable error instead of timing out." | 备注（原来是空的）写表后 R110 的那一段 |
| R111 | L829 `rurge status` | `tools/cli.html`（`status`）："Show the active profile, outbound mode, feature states, uptime, and version information." | 第一格"rurge 扩展"改成"`status`（rurge 今天已有的）"；备注开头的"Surge 无对应命令；"改成"Surge 的 CLI 有同名的 `status`，显示当前 Profile、出站模式、各功能的开关、运行时间与版本（上面 `status` `summary` `version` 一行，阶段 6 对齐）；这一行是 rurge 今天已有的那一个，显示的是第二格列的几项，没有当前 Profile、功能开关、运行时间与版本；" |
| R112 | 新行，接在 L830（`environment` `set` `set-log-level`）之后 | `tools/cli.html`（Editor Language Server，Mac 6.10.0+）："The offline language server requires Surge Mac 6.10.0 or later."；"The optional `--stdio` flag is accepted for clients that append it automatically."；"An editor's LSP client starts this process and communicates over standard input and output. It does not require Surge to be running, does not change the active configuration, and cannot be combined with remote Controller options." | 新增一行（❓），见表后 R112 |
| R113 | L839 10.4 节表前的一段 | `tools/http-api.html`（Basic Constraints）："Surge only uses GET and POST methods."；"- For the GET method, use URL queries to send parameters."；"- For the POST method, use a JSON body to send parameters."；"Surge always returns a JSON body as the response." | 段末加表后 R113 的那一句 |
| R114 | L853 `GET/POST /v1/policy_groups/select` | `tools/http-api.html`："Obtain the option of a select group."；"Change the option of a select group." | 备注开头加表后 R114 的那一句；"`subnet` 组仍 400"改成"`subnet` 组仍 400（手册把它也列为可以临时覆盖的自动组，见 5.1 节「临时覆盖」一行；随阶段 3）" |
| R115 | L854 `POST /v1/policy_groups/test` | `tools/http-api.html`："Test a group immediately." | 备注开头加表后 R115 的那一句 |
| R116 | L855 `requests/*` | `tools/http-api.html`："Kill an active request." | 备注"阶段 6 对齐"后面加表后 R116 的那一句 |
| R117 | L856 `GET /v1/profiles/current` | `tools/http-api.html`："Obtain the text content of the current profile. If `sensitive` is false, all password fields are masked."；"Surge always returns a JSON body as the response." | 标记 ✅ 改成 🟡，两个理由：应答的形状与手册的基本约束不同（rurge 回的是 `text/plain`）；脱敏的范围与手册的那句话不同（这一行早就写着，标记没有跟上）。备注"`sensitive=1` 给出原文。"后面加表后 R117 的那一段 |
| R118 | L860 `GET /v1/external_resources` | — | 备注里去掉"这个端点属于 Surge Mac 6.10 / iOS 5.23，晚于本清单的手册基线（Mac 6.9 / iOS 5.22），只取了它，基线的整体更新仍是阶段 3 开工前单独的一件事。"一句（基线已是这一版） |
| R119 | L861 `POST /v1/external_resources/update` | `tools/http-api.html`："Download remote external resources immediately. Use `"all"` as the key to update all remote resources. Local resources are skipped." | 备注两处："；同上一行，属于晚于本清单的手册基线（Mac 6.9 / iOS 5.22）的端点。"改成"。"；"（手册没有说这种情形）"改成表后 R119 的那一句 |
| R120 | L863 `GET /v1/geoip` | — | 备注末尾去掉"。这个端点属于 Surge Mac 6.10 / iOS 5.23，晚于本清单的手册基线（Mac 6.9 / iOS 5.22），只取了它，基线的整体更新仍是阶段 3 开工前单独的一件事"（连同前面的句号） |
| R121 | L865 `scripting/*` | `tools/http-api.html`："`script_text` is required; `mock_type` is a script type string such as `http-request` or `cron` (default: cron). The `$trigger` global is set to `http-api` during evaluation."；"Evaluate a configured cron script immediately by name. Only cron-type scripts are accepted." | 备注（原来是空的）写表后 R121 的那一段 |
| R122 | L871 `POST /v1/log/level` | `tools/http-api.html`："Change the log level for the current session." | 备注末尾加"；只改内存里的级别，不写任何文件，重启后回到 `--log-level` / `loglevel`（同手册：只对当前这一次运行）" |

R110（L826 的备注）：

```
`test-policy-udp` 手册只在 `policies/tailscale.html` 提了一次名字（"`test-policy-udp` fails immediately with a not-applicable error instead of timing out"），CLI 页的命令表里没有它，没有说它是哪一种入口；rurge 的 UDP 测试今天是 `POST /v1/policies/test` 结果里的 `udp` 键（`test-udp` / `proxy-test-udp`），CLI 的入口随阶段 6
```

R112（新行）：

```
| `lsp [--stdio]`（Mac 6.10.0+） | 给编辑器用的离线语言服务器：经标准输入输出说 LSP，不要求 Surge 在运行，不改动生效的配置 | ❓ | — | 手册（`tools/cli.html`，Editor Language Server）："An editor's LSP client starts this process and communicates over standard input and output. It does not require Surge to be running, does not change the active configuration, and cannot be combined with remote Controller options."。要不要给 rurge 做语言服务器是产品范围的问题，留给项目所有者，没有排进任何阶段；今天 `rurge check --json` 给出同一类诊断（逐条带位置、诊断码与建议），编辑器的插件可以从它出发 |
```

R113（加在 L839 一段的末尾）：

```
手册的基本约束（`tools/http-api.html`）：只用 GET 与 POST，GET 的参数走查询串、POST 的参数走 JSON 正文，应答总是 JSON（"Surge always returns a JSON body as the response."）。rurge 的端点同样只用 GET 与 POST、参数的走法相同；应答是 JSON，错误的应答也是（`{"error": "<文本>"}`，404 与 405 也是）——只有 `GET /v1/profiles/current` 例外，它回 `text/plain`（见该行）。
```

R114（加在 L853 备注的开头）：

```
`GET` 用查询参数 `group_name`、应答 `{"policy": "ProxyA"}`，`POST` 的请求体是 `{"group_name": "GroupA", "policy": "ProxyA"}`（同手册的例子）；
```

R115（加在 L854 备注的开头）：

```
请求体 `{"group_name": "GroupA"}`，应答 `{"available": [...]}`（同手册的例子）；
```

R116（接在 L855 备注的"阶段 6 对齐"之后）：

```
；`POST /v1/requests/kill` 的请求体是 `{"id": 100}` 这样的对象（同手册的例子）
```

R117（接在 L856 备注的"`sensitive=1` 给出原文。"之后）：

```
**应答的形状与手册不同**：rurge 的应答是 `text/plain` 的配置文本本身；手册的基本约束是应答总是 JSON（"Surge always returns a JSON body as the response."），这个端点的 JSON 是什么样它没有写。一个开源的 Surge 面板（geekdada/yasd，2026-10 的版本）把这个端点的应答读成带 `name`、`profile`、`originalProfile` 三个键的对象（处理过的文本与原文各一份）——那是第三方客户端的类型定义，不是手册；rurge 的形状随阶段 6 的 Dashboard 一起对齐（同上一行的请求列表）。**脱敏的范围与手册不同**：手册说的是 "If `sensitive` is false, all password fields are masked."；rurge 遮的不只是口令字段，也不保证遮全（下面"有意多藏的"与"已知没有覆盖的"）。
```

R119（L861 里"（手册没有说这种情形）"换成）：

```
（手册的 "Local resources are skipped." 接在说 `all` 的那一句后面，可以读成只说 `all`，也可以读成按键点名时同样跳过；rurge 取前一种读法）
```

R121（L865 的备注）：

```
手册：`evaluate` 的 `script_text` 必填，`mock_type` 缺省 `cron`，求值期间 `$trigger` 是 `http-api`；`cron/evaluate` 的请求体是 `{"script_name": "<名字>"}`，只收 cron 类型的脚本
```

### 5.10 附录：统计（R123）

| 号 | 行 | 手册（页）与原话 | 改动 |
| --- | --- | --- | --- |
| R123 | L905 附录的说明一段，L909 ～ L919 的表 | — | 段末加一句：文档提交那天的日期（手册基线重对）用同一条规则又整张重数了一次，555 行变成几行、各个标记各变成多少，标记变了的行与新增的行各是哪些——都照重数的结果写。表里的数字换成重数的结果 |

重数的预期（以脚本数出来的为准）：555 行变成 565 行，✅ 395 → 398，🟡 112 → 118，🔁 30、⛔ 14 不变，❓ 4 → 5。标记变了的五行都从 ✅ 变成 🟡：通配命名节（R12）、`[Keystore]` 条目（R19）、`GEOIP`（R35）、`wireguard`（R55）、`GET /v1/profiles/current`（R117）。新增十行：1.1 节一行 🟡（R5）、1.2 节的 `[IP Rewrite]` 一行 ✅（R7）、4.2 节的 `custom` 一行 ✅（R54）、9.10 节六行 ✅（R109）、10.3 节的 `lsp` 一行 ❓（R112）。各节（条目 / ✅ / 🟡 / 🔁 / ⛔ / ❓）：第 1 节 75 / 52 / 19 / 1 / 3 / 0；第 3 节 63 / 45 / 14 / 3 / 1 / 0；第 4 节 89 / 67 / 18 / 1 / 3 / 0；第 9 节 49 / 34 / 7 / 3 / 2 / 3；第 10 节 57 / 25 / 23 / 4 / 3 / 2；第 2、5、6、7、8 节不变。

## 6. 留给后面阶段的话

第 5 节里有 31 项带着留给后面的话：手册今天的句子写到了后面阶段的行上，那个阶段的设计从这些句子出发。其中 R7、R20、R41、R78、R92 同时带着这一轮的改动（解析器认识 `[IP Rewrite]` 这个节，DNS 会话的标签按方案，`W0013` 看进子规则，`subnet` 组的告警文本），其余 26 项今天不改任何行为。

| 阶段 | 项 | 那个阶段的设计要定的 |
| --- | --- | --- |
| 3（增强模式） | R7、R109 | `[IP Rewrite]`：新的 9.10 节六行就是它的语义；写错的地址与动作给哪一个诊断（今天不校验这个节的内容）；模块也可以带这个节 |
| 3 | R78、R80、R114 | `subnet` 组：手册说它在自己的条件之外只有三个参数；不认识的 `key=value` 与不带前缀的旧写法分不开；`default` 手册标着必填，没写时是不是加载错误；它也能临时覆盖（今天 `POST /v1/policy_groups/select` 对它答 400） |
| 3 | R20、R92 | 手册说可以给 DNS 服务器的主机名写规则：内部会话要带着主机名，又不再解析一次 |
| 3 | R105 | ICMP：转发出去、把应答交回，不是在本机伪造应答 |
| 3 | R106 | `[SSID Setting]` 的 `dns-server` / `encrypted-dns-server` 取列表值；列表的下一项与下一个参数靠什么分开，手册没有说 |
| 4（HTTP 引擎） | R98 | 协议嗅探：手册两处说法（发往 80 与 443 端口的；看开头的字节），两句都要对 |
| 4 | R99 | Body Rewrite：改写前按需要先解压 |
| 5（脚本与模块） | R16、R17、R18、R108 | 模块：`[MTProto]` 与 `[Snell Server]` 整节替换；同名的内联规则集合并；`[IP Rewrite]` 可以加行；`#!system=` 与 `#!arguments=` 在手册今天的写法 |
| 5 | R41、R42、R96 | `SCRIPT` 作子规则时真的去问脚本；引用了不存在的脚本名的 `SCRIPT` 规则与 `[Host]` 的 `script:` 条目让 Profile 校验失败 |
| 5 | R100 ～ R104、R121 | 脚本：超时能不能中断同步代码；`$surge` 三个函数的形状；`clearTimeout`；`http-request` 在规则匹配之前；`generic` 的 `$intent`；两个脚本端点的请求体 |
| 6（API、Dashboard 与工具） | R110、R111、R116、R117 | CLI 的 `test-policy-udp` 入口与 `status`；`POST /v1/requests/kill` 的请求体；`GET /v1/profiles/current` 的 JSON 形状 |
| 7（网关与服务端能力） | R107 | Snell 服务器在 iOS 与 tvOS 上也有 |
| 不设阶段，归项目所有者 | R112 | `surge-cli lsp`：要不要给 rurge 做语言服务器 |
| 不设阶段，归项目所有者 | R13 | `CORE_VERSION` 的新编码：Surge 自 Mac 6.8.0 / iOS 5.21.0 起报 `major * 1000000 + minor * 1000 + patch`，rurge 报 22。需求 FR-CFG-08 说这个值"随实现进度递增"，而新编码下 22 与 `6008000` 之间没有值；rurge 什么时候改报新编码的值、报哪一版的，要定 |

**这张表不是后面阶段要看的手册句子的全部。** 照给助手的规矩，对阶段 3 及以后的行，助手只报"清单没有这一项"与"说法不同"；手册比那些行多说的细节（缺省、取值、版本、平台）没有收集（第 9 节）。每个阶段的设计开始时，要把它那几节对应的手册页对着清单重读一遍。

**阶段 6 的一条线索。** 手册对多数端点的应答只给一个例子，或者不给（清单 10.4 节多处写着"响应结构手册未定义"）。核实 R117 时读了一个开源的 Surge 面板（geekdada/yasd，MIT 许可；只取"它把应答读成什么形状"这一事实）的类型定义：`GET /v1/profiles/current` 的应答被它读成带 `name`、`profile`、`originalProfile` 三个键的对象；策略与策略组的列表、最近的请求、流量、模块、脚本、事件、DNS 缓存、设备与测速结果也各有类型。那是第三方客户端对 Surge 应答的读法，不是手册，也不是对 Surge 的观察。阶段 6 对齐应答形状时（需求文档的开放问题 Q4：复用开源的 Surge 兼容面板还是自研）可以从它出发，逐个端点再核对。它对 `GET /v1/external_resources` 与 `GET /v1/geoip` 的读法与 rurge 今天的形状相同，但这两个端点是这一版才有的，面板里的注释照着手册的话写，不能当作手册之外的证据。

## 7. 不改的

### 7.1 318 条里不引出改动的 128 条

裁定按助手给每一条的编号逐条记下；下面的条数是从裁定表里数出来的。

| 类别 | 条数 | 是什么 | 为什么不改 |
| --- | ---: | --- | --- |
| 只是版本标记 | 41 | 手册在标题或句子后面标的"iOS 5.x+ Mac 6.x+"，清单的那一行没有抄 | 设计 MB-D2：清单只在版本影响 rurge 怎么办时才写版本。比旧基线新的十四处另列在 3.2 节 |
| Surge 自己的应用与界面 | 8 | 应用内的语法帮助、规则编辑器、界面上显示哪个 DNS 服务器先答、WireGuard 与 Tailscale 策略的运行时视图、tvOS 与 iOS 的平台说明 | rurge 是没有界面的核心 |
| iOS 专属项的细节 | 44 | iOS 专属的 `[General]` 键的缺省与系统要求（17 条）、`external-controller-access` 的写法（4 条）、Always On（1 条）、四个 iOS 专属的内置策略（4 条）、`CELLULAR-RADIO` / `CELLULAR-CARRIER` 的取值与说明（17 条）、`include-all-networks` 与 `include-apns` 的组合（1 条） | 这些项 rurge 解析并忽略（内置策略按 `DIRECT`，两种规则永不匹配），细节没有落脚的地方 |
| 误报 | 4 | 见 7.3 | 手册的另一页支持清单的那一行 |
| 清单的行已经隐含的，或页面上的建议与链接 | 31 | 行里已经看得出的缺省（布尔参数缺省 false、端口 53、`?` 匹配一个字符）、"See Also"的链接文字、给使用者的建议、独立的 Snell 服务端自己的配置项、Surge 已经去掉的 `[Replica]` 节、`[Ponte]`（解析并忽略）的细节、rurge 不提供的 `cellular-data` 的两个字段 | 写进去不改变任何一行说的事 |

其余 190 条引出了改动，或者要先读代码才能裁定（100 条进了第二遍的 47 处——一处常常对着几条）；它们合起来是第 4 节的十一处与第 5 节的 123 项。

### 7.2 助手另外列出的 46 行

每个助手在自己那几节里另列了"在我读的页上找不到依据的清单行"，共 46 行：

| 去向 | 行数 | 说明 |
| --- | ---: | --- |
| rurge 自己的行，或自己写明手册没有说的行 | 31 | 命令行选项、请求记录、流量统计、旧键名的别名、rurge 专有的诊断与限制——本来就不在手册里。其中六行因为别的发现改了（L78、L79、L219、L370、L392、L429，见第 5 节） |
| 误报 | 11 | 依据在别的助手读的页上，见 7.3 |
| 改了 | 2 | L117（R17）、L558（R98） |
| 归到"手册没有写"的复核 | 2 | L503、L504：仍然没有写（第 8 节） |

### 7.3 误报

十五条：助手说手册与清单不一样，或说找不到依据，而手册的另一页（别的助手读的）支持清单的那一行。

| 助手报的 | 清单 | 依据在 |
| --- | --- | --- |
| fake-IP 应答器的地址：`[General]` 页只写了一个 | 6.4 节应答器监听地址一行（L539） | `dns/advanced.html` 分别给了 macOS 与 iOS 的地址，同清单 |
| TLS 参数页列的协议里没有 MASQUE；它的缺省 ALPN（两条） | 4.4 节 | `policies/masque.html` |
| WireGuard 页的"detached profile section"没有对应的行 | 1.3 节 `#!include` 的几行 | 它就是 `#!include`（`profile/format.html` 的 Detached Profile Section 一节） |
| 值含逗号时加引号 | L38 | `rules/overview.html` |
| 子网表达式的五种写法（五行） | 3.4 节 | `rules/protocol-and-network.html` 的 SUBNET 一节；`policy-groups/subnet.html` 自己说以它为准 |
| HTTPS 的测试 URL | L480 | `profile/general.html`（`internet-test-url`）与 `tools/testing.html` |
| `vmnet` | L722 | `tools/cli.html` |
| `[Testing]` 节（两行） | L60、L785 | `tools/testing.html`、`profile/format.html` |
| `geoip-maxmind-url` 的缺省地址 | L151 | `rules/ip.html` |

另有两条一半是误报。L558 的"80 / 443 端口"：HTTP 概览页没有这个说法，`[General]` 页的 `always-raw-tcp-hosts` 下有；清单的那一行把两页的话都写上（R98）。脚本那几页的助手顺带说 HTTP API 页没有列出出站模式的取值：页上有（"Possible modes: direct, proxy, rule"），L846 不动。

### 7.4 登记而不改代码的差异（索引）

第二遍对着代码核对的 47 处里，27 处照手册（清单的行补上手册的那一句与断言它的用例名），13 处不同，6 处没有，1 处从仓库里判断不了。不同与没有的里面，这一轮改代码的是第 4 节的第 1 ～ 8 项（第 10、11 两项不在这 47 处之内，是裁定第一遍的条目时另外读代码看出来的）；其余的登记在清单的行上，逐行见第 5 节。按性质：

- **手册前后不一，rurge 取一句，清单把两句都写出来**（六处）：`SYSTEM_VERSION` 的例子（R14）；`test-timeout` 的缺省（R28）；`block-quic = always-allow` 那一句的前半与后半（R29）；`CELLULAR-RADIO` / `CELLULAR-CARRIER` 的平台，平台差异页与规则页的标题不一（R40）；Shadow TLS 与其余 QUIC 类协议，TLS 参数页与 MASQUE 页不一（R62）；代理服务器的主机名进不进 `[Host]`（R92）。
- **Surge 有、rurge 没有**：随包的两个 GeoIP 库（R22、R23、R35、R36）；"互联网连通性检查"（R26）；点对点策略这个概念（R60）；WireGuard 上 ECN 标记的合并（R59）；原生测速报告作答的 endpoint（R74）；日志里的 `FAILED` 与 `SUBSTITUTE` 两个标记（R79）。
- **rurge 比手册严**：`[Keystore]` 的 DSA 私钥（R19、R69）。
- **rurge 比手册宽**：通配命名节重名时不让加载失败（R12）；策略行列表里以 `;` 开头的行也算注释（R84）；`trust-tunnel` 行上的 `ws` 当未知参数（R68）。
- **rurge 多说一句**：未知的规则参数，手册说静默忽略，rurge 忽略并给一条 `W0003`（R45）。
- **做法不同**：指向同一个节的 `wireguard` 策略共用一条隧道，手册说每个策略各有协议栈（R55）；`h2-connect` 的服务端支不支持 extended CONNECT，Surge 在连接建立时核对，rurge 到第一个包才知道（R53、R64）；`smart` 组，手册给了轮廓与几个数，其余是 rurge 自己的（R77）；`GET /v1/profiles/current` 回纯文本（R117）。
- **手册的一句可以读成两样**：`POST /v1/external_resources/update` 的"Local resources are skipped."（R119）。
- **手册没有说、rurge 自定的**，这一轮新写进清单的：一个值中间的引号与括号（R5）；位置与命名两种写法的凭据都写时谁优先（R53、R63）；`use-local-host-item-for-proxy` 看不看策略的 `ip-version`（R21、R97）；`url-test` 一个成员都没有通过时（R76）；定义变了的组的临时覆盖（R80）；组的中继自己也在成员里时（R83）；列表值与下一个参数怎样分开（R106）；`test-policy-udp` 是哪一种入口（R110）。这几句各自对着有关的页核对过，没有对着全部 87 页搜；下一次重对时与第 8 节的那 78 处一起查。

## 8. "手册没有写"的复核

清单里有 69 行、共 85 处说"手册没有写 / 未说明 / 未定义 / 只说……"。一个只读的助手拿今天的 87 页逐处再查（先通读有关的页，再对每一处用几组词在全部 87 页里搜，搜过的词记在它交回的报告里），每一处的结论由主持这一轮的会话对着页面复核：

| 结论 | 处数 | 行 |
| --- | ---: | --- |
| 手册现在写了 | 2 | L364 `dns-follow-interface`（R56）；L494 同时在途的相同查找（R89）——只写了一半：合并成一次查询写了，那"连续 5 次"按什么计数仍没有写 |
| 拿不准 | 1 | L861：手册的那一句可以读成两样（R119） |
| 其实不是在说手册 | 4 | L7、L84、L375、L858 各一处：登记的差异，或 rurge 自己的文本。不动 |
| 仍然没有写 | 78 | 其中七行的说法收窄了，其余不动 |

收窄的七行：L78（R10，关于启动手册只有一句）、L79（R11，手册分开说托管 Profile 的只有"不能在本地编辑"）、L201（R31，手册现在写了地址的规矩，没写的只剩口令怎样比较）、L304（R48，补上 `[Rule]` 行的行尾注释那一句）、L446（R76，没说的只是 `url-test`）、L449（R77，手册现在给了算法的轮廓）、L473（R85，换成今天的两句）。

清单里用引号引着的手册原句，今天的手册里一句也没有少。

仍然没有写的 78 处，按题目归一下（下一次重对从这张单子查起）：

- **托管 Profile 与 include**：经不经策略下载，`interval` 的下限（L73）；强制更新只有 CLI 页的一句，自动更新怎样检查（L76）；`strict` 的更新失败时 Surge 拒绝什么（L77）；启动时等不等来源（L78）；托管 Profile 在本机能做的事（L79）；一个文件里两段同名的命名节（L84）。
- **`[General]` 与监听**：GeoIP 库自动更新的周期（L152）；连接空闲超时的缺省（L178）；HTTP 监听的口令怎样比较（L201）；SOCKS5 对 REJECT 回哪个应答码（L202）。
- **规则与集合**：DOMAIN-SET 超过一百万条时（L233）；资源 URL 里的用户名口令发不发（L254）；`update-interval=0`（L267）；Unicode 的域名（L285、L356）；规则集文件超过一百万行时，文件里的行尾注释（L304）；下载用不用条件请求（L307）；嵌套只对内联集说了，相对路径以什么为基准（L308）。
- **策略**：`ss` 的 `obfs-host` / `obfs-uri` 的缺省（L342、L418）；Snell 的 `udp-port` 上发什么（L343、L399）；VMess 的流选项（L344）；TUIC 的 UDP 模式、心跳、0-RTT 与拥塞控制（L346、L422）；MASQUE 的凭据用哪个头、URI 模板（L348）；`trust-tunnel` 的 HTTP/3 回不回落（L350），它上面的 `port-hopping`（L427）；没有 `dns-server` 的 `wireguard` 策略上的目标名（L352）；手册不再列出的 `ss` 方法名（L355、L417）；`direct` 别名上的 `ip-version`（L366）；`trust-tunnel` 算不算 QUIC 类（L370）；空组作 `underlying-proxy`（L375）；`port-hopping-interval` 的下限（L423）；Gecko 的包长（L424）；`masque` 上的 `headers` / `max-streams`（L425）；SSH 的空闲与保活（L428）。
- **策略组**：`url-test` 没有成员通过时（L446）；`smart` 的常数（L449）；自动组上的 select 端点（L452）；对订阅行的限制，中继建不出来的导入策略（L472）；组上 `update-interval` 的 0 与负数（L473）。
- **DNS**：`localhost`（L503）；空应答与错误的否定缓存（L504）；DoH URL 里的用户名口令、没写路径时的路径、`Age` 头（L510）；`h3://` 没写路径时（L511）；`off` 与别的取值混写（L514）；DoQ / DoH3 的载体匹配到不载 UDP 的代理（L516）；`[Host]` 键末尾的点（L525），含 `://` 的键（L526）。
- **HTTP**：`[MITM]` 里手册没有定义的键（L583）。
- **API**：策略、策略组、测试结果、请求、DNS、规则、流量各端点的应答形状（L848 ～ L852、L855、L862、L869、L870）；`profiles/check` 的错误文本（L858）；外部资源的 `key` 怎样构成，列表的顺序与范围（L860）；GeoIP 库的两个日期指什么（L863）；第一条 `FINAL` 之后的规则列不列（L869）；服务端怎样关连接（L876）。

## 9. 这份报告没有覆盖的

- **Surge 的实际行为。** 一处也没有观察。报告与清单里说"同手册"，说的是与手册的文字一致。
- **被读成一致的 2,439 条。** 没有人逐条复核；它只表示读的人没有看出不同。
- **后面阶段的行上手册多说的细节。** 对阶段 3 及以后的行，助手照规矩只报"清单没有这一项"与"说法不同"，手册比那一行多说的缺省、取值、版本与平台没有收集。第 6 节的表因此不全。
- **代码。** 第二遍只对着代码核对了 47 处。阶段 1、2 其余各行，这一轮比的是清单的文字与手册的文字，不是代码与手册：阶段 1 的需求逐句对过代码（`docs/acceptance/phase1-acceptance.md`），阶段 2 的那一遍（2026-10-10）判为"做了"的没有人复核，这一轮没有改变这一点。
- **取文本时丢掉的。** 附录 B 的脚本只留正文的文字：图片（连同截图里的字）、链接指向哪里、表格的行列关系（每个单元格成了一行）、粗体与斜体都没有了。页面上靠这些表达的意思，这一轮读不到。
- **手册之外。** Surge 的知识库、博客与社区文档没有读。发行说明只读了官方频道最近的一页（6.10.0 的几份测试版、6.10.0 / 5.23.0 的正式版与它之后的一份测试版），用来确认手册对应的版本、点出手册还没有写到的东西（3.3 节）；没有拿发行说明逐条去对清单，更早版本的发行说明没有读。
- **两处靠第三方的事实。** 旧的 `custom` 行里字段的位置取自一个开源订阅转换器的读法（R54），`GET /v1/profiles/current` 的三个键取自一个开源面板的类型定义（R117）。都不是手册，也不是对 Surge 的观察。
- **这份报告自己。** 用引号引的手册原句逐句用脚本对着那一页的文本核对过（只忽略空白的差别）；点名的用例用 `git grep` 核对过存在；第 5 节里"改成""后面加"所指的清单原文用脚本核对过在那一行里。这些核对管的是引文与指向，不管新写的话有没有说错——照第 5 节改清单的时候，每一处还要对着那一行再读一遍。
- **CI 与手工验收。** 照旧：CI 没有绿过，所有用例只在一台 Windows 11 x86-64 的开发机上跑过，两份手工清单一行也没有做过。这一轮的十一处改动同样只在那台机器的回环上验证。

## 附录 A：87 页文本的摘要

页的顺序是手册目录的顺序，路径相对 <https://manual.nssurge.com/>。摘要是附录 B 的脚本写出的文本文件（UTF-8，行尾 LF）的 SHA-256。2026-10-13 与 2026-10-14 各取一次，87 页的摘要逐页相同。

下一次重对：照附录 B 取文本、算摘要，与下表不同的页就是改过的页（页面的导航改了也会让摘要变，所以摘要不同只说明要看，摘要相同才说明没有改）。

| 页 | 文本的 SHA-256 |
| --- | --- |
| `getting-started/how-surge-works.html` | `4b4cedc21625525945b9be6079b2299b0d91672c6d62768c14988b2059342a9c` |
| `getting-started/quick-start.html` | `c6c4827eb23fe3e14dc52ce7dabdbf6befdcf3bd8410710cddfce974feae1b4a` |
| `getting-started/platform-differences.html` | `0bc9c5bf0eec3883745e0ccbd903509337b98782ac1be0617ee73515a0ed1b1b` |
| `profile/format.html` | `f32154bad6145ff88edebebd50f0e9a9a846dd0a0a9281e0742ecdde3ada9527` |
| `profile/managed-profile.html` | `34b1c721d6bfb8bd810a48a4b71fd05895532c29fb620d3570aba8eca6dd7fc6` |
| `profile/module.html` | `fdc8be9856886d91640b464528776e07afbb211cceb927e510f1a93433186cb7` |
| `profile/requirement.html` | `31ca8118978d02a33733437f47392c6c07f30eee783ba1321e21cc373d255900` |
| `profile/keystore.html` | `42452bf30c9ea49bed23e6392099c257c6fc2b5d38148b64326e7c671c744f94` |
| `profile/host-list.html` | `9f9a628499270e0932049315e7bb2ef5052388af590662f95edd40e619357f1e` |
| `profile/general.html` | `365c6303284d171914d2b5e92e7bbc0a65b1c5a80e9c3ae9b5bd92e3868375e4` |
| `rules/overview.html` | `3e313d91bf46a2c58cf2195028fd260a98ab85892d2fcc1423b97fa2aae173b4` |
| `rules/domain.html` | `5b5829808aa98aba3d6ac3fedc05f425ec719576dd0c24d24b0c419c46bd3467` |
| `rules/ip.html` | `46aac05046c0f23d6eff5ead5e1545b196255a56444aba98cee102872e0bf32d` |
| `rules/http.html` | `d8c7bacca43d1318ee34ab791fd21ff470cf7321b703302de4a991e8f4b81b96` |
| `rules/process.html` | `9df5d94cc3b44f8783a79e4f322f371205780a838dcc1f8fed55c68d2f2ee46c` |
| `rules/source-and-port.html` | `1ae70e6dbc86f817242e18f934bd9e83e3907a7e2b405371e3fea88c172909c9` |
| `rules/protocol-and-network.html` | `f622aded011097e7191252046dd48812183e9345bfd78ca2683b72c6158701bf` |
| `rules/logical.html` | `9ab3e455a3adf21ce5c8ecb8956f7dac6802b2945b3806314e9f2fc67a524a2e` |
| `rules/script.html` | `038aed032aa59a755a05e5af6c7c126fb052adbff2c6cf4320aa6ff8db83d08c` |
| `rules/ruleset.html` | `934195afb2ce2c5d5bc3612e5b11b00768ba4b54af2753be751a8326fb827348` |
| `rules/final.html` | `5ba6d4599a037f2e492eeaedac8ad44ba57bb5183f4d0f7936617a4552f117d1` |
| `policies/overview.html` | `c8451de6a0e907c0132b091f3fbe5dd51ca4417044717d33648ad9361104c691` |
| `policies/built-in.html` | `9bd3cea3d8a9ed6a582538d642300a811a54ce539512b74d1c83b116efd4d63f` |
| `policies/reject.html` | `11e51b033f8bb6b4e769b92cd9770a891545ace08044e58a48dcfc8673aa22cf` |
| `policies/parameters.html` | `fc6c8dc7a891f6b95fe5190826249be69ab6fe44102e3d639b579119ea2beb4c` |
| `policies/tls.html` | `a12a60824e0f4efd2a6612048d271cd45450650198b4d967ce80d41d855195bc` |
| `policies/udp.html` | `2bc67edf714a7bdebded6ab3ff2e9e058b5ec4ba24830d2ceede1e2f537c74f1` |
| `policies/http.html` | `e947f73a5a8f18ce641a337da53b322dd6937b24fa9f46a407e007a00536f0af` |
| `policies/socks5.html` | `90454273386279bb10905ee142f300e3a9ad95f2e02bcd396c576b3d0597df91` |
| `policies/shadowsocks.html` | `acfd6afe9a38cbde02539f55f1f6453310ed569227a3db3865c82809002fcd8a` |
| `policies/snell.html` | `84ab94420bd9c747d68bcdf56421784a988c120807910e0d7608a52793005454` |
| `policies/vmess.html` | `0787e1e64d0a58b50be4cf99f2d3a6dea1709088d22d094a19a1927e2f10df6c` |
| `policies/trojan.html` | `13a819055c417f6e24e4f1097469e182a8b98b944f28ba2b0aae748a09d2bfb6` |
| `policies/tuic.html` | `2c899426acdc613adfbfc5b7de6b2ea14b8bc158840abd4b7d95c7e04dff0ffe` |
| `policies/hysteria2.html` | `dfce13fa58f97f927f06cde65d00c95cd5092eaf77c0805e03f7a6ebbc765b31` |
| `policies/masque.html` | `239f08d69d5077b10e03c3d4f5cc300c83b8082819bdbf709b94af0cf672531f` |
| `policies/anytls.html` | `c4dbbed96ff4b49817d8b4f55afd5664c47c4629b09351f18b67e24bd72af5de` |
| `policies/trust-tunnel.html` | `065c9085bf9854eac11e44c2ed11d0bdaf1ac5058971f6c7e3ce871eed77bd5a` |
| `policies/ssh.html` | `f67327d9c0dbdf7f149558bb31880c674b1328de82b5a4e25fab264918065715` |
| `policies/wireguard.html` | `c32a901ff1a85546eaa31ea5d2d382bd07f109af9db7b8bbc89a080cae2cd34a` |
| `policies/tailscale.html` | `92da12cf53441325562c0adff0aa7cf026b9ce7943dbf7ff78b39bfe2ae12a1c` |
| `policies/external.html` | `b2897343d79596bd001729a643f84d39e92bef20c678c03193a243b91eaaf0bc` |
| `policy-groups/overview.html` | `9422258ad8495bc0cfa4084498ba0c3f48a816d3e9770898d17b68d46738810b` |
| `policy-groups/select.html` | `92cc50de7f79675e2f0d43442a5a523dee763954e6730fa6990454e2d2670c35` |
| `policy-groups/url-test.html` | `8cf3a350a461c1954c524c8492242d33b7e919b70c4c20f57f7dbb25a190d207` |
| `policy-groups/fallback.html` | `6a8e52f049a262ad20a2701bf5ff007ab2ea44d4b96cd1efe0d634ba6dfae8b6` |
| `policy-groups/load-balance.html` | `e9c69f380c647469e3483b5bc0171a354e21f286e8b1744208df6e228f1b91c7` |
| `policy-groups/smart.html` | `7a980899b9d528ce2e3ec7d10c7f41ab69ee1833e3d0f55eea7c284fc1708ae3` |
| `policy-groups/subnet.html` | `5e687975962edd691b156d3fa5ea49acefb563a316861137cf00bdb0da8f3668` |
| `policy-groups/policy-including.html` | `1927483c4d9b908106a1cab31ddd0a94e2ed608f00fb3ad1214c3e3302d643a4` |
| `policy-groups/parameters.html` | `a4a2d85a529c06ff772991d073893d9776d81c9d441ca6eedda69f38e33a805c` |
| `dns/overview.html` | `7a306f001bb84aa3709da3beda6b6932a5200fc2ebba38b54a994ab72d2dbbe3` |
| `dns/dns-server.html` | `4bf05d014faa00b41f37a657fa8b409f44cc640f3f7f71ce3ec0b798ae6090b1` |
| `dns/encrypted-dns.html` | `a94e98e7051931b0cdafe4488ecb581df1563c15e271fd79b53911eca4f876e2` |
| `dns/local-dns-mapping.html` | `d14dcfa6c918ccd200a2a6ecd6e6018b9d86d35448246a7521bc422280670f1d` |
| `dns/advanced.html` | `d69c26717b3d54c8192c96340ad1d54604fe40ac9b955c8b57870f5f1f63e9e5` |
| `http/overview.html` | `db72d85f832fdcec47f22f5a43cb953c016d6e7cdc70f86069ff22f9a9fe3e83` |
| `http/mitm.html` | `14e658b0a38bc57c3120f296965d2f009c0f3f7c5107bf160ee076476b465c10` |
| `http/url-rewrite.html` | `37df39477e015bc457a5e40a5f30d994bef5196fe52c19f313320179d9538817` |
| `http/header-rewrite.html` | `94dabd7a0dd3fbabf7d3722f9392333e51b5251d15ac96411ce068036b6b34e6` |
| `http/body-rewrite.html` | `09c870e40039684dbf442f88d8da59d4abe714b22be24f3aae220b4eb47b2335` |
| `http/map-local.html` | `8bc3b21126bd694fa4f735b14b7d1b4ae0070a9ef9566a338dba241ae68d6a2d` |
| `scripting/overview.html` | `f716078f1596d9de90d033de9830100fa663818828837ba16fdc24a6813cb999` |
| `scripting/api.html` | `58df099b15cc056f557adf7405d73244ca6d43623ef2d3e03704c4b3a0fba252` |
| `scripting/http-request.html` | `7c7171a9f2da107e0819199a81265b01049abdc6e14d3f47717c5478ea9128f8` |
| `scripting/http-response.html` | `2e4964f8aef97f7723be6c8d5867ab2078612f1f52f42ae70b12b665c4ec2a64` |
| `scripting/rule.html` | `c508a0638686db2e491dbc4a3dee3c6187805c469fe7ef20b5fd8237f4c43179` |
| `scripting/dns.html` | `3e4d422fc5fcf5c898d90869dfeb3d1850b141fd9541f120c831d8cbacd71586` |
| `scripting/event.html` | `b4ed19b17ae90d7f00502811eae8921f83c2baff8259355c8edc8601f00dbcf4` |
| `scripting/cron.html` | `ed425dbde26cd38593643d2f67c24ef3ecc2b0d4b4ff8ae3bdf13e412f62102c` |
| `scripting/generic.html` | `237e9709c734ffe5e7bdb3a1bddfc5b4c7c6b5c3d718927acd3e3348c4352997` |
| `features/enhanced-mode.html` | `9d000a0df986e456a8696aac5fd64f73ea32963562e61c6e976c754a45af0c4a` |
| `features/ip-rewrite.html` | `8853a70951e611e36579a57b91a026c075ad7e97c0fefba0a1cf196c23c92803` |
| `features/gateway.html` | `1778c2446dd41780b979617f926f407dfeaf0befecddbae776ef19746bf6f5e2` |
| `features/dhcp.html` | `227b59d136a6bb6c311aec8fd541e99b23bb38b717d0919415698c1705b724a6` |
| `features/subnet-settings.html` | `cdba09e9423ffe0f3d31e3e2046348103d4cb925b177033ad8fcdb106a3927ae` |
| `features/port-forwarding.html` | `3c013ed9d5882e73f02b819064ca9c7f7f2b8900d33b0455fc538451c9453091` |
| `features/ponte.html` | `4a2d9c2edf49b9093c4b816b18d0f8e531365bde2d5563f2927b693c74772b8f` |
| `features/snell-server.html` | `31fca4f26614b83d6a97a1b693cf67f1d0ea55d83400da63c42bbca9f1c3964e` |
| `features/mtproto.html` | `0d042bb5fe8d3586ab10b132a537b7a3809044fa9eab3d2a6aeaf95d3c3fed0b` |
| `tools/dashboard.html` | `df8f143e8b8f611cd8a201d0699fd06ce7e5a4e3763c579f828b1e9c1f515d2a` |
| `tools/logbook.html` | `c05680306c381310d3e18b15f5b7e250ac9ad3b4bbab835e4cfb00128b2eac50` |
| `tools/testing.html` | `d6469f8ab18001cb4981ef1259120992f449e2a189335d046d5eecd61903d815` |
| `tools/cli.html` | `798c73c140e74ff0ba56584091bec92719538803b07cd41a29ff7d4118cc195e` |
| `tools/http-api.html` | `46f99ec08ec0ccc81fe98e9388999abafd8b12bafde2df0f4472f0392bd7287c` |
| `tools/url-scheme.html` | `122e410e4df190aacafe8d1a3a863d6b8c4a76715768c9d7f4efce675597b4b3` |
| `tools/panel.html` | `50bf4b587b19fc33bb265ae3715a59440f9e4fd57b62fe577a2c5a08ed526385` |

## 附录 B：取文本的办法

三步，都只把页面当文本读，不运行取来的任何东西。

1. 取首页 `https://manual.nssurge.com/`，从它的目录里收集 `data-path` 属性，得到页的清单（第一段脚本）。
2. 逐页取 HTML（`curl -s`），经第二段脚本变成文本：只留正文，去掉脚本、样式与导航，`<code>` 换成反引号，标题换成 `#`，列表项换成 `- `，一个块一行。
3. 对每个文本文件算 SHA-256（`sha256sum`）。

页的清单：

```python
"""Writes the page list of the manual's summary, in order.

Usage: python -I -X utf8 pages.py <index.html> <out pages.txt>
"""
import re
import sys
from pathlib import Path

raw = Path(sys.argv[1]).read_text(encoding="utf-8", errors="replace")
pages = []
for path in re.findall(r'data-path="([^"]+)"', raw):
    path = path.strip()
    if not path.endswith(".html") or path in ("./", "index.html"):
        continue
    path = path.lstrip("./")
    if path not in pages:
        pages.append(path)
if len(pages) < 20:
    print("only", len(pages), "pages found; the summary may have changed its markup")
    raise SystemExit(1)
Path(sys.argv[2]).write_text("\n".join(pages) + "\n", encoding="utf-8", newline="\n")
print("pages", len(pages))
```

一页的文本：

```python
"""Write a manual page's text, one block per line.

Usage: python -I -X utf8 page_text.py <out-file> < page.html
"""

import html
import re
import sys


def main() -> None:
    raw = sys.stdin.buffer.read().decode("utf-8", errors="replace")
    # Keep the article only when the page marks it; else the whole body.
    m = re.search(r"(?is)<main\b.*?</main>", raw) or re.search(r"(?is)<article\b.*?</article>", raw)
    page = m.group(0) if m else raw
    page = re.sub(r"(?is)<(script|style|nav|header|footer)\b.*?</\1>", " ", page)
    page = re.sub(r"(?i)<code[^>]*>", "`", page)
    page = re.sub(r"(?i)</code>", "`", page)
    page = re.sub(r"(?i)<h([1-6])[^>]*>", lambda k: "\n" + "#" * int(k.group(1)) + " ", page)
    page = re.sub(r"(?i)<li[^>]*>", "\n- ", page)
    page = re.sub(r"(?i)<br\s*/?>", "\n", page)
    page = re.sub(r"(?i)</(p|li|h[1-6]|td|th|tr|div|pre|blockquote)>", "\n", page)
    page = re.sub(r"(?s)<[^>]+>", "", page)
    page = html.unescape(page).replace("\u00a0", " ")
    lines = [re.sub(r"[ \t]+", " ", line).strip() for line in page.split("\n")]
    lines = [line for line in lines if line]
    # The same bytes on every platform: the digests are taken of this file.
    with open(sys.argv[1], "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
```

每个文本文件开头的十来行是页的标题与导航留下的空列表项，正文从 `# 标题` 一行开始；摘要连它们一起算。首页（`index.html`）只用来取页的清单，不在 87 页之内。
