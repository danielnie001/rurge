# 阶段 4 / M4a「抓包」实施计划

> **执行方式**：交给 Pi，由协调的会话逐个任务派工、逐个任务评审（设计第 2 节、M4-D1；做法同 M1a ～ M3b）。
>
> - 协调的会话每个任务开一个 Pi 会话，交给它这个任务的全文与要用的接口（派工文件）。Pi 做完把报告写进派工文件指定的路径，在本地提交。
> - 协调者读改动、重跑门禁、评审。有问题回到同一个 Pi 会话修正，再评审；过了才派下一个任务。
> - 每个任务：先写失败的用例、看到红；再实现到绿；跑门禁；对照本任务的「判据」与「全局约束」逐条自查；最后提交（**只在本地，不推送**）。钉住现有行为的用例（任务里写明"不要求先红"的）可以一写就绿。
> - 与计划的出入记进末尾的「执行期修正记录」，**不改计划的正文**。
>
> **停点**：没有。M4a 没有平台代码、没有只在 CI 上编译的代码（设计第 2 节）。M4a 之后不单独停下来等推送；M4b 的计划在 M4a 合并之后对着那时的代码写。

**目标**：让抓包生效（FR-HTTP-11、FR-RULE-14、FR-HTTP-13 的 `capture`）。

- `rurge-inbound`：`SessionHandle::when_finished`（已经结束就当场调）与抓包会话的槽；`rurge-config`：空的 `always-capture=` 给 `W0012`。
- `rurge-http` 新 `capture` 模块：只在内存里的存储、总量与单条正文两个上限、按块记账、挤掉最早的、两种截断与丢掉、命名会话、读出时解开 `gzip` / `x-gzip` / `deflate`。
- HTTP 引擎里每个请求一个 `CaptureSlot`：请求的两个取样点、在 `Idle` 里抄请求正文、包住交给客户端的应答正文、在 HTTP 部分结束的那一刻存进去。
- `rurge-engine`：一个引擎一份存储、`capture` 开关与 `state.json`、`choose_policy_with` 填抓包会话的槽、`ConnUpstreams::route` 给 `capture`。
- `rurge-api`：`/v1/features/capture` 生效，`/v1/rurge/capture/…` 六个端点（含 HAR 1.2）。
- 可执行文件：`--capture-size` / `RURGE_CAPTURE_SIZE` 与 `--capture-body-size` / `RURGE_CAPTURE_BODY_SIZE`。
- 量一次内存；手工清单加 M4a 的条目；M4a 的收尾文档。

**架构**：

- 不加工作区边、不加外部包（M4-D2，P2 订正）：`rurge-api` 新增正式依赖 `base64` 与 `futures-util`（都已在工作区的依赖表与 `Cargo.lock` 里）；`rurge-http` 加一个 `testing` 特性（只给别的 crate 的用例存进自己造的条目）。锁文件的包数不变（520）。
- `rurge-http::capture` 是不碰网络的函数与类型（任务 2）；`capture/slot.rs` 的 `CaptureSlot` 是引擎与存储之间的那一层（任务 3）；引擎里只多几个调用点，`answer`、`failed`、`page`、`map_local`、`Tail`、`LocalFile`、`Guard` 一行不改（P9）。
- `rurge-engine` 经 `pub use rurge_http::capture` 把类型交给 `rurge-api`（同 `ApiTls`）：`rurge-api` 的正式依赖仍只有 `rurge-engine` 与 `rurge-config` 两条工作区边。

**技术栈**：都已在 `Cargo.lock` 里，源码在本机 cargo 注册表里。**不下载、不加新包**（M4-D2）。

- http 1.5.0：`HeaderMap`、`Method`、`StatusCode::canonical_reason`、`Version`（`Debug` 写 `HTTP/1.1`、`HTTP/2.0`）。
- http-body 1 与 http-body-util 0.1：`Body::poll_frame`、`is_end_stream`、`size_hint`、`Frame::data_ref`、`BodyExt::boxed`。
- bytes 1：`Bytes::copy_from_slice`。
- flate2 1.1.10：经 M3b 的 `rewrite::body` 用（P7），不另写解码。
- time 0.3.45：`OffsetDateTime::from(SystemTime)`（`src/interop/offsetdatetime_systemtime.rs`）与它的 `year`、`month`、`day`、`hour`、`minute`、`second`、`millisecond`。不开 `formatting` 特性。
- axum 0.8.9 / axum-core 0.5.6：`Path<String>`、`Query`、`Bytes` 的提取与 `BytesRejection`、`Body::from_stream`（要 `TryStream + Send + 'static`，`Ok: Into<Bytes>`，`Error: Into<BoxError>`）。
- futures-util 0.3.34：`stream::iter`（`src/stream/mod.rs` 里不在任何特性之后；工作区给它关了缺省特性）。
- base64 0.22：`engine::general_purpose::STANDARD`。
- url 2：`Url::parse`、`query_pairs`（`rurge-api` 已经依赖它）。

**依据**：

- M4 细化设计（下面写"设计"指它）：`docs/superpowers/specs/2026-11-01-phase4-m4-capture-records-design.md`。
  - M4a 用到第 1 ～ 9、11 ～ 14、16、17 节，与已决事项 M4-D1 ～ D20、D26 ～ D29。
  - **先读它第 18 节**：本计划改了设计正文的地方随本计划写在那里（"写 M4a 计划时"之下）。
  - 设计与本计划的「计划期决定」不一致的，以计划为准。
- M3b 计划（`docs/superpowers/plans/2026-10-31-phase4-m3b-body-rewrite-map-local-plan.md`）的 P9 ～ P16 与执行期修正记录：`ReadAhead`、`Tail`、`LocalFile`、`rewrite::body::decode` 的来历；`docs/status.md` 阶段 4 / M3b 一节「以后的会话必须知道的事」整段适用，M3a、M2b 两节的同名段落也适用。
- M2b 计划的 P11（`Meter` 与一个请求自己的上下行字节）、P6（HTTP/2 上的 `CONNECT` 答 501）。
- HAR 1.2 的字段与 `-1` 的约定：`docs/reports/2026-10-29-phase4-http-mitm-survey.md` 第 5 节。
- 写计划时对着 `a6e78ad`（分支 `phase4-m4`）核对过的事实写在「计划期决定」里。外部库的事实对着注册表里锁定版本的源码。

## 全局约束

**依赖**

- 只改两处 `Cargo.toml`：`crates/rurge-http/Cargo.toml` 加 `[features]` 一节 `testing = []`（任务 2）；`crates/rurge-api/Cargo.toml` 的正式依赖加 `base64.workspace = true`（任务 5）与 `futures-util.workspace = true`（任务 6），开发依赖的 `rurge-http.workspace = true` 改成 `rurge-http = { workspace = true, features = ["testing"] }`（任务 5）。
- `Cargo.lock` 的 `[[package]]` 今天 520 个，每个任务之后照旧 520 个；锁文件里只有 `rurge-api` 一个包的 `dependencies` 列表变（多 `base64` 与 `futures-util`），别的不变。`cargo` 要下载任何东西就停下报告。
- 依赖方向不变：`rurge-api` 的正式依赖里工作区的 crate 仍只有 `rurge-engine` 与 `rurge-config`，抓包的类型经 `rurge_engine::capture` 用；`rurge-http` 不依赖 `rurge-engine`；`rurge-inbound` 不依赖 `rurge-http`。
- `unsafe_code`：不加放开的函数，`crates/rurge-platform/tests/unsafe_functions.rs` 的 `LISTED` 仍是 23 项。
- `crates/rurge/tests/sockets.rs` 的 `LISTED` 仍是 26 行：抓包不建套接字、不问解析库。名单之外的原因让那条扫描用例变红，就停下报告。

**缺省的 `cargo test` 不碰系统**

- 同 M3b：不建网卡，不改系统的任何设置；不执行 `netsh`、`route`、`ipconfig`、`ip`、`resolvectl`、`networksetup`。
- **测试从不把任何 CA 装进信任库，也不读系统的信任库**：建引擎的用例一律给 `EngineShared.roots`（M3a 记录第 20、32 行）；解密的用例用 `TlsFixture` 的根。
- 量内存的两条用例带 `#[ignore]`，缺省的 `cargo test` 不跑；它们读峰值工作集时起的 PowerShell 是只读的系统工具（同 `crates/rurge-tun/src/tests/bench/peak.rs`）。

**测试的约定**

`CLAUDE.md`「测试约定」整节适用。写用例之前读 `docs/testing.md`「测试约定（全文）」的阶段 4 各节。与本计划直接相关的：

- 只用回环与端口 0。端到端的示例主机名经一行 `[Host]` 钉到 `127.0.0.1`，第二道防线是共用夹具的 `dns-server`（回环上的 `MockDns`，M3a P19）；源站是回环上的 `TestServer`，目标端口写在请求里。
- **大正文**在用例里生成字节（重复的 ASCII），不读写磁盘上的大文件。
- 等待都有时限。"条目存进去了"用有时限的轮询等（`wait_until`），不用固定的等待；固定等待只许"断言什么都没发生"，注释写它等的是什么、余量多大。新写的、带等待或并发的用例提交前单独循环跑二十次以上（任务 1、3、4、7、8）。
- 抓日志的用例用 `LogCapture`，跑在当前线程运行时。测试里手写的 HTTP/1.1 客户端把每个请求——请求头与正文——在一次写里发出。
- **内联的测试模块放在文件的末尾**：`crates/rurge/tests/sockets.rs` 读生产代码，读到第一个 `#[cfg(test)] mod … {` 为止。

**门禁与这台开发机**（每个任务末尾一次；说"通过"之前必跑）

- 门禁：`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test -j 8 --workspace --no-fail-fast`。
- 本机 msvc 工具链没有 rustfmt。跑 `cargo fmt` 之前让环境变量 `RUSTFMT` 指向 gnu 工具链里同版本的 rustfmt：用户目录下的 `.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe`。不要为此安装或升级工具链。
- **门禁的日志用重定向写、再追加退出码，不用 `Tee-Object`**（M3a 记录第 54 行）；单独重跑的结果也写进日志再引，引之前先看它在、看它写的是什么。
- 聚焦地跑用例也经整个工作区：
  - 单元测试：`cargo test -j 8 --workspace --lib -- <过滤词>`；
  - 集成测试：`cargo test -j 8 --workspace --test <名字> -- <过滤词>`。
- **不用 `-p`**：它另编一份依赖，`target/` 涨得很快，写满过 D: 盘。**也不用 `--release`**：量内存的用例在调试构建里跑（P15）。
- 跑全量之前 `df -h /d`：剩余不足约 15 GiB 就停下报告。
- 测试二进制以 `STATUS_HEAP_CORRUPTION`（本机管控软件注入的钩子 DLL）或 `STATUS_ACCESS_VIOLATION` 退出：单独重跑那一个二进制，报告里写明。
- `cli` 的用例偶发 `give_up` 20 秒超时：单独重跑 `cli`，把标准错误原样抄进报告。
- rustc 在增量编译里报内部编译器错误：删掉出错的那个目标在 `target/debug/incremental/` 下的目录再编。
- 回环的 TCP / UDP 7、9、13、17、19 号端口在这台机器上开着，CI 上关着：用例不假定任何固定端口的状态。
- 结果照实记：本机的全量运行是"一台机器上的一次运行，不是 CI"；需要参照服务端的互操作用例在本机立刻返回、算在通过的数里，要说出来。

**仓库的规矩**（`CLAUDE.md`「必须遵守的规矩」整节适用；与本计划直接相关的）

- **凭据与用户流量**（设计第 6.6 节，M4-D15）：抓到的头、正文、URL 从不进日志、错误文本与 `Debug`；`Entry`、`Head`、`ResponseHead`、`Body`、`Capture`、`CaptureStore` 与 crate 里的 `Pending`、`CaptureSlot` 不实现 `Debug`（P5）。`authorization`、`proxy-authorization`、`cookie`、`set-cookie` 四个头的值在 API 与 HAR 里写 `***`，存储里原样。rurge 自己的凭据（监听器的口令、对上游 HTTP 代理的 `Proxy-Authorization`）从不进抓包（任务 3、8 各有用例）。抓包的值是不是"API 输出"的凭据，是所有者的 Q1（设计第 17 节），不要自己改这个做法。
- **诊断**：不设新码；`W0012` 照 `codes::hint` 里已有的决定（任务 0 核对）。
- **日志的分级**：M4a 只加两行：开关切换的 INFO（`capture switched via http-api`）与单条上限被降到总量的启动 WARN（P13）。条目的任何内容不进任何日志，任何一级都不（任务 8 有用例）。
- 文档与报告用写文件 / 改文件的工具改。**永远不要用把一个文件的内容经管道写回同一个文件的命令**（2026-10-27 这样写满过 D: 盘）。
- 计划末尾两张表里已写下的行不改写、不删除。追加新行时**不要把已有的行当成锚点替换掉**（M2a 任务 9 这样删掉过一行）。

**写法**

- 注释、日志、错误文本与诊断用英文，照所在文件的写法：文档注释写"为什么"与前提，句子完整。注释里提到设计或计划，写 `phase 4 M4 design 6.4`、`the phase 4 / M4a plan's P8`。
- 提交说明照 `git log` 的样子写，标题用中文：`feat(inbound): …`、`feat(config): …`、`feat(http): …`、`feat(engine): …`、`feat(api): …`、`feat(cli): …`、`test(engine): …`、`refactor(http): …`、`docs: …`。正文说做了什么、改了哪些现有用例的期望、为什么。Pi 的提交不带署名行。
- 只在本地提交：不 `push`、不 `pull`、不 `fetch` 远端、不 `amend`、不改写历史。

**不做的事**

- 只运行本仓库构建出来的二进制与 Pi 自己。不下载、不安装任何东西。不手动跑 `rurge run`、`rurge dns lookup`；`rurge service` 只用 `--dry-run`。
- 参考实现：GPL 的（sing-box、mihomo、Xray-core）只取事实，代码不抄。
- 不做设计第 1 节「不做」一栏的事，也不做这些：
  - M4b 的事：记录的 `receiveMs`、`requestBodyBytes`、`responseBodyBytes`，直通的原因，`remoteAddr` 的新口径，`dnsMs` 在途（设计第 10 节）；
  - `br` 与 `zstd` 的解码（总设计 Q4）；落盘；导入 HAR；拆 `cookies`；
  - 让 `always-capture` 把连接留在 HTTP 引擎里或让 TLS 被解密（M4-D8，归所有者）；
  - 改 `redact.rs`、`crates/rurge-config/tests/secrets.rs`、`RuleParams`、`--request-log-size` 的缺省。

**计划没有写到、要做决定的地方**

- 能按设计与总设计的原则定的：自己定，记进修正记录，写"执行者的裁定"。
- 要停下来列出、交给协调者的：去向写着"项目所有者"的（设计第 17 节）；会改变 M4-D1 ～ D31 的。
- 现有用例变红：P16 的表点了名的，照计划改，在提交说明里说；没点名的，停下来，记进修正记录，不顺手改期望值。

## 评审要点

设计没写透、而最可能咬到使用者的几种输入，与负责它们的任务和用例：

1. **巨大的与没有尽头的正文**：几百 MB 的下载、一直不结束的流式应答、开着抓包上传大文件。
   - 期望：每个正文至多存单条上限、之后的字节只计数；存储记着的字节在任何时刻不超过总量；挤不出地方时从那一块起不再存（`budget`）；客户端照样收到全部字节；内存不随正文无限地涨。
   - 用例（任务 2、3、8）：`a_body_over_its_limit_keeps_its_beginning`、`the_store_never_holds_more_than_its_total`、`a_chunk_that_cannot_fit_even_in_an_empty_store_is_truncated_for_the_budget`、`a_body_over_the_limit_is_captured_truncated`、量内存的 `a_full_store_at_the_default_limits`。
2. **读出与导出 HAR 的途中条目被挤掉或清除**：一个会话导出到一半，新的请求把它的条目挤掉了；有人在导出时清除。
   - 期望：读到的、正在导出的条目完整，内存到读完、写完才放掉；导出的 HAR 是取快照那一刻的全部条目。
   - 用例（任务 2、6）：`an_evicted_entry_lives_on_while_it_is_held`、`an_export_holds_its_entries_while_they_are_cleared`。
3. **断开、重置与终止**：客户端在正文中途走掉、HTTP/2 上在应答之前重置流、`kill`、`REJECT-DROP` 的停留。
   - 期望：每一种都存成一条（结局失败或没有应答），预留在存进去或丢下时还回去，`used` 回到存进去的条目之和；`REJECT-DROP` 的条目不等停留结束。
   - 用例（任务 2、3）：`a_pending_entry_dropped_gives_its_reservation_back`、`a_client_that_leaves_mid_body_is_captured_as_failed`、`a_killed_request_leaves_no_reservation_behind`、`an_http2_stream_reset_before_its_answer_is_captured_with_no_response`、`a_dropped_request_is_captured_before_its_hold_ends`。
4. **不是 UTF-8 的、二进制的、压缩的正文与头，路径里奇怪的会话名**：图片、`br` 压缩的应答、头的值里有 0x80 ～ 0xFF 的字节、会话名带空格、斜杠或中文。
   - 期望：API 的正文照原样给 Base64，头的值按 U+FFFD 替换（存储里照原样）；`decode=true` 解得开的解开、解不开的说原因；HAR 里不是 UTF-8 的写 Base64 并标 `encoding`；会话名经百分号编码到达那个会话。
   - 用例（任务 2、5、6）：`what_cannot_be_decoded_says_why`、`an_entry_is_shown_whole_with_four_headers_masked`、`decode_undoes_gzip_and_says_why_it_cannot`、`odd_session_names_are_reached_through_the_path`、`har_bodies_are_text_or_base64_and_say_what_was_left_out`。
5. **凭据与日志**：带口令的监听器、带凭据的上游 HTTP 代理、请求里的 `Cookie` 与 `Authorization`、DEBUG 级的日志。
   - 期望：rurge 自己的两种凭据从不进抓包；四个头在 API 与 HAR 里写 `***`；任何一级的日志里没有抓到的内容。
   - 用例（任务 3、5、6、8）：`the_captured_request_is_the_one_that_leaves_without_rurges_forward_headers`、`an_entry_is_shown_whole_with_four_headers_masked`、`har_masks_the_four_headers_and_leaves_cookies_empty`、`a_listener_password_is_not_captured`、`an_upstream_proxy_credential_is_not_captured`、`captured_content_never_reaches_the_log`。

---

## 文件结构

| 文件 | 任务 | 职责 |
| --- | --- | --- |
| `crates/rurge-inbound/src/session.rs` | 1 | `when_finished`；抓包会话的槽 |
| `crates/rurge-config/src/config.rs` | 1 | 空的 `always-capture=` 给 `W0012` |
| `crates/rurge-http/Cargo.toml`、`crates/rurge-http/src/lib.rs`、`crates/rurge-http/src/capture/{mod.rs,store.rs}`（新）、`crates/rurge-http/src/rewrite/body.rs` | 2 | 类型、存储、记账、挤掉与截断、读出时解开；`decode` 拆出带类型的两步 |
| `crates/rurge-http/src/capture/slot.rs`（新）、`crates/rurge-http/src/engine.rs`、`crates/rurge-engine/src/http.rs`（只补 `capture: None`） | 3 | `CaptureSlot`、两个取样点、`Idle` 里抄请求正文、包住应答正文的一层 |
| `crates/rurge-engine/src/{shared.rs,engine.rs,http.rs,lib.rs}`、`crates/rurge-engine/tests/capture.rs`（新） | 4 | 一份存储、`capture` 开关、填槽、给 `capture`；端到端的前一半 |
| `crates/rurge-api/Cargo.toml`、`crates/rurge-api/src/lib.rs`、`crates/rurge-api/src/routes/{mod.rs,features.rs,capture.rs（新）}`、`crates/rurge-api/tests/api.rs` | 5 | 开关与五个端点 |
| `crates/rurge-api/Cargo.toml`、`crates/rurge-api/src/{lib.rs,har.rs（新）}`、`crates/rurge-api/src/routes/capture.rs`、`crates/rurge-api/tests/api.rs` | 6 | HAR |
| `crates/rurge/src/cli/run.rs`、`crates/rurge/tests/cli.rs` | 7 | 两个参数、大小的写法、降低与 WARN；命令行的端到端 |
| `crates/rurge-engine/tests/capture.rs`、`crates/rurge-http/src/capture/{memory.rs（新）,mod.rs}` | 8 | 端到端的后一半；量内存 |
| `docs/acceptance/phase4-manual.md`、`docs/api/phase4.md`、`docs/testing.md` | 9a | 手工清单加 M4a 的条目；M4a 的 API 与测试文档 |
| 收尾文档（任务 9b 列全） | 9b | M4a 的收尾 |

模块的名字与拆法可以照实际调整，调了记进修正记录。

---

## Task 0：开工前核对

**做什么**：对着当时的分支核对下面每一项。有出入的记进「执行期修正记录」，一项一行，写"计划写 X，代码是 Y，按 Y 做"；按记录调整后面的任务。**不改计划的正文。** 没有出入也记一行。

1. **工作树**：`git status --short` 空；当前分支 `phase4-m4`；`a6e78ad`（M4 设计）在历史里，之后只多了本计划的提交。
2. **读**：设计第 18 节（先读）、第 1 ～ 9、11 ～ 14、16、17 节；本计划的「计划期决定」与 P16 的表；`docs/status.md` 阶段 4 / M3b 一节（「以后的会话必须知道的事」整段）与 M3a、M2b 两节的同名段落。
3. **名字与签名**（设计第 14 节 V1 ～ V12）：用 `git grep -n` 逐项找到、记下行号；与「计划期决定」写的不同的照实记。
   - `rurge-inbound` 的 `session.rs`：`SessionOutcome`、`FinishHook`、`SessionHandle` 的字段（`method`、`status`、`finished`、`outcome`、`on_finish`）、`new`、`new_with_token`、`id`、`elapsed`、`on_first_byte`、`on_finish`、`finish`、`outcome`、`connect_time`、`first_byte_time`、`error`、`rule`、`policy_chain`、`notes`、`trail`、`kill`；它的测试模块。`rurge-net` 的 `trail.rs`：`Trail::{peer, dns, set_peer}`。
   - `rurge-config`：`rule.rs` 的 `RuleParams`（`always_capture`、`unknown`）与 `RuleParams::parse` 里 `always-capture` 那一支——`always-capture=`、`always-capture= `（等号后一个空格）与不带 `=` 的 `always-capture` 各读成什么；`config.rs` 读 `[Rule]` 的循环（给 `W0003` 的那两段）与测试模块里断言 `W0003` 的用例（任务 1 照它写）；`diagnostic.rs` 的 `W_INVALID_VALUE`、`codes::hint` 对它的决定、`every_code_is_decided_once`；语料库 `kitchen-sink.conf` 里的 `always-capture=debug`。
   - `rurge-rules`：`CompiledRule`（`index`、`raw`、`params`）、`RuleEngine::rules`；命中 `RULE-SET`、`DOMAIN-SET` 与逻辑规则时 `Decision.matched` 是哪一行的下标；逻辑规则（`AND`）在 `[Rule]` 里的写法与一条现有的用例。
   - `rurge-http`：
     - `lib.rs` 的模块与再导出；`Cargo.toml`（今天没有 `[features]`）；
     - `rewrite/body.rs` 的 `Limit`、`REQUEST`、`RESPONSE`、`decode`、`zlib_header`、`is_token` 与 `decode` 的现有用例；
     - `engine.rs` 的 `Entry`（连接的入口，与 `capture::Entry` 同名）、`Conn`（`h2`、`idle`、`drop_hold`、`entry`）、`Routed`、`Requests::handle` 的各步：HTTP/2 上 `CONNECT` 的 501、监听器的 400、`prepare`、`open`、`Guard::new`、`route`、`token` 的检查、`Routed` 的解出、`prepared.unknown`、`sniffed`、`url`、`wants_upgrade`、`tunnel`、`needs_h1`、`borrow_h2` 交回 `Some` 的那一支、`pool.take` 交回 `Some` 的那一支、`redial`、`Form::of`、第 5 步的 `headers::strip` 与 `Keep`、`absolute_form` / `origin_form`、补 chunked 的那一句、`fit`、`write_host`、`head_of`、`client_upgrade`、`request_meter`、`method`、`Idle::new`（唯一的调用处）、发送的 `select!` 与重发、升级那一支与 `promote`、HTTP/2 的应答改成 HTTP/1.1 的那一句、应答的 `strip`、`set_status`、`Tail::new`；
     - `engine.rs` 的 `answer`（`REJECT-DROP` 那一支）、`failed`、`page`、`timed_out`、`map_local`、`promote`；`Idle`（字段、`new`、`poll_frame`）；`Tail::poll_frame` 里"先 `complete()`、再交出最后一帧"的那一支；`Tail`、`LocalFile` 与 `Guard` 的 `Drop`；`headers.rs` 的 `Keep`、`strip`、`HOP_BY_HOP`；
     - `engine.rs` 的测试模块：`Step`、`Inner`、`Fake`（`new`、`script`、`rewrite`、`forward`、`go_pages`、`handle`、`finished`）、`Fake::route` 的两处 `Routed`、`Client`、`Rig`、`connected`、`rig_timed`（参数各是什么）、`rig_h2`、`h2_client`、`h2_request`、`recording_origin`、`streaming_origin` 与 `Holding`、`fixed_origin`、`chunked_origin`、`origin_answering`、`stalled_origin`、`upgrading_origin`、`body_and_trailers`、`wait_until`、建 `Rewrite` 的夹具（`rewrite_of` 等）；现有用例 `a_dropped_request_is_held_then_closed`、`a_killed_request_closes_its_connection`、`a_websocket_upgrade_is_relayed_and_counted_once`、`a_connect_on_a_sniffed_connection_goes_to_its_target`、`proxy_authorization_is_forwarded_only_on_a_sniffed_connection`、`a_stream_the_client_resets_is_cancelled_and_the_connection_stays`、`an_h2_client_is_served_over_an_http1_upstream` 与测 Map Local 文字数据的那一条。
   - `rurge-engine`：
     - `shared.rs` 的 `EngineShared`（字段、`new`、`Default` 的写法）；`engine.rs` 的 `mitm_on` / `rewrite_on` 两个字段与它们的初值、`rewrite_on`、`set_rewrite`、`restore_switches`、`shared()`、`http_counts`、`choose_policy_with`（三处早退与 `set_rule` 那一句）、`rule_raw`、`settle`（它结不结束句柄）、出站模式怎样切换（用例里怎样切到直连）；`http.rs` 的 `ConnUpstreams::route`；`lib.rs` 的再导出；`state.rs` 的 `Features.capture`、`STATE_VERSION`；
     - 用例：`tests/common/mod.rs` 的 `Harness`、`Profile`、`harness`、`harness_with`、`plain_get`、`wait_until`、`the_records`、`LogCapture`；`tests/rewrite.rs` 的 `RecordingProxy`、`harness_mitm`、`decrypted_get`、`mitm_section`；`tests/mitm.rs` 的 `the_switch_is_restored_from_state_json`；`TlsFixture::with_ca_name`；用例怎样重载一次 Profile；`tests/pipeline.rs` 里带 `password@` 的 `http-listen` 与它的 Basic 载荷。
   - `rurge-api`：`lib.rs` 的路由与 `App`；`routes/mod.rs`；`features.rs`（`FEATURES`、`get_feature`、`set_feature`、模块的文档注释）；`requests.rs`（`DEFAULT_LIMIT`、`recent` 里 `limit` 的读法）；`error.rs`；`profiles.rs` 的 `requested_name` 与 `BAD_BODY`；`tests/api.rs` 的 `Api`（`engine`、`state_path`）、`api()`、`get`、`post`、`call`、`wait_until`、`features_collections_stop_and_unknown_paths`、`the_rewrite_switch_is_read_switched_and_kept`。
   - `rurge`：`cli/run.rs` 的 `request_log_size` 参数、`RunOptions`、`parse_log_level`（`value_parser` 的写法）、`EngineShared::new` 与其后改字段的几句、`restore_switches` 的调用；它的测试模块；`tests/cli.rs` 的 `run` 模块：`write_conf`、`rurge_run`、`spawn_command`、`Daemon`（`http`、`lines`）、`wait_for_line`、`api_port`、`api_call`、`http_get`、`API_GENERAL`，`the_request_log_holds_as_many_requests_as_request_log_size_says`，别处怎样断言 clap 的错误与退出码。
   - `crates/rurge-tun/src/tests/bench/peak.rs` 的 `peak_resident_bytes`、`parse_count`、`parse_vm_hwm` 与它的用例；`crates/rurge-rules/tests/memory.rs` 开头的说明。
   - 外部库（注册表源码），行号写进记录：
     - hyper 1.11.1：HTTP/1.1 服务端在应答正文 `is_end_stream` 为真时不再 poll、写完头就丢下它（`proto/h1/dispatch.rs`）；`size_hint` 确切时写 `content-length`（`proto/h1/role.rs`）；HTTP/2 服务端不看应答的 `version()`、客户端重置流时丢下服务的 future；
     - http 1.5.0：`Version` 的 `Debug`、`StatusCode::canonical_reason`、`HeaderMap::iter` 的次序；
     - time 0.3.45、axum-core 0.5.6、futures-util 0.3.34、base64 0.22：「技术栈」里列的几处。
