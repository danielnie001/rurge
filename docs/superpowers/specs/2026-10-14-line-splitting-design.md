# 一行怎样拆成各项：设计（引号与括号的读法；解析器与脱敏一起改）

- 日期：2026-10-14
- 状态：由实施者定稿，没有经项目所有者评审。项目所有者的常设授权是"自己决定、继续做"；第 3 节的每一条都可以事后改。
- 对应需求：FR-CFG-01（"引号值与 `\"` `\\` 转义、含逗号的引号值"）与 FR-OBS-01（"凭据不落日志"——这里是诊断与脱敏后的视图）。没有新的需求行，阶段栏不动（阶段 1）。
- 上游文档：手册基线重对的设计 `docs/superpowers/specs/2026-10-14-manual-baseline-recheck-design.md` 第 6 节（问题从那里来）与它的计划 `docs/superpowers/plans/2026-10-14-manual-baseline-recheck-plan.md`「延后事项」第 1、9 行（这一份接手的两行）；兼容性清单 1.1 节"一行怎样拆成各项（引号与括号）"一行；实施计划在 `docs/superpowers/plans/2026-10-14-line-splitting-plan.md`。
- 调研只读（2026-10-14）：Surge 手册（手册基线重对时取的 2026-10-14 文本）里讲引号与括号的各句；RFC 4180；Go 标准库 `encoding/csv` 的文档；CPython `Modules/_csv.c` 的源码（只读它的状态机认引号的那几处）；两个开源的订阅转换器读 Surge 策略行的代码（都是 GPL，只取"它们怎样认引号"这一个事实）；本仓库的源码与离线的 `rurge check`（数据目录在草稿目录里，不联网）。另用一个不进仓库的草稿脚本把第 4 节的读法与今天的读法对着穷举了一遍（第 6.2 节）。没有下载、安装或运行本仓库之外的任何东西；没有在 Surge 上观察过任何行为。

## 1. 背景与问题

规则、策略、组、`[Keystore]`、`[Host]`、WireGuard 的 peer、`#!include` 与 `[General]` 的列表都经同一个函数拆成各项：`rurge_config::value::split_list`（三十来处调用）。脱敏（`crates/rurge-config/src/redact.rs`）有一份照抄它的状态机（`next_comma`），据此决定一个值在哪里结束。手册基线重对读发行说明时看到了它的问题（那份设计第 6 节），这一轮把它改掉。今天的读法与它带来的五件事：

1. **引号在一项的任何位置都开启一段引用**，直到下一个同样的引号。带撇号的名字于是吃掉一行的后半：`G = select, Tom's HK, Tom's US` 把两个成员读成一个（`E0008`），`USER-AGENT,Bob's App*,DIRECT` 缺策略（`E0001`），`#!include` 的文件名里有撇号也一样。
2. **圆括号在任何位置都计层**，没有配对的 `(` 一直管到行尾：`DOMAIN,b.example,Proxy (US,extended-matching` 把 `Proxy (US,extended-matching` 读成策略名（`E0007`），`URL-REGEX,^http://a\.example/[^(]*$,REJECT` 缺策略（`E0001`）。配对的括号没有问题（`a_policy_name_with_balanced_parentheses_is_read_whole` 钉着）。
3. **`key="…"` 的值被去两次引号**：`split_list` 去一次，`ParamMap::insert` 再去一次，`split_definition` 还去掉两头的空白。`sni="\"a.example\""` 读成 `a.example`，`password=" pw "` 读成 `pw`。手册的"Quoted Values"一节说的恰恰相反：引号与转义让含引号或反斜杠的值"saved and reloaded without changing their contents"。`with_params` 为了绕开它把这样的值引两次（`param_value`）。
4. **整个用引号包起来的一项仍按 `key=value` 读**：位置上的口令含 `=` 时，没有办法写对。`http, 127.0.0.1, 8080, alice, pa=ss` 与 `…, alice, "pa=ss"` 都读成参数 `pa`（口令没有读到），旧的 `custom` 行同样（重对计划「延后事项」第 9 行）。
5. **告警说出了口令的前一半**：上面不带引号的那一行得到 `W0001` `policy \`PwEq\`: unknown parameter \`pa\` ignored`——`pa` 是口令 `pa=ss` 的前一半（`written_as_name` 只看写法像不像参数名）。

另有一处不一致：今天的 `unquote` 只看两头是不是引号，`"P1" "x"` 读成 `P1" "x`（`E0008` 报的就是这个名字）。

用离线的 `rurge check` 看到的（`a.conf` 是草稿目录里的一份 Profile；`Proxy (US`、`P1`、`Tom's HK`、`Tom's US` 都定义了）：

| 行 | 今天 |
| --- | --- |
| `PwEq = http, 127.0.0.1, 8080, alice, pa=ss` | `W0001` `unknown parameter \`pa\` ignored`，口令没有读到 |
| `PwEqQuoted = http, 127.0.0.1, 8080, alice, "pa=ss"` | `W0001` `unknown parameter ignored`，口令没有读到 |
| `G = select, Tom's HK, Tom's US` | `E0008` `references unknown policy \`Tom's HK, Tom's US\`` |
| `"Quoted G" = select, DIRECT` 与 `DOMAIN,c.example,"Quoted G"` | `E0007` `rule references unknown policy \`Quoted G\``（定义的名字是带引号的 `"Quoted G"`） |
| `G2 = select, "P1" "x"` | `E0008` `references unknown policy \`P1" "x\`` |
| `USER-AGENT,Bob's App*,DIRECT` | `E0001` `USER-AGENT: missing policy` |
| `URL-REGEX,^http://a\.example/[^(]*$,REJECT` | `E0001` `URL-REGEX: missing policy` |
| `DOMAIN,b.example,Proxy (US,extended-matching` | `E0007` `rule references unknown policy \`Proxy (US,extended-matching\`` |
| `URL-REGEX,^http://a\.example/"b,c"d,P1` | 加载得了，值是 `^http://a\.example/"b,c"d` |

