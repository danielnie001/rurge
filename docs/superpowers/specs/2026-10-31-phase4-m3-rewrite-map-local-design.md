# 阶段 4 / M3「重写与 Map Local」细化设计

| 项       | 内容 |
| -------- | ---- |
| 日期     | 2026-10-31 |
| 状态     | 草案，已过构建者检查（2026-10-31：四个问题——选定 REJECT 的请求、换目标之后请求的形式与转发、改过的 `Host`、解密的连接上对上游的 TLS 配置——都由协调者定了，是 M3-D27 ～ D31；没有新的归所有者的事）。已决事项 M3-D1 ～ D31 是在项目所有者"自己决定、继续做"的常设授权下定的（代项目所有者作的裁定），所有者可以改。执行交给 Pi：M3a、M3b 两份计划由协调的会话逐个任务调 Pi 做、逐个任务评审；M3c 等所有者回答总设计 Q4 再写（第 2 节） |
| 上位设计 | [阶段 4 总设计](2026-10-29-phase4-http-engine-design.md) 第 2.3 节（3.1、3.13 ～ 3.17、3.24、3.25）、第 3.3 ～ 3.5、3.7、6.2、6.3、8、10.1、12、13、14、15 节；[M1 细化设计](2026-10-29-phase4-m1-session-entry-http-engine-design.md) 第 8 节（引擎，先读第 20、21 节的订正）；[M2 细化设计](2026-10-30-phase4-m2-mitm-design.md) 第 9、10 节（先读第 23 节的订正）。不一致处以本文为准，订正列在第 19 节 |
| 需求     | FR-HTTP-05（URL Rewrite，M3a）；FR-HTTP-06（Header Rewrite，M3a）；FR-HTTP-07（Body Rewrite：正则多对替换、按行锚定、`^$`、UTF-8、两个上限、chunked 与 `Expect`、解压与 `Content-Length` 在 M3b，jq 与 `br`、`zstd` 的解压在 M3c，"先于脚本"在阶段 5）；FR-HTTP-08（Map Local，M3b）；FR-HTTP-09（流水线的顺序与 Map Local 短路；"每请求 / 响应最多一个脚本"在阶段 5）；FR-HTTP-13 的 `rewrite`（M3a） |
| 清单     | 1.x 节 L50 ～ L53（四个节）；7.3 节 L590 ～ L599；7.4 节 L601 ～ L608；7.5 节 L610 ～ L620；7.6 节 L622 ～ L632；11.x 节 L871（功能开关）。行号是 2026-10-31（`cf6b1df`）的 |
| 前置     | M2（M2a、M2b、M2c）做完，本地 `main` = `cf6b1df`；`origin/main` = `41c35d4`（M1 与 M2 的 98 个提交等项目所有者推送，推送之后读那次 CI）。M3 在分支 `phase4-m3` 上，从 `cf6b1df` 起 |
| 调研     | 总设计的三份调研（`docs/reports/2026-10-29-phase4-*.md`）；手册五页（`http/url-rewrite.html`、`http/header-rewrite.html`、`http/body-rewrite.html`、`http/map-local.html`、`http/overview.html`，2026-10-31 重新取的文本，取法照基线报告的附录 B；前四页的 SHA-256 与 2026-10-14 的基线相同，没有改过），另有 `getting-started/how-surge-works.html`（"After a policy is selected, HTTP requests and responses can be rewritten, mapped to local data, or processed by scripts."），下文的引文出自它们；ICU 的正则用户指南（`\h`、`\v`、`\R`、`\Z` 的定义，`.` 与多行模式的 `^` `$` 认的行终止符，替换文本里的 `$n` 与 `\`）；swift-corelibs-foundation 的 `NSRegularExpression.replacementString(for:in:offset:template:)`（Apache-2.0，读的是它的源码：`$` 后最多读"分组数的位数"那么多位数字，编号不小于范围数时得空串，`\` 被去掉、它后面的一个字照字面且不再参与展开，结尾单独的 `\` 被去掉；Apple 平台上的 Foundation 不开源，没有核对）；本机 cargo 注册表里锁定版本的源码：fancy-regex 0.14.0（`src/parse.rs` 第 248 ～ 259 行多行模式的 `^` `$` 写死 `crlf: false`，注释 "TODO: support crlf flag"；`RegexBuilder` 只有 `case_insensitive`、`backtrack_limit` 与两个委托的上限，没有 `multi_line`，多行只能在式子前写 `(?m)`；`replace_all` / `replacen` 遇到运行期的错误（回溯超限）会 panic，`try_replacen` 与 `captures_iter` 把错误交回）、regex 1.13.1（`RegexBuilder::multi_line` 与 `crlf`：`.` 不配 `\r` 与 `\n`，`^` `$` 把两者都当行尾、不在 `\r\n` 中间匹配）、flate2 1.1.10。**注册表里没有 jaq 一族、brotli-decompressor、ruzstd，fancy-regex 只有 0.14.0** |

M3 让四个重写的节生效：`[URL Rewrite]` 与 `[Header Rewrite]`（M3a），`[Body Rewrite]` 的正则形式与 `[Map Local]`（M3b）。M1b 的引擎里，每个请求在求过规则、选定策略之后（选的是 REJECT 一族的照旧被拒绝、不过重写，第 8.4 节），依次过 Header Rewrite → URL Rewrite（`302` / `307` / `reject` 在这里答复，`header` 模式换掉拨号的目标）→ Map Local（命中就答复，不连上游）→ Body Rewrite，再交给上游；应答依次过 Header Rewrite 与 Body Rewrite，再交给客户端。明文的请求与 M2 解密之后的请求走同一条路。每个起了作用的改写、每个被跳过的改写，在请求的记录里留一条备注；`rewrite` 开关生效并持久化。

**jq、`br` 与 `zstd` 不在 M3a、M3b 里**：总设计为它们定的三个新包（jaq 一族、brotli-decompressor、ruzstd）本机的注册表里没有，引入要下载，而"本阶段新引入的包……同意吗"是所有者的 Q4、还没有回答——同 M2 不升级 `p12-keystore`（M2-D2）。M3b 读到 jq 的行告警并忽略（`W0042`）；`br` 与 `zstd` 编码的正文不改写、原样转发、留备注。所有者回答 Q4 之后另写 M3c（第 2 节）。fancy-regex 同样留在 0.14.0（总设计第 3.3 节要升到 ≥ 0.18 以得到 CRLF 模式，同样要下载）：Body Rewrite 按 CRLF 读行改由锁文件里已有的 `regex` 1.13.1 来做（第 5.4 节）。

**M3 没有平台代码**：改的是 `rurge-config`、`rurge-net`、`rurge-http`、`rurge-engine`、`rurge-api` 与可执行文件里不带 `cfg` 的部分，开发机上全部编译、全部跑；没有等 CI 的停点。M1 与 M2 的推送照旧归所有者，M3 不等它。

## 1. 范围

| 做 | 不做（去向） |
| -- | ------------ |
| 一行的切法：按空白切，一项可以用 `"…"` 或 `'…'` 包住（第 4.1 节，M3a）；`value.rs` 加 `split_words`，`redact.rs` 不改、理由写进提交说明（第 4.3 节） | 脱敏视图里藏起重写节的头值与 `[Map Local]` 的 `data=` / `header=`（所有者的 Q2） |
| 正则：编译之前把 ICU 与 Rust 的正则包悄悄不同的几种写法改写成等价的写法，`URL-REGEX` 规则同样（第 5.2 节）；照 `NSRegularExpression` 展开的替换模板（第 5.3 节）（M3a） | fancy-regex 升到 ≥ 0.18（要下载，所有者的 Q4，第 19 节第 1 条） |
| `[URL Rewrite]`：类型化、四种模式、三个候选的 URL、`{{{GATEWAY_ADDRESS}}}`、`header` 模式换目标而不重选策略、改成 `https` 时上游走 TLS（第 6、8.2 节，M3a） | — |
| `[Header Rewrite]`：类型化、两个方向、四种动作、改到框架头时请求失败（第 7 节，M3a） | 改逐跳的头（`Connection` 等）对上游生效（不做：引擎自己管这些头，第 7.4 节，登记） |
| `rewrite` 开关生效并持久化（第 11 节，M3a） | `capture` 生效（M4；在那之前照旧读作关、`POST` 答 501） |
| 备注、计数、`/v1/rurge/http` 的 `rewrite`（第 8.5、12 节，M3a 起） | 抓包（M4）；Surge 的请求查看器里的"重写的备注"以外的展示 |
| `[Replica]` 认得并忽略，提示 `I0005`（第 4.4 节，M3a） | — |
| `[Body Rewrite]` 的正则形式：何时缓冲、两个上限、`gzip` 与 `deflate` 的解压、UTF-8、按 CRLF 读行（第 9 节，M3b） | jq 形式（M3c，等 Q4；M3b 告警并忽略，`W0042`）；`br`、`zstd` 的解压（M3c，等 Q4；M3b 不改写、留备注） |
| `[Map Local]`：四种 `data-type`、文件（每个请求时打开、流式读、路径先问 `Reach`）、URL（资源管理器的新一类资源）、`header` 的两种写法、`status-code`、`Content-Type` 的表（第 10 节，M3b） | — |
| 手册四页上的每个示例一条黄金用例，jq 的那一条除外（第 16 节第 3 条） | jq 示例的黄金用例（M3c）；手工验收本身（归所有者） |
| 手工清单 `docs/acceptance/phase4-manual.md` 加 M3 的几项（M3b） | — |
| — | 脚本与"先于脚本"（阶段 5）；HTTP 引擎之外的流量（不做：手册 "URL Rewrite only applies to requests handled by the HTTP engine."） |

## 2. 计划与执行

三份计划。后一份在前一份合并之后开工，它的计划也在那时对着合并之后的代码写（同 M1、M2）：

- `docs/superpowers/plans/2026-10-31-phase4-m3a-url-header-rewrite-plan.md`
- M3b（Body Rewrite 的正则形式与 Map Local）的计划日后写，文件名同样的格式。
- M3c（jq 与 `br`、`zstd`）等所有者回答 Q4 再写；所有者不同意引入新包时 M3c 不做，三者停在 `W0042` 与备注。

M3a 的任务：

| 任务 | 内容 |
| ---- | ---- |
| 0 | 开工前核对：第 18 节的各项对着当时的代码核一遍，出入记进「执行期修正记录」；量一次基线 |
| 1 | `rurge-config`：`split_words`（第 4.1 节）；新 `dialect.rs` 的 `translate` 与 `Template`，`Pattern::new` 先改写再编译（第 5.2、5.3 节） |
| 2 | `rurge-config`：`[URL Rewrite]` 与 `[Header Rewrite]` 类型化（新 `rewrite.rs`）、`[Replica]` 与 `I0005`、`W0041`、从 `W0016` 的名单拿掉这两节、语料库快照、脱敏的那一条用例（第 4.3、4.4、6.1、7.1、13 节） |
| 3 | `rurge-http`：新 `rewrite` 模块——一代配置的 `Rewrite`（规则与对上游的两份 TLS 配置）、三个候选的 URL、URL Rewrite 与 Header Rewrite 的求值，都是不碰网络的函数、各有单元用例（第 6、7、8.2 节） |
| 4 | `rurge-http` 的引擎：每个请求里的位置、`Upstreams::retarget` 与 `gateway`、`Routed::Go` 带 `rewrite`、换目标之后请求的形式与 `Proxy-Authorization`、改过的 `Host` 的写回、每个请求自己的对上游 TLS、池的键 `(K, OriginKey)`、框架头的失败、`302` / `307` / `reject` 的答复、嗅探到的连接上不过重写的两种请求、备注（第 6.3、7.4、8 节） |
| 5 | 引擎：`Runtime.rewrite`、`rewrite` 开关与 `state.json`、启动时还原、`route` 对选定 REJECT 一族的请求给 `rewrite: None`、`retarget` 与 `gateway` 的实现、计数；API：`/v1/features/rewrite` 生效、`/v1/rurge/http` 的 `rewrite`（第 8.2、8.4、11、12 节） |
| 6 | 端到端用例：URL Rewrite 与 Header Rewrite 两页的黄金用例，与第 16 节第 3 条里属于 M3a 的其余几条 |
| 7 | 文档收尾（M3a 的部分，第 17 节） |

M3b 的任务（计划日后写，这里是大纲）：核对；`[Body Rewrite]` 与 `[Map Local]` 类型化（`BodyPattern`、jq 行的 `W0042`、`Reach::map_local_refusal` 与 `W0033`、从 `W0016` 拿掉这两节、`CORE_VERSION` 的注释）；正文的缓冲、上限与解压；Body Rewrite 在引擎里的两个方向；Map Local 的四种数据与 `ResourceKind::MapLocal`；端到端（两页的黄金用例、32 MiB 与 10 MiB 的边界、chunked 与 `Expect`）；手工清单加 M3 的几项；M3 收尾的文档（`docs/status.md`、清单、两份 README、需求文档第 7 节的状态一句、`CLAUDE.md`）。

M3c 的任务（等 Q4，这里只列要回答的事）：jaq 的引入与版本、许可与传递依赖；总设计第 3.4 节的隔离——专用的少数线程、各自的栈大小、等到时限就放弃这次改写，先核实 jaq 有没有递归深度的上限；`br` 与 `zstd` 的解码；单引号里的 jq 式子遇到 ` //`（jq 的替代运算符）会被行内注释截断（第 4.2 节）的处理；jq 的黄金用例。

