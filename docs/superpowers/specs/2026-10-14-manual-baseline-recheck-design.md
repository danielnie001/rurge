# 手册基线重对：设计（Surge Mac 6.10.0 / iOS 5.23.0 的手册对兼容性清单）

- 日期：2026-10-14
- 状态：由实施者定稿，没有经项目所有者评审。项目所有者的常设授权是"自己决定、继续做"；第 3 节的每一条都可以事后改。
- 对应需求：这一轮是"阶段 3 开工之前对一遍手册的基线"（M8 设计第 1 节；阶段 1 / M5b 计划「延后事项」第 75 行）。它改到的功能需求：FR-CFG-01、06（节与键），FR-RULE-01、02、06、08（规则类型、参数、GeoIP / ASN、rurge 自己的 DNS 连接的协议识别），FR-OUT-03、05、11、12（通用参数、`ss`、QUIC 阻断、WireGuard），FR-GRP-02（`subnet` 组，只改一条告警的文本），FR-DNS-04、07（加密 DNS、`use-local-host-item-for-proxy`）。
- 上游文档：`docs/surge-compatibility-matrix.md`（被对照的清单）、`docs/requirements.md`、`docs/acceptance/phase1-acceptance.md`、`docs/acceptance/phase2-acceptance.md`；这一轮的调查结果在 `docs/reports/2026-10-14-manual-baseline-recheck.md`（下面写"报告"指它），实施计划在 `docs/superpowers/plans/2026-10-14-manual-baseline-recheck-plan.md`。
- 调研只读（2026-10-13 ～ 14）：Surge 手册 <https://manual.nssurge.com/> 的全部 87 页（2026-10-14 的文本，与 2026-10-13 取的一份逐页相同；每页文本的 SHA-256 在报告的附表里，手册的文字不进仓库）、Surge 官方频道公开的发行说明（只用来确认手册对应的版本）、本仓库的源码，以及两处第三方的开源代码，各只取一个事实：一个订阅转换器读旧式 `custom` 行的写法（GPL，只取"第几个字段是什么"），一个 Surge 面板读 `GET /v1/profiles/current` 的应答时用的类型定义（MIT，只取"应答有哪几个键"）。没有下载、安装、构建或运行本仓库之外的任何东西；没有在 Surge 上观察过任何行为。

## 1. 背景与范围

兼容性清单对照的是 Surge Mac 6.9 / iOS 5.22 时期的手册。手册之后改过：2026-10-13 Surge Mac 6.10.0（12500）与 iOS 5.23.0 正式发布，今天读到的手册就是这一版的手册。清单里已有几行标着"比手册基线新"（`GET /v1/geoip`、两个外部资源端点、`GEOIP,UNKNOWN` 与 `IP-ASN,UNKNOWN`、`include-device-communication`），都是做别的事时碰上的；把整本手册对着清单重对一遍，是阶段 3 开工之前登记着的一件事，这一轮做它。

这一轮产出三样东西：

1. **清单与手册一致**：手册今天的每一句，清单里要么有一行说 rurge 怎么办，要么有理由不设行；清单里引用手册的每一句，今天的手册里还在。
2. **阶段 1、2 已经声称支持的条目，手册的新句子落到代码上**：读出来的缺口里，小而且能在回环上验证的，这一轮补上（第 4 节的十一处）；其余登记。
3. **基线前移**：项目的各份文档改称 Surge Mac 6.10.0 / iOS 5.23.0。

不在这一轮：

- **一行怎样拆分**（引号与括号）。读发行说明时发现 rurge 的拆分比手册说的宽，带撇号的名字会吃掉一行的后半（第 6 节）；改它要同时改解析器与脱敏，单独设计，排在这一轮之后、阶段 3 的设计之前。
- 阶段 3 及以后的条目只改清单的文字（把手册的新句子写进去，供那个阶段的设计从它出发），不写代码：`[IP Rewrite]` 的语义、`subnet` 组的临时覆盖、DNS 服务器主机名上的规则、模块的几处新能力、脚本的几处新句子。
- 发行说明里 2026-10-13 之后的测试版条目（自适应的 `block-quic`、`GET /v1/policies?include_external=true`、跨多个包的 QUIC 主机名识别、QUIC v2 / draft-29）：手册没有一页写到它们，不属于这一轮；报告里点了名，下一次重对从那里开始。
- Surge 的知识库、博客与社区文档：没有读。
- 三平台 CI 与手工验收：不是这一轮能产出的证据。

## 2. 做法

手册是 HonKit 生成的静态站点，没有公开的源；把 87 页各取成一份文本（去掉导航与脚本，保留正文的行），每页记下 SHA-256。清单 919 行，按节分给十一个只读的助手，每个助手拿到手册的几页与清单里对应的几节，逐句给出四种结论之一：`same`（清单有这一句、说法一致）、`missing`（手册有、清单没有）、`differs`（两边说法不同）、`more`（清单有这一行，而手册对这一项多说了那一行没有写的：缺省、上限、取值、单位、版本、平台、与别的项的关系）。`more` 只用于阶段 1、2 的行；阶段 3 及以后的行，除非 `missing` 或 `differs`，一律记 `same`，手册比那些行多说的细节这一遍没有收集（第 8 节）。第一遍共 2,757 条：`same` 2,439，`missing` 75，`differs` 31，`more` 212。每个助手另外列出它那几节里"读的这几页不支持"的清单行，共 46 行。

第二遍三个只读的助手：两个对着代码核对第一遍里挑出来的 47 处（阶段 1、2 的条目里手册的句子是新的、或与清单不同的；配置与规则 27 处，运行期 20 处），一个把清单里说"手册没有写"的 69 行（85 处）逐处拿今天的手册再查一遍。

三条规矩：

- **`same` 不是证据。** 助手说一致的，没有人逐条复核；它只表示"读的人没有看出不同"。
- **每一条 `missing` / `differs` / `more` / 代码不符，都由主持这一轮的会话自己对着页面文本、清单的行与（阶段 1、2 的条目）代码核对之后才裁定。** 助手报的差异里有十五条是误报——另一页手册支持清单的那一行——，报告 7.3 节列了。
- **清单的一行要写进关于 rurge 的新说法时，引用的用例名先用 `git grep` 核对。**