## 2. 别人怎么读

**手册**只讲"整个值用引号包起来"一种写法，没有一句说值中间的引号或括号算什么：

- `rules/overview.html`："If a value contains commas (e.g. a regex), wrap it in double or single quotes."
- `policies/overview.html`："Quote a value if it contains commas."
- `rules/http.html`（`URL-REGEX`）："If the value contains commas, wrap it in double quotes"；`rules/logical.html`："Sub-rule values containing commas may be quoted."
- 参数页的写法都是 `key="…"`：`alpn`、WireGuard 的 `allowed-ips`（"quote the value if it contains commas"）、`external-policy-modifier="test-url=http://apple.com/,tfo=true"`、`include-other-group="group1,group2"`、`policy-priority="Premium:0.9;Backup:1.3"`。
- `profile/format.html` 的"Quoted Values"（iOS 5.21.0+ / Mac 6.8.0+）："Inside a double-quoted profile value, use `\"` for a literal double quote and `\\` for a literal backslash. This allows values containing quotes or backslashes to be saved and reloaded without changing their contents."
- 括号只出现在两处结构里：逻辑规则的子规则（`AND,((Rule1),(Rule2),...),Policy`）与 WireGuard 的 peer（"Each peer is a parenthesized field list"）。Mac 6.10.0 的发行说明有一条"Fixed logical rules incorrectly parsing policy names containing parentheses."
- 定义名字的那一边（`Name = …`）手册没有说能不能加引号。

**通行的做法**都是"引号只在字段开头才开启引用"：

