# 阶段 4 / M3a「URL Rewrite 与 Header Rewrite」实施计划

> **执行方式**：交给 Pi，由协调的会话逐个任务派工、逐个任务评审（设计第 2 节、M3-D1；做法同 M1a ～ M2c）。
>
> - 协调的会话每个任务开一个 Pi 会话，交给它这个任务的全文与要用的接口（派工文件）。Pi 做完把报告写进派工文件指定的路径，在本地提交。
> - 协调者读改动、重跑门禁、评审。有问题回到同一个 Pi 会话修正，再评审；过了才派下一个任务。
> - 每个任务：先写失败的用例、看到红；再实现到绿；跑门禁；对照本任务的「判据」与「全局约束」逐条自查；最后提交（**只在本地，不推送**）。钉住现有行为的用例（任务里写明"不要求先红"的）可以一写就绿。
> - 与计划的出入记进末尾的「执行期修正记录」，**不改计划的正文**。
>
> **停点**：没有。M3a 没有平台代码、没有只在 CI 上编译的代码（设计第 2 节），做完之后接着写 M3b 的计划，不停下来等推送。M1 与 M2 的推送照旧归项目所有者。

**目标**：让 `[URL Rewrite]` 与 `[Header Rewrite]` 生效，`rewrite` 开关生效并持久化。

- 一行的切法（`split_words`）、编译之前的正则改写（`translate`，`URL-REGEX` 规则同样）、照 `NSRegularExpression` 的替换模板（`Template`）。
- 两个节类型化；`[Replica]` 认得并忽略（`I0005`）；改框架头的行给 `W0041`；两个节从 `W0016` 的名单拿掉。
- HTTP 引擎里每个请求在选定策略之后依次过 Header Rewrite（请求）→ URL Rewrite（`302` / `307` / `reject` 在这里答复，`header` 换掉拨号的目标而不重选策略），应答过 Header Rewrite（应答）；改过的 `Host` 在发出之前写回；每个请求自己的对上游 TLS，池以 `(K, OriginKey)` 为键。
- 备注、`/v1/rurge/http` 的 `rewrite` 六个数（`body`、`mapLocal` 到 M3b 之前恒为 0）、`/v1/features/rewrite`。

**架构**：

- 不加工作区边、不加外部包（M3-D2）：`rurge-http → rurge-config`、`rurge-http → rurge-net`、`rurge-engine → rurge-http` 都已有。
- `rurge-config` 只做读法：类型、诊断、正则的编译与模板的解析；它不依赖 `http`，头的名字与值是加载时校验过的 `String`（设计第 3 节）。
- `rurge-http` 新 `rewrite` 模块是不碰网络的函数（任务 3）；引擎里先把"每个请求的对上游 TLS 与池的键"改成按请求（任务 4，行为不变），再接上重写的各步（任务 5）。
- `rurge-engine` 建每一代的 `Rewrite`、管开关与计数；`rurge-api` 读它们（任务 6）。可执行文件不改（设计第 3 节）。

**技术栈**：都已在 `Cargo.lock` 里，源码在本机 cargo 注册表里。**不下载、不加新包**（M3-D2）。

- fancy-regex 0.14.0：`Regex::new`、`is_match`、`captures_iter`（交回 `Result`）、`captures_len`；`RuntimeError::BacktrackLimitExceeded`；缺省的回溯上限 1 000 000（`src/lib.rs`）。**不用 `replace_all` / `replacen`**：回溯超限时它们 panic（设计第 5.5 节）。
- http 1.5.0：`Uri`、`uri::Authority`、`HeaderMap` 的 `entry` / `OccupiedEntry::iter_mut` / `OccupiedEntry::insert`、`HeaderName::from_bytes`、`HeaderValue::from_bytes`。
- rustls 0.23.43 与 rustls-pki-types 1.15.1：`ServerName<'static>`（`Clone, Eq, Hash, PartialEq`，`src/server_name.rs`）；`rurge_proto::transport::tls::origin_client_config`。
- hyper 1.11.1：源站形式与绝对形式的 URI 照 M1b、M2b。

**依据**：

- M3 细化设计（下面写"设计"指它）：`docs/superpowers/specs/2026-10-31-phase4-m3-rewrite-map-local-design.md`。
  - M3a 用到第 3 ～ 8 节、第 11 ～ 16 节、第 18 节，与已决事项 M3-D1 ～ D7、D9 ～ D18、D24 ～ D31（D8 与 D19 ～ D23 是 M3b 的）。
  - **先读它第 22 节**：第 1 ～ 8 条是本计划改了设计正文的地方（随本计划提交）。
  - 设计与本计划的「计划期决定」不一致的，以计划为准。
- M1b、M2a、M2b、M2c 计划的「计划期决定」与执行期修正记录：`Upstreams`、池、共享的 HTTP/2 连接、`fit`、`OriginTls`、`mitm` 开关的来历。
- 手册两页（`http/url-rewrite.html`、`http/header-rewrite.html`，2026-10-31 取的文本，与 2026-10-14 的基线逐字节相同）：黄金用例照它们的示例行逐字写。
- swift-corelibs-foundation 的 `NSRegularExpression.replacementString(for:in:offset:template:)`（Apache-2.0）：只取它的行为（P4）。
- 写计划时对着 `58ec98f`（分支 `phase4-m3`）核对过的事实写在「计划期决定」里。外部库的事实对着注册表里锁定版本的源码。

## 全局约束

**依赖**

- 不改任何 `Cargo.toml`。`Cargo.lock` 的 `[[package]]` 今天 520 个，每个任务之后照旧 520 个，锁文件不变。
- 依赖方向不变：`rurge-config` 不依赖 `http`；`rurge-http` 不依赖 `rurge-engine`；`rurge-api` 的正式依赖仍只有 `rurge-engine` 与 `rurge-config` 两条工作区边。
- `unsafe_code`：不加放开的函数，`crates/rurge-platform/tests/unsafe_functions.rs` 的 `LISTED` 仍是 23 项。
- `crates/rurge/tests/sockets.rs` 的 `LISTED` 仍是 26 行：重写不建套接字，换目标之后的拨号仍经 `dial_route`，TLS 包在拨好的流上。名单之外的原因让那条扫描用例变红，就停下报告。

**缺省的 `cargo test` 不碰系统**

- 同 M2：不建网卡，不改系统的任何设置；不执行 `netsh`、`route`、`ipconfig`、`ip`、`resolvectl`、`networksetup`。
- **测试从不把任何 CA 装进信任库，也不读系统的信任库**：建引擎的用例一律给 `EngineShared.roots`（`TlsFixture` 的根，或用不到 TLS 时 `RootCertStore::empty()`），所以 `union_root_store()` 在用例里从不被调用（P19）。
- 网关的地址只经 `EngineShared.network` 给（`NetworkCell` 与 `NetworkFacts`），用例从不读本机的网络。

**测试的约定**

`CLAUDE.md`「测试约定」整节适用。写用例之前读 `docs/testing.md`「测试约定（全文）」的阶段 4 各节。与本计划直接相关的：

- 只用回环与端口 0。示例里的主机名（含换目标之后的新主机）经一行 `[Host]` 钉到 `127.0.0.1`；第二道防线是共用夹具的 `dns-server`——回环上的 `MockDns`（P19）。要连上游的请求，目标端口是测试的源站的端口、写在请求里；不该连上游的请求，规则选的策略是指向**测试自己持有的守卫监听器**的 HTTP 代理，守卫被连上就判失败。
- 回环之外的地址不写成可能被拨的目标。`302` 的 `Location` 与网关只是文字、从不被拨：用例里的网关用 `127.0.0.2`。
- 等待都有时限。固定等待只许"断言什么都没发生"，注释写它等的是什么、余量多大。新写的、带等待的用例提交前单独循环跑二十次以上（任务 5、6、7）。
- 抓日志的用例用 `LogCapture`，跑在当前线程运行时。测试里手写的 HTTP/1.1 客户端把每个请求——请求头与正文——在一次写里发出。
- **内联的测试模块放在文件的末尾**：`crates/rurge/tests/sockets.rs` 读生产代码，读到第一个 `#[cfg(test)] mod … {` 为止。

**门禁与这台开发机**（每个任务末尾一次；说"通过"之前必跑）

- 门禁：`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test -j 8 --workspace --no-fail-fast`。
- 本机 msvc 工具链没有 rustfmt。跑 `cargo fmt` 之前让环境变量 `RUSTFMT` 指向 gnu 工具链里同版本的 rustfmt：用户目录下的 `.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe`。不要为此安装或升级工具链。
- 聚焦地跑用例也经整个工作区：
  - 单元测试：`cargo test -j 8 --workspace --lib -- <过滤词>`；
  - 集成测试：`cargo test -j 8 --workspace --test <名字> -- <过滤词>`。
- **不用 `-p`**：它另编一份依赖，`target/` 涨得很快，写满过 D: 盘。
- 跑全量之前 `df -h /d`：剩余不足约 15 GiB 就停下报告。
- 测试二进制以 `STATUS_HEAP_CORRUPTION`（本机管控软件注入的钩子 DLL）或 `STATUS_ACCESS_VIOLATION` 退出：单独重跑那一个二进制，报告里写明。
- `cli` 的用例偶发 `give_up` 20 秒超时：单独重跑 `cli`，把标准错误原样抄进报告。
- rustc 在增量编译里报内部编译器错误：删掉出错的那个目标在 `target/debug/incremental/` 下的目录再编。
- 回环的 TCP / UDP 7、9、13、17、19 号端口在这台机器上开着，CI 上关着：用例不假定任何固定端口的状态。
- 结果照实记：本机的全量运行是"一台机器上的一次运行，不是 CI"；需要参照服务端的互操作用例在本机立刻返回、算在通过的数里，要说出来。

**仓库的规矩**（`CLAUDE.md`「必须遵守的规矩」整节适用；与本计划直接相关的）

- **凭据与用户流量**（设计第 8.5、14 节，M3-D18）：头的值、网关的地址从不进备注、日志、错误文本与 `Debug`；备注里的 URL 不带查询串。`Word`、`Template`、`HeaderAction` 的 `Debug` 不显示值（P2、P4、P6）。
- **一行怎样拆成各项**：`split_words` 加在 `value.rs`，`redact.rs` 不改（M3-D4）——任务 1 的提交说明写设计第 4.3 节的理由，并加一条脱敏的用例钉住输出不变。
- **诊断**：`W0041` 进 `PER_SITE`、`I0005` 进 `WITHOUT`，`codes::hint` 的文档注释同时改（P7）。
- **日志的分级**：M3a 不加每个请求的日志行；只有建不成一代的 `Rewrite` 时一行 WARN（P17）。
- 文档与报告用写文件 / 改文件的工具改。**永远不要用把一个文件的内容经管道写回同一个文件的命令**（2026-10-27 这样写满过 D: 盘）。
- 计划末尾两张表里已写下的行不改写、不删除。追加新行时**不要把已有的行当成锚点替换掉**（M2a 任务 9 这样删掉过一行）。

**写法**

- 注释、日志、错误文本与诊断用英文，照所在文件的写法：文档注释写"为什么"与前提，句子完整。注释里提到设计或计划，写 `phase 4 M3 design 6.2`、`the phase 4 / M3a plan's P9`。
- 提交说明照 `git log` 的样子写，标题用中文：`feat(config): …`、`feat(http): …`、`feat(engine): …`、`feat(api): …`、`test(engine): …`、`refactor(http): …`、`docs: …`。正文说做了什么、改了哪些现有用例的期望、为什么。Pi 的提交不带署名行。
- 只在本地提交：不 `push`、不 `pull`、不 `fetch` 远端、不 `amend`、不改写历史。

**不做的事**

- 只运行本仓库构建出来的二进制与 Pi 自己。不下载、不安装任何东西。不手动跑 `rurge run`、`rurge dns lookup`；`rurge service` 只用 `--dry-run`。
- 参考实现：GPL 的（sing-box、mihomo、Xray-core）只取事实，代码不抄。
- 不做设计第 1 节「不做」一栏的事，也不做这些：
  - M3b 的事：`[Body Rewrite]`、`[Map Local]`（两节照旧在 `W0016` 的名单里）、正文的缓冲与解压、`regex` 与 `flate2` 依赖、`W0042`；
  - M3c 的事：jq、`br`、`zstd`；对上游收窄 `Accept-Encoding`（设计第 21 节，归所有者）；
  - 改 `redact.rs`、`crates/rurge-config/tests/secrets.rs`、节的解析器（`text::parse_str`、`strip_inline_comment`）；
  - `capture` 生效（M4）。

**计划没有写到、要做决定的地方**

- 能按设计与总设计的原则定的：自己定，记进修正记录，写"执行者的裁定"。
- 要停下来列出、交给协调者的：去向写着"项目所有者"的（设计第 21 节）；会改变 M3-D1 ～ D31 的。
- 现有用例变红：P20 的表点了名的，照计划改，在提交说明里说；没点名的，停下来，记进修正记录，不顺手改期望值。

## 评审要点

设计没写透、而最可能咬到使用者的几种输入，与负责它们的任务和用例：

1. **会回溯很久的正则**：URL 的式子或 `header-replace-regex` 的式子在某个 URL 或头的值上回溯超限。
   - 期望：这一条改写跳过、请求照常走、留一条备注、`skipped` 加一；进程不 panic；`URL-REGEX` 规则一侧照阶段 1 按不匹配。
   - 用例（任务 3、7）：`a_rule_that_hits_the_backtracking_limit_is_skipped_with_a_note`、`a_regex_that_backtracks_too_long_skips_the_rewrite`。
2. **头的值与查询串漏出来**：替换 `Authorization` 一类的值、URL 带着令牌一类的查询串。
   - 期望：备注、日志、`Debug`、诊断的消息里都没有头的值与查询串。
   - 用例（任务 1、2、3、7）：`words_and_templates_show_nothing_in_debug`、`the_debug_of_header_rewrites_shows_no_value`、`notes_never_carry_values_or_queries`、`no_header_value_reaches_the_notes_or_the_log`。
3. **奇怪的改写结果**：别的 scheme、空的主机、端口 0 或 65536、带用户名口令、带 `#`、IPv6 的字面地址、`wss`、能当 URL 却不能当 TLS 名字的主机；`Location` 里不能当头的值的字。
   - 期望：不合法的 `header` 结果丢弃、留备注、下一条 `header` 规则照样能生效；用户名口令与 `#` 去掉；`Location` 不合法的放弃、接着求下一条。
   - 用例（任务 3）：`which_rewritten_urls_are_valid`、`an_invalid_header_result_does_not_count_as_the_first`、`a_location_that_cannot_be_a_header_value_is_skipped`。