**执行的约定**（两份计划开头重申，同 M1、M2）：协调的会话逐个任务开 Pi 的会话、把任务的全文与要用的接口交给它；Pi 做完写报告、提交；协调者读改动、跑门禁、评审，有问题回到同一个 Pi 会话修正，再评审，过了才派下一个任务。每个任务先写失败的用例、看到红，再实现到绿；门禁是 `CLAUDE.md` 的全量门禁；提交说明的标题用中文。计划没有写到、要做决定的，Pi 停下来报告，不自己定。

做完 M3b 之后：项目所有者推送（若那时 M1 与 M2 还没有推送，就三个里程碑一起），读回那次 CI；再往后是 M4（抓包与请求记录的完整）。M3 没有只在 CI 上编译的代码，M3a 之后不单独停下来等推送。

## 3. crate 与接口

**不加新包、不下载**：M3 用到的 `regex` 1.13.1（今天由 criterion 与 predicates 带进锁文件，`aho-corasick`、`memchr`、`regex-automata`、`regex-syntax` 都在）、fancy-regex 0.14.0、flate2 1.1.10 都已在 `Cargo.lock` 里、源码在本机注册表里。工作区的依赖表加 `regex = "1"` 一行（M3b）；`rurge-config` 新增正式依赖 `regex`（Body Rewrite 的正则在加载时编译，第 5.4 节，M3b），`rurge-http` 新增正式依赖 `flate2`（M3b）。`Cargo.lock` 的包数应当不变（今天 520 个），用到新依赖的任务各核一次。依赖方向不加新的工作区边：`rurge-http → rurge-config`、`rurge-http → rurge-net`、`rurge-engine → rurge-http` 都已有。`unsafe` 不变。

| crate | 改动 |
| ----- | ---- |
| `rurge-config` | `value.rs`：`split_words`（M3a）；新 `dialect.rs`：`translate`、`Template`（M3a），`BodyPattern`（M3b）；`rule.rs`：`Pattern::new` 先 `translate` 再编译（M3a；策略组的 `policy-regex-filter` 与 `policy-priority` 也经它，第 5.2 节）；新 `rewrite.rs`：`RewriteConfig` 与四个节的读法（URL / Header 在 M3a，Body / Map Local 在 M3b）；`deferred.rs`：去掉四个节（M3a 两个、M3b 两个）；`[Replica]` 的认得与 `I0005`（M3a）；`reach.rs`：`map_local_refusal`（M3b）；`config.rs`：`Config.rewrite`；`diagnostic.rs`：`W0041`、`I0005`（M3a），`W0042`（M3b） |
| `rurge-net` | `ResourceKind::MapLocal`（M3b） |
| `rurge-http` | 新 `rewrite` 模块：`Rewrite`、`RewriteCounters`、候选的 URL、URL Rewrite 与 Header Rewrite（M3a），正文的缓冲、解压与改写，Map Local 的应答与扩展名的表（M3b）；`engine.rs`：每个请求里的位置、`Upstreams::retarget` / `gateway`、`Routed::Go` 的 `rewrite`、换目标之后的形式、`Host` 的写回、每个请求的对上游 TLS、池的键（M3a），正文的两个方向与 Map Local（M3b）；`mitm/decrypt.rs`：`OriginTls` 可以克隆（M3a） |
| `rurge-engine` | `Runtime.rewrite`；`Engine` 里的 `rewrite` 开关（`rewrite_on` / `set_rewrite`，与 `mitm_on` 并排）、`state.json`、`restore_switches`；`ConnUpstreams::route` 对选定 REJECT 一族的请求给 `rewrite: None`，`retarget` 与 `gateway`；`EngineShared` 里的计数（M3a）；Map Local 的 URL 资源、`external_resources.rs` 的 `type_name` 加 `map-local`（M3b） |
| `rurge-api` | `/v1/features/rewrite` 生效；`/v1/rurge/http` 的 `rewrite`（M3a） |
| `rurge`（可执行文件） | M3a 不改：启动时 `run.rs` 已经调 `restore_switches`，它多读 `features.rewrite` 就够了；`capabilities.rs` 里 `CORE_VERSION` 的注释（M3b） |

接口的形状（细节以计划为准）：

```rust
// rurge-config（M3a）
/// 一行里的各项（第 4.1 节）：去掉引号之后的文字、在原文里的位置、是不是参数
pub struct Word { pub text: String, pub start: usize, pub end: usize, pub param: Option<(String, String)> }
pub fn split_words(text: &str) -> Vec<Word>;
/// 编译之前的改写（第 5.2 节）；Err：这里不支持的写法，消息说是哪一种
pub fn translate(source: &str) -> Result<String, String>;
pub struct Template { /* 字面与组号，第 5.3 节 */ }
impl Template {
    pub fn parse(text: &str) -> Template;   // 任何文字都是合法的模板
    /// `group(i)`：第 i 组匹配到的文字，没有参与匹配时 None；`count`：分组数（不含第 0 组）
    pub fn expand(&self, group: &dyn Fn(usize) -> Option<&str>, count: usize, out: &mut String);
}
pub struct RewriteConfig {
    pub url: Vec<UrlRewrite>,
    pub header: Vec<HeaderRewrite>,
    pub body: Vec<BodyRewrite>,     // M3b
    pub map_local: Vec<MapLocal>,   // M3b
}
pub struct UrlRewrite { pub pattern: Pattern, pub replacement: Template, pub mode: UrlMode, pub span: Span }
pub enum UrlMode { Header, Found, Temporary, Reject }   // header / 302 / 307 / reject
pub enum Direction { Request, Response }
pub struct HeaderRewrite { pub direction: Direction, pub pattern: Pattern, pub action: HeaderAction, pub span: Span }
pub enum HeaderAction {
    Add { name: String, value: String },
    Del { name: String },
    Replace { name: String, value: String },
    ReplaceRegex { name: String, regex: Pattern, template: Template },
}

// rurge-config（M3b）
pub enum BodyPattern { Plain(regex::Regex), Fancy(fancy_regex::Regex) }   // 第 5.4 节
pub struct BodyRewrite { pub direction: Direction, pub pattern: Pattern, pub pairs: Vec<(BodyPattern, Template)>, pub span: Span }
pub struct MapLocal { pub pattern: Pattern, pub data: MapData, pub status: u16, pub headers: Vec<(String, String)>, pub span: Span }
pub enum MapData { File(PathBuf), Url(String), Text(String), Base64(Vec<u8>), TinyGif }

// rurge-http（M3a）
/// 第 12 节的六个数（原子的），一个引擎一份：放在 `EngineShared` 里，交给每一代的 `Rewrite`
pub struct RewriteCounters { /* url、header、body、map_local、skipped、failed */ }
pub struct Rewrite { /* 一代配置的规则；对上游的两份 TLS 配置（第 8.2 节）；计数 */ }
impl Rewrite {
    /// Ok(None)：四个节都没有可用的行；Err：建 TLS 配置失败（同 `Mitm::build`），引擎告警、这一代没有重写
    pub fn build(cfg: &RewriteConfig, mitm: &MitmConfig, roots: Arc<RootCertStore>, counters: Arc<RewriteCounters>) -> Result<Option<Rewrite>, BuildError>;
}
pub trait Upstreams {
    // 已有的 named、open、route、dial、relay 不变；新加两个（第 8.2 节）
    /// URL Rewrite 的 `header`：同一个策略、同样的尝试，换一个目标
    fn retarget(&self, route: Self::Route, key: Self::Key, target: Target, forwardable: bool) -> (Self::Route, Self::Key);
    /// `{{{GATEWAY_ADDRESS}}}`：当前出口的 IPv4 缺省网关
    fn gateway(&self) -> Option<Ipv4Addr>;
}
pub enum Routed<R, K> {
    /// `rewrite`：None 表示开关关着或这一代没有规则，这个请求不过任何重写
    Go { route: R, key: K, pages: Pages, rewrite: Option<Arc<Rewrite>> },
    Refused { refusal: Refusal, pages: Pages },
}
```

头的名字与值在 `rurge-config` 里是加载时校验过的 `String`（`rurge-config` 不依赖 `http`），`rurge-http` 转成 `HeaderName` / `HeaderValue` 时不会失败。

## 4. 一行的读法（M3a）

### 4.1 切法

四个重写的节与 `[Map Local]` 的一行，先照今天由节的解析器（`text::parse_str`）处理：整行的注释、行内注释、`#!REQUIREMENT` 一类的前缀、停用的行，都与别的节一样。剩下的文字由新的 `split_words` 切成项（M3-D3）：

- 项与项之间是一段空白（空格与制表符），引号外的；
- 引号只在一项开始的地方、或一项里第一个 `=` 的紧后面打开（`key="v w"`）；`"` 里反斜杠连同它后面的一个字一起算，`'` 里什么也不转义；没有配对的引号一直到这一行的末尾（这一项照字面，那个引号也在内）。别处的引号是普通的字；
- 一项整个在引号里时去掉引号：`"…"` 里 `\"` 是 `"`、`\\` 是 `\`、别的反斜杠照留，`'…'` 里照字面；`key=value` 的值整个在引号里时，这一项是去掉引号的那个参数；
- 每一项记着它在原文里的起止，第 7.1 节"这一行余下的部分"要用。

这与逗号分隔的列表的读法（`value.rs` 开头的说明与 LS-D4，`item_spans` 与 `wholly_quoted`）是同一套规矩，只是以空白代逗号、没有括号的分组、不认全角逗号。

**总设计第 2.3 节 3.25 写"没有转义"，本文改为照 LS-D4 认 `\"` 与 `\\`**（第 19 节第 4 条）：节的解析器去行内注释时（`strip_inline_comment`）在 `"` 里本来就把反斜杠与后一个字当一体，不认转义的切法会与它对不上引号在哪里结束；整份 Profile 也只有一种读引号的办法。代价：写在 `"…"` 里的正则要字面的反斜杠时得写 `\\\\`；写在 `'…'` 里或不加引号则不必（登记）。

### 4.2 引号与行内注释

`strip_inline_comment` 只认 `"` 是引号：` #`、` ;`、` //` 在双引号之外就开始一段注释。所以写在 `'…'` 里的文字遇到这三者照样被截断；含有它们的项要写在 `"…"` 里（登记，M3-D3）。本文不改节的解析器：它是全部节共用的，`redact.rs` 也照它读；让它认 `'` 会改变别的节，例如名字里有撇号的策略行 `Bob's HK = ss, … # note` 的注释从此去不掉。手册唯一的 jq 例子（`'.headers |= with_entries(select(.key | test("^X-") | not))'`）不受影响；jq 的 `//` 运算符受影响，归 M3c。

### 4.3 脱敏

`redact.rs` 不改（M3-D4）。它不分节、每一行同样地读；这几个节的行照阶段 1 的裁定照写的显示（`docs/api/phase1.md`「有意照原样显示的」：`[URL Rewrite]`、`[Body Rewrite]` 各行的文字，`[Header Rewrite]` 写进去的头值，`[Map Local]` 的 `header=` / `data=`），不分节的那几条规矩（URL 里的用户名口令、`password=` 一类的参数、不是那六种 scheme 的整个 URL）照旧作用于它们。`CLAUDE.md`「一行怎样拆成各项」要两者一起改，是为了解析器读成秘密的东西脱敏也藏得住；这几个节没有解析器读成秘密的项，要不要藏、怎样藏是所有者的 Q2。所以新的切法不改变脱敏藏什么：加 `split_words` 的提交在说明里写这一段理由，并加一条脱敏的用例钉住——这几个节各一行（两种引号都有），`redact_profile` 的输出与今天相同。

### 4.4 `[Replica]`

`[Replica]` 是认得的节：它的行都忽略，节头一处给一条提示 `I0005`："[Replica] was removed in Surge Mac 5.4.0; the section is ignored"（手册 `profile/module.html`："The legacy `[Replica]` section used by the HTTP capture feature was removed in Surge Mac 5.4.0"）。不再报未知的节（`W0002`），也不进"解析并保留"的名单（`W0016`）：它以后也不会有效果（M3-D5）。

## 5. 正则（M3a；正文的一半在 M3b）

### 5.1 三处用法

- **URL 的式子**：四个节每一行的第一个正则，比的是一个 URL；
- **`header-replace-regex` 的式子**：比的是一个头的值；
- **Body Rewrite 的式子**（M3b）：比的是整个正文，多行。

前两种用 `rurge_config::rule::Pattern`（fancy-regex，与 `URL-REGEX` 规则同一个类型），不开多行（URL 与头的值都是一行，总设计第 8.4 节）；第三种用 `BodyPattern`（第 5.4 节）。三种在加载时编译，编译不过的行告警并跳过（`W0012`，消息带正则包给的原因）。

### 5.2 编译之前的改写

`translate` 在编译之前把下面几种写法改写成两个正则包都认得、意思与 ICU 相同的写法（M3-D6）。它由 `Pattern::new` 与 `BodyPattern` 共用，**`URL-REGEX` 规则因此也改写**，经 `Pattern::new` 的策略组参数 `policy-regex-filter` 与 `policy-priority`（`spec/group.rs`）同样（总设计第 3.3 节"规则与重写不用两套读法"；登记）：