## 3. 已决事项

MB-D1 ～ D22 是在项目所有者"自己决定、继续做"的常设授权下定的，所有者可以改。

| 编号 | 决定 | 理由 | 错了的代价 |
| --- | --- | --- | --- |
| MB-D1 | 项目的手册基线改为 Surge Mac 6.10.0 / iOS 5.23.0 的手册（2026-10-14 的文本，逐页 SHA-256 在报告里） | 它是手册今天描述的正式版，清单的每一行这一轮都对着它读过 | 对行为没有；某一行对 6.9 是对的而被说成 6.10 的，下一次重对从记下的摘要开始，会查出来 |
| MB-D2 | 手册在小节标题上标的版本号（"iOS 5.x+ Mac 6.x+"）不逐行补进清单 | 清单的"Surge 版本"列只在版本影响 rurge 怎么办时才有用；逐行补是几百处与行为无关的改动 | 想知道某项从哪一版起有的人要去看手册 |
| MB-D3 | 登记着一处与手册不同的行，状态标记是 ✅ 的改成 🟡 | 图例：✅ 是"语义与 Surge 一致"。这一轮改五行：通配命名节重名（`W0020` 而不是加载失败）、`[Keystore]` 的 DSA 私钥、`GEOIP`（Surge 随包带一份国家库，rurge 不带，第一次下载之前规则不匹配）、WireGuard 策略共用隧道、`GET /v1/profiles/current`（回纯文本而手册说应答总是 JSON；脱敏的范围） | 附录的数字要重数一次 |
| MB-D4 | 十一处代码改动进这一轮（第 4 节），各自先写失败的用例，各一个提交（4.9 节的八条用例本来就该是绿的，见那一节） | 每一处都是阶段 1、2 已经声称支持的条目上手册的一句话，小，在回环上验证得了；项目的做法是重读查出的小缺口当场补 | 每一处是一个可以单独撤回的提交 |
| MB-D5 | `GEOIP,UNKNOWN` / `IP-ASN,UNKNOWN`：数据库**没有载入**时不匹配 | 手册说的是"查询返回空结果"；Surge 总有一份随包的库，"没有库"这种情形只在 rurge 里有（rurge 不捆绑库，第一次下载之前没有）。没有库时别的 `GEOIP` / `IP-ASN` 规则都不匹配，`UNKNOWN` 若在这时匹配，一条 `GEOIP,UNKNOWN,REJECT` 会在第一次下载完成之前拒掉所有会话 | 把"没有库"当成"查不到"的使用者得不到匹配；`I0003` 与一条 WARN 说了库没有载入 |
| MB-D6 | `[IP Rewrite]` 现在就是已知的节（有序、留到阶段 3），不再是未知的节 | 手册的节清单有它；未知节的 `W0002` 说的是"不认识"，而 rurge 认识它、只是还没有 TUN | 没有 |
| MB-D7 | 旧的 `custom` 类型读作 `ss`：端口之后的两个位置字段是加密方法与口令，第三个（模块的链接）忽略，其后的命名参数照 `ss` 读；每行一条 `I0001` | 手册："parsed as a Shadowsocks policy; the external module URL in its declaration is ignored"。手册没有给字段的位置；这个位置是 Surge 2 / 3 时期的写法，一个开源订阅转换器也这样读 | 位置读错的行得到 `encrypt-method` 的错误，而不是悄悄连错 |
| MB-D8 | `tcp://` 的 DNS 服务器写了路径或查询串的是 `W0026`；`tls://` 不动 | 手册只对 `tcp://` 说"may only contain a host and an optional port" | 今天带着路径也能用的 `tcp://` 服务器（路径被丢掉）这一轮起被跳过并告警；结尾单独一个 `/` 照旧接受 |
| MB-D9 | `wireguard` 行上的 `tos` 生效 | 手册：通用参数适用于 WireGuard，例外只有 `interface` 与 `shadow-tls-*` | 没有；握手包照旧另标 `0x88` |
| MB-D10 | "永不匹配"的告警（`W0013`）也看 `[Rule]` 里 `AND` / `OR` / `NOT` 的子规则 | 今天 `AND,((PROCESS-NAME,x),(DEST-PORT,443)),P` 不声不响地永不匹配，`NOT,((SCRIPT,s)),P` 不声不响地总是匹配 | 多出告警，不改匹配 |
| MB-D11 | `use-local-host-item-for-proxy`：`[Host]` 给一个名字列了几个地址时随机取一个 | 手册原话："If the record has multiple addresses, one is chosen at random." 清单 6.3 节的那一行早就这样写着，代码取的是第一个 | 依赖"总是第一个"的配置（没有理由这样依赖）行为变了 |
| MB-D12 | 跟随出站模式下 DNS 会话的协议标签按 URL 的方案，不按端口 | 手册按方案列标签；按端口时，不在 443 的 DoH3 服务器被标成 `DOQ`，不在 853 的 DoT 服务器被标成 `DNS`，`PROTOCOL` 规则对它们匹配不上 | 写了 `PROTOCOL,DOQ` 来匹配一个不在 443 的 `h3://` 服务器的配置要改成 `PROTOCOL,DOH3` |
| MB-D13 | 八条用例钉住代码已有、而没有用例的行为（4.9 节） | 清单的行要引用用例；这八处今天只靠读代码 | 没有 |
| MB-D14 | 一行怎样拆分这一轮不改，单独设计，排在这一轮之后 | 第 6 节 | 带撇号的名字在那之前仍然加载不了，诊断指到那一行 |
| MB-D15 | 手册前后不一的六处，各取一句并在清单里把两句都写出来：`SYSTEM_VERSION` 的示例（取 Requirement 页的 `System Version …`）、`test-timeout` 的缺省（取策略组参数页的那一句）、`block-quic = always-allow` 管不管直连类策略（取"allowing everything"）、两条 `CELLULAR-*` 规则的平台（取平台差异页的 iOS / tvOS）、Shadow TLS 与其余 QUIC 类协议（取 MASQUE 页的"不能组合"）、代理服务器的主机名进不进 `[Host]`（取"从不匹配"的那一句） | 每一处的两句不能同时成立；rurge 现有的行为各合其中一句，这一轮不为它们改代码 | 清单里写着两句，读的人自己能判断 |
| MB-D16 | 登记而不改的（第 5 节）：手册说"静默忽略"而 rurge 多一条告警的、Surge 自带而 rurge 没有的、rurge 比手册宽或严的 | rurge 的诊断是它自己的；宽严的每一处都有登记的理由 | 清单里有，下一个读的人看得到 |
| MB-D17 | `surge-cli lsp`（Mac 6.10.0 新增的编辑器语言服务器）在清单里是 ❓，不设阶段 | 要不要给 rurge 做语言服务器是产品范围的问题，归项目所有者；`rurge check --json` 今天给出同一类诊断 | 没有 |
| MB-D18 | 阶段 3 ～ 7 的条目里手册的新句子写进各自的行，不写代码 | 那些阶段的设计从清单出发 | 没有；这些行上手册多说的细节这一遍没有收全（第 8 节），各阶段的设计开始时要把它那几页重读一遍 |
| MB-D19 | `[General] block-quic = all-proxy` 只盖过代理策略的参数：直连类策略（`DIRECT` 与 `direct` 别名）照它自己的 `block-quic`（4.10 节） | 手册："Overrides the proxy policies' `block-quic` parameter, blocking all QUIC traffic through proxies."——它说的是代理策略；今天这个值下直连类一律放行，写了 `block-quic=on` 的 `direct` 别名也不阻断 | 依赖"`all-proxy` 下直连一律放行"而又给直连类策略写了 `block-quic=on` 的配置（两句互相矛盾的写法）行为变了 |
| MB-D20 | `subnet` 组的 `W0008` 换一句说实际发生的事的话（4.11 节）；组的语义不动 | 今天的文本说"the first member is used"，而这样的组没有成员表，rurge 在每个网络上用的是它的 `default`；告警的话不该让人去找一个不存在的"第一个成员" | 没有；匹配文本的脚本要改 |
| MB-D21 | `GET /v1/profiles/current` 这一轮不改形状，照旧回 `text/plain`；清单那一行从 ✅ 改成 🟡 并写明 | 手册只说应答总是 JSON，没有给这个端点的 JSON 形状；唯一的线索是一个第三方面板的类型定义。照一个第三方的读法改形状，会打破今天读纯文本的 `rurge` 命令行与使用者的脚本，而换来的"兼容"没有手册可依 | 照手册写的客户端今天读不了这个端点；阶段 6 对齐应答形状时一起做 |
| MB-D22 | `CORE_VERSION` 照旧报 22；新编码登记给项目所有者 | Surge 自 Mac 6.8.0 / iOS 5.21.0 起报 `major * 1000000 + minor * 1000 + patch`；需求 FR-CFG-08 说这个值"随实现进度递增"，而 22 与 `6008000` 之间没有值可取——改报哪一版的数等于声称 rurge 到了哪一版，是产品上的表态 | 照新编码写的 `#!REQUIREMENT CORE_VERSION>=6008000` 在 rurge 里不成立，那样的行不生效 |