4. **同一条客户端连接上明文与 TLS 混着去同一个目标**，与讲 HTTP/2 的客户端被换目标。
   - 期望：明文的与 TLS 的连接不互借；解密的 HTTP/2 请求改写到 `http://` 时明文发出、改写到另一个 `https://` 主机时按新名字校验。
   - 用例（任务 4、5、7）：`origin_keys_tell_plain_from_tls_and_names_apart`、`the_pool_keeps_plain_and_tls_apart`、`plain_and_tls_requests_to_one_target_do_not_share_a_connection`、`a_decrypted_http2_request_can_be_sent_to_plain_http`。
5. **框架头被改、与选定 REJECT 的请求**：请求或应答一侧的规则改了 `Content-Length` / `Transfer-Encoding`；规则选了 REJECT 而又有会命中的 `302`。
   - 期望：框架头变了的请求失败（502 或关闭），请求一侧不拨号；选定 REJECT 的照旧被拒绝、不过任何重写、计数不动。
   - 用例（任务 3、5、6、7）：`framing_changes_are_found`、`a_request_whose_framing_changes_fails_before_the_dial`、`a_response_whose_framing_changes_fails`、`a_rejected_request_is_not_rewritten`。

---

## 文件结构

| 文件 | 任务 | 职责 |
| --- | --- | --- |
| `crates/rurge-config/src/{value.rs,dialect.rs（新）,rule.rs,redact.rs,lib.rs}` | 1 | `split_words`；`translate`、`Template`；`Pattern::new` 先改写再编译；脱敏的钉住用例 |
| `crates/rurge-config/src/{rewrite.rs（新）,config.rs,deferred.rs,diagnostic.rs,text/mod.rs,lib.rs}`、`crates/rurge-config/tests/snapshots/corpus__corpus__kitchen-sink.snap` | 2 | 两个节的读法、`[Replica]`、`W0041`、`I0005` |
| `crates/rurge-http/src/rewrite/{mod.rs,url.rs,header.rs}`（新）、`crates/rurge-http/src/{lib.rs,mitm/decrypt.rs}` | 3 | `Rewrite`、`RewriteCounters`、候选的 URL、新 URL、两种重写的求值、`Host` 的写回、`302` / `307` 的应答 |
| `crates/rurge-http/src/{engine.rs,mitm/decrypt.rs}` | 4 | 每个请求的对上游 TLS；池与共享的 HTTP/2 连接以 `(K, OriginKey)` 为键（行为不变） |
| `crates/rurge-http/src/engine.rs`、`crates/rurge-engine/src/http.rs` | 5 | 引擎里的各步、`Upstreams::retarget` / `gateway`、`Routed::Go` 的 `rewrite` |
| `crates/rurge-engine/src/{runtime.rs,engine.rs,http.rs,shared.rs,serve.rs}`、`crates/rurge-api/src/routes/{features.rs,rurge_http.rs}`、`crates/rurge-api/tests/api.rs`、`crates/rurge/tests/cli.rs`、`crates/rurge-engine/tests/rewrite.rs`（新） | 6 | 一代的 `Rewrite`、开关、计数、两个路由 |
| `crates/rurge-engine/tests/rewrite.rs` | 7 | 端到端：手册的黄金用例与其余的 |
| 文档（任务 8 列全） | 8 | M3a 的收尾 |

模块的名字与拆法可以照实际调整，调了记进修正记录。

---

## Task 0：开工前核对

**做什么**：对着当时的分支核对下面每一项。有出入的记进「执行期修正记录」，一项一行，写"计划写 X，代码是 Y，按 Y 做"；按记录调整后面的任务。**不改计划的正文。** 没有出入也记一行。

1. **工作树**：`git status --short` 空；当前分支 `phase4-m3`；`58ec98f`（设计的提交）在历史里，之后只多了本计划的提交。
2. **读**：设计第 22 节（先读）、第 3 ～ 8 节、第 11 ～ 16 节、第 18 节；本计划的「计划期决定」与 P20 的表；`docs/status.md` 阶段 4 / M2c 一节。
3. **名字与签名**（设计第 18 节 V1 ～ V8、V13、V14）：用 `git grep -n` 逐项找到、记下行号；与「计划期决定」写的不同的照实记。
   - `rurge-config`：
     - `value.rs` 开头的说明与 `item_spans`、`wholly_quoted`、`partner`、`unescape`、`Field` 的 `Debug`；
     - `rule.rs` 的 `Pattern`（`new`、`PartialEq`、`Debug`）与它的三处用处（`URL-REGEX`、`spec/group.rs` 的 `policy-regex-filter` 与 `policy-priority`）；`rurge-rules/src/matcher.rs` 里 `is_match(..).unwrap_or(false)` 的两处；
     - `URL-REGEX` 与那两个参数现有的用例里有没有写 `\h`、`\H`、`\v`、`\V`、`\R`、`\<`、`\>`、`\Q`、`\Z` 的（`git grep -n -e '\\\\[hHvVRQZ<>]' -- crates`，逐条看是不是正则）；
     - `text/mod.rs` 的 `KEY_VALUE_SECTIONS`、`ORDERED_SECTIONS`、`section_kind` 与它的用例；`Profile::section` 找第一个同名的节、`#!include` 进来的同名节并进第一个（`include.rs` 里 `[Rule]` 的用例）；`Entry` 的 `raw`（去掉注释与前缀、去掉首尾空白）与 `span`；
     - `config.rs` 的 `Config`（`ip_rewrite` 字段与它的读法）、`DeferredSections::collect` 与 `W0016` 的那一段、未知的节（`W0002`）的循环、`ip_rewrite_is_read_in_order`；
     - `deferred.rs` 的 `DEFERRED` 与 `prefix_and_membership_matching`；
     - `diagnostic.rs` 的码表末尾（`W_API_TLS_NO_CA`、`I_MANAGED_SOURCE`）、`Diagnostic::{info, warning, at, with_hint}`、`codes::hint` 与它的文档注释、测试的 `WITH_DEFAULT`、`PER_SITE`、`WITHOUT`、`every_code_is_decided_once`；
     - `redact.rs` 的 `redact_profile`；`docs/api/phase1.md`「有意照原样显示的」那一段；
     - 语料库 `tests/corpus/valid/kitchen-sink.conf` 的 `[URL Rewrite]`、`[Header Rewrite]`（第 159 ～ 166 行一带）与快照 `corpus__corpus__kitchen-sink.snap` 的第 98、99、112 行；
     - `git grep -n -i 'replica'`：`[Replica]` 今天在哪里报 `W0002`、有没有用例写着它。
   - `rurge-http`：
     - `engine.rs`：`Entry`、`Conn`、`Routed`、`Pages`、`Refusal`、`Upstream`、`Upstreams`（`named`、`open`、`route`、`dial`、`relay`）、`Pooled`、`Shared`（`pool`、`h2`、`borrow_h2`、`share`）、`SharedH2`、`Requests::handle` 的第 0 ～ 7 段（`prepare`、`open`、`route`、`prepared.unknown`、`sniffed`、`wants_upgrade`、`tunnel`、`needs_h1`、借与拨、`Form::of`、`headers::strip`、`origin_form` / `absolute_form`、`fit`、`head`、发送与重发、407、101、`promote`、`set_status`、应答的 `strip`、`Outing`、`Tail`）、`answer`、`failed`、`page`、`connect`、`redial`、`retry`、`Resend`、`Outing`、`form_of`、`default_port`、`origin_authority`、`path_and_query`、`rename`；测试模块里 `Upstreams` 的那一份实现（`Fake` 一带，`Key = u32`、`Route = u32`）与 TLS 源站的夹具（`origin_client_config` 的三处）；
     - `version.rs` 的 `Form`、`Form::of`、`fit`（三种情形；`fit` 改不改 `req.version()`）；
     - `headers.rs` 的 `HOP_BY_HOP`、`Keep`、`strip`、`host_port`；
     - `pool.rs` 的 `Pool<K, C>`、`Reusable`、`take`、`put`；
     - `mitm/decrypt.rs` 的 `OriginTls`、`Mitm::build`（两份客户端配置与 ALPN）、`server_name`（私有）与 `HandshakeFailure`；
     - `lib.rs` 的模块与再导出。
   - `rurge-inbound`：`SessionHandle::{session, add_note, notes, set_status, finish, rule, policy_chain}`、`SessionOutcome`、`Close`、`responses::{empty, error_page, ErrorPage, tiny_gif}`。
   - `rurge-net`：`connector::Target`（`host: HostName`、`port: u16`、`Target::new`）。
   - `rurge-engine`：
     - `http.rs` 的 `PoolKey`、`ConnUpstreams`、`route`（快照、`request_count`、`Pages`、`way.attempts.first()`）、`dial`；
     - `engine.rs` 的 `Way`、`HANDSHAKE_TIMEOUT`、`mitm_on`、`set_mitm`、`restore_switches`、`network()`、`http_counts()`、`shared`；
     - `rurge-policy` 的 `Resolution.terminal` 与 `TerminalKind`；
     - `runtime.rs` 的 `Runtime.mitm`、`build_mitm`（取根的那一段）；
     - `shared.rs` 的 `EngineShared`（`new`、`Default`、`roots`、`network`、`mitm`）；
     - `serve.rs` 的 `HttpCounts`；`state.rs` 的 `Features.rewrite`；
     - `rurge-config/src/network.rs` 的 `NetworkFacts`、`Primary.routers`（"IPv4 的那个在前"）、`NetworkCell`。
   - `rurge-api`：`routes/features.rs` 的 `FEATURES`、`get_feature`、`set_feature`；`routes/rurge_http.rs` 的 `HttpJson`、`MitmJson`、`http`。
   - 用例的夹具：
     - `crates/rurge-engine/tests/common/mod.rs`：`Profile`（`sections` 是 `[Rule]` 之后的原文）、`harness`、`harness_with`、`harness_trusting`、`plain_get`、`connect_via_http`、`the_records`、`wait_until`、`closed_tcp_port`；
     - `crates/rurge-engine/tests/http.rs` 里起源站、数源站连接、记下源站收到的请求头的夹具，与经 SOCKS5 进来的嗅探到的连接的写法；
     - `crates/rurge-engine/tests/mitm.rs` 的 `mitm_section`、`mitm_ca`、`harness_mitm` 与讲 HTTP/2 的客户端的写法；
     - `rurge_proto::testing::TlsFixture::{new, roots, ca_der, ca_p12_base64}` 与它起 TLS 源站的写法；
     - `LogCapture` 在哪里；
     - `crates/rurge-api/tests/api.rs` 的 `the_mitm_switch_is_read_switched_and_kept`、`rurge_http_counts_start_at_zero`、`features_collections_stop_and_unknown_paths`；
     - `crates/rurge/tests/cli.rs` 的 `a_daemon_restores_the_mitm_switch_from_state_json` 与它用的 `write_conf`、`API_GENERAL`、`spawn_daemon`、`api_port`、`api_call`、`wait_for_exit`。
   - 外部库（注册表源码），行号写进记录：
     - fancy-regex 0.14.0：`src/parse.rs` 的 `parse_escape`（`\h`、`\v`、`\<`、`\>`、`\Z`、`\Q` 怎样读；字母以外的字前加反斜杠是不是照字面）、`Regex::{is_match, captures_iter, captures_len}`、`Captures::get`、缺省的回溯上限、`tests/matching.rs` 第 94 行一带回溯超限的那条用例；
     - http 1.5.0：`Uri` 读 `ws://`、`wss://`、带 `#`、带用户名口令、端口为 0 与大于 65535 的绝对 URI 各怎样；`uri::Authority::{host, port, port_u16}`；`HeaderValue::from_bytes` 与 `from_str` 各收哪些字节；`OccupiedEntry::{iter_mut, insert}` 保不保留这个名字在表里的位置；
     - rustls-pki-types 1.15.1：`ServerName` 的派生、`ServerName::try_from(String)` 收不收下划线。
4. **数**：
   - `Cargo.lock` 里 `[[package]]` 的个数（520）；
   - `unsafe_functions.rs` 的 `LISTED` 的项数（23）；
   - `sockets.rs` 的 `LISTED` 的行数（26）；
   - 带本机账户名的文件数：派工文件给出要数的那个名字，数出来是 26。
5. **现有用例**（P20 的表）：逐条找到、读它断言什么，把"预计要改 / 预计照过"逐条写进修正记录，一条用例一句。表里没有的照实补一行。另扫一遍：`git grep -n 'URL Rewrite\|Header Rewrite'`、`git grep -n 'W_DEFERRED_SECTION'`、`git grep -n 'Routed::Go'`、`git grep -n 'impl Upstreams for\|impl<.*> Upstreams for'`、`git grep -n 'features/rewrite'`。
6. **门禁**：跑一遍，记下测试二进制数、通过数、失败数与忽略数（M2c 收尾时是 91 / 5036 / 0 / 13，一台机器上的一次运行，不是 CI）。

**判据**：修正记录里有任务 0 的行；门禁的数字记下。

---

## Task 1：`split_words`、`translate` 与 `Template`

**改哪里**：

- `crates/rurge-config/src/value.rs`：`Word`、`split_words` 与测试模块里的用例。
- `crates/rurge-config/src/dialect.rs`（新）：`translate`、`Template`，测试模块在文件末尾。
- `crates/rurge-config/src/lib.rs`：`pub mod dialect;`，`pub use dialect::Template;`。
- `crates/rurge-config/src/rule.rs`：`Pattern::new` 与它的文档注释；测试模块一条用例。
- `crates/rurge-config/src/redact.rs`：测试模块一条用例（生产代码不改）。

**接口**（产出）：

```rust
// value.rs
pub struct Word { pub text: String, pub start: usize, pub end: usize, pub param: Option<(String, String)>, pub quoted: bool }
pub fn split_words(text: &str) -> Vec<Word>;
// dialect.rs
pub fn translate(source: &str) -> Result<String, String>;
pub struct Template { /* P4 */ }
impl Template {
    pub fn parse(text: &str) -> Template;
    pub fn expand<'a>(&self, group: &dyn Fn(usize) -> Option<&'a str>, count: usize, out: &mut String);
}
// rule.rs：签名不变
impl Pattern { pub fn new(source: &str) -> Result<Pattern, String>; }
```

**用例（先红）**