| 写法 | ICU 的意思（用户指南） | 不改写时：fancy-regex 0.14 / regex 1.13（读的是 `parse_escape` 与 `regex-syntax` 0.8.11 的源码） | 改写成 |
| ---- | ---------------------- | ------------------------------------------------------------------------------------ | ------ |
| `\h` / `\H` | 水平空白：`\p{Zs}` 与制表符 / 它的补集 | 十六进制数字 / 它的补集；编译不过 | `[\t\p{Zs}]` / `[^\t\p{Zs}]` |
| `\v` / `\V` | 七个行终止符 `\n` `\x0B` `\f` `\r` `\x85` ` ` ` ` 之一（不配 `\r\n` 这个序列）/ 它的补集 | `\v` 在两者里都是竖制表符 `\x0B`；`\V` 两者都编译不过 | `[\n\x0B\f\r\x{85}\x{2028}\x{2029}]` / 补集 |
| `\R` | 上面七个之一，或 `\r\n` | 两者都编译不过 | `(?:\r\n\|[\n\x0B\f\r\x{85}\x{2028}\x{2029}])` |
| `\<` / `\>` | 字面的 `<` / `>`（反斜杠引用它后面的一个字） | 两者都是词的开始 / 结束 | `<` / `>` |
| `\Q…\E` | 中间照字面（没有 `\E` 时到末尾） | 两者都编译不过 | 中间每个字转义 |
| `\Z` | 输入的末尾，或最后一个行终止符之前 | 两者都编译不过 | `(?=(?:\r\n\|[\n\x0B\f\r\x{85}\x{2028}\x{2029}])?\z)`（向前看：只有 fancy-regex 编译得了） |

- `translate` 把反斜杠与它后面的一个字当一体读（`\\h` 不动），认得字符类 `[…]`（含嵌套、`[]…]` 与 `[^]…]` 开头的 `]`、类里的转义）与 `\Q…\E` 的范围。
- 字符类里：`\h`、`\v` 换成它们的成员（不带方括号）；`\<`、`\>` 换成 `<`、`>`；`\Q…\E` 同样逐字转义；`\H`、`\V`、`\R` 在类里不支持，返回 `Err`，这一行 `W0012`、跳过。
- 别的 ICU 才有的写法（`\X`、`\N{…}`、`(?w)` 等）不改写：编译不过就 `W0012`、跳过（登记）。
- `.` 不改写：ICU 的 `.` 不配那七个行终止符，`regex` 的 CRLF 模式不配 `\r` 与 `\n`，fancy-regex 只不配 `\n`（登记）。

### 5.3 替换模板

URL Rewrite 的替换、`header-replace-regex` 的模板、Body Rewrite 的替换共用 `Template`，照 swift-corelibs-foundation 的 `replacementString` 读（M3-D7；总设计第 3.3 节的"照 `NSRegularExpression` 的模板"，第 19 节第 2 条细化了它）：

- `\` 被去掉，它后面的一个字照字面、不再参与展开（`\$1` 是字面的 `$1`，`\\` 是 `\`，`\n` 是 `n`）；结尾单独的 `\` 被去掉；
- `$` 后面最多读 D 位数字，D 是分组数（不含第 0 组）的十进制位数、至少 1——分组数不超过 9 时只读一位，10 ～ 99 时两位，依此类推；读到的数是组号，`$0` 是整个匹配；组号大于分组数、或那一组没有参与匹配时得空串；
- `$` 后面不是数字时是字面的 `$`。`${name}` 不认：`$` 与 `{name}` 照字面（登记）；
- 一次匹配展开一次。不用两个正则包自己的展开：它们把 `$1abc` 读成名为 `1abc` 的组（得空串），不认 `\$`。

例：两个分组时，`$12` 是第 1 组接着字面的 `2`，`$3` 是空串，`a\$b` 是 `a$b`。

### 5.4 两种引擎（Body Rewrite 的式子，M3b）

- 先 `translate`，再用 `regex::RegexBuilder` 以 `multi_line(true)`、`crlf(true)` 编译：`^` `$` 在每一行的首尾匹配、把 `\r` 与 `\n` 都当行尾、不在 `\r\n` 中间匹配，`.` 不配 `\r` 与 `\n`（总设计第 8.4 节"行尾按 CRLF 读"）。编译得了就是 `Plain`（M3-D8）。
- 编译不了的（向前向后看、反向引用、原子组、改写过的 `\Z`）在前面加 `(?m)`，用 `fancy_regex::RegexBuilder` 编译，回溯上限设为 `BODY_BACKTRACK_LIMIT` = 100 000 000，是 `Fancy`。**这一种读不了 CRLF**：fancy-regex 0.14 的多行 `^` `$` 只认 `\n`，`\r` 算在行里，`.` 也配 `\r`（登记）。
- 两者都编译不了：`W0012`，消息带 fancy-regex 的原因，这一行跳过。
- 上限取 1 亿的理由：fancy-regex 0.14 的回溯机里每个失败的起点都计一次回溯，缺省的 100 万在大约 1 MB 以上的正文上不论式子怎样都会超；1 亿约是最大的正文（32 MiB）的三倍，另留真回溯的余量。M3b 量一次最坏情形的耗时、写进计划的记录。两个包编译之后的大小上限照各自的缺省，超过的同样编译不过。

### 5.5 匹配与替换

- URL 的式子在 URL 上求一次匹配（要分组时取第一个匹配的分组）；`header-replace-regex` 在每一个同名的头的值上做。
- 每处替换都**替换全部不重叠的匹配**，从左到右（总设计第 3.3 节）；逐个取匹配（`captures_iter`）、自己拼结果。**不用 fancy-regex 的 `replace_all`**：它遇到回溯超限会 panic。
- fancy-regex 回溯超限时：这一条改写跳过、请求照常走、留备注（第 8.5 节）；`URL-REGEX` 规则一侧照阶段 1 按不匹配。
- 空的匹配（`^$`、`^`）照两个包的做法：式子允许的每个位置一个，空匹配之后往前挪一个字再找。

## 6. URL Rewrite（M3a）

### 6.1 读法

一行是 `正则 替换 [模式]`，两项或三项，多了少了都是 `W0012`、跳过。模式是 `header`（省略时）、`302`、`307`、`reject`，不分大小写（ASCII），别的 `W0012`。正则是 `Pattern`；替换是 `Template`，`reject` 模式不读它（习惯写 `_`）。替换里写着 `{{{GATEWAY_ADDRESS}}}` 时加载时就记下；只有 `302`、`307` 模式替换它，`header` 模式里它是普通的字（手册 "In 302 and 307 modes, the replacement may contain the placeholder"）。

### 6.2 依次求值与三个候选的 URL

规则按配置里的顺序逐条求值（M3-D9）。每一条依次比这个请求的候选 URL：

1. **请求的 URL**：`prepare` 给的那一个（规则与记录看到的就是它，第 8.1 节"当前的 URL"在 `header` 生效之前就是它）；
2. **用 `Host` 头重建的**：请求的 scheme、`://`、`Host` 的值、请求的路径与查询串——请求有 `Host` 头、且结果与第 1 个不同时才有；
3. **用连接的主机名重建的**：scheme、`://`、连接的目标（主机，端口不是 scheme 的缺省时带上）、路径与查询串——与前两个都不同时才有（手册 "Besides the request-line URL, Surge also tries matching with the URL rebuilt from the `Host` header value and from the underlying connection's hostname."）。

第一个匹配上的候选就是这条规则用的：结果是这个候选里全部匹配被替换之后的文字。之后：

- `header`：**只有第一条有效的 `header` 规则生效**（手册 "only the first one is applied"）。它生效之后，当前的 URL 就是新的 URL，此后的规则只比它——请求已经去新的 URL 了，另两个候选不再有意义；之后的 `header` 规则一概跳过。结果不合法而被丢弃的一条不算"第一条"，下一条匹配的 `header` 规则照样能生效（登记）。
- `302`、`307`、`reject`：第一条匹配的答复这个请求，之后的规则、Map Local 与 Body Rewrite 都不再走。

讲 HTTP/2 的客户端的请求通常没有 `Host` 头（URL 本来就由 `:authority` 重建，M2b），候选 2 不出现。

### 6.3 `header`

- **合法的新 URL**：能读成一个绝对的 URI（`http::Uri`），scheme 是 `http`、`https`、`ws`、`wss` 之一（不分大小写），主机不空，写了端口时在 1 ～ 65535。不合法的丢弃这条改写、留备注（手册 "The rewritten URL must be a valid `http`, `https`, or `ws` URL, otherwise the rewrite is discarded."；`wss` 是 `ws` 的 TLS 形式，总设计第 2.3 节 3.13）。用户名口令不用。
- **改写之后的请求**（M3-D28）：
  - **请求保持它来时的形式，只换掉里面的 URL**：源站形式的（嗅探到的 HTTP/1.1 请求）请求目标换成新 URL 的路径与查询串；URI 本来是绝对的——经 HTTP 监听器来的请求，与讲 HTTP/2 的客户端的请求（hyper 由伪头拼出绝对的 URI）——换成新的 URL 本身，`ws` / `wss` 写作 `http` / `https`（升级请求的请求行是 `http` 的 URL），不带用户名口令。发出的形式照旧由 M1b 与 M2b 定：经监听器来的照 `may_forward` 改成源站形式或绝对形式（`origin_form` / `absolute_form`，两者都从 URI 取 authority），版本之间由 `fit` 转换（`Form::of` 先从 URI、再从 `Host` 取 authority）。所以绝对的 URI 不能换成源站形式：那样 `absolute_form` 拼出的是 `http://` 加空的 authority；
  - `Host` 换成新 URL 的 authority：主机，端口不是 scheme 的缺省（`http` / `ws` 80，`https` / `wss` 443）时带上。源站形式的请求没有 `Host` 时加上；讲 HTTP/2 的请求没有 `Host` 时不加（`:authority` 随 URI）；
  - 拨号的目标是新 URL 的主机与端口，**策略不重选**（"After a policy is selected…"；`retarget`，第 8.2 节）；
  - 对上游的 TLS 由新的 scheme 定：`https` / `wss` 走 TLS，`http` / `ws` 不走，哪怕这是解密的连接（第 8.2 节）；
  - 嗅探到的连接上，换了目标的请求不再带客户端的 `Proxy-Authorization`：M1b 在嗅探到的连接上留着它（`Keep.proxy_authorization`：那是客户端给它自己的代理的，不是给 rurge 的），目标换了就不发给客户端没有指名的主机（登记）。
- 规则不再求一次：会话信息（`SessionInfo`）不变，`smart` 组照旧以规则看到的名字记账（`dial_route`）。记录的 `url` 留着请求来时的 URL，备注写新的 URL、去掉查询串（第 8.5 节）。

### 6.4 `302` / `307` 与网关地址

- 应答只有状态码 302 或 307、`Location` 与 `Content-Length: 0`（总设计第 2.3 节 3.13）。`Location` 是展开之后的替换；它不能当头的值（含 CR、LF、NUL 等）时这条规则丢弃、留备注、接着求下一条。别的不查（可以是相对的 URL 或 App 的 scheme）。
- `{{{GATEWAY_ADDRESS}}}`：在展开之后、每一处都换成当前出口的 IPv4 缺省网关（`Upstreams::gateway()`，引擎读网络环境的快照 `NetworkFacts.primary.routers`，IPv4 的那个在前）。没有主出口、或主出口没有 IPv4 网关时这条规则放弃、留备注、接着求下一条（总设计第 2.3 节 3.13；只取 IPv4，登记，M3-D10）。
- 连接照常保持（keep-alive）；HTTP/2 上就是这个流的应答。记录：状态码 302 / 307、`Completed`、没有上游。

### 6.5 `reject`

同 `REJECT` 策略对引擎里一个请求的答复（M1 设计第 8.8 节）：`show-error-page-for-reject` 开着给 403 的错误页，关着就关掉连接（HTTP/1.1）或重置这个流（HTTP/2）。记录是 `Rejected(Reject)`，另有备注说是 URL Rewrite 拒绝的（策略一栏照旧是规则选的那一个）。

## 7. Header Rewrite（M3a）

### 7.1 读法

一行是 `[方向] 正则 动作 名字 [值 …]`：

- **方向**：第一项是 `http-request` 或 `http-response`（不分大小写）时是方向；否则方向是请求、第一项就是正则（手册 "this part can be omitted, which means `http-request`"）。
- **动作**：`header-add`、`header-del`、`header-replace`、`header-replace-regex`，不分大小写；别的 `W0012`。
- **名字**：合法的字段名（RFC 9110 的 token）；以 `:` 开头的（HTTP/2 的伪头）不合法；不合法的 `W0012`。
- `header-del 名字`：名字之后不能再有东西，否则 `W0012`。
- `header-add` / `header-replace`：**值是名字之后这一行余下的部分**（节的解析器留下的文字，去掉首尾空白）；它整个是一项、且整个在引号里时去掉引号（第 4.1 节）。没有值是 `W0012`（空值写作 `""`）；值里不能有 CR、LF、NUL 与制表符以外的控制字，否则 `W0012`（M3-D11）。理由：头的值常有空格（`User-Agent`），手册的例子没有引号；"余下的部分"让两种写法都对。
- `header-replace-regex 名字 正则 模板`：名字之后恰好两项（含空白的要加引号）；正则是 `Pattern`。
- 名字是 `Content-Length` 或 `Transfer-Encoding` 的行：保留，另给 `W0041`（第 7.3 节）。

### 7.2 四种动作

照手册与总设计第 2.3 节 3.15（M3-D12）：

