# 一行怎样拆成各项 实施计划

> **执行方式**：子代理驱动（superpowers:subagent-driven-development）——每个任务一个新的实现者，做完由新的评审者评审，全部做完后整分支评审一次。本计划写到"改哪里、接口是什么、要哪些用例、判据是什么"为止：实现者先写失败的用例、看到红，再实现到绿；与计划的出入记进末尾的「执行期修正记录」。读法与脱敏这两处给出了完整的代码。

**目标**：一行照设计第 4 节的读法拆成各项——引号只在一个值开始的地方开启引用，括号只在配对时分组，整个在引号里的一项是值，引起来的值只去一次引号、原样保留；脱敏照设计第 6 节改看区域；读位置凭据的类型上，端口之后头两个位置的未知参数不点名、带一句提示（LS-D8）。

**做法**：三个任务。任务 1 建共用的扫描 `value::item_spans`，`split_list` 与 `unquote` 改建在它上面（`Field` 与 `split_fields` 在这里就出现，`split_list` 是它的投影），`with_params`、`written_as_name` 与脱敏改用它，脱敏改看区域、两遍的次序对调——解析器与脱敏对"一项在哪里结束"的读法在同一个提交里改；`ParamMap` 这个任务不动。任务 2 让读参数的调用者改用 `split_fields`：整个在引号里的一项是值，`ParamMap` 不再去第二次引号，`with_params` 只引一次，`is_name` 不认带 `=` 的文字，LS-D8 的告警。任务 3 是 `rurge check` 的端到端用例与文档。改动都在 `rurge-config`，另两个 crate 各改一处注释；不新增 crate、依赖与诊断码。

**依据**：设计 `docs/superpowers/specs/2026-10-14-line-splitting-design.md`（第 3 节 LS-D1 ～ D13、第 4 节读法、第 5 节形状、第 6 节脱敏、第 7 节使用者看得到的变化、第 8 节测试、第 9 节文档；下面写"设计"指它）。

## 全局约束

- 不新增 crate，`Cargo.lock` 不变（517 个包），没有新的依赖边。全工作区 `forbid(unsafe_code)`，`rurge-platform` 的两个例外不增加；MSRV 1.89，edition 2024。
- 不新增诊断码，也不新增告警（LS-D12）：LS-D8 只给已有的 `W0001` 换一句话、带一句提示。`codes::hint` 里每个码的决定不变（`every_code_is_decided_once` 不该变红）。
- 凭据不进日志、错误文本、诊断、`Debug` 与 API 输出；诊断不引用配置里的取值。`Field` 的 `Debug` 是 `Field(***)`（一项可以是口令）。新的带凭据的写法（位置上整个加引号、含 `=` 的口令）写进标记用例 `crates/rurge-config/tests/secrets.rs`（任务 2）。
- 解析器与脱敏对"一项在哪里结束"的读法在同一个提交里改（任务 1）。任务 2 改"每一项读成什么"时，对着 `redact.rs` 的三条属性用例核对（计划点名了要改的地方）。
- 脱敏后的视图是一张"藏什么"的清单，尽力而为（M8c 计划「延后事项」第 51 ～ 77 行的规矩）：不追更多的拼法；设计 6.4 留着的照旧留着。
- 测试只用回环地址与端口 0，等待都有时限，不用固定的 sleep 当同步；不联网。这一轮唯一起二进制的是任务 3 的两条 `rurge check` 用例：经 `crates/rurge/tests/cli.rs` 的 `rurge_command()`，给 `--data-dir`（用例自己的临时目录），离线。
- "改一行代码看用例变红"的验证只在 `rurge-config` 的纯函数上做；每一次先写下这一改让哪些会话、查找、下载与文件去哪里（这一轮的都是：哪里也不去）。
- 两条长行计时用例提交前单独循环跑二十次以上。
- 不改系统设置（不运行 `rurge run --system-proxy`，`rurge service install` / `uninstall` 只带 `--dry-run`）；不下载、不安装任何东西；只运行本仓库构建出来的二进制。
- Surge 语法就是 rurge 语法；与 Surge 的每一处行为差异都进兼容性清单（任务 3）。手册的文字不进仓库，除了设计已经引用的句子。
- 注释、日志与错误文本用英文，照所在文件的写法；提交说明的标题用中文，说改了什么、为什么。
- 先写失败的用例、看到红，再实现到绿。现有用例变红时：计划点了名的照计划改，并在提交说明里说；没点名的停下来报告，不顺手改期望值。
- 门禁（每个任务末尾一次）：`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test -j 8 --workspace --no-fail-fast`（本机 `RUSTFMT` 指向 gnu 工具链的 rustfmt）。本机上聚焦地跑用例也经整个工作区（`cargo test -j 8 --workspace --lib -- <过滤词>`，或 `--test <目标> -- <过滤词>`），不用 `-p`（它会另编一份依赖）。测试二进制以 `STATUS_HEAP_CORRUPTION` 退出时重跑那一个（开发机的管控软件，不是 rurge 的缺陷）。跑全量之前 `df -h /d`，剩余不足约 15 GiB 就停下。
- 语料库的七份快照预期不变（它们的配置里没有撇号、不配对的括号与整个加引号的 `key=value`）。本机没有 `cargo insta`：快照变了时用 `INSTA_UPDATE=always cargo test -j 8 --workspace --test corpus` 写出，`git diff` 逐行看，能用设计第 7 节解释的才留下，否则停下来报告。

## 评审要点

计划的用例之外最可能咬到使用者的五种情形，各自归到拥有那段代码的任务里：

1. 名字里有撇号或不配对括号的策略行带着位置上的口令（`X = http, Bob's-host, 80, alice, pw`）：口令照样遮掉——今天脱敏数不到第 4 个字段，一个字也不遮（任务 1，`positional_credentials_are_counted_as_the_parser_counts_its_fields`）。
2. 别的工具写出来的整个加引号的 `key=value`（`"policy-path=…"`、`"password=…"`）：加载结果照设计第 7 节变（组行上是 `E0008`），诊断与 `Debug` 不引用它，脱敏照旧遮掉（任务 2，`secrets.rs` 的新用例）。
3. 值中间成对的引号原来护住的逗号（`password=ab"c,d"`）：解析器切开了，脱敏的输出与今天逐字相同（任务 1，`a_value_ends_at_the_first_top_level_comma_wherever_its_quotes_open` 不变）。
4. 一行几十万个同一种字符（`(`、`)`、`"`、`'`、`(,`、`'a,`、`a=`、`a"(`、`a"=`）：读与脱敏的时间随长度线性增长，不是平方（任务 1，两条长行用例）。
5. `with_params` 写回去再读出来逐字节相同（含引号、反斜杠、两头空白、逗号）：两个任务都让 `a_value_set_by_with_params_reads_back_unchanged` 保持绿。

---

## 文件结构

| 文件 | 任务 | 职责 |
| --- | --- | --- |
| `crates/rurge-config/src/value.rs` | 1、2 | 扫描 `item_spans`、`split_items`、`Field` / `split_fields`、`split_list`、`unquote`（1）；`ParamMap` 不再去第二次引号、`from_fields` 收 `&[Field]`（2） |
| `crates/rurge-config/src/redact.rs` | 1、2 | 区域、两遍的次序、位置凭据的字段与区域、`hide_listed` 与 `hide_include_links` 按区域（1）；属性用例标出"解析器读成值"的写法（2） |
| `crates/rurge-config/src/policy.rs` | 1、2 | `with_params` / `leaves_open` 改用 `split_items`（1）；`parse_policy`、`parse_group`、`with_params` 认键改用 `split_fields`，`param_value` 只引一次，`is_name` 不认 `=`（2） |
| `crates/rurge-config/src/spec/reader.rs` | 1、2 | `written_as_name` 改用 `split_items`（1）；LS-D8（2） |
| `crates/rurge-config/src/rule.rs` | 1、2 | 用例（1）；`parse_rule` 与 `RuleParams::parse` 改用 `split_fields`（2） |
| `crates/rurge-config/src/text/include.rs` | 1 | 用例 |
| `crates/rurge-config/src/{keystore.rs,wireguard.rs,spec/group.rs}` | 2 | 改用 `split_fields` |
| `crates/rurge-config/src/{config.rs,diagnostic.rs,spec/ss.rs,spec/mod.rs}` | 2 | 用例；`codes::hint` 的文档加一句 |
| `crates/rurge-config/tests/{secrets.rs,policy_spec.rs}` | 2 | 标记用例；一条用例的写法 |
| `crates/rurge-engine/src/views.rs` | 1 | 一处注释 |
| `crates/rurge-policy/src/assemble.rs` | 2 | 一处注释 |
| `crates/rurge/tests/cli.rs` | 3 | `rurge check` 的端到端用例 |
| 文档 | 3 | 兼容性清单、阶段 1 验收文档、README（中英）、CLAUDE.md、手册基线重对计划的「延后事项」 |

---

## Task 1：共用的扫描与脱敏（LS-D1 ～ D5、D9、D10）

**文件**：`crates/rurge-config/src/{value.rs,redact.rs,policy.rs,spec/reader.rs,rule.rs,text/include.rs}`、`crates/rurge-engine/src/views.rs`（注释）。

**接口**：

```rust
// rurge-config::value
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ItemSpan { pub start: usize, pub end: usize, pub sep_end: usize, pub eq: Option<usize> }
pub(crate) fn item_spans(text: &str, wide: bool) -> Vec<ItemSpan>;
pub(crate) fn split_items(text: &str) -> Vec<&str>;
#[derive(Clone, PartialEq, Eq)]           // Debug 手写：Field(***)
pub struct Field { pub text: String, pub param: Option<(String, String)> }
pub fn split_fields(text: &str) -> Vec<Field>;
pub fn split_list(text: &str) -> Vec<String>;   // 签名不变：每一项的 `text`
pub fn unquote(text: &str) -> String;           // 签名不变：只去"整个在引号里"的引号，按伴判断
#[cfg(test)] pub(crate) fn short_strings() -> impl Iterator<Item = String>;

// rurge-config::redact（都是私有的）
struct Region { start: usize, first_end: usize, end: usize, sep_end: usize }
fn regions(text: &str) -> Vec<Region>;
fn redact_joined(region: &str, first_len: usize) -> String;
struct FieldSpan { start: usize, end: usize, sep_end: usize, word: bool }

// 去掉：value::unquote_field（只有旧的 split_list 用它）、redact::split_top_level
```

`ParamMap` 这个任务不动：`insert` 照旧再去一次引号，`from_fields` 照旧收 `&[String]`（任务 2 改）。

- [ ] **Step 1：先写用例，看它们红**

分两步写：用到新接口（`item_spans`、`ItemSpan`、`split_fields`、`short_strings`、`regions`）的用例会让 `rurge-config` 的整个库测试目标编译不过，那时别的用例也跑不起来。所以**先**写只用现有接口的那些——`unquote_cases` 的三句、`rule.rs`、`text/include.rs`、`policy.rs` 的两处、`redact.rs` 的 `positional_credentials_are_counted_as_the_parser_counts_its_fields`、长行用例加的种类、Step 5 里属性用例 `{name}=a"{V},{W}"b` 那一种写法的值——跑一遍，看它们以断言失败（长行用例加的种类今天就该是绿的，是守护）；**再**写 `value.rs` 的三条新用例与 `redact.rs` 的 `a_region_ends_where_the_later_of_the_two_readings_ends`，它们在实现之前编译不过，这就是它们的红。

`value.rs` 的测试模块（`split_list_cases` 与 `definitions_and_params` 不改，照旧要绿）：

`unquote_cases` 加三句（今天是红的：今天的 `unquote` 只看两头是不是引号）：

```rust
        assert_eq!(unquote("\"P1\" \"x\""), "\"P1\" \"x\"");
        assert_eq!(unquote("'it's'"), "'it's'");
        assert_eq!(unquote("\"\\\""), "\"\\\"");
```

读法表（设计 4.3 的每一行，连同 4.2 的边角），一行同时比 `split_fields` 与 `split_list`：

```rust
    type Item = (String, Option<(String, String)>);

    fn value(text: &str) -> Item {
        (text.to_string(), None)
    }

    fn param(text: &str, key: &str, val: &str) -> Item {
        (text.to_string(), Some((key.to_string(), val.to_string())))
    }

    fn read(text: &str) -> Vec<Item> {
        split_fields(text).into_iter().map(|f| (f.text, f.param)).collect()
    }

    /// Design 4.3, row by row, and the corners of 4.2: where a quote opens,
    /// which parentheses group, what an item reads as, and what `split_list`
    /// gives for it.
    #[test]
    fn a_quote_opens_only_where_a_value_starts_and_a_parenthesis_groups_only_in_a_pair() {
        let peer = "peer = (public-key = k, allowed-ips = \"10.0.0.0/8, 192.168.0.0/16\", endpoint = a:1)";
        let cases: Vec<(&str, Vec<Item>)> = vec![
            ("select, Tom's HK, Tom's US", vec![value("select"), value("Tom's HK"), value("Tom's US")]),
            ("USER-AGENT,Bob's App*,DIRECT", vec![value("USER-AGENT"), value("Bob's App*"), value("DIRECT")]),
            (
                "DOMAIN,b.example,Proxy (US,extended-matching",
                vec![value("DOMAIN"), value("b.example"), value("Proxy (US"), value("extended-matching")],
            ),
            (
                r"URL-REGEX,^http://a\.example/[^(]*$,REJECT",
                vec![value("URL-REGEX"), value(r"^http://a\.example/[^(]*$"), value("REJECT")],
            ),
            (
                "AND,((DOMAIN,a.example),(DEST-PORT,443)),Proxy (US)",
                vec![value("AND"), value("((DOMAIN,a.example),(DEST-PORT,443))"), value("Proxy (US)")],
            ),
            (
                r#"URL-REGEX,"^http://example\.com/(a|b),?c",Proxy"#,
                vec![value("URL-REGEX"), value(r"^http://example\.com/(a|b),?c"), value("Proxy")],
            ),
            ("password=\"p,w\"", vec![param("password=p,w", "password", "p,w")]),
            ("x='a,b'", vec![param("x=a,b", "x", "a,b")]),
            ("password=ab\"c,d\"", vec![param("password=ab\"c", "password", "ab\"c"), value("d\"")]),
            ("a=b=\"c,d\"", vec![param("a=b=\"c", "a", "b=\"c"), value("d\"")]),
            ("\"pa=ss\"", vec![value("pa=ss")]),
            (r#"sni="\"a.example\"""#, vec![param("sni=\"a.example\"", "sni", "\"a.example\"")]),
            ("password=\" pw \"", vec![param("password= pw ", "password", " pw ")]),
            ("\"P1\" \"x\"", vec![value("\"P1\" \"x\"")]),
            ("\"Fast\" Proxy", vec![value("\"Fast\" Proxy")]),
            ("P1, \"P1", vec![value("P1"), value("\"P1")]),
            ("Proxy (\"US, Other", vec![value("Proxy (\"US, Other")]),
            ("(USER-AGENT,\"a)b\")", vec![value("(USER-AGENT,\"a)b\")")]),
            (
                peer,
                vec![param(
                    peer,
                    "peer",
                    "(public-key = k, allowed-ips = \"10.0.0.0/8, 192.168.0.0/16\", endpoint = a:1)",
                )],
            ),
            ("=(a)=b", vec![value("=(a)=b")]),
            ("(a=b)", vec![value("(a=b)")]),
            ("(a=b)=c", vec![value("(a=b)=c")]),
            ("a(b)=c", vec![param("a(b)=c", "a(b)", "c")]),
            ("(tfo=false", vec![param("(tfo=false", "(tfo", "false")]),
            ("peer = (a = b)", vec![param("peer = (a = b)", "peer", "(a = b)")]),
            ("a = b", vec![param("a = b", "a", "b")]),
            ("\"a\"=b", vec![param("\"a\"=b", "\"a\"", "b")]),
            ("\"\"", vec![]),
            ("''", vec![]),
            ("\" \"", vec![value(" ")]),
            (", ,", vec![]),
        ];
        for (input, expected) in cases {
            assert_eq!(read(input), expected, "{input:?}");
            let texts: Vec<String> = expected.into_iter().map(|(text, _)| text).collect();
            assert_eq!(split_list(input), texts, "{input:?}");
        }
    }
```