4. **数**：
   - `Cargo.lock` 里 `[[package]]` 的个数（520）；
   - `unsafe_functions.rs` 的 `LISTED` 的项数（23）；
   - `sockets.rs` 的 `LISTED` 的行数（26）；
   - 带本机账户名的文件数：派工文件给出要数的那个名字，数出来是 26。
5. **现有用例**（P16 的表）：逐条找到、读它断言什么，把"预计要改 / 预计照过"逐条写进修正记录，一条用例一句。表里没有的照实补一行。另扫一遍：`git grep -n 'Routed::'`、`git grep -n 'Idle::new'`、`git grep -n 'always_capture\|always-capture'`、`git grep -n '"capture"'`、`git grep -n 'EngineShared {'`。
6. **门禁**：跑一遍，记下测试二进制数、通过数、失败数与忽略数（M3b 收尾时是 92 / 5227 / 0 / 13，一台机器上的一次运行，不是 CI）。

**判据**：修正记录里有任务 0 的行；门禁的数字记下。

---

## Task 1：`when_finished`、抓包会话的槽与空的 `always-capture=`

**改哪里**：

- `crates/rurge-inbound/src/session.rs`：`SessionHandle` 多一个字段 `capture_session: OnceLock<Arc<str>>`（与 `method`、`status` 并排，三个构造函数都初始化）；`when_finished`、`set_capture_session`、`capture_session`；测试模块里的用例。
- `crates/rurge-config/src/config.rs`：读 `[Rule]` 的循环里给 `W0012`；测试模块里一条用例。

**接口**

- 消费：现有的 `on_finish`、`finish`、`outcome`。
- 产出：

```rust
impl SessionHandle {
    /// 记录还没有结束：同 `on_finish`；已经结束：当场以它的结局调 `f`。与 `finish` 并发时不漏、不调两次（P3）
    pub fn when_finished(&self, f: impl FnOnce(&SessionHandle, &SessionOutcome) + Send + 'static);
    /// 命中的那一行的 `always-capture`：只有第一次设的算（P3）
    pub fn set_capture_session(&self, name: Arc<str>);
    pub fn capture_session(&self) -> Option<Arc<str>>;
}
```

**用例（先红）**

- `session.rs`：
  - `when_finished_waits_for_an_open_session`：先注册，`finish` 之前没有调；`finish(Completed)` 之后恰好调了一次、结局是 `Completed`；再 `finish(Failed(..))` 不多调。
  - `when_finished_runs_at_once_on_a_finished_session`：先 `finish(Rejected(RejectKind::Drop))`，再注册：在 `when_finished` 返回之前就调了，恰好一次，结局是 `Rejected(Drop)`。
  - `when_finished_hooks_keep_their_order`：`on_finish` 记 A、`when_finished` 记 B、`on_finish` 记 C，`finish` 之后次序是 A、B、C。
  - `a_hook_may_ask_for_another_from_inside`：在已经结束的句柄上 `when_finished`，它的 `f` 里再调一次 `when_finished`：两个都调了，没有死锁（钉住"`f` 在锁之外调"）。
  - `when_finished_and_finish_race_without_losing_or_repeating`：一万轮，每轮一个新句柄与一个 `std::sync::Barrier::new(2)`，一个线程 `finish(Completed)`、另一个线程 `when_finished(计数加一)`，两个都先等在屏障上；`join` 之后计数恰好是 1。提交前单独循环跑二十次。
  - `the_capture_session_is_the_first_name_set`：新句柄是 `None`；先设 `"a"`、再设 `"b"`，读到 `Some("a")`。
- `config.rs`：`an_empty_always_capture_is_ignored_with_w0012`（照同一模块断言 `W0003` 的用例的写法）：`[Rule]` 里
  - `FINAL,DIRECT,always-capture=` → 恰好一条 `W0012`，级别警告，消息恰好是表后「原文」的 P4，`span` 是这一行；规则照常在 `config.rules` 里；
  - `DOMAIN,c.test,DIRECT,always-capture= ` → 同样一条 `W0012`（任务 0 记下它读成了什么，读法不同就照实改这一条、记进修正记录）；
  - `DOMAIN,a.test,DIRECT,always-capture=debug` → 没有诊断；
  - `DOMAIN,b.test,DIRECT,always-capture` → 一条 `W0003`（照今天），没有 `W0012`（不要求先红）。

**实现**

- `when_finished`、`set_capture_session`、`capture_session`：照 P3。
- `W0012`：照 P4。

**判据**

- 上面的用例都在、都绿；并发的那一条单独循环跑过二十次，次数与结果写进报告。
- 语料库快照不变（`always-capture=debug` 不是空的）；不设新的诊断码。
- 门禁绿。

---

## Task 2：`capture` 模块

**改哪里**：

- `crates/rurge-http/Cargo.toml`：`[features]` 一节，`testing = []`（P2）。
- `crates/rurge-http/src/lib.rs`：`pub mod capture;`。
- `crates/rurge-http/src/capture/mod.rs`（新）：类型、常量、`Capture`、`Body::decoded`、`NotDecoded`、`Entry` 的两个时间方法；测试模块在文件末尾。
- `crates/rurge-http/src/capture/store.rs`（新）：`CaptureStore`、`Status`、`SessionSummary`、`Pending`、条目的账；测试模块在文件末尾。
- `crates/rurge-http/src/rewrite/body.rs`：`decode` 拆出 `layers`、`undo` 与 `Undo`（P7），现有用例不改。

**接口**

- 消费：`rewrite::body` 的 `zlib_header`、`is_token` 与三种解码器（经 `undo`）。
- 产出（`rurge_http::capture`；细节见 P5、P6、P7）：

```rust
pub const DEFAULT_SESSION: &str = "default";
pub const DEFAULT_TOTAL: u64 = 100 * 1024 * 1024;
pub const DEFAULT_BODY: u64 = 5 * 1024 * 1024;
/// 每个条目在头与文字之外另记的字节（设计第 4.3 节；任务 8 量过之后可以改）
pub const ENTRY_OVERHEAD: u64 = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits { pub total: u64, pub body: u64 }
impl Limits { pub fn new(total: u64, body: u64) -> Limits; }      // body 降到 total
impl Default for Limits { /* DEFAULT_TOTAL、DEFAULT_BODY */ }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status { pub limits: Limits, pub used: u64, pub entries: usize,
                    pub stored: u64, pub evicted: u64, pub dropped: u64, pub truncated: u64 }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSummary { pub name: Arc<str>, pub entries: usize, pub bytes: u64 }

pub struct CaptureStore { /* limits；Mutex：队列、kept、reserved、四个数 */ }
impl CaptureStore {
    pub fn new(limits: Limits) -> Arc<CaptureStore>;
    pub fn limits(&self) -> Limits;
    pub fn status(&self) -> Status;
    pub fn sessions(&self) -> Vec<SessionSummary>;          // 只有有条目的，按名字（字节序）排
    pub fn session(&self, name: &str) -> Vec<Arc<Entry>>;   // 由旧到新；锁只握到拿完 Arc
    pub fn entry(&self, id: u64) -> Option<Arc<Entry>>;
    pub fn clear(&self, session: Option<&str>) -> usize;
    #[cfg(any(test, feature = "testing"))]
    pub fn put(&self, entry: Entry) -> bool;                 // 照同样的账存进一条做好的（P6）
}

#[derive(Clone)]
pub struct Capture { pub store: Arc<CaptureStore>, pub session: Arc<str> }
impl Capture { pub(crate) fn begin(&self) -> Pending; }

pub(crate) struct Pending { /* store、两个正文的 Taking、held */ }
impl Pending {
    pub(crate) fn request_chunk(&mut self, data: &[u8]);
    pub(crate) fn response_chunk(&mut self, data: &[u8]);
    /// 把两个正文交给 `make` 造出条目，存进去；false：丢掉了
    pub(crate) fn commit(self, make: impl FnOnce(Body, Body) -> Entry) -> bool;
}
impl Drop for Pending { /* 没有存进去：预留还回去 */ }

pub struct Entry {
    pub id: u64, pub session: Arc<str>, pub started: SystemTime,
    pub request: Head, pub request_body: Body,
    pub response: Option<ResponseHead>, pub response_body: Body,
    pub timings: Timings, pub remote: Option<SocketAddr>,
    pub outcome: Outcome, pub error: Option<String>,
    pub rule: Option<String>, pub policy: Vec<String>, pub notes: Vec<String>,
}
impl Entry {
    pub fn started_ms(&self) -> u64;                 // Unix 毫秒
    pub fn started_rfc3339(&self) -> String;         // `YYYY-MM-DDTHH:MM:SS.mmmZ`
}
pub struct Head { pub method: Method, pub url: String, pub version: Version, pub headers: HeaderMap }
pub struct ResponseHead { pub status: StatusCode, pub version: Version, pub headers: HeaderMap }
pub struct Body { pub data: Bytes, pub size: u64, pub truncated: Option<Truncated> }
impl Body {
    pub fn empty() -> Body;
    /// `headers` 是这一侧的头；Ok(None)：没有要解的编码（P7）
    pub fn decoded(&self, headers: &HeaderMap, limit: u64) -> Result<Option<Bytes>, NotDecoded>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Truncated { Limit, Budget }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Timings { pub connect: Option<Duration>, pub dns: Option<Duration>, pub wait: Option<Duration>,
                     pub receive: Option<Duration>, pub total: Duration, pub reused: bool }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome { Completed, Upgraded, Rejected(RejectKind), Failed }
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotDecoded { Truncated, Unsupported(Option<String>), Corrupt(String), TooLarge(u64) }
impl fmt::Display for NotDecoded { /* 原文 P7 */ }

// rewrite/body.rs（crate 内）
pub(crate) enum Undo { Unsupported(Option<String>), Corrupt(String), Over }
pub(crate) fn layers(encodings: &[HeaderValue]) -> Result<Vec<String>, Undo>;
pub(crate) fn undo(layers: &[String], body: &Bytes, limit: u64) -> Result<Bytes, Undo>;
```

**用例（先红）**

- `capture/mod.rs`：
  - `limits_lower_the_body_to_the_total`：`Limits::new(10, 20)` 是 `{10, 10}`；`Limits::new(20, 10)` 是 `{20, 10}`；`Limits::default()` 是 `{104857600, 5242880}`。
  - `started_is_written_as_rfc3339_in_utc_with_milliseconds`：`started = UNIX_EPOCH + 1_761_984_000_123 ms` 的条目：`started_ms()` 是 `1761984000123`，`started_rfc3339()` 是 `"2025-11-01T08:00:00.123Z"`；`UNIX_EPOCH + 5 ms` 是 `"1970-01-01T00:00:00.005Z"`。
  - `bodies_are_decoded_layer_by_layer`：`content-encoding: deflate, gzip`（先 deflate 再 gzip 压的）解回原文；`x-gzip` 同 `gzip`；`Content-Encoding` 分两行写同样；空的 `gzip` 正文解成空的（`Ok(Some(空))`）。
  - `a_body_without_an_encoding_is_not_decoded`：没有 `content-encoding`、只有 `identity` 或空的项 → `Ok(None)`，截断的也是。
  - `what_cannot_be_decoded_says_why`：截断的 `gzip` → `Truncated`；`br` → `Unsupported(Some("br"))`；值里有 `@` 的 → `Unsupported(None)`；坏的 `gzip` → `Corrupt("gzip")`；解开之后超过 `limit`（10）→ `TooLarge(10)`。每一种的 `to_string()` 恰好是原文 P7 那一句。
- `capture/store.rs`（用一个自己造的条目的夹具：几个头、URL、规则、策略、备注各定好长度）：
  - `an_entry_costs_its_headers_texts_bodies_and_the_overhead`：两个头各 `名字 + 值 + 32`、URL、方法、规则、策略、备注、错误的字节、两个正文存下的字节与 `ENTRY_OVERHEAD`，用例里逐项写出加起来的数；`put` 之后 `status().used` 恰好是它，`sessions()[0].bytes` 也是。
  - `a_body_over_its_limit_keeps_its_beginning`：单条上限 4，应答的块 `"012"`、`"345"`、`"6"` → `data == "0123"`、`size == 7`、`truncated == Some(Limit)`；存进去之后 `status().truncated == 1`。
  - `a_body_limit_of_zero_keeps_the_heads_only`：单条 0，块 `"abc"` → `data` 空、`size == 3`、`Some(Limit)`；条目照样存进去。
  - `the_oldest_entries_go_first_whatever_their_session`：总量刚好放得下三条，依次存 `a`、`b`、`a`、`b` 四个会话的条目 → `evicted == 1`，留下的是后三条，最早那条 `a` 不在了。
  - `a_chunk_evicts_older_entries_to_find_room`：队列里有两条，一个进行中的条目的一块要的地方只有挤掉最早那条才够 → 那一条被挤掉、`evicted == 1`、这一块整块存下。
  - `a_chunk_that_cannot_fit_even_in_an_empty_store_is_truncated_for_the_budget`：总量 2000、单条 2000；队列里先 `put` 一条账在 1000 以内的条目（用例里写出它的账）；进行中的 A 存下一块 800（`kept + 800 <= 2000`，不挤）；进行中的 B 的一块 1300 → `reserved + 1300 > 2000`，挤光队列也放不下：B 是 `Some(Budget)`、`evicted == 0`、那一条还在；A 的预留不变（`used` 是那一条的账加 800）。
  - `an_entry_whose_head_does_not_fit_is_dropped`：总量 0 → `put` 交回 `false`、`dropped == 1`、`used == 0`；总量放得下正文而放不下头的 → 同样丢掉，正文的预留还回去（`used == 0`）。
  - `a_pending_entry_dropped_gives_its_reservation_back`：开始一条、存下 100 字节的块、不存进去就丢下 → `used == 0`。
  - `the_store_never_holds_more_than_its_total`：确定的伪随机序列（例如线性同余，种子写在用例里）跑一千步：开始、喂块、存进去、丢下；每一步之后 `status().used <= limits.total`；最后全部丢下、清除之后 `used == 0`。
  - `entries_are_listed_by_session_and_found_by_id`：`session("a")` 由旧到新；`sessions()` 按名字排、带条数与字节；`entry(id)` 找得到、不在的是 `None`；没有条目的会话不在 `sessions()` 里，`session(..)` 是空的。
  - `clearing_one_session_or_all_keeps_the_four_counts`：`clear(Some("a"))` 交回 `a` 的条数、`b` 的还在；`clear(Some("nope"))` 是 0；`clear(None)` 交回余下的条数、`used == 0`；四个数不变。
  - `an_evicted_entry_lives_on_while_it_is_held`：拿着 `session(..)` 交回的 `Arc<Entry>`，把它挤掉、再清除，它的正文照样读得出、与存进去的相同。
  - `put_stores_an_entry_made_elsewhere`：`put` 存进去的条目与 `commit` 的一样记账、计 `stored`。
- `rewrite/body.rs`：现有的 `decode` 用例一条不改、照过（不要求先红）。

**实现**

- 类型、`Display`、两个时间方法：照 P5、P7。
- 存储与记账：照 P6。
- `decode` 的拆法：照 P7。

**判据**

- 上面的用例都在、都绿；`rewrite` 的现有用例照过。
- `Entry`、`Head`、`ResponseHead`、`Body`、`Capture`、`CaptureStore`、`Pending` 没有 `Debug`（不派生、不手写）。
- `Cargo.lock` 的包数仍是 520，锁文件不变。
- 门禁绿。

---

## Task 3：引擎里的抓包

**改哪里**：

- `crates/rurge-http/src/capture/slot.rs`（新）：`CaptureSlot`、`RequestTap`、包住应答正文的一层（P8）。它的用例在 `engine.rs` 的测试模块里（要用那里的夹具）。
- `crates/rurge-http/src/capture/mod.rs`：`mod slot;` 与 crate 内的再导出。
- `crates/rurge-http/src/engine.rs`：`Routed` 的两个变体多 `capture: Option<Capture>`；`Requests::handle` 改成包着 `serve` 的一层；求规则之后 `arm`；第 1 个取样点与 `reused`；`Idle` 多一个参数；升级那一支先 `upgrading()`（P9）；测试模块的 `Fake` 多一个格子、用例。
- `crates/rurge-engine/src/http.rs`：`ConnUpstreams::route` 的两处 `Routed` 补 `capture: None`（任务 4 换成真的）。

**接口**

- 消费：任务 1 的 `when_finished`；任务 2 的 `Capture`、`Pending`、`Entry` 等。
- 产出：

```rust
// engine.rs
pub enum Routed<R, K> {
    Go { route: R, key: K, pages: Pages, rewrite: Option<Arc<Rewrite>>, capture: Option<Capture> },
    Refused { refusal: Refusal, pages: Pages, capture: Option<Capture> },
}
// capture/slot.rs（crate 内）
pub(crate) struct CaptureSlot { /* OnceLock<Arc<Mutex<Armed>>> */ }
impl CaptureSlot {
    pub(crate) fn new() -> CaptureSlot;
    pub(crate) fn arm(&self, capture: Option<Capture>, handle: &Arc<SessionHandle>, head: impl FnOnce() -> Head);
    pub(crate) fn armed(&self) -> bool;
    pub(crate) fn leaving(&self, head: Head, reused: bool);
    pub(crate) fn request_tap(&self) -> Option<RequestTap>;
    pub(crate) fn upgrading(&self);
    pub(crate) fn answered(self, result: Result<Response<ResponseBody>, Close>, h2: bool)
        -> Result<Response<ResponseBody>, Close>;
}
impl Drop for CaptureSlot { /* 没有回答过：没有应答 */ }
#[derive(Clone)]
pub(crate) struct RequestTap(/* Arc<Mutex<Armed>> */);
impl RequestTap { pub(crate) fn chunk(&self, data: &[u8]); }
```

**用例（先红）**（`engine.rs` 的测试模块；`Fake` 多一个 `capture: Mutex<Option<Capture>>` 与 `fn capture(&self, capture: Capture)`，`route` 把它的克隆放进两个变体。夹具 `fn capturing(fake: &Fake, limits: Limits) -> Arc<CaptureStore>` 建一份存储、以会话 `"t"` 交给 `fake`；`async fn one_entry(store: &CaptureStore) -> Arc<capture::Entry>` 以 `wait_until` 等 `"t"` 里恰好有一条再交回。`capture::Entry` 与连接的 `Entry` 同名，测试里写全路径）

1. `a_request_and_its_answer_are_captured`：`recording_origin`，经监听器问 `http://a.test/x?y=1`、带 `host: a.test`、`x-a: 1` 与 `connection: keep-alive`：
   - 条目的 `id` 是这个请求的句柄的 `id()`，`session` 是 `"t"`；
   - 请求：`GET`、`url` 是记录的 URL、`HTTP/1.1`，头里有 `x-a` 与 `host`、没有 `connection`；
   - 应答：200、`HTTP/1.1`、源站给的头在、正文是客户端收到的正文；
   - `Outcome::Completed`、`error` 是 `None`；`connect`、`wait`、`receive` 都是 `Some`，`connect + wait + receive == total`；`reused` 是假；`remote` 是源站的地址。
2. `a_second_request_on_a_kept_connection_is_captured_as_reused`：同一条客户端连接上的第二个请求借了池里的连接：它的条目 `reused` 为真、第一个的为假。
3. `the_captured_request_is_the_one_that_leaves_without_rurges_forward_headers`：`fake.forward(vec![("Proxy-Authorization", "Basic cnVyZ2U6c2VjcmV0")])` 让请求以绝对形式交给"上游代理"，另用 `rewrite_of` 给一条加 `x-added: 1` 的 Header Rewrite：源站收到了 `proxy-authorization`；抓到的请求头里有 `x-added`、没有 `proxy-authorization`。
4. `a_request_that_never_left_is_captured_as_it_was_routed`：`Step::Refused { Reject(Reject), pages.reject: true }`，请求带 `connection: keep-alive`、`keep-alive: timeout=5`、`x-a: 1`：抓到的请求头有 `x-a`、没有那两个逐跳头；应答 403；`Rejected(Reject)`；`connect` 与 `remote` 是 `None`；请求正文 `size == 0`。
5. `the_wrapped_answer_keeps_its_length_and_its_last_frame`：`fixed_origin(b"0123456789")`：客户端收到的头里 `content-length: 10`；抓到的应答正文恰好是这十个字节、`truncated` 是 `None`。
6. `a_chunked_answer_is_captured_whole_without_its_trailers`：源站以 chunked 答、带一个 trailer `x-t: 1`：抓到的正文是全部数据，应答头里没有 `x-t`。
7. `a_body_over_the_limit_is_captured_truncated`：单条上限 4、`fixed_origin` 十个字节：`data == "0123"`、`size == 10`、`Some(Limit)`；`status().truncated == 1`；客户端照样收到十个字节。
8. `a_client_that_leaves_mid_body_is_captured_as_failed`：`streaming_origin` 答 `content-length: 100` 与十个字节、然后停住；客户端读到这十个字节之后关掉连接：条目 `Outcome::Failed`、应答 200、正文恰好是那十个字节；`store.clear(None)` 之后 `status().used == 0`。
9. `a_killed_request_leaves_no_reservation_behind`：同上的源站，客户端读到一部分之后 `kill` 这个请求的句柄：条目 `Outcome::Failed`；清除之后 `used == 0`。
10. `an_upgrade_is_captured_at_its_101`：`upgrading_origin`：在隧道还开着的时候条目就在了：`Outcome::Upgraded`、应答 101、应答正文 `size == 0`、请求头里留着 `upgrade` 与 `connection`；之后关掉隧道，条目不变、没有第二条。
11. `a_connect_tunnel_is_captured_at_its_2xx_with_host_and_port_as_its_url`：嗅探到的连接上 `CONNECT a.test:443`：条目的 `method` 是 `CONNECT`、`url` 是 `"a.test:443"`、`Outcome::Upgraded`、应答 200。
12. `a_closed_answer_is_captured_with_no_response`：`Step::Refused { Reject(Reject), pages.reject: false }`（关掉连接作答）：`response` 是 `None`、应答正文 `size == 0`、`Rejected(Reject)`。
13. `a_dropped_request_is_captured_before_its_hold_ends`：`rig_timed` 把 `drop_hold` 设成 60 秒，`Step::Refused { Reject(Drop) }`：五秒之内条目就在了，`response` 是 `None`、`Rejected(Drop)`、`timings.total < 5 s`；然后 `rig.stop()`。
14. `an_http2_stream_reset_before_its_answer_is_captured_with_no_response`：`rig_h2` 对着 `stalled_origin`，发出请求之后重置这个流（照现有的 `a_stream_the_client_resets_is_cancelled_and_the_connection_stays` 的做法）：条目 `response` 是 `None`、`Outcome::Failed`。
15. `an_http2_client_over_an_http1_upstream_is_captured_as_http2`：`rig_h2` 对着 HTTP/1.1 的源站：请求与应答的 `version` 都是 `HTTP_2`，正文在。
16. `a_rewritten_request_body_is_captured_as_sent`：`rewrite_of` 给一条把 `a` 换成 `b` 的请求 Body Rewrite，`POST` 正文 `aaa`：源站收到 `bbb`；抓到的请求正文是 `bbb`、`size == 3`。
17. `a_map_local_answer_is_captured`：`rewrite_of` 给一条 `data-type=text`、`data=hello` 的 Map Local：抓到的应答 200、正文 `hello`；`connect` 与 `remote` 是 `None`。

用例 8、9、10、13、14 带等待：提交前各自单独循环跑二十次。

**实现**

- `CaptureSlot`：照 P8。
- 引擎里的位置：照 P9。
- `crates/rurge-engine/src/http.rs`：两处 `Routed` 补 `capture: None`，注释写"the phase 4 / M4a plan's task 4 gives the real one"。

**判据**

- 上面的用例都在、都绿；`engine.rs` 测试模块的现有用例一条不改期望、照过（P16）。
- `answer`、`failed`、`page`、`map_local`、`timed_out`、`Tail`、`LocalFile`、`Guard` 的函数体没有改动（`git diff` 里没有它们的行）。
- 带等待的用例各自循环二十次的结果写进报告。
- 门禁绿。

---

## Task 4：`rurge-engine`：存储、开关、填槽与给 `capture`

**改哪里**：

- `crates/rurge-engine/src/shared.rs`：`EngineShared.capture`。
- `crates/rurge-engine/src/engine.rs`：`capture_on` 字段、`capture_on`、`set_capture`、`restore_switches`、`capture`、`capture_for`；`always_capture`；`choose_policy_with` 填槽。
- `crates/rurge-engine/src/http.rs`：两处 `capture` 换成 `self.engine.capture_for(handle)`。
- `crates/rurge-engine/src/lib.rs`：`pub use rurge_http::capture;`。
- `crates/rurge-engine/tests/capture.rs`（新）：端到端的前一半。

**接口**

- 消费：任务 1 的 `set_capture_session` / `capture_session`；任务 2 的 `CaptureStore`、`Capture`、`Limits`、`DEFAULT_SESSION`；任务 3 的 `Routed`。
- 产出（细节见 P10）：

```rust
// shared.rs
pub capture: Arc<rurge_http::capture::CaptureStore>,
// engine.rs
impl Engine {
    pub fn capture(&self) -> Arc<CaptureStore>;
    pub fn capture_on(&self) -> bool;
    pub async fn set_capture(&self, on: bool);
    pub(crate) fn capture_for(&self, handle: &SessionHandle) -> Option<Capture>;
}
pub(crate) fn always_capture(rules: &rurge_rules::RuleEngine, index: usize) -> Option<Arc<str>>;
// lib.rs
pub use rurge_http::capture;
```

**用例（先红）**（`tests/capture.rs`，做法见 P14；`[Host] example.test = 127.0.0.1`，源站是回环上的 `TestServer`，经 HTTP 监听器以 `plain_get` 问 `http://example.test:{端口}/…`）

- `nothing_is_captured_while_the_switch_is_off`：开关缺省关、规则没有 `always-capture`：记录结束之后（`the_records`），固定等待 200 ms（注释：等一个本来就不该来的存进去；存进去在记录结束与正文交完两者之后，回环上是微秒级）→ `status().stored == 0`、`capture_on()` 为假。
- `the_switch_captures_into_default`：`set_capture(true).await` 之后的请求进 `default`：`sessions()` 恰好是 `[default, 1 条]`，条目的 `id` 是记录的 `id`。
- `always_capture_captures_with_the_switch_off`：`DOMAIN,example.test,DIRECT,always-capture=debug-1`、开关关着：条目在 `debug-1`，`default` 不在。
- `always_capture_on_final_captures_too`：`FINAL,DIRECT,always-capture=last`：条目在 `last`。
- `a_logical_rule_names_the_session_of_its_line`：`AND,((DOMAIN,example.test),(DEST-PORT,{端口})),DIRECT,always-capture=logic`（逻辑规则的写法照任务 0 找到的现有用例）：条目在 `logic`。
- `the_session_a_rule_names_comes_before_default`：开关开着、规则写 `always-capture=named`：条目在 `named`、`default` 没有条目。
- `the_direct_mode_reads_no_rule_and_so_no_always_capture`：出站模式切到直连（照任务 0 找到的写法）、规则写着 `always-capture=named`：开关关着时什么也不存（同第一条的等待与注释）；开关开着时进 `default`。
- `turning_the_switch_off_keeps_what_was_captured`：开着存进一条、关掉：那一条还在；之后的请求不存（同上的等待）。
- `the_capture_switch_is_kept_in_state_json_and_restored`：照 `tests/mitm.rs` 的 `the_switch_is_restored_from_state_json` 写：`set_capture(true)` 之后 `state.json` 的 `features.capture` 是 `true`；用这份 `state.json` 起的新引擎 `restore_switches` 之后 `capture_on()` 为真；`false` 同样。