- `header-add`：追加一行，同名的已经有也追加（"even if the header field already exists"）；
- `header-del`：删掉全部同名的行；
- `header-replace`：全部同名的行换成这个值；一行也没有时什么也不做（"If the header field doesn't exist, nothing happens."）；
- `header-replace-regex`：在每一个同名的行的值里，全部匹配换成模板展开的结果；一行也没有时什么也不做。结果不能当头的值时那一行的值不变、留备注。

名字不分大小写。多条规则按顺序作用，后一条看到前一条的结果。请求一侧比的 URL 是请求的 URL（Header Rewrite 在 URL Rewrite 之前）；应答一侧比的是当前的 URL（URL Rewrite 之后，第 8.1 节）。

### 7.3 框架头

一个方向的规则作用之前与之后，比较 `Content-Length` 与 `Transfer-Encoding` 两个名字的全部行（值与次序）。有任何不同（加了、删了、改了）时，这个请求失败（手册 "If a rewrite changes the `Content-Length` or `Transfer-Encoding` headers, the modification is rejected and the request fails."；总设计第 2.3 节 3.15，M3-D13）：

- 请求一侧：请求不发往上游；`show-error-page` 开着给 502 的错误页，关着就关掉连接（HTTP/1.1）或重置这个流（HTTP/2）；记录 `Failed`，原因 "Header Rewrite changed Content-Length or Transfer-Encoding"，另有备注。
- 应答一侧：丢掉上游的应答（它的连接不再借出：HTTP/1.1 的关掉，共享的 HTTP/2 连接上重置那个流），客户端同上得到 502 或关闭，记录同上。
- 加载时：写明这两个名字的行给 `W0041`、行照样保留——`header-del Transfer-Encoding` 对一个没有它的消息什么也不改，这一行本身不算错。

### 7.4 作用于哪些头

- **请求**：客户端的请求头，在选定策略之后、M1b 的 `headers::strip` 之前。所以对逐跳的头（`Connection`、`Keep-Alive`、`Proxy-Connection`、`TE`、`Trailer`、`Upgrade` 与 `Connection` 里点名的）的改动照旧被 `strip` 去掉，到不了上游（登记）；删掉 `Upgrade` 的规则让这个请求成为普通的请求（要不要升级在重写之后才读）。
- **`Host`**（M3-D30）：改 `Host` 的规则改的是发往上游的 `Host`（对 HTTP/2 的上游是 `:authority`），不改拨号的目标与 TLS 的 SNI（目标是连接的目标，总设计第 6.3 节；换目标用 URL Rewrite 的 `header`）。第 9 步会按请求目标重写 `Host`——经监听器来的请求在 `origin_form` 里取请求行的 authority，`fit` 从 URI 的 authority 取——所以引擎记下改过的值、在第 9 步的最后写回：
  - 请求一侧的规则作用之后，`Host` 的各行与之前不同、且还剩至少一行时，改过的值是最后一行（`header-add` 追加的就在最后），请求里只留这一行；
  - 第 9 步的 `fit` 之后：对 HTTP/1.1 的上游设 `Host`；对 HTTP/2 的上游换掉 URI 的 authority（`:authority`）、去掉 `Host`，读不成 authority 时不换、留备注；以绝对形式交给转发的 HTTP 代理时不写回（RFC 9112 第 3.2.2 节：代理按请求行换掉 `Host`，登记）；
  - 删掉了全部 `Host` 时放回原来的各行、留备注（HTTP/1.1 的请求必须有 `Host`，RFC 9112 第 3.2 节，登记）；
  - 第 5 步的 `header` 生效时 `Host` 由新的 URL 定（第 6.3 节），此前改过的值不再写回：后一步胜；
  - 改过的 `Host` 也是 URL Rewrite 的候选 2 用的（第 6.2 节）。
- **讲 HTTP/2 的客户端的请求**：hyper 给的头表（没有伪头，`Host` 通常没有，`cookie` 可能分成几行——M2b 对 HTTP/1.1 的上游合并它，Header Rewrite 在那之前，看到的是客户端发来的样子）。所以 `Host` 的 `header-replace`、`header-replace-regex` 与 `header-del` 在这样的请求上什么也不做（没有那一行），`header-add Host` 加上一行、照上一条写回（登记）。
- **应答**：上游的应答头经 `headers::strip(resp, Keep::NONE)` 之后、决定缓冲正文之前；101 的头也算。rurge 自己的答复（错误页、`302` / `307`、Map Local、`REJECT` 的页）不过应答一侧的重写。

## 8. 引擎里的位置（M3a；M3b 加两步）

### 8.1 每个请求

M1b 的 `Requests::handle` 加上 M3 的几步（M3-D14）：

```
0  HTTP/2 上的 CONNECT → 501                      （M2b，不变）
1  prepare：会话信息与 URL                          （不变）
2  open：句柄与 Guard                              （不变）
3  route：规则与策略；拒绝的在这里答复                （不变）；Go 多带 rewrite（选定 REJECT 一族的为 None）
—— rewrite 为 None 时 4 ～ 7、11、12 全部跳过；不过重写的另两种见第 8.4 节
4  Header Rewrite（请求）                           框架头变了 → 502
5  URL Rewrite                                     302 / 307 / reject 在这里答复；header 换目标
6  Map Local（M3b）                                 命中就答复，不连上游
7  Body Rewrite（请求，M3b）                         超过 32 MiB 断开；chunked 与 Expect 不改写
8  借或拨上游；REJECT 一族在这里被拒绝               （不变；池的键见第 8.3 节）
9  strip、请求目标的形式、fit，发出                   （不变；改过的 Host 在 fit 之后写回，第 7.4 节）
10 应答头到达：strip                                （不变）
11 Header Rewrite（应答）                           框架头变了 → 502
12 Body Rewrite（应答，M3b）                         超过 10 MiB 原样流过
13 Tail：交给客户端，记录                             （不变）
```

**当前的 URL**：从第 1 步的 URL 起，第 5 步的 `header` 生效时换成新的 URL；第 6、7、11、12 步比的都是它（总设计第 2.3 节 3.1："Map Local 比的是 URL Rewrite 之后的 URL"）。

**要升级的请求**（WebSocket）：第 4、5、6 步照做（只有头）；第 7、12 步不做（没有正文，101 之后是透传的字节）；第 11 步作用于 101 的头。

### 8.2 换目标与对上游的 TLS

- **`retarget`**：`Upstreams::retarget(route, key, target, forwardable)` 交回新的 `route` 与 `key`：同一个策略、同样的尝试，目标换成新的，`may_forward` 换成 `forwardable`。`forwardable` 由 `rurge-http` 算（M3-D28）：经 HTTP 监听器来的请求、新的 scheme 是 `http` / `ws` 时为真，别的都为假——嗅探到的连接上的请求照 M1b 恒为假（`prepare` 按入口定，它们从不以绝对形式转发），`https` / `wss` 要经隧道再走 TLS。引擎的实现换掉 `Way` 的 `target`、`may_forward` 与 `PoolKey` 的 `target`、`may_forward`，`attempts`、`book`、代次都不动；不再求规则（总设计第 8.2 节"策略不重选"，M3-D15）。新的主机不问 fake 地址池：它是规则写出来的名字或地址；也不按 `use-local-host-item-for-proxy` 查 `[Host]`（`route_session` 只为原来的目标查过，登记）。
- **`gateway`**：引擎读 `EngineShared` 里网络环境的快照（`shared.network`），取主出口的第一个 IPv4 网关。
- **每个请求的对上游 TLS**（M3-D16、M3-D31）：缺省是连接的（`Conn.origin`，解密的连接上是 `Some`）；`header` 生效之后由新的 URL 定：
  - `https` / `wss`：TLS，以新的主机校验与发 SNI（名字发 SNI 并按它校验，IP 不发 SNI、按 IP 的 SAN 校验，同 M2 设计第 9.2 节）。配置分两种：**解密的连接上用连接自己的那一份**——握手时那一代的 `OriginTls` 的 `config` 与 `h1`（要升级的请求用 `h1`，M2b 计划 P12）和它的握手时限 `wait`，与这条连接上没有改写的请求同一份；**别的连接**（嗅探到的明文、经监听器来的）用这一代 `Rewrite` 的两份客户端配置——`origin_client_config(roots, skip, alpn)`，`skip` 是 `[MITM] skip-server-cert-verify`，ALPN 照 M2b（`[MITM] h2 = true` 时给 `h2` 与 `http/1.1`，否则只给 `http/1.1`），要升级的请求用只给 `http/1.1` 的那一份，握手的时限 30 秒（同 `rurge-engine` 的 `HANDSHAKE_TIMEOUT`）；
  - `http` / `ws`：明文，哪怕连接是解密的。
- **不要 CA**：`Rewrite` 自己建这两份配置，只用 `[MITM]` 的 `skip-server-cert-verify` 与 `h2`（没有 `[MITM]` 时都是假）和 M2 的根（同 `build_mitm`：`EngineShared.roots` 有值时用它，否则在 `spawn_blocking` 里取 `union_root_store()`）。所以明文的 `http://` 改写成 `https://` 不需要 MITM 的 CA。建不成（`BuildError`）时告警，这一代没有重写（同 `build_mitm`）。
- 对上游的握手或校验失败：502 与原因，同 M2 设计第 9.2 节。

### 8.3 池的键

`rurge-http` 的池与共享的 HTTP/2 连接都改用 `(K, OriginKey)` 作键：`K` 照旧由 `Upstreams` 给（`header` 生效时由 `retarget` 给），`OriginKey` 是"明文"或"TLS 与校验用的名字"，由 `rurge-http` 自己定（M3-D17；M2 设计第 9.4 节留给 M3 的那一件）。理由：重写能改 scheme 与名字之后，同一条客户端连接上的两个请求可能以不同的方式到同一个目标——例如 `http://1.2.3.4:443/` 与解密的连接上一个 SNI 是 `a.test`、目标是 `1.2.3.4:443` 的请求。同一个键下的 TLS 连接都是用同一份配置拨的，所以可以互借（M3-D31）：解密的连接上，没有改写的与改写过的请求都用连接握手时那一代的配置；别的连接上只有改写过的请求走 TLS，用的是这一代 `Rewrite` 的配置，而 `route` 从同一个快照取 `Rewrite` 与 `K` 里的代次，同一个 `K` 下也就是同一份。**解密的连接上不用请求那一代的配置**：连接的配置是握手时那一代的，`K` 里的代次是每个请求选路时读的，重载改了 `skip-server-cert-verify` 之后同一个键下会有用两份配置拨的连接，要分开就得把配置放进 `OriginKey`（构建者检查的第 4 个问题）。

### 8.4 不过重写的请求

- 嗅探到的连接里客户端发来的 `CONNECT` 与绝对形式的请求（请求行是绝对形式的 HTTP/1.x 请求；M1b 把两者照原样转给连接的目标，M1 设计第 8.9 节）：客户端在对它自己的代理说话，不是对源站的请求（M3-D29）。讲 HTTP/2 的请求不算：hyper 给的 URI 总是绝对的，那是它的 `:authority`；
- `route` 拒绝了的请求（暂停、DNS 失败、求值失败，在第 3 步答复）；
- **选定的策略是 REJECT 一族的请求**（M3-D27）：REJECT 是拨号时才定的——`REJECT` 一族的出站在 `connect_tcp` 里答拒绝，`dial_route` 经 `escalation` 升级（M1b 计划 P3）——所以 `route` 给的是 `Routed::Go`。`ConnUpstreams::route` 看第一个尝试的终点（`Way.attempts` 第一个的 `Resolution.terminal` 是 `TerminalKind::Reject`：REJECT 一族本身，以及照今天落到 REJECT 的，例如 rurge 不支持的协议、没有成员又不代以 DIRECT 的组），是就给 `rewrite: None`：这个请求照 M1b 在第 8 步被拒绝，不过任何重写（Header Rewrite 的 502、`302`、Map Local 都不会代替拒绝，M3b 的请求正文也不会先缓冲），不算进计数。拒绝不挪到选路时，升级照旧在拨号时做。拨号时才出现的拒绝不算：出站答 `Unsupported`、`evaluate-before-use` 的组测完之后改选了 REJECT，那时请求已经过了重写（登记）；
- 开关关着、或这一代四个节都没有可用的行（`Routed::Go.rewrite` 为 `None`）。

### 8.5 记录与备注

- 记录的 `url` 照旧是请求来时的 URL（规则与记录一致）；`statusCode` 是客户端拿到的状态码。
- **备注**（`SessionHandle::add_note`，经 `/v1/requests/*` 的 `notes` 出去）：每个起了作用的改写一条、每个被跳过的改写一条（总设计第 8.1 节）。英文；写节、动作与这条规则的 URL 式子（照写的）；备注里的 URL 不带查询串（`scheme://主机[:端口]/路径`）；**头的值、正文、Map Local 的数据与网关的地址从不进备注**（M3-D18）。措辞在计划里定稿，形如：
  - `URL Rewrite: header by ^http://www\.google\.cn -> http://www.google.com/`
  - `URL Rewrite: 302 by ^http://yachen\.com`、`URL Rewrite: reject by ^http://ad\.com/ad\.png`
  - `URL Rewrite skipped: ^…: the rewritten URL is not a valid http, https, ws or wss URL`、`URL Rewrite skipped: ^…: there is no IPv4 gateway`
  - `Header Rewrite: request header-add DNT by ^http://example.com`
  - `Header Rewrite failed: ^…: Content-Length or Transfer-Encoding changed`
  - `Header Rewrite skipped: Host cannot be removed`、`Header Rewrite skipped: the rewritten Host is not a valid HTTP/2 authority`（第 7.4 节，每个请求至多一条，各计入 `skipped`）
  - `URL Rewrite skipped: ^…: the regular expression hit its backtracking limit`（另三个节同样的形状）
  - M3b：`Body Rewrite: response by ^…`、`Body Rewrite skipped: ^…: the body is not valid UTF-8`（与 `is over 10 MiB`、`content-encoding br is not supported`、`the request body is chunked`、`the request expects 100-continue`）、`Map Local: text by ^…`