## 4. 十一处改动

每一处写到"语义、接口、判据"为止；改哪些文件、要哪些用例在计划里。4.1 ～ 4.8 与 4.10、4.11 改行为或诊断的文本，4.9 只加用例。

### 4.1 `GEOIP,UNKNOWN` 与 `IP-ASN,UNKNOWN`（FR-RULE-06）

手册 IP 页（Unknown Database Results，iOS 5.102.0+ / Mac 6.9.1+）："Use `UNKNOWN` (case-insensitive) to match a destination IP for which the database lookup returns no country or ASN"；"`GEOIP,UNKNOWN` matches an empty country result; `IP-ASN,UNKNOWN` matches an empty ASN result. Both forms also work in rule sets, where the policy is omitted."；"These rules require a destination IP. An unresolved domain is not an unknown database result: normal DNS resolution and `no-resolve` behavior still apply."

今天：`GEOIP,UNKNOWN` 能加载，`UNKNOWN` 被读成国家码，不是两个字母，规则编译成永不匹配，没有任何提示；`IP-ASN,UNKNOWN` 是加载错误 `E0011`，写在规则集文件里是一条被跳过的行。

语义：

- 取值是 `UNKNOWN`（不分 ASCII 大小写）时，规则匹配这样的目标地址：对应的数据库**已经载入**，而对它的查询没有给出国家（没有记录、记录里没有国家码、国家码不是两个字母、查询出错）或没有给出 AS 号。
- 数据库没有载入时不匹配（MB-D5），照旧只有那一条每个库一次的 WARN。
- 目标是域名时照 `GEOIP` / `IP-ASN` 的老规矩：没写 `no-resolve` 就先解析，写了就跳过；判的是解析出的第一个 IPv4 地址，没有 IPv4 时是第一个 IPv6 地址。
- `[Rule]`、逻辑规则的子规则、规则集文件、内联 `[Ruleset]` 里都能写；`pre-matching`、`no-resolve` 照它们的类型。

接口：