- RFC 4180：含逗号、引号或换行的字段整个用双引号包起来。
- Go `encoding/csv`："Fields which start and stop with the quote character " are called quoted-fields."；缺省时字段中间的引号是错误（`bare " in non-quoted-field`），`LazyQuotes` 时"a quote may appear in an unquoted field"——当普通字符。
- CPython 的 `csv`：只在"字段开始"的状态里认 `quotechar`；字段中间（`IN_FIELD`）根本不看它；引号的伴之后（非严格模式）接着的字符照常进这个字段。
- 两个开源的订阅转换器读 Surge 的策略行：一个只在 `key=` 之后把引号认作一段值的开头，另一个按逗号直接切开、不认引号。

rurge 今天的"任何位置都开启"是这几种里最宽的，而且是唯一会让一个撇号吃掉一行后半的读法。

## 3. 已决事项

LS-D1 ～ D13 是在项目所有者"自己决定、继续做"的常设授权下定的，所有者可以改。

| 编号 | 决定 | 理由 | 错了的代价 |
| --- | --- | --- | --- |
| LS-D1 | 引号（`"` 或 `'`）只在**一个值开始的地方**开启引用：一项的开头——这段文字的开头、引用之外的逗号之后、引用之外的左括号之后——或自上一个这样的位置以来第一个 `=` 之后；中间可以有空白（`char::is_whitespace` 认的空白，含全角空格与不换行空格）。别处的引号是普通字符 | 手册的每一句都是"整个值用引号包起来"；RFC 4180、Go 与 CPython 的 CSV 读法都只在字段开头认引号；带撇号的名字是真实的写法 | 一个值中间成对的引号原来护住的逗号现在切开（`password=ab"c,d"` 读成 `password=ab"c` 与 `d"`），这样写的行要把整个值用引号包起来；切出去的后半是一个普通的项：在组行上是一个成员，`E0008` 与组的 `Debug` 照样引用它 |
| LS-D2 | 值开始处开启的引用没有伴时，一直到这段文字的末尾（同今天） | 那是写的人想引起来而漏了收尾的引号：读成一项，诊断指到它；`with_params` 把追加的参数放在这样一项之前的逻辑与脱敏"没有伴的管到末尾"这一侧都照旧成立 | 那一项之后的各项被读进它里面，同今天 |
| LS-D3 | 圆括号只在**配对**时分组：引用之外的 `(` 与 `)` 按栈配对（一个 `)` 配它前面最近的、还没有配上的 `(`），配上的一对之间的逗号不切；配不上的 `(` 与 `)` 是普通字符。认引号不看括号配不配对：左括号之后总是一项的开头 | `Proxy (US`、正则里的 `[^(]` 是真实的写法；子规则与 peer 只用配对的括号；"认引号不看配对"让读法两遍就能做完（第 5 节），也让子规则或 peer 里第一项的引号照旧开启（`("a)b",c)`） | 子规则里的正则有不配对的括号时打乱配对，要把那个值用引号包起来（手册："Sub-rule values containing commas may be quoted"）；一个不配对的左括号紧跟着引号时那个引号仍开启引用（`Proxy ("US, Other` 照旧是一项） |
| LS-D4 | 伴的规则同今天：`"` 的伴是其后第一个没有被转义的 `"`（引用里 `\` 带走紧跟着的一个字符），`'` 的伴是其后第一个 `'`（没有转义）。伴之后，这一项照普通文字往下读。只有**整个在引号里**的一项（去掉两头空白之后以开启的引号开头、以它的伴结尾），或 `=` 之后整个在引号里的值，去掉引号；别的原样，引号字符留着 | 只改"哪里开启"，伴不变，改动最小；CPython 也在伴之后接着读这个字段；"整个在引号里"按伴判断，`"P1" "x"` 不再被剥成 `P1" "x` | 伴之后还有文字的值保留引号字符（`"Fast" Proxy` 是名字 `"Fast" Proxy`，同今天） |
| LS-D5 | 一个共用的扫描：两遍、线性、一次给出整段文字的全部切分处；`split_list`、新的 `split_fields`、`with_params`、`written_as_name` 与脱敏都经它。今天的扫描只留在脱敏里，用来量一项会被带到哪里（LS-D9） | 解析器与脱敏对"一项在哪里结束"必须一致；今天靠两份各自写的状态机保持一致。括号配不配对要看到后面，所以分两遍；每个切分处之后重新开始会让一行的代价变成平方 | 一次扫描多两个与引用之外的标点个数等长的数组（标点的位置、括号配上没有） |
| LS-D6 | 整个在引号里的一项是一个**值**，不是参数：`"pa=ss"` 是值 `pa=ss`。凡读 `key=value` 的列表都如此：策略行、组行、`[Keystore]`、WireGuard 的 peer、`external-policy-modifier` 的列表、规则的参数。名字里不会有 `=`（定义在第一个 `=` 处切开），所以带 `=` 的名字——组行里这样的一项成了成员——不在诊断与 `Debug` 里引用（`policy::is_name` 多认一种） | 手册"Quote a value if it contains commas"——引号的意思是"这是一个值"；手册的"Quoted Values"说引起来的值存进去、读回来不变，照这个规矩写出来的 Profile 会把含 `=` 的位置值整个引起来；今天给位置上的口令加引号也躲不开 `=`（重对计划「延后事项」第 9 行） | 把整个 `key=value` 用引号包起来当参数写的行，参数不再生效：策略行多一条 `W0001`（`unexpected positional value #n ignored`，不引用取值），组行多一个找不到的成员（`E0008`，加载错误，不引用名字），peer 少一个字段（`a field without \`=\` ignored`），`[Keystore]` 少一个字段（`W0001`）；这样写的人要改成 `key="…"` |
| LS-D7 | 引号只去一次，引起来的值**原样**保留：里面的引号字符、两头的空白都在。`key="\"v\""` 得 `"v"`，`password=" pw "` 得 ` pw `；没有引号的值照旧去掉两头空白。`with_params` 写回去时只引一次 | 手册的"Quoted Values"；去两次、再去空白让这一句不成立 | 依赖"去两次"的写法（`key="'v'"` 今天读成 `v`）以后读成 `'v'`；值两头有意加的空白以后真的在值里 |
| LS-D8 | 读位置凭据的类型（`http`、`https`、`socks5`、`socks5-tls`、`h2-connect` 与旧的 `custom`）上，站在第 4、5 个字段（端口之后的头两个，按解析器读到的字段数，空的一项不算）的一项读成未知参数时，`W0001` 不点出它的名字，并带一句它自己的提示：含 `=` 的用户名或口令要整个用引号包起来。提示的文字固定，不引用这一行的任何内容 | 那两个位置上多半是一个含 `=` 的口令，名字就是口令的前一半；LS-D6 之后加引号就能写对，提示告诉人怎么写 | 在那两个位置上写错的参数名不再被点名（同一行别处的照旧点名） |
| LS-D9 | 脱敏看的是**区域**：解析器的一项，连同今天的读法从这一项的开头读下去会带进来的后续各项（读到的末尾往后对齐到解析器的下一个切分处）。逐项那一遍：区域的第一项是秘密参数，它的值连同并进来的各项一起遮掉；不是，就在整个区域里按名字找。位置凭据那一遍：要遮的字段连同它的区域一起遮掉。逐项那一遍先做、在原文上量；位置凭据那一遍随后在它写出的文字上按解析器的读法数字段 | 值中间的引号或括号今天护住了逗号，写的人多半是那样想的；读法变了，被切出去的后半仍可能是口令的一部分。区域的末尾是两种读法里较晚的那一处：草稿脚本找到了新读法比今天结束得晚的写法（6.2 节），只量一种都会漏。逐项先做，今天那几条"值在第一个顶层逗号处结束"的输出才不变（位置凭据先做会把 `password=ab"c,d"` 后半的 `d"` 先换成 `***`，今天的读法从 `ab"c` 读下去就一直到行尾） | 少数行上遮掉的比口令多（同今天：今天的读法本来就遮到那里）；位置凭据那一遍量的是逐项写出之后的文字，被换成 `***` 的值里的引号不在了，从它前面的字段量起可能多遮或少遮一段（少遮的那一段仍按自己的字段号遮，除非它读起来是命名参数） |
| LS-D10 | 全角逗号 `，` 只在脱敏数位置凭据时算作切分处，不开启一个值，也不改变引号与括号的状态；解析器照旧不认它 | 输入法打出的 `，` 要算进字段数（今天如此）；若让它开启一个值，紧跟在它后面的引号会让脱敏对字段的划分与解析器错开 | 没有 |
| LS-D11 | `=` 左边定义的名字照原样读，不去引号（`[Proxy]`、`[Proxy Group]` 都同今天） | 手册说的是值；新的读法下名字里的撇号与括号不再需要引号，名字里本来不能有 `=`（定义在第一个 `=` 处切）；去掉定义处的引号会让 `"A" = …` 与 `A = …` 成为同一个名字，重名的判定跟着变 | 照别的工具的习惯给定义处的名字加了引号的 Profile，那个名字带着引号被定义，引用处整个加引号的写法对不上它（今天也对不上） |
| LS-D12 | 不加新的诊断码，不加新的告警；LS-D8 只给已有的 `W0001` 换一句话、带一句提示 | 读法变了是行为变化，写进兼容性清单；多出来的告警会落在每一行写得古怪的配置上 | 读法变了的行（LS-D1、LS-D6）没有专门的告警，读的人从后续的诊断（未知策略、多余的位置值）看出来 |
| LS-D13 | 行内注释（`text::strip_inline_comment`）与行尾的要求指令（`requirement::find_suffix_marker`）认 `"` 的方式这一轮不改：它们只认双引号，在任何位置开启 | 它们决定一行的哪一段是注释或指令，作用于每一个节的每一行（包括取值不是列表的节），脱敏照它们的顺序剥注释；改它们是另一件事 | 一个值中间的 `"` 之后、同一行的行内注释仍认不出来，注释并进这一行的最后一项（今天也是这样，而且今天整行读坏） |