- `value.rs`（P2）：
  - `split_words_cuts_at_blanks_outside_quotes`：`"a  b\tc"` → 三项，`text` 是 `a`、`b`、`c`，`(start, end)` 是 `(0, 1)`、`(3, 4)`、`(5, 6)`，`quoted` 都是假，`param` 都是 `None`。
  - `split_words_reads_both_quotes`：`"\"a b\" 'c d' e"` → `text` 是 `a b`、`c d`、`e`；`quoted` 是真、真、假；`(start, end)` 是 `(0, 5)`、`(6, 11)`、`(12, 13)`。
  - `split_words_unescapes_only_inside_double_quotes`：`r#""a\"b\\c\d" 'x\y'"#` → `text` 是 `r#"a"b\c\d"#` 与 `r"x\y"`。
  - `split_words_reads_a_quoted_value_after_the_first_equals`：`r#"data="a b" k=v x"#` → 三项；第一项 `text` 是 `r#"data="a b""#`（原文）、`param` 是 `("data", "a b")`、`quoted` 假；第二项 `param` 是 `("k", "v")`；第三项 `param` 是 `None`。
  - `an_unpaired_quote_runs_to_the_end`：`r#"a "b c"#` → 两项，第二项 `text` 是 `r#""b c"#`、`end` 是全文的长度、`quoted` 假。
  - `a_quote_inside_a_word_is_a_character`：`r#"a"b c"d"#` → `text` 是 `r#"a"b"#` 与 `r#"c"d"#`。
  - `an_empty_quoted_word_is_kept`：`r#"x """#` → 两项，第二项 `text` 是空串、`quoted` 真。
  - `""` 与 `"   "` → 空的 `Vec`。
- `dialect.rs`（P3、P4）：
  - `translate_rewrites_the_icu_escapes`：表后「原文」P3 类外那一块的前八行各一条 `assert_eq!(translate(写的), Ok(改成的.to_string()))`，另有 `\Qa.b` → `a\.b`（没有 `\E` 到末尾）、`\Q a\E` → `\x{20}a`、`\Q\E` → 空串。
  - `translate_leaves_everything_else`：`\\h`、`\d+\.`、`[a-z]`、`(?i)abc`、`\x{41}`、`a\b` 原样交回。
  - `translate_inside_a_class`：`[\h]` → `[\t\p{Zs}]`；`[a\v]` → `[a\n\x0B\f\r\x{85}\x{2028}\x{2029}]`；`[\<\>]` → `[<>]`；`[\Q-]\E]` → `[\-\]]`；`[]\h]` → `[]\t\p{Zs}]`；`[^]\h]` → `[^]\t\p{Zs}]`；`[[:alpha:]\h]` → `[[:alpha:]\t\p{Zs}]`。
  - `translate_refuses_three_inside_a_class`：`[\H]`、`[\V]`、`[\R]` → `Err`，消息恰好是「原文」P3 字符类那一块的那一句（另两个只换字母）。
  - `translated_patterns_mean_what_icu_means`：经 `rurge_config::rule::Pattern::new`：
    - `r"a\hb"` 配 `"a b"`、`"a\tb"`，不配 `"aab"`；`r"a\Hb"` 配 `"aab"`、不配 `"a b"`；
    - `r"a\vb"` 配 `"a\nb"`、`"a\u{2028}b"`；`r"\R"` 在 `"\r\n"` 上的第一个匹配是整个 `"\r\n"`；
    - `r"a\<b\>"` 配 `"a<b>"`；`r"\Qa.b\E"` 配 `"a.b"`、不配 `"axb"`；
    - `r"x\Z"` 配 `"x"`、`"x\n"`、`"x\r\n"`，不配 `"x\ny"`；
    - `r"^\Q\.+*?()|[]{}^$#&-~\E$"` 配恰好 `r"\.+*?()|[]{}^$#&-~"` 这一串。
  - `template_expands_like_nsregularexpression`：辅助函数 `fn expand(t: &str, groups: &[Option<&str>]) -> String`，`count = groups.len() - 1`、`group(i) = groups.get(i).copied().flatten()`：
    - `groups = [Some("w"), Some("A"), Some("B")]`：`$1-$2` → `A-B`；`$12` → `A2`；`$3` → 空；`$0` → `w`；`a\$b` → `a$b`；`\\` → `\`；`\n` → `n`；`x\` → `x`；`${name}` → `${name}`；`$x` → `$x`；`$` → `$`；`$1$1` → `AA`。
    - 十个分组（第 1 ～ 10 组是 `A` ～ `J`）：`$10` → `J`；`$1` → `A`；`$100` → `J0`；`$01` → `A`。
    - `groups = [Some("w"), None]`：`[$1]` → `[]`。
  - `words_and_templates_show_nothing_in_debug`：`format!("{:?}", Template::parse("s3cr3t"))` 恰好是 `Template(***)`；`split_words("s3cr3t")` 的第一项的 `Debug` 恰好是 `Word(***)`。
- `rule.rs`：`url_regex_rules_read_icu_escapes`：读 `URL-REGEX,^http://x\.test/a\hb,DIRECT`（照这个文件里读规则的用例的写法），得到的 `Pattern` 的 `source` 恰好是 `^http://x\.test/a\hb`，它配 `http://x.test/a b`、不配 `http://x.test/a1b`（今天的 `\h` 是十六进制数字，所以这条先红）。
- `redact.rs`：`rewrite_lines_are_shown_as_written`，不要求先红。输入是下面五行，`assert_eq!(redact_profile(input), input)`：

  ```
  [URL Rewrite]
  ^http://a\.test/(.*) "http://b.test/$1" header
  '^http://c\.test' _ reject
  [Header Rewrite]
  http-request ^http://a\.test header-add X-Trace "v1 v2"
  http-response '^http://a\.test' header-replace-regex Set-Cookie 'n=\d+' "n=0"
  ```

  若不相等，**停下报告**：那说明设计第 4.3 节"这几个节的行照写的显示"不实，`redact.rs` 要不要改归协调者。

**实现**

- `split_words`：照 P2，复用 `wholly_quoted` 与 `partner`，不复制它们。`Word` 的 `Debug` 照 `Field`，写 `Word(***)`。
- `translate`、`Template`：照 P3、P4。`Template` 派生 `Clone, PartialEq, Eq`，`Debug` 手写成 `Template(***)`。
- `Pattern::new`：先 `translate(source)?`，再 `fancy_regex::Regex::new(&translated)`；`source` 留着写的那一串（`PartialEq` 与 `Debug` 照旧按它）。文档注释写：改写的是 ICU 与这个正则包意思不同的几种写法（`phase 4 M3 design 5.2`），`URL-REGEX` 规则、`policy-regex-filter`、`policy-priority` 与四个重写的节共用它。
- **提交说明**写设计第 4.3 节的理由（`redact.rs` 不分节、每一行同样地读；这几个节的行照阶段 1 的裁定照写的显示；新的切法不改变脱敏藏什么；要不要藏是所有者的 Q2），并说加了 `rewrite_lines_are_shown_as_written` 钉住它（`CLAUDE.md`「一行怎样拆成各项」）。

**判据**

- 上面的用例都在、都绿；`URL-REGEX` 与策略组的现有用例照过（P20）。
- `redact.rs` 的生产代码一行没改（`git diff` 只有测试模块）。
- 门禁绿。

---

## Task 2：`[URL Rewrite]` 与 `[Header Rewrite]` 的读法、`[Replica]`

**改哪里**：

- `crates/rurge-config/src/rewrite.rs`（新）：类型、两个节的读法、测试模块。
- `crates/rurge-config/src/lib.rs`：`pub mod rewrite;`，`pub use rewrite::{Direction, HeaderAction, HeaderRewrite, RewriteConfig, UrlMode, UrlRewrite};`。
- `crates/rurge-config/src/config.rs`：`Config.rewrite`；读两个节；`[Replica]` 的 `I0005`；测试模块里两条用例。
- `crates/rurge-config/src/text/mod.rs`：`KEY_VALUE_SECTIONS` 加 `"Replica"`。
- `crates/rurge-config/src/deferred.rs`：`DEFERRED` 去掉 `"URL Rewrite"`、`"Header Rewrite"`；它的用例。
- `crates/rurge-config/src/diagnostic.rs`：两个码、`hint` 与它的文档注释、测试的两张名单。
- `crates/rurge-config/tests/snapshots/corpus__corpus__kitchen-sink.snap`：手改（P20）。

**接口**

- 消费：任务 1 的 `split_words`、`Word`、`Template`、`Pattern::new`。
- 产出（`rurge_config::rewrite`）：

```rust
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RewriteConfig { pub url: Vec<UrlRewrite>, pub header: Vec<HeaderRewrite> }
impl RewriteConfig { pub fn is_empty(&self) -> bool }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UrlMode { Header, Found, Temporary, Reject }      // header / 302 / 307 / reject
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UrlRewrite { pub pattern: Pattern, pub replacement: Template, pub mode: UrlMode, pub gateway: bool, pub span: Span }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction { Request, Response }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderRewrite { pub direction: Direction, pub pattern: Pattern, pub action: HeaderAction, pub span: Span }
#[derive(Clone, PartialEq, Eq)]   // Debug 手写，不显示值（P6）
pub enum HeaderAction {
    Add { name: String, value: String },
    Del { name: String },
    Replace { name: String, value: String },
    ReplaceRegex { name: String, regex: Pattern, template: Template },
}
impl HeaderAction { pub fn name(&self) -> &str; pub fn verb(&self) -> &'static str }
pub const GATEWAY_ADDRESS: &str = "{{{GATEWAY_ADDRESS}}}";
pub fn parse_url<'a>(entries: impl IntoIterator<Item = &'a Entry>, diags: &mut Diagnostics) -> Vec<UrlRewrite>;
pub fn parse_header<'a>(entries: impl IntoIterator<Item = &'a Entry>, diags: &mut Diagnostics) -> Vec<HeaderRewrite>;
```

- `Config.rewrite: RewriteConfig`；`codes::W_REWRITE_FRAMING`（`"W0041"`）、`codes::I_REPLICA_REMOVED`（`"I0005"`）。

**用例（先红）**

- `rewrite.rs`（读法照 P6，消息照 P7；用 `parse_str` 造 `Entry`，照 `ip_rewrite.rs` 的测试模块）：
  - `url_rewrite_reads_the_manual_example`：手册的三行 → 三条，`mode` 依次是 `Header`、`Found`、`Reject`，`pattern.source` 依次是 `^http://www\.google\.cn`、`^http://yachen\.com`、`^http://ad\.com/ad\.png`；没有诊断。
  - `url_rewrite_modes_and_shapes`：
    - `^a b 307` → `Temporary`；`^a b HEADER` → `Header`；`^a b` → `Header`；
    - `^a`、`^a b c d`、`^a b 301` → 各一条 `W0012`，消息恰好是 P7 的形状那一句，行不进结果；
    - `( b` → 一条 `W0012`，消息以 `[URL Rewrite]: the regular expression cannot be used (` 开头；
    - `^a "x y" 302` → 替换展开是 `x y`；
    - `^a http://{{{GATEWAY_ADDRESS}}}/ 302` → `gateway` 真；`^a http://b/ 302` → `gateway` 假。
  - `header_rewrite_reads_the_manual_examples`：手册的四行（`http-request … header-add DNT 1`、`… header-del Cookie`、`… header-replace User-Agent Unknown`、`http-response … header-replace-regex Date 2022 2023`）与省略方向的 `^http://example.com header-add DNT 1`：方向、动作、名字、值（`1`、`Unknown`）、`regex.source`（`2022`）、模板展开（`2023`）逐条断言；没有诊断。
  - `a_header_value_is_the_rest_of_the_line`：
    - `^a header-add User-Agent Mozilla/5.0 (X; Y) Z` → 值 `Mozilla/5.0 (X; Y) Z`；
    - `^a header-add X "a b"` → 值 `a b`；`^a header-add X ""` → 值是空串；
    - `^a header-add X "a" "b"` → 值 `"a" "b"`（两项，余下的部分照写）；
    - `^a header-add X`、`^a header-replace X` → `W0012`，消息是 P7 那句缺值的。
  - `header_rewrite_refuses`：
    - `^a header-set X 1`、`^a`、`^a header-add` → `W0012` 形状那一句；
    - `^a header-add X:Y 1`、`^a header-add :authority 1` → `W0012` 名字那一句；
    - `^a header-del X extra` → `W0012` `header-del` 那一句；
    - `^a header-replace-regex X a`、`^a header-replace-regex X a b c` → `W0012` `header-replace-regex` 那一句；
    - `^a header-replace-regex X ( b` → `W0012` 正则那一句；
    - 值里有 `\u{1}` 的 `^a header-add X a\u{1}b-s3cr3t` → `W0012` 值那一句，**消息里没有 `s3cr3t`**。
  - `framing_headers_are_kept_with_w0041`：`^a header-del Transfer-Encoding` 与 `^a header-replace content-length 5` → 各留一条规则、各一条 `W0041`（级别警告，消息与 `hint` 恰好是 P7 的）。
  - `the_debug_of_header_rewrites_shows_no_value`：`header-add X-Token s3cr3t-v` 与 `header-replace-regex X-Token a t3mpl` 两条的 `HeaderRewrite` 的 `{:?}` 里没有 `s3cr3t-v` 与 `t3mpl`，有 `X-Token`。
- `config.rs`：
  - `rewrite_sections_are_read_and_no_longer_deferred`：照 `ip_rewrite_is_read_in_order` 的写法读两份 Profile，一份的节头是 `[URL Rewrite]` 与 `[Header Rewrite]`，另一份是 `[url rewrite]` 与 `[HEADER REWRITE]`（同一份里写两个同名的节，`Profile::section` 只认第一个）。每一份：没有错误；没有 `W0016`、`W0002`、`W0012`；`unknown_sections` 空；`deferred.get("URL Rewrite")` 与 `get("Header Rewrite")` 都是 `None`；`config.rewrite.url` 与 `.header` 各一条。
  - `replica_is_known_and_ignored`：`[Replica]\nhide-apple-request=true\n[Replica]\nx=1\n[Rule]\nFINAL,DIRECT\n` → 恰好一条 `I0005`，级别提示，消息恰好是 P7 那句，`span` 是第一个 `[Replica]` 的节头；没有 `W0002`、`W0016`；`unknown_sections` 空。
- `deferred.rs` 的 `prefix_and_membership_matching`：加 `assert!(!is_deferred("URL Rewrite"))`、`assert!(!is_deferred("header rewrite"))`、`assert!(is_deferred("Body Rewrite"))`、`assert!(is_deferred("Map Local"))`，注释照它的写法写 `// typed since phase 4 M3a`。
- `diagnostic.rs` 现有的 `every_code_is_decided_once`：加了常量而没有做决定时它变红，这就是它的"先红"。
- 语料库快照：`kitchen-sink.conf` 的两个节的行都读得了，`W0016` 的消息与 `deferred` 少了这两节（P20）。