- `rurge_config::rule::RuleKind` 多两个不带数据的变体 `GeoIpUnknown` 与 `IpAsnUnknown`；`type_name()` 仍是 `GEOIP` / `IP-ASN`（能力表、`W0013`、`GET /v1/rules` 的类型都不变），`is_ip_based()` 与 `supports_pre_matching()` 同它们的类型。显示用的文字照旧是使用者写的那一行。
- `rurge_rules::matcher::GeoLookup` 的两个方法改为返回三种结果之一：

  ```rust
  /// What a database says of an address.
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

  "没有库"与"查不到"由同一次调用、读同一个数据库句柄给出，不是先问一句"有没有库"再问一次结果。`NoGeo` 两个方法都是 `NotLoaded`。
- 规则集的编译：`UNKNOWN` 的两种行进逐条求值的那一列（按文件里的顺序），不进 AS 号的表；没写 `no-resolve` 时这个集合要先解析名字，同 `GEOIP` 行。
- `GET /v1/geoip` 与 `GeoDb::describe` 不变（它们本来就把"没有库"与"查不到"都报成 `null`）。

判据：一份测试用的数据库里没有的回环地址，`GEOIP,UNKNOWN` 与 `IP-ASN,UNKNOWN` 匹配，库里有的地址不匹配；没有库时两者都不匹配；`rurge check` 对 `IP-ASN,unknown` 不再报 `E0011`；规则集文件里的两种行不再被跳过。

不改的：`GEOIP` 后面不是两个字母、也不是 `UNKNOWN` 的词（`GEOIP,USA`）照旧能加载、永不匹配、没有提示——登记在那一行。

### 4.2 `[IP Rewrite]` 是已知的节（FR-CFG-01）

手册（节清单与 `features/ip-rewrite.html`，iOS 5.23.0+ / Mac 6.10.0+）：逐行的节，每行 `<IP 地址或 CIDR> = <动作>`，动作是 `reflect` / `reject` / `drop`，自上而下、第一条命中的生效；只作用于虚拟网卡收到的流量，在 TCP / UDP 协议栈、规则与策略之前。

它要 TUN，属于阶段 3。这一轮只让解析器认识它：有序的节，行原样留着，同 `[Port Forwarding]`、`[SSID Setting]` 这些留给后面阶段的节一样进"解析了、这一版不生效"的那一条 `W0016`，不再是"未知的节"的 `W0002`。动作与地址这一轮不校验（阶段 3 的设计定诊断）。

判据：带 `[IP Rewrite]` 的 Profile 加载后 `W0016` 的列表里有它、没有 `W0002`，节里的行按书写顺序留着。

### 4.3 旧的 `custom` 类型读作 `ss`（FR-OUT-05）

手册（`policies/shadowsocks.html`）："The legacy `custom` policy type from very old configurations is parsed as a Shadowsocks policy; the external module URL in its declaration is ignored. Use the `ss` type instead."

今天：`custom` 是未知的类型，`E0004`，含它的 Profile 加载不了。

语义：`Name = custom, <服务器>, <端口>, <加密方法>, <口令>[, <模块链接>][, 命名参数…]` 读作同一台服务器、同一种方法与口令的 `ss` 策略。

- 端口之后的位置字段，第一个是加密方法，第二个是口令，第三个（有的话）是模块的链接、读过即丢；再多的位置字段照旧是"多余的位置值"（`W0001`）。
- 同一行上命名的 `encrypt-method=` / `password=` 胜过位置上的（同 `http` 的用户名口令）。
- 位置字段怎样认，同今天所有带位置凭据的行：字段里有 `=` 的被读成命名参数。口令里带 `=` 的旧行因此读不出口令，得到 `encrypt-method` / `password` 的那条错误——与 `http, 主机, 端口, 用户, 口令` 的行是同一个已登记的限制；脱敏按同一种读法遮字段，两边一致。
- 其余一切是 `ss` 的：方法名的校验（手册不再列出的方法名照旧 `W0007` + REJECT，流式方法照旧 `W0031`）、`obfs`、`udp-relay`、通用参数、重载时的指纹。
- 主配置里每一行一条 `I0001`：``policy `<名字>`: legacy type `custom` read as `ss`; the module URL is ignored``。订阅导入的 `custom` 行同样读作 `ss`，不另说什么（那不是使用者能改的行）。
- 类型关键字的表里 `custom` 排在 `ss` 之后，所以这种策略在诊断、API 与日志里的类型是 `ss`；`GET /v1/policies/detail` 显示的那一行是使用者写的原文（脱敏之后），仍以 `custom` 开头。

凭据：脱敏今天就把这样的行从第四个字段起的位置字段全部遮掉（它认"第三个字段是端口"的行）；标记用例（`crates/rurge-config/tests/secrets.rs`）的那份配置加一行能加载的 `custom`，口令带标记。

判据：一行 `custom` 加载出与对应的 `ss` 行相同的 spec；经引擎的端到端用例里它连上回环的假 Shadowsocks 服务端；`rurge check` 给出 `I0001` 而没有错误；标记用例找不到标记。

### 4.4 `tcp://` 的 DNS 服务器只写主机与端口（FR-DNS-04）

手册（`dns/encrypted-dns.html`）："A `tcp://` URL may only contain a host and an optional port."

今天：`tcp://host/anything` 被接受，`/` 之后的被丢掉。

语义：`tcp://` 之后除了主机与可选的端口还有别的（路径、查询串、片段；结尾单独一个 `/` 不算）时，这个上游是 `W0026`：`` `<写的那一段>`: a tcp:// URL names a host and a port, nothing else ``，与 `quic://` 今天的检查与措辞相同；带用户名口令的照旧先被拒（文本只引用最后一个 `@` 之后的部分）。`dns-server` 里的 `tcp://` 与 `encrypted-dns-server` 里的走同一个解析函数，一起变。`tls://` 不动（手册没有对它说这句话）。

判据：`tcp://127.0.0.1:5353/x`、`tcp://127.0.0.1?x` 是 `W0026`；`tcp://127.0.0.1:5353/` 与 `tcp://127.0.0.1` 照旧。

### 4.5 `wireguard` 行上的 `tos`（FR-OUT-03、12）

手册（`policies/wireguard.html`）："Common policy parameters apply, with L3-specific exceptions: `interface` binding and the `shadow-tls-*` parameters are not supported for WireGuard policies."；"Following the WireGuard protocol recommendation, Surge marks handshake packets with DSCP `0x88` (AF41) to improve handshake success on networks that honor this class. Regular tunnel packets are not automatically marked."

今天：`tos` 写在 `wireguard` 行上是 `W0028`，被清成 0。