## 4. 新的读法

### 4.1 切分处

一段文字（一行的 `=` 右边、一条规则、一个括号组的内部、一个参数的取值）按下面的规则切成项：

1. **值开始的地方**：这段文字的开头、引用之外的每一个逗号与左括号之后——不管它在括号里多深、括号配不配对——，以及自上一个这样的位置以来第一个 `=` 之后。空白不改变"在值开始的地方"这件事。
2. **引用**：在值开始的地方遇到的 `"` 或 `'` 开启一段引用，到它的伴为止；没有伴就到这段文字的末尾。`"` 的引用里 `\` 带走紧跟着的一个字符（什么字符都一样）；`'` 的引用里没有转义。别处的 `"` 与 `'` 是普通字符。
3. **括号**：引用之外的 `(` 与 `)` 按栈配对，配上的一对是一个括号组；配不上的是普通字符。
4. **切分处**：引用之外、又不在任何括号组里的逗号。

第 1、2 条是第一遍（只看引号，不看括号配不配对），第 3 条是第二遍（只看第一遍认出来的引用之外的括号），第 4 条由两遍的结果读出。

### 4.2 每一项读成什么

去掉两头空白之后为空的一项丢掉，整个在引号里而引号里什么也没有的一项（`""`、`''`）也丢掉（都同今天，第 10 节）。其余的：

- **整个在引号里**（以第 2 条开启的引号开头、以它的伴结尾）：一个**值**，去掉引号；`"` 的引用里 `\"` 还原成 `"`、`\\` 还原成 `\`，别的 `\x` 原样留着（同今天的 `unquote`）；里面的空白与 `=` 都原样。
- 否则，这一项里第一个开启了一个值（第 1 条里那种：自上一个值开始的地方以来的第一个 `=`）、又不在任何括号组里的 `=`，它前面去掉空白不为空时：一个**参数**。名字是这个 `=` 之前的文字，去掉两头空白，原样（不去引号、不改大小写——`ParamMap` 存的时候照旧改成小写）；值是这个 `=` 之后的文字：整个在引号里时去掉引号、原样保留，否则去掉两头空白。`peer = (a = b)` 是参数 `peer`，`a(b)=c` 是参数 `a(b)`；`(a=b)`、`(a=b)=c` 与 `=(a)=b` 是值（`=` 在括号组里；不是开启值的那一个；这一项里第一个那样的 `=` 前面是空的——后面再有一个也不算）。
- 否则：一个**值**，去掉两头空白的原文，引号字符原样留着。

`split_list` 给出的文字照旧：值给它的文字，参数给 `名字=值`——值整个在引号里时是去掉引号的值，否则是这一项去掉两头空白的原文（今天 `a = b` 给出 `a = b`，照旧）。读参数的调用者改用 `split_fields`（第 5 节），从那里拿到"是值还是参数"与原样的值。

### 4.3 例子

| 写法 | 今天读成 | 以后读成 |
| --- | --- | --- |
| `select, Tom's HK, Tom's US` | `select` ·`Tom's HK, Tom's US` | `select` · `Tom's HK` · `Tom's US` |
| `USER-AGENT,Bob's App*,DIRECT` | `USER-AGENT` · `Bob's App*,DIRECT` | `USER-AGENT` · `Bob's App*` · `DIRECT` |
| `DOMAIN,b.example,Proxy (US,extended-matching` | 三项，最后一项是 `Proxy (US,extended-matching` | `DOMAIN` · `b.example` · `Proxy (US` · `extended-matching` |
| `URL-REGEX,^http://a\.example/[^(]*$,REJECT` | 两项 | 三项 |
| `AND,((DOMAIN,a.example),(DEST-PORT,443)),Proxy (US)` | 三项 | 三项（不变） |
| `URL-REGEX,"^http://example\.com/(a\|b),?c",Proxy` | 三项，值去掉引号 | 同左 |
| `password="p,w"` 与 `x='a,b'` | 参数，值 `p,w`、`a,b` | 同左 |
| `password=ab"c,d"` | 一项，参数值 `ab"c,d"` | 两项：参数 `password`（值 `ab"c`）与值 `d"` |
| `a=b="c,d"` | 一项，参数值 `b="c,d"` | 两项：参数 `a`（值 `b="c`）与值 `d"`（第二个 `=` 之后不是值开始的地方） |
| `"pa=ss"` | 参数 `pa`，值 `ss` | 值 `pa=ss` |
| `sni="\"a.example\""` | 参数值 `a.example` | 参数值 `"a.example"` |
| `password=" pw "` | 参数值 `pw` | 参数值 ` pw ` |
| `"P1" "x"` | 值 `P1" "x` | 值 `"P1" "x"` |
| `"Fast" Proxy` | 值 `"Fast" Proxy` | 同左 |
| `P1, "P1`（第二项没有伴） | `P1` · `"P1` | 同左 |
| `Proxy ("US, Other` | 一项 | 一项（左括号之后的引号开启引用，没有伴，到末尾） |
| `(USER-AGENT,"a)b")`（子规则） | 引号护住 `)` | 同左 |
| `peer = (public-key = k, allowed-ips = "10.0.0.0/8, 192.168.0.0/16", endpoint = a:1)` | 一项 | 同左（左括号与逗号之后各是一项的开头，`allowed-ips =` 之后是值开始的地方） |

## 5. 实现的形状

改动都在 `rurge-config` 里；别的 crate 只经 `split_list`、`parse_policy` 等已有的入口，没有接口变化。没有新的依赖。