**实现**

- 读法照 P6；诊断的码、消息、`span` 与 `hint` 照 P7；`[Replica]` 照 P8。
- `config.rs`：在 `[IP Rewrite]` 那一段之后，照它的写法读两个节（`profile.section("URL Rewrite").into_iter().flat_map(Section::active_entries)`），结果放进 `Config.rewrite`；字段的文档注释写"`[URL Rewrite]` 与 `[Header Rewrite]` 读得了的行，照写的次序（phase 4 M3 design 6.1、7.1）；`[Body Rewrite]` 与 `[Map Local]` 到 M3b"。
- 快照照 M2a P25 的做法手改：删掉第 98、99 行（`- URL Rewrite`、`- Header Rewrite`），第 112 行的消息删掉 `[URL Rewrite], [Header Rewrite], `。不跑 `cargo insta accept`。

**判据**

- 上面的用例都在、都绿；`ConfigSummary` 的形状不变。
- `W0041` 在 `PER_SITE`，`I0005` 在 `WITHOUT`，`hint` 的文档注释提到两者。
- 门禁绿。

---

## Task 3：`rurge_http::rewrite`

**改哪里**：

- `crates/rurge-http/src/rewrite/mod.rs`（新）：`Rewrite`、`RewriteCounters`、`RewriteCounts`、`Note`、`REWRITE_HANDSHAKE`。
- `crates/rurge-http/src/rewrite/url.rs`（新）：`candidates`、`NewUrl`、`Rewrite::url` 的求值、`retarget_request`、`redirect`。
- `crates/rurge-http/src/rewrite/header.rs`（新）：`Rewrite::headers` 的求值、`Headers`、`HostChange`、`write_back_host`。
- `crates/rurge-http/src/lib.rs`：`pub mod rewrite;`，`pub use rewrite::{Rewrite, RewriteCounters, RewriteCounts};`。
- `crates/rurge-http/src/mitm/decrypt.rs`：`server_name` 改成 `pub(crate)`。
- `crates/rurge-http/src/engine.rs`：`default_port`、`origin_authority`、`path_and_query` 改成 `pub(crate)`（只改可见性）。

**接口**

- 消费：任务 2 的 `RewriteConfig`、`UrlRewrite`、`UrlMode`、`HeaderRewrite`、`HeaderAction`、`Direction`、`GATEWAY_ADDRESS`；任务 1 的 `Template::expand`；`rurge_config::MitmConfig`（`h2`、`skip_server_cert_verify`）；`rurge_proto::transport::tls::origin_client_config`；`rurge_proto::BuildError`；`mitm::decrypt::{OriginTls, server_name}`。
- 产出（任务 4、5、6 用）：

```rust
// rewrite/mod.rs
#[derive(Debug, Default)]
pub struct RewriteCounters { /* url, header, body, map_local, skipped, failed: AtomicU64 */ }
impl RewriteCounters {
    pub fn snapshot(&self) -> RewriteCounts;
    pub(crate) fn url(&self); pub(crate) fn header(&self); pub(crate) fn skipped(&self); pub(crate) fn failed(&self);
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RewriteCounts { pub url: u64, pub header: u64, pub body: u64, pub map_local: u64, pub skipped: u64, pub failed: u64 }
pub(crate) const REWRITE_HANDSHAKE: Duration = Duration::from_secs(30);
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Note { Done(String), Skipped(String), Failed(String) }
pub struct Rewrite { /* P9 */ }
impl Rewrite {
    pub fn build(cfg: &RewriteConfig, mitm: &MitmConfig, roots: Arc<RootCertStore>, counters: Arc<RewriteCounters>) -> Result<Option<Rewrite>, BuildError>;
    pub(crate) fn counters(&self) -> &RewriteCounters;
    pub(crate) fn origin_tls(&self, name: ServerName<'static>) -> OriginTls;
    pub(crate) fn has_response_rules(&self) -> bool;
    pub(crate) fn url(&self, candidates: &[String], gateway: &dyn Fn() -> Option<Ipv4Addr>, notes: &mut Vec<Note>) -> UrlOutcome;
    pub(crate) fn headers(&self, direction: Direction, url: &str, headers: &mut HeaderMap, notes: &mut Vec<Note>) -> Headers;
}
// rewrite/url.rs
pub(crate) enum UrlOutcome { Go(Option<NewUrl>), Redirect { status: StatusCode, location: HeaderValue }, Reject }
pub(crate) struct NewUrl { /* P10 */ }
impl NewUrl {
    pub(crate) fn parse(text: &str) -> Option<NewUrl>;
    pub(crate) fn url(&self) -> &str;                       // 规整之后的全文，第 8.1 节"当前的 URL"
    pub(crate) fn tls(&self) -> bool;                       // https / wss
    pub(crate) fn target(&self) -> Target;
    pub(crate) fn server_name(&self) -> Option<&ServerName<'static>>;   // tls() 时 Some
    pub(crate) fn note(&self) -> String;                    // 备注里的样子，不带查询串
}
pub(crate) fn candidates(scheme: &str, url: &str, host: Option<&HeaderValue>, path_and_query: &str, target: &HostName, port: u16) -> Vec<String>;
pub(crate) fn retarget_request<B>(req: &mut Request<B>, new: &NewUrl, h2_client: bool);
pub(crate) fn redirect(status: StatusCode, location: HeaderValue) -> Response<ResponseBody>;
// rewrite/header.rs
pub(crate) struct Headers { pub(crate) changed: bool, pub(crate) framing: bool, pub(crate) host: HostChange }
pub(crate) enum HostChange { Same, Changed(HeaderValue) }
pub(crate) fn write_back_host<B>(req: &mut Request<B>, host: &HeaderValue, version: Version) -> bool;
```

**用例（先红）**（都在 `rewrite/` 各文件末尾的测试模块里；规则用 `rurge_config` 的 `parse_str` 与 `rewrite::parse_url` / `parse_header` 造，或直接构造）

- `url.rs`：
  - `candidates_are_the_url_the_host_and_the_target`：
    - `("http", "http://a.test/x?q=1", Some("b.test"), "/x?q=1", c.test, 80)` → `["http://a.test/x?q=1", "http://b.test/x?q=1", "http://c.test/x?q=1"]`；
    - `Host` 与 URL 的主机相同 → 两项；目标也相同 → 一项；
    - 目标端口 8080 → 第三项 `http://c.test:8080/x?q=1`；`("https", …, 443)` → 不带端口；
    - 没有 `Host` → `[URL, 目标重建的]`；`Host` 不是 UTF-8 的字节 → 没有第二项。
  - `only_the_first_header_rule_applies`：规则依次是 `^http://a\.test http://b.test header`、`^http://a\.test http://c.test header`、`^http://b\.test https://d.test/ 302`；候选 `["http://a.test/x"]` → `Redirect { 302, "https://d.test//x" }`（第三条比的是新的 URL `http://b.test/x`：`http://b.test` 换成 `https://d.test/`，余下的 `/x` 接上）；第二条不求、没有备注；备注恰好是 `[Done("URL Rewrite: header by ^http://a\.test -> http://b.test/x"), Done("URL Rewrite: 302 by ^http://b\.test")]`。
  - `an_invalid_header_result_does_not_count_as_the_first`：`^http://a\.test ftp://b.test header` 之后 `^http://a\.test http://c.test header` → `Go(Some(_))`，`url()` 是 `http://c.test/x`；备注是一条 `Skipped(…not a valid http, https, ws or wss URL)` 与一条 `Done`。
  - `which_rewritten_urls_are_valid`（P10）：
    - 合法：`http://a.test/`；`HTTPS://a.test:8443/p?q`（`tls()` 真，`target()` 是 `a.test:8443`）；`ws://a.test`（端口 80，`url()` 是 `ws://a.test/`）；`wss://[::1]:9/`（目标 `::1` 的 9 号端口）；`http://u:p@a.test/`（`url()` 与 `note()` 里没有 `u:p`）；`http://a.test/p#f`（`url()` 是 `http://a.test/p`）；`http://a.test:80/`（`url()` 是 `http://a.test/`）；
    - 不合法：`ftp://a.test/`、`http:///x`、`http://a.test:0/`、`http://a.test:65536/`、`/relative`、`a.test`、空串。
  - `retargeting_keeps_the_form_and_sets_host`：
    - 源站形式的 HTTP/1.1 请求 `/a?x`、`Host: a.test`，新 URL `http://b.test:8080/p?q` → URI `/p?q`、`Host: b.test:8080`（只一行）；
    - 同上而没有 `Host` → 加上 `Host: b.test:8080`；
    - 绝对的 URI `http://a.test/a` → URI `http://b.test:8080/p?q`；
    - HTTP/2 的请求（URI `https://a.test/a`，没有 `Host`，`h2_client` 真），新 URL `https://b.test/p` → URI `https://b.test/p`，没有加 `Host`；
    - 新 URL `ws://b.test/s` 与 `wss://b.test/s` → 绝对的 URI 分别是 `http://b.test/s` 与 `https://b.test/s`。
  - `redirects_put_the_gateway_in_and_give_up_without_one`：`^http://r\.test http://{{{GATEWAY_ADDRESS}}}/login 307`、之后 `^http://r\.test https://s.test/ 302`：
    - 网关是 `Some(127.0.0.2)` → `Redirect { 307, "http://127.0.0.2/login/x" }`（候选 `http://r.test/x`）；
    - 网关是 `None` → 第一条一条 `Skipped("URL Rewrite skipped: ^http://r\.test: there is no IPv4 gateway")`，结果是第二条的 `Redirect { 302, … }`；
    - 网关的闭包只在第一条匹配时被调一次（用一个计数的闭包数）。
  - `a_location_that_cannot_be_a_header_value_is_skipped`：直接构造一条 `UrlRewrite`，替换是 `Template::parse("a\u{1}b")`、模式 `Found` → `Go(None)` 与一条 `Skipped("…the rewritten Location cannot be a header value")`。
  - `reject_answers_and_ends_the_rules`：`^http://ad\.com/ad\.png _ reject` 之后一条会命中的 `header` 规则 → `Reject`，备注只有 `Done("URL Rewrite: reject by ^http://ad\.com/ad\.png")`。
  - `a_rule_that_hits_the_backtracking_limit_is_skipped_with_a_note`：式子 `(?i)(a|b|ab)*(?=c)`，URL 是 `http://x.test/` 接 40 个 `ab`；用例先断言 `pattern.regex.is_match(url)` 是 `Err`（说明这个输入确实超限），再断言 `url()` 是 `Go(None)`、备注恰好一条 `Skipped("URL Rewrite skipped: (?i)(a|b|ab)*(?=c): the regular expression hit its backtracking limit")`。
- `header.rs`：
  - `the_four_actions`（名字不分大小写，用 `dnt` 去配 `DNT`）：
    - `header-add DNT 1` 在已经有 `DNT: 0` 时 → 两行，`0` 在前、`1` 在后；
    - `header-del Cookie` 删掉两行 `Cookie`；
    - `header-replace User-Agent Unknown`：没有时什么也不变（`changed` 假、没有备注）；有两行时变成一行 `Unknown`，在原来第一行的位置；
    - `header-replace-regex User-Agent Safari Chrome`：手册的 User-Agent 原文 → 手册的结果原文；
    - `header-del DNT` 之后 `header-add DNT 1`（手册"use `header-del` and `header-add` together"）→ 恰好一行 `DNT: 1`。
  - `a_replace_regex_result_that_is_no_header_value_leaves_the_value`：模板是 `Template::parse("\u{1}")` 的一条 → 值不变，一条 `Skipped("Header Rewrite skipped: ^…: header-replace-regex made a value a header cannot carry")`。
  - `framing_changes_are_found`：
    - 头里有 `Transfer-Encoding: chunked`，`header-del Transfer-Encoding` → `framing` 真，一条 `Failed("Header Rewrite failed: ^…: Content-Length or Transfer-Encoding changed")`，它之后的规则不再作用；
    - 同一条规则作用于没有 `Transfer-Encoding` 的头 → `framing` 假、`changed` 假；
    - `header-add Content-Length 5` → `framing` 真；
    - 已经是 `Content-Length: 5` 时 `header-replace Content-Length 5` → `framing` 假。
  - `a_changed_host_is_the_last_line_and_a_removed_one_comes_back`（方向是请求）：
    - `header-replace Host b.test` → `HostChange::Changed("b.test")`；
    - 有 `Host: a.test` 时 `header-add Host c.test` → `Changed("c.test")`，表里只剩这一行；
    - `header-del Host` → `Same`，`Host: a.test` 回来了，一条 `Skipped("Header Rewrite skipped: Host cannot be removed")`；
    - 方向是应答时，同样的规则 `host` 总是 `Same`。
  - `writing_host_back`：`Version::HTTP_11` → 设 `Host`，交回真；`Version::HTTP_2`（URI `https://a.test/p?q`）→ URI 变成 `https://b.test:8443/p?q`、没有 `Host`，交回真；`Version::HTTP_2` 而值是 `a b` → 交回假，请求不变。
  - `a_header_rule_that_hits_the_backtracking_limit_is_skipped`：URL 的式子超限（同上那个式子与输入）→ 一条 `Skipped`、头不变。
- `mod.rs`：
  - `notes_never_carry_values_or_queries`：URL `http://a.test/p?tok=q1`、规则 `^http://a\.test http://b.test header` 与 `header-add X-Token v4lue-x`、`header-replace-regex X-Token v4lue t3mpl` → 全部备注里没有 `q1`、`v4lue-x`、`t3mpl`；`header` 的那一条恰好是 `URL Rewrite: header by ^http://a\.test -> http://b.test/p`。
  - `build_gives_none_without_lines`：`RewriteConfig::default()` → `Ok(None)`。
  - `build_makes_the_two_tls_configs`：`MitmConfig { h2: true, .. }` → `origin_tls(name)` 的 `config.alpn_protocols` 是 `[b"h2", b"http/1.1"]`、`h1.alpn_protocols` 是 `[b"http/1.1"]`、`wait` 是 30 秒、`name` 是给的那个；`h2: false` 时两份都只有 `http/1.1`。
  - `counters_count_and_snapshot`：各加一次，`snapshot()` 的六个数；`body` 与 `map_local` 恒为 0。

**实现**