语义：`tos` 不再是不适用的参数；它到达隧道自己的 UDP socket（直连时）。握手的发起包照旧标 `0x88`，发完之后标记回到策略自己的 `tos`（没写就是 0）——载体的 `set_tos(0)` 本来就是"回到策略自己的"。经 `underlying-proxy` 时载体不是一个本机的 socket，`tos` 没有作用（同 `hysteria2` 等经载体时）。`interface`、`allow-other-interface`、`tfo` 照旧 `W0028`（`interface` 手册点了名；另两个没有东西可作用），`ecn` 照旧 `W0029`，`shadow-tls-*` 照旧 `E0018`。

指向同一个节的几个 `wireguard` 策略共用一条隧道（第 5 节登记的做法），隧道的 socket 是起它的那个策略的连接器开的：这几个策略的 `tos` 不同时，隧道的标记是起它的那个策略的；重载只改了 `tos` 时，旧隧道还被连接拿着就继续用旧的标记，到它结束为止。决定谁与谁共用隧道的那一串（载体键）不加 `tos`——加了的话，同一个节上 `tos` 不同的两个策略每拨一次就顶掉对方的隧道、重置对方的连接。

判据：`wireguard, section-name=…, tos=0x28` 加载没有告警、spec 的 `tos` 是 `0x28`；经引擎的用例里 socket 钩子依次见到 `0x28`（建 socket 时）、`0x88`、`0x28`。

### 4.6 "永不匹配"的告警看进逻辑规则（FR-RULE-01）

今天 `W0013`（``` `<类型>` rules never match on <平台> in this version ```，每种类型每次加载一条）只看 `[Rule]` 顶层规则的类型。

语义：检查走进 `AND` / `OR` / `NOT` 的子规则（任意层）。只在子规则里出现的类型也报，报在包含它的那条顶层规则的位置上；每种类型仍只报一次，文本不变。`[Ruleset]` 与规则集文件里的行不看（同今天）。

判据：`AND,((PROCESS-NAME,x),(DEST-PORT,443)),P` 与 `NOT,((SCRIPT,s)),P` 各得一条 `W0013`；顶层已经报过的类型，子规则里再出现不重复报。

### 4.7 `use-local-host-item-for-proxy` 随机取一个地址（FR-DNS-07）

语义：`[Host]` 给目标名字列了几个地址时，交给代理的是其中随机的一个（每个会话各取一次，取法同 `load-balance` 的随机：标准库的随机哈希），不再总是第一个。不看策略的 `ip-version`（照旧，登记）；别名、`server:`、`script:` 条目照旧不受影响。

判据：一个名字映射到几个回环地址，经假 SOCKS5 上游发几十个会话，上游见到的目标地址不止一个，且都在那几个地址之内。

### 4.8 DNS 会话的协议标签按 URL 的方案（FR-RULE-08、FR-DNS-04）

手册（`dns/encrypted-dns.html`）按方案列出跟随出站模式下 `PROTOCOL` 规则可用的标签：DoH 是 `DOH`，DoH3 是 `DOH3`，DoQ 是 `DOQ`，DoT 是 `DOT`，明文 TCP 是 `DNS`。

今天：标签按端口猜——TCP 连接 853 是 `DOT`、443 是 `DOH`、其余 `DNS`；QUIC 载体 443 是 `DOH3`、其余 `DOQ`。不在缺省端口上的服务器标错。

语义：`https://` 的连接是 `DOH`，`h3://` 的载体是 `DOH3`，`quic://` 的载体是 `DOQ`，`tls://` 的连接是 `DOT`，`tcp://` 的连接是 `DNS`，与端口无关。`[Host]` 的 `server:` 写的加密服务器同样。

接口：解析器的依赖从"一个连接器"变成"每种上游一个连接器"——

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
```

解析器给五个连接器各包一层引导解析（共用一个 `Bootstrap`），DoH 的内部 HTTP 客户端建在 `https` 那一个上。引擎一侧，`PipelineConnector` 仍是握着引擎与回落连接器的那一个对象，另给出五个带标签的薄连接器（`PipelineConnector::connectors()`），各自以固定的 `ProtocolKind` 建内部会话；`StackOptions.dns_connector` 的类型随之变成 `Option<DnsConnectors>`。`ConnectOpts` 不加字段（它只有一个字段，全仓库有几十处字面量在构造它）。

判据：一个不在 853 的 `tls://` 服务器的内部会话匹配 `PROTOCOL,DOT`，一个不在 443 的 `https://` 匹配 `PROTOCOL,DOH`，一个在 853 的 `tcp://` 匹配 `PROTOCOL,DNS`，一个不在 443 的 `h3://` 的载体匹配 `PROTOCOL,DOH3`，一个在 443 的 `quic://` 的载体匹配 `PROTOCOL,DOQ`。

不在这一轮：手册同一段还说可以给 DNS 服务器的主机名写规则；rurge 的内部会话带的是引导解析之后的地址，域名规则匹配不上（清单今天就登记着）。它要让会话带着名字又不再解析一次，随阶段 3 的会话模型一起做。

### 4.9 八条钉住现有行为的用例