设计 6.2 的第一条性质（穷举，P7）：

```rust
    fn shift(spans: Vec<ItemSpan>, by: usize) -> Vec<ItemSpan> {
        spans
            .into_iter()
            .map(|s| ItemSpan {
                start: s.start + by,
                end: s.end + by,
                sep_end: s.sep_end + by,
                eq: s.eq.map(|e| e + by),
            })
            .collect()
    }

    /// Design 6.2: read again from any cut, the rest of a text is cut where it
    /// was, and an item read alone is that item — what redaction and
    /// `with_params` rely on when they take an item out and read it again.
    /// Every string of up to six characters the reading looks at.
    #[test]
    fn the_reading_is_the_same_from_any_cut_and_of_any_item_alone() {
        let mut checked = 0usize;
        for text in short_strings() {
            let spans = item_spans(&text, false);
            for (k, span) in spans.iter().enumerate() {
                if k + 1 < spans.len() {
                    assert_eq!(
                        shift(item_spans(&text[span.sep_end..], false), span.sep_end).as_slice(),
                        &spans[k + 1..],
                        "{text:?}"
                    );
                }
                assert_eq!(
                    shift(item_spans(&text[span.start..span.end], false), span.start),
                    [ItemSpan { sep_end: span.end, ..*span }],
                    "{text:?}"
                );
            }
            checked += 1;
        }
        assert!(checked > 500_000, "{checked}");
    }
```

长行（设计第 8 节；平方的写法在这里卡住，线性的在 debug 构建下也是毫秒级，所以界限放得宽，不受机器快慢左右）：

```rust
    /// A line of one kind of character, three hundred thousand of them, is
    /// read in time that grows with its length: a reading that starts over
    /// after every cut, or looks ahead from every parenthesis, would take
    /// minutes.
    #[test]
    fn a_long_line_of_one_kind_is_read_in_time_that_grows_with_its_length() {
        let n = 300_000usize;
        let lines: Vec<(&str, String)> = vec![
            ("open parentheses", "(".repeat(n)),
            ("close parentheses", ")".repeat(n)),
            ("double quotes", "\"".repeat(n)),
            ("single quotes", "'".repeat(n)),
            ("open parentheses and commas", "(,".repeat(n / 2)),
            ("apostrophes and commas", "'a,".repeat(n / 3)),
            ("first equals signs", "a=".repeat(n / 2)),
            ("quotes before parentheses", "a\"(".repeat(n / 3)),
            ("quotes before equals signs", "a\"=".repeat(n / 3)),
        ];
        for (kind, line) in &lines {
            let started = std::time::Instant::now();
            let fields = split_fields(line);
            let took = started.elapsed();
            std::hint::black_box(fields);
            assert!(
                took < std::time::Duration::from_secs(10),
                "{kind} ({} bytes) took {took:?}",
                line.len()
            );
        }
    }
```

`rule.rs` 的测试模块（今天前三条是红的，第四条是今天就绿的守护）：

```rust
    /// An apostrophe or a parenthesis that pairs with nothing is an ordinary
    /// character (line-splitting design LS-D1, LS-D3); a quote at the start
    /// of a sub-rule's value still guards a `)` and a comma.
    #[test]
    fn an_apostrophe_or_an_unpaired_parenthesis_does_not_take_the_rest_of_the_line() {
        let ua = parse("USER-AGENT,Bob's App*,DIRECT").unwrap();
        assert!(matches!(&ua.kind, RuleKind::UserAgent(g) if g.source() == "Bob's App*"), "{ua}");
        assert_eq!(ua.policy, PolicyRef::Builtin(Builtin::Direct), "{ua}");
        let regex = parse(r"URL-REGEX,^http://a\.example/[^(]*$,REJECT").unwrap();
        assert!(
            matches!(&regex.kind, RuleKind::UrlRegex(p) if p.source == r"^http://a\.example/[^(]*$"),
            "{regex}"
        );
        assert_eq!(regex.policy, PolicyRef::Builtin(Builtin::Reject), "{regex}");
        let paren = parse("DOMAIN,b.example,Proxy (US,extended-matching").unwrap();
        assert_eq!(paren.policy, PolicyRef::Named("Proxy (US".into()), "{paren}");
        assert!(paren.params.extended_matching && paren.params.unknown.is_empty(), "{paren}");
        let and = parse(r#"AND,((USER-AGENT,"a)b,c"),(DEST-PORT,443)),DIRECT"#).unwrap();
        assert!(
            matches!(&and.kind, RuleKind::And(subs) if matches!(&subs[0].kind, RuleKind::UserAgent(g) if g.source() == "a)b,c")),
            "{and}"
        );
    }
```

`text/include.rs` 的测试模块（照 `single_multiple_and_mixed_includes` 的写法；今天是红的）：

```rust
    /// A target with an apostrophe in its name is one target: the next one
    /// is read too (line-splitting design LS-D1).
    #[test]
    fn a_target_with_an_apostrophe_in_its_name_is_read_with_the_next_one() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("main.conf"),
            "[Rule]\n#!include Tom's rules.dconf, b.dconf\nFINAL,DIRECT\n",
        )
        .unwrap();
        fs::write(dir.path().join("Tom's rules.dconf"), "[Rule]\nDOMAIN,a,DIRECT\n").unwrap();
        fs::write(dir.path().join("b.dconf"), "[Rule]\nDOMAIN,b,DIRECT\n").unwrap();
        let (p, d) = load(dir.path(), "main.conf");
        assert!(d.is_empty(), "{:?}", d.into_vec());
        let rules: Vec<_> = p
            .section("Rule")
            .unwrap()
            .entries
            .iter()
            .map(|e| e.raw.as_str())
            .collect();
        assert_eq!(rules, ["DOMAIN,a,DIRECT", "DOMAIN,b,DIRECT", "FINAL,DIRECT"]);
    }
```

`policy.rs` 的测试模块：`with_params_overrides_in_place_and_appends_the_rest` 加一句（今天得到的是 `…, user, tfo=true, Bob's pw, tfo=false`：撇号开启的引用吞掉了后面的 `tfo=false`），再加一条用例（今天是红的，两个成员被读成一个）：

```rust
        assert_eq!(
            with_params("http, h.test, 80, user, Bob's pw, tfo=false", &pairs(&[("tfo", "true")])),
            "http, h.test, 80, user, Bob's pw, tfo=true"
        );
```

```rust
    /// An apostrophe or a parenthesis that pairs with nothing is part of a
    /// member's name (line-splitting design LS-D1, LS-D3).
    #[test]
    fn a_member_with_an_apostrophe_or_an_unpaired_parenthesis_is_one_member() {
        let g = parse_group("G", "select, Tom's HK, Tom's US, Proxy (US, DIRECT", &span()).unwrap();
        assert_eq!(g.members, ["Tom's HK", "Tom's US", "Proxy (US", "DIRECT"]);
    }
```

`redact.rs` 的测试模块（第一条的前两行今天是红的，第三行是守护；第二条在 `regions` 写出来之前编译不过）：

```rust
    /// The positional values are counted the way the parser counts its
    /// fields (line-splitting design 6.3): an apostrophe or a parenthesis
    /// that pairs with nothing in a server's name no longer hides the
    /// password behind it, and a group's members stay.
    #[test]
    fn positional_credentials_are_counted_as_the_parser_counts_its_fields() {
        for (line, shown) in [
            ("X = http, Bob's-host, 80, alice, pw", "X = http, Bob's-host, 80, ***, ***"),
            ("X = http, Proxy (US-host, 80, alice, pw", "X = http, Proxy (US-host, 80, ***, ***"),
            (
                "G = select, Tom's HK, policy-path=https://sub.example/list?token=t0k3n",
                "G = select, Tom's HK, policy-path=***",
            ),
        ] {
            assert_eq!(redact_profile(line), shown, "{line:?}");
        }
    }

    /// Design 6.2: a region starts with an item of the parser's, ends where
    /// the later of the two readings ends — moved on to the parser's next
    /// cut — and the regions cover the text. Every string of up to six
    /// characters the reading looks at.
    #[test]
    fn a_region_ends_where_the_later_of_the_two_readings_ends() {
        let mut checked = 0usize;
        for text in crate::value::short_strings() {
            let spans = item_spans(&text, false);
            let mut at = 0;
            for r in regions(&text) {
                assert_eq!(r.start, at, "{text:?}");
                let first = spans
                    .iter()
                    .position(|s| s.start == r.start)
                    .unwrap_or_else(|| panic!("{text:?}"));
                assert_eq!(spans[first].end, r.first_end, "{text:?}");
                let last = spans
                    .iter()
                    .position(|s| (s.end, s.sep_end) == (r.end, r.sep_end))
                    .unwrap_or_else(|| panic!("{text:?}"));
                let old = next_comma(&text[r.start..], false).map_or(text.len(), |(c, _)| r.start + c);
                assert!(r.end >= r.first_end && r.end >= old, "{text:?}");
                assert!(spans[first..last].iter().all(|s| s.end < old), "{text:?}");
                at = r.sep_end;
            }
            assert_eq!(at, text.len(), "{text:?}");
            checked += 1;
        }
        assert!(checked > 500_000, "{checked}");
    }
```

`a_long_line_of_one_kind_is_redacted_in_time_that_grows_with_its_length`（`redact.rs`，`n = 100_000`）的种类表里加这几种（原有的不动，用例的其余部分不动）：

```rust
            ("open parentheses", "(".repeat(n)),
            ("close parentheses", ")".repeat(n)),
            ("open parentheses and commas", "(,".repeat(n / 2)),
            ("apostrophes and commas", "'a,".repeat(n / 3)),
            ("first equals signs", "a=".repeat(n / 2)),
            ("quotes before parentheses", "a\"(".repeat(n / 3)),
            ("quotes before equals signs", "a\"=".repeat(n / 3)),
            ("names with apostrophes", "Tom's, ".repeat(n / 7)),
```

先写的那一批跑一遍，看它们以断言失败：

```bash
cargo test -j 8 --workspace --lib -- value::tests::unquote_cases rule::tests::an_apostrophe text::include::tests::a_target_with_an_apostrophe policy::tests::a_member_with policy::tests::with_params_overrides redact::tests::positional_credentials_are_counted redact::tests::whatever_the_parser_reads
```

然后写需要新接口的那一批，`cargo test -j 8 --workspace --lib --no-run` 编译不过（缺 `item_spans` 等），这是它们的红。

- [ ] **Step 2：`value.rs`**

文件开头的 `split_list`、`unquote`、`unquote_field`（今天的第 1 ～ 91 行，到 `starts_with_ci` 之前）整个换成下面这些；`starts_with_ci` 往后不动：

```rust
//! Value-level helpers shared by all section parsers.
//!
//! How a list is cut into items (the line-splitting design, 4.1): a quote,
//! `"` or `'`, opens only where a value starts — at the start of the text,
//! after a comma or a `(` outside quotes, or after the first `=` since the
//! last of those, white space between allowed; inside `"` a backslash takes
//! the next character with it, inside `'` nothing escapes, and a quote with
//! no partner runs to the end. A quote anywhere else is a character. `(` and
//! `)` outside quotes group only when they pair up; a parenthesis that pairs
//! with nothing is a character. The items are cut at the commas outside
//! quotes and outside groups.

/// What the first pass of `item_spans` notes outside quotes.
#[derive(Clone, Copy)]
enum Mark {
    /// A comma that may cut: where it stands and how wide it is.
    Comma(usize, usize),
    Open,
    Close,
    /// The first `=` since a value last started.
    Eq(usize),
}

/// One item of a list: where it starts, where it ends, where the separator
/// after it ends, and where its first `=` that starts a value outside any
/// group stands (design 4.2; with nothing in front of it the item is still a
/// value).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ItemSpan {
    pub start: usize,
    pub end: usize,
    pub sep_end: usize,
    pub eq: Option<usize>,
}

/// The items of `text` (design 4.1), in two passes over it: the first finds
/// the quotes and notes the commas, parentheses and value-starting `=` outside
/// them; the second pairs the parentheses on a stack and reads the cuts off
/// the notes. Every item is there, an empty one too. When `wide`, the
/// full-width comma an input method types cuts as well (design LS-D10); it
/// starts no value.
pub(crate) fn item_spans(text: &str, wide: bool) -> Vec<ItemSpan> {
    let mut marks = Vec::new();
    let mut at_start = true;
    let mut seen_eq = false;
    let mut quote: Option<char> = None;
    let mut chars = text.char_indices();
    while let Some((i, c)) = chars.next() {
        if let Some(q) = quote {
            if q == '"' && c == '\\' {
                chars.next();
            } else if c == q {
                quote = None;
            }
            continue;
        }
        if at_start && c.is_whitespace() {
            continue;
        }
        if at_start && matches!(c, '"' | '\'') {
            quote = Some(c);
            at_start = false;
            continue;
        }
        at_start = false;
        match c {
            ',' => {
                marks.push(Mark::Comma(i, 1));
                at_start = true;
                seen_eq = false;
            }
            '，' if wide => marks.push(Mark::Comma(i, '，'.len_utf8())),
            '(' => {
                marks.push(Mark::Open);
                at_start = true;
                seen_eq = false;
            }
            ')' => marks.push(Mark::Close),
            '=' if !seen_eq => {
                marks.push(Mark::Eq(i));
                seen_eq = true;
                at_start = true;
            }
            _ => {}
        }
    }
    let mut paired = vec![false; marks.len()];
    let mut open = Vec::new();
    for (k, mark) in marks.iter().enumerate() {
        match mark {
            Mark::Open => open.push(k),
            Mark::Close => {
                if let Some(o) = open.pop() {
                    paired[o] = true;
                    paired[k] = true;
                }
            }
            _ => {}
        }
    }
    let mut spans = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut eq = None;
    for (k, mark) in marks.iter().enumerate() {
        match *mark {
            Mark::Open if paired[k] => depth += 1,
            Mark::Close if paired[k] => depth -= 1,
            Mark::Eq(at) if depth == 0 && eq.is_none() => eq = Some(at),
            Mark::Comma(at, width) if depth == 0 => {
                spans.push(ItemSpan {
                    start,
                    end: at,
                    sep_end: at + width,
                    eq: eq.take(),
                });
                start = at + width;
            }
            _ => {}
        }
    }
    spans.push(ItemSpan {
        start,
        end: text.len(),
        sep_end: text.len(),
        eq,
    });
    spans
}