- **日志**：M3a 不加每个请求的新行，会话日志行不变；M3b 只加 chunked 与 `Expect` 的那一行 WARN（第 14 节）。

## 9. Body Rewrite（M3b）

### 9.1 读法

一行是 `方向 正则 （式子 替换）…`：

- **方向**：`http-request` 或 `http-response`，必须有（手册 "starting with `http-request` or `http-response`"），不分大小写。`http-request-jq` / `http-response-jq` 的行给 `W0042`、忽略（到 M3c）。别的 `W0012`。
- 至少一对、项数成对，否则 `W0012`。式子是 `BodyPattern`（第 5.4 节），替换是 `Template`。

### 9.2 何时缓冲

**在读完头的那一刻定**（总设计第 6.2 节）：有这个方向的 Body Rewrite 规则的 URL 式子匹配当前的 URL，并且：

- **请求**：没有 `Transfer-Encoding`（chunked）、没有 `Expect: 100-continue`（手册 "Request body rewrite is not supported when the request uses `Transfer-Encoding: chunked` or `Expect: 100-continue`. Surge logs a warning and forwards the request unmodified."）；讲 HTTP/2 的客户端的请求有 `content-length`、或头之后就结束了（没有正文）——两者都不是时同 chunked（总设计第 8.4 节）；不是要升级的请求。没有正文的请求（`GET`）也缓冲：正文是空的，`^$` 能在上面生成正文（手册 "Even if the original request does not contain a body, new content may still be generated through body rewrite"）。
- **应答**：应答可以有正文：不是 `HEAD` 的应答，不是 1xx、204、304（M3-D19）。

不缓冲的那三种请求（chunked、`Expect`、HTTP/2 上长度不知道的）原样转发，留备注，日志照第 14 节。

### 9.3 上限

上限按线上的字节算：请求 32 MiB（33 554 432 字节），应答 10 MiB（10 485 760 字节）（手册的 32 MB 与 macOS 的 10 MB，取二进制的读法，M3-D20）：

- **请求**：`Content-Length` 已经超过时不读、立刻断开；读的过程中超过时断开。断开在 HTTP/1.1 上是关掉连接（手册 "the connection is dropped"），在 HTTP/2 上是重置这个流（登记）。记录 `Failed("Body Rewrite: the request body is over 32 MiB")`，计入 `failed`。
- **应答**：`Content-Length` 已经超过时根本不缓冲、照常流过；读的过程中超过时放弃修改：应答头（Header Rewrite 之后的样子）、已经缓冲的字节、其余的字节依次原样流过（手册 "Surge gives up the modification and streams the response through unmodified."）。留备注。
- **解压之后**：每解开一层，大小同样以这两个数为上限（防解压炸弹）；超过时不改写、原样转出原来的字节，留备注。请求不因此断开：它在线上的大小没有超。
- **等待**：缓冲时帧与帧之间照 M1b 的 `idle`；请求的令牌、终止与停止照常作用。

### 9.4 解压

`Content-Encoding` 的各项（逗号分隔；`identity` 不算；`x-gzip` 同 `gzip`）按相反的顺序逐层解开（总设计第 3.5 节）：

- `gzip`：flate2 的 `MultiGzDecoder`（解全部的成员）；
- `deflate`：头两个字节是合法的 zlib 头（`CM` 是 8，`(CMF × 256 + FLG)` 能被 31 整除）时按 zlib 解，否则按裸的 deflate 流解；
- `br`、`zstd`、`compress` 与别的：不改写、原样转发，留备注 `content-encoding <名字> is not supported`（`br`、`zstd` 到 M3c）。解码出错同样不改写、留备注。

### 9.5 改写

- 每条匹配的规则依次作用（"If a request hits multiple body rewrite rules, they are executed in sequence."）：正文必须是合法的 UTF-8（严格的），否则这条规则跳过、留备注（手册）；一条规则里的各对依次作用于上一对的结果；每对替换全部匹配（第 5.5 节）。
- 解压、改写与新正文的拼接在 `spawn_blocking` 里做（M3-D21）。`Plain` 是线性时间的；`Fancy` 回溯超限时这条规则跳过（正文回到它之前的样子）、留备注。
- 改写的结果为空也照样发出（`Content-Length: 0`）。

### 9.6 改过之后的头

最后的文字与解开之后的原文不同时：去掉 `Content-Encoding`，`Content-Length` 设为新的长度，去掉 `Transfer-Encoding`（由 hyper 按长度分帧），原来的 trailer 丢掉（登记）——不压回（总设计第 2.3 节 3.16）。没有任何改动时原样转出原来的字节（连同压缩），头不变（总设计第 8.4 节）。

## 10. Map Local（M3b）

### 10.1 读法

一行的第一项是 URL 的式子，其余每一项是 `键=值`（键不分大小写）：`data-type`、`data`、`header`、`status-code`。不是参数的项 `W0012`、跳过；不认识的键照 rurge 对未知参数的诊断（第 18 节 V13）、行保留；值不合法的 `W0012`、跳过（M3-D22）：

- **`data-type`**：`file`（缺省）、`text`、`tiny-gif`、`base64`，不分大小写。
- **`data`**：
  - `file`：必须有。以 `http://` 或 `https://` 开头的是 URL（第 10.3 节）；否则是路径：相对的接到主 Profile 所在的目录（`ParseCtx.base_dir`，手册 "relative paths being relative to the profile's directory"，从本地 include 的文件里来的行也一样），绝对的在三个平台都收（手册只说 macOS，登记）。**路径来自下载到的文本（`Reach` 不是 `Anywhere`）时给 `W0033`、这一行跳过**（`Reach::map_local_refusal`，`CLAUDE.md`「下载到的文本」）；URL 不受这一条限制。加载时不查文件在不在：每个请求时读。
  - `text`：值就是正文（UTF-8）；没有 `data` 时是空的（"You can use `data-type=text data=""` to return an empty result."）。
  - `base64`：加载时解码（标准字母表），解不开的 `W0012`。
  - `tiny-gif`：`data` 不读。
- **`header`**：值里有 `:` 时是 `a:b|c:d`——以 `|` 分成各对、每对在第一个 `:` 处分成名字与值、两边去掉空白、空的对忽略；没有 `:` 时是 Base64，解开是以 `\n`（`\r\n` 也收）分隔的头的行、每行 `名字: 值`、空行忽略（手册 "If the value contains no `:`, Surge treats it as base64."）。名字或值不合法、Base64 解不开的 `W0012`。其中的 `Content-Length` 与 `Transfer-Encoding` 不用、给 `W0041`（由引擎按数据算，总设计第 2.3 节 3.17）。
- **`status-code`**：200 ～ 999 的整数，缺省 200；以外的 `W0012`（"Values outside 200–999 are invalid."）。

### 10.2 何时答复

在第 8.1 节的第 6 步：**第一条**式子匹配当前的 URL 的 Map Local 规则答复这个请求（M3-D23），不连上游（"When a rule matches, Surge builds the response locally and skips the upstream request entirely."），请求的正文不读（照 hyper 的做法，同 M1b 的 `REJECT` 页）。它的应答不过第 11、12 步（总设计第 2.3 节 3.1）。开关关着时 Map Local 同样不生效（"only when the Rewrite feature is enabled"）。

### 10.3 四种数据

- **文件**：请求到达时打开（`tokio::fs::File`），那时读它的大小，流式读出、不进内存、不设上限（总设计第 2.3 节 3.17）。打不开：`show-error-page` 开着给 502，关着就关掉连接或重置这个流；记录 `Failed("Map Local: cannot read <路径>")`，计入 `failed`。读到一半出错：应答正文以错误结束（HTTP/1.1 关掉连接，HTTP/2 重置这个流），同上游正文出错（M1b、M2b）。
- **URL**：资源管理器里新的一类资源 `ResourceKind::MapLocal`（控制面的名字 `map-local`，与别的资源一起列出、可以按键更新，FR-CFG-14），它的下载、缓存、大小上限（缺省 64 MiB）与更新间隔都照资源管理器（手册说 iOS 与 tvOS 限 15 MB，rurge 的平台不限，登记）。请求读当时的那一份；还没有一份（第一次下载没做完或失败了）时同打不开的文件，原因是 "Map Local: the data from <来源> is not available yet"，URL 只显示到 `rurge_config::remote::origin`。
- **文本**：UTF-8 的字节。
- **Base64**：解开之后的字节。
- **`tiny-gif`**：`responses::tiny_gif()` 那一张 GIF 的字节（M1 的 `REJECT-TINYGIF` 同一张）。

### 10.4 应答的头

状态码取 `status-code`；头依次是 `header` 给的各行；没有给 `Content-Type` 时补上（手册「About Content-Type」）：文件与 URL 按路径（URL 取它的路径）的扩展名查一张自带的表（json、html / htm、js / mjs、css、png、jpg / jpeg、gif、webp、svg、ico、txt、xml、mp4 等常见的几十种，表在计划里），查不到是 `application/octet-stream`；`text` 是 `text/plain`；`tiny-gif` 是 `image/gif`；`base64` 是 `application/octet-stream`。`Content-Length` 是数据的长度（文件取打开时的大小）。别的什么也不加。记录：`statusCode`、`Completed`、没有远端地址；备注 `Map Local: <data-type> by <式子>`。

## 11. 功能开关（M3a）

- `rewrite`：`Engine` 里与 `mitm_on` 并排的一个原子的布尔 `rewrite_on`；`GET /v1/features/rewrite` 读它，`POST` 设它并写进 `state.json` 的 `features.rewrite`（第 2 版已经是三态，M2a）；启动时还原（`restore_switches`）。缺省（没设过时）开（总设计 P4-D19，M3-D24）。
- 切换立即对新的请求生效：每个请求在第 3 步读它（`Routed::Go.rewrite`），已经过了那一步的请求照它开始时的样子走完。
- 关着时四种一概跳过，Map Local 也在内（总设计第 6.2 节）。
- `capture` 照旧读作关、`POST` 答 501，到 M4；`scripting` 到阶段 5。

## 12. 计数与 API

`GET /v1/rurge/http` 在 `mitm` 之后多一个对象 `rewrite`，键的次序固定。M3a 就定下全部六个键，`body` 与 `mapLocal` 在 M3b 之前一直是 0（M3-D25）：

```json
{"sniffed":{"tls":0,"http":0,"other":0,"timeout":0,"closed":0},"requests":0,"mitm":{"decrypted":0,"untrusted":0,"failed":0,"noRequest":0},"rewrite":{"url":0,"header":0,"body":0,"mapLocal":0,"skipped":0,"failed":0}}
```

- `url`：有 URL Rewrite 规则起了作用的请求（四种模式都算）；
- `header`：头被 Header Rewrite 改过的请求（两个方向，一个请求只计一次）；
- `body`：正文被 Body Rewrite 改过的请求（一个请求只计一次）；
- `mapLocal`：由 Map Local 答复的请求；
- `skipped`：留了备注而跳过的改写，每一个计一次；
- `failed`：因为重写而失败的请求（框架头变了、Map Local 的数据读不到、请求正文超过上限）。

自进程启动起累计，重载不清零：计数器是 `rurge-http` 的 `RewriteCounters`（原子的数），一个引擎一份，放在 `EngineShared` 里——每一代的 `Runtime` 由它建，`Rewrite::build` 在那里拿到计数器（M2 的 `mitm` 计数在 `Engine` 里，那是 `serve.rs` 自己数的）；`Engine::http_counts()` 读它。`/v1/features/rewrite` 见第 11 节。没有别的新端点。

## 13. 诊断

| 码 | 什么时候 | `codes::hint` 里的决定 |
| -- | -------- | ---------------------- |
| `W0012` | 四个节读不了的行：项数不对，模式、方向、动作、`data-type` 不认识，正则编译不了（消息带正则包的原因），字段名或值不合法，`status-code` 在 200 ～ 999 之外，Base64 解不开；这一行跳过 | 已有 |
| `W0041`（新，M3a） | `[Header Rewrite]` 写明 `Content-Length` 或 `Transfer-Encoding` 的行（行保留，改到它们的请求失败）；`[Map Local]` 的 `header` 里的这两个名字（不用，M3b） | 每处自带：两处的意思不同 |
| `W0042`（新，M3b） | `http-request-jq` / `http-response-jq` 的行："jq body rewrite is not available in this build of rurge; the line is ignored" | 不设：消息已经说了这一行没有效果，Profile 里除了删掉它没有别的可改 |
| `I0005`（新，M3a） | `[Replica]` 的节头（第 4.4 节） | 不设：提示级，没有要改的 |
| `W0033` | `[Map Local]` 的文件路径来自下载到的文本（`map_local_refusal`，M3b） | 已有 |
| `W0016` | 不再列这四个节（M3a 两个、M3b 两个） | — |

重写的行从不让加载失败：没有新的 `E` 级码。消息与节、行的对应在计划里定稿（M3-D26）。