| 用例说的是 | 手册 | 今天靠什么 |
| --- | --- | --- |
| 逻辑规则的策略名里有成对的括号（`AND,((DOMAIN,a.example),(DEST-PORT,443)),Proxy (US)`）读得对 | 6.10.0 的发行说明修了 Surge 自己在这里的一个错 | 拆分函数的括号计数，没有用例 |
| `ipv6 = false` 时，目标是 IPv6 字面量的请求经 DIRECT 照常到达 | `[General]` 页："Even if this option is not enabled, you can still access IPv6 sites by using IPv6 addresses directly." | 读代码 |
| 组的 `external-policy-modifier` 给导入的行设了 `underlying-proxy`，组自己也写了 `underlying-proxy` 时，用的是组自己的 | 策略组参数页："If both are present, the group-level `underlying-proxy` takes precedence over an `underlying-proxy` set through `external-policy-modifier`." | 读代码 |
| `URL-REGEX` 的模式没有写锚点时匹配 URL 的任何位置 | 规则页的例子 | 正则库的 `is_match` |
| 发给 WireGuard 隧道自己 IPv6 地址的 ICMPv6 回显请求得到应答，发给别的地址的没有 | "Surge only answers ICMP/ICMPv6 echo requests sent to the configured local tunnel address." | 协议栈库的行为，IPv4 有用例、IPv6 没有 |
| 经 `underlying-proxy` 时，UDP 路径把主机名目标原样交给底下的载体（WireGuard 的 endpoint 写成域名时走的就是这条路） | WireGuard 页："endpoint hostname resolution can occur through that connector rather than locally" | TCP 路径有用例、UDP 没有 |
| 组的 `underlying-proxy` 作用于每一种来源的成员：`include-all-proxies` 与 `include-other-group` 带进来的也派生出经中继的策略 | 策略组参数页："It applies to members from every source — explicitly listed members, `policy-path`, `include-all-proxies`, and `include-other-group`" | 已有的用例只有显式写的成员与 `policy-path` 两种来源 |
| 跟随出站模式下，DNS 会话跟随的是出站模式：直连模式走 DIRECT，全局模式走全局策略，规则模式才过规则 | 加密 DNS 页："encrypted DNS connections follow the outbound mode settings and are matched against the rules like normal requests" | 已有的用例都在规则模式下；另两种模式读代码 |

这八条里哪一条写出来是红的，就是又查出一处缺口：照计划的规矩停下来裁定，不顺手改。

### 4.10 `block-quic = all-proxy` 只管代理策略（FR-OUT-11）

手册（`profile/general.html`）："`all-proxy`: Overrides the proxy policies' `block-quic` parameter, blocking all QUIC traffic through proxies."；同一张表的另三句："`per-policy`: Determined by each policy's `block-quic` parameter. This is the default value."、"`all`: Overrides all policies' `block-quic` parameters, blocking everything including DIRECT policies."、"`always-allow`: Overrides the proxy policies' `block-quic` parameter, allowing everything."

今天（`rurge-engine` 的 `quic_blocked`）：`all-proxy` 下经代理的一律阻断，直连类策略（`DIRECT` 与 `direct` 别名）一律放行——连写了 `block-quic=on` 的 `direct` 别名也放行。手册说这个值盖过的是**代理策略**的参数，直连类策略的参数它没有盖过。

语义：

| 全局取值 | 经代理 | 直连类 |
| --- | --- | --- |
| `per-policy`（缺省） | 策略自己的：`on` 阻断，`off` 放行，`auto` 阻断 | 策略自己的：`on` 阻断，`off` 放行，`auto` 放行 |
| `all-proxy` | 阻断 | **策略自己的**：`on` 阻断，`off` 与 `auto` 放行（这一轮改的一格） |
| `all` | 阻断 | 阻断 |
| `always-allow` | 放行 | 放行 |

`always-allow` 一行不动：手册那一句的前半说它盖过的是代理策略的参数，后半说"allowing everything"，两半对写了 `block-quic=on` 的直连类策略说法不一；rurge 照后半（MB-D15）。

接口：`quic_blocked(global, policy, direct) -> bool` 的签名不变，只改 `AllProxy` 一支。

判据：单元用例的那张表里 `(all-proxy, on, 直连)` 从放行变成阻断，另加 `(all-proxy, auto, 直连)` 与 `(all-proxy, off, 直连)` 两行（都放行）；其余各行不变。

### 4.11 `subnet` 组的告警说实际发生的事（FR-GRP-02）

`subnet` 组要知道此刻在哪个网络上，那是阶段 3 的事；在那之前它在每个网络上都代表它的 `default`（阶段 2 / M3a）。加载时每种没有实现的组类型给一条 `W0008`，文本今天对所有类型是同一句：``policy group type `<类型>` is not implemented in this version; the first member is used``。能力表里现在只剩 `subnet` 一种会得到它，而 `subnet` 组没有成员表——它的各项是"网络条件 = 策略"，手册对 `default` 的说法是"The policy used when no subnet expression matches."。

语义：`subnet` 组的 `W0008` 改成 ``policy group type `subnet` is not implemented in this version; its `default` policy is used on every network``。别的组类型（今天没有）照旧用原来的那一句。组怎样求值不动；没写 `default` 的 `subnet` 组照旧（加载时不报，运行时按空组告警并兜底），是不是加载错误随阶段 3 定（第 5 节）。

判据：带一个 `subnet` 组的 Profile 的 `W0008` 是新的那一句，里面没有"first member"。

## 5. 登记而不改的

每一条在清单里各有一行写着手册的原话与 rurge 的做法；这里只列名字与理由，逐行的改动在报告的第 5 节。