- 照 P9（结构与求值）、P10（新 URL）、P11（备注的措辞）、P13（`302` / `307` 的应答）。
- 全部替换用 `captures_iter` 自己拼（设计第 5.5 节）：每个 `Ok(caps)` 先接上一段原文，再 `template.expand(&|i| caps.get(i).map(|m| m.as_str()), regex.captures_len() - 1, &mut out)`；任何一个 `Err` 都是"回溯超限"，这一条改写放弃（结果丢掉、原文不变）。
- `Rewrite::build`：`cfg.is_empty()` → `Ok(None)`；否则照 `Mitm::build` 建两份客户端配置（`origin_client_config(roots, mitm.skip_server_cert_verify, alpn)`，ALPN 照 `mitm.h2`），把头的名字与值转成 `HeaderName` / `HeaderValue` 存起来（`rurge-config` 已经校验过；万一转不成，这一条丢掉，DEBUG 一行只写名字）。

**判据**

- 上面的用例都在、都绿；这个模块里没有网络、没有 `tokio::spawn`。
- 门禁绿。

---

## Task 4：每个请求的对上游 TLS 与池的键

**改哪里**：`crates/rurge-http/src/engine.rs`；`crates/rurge-http/src/mitm/decrypt.rs`（`OriginTls` 派生 `Clone`）。**行为不变**：现有用例全部照过（P20）。

**接口**