## 14. 日志与凭据

- 没有新的带凭据的键，`crates/rurge-config/tests/secrets.rs` 不改：这几个节的行照阶段 1 的裁定照写显示，要不要藏是所有者的 Q2（第 4.3 节）。
- 备注照第 8.5 节；日志从不写查询串、头的值、正文、Map Local 的数据与网关的地址（总设计 P4-D25；网关属于"网络"一类，`CLAUDE.md`「日志的分级」）。网关的地址只出现在 `302` / `307` 的 `Location` 里——那就是这个功能本身。
- M3b 的那一行 WARN（chunked 与 `Expect` 的请求正文不改写）：一代配置里每条规则每小时至多一行 WARN，其余的在 DEBUG；WARN 里只写这条规则的式子与原因，不带主机与 URL；同一个请求另起一行 DEBUG，带主机（经虚拟网卡来的会话的名字只在 DEBUG，`CLAUDE.md`「日志的分级」与 M2 设计第 15 节）。
- 用户流量：记录照旧带完整的 URL（含查询串），总设计 Q1 归所有者；备注里的 URL 去掉查询串。

## 15. 行为的变化（登记）

M3 的文档任务把下面各条写进兼容性清单（`docs/surge-compatibility-matrix.md`，多数在 7.3 ～ 7.6 节，第 1、2 条另在规则的 `URL-REGEX` 一行），没有观察过 Surge 的实际行为的照实写：

1. `URL-REGEX` 规则、策略组的 `policy-regex-filter` 与 `policy-priority`、四个节的正则在编译之前改写 `\h`、`\H`、`\v`、`\V`、`\R`、`\<`、`\>`、`\Q…\E`、`\Z`（第 5.2 节）：`\h`、`\H`、`\v`、`\<`、`\>` 在 `URL-REGEX` 与那两个参数里的意思从此与以前不同（以前是十六进制数字、竖制表符与词的边界），`\V`、`\R`、`\Q…\E`、`\Z` 从编译不了变成编译得了。
2. `.` 与多行的 `^` `$` 认的行终止符与 ICU 不同：`regex` 一侧只认 `\r` 与 `\n`；Body Rewrite 里落到 fancy-regex 的式子（向前向后看、反向引用等）不认 CRLF（第 5.4 节）。ICU 才有的别的写法编译不了就跳过这一行。
3. 替换模板：`${name}` 不认；结尾单独的 `\` 被去掉（第 5.3 节）。
4. 一行的切法：`"…"` 里认 `\"` 与 `\\`；`'…'` 里的 ` #`、` ;`、` //` 照样开始行内注释（第 4.1、4.2 节）。
5. `header-add` / `header-replace` 的值是这一行余下的部分，不加引号也能有空格（第 7.1 节）。
6. Header Rewrite 对逐跳的头的改动到不了上游；改过的 `Host` 在以绝对形式交给转发的 HTTP 代理时不生效，删掉 `Host` 不生效，URL Rewrite 的 `header` 生效时 `Host` 由新的 URL 定；讲 HTTP/2 的请求通常没有 `Host`，`header-replace` 一类的 `Host` 规则在它上面什么也不做（第 7.4 节）。
7. URL Rewrite：每条规则比当前的 URL，`header` 生效之后只比新的 URL；结果不合法的 `header` 规则不算"第一条"；`wss` 也算合法；`header` 模式里的 `{{{GATEWAY_ADDRESS}}}` 是普通的字；网关只取 IPv4，没有时规则放弃；换了目标的请求，新的主机不按 `use-local-host-item-for-proxy` 钉住，嗅探到的连接上不带客户端的 `Proxy-Authorization`（第 6、8.2 节）。
8. `302` / `307` 的应答只有 `Location` 与 `Content-Length: 0`（第 6.4 节）。
9. 嗅探到的连接里客户端发来的 `CONNECT` 与绝对形式的请求不过任何重写；选定的策略是 REJECT 一族的请求不过任何重写（Map Local 与 `302` / `307` 不代替拒绝），拨号时才出现的拒绝（出站答 `Unsupported`、`evaluate-before-use` 的组测完之后改选）在重写之后（第 8.4 节）。
10. Body Rewrite：两个上限按 MiB；HTTP/2 上请求超限重置这个流而不是断开连接；HTTP/2 上没有 `content-length` 又有正文的请求同 chunked；解压之后的大小同样受上限；`HEAD` 的应答与 1xx、204、304 不改写；改写过的正文丢掉 trailer；chunked 与 `Expect` 的 WARN 每条规则每小时至多一行（第 9 节）。
11. Body Rewrite 的 `br` 与 `zstd` 不改写、jq 的行告警并忽略，到 M3c（第 9.1、9.4 节）。
12. Map Local：绝对路径在三个平台都收；相对路径一律相对主 Profile 的目录；第一条匹配的规则答复；URL 的数据照资源管理器的上限（缺省 64 MiB）、文件不设上限；`header` 里的 `Content-Length` 与 `Transfer-Encoding` 不用（第 10 节）。
13. `[Replica]` 认得、忽略、给 `I0005`（第 4.4 节）。

## 16. 测试

沿用的规矩（`CLAUDE.md`「测试约定」与 `docs/testing.md`，含 M2 加的"测试从不碰信任库"）：只用回环与端口 0；失败时也不离开回环与临时目录；有界等待；"什么都没发生"的等待注释写等的是什么与余量；新写的计时用例提交前单独循环跑二十次以上。

1. **单元**（`rurge-config`）：`split_words`（两种引号、`key="v w"`、`\"` 与 `\\`、没有配对的引号、引号在一项中间、各项的起止）；`translate`（表里的每一种、字符类里的每一种、`\\h` 不动、`\Q…\E` 到末尾、类里不支持的三种）；`Template`（一位与两位的组号、越界、没有参与的组、`\$`、`\\`、`\n`、结尾的 `\`、`${name}`、`$` 后不是数字）；`URL-REGEX` 规则的改写（`\h` 现在配空格）；两个节的读法与每一种诊断；`[Replica]` 与 `I0005`；`W0016` 不再列这两个节（语料库快照）；脱敏的那一条（第 4.3 节）。M3b 加：`BodyPattern` 的两种引擎（CRLF 下的 `^foo=(.*)$` 不捕获 `\r`；向前看的式子落到 fancy-regex）、两个节的读法与诊断、`Reach` 的拒绝（经一个远程 include 的文本）。
2. **单元**（`rurge-http`）：三个候选的 URL（`Host` 与目标不同、相同时去重、HTTP/2 没有候选 2）；URL Rewrite 的依次求值（`header` 只第一条有效的生效、不合法的不算、`302` 在 `header` 之后比新的 URL、网关没有时放弃）；新 URL 的合法性；换目标之后的 URI、`Host` 与 `forwardable`（源站形式、经监听器来的绝对形式、HTTP/2，各配 `http` / `https` / `ws`）；四种动作与框架头的比较；改过的 `Host` 的读法（最后一行、删掉、没有变）与写回（HTTP/1.1、HTTP/2、转发的绝对形式、读不成 authority）；备注的措辞。M3b 加：缓冲的决定（chunked、`Expect`、HTTP/2 的三种、`HEAD`、204、304）；两个上限的边界（恰好等于、多一个字节）；解压（多成员的 gzip、zlib 与裸的 deflate、解压炸弹、不认识的编码、解码出错）；UTF-8 的检查；多对与多条依次作用；扩展名的表；`header` 的两种写法。
3. **端到端**（引擎）：**手册的黄金用例**——URL Rewrite 页四个（`header`、`302`、`307`、`reject`）与"Matching details"（`Host` 与请求行不同时按 `Host` 重建的 URL 命中）；Header Rewrite 页每个例子（四种动作各一、省略方向、`header-del` 与 `header-add` 连用、改框架头而失败）；M3b 加 Body Rewrite 页的两个正则例子、多对的写法、`^$` 生成正文，Map Local 页的四个例子与 `data=""`。示例里的主机名经一行 `[Host]` 钉到 `127.0.0.1`，另加一条按地址的规则作第二道防线；不连上游的例子（`302`、`307`、`reject`、Map Local）照手册的原文，要连上游的（`header` 模式）在替换里加上测试源站的端口——这一处与手册的出入写进用例的注释，M6 写进验收文档。另有：开关关着时一概不生效；解密的 `https://` 请求上的重写（MITM 与重写一起）；`http://` 改写成 `https://` 连到回环上的 TLS 源站（根经 `EngineShared.roots` 给，没有 MITM 的 CA）；`header` 换目标之后规则没有再求、策略不变（规则计数与记录的策略）；同一个目标、明文与 TLS 两种请求不共用池里的连接（源站数连接）；选定 REJECT 的请求遇到会命中的 `302` 规则照旧被拒绝、计数不动（M3b 加 Map Local 的规则）；经 HTTP 监听器来的请求改到另一个 `http://` 目标、策略是 `always-use-connect = false` 的 HTTP 代理（回环上的测试服务端记下请求行）时，代理收到新 URL 的绝对形式，改到 `https://` 时收到 `CONNECT`；Header Rewrite 改 `Host`：经监听器来的、嗅探到的 HTTP/1.1、解密的 HTTP/2 请求各一，源站看到改过的 `Host` / `:authority`，删掉 `Host` 的规则不生效；嗅探到的连接上的绝对形式请求不过重写；换了目标的请求不带客户端的 `Proxy-Authorization`；备注与计数。M3b 另有：32 MiB 断开与 10 MiB 原样流过（生成的字节，时间与内存有界）；chunked 与 `Expect` 的请求原样转发、WARN 一行（`LogCapture`）而第二次在 DEBUG；Map Local 的文件改了不必重载、文件打不开的 502；URL 的数据经回环上的 `TestServer`。
4. **API**：`/v1/features/rewrite` 的读与切换、写进 `state.json`、重启之后照用；`/v1/rurge/http` 的正文字节恰好是第 12 节那一串（用例 `rurge_http_counts_start_at_zero`）。

## 17. 文档

| 文档 | 改动 | 时机 |
| ---- | ---- | ---- |
| `docs/api/phase4.md` | M3a：`/v1/features/rewrite`、`/v1/rurge/http` 的 `rewrite`、备注；M3b：Body Rewrite 与 Map Local 的备注与那一行 WARN | 各自收尾 |
| `docs/testing.md` | 重写的测试约定（黄金用例的主机名与端口的做法）；M3 的用例过滤词 | M3a 起 |
| `docs/surge-compatibility-matrix.md` | L50、L51、7.3、7.4 节各行、L871 的 `rewrite`（M3a）；L52、L53、7.5、7.6 节各行（M3b）；第 15 节的登记；规则一节 `URL-REGEX` 的改写 | 各自收尾 |
| `docs/acceptance/phase4-manual.md` | 加：真实的浏览器经明文与解密的站点看到 URL / Header / Body Rewrite 与 Map Local 的效果、`rewrite` 开关 | M3b |
| `README.md`、`README_en.md` | 状态、特性表、路线图（两份一致） | M3b 收尾 |
| `docs/requirements.md` | 第 7 节阶段 4 的状态一句；FR 的字面不改 | M3b 收尾 |
| `docs/status.md` | 阶段 4 下 M3a、M3b 各一节 | 各自收尾 |
| `CLAUDE.md` | 「现在在哪里」 | 随本文与各自收尾 |
| `docs/README.md` | 本文与各份计划各一项 | 随本文与计划 |

## 18. 写计划时要核对的事项