- **手册前后不一**（MB-D15，六处）：`SYSTEM_VERSION` 的示例（Requirement 页带开头的 `System `，模块页不带）；`test-timeout` 的缺省（`[General]` 页一句，策略组参数页一句）；`block-quic = always-allow`（同一句的前半说盖过代理策略的参数，后半说"allowing everything"）；`CELLULAR-RADIO` / `CELLULAR-CARRIER` 的平台（平台差异页说 tvOS 也有，规则页的标题仍写 iOS Only）；Shadow TLS 与 TUIC / WireGuard / Tailscale 之外的 QUIC 类协议（TLS 参数页说"没有意义"，MASQUE 页说"不能组合"；rurge 一律 `E0018`，合后一句，所以这不是"rurge 比手册严"）；代理服务器的主机名进不进 `[Host]`（加密 DNS 页建议"给代理的域名加一条 `[Host]` 映射"，本地 DNS 映射页说"代理服务器的主机名从不与 `[Host]` 匹配"）。
- **rurge 多说一句**：未知的规则参数，手册说"silently ignored"，rurge 同样忽略并给一条 `W0003`。
- **Surge 自带、rurge 没有**：两个 GeoIP 数据库（Surge 捆绑两份、只下载国家库的更新；rurge 一份不捆绑、两份都下载，ASN 库另有 `--geoip-asn-url`，第一次下载之前相关规则不匹配）；WireGuard 原生测速"报告应答的 endpoint"（rurge 只报时间）；"点对点策略"这个概念（手册：点对点的 WireGuard 策略只提供往返时间的测试；rurge 没有这个概念，对这样的策略照样去测，得到的是那一次测试自己的结果）；"互联网连通性检查"（`internet-test-url` 在 rurge 里只是直连类策略的测试 URL）；WireGuard 上 ECN 标记的合并；日志里的 `FAILED` 与 `SUBSTITUTE` 两个标记。
- **rurge 比手册严**：`[Keystore]` 的 DSA 私钥（手册列为支持，rurge `E0022`：SSH 库没有编进 DSA）。
- **rurge 比手册宽**：通配命名节重名（手册：加载失败；rurge：`W0020`、用第一段）；`policy-path` 的列表里以 `;` 开头的行也算注释；`trust-tunnel` 行上的 `ws`（手册说它不能与 `h3` 合用，却没有在参数表里列出它；rurge 当未知参数，`W0001`）。
- **手册的一句可以读成两样**：`POST /v1/external_resources/update` 的"Local resources are skipped."。
- **rurge 自己的数字**：`smart` 组。手册现在讲了算法的轮廓，清单里"算法细节手册未公开"那一句去掉，换成 rurge 与它不同的地方（多一个"还没有样本"的状态）与 rurge 自己取的各个常数。
- **做法不同**：指向同一个节的 `wireguard` 策略共用一条隧道（手册：每个策略各有协议栈）；`h2-connect` 的服务端支不支持 extended CONNECT，Surge 在连接建立时核对，rurge 到第一个包才知道；`GET /v1/profiles/current` 回纯文本（MB-D21）。
- **这一轮看出来、留着的四件**：
  - 组的中继自己也在成员里时（如 `include-all-proxies` 把它带进来），同样派生出 `R (via R)`——经中继再连中继自己的一跳，多半不是使用者要的。手册没有说这种情形；用 `policy-regex-filter` 可以把它滤掉。读代码得出，没有用例，这样的成员拨号时怎样没有试过，这一轮不改。
  - 没写 `default` 的 `subnet` 组：手册标着必填，rurge 加载时不报、运行时按空组告警并兜底。是不是加载错误随阶段 3（那时这个组才真的求值）。
  - 手册说跟随出站模式时"可以给 DNS 服务器的主机名写规则"：rurge 的内部会话带的是引导解析之后的地址，域名规则匹配不上。要让会话带着名字又不再解析一次，随阶段 3 的会话模型。
  - `CORE_VERSION` 的新编码（MB-D22）：归项目所有者。

## 6. 一行怎样拆分（不在这一轮）

发行说明里有一条"修复了逻辑规则错误解析带括号的策略名"，由此看了 rurge 自己的拆分函数（`rurge_config::value::split_list`，三十来处调用，规则、策略、组、`[Host]` 都经它）：

- 引号在一项的**任何位置**都开启一段引用，直到配对的引号；圆括号在任何位置都计层。
- 所以一个没有配对的 `'`、`"` 或 `(` 会把这一行余下的部分全吃进去：`USER-AGENT,Bob's App*,Proxy` 是 `E0001`（缺策略）；`G = select, Tom's HK, Tom's US` 把两个成员读成一个。成对的括号没有问题（4.9 节钉住它）。
- 手册只认"整个值用引号包起来"（规则页、策略页、格式页各一句），没有说一个值中间的引号算什么；Surge 怎样读这样的行观察不到。

合理的规则是"引号只在一个值的开头才开启引用——一项的开头，或它第一个 `=` 之后——；没有配对的引号或括号是普通字符"。这一轮不改，两条理由：这个函数的每一个调用者都跟着变；脱敏照着它的读法遮字段（`redact.rs` 里五个扫描函数），它的安全论证恰好建在今天的读法上——"没有配对的引号一直管到行尾，对脱敏是安全的一侧"。解析器与脱敏要在同一次改动里一起改、一起评审。

这一轮做的：清单 1.1 节加一行写明 rurge 今天的读法、哪样的行会加载不了、怎样绕开；下一步是它自己的设计（排在阶段 3 的设计之前）。绕开的办法只对"引用名字的地方"成立：规则的值、组的成员、规则里的策略名，把那一项整个用双引号包起来；`[Proxy]` 里 `=` 左边定义名字的地方照原样写、不加引号（用 `rurge check` 试过：加了引号，那个名字就没有被定义），`[Proxy Group]` 里 `=` 左边的名字没有试过。新的一行的全文在报告第 5 节的 R5。

## 7. 文档

- **兼容性清单**：表头的基线一句与"最近一次"；报告第 5 节列出的每一处（123 项：补手册的句子、改写过时的说法、五行改标记、新增十行——`[IP Rewrite]`、`surge-cli lsp`、旧的 `custom` 类型、一行怎样拆分各一行，与第 9 节新的一个小节六行）；写着"比手册基线新"的几行去掉这句话；十一处改动对应的行改成做了之后的样子并引用用例；附录整张重数（预期 555 行变成 565 行，以脚本数出来的为准）。
- **需求文档**：参考基线一行；`[General]` 的键数 57 改 58（FR-CFG-06；阶段 1 补遗（M5b）已经认了第 58 个键）；第 7 节的状态行。阶段栏不动。
- **阶段 1 验收文档**：`GEOIP,UNKNOWN` / `IP-ASN,UNKNOWN` 那一句从"没有做完"挪到"补上了"，三个数随之变成"2 句没有做完、37 句阶段栏与登记不一致、14 句有意不同"；引用这三个数的地方（清单表头、阶段 2 验收文档的那一小节、CLAUDE.md、README）一起改。
- **两份手工清单**：回环上验不了的三件各加一项——`wireguard` 的 `tos` 在线上的 DSCP 位、一行真实的旧 `custom` 策略对真实的服务端（阶段 2 的清单，新的一节两项），真实的 GeoLite2 库之下的 `GEOIP,UNKNOWN` 与 `IP-ASN,UNKNOWN`（阶段 1 的清单「与平台无关」一行）；两份清单的项数与引用它们的地方随之改。
- **API 参考**：`docs/api/phase2.md` 的"跟随出站模式下 DNS 的内部会话"一节（标签按方案）；`docs/api/phase1.md` 两处——「外部资源」一节里说这两个端点晚于手册基线的话去掉（它们现在在基线之内），`GET /v1/profiles/current` 的 `text/plain` 旁边加一句"手册说应答总是 JSON，rurge 的形状随阶段 6 对齐"（MB-D21）。
- **CLAUDE.md**：事实来源一句的基线；状态一段加这一轮；文档清单加三份；"接下来的工作"把"对一遍手册的基线"换成"一行怎样拆分的设计"。
- **README / README_en**：状态一段。