/// The items of `text` as they stand in it, blanks around them included
/// (design 4.1).
pub(crate) fn split_items(text: &str) -> Vec<&str> {
    item_spans(text, false)
        .into_iter()
        .map(|span| &text[span.start..span.end])
        .collect()
}

/// One item of a list as every parser reads it (design 4.2).
#[derive(Clone, PartialEq, Eq)]
pub struct Field {
    /// What `split_list` gives for the item.
    pub text: String,
    /// The parameter the item is: its key as written, and its value with its
    /// quotes taken off once. `None` for a value.
    pub param: Option<(String, String)>,
}

/// An item can be a password: nothing of it is shown.
impl std::fmt::Debug for Field {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Field(***)")
    }
}

/// The items of a comma-separated list, each read as a value or a parameter
/// (design 4.2). An item that is blank, or wholly in quotes with nothing
/// inside them, is dropped.
pub fn split_fields(text: &str) -> Vec<Field> {
    item_spans(text, false)
        .into_iter()
        .filter_map(|span| {
            read_field(
                &text[span.start..span.end],
                span.eq.map(|at| at - span.start),
            )
        })
        .collect()
}

/// Split a comma-separated list into its items (design 4.1): each is trimmed,
/// one wholly in quotes loses them, a parameter whose value is wholly in
/// quotes is `key=value` with them off; empty items are dropped.
pub fn split_list(text: &str) -> Vec<String> {
    split_fields(text)
        .into_iter()
        .map(|field| field.text)
        .collect()
}

/// `item`, one item of a list, as a value or a parameter; `eq` is where its
/// first `=` that starts a value outside any group stands, if it has one.
fn read_field(item: &str, eq: Option<usize>) -> Option<Field> {
    let trimmed = item.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(inner) = wholly_quoted(trimmed) {
        return (!inner.is_empty()).then_some(Field {
            text: inner,
            param: None,
        });
    }
    if let Some(at) = eq {
        let key = item[..at].trim();
        if !key.is_empty() {
            let raw = item[at + 1..].trim();
            let (value, text) = match wholly_quoted(raw) {
                Some(value) => {
                    let text = format!("{key}={value}");
                    (value, text)
                }
                None => (raw.to_string(), trimmed.to_string()),
            };
            return Some(Field {
                text,
                param: Some((key.to_string(), value)),
            });
        }
    }
    Some(Field {
        text: trimmed.to_string(),
        param: None,
    })
}

/// The text inside the quotes when `text` is wholly in them: it opens with a
/// quote whose partner is its last character (design LS-D4). Inside `"`, `\"`
/// is `"` and `\\` is `\`; any other backslash stays.
fn wholly_quoted(text: &str) -> Option<String> {
    let quote = text.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    if partner(text, quote)? != text.len() - 1 {
        return None;
    }
    let inner = &text[1..text.len() - 1];
    Some(if quote == '"' {
        unescape(inner)
    } else {
        inner.to_string()
    })
}

/// Where the partner of the quote that opens `text` stands: the next `quote`
/// after it, one escaped by a backslash not counted inside `"`.
fn partner(text: &str, quote: char) -> Option<usize> {
    let mut chars = text.char_indices().skip(1);
    while let Some((i, c)) = chars.next() {
        if quote == '"' && c == '\\' {
            chars.next();
        } else if c == quote {
            return Some(i);
        }
    }
    None
}

/// Strip the quotes of a text that is wholly in them (design LS-D4); any
/// other text is returned as it is. Double quotes support `\"` and `\\`
/// escapes.
pub fn unquote(text: &str) -> String {
    wholly_quoted(text).unwrap_or_else(|| text.to_string())
}

/// The inside of a double-quoted value: `\"` is `"`, `\\` is `\`, any other
/// backslash stays with what follows it, and one at the end stays.
fn unescape(inner: &str) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Every string of up to six characters over the characters the reading
/// looks at — 597,870 of them — for the tests that compare readings.
#[cfg(test)]
pub(crate) fn short_strings() -> impl Iterator<Item = String> {
    const ALPHABET: [char; 9] = ['a', ',', '"', '\'', '(', ')', '=', '\\', ' '];
    (1..=6u32).flat_map(|len| {
        (0..ALPHABET.len().pow(len)).map(move |mut n| {
            (0..len)
                .map(|_| {
                    let c = ALPHABET[n % ALPHABET.len()];
                    n /= ALPHABET.len();
                    c
                })
                .collect::<String>()
        })
    })
}
```

- [ ] **Step 3：`policy.rs` 与 `spec/reader.rs` 改用 `split_items`**

`policy.rs`：
- `with_params` 里 `let items = crate::redact::split_top_level(definition);` 改成 `let items = crate::value::split_items(definition);`。文档注释的最后一段改成："The line then reads as it did alone, the overrides in place. What is appended therefore stands in front of a last item that leaves a quote open: the parser gives such an item the rest of the line (a parenthesis that pairs with nothing groups nothing, design LS-D3)."
- `leaves_open` 改成 `crate::value::split_items(&format!("{item},")).len() == 1`，文档注释改成："Whether `item` — one item of a line — ends inside a quote: a comma after it would not end it. Only a quote can be left open; a parenthesis that pairs with nothing groups nothing (line-splitting design LS-D3)."
- 测试模块：`a_modified_line…` 那条用例的文档注释里 "gives an unclosed quote or group the rest of the line" 改成 "gives an unclosed quote the rest of the line (a parenthesis that pairs with nothing groups nothing, design LS-D3)"；它的 `ITEMS` 里注释 `// an unclosed group` 改成 `// a parenthesis left open, which groups nothing (design LS-D3)`。断言不改（P29）。

`spec/reader.rs`：`written_as_name` 遍历 `crate::value::split_items(definition)`（原来是 `split_top_level`）；引入从 `use crate::redact::split_top_level;` 与 `use crate::value::{parse_bool, parse_key_value, split_list};` 改成 `use crate::value::{parse_bool, parse_key_value, split_items, split_list};`。

- [ ] **Step 4：`redact.rs`**

引入改成 `use crate::value::{item_spans, parse_key_value, split_list};`。

`redact_entry` 里判断第一个 `=` 前面有没有逗号的那一句（今天是 `if next_comma(key, true).is_some() {`）改成：

```rust
    // a comma in front of the `=`, by the parsers' reading or by today's
    // (`next_comma`, which also takes a quote in the middle of a value for an
    // opening one): redacting the line both ways only hides more
    if next_comma(key, true).is_some() || item_spans(key, true).len() > 1 {
        return redact_both_ways(line);
    }
```

`key_line` 里同样的那一句（`if next_comma(key, true).is_some() {`）不改，上面的注释后面加一句："It keeps today's reading only (line-splitting design 6.1): with the parsers' reading as well, `a'b, http-listen = …` would no longer be a key line, and its password would stay where it is."

`redact_definition`、`redact_items`、`hide_listed`、`hide_include_links`、`redact_positional_credentials`、`words_of_type_field` 换成下面的样子，新加 `Region`、`regions`、`redact_joined`、`FieldSpan`；`split_top_level` 删掉；`next_comma`、`next_top_level_comma`、`param_value_len`、`redact_item`、`group_inside`、`is_param_name`、`is_named_param`、`takes_server_type` 的代码不动：

```rust
/// A policy definition (the text right of `name =`) with its secrets blanked:
/// every secret `name=value` parameter, however the line spells it, and the
/// positional values of every type that takes a server and a port. The items
/// go first, on the text as written, and the positional values then on what
/// that made of it (design LS-D9).
pub fn redact_definition(definition: &str) -> String {
    hide_url_credentials(&redact_positional_credentials(&redact_items(definition, 0)))
}

/// One region of a list: where it starts, where its first item ends, where
/// its last item ends and where the separator after that one ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Region {
    start: usize,
    first_end: usize,
    end: usize,
    sep_end: usize,
}

/// Where the regions of `text` are (design LS-D9): a region is an item of the
/// parsers' (`value::item_spans`) together with the items that follow it as
/// long as today's reading, from the item's start (`next_comma`), runs past
/// their end — a quote or a parenthesis in the middle of a value guarded a
/// comma, and its writer may have meant it to. A region ends where the later
/// of the two readings ends, moved on to the parsers' next cut; the next
/// region starts with the item after it. Every scan stops at its own region,
/// so a text is read twice at most.
fn regions(text: &str) -> Vec<Region> {
    let spans = item_spans(text, false);
    let mut out = Vec::new();
    let mut i = 0;
    while i < spans.len() {
        let start = spans[i].start;
        let reach = next_comma(&text[start..], false).map_or(text.len(), |(at, _)| start + at);
        let mut j = i;
        while j + 1 < spans.len() && spans[j].end < reach {
            j += 1;
        }
        out.push(Region {
            start,
            first_end: spans[i].end,
            end: spans[j].end,
            sep_end: spans[j].sep_end,
        });
        i = j + 1;
    }
    out
}

/// `text`, a list of items, with every item redacted. The items are the
/// parsers' own (`value::item_spans`); an item whose value today's reading
/// (`next_comma`) carries past the item's end is redacted together with the
/// items it carries along — a region (design LS-D9). The separators between
/// the regions stay.
fn redact_items(text: &str, depth: usize) -> String {
    let mut out = String::with_capacity(text.len());
    for r in regions(text) {
        let body = &text[r.start..r.end];
        if r.first_end == r.end {
            out.push_str(&redact_item(body, depth));
        } else {
            out.push_str(&redact_joined(body, r.first_end - r.start));
        }
        out.push_str(&text[r.end..r.sep_end]);
    }
    out
}

/// A region of several items (design LS-D9): when the first is a secret
/// parameter, its value is hidden together with the items the region takes
/// along; otherwise every secret it names is searched for by name across the
/// whole region.
fn redact_joined(region: &str, first_len: usize) -> String {
    let rest = region.trim_start();
    let lead = &region[..region.len() - rest.len()];
    let body = rest.trim_end();
    let trail = &rest[body.len()..];
    let secret = split_list(&region[..first_len])
        .pop()
        .and_then(|field| parse_key_value(&field).map(|(key, _)| key.to_string()))
        .filter(|key| {
            SECRET_PARAMS
                .iter()
                .any(|param| key.eq_ignore_ascii_case(param))
        });
    match (secret, body.split_once('=')) {
        (Some(key), Some((name, value))) if name.trim() == key => {
            let blank = &value[..value.len() - value.trim_start().len()];
            format!("{lead}{name}={blank}***{trail}")
        }
        (Some(key), _) => format!("{lead}{key}=***{trail}"),
        (None, _) => redact_by_name(region),
    }
}

/// A comma-separated `value` with each of its items shown as `shown` makes
/// of it. The items are the parsers' own, taken in regions (design LS-D9): a
/// region of several items is shown as one, which hides at least as much.
fn hide_listed(value: &str, shown: fn(&str) -> String) -> String {
    let mut out = String::with_capacity(value.len());
    for r in regions(value) {
        let item = &value[r.start..r.end];
        let t = item.trim_start();
        out.push_str(&item[..item.len() - t.len()]);
        out.push_str(&shown(t));
        out.push_str(&value[r.end..r.sep_end]);
    }
    out
}

/// The targets of `#!include` are a list separated by commas, and a path with
/// a space in it can be quoted: a target with `://` in it is a link, a local
/// path stays. The targets are taken in regions (design LS-D9), and a region
/// with a link in it goes whole.
fn hide_include_links(targets: &str) -> String {
    let mut out = String::with_capacity(targets.len());
    for r in regions(targets) {
        let target = &targets[r.start..r.end];
        if target.contains("://") {
            let rest = target.trim_start();
            let lead = &target[..target.len() - rest.len()];
            let trail = &rest[rest.trim_end().len()..];
            out.push_str(lead);
            out.push_str("***");
            out.push_str(trail);
        } else {
            out.push_str(target);
        }
        out.push_str(&targets[r.end..r.sep_end]);
    }
    out
}

/// One field of a policy line as `redact_positional_credentials` counts it:
/// where it starts, ends and where the separator after it ends, and whether
/// it is one of the words a first field was split into
/// (`words_of_type_field`).
#[derive(Clone, Copy)]
struct FieldSpan {
    start: usize,
    end: usize,
    sep_end: usize,
    word: bool,
}

fn redact_positional_credentials(value: &str) -> String {
    // the fields as the parsers read them. A line typed with an input method
    // may have `，` for some of its commas: it is read here as the author
    // meant it (design LS-D10), and the separators stay
    let spans: Vec<FieldSpan> = item_spans(value, true)
        .into_iter()
        .map(|span| FieldSpan {
            start: span.start,
            end: span.end,
            sep_end: span.sep_end,
            word: false,
        })
        .collect();
    let spans = words_of_type_field(value, spans);
    // the fields the parser reads, an empty item being none; only as many as
    // the decision needs are looked at
    let mut read = spans
        .iter()
        .filter_map(|span| split_list(&value[span.start..span.end]).pop());
    let takes_server = read
        .next()
        .is_some_and(|keyword| takes_server_type(&keyword));
    let port_third = read
        .nth(1)
        .is_some_and(|field| field.trim_matches(is_blank).parse::<u16>().is_ok());
    if !takes_server && !port_third {
        return value.to_string();
    }
    let mut index = 0usize;
    let mut out = String::with_capacity(value.len());
    let mut k = 0usize;
    while k < spans.len() {
        let span = spans[k];
        let tok = &value[span.start..span.end];
        let blanked = match split_list(tok).pop() {
            Some(field) => {
                index += 1;
                let t = tok.trim_start();
                let stray_port = index == 3
                    && takes_server
                    && field.trim_matches(is_blank).parse::<u16>().is_err();
                (index > 3 || stray_port) && !is_named_param(t.trim_end())
            }
            None => false,
        };
        if !blanked {
            out.push_str(&value[span.start..span.sep_end]);
            k += 1;
            continue;
        }
        let t = tok.trim_start();
        out.push_str(&tok[..tok.len() - t.len()]);
        out.push_str("***");
        // the items today's reading takes along with the field (design
        // LS-D9) go with it, each that is not empty counted as a field; a
        // word of the first field takes none (the blanks between the words
        // are no commas)
        let mut last = k;
        if !span.word {
            let reach = next_comma(&value[span.start..], true)
                .map_or(value.len(), |(at, _)| span.start + at);
            while last + 1 < spans.len() && spans[last].end < reach {
                last += 1;
                let taken = spans[last];
                if split_list(&value[taken.start..taken.end]).pop().is_some() {
                    index += 1;
                }
            }
        }
        out.push_str(&value[spans[last].end..spans[last].sep_end]);
        k = last + 1;
    }
    out
}