- **扫描**（`crates/rurge-config/src/value.rs`）：`pub(crate) fn item_spans(text: &str, wide: bool) -> Vec<ItemSpan>`，`ItemSpan { start, end, sep_end, eq }`——每一项的起点、终点、它后面分隔符的终点，与这一项里第一个开启值、又不在括号组里的 `=`（4.2 节；它前面为空时这一项仍是值）的位置；`wide` 时 `，` 也切（LS-D10）。第一遍认引号，记下引用之外的 `,`、`(`、`)`、开启值的 `=`（与 `wide` 时的 `，`）；第二遍用一个放在堆上的栈给括号配对（一行十万个左括号也只是一个数组），然后按配上的括号数层、读出切分处与每一项的 `=`。整段文字只读一遍，第二遍与之后只走记下的标点，不在切分处之后重新开始。`pub(crate) fn split_items(text: &str) -> Vec<&str>` 给出各项的原文（含两头空白）。
- **项**：
  ```rust
  /// One item of a list as every parser reads it (design 4.2).
  #[derive(Clone, PartialEq, Eq)]
  pub struct Field {
      /// What `split_list` gives for the item.
      pub text: String,
      /// The parameter the item is: its key as written, and its value with
      /// its quotes taken off once. `None` for a value.
      pub param: Option<(String, String)>,
  }
  pub fn split_fields(text: &str) -> Vec<Field>;
  pub fn split_list(text: &str) -> Vec<String>; // 每一项的 `text`
  pub fn unquote(text: &str) -> String;         // 只去"整个在引号里"的引号
  ```
  `Field` 的 `Debug` 手写成 `Field(***)`：一项可以是口令。
- **`ParamMap`**：`from_fields(&[Field]) -> (ParamMap, Vec<String>)`；`insert` 不再去引号、不再去值两头的空白，值照给的存（键照旧去空白、改小写）。
- **改用 `split_fields` 的调用者**（LS-D6、LS-D7）：`policy::parse_policy`、`policy::parse_group`、`policy::with_params`（认键；`param_value` 只引一次）、`keystore::parse_keystore_item`、`wireguard` 的 peer 字段、`spec::group` 的 `modifier`、`rule::RuleParams::parse`（规则的参数部分）。`keystore::unknown_field` 与 `spec::reader::written_as_name` 照旧读文字（它们看的是一项写成什么样，整个在引号里的一项写出来不像名字，照旧不点名）。只要文字的调用者（`[General]` 与 `[Host]` 的列表、`#!include`、`include-other-group`、`allowed-ips` 的列表、规则的类型、值与策略、子规则的标志）照旧用 `split_list`。
- **`policy::is_name`**：带 `=` 的文字不是名字（LS-D6）。
- **LS-D8**：`ParamReader::finish` 决定点不点名时，除了 `written_as_name`，还看读成这个参数的项有没有站在读位置凭据的类型的第 4、5 个字段上；有，就用不点名的那句话，并以 `Diagnostic::with_hint` 带上那句提示。
- **脱敏**：第 6 节。

## 6. 脱敏

### 6.1 规则

- **切分处**：脱敏用第 5 节同一个扫描（`item_spans`）切出解析器的各项；`split_top_level` 去掉。今天的扫描（`next_comma`：引号在任何位置开启、括号在任何位置计层、没有伴或没有配上的一直到末尾）留在脱敏里，只用来量区域。
- **区域**（LS-D9）：从一项的开头用今天的读法读到它的第一个顶层逗号（没有就是末尾）。这一处落在这一项的末尾之后时，这一项连同读到那里为止的后续各项是一个区域，区域的末尾往后对齐到解析器的下一个切分处；否则这一项自己是一个区域。下一个区域从这个区域之后的第一项起量。每次量都止于自己的区域，所以一段文字只走两遍（新的扫描一遍，今天的扫描一遍）。
- **逐项那一遍**（`redact_items`，先做，在原文上；括号组里同样，至多 4 层）：
  - 只有一项的区域：同今天的 `redact_item`。这一项去掉引号之后的文字（`split_list` 给的）读起来是秘密参数，就遮掉它的值——整个在引号里的 `"password=…"` 照旧只留 `password=***`，虽然解析器现在把它读成一个值（LS-D6）；括号组进去逐项再看；别的按名字找。
  - 有几项、第一项读起来是秘密参数：从它的值开始，一直遮到区域的末尾，并进来的各项一起遮掉。
  - 否则在整个区域里按名字找（`redact_by_name`）：找到的值按今天的读法量，止于区域的末尾；紧跟在一个引号后面的名字（`x"password=…`），值照旧一直到区域的末尾。
- **位置凭据那一遍**（`redact_positional_credentials`，随后，在逐项那一遍写出的文字上）：用同一个扫描切出字段（`wide`，LS-D10），按解析器的字段数数到第 4 个字段起，不是命名参数的整个遮掉——同今天，只是字段换成新读法的字段。要遮的字段连同它的区域（从这个字段的开头量，今天的读法在这里也认 `，`）一起遮掉，并进来的非空字段计入字段数。整个在引号里的一项照旧不算命名参数（`is_named_param` 早就这样），所以 `"pa=ss"` 在位置上被遮掉。第一个字段拆成的词（`words_of_type_field`：类型后面漏了逗号的 `http h.example 8080 alice hunter2`）照旧逐词遮，不按区域延伸——词与词之间是空白，不是逗号。
- **列表**：`[General]` 里列出地址的几个键（`hide_listed`：监听地址、控制器、DNS 服务器）与 `#!include` 的目标（`hide_include_links`）按区域切，一个区域整个当作一项交给原来的处理（藏掉最后一个 `@` 之前的部分、带 `://` 的整个换成 `***`）。这几种处理藏的都是"最后一个 `@` 之前"或整项，一个区域里有几项时藏得比按项多，不会少。
- 其余照旧：注释与指令按今天的顺序先剥去，按名字找照旧覆盖注释里与括号组太深处的秘密，URL 里的用户信息最后隐去。一行的第一个 `=` 前面有没有逗号（`redact_entry` 据此决定把这一行当作定义的两种读法都脱敏一遍，`redact_both_ways`）：两种读法里任何一种在那里看到逗号就算——多脱敏一遍只会藏得更多。`[General]` 一类键行认键（`key_line`）照旧只用今天的读法：换成"任何一种"会让 `a'b, http-listen = 口令@地址` 少藏——新读法在撇号后面看到逗号，这一行就不再被认作监听地址的键行，口令留在原处。