| 编号 | 核对什么 |
| ---- | -------- |
| V1 | 节的解析器（`text::parse_str`）怎样把这几个节的行交出来：`ORDERED_SECTIONS`、`Profile::section`、停用的行、`#!REQUIREMENT` 的前缀、`strip_inline_comment`；`deferred.rs` 的名单与它的用例；`config.rs` 里提到这四个节的用例；语料库快照 `corpus__corpus__kitchen-sink.snap` 里那一行；`[Replica]` 今天报的 `W0002` 在哪里 |
| V2 | `value.rs` 的 `item_spans`、`wholly_quoted`、`partner` 与开头的说明，`split_words` 照它们写；`redact.rs` 读这几个节的行的路径（`redact_text` 一路）与 `docs/api/phase1.md`「有意照原样显示的」那一段 |
| V3 | `rule::Pattern` 的全部用处（`URL-REGEX`、`rurge rule match --url`、别处）；回溯超限在规则一侧怎样按不匹配；`URL-REGEX` 现有的用例里有没有用到 `\h`、`\v`、`\<` 一类、意思会变的 |
| V4 | fancy-regex 0.14.0：`(?m)`、`RegexBuilder::backtrack_limit`、`captures_iter` 交回的错误、`try_replacen`、向前看与原子组；regex 1.13.1：`RegexBuilder::multi_line` / `crlf` / `size_limit`、`captures_iter`；二者对 `\p{Zs}`、`\u{2028}` 的写法 |
| V5 | `rurge-http` 的 `Requests::handle` 第 0 ～ 13 步各在哪里：`prepare` 与 URL 的建法（`form_of`、`origin_authority`）、`route` 之后到借或拨之间、`wants_upgrade`、`headers::strip` 与 `Keep`、`fit`、应答一侧的 `strip` 与 `Tail`、`Close` 与 HTTP/2 上的重置（M2b 第 23 节第 20 条）、`connect()` 与 `OriginTls`（字段、能不能克隆）、池与共享的 HTTP/2 连接的键；`Keep.proxy_authorization`；`Form::of` 取 authority 的次序与 `fit` 的三种情形；`origin_form` / `absolute_form` 从 URI 取 authority；hyper 的 HTTP/1.1 服务端给源站形式与绝对形式的 URI、HTTP/2 服务端给的 URI |
| V6 | 引擎：`ConnUpstreams::route` 读的快照与 `Pages`、`Way` 与 `PoolKey`、`Way.attempts` 第一个的 `Resolution.terminal`（选定 REJECT 一族怎样认出）、`route_session` 里 `use-local-host-item-for-proxy` 那一段、`dial_route` 用 `Way.target` 拨号与 REJECT 的升级、`smart` 组记账用的名字、`shared.network`；`build_mitm` 取根的写法与 `HANDSHAKE_TIMEOUT`；`mitm_on` / `set_mitm` / `restore_switches` 与 `features.rs`；`state.rs` 第 2 版的 `rewrite`；`HttpCounts` 与 `/v1/rurge/http` 字节恰好的那条用例 |
| V7 | `SessionHandle::add_note` / `notes()` 怎样到 `/v1/requests/*`；备注有没有条数或长度的上限 |
| V8 | `http::Uri` 读绝对 URI 时对 `ws` / `wss`、带 `#` 的、带用户名口令的、端口越界的怎样；`HeaderName::from_bytes`、`HeaderValue::from_bytes` 对各种字的接受 |
| V9 | （M3b）hyper 的 `Incoming`：帧、`size_hint`、`is_end_stream`；HTTP/2 请求的 `content-length`；trailer；`Expect: 100-continue` 在哪里答；`http_body_util` 的 `Limited` 与收集；把已缓冲的字节与其余的正文接成一个正文的写法 |
| V10 | （M3b）flate2 1.1.10 的 `MultiGzDecoder`、`ZlibDecoder`、`DeflateDecoder`，以字节数为上限地读（`Read::take`）；工作区给 flate2 开的是哪个后端 |
| V11 | （M3b）资源管理器：加一类资源（`ResourceKind`、`type_name`、`set_kind`、列表的次序）、`get_labelled`、`ResourceHandle::current()` 与数据、更新；测试照 `resource/mod.rs` 的 `manager()` 与 `NoNames` |
| V12 | （M3b）`Reach` 与 `Origin::reach()`、`ParseCtx.base_dir`、`rule.rs` 第 164 行一带相对路径的接法；`responses::tiny_gif()` 的字节；`ResponseBody` 与流式读文件（`tokio-util` 的 `io` 特性在不在 `rurge-http` 的依赖里）；`rurge_config::remote::origin` |
| V13 | `codes::hint` 的表与 `every_code_is_decided_once`；`ParamReader::has_unrejected`；未知参数用哪个诊断码；Base64 解码在 `rurge-config` 里用的函数 |
| V14 | 测试夹具：`LogCapture`、引擎用例的 `geo_urls`、`[Host]` 钉住与按地址的规则、`TlsFixture` 与 `EngineShared.roots`、HTTP 监听器的绝对形式请求、`TestServer` 数连接 |
| V15 | （M3b）`capabilities.rs` 里 `CORE_VERSION` 那段注释；`docs/surge-compatibility-matrix.md` 里 `CORE_VERSION` 20 的那一处 |

## 19. 对总设计的订正

1. 第 3.3 节"**fancy-regex 升到 ≥ 0.18 在 M3 定**"、第 3.7 节表里 fancy-regex 一行 → **不升级，留在 0.14.0**（M3-D2）。理由同 M2 设计第 20 节第 1 条：本机的注册表里只有 0.14.0，升级要下载，新包与升级是所有者的 Q4。CRLF 改由锁文件里已有的 `regex` 1.13.1 做（第 5.4 节）；落到 fancy-regex 的 Body Rewrite 式子不认 CRLF（登记）。第 3.3 节"`\Z`（0.15 起有）"→ 改写成向前看（第 5.2 节）；"有界的变长后行断言（0.17 起有）"→ 0.14.0 没有，编译不了就跳过这一行。
2. 第 3.3 节替换模板"反斜杠只转义 `$` 与它自己" → 照 swift-corelibs-foundation 的实现：反斜杠去掉、后一个字照字面，对任何字都如此；结尾单独的 `\` 去掉（第 5.3 节）。"`$` 后读不超过分组数所需位数的数字，编号超出分组数的得空串"照旧。
3. 第 3.4、3.5、3.7 节与 P4-D13、P4-D15、P4-D24 里 jaq 一族、brotli-decompressor、ruzstd"版本在 M3 定稿" → **M3a、M3b 不引入**，等所有者回答 Q4 之后另写 M3c；在那之前 jq 的行 `W0042`、`br` 与 `zstd` 的正文不改写（第 2 节、第 9.1、9.4 节）。总设计第 15 节第 3 条"Body Rewrite 与 jq 用例通过"里 jq 的一半随 M3c。
4. 第 2.3 节 3.25"没有转义" → `"…"` 里照 LS-D4 认 `\"` 与 `\\`（第 4.1 节）；另：`'…'` 不挡行内注释（第 4.2 节）。"`value.rs` 与 `redact.rs` 在同一个提交里改" → `redact.rs` 不改，理由在第 4.3 节。
5. 第 2.3 节 3.15 没有说值里的空格 → `header-add` / `header-replace` 的值是这一行余下的部分（第 7.1 节）。
6. 第 8.4 节"请求正文至多 32 MB，超过就断开这条连接" → HTTP/2 上重置这个流（第 9.3 节）；"32 MB"与"10 MB"读作 MiB。
7. 第 6.4 节（与 M2 设计第 9.4 节"M3 加 scheme 与 SNI"）"键：……TLS 的参数" → `rurge-http` 的池以 `(K, OriginKey)` 为键，引擎的 `PoolKey` 不变（第 8.3 节）。
8. 第 8.2 节"新目标经已经选定的策略连接" → `Upstreams::retarget`；`http` 改成 `https` 时的 TLS 不要 MITM 的 CA，配置由 `Rewrite` 自己建（第 8.2 节）。
9. 第 8.5 节没有说几条 Map Local 规则都匹配时怎样、从 include 的文件来的相对路径接到哪里 → 第一条答复；一律接到主 Profile 的目录（第 10.1、10.2 节）。
10. 第 13 节"计数：……缓冲放弃（超限、不是 UTF-8、不是 JSON）" → `/v1/rurge/http` 的 `rewrite` 六个数（第 12 节）。
11. 第 8.4 节"不改写、原样转发并告警" → 每条规则每小时至多一行 WARN（第 14 节）。
12. 第 1.4 节 M3 一行 → 三份计划，M3c 等 Q4（第 2 节）。

## 20. 已决事项