fn words_of_type_field(value: &str, spans: Vec<FieldSpan>) -> Vec<FieldSpan> {
    let Some(first) = spans
        .iter()
        .position(|span| split_list(&value[span.start..span.end]).pop().is_some())
    else {
        return spans;
    };
    let FieldSpan {
        start,
        end,
        sep_end,
        ..
    } = spans[first];
    let field = split_list(&value[start..end]).pop().unwrap_or_default();
    let mut words = field.split(char::is_whitespace).filter(|w| !w.is_empty());
    if !(words.next().is_some_and(takes_server_type) && words.next().is_some()) {
        return spans;
    }
    let mut split = spans[..first].to_vec();
    let mut at = start;
    loop {
        let rest = &value[at..end];
        let lead = rest.len() - rest.trim_start().len();
        let word = lead
            + rest[lead..]
                .find(char::is_whitespace)
                .unwrap_or(rest.len() - lead);
        let gap = rest[word..].len() - rest[word..].trim_start().len();
        if word + gap >= rest.len() {
            split.push(FieldSpan {
                start: at,
                end: at + word,
                sep_end,
                word: true,
            });
            break;
        }
        split.push(FieldSpan {
            start: at,
            end: at + word,
            sep_end: at + word + gap,
            word: true,
        });
        at += word + gap;
    }
    split.extend_from_slice(&spans[first + 1..]);
    let port_third = split
        .iter()
        .filter_map(|span| split_list(&value[span.start..span.end]).pop())
        .nth(2)
        .is_some_and(|field| field.trim_matches(is_blank).parse::<u16>().is_ok());
    if port_third { split } else { spans }
}
```

文档注释（代码之外只改这几处）：
- `redact_positional_credentials` 上面那段长注释里 "The fields are the parser's: `split_list` reads the type and the port with the quotes off, and an empty item is none." 一句改成："The fields are the parser's (`value::item_spans`, with `，` cutting too — design LS-D10): `split_list` reads the type and the port with the quotes off, and an empty item is none; a field that is blanked goes together with the items today's reading takes along with it (`next_comma`, design LS-D9), and each of them that is not empty counts as a field." 其余照旧。
- `words_of_type_field` 的注释里 "`spans`, the fields of a policy definition" 照旧，不改。
- `next_top_level_comma` 的注释整段换成："The first comma that is inside no quote and no parenthesised group, by the reading of lists before the line-splitting design: a `"` or a `'` opens a quote **wherever** it appears, inside `"` a backslash escapes the next character, inside `'` nothing escapes, and `(` / `)` nest outside quotes. `None` when there is none — an unterminated quote or group therefore runs to the end of the line, which is the safe side for redaction. The parsers no longer read this way (`value::item_spans`); redaction keeps it to measure how far a value it hides reaches (design LS-D9)."（原来的 "Everything that has to agree with the parser…" 一段去掉。）
- `param_value_len` 的注释改成："How long a parameter's value is: up to the first comma today's reading would split on (`next_top_level_comma`) — quotes may open anywhere inside the value and parentheses group, so `ab"c,d"` and `a(b,c)d` are each one value, as their writer may have meant them. When the scan starts inside a group (a secret nested in a `peer = (…)`), depth starts at 0 there, so the value ends at the group's own next comma."

- [ ] **Step 5：脱敏现有用例的注释与属性用例的一种写法**

`redact.rs` 的测试模块：
- `a_value_ends_at_the_first_top_level_comma_wherever_its_quotes_open` 的文档注释换成："A quote or a parenthesis in the middle of a value opens nothing for the parser (design LS-D1, LS-D3), but its writer may have meant it to guard the comma: the value is hidden as far as today's reading takes it, the items it cut off included — a region (design LS-D9)." 期望输出不改。
- `'args = a,b'` 那组用例里的注释 "a quote that opens inside the value takes the rest of the line, as it does for the parser; so does one left open" 换成 "a quote in the middle of the value opens nothing for the parser (design LS-D1), but the value is hidden as far as today's reading takes it; a quote left open where a value starts takes the rest of the line, for the parser too"。期望输出不改。
- `behind_a_quote_the_value_runs_to_the_end_of_the_item` 的文档注释里 "the value is the rest of the item" 改成 "the value is the rest of the item — of the region (design LS-D9) when today's reading carries the item past a comma"。期望输出不改。
- `spellings()` 里 `spelling(format!("{name}=a\"{both}\"b"), &format!("a\"{both}\"b"))` 一行换成（Step 1 已经换过的话跳过）：

```rust
            // a quote in the middle of a value opens nothing (design LS-D1):
            // the parser reads `a"{V}` and the next item is `{W}"b`
            spelling(format!("{name}=a\"{both}\"b"), &format!("a\"{V}")),
```

- 主属性用例 `whatever_the_parser_reads_as_a_secret_parameter_is_hidden_however_it_is_written` 里查 `Debug` 的那几行换成（脱敏输出的检查不动）：

```rust
                                // what the parser read into the parameter: a
                                // part it cut off is an item of its own — on a
                                // group line a member, which the group's
                                // `Debug` lists (design 6.3)
                                for secret in [V, W] {
                                    if spelling.value.contains(secret) && debug.contains(secret) {
                                        bad.push(format!("Debug shows {secret}"));
                                    }
                                }
```

`[Keystore]` 与 peer 的两条属性用例这个任务不动。

- [ ] **Step 6：`crates/rurge-engine/src/views.rs` 的一处注释**

`mid` / `same_but_mid` 两个注册表上面的注释 `// the quote may open in the middle of the value, too` 换成 `// a quote in the middle of the value opens nothing (line-splitting design LS-D1): the parser reads `ab"c` and an item `d"`, and neither half reaches the hash`。断言不改（指纹取自 `redact_definition`）。

- [ ] **Step 7：跑到绿**

```bash
cargo test -j 8 --workspace --lib -- value::tests redact::tests policy::tests rule::tests text::include::tests spec::reader::tests
```

预期全绿。下面这些现有用例的期望输出不改，必须照旧绿（逐条推过，P32）：`a_list_of_listeners_or_servers_is_split_as_its_parser_splits_it`、`the_positional_values_are_blanked_after_the_type_the_parser_reads`、`a_token_is_a_named_parameter_only_when_a_name_stands_before_its_equals_sign`、`a_name_is_found_behind_what_an_input_method_or_a_web_page_leaves_in_front`、`the_words_of_a_type_field_are_fields`、`a_definition_is_redacted_like_its_profile_line`、`a_quoted_value_is_blanked_whole_however_it_is_quoted`、`a_positional_value_is_blanked_on_every_type_that_takes_a_server_and_a_port`、`a_value_ends_at_the_first_top_level_comma_wherever_its_quotes_open`、`a_secret_right_after_an_opening_parenthesis_is_found`、`a_wireguard_peers_preshared_key_is_redacted`、`a_group_line_loses_its_subscription_and_its_modifier`、`a_remote_include_loses_its_link_and_a_local_one_stays`、`behind_a_quote_the_value_runs_to_the_end_of_the_item`、`an_item_is_hidden_the_way_the_parser_reads_it`、`the_items_of_a_group_are_hidden_the_way_the_parser_reads_them`、`a_policy_name_with_balanced_parentheses_is_read_whole`、`a_value_set_by_with_params_reads_back_unchanged`，以及 `redact.rs` 里按名字找与任意文本不 panic 的那几条。

- [ ] **Step 8：改一行看它变红（每一处都只动 `rurge-config` 的纯函数，哪里也不去；看过就还原）**

| 改动 | 应当变红的用例 |
| --- | --- |
| `item_spans` 里开启引号的那一句去掉 `at_start &&`（引号在任何位置开启） | `a_quote_opens_only_where_a_value_starts_and_a_parenthesis_groups_only_in_a_pair` |
| 第二遍里 `Mark::Open if paired[k]` / `Mark::Close if paired[k]` 去掉 `if paired[k]`（`depth -= 1` 改成 `depth = depth.saturating_sub(1)`，每个括号都分组） | 同上 |
| `regions` 里 `while` 不进（`j` 恒等于 `i`，区域不延伸） | `a_region_ends_where_the_later_of_the_two_readings_ends` 与 `a_value_ends_at_the_first_top_level_comma_wherever_its_quotes_open` |
| `redact_definition` 里两遍的次序换回位置凭据先做 | `a_value_ends_at_the_first_top_level_comma_wherever_its_quotes_open` |

- [ ] **Step 9：两条长行用例单独循环二十次**

```bash
for i in $(seq 1 20); do cargo test -j 8 --workspace --lib -- value::tests::a_long_line_of_one_kind_is_read_in_time_that_grows_with_its_length redact::tests::a_long_line_of_one_kind_is_redacted_in_time_that_grows_with_its_length || break; done
```

- [ ] **Step 10：门禁与提交**

全局约束里的门禁；语料库快照预期不变。提交：`fix: 一行拆成各项时引号只在值的开头开启、括号只在配对时分组，脱敏随之按区域遮`（一个提交）。

**判据**：上面点名的新用例与现有用例全绿；门禁全绿；`git grep -n split_top_level -- crates` 没有结果；计划没有点名的现有用例的期望输出一行也没有改。

---

## Task 2：每一项读成什么（LS-D6 ～ D8）

**文件**：`crates/rurge-config/src/{value.rs,policy.rs,keystore.rs,wireguard.rs,rule.rs,redact.rs,config.rs,diagnostic.rs}`、`crates/rurge-config/src/spec/{reader.rs,group.rs,ss.rs,mod.rs}`、`crates/rurge-config/tests/{secrets.rs,policy_spec.rs}`、`crates/rurge-policy/src/assemble.rs`（注释）。

**接口**：

```rust
// rurge-config::value（ParamMap）
pub fn insert(&mut self, key: &str, value: &str);                  // 值照给的存，不再去引号、不再去空白
pub fn from_fields(fields: &[Field]) -> (ParamMap, Vec<String>);   // 原来收 &[String]
// rurge-config::spec::reader
pub(crate) const CREDENTIAL_HINT: &str =
    "if this is a user name or a password that contains `=`, put it wholly in quotes";
// rurge-config::policy
pub(crate) fn is_name(name: &str) -> bool;   // 签名不变，带 `=` 的文字不是名字
```

`ParamMap::from_fields` 的调用者只有 `policy.rs`、`keystore.rs` 与 `value.rs` 自己的用例；`split_definition` 的外部调用者（`rurge-policy` 的 `subscription.rs`）不受影响。

- [ ] **Step 1：先写用例，看它们红**

同任务 1，分两步写：`a_parameter_keeps_its_value_as_the_quotes_held_it` 与 `definitions_and_params` 的改动要 `from_fields(&[Field])`，`an_unknown_parameter_where_a_password_stands_is_not_named` 与 `spec/ss.rs` 加的两句要 `CREDENTIAL_HINT`——这几处在实现之前编译不过，**后**写；其余的**先**写，跑一遍看它们以断言失败。

`value.rs`：

```rust
    /// A value in quotes is kept as the quotes held it, blanks and quotes
    /// inside included; a whole item in quotes is a value, never a parameter
    /// (line-splitting design LS-D6, LS-D7).
    #[test]
    fn a_parameter_keeps_its_value_as_the_quotes_held_it() {
        let (params, positional) = ParamMap::from_fields(&split_fields(
            "sni=\"\\\"a.example\\\"\", password=\" pw \", key='\"v\"', plain =  x  ",
        ));
        assert!(positional.is_empty());
        assert_eq!(params.get("sni"), Some("\"a.example\""));
        assert_eq!(params.get("password"), Some(" pw "));
        assert_eq!(params.get("key"), Some("\"v\""));
        assert_eq!(params.get("plain"), Some("x"));
        let (params, positional) = ParamMap::from_fields(&split_fields("alice, \"pa=ss\""));
        assert!(params.is_empty());
        assert_eq!(positional, ["alice", "pa=ss"]);
    }
```

`definitions_and_params` 里 `let fields = split_list("user, pass, tfo=true, args=a, args=b, Port=443");` 改成 `split_fields(…)`（同一段文字）。

`policy.rs`：

```rust
    /// A text that holds an `=` is no name (line-splitting design LS-D6): a
    /// definition is cut at its first `=`, and a member that holds one is a
    /// whole item in quotes — a value, which can be a password.
    #[test]
    fn a_text_with_an_equals_sign_is_no_name() {
        assert!(is_name("Tom's HK") && is_name("Proxy (US"));
        assert!(!is_name("pa=ss") && !is_name("a = b"));
        assert_eq!(named("policy", "pa=ss"), "policy");
        let g = parse_group("G", "select, A, \"pa=ss\"", &span()).unwrap();
        assert_eq!(g.members, ["A", "pa=ss"]);
        assert!(g.params.is_empty());
        assert_eq!(shown_members(&g.members), ["A", "***"]);
    }
```

`with_params_writes_around_quotes`：前两组的期望改成（整个在引号里的一项是值，原样留着；参数照旧在原位替换、缺的追加在行尾），其余几组与 `tfo` / `custom` / `direct` 的断言不改：

```rust
            "trojan, t.test, 443, 'tfo=false', password=pw, \"TFO=false\", tfo=true, sni=edge.test"
```

```rust
            "trojan, t.test, 443, \"password=p,w\", \"sni=a.test,b.test\", tfo=true, sni=edge.test"
```

（两组的输入不变，只改期望；先核对今天的期望各是什么，改动只能是"引起来的那一项原样留下、对应的参数追加"。）这条用例的文档注释改成 "How such lines are written: a whole item in quotes is a value and stays as it is (design LS-D6), and what is appended stands in front of a last item that would take it for its own."。`a_modified_line…` 用例 `ITEMS` 里的注释 `// a whole item in quotes is the parameter it reads as` 改成 `// a whole item in quotes is a value (design LS-D6)`，它的文档注释里 "the parser takes the quotes off a whole item before it looks for a key" 改成 "a whole item in quotes is a value, not a key (design LS-D6)"；断言不改（P29）。

`spec/reader.rs`：

```rust
    /// Where a type reads its user name and its password from positions, an
    /// unknown parameter in those two places is most likely a password with
    /// an `=` in it: it is not named, and the hint says how to write it
    /// (line-splitting design LS-D8). Elsewhere on the line, and on a type
    /// that reads no positions, the name is said as before.
    #[test]
    fn an_unknown_parameter_where_a_password_stands_is_not_named() {
        let said = |def: &str| {
            let p = policy(def);
            let mut r = ParamReader::new(&p);
            let _ = r.positional(0);
            let _ = r.positional(1);
            r.finish()
                .into_iter()
                .map(|d| (d.message, d.hint))
                .collect::<Vec<_>>()
        };
        for def in [
            "http, 1.2.3.4, 80, alice, k3y=v4lue",
            "http, 1.2.3.4, 80, k3y=v4lue",
            "socks5-tls, 1.2.3.4, 443, alice, k3y=v4lue",
            "h2-connect, 1.2.3.4, 443, alice, k3y=v4lue",
            "custom, 1.2.3.4, 8388, aes-256-gcm, k3y=v4lue",
        ] {
            assert_eq!(
                said(def),
                [("policy `P`: unknown parameter ignored".to_string(), Some(CREDENTIAL_HINT))],
                "{def}"
            );
        }
        assert_eq!(
            said("http, 1.2.3.4, 80, alice, pw, mystery=1"),
            [(
                "policy `P`: unknown parameter `mystery` ignored".to_string(),
                codes::hint(codes::W_UNKNOWN_KEY)
            )]
        );
        assert_eq!(
            said("trojan, 1.2.3.4, 443, k3y=v4lue"),
            [(
                "policy `P`: unknown parameter `k3y` ignored".to_string(),
                codes::hint(codes::W_UNKNOWN_KEY)
            )]
        );
    }
```