### 6.2 为什么是"两者中较晚的一处"

草稿脚本把第 4 节的读法与今天的读法（`redact.rs` 的 `next_comma`）照字面写出来，对字母表 `a , " ' ( ) = \` 上长度不超过 6 的全部字符串、再加三十万条长度 7 ～ 24 的随机字符串，比较两件事：

- **新读法的切分处，在任何一处切开之后重新读余下的部分都不变**：没有反例。脱敏与 `with_params` 把一项单独拿出来再读（`split_list(item)`），靠的就是这一条（连同"一项单独读还是这一项"，计划把它一并写进用例）。
- **从一项的开头读，新读法是不是总不晚于今天的读法结束**：不是。599,592 条里 4,147 个反例，最短的如 `a"(",` 与 `a"=",`：今天的读法里第一个引号开启、第二个收尾，逗号处结束；新读法里第一个引号是普通字符，左括号或 `=` 之后的第二个引号开启一段没有伴的引用，一直到末尾。

所以只量今天的读法会漏掉解析器读给秘密的后半，只量新读法会漏掉写的人按今天的读法以为是值的后半；区域取两者中较晚的一处。计划把这两条写成 Rust 里的用例（穷举短字符串，在门禁里跑）。

### 6.3 与今天相比

- 位置凭据按解析器的字段数：`X = http, Bob's-host, 80, alice, pw` 今天一个字也不遮（今天的读法把撇号之后读成一个字段，数不到第 4 个；解析器今天也读不了这一行），以后是 `X = http, Bob's-host, 80, ***, ***`。名字里不配对的括号同样。
- 组的成员照旧留着：`G = select, Tom's HK, policy-path=https://…` 今天与以后都是 `G = select, Tom's HK, policy-path=***`（以后的区域从 `Tom's HK` 量到行尾，在区域里按名字找到它）。
- 今天遮得掉的写法照旧遮得掉，输出不变：`trojan, h, 443, password=ab"c,d", ws=true`、`…password=ab'c,d'…`、`…password=a(b,c)d…` 都是 `trojan, h, 443, password=***, ws=true`；`external, x"password=V,W", tfo=true` 是 `external, x"password=***, tfo=true`；`external, "password=V,W"` 是 `external, password=***`；`T = trojan, h, 443, (password=V), tfo=true` 是 `T = trojan, h, 443, ***, tfo=true`。
- 属性测试（`whatever_the_parser_reads_as_a_secret_parameter_is_hidden_however_it_is_written` 与 `[Keystore]`、peer 的两条）的前提是"解析器把这种写法读成这个参数"。LS-D1、LS-D6 之后有几种写法解析器读成别的：`{name}=a"{V},{W}"b` 读成参数值 `a"{V}` 与另一项 `{W}"b`，整个在引号里的 `"{name}={V}"` 一类（五种）读成一个值。前者改写成解析器现在读到的值（`a"{V}`），被切出去的 `{W}"b` 是另一项：脱敏照样不许露出 `W`，`Debug` 只查解析器读进这个参数的那一段——在组行上 `{W}"b` 是一个成员，组的 `Debug` 照样列出它（LS-D1 的代价）；后者标成"解析器读成值"，对它们断言解析器**不**把它们读成这个参数，脱敏与 `Debug` 照样一个字节也不许露出来。其余的检查（`V`、`W` 与位置凭据一个字节也不许出现在脱敏的输出里、邻居照旧）不变。

### 6.4 留着的

位置上不带引号、含 `=` 的口令（`http, h, 80, alice, pa=ss`）在脱敏后的视图里照旧显示：`pa=ss` 读起来就是一个参数，解析器也把它读成参数，脱敏不知道每种策略认哪些参数名。这是一张"藏什么"的清单（尽力而为，M8c 计划「延后事项」第 51 ～ 77 行的规矩：不追更多的拼法）；LS-D8 的提示让人改成带引号的写法，那种写法解析器读成口令、脱敏也遮掉。

## 7. 使用者看得到的变化

加载得了、以前加载不了的：

- 名字与值里的撇号：`G = select, Tom's HK, Tom's US`、`USER-AGENT,Bob's App*,DIRECT`、`#!include` 的文件名。
- 名字与正则里不配对的括号：`DOMAIN,b.example,Proxy (US,extended-matching`、`URL-REGEX,^http://a\.example/[^(]*$,REJECT`。
- 位置上含 `=` 的用户名或口令整个加引号：`http, 127.0.0.1, 8080, alice, "pa=ss"`、旧的 `custom` 行的口令。

读法变了的：

- 一个值中间成对的引号不再护住逗号：`password=ab"c,d"`、`URL-REGEX,^http://a\.example/"b,c"d,P1`（后者以后读成策略 `c"d`）。要护住逗号，把整个值用引号包起来。
- 整个用引号包起来的 `key=value` 是值（LS-D6），凡读 `key=value` 的地方都如此：
  - 策略行：`"tfo=true"` 不再打开 TFO，多一条 `W0001`（`unexpected positional value #n ignored`）；`external` 的 `"args=…"` 同样成了多余的位置值，程序不带这个参数启动。
  - 组行：`"policy-path=…"` 成了一个找不到的成员（`E0008`，加载错误；带 `=` 的名字不在诊断里引用）。
  - WireGuard 的 peer：`"preshared-key=…"` 不读（`a field without \`=\` ignored`），peer 照常建起来而没有预共享密钥。
  - `[Keystore]`：`"password=…"` 不读（`W0001` `unknown [Keystore] field ignored`），`"base64=…"` 不读（缺 `base64`，加载错误）。
  - 规则的参数：`"notification-text=…"` 是一个不认识的参数（同今天写错了的参数）。