| 编号 | 事项 | 决定 |
| ---- | ---- | ---- |
| M3-D1 | 计划与执行 | 三份计划（M3a URL 与 Header Rewrite → M3b Body Rewrite 的正则形式与 Map Local → M3c jq 与 `br`、`zstd`，等 Q4），后一份在前一份合并之后对着代码写；逐个任务调 Pi、逐个任务评审；没有只在 CI 上编译的代码（第 2 节） |
| M3-D2 | 依赖 | 不加新包、不下载；工作区依赖表加 `regex`；`rurge-config` 正式依赖 `regex`（M3b），`rurge-http` 正式依赖 `flate2`（M3b）；fancy-regex 留在 0.14.0；`Cargo.lock` 的包数不变（第 3 节） |
| M3-D3 | 一行的切法 | `split_words`：空白分项、两种引号、`"…"` 照 LS-D4 认 `\"` 与 `\\`、`'…'` 照字面、没有配对的引号到行末、各项记着起止；节的解析器不改，`'…'` 不挡行内注释（第 4.1、4.2 节） |
| M3-D4 | 脱敏 | `redact.rs` 不改，理由写进提交说明，加一条用例钉住输出不变（第 4.3 节） |
| M3-D5 | `[Replica]` | 认得、忽略、节头一处 `I0005`，不进 `W0016` 的名单（第 4.4 节） |
| M3-D6 | 正则的改写 | `translate` 改写 `\h` `\H` `\v` `\V` `\R` `\<` `\>` `\Q…\E` `\Z`；`Pattern::new` 与 `BodyPattern` 共用，`URL-REGEX` 规则同样；类里的 `\H` `\V` `\R` 不支持；`.` 不改写（第 5.2 节） |
| M3-D7 | 替换模板 | 照 swift-corelibs-foundation：`\` 去掉而后一个字照字面、结尾的 `\` 去掉、`$` 后最多读分组数的位数、越界与没有参与的组得空串、`${name}` 不认；三种重写共用（第 5.3 节） |
| M3-D8 | Body Rewrite 的式子 | 先 `regex`（多行、CRLF），编译不了再 fancy-regex（`(?m)`、回溯上限 1 亿）；都不行 `W0012`；不用 fancy-regex 的 `replace_all`（第 5.4、5.5 节） |
| M3-D9 | URL Rewrite 的求值 | 依次逐条、每条比当前的 URL 的三个候选、第一个匹配的候选；只有第一条有效的 `header` 生效、之后只比新的 URL；不合法的不算第一条；`302` / `307` / `reject` 第一条匹配的答复（第 6.2 节） |
| M3-D10 | `302` / `307` | 只有 `Location` 与 `Content-Length: 0`；`Location` 不能当头的值时放弃；网关只取 IPv4、没有时放弃，都留备注接着求下一条（第 6.4 节） |
| M3-D11 | Header Rewrite 的读法 | 方向可省；动作不分大小写；名字是 token、不收伪头；`header-add` / `header-replace` 的值是余下的部分；`header-replace-regex` 恰好两项（第 7.1 节） |
| M3-D12 | 四种动作 | 名字不分大小写；`add` 追加；`del` 删全部；`replace` 换全部、没有不动；`replace-regex` 每行替换全部匹配；依次作用（第 7.2 节） |
| M3-D13 | 框架头 | 比较作用前后两个名字的全部行；变了则请求失败、502、记录与备注；加载时 `W0041`、行保留（第 7.3 节） |
| M3-D14 | 引擎里的位置 | 第 8.1 节的十四步；当前的 URL；要升级的请求只过头的那几步；嗅探到的连接里的 `CONNECT` 不过（另两种不过的见 M3-D27、D29；第 8.1、8.4 节） |
| M3-D15 | 换目标 | `Upstreams::retarget`：同一个策略与尝试、新的目标与 `may_forward`；规则不再求；新主机不问 fake 地址池（第 8.2 节） |
| M3-D16 | 对上游的 TLS | 每个请求自己的：缺省是连接的；`header` 生效后由新的 scheme 定；`https` 在解密的连接上用连接的配置（M3-D31），别的连接上用 `Rewrite` 自己的两份配置（`[MITM]` 的 `skip-server-cert-verify` 与 `h2`、M2 的根），不要 CA（第 8.2 节） |
| M3-D17 | 池的键 | `rurge-http` 的池与共享的 HTTP/2 连接以 `(K, OriginKey)` 为键；引擎的 `PoolKey` 不变（第 8.3 节） |
| M3-D18 | 备注 | 每个起作用的与每个跳过的改写一条；写节、动作与式子；URL 不带查询串；头的值、正文、Map Local 的数据与网关不进备注（第 8.5 节） |
| M3-D19 | 何时缓冲正文（M3b） | 读完头时定；请求不缓冲 chunked、`Expect`、HTTP/2 上长度不知道的与要升级的，没有正文的也缓冲；应答不缓冲 `HEAD` 的与 1xx、204、304（第 9.2 节） |
| M3-D20 | 上限（M3b） | 32 MiB / 10 MiB 的线上字节；请求超限 HTTP/1.1 关连接、HTTP/2 重置流；应答超限原样流过；解压之后同样的上限、超了不改写（第 9.3 节） |
| M3-D21 | 解压与改写（M3b） | `gzip` 用 `MultiGzDecoder`、`deflate` 先 zlib 后裸流；别的编码不改写；UTF-8 严格；在 `spawn_blocking` 里；改过之后去掉 `Content-Encoding` 与 `Transfer-Encoding`、设 `Content-Length`、丢 trailer；没改动原样转出（第 9.4 ～ 9.6 节） |
| M3-D22 | Map Local 的读法（M3b） | 第一项是式子、其余是参数；路径相对主 Profile 的目录、绝对的都收、下载到的文本里的路径 `W0033`；`header` 两种写法；`status-code` 200 ～ 999（第 10.1 节） |
| M3-D23 | Map Local 的答复（M3b） | 第一条匹配的答复；不连上游、不读请求正文、不过应答一侧的重写；文件每次打开、流式读；URL 是 `ResourceKind::MapLocal`；读不到 502；`Content-Type` 的表（第 10.2 ～ 10.4 节） |
| M3-D24 | `rewrite` 开关 | 缺省开；`state.json` 第 2 版；启动时还原；每个请求在选路时读；关着时四种都不生效（第 11 节） |
| M3-D25 | 计数 | `/v1/rurge/http` 的 `rewrite` 六个数，M3a 就定全，重载不清零（第 12 节） |
| M3-D26 | 诊断与日志 | `W0012` 给读不了的行；新 `W0041`（每处自带）、`W0042`（不设）、`I0005`（不设）；`W0033` 给 Map Local 的路径；没有 `E` 级；chunked 与 `Expect` 的 WARN 每条规则每小时一行、不带主机（第 13、14 节） |
| M3-D27 | 选定 REJECT 的请求（构建者检查之后定的，下同） | REJECT 是拨号时定的（M1b 计划 P3）；`ConnUpstreams::route` 看第一个尝试的终点，是 REJECT 就给 `rewrite: None`：照 M1b 在拨号时被拒绝，不过任何重写、不计数；拒绝不挪到选路时，升级照旧在拨号时做；拨号时才出现的拒绝在重写之后（第 8.4 节） |
| M3-D28 | 换目标之后的请求 | 保持来时的形式、只换 URL（绝对的 URI 换成新的 URL，`ws` / `wss` 写作 `http` / `https`）；`Host` 跟着新的 URL；`forwardable` 只在经监听器来、新的 scheme 是明文时为真；嗅探到的连接上去掉客户端的 `Proxy-Authorization`；新的主机不按 `use-local-host-item-for-proxy` 钉住（第 6.3、8.2 节） |
| M3-D29 | 嗅探到的连接上的绝对形式请求 | 同 `CONNECT`，不过任何重写：客户端在对它自己的代理说话，M1b 照原样转给连接的目标；讲 HTTP/2 的请求不算（第 8.4 节） |
| M3-D30 | 改过的 `Host` | 记下最后一行，在第 9 步的 `fit` 之后写回：HTTP/1.1 的 `Host`、HTTP/2 的 `:authority`；以绝对形式交给转发的 HTTP 代理时不写回；删掉不生效、放回原来的；URL Rewrite 的 `header` 生效时由新的 URL 定（第 7.4 节） |
| M3-D31 | 解密的连接上对上游的 TLS 配置 | 改写过的请求同样用连接握手时那一代的 `OriginTls`（`config`、`h1`、`wait`），只换名字；别的连接用这一代 `Rewrite` 的；所以池的键 `(K, OriginKey)` 下的连接配置相同，`OriginKey` 不放配置（第 8.2、8.3 节） |

## 21. 留给项目所有者的

| 问题 | 与 M3 的关系 |
| ---- | ------------ |
| 总设计 Q4：新包（jaq 一族、brotli-decompressor、ruzstd）与升级（`p12-keystore`、fancy-regex） | M3a、M3b 不引入、不升级（第 19 节第 1、3 条）：jq 的行告警并忽略（`W0042`），`br` 与 `zstd` 的正文不改写，落到 fancy-regex 的 Body Rewrite 式子不认 CRLF。所有者同意之后另写 M3c，fancy-regex 的升级也在那时（0.19 起公开的类型改成泛型，要改调用处，总设计第 3.3 节）。在那之前，另一种做法是 rurge 对上游把 `Accept-Encoding` 收窄成它解得开的几种，使 `br` 与 `zstd` 不出现；本文没有这样做（那会改变每个经引擎的请求，而不只是有 Body Rewrite 规则的那些），要不要做归所有者 |
| 总设计 Q2：脱敏视图里重写节的头值、`[Map Local]` 的 `header=` / `data=` | 照阶段 1 的裁定照写的显示；M3 加了新的切法而 `redact.rs` 不改（第 4.3 节） |
| 总设计 Q1：用户流量的内容算不算 API 输出 | 记录照旧带完整的 URL；M3 的备注不带查询串、不带头的值与正文。Header Rewrite 写进去的头值（例如替换的 `Authorization`）会随请求发往上游——那是使用者的配置本来的意思 |
| 总设计 Q2、Q3、Q5、Q6 中别的 | 与 M3 无直接关系，照旧开着 |

另有两件事要项目所有者做：**推送**（M1 与 M2 的 98 个提交还没有推送；M3b 做完之后若仍没有，就三个里程碑一起，推送之后读回那次 CI）；**手工验收**（`docs/acceptance/phase4-manual.md` 在 M3b 加 M3 的几项；阶段 1、2、3 的清单与阶段 4 的照旧一项也没有做）。

所有者可以改的裁定：本文 M3-D1 ～ D31，尤其 M3-D2（不引入新包、不升级 fancy-regex，jq 与 `br`、`zstd` 等 Q4）、M3-D3（`'…'` 不挡行内注释）、M3-D4（`redact.rs` 不改）、M3-D7（模板照 swift-corelibs-foundation）、M3-D9（不合法的 `header` 规则不算第一条）、M3-D11（值是这一行余下的部分）、M3-D16（`http` 改成 `https` 时照 `[MITM]` 的 `skip-server-cert-verify` 校验上游）、M3-D27（选定 REJECT 的请求不过重写，Map Local 与 `302` 也不代替拒绝）与 M3-D30（改过的 `Host` 写回、删掉 `Host` 不生效）。

## 22. 计划与执行期的订正

写计划与执行时对本文正文的改动记在这里，一条一段：改了正文哪一处、以哪一处为准。正文不逐处改，读正文之前先读这一节。

写 M3a 计划时（`docs/superpowers/plans/2026-10-31-phase4-m3a-url-header-rewrite-plan.md`，对着 `58ec98f` 核对）：

1. 第 2 节 M3a 的任务表是任务 0 ～ 7；计划拆成任务 0 ～ 8：本文的任务 4 拆成计划的任务 4（每个请求的对上游 TLS 与池的键 `(K, OriginKey)`，行为不变）与任务 5（引擎里的各步），本文的任务 5 ～ 7 依次是计划的任务 6 ～ 8。脱敏的那一条用例在计划的任务 1、随 `split_words` 一起：第 2 节把它写在任务 2，以第 4.3 节"加 `split_words` 的提交在说明里写这一段理由，并加一条脱敏的用例钉住"为准（计划 P1）。

2. 第 3 节的 `Word` 多一个字段 `quoted: bool`（这一项整个在引号里）。第 4.1 节没有说空的引号：`""` 留成一项、文字是空串——逗号分隔的列表丢掉它们（`split_fields`），第 7.1 节的 `header-add X ""` 要它。`Word` 与 `Template` 的 `Debug` 不显示文字（`Word(***)`、`Template(***)`）。以计划 P2、P4 为准。

3. 第 3 节的 `UrlRewrite` 多一个字段 `gateway: bool`：第 6.1 节"替换里写着 `{{{GATEWAY_ADDRESS}}}` 时加载时就记下"记在这里。M3a 的 `RewriteConfig` 只有 `url` 与 `header` 两个字段，`body` 与 `map_local` 由 M3b 加（计划 P6）。

4. 第 7.2 节 `header-replace`"全部同名的行换成这个值"：计划定为换成**一行**，留在原来第一行的位置（`HeaderMap` 的 `OccupiedEntry::insert`）；登记（计划 P9、P21）。

5. 第 6.2 节"之后的 `header` 规则一概跳过"与第 8.5 节"每个被跳过的改写一条"：第一条 `header` 生效之后的 `header` 规则不求、不留备注、不计入 `skipped`——手册说只有第一条生效，它们不是被跳过的改写（计划 P9、P11）。

6. 第 6.3 节合法的新 URL 另加两条：`https` / `wss` 的主机要能当 TLS 的名字（`ServerName`），否则不合法——不加这一条，就要到拨号之后才失败；`#` 之后的部分去掉（第 6.3 节只说用户名口令不用）。以计划 P10 为准；前一条登记。

7. 第 6.5 节 `reject`"同 `REJECT` 策略对引擎里一个请求的答复"：状态码、标题与关掉连接的办法同 REJECT，错误页的说明改成 `The request was rejected by a URL Rewrite rule.`——REJECT 那一页的 "…by the REJECT policy." 在这里不实（计划 P12）；登记。

8. 第 16 节第 3 条"另加一条按地址的规则作第二道防线"与"要连上游的（`header` 模式）在替换里加上测试源站的端口"：规则照手册原文逐字写，测试源站的端口写在请求里（式子照样命中，替换之后端口留着），不改手册的规则；不连上游的例子照手册的 URL 发，规则选测试自己持有的守卫监听器。第二道防线是共用夹具的 `dns-server`——回环上的 `MockDns`；夹具在规则之后自带 `FINAL,DIRECT`，用例不另写 `FINAL`（计划 P19）。

**执行 M3a 时**：由 M3a 的任务 8 补上（`docs/superpowers/plans/2026-10-31-phase4-m3a-url-header-rewrite-plan.md` 的「执行期修正记录」）。

9. 第 6.3 节"写了端口时在 1 ～ 65535"：执行时定空着的端口算没写、用缺省端口（`http://a.test:/`、`wss://[::1]:/`），不是不合法。依据是 RFC 3986 第 3.2.3 节（`port = *DIGIT`，`URI producers and normalizers should omit the port component and its ':' delimiter if port is empty`）与 WHATWG URL Standard 的 port state；照字面读，"写了端口"也可以读成包括空的那一种。以修正记录第 37 行为准。

10. 第 6.3 节"合法的新 URL"只列了 scheme、主机不空与端口：执行时多一条——URL 的三样（绝对形式的 `Uri`、源站形式的 `Uri`、`Host` 的值）造不成一样就不合法。例子是 `http://[fe80::1%25eth0]/`：`http::Uri` 读得成，主机却读不成地址，`origin_authority` 写出的东西读不回。以修正记录第 28 行为准。

11. 第 7.2 节"结果不能当头的值时那一行的值不变、留备注"读来是一行一条：执行时定一条 `header-replace-regex` 规则至多一条 `Skipped`（同名的几行都失败时留最后失败的那一行的），`skipped` 只数一次。以修正记录第 35 行为准。

12. 第 7.4 节"删掉了全部 `Host` 时放回原来的各行、留备注"与第 12 节"`header` 是头被 Header Rewrite 改过的请求"：执行时 `header-del Host` 那一条规则照 P9 留 `Done`、`changed` 为真、`header` 加一，之后 `Host` 放回、再留 `Skipped(Host cannot be removed)`——请求的头最终没有变，却数了一次 `header`，第 12 节的定义对这一种不成立。以修正记录第 36 行为准。

写 M3b 计划时（`docs/superpowers/plans/2026-10-31-phase4-m3b-body-rewrite-map-local-plan.md`，对着 `ba6646a` 核对）：

13. 第 2 节 M3b 的大纲拆成计划的任务 0 ～ 9：大纲里"正文的缓冲、上限与解压""Body Rewrite 在引擎里的两个方向""Map Local 的四种数据与 `ResourceKind::MapLocal`"分成任务 3（正文的读入、何时缓冲与解压）、任务 4（改写的求值与 Map Local 的规则）、任务 5（引擎里的三步）与任务 6（Map Local 的 URL 与资源管理器）；"手工清单加 M3 的几项"与"M3 收尾的文档"分成任务 8、9（计划 P1）。

14. 第 3 节的 `BodyPattern` 是公开的枚举 `Plain(regex::Regex)` / `Fancy(fancy_regex::Regex)`：计划改成结构体 `{ pub source: String, … }`，编译好的那一种是私有的，替换（`replace_all`，逐个取匹配、照 `Template` 展开）也在 `rurge-config` 里——`rurge-http` 不依赖 `regex`（第 3 节只给它加 `flate2`），用不了编译好的式子。以计划 P2 为准。

15. 第 3 节的 `MapData::Url(String)` 改成 `Url(url::Url)`：加载时就读成 URL，读不成的 `W0012`；`MapData` 与 `MapLocal` 的 `Debug` 不显示文本、Base64 与头的值，URL 只显示到 `rurge_config::remote::origin`（计划 P5）。

16. 第 3 节的表与第 10.3 节"控制面的名字 `map-local`"改成 **`data`**：手册 HTTP API 的 `GET /v1/external_resources` 列的种类是 "ruleset, domainset, script, policy-group and data"，`docs/api/phase1.md` 与兼容性清单已经把 `data` 对到 Map Local；`map-local` 是 Surge 的 API 不给的名字（计划 P19）。

17. 第 13 节 `W0042` 的消息多 `[Body Rewrite]: ` 的前缀，同这个节别的诊断："[Body Rewrite]: jq body rewrite is not available in this build of rurge; the line is ignored"（计划 P7）。

18. 第 9.2 节讲 HTTP/2 的客户端没有 `content-length` 而有正文的请求"同 chunked"：不缓冲、原样转发、WARN 的限频同 chunked，备注的原因是它自己的 `the request body has no length`，不说成 chunked（计划 P10）。

19. 第 10.3 节"`tiny-gif`：`responses::tiny_gif()` 那一张 GIF 的字节"：`tiny_gif()` 交回的是一个应答，字节是 `rurge_inbound::responses::TINY_GIF`（计划 P15）。

20. 第 3 节 `Rewrite::build` 的签名多一个参数 `url_data: &dyn Fn(&str) -> UrlData`（`UrlData` 是 `Arc<dyn Fn() -> Option<Bytes> + Send + Sync>`）：引擎把 Map Local 的每个 URL 挂进这一代的资源管理器，交回读当时那一份数据的闭包；`rurge-http` 不必认识资源管理器怎样登记一类资源（计划 P19）。

21. 第 10.3 节"读到一半出错……同上游正文出错"与第 12 节 `failed` 的"Map Local 的数据读不到"：`failed` 只数答复之前就读不到的（文件打不开、不是普通文件、URL 还没有数据，都是 502 或关闭）；读到一半出错或文件读到一半变短的不计入 `failed`——这个请求已经由 Map Local 答复、计入 `mapLocal`，坏的是正文的递送，同上游正文出错不算重写的失败。记录分别是 `Map Local: reading <路径> failed: <错误>` 与 `Map Local: <路径> became shorter while it was read`（计划 P15、P16）。