（`policy(def)` 是这个测试模块里现有的辅助函数，策略名是 `P`；`Diagnostic` 的 `hint` 字段是 `Option<&'static str>`。）现有的点名用例（文档注释以"…is said only when…"开头、所有行都以 `http, 1.2.3.4, 80` 开头的那一条）：每一行的底子换成 `trojan, 1.2.3.4, 443`（连同 `"trojan, 1.2.3.4, 443,\u{3000}mystery\u{3000}=1"`），`\"pa=ss\"` 与 `'pa=ss'` 两行的期望改成 `vec!["policy `P`: unexpected positional value #1 ignored".to_string()]`；文档注释里说 `"pa=ss"` 的一句改成 "…what somebody wrote as a password (`aHVudGVyMg==` reads as `ahvudgvymg`) is told by the line; a whole item in quotes is a value (design LS-D6), an unexpected positional one."（P9）。

`rule.rs`：

```rust
    /// A rule's parameter in quotes keeps what they held; a whole item in
    /// quotes is a value, which no parameter is (line-splitting design LS-D6,
    /// LS-D7).
    #[test]
    fn a_quoted_rule_parameter_is_read_once_and_a_whole_quoted_one_is_unknown() {
        let r = parse("DOMAIN,a.com,DIRECT,notification-text=\" Hi \"").unwrap();
        assert_eq!(r.params.notification_text.as_deref(), Some(" Hi "));
        let r = parse("DOMAIN,a.com,DIRECT,\"notification-text=Hi\"").unwrap();
        assert_eq!(r.params.notification_text, None);
        assert_eq!(r.params.unknown, ["notification-text=Hi"]);
    }
```

`spec/ss.rs`：`a_legacy_custom_password_with_an_equals_sign_is_an_error` 加两句（今天是红的：今天的告警说出 `hunter`），文档注释改成说明"不加引号时那一项读成未知参数，不点名、带提示"：

```rust
        assert!(diags.iter().all(|d| !d.message.contains("hunter")), "{diags:?}");
        assert!(
            diags.iter().any(|d| d.code == codes::W_UNKNOWN_KEY
                && d.hint == Some(crate::spec::reader::CREDENTIAL_HINT)),
            "{diags:?}"
        );
```

```rust
    /// A legacy `custom` line's password with an `=` in it reads when the
    /// password is wholly in quotes (line-splitting design LS-D6).
    #[test]
    fn a_legacy_custom_password_with_an_equals_sign_reads_in_quotes() {
        let (got, failed, diags) = read("custom, 127.0.0.1, 8388, aes-256-gcm, \"hunter=2secret\"");
        assert!(!failed, "{diags:?}");
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(got.spec.password.expose(), "hunter=2secret");
    }
```

（`read` 是这个测试模块里现有的辅助函数，返回 `(SsRead, bool, Vec<Diagnostic>)`；口令在 `SsRead.spec.password`，是 `Secret<String>`。）

`spec/mod.rs`（`outcome(name, def) -> SpecOutcome` 是测试模块里现有的辅助函数）：

```rust
    /// A positional user name or password that holds an `=` reads as one
    /// when it is wholly in quotes (line-splitting design LS-D6).
    #[test]
    fn a_positional_credential_with_an_equals_sign_reads_in_quotes() {
        let o = outcome("P", "http, 127.0.0.1, 8080, \"al=ice\", \"k3y=v4lue\"");
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        let ProtoSpec::Http(http) = &o.spec.unwrap().proto else {
            panic!()
        };
        assert_eq!(http.username.as_ref().map(|u| u.expose().as_str()), Some("al=ice"));
        assert_eq!(http.password.as_ref().map(|p| p.expose().as_str()), Some("k3y=v4lue"));
    }
```

`spec/group.rs`：`the_name_of_an_unknown_parameter_of_a_group_is_quoted_only_when_it_is_written_as_one` 里 `assert_eq!(messages("select, A, \"pa=ss\""), nameless());` 改成 `assert!(messages("select, A, \"pa=ss\"").is_empty());`，旁边注释：a whole item in quotes is a member now (line-splitting design LS-D6), one the loader reports (`E0008`)。

`config.rs`：`an_unknown_policy_is_quoted_only_when_it_is_a_name`——组行末尾加 `, \"pa=t0k3n6\"`；`FINAL,DIRECT` 之前加两条规则 `DOMAIN,d.com,\"Q=t0k3n7\"\nDOMAIN,e.com,Q=t0k3n8\n`；`E0008` 的期望成了五条（`Missing 01` 那一条点名，其余四条是 `policy group `G` references an unknown policy`），`E0007` 的期望成了六条（`Nope` 点名，接着四条 `rule references an unknown policy`，最后是 `policy `P`: `underlying-proxy` references unknown policy`）；末尾"任何诊断都没有 `t0k3n`"的循环照旧（今天是红的：`Q=t0k3n7` 与 `Q=t0k3n8` 都被当成名字说出来）。文档注释改成 "…A member is an item of the group line that is no parameter: a link whose `=` was typed as a `:`, text an input method joined with a full-width `＝` or `，`, a whole item in quotes that holds an `=` (line-splitting design LS-D6), is none."

`redact.rs` 的三条属性用例（设计 6.3；P2、P24）：
- `struct Spelling` 的 `value` 改成 `Option<String>`（`None`：解析器把这种写法读成值，不是这个参数）；`spelling` 闭包包一层 `Some`；新加闭包 `let quoted = |text: String| Spelling { text, value: None, open: false };`，第 4、5、6、11、13 种写法（`"{name}={V}"`、`'{name}={V}'`、`"{name}={both}"`、`'{name}={both}'`、`"{upper} = {V}"`）改用它，前面加注释 `// a whole item in quotes is a value, never the parameter it reads like (design LS-D6)`；最后一种（没有伴的引号）的 `value` 是 `Some(format!("\"{both}"))`。
- 主属性用例里查"解析器读到了"的那句换成：

```rust
                                    let as_meant = match &spelling.value {
                                        Some(value) => read.contains(value),
                                        None => read.is_empty(),
                                    };
                                    if !as_meant {
                                        unread.push(text);
                                        continue;
                                    }
```

  查 `Debug` 的那句（任务 1 改过的）换成 `if spelling.value.as_deref().is_none_or(|v| v.contains(secret)) && debug.contains(secret) {`——读成值的写法，`V` 与 `W` 都不许出现在 `Debug` 里（组行上它是一个成员，`is_name` 不认带 `=` 的文字，`Debug` 里是 `***`）。
- `a_keystore_line_loses_what_the_parser_reads_as_its_password_and_payload`：

```rust
            let parsed = crate::keystore::parse_keystore_item("k", &line, &span());
            match (&spelling.value, name) {
                (None, "base64") => assert!(parsed.is_err(), "{line}"),
                (None, _) => assert!(parsed.unwrap().password.is_none(), "{line}"),
                (Some(value), _) => {
                    let item = parsed.unwrap();
                    let read = if name == "password" {
                        item.password.clone().unwrap_or_default()
                    } else {
                        item.base64.clone()
                    };
                    assert_eq!(&read, value, "{line}");
                }
            }
```

  （照这条用例现有的变量名接上；脱敏的检查对所有写法照旧做。）
- peer 的属性用例：底子换成 `format!("public-key = {PUBLIC}, allowed-ips = \"10.0.0.0/8, 192.168.0.0/16\", endpoint = a.test:1")`（原来整个加引号的 `"allowed-ips = …"` 在 LS-D6 之后是值，peer 会少一个字段）；挑写法的条件 `if spelling.value != V { continue; }` 换成 `if spelling.text.contains(W) { continue; }`（留下第 1、2、3、4、5、8、9、10、13、14、15 种）；断言换成 `assert_eq!(last.preshared_key.as_ref().map(|k| *k.expose()), spelling.value.as_ref().map(|_| psk), "{line}");`。
- `redact_item` 的文档注释第一条后面加一条："- A whole item in quotes is a value for the parsers (line-splitting design LS-D6), but a secret parameter it reads like is hidden as one: `"password=…"` keeps `password=***`." 测试模块里注释 "the whole item in quotes: read as the parameter of that name" 改成 "a whole item in quotes: a value for the parser (design LS-D6), hidden as the parameter it reads like"。

`tests/secrets.rs`（凭据的标记用例）：
- 带标记的那份 Profile 里四处整个加引号的 `key=value` 改成 `key="…"`：`ExtQ = external, exec=/bin/true, local-port=1081, args=\"-p {MARKER}-quotedargs\"`、`GQ = select, Http, policy-path=\"https://sub.example/list?token={MARKER}-quotedpath\"`、`certq = type=p12, password=\"{MARKER}-quotedp12\", base64={MARKER}Quoted`、`peer = (public-key = {PUBLIC}, preshared-key=\"{KEY_QUOTED}\", allowed-ips = 10.1.0.0/16, endpoint = wg2.example:51820)`；`KEY_QUOTED` 上面的注释 `/// 32 bytes of it, behind the quotes of a whole item.` 改成 `/// 32 bytes of it, in quotes.`。`QType` 一行之后加两行：`PosEq = http, pe.example, 8080, user, \"{MARKER}-pos=eq\"` 与 `CustomEq = custom, ce.example, 8388, aes-128-gcm, \"{MARKER}-custom=eq\"`。
- `the_profile_loads`：名字表加 `"PosEq", "CustomEq"`；"quotes around a whole item, a requirement directive in front, …" 一句改成 "a value in quotes, a requirement directive in front, a comment behind, a comma in a key"，`// a whole item in quotes` 改成 `// a value in quotes`；`user_and` 那几行后面加 `user_and("PosEq", "pos=eq");`；`Custom` 的口令断言之后照它的写法断言 `CustomEq` 的口令是 `{MARKER}-custom=eq`。
- `a_diagnostic_of_a_mistyped_general_value_holds_no_secret`：`UnkQ = trojan, uq.example, 443, password=pw, \"{MARKER}-pa=ss\"` 不改；期望里的 ``"policy `UnkQ`: unknown parameter ignored"`` 改成 ``"policy `UnkQ`: unexpected positional value #1 ignored"``。
- `what_the_section_parser_reads_is_hidden_in_the_redacted_profile`：三行改成 `Ext = external, exec=/bin/true, local-port=1080, args=\"-p {MARKER}-quotedargs\" // tunnel`、`GQ = select, Http, policy-path=\"https://sub.example/list?token={MARKER}-quotedpath\" #!MACOS-ONLY`、`certq = type=p12, base64=AAAA, password=\"{MARKER}-quotedp12\" ; work`；断言不改。
- 新用例（今天是红的：今天这四处都读成参数）：

```rust
/// A whole `key=value` in quotes is a value, never that parameter
/// (line-splitting design LS-D6): none of the four is read, and none of them
/// shows anywhere — not in the diagnostics, the `Debug`, the summary or the
/// redacted profile.
#[test]
fn a_whole_parameter_in_quotes_is_read_as_a_value_and_shown_nowhere() {
    let text = format!(
        "[Proxy]
ExtQ = external, exec=/bin/true, local-port=1081, \"args=-p {MARKER}-quotedargs\"
[Proxy Group]
GQ = select, ExtQ, \"policy-path=https://sub.example/list?token={MARKER}-quotedpath\"
[Keystore]
certq = type=p12, base64=AAAA, \"password={MARKER}-quotedp12\"
[WireGuard Home]
private-key = {KEY}
self-ip = 10.0.0.2
peer = (public-key = {PUBLIC}, \"preshared-key={KEY_QUOTED}\", allowed-ips = 10.1.0.0/16, endpoint = wg2.example:51820)
[Rule]
FINAL,DIRECT
"
    );
    let loaded = load(&text);
    let config = &loaded.config;
    let ext = config.policies.iter().find(|p| p.name == "ExtQ").unwrap();
    assert_eq!(ext.params.get("args"), None);
    let group = config.groups.iter().find(|g| g.name == "GQ").unwrap();
    assert_eq!(group.params.get("policy-path"), None);
    let certq = config.keystore.iter().find(|k| k.name == "certq").unwrap();
    assert_eq!(certq.password, None);
    assert!(config.wireguard[0].peers[0].preshared_key.is_none());
    let messages: Vec<String> = loaded.diagnostics.iter().map(|d| d.message.clone()).collect();
    assert!(
        messages.iter().any(|m| m == "policy group `GQ` references an unknown policy"),
        "{messages:#?}"
    );
    let summary = config.summary();
    assert_eq!(summary.groups, ["GQ (select) -> [ExtQ, ***]"]);
    for (what, shown) in [
        ("diagnostics", messages.join("\n")),
        ("Debug", format!("{config:#?}")),
        ("summary", format!("{summary:?}")),
        ("summary as JSON", serde_json::to_string(&summary).unwrap()),
        ("redacted profile", redact_profile(&text)),
    ] {
        assert!(leaks(&shown).is_empty(), "{what}: {:?}", leaks(&shown));
    }
}
```

  （`load`、`leaks`、`MARKER`、`KEY`、`PUBLIC`、`KEY_QUOTED` 是这个文件里现有的；照现有用例的写法接上——`leaks` 的签名、`summary()` 的字段名、`redact_profile` 的路径与 JSON 的写法以文件里现有用例为准。）

`tests/policy_spec.rs`：未知参数点名的那条用例里 `{link}W = http, w.example, 80, mystery=1` 改成 `{link}W = http, w.example, 80, alice, pw, mystery=1`（`mystery=1` 站在口令的位置上，LS-D8 之后不点名；挪到两个位置之后，期望里的 "policy: unknown parameter `mystery` ignored" 照旧）。

先写的那一批跑一遍，看它们以断言失败（`redact.rs` 的三条属性用例这时也红：任务 1 的解析器仍把整个加引号的 `key=value` 读成参数）：

```bash
cargo test -j 8 --workspace --lib -- policy::tests::a_text_with policy::tests::with_params_writes spec::reader::tests rule::tests::a_quoted_rule spec::ss::tests::a_legacy_custom_password_with_an_equals_sign_reads spec::tests::a_positional_credential spec::group::tests::the_name_of_an_unknown config::tests::an_unknown_policy_is_quoted redact::tests::whatever_the_parser_reads redact::tests::a_keystore_line redact::tests::a_wireguard
cargo test -j 8 --workspace --test secrets
```

然后写需要新接口的那一批，`cargo test -j 8 --workspace --lib --no-run` 编译不过，这是它们的红。

- [ ] **Step 2：`value.rs` 的 `ParamMap`**

```rust
    /// Adds `key=value`: the key trimmed and lower-cased, the value as given
    /// — the list reader has taken its quotes off once already
    /// (line-splitting design LS-D7).
    pub fn insert(&mut self, key: &str, value: &str) {
        self.entries
            .push((key.trim().to_ascii_lowercase(), value.to_string()));
    }
```

```rust
    /// Split the items of a list into `key=value` parameters and positional
    /// values (line-splitting design 4.2): a whole item in quotes is a value.
    pub fn from_fields(fields: &[Field]) -> (ParamMap, Vec<String>) {
        let mut params = ParamMap::default();
        let mut positional = Vec::new();
        for field in fields {
            match &field.param {
                Some((key, value)) => params.insert(key, value),
                None => positional.push(field.text.clone()),
            }
        }
        (params, positional)
    }
```

- [ ] **Step 3：`policy.rs`**

- 引入改成 `use crate::value::{ParamMap, split_fields, split_list, strip_prefix_ci};`（`unquote` 与 `parse_key_value` 不再用，P28）。
- `parse_policy`：`let fields = split_fields(definition);`，`let Some(type_kw) = fields.first().map(|f| f.text.as_str()) else { … }`，服务器与端口各是 `fields.get(1).map(|f| f.text.as_str()).ok_or_else(…)?` 与 `fields.get(2).map(|f| f.text.as_str()).ok_or_else(…)?`，`&fields[3..]` / `&fields[1..]` 交给 `ParamMap::from_fields(rest)`；其余不动。
- `parse_group`：`let fields = split_fields(definition);`，`type_kw` 同上；循环换成：