带等待的用例提交前各自单独循环跑二十次。

**实现**：照 P10。

**判据**

- 上面的用例都在、都绿；`rurge-engine` 的现有用例照过（P16）。
- 门禁绿。

---

## Task 5：API：开关与五个端点

**改哪里**：

- `crates/rurge-api/Cargo.toml`：正式依赖加 `base64.workspace = true`；开发依赖的 `rurge-http` 带 `features = ["testing"]`（P2）。
- `crates/rurge-api/src/lib.rs`：五条路由（P11）。
- `crates/rurge-api/src/routes/mod.rs`：`pub mod capture;`。
- `crates/rurge-api/src/routes/features.rs`：`capture` 生效；模块的文档注释加上它。
- `crates/rurge-api/src/routes/capture.rs`（新）：五个处理函数与它们的 JSON。
- `crates/rurge-api/tests/api.rs`：用例；`features_collections_stop_and_unknown_paths` 照 P16 改。

**接口**

- 消费：任务 4 的 `Engine::capture`、`capture_on`、`set_capture`；`rurge_engine::capture` 的类型。
- 产出：`GET /v1/rurge/capture`、`GET /v1/rurge/capture/sessions`、`GET /v1/rurge/capture/sessions/{name}`、`GET /v1/rurge/capture/entries/{id}`、`POST /v1/rurge/capture/clear`；`/v1/features/capture` 的 `GET` 与 `POST`。形状与错误照 P11。

**用例（先红）**（`api.rs`；条目经 `api.engine.capture().put(..)` 存进去，用例里写一个造条目的夹具，字段都是 `pub`）

- `the_capture_switch_is_read_switched_and_kept`：照 `the_rewrite_switch_is_read_switched_and_kept`：`GET` 是 `{"enabled":false}`；`POST {"enabled":true}` 之后 `GET` 是 `true`、`state.json` 的 `features.capture` 是 `true`；再关掉同样。
- `the_capture_status_says_the_limits_and_the_counts`：起始恰好是 `{"limits":{"total":104857600,"body":5242880},"used":0,"entries":0,"stored":0,"evicted":0,"dropped":0,"truncated":0}`；`put` 两条之后 `entries == 2`、`stored == 2`、`used` 等于 `api.engine.capture().status().used`（不是 0）。
- `capture_sessions_are_listed_by_name`：会话 `b`、`a` 各有条目：`[{"name":"a",…},{"name":"b",…}]`，`entries` 与 `bytes` 对。
- `a_capture_session_lists_its_entries_newest_first`：三条：摘要由新到旧、键与值照设计第 7.3 节（`totalMs` 是 `total` 向下取整的毫秒、`truncated` 是两个正文有一个截断就真、没有应答的 `status` 是 `null`）；`?limit=1` 只有最新的一条；`?limit=0` 是 400、`limit must be at least 1`；没有的会话是 404、消息照原文 P11。
- `an_entry_is_shown_whole_with_four_headers_masked`：一条带 `authorization`、`proxy-authorization`、`cookie`、`set-cookie` 与 `x-other: v` 的条目：四个头的值是 `***`、`x-other` 照原样；一个值里有 `0xFF` 字节的头读出来带 U+FFFD；两个 `body` 的 `base64` 解回存下的字节、`size` 与 `stored` 与 `truncated`（`"limit"`）对；`timings`、`remoteAddr`、`outcome`、`rejectKind`（`REJECT-DROP` 的写法照 `RejectKind::name`）、`error`、`rule`、`policy`、`notes` 照设计第 7.5 节的键；没有应答的条目 `"response": null`；没有 `decode` 时 `body` 里没有 `decoded` 与 `notDecoded`。
- `entry_ids_are_numbers_of_entries_that_are_there`：`/entries/abc` 是 400、`entry id must be a number`；`/entries/99` 是 404、`unknown capture entry 99`。
- `decode_undoes_gzip_and_says_why_it_cannot`：`?decode=true`：gzip 的应答 `decoded: true`、`base64` 是解开之后的、`notDecoded: null`；`br` 的 `decoded: false`、`notDecoded` 是原文 P7 那一句；截断的 gzip 是 `the body was truncated`；没有编码的 `decoded: false`、`notDecoded: null`。`?decode=false` 同没有写；`?decode=yes` 是 400、`decode must be true or false`。
- `clearing_takes_one_session_or_all`：没有请求体的 `POST` 清除全部、答 `{"cleared":N}`；`{}` 同；`{"session":"a"}` 只清 `a`；`{"session":"nope"}` 答 `{"cleared":0}`；`{"session":5}`、`[]`、`x` 各是 400、消息是原文 P11 的 `BAD_BODY`；四个数不清零。
- `odd_session_names_are_reached_through_the_path`：会话 `a b/c` 与 `调试` 各有一条：`/sessions/a%20b%2Fc` 与 `/sessions/%E8%B0%83%E8%AF%95` 各读到自己的那一条。
- 现有的 `features_collections_stop_and_unknown_paths`：照 P16 改。

**实现**：照 P11。

**判据**

- 上面的用例都在、都绿；`features_collections_stop_and_unknown_paths` 改了的地方写进提交说明。
- `Cargo.lock` 的包数仍是 520，只有 `rurge-api` 的依赖列表多了 `base64`。
- 门禁绿。

---

## Task 6：API：HAR

**改哪里**：

- `crates/rurge-api/Cargo.toml`：正式依赖加 `futures-util.workspace = true`。
- `crates/rurge-api/src/har.rs`（新）：一条的映射、`timings`、逐条写出的正文；单元用例在文件末尾。
- `crates/rurge-api/src/lib.rs`：`mod har;` 与第六条路由 `GET /v1/rurge/capture/sessions/{name}/har`。
- `crates/rurge-api/src/routes/capture.rs`：`har` 处理函数。
- `crates/rurge-api/tests/api.rs`：用例。

**接口**

- 消费：任务 5 的会话查找与 404；`Body::decoded`、`NotDecoded`、`Entry::started_rfc3339`。
- 产出（crate 内）：

```rust
// har.rs
pub(crate) struct HarTimings { pub blocked: i64, pub dns: i64, pub connect: i64, pub ssl: i64,
                               pub send: i64, pub wait: i64, pub receive: i64 }
/// 各项与 `time`（P12）
pub(crate) fn timings(t: &Timings) -> (HarTimings, i64);
pub(crate) fn entry(e: &Entry, body_limit: u64) -> serde_json::Value;
/// `{"log":…}`，条目由旧到新、一条一条序列化（P12）
pub(crate) fn document(entries: Vec<Arc<Entry>>, body_limit: u64) -> axum::body::Body;
```

**用例（先红）**

- `har.rs` 的单元用例：
  - `har_timings_add_up_to_the_total`：三种情形各一条，都带求规则时的一次解析：
    - 新拨的：`connect 30 ms`、`dns 10 ms`、`wait 20 ms`、`receive 5 ms`、`total 55 ms` → `blocked -1`、`dns 10`、`connect 20`、`ssl -1`、`send 0`、`wait 20`、`receive 5`，`time 55`；
    - 借来的：`connect 4 ms`、`dns 3 ms`、`wait 10 ms`、`receive 2 ms`、`total 16 ms`、`reused` → `blocked 1`、`dns 3`、`connect -1`、`wait 10`、`receive 2`，`time 16`；
    - 没连上的：`dns 7 ms`、`total 12 ms` → `blocked -1`、`dns 7`、`connect -1`、`wait 5`、`receive 0`，`time 12`；
    - 连上了而没有第一个字节的：`connect 8 ms`、`total 20 ms`、没有 `dns` → `dns -1`、`connect 8`、`wait 12`、`receive 0`，`time 20`；
    - 解析比连接长的：`dns 50 ms`、`connect 30 ms`、`wait 1 ms`、`receive 1 ms`、`total 32 ms` → `dns 30`、`connect 0`，`time 32`；
    - 带零头的（各段带几百微秒）：`time` 等于不是 `-1` 的各项之和，`totalMs − time` 在 0 到 3 之间。
  - `an_export_holds_its_entries_while_they_are_cleared`：存进三条、取 `session(..)` 建 `document`，在读第一块之前 `clear(None)`；读完整个正文、`serde_json` 解析：三条都在、正文完整。
- `api.rs`：
  - `a_session_is_exported_as_har_oldest_first`：`Content-Type: application/json`；`log.version == "1.2"`、`creator == {"name":"rurge","version":env!("CARGO_PKG_VERSION")}`、`pages == []`；条目由旧到新；`startedDateTime` 是 `started_rfc3339()`；`_rurgeId`、`_session`、`_outcome`、`_notes` 在；没有这个会话是 404、消息同任务 5。
  - `har_bodies_are_text_or_base64_and_say_what_was_left_out`：
    - gzip 的文字应答：`content.size` 是解开之后的字节数、`compression` 是 `size − bodySize`、`text` 是解开之后的文字、没有 `encoding`；
    - 二进制的应答：`text` 是 Base64、`"encoding":"base64"`；
    - 截断的应答：`comment` 是 `"{存下} of {见过} bytes stored"`；
    - `br` 的应答：`size` 是存下的字节数、`text` 是存下的字节的 Base64、`comment` 是 `"not decoded: content-encoding br is not supported"`；截断的 gzip 两种都有，以 `"; "` 接起来、截断在前；
    - 请求：UTF-8 的 JSON 正文 `postData.text` 照写、`mimeType` 取 `content-type`；二进制的写 Base64 与 `"_encoding":"base64"`；没有见过字节的请求没有 `postData`；`request.bodySize` 与 `response.bodySize` 是见过的字节数。
  - `har_says_no_response_as_mitmproxy_does`：没有应答的条目：`response.status 0`、`statusText ""`、`httpVersion ""`、`headers []`、`bodySize -1`、`content` 是 `{"size":0,"mimeType":""}`；`_error` 是条目的 `error`；没有 `remote` 时没有 `serverIPAddress` 这个键、有时是 IP（不带端口）。
  - `har_masks_the_four_headers_and_leaves_cookies_empty`：四个头写 `***`；`request.cookies` 与 `response.cookies` 是 `[]`；`redirectURL` 取 `location`、没有时空串；`statusText` 是 `canonical_reason`。
  - `har_splits_query_strings_and_gives_connect_none`：`https://a.test/p?x=1&y=%20z` → `[{"name":"x","value":"1"},{"name":"y","value":" z"}]`；`CONNECT` 的条目（`url` 是 `a.test:443`）是 `[]`；`request.httpVersion` 是 `HTTP/1.1` / `HTTP/2.0`；`headersSize` 两个都是 `-1`；`cache` 是 `{}`。

**实现**：照 P12。

**判据**

- 上面的用例都在、都绿。
- 导出不先在内存里拼出整个文件：`document` 交给 `Body::from_stream` 的是一条一条序列化的流（读代码可见）。
- `Cargo.lock` 的包数仍是 520，只有 `rurge-api` 的依赖列表多了 `futures-util`。
- 门禁绿。

---

## Task 7：可执行文件：两个参数与命令行的端到端

**改哪里**：

- `crates/rurge/src/cli/run.rs`：两个参数、`parse_size`、`capture_limits`、`EngineShared::new` 之后换上存储与 WARN；测试模块里的单元用例。
- `crates/rurge/tests/cli.rs`：`run` 模块里的用例。

**接口**

- 消费：任务 2 的 `CaptureStore::new`、`Limits::new`、`DEFAULT_TOTAL`、`DEFAULT_BODY`；任务 4 的 `EngineShared.capture`；任务 5、6 的 API。
- 产出：`--capture-size` / `RURGE_CAPTURE_SIZE`、`--capture-body-size` / `RURGE_CAPTURE_BODY_SIZE`；`pub(crate) fn parse_size(s: &str) -> Result<u64, String>`；`pub(crate) fn capture_limits(size: Option<u64>, body: Option<u64>) -> (Limits, Option<String>)`（P13）。

**用例（先红）**

- `run.rs` 的单元用例：
  - `sizes_are_bytes_with_an_optional_binary_suffix`：`"0"` → 0；`"1"` → 1；`"1k"`、`"1K"` → 1024；`"2m"` → 2 097 152；`"3G"` → 3 221 225 472；`"100M"` → 104 857 600；`"18446744073709551615"` → `u64::MAX`。`""`、`"k"`、`"1.5m"`、`"-1"`、`"+1"`、`"1kb"`、`" 1"`、`"1 k"`、`"0x10"`、`"1t"` 各是"invalid size"那一句；`"18446744073709551616"`、`"17179869184g"` 各是"too large"那一句（原文 P13，反引号里是写的原样）。
  - `the_body_size_is_lowered_to_the_capture_size`：`(Some(1024), Some(2048))` → `({1024, 1024}, Some(WARN 那一句，2048 与 1024))`；`(Some(1024), None)` → `({1024, 1024}, None)`；`(None, None)` → 缺省、`None`；`(None, Some(200 MiB))` → `({100 MiB, 100 MiB}, Some(..))`；`(Some(10), Some(10))` → `({10, 10}, None)`。
- `cli.rs` 的 `run` 模块（每条都 `env_remove` 两个环境变量，照 `the_request_log_holds_as_many_requests_as_request_log_size_says`）：
  - `the_capture_limits_come_from_the_options_or_the_variables`：三轮（参数、环境变量、都没有）：参数 `--capture-size 2m --capture-body-size 3k`、环境变量 `RURGE_CAPTURE_SIZE=2M` 与 `RURGE_CAPTURE_BODY_SIZE=3K` 都让 `GET /v1/rurge/capture` 的 `limits` 是 `{"total":2097152,"body":3072}`；都没有是 `{"total":104857600,"body":5242880}`。
  - `a_body_size_over_the_capture_size_is_lowered_with_a_warning`：`--capture-size 1k --capture-body-size 2k`：读到 `api on http://` 那一行之前的行里恰好有一行含 WARN 那一句（2048、1024）；`limits` 是 `{1024, 1024}`。`--capture-size 1k` 单独：`limits` 是 `{1024, 1024}`，`api on http://` 之前的行里没有 `capture body size`。日志与 `api on` 都写在守护进程的标准输出上（`init_logging` 的层写标准输出，`daemon.lines` 读的就是它），WARN 在起 API 之前写，所以读到 `api on` 时它要么已经读到、要么不会有。`wait_for_line` 丢掉它跳过的行：这一条自己从 `daemon.lines` 有时限地读到 `api on http://`，留着之前的行。
  - `bad_capture_sizes_are_refused`：`rurge run -c <conf> --capture-size 12x` 与 `--capture-body-size 17179869184g` 各以 clap 写错参数的退出码退出（照任务 0 找到的现有写法断言），标准错误里有原文 P13 的那一句。
  - `a_capture_switched_on_over_the_api_is_exported_as_har`：回环上的 `TestServer` 给 `/hello` 答 `hi from target`；守护进程带 `--capture-size 1m`；`POST /v1/features/capture {"enabled":true}` 答 200；经 HTTP 监听器问 `http://127.0.0.1:{端口}/hello`；有时限地（五秒）轮询 `GET /v1/rurge/capture/sessions` 到 `default` 有一条；`GET /v1/rurge/capture/sessions/default/har` 用 `serde_json` 解析：`log.version == "1.2"`、一条、`request.url` 以 `/hello` 结尾、`response.status == 200`、`response.content.text == "hi from target"`。

命令行的用例提交前各自单独循环跑二十次（经 `--test cli -- <名字>`）。

**实现**：照 P13。

**判据**

- 上面的用例都在、都绿；`cli` 的现有用例照过。
- `rurge run --help` 里有两个参数与原文 P13 的帮助（读帮助输出核一次，写进报告）。
- 门禁绿。

---

## Task 8：端到端的后一半与量内存

**改哪里**：

- `crates/rurge-engine/tests/capture.rs`：用例（夹具照 P14 从 `tests/rewrite.rs` 照抄）。
- `crates/rurge-http/src/capture/memory.rs`（新，只在 `#[cfg(test)]` 下）与 `capture/mod.rs` 末尾的 `#[cfg(test)] mod memory;`：量内存的两条忽略的用例与读峰值工作集的函数（P15）。

**接口**

- 消费：前面各任务的全部；`CaptureStore`、`Capture::begin`、`Pending`（量内存的用例在 crate 里，用得上 `Pending`）。
- 产出：没有新的生产接口；`ENTRY_OVERHEAD` 可能照量到的数改（P15）。

**用例（先红）**

- `tests/capture.rs`（开关都由用例 `set_capture(true)` 打开，除非写明）：
  - `a_request_a_reject_rule_refuses_is_captured`：`DOMAIN,example.test,REJECT`：条目 `Rejected(Reject)`、应答的状态码是记录的 `statusCode`（有拒绝页时 403，照这一代的 `show-error-page-for-reject`）、`remote` 是 `None`。
  - `a_listener_password_is_not_captured`：`http-listen` 带 `password@`（照 `tests/pipeline.rs` 的写法），客户端以 Basic 口令问：请求照常到源站；抓到的请求头里没有 `proxy-authorization`。
  - `an_upstream_proxy_credential_is_not_captured`：规则选一个带用户名与口令的 HTTP 代理（`P = http, 127.0.0.1, {端口}, user, pass`，以绝对形式转发，照 `RecordingProxy` 的写法）：代理收到了 `proxy-authorization`；抓到的请求头里没有。
  - `a_decrypted_request_is_captured_as_https`：`[MITM]` 解密的 HTTPS（`TlsFixture`，两张 CA 不同名，照 `tests/rewrite.rs` 的 `harness_mitm` 与 `decrypted_get`）：条目的 `url` 以 `https://` 开头、应答正文是源站的正文。
  - `entries_outlive_a_reload`：存进一条、重载一次 Profile（照任务 0 找到的写法）：条目还在、开关仍开着；重载之后的请求照样存进去。
  - `captured_content_never_reaches_the_log`：`LogCapture` 接最详细的一级（当前线程运行时），一个请求的头 `x-marker: rurge-capture-marker-1` 与正文 `rurge-capture-marker-2` 经引擎被抓：条目里有这两样；抓到的日志里没有一行含 `rurge-capture-marker`。红了而写出它的那一行日志不是 M4a 加的：停下报告，不改那一行、不放宽断言。
- `capture/memory.rs`（`#[ignore]`，各自单独跑，P15）：
  - `a_full_store_at_the_default_limits`；
  - `an_empty_store_is_the_baseline`；
  - 读峰值的解析照抄 `peak.rs` 的那条用例（不忽略，缺省就跑）。

带等待的用例提交前各自单独循环跑二十次。

**实现**

- 端到端：照 P14。
- 量内存：照 P15；两条各跑一次，命令与输出原样写进修正记录与报告（写明是调试构建、一台机器上的一次运行）；照 P15 判断要不要改 `ENTRY_OVERHEAD`，改了就重跑那一条一次、两次的数都记下。

**判据**

- 上面的用例都在、都绿；量内存的两条在缺省的 `cargo test` 里是"ignored"。
- 量到的数、比值与对 `ENTRY_OVERHEAD` 的决定在修正记录里。
- 门禁绿。

---

## Task 9a：手工清单、API 与测试的文档

**改哪里**：`docs/acceptance/phase4-manual.md`、`docs/api/phase4.md`、`docs/testing.md`。

**做什么**

- `docs/acceptance/phase4-manual.md`：照文件里已有条目的格式加 M4a 的条目（设计第 13 节）：
  - 真实浏览器的流量：开着抓包浏览几个站（明文与 `[MITM]` 解密的），`/v1/rurge/capture/sessions` 与一条的读出合乎实际；
  - 导出的 HAR 能被 Chrome 与 Firefox 的开发者工具导入，耗时的瀑布图合乎实际；
  - `always-capture` 在开关关着时把命中的请求存进那个名字的会话；
  - 截断与挤掉在真实的大下载上（例如 `--capture-size 20m` 下载几十 MB 的文件）：客户端完整收到，条目截断，更早的条目被挤掉；
  - 清除与开关的持久化（重启之后还原）。
  - 文件开头的项数随之改。
- `docs/api/phase4.md`：「M4a　抓包」一节：`capture` 开关；六个端点的形状、错误与例子（照 P11、P12）；HAR 的映射与 `timings` 的算法；两个上限与命令行（写法、缺省、降低与 WARN）；量到的内存（任务 8 的数，写明是调试构建、一台机器上的一次运行）；两行日志；登记的几件（设计第 11 节第 1 ～ 5、9、10 条，本计划 P5 的 `error`，P8 的 REJECT-DROP，P9 ④ 的重发）。
- `docs/testing.md`：阶段 4 的一节加一条：量抓包内存的做法（P15：调试构建、`rurge-http` 的库内用例、怎样单独跑两条、峰值怎样读），与 `crates/rurge-rules/tests/memory.rs` 的不同写明；另一条：抓包的用例用小的上限、以有时限的轮询等条目存进去。

**判据**：三份文档改了；`docs/api/phase4.md` 里的数与修正记录一致；不写"通过"。

---

## Task 9b：M4a 的收尾文档

**改哪里与做什么**（P17）

- `docs/surge-compatibility-matrix.md`：L271（`always-capture`：M4a 起生效；备注写 M4-D8：只作用于本来就经 HTTP 引擎的请求、不让连接进引擎；规则集文件里的行上的不读）、L569（抓包视图：API 与 HAR 有了，Dashboard 在阶段 6；备注写设计第 11 节第 3、9、10 条）、L863（`rurge run` 的两个参数）、L871（`capture` 生效）；开头"最近一次"的括号。行号以任务 0 那时为准。
- `docs/status.md`：阶段 4 下新的一节「M4a」：做了什么、没做的与不要当成已经有的（M4b 的事、`br` / `zstd`、Dashboard、手工验收）、以后的会话必须知道的事（P3 的锁的次序、P8 的状态机与锁的次序、P9 里不许握着槽的锁 poll 正文、REJECT-DROP 的认法、`testing` 特性、量到的内存）、去向写着项目所有者的事（设计第 17 节）；细节指到计划的两张表。
- `README.md` 与 `README_en.md`：状态一段与特性表的 HTTP 处理一行（抓包：M4a 做完，本地提交，CI 没有见过）；两份内容一致。
- `docs/requirements.md` 第 7 节阶段 4 的状态一句：加 M4a。
- `CLAUDE.md`：只改「现在在哪里」：M4a 做完、M4b 的计划在下一步写。
- 设计第 18 节：执行期改了设计正文的意思的，接着写（没有就不写）。
- `docs/README.md`：本计划那一项末尾加"做完了"的一句（同 M3b 那一项的写法）。

**判据**：上面的文档都改了；README 两份一致；`CLAUDE.md` 只动了「现在在哪里」；不写"通过"，不写"齐备"。

---

## 计划期决定

写计划时对着 `a6e78ad` 核对过下面引的名字与行号。行号会随前面任务的改动漂移，以名字为准；名字不同就照实改、记进修正记录，不改语义。标了"订正设计"的，已随本计划写进设计第 18 节。