- 引起来的值只去一次引号、原样保留：`sni="\"a.example\""` 是 `"a.example"`，`password=" pw "` 是 ` pw `，`notification-text=" Hi "` 是 ` Hi `。
- 不是整个在引号里的一项保留引号字符：`"P1" "x"` 是名字 `"P1" "x"`（`E0008` 报的名字随之变）。
- 读位置凭据的类型上，端口之后头两个位置上的未知参数，`W0001` 不再点名，带一句提示。

不变的：配对的括号、整个值用引号包起来的写法、`key="…"` 里的逗号、没有伴的引号一直到末尾、`=` 左边定义的名字（带引号就带着引号被定义）、行内注释与行尾指令认引号的方式、空的一项丢掉。

## 8. 测试与判据

- **读法**（`value.rs`）：第 4.3 节表里的每一行一条断言（`split_fields` 与 `split_list` 各自的结果）；`unquote` 只去整个在引号里的引号；`ParamMap` 存的值原样；一行几十万个字符的 `(`、`)`、`"`、`'`、`(,`、`'a,`、`a=` 读得完（平方的写法会卡住，用例用不会被墙钟左右的长度）；穷举短字符串的性质（6.2 节：新读法在每个切分处之后重新读不变、每一项单独读还是这一项；区域的末尾不早于两种读法里任何一种的末尾）。
- **解析器**：带撇号的组成员、规则值与 `#!include` 文件名；不配对括号的策略名与正则；`a_policy_name_with_balanced_parentheses_is_read_whole` 照旧绿；子规则里整个加引号、含 `)` 的值；`"pa=ss"` 读成 `http` 的口令与旧 `custom` 行的口令（在 crate 里读 spec，口令只经 `Secret` 比较，不进输出）；第 7 节"整个用引号包起来的 `key=value` 是值"下的每一条；`key="\"v\""`、`password=" pw "` 与 `notification-text=" Hi "` 原样；`with_params` 写回的值再读出来逐字节相同（含引号、反斜杠、两头空白、逗号）；`is_name` 不认带 `=` 的文字，`E0008` 与组的 `Debug` 不引用这样的成员；LS-D8 的告警不点名、带提示，同一行别处的未知参数照旧点名，不读位置凭据的类型（`trojan`）上同一个位置的未知参数照旧点名。
- **凭据**：`crates/rurge-config/tests/secrets.rs` 里四处整个加引号的 `key=value`（`external` 的 `args`、组的 `policy-path`、`[Keystore]` 的 `password`、peer 的 `preshared-key`）改写成 `key="…"`，仍断言解析器读到它们；整个加引号的写法挪进"读不出来、也不许露出来"的那份 Profile；加位置上整个加引号、含 `=` 的口令（`http` 与 `custom` 各一），断言解析器把它读成口令，带标记的值在 `Debug`、诊断与脱敏输出里都找不到。
- **脱敏**：6.3 节的每一条；`a_value_ends_at_the_first_top_level_comma_wherever_its_quotes_open`、`a_quoted_value_is_blanked_whole_however_it_is_quoted`、`behind_a_quote_the_value_runs_to_the_end_of_the_item`、`an_item_is_hidden_the_way_the_parser_reads_it` 与 `the_items_of_a_group_are_hidden_the_way_the_parser_reads_them` 的期望输出不变；属性测试按 6.3 节改写后照旧检查三万条以上的行；`a_long_line_of_one_kind_is_redacted_in_time_that_grows_with_its_length` 加上 `'`、`a"(`、`a"=` 几种。
- **端到端**：`rurge check`（离线、`--data-dir` 在用例的临时目录）读一份含第 7 节"加载得了"各行的 Profile（位置上的口令写成 `"k3y=v4lue"`），没有错误、没有告警；把那个口令换成不带引号的 `k3y=v4lue` 时恰好一条 `W0001`，带 LS-D8 的提示，输出里没有 `k3y` 与 `v4lue`（不用 `pa`：`parameter` 里就有 `pa`）。

每条新用例先写、先看它红；每个改动另做"改一行代码看它变红"的验证（只在 `rurge-config` 的纯函数上做，不涉及网络与文件）。

## 9. 文档

- **兼容性清单**：1.1 节"引号值"一行的备注写 LS-D7（只去一次、原样保留）与用例；"值含逗号"一行不变；"一行怎样拆成各项"一行改写成新的读法（仍是 🟡：手册没有说的那几种情形——值中间的引号、不配对的括号、没有伴的引号、定义处带引号的名字、行内注释——是 rurge 自己的读法，没有在 Surge 上观察过），并写上 `[Proxy Group]` 里 `=` 左边的名字试过了（同 `[Proxy]`，带引号就带着引号被定义）；`http` / `socks5` 的位置凭据与旧 `custom` 行各补一句"含 `=` 时整个加引号"；附录的数字只在标记变了时重数。
- **阶段 1 验收文档**：FR-CFG-01 的"引号值与转义"与"含逗号的引号值"两行改指新的用例，写明这一轮改了什么；三个数不变。
- **CLAUDE.md**：状态一段加这一轮；「先读这些文档」加设计与计划；「常用命令」加新用例的过滤词；"接下来的工作"去掉这一件。
- **README / README_en**：状态一段。

## 10. 不在这一轮的