```rust
    for field in &fields[1..] {
        match &field.param {
            Some((k, v)) => {
                if kind == GroupKind::Subnet
                    && !GROUP_PARAMS.contains(&k.to_ascii_lowercase().as_str())
                {
                    conditions.push((SubnetExpr::parse(k)?, v.clone()));
                } else {
                    params.insert(k, v);
                }
            }
            None => members.push(field.text.clone()),
        }
    }
```

- `with_params` 认键（数开头的类型、服务器、端口仍用 `split_list`）：

```rust
        let field = split_fields(item).pop();
        let key = match field {
            Some(_) if head > 0 => {
                head -= 1;
                None
            }
            Some(field) => field.param.map(|(k, _)| k.to_ascii_lowercase()),
            None => None,
        };
```

  文档注释的第一段里 "An item's key is the one the parser reads — with the quotes around a whole item taken off, without its case" 改成 "An item's key is the parser's (`split_fields`) without its case — a whole item in quotes is a value and has none (design LS-D6)"。
- `param_value`：

```rust
/// `value` as a line has to spell it to read it back the same: as it is when
/// the list reader leaves it so, otherwise in double quotes, which the reader
/// takes off once and keeps what they held (line-splitting design LS-D7).
fn param_value(value: &str) -> String {
    let plain = !value.is_empty()
        && value.trim() == value
        && !value.contains([',', '"', '\'', '(', ')', '\\']);
    if plain { value.to_string() } else { quoted(value) }
}
```

- `is_name`：`!name.contains("://") && !name.contains(['=', '＝', '，'])`；文档注释加一句："Nor is one that holds an `=`: a definition is cut at its first `=`, so no name has one, and an item of a group line that holds one is a member only when it is wholly in quotes — a value, which can be a password (line-splitting design LS-D6)."
- `PolicyGroup` 的 `Debug` 上面的注释里 "A member is whatever lies between two commas and has no `=`, a link whose `=` was typed as a `:` among it: one that is no name is `***`." 改成 "A member is an item of the line that is no parameter; one that is no name is `***`."

- [ ] **Step 4：其余读参数的调用者**

`keystore.rs`：引入改成 `use crate::value::{ParamMap, parse_key_value, split_fields};`（`unknown_field` 仍用 `parse_key_value`）；`parse_keystore_item` 里 `let fields = split_fields(definition);`，`ParamMap::from_fields(&fields)` 照旧；`unknown` 换成：

```rust
    let unknown = fields
        .iter()
        .filter(|field| match &field.param {
            Some((key, _)) => !matches!(
                key.to_ascii_lowercase().as_str(),
                "type" | "base64" | "password"
            ),
            None => true,
        })
        .map(|field| field.text.clone())
        .collect();
```

（现有用例 `said("\"passphrase=hunter2\"")` 照旧不点名：`unknown_field` 看这一项写成什么样。）

`wireguard.rs`：引入改成 `use crate::value::{parse_bool, split_definition, split_fields, split_list};`；peer 字段的循环开头换成：

```rust
    for field in split_fields(inner) {
        let Some((key, value)) = &field.param else {
            // never quoted: it may be a key that lost its name, or a whole
            // item in quotes, which is a value (line-splitting design LS-D6)
            report.warn(span, format!("peer {n}: a field without `=` ignored"));
            continue;
        };
        let (key, value) = (key.as_str(), value.as_str());
```

（循环体其余照旧。）

`spec/group.rs`：引入改成 `use crate::value::{parse_bool, split_fields, split_list};`；`modifier` 换成：

```rust
/// `key=value` items the way the list reader reads them (line-splitting
/// design 4.2); `None` when the list is empty or an item is not a pair — a
/// whole item in quotes is a value, not a pair (design LS-D6).
fn modifier(value: &str) -> Option<Vec<(String, String)>> {
    let fields = split_fields(value);
    if fields.is_empty() {
        return None;
    }
    fields
        .into_iter()
        .map(|field| field.param.map(|(key, value)| (key.to_ascii_lowercase(), value)))
        .collect()
}
```

`rule.rs`：引入改成 `use crate::value::{Field, split_fields, split_list};`；`parse_rule` 里 `let fields = split_fields(raw);`，类型是 `fields.first()` 的 `.text`（转大写照旧），值与策略各是 `fields.get(…).map(|field| field.text.as_str()).ok_or_else(…)?`，参数是 `RuleParams::parse(&fields[policy_idx + 1..])`；`RuleParams::parse` 换成（数字先去空白再 parse，P26）：

```rust
    fn parse(fields: &[Field]) -> RuleParams {
        let mut p = RuleParams::default();
        for f in fields {
            let (k, v) = match &f.param {
                Some((k, v)) => (k.as_str(), Some(v.as_str())),
                None => (f.text.as_str(), None),
            };
            match (k.to_ascii_lowercase().as_str(), v) {
                ("no-resolve", None) => p.no_resolve = true,
                ("dns-failed", None) => p.dns_failed = true,
                ("extended-matching", None) => p.extended_matching = true,
                ("pre-matching", None) => p.pre_matching = true,
                ("requires-resolve", None) => p.requires_resolve = true,
                ("notification-text", Some(v)) => p.notification_text = Some(v.to_string()),
                ("notification-interval", Some(v)) if v.trim().parse::<u32>().is_ok() => {
                    p.notification_interval = v.trim().parse().ok()
                }
                ("update-interval", Some(v)) if v.trim().parse::<i64>().is_ok() => {
                    p.update_interval = v.trim().parse().ok()
                }
                ("always-capture", Some(v)) => p.always_capture = Some(v.to_string()),
                _ => p.unknown.push(f.text.clone()),
            }
        }
        p
    }
```

（子规则里的 `split_list` 照旧：子规则的标志只要文字。）

- [ ] **Step 5：LS-D8（`spec/reader.rs`）与 `codes::hint` 的文档**

```rust
use crate::policy::{PolicyKind, ProxyPolicy, is_name, named};
use crate::value::{parse_bool, parse_key_value, split_fields, split_items, split_list};

/// The hint of an unknown parameter where a user name or a password stands
/// (line-splitting design LS-D8): it says how such a value is written, and
/// nothing of the line.
pub(crate) const CREDENTIAL_HINT: &str =
    "if this is a user name or a password that contains `=`, put it wholly in quotes";

/// The keys of the items that stand where a policy of `policy`'s type takes
/// its user name and its password — the fourth and the fifth field the
/// parser reads (line-splitting design LS-D8) — and that it reads as
/// parameters: there, a key is most likely the front of a password with an
/// `=` in it.
fn credential_keys(policy: &ProxyPolicy) -> HashSet<String> {
    let positional_credentials = policy.legacy_custom
        || matches!(
            policy.kind,
            PolicyKind::Http
                | PolicyKind::Https
                | PolicyKind::Socks5
                | PolicyKind::Socks5Tls
                | PolicyKind::H2Connect
        );
    if !positional_credentials {
        return HashSet::new();
    }
    split_fields(&policy.definition)
        .into_iter()
        .skip(3)
        .take(2)
        .filter_map(|field| field.param.map(|(key, _)| key.to_ascii_lowercase()))
        .collect()
}
```

`finish` 里，参数循环之前 `let credentials = credential_keys(policy);`；在 `if !self.used.contains(key) && reported.insert(key.to_string()) {` 的块里、算 `message` 之前：

```rust
                if credentials.contains(key) {
                    let message = format!("{}: unknown parameter ignored", named("policy", &policy.name));
                    self.diags.push(
                        Diagnostic::warning(codes::W_UNKNOWN_KEY, message)
                            .at(policy.span.clone())
                            .with_hint(CREDENTIAL_HINT),
                    );
                    continue;
                }
```

`written_as_name` 的文档注释里 "The key is held lower-cased and unquoted — a Base64 password `aHVudGVyMg==` arrives as `ahvudgvymg`, `"pa=ss"` as `pa` — so this is decided on the text of the line, not on the key." 改成 "The key is held lower-cased — a Base64 password `aHVudGVyMg==` arrives as `ahvudgvymg` — so this is decided on the text of the line, not on the key."（P27）。

`diagnostic.rs`：`codes::hint` 的文档注释第一段之后加一段："`W0001` keeps its sentence, and one of its places carries one of its own: an unknown parameter where a user name or a password stands (`spec::reader`, line-splitting design LS-D8)."

`crates/rurge-policy/src/assemble.rs`：说 modifier 怎样够到一行的那段测试注释里 "an item in quotes is the parameter it reads as and takes the modifier's value" 改成 "a whole item in quotes is a value (a user name on these types, line-splitting design LS-D6) and the modifier's parameter is appended"；断言不改。

- [ ] **Step 6：跑到绿**

```bash
cargo test -j 8 --workspace --lib -- value::tests redact::tests policy::tests rule::tests keystore::tests wireguard::tests spec:: config::tests diagnostic
cargo test -j 8 --workspace --test secrets --test policy_spec
cargo test -j 8 --workspace --lib -- assemble
```

预期全绿；`a_value_set_by_with_params_reads_back_unchanged` 与 `a_modified_line…` 照旧绿（P18、P29）。

- [ ] **Step 7：改一行看它变红（只动 `rurge-config` 的纯函数，哪里也不去；看过就还原）**

| 改动 | 应当变红的用例 |
| --- | --- |
| `credential_keys` 恒返回空集 | `an_unknown_parameter_where_a_password_stands_is_not_named` |
| `ParamMap::insert` 照旧再去一次引号（`unquote(value.trim())`） | `a_parameter_keeps_its_value_as_the_quotes_held_it` |
| `is_name` 不认 `=` | `a_text_with_an_equals_sign_is_no_name` 与 `an_unknown_policy_is_quoted_only_when_it_is_a_name` |

- [ ] **Step 8：门禁与提交**

提交：`fix: 整个在引号里的一项是值，引起来的值只去一次引号，位置凭据处的未知参数不点名`（一个提交）。

**判据**：上面点名的新用例与改过的用例全绿；门禁全绿；`git grep -n "parse_key_value" -- crates/rurge-config/src/policy.rs` 没有结果；语料库快照没有变（变了的照全局约束处理）。

---

## Task 3：端到端与文档

**文件**：`crates/rurge/tests/cli.rs`；`docs/surge-compatibility-matrix.md`、`docs/acceptance/phase1-acceptance.md`、`README.md`、`README_en.md`、`CLAUDE.md`、`docs/superpowers/plans/2026-10-14-manual-baseline-recheck-plan.md`。

- [ ] **Step 1：`rurge check` 的两条用例（`crates/rurge/tests/cli.rs`）**