| # | 事项 | 决定 |
| - | ---- | ---- |
| P1 | 任务的拆法（订正设计） | 设计第 2 节的 M4a 任务表拆成任务 0 ～ 8、9a、9b，次序与设计有两处不同：<br>• API 的两个任务提前：设计的任务 6（端点）、7（HAR）在这里是任务 5、6，设计的任务 5（可执行文件）在这里是任务 7——可执行文件的用例要经 API 读上限、取 HAR；设计第 12 节第 3 条那一条命令行的端到端用例随它在任务 7。<br>• 设计的任务 9 分成 9a（手工清单、`docs/api/phase4.md`、`docs/testing.md`）与 9b（其余的收尾文档）：一个 Pi 任务写不完全部的收尾文档（同 M3b 的任务 8、9）。<br>设计第 12 节第 3 条的端到端用例分在任务 4（开关、`always-capture`、`FINAL`、逻辑规则、直连模式、关掉之后、`state.json`）与任务 8（拒绝、两种凭据、解密的、重载、日志），P14 |
| P2 | 依赖（订正设计 M4-D2） | `rurge-api` 正式依赖只加工作区的 `base64`（任务 5）与 `futures-util`（任务 6），**不加 `time`**：HAR 的 `startedDateTime` 由 `rurge-http` 的 `Entry::started_rfc3339()` 写（`rurge-http` 本来就依赖 `time`；用 `OffsetDateTime::from(SystemTime)` 与它的取值方法拼成 `YYYY-MM-DDTHH:MM:SS.mmmZ`，不开 `formatting`）。<br>`rurge-http` 加 `[features]` 一节 `testing = []`，注释照 `rurge-net` 那一行的写法写"`CaptureStore::put` for the tests of other crates, which store entries they made; enabled by dependants' dev-dependencies."；`rurge-api` 的开发依赖带上它。生产代码不开它：`put` 不在任何生产路径上。<br>`futures-util` 只用 `stream::iter`：`document` 交给 `axum::body::Body::from_stream` 一条 `Stream<Item = Result<Vec<u8>, Infallible>>`。<br>锁文件只有 `rurge-api` 的依赖列表变（特性不进锁文件），包数 520 |
| P3 | `when_finished` 与抓包会话的槽 | **`when_finished(f)`**：先锁 `on_finish`，在这把锁里读一次 `outcome`（它自己的锁，读完就放）：是 `Some(o)` → 放掉 `on_finish` 的锁，再 `f(self, &o)`；是 `None` → 推进 `on_finish`、放锁。`finish` 不改：它先置 `finished`、再写 `outcome`（写完放锁）、最后在 `on_finish` 的锁里取走全部钩子。所以 `when_finished` 读到 `None` 时 `finish` 还没有取钩子，推进去的会被它取到；读到 `Some` 时当场调，`finish` 取走的钩子里没有它；不漏、不调两次。`finish` 从不同时握两把锁，`when_finished` 的次序是 `on_finish` → `outcome`，不会死锁。`f` 在任何锁之外调：`f` 里再调 `when_finished` 或 `on_finish` 不死锁。文档注释写这段道理（同 `on_first_byte` 的写法）。<br>**`capture_session`**：`OnceLock<Arc<str>>`，`set_capture_session` 只有第一次的算（同 `method`、`status`）。文档注释："The `always-capture` session the matched rule names (phase 4 M4 design 5.3). A session that is no HTTP request may have one too; nobody reads it then." |
| P4 | 空的 `always-capture=` | 在 `config.rs` 读 `[Rule]` 的循环里、给 `W0003` 的两段之后：`r.params.always_capture` 是 `Some(v)` 而 `v.trim()` 是空的 → 一条 `Diagnostic::warning(codes::W_INVALID_VALUE, 原文 P4)`，`.at(e.span.clone())`；规则照常进 `rules`。`RuleParams` 不改，引擎读它时去掉空白、空的不填（P10）。不设新码；不带 `=` 的照旧 `W0003`；规则集文件里的行上的参数不读、不告警（设计第 5.2 节） |
| P5 | `capture` 模块的类型（订正设计） | 设计第 3 节的接口改三处：<br>① `Outcome::Failed` 不带文字，`Entry` 多 `pub error: Option<String>`：记录的 `error` 是 `set_error` 写的说明、没有时是失败的消息（`observe.rs` 的 `record_of`），被拒绝的请求也可能有（`engine.rs` 里 `set_error(why)` 那几处），所以条目照它取，与记录同一句；<br>② `Body::decoded` 交回 `Result<Option<Bytes>, NotDecoded>`：`Ok(None)` 是没有要解的编码；<br>③ 多 `Status`、`SessionSummary`、`NotDecoded`、`ENTRY_OVERHEAD`、`Body::empty`、`Entry::started_ms` / `started_rfc3339`、`Capture::begin` 与 crate 里的 `Pending`；`put` 只在 `test` 或 `testing` 下。<br>**派生**：`Limits`、`Status`、`Truncated`、`Outcome` 派生 `Clone, Copy, Debug, PartialEq, Eq`；`Timings`、`NotDecoded`、`SessionSummary` 派生 `Clone, Debug, PartialEq, Eq`；`Capture` 只派生 `Clone`；`Entry`、`Head`、`ResponseHead`、`Body`、`CaptureStore`、`Pending` 什么也不派生——它们与 `Capture` 都**不实现 `Debug`**（设计第 6.6 节）。<br>**`started_ms`**：`started.duration_since(UNIX_EPOCH)` 的毫秒，早于纪元的是 0。**`started_rfc3339`**：`OffsetDateTime::from(started)`（UTC），`format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z", ..)`，月份写数字 |
| P6 | 记账、挤掉、截断与丢掉 | **存储**：`limits` 与一把锁里的 `queue: VecDeque<Kept { entry: Arc<Entry>, cost: u64 }>`（由旧到新）、`kept`（队列里的账之和）、`reserved`（进行中的预留之和）与 `stored`、`evicted`、`dropped`、`truncated`。`used = kept + reserved`。<br>**一条的账** `cost(e)`：请求头与应答头每一行 `名字.len() + 值.len() + 32`（没有应答时 0）+ `url.len()` + 方法的长度 + 规则、每个策略、每条备注、`error` 的字节 + 两个正文 `data.len()` + `ENTRY_OVERHEAD`。<br>**`reserve(n)`**：`reserved + n > total` → 假，什么也不挤（队列空了也放不下）；否则从队头一条一条挤掉（`kept` 减它的账、`evicted` 加一）直到 `kept + reserved + n <= total`，`reserved += n`，真。<br>**`insert(entry, held)`**（`held` 是这一条已有的预留）：`reserved -= held`；`reserved + cost > total` → `dropped` 加一、假；否则同样从队头挤到放得下，推到队尾，`kept += cost`、`stored` 加一，两个正文有一个截断就 `truncated` 加一，真。<br>**`Pending`**：两个正文各一份 `{ chunks: Vec<Bytes>, stored, size, truncated }` 与 `held`。一块 `data` 进来：空的不理；`size += len`；已经截断的到此为止；`n = min(limits.body − stored, len)`；`n == 0` → `Limit`；`reserve(n)` 是假 → `Budget`；否则 `chunks.push(Bytes::copy_from_slice(&data[..n]))`（**抄，不切**：切出来的 `Bytes` 握着 hyper 整块的缓冲，账就不对了）、`stored += n`、`held += n`，`n < len` → `Limit`。<br>**`commit(make)`**：两个正文接成一份（只有一块的就是它；多块的接进一块恰好那么大的内存），交给 `make` 造条目，`insert(entry, held)`，然后 `held = 0`。**`Drop`**：`held > 0` 时锁住、`reserved -= held`。<br>**`put(entry)`** 是 `insert(entry, 0)`。<br>**读**：`sessions` 遍历队列按名字合计（`BTreeMap`）；`session`、`entry` 遍历队列（`entry` 按 `id` 找）；`clear` 去掉相应的、`kept` 减它们的账，四个数不动。锁只握到拿完 `Arc` |
| P7 | 读出时解开（`decode` 拆开） | `rewrite/body.rs` 拆出两步，`decode` 改成调它们、消息逐字不变：<br>• `layers(encodings)`：照今天的读法交回各项的名字（字段的次序，去掉空的与 `identity`）；某一行不是可见的 ASCII → `Err(Undo::Unsupported(None))`。<br>• `undo(layers, body, limit)`：从最后一个往前逐层解，每层的做法照今天（空的 `gzip` / `x-gzip` / `deflate` 一层解成空的、`take(limit + 1)`）；不认识的名字：是 token → `Unsupported(Some(名字))`、否则 `Unsupported(None)`；解不开 → `Corrupt(名字)`；超过 → `Over`。<br>• `decode` = `undo(&layers(e)?, body, limit.bytes)`，`Undo` 照今天的四句转成文字（`Over` 用 `limit.text`）。<br>**`Body::decoded(headers, limit)`**：取 `headers` 里全部的 `content-encoding` 行 → `layers`（`Err` 照转）；没有名字 → `Ok(None)`（截断的也是）；截断的 → `Err(Truncated)`；`undo(.., &self.data, limit)`：`Ok(b)` → `Ok(Some(b))`，`Unsupported(x)` → `Unsupported(x)`，`Corrupt(n)` → `Corrupt(n)`，`Over` → `TooLarge(limit)`。`NotDecoded` 的 `Display` 照抄原文 P7 |
| P8 | `CaptureSlot`（订正设计：REJECT-DROP 的认法） | **状态**：`Arc<Mutex<Armed>>`，`Armed` 有 `handle: Arc<SessionHandle>`、`session`、`started: SystemTime`（`SystemTime::now()` 减去句柄的 `elapsed()`，减不了就是现在）、`pending: Option<Pending>`（存进去之后是 `None`）、`request: Option<Head>`（最后一次取样）、`reused`、`upgrading`、`finished: Option<(Duration, SessionOutcome)>`、`answer: Waiting \| Nothing \| Given { head: ResponseHead, done: Option<Duration> }`。<br>**`arm(capture, handle, head)`**：`capture` 是 `None` 或已经 arm 过 → 什么也不做（`head` 不调）。否则建 `Armed`（`pending = Some(capture.begin())`、`request = Some(head())`），放进 `OnceLock`，**然后在任何锁之外** `handle.when_finished(回调)`——句柄可能已经结束（`settle`），那时回调就在这一句里跑。<br>**回调(h, outcome)**：锁住；`finished = Some((h.elapsed(), outcome.clone()))`；结局是 `Rejected(Drop)` 而 `answer` 还是 `Waiting` → `answer = Nothing`：REJECT-DROP 先结束记录、再停留，停留完才 `Err(Close)`，引擎里只有它这样结束而后没有应答（订正设计：第 6.4 节写"在停留之前告诉 `CaptureSlot`"，这里改成由回调认出，`answer` 一行不改）；`try_commit`。<br>**`leaving(head, reused)`**、**`upgrading()`**：锁住，写进去。**`RequestTap::chunk(data)`**：锁住，`pending` 还在就 `request_chunk(data)`。<br>**`answered(self, result, h2)`**：没有 arm → 原样交回。`Err(Close)` → 锁住，`Waiting` 改 `Nothing`，`try_commit`，交回 `Err(Close)`。`Ok(resp)` → 锁住；`pending` 已经没有了 → 原样交回；`head = ResponseHead { status, version: if h2 { HTTP_2 } else { resp.version() }, headers: resp.headers().clone() }`（M4-D28）；`upgrading` → 当场存进去：应答是这个头、应答正文空、结局 `Upgraded`、结束的那一刻是此刻的 `handle.elapsed()`，交回原样的 `resp`（不包）；否则 `answer = Given { head, done: None }`，放锁，交回 `resp.map(\|b\| CaptureBody { inner: b, tap: ResponseTap { .. } }.boxed())`。<br>**`Drop for CaptureSlot`**：arm 了而 `answer` 还是 `Waiting` → `Nothing`，`try_commit`（`handle` 的 future 被丢下：HTTP/2 上客户端在应答之前重置了流）。<br>**`try_commit`**：`pending` 在，`finished` 是 `Some((at, outcome))`，而且 `answer` 是 `Nothing`（应答 `None`、结束 = `at`）或 `Given { head, done: Some(d) }`（应答是 `head`、结束 = `max(at, d)`）→ 存进去，结局 `Completed` / `Rejected(k)` / `Failed` 照 `outcome`。<br>**存进去**：`pending.take().commit(\|请求正文, 应答正文\| Entry { .. })`：`id`、`session`、`started`、`request`（`take`）；`timings`：`connect = handle.connect_time()`、`wait = handle.first_byte_time()`、`dns = handle.trail().dns()`、两者都有时 `receive = 结束.saturating_sub(connect + wait)`、`total = 结束`、`reused`；`remote = handle.trail().peer()`；`error = handle.error()`、没有时是 `Failed(m)` 的 `m`；`rule`、`policy_chain`、`notes` 从句柄读。<br>**`CaptureBody { inner, tap }`**（字段的次序就是丢下的次序：先丢下 `inner`——`Tail`、`LocalFile` 与守卫在那时结束记录——再丢下 `tap`；`CaptureBody` 自己**不**实现 `Drop`）：`poll_frame` 先 poll `inner`（不握锁）：数据帧 → `tap.chunk(data)`，之后 `inner.is_end_stream()` 为真 → `tap.done()`；`None` 或错误 → `tap.done()`；帧与错误原样交出。`is_end_stream`、`size_hint` 照 `inner`。`ResponseTap::chunk` 锁住、`response_chunk`；`done()` 只做一次：锁住，`Given { done: None }` 改成 `Some(handle.elapsed())`，`try_commit`；`Drop for ResponseTap` 调 `done()`。<br>**锁**：次序是 `Armed` → 存储；握着 `Armed` 的锁时不 poll 正文、不调 `finish`、不调 `when_finished`，所以回调（在 `finish` 里跑）与 `done`（在 poll 与 `Drop` 里跑）都不会重入。<br>**环**：回调握着 `Arc<Mutex<Armed>>`，`Armed` 握着 `Arc<SessionHandle>`，句柄的 `on_finish` 里握着回调：句柄结束之前这是一个环。句柄结束时 `finish` 取走并调掉钩子，环就断了；引擎里每个请求的句柄都会结束（`Guard` 的 `Drop`、`settle`、`answer`），所以回调握强引用、不用 `Weak`——用 `Weak` 的话，槽与应答正文都先丢下、句柄后结束时条目就丢了。任务 3 的用例 8、9 的 `used == 0` 钉住它不漏 |
| P9 | 引擎里的位置 | ① 今天的 `Requests::handle` 的函数体改名为私有的 `serve(&self, req, slot: &CaptureSlot)`；新的 `pub async fn handle(&self, req)`：`let slot = CaptureSlot::new(); let answer = self.serve(req, &slot).await; slot.answered(answer, self.shared.conn.h2)`。签名不变；`serve` 里早退的地方（HTTP/2 的 `CONNECT`、监听器的 400）没有 arm，原样交回。<br>② 第 3 步 `route(..).await` 之后、`token` 的检查之前：取 `routed` 两个变体里 `capture` 的克隆，`slot.arm(capture, &handle, \|\| Head { method, url: captured_url(handle.session().url.as_deref(), &req), version: req.version(), headers })`，`headers` 是 `req.headers()` 的克隆再 `headers::strip(&mut h, Keep { upgrade: wants_upgrade(&req), proxy_authorization: sniffed })`（`sniffed` 照 `self.shared.conn.entry == Entry::Sniffed`，挪到这里先算）。同一处记下 `let client_version = req.version();`。解出 `Routed` 的两处 `match` 加上 `..`。<br>③ `fn captured_url<B>(url: Option<&str>, req: &Request<B>) -> String`：`url` 有就是它；没有（`CONNECT`）是 `req.uri().authority()` 的文字，再没有是空串（M4-D26）。<br>④ `let mut reused = false;`，`borrowed_h2` 是 `Some` 的那一支与 `pool.take` 交回 `Some` 的那一支各置真；`redial` 那一支照旧假。借来的连接没有接下请求、在新拨的连接上再发一次的（`resend`）不改 `reused`：`connect_time` 也是借的那一刻（`connected` 是 `OnceLock`，只有第一次的算），两者一致；任务 9a 登记。<br>⑤ **第 1 个取样点**：第 5 步 `headers::strip(req.headers_mut(), Keep { upgrade, proxy_authorization: sniffed && !retargeted })` 之后、`if !sniffed {` 之前：`slot.armed()` 时 `slot.leaving(Head { method: req.method().clone(), url: captured_url(url.as_deref(), &req), version: client_version, headers: req.headers().clone() }, reused)`。`absolute_form` 加的 `Proxy-Authorization`、补的 `Transfer-Encoding`、`fit` 与 `write_host` 都在它之后。<br>⑥ `Idle::new` 多一个参数 `tap: Option<RequestTap>`（调用处给 `slot.request_tap()`），存成字段；`poll_frame` 里数据帧计数的那一句旁边 `if let Some(tap) = &this.tap { tap.chunk(data) }`；trailer 不抄。<br>⑦ 升级那一支 `return Ok(self.promote(..))` 之前 `slot.upgrading();`。<br>⑧ 别处一行不改：`answer`、`failed`、`page`、`map_local`、`timed_out`、`Tail`、`LocalFile`、`Guard`。<br>⑨ `engine.rs` 里连接的入口也叫 `Entry`、`http_body::Body` 也叫 `Body`：抓包的类型一律写 `capture::Entry`、`capture::Body`，不 `use` 进来 |
| P10 | `rurge-engine` | **存储**：`EngineShared.capture: Arc<CaptureStore>`，文档注释"The capture store (phase 4 M4 design 4.1): one for the engine's lifetime, kept across reloads; `rurge run` replaces it with one of the limits its options give."；`new` 给 `CaptureStore::new(Limits::default())`。<br>**开关**：`Engine` 多 `capture_on: AtomicBool`（与 `rewrite_on` 并排，初值 `false`：缺省关，设计第 5.1 节）；`capture_on()`；`set_capture(on)` 照 `set_rewrite`，写 `s.features.capture = Some(on)`；`restore_switches` 加 `features.capture`；`capture()` 交回 `self.shared.capture.clone()`。文档注释照 `rewrite_on` 的写法。<br>**`always_capture(rules, index)`**：在 `rule_raw` 旁边，同样二分查找；`params.always_capture` 去掉首尾空白，空的是 `None`，否则 `Arc::from`。<br>**`choose_policy_with`**：`handle.set_rule(rule_raw(..))` 那一句之后，`always_capture(&rt.rules, i)` 是 `Some(name)` → `handle.set_capture_session(name)`。早退的三处（`external`、直连、全局代理）不填：没有命中的那一行（设计第 5.2 节）。<br>**`capture_for(handle)`**：`handle.capture_session()`，没有而 `capture_on()` 为真时是 `DEFAULT_SESSION`，都没有是 `None`；`store` 是 `self.shared.capture.clone()`。<br>**`ConnUpstreams::route`**：两个分支都 `capture: self.engine.capture_for(handle)`；`Refused` 那一支在 `settle` 之后取（REJECT 一族照样带着，设计第 5.3 节）。<br>**`lib.rs`**：`pub use rurge_http::capture;`，注释照 `ApiTls` 那一行写"for `rurge-api`" |
| P11 | API 的五个端点与开关 | **路由**（`lib.rs`，放在 `/v1/rurge/http` 之后）：`/v1/rurge/capture`（`GET`）、`/v1/rurge/capture/sessions`（`GET`）、`/v1/rurge/capture/sessions/{name}`（`GET`）、`/v1/rurge/capture/entries/{id}`（`GET`）、`/v1/rurge/capture/clear`（`POST`）；任务 6 加 `/v1/rurge/capture/sessions/{name}/har`。鉴权与错误体照阶段 1。<br>**`/v1/features/capture`**：`get_feature` 给 `app.engine.capture_on()`；`set_feature` 调 `set_capture(..).await`、一行 INFO（原文 P11，字段 `enabled`）。<br>**`GET /v1/rurge/capture`**：设计第 7.1 节的形状，取自 `status()`。<br>**`GET …/sessions`**：设计第 7.2 节，取自 `sessions()`。<br>**`GET …/sessions/{name}`**：`Path<String>`（axum 已解开百分号）；`limit` 照 `requests.rs` 的 `recent`（缺省 `DEFAULT_LIMIT`、`0` 答 400）；`session(name)` 是空的 → 404；由新到旧取 `limit` 条。摘要的键照设计第 7.3 节：`id`、`startedMs`、`method`、`url`、`status`（没有应答 `null`）、`outcome`（`completed` / `upgraded` / `rejected` / `failed`）、`requestBodySize`、`responseBodySize`（`size`）、`truncated`（两个正文有一个截断）、`totalMs`。<br>**`GET …/entries/{id}`**：`Path<String>` 读成 `u64`，读不了 → 400；`entry(id)` 没有 → 404。键照设计第 7.5 节的例子：`id`、`session`、`startedMs`、`request {method, url, version, headers, body}`、`response`（`null` 或 `{status, version, headers, body}`）、`timings {connectMs, dnsMs, waitMs, receiveMs, totalMs, reused}`（`as_millis()` 向下取整，没有是 `null`）、`remoteAddr`（`SocketAddr` 的文字）、`outcome`、`rejectKind`（`RejectKind::name()`，不是拒绝的 `null`）、`error`、`rule`、`policy`、`notes`。`version` 是 `format!("{:?}", v)`；`headers` 是 `[{name, value}]`，照 `HeaderMap::iter` 的次序，值 `String::from_utf8_lossy`，名字（小写）是 `authorization`、`proxy-authorization`、`cookie`、`set-cookie` 的值写 `***`；`body` 是 `{size, stored, truncated, base64}`（`truncated` 是 `null` / `"limit"` / `"budget"`，`base64` 是 `STANDARD` 编码的 `data`）。<br>**`?decode=`**：只认 `true` 与 `false`（缺省 `false`），别的 → 400。`true` 时两个 `body` 各多 `decoded` 与 `notDecoded`：`body.decoded(这一侧的头, limits.body)` 是 `Ok(Some(b))` → `decoded: true`、`base64` 改成 `b` 的、`notDecoded: null`；`Ok(None)` → `false`、`null`；`Err(r)` → `false`、`r.to_string()`。没有应答的 `response` 照旧 `null`。<br>**`POST …/clear`**：请求体读成 `Result<Bytes, BytesRejection>`（照 `profiles.rs` 的 `requested_name`）：读不了 → 400；全是空白 → 全部；是 JSON 对象 → `session` 没有是全部、是字符串是那个会话、别的 → 400；不是对象 → 400；消息都是原文 P11 的 `BAD_BODY`。应答 `{"cleared":N}`。<br>**`features.rs`** 的模块注释加上"from phase 4 / M4a `capture`" |
| P12 | HAR | **`timings(t)`**：`reach = t.connect.unwrap_or(t.total)`；`d = t.dns.map(\|x\| x.min(reach))`；`dd = d.unwrap_or(0)`；各项（`ms(x)` 是 `x.as_millis() as i64`）：<br>• `blocked`：`reused` 而 `connect` 是 `Some(c)` → `ms(c − dd)`，否则 `-1`；<br>• `dns`：`d` 的 `ms`，没有是 `-1`；<br>• `connect`：不 `reused` 而 `connect` 是 `Some(c)` → `ms(c − dd)`，否则 `-1`；<br>• `ssl`：`-1`；`send`：`0`；<br>• `wait`：`connect` 与 `wait` 都有 → `ms(wait)`；只有 `connect`（`c`）→ `ms(total − c)`；没有 `connect` → `ms(total − dd)`；<br>• `receive`：`ms(receive)`，没有是 `0`；<br>• `time`：不是 `-1` 的各项之和。减法都用 `saturating_sub`。<br>**一条**（`entry(e, body_limit)`）照设计第 8.2 节的表，补全：`request` 是 `{method, url, httpVersion, cookies: [], headers, queryString, headersSize: -1, bodySize, postData?}`；`queryString`：方法是 `CONNECT` 的 `[]`，否则 `url::Url::parse(url)` 的 `query_pairs()`，读不了的 `[]`；`postData` 只在 `request_body.size > 0` 时有：`{mimeType, text}`，正文没有截断、请求没有 `content-encoding`、是 UTF-8 时 `text` 照写，否则 `text` 是 Base64 并加 `"_encoding":"base64"`，截断的加 `comment`。`response` 是 `{status, statusText, httpVersion, cookies: [], headers, content, redirectURL, headersSize: -1, bodySize}`；`content`：`decoded(应答的头, body_limit)` 是 `Ok(Some(b))` → `size = b.len()`、`compression = size − bodySize`（可以是负的）、文字是 `b`；`Ok(None)` → `size = data.len()`、文字是 `data`；`Err(r)` → `size = data.len()`、文字是 `data`、`comment` 加 `not decoded: {r}`；文字是 UTF-8 照写，否则 Base64 加 `"encoding":"base64"`；截断的 `comment` 加 `{stored} of {size} bytes stored`（截断在前，两句以 `"; "` 接）；`mimeType` 取 `content-type`（没有是空串）。没有应答：`status 0`、`statusText ""`、`httpVersion ""`、`headers []`、`content {"size":0,"mimeType":""}`、`redirectURL ""`、`bodySize -1`。另有 `startedDateTime`、`time`、`cache: {}`、`timings`、`serverIPAddress`（`remote` 的 IP，没有时不写这个键）、`_rurgeId`、`_session`、`_outcome`、`_rejectKind`（没有时不写）、`_error`（没有时不写）、`_notes`。头与遮住同 P11。<br>**`document(entries, body_limit)`**：一条流：先 `{"log":{"version":"1.2","creator":{"name":"rurge","version":"<CARGO_PKG_VERSION>"},"pages":[],"entries":[`，再每条一块（第二条起前面带 `,`，在 poll 到它时才序列化），最后 `]}}`；`Body::from_stream(futures_util::stream::iter(..))`。处理函数先取 `session(name)` 的快照（空的 404），交 `([(CONTENT_TYPE, "application/json")], document(..))` |
| P13 | 命令行 | **参数**（`RunArgs`，紧跟 `request_log_size`）：`capture_size: Option<u64>`（`#[arg(long, env = "RURGE_CAPTURE_SIZE", value_name = "SIZE", value_parser = parse_size)]`）与 `capture_body_size`（`RURGE_CAPTURE_BODY_SIZE`），帮助照抄原文 P13。<br>**`parse_size(s)`**：最后一个字是 `k`/`m`/`g`（ASCII 不分大小写）时去掉它、乘数是 1024、1024²、1024³，否则乘数 1；余下必须是一个以上的 ASCII 十进制数字（没有符号、空白、小数点、前缀）；数字读不成 `u64` 或乘积溢出 → "too large"；别的 → "invalid size"（原文 P13，反引号里是 `s` 的原样）。<br>**`capture_limits(size, body)`**：`total = size.unwrap_or(DEFAULT_TOTAL)`；`Limits::new(total, body.unwrap_or(DEFAULT_BODY))`；`body` 是 `Some(b)` 且 `b > total` 时第二项是 WARN 那一句。<br>**启动**：`EngineShared::new(..)` 之后、`empty_group` 那几句旁边：`let (limits, lowered) = capture_limits(args.capture_size, args.capture_body_size); if let Some(w) = lowered { tracing::warn!("{w}"); } shared.capture = CaptureStore::new(limits);`。只有 `rurge run` 有这两个参数。WARN 与别的启动告警一样写在标准输出上、排在 `api on` 之前（没有告警时 `api on` 是第三行，见 `run.rs` 起 API 那一段的注释） |
| P14 | 端到端的做法（订正设计：用例的文件） | 设计第 12 节第 3 条写的是 `tests/http.rs` 与 `tests/mitm.rs`；本计划改成新文件 `crates/rurge-engine/tests/capture.rs`（`mod common;`）：那两个文件已经很长，抓包的用例集中在一处。要的夹具（`RecordingProxy`、`harness_mitm`、`decrypted_get`、`mitm_section`）从 `tests/rewrite.rs` 照抄，注释写它从哪里来、为什么不挪进 `common`（别的文件也在用它们的原样）。<br>• Profile 照 `common::Profile`；示例主机 `example.test` 经 `[Host]` 钉到 `127.0.0.1`；源站是 `TestServer`，URL 带它的端口；经 HTTP 监听器以 `plain_get` 问。<br>• 存进去用 `wait_until`（`common` 的，有时限）等 `h.engine.capture().session(..)` 不空。<br>• "什么也不存"的断言：记录结束之后固定等待 200 ms，注释写它等的是什么、余量多大。<br>• 解密：`TlsFixture`，两张 CA 不同名（同名的 CA 会让握手报 `BadSignature`）。<br>• 带口令的监听器、带凭据的上游代理照 `tests/pipeline.rs` 与 `RecordingProxy` 的写法；凭据用用例里的常量，不用真的口令 |
| P15 | 量内存（订正设计） | 设计第 4.5 节写"各自单独以 release 运行"；本计划改成**调试构建、`rurge-http` 的库内用例**：`--release` 要把整个工作区另编一份（`target/` 的盘位，见「门禁与这台开发机」）；条目、`HeaderMap`、`Bytes` 在两种构建里分配的大小相同，量的是装满与基线两次运行的差。库内的用例用得上 `Pending`，走的是真实的按块记账。<br>**文件**：`crates/rurge-http/src/capture/memory.rs`，`capture/mod.rs` 末尾 `#[cfg(test)] mod memory;`；开头的模块注释照 `crates/rurge-rules/tests/memory.rs` 写这是量、不是门，与跑的命令。`peak_resident_bytes`、`parse_count`、`parse_vm_hwm` 与它们的用例从 `crates/rurge-tun/src/tests/bench/peak.rs` 照抄，注释写来处。<br>**`a_full_store_at_the_default_limits`**（`#[ignore]`）：`Limits::default()` 的存储；条目像真实的页面请求：请求头与应答头各 14 行（名字约 12 字节、值约 40 字节）、URL 约 80 字节、一条备注、请求正文空、应答正文 4 KiB（四块，每块 1 KiB）；每 1024 条里有一条 6 MiB 的应答正文（64 KiB 一块，截断在 5 MiB）。经 `Capture::begin`、`response_chunk`、`commit` 存，直到 `evicted` 超过已存条数的十分之一。打印 `status()` 的各个数与 `peak_resident_bytes()`。<br>**`an_empty_store_is_the_baseline`**（`#[ignore]`）：只建一份空的存储，打印峰值。<br>**跑法**：`cargo test -j 8 --workspace --lib -- capture::memory::<名字> --ignored --exact --nocapture`，两条各单独跑一次。<br>**判断**：多出的 = 装满的峰值 − 基线的峰值；比值 = 多出的 / `limits.total`。比值超过 1.25 时把 `ENTRY_OVERHEAD` 调到使记的账接近实际（新值 ≈ 512 + (多出的 − total) / 存着的条数，取整到 64 的倍数），重跑装满的那一条一次；不超过就不改。缺省的上限不因此改（M4-D7）。两次的数、比值与决定写进修正记录，任务 9a 写进 `docs/api/phase4.md` |
| P16 | 现有的用例 | 见下面的表。表里"预计照过"的若红，停下报告，不改期望 |
| P17 | 收尾文档的清单 | 见任务 9a、9b。兼容性清单登记设计第 11 节第 1 ～ 5、9、10 条与本计划的三处：条目的 `error` 与记录同一句（P5）；REJECT-DROP 的条目在记录结束时存进去（P8，同设计 M4-D29）；量内存在调试构建里（P15） |

### 表后的原文（照抄）

P4、P7、P11、P12、P13 要照抄的文字含反引号与引号，放在这里。每个代码块里的文字就是要的那一串，没有为 Markdown 加的转义；`{…}` 是要填的值。

**P4：`W0012` 的消息（`codes::W_INVALID_VALUE`）**

```text
`always-capture` needs a capture session name; the parameter is ignored
```

**P7：`NotDecoded` 的 `Display`**

```text
Truncated              the body was truncated
Unsupported(Some(名字)) content-encoding {名字} is not supported
Unsupported(None)      the content-encoding is not supported
Corrupt(名字)           the body cannot be decoded as {名字}
TooLarge(上限)          the decoded body is over {上限} bytes
```

**P11：API 的日志与错误**

```text
INFO（字段 enabled）
capture switched via http-api

404
unknown capture session `{名字}`
unknown capture entry {id}

400
entry id must be a number
decode must be true or false
limit must be at least 1
the body must be a JSON object; "session", when given, must be a string
```

**P12：HAR 的 `comment`**

```text
{存下的字节数} of {见过的字节数} bytes stored
not decoded: {NotDecoded 的 Display}
```

两句都有时以 `; ` 接起来，截断的那一句在前。

**P13：命令行**

```text
--capture-size 的帮助
Keep at most this much of the captured requests in memory: bytes, or a number with k, m or g (default 100m)

--capture-body-size 的帮助
Keep at most this much of each captured body (default 5m)

parse_size 的两种错误
invalid size `{写的}`: expected a number of bytes, optionally followed by k, m or g
size `{写的}` is too large

启动时的 WARN
the capture body size ({单条} bytes) is larger than the capture size; lowered to {总量} bytes
```

### P16 的表：要改的、要留意的现有用例