- 产出（任务 5 用）：

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum OriginKey { Plain, Tls(ServerName<'static>) }
impl OriginKey { pub(crate) fn of(origin: Option<&OriginTls>) -> OriginKey; }
// Shared：pool: Mutex<Pool<(U::Key, OriginKey), Pooled>>，h2: Mutex<Vec<SharedH2<(U::Key, OriginKey)>>>
// connect(&self, handle, route, origin: Option<&OriginTls>, h1_only)
// redial(&self, guard, pages, route, key: &mut Option<(U::Key, OriginKey)>, origin: Option<&OriginTls>, h1_only)
// Resend 多 origin: Option<&'a OriginTls>；key 的类型随之改
```

**用例**

- `origin_keys_tell_plain_from_tls_and_names_apart`（先红，`OriginKey` 还不存在）：`of(None)` 是 `Plain`；名字 `a.test` 的 `OriginTls` 是 `Tls(a.test)`，不等于名字 `b.test` 的，也不等于 `Plain`；两份配置不同而名字相同的两个 `OriginTls` 的键相等（P14）。
- 现有的全部用例照过：它们就是这一步"行为不变"的证据。

**实现**

- 照 P14。`handle` 里用 `let origin = self.shared.conn.origin.clone();`，键是 `(key, OriginKey::of(origin.as_ref()))`；`connect` 用传进来的 `origin`，不再读 `self.shared.conn.origin`；`redial` 与 `retry` 把它传下去。
- 文档注释写：键里有 `OriginKey`，是因为重写能改 scheme 与名字（`phase 4 M3 design 8.3`）；同一个键下的 TLS 连接是用同一份配置拨的（M3-D31）。

**判据**

- 新用例绿；现有用例一条也没有改期望；`connect` 里不再出现 `self.shared.conn.origin`。
- 门禁绿。

---

## Task 5：引擎里的重写

**改哪里**：

- `crates/rurge-http/src/engine.rs`：`Upstreams` 加两个方法、`Routed::Go` 加 `rewrite`、`handle` 的各步、`retry` 的写回；测试模块里 `Upstreams` 的那一份实现与新用例。
- `crates/rurge-engine/src/http.rs`：`ConnUpstreams` 实现 `retarget` 与 `gateway`；`route` 给 `rewrite: None`（任务 6 接上）。

**接口**

- 消费：任务 3 的 `Rewrite`、`Note`、`Headers`、`HostChange`、`UrlOutcome`、`NewUrl`、`candidates`、`retarget_request`、`write_back_host`、`redirect`；任务 4 的 `OriginKey` 与按请求的 `origin`。
- 产出：

```rust
pub trait Upstreams {
    // 已有的不变
    fn retarget(&self, route: Self::Route, key: Self::Key, target: Target, forwardable: bool) -> (Self::Route, Self::Key);
    fn gateway(&self) -> Option<Ipv4Addr>;
}
pub enum Routed<R, K> {
    Go { route: R, key: K, pages: Pages, rewrite: Option<Arc<Rewrite>> },
    Refused { refusal: Refusal, pages: Pages },
}
```

**用例（先红）**（`engine.rs` 的测试模块，用它的 `Upstreams` 实现与源站夹具；那一份实现的 `retarget` 记下收到的 `target` 与 `forwardable`，`gateway` 交回用例给的值，`route` 交回用例给的 `Option<Arc<Rewrite>>`）

- `request_headers_are_rewritten_before_the_hop_by_hop_ones_go`：`header-add X-A 1` 与 `header-add Keep-Alive x` → 源站收到 `X-A: 1`，没有 `Keep-Alive`；记录里一条 `Header Rewrite: request header-add X-A by …` 的备注。
- `a_redirect_is_answered_without_a_dial`：`302` 规则 → 客户端收到 302、`Location`、`Content-Length: 0`、空的正文；记录 `statusCode` 302、`Completed`；`dial` 一次也没有被调。`307` 同样。
- `a_url_rewrite_reject_answers_like_reject`：`pages.reject` 真 → 403、标题 `Request rejected`、P12 那句；假 → 连接被关掉；记录都是 `Rejected(Reject)`；`dial` 没有被调。
- `header_mode_sends_the_request_to_the_new_target`：嗅探到的 HTTP/1.1 请求 `/a`、`Host: a.test`，规则 `^http://a\.test http://b.test:{源站端口} header` → `retarget` 收到 `b.test:{源站端口}` 与 `forwardable` 假；源站收到 `GET /a`、`Host: b.test:{源站端口}`；记录的 `url` 还是原来的。
- `a_listener_request_keeps_its_absolute_uri_when_retargeted`：`Entry::Listener` 的请求改到 `http://…` → `forwardable` 真；改到 `https://…` → 假。
- `a_retargeted_sniffed_request_drops_proxy_authorization`：嗅探到的请求带 `Proxy-Authorization` → 换了目标的源站收不到它；没有换目标的（同一个用例里另一条请求）照 M1b 收得到。
- `a_changed_host_is_written_back`：`Entry::Listener` 的请求，`header-replace Host h.test` → 源站收到 `Host: h.test`（不是 `origin_form` 按 URI 设的那个）；`dial` 交回 `forward` 有值（转发的 HTTP 代理）时，收到的 `Host` 是 URI 的 authority、不是 `h.test`。
- `deleting_host_does_not_take`：`header-del Host` → 源站收到原来的 `Host`；一条 `Header Rewrite skipped: Host cannot be removed`。
- `a_request_whose_framing_changes_fails_before_the_dial`：带 `Content-Length` 的 `POST`，`header-replace Content-Length 1` → 502（`show-error-page` 开着时是错误页）、`dial` 没有被调、记录 `Failed("Header Rewrite changed Content-Length or Transfer-Encoding")`、一条 `Failed` 的备注；`pages.error` 假时连接被关掉。
- `a_response_whose_framing_changes_fails`：源站的应答带 `Content-Length`，`http-response … header-del Content-Length` → 客户端收到 502；记录同上；源站的那条连接没有被放回池里（同一个客户端连接上的下一个请求另拨了一次）。
- `connect_and_absolute_form_on_a_sniffed_connection_are_not_rewritten`：嗅探到的连接上客户端发 `CONNECT` 与绝对形式的 `GET http://a.test/`，会命中的 `header-add` 与 `reject` 规则 → 都没有作用、没有备注。
- `the_101_head_is_rewritten`：WebSocket 升级，`http-response … header-add X-W 1` → 客户端收到的 101 带 `X-W: 1`，之后照常透传。
- `deleting_upgrade_makes_a_plain_request`：`header-del Upgrade` → 源站收到的请求没有 `Upgrade`，客户端收到源站的普通应答。
- `https_on_a_plain_connection_uses_the_rewrite_tls`：明文的嗅探到的连接，规则改到 `https://localhost:{TLS 源站端口}/`，`Rewrite` 用 `TlsFixture` 的根建 → 请求经 TLS 到源站、按 `localhost` 校验。
- `the_pool_keeps_plain_and_tls_apart`：同一个客户端连接上先一个明文请求到源站 T（明文的源站），再一个改写到 `https://T` 的请求 → 第二个**没有借第一个的连接**：它对明文的源站握 TLS 失败，客户端收到 502、原因是 `upstream TLS handshake failed`；源站数到两条连接。
- `a_backtracking_rule_does_not_stop_the_request`：会超限的 URL 规则 → 请求照常到源站，一条 `Skipped` 的备注。
- 计数：上面各用例里 `RewriteCounters::snapshot()` 的数照 P11 断言（一个请求的 `url` 只计一次，`header` 两个方向只计一次）。

**实现**

- 照 P15（各步的位置与顺序）、P11（备注与计数）、P12（`reject`）、P13（`302` / `307`）、P16（引擎一侧的两个方法）。
- `retarget` 与 `gateway` 的文档注释写 `phase 4 M3 design 8.2`；`Routed::Go` 的 `rewrite` 的文档注释写"`None`：开关关着、这一代没有规则、或选定的是 REJECT 一族（`phase 4 M3 design 8.4`）"。

**判据**

- 上面的用例都在、都绿；`Upstreams` 的两份实现都有两个新方法。
- 新写的带等待的用例单独循环跑二十次（命令与结果写进报告）。
- 门禁绿。

---

## Task 6：一代的 `Rewrite`、开关、计数与 API

**改哪里**：

- `crates/rurge-engine/src/runtime.rs`：`Runtime.rewrite`、`build_rewrite`。
- `crates/rurge-engine/src/shared.rs`：`EngineShared.rewrite`。
- `crates/rurge-engine/src/engine.rs`：`rewrite_on`、`rewrite_on()`、`set_rewrite`、`restore_switches`、`http_counts`。
- `crates/rurge-engine/src/serve.rs`：`HttpCounts.rewrite`。
- `crates/rurge-engine/src/http.rs`：`route` 给 `rewrite`。
- `crates/rurge-api/src/routes/{features.rs,rurge_http.rs}`。
- 用例：`crates/rurge-api/tests/api.rs`、`crates/rurge/tests/cli.rs`、`crates/rurge-engine/tests/rewrite.rs`（新）。

**接口**

- 消费：任务 3 的 `Rewrite::build`、`RewriteCounters`、`RewriteCounts`；任务 5 的 `Routed::Go.rewrite`。
- 产出：

```rust
// rurge-engine
pub struct EngineShared { /* 已有的 */ pub rewrite: Arc<rurge_http::RewriteCounters> }
impl Engine {
    pub fn rewrite_on(&self) -> bool;
    pub async fn set_rewrite(&self, on: bool);
}
pub struct HttpCounts { /* 已有的 */ pub rewrite: rurge_http::RewriteCounts }
// Runtime（crate 内）：pub(crate) rewrite: Option<Arc<Rewrite>>
```

**用例（先红）**

- `crates/rurge-api/tests/api.rs`：
  - `the_rewrite_switch_is_read_switched_and_kept`：照 `the_mitm_switch_is_read_switched_and_kept` 逐句写，`mitm` 换成 `rewrite`：缺省 `{"enabled": true}`；`POST false` → `{}`；再读是 `false`；`state.json` 的 `version` 是 2、`features.rewrite` 是 `false`；`POST true` 之后读是 `true`。
  - `rurge_http_counts_start_at_zero`：改（P20）——解析之后的正文与字节都多 `rewrite`，字节恰好是设计第 12 节那一串。
- `crates/rurge/tests/cli.rs` 的 `mod run`：`a_daemon_restores_the_rewrite_switch_from_state_json`：照 `a_daemon_restores_the_mitm_switch_from_state_json` 写，`state.json` 是 `{"version":2,"features":{"rewrite":false}}`：读 `/v1/features/rewrite` 是 `{"enabled":false}`；之后 `POST` `{"enabled":true}` 得 `{}`，再读是 `{"enabled":true}`。只读 `false` 那一步今天也绿（不认识的开关读作 `false`），今天 `POST` 答 501，所以整条先红。可执行文件不改。
- `crates/rurge-engine/tests/rewrite.rs`（新；夹具照 P19）：
  - `the_rewrite_switch_turns_every_rewrite_off`：`header-add X-A 1` 与一条 `302` 规则；开关开着 → 源站收到 `X-A`、另一个请求得 302；`set_rewrite(false)` 之后两者都不生效、没有备注、计数不动；再开 → 又生效。
  - `a_rejected_request_is_not_rewritten`：规则选 `REJECT`，另有会命中的 `^http://r\.test https://s.test/ 302` 与 `header-add` → 照 REJECT 答复（403 页或关闭）、记录 `Rejected(Reject)`、没有重写的备注、`http_counts().rewrite` 六个数都是 0。
  - `a_redirect_carries_the_first_ipv4_gateway`：`[Host] g.test = 127.0.0.1`、`DOMAIN,g.test,DIRECT`，URL Rewrite 只有 `^http://g\.test:\d+/ http://{{{GATEWAY_ADDRESS}}}/ 302`，请求 `http://g.test:{源站端口}/x`：
    - `EngineShared.network` 的主出口 `routers` 是 `[fe80::1, 127.0.0.2]` → 302、`Location: http://127.0.0.2/x`，源站没有被连过；
    - 另一个引擎，`primary` 是 `None` → 这条规则放弃、请求照常到源站、一条 `URL Rewrite skipped: ^http://g\.test:\d+/: there is no IPv4 gateway` 的备注、`skipped` 是 1。
  - `counts_survive_a_reload`：一个请求过了 `header-add` 之后重载（同一份 Profile），`http_counts().rewrite.header` 仍是 1，再一个请求之后是 2。

**实现**

- 照 P17（开关、`Runtime.rewrite` 与 `build_rewrite`）、P18（计数与 API 的形状）、P16（`route` 的 `rewrite`）。
- `set_feature` 的 `rewrite` 一支照 `mitm` 那一支写，INFO `rewrite switched via http-api`（带 `enabled`）。`get_feature` 同样。

**判据**

- 上面的用例都在、都绿；`capture` 照旧读作 `false`、`POST` 答 501。
- 新写的带等待的用例单独循环跑二十次。
- 门禁绿。

---

## Task 7：端到端

**改哪里**：`crates/rurge-engine/tests/rewrite.rs`。夹具照 P19；不改生产代码（要改就停下报告）。

**用例**（都是新的；"先红"不适用——它们验证任务 1 ～ 6 已经做好的，一写就绿时照实记；变红就是缺陷，停下报告）

- 手册的黄金用例（规则照手册的原文逐字写，与手册的出入写在用例的注释里，P19）：
  - `the_manual_url_rewrite_header_example`：`^http://www\.google\.cn http://www.google.com header`。经 HTTP 监听器的 `GET http://www.google.cn:{源站端口}/s?q=1`，与经 SOCKS5 到 `www.google.cn:{源站端口}` 的嗅探到的 `GET /s?q=1` 各一遍：源站收到 `Host: www.google.com:{源站端口}` 与路径 `/s?q=1`；记录的 `url` 是原来的；备注恰好是 `URL Rewrite: header by ^http://www\.google\.cn -> http://www.google.com:{源站端口}/s`；记录的规则与策略是改写之前规则选的那一条。
  - `the_manual_url_rewrite_302_and_307_examples`：`^http://yachen\.com https://yach.me 302` 与 `307` 各一遍，请求 `http://yachen.com/`、规则选守卫 → 状态码、`Location: https://yach.me/`、`Content-Length: 0`；守卫没有被连过。
  - `the_manual_url_rewrite_reject_example`：`^http://ad\.com/ad\.png _ reject`，请求 `http://ad.com/ad.png`，规则选指向守卫的策略 → `show-error-page-for-reject` 开着得 403 页、关着连接被关；守卫没有被连过。
  - `the_manual_matching_details_example`：经监听器的 `GET http://127.0.0.1:{守卫端口}/ad.png`、`Host: ad.com`，同一条 `reject` 规则 → 按 `Host` 重建的 URL 命中、被拒绝；守卫没有被连过。
  - `the_manual_header_rewrite_examples`：手册的三组例子都写 `^http://example.com`，放在一份 Profile 里会互相作用，所以三个引擎各读一组：
    - 手册"Example"的四行：请求带两行 `Cookie` 与一个 `User-Agent` → 源站收到 `DNT: 1`、没有 `Cookie`、`User-Agent: Unknown`；源站应答 `Date: Sat, 01 Jan 2022 00:00:00 GMT` → 客户端收到 `Date: Sat, 01 Jan 2023 00:00:00 GMT`；
    - 省略方向的 `^http://example.com header-add DNT 1` → 源站收到 `DNT: 1`；
    - `header-del` 与 `header-add` 连用的两行，请求带 `DNT: 0` → 源站恰好收到一行 `DNT: 1`。
  - `the_manual_header_rewrite_regex_example`：`http-request ^http://example.com header-replace-regex User-Agent Safari Chrome` → 源站收到手册"After"的 User-Agent 原文。
  - `the_manual_header_rewrite_framing_limit`：`http-request ^http://example.com header-del Content-Length`，带正文的 `POST` → 502、源站没有收到请求。
- 其余（设计第 16 节第 3 条属于 M3a 的）：
  - `rewrites_on_a_decrypted_https_request`：MITM 与重写一起：解密的 `https://` 请求过 `header-add` 与 `header` 换到另一个 `https://localhost:{另一个 TLS 源站端口}/`。
  - `a_decrypted_http2_request_can_be_sent_to_plain_http`：讲 HTTP/2 的客户端的解密请求改写到 `http://localhost:{明文源站端口}/` → 明文的源站收到 HTTP/1.1 的请求、`Host` 是新的 authority。
  - `a_changed_authority_reaches_an_http2_origin`：解密的 HTTP/2 请求，`header-add Host h.test:{端口}`（HTTP/2 的客户端没有 `Host`）→ 讲 HTTP/2 的源站收到 `:authority` 是 `h.test:{端口}`。
  - `http_rewritten_to_https_reaches_a_tls_origin_without_a_ca`：Profile 没有 `[MITM]`，根经 `EngineShared.roots` 给 → `http://` 改写到 `https://localhost:{TLS 源站端口}/` 成功。
  - `plain_and_tls_requests_to_one_target_do_not_share_a_connection`：同一条客户端连接上明文与改写成 TLS 的两个请求到同一个回环目标（P19：一个是明文的源站）→ 源站数到两条连接，TLS 的那个 502。
  - `a_listener_request_rewritten_through_a_forwarding_proxy`：策略是 `always-use-connect = false` 的 HTTP 代理，代理是测试手写的回环服务端、记下请求行：改到 `http://new.test:{端口}/p` → 代理收到 `GET http://new.test:{端口}/p HTTP/1.1`；改到 `https://new.test:{端口}/p` → 代理收到 `CONNECT new.test:{端口} HTTP/1.1`。
  - `a_changed_host_reaches_the_origin`：经监听器来的、嗅探到的 HTTP/1.1 各一个，`header-replace Host h.test` → 源站收到 `Host: h.test`；`header-del Host` → 源站收到原来的。
  - `an_absolute_form_request_on_a_sniffed_connection_is_not_rewritten`。
  - `a_retargeted_request_carries_no_proxy_authorization`。
  - `a_regex_that_backtracks_too_long_skips_the_rewrite`：同任务 3 的式子与输入 → 请求照常到源站，一条备注，`skipped` 是 1。
  - `no_header_value_reaches_the_notes_or_the_log`（`LogCapture`，日志级别开到 TRACE）：请求带 `Authorization: old`，`header-replace Authorization Bearer s3cr3t-z`，URL 带 `?tok=q1` → 源站收到 `Authorization: Bearer s3cr3t-z`；抓到的日志、记录的备注里都没有 `s3cr3t-z`，备注里没有 `q1`。
  - `notes_and_counts_reach_the_api_shape`：几种改写之后 `http_counts().rewrite` 的六个数恰好是期望的。

**判据**

- 上面的用例都在、都绿；生产代码没有改。
- 新写的带等待的用例单独循环跑二十次。
- 门禁绿。

---

## Task 8：M3a 的收尾文档

**改哪里**（设计第 15、17 节，P21）。照 M2c 任务 8 的写法与规矩：引的提交、用例名与数字对着仓库核过；历史的段落不改写；本地提交，CI 没有见过。

- `docs/api/phase4.md`：加「M3a　URL Rewrite 与 Header Rewrite」一节：`/v1/features/rewrite`、`/v1/rurge/http` 的 `rewrite`（六个键与次序，`body`、`mapLocal` 到 M3b 之前是 0）、备注的全部措辞（P11）、`build_rewrite` 的那一行 WARN。
- `docs/testing.md`：「命令」加 M3a 的过滤词（`--lib -- dialect`、`--lib -- rewrite`、`--test rewrite`）；「测试约定（全文）」加阶段 4 / M3a 一节：黄金用例的规则照手册原文、主机名的钉法与端口写在请求里、守卫监听器、根经 `EngineShared.roots`、网关经 `EngineShared.network`。
- `docs/surge-compatibility-matrix.md`：1.x 节 L50、L51（两个节），7.3、7.4 节各行，11.x 节 L871 的 `rewrite`，规则一节 `URL-REGEX` 那一行；设计第 15 节第 1 ～ 9 条与第 13 条登记进去（第 10 ～ 12 条是 M3b 的）；状态标记照规矩；附录照规则整张重数；第 5 行的「最近一次」改成 M3a。
- `docs/status.md`：阶段 4 下加「### M3a（URL Rewrite 与 Header Rewrite）」一节，照 M2c 一节的四段；第四段"去向写着项目所有者的事"列出本计划「延后事项」里去向是项目所有者的行与设计第 21 节。
- `CLAUDE.md`：只改「现在在哪里」阶段 4 那一条：M3a 做完；M1、M2 与 M3a 的提交都在本地，CI 都没有见过；下一步是 M3b 的计划。
- 设计第 22 节：在"写 M3a 计划时"那几条之后加"执行 M3a 时"：修正记录里改了设计正文意思的行，一条一段；没有就写一句"M3a 执行期没有改设计正文的意思"。
- `docs/README.md`：本计划那一项末尾接一句做完。
- 两份 README 与需求文档不改（M3b 收尾时改，设计第 17 节）。

**判据**

- 文档里引的提交、用例名与数字对着仓库核过；
- `git grep -l <账户名>` 数出来的文件数仍是 26；
- 门禁不用跑（只改文档），报告写明；`git diff --check` 无输出。

---

## 计划期决定

写计划时对着 `58ec98f` 核对过下面引的名字与行号。行号会随前面任务的改动漂移，以名字为准；名字不同就照实改、记进修正记录，不改语义。标了"订正设计"的，已随本计划写进设计第 22 节。

| # | 事项 | 决定 |
| - | ---- | ---- |
| P1 | 任务的拆法（订正设计） | 设计第 2 节的 M3a 表是任务 0 ～ 7；本计划拆成 0 ～ 8：<br>0 核对；<br>1 `split_words`、`translate`、`Template`；<br>2 两个节的读法、`[Replica]`；<br>3 `rurge_http::rewrite` 的纯函数；<br>4 每个请求的对上游 TLS 与池的键（行为不变）；<br>5 引擎里的各步；<br>6 引擎的一代、开关、计数与 API；<br>7 端到端；<br>8 收尾。<br>设计的任务 4 拆成本计划的 4 与 5：键与 TLS 的改法能单独被拒，而且先做它，任务 5 的换目标就有按请求的 TLS 可用。<br>脱敏的那一条用例随 `split_words` 在任务 1（设计第 4.3 节"加 `split_words` 的提交……加一条脱敏的用例"；第 2 节把它写在任务 2，以第 4.3 节为准） |
| P2 | `Word` 与 `split_words`（订正设计） | `Word` 比设计第 3 节多一个字段 `quoted: bool`：这一项整个在引号里。`Debug` 写 `Word(***)`（一项可能是头的值）。<br>**项**：引号外的一段空格与制表符分项；开头与末尾的空白不成项。<br>**引号**：`"` 或 `'` 只在一项开始的地方、或这一项里第一个 `=` 的紧后面打开；`"` 里反斜杠连同它后面的一个字一起算，`'` 里什么也不转义（`partner`）；没有配对的引号一直到这一行的末尾，这一项照字面、那个引号也在内；别处的引号是普通的字。<br>**`text`**：整项在引号里时去掉引号（`wholly_quoted`：`"…"` 里 `\"` 是 `"`、`\\` 是 `\`、别的反斜杠照留；`'…'` 照字面），`quoted` 为真；否则是原文。空的引号（`""`）留着，`text` 是空串（`split_fields` 丢掉它们，这里不丢：`header-add X ""` 要它）。<br>**`param`**：不是整项在引号里、而这一项有 `=` 且它前面不空时，是 `(=` 前面的原文`, =` 后面去掉一层引号的值`)`，同 `read_field`；否则 `None`。URL Rewrite 与 Header Rewrite 不读 `param`（URL 的式子里常有 `=`），`[Map Local]` 在 M3b 读它。<br>**`start` / `end`**：字节偏移，`end` 不含，引号在内 |
| P3 | `translate` | 从左到右读，反斜杠与它后面的一个字当一体。九种写法各改成什么、类里的成员、`Err` 的消息、`\Q…\E` 里要转义的 18 个字，**照抄表后「原文」的 P3 各块**（那些文字含竖线，写在表格里要转义，照抄会抄错）。<br>**类外**：那九种照「原文」改写；别的反斜杠连同后一个字照抄；`[` 打开字符类，紧跟它的 `^` 与再紧跟的 `]` 是类的一部分（`]` 照字面）。<br>**`\Q`**：打开照字面的一段，到下一个 `\E` 为止、没有就到末尾；段里的反斜杠也照字面（`\Q\.\E` 是反斜杠与点两个字）；段里每个字：是那 18 个字之一就前加反斜杠，是空白（`char::is_whitespace`）就写成 `\x{十六进制}`（`(?x)` 下空白与 `#` 也就照样是字面），别的照写。<br>**类里**：`[` 加一层（`[[:alpha:]]` 因此成对）、`]` 减一层，减到 0 回到类外；`\h`、`\v` 换成它们的成员（不带方括号）；`\<`、`\>` 换成 `<`、`>`；`\Q…\E` 同类外；`\H`、`\V`、`\R` 返回 `Err`；别的照抄。<br>没有闭合的类照抄到末尾，交给正则包去报错。<br>依据：fancy-regex 0.14 `parse_escape`（`\h` 是十六进制数字、`\v` 是竖制表符、`\<` `\>` 是词的边界、`\Z` `\Q` 是错误；字母以外的字前加反斜杠照字面）；regex-syntax 0.8.11 `src/lib.rs` 的 `is_meta_character`（18 个字） |
| P4 | `Template` | 照 swift-corelibs-foundation 的 `replacementString` 的行为（只取行为，不抄代码）。<br>**`parse`**（任何文字都合法）：从左到右：`\` 接一个字 → 那个字照字面（不再参与展开）；结尾单独的 `\` → 丢掉；`$` 接一串 ASCII 数字 → 一个"组"项，记下**整串数字**；`$` 后不是数字 → 字面的 `$`；别的照字面。<br>**`expand(group, count, out)`**：D = `count` 的十进制位数、至少 1（`count` 不超过 9 时 1 位，10 ～ 99 时 2 位……，与 Foundation 的 `numberOfRanges = count + 1` 的算法一致）；一个"组"项取它的数字串的前 min(D, 长度) 位作组号，其余的数字照字面接在后面；组号大于 `count`、或 `group(n)` 是 `None` 时得空串；第 0 组是整个匹配。<br>`${name}` 因此是字面的 `$` 接 `{name}`。<br>派生 `Clone, PartialEq, Eq`（`Config` 要 `Eq`）；`Debug` 写 `Template(***)` |
| P5 | `Pattern::new` | 先 `translate`，再 `fancy_regex::Regex::new`，缺省的回溯上限（1 000 000）不改。`Err` 是 `translate` 的消息或正则包的 `to_string()`。<br>它的三处用处的错误照旧经各自的诊断出去（`URL-REGEX` 的 `invalid(..)`、策略组的 `E_INVALID_POLICY_PARAM`）。<br>`rurge-rules` 的两处 `is_match(..).unwrap_or(false)` 不改：回溯超限按不匹配（设计第 5.5 节） |
| P6 | 两个节的读法（订正设计） | 一行的文字是 `Entry.raw`，切成 `split_words`。<br>**`[URL Rewrite]`**：两项或三项；第一项是式子（`Pattern::new(&words[0].text)`），第二项是替换（`Template::parse(&words[1].text)`），第三项是模式，ASCII 不分大小写：`header` / `302` / `307` / `reject`，省略是 `header`。`reject` 也照样解析替换（不读它）。<br>**`gateway`**（设计第 3 节的接口没有这个字段，第 6.1 节说"加载时就记下"）：替换的文字（`words[1].text`）里有 `{{{GATEWAY_ADDRESS}}}` 时为真，模式不论；引擎只在 `302` / `307` 时用它。<br>**`[Header Rewrite]`**：第一项 ASCII 不分大小写地是 `http-request` / `http-response` 时是方向、下一项是式子；否则方向是请求、第一项就是式子。之后依次是动作（不分大小写）、名字（RFC 9110 的 token：ASCII 的字母、数字与表后「原文」P6 一块的 15 个字，至少一个字；不是 token 的 `W0012`，`:` 开头的因此也不合法）。<br>• `header-del`：名字之后没有项；<br>• `header-add` / `header-replace`：值是 `raw[名字那一项的 end..]` 去掉首尾空白；名字之后恰好一项且它 `quoted` 时，值是那一项的 `text`；值是空的（名字之后什么也没有）时 `W0012`；值的每个字要么是制表符，要么不小于空格且不是 DEL（`'\x7f'`），否则 `W0012`（ASCII 以外的字都收：它们的 UTF-8 字节都不小于 0x80，`HeaderValue` 收）；<br>• `header-replace-regex`：名字之后恰好两项：式子（`Pattern::new`）与模板（`Template::parse`）。<br>名字照写的存（大小写不改）。名字 ASCII 不分大小写地是 `content-length` 或 `transfer-encoding` 时，行照样进结果，另给 `W0041`。<br>**`HeaderAction` 的 `Debug`**：`Add(X-A, ***)`、`Del(Cookie)`、`Replace(User-Agent, ***)`、`ReplaceRegex(Date, ***)`；`verb()` 是 `header-add` / `header-del` / `header-replace` / `header-replace-regex`。<br>`UrlRewrite` 与 `HeaderRewrite` 派生 `Debug`（`Pattern` 显示式子、`Template` 显示 `***`）。<br>`RewriteConfig::is_empty()`：两个 `Vec` 都空 |
| P7 | 诊断的措辞 | 消息与建议**照抄表后「原文」的 P7 一块**（`W0012` 的两句形状含竖线）；`{原因}` 是 `Pattern::new` 的 `Err`，`{名字}` 是写的名字。都带这一行的 `span`（`.at(entry.span.clone())`），`[Replica]` 带节头的 `span`。值不合法的那一句不带值。<br>**`W0041`**（`codes::W_REWRITE_FRAMING`，文档注释：`[Header Rewrite]` 写明 `Content-Length` 或 `Transfer-Encoding` 的行，行保留、改到它们的请求失败；`[Map Local]` 的 `header` 里的这两个名字不用（M3b）——两处的意思不同，所以每处自带建议，phase 4 M3 design 13）：级别警告，`with_hint(…)`；进 `PER_SITE`。<br>**`I0005`**（`codes::I_REPLICA_REMOVED`）：级别提示（`Diagnostic::info`）；进 `WITHOUT`。<br>`hint` 的文档注释：`W0041` 加进"每处自带"那一句（与 `E0014`、`W0033`、`W0034` 并列，写"each of its two places carries one"）；`I0005` 加进 `I0001` ～ `I0004` 那一条（"they inform"） |
| P8 | `[Replica]` | `text/mod.rs` 的 `KEY_VALUE_SECTIONS` 加 `"Replica"`：它不再是 `SectionKind::Unknown`，所以不报 `W0002`、不进 `unknown_sections`；`is_deferred` 不认它，所以不进 `W0016`。`SectionKind` 只被 `config.rs` 那一处读（核过），它的行照旧被节的解析器读进来、没有人用。<br>`config.rs` 在未知的节那一段之后：`profile.section("Replica")` 有值时给一条 `Diagnostic::info(codes::I_REPLICA_REMOVED, …).at(sec.span.clone())`——只看第一个，同名的第二个不另给（同 `W0002` 一个名字一条） |
| P9 | `Rewrite` 的结构与求值（订正设计） | **字段**：URL 的规则（`Vec<UrlRewrite>`）；头的规则（方向、`Pattern`、转好的动作：`HeaderName` 与 `HeaderValue`，`ReplaceRegex` 留 `Pattern` 与 `Template`，另存名字的原文与 `verb()` 供备注）；两份客户端配置 `config`、`h1`；`counters: Arc<RewriteCounters>`。<br>**`url()`**（设计第 6.2 节，M3-D9）：`current: Option<NewUrl>` 起初是 `None`。逐条：<br>• 要比的候选：`current` 有值时只有 `current.url()`，否则是给的 `candidates`；<br>• `header` 规则而 `current` 已有值：跳过，不求、不留备注（手册 "only the first one is applied"；这些不是"被跳过的改写"）；<br>• 依次 `is_match` 各候选，`Err` → `Skipped(回溯)`、这一条结束；都不配 → 下一条；<br>• 第一个配上的候选做全部替换（`captures_iter`），`Err` → `Skipped(回溯)`、下一条；<br>• `Header`：`NewUrl::parse(结果)`，`None` → `Skipped(不合法)`、下一条（它不算"第一条"）；`Some(n)` → `Done(header)`、`current = Some(n)`；<br>• `Found` / `Temporary`：`gateway` 为真时调闭包，`None` → `Skipped(没有网关)`、下一条；`Some(ip)` → 结果里每一处 `{{{GATEWAY_ADDRESS}}}` 换成 `ip`；`HeaderValue::from_bytes(结果)` 失败 → `Skipped(Location)`、下一条；成功 → `Done(302/307)`，交回 `Redirect`（`Found` 是 302、`Temporary` 是 307）；<br>• `Reject`：`Done(reject)`，交回 `Reject`。<br>走完交回 `Go(current)`。<br>**`headers()`**（设计第 7.2 ～ 7.4 节，M3-D12、D13、D30）：<br>• 先记下两样：`Content-Length` 与 `Transfer-Encoding` 的全部行（按名字、按次序），与（请求方向）`Host` 的全部行；<br>• 这个方向的每一条：URL 的式子 `is_match(url)`，`Err` → `Skipped(回溯)`、下一条，`false` → 下一条；作用：<br>　◦ `Add`：`append`；<br>　◦ `Del`：`remove`；<br>　◦ `Replace`：有这个名字时用 `entry` 的 `OccupiedEntry::insert` 换成一行（留在第一行的位置），没有时不动；<br>　◦ `ReplaceRegex`：`OccupiedEntry::iter_mut` 逐行，值是 UTF-8 时在上面做全部替换，`Err` → `Skipped(回溯)`、这一行不变，结果 `HeaderValue::from_bytes` 失败 → `Skipped(值)`、这一行不变；值不是 UTF-8 的行不动；<br>　这个名字作用前后的行（`get_all` 收成 `Vec`）不同时 `changed = true`，留 `Done(动作)`；<br>　之后比两个框架头的行与最初的：不同 → 留 `Failed(框架)`、`framing = true`、不再作用后面的规则，交回；<br>• 请求方向：`Host` 的行与最初的不同时：一行也不剩 → 放回最初的各行、留 `Skipped(Host 不能删)`、`host = Same`；否则取最后一行 `v`、只留它、`host = Changed(v)`。应答方向 `host` 总是 `Same`。<br>**`has_response_rules()`**：有方向是应答的头的规则。<br>**`origin_tls(name)`**：`OriginTls { config, h1, name, wait: REWRITE_HANDSHAKE }`（同 `rurge-engine` 的 `HANDSHAKE_TIMEOUT` 的 30 秒；`rurge-http` 不依赖 `rurge-engine`，所以另写一个常量，文档注释写它与那一个相同） |
| P10 | 新 URL 的合法性（订正设计） | `NewUrl::parse(text)`：<br>• `text.parse::<http::Uri>()` 成功、有 scheme 与 authority；<br>• scheme 不分大小写是 `http`、`https`、`ws`、`wss` 之一；<br>• 主机不空；有端口时 `port_u16()` 读得出且不是 0（读不出——例如大于 65535——就不合法）；没有端口时 `http` / `ws` 80、`https` / `wss` 443；<br>• IPv6 的主机去掉方括号再 `HostName::parse(..).canonical()`；<br>• **`https` / `wss` 的主机要能当 TLS 的名字**（`mitm::decrypt::server_name` 交回 `Ok`），否则不合法——设计第 6.3 节没有说这一条，不加就会在拨号之后才失败；<br>• 用户名口令与 `#` 之后的不要。<br>保存：scheme（小写）、`HostName`、端口、规整的 authority（`origin_authority(host, port, 缺省端口)`：缺省端口不写、IPv6 带方括号）、路径与查询串（空的写 `/`）、`ServerName`（只有 TLS 的）。<br>`url()` 是 `{scheme}://{authority}{路径与查询串}`；`note()` 是 `{scheme}://{authority}{路径}`（不带查询串）；`target()` 是 `Target::new(host, port)`；`tls()` 是 `https` / `wss`。<br>**`retarget_request`**（设计第 6.3 节，M3-D28）：URI 有 scheme（经监听器来的、讲 HTTP/2 的）→ 换成 `{http 或 https}://{authority}{路径与查询串}`（`ws` 写作 `http`、`wss` 写作 `https`）；否则（源站形式）→ 换成路径与查询串。`Host`：请求里有 `Host`、或 `h2_client` 为假时，`insert` 成新的 authority（只一行）；讲 HTTP/2 而没有 `Host` 时不加。<br>**`candidates`**：第一项是 `url`；`host` 有值、是 UTF-8、去掉空白之后不空时，第二项是 `{scheme}://{它}{path_and_query}`；第三项是 `{scheme}://{origin_authority(target, port, default_port(scheme))}{path_and_query}`；与前面的相同的不加 |
| P11 | 备注的措辞与计数 | 措辞定稿（`{式子}` 是规则的 `pattern.source` 照写，`{名字}` 是名字的原文，`{方向}` 是 `request` / `response`）：<br>• `Done`：`URL Rewrite: header by {式子} -> {NewUrl::note()}`、`URL Rewrite: 302 by {式子}`、`URL Rewrite: 307 by {式子}`、`URL Rewrite: reject by {式子}`、`Header Rewrite: {方向} {verb} {名字} by {式子}`；<br>• `Skipped`：`URL Rewrite skipped: {式子}: the rewritten URL is not a valid http, https, ws or wss URL`、`URL Rewrite skipped: {式子}: the rewritten Location cannot be a header value`、`URL Rewrite skipped: {式子}: there is no IPv4 gateway`、`URL Rewrite skipped: {式子}: the regular expression hit its backtracking limit`、`Header Rewrite skipped: {式子}: the regular expression hit its backtracking limit`（URL 的式子或 `header-replace-regex` 的式子，后者写 `header-replace-regex` 的那个式子）、`Header Rewrite skipped: {式子}: header-replace-regex made a value a header cannot carry`、`Header Rewrite skipped: Host cannot be removed`、`Header Rewrite skipped: the rewritten Host is not a valid HTTP/2 authority`；<br>• `Failed`：`Header Rewrite failed: {式子}: Content-Length or Transfer-Encoding changed`。<br>引擎把每一条 `add_note`；`Skipped` 每条 `counters.skipped()` 一次。<br>**计数**（设计第 12 节）：`url`——这个请求的 `url()` 交回 `Redirect`、`Reject` 或 `Go(Some(_))` 时一次；`header`——请求或应答一侧的 `changed` 为真，一个请求至多一次；`failed`——因为框架头失败的请求一次；`body`、`map_local` 在 M3a 不动 |
| P12 | `reject` 的答复（订正设计） | 同 `REJECT` 对引擎里一个请求的答复（设计第 6.5 节），只是错误页的说明是 `The request was rejected by a URL Rewrite rule.`（`answer` 里 REJECT 的那一句是 "…by the REJECT policy."，在这里不实）：`pages.reject` 为真 → `page(403, "Request rejected", 那一句)`、`set_status(403)`、`finish(Rejected(RejectKind::Reject))`；为假 → `finish(Rejected(RejectKind::Reject))`、`Err(Close)`（HTTP/2 上就是重置这个流） |
| P13 | `302` / `307` 的应答 | `redirect(status, location)`：`Response::builder().status(status).header(LOCATION, location).header(CONTENT_LENGTH, "0").body(responses::empty())`。引擎：`set_status(302 或 307)`、`finish(SessionOutcome::Completed)`、交回它；连接照常保持。hyper 自己加的头（例如 `date`）不归 rurge 管，用例不断言它们不在 |
| P14 | 每个请求的对上游 TLS 与 `OriginKey` | `OriginTls` 加 `#[derive(Clone)]`（字段都是 `Arc`、`ServerName<'static>` 与 `Duration`）。<br>`OriginKey::of(None) = Plain`，`of(Some(t)) = Tls(t.name.clone())`：键只放名字，不放配置——同一个客户端连接上 TLS 的连接都是用同一份配置拨的（解密的连接用它握手时那一代的，别的连接用这一代 `Rewrite` 的，而 `K` 里有代次；M3-D31、设计第 8.3 节）。<br>池、共享的 HTTP/2 连接、`Outing.key`、`Resend.key` 的类型都换成 `(U::Key, OriginKey)`；`borrow_h2`、`share`、`take`、`put` 的写法不变（`Pool` 本来就只要 `K: PartialEq`）。<br>`connect` 用参数 `origin`：`None` → 明文、只讲 HTTP/1.1（同今天）；`Some(tls)` → 同今天的那一支。<br>任务 4 里 `handle` 的 `origin` 总是 `self.shared.conn.origin.clone()`，所以行为不变 |
| P15 | 引擎里的位置（任务 5） | 在 `handle` 里，依次：<br>① `Routed::Go` 解出 `route`、`key`、`pages`、`rewrite`（`route`、`key` 改成可变的）；`prepared.unknown` 的检查照旧在前；<br>② `let sniffed = self.shared.conn.entry == Entry::Sniffed;` 挪到这里；嗅探到的连接上的 `CONNECT` 与绝对形式的 HTTP/1.x 请求（`!self.shared.conn.h2 && req.uri().scheme().is_some()`）把 `rewrite` 置为 `None`（M3-D29）；<br>③ **请求一侧的 Header Rewrite**（`rewrite` 有值时）：URL 是 `handle.session().url`；`headers(Request, …)`；备注入记录；`framing` → `counters.failed()`，`self.failed(&guard, pages, "Header Rewrite changed Content-Length or Transfer-Encoding", "A Header Rewrite rule changed Content-Length or Transfer-Encoding, so the request was not sent.")`，不拨号；<br>④ **URL Rewrite**：`candidates(scheme, url, req.headers().get(HOST), path_and_query(req.uri()), &handle.session().dst_host, handle.session().dst_port)`（`scheme` 是已有的那个变量）；`url(…, &\|\| self.shared.upstreams.gateway(), …)`；`Redirect` / `Reject` 照 P13 / P12 答复；`Go(Some(new))` → `retarget_request(&mut req, &new, self.shared.conn.h2)`，`(route, key) = upstreams.retarget(route, key, new.target(), entry == Listener && !new.tls())`，`origin` 照下一条，`retargeted = true`，"当前的 URL"换成 `new.url()`；<br>⑤ 按请求的 `origin`：没有换目标 → `conn.origin.clone()`；换到 `https` / `wss` → 连接是解密的时用 `OriginTls { name: new 的 ServerName, ..conn.origin.clone() }`，否则 `rewrite.origin_tls(new 的 ServerName)`；换到 `http` / `ws` → `None`；<br>⑥ 之后照旧：`wants_upgrade`（在重写之后读）、`tunnel`、`needs_h1`、借或拨（键 `(key, OriginKey::of(origin.as_ref()))`）、`Form::of`；<br>⑦ `headers::strip` 的 `Keep.proxy_authorization` 是 `sniffed && !retargeted`；<br>⑧ `fit` 之后：`!retargeted`、`forward` 是 `None`、`host` 是 `Changed(v)` 时 `write_back_host(&mut req, &v, sender.version())`，交回假 → 留 `Skipped(HTTP/2 authority)`；`retry` 里的 `fit` 之后同样写回（`Resend` 多一个 `host: Option<&HeaderValue>`），那一条备注一个请求至多一次；<br>⑨ 应答：101 且是升级的那一支，`rewrite` 有值时先 `headers::strip(resp.headers_mut(), Keep { upgrade: true, proxy_authorization: false })`（`strip` 做两遍与一遍一样，`promote` 不改），再 `headers(Response, 当前的 URL, …)`；普通的那一支在 `headers::strip(resp.headers_mut(), Keep::NONE)` 之后做；`framing` → 丢掉 `resp`（连接不再借出：HTTP/1.1 的随 `sender` 丢掉而关，HTTP/2 的那个流随正文丢掉而重置）、`counters.failed()`、`self.failed(…)`（同 ③ 的两句）；<br>⑩ 普通的那一支把 `handle.set_status(resp.status())` 挪到应答一侧的重写之后（失败时记录的状态码是客户端拿到的 502，或关闭时没有）。<br>`url` 与 `header` 的计数照 P11 |
| P16 | `retarget` / `gateway` 与 `route` 的 `rewrite` | `ConnUpstreams::retarget(way, key, target, forwardable)`：`way.target = target.clone(); way.may_forward = forwardable; key.target = target; key.may_forward = forwardable;`，交回两者；`attempts`、`book`、代次不动，不再求规则（M3-D15）；新的主机不问 fake 地址池、不按 `use-local-host-item-for-proxy` 钉住（设计第 8.2 节，登记）。<br>`gateway()`：`self.engine.network().primary` 的 `routers` 里第一个 IPv4（M3-D10）。<br>任务 5 的 `route` 给 `rewrite: None`；任务 6 改成：`self.engine.rewrite_on()` 且 `first.terminal != TerminalKind::Reject` 时 `rt.rewrite.clone()`，否则 `None`（M3-D27） |
| P17 | 开关、`Runtime.rewrite` 与 `build_rewrite` | `Engine.rewrite_on: AtomicBool`（`new` 里 `true`，与 `mitm_on` 并排）；`rewrite_on()`；`set_rewrite(on)` 照 `set_mitm` 写 `state.features.rewrite = Some(on)`；`restore_switches` 加 `features.rewrite` 一支。<br>`EngineShared.rewrite: Arc<RewriteCounters>`，`EngineShared::new` 里新建（重载不清零）。<br>`runtime.rs` 在 `let mitm = build_mitm(..)` 之后 `let rewrite = build_rewrite(&config, &opts.shared).await;`：<br>• `config.rewrite.is_empty()` → `None`，不读根；<br>• 根照 `build_mitm`：`shared.roots` 有值时用它，否则 `spawn_blocking(union_root_store)`，失败 → DEBUG 一行 `the roots of the origin side cannot be read; rewrites are off for this generation`、`None`；<br>• `Rewrite::build(&config.rewrite, &config.mitm, roots, shared.rewrite.clone())`：`Ok(r)` → `r.map(Arc::new)`；`Err(e)` → WARN `rewrites are off: cannot set up TLS: {e}`、`None` |
| P18 | 计数与 API 的形状 | `HttpCounts.rewrite: rurge_http::RewriteCounts`，`http_counts()` 取 `self.shared.rewrite.snapshot()`。<br>`rurge_http.rs`：`HttpJson` 在 `mitm` 之后加 `pub rewrite: RewriteJson`；`RewriteJson { url, header, body, #[serde(rename = "mapLocal")] map_local, skipped, failed }`，照 `MitmJson` 逐字段抄。模块的文档注释加一句"M3a adds `rewrite`"。<br>`features.rs`：`get_feature` 的 `"rewrite" => json!({ "enabled": app.engine.rewrite_on() })`；`set_feature` 的 `"rewrite"` 一支照 `mitm`。`FEATURES` 不变 |
| P19 | 端到端的做法（订正设计） | **规则照手册原文逐字写**；要连上游的请求把**测试源站的端口写在请求里**（例如 `http://www.google.cn:{源站端口}/`），式子 `^http://www\.google\.cn` 照样命中、替换之后端口留着。设计第 16 节第 3 条写的是"在替换里加上测试源站的端口"，那样改了手册的规则，本计划改成改请求。<br>**主机名**：示例里的每个名字——包括换目标之后的新主机——各一行 `[Host]` 钉到 `127.0.0.1`；规则用 `DOMAIN` 选 `DIRECT` 或守卫。共用的夹具在规则之后自带 `FINAL,DIRECT`，`dns-server` 是回环上的 `MockDns`（`crates/rurge-engine/tests/common/mod.rs` 的 `Profile::text` 与 `harness_at`）：没有钉住的名字也解析不到回环之外，这是第二道防线；用例不另写 `FINAL`（写了就有两条）。换了目标的请求不重选策略、新主机的拨号照旧经引擎的解析，所以新主机那一行 `[Host]` 不能少。<br>**守卫**：不该连上游的请求（`302`、`307`、`reject`、按 `Host` 命中的）照手册的 URL（缺省端口）发，规则选的策略是 `guard = http, 127.0.0.1, {守卫端口}`；守卫是测试自己持有的监听器，用例结束前断言它一次也没有被连过。万一拨了号，连接只会到守卫。<br>**根**：建引擎一律经 `harness_with`（或 `tests/mitm.rs` 那样的夹具），`EngineShared.roots` 是 `TlsFixture` 的根，用不到 TLS 时是 `Arc::new(RootCertStore::empty())`。<br>**网关**：`EngineShared.network` 是 `Arc::new(NetworkCell::new(NetworkFacts { primary: Some(Primary { interface: "test0".into(), kind: NetKind::Wired, routers: vec![…] }), wifi: None }))`。<br>**夹具**：`tests/mitm.rs` 里要用的（`mitm_section`、`mitm_ca`、`harness_mitm` 与讲 HTTP/2 的客户端）照原样抄进 `tests/rewrite.rs`，注释写出处；不挪进 `common`。源站、数连接、记请求头的夹具照 `tests/http.rs` 的写法，在 `tests/rewrite.rs` 里写自己用的 |
| P20 | 现有的用例 | 见下面的表。表里"预计照过"的若红，停下报告，不改期望 |
| P21 | 收尾文档的清单 | 见任务 8。兼容性清单登记设计第 15 节第 1 ～ 9 条与第 13 条，另登记本计划的三处：`header-replace` 换成一行（P9）；`https` / `wss` 的主机要能当 TLS 的名字（P10）；`reject` 的错误页说明（P12）。两份 README 与需求文档在 M3b 收尾时改 |

### 表后的原文（照抄）

P3、P6、P7 要照抄的文字含竖线，放在这里。每个代码块里的文字就是要的那一串，没有为 Markdown 加的转义。

**P3：类外的九种写法**（左栏是写的，右栏是改成的）

```text
\h      [\t\p{Zs}]
\H      [^\t\p{Zs}]
\v      [\n\x0B\f\r\x{85}\x{2028}\x{2029}]
\V      [^\n\x0B\f\r\x{85}\x{2028}\x{2029}]
\R      (?:\r\n|[\n\x0B\f\r\x{85}\x{2028}\x{2029}])
\<      <
\>      >
\Z      (?=(?:\r\n|[\n\x0B\f\r\x{85}\x{2028}\x{2029}])?\z)
\Q…\E   段里每个字照 P3 写成字面（没有 \E 时到末尾）
```

**P3：字符类里**

```text
\h      \t\p{Zs}
\v      \n\x0B\f\r\x{85}\x{2028}\x{2029}
\<      <
\>      >
\H \V \R    Err；消息恰好是下面这一句，以 \H 为例，另两个只换那个字母：
`\H` cannot be used inside a character class
```

**P3：`\Q…\E` 里前加反斜杠的 18 个字**（regex-syntax 0.8.11 的 `is_meta_character`）

```text
\ . + * ? ( ) | [ ] { } ^ $ # & - ~
```

**P6：字段名里字母、数字之外的 15 个字**（RFC 9110 的 tchar）

```text
! # $ % & ' * + - . ^ _ ` | ~
```

**P7：诊断的消息与建议**

```text
W0012
[URL Rewrite]: a line that is not `<regex> <replacement> [header | 302 | 307 | reject]` is ignored
[URL Rewrite]: the regular expression cannot be used ({原因}); the line is ignored
[Header Rewrite]: a line that is not `[http-request | http-response] <regex> <action> <name> …` with an action of header-add, header-del, header-replace or header-replace-regex is ignored
[Header Rewrite]: `{名字}` is not a header field name; the line is ignored
[Header Rewrite]: header-add and header-replace take a value after the field name (write "" for an empty one); the line is ignored
[Header Rewrite]: the value has a control character a header cannot carry; the line is ignored
[Header Rewrite]: header-del takes a field name and nothing after it; the line is ignored
[Header Rewrite]: header-replace-regex takes a field name, a regular expression and a replacement; the line is ignored
[Header Rewrite]: the regular expression cannot be used ({原因}); the line is ignored

W0041 的消息
[Header Rewrite]: a rule on Content-Length or Transfer-Encoding fails every request whose framing it changes; the line is kept
W0041 的建议（with_hint）
rurge lets no rewrite change how a message is framed: remove the line unless failing those requests is what it is for

I0005 的消息
[Replica] was removed in Surge Mac 5.4.0; the section is ignored
```

### P20 的表：要改的、要留意的现有用例

| 用例 | 为什么 | 任务 | 预计 |
| ---- | ------ | ---- | ---- |
| `crates/rurge-config/src/rule.rs` 与 `spec/group.rs` 里 `URL-REGEX`、`policy-regex-filter`、`policy-priority` 的用例 | `Pattern::new` 先改写 | 1 | 照过：任务 0 核过它们的式子里没有那九种写法（有的话照实记、停下报告） |
| `crates/rurge-rules` 里 `URL-REGEX` 的匹配用例、`rurge rule match --url` 的命令行用例 | 同上 | 1 | 照过 |
| `crates/rurge-config/src/redact.rs` 与 `tests/secrets.rs` 的全部 | 不改脱敏 | 1 | 照过 |
| `crates/rurge-config/src/value.rs` 的现有用例（含比较读法的那几条） | 只加 `split_words` | 1 | 照过 |
| `crates/rurge-config/src/diagnostic.rs` 的 `every_code_is_decided_once` 与查缺省建议的那两条 | 多两个码 | 2 | 改：`PER_SITE` 加 `W0041`、`WITHOUT` 加 `I0005`（先红） |
| `crates/rurge-config/src/deferred.rs` 的 `prefix_and_membership_matching` | 两个节不再延后 | 2 | 改：加四句断言 |
| `crates/rurge-config/src/text/mod.rs` 的 `section_kind` 用例 | `Replica` 是 `KeyValue` | 2 | 照过 |
| `crates/rurge-config/src/config.rs` 里断言 `W0016` 名单或 `deferred` 内容的用例（任务 0 列全） | 名单少两个 | 2 | 列出的逐条看：断言的是这两节的改；不涉及的照过 |
| 语料库快照 `corpus__corpus__kitchen-sink.snap` | `deferred` 少两项、`W0016` 的消息少两节 | 2 | 改：手改第 98、99、112 行；`kitchen-sink.conf` 的两节五行都读得了、没有新诊断 |
| `crates/rurge-http/src/engine.rs` 测试模块里 `Upstreams` 的那一份实现与 `Routed::Go` 的构造处 | 两个新方法、一个新字段 | 5 | 改：只加 `retarget`、`gateway` 与 `rewrite: None`，断言不改 |
| `crates/rurge-http/src/engine.rs` 测试模块的其余全部 | 键与按请求的 TLS | 4、5 | 照过 |
| `crates/rurge-http/src/mitm/decrypt.rs` 与 `pool.rs` 的测试模块 | `OriginTls` 可克隆、`server_name` 可见 | 3、4 | 照过 |
| `crates/rurge-engine/tests/http.rs`、`mitm.rs`、`pipeline.rs` 等的全部 | `rewrite` 恒为 `None`（任务 5）、没有规则时为 `None`（任务 6） | 5、6 | 照过 |
| `crates/rurge-api/tests/api.rs` 的 `rurge_http_counts_start_at_zero` | 多 `rewrite` | 6 | 改：解析的正文与字节都加 `"rewrite":{"url":0,"header":0,"body":0,"mapLocal":0,"skipped":0,"failed":0}`（先红） |
| `crates/rurge-api/tests/api.rs` 的 `features_collections_stop_and_unknown_paths` | `rewrite` 生效了 | 6 | 照过；只改那一句断言的说明文字 `only system_proxy, enhanced_mode and mitm are live`，加上 `rewrite` |
| `crates/rurge-api/tests/api.rs` 的 `the_mitm_switch_is_read_switched_and_kept` | 并排的新开关 | 6 | 照过 |
| `crates/rurge-engine/src/state.rs` 的用例 | `features.rewrite` 已有 | 6 | 照过 |
| `crates/rurge/tests/cli.rs` 的 `a_daemon_restores_the_mitm_switch_from_state_json` | 同上 | 6 | 照过 |
| `crates/rurge/tests/sockets.rs` 的扫描用例 | 不建套接字 | 3 ～ 7 | 照过 |

---

## 执行期修正记录

下面写"执行者的裁定"的，是 Pi 在设计与计划的原则下定的；写"协调者的裁定"的，是协调的会话代项目所有者定的。项目所有者都可以改。计划的正文不改：正文里说得不对的地方，以这张表的行为准。

| # | 任务 | 内容 | 提交 |
| - | ---- | ---- | ---- |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