## 8. 这一轮没有覆盖的，和不能由它推出的

- **Surge 的实际行为**：一处也没有观察。清单里"与 Surge 一致"说的是"与手册的文字一致"。
- **助手判为 `same` 的 2,439 条**没有人逐条复核。
- **代码**：第二遍只核对了 47 处。阶段 1、2 其余各行，这一轮比的是清单的文字与手册的文字，不是代码与手册——阶段 1 的需求逐句对过代码（`docs/acceptance/phase1-acceptance.md`），阶段 2 的那一遍（2026-10-10）判为"做了"的没有人复核，这一轮没有改变这一点。
- **阶段 3 及以后的行上手册多说的细节**：对这些行，助手照规矩只报"清单没有这一项"与"说法不同"；手册比那一行多说的缺省、取值、版本与平台没有收集。每个阶段的设计开始时，要把它那几节对应的手册页对着清单重读一遍（计划「延后事项」登记）。
- **取文本时丢掉的**：图片（连同截图里的字）、链接指向哪里、表格的行列关系、粗体与斜体。页面上靠这些表达的意思，这一轮读不到。
- **手册之外**：知识库、博客与社区文档没有读；发行说明只读了官方频道最近的一页，用来确认手册对应的版本、点出手册还没有写到的东西，没有逐条去对清单。
- **两处靠第三方的事实**：旧的 `custom` 行里字段的位置（一个开源订阅转换器的读法），`GET /v1/profiles/current` 的三个键（一个开源面板的类型定义）。都不是手册，也不是对 Surge 的观察。
- **CI 与手工验收**：照旧——CI 没有绿过，所有用例只在一台 Windows 11 x86-64 的开发机上跑过；两份手工清单一行也没有做过。

## 9. 任务草图

1. 配置层的六处小改动：`[IP Rewrite]`、`custom`、`tcp://`、`wireguard` 的 `tos`、`W0013` 看进逻辑规则、`subnet` 组的 `W0008` 文本。
2. `GEOIP,UNKNOWN` / `IP-ASN,UNKNOWN`。
3. 运行期的两处与用例：`use-local-host-item-for-proxy` 的随机地址，`block-quic = all-proxy`，与八条钉住现有行为的用例里的七条。
4. DNS 会话的协议标签按方案，与第八条用例（DNS 会话跟随出站模式）。
5. 文档。

任务之间只有文档依赖前四个；前四个互不依赖，按上面的顺序做。

## 10. 实施期的订正

与第 4 ～ 7 节不同的做法；计划层面的出入在计划的「执行期修正记录」。

- **4.1**：取值去掉首尾空白之后等于 `UNKNOWN`（不分 ASCII 大小写）才是，两种规则一样；`GeoLookup` 与 `GeoResult` 都从 `rurge_rules` 的根导出。
- **4.3**：位置上写错的加密方法，`E0018` 不引用它（`9a01a32`）：少写了方法的行里，那个位置上的是口令；命名的 `encrypt-method=` 写错时照旧引用。
- **4.7**：`rurge_policy::random_below` 是 `smart` 组用的那个函数；`load-balance` 自己写着同一个式子。取法相同（标准库的随机哈希），只是函数是两处。
- **4.8**：`PipelineConnector` 自己不再是一个 `Connector`：五个面是一个私有的类型 `Face`，各带一个固定的 `ProtocolKind`、只是转调，建内部会话的逻辑在 `connect_as` 与 `connect_udp_as`（`42c2294`）。4.8 节的五条判据都在 `a_dns_session_is_labelled_by_its_scheme_whatever_the_port` 一条用例里，另多一行 `[Host]` 的 `server:tls://`；五个面各自的标签由 `the_five_faces_carry_the_five_labels`（一个离线的真引擎）钉住，解析器把每种上游交给自己那一个连接器由 `each_kind_of_upstream_dials_through_its_own_connector` 钉住。缺省端口上的服务器标签与以前相同。
- **4.9**：八条写出来都是绿的，没有查出缺口。第八条（`a_dns_session_follows_the_outbound_mode`，`745beda`）的全局策略是一个拒绝一切的回环代理，不是 REJECT：链底是 REJECT 的 DNS 会话绕过它直连，看不出会话交给了谁。
- **第 7 节**：三个数是"2 句没有做完、37 句阶段栏与登记不一致、10 句有意不同"，不是第 7 节写的 14 句：4.8 让 DNS 连接的标签照手册按方案定，阶段 1 验收文档里原来"有意不同"的四句（FR-RULE-08 的 DoH、DoT、DNS 连接，FR-DNS-04 的 `PROTOCOL` 规则匹配）随之改记"补上了"。清单 3.1 节 `PROTOCOL` 一行里按端口的说法随清单一并改了，4.6 节「WireGuard 生命周期」一行里"其它包不标"随 `tos` 订正（`6f33016`）——报告第 5 节都没有为它们列改动。阶段 2 验收文档里写着日期（2026-10-13）的两段没有改写，各在后面加了一段 2026-10-14 的。清单附录数出来是 565 行，与预期相同。