| 用例 | 为什么 | 任务 | 预计 |
| ---- | ------ | ---- | ---- |
| `crates/rurge-inbound/src/session.rs` 的现有用例 | 多一个字段、两个方法 | 1 | 照过 |
| `crates/rurge-config` 的 `config.rs` 里断言 `W0003` 的用例与 `rule.rs` 里 `always_capture = Some("debug-1")` 的那一条 | 只加 `W0012` | 1 | 照过 |
| 语料库快照 `corpus__corpus__kitchen-sink.snap` | `always-capture=debug` 不是空的 | 1 | 照过 |
| `crates/rurge-config/src/diagnostic.rs` 的 `every_code_is_decided_once` | 不加新码 | 1 | 照过 |
| `crates/rurge-http/src/rewrite/body.rs` 里 `decode` 的用例 | 拆成两步、消息不变 | 2 | 照过 |
| `crates/rurge-http/src/engine.rs` 测试模块的 `Fake::route` | `Routed` 多 `capture` | 3 | 改：只补字段（`Fake` 多一个格子） |
| `crates/rurge-http/src/engine.rs` 测试模块的其余全部 | 外面多一层 `CaptureSlot`；没有 `capture` 时不 arm、不包 | 3 | 照过 |
| `crates/rurge-engine/src/http.rs` 的 `ConnUpstreams::route` | `Routed` 多 `capture` | 3、4 | 改：任务 3 补 `None`，任务 4 换成 `capture_for` |
| `crates/rurge-engine/src/state.rs` 的用例 | `features.capture` 本来就有 | 4 | 照过 |
| `crates/rurge-engine/tests/` 的 `http.rs`、`mitm.rs`、`rewrite.rs`、`sniff.rs`、`pipeline.rs` 与其余 | 开关缺省关 | 3 ～ 8 | 照过 |
| `crates/rurge-api/tests/api.rs` 的 `features_collections_stop_and_unknown_paths` | `capture` 生效了，它拿 `capture` 当"没有生效的开关" | 5 | 改：换成 `scripting`；说明"只有哪几个生效"的文字加上 `capture` |
| `crates/rurge-api/tests/api.rs` 的其余 | 只加端点 | 5、6 | 照过 |
| `crates/rurge/tests/cli.rs` 的 `run` 模块的其余 | 多两个参数 | 7 | 照过 |
| `crates/rurge/tests/sockets.rs` 的扫描用例 | 不建套接字 | 全部 | 照过 |
| `crates/rurge-platform/tests/unsafe_functions.rs` | 不加放开的函数 | 全部 | 照过 |
| `crates/rurge-config/tests/secrets.rs` | 不加带凭据的配置写法 | 全部 | 照过 |

---

## 执行期修正记录

下面写"执行者的裁定"的，是 Pi 在设计与计划的原则下定的；写"协调者的裁定"的，是协调的会话代项目所有者定的。项目所有者都可以改。计划的正文不改：正文里说得不对的地方，以这张表的行为准。