```rust
/// A profile with what the line-splitting design made loadable: names with an
/// apostrophe or a parenthesis that pairs with nothing, in the definitions,
/// the group, the rules and an `#!include`; `password` stands where `PwEq`'s
/// password does.
fn line_splitting_profile(password: &str) -> String {
    format!(
        r"[Proxy]
Tom's HK = direct
Tom's US = direct
Proxy (US = direct
PwEq = http, 127.0.0.1, 8080, alice, {password}

[Proxy Group]
G = select, Tom's HK, Tom's US

[Rule]
#!include Tom's rules.dconf, more.dconf
USER-AGENT,Bob's App*,G
URL-REGEX,^http://a\.example/[^(]*$,REJECT
DOMAIN,b.example,Proxy (US,extended-matching
FINAL,DIRECT
"
    )
}

/// Runs `rurge check` offline on the profile above with `password`, the data
/// directory in the test's own temporary one.
fn check_line_splitting(password: &str) -> std::process::Output {
    let dir = tempfile::tempdir().unwrap();
    let conf = write(&dir, "a.conf", &line_splitting_profile(password));
    write(&dir, "Tom's rules.dconf", "[Rule]\nDOMAIN,a.example,DIRECT\n");
    write(&dir, "more.dconf", "[Rule]\nDOMAIN,c.example,DIRECT\n");
    rurge_command()
        .args(["check", "--data-dir"])
        .arg(dir.path().join("data"))
        .arg("-c")
        .arg(&conf)
        .output()
        .unwrap()
}

/// Names with an apostrophe or a parenthesis that pairs with nothing load,
/// and so does a positional password with an `=` in it when it is wholly in
/// quotes (line-splitting design, section 7).
#[test]
fn line_splitting_reads_apostrophes_parentheses_and_quoted_credentials() {
    let out = check_line_splitting("\"k3y=v4lue\"");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("0 error(s), 0 warning(s), 0 note(s)"), "{stdout}");
}

/// The same password without its quotes is read as a parameter: one `W0001`
/// that does not name it, with the hint that says how to write it, and no
/// part of it anywhere in the output (line-splitting design LS-D8).
#[test]
fn line_splitting_names_no_part_of_an_unquoted_credential() {
    let out = check_line_splitting("k3y=v4lue");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    let warnings: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with("warning["))
        .map(|(at, _)| at)
        .collect();
    assert_eq!(warnings.len(), 1, "{stdout}");
    let at = warnings[0];
    assert!(lines[at].starts_with("warning[W0001]"), "{stdout}");
    assert!(lines[at].ends_with("policy `PwEq`: unknown parameter ignored"), "{stdout}");
    assert_eq!(
        lines[at + 1],
        "  hint: if this is a user name or a password that contains `=`, put it wholly in quotes",
        "{stdout}"
    );
    // the path of the profile holds a random name, which could hold `k3y`
    // but not these
    for part in ["`k3y`", "k3y=", "v4lue"] {
        assert!(!stdout.contains(part), "{part}: {stdout}");
    }
    assert!(stdout.contains("0 error(s)"), "{stdout}");
    assert!(out.status.success(), "{stdout}");
}
```

（`write`、`rurge_command` 是这个文件里现有的辅助函数。提示的那句话抄自 `CREDENTIAL_HINT`——它是 `pub(crate)` 的，P11。）这两条在任务 1、2 之后写出来就该是绿的；"改一行看它变红"：`item_spans` 里开启引号的那一句去掉 `at_start &&` 时第一条变红，`credential_keys` 恒返回空集时第二条变红（只动 `rurge-config` 的纯函数；`rurge check` 离线，数据目录在用例的临时目录里，哪里也不去；看过就还原）。若第一条在 `0 note(s)` 上不成立，是别的提示（不是这一轮的），只断言错误与告警的个数，并记进「执行期修正记录」。

```bash
cargo test -j 8 --workspace --test cli -- line_splitting
```

提交：`test: rurge check 读带撇号与不配对括号的行`。

- [ ] **Step 2：文档**

**兼容性清单**（`docs/surge-compatibility-matrix.md`）：
- 1.1 节"引号值"一行的备注写上：引号只去一次，引起来的值原样保留（里面的引号字符、两头的空白都在；`key="\"v\""` 得 `"v"`），`with_params` 写回时只引一次（一行怎样拆成各项的设计 LS-D7；用例 `a_parameter_keeps_its_value_as_the_quotes_held_it`、`a_quote_opens_only_where_a_value_starts_and_a_parenthesis_groups_only_in_a_pair`）。标记不变。
- "值含逗号"一行不变。
- "一行怎样拆成各项（引号与括号）"一行的备注整格改写成新的读法，标记仍是 🟡：引号只在一个值开始的地方开启引用，括号只在配对时分组，整个在引号里的一项是值；`Tom's HK`、`Proxy (US`、`USER-AGENT,Bob's App*,…`、`[^(]` 这样的写法加载得了；读法变了的（`password=ab"c,d"` 切开；整个加引号的 `key=value` 在策略行上是多余的位置值 `W0001`、在组行上是找不到的成员 `E0008`、在 peer 与 `[Keystore]` 上少一个字段）；`=` 左边定义的名字照原样读——`[Proxy]` 与 `[Proxy Group]` 都用 `rurge check` 试过，带引号就带着引号被定义；行内注释与行尾指令认引号的方式不变（LS-D13）；仍是 🟡 因为手册没有说的那几种情形是 rurge 自己的读法、没有在 Surge 上观察过。引用设计与用例名（`a_quote_opens_only_where_a_value_starts_and_a_parenthesis_groups_only_in_a_pair`、`the_reading_is_the_same_from_any_cut_and_of_any_item_alone`、`an_apostrophe_or_an_unpaired_parenthesis_does_not_take_the_rest_of_the_line`、`a_member_with_an_apostrophe_or_an_unpaired_parenthesis_is_one_member`、`line_splitting_reads_apostrophes_parentheses_and_quoted_credentials`）。原来"怎样绕开"的那几句去掉（不用再绕开）。
- `custom` 一行：备注里"口令里带 `=` 的旧行读不出口令（…）（用例 …）"那一句换成：口令里带 `=` 时把整个口令用引号包起来（`custom, h, 8388, aes-256-gcm, "pa=ss"`）就读得出；不加引号时那一项读成未知参数，`W0001` 不点出它的名字、带一句提示（LS-D6、LS-D8；用例 `a_legacy_custom_password_with_an_equals_sign_reads_in_quotes`、`a_legacy_custom_password_with_an_equals_sign_is_an_error`）。前面引用的另两条用例照旧。
- 策略参数表里 `http` `https` `h2-connect` 的 `username` `password` 一行与 `socks5` `socks5-tls` 那一行，备注各接一句：位置上的用户名或口令含 `=` 时整个用引号包起来；不加引号的那一项读成未知参数，`W0001` 不点出它的名字、带一句提示（LS-D6、LS-D8；用例 `a_positional_credential_with_an_equals_sign_reads_in_quotes`、`an_unknown_parameter_where_a_password_stands_is_not_named`）。
- 标记都没有变，附录不重数。

**阶段 1 验收文档**（`docs/acceptance/phase1-acceptance.md`）：FR-CFG-01 的"引号值与 `\"` `\\` 转义"一行改指 `unquote`（按伴判断）、`item_spans` 与 `split_fields`（只去一次、原样保留），用例加上这一轮的，写一句 2026-10-14 的说明（LS-D1、LS-D4、LS-D7）；"含逗号的引号值"一行的代码改指 `item_spans`，用例加 `the_reading_is_the_same_from_any_cut_and_of_any_item_alone` 与读法表那一条，写一句说明：一个值中间成对的引号不再护住逗号（LS-D1）。两行的结论不变；三个数（2 / 37 / 10）不变。

**README**（中英两份一起改，内容一致）：状态一段里"这一轮没有改一行怎样拆分（带撇号的名字会吃掉一行的后半），登记在兼容性清单里，下一步先为它做设计。"（英文是 "How a line is split (a name with an apostrophe swallows the rest of its line) was not changed in this round; it is registered in the matrix and gets a design of its own next."）换成说这一轮做了什么："随后的一轮改了一行怎样拆成各项：引号只在一个值开始的地方开启引用，括号只在配对时分组，带撇号或不配对括号的名字（`Tom's HK`、`Proxy (US`）加载得了；整个在引号里的一项是值，位置上含 `=` 的口令引起来就读得出；一个值中间成对的引号不再护住逗号（`password=ab"c,d"`），要护住就把整个值引起来；差异登记在兼容性清单 1.1 节。"与它的英文。"现在："那一段（英文 "Where the project stands:"）里接下来的事：一行怎样拆分做完了，下一步是阶段 3 自己的设计；读 CI 的失败清单仍排在最前（第一次运行的失败摘要没有说出失败的用例，修正之后要再推送一次）。

**CLAUDE.md**：
- 状态一段里手册基线重对那一节"没有改、要知道的"中这一句："一个值中间没有配对的引号或括号会吃掉一行的后半（`G = select, Tom's HK, Tom's US` 把两个成员读成一个），解析器与脱敏要一起改，清单 1.1 节写着怎样绕开；"改成"一个值中间没有配对的引号或括号会吃掉一行的后半（随后由一行怎样拆成各项那一轮改掉，见下）；"（P34）。
- "接下来的工作依次是"之前加一段"一行怎样拆成各项（2026-10-14 ～ 16）：……"，写：改了什么（LS-D1 ～ D13 的要点：引号只在值开始的地方开启、括号只在配对时分组、整个在引号里的一项是值、只去一次引号、LS-D8 的告警与提示、脱敏的区域与两遍的次序）、在哪里（`value::item_spans` / `split_fields` / `Field`、`redact.rs` 的 `regions`、`spec::reader::CREDENTIAL_HINT`）、使用者看得到的变化（设计第 7 节的要点）、这些决定是在项目所有者"自己决定、继续做"的常设授权下定的、所有者可以改、没有覆盖的（设计第 10 节：行内注释与行尾指令、`[General]` 的引号值、定义处带引号的名字、引起来的空项、脱敏视图里不带引号的 `=` 口令、Surge 的实际行为）、设计与计划的路径。门禁那一句（"`<提交>` 上的全量运行……"）这个任务不改，主持这一轮的会话在整分支评审之后改。
- "接下来的工作依次是"里去掉"一行怎样拆分的设计"那一项，其余照旧、重新编号。
- 「先读这些文档」：手册基线重对的三条之后加设计与本计划各一条。
- 「常用命令」：照那一节的写法（`cargo test -p …`，P25）加：

```bash
cargo test -p rurge-config value::            # 一行怎样拆成各项：读法表、整个在引号里的一项是值、引起来的值只去一次引号、穷举六个字符以内的全部字符串、一行几十万个同一种字符读得完
cargo test -p rurge-config -- a_region_ends positional_credentials_are_counted   # 脱敏的区域与按解析器的字段数遮位置凭据
cargo test -p rurge-config -- a_text_with_an_equals_sign an_unknown_parameter_where_a_password_stands a_positional_credential_with_an_equals_sign a_legacy_custom_password   # 整个在引号里的一项是值、带 `=` 的文字不是名字、位置凭据处的未知参数不点名
cargo test -p rurge --test cli -- line_splitting   # rurge check 的端到端：带撇号与不配对括号的行、引起来的含 `=` 的口令
```

**手册基线重对计划**（`docs/superpowers/plans/2026-10-14-manual-baseline-recheck-plan.md`）「延后事项」第 1、9 行的去向后面各接："；做掉：一行怎样拆成各项（`docs/superpowers/plans/2026-10-14-line-splitting-plan.md`）"（P33）。

设计的「实施期的订正」与本计划的两张表由主持这一轮的会话在整分支评审之后填写，这个任务不动。

- [ ] **Step 3：核对与提交**

文档里的每一个名字（函数、类型、用例、命令、诊断码）用 `git grep` 在代码里找得到；表格的列数与表头一致；README 两份一致。门禁照旧全绿（文档不该动到它）。提交：`docs: 一行怎样拆成各项`。

**判据**：两条 `rurge check` 用例绿、各自的"改一行"验证红过；文档如上；`git grep -n "清单 1.1 节写着怎样绕开" -- CLAUDE.md` 没有结果。

---

## 计划期决定

写计划时对着设计与本仓库源码核对后定下的事；与设计文字不同的，写进设计的实施期订正。

| # | 事项 | 核对到的事实 | 决定 |
| --- | --- | --- | --- |
| P1 | `Field` / `split_fields` 放在哪个任务 | 设计第 11 节把它们放在任务 2；任务 1 的 `split_list` 要做到"整个在引号里按伴判断""参数的值整个在引号里才去引号"，这两件事正是 `read_field` | 任务 1 就建 `Field`、`split_fields` 与 `read_field`，`split_list` 是它的投影（`text`）；读 `param` 的调用者在任务 2 才改 |
| P2 | 属性用例里 `{name}=a"{V},{W}"b` 一种 | LS-D1 之后解析器读成参数值 `a"{V}` 与另一项 `{W}"b`；组行上 `{W}"b` 是一个成员，组的 `Debug` 列出它（设计 6.3） | 写法的值改成 `a"{V}`；`Debug` 只查解析器读进这个参数的那一段含有的秘密；脱敏的输出照旧不许有 `V`、`W` |
| P3 | 两遍各用哪种读法量 | 逐项那一遍在原文上，区域用不认 `，` 的扫描（解析器不认它）；位置凭据那一遍数字段要认 `，`（LS-D10），量延伸用今天认 `，` 的 `next_comma(…, true)` | 照这样 |
| P4 | 几项并成的区域里有括号组 | 区域的第一项可以是一个配对的括号组（`(a"b),c"d`：今天的读法从 `(` 读下去，引号护住了逗号） | `redact_joined` 不进括号组逐项看；按名字找（`redact_by_name`）在整个区域里找，括号组里的秘密由它遮掉 |
| P5 | 第一个字段拆成的词 | `words_of_type_field` 拆出的词之间是空白，不是逗号 | 词不按区域延伸（`FieldSpan.word`） |
| P6 | 空的引号项 | 今天 `""`、`''` 丢掉，`" "` 是一个空格的值 | 照旧（设计 4.2）；读法表钉住 |
| P7 | 穷举的规模 | 九个字符（`a` `,` `"` `'` `(` `)` `=` `\` 与空格），长度 1 ～ 6，共 597,870 条；debug 构建下一条用例几秒 | `short_strings()` 放在 `value.rs`（`#[cfg(test)] pub(crate)`），两条性质用例共用；断言数过 50 万条 |
| P8 | 扫描记下哪些标点 | 第二遍只要引用之外的 `,`、`(`、`)` 与开启值的 `=` | `Mark::{Comma, Open, Close, Eq}`；`，` 只在 `wide` 时记，不开启值、不改引号与括号的状态（LS-D10） |
| P9 | LS-D8 与 `ParamReader` 的现有点名用例 | 那条用例的底子是 `http, 1.2.3.4, 80`：端口之后头两个位置上的未知参数在 LS-D8 之后不点名 | 底子改成 `trojan, 1.2.3.4, 443`（不读位置凭据的类型）；新用例先读两个位置值，再看告警 |
| P10 | LS-D8 的提示与别的告警的提示 | `Diagnostic::warning` 带 `codes::hint(code)`，`with_hint` 换掉它 | 不点名的那一条带 `CREDENTIAL_HINT`；同一行别处点名的照旧带 `W0001` 的缺省提示——两样都断言 |
| P11 | 提示放在哪里 | 只有 `spec::reader` 用它；CLI 用例在 crate 之外 | `pub(crate) const CREDENTIAL_HINT`；`codes::hint` 的文档加一句；CLI 用例把这句话抄一份 |
| P12 | 语料库快照 | 七份快照的配置里没有撇号、不配对的括号与整个加引号的 `key=value` | 预期不变；本机没有 `cargo insta`，变了用 `INSTA_UPDATE=always` 写出、逐行看 |
| P13 | 别的 crate 里说到旧读法的注释 | `rurge-engine/src/views.rs` 一处，`rurge-policy/src/assemble.rs` 一处 | 任务 1、2 各改一处；断言不变 |
| P14 | `key_line` 认键 | 设计 6.1：换成"两种读法任何一种看到逗号就不是键"会让 `a'b, http-listen = …` 少藏 | 照旧只用 `next_comma`，加一句注释说为什么 |
| P15 | `written_as_name` 与 `keystore::unknown_field` | 它们看一项写成什么样（设计第 5 节） | 任务 1 只把 `split_top_level` 换成 `split_items`；读文字照旧 |
| P16 | `is_name` 的用例从哪里看 | `shown_members` 是组的 `Debug` 与 `summary()` 共用的 | 单元用例经 `shown_members`，`secrets.rs` 再经 `summary()` 看一遍 |
| P17 | "单独读一项还是这一项"在 `wide` 下 | `，` 之后的一项单独读时，前面的 `，` 不在，开头的引号会开启——不一定成立 | 只在不认 `，` 的读法上断言；`wide` 的这一点登记（延后第 9 行） |
| P18 | `with_params` 写回再读 | 任务 1 的 `split_list` 加第二次去引号、任务 2 的只去一次，各自与 `param_value` 对得上（逐种写法推过） | `a_value_set_by_with_params_reads_back_unchanged` 两个任务都不改，照旧绿 |
| P19 | 次序对调之后的一种输出 | 读位置凭据的类型上，整个在引号里的秘密参数今天由位置那一遍先遮成 `***`；对调之后逐项那一遍先写出 `password=***`，位置那一遍把它当命名参数留着 | 接受：藏的一样多；没有用例钉着 `***` 这种输出（逐条看过） |
| P20 | 带 `=` 的成员在诊断里怎么说 | `quoted_name` 对不是名字的说 "a link" | 措辞这一轮不改（延后第 11 行） |
| P21 | 读法表怎样比 | `Field` 的 `Debug` 是 `Field(***)`，比不出差在哪里 | 用例比 `(text, param)` 元组 |
| P22 | `key='it's'` | 单引号没有转义：伴是第二个 `'`，其后还有 `s'`，值不是整个在引号里，读成 `'it's'` 原样 | LS-D4 的代价，不钉 |
| P23 | 聚焦地跑用例 | 本机不用 `-p`（另编一份依赖） | 计划里的命令都经 `--workspace` |
| P24 | peer 属性用例的底子 | 底子里 `"allowed-ips = 10.0.0.0/8, 192.168.0.0/16"` 是整个加引号的 `key=value`，LS-D6 之后读成值、peer 少一个字段 | 任务 2 改成 `allowed-ips = "10.0.0.0/8, 192.168.0.0/16"` |
| P25 | CLAUDE.md 的命令写法 | 那一节一直写 `cargo test -p …` | 新命令照那一节的写法 |
| P26 | 规则参数里数字的空白 | 今天 `v.trim()` 之后再 parse；`split_fields` 的值没有引号时已去空白，有引号时原样 | 数字先 `trim()` 再 parse（`notification-interval`、`update-interval`） |
| P27 | 文档里的旧例子 | `written_as_name` 的文档说 `"pa=ss"` 读成 `pa` | 去掉这个例子（LS-D6 之后它是值） |
| P28 | `unquote` 与 `policy.rs` 的引入 | 任务 2 之后 `unquote` 在生产代码里没有调用者；它是 `pub` 的 | `unquote` 留着、仍 `pub`；`policy.rs` 去掉 `unquote` 与 `parse_key_value` 的引入 |
| P29 | `a_modified_line…` 用例 | 推过：两个任务下断言都成立 | 只改注释 |
| P30 | 新的凭据用例里组的摘要 | `summary()` 经 `shown_members` | 断言 `["GQ (select) -> [ExtQ, ***]"]` |
| P31 | 规则里整个加引号的参数 | `"notification-text=Hi"` 是值，进 `unknown`，同一个写错了的参数 | 不另做：照规则参数的未知项现有的处理 |
| P32 | 任务 1 之后脱敏现有用例的输出 | 逐条推过（任务 1 Step 7 列出的名字），都不变；`\u{feff}password=***` 照旧遮（`is_param_name` 只认 ASCII，`trim()` 不去 BOM） | 计划点名的之外，现有用例的期望输出一律不改 |
| P33 | 手册基线重对计划的延后第 1、9 行 | 这两行由本轮接手 | 任务 3 在它们的去向后面注明做掉 |
| P34 | CLAUDE.md 里手册基线重对那一段说到这件事的一句 | 那一句是当时的状态 | 改写，不删 |

## 执行期修正记录

| # | 任务 | 与计划的出入 | 处理 |
| --- | --- | --- | --- |
| 1 | 任务 1 | 计划没说 `FieldSpan` 放在哪里：第一次放在 `redact_positional_credentials` 的长注释与函数之间，那段注释挂到了结构体上 | 提交之前挪到注释前面；`7c31ab6` |
| 2 | 任务 1 | 属性用例里 `{name}=a"{V},{W}"b` 那一种的值在 Step 1 就改了（它属于先看红的那一批），`Debug` 的检查随 Step 5 改 | 照做 |
| 3 | 任务 1 | 门禁：最后一次全量里 `outbounds_http2` 以 `STATUS_HEAP_CORRUPTION` 退出一次，单独重跑 15 / 15 | 开发机的管控软件，不是 rurge 的缺陷 |
| 4 | 任务 1 | 任务评审：计划给的 `redact_joined` 重复了 `redact_item` 的尾巴（前后空白的拆法、认秘密参数名、三支 `match`）——计划自己写进去的重复 | 主持这一轮的会话代项目所有者裁定：抽成 `blanks_around`、`secret_key`、`hide_value` 三个私有函数，两处共用；输出不变（实现者在 261,550 条生成的字符串上比过前后的散列）；`fc5e30e` |
| 5 | 任务 1 | 任务评审：位置凭据那一遍给区域并进来的非空字段计数（计划照设计 6.1 写的），这个计数改变不了输出——被遮的字段已在端口之后，其后每个字段计不计数都在端口之后 | 去掉；设计第 12 节第 1 条；`fc5e30e` |
| 6 | 任务 1 | 任务评审：三处行为没有用例钉住——`redact_entry` 新加的"解析器的读法在 `=` 前看到逗号"、`redact_joined` 第一支保留 `=` 两边的空白、LS-D10（`，` 只在要求时切开、不开启一个值） | 补三条用例（`a_comma_in_front_of_the_first_equals_sign_by_the_parsers_reading_counts`、`a_value_carried_past_a_comma_keeps_the_spacing_around_its_name`、`a_full_width_comma_cuts_only_when_asked_and_starts_no_value`），各以一行变异看红；`fc5e30e` |
| 7 | 任务 1 | 任务评审拿旧代码与新代码比脱敏的输出：本来就坏了的行上有比此前少遮的写法——服务器字段开头多一个引号、口令引起来且含逗号（`trojan, "h, 443, password="ZQ1,ZQ2", ws=true` 留下 `ZQ2"`）；一行里三个以上多余的引号字符（`external, pw",",  password=ZQ1"x,ZQ2"y` 留下 `ZQ2"y`）。对 54 行基础写法逐字符增删改得到的 62,510 种里，997 种比此前遮得多、8 种少 | 主持这一轮的会话代项目所有者裁定：接受，登记延后第 12 行；要堵上就得改两遍的次序，是设计上的事。终审查出触发面比这里写的宽，见第 18 行 |
| 8 | 任务 2 | 计划没点名的现有用例 `a_load_gives_each_diagnostic_the_hint_of_its_code` 变红：它的 `B = http, h.test, 80, bogus=1` 里 `bogus=1` 正站在口令的位置上，LS-D8 之后带自己的提示 | 主持这一轮的会话裁定：那一行改成 `B = http, h.test, 80, alice, pw, bogus=1`，断言不改（同计划对 `tests/policy_spec.rs` 的改法）；`303a7f1` |
| 9 | 任务 2 | 先写的那一批用例里，`--test secrets` 的一半在实现之前没有跑 | 实现之后以一次额外的变异补证（`from_fields` 把整个加引号的项又读成参数）：`secrets.rs` 的三条用例随之变红 |
| 10 | 任务 2 | 门禁：第一次全量里 `outbounds_snell` 的 `v2_shares_one_connection_and_v1_and_v3_never_do` 失败一次（期望 1 条连接、看到 3 条），单独跑 20 次、整个二进制 30 次、再跑全量都绿 | 与这一轮无关（Snell 行里没有引号，也不在位置凭据处）；登记延后第 17 行 |
| 11 | 任务 2 | 任务评审：照计划的组合，整个加引号的 `key=value` 后面漏了逗号、粘着下一个参数时（`"pa=ss"=1`、`"password=…"sni=x`），读到的键里带着引号里的部分；`written_as_name` 照文字读，找不到读成这个键的一项，于是点了名——引号里的口令（小写）进了 `W0001`，策略与组都是 | 主持这一轮的会话代项目所有者裁定：`written_as_name` 先要求键本身是参数名的写法（`is_plain_name`）；读法的用例加三行，`secrets.rs` 加 `a_parameter_glued_behind_a_whole_one_in_quotes_is_not_named`；`34ff473` |
| 12 | 任务 2 | 任务评审：`ParamReader` 的 LS-D8 一支手拼诊断，而 `GroupReader` 有 `warn_with_hint`；LS-D8 的用例够不到 `https`、`socks5` 两支与"空的一项不算字段" | 补 `ParamReader::warn_with_hint`；用例加三行，各以一行变异看红；`34ff473` |
| 13 | 任务 2 | 试着让 `secrets.rs` 的 `leaks` 按小写找标记（键是小写存的），`a_line_under_general_that_is_no_setting_is_not_quoted` 随之变红：`[General]` 里丢了名字后 `=` 的策略行被当作未知键、小写存下，它的 `Debug` 把 `key = value` 拼回一行交给 `redact_line`，口令（小写）留在里面——这一轮之前就有 | 照裁定还原了 `leaks`，不修；登记延后第 13 行 |
| 14 | 任务 2 | 全量第一次 `rurge-dns` 的 `resolver` 以 `STATUS_HEAP_CORRUPTION` 退出、一次链接器 `LNK1104`（文件被占用），重跑都过 | 开发机的事 |
| 15 | 任务 3 | 两条 `rurge check` 用例写出来就是绿的（`0 note(s)` 也成立），各以计划给的一行变异看红；rustfmt 折了三行 | 照做；`0c7632a` |
| 16 | 任务 3 | 文档里几处照计划写得更细：清单"定义处"与 `custom` 的 `E0018`、验收文档那两行另指到 `wholly_quoted` / `read_field`（`unquote` 已没有生产代码里的调用者）、CLAUDE.md 里手册基线重对那一段的说法 | 照留；`6629016` |
| 17 | 任务 3 | 任务评审：这一轮让几份"写着代码现在的做法"的文档有句子不对了——`external-policy-modifier` 怎样改写一行（清单、阶段 2 验收文档）、脱敏按什么认一个值的终点（阶段 1 与阶段 2 的 API 参考、清单两行）；另有四处小的（阶段 1 验收文档的出处一段、CLAUDE.md 里"`next_comma` 只用来量区域"、清单 1.1 节粘连的例子、清单 `is_name` 的清单与 `[Proxy Group]` 一行）与一条只钉住 `http` 的用例 | 都改了；实现者另找到同一类过时的句子几处（CLAUDE.md 的 M8c 那一句与一条命令的注释、阶段 2 手工清单一项）一并改；用例扩到五种类型；`c3280dd`、`5446065` |
| 18 | 终审 | 已接受的脱敏退步，触发面比第 7 行写的宽：同一行前面任何一个值在值开始的地方开了引号而漏了收尾（服务器字段、`sni=` 一类参数或误打的一个引号），后面又有一个含逗号、加了引号的秘密值时（整个加引号，或值中间带一对引号，如 `password=ab"p1,p2"`），逗号之后那一段露出（`trojan, h.example, 443, sni="a.example, password="p1,p2", ws=true` 留下 `p2"`），在 `GET /v1/profiles/current?sensitive=0` 与 `GET /v1/policies/detail` 里，`lineHash` 随它变。终审的对照：按 Surge 写法生成的 40 万行里新代码多藏 33,196 行、少藏 32 行（都是设计 6.4 已登记的不加引号的 `=` 口令），各插一处误打的 40 万行里多藏 47,429 行、少藏 147 行 | 主持这一轮的会话代项目所有者裁定：这一轮不改代码（收口要在原文上按解析器的字段数决定位置凭据、再落到逐项输出的区域上，或改 LS-D9 两遍的次序，是设计层面的事，归项目所有者）；记录照实写（CLAUDE.md、延后第 12 行、设计第 12 节第 3 条），阶段 1 与阶段 2 的 API 参考、清单的一处加了限定语 |
| 19 | 终审 | 小项：照 LS-D8 的提示写出的位置口令以秘密参数名加 `=` 开头时露出这个前缀（P19 说的"藏的一样多"在读位置凭据的类型上不成立）；LS-D1 把切开的前半放进了不是凭据的位置（监听地址的错误、DNS 服务器 URL 的 `Debug`）；设计第 7 节漏了两处读法变化（`external-policy-modifier`、`key='it's'`） | 登记延后第 18、19 行；设计第 12 节记下；清单 1.1 节与 CLAUDE.md 补上第 7 节漏的两处 |
| 20 | 门禁 | 最后一次全量在 `5446065` 上（此后只有文档变过）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 干净，`cargo test --workspace --no-fail-fast` 74 个结果行、3438 个通过、0 个失败、6 个忽略（`rurge-api` 与 `rurge-policy` 的单元测试二进制在这次全量里以 `STATUS_HEAP_CORRUPTION` 退出，单独重跑 29 / 29、169 / 169，计入这个数）——一台机器的一次运行，不是 CI | — |

## 延后事项

| # | 事项 | 去向 |
| --- | --- | --- |
| 1 | LS-D13：行内注释与行尾的要求指令认 `"` 的方式（只认双引号、在任何位置开启） | 有用户报告再说 |
| 2 | `[General]` 一类键值行整个取值带引号的不去引号（`loglevel = "notify"` 得 `W0012`） | 阶段 1 验收文档已登记，照旧 |
| 3 | LS-D11：`=` 左边定义的名字带引号就带着引号被定义 | 有用户报告再说 |
| 4 | 引起来的空项照旧丢掉（`http, h, 80, "", pw` 把口令读成用户名） | 有用户报告再说；改它要连同脱敏的字段数一起改 |
| 5 | 位置上不带引号、含 `=` 的口令在脱敏后的视图里照旧显示（设计 6.4） | 项目所有者（M8c 计划「延后事项」第 51 行：要不要换成只显示已知无害内容的视图） |
| 6 | Surge 怎样读这些行没有观察过 | 有真机时对一遍；清单照旧 🟡 |
| 7 | 组行上整个加引号、不是秘密参数的 `key=value`（如 `"pa=ss"`）在脱敏后的视图里照原样留着 | 接受：它是一个成员；诊断与 `Debug` 不引用它（`is_name`） |
| 8 | `with_params` 追加的修饰的键里有 `)` 时，会与行里前面一个不配对的 `(` 配成对 | 有用户报告再说 |
| 9 | `wide` 读法下"单独读一项还是这一项"不总成立（`，` 之后开头的引号） | 接受：只有脱敏数位置凭据用 `wide`，切分处照样 |
| 10 | `http h.example 8080 al'ice, pw', tfo=true` 现在脱敏成 `http h.example 8080 ***, ***`（`tfo=true` 被并进区域遮掉） | 接受：偏安全一侧 |
| 11 | `quoted_name` 对带 `=` 的成员说 "a link" | 下次动到时 |
| 12 | 已接受的脱敏退步（执行期修正记录第 7、18 行）：同一行前面某个值在值开始的地方开了引号而漏了收尾（服务器字段、`sni=` 一类参数或误打的一个引号），后面又有一个含逗号、加了引号的秘密值时（整个加引号，或值中间带一对引号，如 `password=ab"p1,p2"`），逗号之后那一段留在脱敏后的视图里（`trojan, h.example, 443, sni="a.example, password="p1,p2", ws=true` 留下 `p2"`；`trojan, "h, 443, password="ZQ1,ZQ2", ws=true` 留下 `ZQ2"`）；一行里三个以上多余的引号字符时另有几种（`external, pw",",  password=ZQ1"x,ZQ2"y` 留下 `ZQ2"y`） | 项目所有者：收口的方向是在原文上按解析器的字段数决定位置凭据，再落到逐项输出里对应的区域上（或改 LS-D9 两遍的次序）；收口时连逐项那一遍一起看——没有位置凭据那一遍的类型（如 `external`）上同一种露出此前就有（`external, exec=/bin/true, sni="a.example, password="p1,p2", ws=true` 前后两版都留下 `p2"`）——，并在 `secrets.rs` 补一条"前面漏了收尾引号、后面一个含逗号的引号口令"的标记用例 |
| 13 | `[General]` 里丢了名字后 `=` 的策略行：未知键小写存下，它的 `Debug` 露出小写的口令；`secrets.rs` 的 `leaks` 区分大小写，看不见这一类（执行期修正记录第 13 行） | 项目所有者（脱敏后的视图与 `Debug` 的边界，同 M8c 计划「延后事项」第 51 行）；修它时把 `leaks` 改成按小写找 |
| 14 | `redact_entry` 新条件里 `item_spans(key, true)` 的 `wide` 没有用例钉住（要一个被旧的读法的引号挡住的 `，`） | 下次动到时 |
| 15 | `blanks_around` 还能替换 `redact.rs` 里另外三处同样的前后空白拆法 | 下次动到时 |
| 16 | `subnet` 组里带引号里 `=` 的条件键，`SubnetExpr::parse` 的错误与组的 `Debug` 原样显示（条件是网络名，不是凭据） | 接受 |
| 17 | `outbounds_snell::v2_shares_one_connection_and_v1_and_v3_never_do` 在这台开发机上失败过一次（期望 1 条连接、看到 3 条） | 再见到时查（连接池与计时） |
| 18 | 读位置凭据的类型上，整个加引号、以秘密参数名加 `=` 开头的口令（`http, h.example, 80, alice, "token=abc123"`）在脱敏后的视图与 `Debug` 里显示成 `token=***`，露出 `token=` 这一段（此前是 `***`）；P19 说的"藏的一样多"在这里不成立 | 项目所有者（与第 12 行的收口一起做；收口时在 `secrets.rs` 补一条以 `token=` 开头的位置口令） |
| 19 | 值中间成对的引号不再护住逗号之后，被切开的前半可能落进不是凭据的位置：`http-listen = ab"p1,p2"@127.0.0.1:6152` 的地址错误（`E_LISTENER_NOT_IP`）引用 `ab"p1`，`dns-server = https://u:"p1,p2"@d.example/dns-query` 的 `EncryptedDns` 的 `Debug` 显示 `https://u:"p1`（找不到 `@`） | 有用户报告再说（把引号放在值中间的口令本来就要整个加引号）；可选的收口：`E_LISTENER_NOT_IP` 不引用不像地址的一项 |