- 行内注释与行尾的要求指令认 `"` 的方式（LS-D13）。
- `[General]` 一类键值行整个取值带引号的不去引号（阶段 1 验收文档已登记：`loglevel = "notify"` 得 `W0012`）。
- `=` 左边定义的名字不去引号（LS-D11）。
- 引起来的空项照旧丢掉：`http, h, 80, "", pw` 把口令读成用户名（今天如此；改它要连同脱敏的字段数一起改）。
- 位置上不带引号、含 `=` 的口令在脱敏后的视图里照旧显示（6.4 节）。
- Surge 怎样读这些行没有观察过；手工验收清单不加项（这一轮的每一处在回环上、甚至不联网就验得了）。

## 11. 任务草图

每个任务结束时整个工作区是绿的；解析器与脱敏对"一项在哪里结束"的读法在同一个任务里改。

1. **共用的扫描与脱敏**（LS-D1 ～ D5、D9、D10）：`value.rs` 的 `item_spans` 与 `split_items`，`split_list` 与 `unquote` 改建在它上面；`policy::with_params`、`policy::leaves_open`、`spec::reader::written_as_name` 改用 `split_items`；`redact.rs` 的区域、两遍的次序、位置凭据的字段与区域，`hide_listed` 与 `hide_include_links` 按区域，`split_top_level` 去掉；属性测试里 `{name}=a"{V},{W}"b` 一种改成解析器现在读到的值；6.2 节的性质写成用例。
2. **每一项读成什么**（LS-D6 ～ D8）：`Field` 与 `split_fields`，`ParamMap` 不再去第二次引号、不再去值两头的空白，第 5 节列出的调用者改用它，`param_value` 只引一次，`is_name` 不认 `=`，`W0001` 在位置凭据处不点名并带提示；三条属性测试标出"解析器读成值"的五种写法；`secrets.rs` 按第 8 节改。
3. **端到端与文档**：`rurge check` 的用例；第 9 节的文档。

## 12. 实施期的订正

实施与评审中定下的、与上面几节文字不同的事（计划 `docs/superpowers/plans/2026-10-14-line-splitting-plan.md` 的「执行期修正记录」与「延后事项」有详情）：

1. 位置凭据那一遍不给区域并进来的字段计数：计不计数输出都一样（被遮的字段已在端口之后，其后每个字段都在端口之后）。6.1 节"并进来的非空字段计入字段数"以此为准。
2. 旧的读法（`next_comma`）在脱敏里不只用来量区域（6.1 节第一条）：`key_line` 判断 `=` 前面的文字是不是键时只用它，`redact_entry` 判断 `=` 前面有没有逗号时两种读法任何一种看到就算——6.1 节最后一条说的就是这两处。
3. LS-D9 代价一格"少遮的那一段仍按自己的字段号遮，除非它读起来是命名参数"不总成立。逐项那一遍把秘密值连同它收尾的引号换成 `***`，位置凭据那一遍随后读到的引号配对就变了：同一行前面某个值在值开始的地方开了引号而漏了收尾（服务器字段、`sni=` 一类参数或误打的一个引号），后面又有一个写法正确、含逗号、整个加引号的秘密值时，逗号之后那一段并进前面那个没收尾的值，留在脱敏后的视图里（`trojan, h.example, 443, sni="a.example, password="p1,p2", ws=true` 留下 `p2"`）；一行里三个以上多余的引号字符时另有几种。整分支终审的对照里，各插一处误打的 40 万行中这样少藏的 147 行、多藏的 47,429 行。接受，代码不在这一轮改（计划延后第 12 行，归项目所有者：收口的方向是在原文上按解析器的字段数决定位置凭据，再落到逐项输出里对应的区域上）。
4. 6.1 节"整个在引号里的 `"password=…"` 照旧只留 `password=***`"在读位置凭据的类型上露出了这个前缀：那里这样一项就是口令本身，以秘密参数名加 `=` 开头时（`"token=abc123"`）显示成 `token=***`（计划延后第 18 行）。
5. LS-D1 的代价还有一面：被切开的前半可能落进不是凭据的位置——`http-listen = ab"p1,p2"@127.0.0.1:6152` 的地址错误引用 `ab"p1`，`dns-server = https://u:"p1,p2"@…` 的 `Debug` 显示 `https://u:"p1`（计划延后第 19 行）。
6. 第 7 节"读法变了的"漏了两处：`external-policy-modifier` 遇到整个加引号的 `"k=v"` 时把它当作一个值留在原处、把修饰的参数追加在行尾（此前替换它），行尾留着没配对的左括号时追加的参数也写在行尾（只有没收尾的引号才让它写在最后一项之前）；单引号里没有转义，作为一行最后一项的 `password='it's'` 此前读成 `it's`、现在读成 `'it's'`（`'it'` 的伴之后还有字符，不是整个在引号里），这样写的口令升级之后会变，要写成 `password="it's"`。
7. 6.1 节两种逐项的情形（只有一项的区域、几项并成的区域的第一项是秘密参数）共用藏值的规则：`blanks_around`、`secret_key`、`hide_value` 三个私有函数。
8. 第 5 节"`spec::reader::written_as_name` 照旧读文字"：它另要求键本身是参数名的写法（`is_plain_name`）。整个加引号的 `key=value` 丢了后面的逗号、粘着下一个参数时，解析器读到的键里带着引号里的那一段（LS-D6 之后才有的情形），照文字读找不到这样一项，不能凭此点名。
9. LS-D8 的告警经 `ParamReader::warn_with_hint`（与组的读取器同形）。
10. 第 8 节之外改了一条现有用例：`a_load_gives_each_diagnostic_the_hint_of_its_code` 的未知参数挪到口令位置之后（LS-D8 的后果）。第 8 节端到端那一份 Profile 没有放旧的 `custom` 行（它会多一条 `I0001` 提示），旧 `custom` 行的口令由 `spec/ss.rs` 的用例钉住。