| # | 任务 | 内容 | 提交 |
| - | ---- | ---- | ---- |
| 1 | 0 | 开工前核对（第 1、2 项）：`git status --short` 空；分支 `phase4-m4`；`HEAD` 是 `8e78469`；`git log --oneline e963d58..HEAD` 恰好两个提交（`8e78469 docs: 阶段 4 / M4a 实施计划（抓包）` 与 `a6e78ad docs: 阶段 4 / M4 细化设计（抓包与请求记录的完整）`），与派工写的"`a6e78ad` 之后只多了本计划的提交"一致。读了设计第 18 节（先读，第 573 ～ 591 行）、第 1 ～ 9 节（第 17 ～ 392 行）、第 11 ～ 14 节（第 451 ～ 506 行）、第 16 节（第 521 ～ 556 行）、第 17 节（第 557 ～ 572 行）；本计划的 P1 ～ P17、P16 的表、表后「原文」与全局约束（工作区的 `common-brief.md` 整份）；`docs/status.md` 阶段 4 的 M3b（第 949 ～ 969 行）、M3a（第 885 ～ 908 行）、M2b（第 772 ～ 789 行）三处「以后的会话必须知道的事」与它们后面的「去向写着项目所有者的」 | 本提交 |
| 2 | 0 | 名字与签名对得上的（第 3 项），`rurge-inbound` 与 `rurge-net`（任务 1）：`session.rs:25` 的 `SessionOutcome`（`Completed` / `Rejected(RejectKind)` / `Failed(String)`）、`:45` 的 `type FinishHook = Box<dyn FnOnce(&SessionHandle, &SessionOutcome) + Send>`、`:46` 的 `FirstByteHook`；`SessionHandle` 在 `:77`，字段 `method: OnceLock<String>`（`:96`）、`status: OnceLock<u16>`（`:97`）、`finished: AtomicBool`（`:105`）、`outcome: Mutex<Option<SessionOutcome>>`（`:106`）、`on_finish: Mutex<Vec<FinishHook>>`（`:107`）；**构造函数有三个**：`new(id, session)`（`:116`）、`new_with_token(id, session, token)`（`:122`）、`new_with_slot(id, session, token, process)`（`:133`），`method` / `status` / `on_finish` 都在 `new_with_slot` 里初始化（`:155`、`:156`、`:159`）；`set_method` / `method`（`:265` / `:271`）、`set_status` / `status`（`:279` / `:285`）、`on_first_byte`（`:331`）、`on_finish`（`:379`）、`finish`（`:386`）、`is_finished`（`:397`）、`outcome`（`:401`）、`connect_time`（`:366`）、`first_byte_time`（`:371`）、`error`（`:224`）、`rule`（`:206`）、`policy_chain`（`:214`）、`notes`（`:235`）、`trail`（`:242`）、`kill`（`:174`）、`id`（`:183`）、`elapsed`（`:198`）；测试模块在 `:634` / `:637`（15 条用例）。`rurge-net/src/trail.rs` 的 `peer`（`:45`）、`set_peer`（`:60`）、`dns`（`:65`）签名如计划。**出入一处**：P3 只提"`method`、`status`、`on_finish` 的初始化"，实际都在第三个构造函数里；`capture_session` 也加在那里（照 `method`、`status` 的 `OnceLock` 写法） | 本提交 |
| 3 | 0 | 名字与签名对得上的（第 3 项），`rurge-config`（任务 1）：`rule.rs:391` 的 `RuleParams`（`always_capture: Option<String>` 在 `:401`、`unknown` 在 `:402`），`RuleParams::parse(fields: &[Field])` 在 `:405`，`always-capture` 那一支在 `:426`（`("always-capture", Some(v)) => p.always_capture = Some(v.to_string())`，值不 trim），不认识的进 `:427` 的 `_ => p.unknown.push(f.text.clone())`；`config.rs` 读 `[Rule]` 的循环在 `:814-852`（`:817` 的 `for e in sec.active_entries()`、`:818` 的 `parse_rule(&e.raw, &ctx_for(&e.origin), &e.span)`），给 `W0003` 的两段是 `:820-828`（`r.params.unknown`）与 `:830-841`（`collect_kind_unknowns`），`rules.push(r)` 在 `:842`；断言 `W0003` 的现成用例是 `:2657` 的 `warnings`；`diagnostic.rs:214` 的 `W_INVALID_VALUE = "W0012"`、`:48` 的 `Diagnostic::warning(code, message)`、`:54` 的 `at(span)`、`:352` 的 `codes::hint`（`match` 里没有 `W0012`，落 `_ => return None`；文档注释 `:346-348` 写它在"many places for different reasons"那一类，`:504` 的名单里有它）——即"不设并写理由"；`every_code_is_decided_once` 在 `:550`。**三项的读法（任务 1 的用例第 2 条照它）**：一项拆成键与值在 `value.rs` 的 `read_field`（`:283`）：键 `item[..at].trim()`、值 `item[at + 1..].trim()`（`:296`、`:302`）；所以 `always-capture=` 与 `always-capture= `（等号后一个空格、在一行的末尾）都读成 `always_capture = Some("")`（行本身不去末尾空白，`split_fields` 收的是整行、只逐项 `trim`），都不进 `unknown`、也没有诊断；不带 `=` 的 `always-capture` 是 `param: None`，落进 `unknown`，于是 `W0003` | 本提交 |
| 4 | 0 | 名字与签名对得上的（第 3 项），`rurge-rules` 与语料库（任务 4）：`engine.rs:86` 的 `CompiledRule { index, matcher, policy, params, raw, span, hits, warned }`（`index` 的注释是"Index into `Config.rules`"）、`:296` 的 `RuleEngine::rules()`；`Decision` 在 `:138`（`matched: Option<usize>`、`sub_rule: Option<SubRuleHit>`）；命中 `RULE-SET`、`DOMAIN-SET` 与逻辑规则时 `matched` **都是那一行顶层规则在 `[Rule]` 里的下标**（`rule.index`）：正常求值 `:425`、FINAL `:437`、pre-pass `:485` / `:531` / `:540`，集合里命中的哪一行在 `sub_rule`（用例 `:1155` 的 `a_pre_matching_rule_set_reports_the_line_it_matched`：`matched == Some(2)`、`sub_rule.entry` 是集合里的那一行；逻辑规则见 `:1189-1206`）。所以 P10 的 `always_capture(&rt.rules, i)` 对这三种都成立。语料库：`tests/corpus/valid/kitchen-sink.conf` 有 `always-capture=debug`，快照 `crates/rurge-config/tests/snapshots/corpus__corpus__kitchen-sink.snap:78`；`rule.rs:1256-1259` 断言 `always-capture=debug-1` 只填 `always_capture`——两者都不是空值 | 本提交 |
| 5 | 0 | 名字与签名对得上的（第 3 项），`rurge-http` 的 `capture` 面（任务 2）：`Cargo.toml` **今天没有 `[features]`**（`flate2`、`base64`、`time`、`bytes`、`http`、`http-body-util` 都已是正式依赖），`lib.rs:19-26` 的八个 `pub mod`（`engine`、`headers`、`mitm`、`pool`、`prefix`、`rewrite`、`sniff`、`tls`）与 `:28-29` 的两个私有模块，`:31-40` 的再导出（`rurge_http::Entry` 是**连接入口**那个枚举，`engine.rs:58-67`）；`rewrite/body.rs:37` 的 `Limit { bytes, text }`、`:45` 的 `REQUEST`（`33_554_432` / `"32 MiB"`）、`:51` 的 `RESPONSE`（`10_485_760` / `"10 MiB"`）、`:289-344` 的 `pub(crate) fn decode(encodings: &[HeaderValue], body: &Bytes, limit: Limit) -> Result<Bytes, String>`、`:352` 的 `zlib_header`、`:363` 的 `is_token`；`decode` 的用例：`decoding_layers`（`:941`）、`unsupported_and_broken_encodings`（`:1006`）、`a_decompression_bomb_stops_at_the_limit`（`:1038`），辅助 `encoding` / `gzipped` / `zlibbed` / `deflated`（`:1051` / `:1056` / `:1063` / `:1070`），间接的 `a_compressed_body_is_decoded_first`（`:1246`）与 `decoding_failures_skip_every_matching_rule`（`:1270`）；`rurge-proto/src/outbound.rs:13` 的 `RejectKind`（派生 `Clone, Copy, Debug, PartialEq, Eq, Hash`）与 `:31` 的 `name()`（`REJECT` / `REJECT-DROP` / `REJECT-NO-DROP` / `REJECT-TINYGIF`）；`time` 今天只用在 `mitm/authority.rs`（`:49` 的 `use time::OffsetDateTime;`、`:477-478` 的 `time::Duration::days`），`Cargo.toml` 里是 `time.workspace = true`。**`decode` 的签名收 `&[HeaderValue]`，不是 `&HeaderMap`**——P7 的 `layers` 照它收切片 | 本提交 |
| 6 | 0 | `rurge-http/src/engine.rs` 的 `Requests::handle`（`pub async fn handle(&self, req: Request<Incoming>) -> Result<Response<ResponseBody>, Close>`，`:398`，函数体 399 ～ 1335）：段 → 行号 → 那一行的代码。`:405` 的 HTTP/2 `CONNECT` 501 那一支（`:406` `prepare`、`:407` `open`、`:409` `Guard::new`、`:416` `set_method`、`:417` `set_status`、`:418` `finish`、`:419` 返回）；`:430` 与 `:431-435` 监听器的两处 400；`:454` 的 `prepare`；`:458` 的 `open`；`:459` 的 `Guard::new`；`:468` 的 `set_method`；`:470-472` 的 `lease`；`:477-481` 的 `route(..).await`；`:482-484` 的 `token` 检查；`:485-495` 的 `match routed`（**两个变体都把字段列全、没有 `..`**）；`:496-502` 的 `prepared.unknown`；`:508` 的 `let sniffed = …`；`:513-519` 的 `let mut rewrite` 与它的条件；`:523` 的 `let mut url`；`:527` 的 `let mut origin`；`:528` 的 `let mut retargeted`（`:584` 置真）；`:531-554` 请求一侧的 Header Rewrite；`:556-625` 的 URL Rewrite（`:585` 的 `url = Some(new.url().to_string())`）；`:627-644` 的 Map Local；`:645` 的 `wants_upgrade`；`:648` 的 `tunnel`；`:649` 的 `needs_h1`；`:651-801` 请求的 Body Rewrite（`:656` 的 `let mut body_changed`）；`:803-812` 第 5 步的注释；`:814` 的 `let mut key`；`:816-822` 的 `borrowed_h2`；`:830` 与 `:852` 两处 `mark_connected`；`:864-869` 的 `redial`；`:885` 的 `Form::of`；`:886-895` 的 `headers::strip(req.headers_mut(), Keep { … })`；`:896` 的 `if !sniffed {`；`:897-899` 的 `absolute_form` / `origin_form`；`:909-917` 补 `chunked`；`:924` 的 `fit`；`:940` 的 `write_host`；`:954-957` 的 `head_of`；`:958` 的 `client_upgrade`；`:965` 的 `request_meter`；`:972` 的 `Idle::new`（**全仓唯一的调用处**）；`:983-988` 的发送 `select!`；`:989-1023` 与 `:1032-1093` 两次 `retry`；`:1128-1165` 升级那一支与 `promote`；`:1171-1173` 的 HTTP/2 应答改成 HTTP/1.1；`:1174` 的应答 `strip`；`:1178-1196` 应答一侧的 Header Rewrite；`:1198-1316` 应答一侧的 Body Rewrite（`:1202` 的 `resp.into_parts()`）；`:1319` 的 `set_status`；`:1334` 的 `Tail::new`；`:1335` 的 `Ok(Response::from_parts(parts, tail.boxed()))` | 本提交 |
| 7 | 0 | `engine.rs` 的其余类型、夹具与点名的用例（任务 3）：`Requests { shared: Arc<Shared<U>> }`（`:360-362`），`Shared`（`:290-310`：`conn`、`upstreams`、`pool`、`routed`、`activity`、`h2`），`Conn`（`:70-114`：`session`、`entry`、`origin`、`idle`、`head_wait`、`drop_hold`、`abort`、`token`、`tracker`、`h2`），`Entry { Sniffed, Listener }`（`:58-67`）；`Routed`（`:119-137`：`Go { route, key, pages, rewrite }`、`Refused { refusal, pages }`）；`Upstreams` trait（`:167-226`：`Key` / `Route` / `Lease` 三个关联类型与 `named` / `open` / `route` / `dial` / `retarget` / `gateway` / `relay`）；`answer`（`:1545-1593`，`REJECT-DROP` 那一支在 `:1578-1589`：先 `finish(Rejected(Drop))` 在 `:1580`、再 `select!` 等 `token` 或 `sleep(drop_hold)` 在 `:1582-1586`、最后 `Err(Close)`）；`failed`（`:1596`）、`timed_out`（`:1615`）、`page`（`:1633`）、`map_local`（`:1342`）、`promote`（`:1959`）；`Resend`（`:2121-2154`）、重发的函数叫 **`retry`**（`:1854-1871`）不是 `resend`，在 `handle` 里被调两次（`:994-1018`、`:1042-1067`），`redial`（`:1801`）；`Idle`（`:2606-2621`，字段 `inner` / `idle` / `deadline` / `progress` / `body`，**没有 `tap`**）、`Idle::new(inner, idle, progress, body)`（`:2624`）、`poll_frame`（`:2639-2678`，数据帧那一支 `:2654-2666`，`this.body.count_up(..)` 在 `:2662`）、`is_end_stream`（`:2680`）、`size_hint`（`:2684`）；`Tail`（`:2776-2807`）、`Tail::new`（`:2810`）、`:3255-3264` 的"先 `complete()`、再交出最后一帧"、`complete`（`:2876`，`finish(Completed)` 在 `:2888`）、`fail`（`:2916`）、`stop`（`:2930`）、`impl Drop for Tail`（`:2938`，**不结束记录**）、`is_end_stream` 读 `reached_end`（`:3323`）、`size_hint`（`:3327`）；`LocalFile`（`:2959`）与 `impl Drop for LocalFile`（`:3081`，**也不结束记录**）；`Guard`（`:3360-3371`）、`Guard::new`（`:3374`）、`outcome`（`:3400`）、`retire`（`:3403`）、`impl Drop for Guard`（`:3405-3411`：没结束时 `finish(self.outcome())`）；`outcome_of`（`:3342-3352`）；`headers.rs` 的 `HOP_BY_HOP`（`:8-18`，九项）、`Keep`（`:21-42`）、`strip`（`:45-68`）；`responses.rs:15` 的 `pub type ResponseBody = BoxBody<Bytes, BoxError>` 与 `:28` 的 `empty()`。测试夹具：`Step`（`:3705`）、`Inner`（`:3720-3748`）、`Fake`（`:3751`，`impl Fake` `:3753-3856`，`impl Upstreams for Fake` `:3857-3976`，两个 `Routed` 在 `:3918-3923`）、`Client`（`:3984`）、`Rig`（`:4111`）、`connected`（`:4133`）、`rig_timed(entry, target, idle, drop_hold)`（`:4144`）、`H2Rig`（`:6322`）、`rig_h2(target, idle, drop_hold)`（`:6335`）、`h2_client(stream)`（`:6364`）、`h2_request(method, url)`（`:6380`）、`recording_origin`（`:4781`）、`Holding`（`:4840`）、`streaming_origin`（`:4854`）、`wait_until`（`:4910`）、`chunked_origin`（`:4924`）、`fixed_origin`（`:4941`）、`stalled_origin`（`:4954`）、`origin_answering`（`:4960`）、`body_and_trailers`（`:5010`）、`upgrading_origin`（`:5119`）、`rewrite_of`（`:7638`）、`rewrite_from`（`:7653`）、`map_local_rules`（`:8541`）。点名的用例：`proxy_authorization_is_forwarded_only_on_a_sniffed_connection`（`:4514`，两种入口对比源站收到的 `proxy-authorization`）、`a_dropped_request_is_held_then_closed`（`:4683`，`drop_hold` 300 ms，句柄在停留前结束、`total` 不含停留）、`a_killed_request_closes_its_connection`（`:5340`，`kill` 之后无应答、结局 `Failed("killed")`）、`a_websocket_upgrade_is_relayed_and_counted_once`（`:5783`）、`a_connect_on_a_sniffed_connection_goes_to_its_target`（`:5838`，`Seen.session.url == None`、`http_host == Some("x.test")`）、`an_h2_client_is_served_over_an_http1_upstream`（`:6583`）、`a_stream_the_client_resets_is_cancelled_and_the_connection_stays`（`:6756`，结局 `Failed("client cancelled the request")`、同一连接上再发一个得 200）、`map_local_answers_without_a_dial`（`:8557`，`data-type=text` 那一条）、`request_headers_are_rewritten_before_the_hop_by_hop_ones_go`（`:7730`，`rewrite_of` 加头的现成用例） | 本提交 |
| 8 | 0 | 与计划对不上的（后面的任务照这里做）：① 任务 1 的 `capture_session` 加在 `new_with_slot`（第三个构造函数，`:133`）里，不在 `new` 里（P3 只写了"构造函数"的 `method` / `status` / `on_finish` 那几行）。② `always-capture=` 与 `always-capture= `（行尾一个空格）**读法完全相同**（都是 `Some("")`，因为 `read_field` 对值 `trim()`、对行不去尾空白）——任务 1 的用例第 2 条的期望照今天写就成立，不必按"等号后一个空格"另做一条。③ P16 说测试 `Fake::route`"只补字段"：还要给 `Fake` 多一个格子（`Capture` 的值），否则任务 3 的捕包用例没有地方把 `capture` 递进去；`Fake::route` 的两个 `Routed` 是硬编码的（`:3918-3923`）。④ `handle` 里 `match routed` 两处解构不带 `..`（`:486`、`:489`），P9 ② 说的"加上 `..`"就是它们；`crates/rurge-engine/src/http.rs:161`、`:170` 两个分支也要补字段。⑤ 做重发的函数叫 `retry`（`:1854`），`Resend` 是它的参数结构体；计划与设计里写"`resend`"的地方照实读成 `retry`（不改语义）。⑥ `decode` 收 `&[HeaderValue]`（不是 `&HeaderMap`），P7 的 `layers` 照它收切片。⑦ 断言 clap 写错参数退出码的现成用例是 `crates/rurge/tests/cli.rs:168` 的 `the_wintun_dll_option_is_a_usage_error_elsewhere`，它是 `#[cfg_attr(not(windows), test)]`，**本机跑不到**；任务 7 要新写一条本机能跑的（`--capture-size 10x` → `code(2)` 与 `stderr` 里的原文）。⑧ `Path<String>` 与 matchit 的组合确认了 P11 的假设：路由在原始路径上匹配（`matchit-0.8.4` 不解码），`%2F` 不当分隔符，`name` 解出来是 `a b/c`——任务 5 的 `odd_session_names_are_reached_through_the_path` 可以照写 | 本提交 |
| 9 | 0 | 名字与签名对得上的（第 3 项），`rurge-engine` 与端到端夹具（任务 4）：`shared.rs:173` 的 `EngineShared`（十三个字段，全带文档注释；末两个是 `mitm: Arc<MitmShared>`、`rewrite: Arc<rurge_http::RewriteCounters>`）、`:221-238` 的 `new(initial: GroupSelections)`（字面量 `:222-237`，P10 的 `capture` 加在这里）、`:240-244` 的 `Default`；**`EngineShared::new(` 四处**（`shared.rs:242`、`tests/outbounds.rs:569`、`tests/pipeline.rs:3198`、`src/cli/run.rs:1751`），**十八处 `EngineShared {` 字面量全部带 `..`**（逐个套 12 行窗口确认过），所以加字段不会让它们编不过；`engine.rs:202` 的 `mitm_on`、`:207` 的 `rewrite_on`（初值都在 `:291-292`）、`:595` 的 `shared()`、`:748` 的 `mitm_on()`、`:756` 的 `set_mitm`、`:766` 的 `rewrite_on()`、`:774` 的 `set_rewrite`（写 `s.features.rewrite = Some(on)`）、`:785` 的 `restore_switches(&self, features: &Features)`、`:1515` 的 `http_counts()`；`choose_policy_with(&self, rt: &Runtime, registry: &PolicyRegistry, handle: &SessionHandle, resolver: &dyn LazyResolver) -> Chosen`（`:953`），三处早退在 `:968`（external）、`:978`（直连）、`:979-981`（全局代理），`handle.set_rule(rule_raw(&rt.rules, i))` 在 `:1004-1006`；`rule_raw`（`:1657-1666`）、`settle`（`:1686-1694`，**只改 `error`、不结束句柄**）；出站模式切换是 `set_mode`（`:732-741`），用例里切直连是 `use rurge_engine::control::Mode;` + `h.engine.set_mode(Mode::Direct).await`（`tests/pipeline.rs:3236`、`tests/sniff.rs:107`、`tests/tun_dns.rs:472`）；`lib.rs:44` 的 `pub use rurge_http::mitm::ApiTls;`（P10 的 `pub use rurge_http::capture;` 照它写）；`state.rs:18-33` 的 `Features`（**已有 `capture: Option<bool>`**，`:27-28`）与 `:15` 的 `STATE_VERSION = 2`（任务 4 不动 `state.rs`）。用例夹具：`tests/common/mod.rs:44-49` 的 `Harness { dir, engine, listeners, dns }`、`:199-210` 的 `Profile`、`:231` 的 `harness`（内部 `EngineShared::default()`，所以根是 `None`）、`:244` 的 `harness_with`、`:428` 的 `wait_until(what, check)`、`:446` 的 `the_records(h, n)`、`:680` 的 `plain_get`；`tests/common/log_capture.rs:12` 的 `LogCapture`（`start()` 是 DEBUG、`:42` 的 `everything()` 是 TRACE、`:73` 的 `lines()`），接最详细一级的现成写法是 `tests/rewrite.rs:1694`；`tests/mitm.rs:1316-1356` 的 `the_switch_is_restored_from_state_json`；重载的现成写法 `tests/rewrite.rs:270-272`；`tests/rewrite.rs:799` 的 `mitm_section`、`:896` 的 `decrypted_get`、`:908` 的 `harness_mitm`、`:1294` 的 `RecordingProxy`；`tests/pipeline.rs:1433` 的 `http-listen = old@127.0.0.1:{port}` 与 `:1410-1411` 的 `CRED_OLD` / `CRED_NEW`（`:1386` 的 `connect_status` 写 `Proxy-Authorization: Basic {c}`）；逻辑规则的现成写法 `tests/tun_dns.rs:519`（`AND,((DOMAIN,x.test),(DEST-PORT,{named})),REJECT,pre-matching`）；`src/http.rs:116-178` 的 `ConnUpstreams::route`（两个分支在 `:161`、`:170`，`settle` 在 `:169`） | 本提交 |
| 10 | 0 | 名字与签名对得上的（第 3 项），`rurge-api`（任务 5、6）：`Cargo.toml` 的正式依赖没有 `base64` / `futures-util`、开发依赖有 `rurge-http.workspace = true`；`src/lib.rs:75` 的 `Router::new()` 到 `:122` 的最后一条 `.route(`（`/v1/rurge/http` 在 `:106`），状态是 `Shared`（`:43-53`）与 `type App = Arc<Shared>`（`:55`）；`src/routes/mod.rs` 的十四个 `pub mod`；`routes/features.rs` 的 `FEATURES`（`:14-21`，**已有 `capture`**）、`get_feature`（`"capture"` 落 `_ => {"enabled": false}`）、`set_feature`（`"capture"` 落 `_ => 501`）；`routes/rurge_http.rs` 全文（新模块照它写）；`routes/requests.rs:14` 的 `DEFAULT_LIMIT = 100` 与 `:142-158` 的 `recent`（`limit` 的读法：`Some(0)` → 400 `limit must be at least 1`）；`error.rs:12-15` 的 `ApiError`、八个构造函数、`:68` 的 `json_body`、`:77` 的 `query_params`；`routes/profiles.rs:69` 的 `BAD_BODY`、`:78-96` 的 `requested_name(body: Result<Bytes, BytesRejection>)`；`tests/api.rs:100-112` 的 `Api`、`:204` 的 `api()`、`:494` 的 `call`、`:511` 的 `get`、`:515` 的 `post`、`:544` 的 `wait_until`、`:683` 的 `features_collections_stop_and_unknown_paths`（**拿 `capture` 当未生效开关**：`:686` 假、`:689` 501、`:709` 那一条文字）、`:773` 的 `the_rewrite_switch_is_read_switched_and_kept` | 本提交 |
| 11 | 0 | 名字与签名对得上的（第 3 项），`rurge`（任务 7）：`src/cli/run.rs:60` 的 `RunArgs`、`:79-82` 的 `request_log_size`（属性的三行 + `pub request_log_size: Option<usize>`，新参数紧跟它）、`:123-136` 的 `parse_log_level`（`:69` 的 `value_parser = parse_log_level`）、`:239-242` 的 `RunOptions`、`:1711-1714` 建它、`:1751` 的 `EngineShared::new(` 与 `:1752-1759` 改字段那几句（P13 的 `shared.capture = CaptureStore::new(limits);` 加在这里）、`:1811` 的 `restore_switches` 调用、`:1891-1894` 起 API 之前那一处现成的 WARN（`http-api-tls` 那一句）、`:1910` 的 `println!("api on …")`；测试模块 `:2214-2215` 起（只 `use super::*;`），一条单元用例是 `log_levels_map_like_surge`（`:2298-2306`）。`tests/cli.rs:60` 的 `rurge_command`、`:75` 的 `rurge_process`、`:2894-2899` 的 `Daemon { child, http, socks, lines }`、`:2912` 的 `write_conf`、`:2956` 的 `rurge_run`、`:3015` 的 `spawn_command`、`:3103` 的 `http_get`、`:3142` 的 `wait_for_line`、`:3156` 的 `api_port`、`:3166` 的 `api_call`、`:4104` 的 `API_GENERAL`、`:3858` 的 `the_request_log_holds_as_many_requests_as_request_log_size_says`；断言 clap 参数错的一条是 `:168` 的 `the_wintun_dll_option_is_a_usage_error_elsewhere`（`#[cfg_attr(not(windows), test)]`，本机不跑——见第 8 条 ⑦） | 本提交 |
| 12 | 0 | 任务 8 用的两份（第 3 项）：`crates/rurge-tun/src/tests/bench/peak.rs` 全文 59 行——`pub(super) fn peak_resident_bytes() -> Option<u64>`（Windows 上起 `powershell -NoProfile -Command "(Get-Process -Id <pid>).PeakWorkingSet64"`、Linux 读 `/proc/self/status` 的 `VmHWM`、别的平台 `None`）、`fn parse_count(output: &str) -> Option<u64>`、`fn parse_vm_hwm(status: &str) -> Option<u64>`，用例 `the_peak_is_read_from_what_the_system_prints`（`:48-59`）；`crates/rurge-rules/tests/memory.rs` 开头的说明（`:1-16`："A measurement, not a gate"、跑法 `cargo test --release --test memory -- --ignored --exact <name> --nocapture`——P15 改成调试构建，注释要随之改）与 `fn nothing_is_built_is_the_baseline()`（`:168-172`，`#[ignore = "a measurement, not a gate: run on its own, in release"]`），大条目的构建在 `fn build_and_ask`（`:106-156`） | 本提交 |
| 13 | 0 | hyper 1.11.1 的事实（任务 3）：① HTTP/1.1 服务端写应答头时，正文的 `is_end_stream()` 已经是真就 `self.body_rx.set(None)`、**一次也不 poll、就地丢下**（`hyper-1.11.1/src/proto/h1/dispatch.rs:367-378`）；一个数据帧交出之后 `body.is_end_stream()` 为真时 `*clear_body = true` 让 `OptGuard` 在块尾把正文丢掉并用 `write_body_and_end` 写这一帧（`:389`、`:410-421`、`:519-527`）。② 客户端在应答中途关掉连接：连接出错走 `poll_catch` 的错误支（`:123-141`），`Dispatcher` 这个 future 收场；正文在 `Dispatcher::body_rx: Pin<Box<Option<Bs>>>`（`:26`）里，没有自己的 `Drop`，**随 `Dispatcher` 被丢下时一起丢**（`close()` 在 `:457-461` 只关读写）——所以 `CaptureBody::poll_frame` 看不到结尾，P8 的 `ResponseTap::done()` 在 `Drop` 里调正是为这一步。③ `size_hint().exact()` 交给 `BodyLength::Known`（`dispatch.rs:370-376`），HTTP/1.1 编码器据此写 `content-length`（`src/proto/h1/role.rs:1449-1455`）。④ HTTP/2 服务端**不看应答的 `version()`**：`src/proto/h2/server.rs` 只剥逐跳头、补 `date`、在 `body.size_hint().exact()` 有值时补 `content-length`（`:517-521`），然后 `reply!(me, res, ..)`（`:492`、`:525`）；整个文件没有一处读 `version`。⑤ HTTP/2 上客户端重置一个流，**服务这个请求的 future 会被丢下**：`H2Stream::poll2` 在服务 future 还没好时每轮 `me.reply.poll_reset(cx)`，RST_STREAM 到了就 `return Poll::Ready(Err(..))`（`src/proto/h2/server.rs:451-460`），`H2Stream` 随之结束、手里 `H2StreamState::Service { fut }`（`:378-384`）的 future 随之被丢下——P8 的 `Drop for CaptureSlot` 靠的就是这一件 | 本提交 |
| 14 | 0 | http 1.5.0、time 0.3.45、axum 0.8.9 / axum-core 0.5.6、futures-util 0.3.34、base64 0.22 与 url 的事实（任务 2、5、6）：`Version` 的 `Debug` 写 `HTTP/1.1` 与 `HTTP/2.0`（`http-1.5.0/src/version.rs:63-74`）；`StatusCode::canonical_reason(&self) -> Option<&'static str>`（`src/status.rs:170`）；`HeaderMap::iter` 的文档说次序是任意的但同名值聚在一起（"Each key will be yielded once per associated value"，`src/header/map.rs:911-935`）；`impl From<SystemTime> for OffsetDateTime` 在 `time-0.3.45/src/interop/offsetdatetime_systemtime.rs:59-67`，`year() -> i32` / `month() -> Month` / `day() -> u8` / `hour() -> u8` / `minute() -> u8` / `second() -> u8` / `millisecond() -> u16` 在 `src/offset_date_time.rs:603` / `:621` / `:640` / `:883` / `:902` / `:921` / `:938`，`Month` 的判别值就是 1 ～ 12 且 `u8::from(month)` 拿得到（`src/month.rs:15` 起、`:262-267`）；`time` 的 `default = ["std"]`（`time-0.3.45/Cargo.toml:57`）、`From<SystemTime>` 那个模块只要 `std`（`src/interop/mod.rs:25-26`），工作区是 `time = "0.3"`（根 `Cargo.toml:59`）——**不需要别的特性、不用开 `formatting`**；`Body::from_stream<S> where S: TryStream + Send + 'static, S::Ok: Into<Bytes>, S::Error: Into<BoxError>`（`axum-core-0.5.6/src/body.rs:57-67`）；`Path<String>` 解百分号（`axum-0.8.9/src/extract/path/mod.rs:23-25`，用例 `percent_decoding` `:639-651`），而 matchit 在原始路径上匹配、不解码（`matchit-0.8.4`）；`Bytes` 的 rejection 是 `axum::extract::rejection::BytesRejection`（唯一变体 `FailedToBufferBody`，`axum-core-0.5.6/src/extract/rejection.rs:70-72`）；`futures-util` 的 `pub mod stream;`（`src/lib.rs:292`）与 `mod iter; pub use self::iter::{iter, Iter};`（`src/stream/mod.rs:75-76`）都**不在任何特性之后**，工作区给的是 `default-features = false, features = ["sink"]`（根 `Cargo.toml:82`），`stream::iter` 现在就拿得到；`base64` 的现成写法在 `crates/rurge-http/src/mitm/generate.rs:16-17`（`use base64::Engine as _;` + `use base64::engine::general_purpose::STANDARD;`）与 `:67`、`:127`；锁文件里 `url 2.5.8`（`Cargo.lock:4759-4762`），`Url::parse(input: &str) -> Result<Self, ParseError>`（`url-2.5.8/src/lib.rs:341`）、`query_pairs(&self) -> form_urlencoded::Parse<'_>`（`:1509-1511`） | 本提交 |
| 15 | 0 | 内部的四件（第 3 项末段，也接第 8 条的 ⑤）：① `SessionHandle::finish` 的次序就是 P3 写的那样：`finished.swap(true, AcqRel)`（`session.rs:387`）→ 写 `outcome` 并放锁（`:390`）→ 在 `on_finish` 的锁里 `take` 走全部钩子（`:391`）→ 逐个调（`:392-394`）——所以 `rurge-http` 的 `answer` 对同一个句柄再 `finish` 一次不起作用（`:387` 第一句就返回）。② `prepare` 给 `url = None` 只在 `CONNECT` 上（`engine.rs:1521-1527`，赋值 `:1533`）。③ `choose_policy_with` 命中 `RULE-SET`、`DOMAIN-SET` 与逻辑规则时 `decision.matched` 是顶层规则的下标（同第 4 条）。④ `Guard` 丢下时以 `outcome_of` 的结局结束还开着的记录（`engine.rs:3405-3411`：被杀是 `Failed("killed")`、连接被停是 `Completed`、其余是 `Failed(left)`），而 `settle` **不**结束句柄、只改 `error`（`:1686-1694`）。五、做重发的函数叫 `retry`，`Resend` 是参数结构体（同第 8 条 ⑤） | 本提交 |
| 16 | 0 | 数（第 4 项）：`Cargo.lock` 的 `[[package]]` **520**；`crates/rurge-platform/tests/unsafe_functions.rs` 的 `LISTED` 是 `[(&str, &str); 23]`（`:18`），实际 **23** 项；`crates/rurge/tests/sockets.rs` 的 `LISTED` 是 `[(&str, &str, &str); 26]`（`:58`），实际 **26** 行（数每行以 `"crates/` 开头的）；带本机账户名的文件 **26**（只跑了一次那个 `git grep -l` 数行数，名字不写进任何文件）。四个数都与派工给的基线相同 | 本提交 |
| 17 | 0 | 现有用例（第 5 项，P16 的表逐条核过，按任务）：**任务 1** `session.rs` 的 15 条、`config.rs` 的 126 条（点名的 `warnings` 在 `:2657`）、`rule.rs` 的 `every_rule_parameter_the_manual_lists_is_read_into_its_own_field`（`:1212`，`always-capture=debug-1` 在 `:1256-1259`）、语料库 `corpus.rs:49` + `corpus__corpus__kitchen-sink.snap`、`diagnostic.rs:550` 的 `every_code_is_decided_once`——**全部照过**（没有一条断言这两种写法的读法，也没有一条把 `always_capture` 当成空值）。**任务 2** `body.rs` 的 `decoding_layers`、`unsupported_and_broken_encodings`、`a_decompression_bomb_stops_at_the_limit` 与间接用到 `decode` 的两条——**照过**（前提是拆出来的 `layers` / `undo` 交回的消息与今天逐字相同）。**任务 3** `engine.rs` 测试模块的 103 条里 `Fake::route` **改**（两个 `Routed` 补 `capture` 字段、`Fake` 多一个格子），其余 102 条**照过**（`Idle::new` 只有一个调用处、没有别处写 `Routed::` 字面量、`handle` 的签名不变、`capture` 为 `None` 时 `arm` 什么都不做且 `answered` 原样交回）；`crates/rurge-engine/src/http.rs` 的 `ConnUpstreams::route` **改**（任务 3 补 `None`、任务 4 换成 `capture_for`）。**任务 4** `state.rs` 的 9 条**照过**（`capture` 字段本来就在；`:244` 的 `only_the_switches_that_were_set_are_written` 只设 `mitm`，任务 4 不往 `Features` 加字段、也不在该用例里调 `set_capture`）；`crates/rurge-engine/tests/` 的 650 条**照过**（全部用 `..EngineShared::default()` 建引擎，新字段是空存储、开关初值假）。**任务 5** `api.rs` 的 67 条里 `features_collections_stop_and_unknown_paths`（`:683`）**改**（`capture` 换成 `scripting`，`:709` 的文字加上 `capture`），其余 66 条**照过**。**任务 7** `cli.rs` 的 180 条**照过**（`the_request_log_holds_as_many_requests_as_request_log_size_says` 只碰 `--request-log-size`；两条 `run --help` 的用例只断言子串）。**3 ～ 8 与"全部"** `sockets.rs` 的 6 条、`unsafe_functions.rs` 的 7 条、`secrets.rs` 的 29 条**均照过**（捕包不建套接字、不加放开的函数、不加带凭据的配置写法）。与 P16 预计不同的只有一处：`Fake` 要**多一个格子**（已抄进第 8 条 ③） | 本提交 |
| 18 | 0 | 补扫一遍（第 5 项末段）：扫了 `git grep -n 'Routed::'`、`'Idle::new'`、`'always_capture\|always-capture'`、`'"capture"'`、`'EngineShared {'`，另加 `'SessionHandle {'`、`'Features {'`、`'/v1/features'`、`run --help` 与 `state.json` 的断言。结论：① `Routed::` 的字面量与解构只有四处要改（`crates/rurge-engine/src/http.rs:161`、`:170`，`crates/rurge-http/src/engine.rs:486`、`:489`，另有测试 `Fake::route` 的 `:3900`、`:3906`）；`crates/rurge-tun/src/dispatch.rs` 里的那个 `Routed` 是同名的另一个枚举，与 M4a 无关。② `Idle::new(` 全仓只有 `crates/rurge-http/src/engine.rs:972` 一处。③ `always_capture` / `always-capture` 只在 `crates/rurge-config/src/rule.rs`（`:401`、`:426`、`:1257`、`:1258`）与语料库。④ `"capture"` 只有 `crates/rurge-api/src/routes/features.rs:18`（`FEATURES` 表）与 `crates/rurge-engine/src/state.rs:234`（一条 v1 文件的用例）。⑤ **不带 `..` 的 `EngineShared {` 一处也没有**（十八处字面量全部带 `..`）、**不带 `..` 的 `SessionHandle {` 只有 `session.rs:139`**、**不带 `..` 的 `Features {` 一处也没有**。⑥ `/v1/features` **没有列表端点**（只有 `/v1/features/{name}`），断言 `capture` 的值的只有 `api.rs:686`、`:689`、`:709`。⑦ 断言 `rurge run --help` 输出的两条（`cli.rs:100`、`:139`）只查子串，加两个参数不会红。⑧ 断言 `state.json` 的 `features` 一段的：`state.rs:259-263` 是精确比但只设了 `mitm`；其余都是单键（`api.rs:792`、`:826`、`tests/mitm.rs:1337`、`cli.rs:4742`、`:4786`、`:4812`、`:5017`、`:5131`、`:8043`、`tests/run/enhanced_mode.rs:140`、`:426`、`:631`），`cli.rs:8049` 只查不出现带 `paus` / `suspend` 的键——写 `features.capture` 都不会让它们红 | 本提交 |
| 19 | 0 | 门禁（第 6 项，本任务只改文档，量的是本计划的基线）：先 `df -h /d`（448G 总、374G 已用、剩 74G）；`cargo fmt --all --check` exit=0（`gate-task-0-fmt.log`）、`cargo clippy --all-targets -- -D warnings` exit=0（`gate-task-0-clippy.log`）、`cargo test -j 8 --workspace --no-fail-fast` exit=0（`gate-task-0.log`）；测试 **92 个结果行、5227 通过、0 失败、13 忽略**，与派工给的基线（92 / 5227 / 0 / 13）差 0；没有 `STATUS_HEAP_CORRUPTION` / `STATUS_ACCESS_VIOLATION`、没有 `LNK1104`、没有用例偶发失败，所以没有重跑。这是一台 Windows 11 x86-64 开发机上的一次运行，不是 CI | 本提交 |
| 20 | 0 | 协调者核过任务 0（`85db664`，第 1 ～ 19 行）：门禁的数（92 / 5227 / 0 / 13，一台机器上的一次运行，不是 CI；没有崩溃与重跑；日志里 `cannot bind http-api … (os error 10048)` 那一行是现有用例的预期输出）、`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 的退出码都是 0、计划的两张表只多不少（修正记录多第 1 ～ 19 行，延后事项没有动）、带本机账户名的文件 26、`git diff --check` 干净，都对得上；第 2、3、8、18 行引的本仓行号抽查过（`crates/rurge-config/src/value.rs:294-303` 的 `read_field`，`crates/rurge-inbound/src/session.rs:116`、`:122`、`:133`、`:331`、`:379`、`:386`、`:401`，`crates/rurge-engine/src/http.rs:161`、`:170`，`crates/rurge-http/src/engine.rs:486`、`:489`、`:972`，`crates/rurge/tests/cli.rs:171-190`），都对，只有两处差几行：第 2 行说 `new_with_slot` 里 `method`、`status`、`on_finish` 的初始化在 `:155`、`:156`、`:159`，实际在 `:154`、`:155`、`:161`；第 7 行说测试 `Fake::route` 的两个 `Routed` 在 `:3918-3923`，实际在 `:3900`、`:3906`（第 18 行写对了）。第 8 行的 ① ～ ⑧ 接受：①、②、③ 写进任务 1、3 的派工；⑤ 只是名字，计划与设计里说重发的 `resend` 指今天的 `retry`，正文不改；⑦ 由任务 7 的 `bad_capture_sizes_are_refused` 写成不带平台条件的 `#[test]`（`12x` 与 `17179869184g` 在三个平台上都是错的值），断言照 `the_wintun_dll_option_is_a_usage_error_elsewhere` 的写法（`out.status.code() == Some(2)` 与标准错误里的原文），但不用会一直等下去的 `output()`：值要是被错收下，守护进程会一直跑，所以等它退出要有时限，到时杀掉再判失败（任务 7 的派工写法） | 本提交 |
| 21 | 1 | 并发的用例单独循环跑二十次（任务 1）：先编好，直接跑测试二进制 `target\debug\deps\rurge_inbound-49695a541a367c31.exe` 加 `--exact session::tests::when_finished_and_finish_race_without_losing_or_repeating`，每次用 `Start-Process` 起、`WaitForExit(120000)` 套时限，过程写进工作区的 `loop-task-1.log`：二十次都是 exit=0、`1 passed`（每次约 3 秒，没有一次超时）；二进制名照当时的哈希写，重编之后会变 | 本提交 |
| 22 | 1 | 和计划对不上的（协调者的裁定）：`an_empty_always_capture_is_ignored_with_w0012` 的四行改成 `DOMAIN,c.test,DIRECT,always-capture= `、`DOMAIN,a.test,DIRECT,always-capture=debug`、`DOMAIN,b.test,DIRECT,always-capture`、`FINAL,DIRECT,always-capture=`——`FINAL` 放在 `[Rule]` 的最后一行，不放在第一行（任务简报写的是它排第一）：`FINAL` 之后还跟着规则时，`config.rs` 会在其后第一行的 `span` 上多一条 `W0019`（`rule(s) after FINAL never take effect`），与本条用例无关。于是诊断恰好三条（`c.test` 那一行的 `W0012`、`b.test` 那一行的 `W0003`、`FINAL` 那一行的 `W0012`，行号 2、4、5），用例连整份诊断的次序一起断言；`config.rules` 里四条都在，两个空值的 `params.always_capture` 仍是 `Some("")` | 本提交 |
| 23 | 1 | 执行期看到的与门禁（任务 1）：① 空壳下先跑的六条里红四条——`when_finished_runs_at_once_on_a_finished_session`、`a_hook_may_ask_for_another_from_inside`、`the_capture_session_is_the_first_name_set` 与**并发那一条**（派工说并发可能红也可能绿；空壳下它第 1 轮就红：钩子推进去的时候 `finish` 已经取走了钩子，计数是 0）；`when_finished_waits_for_an_open_session` 与 `when_finished_hooks_keep_their_order` 在空壳下就绿（空壳的行为就是 `on_finish`）。② 第二次聚焦跑遇 `STATUS_HEAP_CORRUPTION`（`rurge_dns` 的测试二进制，exit=0xc0000374），按 `CLAUDE.md`「这台开发机」单独重跑那条命令即全过（`commit-a-tests-rerun.log`）。③ 门禁：`df -h /d` 剩 74G；`cargo fmt --all --check` exit=0（`gate-task-1-fmt.log`）、`cargo clippy --all-targets -- -D warnings` exit=0（`gate-task-1-clippy.log`）、`cargo test -j 8 --workspace --no-fail-fast` exit=0（`gate-task-1.log`），**92 个结果行、5234 通过、0 失败、13 忽略**，与任务 0 的基线加本任务的 7 条（`session.rs` 6 条、`config.rs` 1 条）相符；这是一台 Windows 11 x86-64 开发机上的一次运行，不是 CI，没有重跑 | 本提交 |
| 24 | 1 | 协调者的评审（任务 1）：过了（`fb2c1f8`、`808fb56`；`49dafa3` 只改第 22 行里一个名字的写法，是它自己的行、在评审之前，接受——同 M3b 计划的 `3a6bd8e`），**任务 1 过了**。核过：门禁 92 / 5234 / 0 / 13（一台机器上的一次运行，不是 CI；日志里只有现有用例预期的 `cannot bind http-api … (os error 10048)` 一行；`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 的退出码都是 0），计划的两张表只多不少（第 21 ～ 23 行），新用例 7 条、名字照任务全文，两次 RED（空壳下 `session` 红四条，含并发那一条；`W0012` 那一条红在少了两条 `W0012`）与 GREEN（`session` 的 21 条）的日志都在，`Cargo.toml` 与 `Cargo.lock` 没有动，带本机账户名的文件 26，`git diff --check` 干净。读过的：`when_finished` 先锁 `on_finish`、在这把锁里读 `outcome`（另一把锁，读完就放），读到 `Some` 先放锁再调 `f`；`finish` 写 `outcome` 时不握 `on_finish` 的锁、取钩子时不握 `outcome` 的锁，没有一处反着拿这两把锁，所以不死锁、不漏、不调两次；`capture_session` 照 `method` 的 `OnceLock` 写法、在 `new_with_slot` 里初始化；`W0012` 的消息与 P4 原文逐字相同、放在给 `W0003` 的两段之后，用例照派工的次序写、连整份诊断一起断言。一处记法：`loop-task-1.log` 每一次只记了退出码，第 21 行"每次 `1 passed`"是从同一条 `--exact` 命令与最后一次的输出推出来的，接受；之后的任务循环时每一次记下 `test result:` 那一行（任务 3、4、7、8 的派工写了） | 本提交 |
| 25 | 2 | 提交 (a)（`refactor(http): rewrite::body::decode 拆出 layers 与 undo`）：动手之前与之后各跑一次 `rewrite::body::tests::`，两次都是 23 条、全过（`pre-a-decode-tests.log`、`commit-a-tests.log`）——这一步是重构，不要求先红。`decode` 的现有用例一条不改。`Undo` 是 `pub(crate) enum Undo { Unsupported(Option<String>), Corrupt(String), Over }`，不派生（还没有谁要比较它）；转成 note 的那四句话在一处，调用方看到的消息与拆之前逐字相同（`decode` 的 23 条钉住） | b43c83d |
| 26 | 2 | `Cargo.toml` 的 `[features]` 一节（`testing = []`，注释照 `rurge-net` 那一行的写法）加在 `[dependencies]` 之后、`[dev-dependencies]` 之前；加完先跑 `cargo check --offline --workspace --all-targets`，exit=0，没有要下载的东西；`Cargo.lock` 的 `[[package]]` 仍是 520 个，`git diff Cargo.lock` 是空的（特性不进锁文件） | 9d6dac0 |
| 27 | 2 | `#[allow(dead_code)]` 标在 `crates/rurge-http/src/lib.rs` 的 `pub mod capture;` 上，上面两行注释写引擎从 `phase 4 / M4a plan's task 3` 起用 `Capture::begin` 与 `Pending`；**任务 3 要把它去掉**（那时引擎用上它们，`dead_code` 也就没有了）。`mod store;` 与 `pub use store::{CaptureStore, SessionSummary, Status}`、`pub(crate) use store::Pending` 在 `capture/mod.rs` 顶上 | 9d6dac0 |
| 28 | 2 | 与计划对不上的（执行者的裁定）：① `capture` 要用 `layers` / `undo` / `Undo`，得先在 `crates/rurge-http/src/rewrite/mod.rs` 的 `pub(crate) use body::{…}` 里放出这三个名字——`mod body;` 是私有的，计划没写这一步；② `Undo` 到那四句话的转换抽成私有的 `reason(why, limit)`：`layers` 也会失败（值不是可见的 ASCII），两处要同一句，计划写的是在 `decode` 里转；③ `Body::decoded` 把 `headers.get_all(CONTENT_ENCODING)` 的各项收进一个 `Vec<HeaderValue>` 再交给 `layers`（`layers` 收 `&[HeaderValue]`，同 `decode`），`layers` 交不出 `Undo::Over`，那一支照 `Unsupported(None)` 转；④ `Pending` 的两个正文用私有的 `Taking`，喂块经一个私有的 `take(..)` 与 `Axis` 的两个值，好让 `self.request`、`self.held` 与 `self.store` 各自借一次 | b43c83d、9d6dac0 |
| 29 | 2 | RED 与 GREEN 与门禁（任务 2）：① 空壳下 `capture::tests::` 5 条红 4 条（`a_body_without_an_encoding_is_not_decoded` 在空壳下就绿：空壳正好交回 `Ok(None)`），见 `red-task-2-capture.log`；② 空壳下 `capture::store::` 13 条红 12 条（`the_store_never_holds_more_than_its_total` 在空壳下就绿：空壳的 `used` 一直是 0），见 `red-task-2-store.log`；③ 用例的夹具条目一开始按 694 字节写，实际是 695（URL `http://a.test/x` 是 15 字节，不是 14），按实际的数改用例、不改实现；④ `a_chunk_evicts_older_entries_to_find_room` 的 `evicted == 1` 只在喂块之后成立——条目存进去时它的代价里含这一块预留过的正文，会再挤掉一条（存进去之后 `used` 是 695 + 1000、只剩这一条），用例照实际断言；⑤ 绿：`capture::tests::` 5 条、`capture::store::` 13 条、`capture::` 合计 18 条全过（`green-task-2-capture.log`、`green-task-2-store.log`、`commit-c-tests.log`），这一组用例不等待、不并发，没有循环二十次；⑥ 门禁：`df -h /d` 剩 74G；`cargo fmt --all --check` exit=0（`gate-task-2-fmt.log`）、`cargo clippy --all-targets -- -D warnings` exit=0（`gate-task-2-clippy.log`）、`cargo test -j 8 --workspace --no-fail-fast` exit=0（`gate-task-2.log`），**92 个结果行、5252 通过、0 失败、13 忽略**（基线 5234 加本任务的 18 条），这是一台 Windows 11 x86-64 开发机上的一次运行，不是 CI，没有重跑 | 本提交 |
| 30 | 2 | 协调者的评审（任务 2）：大部分对，一处要改（修正轮 1，见其后一行）。核过：门禁 92 / 5252 / 0 / 13（一台机器上的一次运行，不是 CI；日志里只有现有用例预期的 `cannot bind http-api … (os error 10048)` 一行；`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 的退出码都是 0），计划的两张表只多不少（第 25 ～ 29 行），新用例 18 条、名字照任务全文，两次 RED（空壳下 `capture::tests::` 红四条、`capture::store::` 红十二条）与 GREEN（`capture::` 的 18 条）的日志都在，`Cargo.lock` 没有动（`[[package]]` 仍是 520），带本机账户名的文件 26，`git diff --check` 干净。读过的：`decode` 拆开之后四句消息逐字不变（`reason`），`layers` 与 `undo` 照原来的读法；P6 的记账逐条对得上——`reserve` 在 `reserved + n > total` 时什么也不挤，`insert` 先还 `held` 再按整条的账挤与收，`Pending` 抄块不切，`commit` 之后 `held` 是 0、`Drop` 只还没有交出去的预留；`Entry`、`Head`、`ResponseHead`、`Body`、`Capture`、`CaptureStore`、`Pending` 都不实现 `Debug`。报告的五条顾虑：① 第 29 行 ④ 存进去时再挤掉一条——头的那一半在喂块时不预留，存进去时才连同正文一起记账，照 P6 就是这样，接受；② 夹具的条目是 695 字节（第 29 行 ③），接受；③ `SessionSummary.bytes` 是这个会话的条目的账，照设计第 7.2 节"这个会话的条目记着的字节"，接受；④ `lib.rs` 的 `#[allow(dead_code)]` 是临时的，任务 3 去掉（第 27 行，派工也写了）；⑤ `put` 只在 `test` 或 `testing` 下，照 P5，接受。`014cd9a` 只改第 28、29 行各一个错字，是它自己的行、在评审之前，接受（同第 24 行的 `49dafa3`）。**要改的**：`crates/rurge-http/src/capture/mod.rs:41` 的 `ENTRY_OVERHEAD` 文档注释写"task 8 measured it and left it as it was"——任务 8 还没有做，量过之后照 P15 也可能改这个数，这一句说了没有发生的事；改成任务 8 量它、可能改它。顺手一处：`Body::decoded` 里 `Undo::Over` 那一支的注释"a value nothing was read from is no layer"读不懂，改成说 `layers` 不读正文、交不出 `Over` | 本提交 |
| 31 | 2 | 修正轮 1（照第 30 行）：① `crates/rurge-http/src/capture/mod.rs:39-42` 的 `ENTRY_OVERHEAD` 文档注释第三行 "(`phase 4 M4 design 4.3`; task 8 measured it and left it as it was)." 换成 "(`phase 4 M4 design 4.3`); the phase 4 / M4a plan's task 8 measures it and" 与 "may change it." 两行——原来那句说了没有发生的事（任务 8 还没做），照 P15 量过之后也可能改这个数；引计划的写法照 `crates/rurge-http/src/lib.rs:23` 的 "the phase 4 / M4a plan's task 3"。② `mod.rs:213` 的 "a value nothing was read from is no layer" 换成 "`layers` reads no body, so it never says `Over`"——这一支走不到：`layers` 只读 `Content-Encoding` 的值、不读正文，交不出 `Over`（第 28 行 ③）。两处都照派工整段换掉，别的行一个字没动；改完之后 `git grep -F` 那两个旧句在 `crates/rurge-http/src/` 里一处也没有（exit=1）。③ 只改注释，所以只跑 `cargo fmt --all --check`（exit=0，`gate-task-2-fix1-fmt.log`）、`cargo clippy --all-targets -- -D warnings`（exit=0，`gate-task-2-fix1-clippy.log`）与 `cargo test -j 8 --workspace --lib -- capture::`（18 passed、0 failed、exit=0，`gate-task-2-fix1-test.log`），全量门禁没跑（协调者的裁定，同 M3b 任务 4 的修正轮 1）。④ `git diff --stat` 是 `crates/rurge-http/src/capture/mod.rs` 与 `docs/superpowers/plans/2026-11-01-phase4-m4a-capture-plan.md` 两个 | 本提交 |
| 32 | 2 | 协调者核过修正轮 1（`6c57466`，第 31 行）：两处注释照派工整段换掉、别的行没有动（`git show 6c57466 -- crates/` 只有这两处，`+3 −2`），两句旧话在 `crates/rurge-http/src/` 里一处也没有；`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 的退出码都是 0，`capture::` 的 18 条全过（一台机器上的一次运行，不是 CI；只改注释，全量门禁没有跑，任务 2 的全量是第 29 行 ⑥ 的 92 / 5252 / 0 / 13）；计划的两张表只多第 31 行；带本机账户名的文件 26，`git diff --check` 干净。**任务 2 过了** | 本提交 |
| 33 | 3 | 提交 (a)（`refactor(http): Routed 带 capture、Idle 多一个参数（行为不变）`）：`Routed` 的两个变体各多 `capture: Option<Capture>`、`handle` 里解出它的两处用 `..`；`Idle` 多一个 `tap` 字段与 `new` 的第五个参数（这时是 `capture/slot.rs` 的空壳 `RequestTap`），`poll_frame` 里数据帧计数那一句旁边交给它，唯一的调用处给 `None`；`crates/rurge-engine/src/http.rs` 的两处 `Routed` 补 `capture: None`（注释写任务 4 给真的）。动手之后跑一次 `cargo test -j 8 --workspace --lib -- engine::tests::`，103 条、全过（这一提交是重构，不要求先红；动手之前的 103 条是任务 0 与本表第 17 行记的基线）；`cargo clippy --all-targets -- -D warnings` exit=0 | 2c706b7 |
| 34 | 3 | 与计划对不上的（执行者的裁定）：① 派工的第 3 步与第 6 步把测试夹具的 `fn capture(&self, capture: Capture)` 放在提交 (a)，照做会有一条 `method capture is never used`，`-D warnings` 下 clippy 不过（那时还没有一条用例用它）；所以提交 (a) 只有 `Inner` 的 `capture` 格子与 `route` 的两处克隆，setter 与用例一起进提交 (b)。② 测试夹具的 `Fake::dial` 在连上之后补一次 `handle.trail().set_peer(peer)`：真的 `ConnUpstreams::dial` 经 `trail().follow(..)` 把地址留在 trail 上，`Fake` 原来只在**借来的**连接上留（`pooled.peer`），新拨的那一条不留，于是新拨的请求抓到的 `remote` 永远是 `None`、用例 1 的 `remote == 源站地址` 无法成立。只改夹具，不动生产代码 | 2c706b7、本提交 |
| 35 | 3 | 与计划对不上的（执行者的裁定）：用例 14（`an_http2_stream_reset_before_its_answer_is_captured_with_no_response`）的源站用 `holding_origin(b"")`（收下请求、什么也不答），不用任务简报写的 `stalled_origin`：`stalled_origin` 当场就写应答头与五个字节，客户端重置流与应答是**赛跑**的——跑二十次里十次左右、应答先到，于是 `answered` 已经给出 `Given { done: None }`，条目要等 `Tail` 被丢下才存得进去（hyper 在流被重置之后并不立刻丢它），`one_entry` 的 5 秒等不到。用例的名字要的就是「应答之前重置」；换成不答的源站之后，重置时 `serve` 的 future 还在等应答头，`CaptureSlot` 的 `Drop` 与 P8 写的那条路对上，二十次全过 | 本提交 |
| 36 | 3 | 并发的用例单独循环跑二十次（任务 3）：先 `cargo test -j 8 --workspace --lib --no-run` 编好，再直接跑测试二进制 `target\debug\deps\rurge_http-c459489a1f84fc74.exe` 加 `--exact engine::tests::<名字>`，五条（用例 8、9、10、13、14）各二十次，共一百次，每一次把 `test result:` 那一整行与退出码写进工作区的 `loop-task-3.log`：一百次都是 exit=0、`ok. 1 passed; 0 failed`，没有一次超时或失败（每次 0.01 ～ 0.04 秒）。二进制名照当时的哈希写，重编之后会变 | 本提交 |
| 37 | 3 | 任务 2 标在 `crates/rurge-http/src/lib.rs` 的 `pub mod capture;` 上的 `#[allow(dead_code)]` 与它上面两行注释一起删掉（第 27 行的去向）：`cargo clippy --all-targets -- -D warnings` 仍是零警告，**也没有剩下没人用的项**，所以没有另标 `allow` | 本提交 |
| 38 | 3 | RED、GREEN 与门禁（任务 3）：① 空壳下先跑用例 1 ～ 6，六条全红（`red-task-3-batch1.log`），7 ～ 12 六条全红（`red-task-3-batch2.log`），13 ～ 17 五条全红（`red-task-3-batch3.log`），红的都是 `one_entry` 到 5 秒时报 `entries 0, stored 0, …, used 0`（`a_second_request_on_a_kept_connection_is_captured_as_reused` 是它自己的 `wait_until` 报的）；② 绿：`engine::tests::` 103 + 17 = 120 条全过；`answer`、`failed`、`page`、`map_local`、`timed_out`、`Tail`、`LocalFile`、`Guard` 的函数体没有改动——`git diff 10bdb5d -- crates/rurge-http/src/engine.rs` 的 hunk 头是 `:21`（import）、`:130`（`Routed`）、`:396`（`handle` 改名）、`:482`、`:491`、`:505`（求规则之后 arm、取样点、去掉旧 `sniffed`）、`:819`、`:827`、`:849`（reused）、`:893`（`Idle::new`）、`:969`（赋值处的文件缩进）、`:1161`（升级那一支 `slot.upgrading()`）、`:2258`（`captured_url` 加在 `wants_upgrade` 前）、`:2615`、`:2659`（`Idle`）、`:3743` 起（测试模块），`answer` 起（旧文件 1545）到 `write_host` 之间除 `:1161` 那一处外没有 hunk，`Tail`（旧 2776）到测试模块之间没有 hunk；③ 门禁：`df -h /d` 剩 73G；`cargo fmt --all --check` exit=0（`gate-task-3-fmt.log`）、`cargo clippy --all-targets -- -D warnings` exit=0（`gate-task-3-clippy.log`）、`cargo test -j 8 --workspace --no-fail-fast` exit=0（`gate-task-3.log`），**92 个结果行、5269 通过、0 失败、13 忽略**（基线 5252 加本任务的 17 条）；第一次全量里 `rurge-engine --test profile_sources` 以 `STATUS_HEAP_CORRUPTION`（0xc0000374）退出（`CLAUDE.md`「这台开发机」的那一条），单独重跑那一个二进制 45 条全过（`gate-task-3-rerun-profile-sources.log`），再跑一次全量即上面这一次；这是一台 Windows 11 x86-64 开发机上的一次运行，不是 CI | 本提交 |
| 39 | 3 | 协调者的评审（任务 3）：大部分对，四处要改（修正轮 1，见其后一行）。核过：门禁 92 / 5269 / 0 / 13（一台机器上的一次运行，不是 CI；第一次全量里 `profile_sources` 以 `STATUS_HEAP_CORRUPTION` 退出，单独重跑 45 条全过，再跑一次全量；日志里只有现有用例预期的 `cannot bind http-api … (os error 10048)` 一行；`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 的退出码都是 0），计划的两张表只多不少（第 33 ～ 38 行），新用例 17 条、名字照任务全文，三批 RED（6、6、5 条，都红在等条目的时限）与 GREEN（`engine::tests::` 120 条）的日志都在，五条用例各循环二十次、每一次都记了 `test result:` 那一行，`Cargo.toml` 与 `Cargo.lock` 没有动，带本机账户名的文件 26，`git diff --check` 干净。读过的：`slot.rs` 逐条照 P8——`Armed` 的字段、`arm` 在一切锁之外登记回调、回调认出 `Rejected(Drop)`、`answered` 的三支、`Drop` 的 `Waiting` 改 `Nothing`、`try_commit` 的条件与结束的那一刻、`CaptureBody` 的字段次序与不实现 `Drop`、`done` 只做一次、锁的次序 `Armed` → 存储；`answer`、`failed`、`page`、`map_local`、`timed_out`、`Tail`、`LocalFile`、`Guard` 的函数体没有改动。第 34 行 ①（`Fake::capture` 随用例进提交 (b)）与 ②（`Fake::dial` 补 `set_peer`：现有用例里读 `trail().peer()` 的只有 `engine.rs:4463` 一条，断言的是借来的连接）接受；第 33 行说提交 (a) 里解出 `Routed` 的两处用 `..`，(a) 里写的是 `capture: _`、(b) 才改成 `..`，以本行为准。报告的顾虑 1、3、4 接受（1 照 P9 ④ 由任务 9a 登记）。**要改的**：① `captured_url` 插在了 `wants_upgrade` 的文档注释与 `fn wants_upgrade` 之间（`engine.rs:2338-2355`），那三行注释成了 `captured_url` 的文档注释的开头，`wants_upgrade` 没有了注释；② `arm` 之前的 `req.headers().clone()` 与 `headers::strip` 写在闭包外面，不抓的请求也做一遍——P8 是"`capture` 是 `None` … `head` 不调"，派工写的是在闭包里；③ `arm` 放在了 `token` 的检查之后，P9 ② 写的是之前：在求规则时被 `kill` 或随连接停下的请求有记录、没有条目（第 33 ～ 38 行没有记这一处）；④ 第 35 行与报告的顾虑 2 说重置之后"hyper 在流被重置之后并不立刻丢它"（正文），照源码不成立：hyper 1.11.1 的 `PipeToSendStream::poll` 每一轮先 `poll_reset(cx)`（`src/proto/h2/mod.rs:145-155`），h2 0.4.19 的 `recv_reset` 调 `notify_send`（`src/proto/streams/recv.rs:929`），流被重置时这个流的任务被叫醒、`H2Stream` 收场（`src/proto/h2/server.rs:530-532`）并丢下正文。用 `stalled_origin` 时红的那几次，是应答头先到：`hang` 那个任务已经带着 `Response`（连同它的正文）结束，`abort()` 对结束了的任务不起作用，`JoinHandle` 一直握着正文，客户端并没有重置这个流；条目要等 `rig_h2` 的 5 秒空闲时限结束正文才来，正好在 `one_entry` 的 5 秒之后。用例 14 改用不答的源站是对的（它要的就是"应答之前"）；正文中途在 HTTP/2 上被重置的那一种另补一条用例钉住 | 本提交 |
| 40 | 3 | 修正轮 1（照第 39 行）：① `captured_url` 连同它自己的四行文档注释整段挪到 `fn wants_upgrade` 的右花括号之后（中间空一行），`wants_upgrade` 的三行注释回到紧贴它的位置，字一个不改。② `arm` 挪到 `token` 的检查之前，`req.headers().clone()` 与 `headers::strip` 放进闭包（不抓的请求不调 `head`、不复制头）。③ 两处都是 `crates/rurge-http/src/engine.rs` 的 `serve` 里那一段，`cargo fmt` 之后的形状以 rustfmt 为准；被 `kill` 的时刻正好落在 `route` 与检查之间那一种今天的 `Fake` 造不出来，所以没有新用例（报告里写了）。④ 在用例 14 之后补 `an_http2_stream_reset_mid_body_is_captured_with_what_went_by`：`stalled_origin` 的十个字节里读五个就 `drop(resp)`，空闲时限给 60 秒，条目在 `one_entry` 的 5 秒之内就到了（应答 200、正文 `xxxxx`、`size == 5`、`truncated` 是 `None`、`Failed`），这一条也钉住本行的前半句。**本行覆盖第 35 行里 "hyper 在流被重置之后并不立刻丢它" 那一句的原因与报告的顾虑 2**：照第 39 行 ④，hyper 1.11.1 的 `PipeToSendStream::poll` 每一轮先 `poll_reset`、h2 0.4.19 的 `recv_reset` 调 `notify_send`，流被重置时任务被叫醒、`H2Stream` 收场并丢下正文；用 `stalled_origin` 时红的那几次是应答头先到（`hang` 任务已带着 `Response` 结束，`abort()` 不起作用，`JoinHandle` 握着正文，客户端并没有重置那条流），条目等的是 `rig_h2` 的 5 秒空闲时限。用例 14 改用不答的源站是对的，不动。⑤ 循环：新那一条先 `cargo test -j 8 --workspace --lib --no-run` 编好，再直接跑 `target\debug\deps\rurge_http-c459489a1f84fc74.exe --exact engine::tests::an_http2_stream_reset_mid_body_is_captured_with_what_went_by` 二十次，每一次的 `test result:` 那一行与退出码写进 `loop-task-3-fix-1.log`：二十次都是 exit=0、`ok. 1 passed; 0 failed`。⑥ 门禁：`df -h /d` 剩 73G（跑之前）；`cargo fmt --all --check` exit=0（`gate-task-3-fix1-fmt.log`）、`cargo clippy --all-targets -- -D warnings` exit=0（`gate-task-3-fix1-clippy.log`）、`cargo test -j 8 --workspace --no-fail-fast` exit=0（`gate-task-3-fix1.log`），**92 个结果行、5270 通过、0 失败、13 忽略**（第 38 行的 5269 加新的一条）；这是一台 Windows 11 x86-64 开发机上的一次运行，不是 CI。⑦ `git diff --stat`：`crates/rurge-http/src/engine.rs` `+72 −21`、计划文件 `+1 −0`，两个 | 本提交 |
| 41 | 3 | 协调者核过修正轮 1（`551728e`，第 40 行）：四处都照第 39 行改了——`git diff 10bdb5d..551728e -- crates/rurge-http/src/engine.rs` 里 `wants_upgrade` 的注释与函数体一行也不出现（Pi 挪的是 `wants_upgrade`、不是 `captured_url`，结果相同：它的三行注释紧贴 `fn wants_upgrade`，`captured_url` 连同注释接在它后面），`arm` 在 `token` 的检查之前、复制与去掉逐跳头在闭包里；新的 `an_http2_stream_reset_mid_body_is_captured_with_what_went_by` 在空闲时限 60 秒下 5 秒之内得到条目（应答 200、正文五个字节、`Failed`），二十次全过（`loop-task-3-fix-1.log` 每一次都记了 `test result:` 那一行；文件只跑这一条，每行不带名字，`274 filtered out` 对得上），第 39 行 ④ 的读法由它证实。门禁 92 / 5270 / 0 / 13（一台机器上的一次运行，不是 CI；日志里只有现有用例预期的 `cannot bind http-api … (os error 10048)` 一行），`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 的退出码都是 0，计划的两张表只多第 40 行，带本机账户名的文件 26，`git diff --check` 干净。报告的顾虑（`arm` 在检查之前没有用例钉住）：协调者的裁定，不给 `Fake` 加钩子——只有在 `route` 与检查之间被 `kill` 或随连接停下的请求才走这一支，次序是 P9 ② 定的，这一处照实记在这里，任务 9b 写进 `docs/status.md` 的"没有用例钉住"。**任务 3 过了** | 本提交 |
| 42 | 4 | 照 P10 做：`EngineShared.capture`（`CaptureStore::new(Limits::default())`）；`Engine` 多 `capture_on`（初值假）、`capture_on()`、`set_capture()`（照 `set_rewrite`，写 `s.features.capture`），`restore_switches` 加 `features.capture`，另有 `capture()` 与 `capture_for()`（`handle.capture_session()`，没有而开关开着是 `DEFAULT_SESSION`）；`always_capture(rules, index)` 放在 `rule_raw` 旁边（同样 `binary_search_by_key`，去首尾空白、空的是 `None`）；`choose_policy_with` 在 `set_rule` 那一句之后填槽，三处早退不填；`lib.rs` 多 `pub use rurge_http::capture;`（比 P10 说的多三行文档注释：`ApiTls` 那一行没有注释可照，记在这里）；`crates/rurge-engine/src/http.rs` 的两处 `capture: None` 换成 `capture: self.engine.capture_for(handle)`，`Refused` 那一支在 `settle` 之后取。新文件 `crates/rurge-engine/tests/capture.rs` 九条端到端用例（P14），开头一段英文的模块注释。RED 一次：`cargo test -j 8 --workspace --test capture`（`red-task-4.log`）是 1 通过、8 失败——`nothing_is_captured_while_the_switch_is_off` 通过（开关缺省关，本来什么都不存），其余七条在 `wait_until("the entry is stored")` 上超时（`the_direct_mode_reads_no_rule_and_so_no_always_capture` 是它的前半段过、后半段超时），`the_capture_switch_is_kept_in_state_json_and_restored` 在 restore 之后那一句 `the file's word is taken` 上红。GREEN 一次：同一条命令九条全过（`green-task-4.log`）。门禁：`cargo fmt --all --check` exit=0（`gate-task-4-fmt.log`；第一次因为 `always_capture` 的签名与 `a_logical_rule_names_the_session_of_its_line` 的 `format!` 换行而红，`cargo fmt --all` 之后 exit=0）、`cargo clippy --all-targets -- -D warnings` exit=0（`gate-task-4-clippy.log`）、`cargo test -j 8 --workspace --no-fail-fast` 的 `rewrite` 测试二进制以 `STATUS_HEAP_CORRUPTION`（0xc0000374）退出（本机已知的问题，`CLAUDE.md`「这台开发机」），单独重跑 `cargo test -j 8 --workspace --test rewrite` exit=0、50 条全过（`rerun-task-4-rewrite.log`）；合起来 **93 个结果行、5279 通过、0 失败、13 忽略**（第 40 行的 92 / 5270 / 0 / 13，加新文件的九条与它那一行结果）；这是一台 Windows 11 x86-64 开发机上的一次运行，不是 CI。`df -h /d` 跑之前剩 73G。 | 本提交 |
| 43 | 4 | 循环二十次：九条用例各自单独跑二十次——先 `cargo test -j 8 --workspace --test capture` 编好，再依次跑 `target\debug\deps\capture-b4a3ced5c82b5b22.exe --exact <用例的全名>`（脚本 `.superpowers/…/loop-task-4.ps1`），每次套 120 秒的时限，每一次的退出码与 `test result:` 那一行都写进 `loop-task-4.log`：180 行，每行都带 `test result:`，九条各一行 `TOTAL ok=20 bad=0`——180 次全部 exit=0、`ok. 1 passed; 0 failed`，没有一次超时。 | 本提交 |
| 44 | 4 | `always_capture_on_final_captures_too` 用 `harness_edited` 改 `cfg.rules` 的最后一条，而不是在 `rules` 里写一行 `FINAL,DIRECT,always-capture=last`：共用的 `Profile::text` 总在用例的 `rules` 之后补一行 `FINAL,DIRECT`，`RuleEngine::build` 编到第一个 `FINAL` 为止、把最后一个放在表尾，两行 `FINAL` 时生效的是后补的那一行——写在 `rules` 里的那一行到不了引擎。用例的文档注释里写了这一段。 | 本提交 |
| 45 | 4 | 协调者的评审（任务 4）：大部分对，两处注释说得不对，要改（修正轮 1，见其后一行）。核过：门禁 93 / 5279 / 0 / 13（一台机器上的一次运行，不是 CI；全量里 `rewrite` 以 `STATUS_HEAP_CORRUPTION` 退出，单独重跑 50 条全过，合起来数；日志里错误与告警的行与第 40 行那一次的全量逐行相同），`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 的退出码都是 0，计划的两张表只多不少（第 42 ～ 44 行），新用例 9 条、名字照任务全文，RED 1 过 8 红（过的是开关关着什么也不存的那一条，没有实现时本来就该过；七条红在等条目的 `wait_until`，第九条红在 restore 之后），GREEN 9 条，九条各循环二十次（180 次，每一次都记了 `test result:` 那一行），提交只动了六个文件、`Cargo.toml` 与 `Cargo.lock` 没有动，带本机账户名的文件 26，`git diff --check` 干净。读过的：P10 逐条——`EngineShared.capture` 与它的文档注释、`capture_on` 初值假且与 `rewrite_on` 并排、`set_capture` 照 `set_rewrite` 写 `features.capture`、`restore_switches` 读回、`capture()`、`capture_for` 的次序（规则给的名字 → 开关开着是 `default` → `None`）、`always_capture` 去首尾空白而空的是 `None`（`W0012` 在 `crates/rurge-config/src/config.rs:840-853` 报，`an_empty_always_capture_is_ignored_with_w0012` 钉着）、只在 `set_rule` 之后填槽而三处早退不填、两个分支都给 `capture`。第 44 行接受（`Profile::text` 在用例的规则之后补一行 `FINAL,DIRECT`，`crates/rurge-engine/tests/common/mod.rs:219`；`RuleEngine::build` 用最后一行 `FINAL`，`crates/rurge-rules/src/engine.rs:216-219`）。第 42 行说 `lib.rs` 多了"三行文档注释"，实际是两行 `///`（另一行是 `pub use` 本身），以本行为准；这两行注释接受：P10 说"照 `ApiTls` 那一行写"，可那一行上面没有注释，是计划写错了，注释说的属实（`rurge-api` 经 `rurge_engine::ApiTls` 用它，`crates/rurge-api/src/tls.rs:24`）。报告的顾虑：① 不是 HTTP 请求的会话也填槽、没有人读——设计第 5.3 节本来如此，接受；② `Capture` 用结构体字面量建——字段是任务 2 公开的，接受；③ 全局代理的早退没有用例——任务全文只要直连那一条，三处早退共用不求规则这一点，接受，照实记在这里；④ `rewrite` 的重跑——照惯例合起来数，接受。**要改的**：① `choose_policy_with` 里新加的注释说三处早退的请求"the entry goes to the default session"，这只在开关开着时成立，开关关着时它们不被抓（设计第 5.2 节"`always-capture` 不起作用；开关照样起作用"，`the_direct_mode_reads_no_rule_and_so_no_always_capture` 的前半段钉着）；② `crates/rurge-engine/src/http.rs` 里 `Refused` 那一支的注释说"REJECT answers without a dial and is captured too, so the name is read after `settle`"，照代码不成立：`route_session` 只交回 `Refused::Fail`（`engine.rs:1781` 托管配置要求更新、`:1789` DNS 查找失败、`:1803` `evaluate-before-use` 组求值失败），`Refused::Reject` 只在 `dial_route` 里出现（`:1958`、`:1963`），REJECT 一族是经 `Go` 那一支、拨号时才被拒（`M3-D27`，设计第 5.3 节）；"after `settle`"也说不出因果——`settle` 只改句柄上的错误文字（`:1756-1762`），与抓包会话的名字无关。这一句的误导一半在计划：P10 把"REJECT 一族照样带着"写在了 `Refused` 那一支下面。改法：`Refused` 那一支的注释说它实际接住的三种拒绝，REJECT 的那一句挪到 `Go` 那一支（与它上面"a request routed to a REJECT keeps no rewrite at all"对照）。引擎这一层 REJECT 的请求被抓，由任务 8 的 `a_request_a_reject_rule_refuses_is_captured` 钉住 | 本提交 |
| 46 | 4 | 修正轮 1（照第 45 行）：只改三段注释，代码与用例一个字不动。① `crates/rurge-engine/src/engine.rs:1047` 填槽那一段：把 "the entry goes to the default session" 换成 "their requests are captured only while the switch is on, into the default session"——开关关着时早退的请求不被抓（设计第 5.2 节：`always-capture` 不起作用、开关照样起作用；任务 4 的 `the_direct_mode_reads_no_rule_and_so_no_always_capture` 前半段钉的就是这一点）。② `crates/rurge-engine/src/http.rs:166` `Routed::Go` 那一支：补一句 "unlike the rewrite, a request routed to a REJECT keeps it"——`route_session` 只交回 `Refused::Fail`（`engine.rs` 里三处：托管配置要求更新、DNS 查找失败、`evaluate-before-use` 组求值失败），`Refused::Reject` 只在 `dial_route` 里出现，所以 REJECT 一族走的是 `Go` 那一支、拨号时才被拒（`M3-D27`）。③ `crates/rurge-engine/src/http.rs:179` `Routed::Refused` 那一支：换成 "the same for a request its routing refused"，列出那三种；删掉 "REJECT answers without a dial" 与 "read after `settle`"——`settle` 只改句柄上的错误文字，与抓包会话的名字无关，那句因果说不通（错有一半在计划的 P10，它把 "REJECT 一族照样带着" 写在了 `Refused` 这一支下）。改完的行都不超过 80 列。`git grep -n -F -e 'goes to the default session' -e 'the same capture a refusal keeps' -e 'read after `settle`' -- crates/rurge-engine/src/` 一行也不输出。跑了 `cargo fmt --all --check` exit=0（`gate-task-4-fix1-fmt.log`）、`cargo clippy --all-targets -- -D warnings` exit=0（`gate-task-4-fix1-clippy.log`）、`cargo test -j 8 --workspace --test capture` exit=0、九条全过（`gate-task-4-fix1-test.log`）；全量门禁不跑（协调者的裁定，这一轮只动注释）。`git diff --stat` 是三个文件：`crates/rurge-engine/src/engine.rs` `+3 −2`、`crates/rurge-engine/src/http.rs` `+8 −4`、计划文件 `+1 −0`。 | 本提交 |
| 47 | 4 | 协调者核过修正轮 1（`3e76e78`，第 46 行）：三段注释与第 45 行定的逐字相同——`engine.rs` 里填槽那一段说早退的请求只在开关开着时进 `default`，`http.rs` 里 `Go` 那一支补了 REJECT 留着 `capture`、拨号时才被拒（`M3-D27`），`Refused` 那一支说它接住的三种拒绝；`settle` 与两处 `capture_for` 没有动，代码与用例一个字没改。`git grep` 查那三句旧话没有输出，`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 的退出码都是 0，`--test capture` 九条全过；只改注释，全量不跑，全量的数仍是第 45 行的 93 / 5279 / 0 / 13（一台机器上的一次运行，不是 CI）。计划的两张表只多第 46 行，带本机账户名的文件 26，`git diff --check` 干净。**任务 4 过了** | 本提交 |
| 48 | 5 | `crates/rurge-api/src/routes/features.rs`：`get_feature` 与 `set_feature` 各加一支 `capture`（读 `Engine::capture_on()`，切换调 `set_capture` 并把新状态写进 `state.json`，INFO 照原文 P11 的 `capture switched via http-api`、字段 `enabled`，写法照 `rewrite` 那一支），模块的文档注释加上 "from phase 4 / M4a `capture`"（行首的三行注释也重排了断行）。新用例 `the_capture_switch_is_read_switched_and_kept`（照 `the_rewrite_switch_is_read_switched_and_kept`：缺省 `false`、`POST` 打开之后 `GET` 是 `true` 而 `state.json` 的 `features.capture` 是 `true`、再关掉同样）。**P16 点名的现有用例照计划改了**：`features_collections_stop_and_unknown_paths` 拿 `capture` 当"没有生效的开关"的那两处换成 `scripting`（`GET` 是 `{"enabled":false}`、`POST` 是 501），说明哪几个开关生效的那一句加上 `capture`；`capture` 的读与切换由新那一条钉住。RED：`cargo test -j 8 --workspace --test api -- capture_switch features_collections`（`red-task-5-switch.log`，exit=101）1 通过、1 失败——新那一条红在 `POST /v1/features/capture` 答 `(501, {"error":"feature `capture` is not available in this phase"})`，`features_collections_stop_and_unknown_paths` 已经照改过、绿。GREEN：同一条命令绿（`green-task-5-switch.log`）。`cargo fmt --all --check` exit=0（`commit-a-fmt.log`）、`cargo clippy --all-targets -- -D warnings` exit=0（`commit-a-clippy.log`）。 | 本提交 |
| 49 | 5 | 两处 `Cargo.toml` 的改动照 P2 与派工：`crates/rurge-api/Cargo.toml` 的 `[dependencies]` 在 `axum` 之后加 `base64.workspace = true`，`[dev-dependencies]` 的 `rurge-http.workspace = true` 换成 `rurge-http = { workspace = true, features = ["testing"] }`、另加一行 `flate2.workspace = true`（**协调者的裁定**，派任务 5 时加的：用例要当场压 gzip；它已在锁文件里，只多一条只供测试的依赖边、不多包；这一行覆盖 `dispatch-rules.md`「只改计划写明的两处 `Cargo.toml`」在 `rurge-api` 开发依赖上的那一句）。改完先跑 `cargo check --offline --workspace --all-targets`，过了（不需要联网）。`Cargo.lock` 的 `[[package]]` 仍是 520 个，`git diff Cargo.lock` 只有 `rurge-api` 的 `dependencies` 列表多两行 `"base64",` 与 `"flate2",`（特性不进锁文件）。 | 本提交 |
| 50 | 5 | `crates/rurge-api/src/routes/capture.rs`（375 行）按 P11 做：`status` / `sessions` / `session` / `entry` / `clear` 五个处理函数，路由加在 `lib.rs` 的 `/v1/rurge/http` 之后（axum 0.8 的 `{name}`、`{id}` 写法），`routes/mod.rs` 加 `pub mod capture;`。非要紧的三处自主定：① 一条的详情与摘要用 `#[derive(Serialize)]` 的结构体（`StatusJson`、`SessionJson`、`SummaryJson`、`TimingsJson`，后两个的 `started_ms` 等靠 `#[serde(rename_all = "camelCase")]` 变 `startedMs`），不用 `json!`，键名逐个照设计第 7.1、7.3、7.5 节，四个头遮住的名单是私有常量 `MASKED`（值写 `***`，存储里原样）；② 头拿不进 `HeaderMap` 这个类型（`rurge-api` 的依赖里没有 `http`，计划也不加）：`headers_json` 收一个 `Iterator<Item = (String, String)>`，调用处从 `head.headers.iter()` 现拼；③ `?decode` 先读、再接 `id`，两处的 400 次序与 P11 列的次序不同，没有用例要求先读 `id`。一条没有应答时 `responseBodySize` 取应答正文的 `size`（设计第 7.3 节），用例里的夹具因此把没有应答那一条的 `response_body` 也设成 `Body::empty()`（第一次跑时红在这一条上，`responseBodySize` 是 2）。 | 本提交 |
| 51 | 5 | 用例：`crates/rurge-api/tests/api.rs` 多 612 行——一条造条目的夹具 `capture_entry(id, session)`（头、URL、正文写死，字段都是 `pub`）与 8 条用例（`the_capture_status_says_the_limits_and_the_counts`、`capture_sessions_are_listed_by_name`、`a_capture_session_lists_its_entries_newest_first`、`an_entry_is_shown_whole_with_four_headers_masked`、`entry_ids_are_numbers_of_entries_that_are_there`、`decode_undoes_gzip_and_says_why_it_cannot`、`clearing_takes_one_session_or_all`、`odd_session_names_are_reached_through_the_path`），名字全照任务全文。**执行者的裁定**：这些用例要的 `HeaderMap`、`Method`、`StatusCode`、`Version`、`header` 四个名字从已经有的开发依赖 `hyper` 拿（`hyper` 1.11.1 的 `lib.rs:121` 把 `http` 的这几项原样重新导出），不为用例加 `http` 依赖；gzip 在用例里用 `flate2::write::GzEncoder` 当场压，截断的 gzip 取压出来字节的前一半。RED 两次：第 1 步 `cargo test -j 8 --workspace --test api -- capture_switch features_collections`（`red-task-5-switch.log`）1 通过、1 失败；第 5 步 `cargo test -j 8 --workspace --test api`（`red-task-5-endpoints.log`，第一次跑时这个测试二进制以 `STATUS_HEAP_CORRUPTION` 退出、延后一行结果，按 `CLAUDE.md`「这台开发机」单独重跑一次）68 通过、8 失败——八条新的都在 501 上红（`{"error":"capture is not available yet"}`）。GREEN：同一条命令 76 条全过（`green-task-5-endpoints.log`）。这九条用例没有等待也没有并发（条目用 `put` 直接存进去），不循环二十次。门禁：`cargo fmt --all --check` exit=0（`gate-task-5-fmt.log`）、`cargo clippy --all-targets -- -D warnings` exit=0（`gate-task-5-clippy.log`）、`cargo test -j 8 --workspace --no-fail-fast` 的 `outbounds_wireguard` 测试二进制以 `STATUS_HEAP_CORRUPTION`（0xc0000374，本机已知的问题）退出，单独重跑 `cargo test -j 8 --workspace --test outbounds_wireguard` exit=0、16 条全过（`rerun-task-5-wireguard.log`）；合起来 **93 个结果行、5288 通过、0 失败、13 忽略**（第 42 行那一次的 93 / 5279 / 0 / 13，加本任务的 9 条），这是一台 Windows 11 x86-64 开发机上的一次运行、不是 CI。`df -h /d` 跑之前剩 70G；`rurge.exe` 的残留进程一个也没有。 | 本提交 |
| 52 | 5 | 协调者的评审（任务 5）：大部分对，四处要改（修正轮 1，见其后一行）。核过：门禁 93 / 5288 / 0 / 13（一台机器上的一次运行，不是 CI；全量里 `outbounds_wireguard` 以 `STATUS_HEAP_CORRUPTION` 退出，单独重跑 16 条全过，合起来数；日志里错误与告警的行与第 42 行那一次的全量逐行相同），`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 的退出码都是 0，计划的两张表只多不少（第 48 ～ 51 行），新用例 9 条、名字照任务全文，两批 RED（1 过 1 红、68 过 8 红，都红在 501）与 GREEN 的日志都在，`Cargo.lock` 仍是 520 个包、只多 `rurge-api` 的 `"base64",` 与 `"flate2",` 两行，`rurge-api` 正式依赖里的工作区 crate 仍只有 `rurge-engine` 与 `rurge-config`，带本机账户名的文件 26，`git diff --check` 干净。读过的：`features.rs` 两支与模块注释照 P11，`features_collections_stop_and_unknown_paths` 照 P16 的表（换成 `scripting`、文字加上 `capture`）；`routes/capture.rs` 照 P11 与设计第 7.1 ～ 7.6 节——五条路由在 `/v1/rurge/http` 之后，键名逐个对得上，`limit` 照 `recent`（`requests.rs` 的 `DEFAULT_LIMIT` 是私有的，另写一个接受），两个 404 与两个 400 的字句照派工，`BAD_BODY` 与计划第 789 行逐字相同，`requested_session` 与 `profiles.rs` 的 `requested_name` 同一写法，`decode` 的三支照 P11、两句原因与计划第 768 ～ 770 行一致，处理函数拿到 `Arc<Entry>` 之后才序列化，`capture.rs` 里没有 `tracing` 也没有 `Debug`。报告的顾虑：① `?decode` 先于 `id` 读——P11 与设计都没有定次序，接受；② `bytes` 不含进行中的预留——设计第 7.2 节本来如此，接受；③ 条目都被挤掉的会话答 404——设计第 7.2 节"只有有条目的会话"，接受；④ 用例经 `put` 不走引擎——计划这样定的，引擎到 API 的衔接由任务 8 的端到端用例看，接受。第 50 行 ③ 接受。第 51 行说用例从 `hyper` 拿"四个名字"，后面列的是五个（`HeaderMap`、`Method`、`StatusCode`、`Version`、`header`），以本行为准。另记：这一次全量之前 D: 剩 70G，之后 46G——`rurge-api` 的开发依赖开了 `rurge-http` 的 `testing`，每次 `cargo test --workspace` 与 `cargo clippy --all-targets` 都带着它编，`rurge-http` 与依赖它的 crate 各多编了一份（`target/debug/deps` 里 18:53 新出一份 `librurge_http-*.rlib` 与 `librurge_engine-*.rlib`），旧的那份留在 `target/` 里；之后的任务不再改特性。**要改的**：① 第 50 行 ② 说 `rurge-api` 拿不到 `HeaderMap` 这个类型（"依赖里没有 `http`"），不成立：axum 0.8.9 的 `src/lib.rs:508` 是 `pub use http;`，生产代码写 `axum::http::HeaderMap` 就有；绕开它的写法让两个调用处各自把每个头的值先转成 `String` 再遮，凭据头的值也被复制出来一次，任务 6 的 HAR 要共用这个函数（任务 6 的派工），照现在的签名每个调用处都得再抄一遍那段转换——改成 `headers_json(headers: &HeaderMap) -> Value`，先看名字、遮住的不转，`text_of` 随之去掉；② 任务全文写明"`totalMs` 是 `total` 向下取整的毫秒"，P11 也写"`as_millis()` 向下取整"，可夹具里的时长都是整毫秒（3、20、12、35），换成四舍五入也没有一条会红——两条用例各给一个不整的时长（`35_999` 与 `12_999` 微秒），断言照旧是 35 与 12，再改一行看它们红；③ `a_capture_session_lists_its_entries_newest_first` 里的注释说最新那一条"was refused without an answer"，它设的是 `Outcome::Failed`；④ `routes/capture.rs` 的 `use serde::Deserialize;` 与 `use serde::Serialize;` 分两行，同目录别的路由文件写 `use serde::{Deserialize, Serialize};` | 本提交 |
| 53 | 5 | 修正轮 1（照第 52 行）：四处都照派工改了。① `crates/rurge-api/src/routes/capture.rs` 多 `use axum::http::HeaderMap;`，`headers_json` 改成收 `&HeaderMap`（头在函数里一次转成 name / value，凭据头的值不再先转成 `String` 再遮；`text_of` 删掉，两个调用处各换成一行 `headers_json(&head.headers)`），私有可见性不变、`pub(crate)` 是任务 6 的事。**本行覆盖第 50 行 ② 里 "`rurge-api` 的依赖里没有 `http`" 那一句的理由**（照第 52 行 ①）：axum 0.8.9 的 `src/lib.rs:508` 是 `pub use http;`，生产代码写 `axum::http::HeaderMap` 就有，锁文件里只有一个 `http` 1.5.0、与 `capture::Head` 的 `headers` 同一个类型。② `use serde::Deserialize;` 与 `use serde::Serialize;` 合成 `use serde::{Deserialize, Serialize};`（同目录其它路由文件都这样写）。③ 两条用例各给一个不整的时长、断言一个字不改：`a_capture_session_lists_its_entries_newest_first` 把 `newest.timings.total` 设成 35.999 ms，`an_entry_is_shown_whole_with_four_headers_masked` 把 `receive` 设成 12.999 ms、`total` 设成 35.999 ms。④ `a_capture_session_lists_its_entries_newest_first` 的注释从 "was refused without an answer" 改成 "failed without an answer"（那条设的是 `Outcome::Failed`）。**改一行看它红**：把 `millis` 的函数体临时改成 `((d.as_micros() + 500) / 1000) as u64`（四舍五入），`cargo test -j 8 --workspace --test api -- a_capture_session_lists_its_entries_newest_first an_entry_is_shown_whole_with_four_headers_masked`（`red-task-5-fix-1.log`）两条都红——一条的 `totalMs` 是 36 对 35，另一条的 `receiveMs` 是 13 对 12、`totalMs` 是 36 对 35；改回 `d.as_millis() as u64` 后同一条命令两条全绿（`green-task-5-fix-1.log`），`git diff -- crates/rurge-api/src/routes/capture.rs` 里不再出现 `millis` 与 `as_micros`。`git grep -n -F -e 'text_of' -e 'impl Iterator<Item = (String, String)>' -e 'was refused without an answer' -- crates/rurge-api/` 一行也不输出。门禁：`df -h /d` 跑之前剩 46G；`cargo fmt --all --check` exit=0（`gate-task-5-fix1-fmt.log`）、`cargo clippy --all-targets -- -D warnings` exit=0（`gate-task-5-fix1-clippy.log`）、`cargo test -j 8 --workspace --no-fail-fast` exit=0（`gate-task-5-fix1.log`），**93 个结果行、5288 通过、0 失败、13 忽略**（第 51 行的数不变，这一轮不加用例），这是一台 Windows 11 x86-64 开发机上的一次运行、不是 CI，没有重跑、`rurge.exe` 的残留进程一个也没有。`git diff --stat`：`crates/rurge-api/src/routes/capture.rs` `+12 −19`、`crates/rurge-api/tests/api.rs` `+6 −1`、计划文件 `+1 −0`。 | 本提交 |
| 54 | 5 | 协调者核过修正轮 1（`c09717b`，第 53 行）：四处都照第 52 行改了——`headers_json(headers: &HeaderMap)` 经 `axum::http::HeaderMap`，先看名字、四个凭据头的值不转，`text_of` 去掉、两个调用处各是一行；`use serde::{Deserialize, Serialize};`；两条用例各多一个不整的时长（35.999 ms、12.999 ms），断言没有改；那一条注释改成"failed"。改一行看它红的日志在：`millis` 改成四舍五入时两条都红（`red-task-5-fix-1.log`，36 对 35、13 对 12），改回之后两条都绿（`green-task-5-fix-1.log`），`git diff d491a43 c09717b` 里没有 `millis` 那一行。门禁 93 / 5288 / 0 / 13（一台机器上的一次运行，不是 CI；没有重跑，日志里错误与告警的行与第 51 行那一次的全量逐行相同），`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 的退出码都是 0；跑之前与之后 D: 都剩 46G。计划的两张表只多第 53 行，带本机账户名的文件 26，`git diff --check` 干净，`git grep` 查那三处旧写法没有输出。**任务 5 过了** | 本提交 |
| 55 | 6 | 提交 (a)。`crates/rurge-api/Cargo.toml` 的正式依赖加 `futures-util.workspace = true`（放在 `base64` 之后）：`cargo check --offline --workspace --all-targets` exit=0，`Cargo.lock` 仍是 520 个 `[[package]]`、`git diff Cargo.lock` 只有 `rurge-api` 的 `dependencies` 多一行 `"futures-util",`。新的 `crates/rurge-api/src/har.rs`（536 行：生产代码 289 行、用例 247 行）：模块注释、`HarTimings`、`timings`、`entry`、`document` 照 P12。`lib.rs` 加 `mod har;`（按字母序放在 `error` 与 `linger` 之间），暂带 `#[allow(dead_code)]` 与注释 `// the handler of the phase 4 / M4a plan's task 6 (b) uses it`，**提交 (b) 去掉**。`routes/capture.rs` 里任务 5 写的**三个函数改成 `pub(crate)`**：`headers_json`（遮住只在它里面，`MASKED` 不另放出来）、`outcome_name`、`reject_kind`；`millis` 不共用（HAR 的 `ms` 交 `i64`，P12）。**执行者的裁定**（与派工的字面不完全一样的四处）：① `document` 的一条一块用 `serde_json::to_writer` 写进一个 `Vec`、第二条起先 `push(b',')`，不是 `to_vec` 之后 `insert(0, b',')`——`insert` 要把整条的 JSON 再挪一遍（一个会话的正文可能几 MiB），写出仍是一条一块、仍由 `map` 惰性地做；② 应答的 `content.compression` 只在解得开时写（设计第 8.2 节的表只在那一行给 `compression`）；③ 没有应答的 `content` 只有 `size` 与 `mimeType` 两个键（照设计第 8.2 节那句 `{"size":0,"mimeType":""}`），有应答而正文为空的照常写 `text`；④ 用例里造条目的夹具是 `har.rs` 测试模块自己的私有函数 `kept_entry`。RED：`cargo test -j 8 --workspace --lib -- har::tests::`，两条都红（`red-task-6-har.log`）——`har_timings_add_up_to_the_total` 的第一条是 `left: (-1, -1, -1, -1, 0, -1, 0)` 对 `right: (-1, 10, 20, -1, 0, 20, 5)`（空壳全交 `-1`），`an_export_holds_its_entries_while_they_are_cleared` 在 `serde_json::from_slice` 上 `a JSON document: Error("EOF while parsing a value", line: 1, column: 0)`（空壳的 `document` 交 `Body::empty()`）。GREEN：同一条命令两条全过（`green-task-6-har.log`）。第一次实现之后那条导出用例还红过一次，原因是我漏了 P12 的最后一块 `]}}`，补上即绿。 | 本提交 |
| 56 | 6 | 提交 (b)。`routes/capture.rs` 加 `har` 处理函数（先答 501 `HAR is not available yet`，第 7 步换成真的），`lib.rs` 在 `/v1/rurge/capture/sessions/{name}` 之后加第六条路由 `GET /v1/rurge/capture/sessions/{name}/har`，去掉第 55 行的 `#[allow(dead_code)]`。`crates/rurge-api/tests/api.rs` 多 443 行：三个小夹具（`har_of_a`、`har_entry`）与任务全文要的五条用例（`a_session_is_exported_as_har_oldest_first`、`har_bodies_are_text_or_base64_and_say_what_was_left_out`、`har_says_no_response_as_mitmproxy_does`、`har_masks_the_four_headers_and_leaves_cookies_empty`、`har_splits_query_strings_and_gives_connect_none`），gzip 与截断的 gzip 照任务 5 用 `flate2::write::GzEncoder` 当场压。处理函数照 P12：先取 `session(name)` 的快照、空的答 404（与任务 5 同一句 `unknown capture session \`{name}\``），再交 `([(CONTENT_TYPE, "application/json")], document(entries, store.limits().body))`。**执行者的裁定**：① `br` 那一条的用例把存下的字节写成不是 UTF-8 的 `[0x1b, 0x9c, 0xff, 0x00, 0x80]`——任务全文说它的 `text` 是 Base64，而设计第 8.2 节写的是"`text` 是 UTF-8 时照写，否则 Base64"，只有存下的字节不是 UTF-8 时两处才说的是同一件事；② 请求正文二进制那一条（第 7 条）的 `mimeType` 是空串，因为夹具的请求头里没有 `content-type`（`capture_entry` 的 `content-type` 在应答那一侧）；③ 处理函数的返回是 `ApiResult<impl IntoResponse>`，照 `profiles.rs` 的 `current`。RED：`cargo test -j 8 --workspace --test api -- har`，五条全红在 501 上（`red-task-6-api.log`，`{"error":"HAR is not available yet"}`）。GREEN 同一条命令五条全过（`green-task-6-api.log`）。五条用例没有等待也没有并发（条目用 `put` 直接存进去），不循环二十次。门禁：`df -h /d` 跑之前剩 44G；`cargo fmt --all --check` exit=0（`gate-task-6-fmt.log`）、`cargo clippy --all-targets -- -D warnings` exit=0（`gate-task-6-clippy.log`；中间红过一次 `clippy::needless_lifetimes`，改掉之后绿）、`cargo test -j 8 --workspace --no-fail-fast` exit=0（`gate-task-6.log`），**93 个结果行、5295 通过、0 失败、13 忽略**（第 53 行那一次的 93 / 5288 / 0 / 13，加本任务的 7 条），这是一台 Windows 11 x86-64 开发机上的一次运行、不是 CI，没有重跑、`rurge.exe` 的残留进程一个也没有。 | 本提交 |
| 57 | 6 | 协调者的评审（任务 6）：大部分对，七处要改（修正轮 1，见其后一行），都是注释、名字与写法，行为不变。核过：门禁 93 / 5295 / 0 / 13（一台机器上的一次运行，不是 CI；没有重跑，日志里错误与告警的行与第 53 行那一次的全量逐行相同），`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 的退出码都是 0，计划的两张表只多不少（第 55、56 行），新用例 7 条、名字照任务全文，两批 RED（单元用例 2 条都红在空壳交回的 `-1` 与 0、空的正文上；`api.rs` 5 条都红在 501）与 GREEN 的日志都在，`Cargo.lock` 仍是 520 个包、只多 `rurge-api` 的 `"futures-util",` 一行，带本机账户名的文件 26，`git diff --check` 干净。读过的：`har.rs` 全文——`timings` 照 P12 逐项（`reach`、`d` 取 `min`、`dd`、新拨与借来两支、`wait` 三支、减法都用 `saturating_sub`、`time` 是不是 `-1` 的各项之和）；`entry` 的键照设计第 8.2 节，`serverIPAddress`、`_rejectKind`、`_error` 没有时不写；`postData` 只在见过字节时有，文字与 Base64 的判断照设计第 8.2 节那一行（请求一侧截断的、带 `content-encoding` 的一律 Base64，应答一侧照第 7.5 节解开再判断，两侧不一样是设计定的）；没有应答的照 mitmproxy 的写法；`compression` 只在解得开时写；`comment` 截断的在前；`document` 用 `map` 惰性地一条一块，开头与 `]}}` 各一块，交给 `Body::from_stream`（判据"导出不先在内存里拼出整个文件"读代码可见）。`har` 处理函数先取快照、空的答 404，字句同任务 5；第六条路由在 `/sessions/{name}` 之后；共用的三个函数改成 `pub(crate)`，遮住仍只在 `headers_json` 一处。七条用例逐条对过任务全文的断言；`capture_entry` 的 `started` 是定值，第一条用例调它两次得同一个。第 55 行 ① ～ ④ 与第 56 行 ① ～ ③ 接受；第 56 行 ① 是对的：任务全文说 `br` 的 `text` 是 Base64，只有存下的字节不是 UTF-8 时才与设计第 8.2 节一致，是任务全文写得不全。第 56 行说"三个小夹具"，括号里列的是两个（`har_of_a`、`har_entry`），以本行为准。报告的顾虑：① `compression` 只在解得开时写——设计第 8.2 节的表如此，接受；② 没有应答的 `content` 只有两个键——设计原句，接受；③ `startedDateTime` 与各项向下取整之间差不到几毫秒——设计第 8.3 节接受，接受；④ `to_writer` 代替 `to_vec`——接受（失败的那一支见下面 ⑤）。**要改的**：① `har.rs` 的模块注释说逐条写出见 `phase 4 M4 design 8.3`，应是 8.4（8.3 是时长）——派工第 2 节写的就是 8.3，错在协调者；② `post_data` 的文档注释说正文"is no UTF-8"时照写文字，与代码相反，应是"is UTF-8"；③ 同一句说"which HAR 1.2 spells `_encoding` here"，不成立：HAR 1.2 的 `postData` 没有 `encoding`，`_encoding` 是 rurge 自己的、下划线开头的键（设计第 8.2 节那一行的括号）；④ 测试模块的 `seconds(n)` 交回 `Duration::from_millis(n)`，`seconds(30)` 是 30 毫秒，名不副实——改名为 `millis`；⑤ `document` 为 `serde_json::to_writer` 的失败写了一支退路（截掉、写 `null`），这个失败不会发生（`entry` 造的是 `Value`，写进内存里的 `Vec`），不为它写处理；本仓库对这种写 `expect`（`crates/rurge-engine/src/state.rs:121`、`:141`，本 crate 的 `auth.rs:45`）——改成 `expect`，退路与它的注释去掉；⑥ `query_string` 与 `header_text` 的签名里写全路径的 `axum::http::Method`、`axum::http::header::HeaderName`，测试模块的 `kept_entry` 写 `axum::http::header::CONTENT_TYPE`，而同一个文件的别的类型、本 crate 别的文件都在顶上 `use`——改成在顶上 `use`；⑦ `routes/capture.rs` 里 `har` 的文档注释说条目"are taken out of the store"，读起来像导出把条目从存储里拿走了，实际是取一份 `Arc` 的快照——改成说快照 | 本提交 |
| 58 | 6 | 修正轮 1（照第 57 行）：七处都照派工改了，行为不变、不加用例。① `crates/rurge-api/src/har.rs:6` 的模块注释里 `phase 4 M4 design 8.3` 改成 `8.4`（逐条写出在设计第 8.4 节，第 8.3 节是时长；`timings` 与时长用例注释里的 `8.3` 是对的、不动）。② `post_data` 的文档注释：`is no UTF-8` 改成 `is UTF-8`（代码是 `from_utf8` 成功才照写文字），`which HAR 1.2 spells \`_encoding\` here` 改成 `marked with rurge's own \`_encoding\`, since HAR 1.2 gives \`postData\` no \`encoding\``（HAR 1.2 的 `postData` 没有 `encoding`，`_encoding` 是 rurge 自己加的键、以下划线开头）。③ 测试里的 `fn seconds(n: u64) -> Duration` 改名 `fn millis`（它交回的是毫秒），同一模块里的 23 处调用（`kept_entry` 4 处、`har_timings_add_up_to_the_total` 19 处）跟着改；`millis_of` 不动。`super::*` 不会撞名。④ `document` 里 `to_writer` 的失败分支（`is_err()` 那一支，加三行注释与补 `null`）换成 `.expect("a JSON value serializes into memory")`：`entry` 造的是 `serde_json::Value`、写进的是内存里的 `Vec`，这个失败不会发生，本仓库对这种写法用 `expect`（`rurge-engine/src/state.rs:121`、`auth.rs:45`）；「One entry at a time」那三行注释留着，`enumerate` 与 `i` 不动。⑤ `Method` 与 `HeaderName` 在顶上 `use`：`use axum::http::{HeaderMap, Method};`、`use axum::http::header::{CONTENT_ENCODING, CONTENT_TYPE, HeaderName, LOCATION};`，于是 `query_string` 的 `&Method` / `Method::CONNECT`、`header_text` 的 `name: HeaderName`、测试里 `kept_entry` 的 `CONTENT_TYPE` 都写短名（测试模块自己那一行 `use` 不动）。⑥ `crates/rurge-api/src/routes/capture.rs` 的 `har` 文档注释：`The entries are taken out of the store and the lock let go` 改成 `A snapshot of the session's entries is taken, and the store's lock let go`（导出没有把条目从存储里拿走，存的是 `Arc` 的快照）。⑦ 派工列的白名单里的另一处也在 ② 与 ⑤ 里办完了（第 57 行把 ② 算作两条）。两条 `git grep` 检查一行也没输出（退出码 1）。聚焦的用例：`cargo test -j 8 --workspace --lib -- har::tests::` 2 条全过（`green-task-6-fix-1-har.log`）、`cargo test -j 8 --workspace --test api -- har` 5 条全过（`green-task-6-fix-1-api.log`），两条命令 exit=0。门禁：`df -h /d` 跑之前剩 43G；`cargo fmt --all --check` exit=0（`gate-task-6-fix1-fmt.log`）、`cargo clippy --all-targets -- -D warnings` exit=0（`gate-task-6-fix1-clippy.log`）、`cargo test -j 8 --workspace --no-fail-fast` 的 `pipeline` 测试二进制以 `STATUS_HEAP_CORRUPTION`（0xc0000374，本机已知的问题）退出（`gate-task-6-fix1.log`），单独重跑 `cargo test -j 8 --workspace --test pipeline` exit=0、78 条全过（`rerun-task-6-fix1-pipeline.log`）；合起来 **93 个结果行、5295 通过、0 失败、13 忽略**（第 56 行那一次的四个数，这一轮不加用例），这是一台 Windows 11 x86-64 开发机上的一次运行、不是 CI，`rurge.exe` 的残留进程一个也没有。`git diff --stat`：`crates/rurge-api/src/har.rs` `+34 −54`、`crates/rurge-api/src/routes/capture.rs` `+4 −4`、计划文件 `+1 −0`（`har.rs` 的行数变少是 `millis` 比 `seconds` 短、几处 `taken(..)` 被 rustfmt 并回一行）。 | 本提交 |
| 59 | 6 | 协调者核过修正轮 1（`7cab029`，第 58 行）：第 57 行的七处都照改了——模块注释的节号是 8.4；`post_data` 的注释说"is UTF-8"时照写文字、`_encoding` 是 rurge 自己的键；测试里的 `seconds` 改名 `millis`，`git grep` 查 `seconds(` 没有输出；`document` 里 `to_writer` 的那一支换成 `expect`，`enumerate` 与 `if i > 0` 的逗号照旧；`Method` 与 `HeaderName` 在顶上 `use`，三处全路径换成短名；`har` 的文档注释说取的是快照。改名之后有两处 `taken(..)` 被 rustfmt 并成一行，别处没有动。派工里的两条 `git grep` 都没有输出。聚焦的两条命令 2 条与 5 条都绿（`green-task-6-fix-1-har.log`、`green-task-6-fix-1-api.log`）。门禁 93 / 5295 / 0 / 13（一台机器上的一次运行，不是 CI；全量里 `pipeline` 以 `STATUS_HEAP_CORRUPTION` 退出，单独重跑 78 条全过，合起来数；合起来的错误与告警的行与第 56 行那一次的全量逐行相同，只多崩溃带出的 `error: 1 target failed:` 与 `error: test failed, to rerun pass` 两行），`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 的退出码都是 0；跑之前与之后 D: 都剩 43G。计划的两张表只多第 58 行，带本机账户名的文件 26，`git diff --check` 干净。订正第 58 行两处：① 它的编号照派工的六节排，⑦ 那一句说那一处"在 ② 与 ⑤ 里办完了"不准——第 57 行的七处对第 58 行是 ①→①、② 与 ③→②、④→③、⑤→④、⑥→⑤、⑦→⑥，与 ⑤ 无关；② 它的 ④ 说去掉的那一支"加三行注释"，实际是两行。另：工作区报告的门禁表把测试那一步的退出码写成 0，日志末尾是 `exit=101`（`pipeline` 崩了）；第 58 行写的是对的。**任务 6 过了** | 本提交 |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
