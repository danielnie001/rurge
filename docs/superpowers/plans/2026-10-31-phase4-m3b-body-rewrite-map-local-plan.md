# 阶段 4 / M3b「Body Rewrite 的正则形式与 Map Local」实施计划

> **执行方式**：交给 Pi，由协调的会话逐个任务派工、逐个任务评审（设计第 2 节、M3-D1；做法同 M1a ～ M3a）。
>
> - 协调的会话每个任务开一个 Pi 会话，交给它这个任务的全文与要用的接口（派工文件）。Pi 做完把报告写进派工文件指定的路径，在本地提交。
> - 协调者读改动、重跑门禁、评审。有问题回到同一个 Pi 会话修正，再评审；过了才派下一个任务。
> - 每个任务：先写失败的用例、看到红；再实现到绿；跑门禁；对照本任务的「判据」与「全局约束」逐条自查；最后提交（**只在本地，不推送**）。钉住现有行为的用例（任务里写明"不要求先红"的）可以一写就绿。
> - 与计划的出入记进末尾的「执行期修正记录」，**不改计划的正文**。
>
> **停点**：没有。M3b 没有平台代码、没有只在 CI 上编译的代码（设计第 2 节）。M3b 做完之后由项目所有者把 M1、M2 与 M3 一起推送，推送之后读回那次 CI；推送与读回都归所有者，不是本计划的任务。

**目标**：让 `[Body Rewrite]` 的正则形式与 `[Map Local]` 生效，收尾 M3。

- `rurge-config`：Body Rewrite 的式子（`BodyPattern`：先 `regex` 的多行与 CRLF，编译不了再 fancy-regex）；两个节类型化；jq 的行告警并忽略（`W0042`）；Map Local 的路径来自下载到的文本时拒绝（`W0033`）；两个节从 `W0016` 的名单拿掉。
- HTTP 引擎里每个请求在 URL Rewrite 之后过 Map Local（命中就答复，不连上游）→ Body Rewrite（请求），应答在 Header Rewrite 之后过 Body Rewrite（应答）；两个上限、`gzip` 与 `deflate` 的解压、严格的 UTF-8；chunked 与 `Expect` 的请求原样转发、每条规则每小时至多一行 WARN。
- Map Local 的四种数据：文件（每个请求时打开、流式读）、URL（资源管理器的新一类资源，控制面的名字是 `data`）、文本、Base64、`tiny-gif`；`Content-Type` 的表。
- 备注、`/v1/rurge/http` 的 `body` 与 `mapLocal` 两个数开始计；手工清单加 M3 的条目；M3 的收尾文档（两份 README、需求文档第 7 节、兼容性清单、`docs/status.md`、`CLAUDE.md`）。

**架构**：

- 不加工作区边、不加外部包（M3-D2）：工作区的依赖表加 `regex = "1"`；`rurge-config` 新增正式依赖 `regex`，`rurge-http` 新增正式依赖 `flate2` 与开发依赖 `tempfile`。三者都已在 `Cargo.lock` 里，锁文件的包数不变（520）。
- `rurge-config` 做读法，也做 Body Rewrite 的替换（`BodyPattern::replace_all`）：`rurge-http` 不依赖 `regex`，编译好的式子只在 `rurge-config` 里用（P2）。
- `rurge-http` 的 `rewrite` 模块加两个文件：`body.rs`（正文的读入、上限、解压与改写的求值）与 `map_local.rs`（Map Local 的规则、数据与应答的头），都是不碰网络的函数（任务 3、4）；引擎里接上三步，另加流式读文件的正文（任务 5）。
- `rurge-net` 加 `ResourceKind::MapLocal`；`rurge-engine` 在建一代的 `Rewrite` 时把 Map Local 的 URL 挂进这一代的资源管理器（任务 6）。可执行文件只改 `CORE_VERSION` 的注释。

**技术栈**：都已在 `Cargo.lock` 里，源码在本机 cargo 注册表里。**不下载、不加新包**（M3-D2）。

- regex 1.13.1：`RegexBuilder::new(..).multi_line(true).crlf(true).build()`、`Regex::captures_iter`、`Regex::captures_len`、`Captures::get`。
- fancy-regex 0.14.0：`RegexBuilder::new(..).backtrack_limit(n).build()`、`Regex::captures_iter`（交回 `Result`）、`Regex::captures_len`。**不用 `replace_all` / `replacen`**：回溯超限时它们 panic（设计第 5.5 节）。
- flate2 1.1.10：`read::MultiGzDecoder`、`read::ZlibDecoder`、`read::DeflateDecoder`，以 `std::io::Read::take` 限量地读。
- hyper 1.11.1 与 http-body 1：`Incoming` 的 `poll_frame`、`is_end_stream`、`size_hint`；`Frame::data` / `Frame::trailers`。
- tokio 1.53.1 的 `fs`（`File::open`、`File::metadata`）与 `io-util`（`AsyncReadExt::take`）；`tokio_util::sync::CancellationToken`。不加 `tokio-util` 的 `io` 特性、不加 `futures`。

**依据**：

- M3 细化设计（下面写"设计"指它）：`docs/superpowers/specs/2026-10-31-phase4-m3-rewrite-map-local-design.md`。
  - M3b 用到第 3、5.4、5.5、8、9、10、12 ～ 18 节，与已决事项 M3-D1、D2、D8、D14、D18 ～ D23、D25 ～ D27。
  - **先读它第 22 节**：第 1 ～ 12 条是 M3a 改了设计正文的地方；本计划改的在"写 M3b 计划时"之下（第 13 条起，随本计划提交）。
  - 设计与本计划的「计划期决定」不一致的，以计划为准。
- M3a 计划（`docs/superpowers/plans/2026-10-31-phase4-m3a-url-header-rewrite-plan.md`）的「计划期决定」与执行期修正记录：`Rewrite`、`Note`、`note_rewrites`、计数、引擎里第 3 ～ 5 步的写法与测试夹具的来历；`docs/status.md` 阶段 4 / M3a 一节「以后的会话必须知道的事」整段适用。
- M1b、M2b 计划的「计划期决定」：`Tail`、看门狗、`Meter` 与 `Close` 在 HTTP/2 上重置流的来历（M2b 的 P11、P14）。
- 手册两页（`http/body-rewrite.html`、`http/map-local.html`，2026-10-31 取的文本，与 2026-10-14 的基线逐字节相同）：黄金用例照它们的示例行逐字写。
- MDN 的 "Common media types" 一页：`Content-Type` 的表（P17）。
- 写计划时对着 `ba6646a`（分支 `phase4-m3`）核对过的事实写在「计划期决定」里。外部库的事实对着注册表里锁定版本的源码。

## 全局约束

**依赖**

- 只改三处 `Cargo.toml`：工作区的依赖表加 `regex = "1"`（任务 1）；`crates/rurge-config/Cargo.toml` 加 `regex.workspace = true`（任务 1）；`crates/rurge-http/Cargo.toml` 加 `flate2.workspace = true`（任务 3）与开发依赖 `tempfile.workspace = true`（任务 5）。
- `Cargo.lock` 的 `[[package]]` 今天 520 个，每个任务之后照旧 520 个；锁文件里只有 `rurge-config` 与 `rurge-http` 两个包的 `dependencies` 列表变，别的不变。`cargo` 要下载任何东西就停下报告。
- 依赖方向不变：`rurge-config` 不依赖 `http`；`rurge-http` 不依赖 `rurge-engine`、不依赖 `regex` 与 `url`；`rurge-api` 的正式依赖仍只有 `rurge-engine` 与 `rurge-config` 两条工作区边。
- `unsafe_code`：不加放开的函数，`crates/rurge-platform/tests/unsafe_functions.rs` 的 `LISTED` 仍是 23 项。
- `crates/rurge/tests/sockets.rs` 的 `LISTED` 仍是 26 行：读文件不建套接字，Map Local 的 URL 经资源管理器现有的下载。名单之外的原因让那条扫描用例变红，就停下报告。

**缺省的 `cargo test` 不碰系统**

- 同 M3a：不建网卡，不改系统的任何设置；不执行 `netsh`、`route`、`ipconfig`、`ip`、`resolvectl`、`networksetup`。
- **测试从不把任何 CA 装进信任库，也不读系统的信任库**：建引擎的用例一律给 `EngineShared.roots`（M3a 记录第 20、32 行：空的根让 `Rewrite::build` 交 `Err`，用不到 TLS 时也给 `TlsFixture` 的根）。
- Map Local 的文件只在用例自己的临时目录里（`tempfile`）；用例从不读、不写临时目录之外的文件。URL 的数据只经回环上的 `TestServer`。

**测试的约定**

`CLAUDE.md`「测试约定」整节适用。写用例之前读 `docs/testing.md`「测试约定（全文）」的阶段 4 各节（M3a 一节在内）。与本计划直接相关的：

- 只用回环与端口 0。示例里的主机名经一行 `[Host]` 钉到 `127.0.0.1`，第二道防线是共用夹具的 `dns-server`（回环上的 `MockDns`，M3a P19）。要连上游的请求，目标端口是测试的源站的端口、写在请求里；不该连上游的请求（Map Local），规则选的策略是指向**测试自己持有的守卫监听器**的 HTTP 代理，守卫被连上就判失败。
- **大正文**：32 MiB 与 10 MiB 的边界用例在用例里生成字节（重复的 ASCII），不读写磁盘上的大文件；式子用简单的字面（例如 `^x`），时间与内存都有界。超过上限而不该读的正文（`Content-Length` 已经超过的请求）不必真的发出全部字节。
- 等待都有时限。固定等待只许"断言什么都没发生"，注释写它等的是什么、余量多大。新写的、带等待的用例提交前单独循环跑二十次以上（任务 5、6、7）。
- 抓日志的用例用 `LogCapture`，跑在当前线程运行时。测试里手写的 HTTP/1.1 客户端把每个请求——请求头与正文——在一次写里发出（大正文的用例例外：头与正文分几次写，注释写明为什么）。
- **内联的测试模块放在文件的末尾**：`crates/rurge/tests/sockets.rs` 读生产代码，读到第一个 `#[cfg(test)] mod … {` 为止。

**门禁与这台开发机**（每个任务末尾一次；说"通过"之前必跑）

- 门禁：`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test -j 8 --workspace --no-fail-fast`。
- 本机 msvc 工具链没有 rustfmt。跑 `cargo fmt` 之前让环境变量 `RUSTFMT` 指向 gnu 工具链里同版本的 rustfmt：用户目录下的 `.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe`。不要为此安装或升级工具链。
- **门禁的日志用重定向写、再追加退出码，不用 `Tee-Object`**（M3a 记录第 54 行）；单独重跑的结果也写进日志再引，引之前先看它在、看它写的是什么。
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

- **凭据与用户流量**（设计第 8.5、14 节，M3-D18）：正文、头的值、Map Local 的数据从不进备注、日志、错误文本与 `Debug`；备注里的 URL 不带查询串；Map Local 的 URL 在 `Debug` 与日志里只显示到 `rurge_config::remote::origin`。`BodyPattern` 的 `Debug` 只显示式子，`MapData` 与 `MapLocal` 的 `Debug` 不显示文本、Base64 与头的值（P2、P5）。
- **下载到的文本**（`CLAUDE.md`「下载到的文本」）：Map Local 的文件路径先问 `Reach`（`map_local_refusal`，P6）。
- **诊断**：`W0042` 进 `WITHOUT`，`codes::hint` 的文档注释同时改（P7）；Map Local 的未知键先问 `is_plain_name` 再决定消息里写不写键（P5）。
- **日志的分级**：M3b 只加 chunked、`Expect` 与 HTTP/2 上长度不知道的请求正文不改写的那一行 WARN（每条规则每小时至多一行，不带主机）与同一个请求的 DEBUG（带主机），另有改写的阻塞任务 panic 时的 ERROR（P18）。
- 文档与报告用写文件 / 改文件的工具改。**永远不要用把一个文件的内容经管道写回同一个文件的命令**（2026-10-27 这样写满过 D: 盘）。
- 计划末尾两张表里已写下的行不改写、不删除。追加新行时**不要把已有的行当成锚点替换掉**（M2a 任务 9 这样删掉过一行）。

**写法**

- 注释、日志、错误文本与诊断用英文，照所在文件的写法：文档注释写"为什么"与前提，句子完整。注释里提到设计或计划，写 `phase 4 M3 design 9.2`、`the phase 4 / M3b plan's P9`。
- 提交说明照 `git log` 的样子写，标题用中文：`feat(config): …`、`feat(http): …`、`feat(engine): …`、`feat(net): …`、`test(engine): …`、`refactor(http): …`、`docs: …`。正文说做了什么、改了哪些现有用例的期望、为什么。Pi 的提交不带署名行。
- 只在本地提交：不 `push`、不 `pull`、不 `fetch` 远端、不 `amend`、不改写历史。

**不做的事**

- 只运行本仓库构建出来的二进制与 Pi 自己。不下载、不安装任何东西。不手动跑 `rurge run`、`rurge dns lookup`；`rurge service` 只用 `--dry-run`。
- 参考实现：GPL 的（sing-box、mihomo、Xray-core）只取事实，代码不抄。
- 不做设计第 1 节「不做」一栏的事，也不做这些：
  - M3c 的事：jq（M3b 只给 `W0042`）、`br` 与 `zstd` 的解码（M3b 不改写、留备注）、fancy-regex 的升级；对上游收窄 `Accept-Encoding`（设计第 21 节，归所有者）；
  - 改 `redact.rs`、`crates/rurge-config/tests/secrets.rs`、节的解析器（`text::parse_str`、`strip_inline_comment`）；
  - 把改写过的正文压回去（设计第 9.6 节：不压回）；
  - `capture` 生效（M4）、脚本（阶段 5）。

**计划没有写到、要做决定的地方**

- 能按设计与总设计的原则定的：自己定，记进修正记录，写"执行者的裁定"。
- 要停下来列出、交给协调者的：去向写着"项目所有者"的（设计第 21 节）；会改变 M3-D1 ～ D31 的。
- 现有用例变红：P21 的表点了名的，照计划改，在提交说明里说；没点名的，停下来，记进修正记录，不顺手改期望值。

## 评审要点

设计没写透、而最可能咬到使用者的几种输入，与负责它们的任务和用例：

1. **解压炸弹与大正文**：几 KB 的 gzip 解开是几百 MB；没有 `Content-Length` 的应答越读越长；`Content-Length` 写着 10 GB 的请求。
   - 期望：每一层解压都以上限加一个字节为界地读，超了就原样转出原来的字节、留备注；应答越过 10 MiB 时已缓冲的字节与其余的字节依次原样流过；请求的 `Content-Length` 超过 32 MiB 时一个字节也不读就断开。内存不随正文无限地涨。
   - 用例（任务 3、5、7）：`a_decompression_bomb_stops_at_the_limit`、`read_ahead_stops_one_byte_past_the_limit`、`a_response_over_the_limit_streams_through`、`a_request_whose_length_is_over_the_limit_closes_before_the_dial`、`the_response_limit_is_ten_mib`、`the_request_limit_is_thirty_two_mib`。
2. **不是文字的正文被宽的 URL 式子配上**：图片、视频、二进制的 API 应答。
   - 期望：不是严格的 UTF-8 时这条规则跳过、留备注；正文的字节与头（`Content-Encoding`、`Content-Length`、trailer）都不变。
   - 用例（任务 4、7）：`a_body_that_is_not_utf8_is_left_as_it_was`、`the_manual_utf8_requirement`。
3. **缓冲途中停住、出错与被结束**：客户端上传到一半不动了；源站的应答正文发到一半不动了或断了；缓冲时请求被 `kill`、引擎停止。
   - 期望：请求一侧在 `idle` 之后关掉连接（HTTP/2 重置这个流），应答一侧在 `idle` 之后答 502（`show-error-page` 关着时关掉）、那条上游连接不回池；被结束的请求立刻停止缓冲。等待都以 `idle` 为界。
   - 用例（任务 3、5）：`read_ahead_stalls_after_idle`、`read_ahead_ends_when_cancelled`、`a_request_body_that_stalls_while_buffered_closes`、`a_response_that_stalls_while_buffered_answers_502`、`a_killed_request_stops_buffering`。
4. **Map Local 的文件出各种状况**：文件不在、是个目录、两次请求之间被改了、读的时候变短了；客户端不读；路径来自远程 include。
   - 期望：打不开的答 502（或关闭）、记录 `Failed("Map Local: cannot read <路径>")`、`failed` 加一；改过的文件下一个请求就读到新的；读到一半出错时正文以错误结束；客户端不读时看门狗照 M2b 放掉它；下载到的文本里的路径加载时 `W0033`、这一行跳过。
   - 用例（任务 2、5、7）：`a_map_local_path_from_downloaded_text_is_refused`、`a_missing_file_answers_502`、`a_file_that_is_a_directory_answers_502`、`a_file_that_becomes_shorter_ends_the_answer_with_an_error`、`a_local_file_nobody_reads_is_let_go`、`the_file_is_read_on_every_request`、`a_missing_map_local_file_answers_502`。
5. **改写之后的头与分帧**：chunked 的应答被改写；压缩过的应答被改写或没被改写；`HEAD`、204、304；讲 HTTP/2 的客户端。
   - 期望：改过的去掉 `Content-Encoding` 与 `Transfer-Encoding`、`Content-Length` 是新的长度（改成空的也是，`Content-Length: 0`）、trailer 丢掉；没改动的原样转出原来的字节（连同压缩与 trailer）；`HEAD`、1xx、204、304 的应答不缓冲；讲 HTTP/2 的客户端没有 `content-length` 的请求同 chunked。
   - 用例（任务 3、5、7）：`which_answers_are_buffered`、`which_requests_are_buffered`、`read_ahead_bodies_give_back_what_was_read`、`a_rewritten_chunked_response_gets_a_length`、`an_unchanged_response_keeps_its_bytes_and_trailers`、`a_rewritten_gzip_response_is_sent_plain`、`a_rewritten_gzip_response_and_an_unchanged_one`、`an_http2_request_without_a_length_is_forwarded_unmodified`。

---

## 文件结构

| 文件 | 任务 | 职责 |
| --- | --- | --- |
| `Cargo.toml`、`crates/rurge-config/Cargo.toml`、`crates/rurge-config/src/{dialect.rs,rewrite.rs,config.rs,deferred.rs,diagnostic.rs,lib.rs}`、`crates/rurge-config/tests/snapshots/corpus__corpus__kitchen-sink.snap` | 1 | `BodyPattern`；`[Body Rewrite]` 的读法；`W0042` |
| `crates/rurge-config/src/{rewrite.rs,reach.rs,config.rs,deferred.rs,spec/reader.rs,lib.rs}`、同一份快照、`crates/rurge/src/capabilities.rs` | 2 | `[Map Local]` 的读法；`map_local_refusal`；`CORE_VERSION` 的注释 |
| `crates/rurge-http/Cargo.toml`、`crates/rurge-http/src/rewrite/{body.rs（新）,mod.rs}` | 3 | 正文的读入（`ReadAhead`、`read_ahead`）、何时缓冲、解压 |
| `crates/rurge-http/src/rewrite/{body.rs,map_local.rs（新）,mod.rs}`、`crates/rurge-http/src/lib.rs`、`crates/rurge-engine/src/runtime.rs`（只改调用处） | 4 | 改写的求值、备注与计数、WARN 的限频、Map Local 的规则与应答的头、`Rewrite::build` 多一个参数 |
| `crates/rurge-http/Cargo.toml`（开发依赖）、`crates/rurge-http/src/engine.rs` | 5 | 引擎里的第 6、7、12 步；`ReadAhead` 接进 `Tail`；看门狗抽成函数；流式读文件的正文 |
| `crates/rurge-net/src/resource/mod.rs`、`crates/rurge-engine/src/{runtime.rs,external_resources.rs}`、`crates/rurge-engine/tests/{rewrite.rs,external_resources.rs}` | 6 | `ResourceKind::MapLocal`；控制面的名字 `data`；`build_rewrite` 挂上 URL 的数据 |
| `crates/rurge-engine/tests/rewrite.rs` | 7 | 端到端：两页的黄金用例与其余的 |
| `docs/acceptance/phase4-manual.md`、`docs/api/phase4.md`、`docs/testing.md` | 8 | 手工清单加 M3 的条目；M3b 的 API 与测试文档 |
| 文档（任务 9 列全） | 9 | M3 的收尾 |

模块的名字与拆法可以照实际调整，调了记进修正记录。

---

## Task 0：开工前核对

**做什么**：对着当时的分支核对下面每一项。有出入的记进「执行期修正记录」，一项一行，写"计划写 X，代码是 Y，按 Y 做"；按记录调整后面的任务。**不改计划的正文。** 没有出入也记一行。

1. **工作树**：`git status --short` 空；当前分支 `phase4-m3`；`ba6646a`（M3a 收尾的最后一个提交）在历史里，之后只多了本计划的提交。
2. **读**：设计第 22 节（先读）、第 3、5.4、5.5、8 ～ 10、12 ～ 18 节；本计划的「计划期决定」与 P21 的表；`docs/status.md` 阶段 4 / M3a 一节（「以后的会话必须知道的事」整段）。
3. **名字与签名**（设计第 18 节 V9 ～ V15）：用 `git grep -n` 逐项找到、记下行号；与「计划期决定」写的不同的照实记。
   - `rurge-config`：
     - `dialect.rs` 的 `translate`、`Template::{parse, expand}`；`rule.rs` 的 `Pattern`（`source`、`regex`、`new`）、`ParseCtx.base_dir` 与 `parse_external` 接相对路径的那一段；
     - `rewrite.rs` 的 `RewriteConfig`（派生、`is_empty`）、`Direction`、`parse_url`、`parse_header`、消息的常量、`is_field_name`、`can_be_a_value`、测试的 `read_url` / `read_header` / `said`；
     - `reach.rs` 的 `Reach`、`Refusal` 与三个 `*_refusal`；`text/mod.rs` 的 `Origin::reach`、`Entry`（`raw`、`span`、`origin`）、`Section::active_entries`；
     - `config.rs` 读 `[URL Rewrite]` 与 `[Header Rewrite]` 的那一段与那里能拿到的 `base_dir`；测试模块里断言 `W0016` 的名单或 `deferred` 内容、或写着 `Body Rewrite` / `Map Local` 的用例（列全）；经远程 include 断言 `W0033` 的现有用例（任务 2 照它写）；
     - `deferred.rs` 的 `DEFERRED` 与 `prefix_and_membership_matching`（M3a 加的 `is_deferred("Body Rewrite")`、`is_deferred("Map Local")` 两句）；
     - `diagnostic.rs` 的码表末尾（`W_REWRITE_FRAMING`、`I_REPLICA_REMOVED`）、`W_UNKNOWN_KEY`、`W_INVALID_VALUE`、`W_REMOTE_LOCAL_ACCESS`、`codes::hint` 与它的文档注释、测试的 `WITH_DEFAULT`、`PER_SITE`、`WITHOUT`、`every_code_is_decided_once`；
     - `spec/reader.rs` 的 `is_plain_name`（今天是私有的）；`spec/group.rs` 里 `W0001` 的两句消息；`config.rs` 里解 Base64 的写法（`STANDARD` 与 `STANDARD_NO_PAD`）；`remote.rs` 的 `origin`；
     - 语料库 `tests/corpus/valid/kitchen-sink.conf` 的 `[Body Rewrite]` 与 `[Map Local]`（第 168 ～ 174 行一带）与快照 `corpus__corpus__kitchen-sink.snap` 的 `deferred:` 一段、`W0016` 那一条、诊断的排序与最后一条。
   - `rurge-http`：
     - `rewrite/mod.rs` 的 `Note`、`RewriteCounters`（字段与方法）、`RewriteCounts` 的文档注释（"`body` and `map_local` stay at zero until M3b"）、`Rewrite` 的字段、`build`、`has_response_rules`、`replace_all`；测试的 `config_of`、`read_rules`、`test_roots`、`rewrite_of`，与 `counters_count_and_snapshot`、`build_gives_none_without_lines`；
     - `engine.rs` 的 `handle`：`Routed::Go` 的解出、`sniffed` 与两种不过重写的请求、`url`、`origin`、请求一侧的 Header Rewrite、URL Rewrite 的那一段与它的结尾、`wants_upgrade`、`tunnel`、`needs_h1`、`OriginKey::of`、`Form::of`、`headers::strip`、补 `Transfer-Encoding: chunked` 的那一句、`head`、`client_upgrade`、`request_meter`、`Idle::new`、发送的 `select!`、升级的那一支、应答的 `strip`、应答一侧的 Header Rewrite、`set_status`、`Outing` 与 `Tail::new`；
     - `engine.rs` 的 `answer`、`failed`、`page`、`timed_out`、`upstream_failed`、`note_rewrites`；`ReqBody`、`Outing`、`UpstreamBody`、`UpstreamBodyFailed`、`Stalled`、`Progress`、`Idle`、`Tail`（`new`、`take_upstream`、`watch_the_client`、`complete`、`fail`、`stop`、`Drop`、`PolledUpstream`、`poll_frame`、`is_end_stream`、`size_hint`）、`outcome_of`、`Guard`（`new`、`handle`、`retire`、`left`、`Drop`）；`Tail::new` 的全部调用处；
     - `engine.rs` 的测试模块：`Fake`（`rewrite`、`gateway`、`go_pages`、`forward`、`retargets`、`dials`、`seen`）、`Rig`、`connected`、`rig_timed`、`rig_head_wait`、`rig_tls`、`recording_origin`、`roots_of`、`rewrite_of`、`counted`、`rewrite_notes`、`header_value`、`closing_origin`；测"客户端不读了"（`client stopped reading`）的那条现有用例；测 HTTP/2 上一个请求自己的上下行字节（M2b P11）的那条现有用例；
     - `pool.rs` 的 `Meter`（`set`、`count_up`、`count_down`）；`headers.rs` 的 `HOP_BY_HOP`（有没有 `Transfer-Encoding`）、`Keep::NONE`、`strip`；
     - `rurge-inbound`：`responses::{ResponseBody, TINY_GIF, tiny_gif, error_page}` 与造 `Full` 正文的写法；`SessionHandle::{token, add_note, notes, set_status, finish, add_down}` 与读上下行字节的方法。
   - `rurge-net`：`resource/mod.rs` 的 `ResourceKind`（七个变体与次序）、`ResourceSource`、`ResourceSpec`、`resource_key`、`ResourceManager::get_labelled`、`ResourceHandle::current`、`ResourceState::data`、缺省的 `max_size`（64 MiB）、`update_interval: None` 时用哪个间隔；测试的 `manager()` 与 `NoNames`；`testing` 特性下的 `TestServer`（怎样给一个路径配应答、`url`）。
   - `rurge-engine`：`runtime.rs` 的 `build_rewrite` 与它的调用处、那时 `stack` 里的资源管理器叫什么（`stack.resources`）；`external_resources.rs` 的 `type_name`、`ExternalResource.kind` 的文档注释、测试的 `set_kind`；`tests/external_resources.rs` 的 `the_listing_names_each_kind_and_leaves_the_hosts_file_out`；`tests/rewrite.rs` 的夹具（`shared_on`、`with_a_gateway`、`Guard`、`exchange`、`RecordingOrigin`、`example_engine`、`example_get`、`mitm_section`、`harness_mitm`、`h2_*`、`RecordingProxy`）；`tests/common/mod.rs` 的 `Harness.dir`、`Profile`、`harness_with`、`harness_timed`、`online_runtime_with`、`stack_options`（`no_network`）、`LogCapture`。
   - `rurge`：`capabilities.rs` 里 `CORE_VERSION` 的注释；`docs/surge-compatibility-matrix.md` 里提到 `CORE_VERSION` 20 的那一处（V15）。
   - 外部库（注册表源码），行号写进记录：
     - regex 1.13.1：`RegexBuilder::{new, multi_line, crlf, build}`、`Regex::{captures_iter, captures_len}`、`Captures::get`；多行与 CRLF 下 `^$` 在空串与 `"a\r\n\r\nb"` 上各给几个匹配、在哪里；
     - fancy-regex 0.14.0：`RegexBuilder::{new, backtrack_limit, build}`（参数的类型）、`(?m)` 前缀、`Regex::{captures_iter, captures_len}` 交回的错误；
     - flate2 1.1.10：`read::{MultiGzDecoder, ZlibDecoder, DeflateDecoder}`；工作区的 `flate2 = "1"` 开的是哪个后端；
     - hyper 1.11.1：`Incoming::size_hint` 对 HTTP/1.1 带 `Content-Length` 的、chunked 的、HTTP/2 带与不带 `content-length` 的正文各是什么；HTTP/1.1 的服务端在应答没有 `Content-Length`、正文的 `size_hint` 是确切的值时写不写 `Content-Length`（`proto/h1/role.rs`）；服务端在什么时候答 `100 Continue`；
     - http-body 1：`Frame::{data, trailers, data_ref, into_data, into_trailers}`、`SizeHint`。
4. **数**：
   - `Cargo.lock` 里 `[[package]]` 的个数（520）；
   - `unsafe_functions.rs` 的 `LISTED` 的项数（23）；
   - `sockets.rs` 的 `LISTED` 的行数（26）；
   - 带本机账户名的文件数：派工文件给出要数的那个名字，数出来是 26。
5. **现有用例**（P21 的表）：逐条找到、读它断言什么，把"预计要改 / 预计照过"逐条写进修正记录，一条用例一句。表里没有的照实补一行。另扫一遍：`git grep -n 'Body Rewrite\|Map Local'`、`git grep -n 'RewriteConfig {'`、`git grep -n 'Rewrite::build'`、`git grep -n 'ResourceKind::'`、`git grep -n 'type_name'`。
6. **门禁**：跑一遍，记下测试二进制数、通过数、失败数与忽略数（M3a 收尾时是 92 / 5123 / 0 / 13，一台机器上的一次运行，不是 CI）。

**判据**：修正记录里有任务 0 的行；门禁的数字记下。

---

## Task 1：`BodyPattern` 与 `[Body Rewrite]` 的读法

**改哪里**：

- `Cargo.toml`：`[workspace.dependencies]` 加 `regex = "1"`。`crates/rurge-config/Cargo.toml`：`regex.workspace = true`。
- `crates/rurge-config/src/dialect.rs`：`BodyPattern`、`BODY_BACKTRACK_LIMIT`，测试模块里的用例。
- `crates/rurge-config/src/rewrite.rs`：`BodyRewrite`、`parse_body`、`RewriteConfig.body` 与 `is_empty`，测试模块里的用例。
- `crates/rurge-config/src/config.rs`：读 `[Body Rewrite]`；测试模块一条用例。
- `crates/rurge-config/src/deferred.rs`：`DEFERRED` 去掉 `"Body Rewrite"`；它的用例。
- `crates/rurge-config/src/diagnostic.rs`：`W0042`、`hint` 的文档注释、测试的 `WITHOUT`。
- `crates/rurge-config/src/lib.rs`：再导出。
- `crates/rurge-config/tests/snapshots/corpus__corpus__kitchen-sink.snap`：手改（P8）。
- 工作区里每一处 `RewriteConfig { … }` 的写法（任务 0 列全，至少有 `rurge-http` 的 `rewrite/mod.rs` 测试模块的 `read_rules`）：加上 `body` 字段，断言不改。

**接口**

- 消费：M3a 的 `translate`、`Template`、`Pattern::new`、`split_words`、`Direction`、`RewriteConfig`。
- 产出：

```rust
// dialect.rs
pub const BODY_BACKTRACK_LIMIT: usize = 100_000_000;
#[derive(Clone)]                         // PartialEq、Eq 按 source；Debug 手写（P2）
pub struct BodyPattern { pub source: String, /* 私有：编译好的那一种 */ }
impl BodyPattern {
    pub fn new(source: &str) -> Result<BodyPattern, String>;
    pub fn with_backtrack_limit(source: &str, limit: usize) -> Result<BodyPattern, String>;
    pub fn is_fancy(&self) -> bool;
    /// 全部不重叠的匹配换成 `template` 展开的结果，从左到右；None：fancy-regex 回溯超限
    pub fn replace_all(&self, template: &Template, text: &str) -> Option<String>;
}
// rewrite.rs
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BodyRewrite { pub direction: Direction, pub pattern: Pattern, pub pairs: Vec<(BodyPattern, Template)>, pub span: Span }
pub fn parse_body<'a>(entries: impl IntoIterator<Item = &'a Entry>, diags: &mut Diagnostics) -> Vec<BodyRewrite>;
// RewriteConfig 多 `pub body: Vec<BodyRewrite>`；is_empty 也看它
// diagnostic.rs
codes::W_JQ_REWRITE_UNAVAILABLE   // "W0042"
// lib.rs
pub use dialect::{BodyPattern, Template};
pub use rewrite::{BodyRewrite, /* 已有的 */};
```

**用例（先红）**

- `dialect.rs`（P2）：
  - `body_patterns_read_lines_by_crlf`：`BodyPattern::new(r"^foo=(.*)$")` 不是 fancy；`replace_all(&Template::parse("[$1]"), "foo=1\r\nfoo=2\nbar")` 是 `Some("[1]\r\n[2]\nbar")`（`\r` 不进第 1 组）。
  - `a_lookahead_falls_to_fancy_regex`：`BodyPattern::new(r"a(?=b)")` 是 fancy；在 `"ab ac"` 上换成 `x` 得 `Some("xb ac")`。
  - `fancy_patterns_count_a_carriage_return_into_the_line`（钉住登记的差异，设计第 5.4 节）：`BodyPattern::new(r"^a(.)(?=\n)")` 是 fancy；在 `"a\r\n"` 上换成 `[$1]` 得 `Some("[\r]\n")`。
  - `empty_matches_insert_where_the_engine_allows`：`^$` 换成 `new`：`""` → `"new"`；`"a\n\nb"` → `"a\nnew\nb"`；`"a\r\n\r\nb"` → `"a\r\nnew\r\nb"`（`\r\n` 中间没有匹配）。
  - `body_patterns_translate_icu_spellings`：`BodyPattern::new(r"a\hb")` 在 `"a b"` 上换成 `x` 得 `Some("x")`。
  - `body_patterns_that_compile_nowhere_are_refused`：`BodyPattern::new("(")` 是 `Err`，消息不空；`BodyPattern::new(r"[\H]")` 是 `Err`，消息恰好是 `` `\H` cannot be used inside a character class ``。
  - `a_backtracking_body_pattern_gives_none`：`BodyPattern::with_backtrack_limit(r"(a|b|ab)*(?=c)", 1000)` 在 `"ab"` 重复 40 次上 `replace_all` 是 `None`。另证 `new` 用的是 `BODY_BACKTRACK_LIMIT` 而不是 fancy-regex 缺省的 100 万：同一个式子在 `"ab"` 重复 20 次上，`with_backtrack_limit(.., 1_000_000)` 是 `None`，`BodyPattern::new` 是 `Some`（fancy-regex 0.14 把这个式子里的 `(a|b|ab)*` 放进它的回溯机，`compile.rs` 的 `compile_concat`，回溯约 2 的 21 次方次；重复的次数照实调到两者分开，调了记进修正记录）。
  - `body_patterns_compare_and_show_by_source`：同一串编两次相等；`format!("{:?}", BodyPattern::new("a+").unwrap())` 恰好是 `BodyPattern("a+")`。
- `rewrite.rs`（P3，消息照 P7；用 `parse_str` 造 `Entry`，照 M3a 的 `read_url`）：
  - `body_rewrite_reads_the_manual_examples`：手册的两行（`http-request ^http(s)?://example\.com value abc`、`http-response ^http(s)?://example\.com documents Surge`）与语法一节的 `http-response ^https?://example\.com/ regex1 replacement1 regex2 replacement2` → 三条：方向、`pattern.source`、各对的 `source` 与模板展开的结果逐条断言；没有诊断。
  - `body_rewrite_shapes`：
    - `HTTP-REQUEST ^a b c` → `Request`、一对；
    - `http-response ^a b`、`http-response ^a`、`http-put ^a b c`、`http-response ^a b c d`（式子之后三项，不成对）→ 各一条 `W0012`，消息恰好是 P7 的形状那一句，行不进结果；`http-response ^a b c d e`（两对）照收；
    - `http-request ( b c` → `W0012`，以 `[Body Rewrite]: the regular expression cannot be used (` 开头；
    - `http-request ^a ( c` → `W0012`，以 `[Body Rewrite]: a body pattern cannot be used (` 开头；
    - `http-request ^a "x y" "1 2"` → 一对，`source` 是 `x y`，模板展开是 `1 2`。
  - `jq_lines_are_ignored_with_w0042`：手册的 `http-response-jq ^http://httpbingo.org/anything '.headers |= with_entries(select(.key | test("^X-") | not))'` 与 `HTTP-REQUEST-JQ ^a .` → 各一条 `W0042`（级别警告，消息恰好是 P7 的，`span` 是这一行），结果里没有规则。
- `config.rs`：`body_rewrite_is_read_and_no_longer_deferred`：照 M3a 的 `rewrite_sections_are_read_and_no_longer_deferred` 写，节头 `[Body Rewrite]` 与 `[body rewrite]` 各一份：没有错误；没有 `W0016`、`W0002`、`W0012`；`deferred.get("Body Rewrite")` 是 `None`；`config.rewrite.body` 一条。
- `deferred.rs` 的 `prefix_and_membership_matching`：M3a 加的 `assert!(is_deferred("Body Rewrite"))` 改成 `assert!(!is_deferred("Body Rewrite"))`，注释照它的写法写 `// typed since phase 4 M3b`（P21）。
- `diagnostic.rs` 现有的 `every_code_is_decided_once`：加了常量而没有做决定时它变红，这就是它的"先红"。
- 语料库快照：`kitchen-sink.conf` 第 169 行读得了、第 170 行（jq）一条 `W0042`，`W0016` 的消息与 `deferred` 少了这一节（P8）。

**实现**

- `BodyPattern`：照 P2。`replace_all` 逐个取匹配自己拼（同 M3a 的 `rewrite::replace_all`）：每个匹配先接上一段原文，再 `template.expand(&|i| caps.get(i).map(|m| m.as_str()), captures_len - 1, &mut out)`；fancy-regex 的 `captures_iter` 交回任何一个 `Err` 就交回 `None`。
- `parse_body`：照 P3。
- `config.rs`：在读 `[Header Rewrite]` 之后照它的写法读 `[Body Rewrite]`，进 `RewriteConfig.body`；字段的文档注释写"`[Body Rewrite]` 读得了的行，照写的次序（phase 4 M3 design 9.1）；jq 的行给 `W0042`、不进这里"。
- **一次性的测量**（设计第 5.4 节，不提交）：在 `dialect.rs` 的测试模块里临时写一条 `#[ignore]` 的用例，以 `BODY_BACKTRACK_LIMIT` 编 `(?=a)b` 与 `(a|b|ab)*(?=c)`，各在 32 MiB 的 `a` 与 `ab` 重复 40 次上跑一次 `replace_all`，打印耗时与结果是不是 `None`；用 `cargo test -j 8 --workspace --lib -- <名字> --ignored --nocapture` 跑（调试构建）。两个耗时、结果与命令写进修正记录与报告，写明是调试构建、一台机器上的一次运行；**然后删掉这条用例**，它不进提交。

**判据**

- 上面的用例都在、都绿；`ConfigSummary` 的形状不变。
- `W0042` 在 `WITHOUT`，`hint` 的文档注释写了不设的理由（P7）。
- `Cargo.lock` 的包数仍是 520，只有 `rurge-config` 的依赖列表多了 `regex`。
- 测量的结果在修正记录里，测量的用例不在提交里。
- 门禁绿。

---

## Task 2：`[Map Local]` 的读法

**改哪里**：

- `crates/rurge-config/src/rewrite.rs`：`MapData`、`MapLocal`、`parse_map_local`、`RewriteConfig.map_local` 与 `is_empty`，测试模块里的用例。
- `crates/rurge-config/src/reach.rs`：`map_local_refusal` 与一条用例。
- `crates/rurge-config/src/spec/reader.rs`：`is_plain_name` 改成 `pub(crate)`（只改可见性）。
- `crates/rurge-config/src/config.rs`：读 `[Map Local]`；测试模块两条用例。
- `crates/rurge-config/src/deferred.rs`：`DEFERRED` 去掉 `"Map Local"`；它的用例。
- `crates/rurge-config/src/diagnostic.rs`：`W_REWRITE_FRAMING` 的文档注释里"From phase 4 M3b"那一句照实改成现在时（P7）。
- `crates/rurge-config/src/lib.rs`：再导出 `MapData`、`MapLocal`。
- 同一份快照：手改（P8）。
- `crates/rurge/src/capabilities.rs`：`CORE_VERSION` 的注释（P8）。
- 工作区里每一处 `RewriteConfig { … }` 的写法：加上 `map_local` 字段，断言不改。

**接口**

- 消费：任务 1 的 `RewriteConfig`；M3a 的 `is_field_name`、`can_be_a_value`、`split_words`（`Word.param`）。
- 产出：

```rust
// rewrite.rs
#[derive(Clone, PartialEq, Eq)]          // Debug 手写（P5）
pub enum MapData { File(PathBuf), Url(url::Url), Text(String), Base64(Vec<u8>), TinyGif }
impl MapData {
    /// 备注里写的 data-type：File 与 Url 都是 "file"，其余是 "text"、"base64"、"tiny-gif"
    pub fn kind(&self) -> &'static str;
}
#[derive(Clone, PartialEq, Eq)]          // Debug 手写：头只显示名字（P5）
pub struct MapLocal { pub pattern: Pattern, pub data: MapData, pub status: u16, pub headers: Vec<(String, String)>, pub span: Span }
pub fn parse_map_local<'a>(entries: impl IntoIterator<Item = &'a Entry>, base_dir: &Path, diags: &mut Diagnostics) -> Vec<MapLocal>;
// RewriteConfig 多 `pub map_local: Vec<MapLocal>`；is_empty 也看它
// reach.rs
impl Reach { pub(crate) fn map_local_refusal(self) -> Option<Refusal>; }
```

**用例（先红）**

- `rewrite.rs`（P5，消息照 P7；`base_dir` 用一个临时目录的路径）：
  - `map_local_reads_the_manual_example`：手册的四行与 `^http://surgetest\.com/empty data-type=text data=""`：
    - `Text("{}")`、`status` 500、没有头；
    - `TinyGif`、200；
    - `File(base_dir.join("data/map-local.json"))`、200、头 `[("a","b"), ("foo","bar")]`；
    - `Base64(b"test")`、200；
    - `Text("")`、200；
    - 没有诊断。
  - `map_local_data_types_and_defaults`：
    - 不写 `data-type`、`data="x.bin"` → `File`；`data-type=FILE` 同；
    - `data="https://a.test/x.json"` 与 `data="HTTP://a.test/x"` → `Url`；
    - 绝对路径（用 `std::env::temp_dir().join("abs.json")` 的文字，三个平台都是绝对的）→ `File` 就是那个路径，不接 `base_dir`；
    - `data-type=text` 不写 `data` → `Text("")`；`data-type=base64` 不写 `data` → `Base64` 空；
    - `data-type=tiny-gif data=whatever` → `TinyGif`，没有诊断。
  - `map_local_reads_both_header_forms`：
    - `header="a: b | c:d|"` → `[("a","b"), ("c","d")]`；
    - `header=` 接 `"x-a: 1\r\nx-b: 2\n\n"` 的 Base64（有填充与没有填充各一次）→ `[("x-a","1"), ("x-b","2")]`。
  - `map_local_refuses`：下面每一行各一条 `W0012`，消息恰好是 P7 对应的那一句，行不进结果：
    - `^a data`（式子之后的一项不是参数）→ 参数那一句；
    - `( data-type=text` → 正则那一句；
    - `^a data-type=xml` → `data-type` 那一句；
    - `^a data-type=file`、`^a data=""`、`data-type=text`（只有一项：第一项总是 URL 的式子，P5，所以这是一行没有 `data` 的 `file`）→ 文件没有 `data` 那一句；
    - `^a data="https://[x/"` → URL 那一句；
    - `^a data-type=base64 data=@@@` → Base64 那一句；
    - `^a data-type=text status-code=199`、`status-code=1000`、`status-code=x` → 状态码那一句；
    - `^a data-type=text header="a b:c"`、`header=@@@`、`header="a:b|c"`、`header=` 接 `"a: b\u{1}c"` 的 Base64 → 头那一句（消息里没有值）。
  - `map_local_unknown_keys_get_w0001`：`^a data-type=text mystery=1` → 规则照留，一条 `W0001`，消息恰好是 `` [Map Local]: unknown parameter `mystery` ignored ``；`^a data-type=text Weird.Key=1` → 消息恰好是 `[Map Local]: unknown parameter ignored`（键不是 `is_plain_name` 认的名字，不写进消息）。
  - `a_key_written_twice_takes_the_later`：`^a data-type=text data=a data=b` → `Text("b")`；`DATA-TYPE=base64 data-type=text data=x` → `Text("x")`。
  - `framing_headers_in_map_local_are_dropped_with_w0041`：`header="Content-Length:5|X-A:1|transfer-encoding:chunked"` → 头只剩 `[("X-A","1")]`；恰好一条 `W0041`（级别警告，消息与 `hint` 恰好是 P7 的）。
  - `a_map_local_path_from_downloaded_text_is_refused`：同一行 `^a data=x.json` 经 `Origin::Remote(..)` 与 `Origin::Managed(..)` 读 → 各一条 `W0033`，消息恰好是 `[Map Local]: ` 接 `map_local_refusal` 的那一句、`hint` 是它的那一句，行不进结果；同样两种来源里的 `^a data="https://a.test/x"` 照收。
  - `relative_paths_join_the_main_profile_directory`：经 `Origin::Include(另一个目录里的文件)` 读的 `^a data=x.json` → `File(base_dir.join("x.json"))`。
  - `the_debug_of_map_local_shows_no_data_or_value`：`Text("s3cr3t-t")`、`Base64(b"s3cr3t-b")`、头 `("X-Token", "s3cr3t-h")`、`Url("https://a.test/p?tok=q1")` 各一条，`{:?}` 里没有 `s3cr3t-t`、`s3cr3t-b`、`s3cr3t-h`、`tok`、`q1`，有 `X-Token` 与 `https://a.test/…`。
- `reach.rs`：`only_the_users_own_text_names_a_local_file_for_map_local`：`Anywhere` 是 `None`；另两种恰好是 P7 的两对。
- `config.rs`：
  - `map_local_is_read_and_no_longer_deferred`：照任务 1 的那一条写，节头 `[Map Local]` 与 `[map local]`；`config.rewrite.map_local` 一条，相对路径接在 Profile 所在的目录。
  - `map_local_paths_in_a_remote_include_are_refused`：照任务 0 找到的经远程 include 断言 `W0033` 的那条用例写：远程 include 的文本里一行 `[Map Local]` 的文件 → `W0033`、`config.rewrite.map_local` 空；同一份文本里 URL 的那一行照收。
- `deferred.rs` 的 `prefix_and_membership_matching`：`assert!(is_deferred("Map Local"))` 改成 `assert!(!is_deferred("Map Local"))`，注释 `// typed since phase 4 M3b`。
- 语料库快照：`kitchen-sink.conf` 的两行 Map Local 读得了、没有新诊断，`W0016` 的消息与 `deferred` 少了这一节。

**实现**

- 读法照 P5；`map_local_refusal` 照 P6；诊断照 P7；快照与 `CORE_VERSION` 的注释照 P8。
- `config.rs`：在读 `[Body Rewrite]` 之后读 `[Map Local]`，把那里的 `base_dir` 交给 `parse_map_local`；字段的文档注释写"`[Map Local]` 读得了的行，照写的次序（phase 4 M3 design 10.1）；相对路径接到主 Profile 的目录"。

**判据**

- 上面的用例都在、都绿；`ConfigSummary` 的形状不变；`redact.rs` 与 `tests/secrets.rs` 一行没改。
- `Cargo.lock` 不变。
- 门禁绿。

---

## Task 3：正文的读入、何时缓冲与解压

**改哪里**：

- `crates/rurge-http/Cargo.toml`：`flate2.workspace = true`。
- `crates/rurge-http/src/rewrite/body.rs`（新）：`Limit`、`REQUEST`、`RESPONSE`、`ReadAhead`、`Ahead`、`read_ahead`、`Buffer`、`request_buffer`、`response_buffer`、`decode`，测试模块在文件末尾。
- `crates/rurge-http/src/rewrite/mod.rs`：`mod body;`。

**接口**（都是 `pub(crate)`；任务 4、5 用）

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Limit { pub(crate) bytes: u64, pub(crate) text: &'static str }
pub(crate) const REQUEST: Limit = Limit { bytes: 33_554_432, text: "32 MiB" };
pub(crate) const RESPONSE: Limit = Limit { bytes: 10_485_760, text: "10 MiB" };
/// 读在前面的东西放回正文的前面（P9）
pub(crate) struct ReadAhead<B> { /* ahead: Option<Bytes>, trailers: Option<HeaderMap>, rest: Option<B> */ }
impl<B> ReadAhead<B> {
    pub(crate) fn through(body: B) -> ReadAhead<B>;
    pub(crate) fn whole(data: Bytes, trailers: Option<HeaderMap>) -> ReadAhead<B>;
    pub(crate) fn resumed(ahead: Bytes, rest: B) -> ReadAhead<B>;
    /// 下一帧不是从 `rest` 来的：读在前面的字节还在，或根本没有 `rest`
    pub(crate) fn has_ahead(&self) -> bool;
}
impl<B: Body<Data = Bytes> + Unpin> Body for ReadAhead<B> { type Data = Bytes; type Error = B::Error; /* P9 */ }
pub(crate) enum Ahead<B: Body> {
    Whole { data: Bytes, trailers: Option<HeaderMap> },
    Over { ahead: Bytes, rest: B },
    Stalled,
    Failed(B::Error),
    Cancelled,
}
pub(crate) async fn read_ahead<B: Body<Data = Bytes> + Unpin>(body: B, limit: u64, idle: Duration, token: &CancellationToken, meter: Option<&Meter>) -> Ahead<B>;
pub(crate) enum Buffer { Yes, Skip(&'static str) }
pub(crate) fn request_buffer<B: Body>(req: &Request<B>, h2_client: bool) -> Buffer;
pub(crate) fn response_buffer(method: &Method, status: StatusCode) -> bool;
/// Ok：解开之后的字节（没有要解的编码时就是 `body`）；Err：备注的原因（P11）
pub(crate) fn decode(encodings: &[HeaderValue], body: &Bytes, limit: Limit) -> Result<Bytes, String>;
```

**用例（先红）**（测试用的正文用 `http_body_util::StreamBody` 或手写的小 `Body` 造，能数自己被 poll 了几次）

- `read_ahead_reads_a_whole_body`：三个数据帧加 trailer → `Whole`，`data` 是三帧接起来的，`trailers` 是那一份。
- `read_ahead_stops_one_byte_past_the_limit`：上限 10：`"12345"`、`"67890"` → `Whole`（恰好 10 字节）；`"12345"`、`"678901"`、`"x"` → `Over`，`ahead` 是 `"12345678901"`，再 poll `rest` 得 `"x"`。
- `a_length_over_the_limit_is_not_read`：`size_hint` 是确切的 11、上限 10 → `Over`，`ahead` 是空的，正文一次也没有被 poll。
- `read_ahead_stalls_after_idle`：永远 `Pending` 的正文、`idle` 100 毫秒 → `Stalled`，5 秒之内交回。
- `read_ahead_ends_when_cancelled`：永远 `Pending` 的正文，另一个任务在 50 毫秒后取消令牌 → `Cancelled`，5 秒之内交回。
- `read_ahead_passes_a_body_error_on`：第二帧是错误 → `Failed`。
- `read_ahead_counts_on_the_meter`：`meter` 是 `Some`、借给一个句柄（照 `pool.rs` 测试模块造句柄的写法）→ 句柄记下的下行字节是读到的字节数；`None` 时什么也不记。
- `read_ahead_bodies_give_back_what_was_read`：
  - `whole("abc", Some(t))`：帧依次是 `"abc"`、trailer `t`，然后结束；`size_hint` 的下界是 3、没有上界（有 trailer 时不报确切的长度，P9）；读完 `is_end_stream`；
  - `whole("abc", None)`：`size_hint` 是确切的 3；
  - `whole(空, None)`：一开始就 `is_end_stream`；
  - `resumed("ab", rest)`（`rest` 给 `"cd"`）：`has_ahead` 先真；帧是 `"ab"`、`"cd"`；第一帧之后 `has_ahead` 假；`size_hint` 的下界是 2 加 `rest` 的；
  - `through(body)`：帧与 `body` 的一样，`has_ahead` 恒假；`whole` 的 `has_ahead` 恒真。
- `which_requests_are_buffered`（P10）：
  - 带 `Content-Length` 的 HTTP/1.1 `POST`、没有正文的 `GET` → `Yes`；
  - `Transfer-Encoding: chunked` → `Skip("the request body is chunked")`；`Expect: 100-Continue`（大小写不同）→ `Skip("the request expects 100-continue")`；两者都有时是 chunked 那一个；
  - `h2_client` 真、正文没有结束、`size_hint` 不确切 → `Skip("the request body has no length")`；同样而 `size_hint` 确切或正文已经结束 → `Yes`；`h2_client` 假时不看这一条。
- `which_answers_are_buffered`：`GET` 200、`POST` 500、`GET` 205 → 真；`HEAD` 200、`GET` 100、101、204、304 → 假。
- `decoding_layers`（P11）：
  - 没有 `Content-Encoding` → 原样；`gzip` 的 `"hello"` → `"hello"`；两个 gzip 成员接在一起 → 两段都在；`x-gzip`、`GZIP` 同；
  - zlib 头的 `deflate` 与裸的 `deflate` 都解得开；
  - `gzip, identity`、两行 `Content-Encoding`（`deflate` 与 `gzip`）→ 按相反的次序逐层解开。
- `unsupported_and_broken_encodings`：`br`、`zstd`、`compress` → `Err("content-encoding br is not supported")` 一类（名字照写）；`a b` → `Err("the content-encoding is not supported")`；`gzip` 而字节是乱的 → `Err("the body cannot be decoded as gzip")`。
- `a_decompression_bomb_stops_at_the_limit`：1 MiB 的零压成 gzip，`Limit { bytes: 1000, text: "1000 B" }` → `Err("the decoded body is over 1000 B")`。

**实现**

- 照 P9（`ReadAhead` 与 `read_ahead`）、P10（何时缓冲）、P11（解压）。
- 每个类型与函数的文档注释写它对应设计的哪一节（`phase 4 M3 design 9.2`、`9.3`、`9.4`）。

**判据**

- 上面的用例都在、都绿；这个文件里没有网络、没有 `tokio::spawn`。
- `Cargo.lock` 的包数仍是 520，只有 `rurge-http` 的依赖列表多了 `flate2`。
- 门禁绿。

---

## Task 4：改写的求值与 Map Local 的规则

**改哪里**：

- `crates/rurge-http/src/rewrite/body.rs`：`BodyRule`、`Rewrite::body_rules`、`Rewrite::rewrite_body`，测试模块里的用例。
- `crates/rurge-http/src/rewrite/map_local.rs`（新）：`MapSource`、`MapRule`、`content_type_for`、`Rewrite::map_local`，测试模块在文件末尾。
- `crates/rurge-http/src/rewrite/mod.rs`：`UrlData`；`Rewrite` 多 `body` 与 `map_local` 两组规则；`build` 多一个参数；`RewriteCounters::{body, map_local}`；`RewriteCounts` 的文档注释；测试模块。
- `crates/rurge-http/src/lib.rs`：`pub use rewrite::UrlData;`。
- `Rewrite::build` 的全部调用处（任务 0 列全）：`crates/rurge-engine/src/runtime.rs` 暂时给 `&|_| Arc::new(|| None)`（任务 6 换掉），注释写 `the phase 4 / M3b plan's task 6`；测试夹具同样。

**接口**

- 消费：任务 1、2 的 `BodyRewrite`、`BodyPattern`、`MapLocal`、`MapData`；任务 3 的 `Limit`、`decode`；M3a 的 `Note`、`Pattern` 在 URL 上的匹配与回溯超限的处理。
- 产出（任务 5、6 用）：

```rust
// rewrite/mod.rs
/// Map Local 的一个 URL 此刻的数据；第一次下载还没有成时 None（phase 4 M3 design 10.3）
pub type UrlData = Arc<dyn Fn() -> Option<Bytes> + Send + Sync>;
impl Rewrite {
    pub fn build(cfg: &RewriteConfig, mitm: &MitmConfig, roots: Arc<RootCertStore>, counters: Arc<RewriteCounters>,
                 url_data: &dyn Fn(&str) -> UrlData) -> Result<Option<Rewrite>, BuildError>;
}
impl RewriteCounters { pub(crate) fn body(&self); pub(crate) fn map_local(&self); }
// rewrite/body.rs
pub(crate) struct BodyRule { /* direction、pattern、pairs、warned: Mutex<Option<Instant>> */ }
impl BodyRule {
    pub(crate) fn source(&self) -> &str;                       // URL 的式子，照写
    pub(crate) fn should_warn(&self, now: Instant) -> bool;    // P18
}
impl Rewrite {
    /// 这个方向上式子匹配 `url` 的规则的下标，照写的次序；URL 的式子回溯超限的留 Skipped
    pub(crate) fn body_rules(&self, direction: Direction, url: &str, notes: &mut Vec<Note>) -> Vec<usize>;
    pub(crate) fn body_rule(&self, i: usize) -> &BodyRule;
    /// Some：改过之后的正文（已经不压缩）；None：原样转出原来的字节（P12）
    pub(crate) fn rewrite_body(&self, rules: &[usize], encodings: &[HeaderValue], body: &Bytes, limit: Limit, notes: &mut Vec<Note>) -> Option<Bytes>;
}
// rewrite/map_local.rs
pub(crate) enum MapSource { File(PathBuf), Url { data: UrlData, origin: String }, Bytes(Bytes) }
pub(crate) struct MapRule { /* pattern、source、status、headers、kind */ }
impl MapRule {
    pub(crate) fn source(&self) -> &str;          // URL 的式子，照写
    pub(crate) fn data(&self) -> &MapSource;
    pub(crate) fn status(&self) -> StatusCode;
    pub(crate) fn headers(&self) -> &HeaderMap;  // 已经补了 Content-Type、去了逐跳的头，没有 Content-Length
    pub(crate) fn kind(&self) -> &'static str;   // file / text / base64 / tiny-gif
}
impl Rewrite {
    /// 第一条式子匹配 `url` 的规则；URL 的式子回溯超限的留 Skipped、接着比下一条
    pub(crate) fn map_local(&self, url: &str, notes: &mut Vec<Note>) -> Option<&MapRule>;
}
pub(crate) fn content_type_for(name: &str) -> &'static str;   // P17
```

**用例（先红）**（规则用 `rurge_config` 的 `parse_str` 与 `rewrite::parse_body` / `parse_map_local` 造，或直接构造；`build` 的 `url_data` 给一个记下收到的 URL 的闭包）

- `body.rs`：
  - `body_rules_match_the_current_url_in_order`：请求、应答、请求三条都配得上 → `body_rules(Request, url)` 是 `[0, 2]`，`Response` 是 `[1]`；另加第四条请求的规则，URL 的式子是 `(?i)(a|b|ab)*(?=c)`，URL 是 `http://x.test/` 接 40 个 `ab` → 它不在结果里，一条 `Skipped("Body Rewrite skipped: (?i)(a|b|ab)*(?=c): the regular expression hit its backtracking limit")`。
  - `rules_and_pairs_act_in_sequence`：规则一 `a` → `b`、规则二 `b` → `c` 与 `c` → `d` → `"a"` 变成 `Some("d")`；备注恰好是两条 `Done`：`Body Rewrite: request by {规则一的式子}`、同样的规则二。
  - `a_rule_that_changes_nothing_leaves_no_note`：配不上任何一对的规则 → `None`、没有备注。
  - `a_body_that_is_not_utf8_is_left_as_it_was`：字节 `[0xff, b'a']`、两条匹配的规则 → `None`，两条 `Skipped("…: the body is not valid UTF-8")`。
  - `a_backtracking_pair_undoes_its_rule`：规则一把 `x` 换成 `y`；规则二的第一对把 `y` 换成 `z`、第二对是 `BodyPattern::with_backtrack_limit(r"(a|b|ab)*(?=c)", 1000)`；正文 `"x"` 接 40 个 `ab` → 结果是规则一的（`y` 接 40 个 `ab`）；备注是规则一的 `Done` 与规则二的 `Skipped("…: the regular expression hit its backtracking limit")`。
  - `a_compressed_body_is_decoded_first`：`gzip` 的 `"value"`、`value` → `abc` → `Some("abc")`（不压缩）。
  - `decoding_failures_skip_every_matching_rule`：`Content-Encoding: br`、两条规则 → `None`，两条 `Skipped("…: content-encoding br is not supported")`；解开之后超过上限的同样两条，原因是 `the decoded body is over {text}`。
  - `an_empty_body_can_be_generated`：`^$` → `generated`，空正文 → `Some("generated")`。
  - `warnings_come_at_most_once_an_hour`：`t0` 真；`t0 + 59 分 59 秒` 假；`t0 + 1 小时` 真；再过 1 秒假。
  - `notes_never_carry_bodies`：正文里有 `s3cr3t-b`、URL 带 `?tok=q1` → 全部备注里没有 `s3cr3t-b` 与 `q1`。
- `map_local.rs`：
  - `the_first_matching_rule_answers`：两条都匹配 → 交回第一条；回溯超限的那一条留 `Skipped("Map Local skipped: …")`、交回后面匹配的。
  - `content_types_follow_the_extension`：`a.json` → `application/json`；`A.JSON` 同；`x.tar.gz` → `application/gzip`；`noext`、`.hidden`、`a.unknownext` → `application/octet-stream`；P17 表里的每一行各一个断言（可以写成对着一张表的循环）。
  - `default_content_types`：`text` → `text/plain`；`tiny-gif` → `image/gif`；`base64` → `application/octet-stream`；文件 `x.webp` → `image/webp`；URL `https://a.test/p/x.svg` → `image/svg+xml`（取 URL 的路径）；`header="content-type:x/y"` 的规则 → 留它、不补。
  - `map_local_headers_lose_the_hop_by_hop_ones`：`header="Connection:close|Keep-Alive:x|X-A:1"` → 只剩 `X-A` 与补上的 `Content-Type`。
  - `the_data_of_each_type`：`Text` → `Bytes("…")`；`Base64` → 解开的字节；`TinyGif` → `rurge_inbound::responses::TINY_GIF` 的字节；`File` → 那个路径；`Url` → `url_data` 恰好被调一次、收到的是那个 URL 的全文，`origin` 是 `rurge_config::remote::origin` 的结果。
- `mod.rs`：
  - `counters_count_and_snapshot`：改（P21）——六个都各加一次，`snapshot()` 的六个数都是 1。
  - `build_gives_none_without_lines`：照过；`RewriteConfig` 只有一条 Map Local 时 → `Some`。

**实现**

- 照 P12（改写的求值）、P15（Map Local 的规则与应答的头）、P17（表）、P18（备注的措辞与 `should_warn`）。
- `build`：把 `BodyRewrite` 变成 `BodyRule`（`warned` 是 `None`）；把 `MapLocal` 变成 `MapRule`：头的名字与值转成 `HeaderName` / `HeaderValue`（`rurge-config` 已经校验过；万一转不成，这一个头丢掉，DEBUG 一行只写名字），`headers::strip(&mut headers, Keep::NONE)`，没有 `Content-Type` 时照 P17 补上；`MapData::Url(u)` 调一次 `url_data(u.as_str())`。
- `RewriteCounts` 的文档注释去掉"until M3b"那一句，写 `body` 与 `map_local` 各数什么（设计第 12 节）。

**判据**

- 上面的用例都在、都绿；这两个文件里没有网络、没有 `tokio::spawn`。
- `Rewrite::build` 的每一处调用都编得过；`runtime.rs` 的行为不变。
- 门禁绿。

---

## Task 5：引擎里的 Map Local 与 Body Rewrite

**改哪里**：

- `crates/rurge-http/Cargo.toml`：开发依赖 `tempfile.workspace = true`。
- `crates/rurge-http/src/engine.rs`：`handle` 的第 6、7、12 步；`UpstreamBody.inner` 换成 `ReadAhead<Incoming>`、`Tail` 不数读在前面的帧；`Tail::watch_the_client` 抽成一个函数；新的 `LocalFile` 正文与 `Requests::map_local`；测试模块里的新用例。

**接口**

- 消费：任务 3 的 `ReadAhead`、`Ahead`、`read_ahead`、`request_buffer`、`response_buffer`、`REQUEST`、`RESPONSE`；任务 4 的 `body_rules`、`body_rule`、`rewrite_body`、`map_local`、`MapRule`、`MapSource`、`RewriteCounters::{body, map_local}`；M3a 的 `note_rewrites`。
- 产出：没有新的公开接口。crate 内：

```rust
/// 从 `Tail::watch_the_client` 原样搬出（P16）：`slot` 是看门狗与正文共用的那一格
fn watch_the_client<T: Send + 'static>(conn: &Conn, handle: &Arc<SessionHandle>, left: &'static str,
                                       progress: Arc<Progress>, idle: Duration,
                                       slot: Arc<Mutex<Option<T>>>, done: CancellationToken);
/// Map Local 的文件，流式读给客户端（P16）
struct LocalFile { /* 文件、路径、余下的字节、idle 与 deadline、取消、进度、看门狗、Guard、reached_end、done */ }
impl Body for LocalFile { type Data = Bytes; type Error = BoxError; }
// Requests 上
async fn map_local(&self, guard: Guard, pages: Pages, rules: &Rewrite, rule: &MapRule, method: &Method) -> Result<Response<ResponseBody>, Close>;
```

**用例（先红）**（`engine.rs` 的测试模块，用它的 `Fake`、`Rig` 与源站夹具；`Fake.rewrite` 给用例建的 `Arc<Rewrite>`；临时文件用 `tempfile`）

- Map Local：
  - `map_local_answers_without_a_dial`：`^http://m\.test/ data-type=text data="{}" status-code=500 header="X-A:1"` → 客户端收到 500、`X-A: 1`、`Content-Type: text/plain`、`Content-Length: 2`、正文 `{}`；`dial` 一次也没有被调；记录 `statusCode` 500、`Completed`、没有远端地址；备注恰好一条 `Map Local: text by ^http://m\.test/`；`map_local` 是 1。
  - `map_local_streams_a_file`：临时目录里 200 KiB 的 `big.json`（不止一个 64 KiB 的块）→ 正文与文件逐字节相同、`Content-Length` 是 204800、`Content-Type: application/json`；记录在正文读完之后才是 `Completed`。
  - `map_local_answers_head_with_the_length_and_no_body`：`HEAD`、10 字节的文件 → `Content-Length: 10`、正文空。
  - `map_local_204_and_304_send_no_body`：`status-code=204` 与 `status-code=304` 的 `text` → 没有 `Content-Length`、正文空。
  - `a_missing_file_answers_502`：文件不在：`pages.error` 真 → 502、标题 `Connection failed`、P18 那句说明；假 → 连接被关掉；记录都是 `Failed("Map Local: cannot read {路径}")`；备注 `Map Local failed: …: the file cannot be read`；`failed` 是 1、`map_local` 是 0；`dial` 没有被调。
  - `a_file_that_is_a_directory_answers_502`：`data` 是一个目录 → 同上。
  - `a_file_that_becomes_shorter_ends_the_answer_with_an_error`：8 MiB 的文件；客户端发出请求、收到应答头之后先不读，用例另开一个写的句柄把文件 `set_len(0)`（三个平台上别的句柄开着的文件都能这样截：Rust 在 Windows 上打开文件时共享读、写与删除），然后读到底 → 收到的正文少于 8 MiB、连接被关掉；记录 `Failed("Map Local: {路径} became shorter while it was read")`；`map_local` 是 1、`failed` 是 0（P15）。
  - `url_data_that_is_not_there_answers_502`：`url_data` 交回 `None` → 同上，记录是 `Failed("Map Local: the data from https://d.test/… is not available yet")`、备注 `…: the data is not available yet`。
  - `url_data_answers_from_what_the_engine_gives`：`url_data` 交回 `Some("abc")` → 200、`abc`、`Content-Type` 按 URL 路径的扩展名。
  - `a_local_file_nobody_reads_is_let_go`：照任务 0 找到的测"客户端不读了"的那条 `Tail` 用例写，换成 8 MiB 的文件 → 记录在有界的时间里成为 `Failed("client stopped reading")`。
  - `kill_ends_a_local_file_answer`：客户端不读、用例 `kill` 这个请求 → 记录 `Failed("killed")`，连接在有界的时间里被关掉。
- Body Rewrite（请求）：
  - `a_request_body_is_rewritten_before_the_dial`：`http-request ^http://b\.test/ value abc`，`POST` 正文 `key=value&value=1`、`Content-Length: 17` → 源站收到 `key=abc&abc=1`、`Content-Length: 13`；备注 `Body Rewrite: request by ^http://b\.test/`；`body` 是 1。
  - `an_empty_get_can_get_a_body`：`http-request ^http://b\.test/ ^$ generated`、没有正文的 `GET` → 源站收到正文 `generated`、`Content-Length: 9`。
  - `a_request_whose_length_is_over_the_limit_closes_before_the_dial`：`Content-Length: 33554433` 的 `POST`（只发头）→ 连接被关掉；`dial` 没有被调；记录 `Failed("Body Rewrite: the request body is over 32 MiB")`；备注 `Body Rewrite failed: ^http://b\.test/: the request body is over 32 MiB`；`failed` 是 1。（恰好 32 MiB 的那一边在任务 7 的 `the_request_limit_is_thirty_two_mib`；上限的两边在任务 3 以小的上限钉住。）
  - `chunked_and_expect_requests_are_not_buffered`：chunked 的 `POST` 与带 `Expect: 100-continue` 的 `POST` → 源站收到原来的正文；备注各一条 `Body Rewrite skipped: …: the request body is chunked` / `…: the request expects 100-continue`；`skipped` 是 2、`body` 是 0。
  - `a_request_body_that_stalls_while_buffered_closes`：`rig_timed` 给短的 `idle`（例如 200 毫秒），`Content-Length: 10` 而只发 5 字节 → 连接在有界的时间里被关掉；`dial` 没有被调；记录 `Failed("request body stalled")`。
  - `a_killed_request_stops_buffering`：同上而在缓冲时 `kill` → 连接被关掉、记录 `Failed("killed")`。
- Body Rewrite（应答）：
  - `a_rewritten_chunked_response_gets_a_length`：源站 chunked 地答 `<p>documents</p>`、末尾带一个 trailer，`http-response ^http://b\.test/ documents Surge` → 客户端收到 `<p>Surge</p>`、`Content-Length: 12`、没有 `Transfer-Encoding`、没有 trailer；备注 `Body Rewrite: response by …`；`body` 是 1。另一个请求（`http://e.test/`，前一条规则配不上它）的源站答 `documents`，规则 `http-response ^http://e\.test/ documents ""`（空的替换）→ `Content-Length: 0`、正文空（设计第 9.5 节）。
  - `an_unchanged_response_keeps_its_bytes_and_trailers`：源站 chunked 地答、末尾带一个 trailer `x-t: 1`；客户端的请求带 `TE: trailers`（hyper 的 HTTP/1.1 服务端只在请求带它时写 trailer，`proto/h1/conn.rs` 的 `allow_trailer_fields`）；配不上任何一对的规则 → 客户端照 chunked 读到的正文与 trailer 与源站发的一样、头不变、没有备注、`body` 是 0。
  - `a_rewritten_gzip_response_is_sent_plain`：源站答 `Content-Encoding: gzip` 的 `documents` → 客户端收到不压缩的 `Surge`、没有 `Content-Encoding`。
  - `a_response_over_the_limit_streams_through`：源站 chunked 地答 10 MiB 加 1 字节（含式子）→ 客户端收到的字节与源站的一样；源站以 `Content-Length: 10485761` 答同样的字节 → 同样；两次各一条 `Body Rewrite skipped: …: the body is over 10 MiB`。
  - `a_response_that_stalls_while_buffered_answers_502`：短的 `idle`，源站 `Content-Length: 10` 而只发 5 字节就不动了 → 客户端在有界的时间里收到 502；记录 `Failed("upstream body stalled")`；同一个客户端连接上的下一个请求另拨了一次（那条上游连接没有回池）。
  - `head_and_204_answers_are_not_buffered`：`HEAD` 与源站答 204 的请求过一条会配上的应答规则 → 照原样、没有备注。
  - `the_down_bytes_of_a_buffered_response_count_once`：照任务 0 找到的 M2b 测一个请求自己的上下行字节的那条用例写（HTTP/2 的上游），改写的应答规则配上 → 记录的下行字节是源站发的正文的字节数，不是两倍，也不是改写之后的长度。
- 计数：一个请求两个方向都改了正文时 `body` 只数一次（加在上面某一条里）。

**实现**

- 照 P13（改过之后的头）、P14（各步的位置）、P15（Map Local 的答复）、P16（`LocalFile` 与看门狗）、P18（备注、计数与日志）。
- `watch_the_client` 抽出来时行为一点不变：`Tail` 的现有用例全部照过就是证据。
- 改写在 `spawn_blocking` 里做，与请求的令牌赛跑（P14）；阻塞任务 panic 时转出原来的正文，ERROR 一行（P18）。

**判据**

- 上面的用例都在、都绿；`Tail` 与 `Idle` 的现有用例一条也没有改期望。
- 新写的带等待的用例单独循环跑二十次（命令与结果写进报告）。
- `Cargo.lock` 的包数仍是 520，只有 `rurge-http` 的开发依赖多了 `tempfile`。
- 门禁绿。

---

## Task 6：Map Local 的 URL 与资源管理器

**改哪里**：

- `crates/rurge-net/src/resource/mod.rs`：`ResourceKind::MapLocal`。
- `crates/rurge-engine/src/external_resources.rs`：`type_name`、`ExternalResource.kind` 的文档注释、测试的 `set_kind`。
- `crates/rurge-engine/src/runtime.rs`：`build_rewrite` 多一个参数，给 `Rewrite::build` 真的 `url_data`。
- 用例：`crates/rurge-engine/tests/rewrite.rs`。

**接口**

- 消费：任务 4 的 `UrlData` 与 `Rewrite::build`；`ResourceManager::get_labelled`、`resource_key`、`ResourceHandle::current`、`rurge_config::remote::origin`。
- 产出：`ResourceKind::MapLocal`（在 `Include` 之后、`System` 之前）；`async fn build_rewrite(config: &Config, shared: &EngineShared, resources: &ResourceManager) -> Option<Arc<Rewrite>>`（照任务 0 记下的 `stack` 里资源管理器的类型写）。

**用例（先红）**

- `crates/rurge-engine/src/external_resources.rs` 的 `set_kind`：加 `ResourceKind::MapLocal` → `"data"`（P21）。
- `crates/rurge-engine/tests/rewrite.rs`（在线的一代：`online_runtime_with` 与回环上的 `TestServer`，GeoIP 的两个地址照它指到同一个 `TestServer`）：
  - `map_local_urls_are_listed_as_data`：Profile 一行 `^http://m\.test/ data="{server}/m.json"`，`TestServer` 在 `/m.json` 答 `{"a":1}` → 外部资源的列表里有一项，种类是 `data`、标签以 `map local data ` 开头、显示的来源是 `rurge_config::remote::origin` 的结果；等到它下载好（`wait_until`，以列表里的状态为准）之后，请求 `http://m.test/` → 200、`{"a":1}`、`Content-Type: application/json`；守卫没有被连过。
  - `map_local_url_data_that_never_came_answers_502`：`TestServer` 在那个路径答 404 → 等到第一次下载失败之后，请求得 502、记录 `Failed("Map Local: the data from {origin} is not available yet")`。
  - `the_same_map_local_url_is_one_resource`：两行 Map Local 写同一个 URL → 列表里只有一项。
- `crates/rurge-engine/tests/external_resources.rs` 的 `the_listing_names_each_kind_and_leaves_the_hosts_file_out`：照过（它的 Profile 没有 Map Local）。

**实现**

- 照 P19。`build_rewrite` 的 `url_data` 闭包把收到的文字读回 `url::Url`（来自 `MapData::Url`，读不回是不可能的；万一读不回，交回恒为 `None` 的闭包，DEBUG 一行不带 URL）。
- 文档注释写：资源挂在这一代的资源管理器上，开关关着时照样下载（设计第 10.3 节"照资源管理器"）。

**判据**

- 上面的用例都在、都绿；`ResourceKind` 的其余用例照过。
- 新写的带等待的用例单独循环跑二十次。
- 门禁绿。

---

## Task 7：端到端

**改哪里**：`crates/rurge-engine/tests/rewrite.rs`。夹具照 P20；不改生产代码（要改就停下报告）。

**用例**（都是新的；"先红"不适用——它们验证任务 1 ～ 6 已经做好的，一写就绿时照实记；变红就是缺陷，停下报告）

- 手册的黄金用例（规则照手册的原文逐字写，与手册的出入写在用例的注释里，P20）：
  - `the_manual_body_rewrite_request_example`：`http-request ^http(s)?://example\.com value abc`，经 HTTP 监听器的 `POST http://example.com:{源站端口}/`、正文 `key=value&value=1` → 源站收到 `key=abc&abc=1`、`Content-Length: 13`；经 SOCKS5 的嗅探到的同一个请求再一遍。
  - `the_manual_body_rewrite_response_example`：`http-response ^http(s)?://example\.com documents Surge`，源站答 `text/html` 的 `<p>documents</p>` → 客户端收到 `<p>Surge</p>`。
  - `the_manual_body_rewrite_pairs_example`：`http-response ^https?://example\.com/ regex1 replacement1 regex2 replacement2`，源站答 `regex1 regex2` → 客户端收到 `replacement1 replacement2`；同一行再接 `regex3 replacement3`（手册的第二行）→ 三处都换。
  - `the_manual_body_rewrite_capture_example`：手册说替换认 `$1`：`http-response ^http://example\.com/ (\w+)@(\w+) $2@$1`，正文 `a@b c@d` → `b@a d@c`。
  - `the_manual_body_rewrite_anchors_example`：手册说 `^` `$` 在正文的每一行的首尾匹配：`http-response ^http://example\.com/ ^x$ y`，正文 `x\r\nx\nxx` → `y\r\ny\nxx`。
  - `the_manual_body_rewrite_empty_body_example`：手册的 `^$`：`http-request ^http://example\.com/ ^$ generated`、没有正文的 `GET` → 源站收到 `generated`、`Content-Length: 9`。
  - `the_manual_utf8_requirement`：源站答不是 UTF-8 的字节（含式子的 ASCII）→ 客户端收到原来的字节；备注 `Body Rewrite skipped: …: the body is not valid UTF-8`。
  - `the_manual_map_local_examples`：手册的四行逐字写，`data/map-local.json` 放在 Profile 所在的目录（`Harness.dir`）下；四个请求，规则选守卫：
    - `/json` → 500、`{}`、`Content-Type: text/plain`；
    - `/gif` → 200、43 字节的 GIF、`Content-Type: image/gif`；
    - `/file` → 200、那个文件的字节、`a: b`、`foo: bar`、`Content-Type: application/json`；
    - `/base64` → 200、`test`、`Content-Type: application/octet-stream`；
    - 守卫没有被连过；`map_local` 是 4。
  - `the_manual_map_local_empty_result`：`data-type=text data=""` → 200、`Content-Length: 0`。
- 其余（设计第 16 节第 3 条属于 M3b 的）：
  - `the_request_limit_is_thirty_two_mib`：`Content-Length` 恰好 32 MiB 的请求被改写；多一个字节的连接被关掉、源站没有被连过、记录与计数照任务 5。
  - `the_response_limit_is_ten_mib`：源站 chunked 地答恰好 10 MiB → 改写；多一个字节 → 原样流过、一条备注。
  - `a_chunked_request_is_forwarded_with_one_warning`（`LogCapture`，级别开到 DEBUG）：两个 chunked 的请求过同一条请求规则 → 两个都原样到源站；抓到的 WARN 恰好一行（P18 的消息，带式子，不带主机）；DEBUG 里两行带主机的；两条备注、`skipped` 是 2。
  - `an_expect_request_is_forwarded_unmodified`：`Expect: 100-continue` 的请求 → 原样到源站、一条备注。
  - `an_http2_request_without_a_length_is_forwarded_unmodified`：讲 HTTP/2 的客户端经解密的连接发一个没有 `content-length` 的 `POST` → 原样到源站、备注 `…: the request body has no length`。
  - `an_http2_request_over_the_limit_resets_its_stream`：讲 HTTP/2 的客户端发 `content-length: 33554433` 的 `POST` → 这个流被重置；同一条连接上的下一个请求照常。
  - `a_rewritten_gzip_response_and_an_unchanged_one`：源站答 gzip：改写的 → 不压缩、没有 `Content-Encoding`；没配上任何一对的 → 原来的压缩字节、`Content-Encoding: gzip`。
  - `a_brotli_response_is_not_rewritten`：源站答 `Content-Encoding: br`（随便的字节）→ 原样；备注 `…: content-encoding br is not supported`。
  - `the_file_is_read_on_every_request`：两个请求之间改 Map Local 的文件 → 第二个读到新的，不必重载。
  - `a_missing_map_local_file_answers_502`：Profile 里 Map Local 的文件不在 → 502、记录 `Failed("Map Local: cannot read {路径}")`、`http_counts().rewrite` 的 `failed` 是 1、`mapLocal` 是 0；守卫没有被连过。
  - `a_rejected_request_is_not_mapped`：规则选 `REJECT`，另有会命中的 Map Local → 照 REJECT 答复、记录 `Rejected(Reject)`、没有备注、`http_counts().rewrite` 六个数都是 0。
  - `map_local_and_body_rewrite_on_a_decrypted_https_request`：MITM 与重写一起：解密的 `https://` 请求过 Map Local；另一个过应答的 Body Rewrite。
  - `the_switch_turns_body_rewrite_and_map_local_off`：`set_rewrite(false)` 之后两者都不生效、没有备注、计数不动；再开 → 又生效。
  - `no_body_or_data_reaches_the_notes_or_the_log`（`LogCapture`，级别开到 TRACE）：请求正文与应答正文里有 `s3cr3t-b`、Map Local 的 `text` 是 `s3cr3t-m`、URL 带 `?tok=q1` → 抓到的日志与记录的备注里都没有 `s3cr3t-b`、`s3cr3t-m`，备注里没有 `q1`。
  - `body_and_map_local_counts_reach_the_api_shape`：几种改写之后 `http_counts().rewrite` 的六个数恰好是期望的。

**判据**

- 上面的用例都在、都绿；生产代码没有改。
- 新写的带等待的用例单独循环跑二十次。
- 门禁绿。

---

## Task 8：手工清单、API 与测试的文档

**改哪里**（设计第 17 节）。照 M3a 任务 8 的写法与规矩：引的提交、用例名与数字对着仓库核过；历史的段落不改写；本地提交，CI 没有见过。

- `docs/acceptance/phase4-manual.md`：加 M3 的条目（URL Rewrite、Header Rewrite、Body Rewrite、Map Local、`rewrite` 开关），每一项照这份清单现有的样子写：前提、做什么、看到什么算对、对应的设计节与自动用例；明文的与解密的站点各有，真实的浏览器、真实的站点；Map Local 有文件与 URL 两种。清单开头的项数与"一项也没有做过"那一句照实改。
- `docs/api/phase4.md`：加「M3b　Body Rewrite 与 Map Local」一节：备注的全部措辞（P18）、`body` 与 `mapLocal` 两个数各数什么、记录里的几种 `Failed` 的原因、那一行 WARN 与 DEBUG、外部资源的种类 `data`；M3a 一节里"`body`、`mapLocal` 到 M3b 之前是 0"那一句由这一节订正，不改写那一句。
- `docs/testing.md`：「命令」加 M3b 的过滤词（`--lib -- body`、`--lib -- map_local`、`--test rewrite`），数目对着 `-- --list` 数；「测试约定（全文）」加阶段 4 / M3b 一节：大正文在用例里生成、32 MiB 与 10 MiB 的边界、`tempfile` 里的 Map Local 文件、URL 的数据经回环上的 `TestServer` 与在线的一代、`LogCapture` 数 WARN。

**判据**

- 文档里引的提交、用例名与数字对着仓库核过；
- `git grep -l <账户名>` 数出来的文件数仍是 26；
- 门禁不用跑（只改文档），报告写明；`git diff --check` 无输出。

---

## Task 9：M3 的收尾文档

**改哪里**（设计第 15、17 节，P22）。规矩同任务 8。

- `docs/surge-compatibility-matrix.md`：1.x 节 L52、L53（两个节），7.5、7.6 节各行；设计第 15 节第 10 ～ 12 条登记进去；另登记本计划的几处（P22）；`CORE_VERSION` 20 的那一处照 P8 改；状态标记照规矩（附录里 M2a 立下的那一条）；附录照规则整张重数；第 5 行的「最近一次」改成 M3b。
- `docs/status.md`：阶段 4 下加「### M3b（Body Rewrite 的正则形式与 Map Local）」一节，照 M3a 一节的四段；末尾一段写 M3 至此做完、M3c 等所有者回答 Q4。第四段"去向写着项目所有者的事"列出本计划「延后事项」里去向是项目所有者的行、记录里写"协调者的裁定"的各行与设计第 21 节。
- `README.md` 与 `README_en.md`：状态、特性表（URL / Header / Body Rewrite、Map Local 各一行或照现有的分法）、路线图；两份一致，与需求文档、阶段 2 的验收文档不矛盾。
- `docs/requirements.md`：第 7 节阶段 4 的状态一句（M1、M2、M3a、M3b 做完，M3c 等 Q4，M4 ～ M6 没有做）；FR 的字面不改。
- `CLAUDE.md`：只改「现在在哪里」阶段 4 那一条：M3b 做完、M3 至此做完（M3c 等 Q4）；M1、M2 与 M3 的提交都在本地，CI 都没有见过；下一步是项目所有者推送、推送之后读回那次 CI，再往后是 M4 的细化设计。
- 设计第 22 节：在"写 M3b 计划时"那几条之后加"执行 M3b 时"：修正记录里改了设计正文意思的行，一条一段；没有就写一句"M3b 执行期没有改设计正文的意思"。
- `docs/README.md`：本计划那一项末尾接一句做完。

**判据**

- 文档里引的提交、用例名与数字对着仓库核过；两份 README 一致；
- `git grep -l <账户名>` 数出来的文件数仍是 26；
- 门禁不用跑（只改文档），报告写明；`git diff --check` 无输出。

---

## 计划期决定

写计划时对着 `ba6646a` 核对过下面引的名字与行号。行号会随前面任务的改动漂移，以名字为准；名字不同就照实改、记进修正记录，不改语义。标了"订正设计"的，已随本计划写进设计第 22 节（第 13 条起）。

| # | 事项 | 决定 |
| - | ---- | ---- |
| P1 | 任务的拆法（订正设计） | 设计第 2 节的 M3b 大纲拆成任务 0 ～ 9：<br>0 核对；<br>1 `BodyPattern` 与 `[Body Rewrite]`；<br>2 `[Map Local]`；<br>3 正文的读入、何时缓冲与解压；<br>4 改写的求值与 Map Local 的规则；<br>5 引擎里的三步；<br>6 Map Local 的 URL 与资源管理器；<br>7 端到端；<br>8 手工清单、API 与测试的文档；<br>9 M3 的收尾文档。<br>大纲里"手工清单加 M3 的几项"与"M3 收尾的文档"分成任务 8、9：一个 Pi 任务写不完全部的收尾文档 |
| P2 | `BodyPattern`（订正设计） | 设计第 3 节写成公开的枚举 `Plain(regex::Regex)` / `Fancy(fancy_regex::Regex)`；本计划改成结构体 `{ pub source: String, engine: Engine }`，`Engine` 是私有的同样两支。理由：`rurge-http` 不依赖 `regex`（设计第 3 节只给它加 `flate2`），编译好的式子在它那里用不了，所以替换也放在 `rurge-config`（`replace_all`），同 M3a 的 `Pattern` 留着 `source`。<br>**编译**：`new(s)` 就是 `with_backtrack_limit(s, BODY_BACKTRACK_LIMIT)`。先 `translate(s)?`（它的 `Err` 原样交回）；再 `regex::RegexBuilder::new(&t).multi_line(true).crlf(true).build()`，成了是 `Plain`；不成就 `fancy_regex::RegexBuilder::new(&format!("(?m){t}")).backtrack_limit(limit).build()`，成了是 `Fancy`；再不成交回 fancy-regex 的错误的 `to_string()`（设计第 5.4 节"消息带 fancy-regex 的原因"），`regex` 的错误丢掉。两个包的大小上限照缺省。<br>**`replace_all(template, text)`**：逐个取匹配，每个先接上一段原文、再展开模板（组数是 `captures_len() - 1`），最后接上余下的原文；`Plain` 总是 `Some`；`Fancy` 的 `captures_iter` 交回任何一个 `Err` 就是 `None`（设计第 5.5 节）。空的匹配照两个包自己的做法。<br>**比较与显示**：`PartialEq`、`Eq` 按 `source`；`Debug` 写 `BodyPattern({source:?})`（式子不是秘密，M3a 的 `Pattern` 同样显示它）。<br>`BODY_BACKTRACK_LIMIT: usize = 100_000_000`，文档注释写设计第 5.4 节取 1 亿的理由 |
| P3 | `[Body Rewrite]` 的读法 | 一行的文字是 `Entry.raw`，切成 `split_words`；每一项用它的 `text`（`key=value` 形状的项也是原文，M3a P2）。<br>**第一项**（ASCII 不分大小写）：`http-request` / `http-response` 是方向；`http-request-jq` / `http-response-jq` 给 `W0042`、这一行跳过（不看后面写了什么）；别的 `W0012` 形状那一句。<br>**项数**：少于 4 项，或第二项之后的项数是奇数 → `W0012` 形状那一句。<br>**第二项**：`Pattern::new`，`Err` → `W0012` 正则那一句，`{原因}` 是它的 `Err`。<br>**各对**：式子 `BodyPattern::new`，`Err` → `W0012` 式子那一句；替换 `Template::parse`（任何文字都合法）。`""` 照样是一项（空的式子、空的替换）。<br>诊断都带这一行的 `span` |
| P4 | `RewriteConfig` 的两个新字段 | `RewriteConfig { url, header, body, map_local }`，派生不变；`is_empty` 四个都空才真（所以只有 Map Local 的 Profile 也建一代的 `Rewrite`）。`lib.rs` 再导出 `BodyPattern`、`BodyRewrite`、`MapData`、`MapLocal`。工作区里每一处 `RewriteConfig { … }` 的写法在加字段的那个任务里补上字段，断言不改（P21） |
| P5 | `[Map Local]` 的读法（订正设计） | 设计第 3 节的 `MapData::Url(String)` 改成 `Url(url::Url)`：加载时就读成 URL，`Debug` 只显示 `rurge_config::remote::origin` 的结果。`MapData::kind()`：`File` 与 `Url` 是 `file`（手册："`file`: Returns the content of a specific file or URL"），其余是 `text`、`base64`、`tiny-gif`。<br>**第一项**总是 URL 的式子（`Pattern::new`，`Err` → `W0012` 正则那一句）：式子里常有 `=`（查询串），所以不看它是不是参数的形状。<br>**其余每一项**必须是参数（`Word.param` 是 `Some`），否则 `W0012` 参数那一句。键 ASCII 不分大小写：`data-type`、`data`、`header`、`status-code`；**写了两次的取后一个**（同 `mitm.rs` 的 `a_key_written_twice_takes_the_later_value_and_names_ignore_case`）；别的键 → `W0001`、这一行照留：键照 `spec/group.rs` 那两句的写法先问 `is_plain_name`，是就写进消息、不是就不写（P7）。<br>**`data-type`**：`file`（缺省）、`text`、`tiny-gif`、`base64`，ASCII 不分大小写；别的 `W0012`。<br>**`file`**：没有 `data` 或它是空的 → `W0012`。以 `http://` 或 `https://`（ASCII 不分大小写）开头的是 URL：`url::Url::parse`，`Err` → `W0012` URL 那一句。否则是路径：先问 `entry.origin.reach().map_local_refusal()`，有拒绝就 `W0033`、这一行跳过（P6）；绝对的照用，相对的 `base_dir.join(..)`（`base_dir` 是 `config.rs` 读 `[Rule]` 时用的那一个，从本地 include 来的行也一样，设计第 10.1 节）。不查文件在不在。<br>**`text`**：`data` 的值，没有就是空的。**`base64`**：`data` 的值先按 `STANDARD`、不成再按 `STANDARD_NO_PAD` 解（同 `config.rs` 解 Base64 的那一处），都不成 → `W0012`；没有 `data` 就是空的。**`tiny-gif`**：`data` 不读、写了也不告警。<br>**`header`**：值里有 `:` 时以 `\|` 分成各对，每对去掉首尾空白、空的忽略，在第一个 `:` 处分成名字与值、两边去掉空白；没有 `:` 时按 Base64（同上两种）解开，必须是 UTF-8，以 `\n` 分行、每行去掉末尾的 `\r` 与首尾空白、空行忽略，每行同样在第一个 `:` 处分。名字要 `is_field_name`、值的每个字要 `can_be_a_value`（M3a 的两个函数）；一对没有 `:`、名字或值不合法、解不开 → `W0012` 头那一句（不带值）。名字是 `Content-Length` 或 `Transfer-Encoding`（ASCII 不分大小写）的对丢掉，这一行有这样的对时恰好一条 `W0041`（P7）。<br>**`status-code`**：读成 `u16`、在 200 ～ 999 → 用它；否则 `W0012`。缺省 200。<br>**`Debug`**（手写）：`MapData` 写 `File(<路径的 Debug>)`、`Url(<origin>)`、`Text(***)`、`Base64(***)`、`TinyGif`；`MapLocal` 写 `pattern`、`data`、`status`、头只列名字、`span` |
| P6 | `map_local_refusal` | `Reach` 上第四个 `*_refusal`，文档注释照另三个写："What is said when text of this reach names a local file as `[Map Local]` data, if it may not; the line is ignored."。`Anywhere` 是 `None`；另两种的消息与建议照抄 P7。诊断是 `W0033`（`codes::W_REMOTE_LOCAL_ACCESS`），消息是 `[Map Local]: ` 接拒绝的消息，`with_hint` 是拒绝的建议 |
| P7 | 诊断的措辞 | 消息与建议**照抄表后「原文」的 P7 一块**（含竖线与反引号）。`{原因}` 是 `Pattern::new` 或 `BodyPattern::new` 的 `Err`；`{键}` 是写的键。都带这一行的 `span`。值、数据与头的值从不进消息。<br>**`W0042`**：`codes::W_JQ_REWRITE_UNAVAILABLE`；消息比设计第 13 节多 `[Body Rewrite]: ` 的前缀，同这个节别的诊断（订正设计）；文档注释写"`[Body Rewrite]` 的 jq 行：这个构建里没有 jq（phase 4 M3 design 13；M3c 等所有者的 Q4）"；进 `WITHOUT`，`codes::hint` 的文档注释照那一段的写法写不设的理由（原文 P7 最后一句）。<br>**`W0041`**：`codes::W_REWRITE_FRAMING` 的文档注释里"From phase 4 M3b the two names in a `[Map Local]` `header` are not used and the same code says so"改成现在时；它照旧在 `PER_SITE`，`[Map Local]` 那一处带它自己的建议 |
| P8 | 延后的名单、快照与 `CORE_VERSION` 的注释 | `deferred.rs` 的 `DEFERRED` 去掉 `"Body Rewrite"`（任务 1）与 `"Map Local"`（任务 2）。<br>**快照**照 M2a P25 的做法手改，不跑 `cargo insta accept`：任务 1 删掉 `deferred:` 下的 `- Body Rewrite`、`W0016` 的消息删掉 `[Body Rewrite], `、为 `kitchen-sink.conf` 第 170 行（jq）加一条 `W0042`——条目的形状与位置照 `cargo test -j 8 --workspace --test corpus` 失败时 insta 给的差异写；任务 2 同样删掉 `- Map Local` 与 `[Map Local], `。<br>**`CORE_VERSION`**（`crates/rurge/src/capabilities.rs`）：值仍是 22；注释里"It ties 20 to Body Rewrite and inline Map Local, which rurge parses without effect (`W0016`): a profile that asks for 20 gets the same here either way."换成"It ties 20 to Body Rewrite and inline Map Local, which rurge has from phase 4 M3b (the jq form of Body Rewrite aside, `W0042`)."。兼容性清单里那一处在任务 9 改 |
| P9 | `ReadAhead` 与 `read_ahead` | **`ReadAhead<B>`** 的字段：`ahead: Option<Bytes>`（`None` 或不空）、`trailers: Option<HeaderMap>`、`rest: Option<B>`。<br>• `poll_frame`：`ahead` 还在 → 交出它（一个数据帧）；否则有 `rest` → poll 它，`rest` 结束时置 `None` 再往下；否则 `trailers` 还在 → 交出 trailer 帧；否则结束。<br>• `is_end_stream`：`ahead`、`trailers` 都没有，且 `rest` 没有或它 `is_end_stream`。<br>• `size_hint`：`ahead` 的长度 n 加 `rest` 的（下界与上界各加 n）；没有 `rest` 时，没有 `trailers` 是确切的 n，有 `trailers` 是下界 n、没有上界——hyper 的 HTTP/1.1 服务端对确切的长度写 `Content-Length`、按长度分帧，trailer 就写不出去了（`proto/h1/role.rs` 的 `encode_headers`；长度不知道时它用 chunked，客户端的请求带 `TE: trailers` 时写 trailer，同 M1b 照转 `Incoming` 时）；请求一侧引擎因此补 `Transfer-Encoding: chunked`，hyper 的 HTTP/1.1 客户端见到它会去掉 `Content-Length`（`role.rs` 的 `set_length`）。<br>• `has_ahead`：`ahead.is_some() \|\| rest.is_none()`。<br>**`read_ahead(body, limit, idle, token, meter)`**：<br>• `body.size_hint().lower() > limit` → 一帧也不读，`Over { ahead: 空, rest: body }`（`Content-Length` 已经超过，设计第 9.3 节）；<br>• 否则逐帧读（`http_body_util::BodyExt::frame`），每一帧的等待以 `idle` 为限、与 `token` 赛跑（`biased`，令牌在前）：取消 → `Cancelled`；超时 → `Stalled`；数据帧 → `meter` 是 `Some` 时 `count_down(长度)`，接上，总长 `> limit` → `Over { ahead: 已读的全部, rest: body }`；trailer 帧 → 记下；结束 → `Whole`；错误 → `Failed`。<br>• 请求一侧给 `meter: None`（读的是客户端；对上游的字节由 `Idle` 与连接上的 `Metered` 照旧数），应答一侧给 `Some(&request_meter)`：讲 HTTP/2 的上游没有连接上的计数，读在前面的帧不再经过 `Tail`（M2b P11） |
| P10 | 何时缓冲 | 只在这个方向有规则配上当前的 URL 时才问（设计第 9.2 节）。<br>**请求**（`request_buffer`；调用处先排除要升级的与嗅探到的 `CONNECT`，它们不问、不留备注）：有 `Transfer-Encoding` 头（不论值）→ `Skip("the request body is chunked")`；否则有值是 `100-continue`（ASCII 不分大小写）的 `Expect` → `Skip("the request expects 100-continue")`；否则讲 HTTP/2 的客户端（`Conn.h2`）、正文没有结束、`size_hint().exact()` 是 `None` → `Skip("the request body has no length")`（订正设计：第 9.2 节只说"同 chunked"，没有给原因的文字）；否则 `Yes`——没有正文的 `GET` 也是。<br>**应答**（`response_buffer`）：方法不是 `HEAD`，状态码不是 1xx、204、304（M3-D19） |
| P11 | 解压 | `encodings` 是应答或请求全部的 `Content-Encoding` 行。每行以 `,` 分项、去掉首尾空白；空的项与 `identity` 去掉；从最后一项往前逐层解（设计第 9.4 节）。名字 ASCII 不分大小写：`gzip` 与 `x-gzip` 用 `flate2::read::MultiGzDecoder`；`deflate` 在头两个字节满足 `b[0] & 0x0f == 8` 且 `(b[0] as u16 * 256 + b[1] as u16) % 31 == 0` 时用 `ZlibDecoder`，否则（含不足两个字节的）`DeflateDecoder`；别的 → `Err`。<br>每一层 `decoder.take(limit.bytes + 1).read_to_end(..)`：出错 → `Err("the body cannot be decoded as {名字}")`；读到的多于 `limit.bytes` → `Err("the decoded body is over {limit.text}")`。<br>不认识的编码：名字是 RFC 9110 的 token（字母、数字与 `` !#$%&'*+-.^_`\|~ ``）→ `Err("content-encoding {名字} is not supported")`，否则（或这一行不是可见的 ASCII）→ `Err("the content-encoding is not supported")`。`{名字}` 是去掉空白之后照写的 |
| P12 | 改写的求值 | **`body_rules(direction, url, notes)`**：照写的次序，这个方向的每条规则以 `pattern.regex.is_match(url)` 比：`Ok(true)` 收下标；`Err`（回溯超限）留 `Skipped("Body Rewrite skipped: {式子}: the regular expression hit its backtracking limit")`。<br>**`rewrite_body(rules, encodings, body, limit, notes)`**（同步的；调用处在 `spawn_blocking` 里调）：<br>① `decode`，`Err(原因)` → `rules` 里每一条一条 `Skipped(原因)`，交回 `None`；<br>② 解开的字节不是严格的 UTF-8（`std::str::from_utf8`）→ 每一条一条 `Skipped("…: the body is not valid UTF-8")`，`None`；<br>③ 依次作用每一条：这一条的各对依次作用于上一对的结果（`BodyPattern::replace_all`）；有一对是 `None` → 这一条作废（正文回到它之前的样子），一条 `Skipped("…: the regular expression hit its backtracking limit")`，接着下一条；这一条的结果与它之前不同 → 一条 `Done("Body Rewrite: {方向} by {式子}")`；<br>④ 最后的文字与解开之后的原文不同 → `Some`，否则 `None`。<br>备注里只有规则的 URL 式子，没有正文 |
| P13 | 改过之后的头 | `Some(新)`：去掉 `Content-Encoding` 与 `Transfer-Encoding`，`Content-Length` 设为新的长度，正文是 `ReadAhead::whole(新, None)`（trailer 丢掉，设计第 9.6 节）。`None`：头不动，正文是 `ReadAhead::whole(原来的字节, 原来的 trailer)`（连同压缩，设计第 8.4 节）。`body` 计数一个请求只加一次：`handle` 里一个与 `headers_changed` 并排的 `body_changed` |
| P14 | 引擎里的位置（任务 5） | 在 `handle` 里：<br>① **第 6 步 Map Local**：URL Rewrite 那一段之后、`let upgrade = wants_upgrade(&req)` 之前。`rewrite` 与当前的 `url` 都有时 `rules.map_local(text, &mut notes)`，`note_rewrites`；有规则就 `return self.map_local(guard, pages, rules, rule, req.method()).await`。要升级的请求也过这一步（只看头）。<br>② `upgrade`、`tunnel`、`needs_h1` 照旧。<br>③ **第 7 步请求的 Body Rewrite**：在键 `(key, OriginKey::of(..))` 与借或拨之前。`rewrite`、`url` 都有、`!upgrade && !tunnel`、`body_rules(Request, ..)` 不空时：`request_buffer` 是 `Skip(原因)` → 每条配上的规则一条 `Skipped(原因)`，每条各问 `should_warn(Instant::now())` 决定 WARN，另起一行 DEBUG（P18），正文 `through`；是 `Yes` → `into_parts`，`read_ahead(body, REQUEST.bytes, conn.idle, handle.token(), None)`：<br>　• `Whole` → 改写（⑥），头照 P13；<br>　• `Over` → 第一条配上的规则一条 `Failed("Body Rewrite failed: {式子}: the request body is over 32 MiB")`，`failed` 加一，`handle.finish(Failed("Body Rewrite: the request body is over 32 MiB"))`，`Err(Close)`（HTTP/1.1 关连接，HTTP/2 重置流）；<br>　• `Stalled` → `finish(Failed("request body stalled"))`、`Err(Close)`；`Failed(e)` → `finish(Failed("request body failed: {e}"))`、`Err(Close)`；两者不计入 `failed`（不是重写失败的）；<br>　• `Cancelled` → `Err(Close)`（`Guard` 照旧记下怎样结束的）。<br>　别的情形一律 `req.map(ReadAhead::through)`。此后 `req` 是 `Request<ReadAhead<Incoming>>`，后面的 `Form::of`、`strip`、补 chunked、`fit`、`write_host`、`head`、`client_upgrade`、`Idle::new` 都是泛型的、不改。<br>④ **第 12 步应答的 Body Rewrite**：应答一侧的 Header Rewrite 那一段之后、`set_status` 之前。请求的方法在发出之前存下。`rewrite`、`url` 都有、`response_buffer(&method, status)`、`body_rules(Response, ..)` 不空时 `read_ahead(body, RESPONSE.bytes, conn.idle, handle.token(), Some(&request_meter))`：<br>　• `Whole` → 改写（⑥），头照 P13；<br>　• `Over { ahead, rest }` → 每条配上的规则一条 `Skipped("…: the body is over 10 MiB")`，正文 `resumed(ahead, rest)`；<br>　• `Stalled` → `return self.failed(&guard, pages, "upstream body stalled", 说明)`；`Failed(e)` → `"upstream body failed: {e}"`（说明照 P18）；那条上游连接随 `sender` 丢掉而关、不回池；<br>　• `Cancelled` → `Err(Close)`。<br>　别的情形 `ReadAhead::through(body)`。然后 `set_status`、`Tail::new`。<br>⑤ **`Tail`**：`UpstreamBody.inner` 是 `ReadAhead<Incoming>`；`poll_frame` 在锁里 poll 之前先记 `let counted = !up.inner.has_ahead();`，只有 `counted` 的数据帧 `count_down`（读在前面的在 `read_ahead` 里数过了，改写出来的不是上游的字节）。别处的 `Tail::new` 给 `ReadAhead::through(body)`。<br>⑥ **改写**：`let job = tokio::task::spawn_blocking(move \|\| { … rules.rewrite_body(..) … })`，交进去的是 `Arc<Rewrite>` 的克隆、下标、`Content-Encoding` 各行的克隆与正文的 `Bytes`（克隆不复制）；与 `handle.token().cancelled()` 赛跑（`biased`，令牌在前 → `Err(Close)`）；交回之后 `note_rewrites`。`JoinError` → ERROR 一行（P18），当作 `None`、没有备注 |
| P15 | Map Local 的答复 | **建（任务 4）**：`MapRule` 的数据：`File(路径)`；`Url { data: url_data(u.as_str()), origin: remote::origin(u) }`；`Bytes`（`text` 的 UTF-8 字节、`base64` 解开的字节、`tiny-gif` 是 `rurge_inbound::responses::TINY_GIF`——订正设计：第 10.3 节写的 `responses::tiny_gif()` 是一个应答，字节是 `TINY_GIF`）。状态码 `StatusCode::from_u16`（200 ～ 999 总是合法的）。头：转好的各行，`headers::strip(&mut h, Keep::NONE)`，没有 `Content-Type` 时照 P17 补；没有 `Content-Length`。<br>**`Rewrite::map_local(url, notes)`**：照写的次序比，第一条 `Ok(true)` 交回；`Err` 留 `Skipped("Map Local skipped: {式子}: the regular expression hit its backtracking limit")`、接着比。<br>**`Requests::map_local(guard, pages, rules, rule, method)`**：<br>• 状态码是 204 或 304：数据不读（文件不打开、URL 的数据不问），没有 `Content-Length`，正文空；<br>• `File`：`tokio::fs::File::open`，再 `metadata()`；打不开、读不到元数据或**不是普通文件**（目录等：三个平台上打开目录的结果不同，这样才一致）→ 失败；`HEAD` → 关掉文件，`Content-Length` 是大小、正文空；否则 `LocalFile`（P16）；<br>• `Url`：`data()` 是 `None` → 失败；<br>• `Bytes` 与有了的 URL 数据：`Content-Length` 是长度；`HEAD` 时正文空。<br>**成**：一条 `Done("Map Local: {类型} by {式子}")`、`map_local` 加一、`set_status`；正文不是 `LocalFile` 的，`finish(Completed)` 当场做，`LocalFile` 读完自己做。不 `mark_connected`：没有远端地址。<br>**败**：`failed` 加一，一条 `Failed`（P18），`self.failed(&guard, pages, 记录, 说明)`——记录是 `Map Local: cannot read {路径}` 或 `Map Local: the data from {来源} is not available yet`。<br>**读文件到一半出错**：记录照 P16，不计入 `failed`（这个请求已经由 Map Local 答复、计入 `map_local`；同上游正文出错，坏的是正文的递送）——订正设计第 10.3 节"读到一半出错…同上游正文出错"没有说的记录与计数。<br>请求的正文不读（同 M1b 的 `REJECT` 页）；要升级的请求得到的是普通的应答 |
| P16 | `LocalFile` 与看门狗 | **看门狗**：`Tail::watch_the_client` 的函数体原样搬进一个自由函数 `watch_the_client<T: Send + 'static>(conn, handle, left, progress, idle, slot, done)`，捕获的东西一样（`conn.abort`、`conn.tracker`、`conn.h2`、`conn.token`），`up` 换成 `slot`；`Tail` 改成调它。行为不变。<br>**`LocalFile`** 照 `Tail` 的样子写：字段是共用的一格 `Arc<Mutex<Option<tokio::io::Take<tokio::fs::File>>>>`（`AsyncReadExt::take(大小)`）、路径、余下的字节数、`idle` 与 `deadline`、请求令牌的 `cancelled`、`progress`、`watchdog`、`guard`、`reached_end`、`done`。<br>• `new`：大小是 0 → 当场 `complete`；否则 `watch_the_client(..)`。<br>• `poll_frame`：`done` → 结束；锁里读一次（一块至多 64 KiB、至多余下的字节，读进一块缓冲再拷成 `Bytes`）：<br>　读到 n > 0 → 重设 `deadline`、`progress.moved()`、余下的减 n，交出数据帧，余下是 0 → `complete`；<br>　读到 0 而还有余下的 → `fail("Map Local: {路径} became shorter while it was read")`、交出错误；<br>　出错 → `fail("Map Local: reading {路径} failed: {e}")`、交出错误（错误类型用 `UpstreamBodyFailed` 包着文字）；<br>　格子空了（看门狗放掉了，HTTP/2）→ 同 `Tail` 的 `Gone`；<br>　`Pending` → `progress.waiting()`；令牌取消 → `stop`、`Err(Close)`；`deadline` 到了 → `fail("Map Local: reading {路径} stalled")`、`Err(Stalled(..))`。<br>• `complete` / `fail` / `stop` / `Drop` 同 `Tail`（`complete` 是 `finish(Completed)`，没有要放回池里的连接）。<br>• `is_end_stream` 是 `reached_end`；`size_hint` 是确切的余下的字节数。<br>不加 `tokio-util` 的 `io` 特性、不加 `futures`（设计第 18 节 V12） |
| P17 | `Content-Type` 的表 | 表照抄表后「原文」的 P17 一块（MDN 的 "Common media types"，另加 RFC 8216 的 `m3u8`）。`content_type_for(name)`：`name` 是最后一段（文件取文件名，URL 取 `url.path()` 的最后一段）；扩展名是最后一个 `.` 之后的部分，那个 `.` 不能是第一个字（`.hidden` 没有扩展名）；ASCII 小写之后查表，查不到是 `application/octet-stream`。不带 `charset`。缺省（手册「About Content-Type」）：`text` 是 `text/plain`，`tiny-gif` 是 `image/gif`，`base64` 是 `application/octet-stream`，文件与 URL 查表。规则的 `header` 写了 `Content-Type`（不分大小写）时不补 |
| P18 | 备注、计数与日志 | 措辞**照抄表后「原文」的 P18 一块**。<br>**计数**：`body` 是正文被改过的请求，一个请求只数一次（两个方向都改也一次，P13）；`map_local` 是由 Map Local 答复的请求（`HEAD` 与 204、304 在内，502 不算）；`skipped` 是每一条 `Skipped`（`note_rewrites` 照旧数）；`failed` 加两种：请求正文超过上限、Map Local 的数据读不到（502 的那几种）。<br>**`should_warn(now)`**：`BodyRule.warned: Mutex<Option<Instant>>`；是 `None` 或 `now` 距它不少于 3600 秒时交回真并记下 `now`，否则假。它属于一代的 `Rewrite`，所以"每条规则每小时、每一代"自然成立（设计第 14 节）。<br>**日志**：chunked、`Expect` 与长度不知道的三种，每条配上的规则：`should_warn` 真时 WARN 一行（只带 `pattern`）；不论真假都 DEBUG 一行（带 `host` 与 `pattern`），`host` 是会话的目标主机。日志里没有 URL、查询串、正文与头的值 |
| P19 | `ResourceKind::MapLocal` 与 `build_rewrite`（订正设计） | 设计第 3、10.3 节的控制面名字 `map-local` 改成 **`data`**：手册 HTTP API 的 `GET /v1/external_resources` 列的种类是 "ruleset, domainset, script, policy-group and data"，`docs/api/phase1.md` 的那张表（第 124 行一带）与兼容性清单（L889 一带）已经把 `data` 对到 Map Local；`map-local` 是 Surge 的 API 从来不给的名字。<br>`ResourceKind::MapLocal`（在 `Include` 之后、`System` 之前），文档注释："The data of a `[Map Local]` line that names a URL (phase 4 M3 design 10.3)."。`type_name` 给 `Some("data")`；`ExternalResource.kind` 的文档注释加上它。<br>`Rewrite::build` 多一个参数 `url_data: &dyn Fn(&str) -> UrlData`（订正设计：第 3 节的签名没有它）：`rurge-http` 不必认识资源管理器怎样登记一类资源（种类、标签、更新间隔），它的用例也不必建一个资源管理器。<br>`build_rewrite(config, shared, resources)` 交给 `Rewrite::build` 的 `url_data`：把文字读回 `url::Url`；`ResourceSpec { source: ResourceSource::Url(url.clone()), update_interval: None, kind: ResourceKind::MapLocal }`；`key = resource_key(&spec.source)`；标签 `format!("map local data {} ({})", key.get(..8).unwrap_or(&key), origin(&url))`（照 `profile.rs` 的远程 include）；`resources.get_labelled(&spec, &label)`；交回 `Arc::new(move \|\| handle.current().data().map(\|(data, _)\| Bytes::clone(&data)))`。同一个 URL 写两次只登记一次（第一次登记定下种类与标签）。下载、缓存、大小上限（缺省 64 MiB）与更新间隔都照资源管理器；不联网的一代（`no_network`）从不下载，它的 Map Local URL 一律"not available yet" |
| P20 | 端到端的做法 | M3a P19 整条适用（规则照手册原文、源站的端口写在请求里、守卫监听器、`MockDns`、`[Host]` 钉住）。另：<br>• 答复指定的头与正文的源站（chunked 或带长度、gzip、trailer、发到一半停住）写在 `tests/rewrite.rs` 里（名字自定，注释写它为什么在那里）；要断言源站收到的正文用 `RecordingOrigin`；<br>• Map Local 的文件放在 `Harness.dir`（Profile 所在的目录），手册的相对路径 `data/map-local.json` 就接在那里；<br>• 大正文在内存里生成，上限用真的；超过上限的请求只发头；<br>• URL 的数据经 `online_runtime_with` 与回环上的 `TestServer`；等它下载好用 `wait_until` 看引擎的外部资源列表，不用固定的等待；<br>• 手册没有完整的 `^$` 行与 `$1` 行：那两条用例自己写一行，注释说明；与手册的别的出入（端口、主机）同样写在注释里，M6 写进验收文档 |
| P21 | 现有的用例 | 见下面的表。表里"预计照过"的若红，停下报告，不改期望 |
| P22 | 收尾文档的清单 | 见任务 8、9。兼容性清单登记设计第 15 节第 10 ～ 12 条，另登记本计划的六处：`base64` 收没有填充的、没有 `data` 的是空的正文（P5）；`Content-Type` 不带 `charset`（P17）；204 / 304 不读数据、`HEAD` 给长度不给正文（P15）；URL 的数据在控制面叫 `data`（P19）；缓冲应答时源站停住或出错答 502（P14）；读文件到一半出错不计入 `failed`（P15）。两份 README 与需求文档在任务 9 改 |

### 表后的原文（照抄）

P7、P17、P18 要照抄的文字含竖线与反引号，放在这里。每个代码块里的文字就是要的那一串，没有为 Markdown 加的转义。

**P7：诊断的消息与建议**

```text
W0012（[Body Rewrite]）
[Body Rewrite]: a line that is not `http-request | http-response <regex> <pattern> <replacement> [<pattern> <replacement> …]` is ignored
[Body Rewrite]: the regular expression cannot be used ({原因}); the line is ignored
[Body Rewrite]: a body pattern cannot be used ({原因}); the line is ignored

W0042 的消息（codes::W_JQ_REWRITE_UNAVAILABLE）
[Body Rewrite]: jq body rewrite is not available in this build of rurge; the line is ignored

W0012（[Map Local]）
[Map Local]: the regular expression cannot be used ({原因}); the line is ignored
[Map Local]: every item after the regular expression is a key=value parameter; the line is ignored
[Map Local]: data-type is file, text, tiny-gif or base64; the line is ignored
[Map Local]: data-type=file takes a path or a URL in data; the line is ignored
[Map Local]: data is not a URL rurge can download; the line is ignored
[Map Local]: data is not valid base64; the line is ignored
[Map Local]: header is `name:value|name:value` or base64 of header lines, with valid names and values; the line is ignored
[Map Local]: status-code is a whole number from 200 to 999; the line is ignored

W0001（[Map Local]）
[Map Local]: unknown parameter `{键}` ignored
[Map Local]: unknown parameter ignored

W0041（[Map Local]）的消息
[Map Local]: Content-Length and Transfer-Encoding in header are not used; the rest of the line is kept
W0041（[Map Local]）的建议（with_hint）
rurge frames the answer by the length of its data: remove the two names from header

W0033（[Map Local]）：消息是 "[Map Local]: " 接下面的消息，建议是下面的建议
map_local_refusal，ManagedCopy
  消息：the downloaded copy of a managed profile cannot name a local file for Map Local; the line is ignored
  建议：give `data` a URL instead
map_local_refusal，Nowhere
  消息：a downloaded file cannot name a local file for Map Local; the line is ignored
  建议：write the line in the local profile to map to a local file

codes::hint 的文档注释里 W0042 不设建议的理由
the message already says the line has no effect, and nothing in the profile but removing the line changes that
```

**P17：扩展名与 `Content-Type`**（左栏是 ASCII 小写的扩展名，一行几个的共用右栏）

```text
aac            audio/aac
apng           image/apng
avif           image/avif
bin            application/octet-stream
bmp            image/bmp
css            text/css
csv            text/csv
gif            image/gif
gz             application/gzip
htm html       text/html
ico            image/vnd.microsoft.icon
jpeg jpg       image/jpeg
js mjs         text/javascript
json           application/json
jsonld         application/ld+json
m3u8           application/vnd.apple.mpegurl
m4a            audio/mp4
md             text/markdown
mp3            audio/mpeg
mp4            video/mp4
mpeg           video/mpeg
oga opus       audio/ogg
ogv            video/ogg
otf            font/otf
pdf            application/pdf
png            image/png
svg            image/svg+xml
tif tiff       image/tiff
ts             video/mp2t
ttf            font/ttf
txt            text/plain
wasm           application/wasm
wav            audio/wav
weba           audio/webm
webm           video/webm
webmanifest    application/manifest+json
webp           image/webp
woff           font/woff
woff2          font/woff2
xhtml          application/xhtml+xml
xml            application/xml
zip            application/zip
```

**P18：备注、记录、错误页与日志**

```text
备注（{式子} 是规则的 URL 式子照写；{方向} 是 request 或 response；{类型} 是 file、text、base64 或 tiny-gif）
Body Rewrite: {方向} by {式子}
Body Rewrite skipped: {式子}: {原因}
Body Rewrite failed: {式子}: the request body is over 32 MiB
Map Local: {类型} by {式子}
Map Local skipped: {式子}: the regular expression hit its backtracking limit
Map Local failed: {式子}: the file cannot be read
Map Local failed: {式子}: the data is not available yet

Body Rewrite skipped 的原因
the body is not valid UTF-8
the body is over 10 MiB
the decoded body is over 32 MiB
the decoded body is over 10 MiB
content-encoding {名字} is not supported
the content-encoding is not supported
the body cannot be decoded as {名字}
the request body is chunked
the request expects 100-continue
the request body has no length
the regular expression hit its backtracking limit

记录（SessionOutcome::Failed 的文字；{路径} 是 Path::display，{来源} 是 rurge_config::remote::origin，{错误} 是错误的 Display）
Body Rewrite: the request body is over 32 MiB
request body stalled
request body failed: {错误}
upstream body stalled
upstream body failed: {错误}
Map Local: cannot read {路径}
Map Local: the data from {来源} is not available yet
Map Local: reading {路径} failed: {错误}
Map Local: {路径} became shorter while it was read
Map Local: reading {路径} stalled

错误页的说明（failed 的最后一个参数；标题照 failed 是 Connection failed）
Map Local 的文件读不到：The file a Map Local rule names cannot be read.
Map Local 的 URL 还没有数据：The data a Map Local rule names has not been downloaded yet.
缓冲应答时源站停住：The upstream server stopped sending the response body.
缓冲应答时正文出错：The upstream server's response body failed.

日志
WARN   消息 request body not rewritten: {原因}    字段 pattern = {式子}
DEBUG  消息 request body not rewritten: {原因}    字段 host = {目标主机}、pattern = {式子}
ERROR  消息 body rewrite task failed: {错误}
```

### P21 的表：要改的、要留意的现有用例

| 用例 | 为什么 | 任务 | 预计 |
| ---- | ------ | ---- | ---- |
| `crates/rurge-config/src/deferred.rs` 的 `prefix_and_membership_matching` | 两个节不再延后 | 1、2 | 改：M3a 加的 `is_deferred("Body Rewrite")`、`is_deferred("Map Local")` 两句翻成 `!is_deferred(..)` |
| `crates/rurge-config/src/diagnostic.rs` 的 `every_code_is_decided_once` 与查缺省建议的那两条 | 多一个码 | 1 | 改：`WITHOUT` 加 `W0042`（先红） |
| 语料库快照 `corpus__corpus__kitchen-sink.snap` | `deferred` 少两项、`W0016` 的消息少两节、多一条 `W0042` | 1、2 | 改：手改（P8）；`kitchen-sink.conf` 的 Body Rewrite 一行与 Map Local 两行读得了 |
| `crates/rurge-config/src/config.rs` 里断言 `W0016` 名单或 `deferred` 内容、或写着这两节的用例（任务 0 列全） | 名单少两个 | 1、2 | 列出的逐条看：断言的是这两节的改；不涉及的照过 |
| `crates/rurge-config/src/config.rs` 的 `rewrite_sections_are_read_and_no_longer_deferred` | `RewriteConfig` 多两个字段 | 1、2 | 照过 |
| 工作区里每一处 `RewriteConfig { … }` 的写法（`rurge-http` 的 `read_rules` 等，任务 0 列全） | 多两个字段 | 1、2 | 改：只补字段，断言不改 |
| `crates/rurge-config` 的 `rewrite.rs`、`dialect.rs`、`value.rs`、`rule.rs`、`reach.rs`、`redact.rs` 的现有用例与 `tests/secrets.rs` | 只加 | 1、2 | 照过 |
| `crates/rurge/tests/cli.rs` 里 `rurge check` 的输出写着 `[Body Rewrite]` 或 `[Map Local]` 的用例（任务 0 列全） | 不再 `W0016` | 1、2 | 列出的逐条看 |
| `crates/rurge-http/src/rewrite/mod.rs` 的 `counters_count_and_snapshot` | `body` 与 `map_local` 开始计 | 4 | 改：六个都各加一次、都是 1 |
| `crates/rurge-http/src/rewrite/` 与 `engine.rs` 测试模块里建 `Rewrite` 的夹具（`rewrite_of` 等） | `build` 多一个参数 | 4 | 改：只加参数 |
| `crates/rurge-http/src/engine.rs` 测试模块的 `rewrite_notes` | 多两种备注 | 5 | 改：前缀加 `Body Rewrite`、`Map Local` |
| `crates/rurge-http/src/engine.rs` 测试模块的 `counted` | M3a 的用例里 `body`、`map_local` 恒为 0 | 5 | 照过；M3b 的用例另写断言 |
| `crates/rurge-http/src/engine.rs` 测试模块的其余全部（`Tail`、`Idle`、看门狗、M2b 的计字节） | `ReadAhead` 接进 `Tail`、看门狗抽成函数 | 5 | 照过 |
| `crates/rurge-engine/src/external_resources.rs` 的 `set_kind` | 多一种 | 6 | 改：加 `MapLocal` → `"data"` |
| `crates/rurge-engine/tests/external_resources.rs` 的 `the_listing_names_each_kind_and_leaves_the_hosts_file_out` | 多一种 | 6 | 照过（它的 Profile 没有 Map Local） |
| `crates/rurge-engine/tests/rewrite.rs`、`http.rs`、`mitm.rs`、`sniff.rs`、`pipeline.rs` 的全部 | 加了三步 | 4 ～ 7 | 照过 |
| `crates/rurge-api/tests/api.rs` 的 `rurge_http_counts_start_at_zero` | 两个数开始计 | 5 | 照过（起始仍是 0） |
| `crates/rurge/tests/sockets.rs` 的扫描用例 | 不建套接字 | 3 ～ 7 | 照过 |
| `crates/rurge-platform/tests/unsafe_functions.rs` | 不加放开的函数 | 全部 | 照过 |

---

## 执行期修正记录

下面写"执行者的裁定"的，是 Pi 在设计与计划的原则下定的；写"协调者的裁定"的，是协调的会话代项目所有者定的。项目所有者都可以改。计划的正文不改：正文里说得不对的地方，以这张表的行为准。

| # | 任务 | 内容 | 提交 |
| - | ---- | ---- | ---- |
| 1 | 0 | 开工前核对（第 1、2 项）：`git status --short` 空；分支 `phase4-m3`；`HEAD` 是 `5bc54e6`；`git log --oneline ba6646a..HEAD` 只有 `5bc54e6 docs(plan): 阶段 4 / M3b 实施计划（Body Rewrite 的正则形式与 Map Local）`，与派工写的"`ba6646a` 之后只多了本计划的提交"一致。读了设计第 22 节（先读，全节：第 1 ～ 12 条是 M3a 的、第 13 ～ 21 条是本计划的）、第 3 节（crate 与接口表，`BodyPattern` / `MapData` / `Rewrite::build` / `Routed` 的形状）、第 5.4、5.5 节、第 8 节（8.1 的 0 ～ 13 步表、8.2 的 `retarget` 与对上游 TLS、8.4 的"不过重写"、8.5 的备注）、第 9 ～ 17 节、第 18 节（V1 ～ V15）、第 19 节（12 条）；本计划的 P1 ～ P22、P21 的表与表后「原文」（P7、P17、P18）逐字；`docs/status.md` 阶段 4 / M3a 一节 | 本提交 |
| 2 | 0 | 名字与签名对得上的（第 3 项），`rurge-config`（任务 1）：`dialect.rs:1-11` 的模块注释、`:27` 的 `pub fn translate(source: &str) -> Result<String, String>`、`:171` 的 `Template`（`{ pieces: Vec<Piece> }`，手写 `Debug` 写 `Template(***)`）、`:186` 的 `Template::parse(text: &str) -> Template`、`:231` 的 `expand<'a>(&self, group: &dyn Fn(usize) -> Option<&'a str>, count: usize, out: &mut String)`，测试模块 `:262` 起（`use super::*;` 与 `use crate::rule::Pattern;`，第一条用例 `translate_rewrites_the_icu_escapes` 从 `:270`）；`rule.rs:171` 的 `Pattern { pub source: String, pub regex: fancy_regex::Regex }`、`:182` 的 `new(source: &str) -> Result<Pattern, String>`、`:199` 的 `PartialEq`（比 `source`）、`:203` 的 `Debug`（`Pattern({source:?})`），`BodyPattern` 照它写；`rewrite.rs:1-21` 的模块注释、`:27` 的 `RewriteConfig { pub url, pub header }`（`#[derive(Clone, Debug, Default, PartialEq, Eq)]`）、`:34` 的 `is_empty`（两个都空才真）、`:65` 的 `Direction { Request, Response }`、`:169` 的 `parse_url<'a>(entries: impl IntoIterator<Item = &'a Entry>, diags: &mut Diagnostics) -> Vec<UrlRewrite>`、`:220` 的 `parse_header`、`:91-137` 的十一个消息常量（`URL_SHAPE`、`URL_REGEX`、`HEADER_SHAPE`、`HEADER_NAME`、`HEADER_VALUE`、`HEADER_CONTROL`、`HEADER_DEL_EXTRA`、`HEADER_REGEX_SHAPE`、`HEADER_REGEX`、`FRAMING`、`FRAMING_HINT`）、`:374` 的 `fn is_field_name(text: &str) -> bool`（私有）、`:400` 的 `fn can_be_a_value(c: char) -> bool`（私有），测试模块 `:406` 起（`read_url` 在 `:414`、`read_header` 在 `:428`、`said` 在 `:441`，`url_rewrite_reads_the_manual_example` 从 `:446`）；`config.rs:937-953` 的 `RewriteConfig { url: rewrite::parse_url(…), header: rewrite::parse_header(…) }`（在 `from_profile` 里，`:661-666` 的签名带 `base_dir: &Path`，`:684-688` 的 `ctx_for` 造 `ParseCtx { inline_rulesets, base_dir, reach }`）、`:2743` 的 `rewrite_sections_are_read_and_no_longer_deferred`（`:2759-2770` 断言没有 `W0016`/`W0002`/`W0012` 并且 `deferred.get(..)` 是 `None`）；`deferred.rs:6` 的 `DEFERRED`（头两项就是这两个节）、`:21` 的 `is_deferred`、`:47` 的 `prefix_and_membership_matching`（`:71`、`:72` 两句是 `assert!(is_deferred("Body Rewrite"))` 与 `assert!(is_deferred("Map Local"))`）；`diagnostic.rs:203` 的 `W_UNKNOWN_KEY`、`:214` 的 `W_INVALID_VALUE`、`:260` 的 `W_REMOTE_LOCAL_ACCESS`、`:287` 的 `W_REWRITE_FRAMING`、`:299` 的 `I_REPLICA_REMOVED`、`:305-344` 的 `codes::hint` 文档注释（`W0001`、`W0033`、`I0001` ～ `I0005` 那几句都在）、`:346` 的 `hint` 与末尾的 `_ => return None`（`:422`）、`:437` 的 `WITH_DEFAULT`（38 项，末项 `W_API_TLS_NO_CA`）、`:479` 的 `PER_SITE`（四项）、`:487` 的 `WITHOUT`（27 项，末项 `I_REPLICA_REMOVED`）、`:543` 的 `every_code_is_decided_once`；`lib.rs:44` 的 `pub use dialect::Template;`、`:61` 的 `pub use rewrite::{Direction, HeaderAction, HeaderRewrite, RewriteConfig, UrlMode, UrlRewrite};`（四个新名字在这里加）；语料库 `tests/corpus/valid/kitchen-sink.conf:168-174`（**`[Body Rewrite]` 在 `:168`、`http-response` 那行在 `:169`、jq 的 `http-response-jq` 在 `:170`、`[Map Local]` 在 `:172-174`**）与快照 `crates/rurge-config/tests/snapshots/corpus__corpus__kitchen-sink.snap:97-107`（`deferred:` 的十项，头两项是这两个节）、`:110`（一条 `W0016` 列十个节）、`:111-117`（诊断按行号排，最后一条是 `:117` 的 `W0038`，说 `kitchen-sink.conf` 第 104 行；`W0042` 落在第 170 行，加在末尾） | 本提交 |
| 3 | 0 | 名字与签名对得上的（第 3 项），`rurge-config`（任务 2）：`reach.rs` 共 110 行：`Reach { Anywhere, ManagedCopy, Nowhere }`（`:11-24`）、`pub(crate) struct Refusal { message, hint }`（`:29-33`）、三个 `*_refusal`（`include_refusal` `:40`、`set_refusal` `:58`、`policy_path_refusal` `:76`，`Anywhere` 都交 `None`）；`text/mod.rs:12` 的 `Origin`（`Main`、`Include`、`Module`、`Remote`、`Managed`）、`:27` 的 `Origin::reach`（`Main`/`Include` → `Anywhere`、`Managed` → `ManagedCopy`、`Module`/`Remote` → `Nowhere`）、`:67` 的 `Entry { raw, span, origin, requirement, disabled }`、`:87` 的 `Section::active_entries`、`:182-195` 的 `ORDERED_SECTIONS`（**这两个节已经在里面**，`section_kind` 不用改）；`rule.rs:18` 的 `ParseCtx { inline_rulesets, base_dir, reach }`、`:148-168` 的 `parse_external` 接相对路径那一段（`:153-158` 先问 `ctx.reach.set_refusal()` 给 `W0033`，`:163-167` 绝对的照用、相对的 `ctx.base_dir.join(path)`）；`spec/reader.rs:48` 的 `fn is_plain_name(text: &str) -> bool`（**私有**：1 ～ 32 个 ASCII 小写字母、数字、`-`、`_`），P7 要照的两句在 `spec/group.rs:303-313`（`written_as_name(..)` 真才写 `unknown parameter `{key}` ignored`，否则 `unknown parameter ignored`）；`config.rs:1078-1081` 的 `is_base64`（`STANDARD.decode(text).is_ok() \|\| STANDARD_NO_PAD.decode(text).is_ok()`）、`remote.rs:151` 的 `pub fn origin(url: &Url) -> String`（`{scheme}://{BeforeHost..AfterPort}/…`）；经远程 include 断言 `W0033` 的现有用例是 `config.rs:5732` 的 `a_remote_text_cannot_name_a_local_file_as_a_set_in_a_rule`（`Fake::new().text(link, …)` + `load_with_remote` + `refusals(&l)` 与 `refused("1.conf", &[2, 3, 4, 5], SET_IN_DOWNLOADED, SET_IN_DOWNLOADED_HINT)` + `assert_no_path_is_shown`，辅助件 `refusals` / `refused` / `assert_no_path_is_shown` 在 `:5664-5726`），任务 2 的 `a_map_local_path_from_downloaded_text_is_refused` 照它写；`crates/rurge/src/capabilities.rs:20-24` 的 `CORE_VERSION` 注释与值（`22`，要换的两句原文在 P8）；`docs/surge-compatibility-matrix.md:98` 那一行（`变量 CORE_VERSION（整数）`）里有 `20（Mac 5.6.0 / iOS 5.10.0，Body Rewrite、Inline Map Local）`，是任务 9 改的那一处 | 本提交 |
| 4 | 0 | 名字与签名对得上的（第 3 项），`rurge-http/src/rewrite/mod.rs`（任务 3、4）：`:1-17` 的模块注释（"Nothing here touches the network or a session"）、`:19-20` 的 `mod header; mod url;`（`body.rs` 与 `map_local.rs` 加在这里）、`:50` 的 `Note { Done, Skipped, Failed }`（**只有 `text()`，没有手写的 `Display` / `Debug`**）、`:63` 的 `RewriteCounts { url, header, body, map_local, skipped, failed }`（文档注释 `:61-62` 写"`body` and `map_local` stay at zero until M3b"，任务 4 要改这一句）、`:75` 的 `RewriteCounters`（六个 `AtomicU64`，`snapshot` 在 `:85`，`:98-117` 只有 `url` / `header` / `skipped` / `failed` 四个加法，**没有 `body()` 与 `map_local()`**，任务 4 加）、`:120` 的 `Rewrite { url, header, config, h1, counters }`、`:191` 的 `build(cfg: &RewriteConfig, mitm: &MitmConfig, roots: Arc<RootCertStore>, counters: Arc<RewriteCounters>) -> Result<Option<Rewrite>, BuildError>`（`cfg.is_empty()` 交 `Ok(None)`，P19 在这里加第五个参数）、`:242` 的 `has_response_rules`、`:280` 的 `pub(super) fn replace_all(pattern: &rurge_config::rule::Pattern, template: &rurge_config::dialect::Template, text: &str) -> Result<String, ()>`（组数是 `captures_len() - 1`，`Err(())` 是回溯超限）；测试夹具 `config_of`（`:384`，只放过 `W0041`）、`read_rules`（`:403`，**工作区里唯一一处测试用的 `RewriteConfig { … }`**，任务 1 在这里补 `body`、任务 2 补 `map_local`）、`test_roots`（`:431`）、`rewrite_of`（`:442`，交 `(Arc<Rewrite>, Arc<RewriteCounters>)`，任务 4 在这里补 `build` 的参数）、`build_gives_none_without_lines`（`:458`）、`counters_count_and_snapshot`（`:522`，六个数的断言在 `:531-543`） | 本提交 |
| 5 | 0 | `rurge-http/src/engine.rs` 的 `Requests::handle`（`pub async fn handle(&self, req: Request<Incoming>) -> Result<Response<ResponseBody>, Close>`，`:395`）：段 → 行号 → 那一行的代码。`:412` 段 0 `if self.shared.conn.h2 && req.method() == http::Method::CONNECT`；`:429` 段 1 `let authority = match self.shared.conn.entry`；`:461` 段 2 `let handle = self.shared.upstreams.open(prepared.session)` 与 `Guard::new`；`:471` 段 3 `self.shared.upstreams.route(&handle, prepared.forwardable)`；`:480` 的 `match routed`、`:486` 的 `Routed::Go {`、`:493` 的 `if prepared.unknown`；`:505` 的 `let sniffed = self.shared.conn.entry == Entry::Sniffed`；`:508-515` 的两种不过重写的请求 `if sniffed && (req.method() == http::Method::CONNECT \|\| (!self.shared.conn.h2 && req.uri().scheme().is_some()))` → `rewrite = None`；`:520` 的 `let mut url = handle.session().url.clone()`；`:524` 的 `let mut origin = self.shared.conn.origin.clone()`；`:531-556` 请求一侧的 Header Rewrite（`:533` 的 `rules.headers(Direction::Request, text, req.headers_mut(), &mut notes)`、`:539` 的 `headers_changed = true`、`:548-555` 的 `out.framing` 交 `self.failed`）；`:557-621` 的 URL Rewrite（`:568` 的 `rules.url(&urls, &\|\| self.shared.upstreams.gateway(), &mut notes)`、`:571` 的 `UrlOutcome::Go(Some(new))`、`:595` 一带把 `origin` 换成新的 `OriginTls`、`:600` 的 `Redirect`、`:607` 的 `Reject`、`:621` 的 `UrlOutcome::Go(None) => {}` 是它的结尾）；`:628` 的 `let upgrade = wants_upgrade(&req)`；`:631` 的 `let tunnel = sniffed && req.method() == http::Method::CONNECT`；`:632` 的 `let needs_h1 = upgrade \|\| tunnel`；`:644` 的 `let mut key = Some((key, OriginKey::of(origin.as_ref())))`；`:715` 的 `let form = Form::of(&req, fallback)`；`:716-724` 的 `headers::strip(req.headers_mut(), Keep { upgrade, proxy_authorization: sniffed && !retargeted })`；`:742-746` 的补 `Transfer-Encoding: chunked`（`:742` 的 `if !req.body().is_end_stream() && req.body().size_hint().exact().is_none()`）；`:784` 的 `let head = (route.is_some() …)`；`:788` 的 `let client_upgrade = (upgrade \|\| tunnel).then(\|\| hyper::upgrade::on(&mut req))`；`:795` 的 `let request_meter = Meter::new()`；`:799` 的 `Idle::new(request_body, idle, progress.clone(), request_meter.clone()).boxed()`；`:810-815` 的发送 `tokio::select!`（`biased`，令牌在前）；`:955-994` 升级的那一支（`:955` 的 `if (upgrade && switching) \|\| (tunnel && resp.status().is_success())`、`:970-984` 的 101 的头也过 Header Rewrite、`:991` 的 `self.promote(…)`）；`:1001` 的 `headers::strip(resp.headers_mut(), Keep::NONE)`；`:1006-1024` 应答一侧的 Header Rewrite（`:1010` 的 `rules.headers(Direction::Response, …)`）；`:1025` 的 `handle.set_status(resp.status().as_u16())`；`:1027-1039` 的 `let outing = Outing { key, sender, meter, peer, forward }`；`:1041` 的 `let tail = Tail::new(body, guard, self.shared.clone(), outing, request_meter)`。**任务 5 的三处插入点**：第 6 步 Map Local 在 `:621` 之后、`:628` 之前；第 7 步在 `:632` 之后、`:644` 之前；第 12 步在 `:1024` 之后、`:1025` 之前 | 本提交 |
| 6 | 0 | 名字与签名对得上的（第 3 项），`engine.rs` 的其余类型与夹具、`pool.rs`、`rurge-inbound`（任务 5）：`type ReqBody = BoxBody<Bytes, BoxError>`（`:225`）；`struct Outing<K> { key, sender, meter, peer, forward }`（`:1982-1996`）；`struct UpstreamBody<K> { inner: Incoming, outing: Box<Outing<K>> }`（`:2002`，P14 ⑤ 要 `inner` 换成 `ReadAhead<Incoming>`）；`struct UpstreamBodyFailed(String)`（`:2061`，手写 `Display`）、`struct Stalled(&'static str)`（`:2075`，同）；`struct Progress`（`:2091`，`new` 在 `:2107`、`moved` 在 `:2134`、`waiting` 在 `:2141`、`remaining` 在 `:2150`）；`struct Idle<B> { inner, idle, deadline, progress, body }`（`:2180`）与 `Idle::new(inner: B, idle: Duration, progress: Arc<Progress>, body: Meter) -> Idle<B>`（`:2195`）；`struct Tail<U: Upstreams>`（`:2267`，字段 `up`（`Arc<Mutex<Option<UpstreamBody<…>>>>`）、`idle`、`deadline`、`cancelled`、`progress`、`body`、`watchdog`、`guard`、`shared`、`reached_end`、`done`）、`Tail::new(inner: Incoming, guard: Guard, shared: Arc<Shared<U>>, outing: Outing<UpstreamKey<U>>, body: Meter) -> Tail<U>`（`:2300`）、`take_upstream`（`:2347`）、`watch_the_client(&self)`（`:2361`，P16 把它原样搬成自由函数）、`complete`（`:2431`）、`fail`（`:2471`）、`stop`（`:2482`）、`impl Drop for Tail`（`:2493`）、`enum PolledUpstream { Frame(Frame<Bytes>, bool), Failed(hyper::Error), End, Pending, Gone }`（`:2507`）、`poll_frame`（`:2519`）、`is_end_stream`（`:2610`，读 `reached_end`）、`size_hint`（`:2614`）；`outcome_of(handle: &SessionHandle, stop: &CancellationToken, left: &'static str) -> SessionOutcome`（`:2633`）；`struct Guard`（`:2652`）、`Guard::new(handle, stop, activity, left)`（`:2665`）、`handle`（`:2680`）、`outcome`（`:2686`）、`retire`（`:2692`）、`impl Drop for Guard`（`:2697`）；`answer`（`:1120`）、`failed`（`:1170`）、`timed_out`（`:1189`）、`page`（`:1207`）、`upstream_failed`（`:1512`）、`note_rewrites(handle: &Arc<SessionHandle>, rewrite: &Rewrite, notes: Vec<crate::rewrite::Note>)`（`:1797`，`Skipped` 在这里数 `skipped`）；`Tail::new(` 的调用处**只有 `:1041` 一处**；`pool.rs:103` 的 `pub struct Meter(Arc<Mutex<Option<Arc<SessionHandle>>>>)`、`set(&self, handle: Option<Arc<SessionHandle>>)`（`:111`）、`count_up(&self, n: u64)`（`:125`）、`count_down(&self, n: u64)`（`:133`）；`crates/rurge-inbound/src/session.rs:169` 的 `token()`、`:231` 的 `add_note(impl Into<String>)`、`:235` 的 `notes() -> Vec<String>`、`:279` 的 `set_status(u16)`、`:386` 的 `finish(SessionOutcome)`、`:289` 的 `add_up(u64)`、`:293` 的 `add_down(u64)`、`:298` 的 `bytes() -> (u64, u64)`；测试模块的夹具：`struct Fake(Arc<Inner>)`（`:3025`，`Inner` 的 `rewrite`、`retargets`、`gateway`、`go_pages`、`forward` 在 `:3000-3018`，`Fake::new` 在 `:3031`）、`dials`（`:3074`）、`retargets`（`:3079`）、`rewrite`（`:3083`）、`gateway`（`:3088`）、`go_pages`（`:3093`）、`forward`（`:3098`）、`seen`（`:3101`）；`struct Rig`（`:3342`）、`connected`（`:3364`）、`rig_timed`（`:3374`）、`rig_head_wait`（`:3386`）、`rig_tls`（`:3414`）、`rig_tls_h2`（`:3436`）、`recording_origin`（`:4008`）、`roots_of`（`:5105`）、`rewrite_of`（`:6714`）、`counted(url, header, skipped, failed)`（`:6732`，`body` 与 `map_local` 写 0）、`rewrite_notes`（`:6744`，只看 `URL Rewrite` 与 `Header Rewrite` 两个前缀，任务 5 加两个）、`header_value`（`:6753`）、`closing_origin`（`:6763`）；测"客户端不读了"的两条现有用例是 `:4797` 的 `a_client_that_stops_reading_fails_after_idle`（HTTP/1.1）与 `:6400` 的 `an_h2_client_that_stops_reading_one_stream_fails_only_that_request`（HTTP/2 上只失败那一个流），任务 5 的 `a_local_file_nobody_reads_is_let_go` 照前一条写；M2b P11 那条是 `:6125` 的 `the_bytes_of_requests_on_a_shared_h2_connection_are_their_bodies`（`:6164-6168` 断言第二个请求的 `bytes()` 是 `(100, 6)`） | 本提交 |
| 7 | 0 | 名字与签名对得上的（第 3 项），`rurge-net`、`rurge-engine` 与端到端夹具（任务 6、7）与 `rurge-inbound` 的应答（任务 4、5）：`resource/mod.rs:27` 的 `DEFAULT_UPDATE_INTERVAL = 86_400`（`update_interval: None` 用它，`:451`）、`:49` 的 `ResourceKind`（**七个变体，次序 `RuleSet`、`DomainSet`、`PolicyGroup`、`GeoIp`、`Profile`、`Include`、`System`**，P19 把 `MapLocal` 插在 `Include` 与 `System` 之间）、`:67` 的 `ResourceSource { Url(Url), File(PathBuf) }`、`:110` 的 `ResourceSpec { source, update_interval, kind }`、`:123` 的 `ResourceOptions.max_size`（缺省 `64 * 1024 * 1024`，`:138`）、`:198` 的 `ResourceState::data() -> Option<(Arc<Bytes>, u64)>`（`Missing` 与没有 `cached` 的 `Failed` 都交 `None`）、`:259` 的 `pub fn resource_key(source: &ResourceSource) -> String`（`cache::name_of(&source.key())`，同一个 URL 就是同一个键）、`:461` 的 `ResourceHandle::current() -> ResourceState`、`:640` 的 `get_labelled(&self, spec: &ResourceSpec, label: &str) -> ResourceHandle`（`register` 在 `:658` 按 `spec.source.key()` 去重，第一次登记定下种类与标签）；测试的 `manager(root: &Path, opts: ResourceOptions)`（`:1645`）与 `struct NoNames`（`:1565`）；`testing` 特性下的 `TestServer`（`crates/rurge-net/src/testing.rs:67`）的 `spawn`（`:75`）、`spawn_tls_with(acceptor)`（`:85`）、`url(&self, path: &str) -> Url`（`:134`）、`set(&self, path: &str, body: impl Into<Bytes>)`（`:139`，**用例中途可以再调它换同一个路径的应答**）、`set_status`（`:170`）、`set_delay`（`:157`）；`crates/rurge-engine/src/runtime.rs:302` 的 `async fn build_rewrite(config: &Config, shared: &EngineShared) -> Option<Arc<Rewrite>>`（取根那一段就是 P19 要照拄的）与它的调用处 `:176`（`let rewrite = build_rewrite(&config, &opts.shared).await`，资源管理器那时在 `stack.resources` 里，`:149` 已经在用）、`:61` 的 `build_mitm` 旁边就是它；`external_resources.rs:34` 的 `ExternalResource`（`kind: &'static str` 的注释列六个名字）、`:80-89` 的 `fn type_name(kind: ResourceKind) -> Option<&'static str>`（**穷尽的 `match`、没有 `_` 分支**，P19 要加 `MapLocal => Some("data")`）、`:116-121` 的 `fn set_kind(kind) -> Option<SetKind>`（有 `_ => None`，**不用改**）、`:104-113` 的 `in_listing_order`、`:142` 的 `lists`；`tests/external_resources.rs:279` 的 `the_listing_names_each_kind_and_leaves_the_hosts_file_out`（`:292` 断言 `listing.len() == 6`，那份 Profile 没有 Map Local）、`:239` 的 `kinds`；`crates/rurge-inbound/src/responses.rs:15` 的 `pub type ResponseBody = BoxBody<Bytes, BoxError>`（**装箱的正文，能装流式的一种**）、`:18` 的 `pub const TINY_GIF: [u8; 43]`（前六个字节 `0x47 0x49 0x46 0x38 0x39 0x61` 即 `GIF89a`，后两个字节是 `0x01 0x00 0x01 0x00`，是 1×1）、`:24` 的 `pub fn full(bytes: Bytes) -> ResponseBody`（造 `Full` 正文用 `Full::new(bytes).map_err(\|never\| match never {}).boxed()`）、`:41` 的 `pub fn tiny_gif() -> Response<ResponseBody>`（**是一个应答**，字节才是 `TINY_GIF`）、`:78` 的 `ErrorPage`、`:102` 的 `pub fn error_page(status: StatusCode, page: &ErrorPage<'_>) -> Response<ResponseBody>`（`Content-Type: text/html; charset=utf-8`） | 本提交 |
| 8 | 0 | 与计划对不上的、以及计划没写到的（后面的任务照这里做）：① P21 说 `crates/rurge-engine/src/external_resources.rs` 的 **`set_kind`** 要改，代码是 **`type_name`**（`:80-89`，那个穷尽的 `match` 才需要 `MapLocal => Some("data")`；`set_kind`（`:116-121`）只把两类集合映成 `SetKind`，有 `_ => None`，**不用改**）；设计第 3 节写的也是 `type_name`。② 同文件 `:417-430` 的 `only_a_rule_set_and_a_domain_set_are_read_as_sets` 用数组列了五个非集合变体（`PolicyGroup`、`GeoIp`、`Profile`、`Include`、`System`），**不改也照过**，但就不再盖住新的一类；任务 6 把 `MapLocal` 加进那个数组（断言仍是一字不改）。③ P21 写"`cli.rs` 里 `rurge check` 的输出写着这两节的用例（任务 0 列全）"：**一条也没有**（`crates/rurge/tests/cli.rs`、`crates/rurge-api/tests/`、`crates/rurge-engine/tests/` 里提 `W0016` 与那句 `sections parsed but inactive` 的都是零，`git grep` 扫过），这一项按"没有可改的"做。④ 断言 `W0016` 或 `deferred` 的现有用例全在 `crates/rurge-config/src/config.rs`，共六条：`warnings`（`:2643`，deferred 里只有 `[Script]`）、`ip_rewrite_is_read_in_order`（`:2678`）、`replica_is_known_and_ignored`（`:2776`，断言 `!c.contains(&W_DEFERRED_SECTION)`）、`rewrite_sections_are_read_and_no_longer_deferred`（`:2743`）、`wireguard_sections_are_typed`（`:4103`，断言 `deferred.sections.is_empty()`）、`the_mitm_section_is_typed_and_no_longer_deferred`（`:6353`）、`an_ssid_setting_section_is_read_and_no_longer_inactive`（`:6374`，逐字断言 `sections parsed but inactive in this version: [Script]`）——**七条一条也不用改**（它们里的 deferred 项只涉 `[Script]` 与各自的节，没有一条涉 `[Body Rewrite]` 或 `[Map Local]`）。⑤ `改写节读法的文档注释`（任务 1、2）：`config.rs:251-252` 写"`[Body Rewrite]` and `[Map Local]` follow in M3b"，任务 1 之后只有 Map Local 还没读，任务 2 之后这句完全过时——两句都要改；`rurge-http/src/rewrite/mod.rs:61-62`、`crates/rurge-http/src/engine.rs:6730-6731` 与 `crates/rurge-api/src/routes/rurge_http.rs:25-27` 的"until M3b"同样过时（任务 4 改前两个、任务 5 改第三个；它们不是会红的用例，只是文字）。⑥ `RewriteConfig {` 的构造全仓只有两处：`crates/rurge-config/src/config.rs:937` 与 `crates/rurge-http/src/rewrite/mod.rs:409`；`RewriteCounts {` 有三处（`rewrite/mod.rs:86` 的 `snapshot`、`:531` 的测试、`engine.rs:6733` 的 `counted`），**一个字段也不加**（`body` 与 `map_local` 已经在了）；`HttpCounts` 在 `crates/rurge-engine/src/serve.rs:86`，构造处 `engine.rs:1516` 与 `tests/sniff.rs:958`，`HttpJson` 在 `crates/rurge-api/src/routes/rurge_http.rs:14`、构造处 `:68`，`ExternalResource {` 只有 `external_resources.rs:207` 与 `crates/rurge-api/src/routes/resources.rs:155` 两处，**都不加字段**，所以 M3b 不会把它们编不过。⑦ P5 说“键照 `spec/group.rs` 那两句的写法先问 `is_plain_name`”，但那个函数在 `crates/rurge-config/src/spec/reader.rs:48` 是**私有的 `fn`**（`spec` 模块之外看不到）；任务 2 要把它改成 `pub(crate)`（不改语义、不改它的用例），或在 `rewrite.rs` 里写一个同形的；执行者选前者（照仓库现有的分层），在提交说明里写一句 | 本提交 |
| 9 | 0 | regex 1.13.1 的事实（任务 1；`regex-automata` 0.4.18，`regex-syntax` 0.8.11）：`RegexBuilder::new(pattern: &str) -> RegexBuilder`（`regex-1.13.1/src/builders.rs:223`）与 `multi_line(&mut self, yes: bool) -> &mut RegexBuilder`（`:361`）、`crlf(&mut self, yes: bool) -> &mut RegexBuilder`（`:450`）、`build(&self) -> Result<Regex, Error>`（`:232`）；`Regex::captures_iter<'r, 'h>(&'r self, haystack: &'h str) -> CaptureMatches<'r, 'h>`（`src/regex/string.rs:469`，`Item` 是 `Captures`、**不是 `Result`**）、`Regex::captures_len(&self) -> usize`（`:1405`，**算第 0 组**，文档 `:1416-1418` 说`至少为 1`）、`Captures::get(&self, i: usize) -> Option<Match<'h>>`（`:1721`）。**多行 + CRLF 下 `^` `$` 在哪里匹配**（`regex-automata-0.4.18/src/util/look.rs:955-960` 的 `is_start_crlf` 与 `:970-975` 的 `is_end_crlf`）：`^` 在 `at == 0`、或前一个字是 `\n`、或前一个是 `\r` 而下一个不是 `\n`；`$` 在 `at == len`、或当前字是 `\r`、或当前是 `\n` 而前一个字不是 `\r`。由此：`^$` 在 `""` 上**一个**匹配，在位置 0（`^` 与 `$` 都在 0 成立）；在 `"a\r\n\r\nb"` 上 `^` 在 {0,3,5}、`$` 在 {1,3,6}，交集中的**一个**在位置 3（第一个 `\r\n` 与第二个之间），即 `\r\n` 中间没有匹配——与任务 1 的 `empty_matches_insert_where_the_engine_allows`（`""` → `"new"`；`"a\r\n\r\nb"` → `"a\r\nnew\r\nb"`；`"a\n\nb"` 的 `^` 在 {0,2,3}、`$` 在 {1,2,4}，交集在 2，得 `"a\nnew\nb"`）写的期望**完全一致**。`.` 配不配 `\r`：**不配**——`crlf(true)` 且 `dot_matches_new_line(false)` 时 `.` 是 `hir::Dot::AnyCharExceptCRLF`（`regex-syntax-0.8.11/src/hir/translate.rs:896-910`），所以 `^foo=(.*)$` 在 `foo=1\r\n…` 上第 1 组就是 `1`（也不是靠回退）。空匹配之后怎样前进：`regex-automata-0.4.18/src/util/iter.rs:433-439` 与 `:667-677` 的 `handle_overlapping_empty_match`——`m.is_empty() && Some(m.end()) == last_match_end` 时把起点 `checked_add(1)`（**一个字节**，不是一个字）再搜，所以**同一个位置不会交回第二个空匹配**；那 +1 字节可能落在一个 UTF-8 字的中间，但 Unicode 模式下的搜索只会在字边界上找到匹配，所以交回的位置仍都在字边界上 | 本提交 |
| 10 | 0 | fancy-regex 0.14.0 的事实（任务 1）：`RegexBuilder::new(pattern: &str) -> Self`（`fancy-regex-0.14.0/src/lib.rs:545`）、`backtrack_limit(&mut self, limit: usize) -> &mut Self`（`:575`，缺省 1 000 000）、`build(&self) -> Result<Regex>`（`:554`）；`Regex::captures_iter<'r, 't>(&'r self, text: &'t str) -> CaptureMatches<'r, 't>`（`:833`，`Item` 是 **`Result<Captures>`**，doctest `:821` 是 `.next().unwrap().unwrap()`）、`Regex::captures_len(&self) -> usize`（`:925`，**含第 0 组**）。回溯超限的错误是 `Error::RuntimeError(RuntimeError::BacktrackLimitExceeded)`，计数在 `src/vm.rs:714-717`（`backtrack_count += 1` 之后与 `options.backtrack_limit` 比）。`(?m)` 的读法：`src/parse.rs:246-262` 仍然写死 `crlf: false`（`^` → `Assertion::StartLine { crlf: false }` 在 `:250`、`$` → `EndLine { crlf: false }` 在 `:259`，两处 `// TODO: support crlf flag` 在 `:249`、`:258`），与设计第 5.4 节引的一样；所以落到 fancy-regex 的式子只认 `\n` 作行尾。`(a\|b\|ab)*(?=c)` 的 `(a\|b\|ab)*` 是不是交给回溯机：**是**——`src/compile.rs:252-285` 的 `compile_concat` 里 `prefix_end` 是 0（那个 `*` 不是 `const_size`）、`suffix_len` 是 0（`(?=c)` 是 hard），所以两个子项都走 `:280-282` 的 `self.visit(child, true)`；再到 `compile_repeat`（`:287-374`）的 `e*` 那一支（`:334-341`）发 `Insn::Split` 与 `Insn::Jmp`、把子项也以 `hard = true` 交下去，整串在回溯机里跑。`translate` 之后三例的结果：`regex` 把 `(?=`、`(?!`、`(?<=`、`(?<!` 在语法层就拒掉（`regex-syntax-0.8.11/src/ast/parse.rs:531-536` 的 `is_lookaround_prefix` 与 `:1232-1236` 的 `UnsupportedLookAround`），所以 `a(?=b)`、`^a(.)(?=\n)`、`(a\|b\|ab)*(?=c)` 三例 **`regex` 都编不了**；fancy-regex 有向前看（`src/compile.rs:145-147` 的 `Expr::LookAround` → `compile_lookaround`），三例**都编得了** | 本提交 |
| 11 | 0 | flate2 1.1.10 的事实（任务 3）：`MultiGzDecoder::new(r: R) -> MultiGzDecoder<R>`（`flate2-1.1.10/src/gz/read.rs:272`）、`ZlibDecoder::new(r: R) -> ZlibDecoder<R>`（`src/zlib/read.rs:178`）、`DeflateDecoder::new(r: R) -> DeflateDecoder<R>`（`src/deflate/read.rs:170`），三者都只要求 `R: Read`。工作区的 `flate2 = "1"`（`Cargo.toml:43`，今天只由 `crates/rurge-rules/Cargo.toml:21` 用）开的是**缺省特性**：`flate2-1.1.10/Cargo.toml:62-65` 的 `default = ["rust_backend", "runtime_detection"]`，`src/lib.rs:19-24` 的文档说 `default` / `rust_backend` 今天用 `miniz_oxide`；锁文件里 flate2 的 `dependencies` 是 `crc32fast`、`miniz_oxide`、`zlib-rs`（zlib-rs 也是可选依赖），**没有 `libz-sys`**（那个才是 C 的 zlib），且 `target/debug/deps` 里只有 `libminiz_oxide-*.rlib`、没有 `libzlib_rs*`——后端是 miniz_oxide 0.9.1。读的错：输入坏了是 `InvalidInput`（`src/zio.rs:165-166` 与 `:245-246` 的 `"corrupt deflate stream"`；gzip 头不对是 `src/gz/mod.rs:289` 与 `:294` 的 `"invalid gzip header"`）；输入中途断了是 `UnexpectedEof`（gzip 的 `src/gz/mod.rs:252-256` 与 `:278`，zlib 与 deflate 各有一条断言这个的测试在 `src/zlib/mod.rs:144` 与 `src/deflate/mod.rs:172`）；gzip 成员之后跟着不是 gzip 的字节时，`MultiGzDecoder` 把下一个成员当头读、失败于 `"invalid gzip header"`（`src/gz/bufread.rs:336-345` 只在 EOF 时结束）——即 `Err(InvalidInput)`，不是照常结束 | 本提交 |
| 12 | 0 | hyper 1.11.1、h2 0.4.19 与 http-body 1.1.0 的事实（任务 3、5）：`Incoming::size_hint`（`hyper-1.11.1/src/body/incoming.rs:300-322`）在长度知道时是 `SizeHint::with_exact`（HTTP/1.1 带 `Content-Length` 的、HTTP/2 带 `content-length` 的），否则是 `SizeHint::default()`（下界 0、没有上界：HTTP/1.1 的 chunked、HTTP/2 不带 `content-length` 的）；`is_end_stream`（`:288-298`）对 HTTP/1.1 是 `content_length == ZERO`、对 HTTP/2 问 h2 的 `is_end_stream()`。HTTP/1.1 的**服务端**（`src/proto/h1/role.rs:919-962`）：应答没有 `Content-Length` 与 `Transfer-Encoding` 时，正文的 `size_hint().exact()` 是确切的值就**写 `content-length: <n>` 并按长度分帧**（`:948-960`），不知道长度时用 chunked（`:927-932`；HTTP/1.0 或该状态不能 chunk 时才是 close-delimited），长度 0 时写 `content-length: 0`（`:935-946`）；长度来源是 `src/proto/h1/dispatch.rs:367-379` 的 `body.is_end_stream()` 为 `None`、否则 `size_hint().exact()`。应答里已经写了 `Transfer-Encoding: chunked` 时 hyper 照旧用 chunked（`role.rs:806-837`，所以 P13 要先把那个头去掉）。**trailer**：只有 chunked 且应答头里有 `Trailer` 时才会发（`role.rs:859-900` 的 `allowed_trailer_fields`），长度分帧的应答上的 trailer 被静默丢掉（`src/proto/h1/conn.rs:739-746` 的 `write_trailers` 里的 `if T::is_server() && !self.state.allow_trailer_fields { debug!("trailers not allowed to be sent"); return; }`）——P9 把有 trailer 时的 `size_hint` 定成下界 n、没有上界，因此 hyper 用 chunked，这一条成立。HTTP/1.1 的**客户端**：请求同时有 `Transfer-Encoding` 与 `Content-Length` 时 `Transfer-Encoding` 赢、`Content-Length` 被去掉（`role.rs:1366-1371` 与 `:1439-1443` 的 `should_remove_con_len`），所以 P9 的补 `chunked` 能使 hyper 不按 `Content-Length` 分帧。**服务端什么时候答 `100 Continue`**：`src/proto/h1/conn.rs:405-411` 的 `Reading::Continue` 分支，即请求正文**第一次被 poll 要数据**时才写 `HTTP/1.1 100 Continue`（读头时不写；正文被排干而不读时跳过，`:858-864`；服务端已经开始写时应答时也不写，`:407` 的 `if let Writing::Init`）。HTTP/2：拿到应答头写着 `content-length` 而正文长度不同时，h2 0.4.19 在**收**一侧把 head 里的值存下（`h2-0.4.19/src/proto/streams/recv.rs:184-197`）并逐帧比对：多了是 `content-length overflow`、少了是 `underflow`，两者都 `Error::library_reset(stream.id, Reason::PROTOCOL_ERROR)`（`:706-724`）；带 `END_STREAM` 而 `content-length > 0` 的头也直接拒（`:200-209`）。h2 的**发**一侧不校验（`src/proto/streams/send.rs` 里没有 `content_length`）——所以写错的 `content-length` 由对端发现，不是 rurge，P13 必须真的改头。http_body 1.1.0：`Frame::{data(buf: T), trailers(map: HeaderMap), data_ref(&self) -> Option<&T>, into_data(self) -> Result<T, Self>, into_trailers(self) -> Result<HeaderMap, Self>}`（`http-body-1.1.0/src/frame.rs:21`、`:28`、`:68`、`:58`、`:94`）；`SizeHint::{new, with_exact, exact, lower, upper, set_lower, set_upper, set_exact}`（`src/size_hint.rs:16`、`:23`、`:70`、`:33`、`:51`、`:43`、`:61`、`:80`）：`set_lower` 在 `value > upper` 时 **panic**（`:44` 的 `assert!`），`set_upper` 在 `value < lower` 时 **panic**（`:62`），`set_exact` 两个一起设、不 panic（`:80-83`） | 本提交 |
| 13 | 0 | tokio 1.53.1 的事实（任务 5）：`tokio::fs::File::open(path: impl AsRef<Path>) -> io::Result<File>`（`tokio-1.53.1/src/fs/file.rs:157`）、`File::metadata(&self) -> io::Result<Metadata>`（`:449`，两者都是 `async`）；`AsyncReadExt::take(self, limit: u64) -> Take<Self>`（`src/io/util/async_read_ext.rs:1445`）。`tokio::fs` 的读**确实交给阻塞线程池**：缺省路径是 `spawn_blocking`（`:1076` 的 `spawn_blocking_read`，状态放在 `File` 的 `State::Busy` 里，`:621-623`）。一次读在进行时丢掉它的 future：**阻塞任务不会被取消**，它跑完的字节写进 `File` 的缓冲，下一次操作先看到 `State::Busy`（`:623`）并把之前那次的结果丢掉、继续拿下一次的；所以被取消的请求会留下一个跑到完的阻塞读，`File` 与它的缓冲跟着它活着到那时（LocalFile 的 `Arc<Mutex<Option<…>>>` 在丢掉文件时也一样） | 本提交 |
| 14 | 0 | 资源管理器的三件事（任务 6）：缺省的 `max_size` 是 `64 * 1024 * 1024`（`crates/rurge-net/src/resource/mod.rs:138`，P19 写的 64 MiB 对）；`update_interval: None` 用 `DEFAULT_UPDATE_INTERVAL = 86_400` 秒（`:27`、`:451`），写 0 也当没写（`:664` 一带的 `filter(\|&seconds\| seconds != 0)`）。两条规则给同一个 URL 时**是同一个资源**：`register`（`:658`）按 `spec.source.key()`（URL 就是它的全字）进同一张表，`resource_key` 只在 P19 的标签与「列表里怎样认它」里用。还没下载完时 `ResourceHandle::current()` 交 `ResourceState::Missing`、下载失败过而没缓存时交 `Failed { cached: None, .. }`，两者的 `data()` 都是 `None`；失败过但手里还有旧的字节时 `Failed { cached: Some(..) }` 的 `data()` 交那份旧字节（`:198-205`）——即 P15 的"还没有一份"就只是前两种。`TestServer::set`（`crates/rurge-net/src/testing.rs:139`）可以在用例中途再调一次、给同一个路径换一个应答（但也换不了 ETag 前后关系，要"下载到的数据真的变了"得同时看 `set_validators`） | 本提交 |
| 15 | 0 | 数（第 4 项）：`Cargo.lock` 的 `[[package]]` **520**；`crates/rurge-platform/tests/unsafe_functions.rs` 的 `LISTED` 是 `[(&str, &str); 23]`（`:18`），实际 **23** 项；`crates/rurge/tests/sockets.rs` 的 `LISTED` 是 `[(&str, &str, &str); 26]`（`:58`），实际 **26** 行（每行是一个三行的元组，数的是 `    (` 开头的那些）；带本机账户名的文件 **26**（只跑了一次 `git grep -l` 数行数，那个名字不写进任何文件）。四个数都与派工给的基线相同 | 本提交 |
| 16 | 0 | 现有用例（第 5 项，P21 的表逐条核过，按任务）：**任务 1、2** `deferred.rs` 的 `prefix_and_membership_matching`（`:47`，`:71`、`:72` 两句要翻成 `!is_deferred(..)`，注释写 `// typed since phase 4 M3b`）**改**；`diagnostic.rs` 的 `every_code_is_decided_once`（`:543`）与它的两条附加断言（`WITH_DEFAULT` 每个要有缺省建议、`PER_SITE` + `WITHOUT` 每个要没有）**改**（`WITHOUT` 加 `W0042`，先红）；语料库快照 `corpus__corpus__kitchen-sink.snap` **改**（第 98、99 行少两项，第 110 行的 `W0016` 消息少两节，第 170 行的 jq 多一条 `W0042` 排在最后——今天最后一条是第 117 行的 `W0038`，加在它之后）；`config.rs` 的 `rewrite_sections_are_read_and_no_longer_deferred` **照过**（只多两个字段，`config.rewrite.body` 的新断言是那条新用例的事）；两处 `RewriteConfig { … }` **改**（只补字段，断言不改）；`crates/rurge-config` 的 `rewrite.rs` / `dialect.rs` / `value.rs` / `rule.rs` / `reach.rs` / `redact.rs` 现有用例与 `tests/secrets.rs` **照过**（本计划只加）；`cli.rs` 里写着这两节的用例**零条**（见第 8 行 ③）。**任务 4** `rewrite/mod.rs` 的 `counters_count_and_snapshot`（`:522`）**改**（`body` 与 `map_local` 各加一次、都变成 1）；`rewrite_of` / `config_of` / `read_rules` 与 `engine.rs` 的 `rewrite_of`（`:6714`）、`url.rs:734` **改**（`build` 多一个参数，只加参数）；`build_gives_none_without_lines`、`build_makes_the_two_tls_configs`、`notes_never_carry_values_or_queries` **照过**。**任务 5** `engine.rs` 的 `rewrite_notes`（`:6744`）**改**（前缀加 `Body Rewrite`、`Map Local`）；`counted`（`:6732`）**照过**（M3a 的用例里两个数仍是 0）；`engine.rs` 测试模块的其余全部（`Tail`、`Idle`、看门狗、M2b 的计字节）**照过**：`Tail::new` 只有 `:1041` 一处调用、`ReadAhead::through` 对 M3a 的请求不改变 `size_hint` / `is_end_stream`，而 101 的 `promote` 路径不经过 `Tail`；`api.rs:1471` 的 `rurge_http_counts_start_at_zero` **照过**（两处正文里 `body` 与 `mapLocal` 仍是 0，键与次序不变）。**任务 6** `external_resources.rs` 的 `type_name` **改**（加一支）、`:417` 的 `only_a_rule_set_and_a_domain_set_are_read_as_sets` **改**（加 `MapLocal` 进那个数组，见第 8 行 ②）；`tests/external_resources.rs:279` 的 `the_listing_names_each_kind_and_leaves_the_hosts_file_out` **照过**（那份 Profile 没有 `[Map Local]`，`listing.len()` 仍是 6）；`api.rs:2535` 的 `external_resources_are_listed_one_item_for_each_kind` **照过**（同理，它那个 Profile 也没有 Map Local；若任务 6、7 把一条 Map Local URL 写进那份共用 Profile，这个列表会变多一项）。**任务 4 ～ 7** `tests/rewrite.rs`、`http.rs`、`mitm.rs`、`sniff.rs`、`pipeline.rs` 的全部**预计照过**：这些 Profile 没有 `[Body Rewrite]` 与 `[Map Local]`，所以 `body_rules` 与 `map_local` 都是空、一条备注也不多（`tests/rewrite.rs` 里精确断言备注列表的 `:465`、`:525`、`:592`、`:633`、`:1679` 五处因此仍成立，`:1727` 的 `>= 2` 也仍成立）；`HttpCounts` / `HttpJson` / `ExternalResource` 都不加字段，`sniff.rs:958` 不用改。**任务 3 ～ 7** `sockets.rs` 的扇描用例与 `unsafe_functions.rs` **照过**（名单不变：读文件不建套接字） | 本提交 |
| 17 | 0 | 补扫一遍（第 5 项末段）：扫了 `git grep -n 'Body Rewrite\|Map Local'`、`'RewriteConfig {'`、`'RewriteCounts {'`、`'HttpCounts {\|HttpJson {'`、`'ExternalResource {'`、`'ResourceKind::'`、`'Rewrite::build'`、`'Tail::new('`、`'match kind'`、`'type_name'`、`'\.notes()'`、`'until M3b'`、`'\[Body Rewrite\]\|\[Map Local\]'`（对全部 `crates/` 与 `tests/`，以及整个仓库的 `.conf`），结论：① 写这两节的 Profile **只有** `tests/corpus/valid/kitchen-sink.conf`（`:168-174`），别处的命中都是代码注释与文档；② 不带 `..` 构造 `RewriteConfig {`、`RewriteCounts {`、`HttpCounts {`、`HttpJson {`、`ExternalResource {` 的地方已列在第 8 行 ⑥，一个字段也不加，都不会编不过（真正会编不过的只有 `RewriteConfig {` 那两处）；③ 对 `ResourceKind` 的穷尽 `match` 只有 `type_name` 一处（`set_kind` 有 `_` 分支）；④ `Tail::new(` 只有 `engine.rs:1041`；`Rewrite::build(` 的调用处共**八处**：`rurge-engine/src/runtime.rs:319`、`rurge-http/src/engine.rs:6719`、`rurge-http/src/rewrite/mod.rs:443`、`rurge-http/src/rewrite/mod.rs:459`、`:476`、`:494`、`:511` 与 `rurge-http/src/rewrite/url.rs:734`（任务 4 要全部补参数：只改 `mod.rs` 的 `rewrite_of` 会让其余七处编不过，这是 P21 没列到的）；⑤ 断言 `/v1/rurge/http` 正文里 `body`、`mapLocal` 的用例就是 `api.rs:1471` 一条（两处），断言 `/v1/external_resources` 列表的另有 `api.rs:2535`、`:2597`、`:2664` 三条，都不涉 Map Local；⑥ 断言 `CORE_VERSION` 的值的用例不能直接找到（`crates/rurge/tests/cli.rs:896-926` 只断言 22 能过、23 不能，值不改就不动）；⑦ 断言一个请求的记录里备注条数或内容的用例已列在第 16 行（`tests/rewrite.rs` 五处精确断言 + `:1727` 的 `>= 2` + `api.rs:1277` 的 `notes`），没有规则时不多一条，预计照过 | 本提交 |
| 18 | 0 | 门禁（第 6 项，本任务不改代码，量的是本计划的基线）：先 `df -h /d`（108 GiB 可用）；`cargo fmt --all --check` exit=0（`gate-task-0-fmt.log`）、`cargo clippy --all-targets -- -D warnings` exit=0（`gate-task-0-clippy.log`，`Finished dev profile … in 0.95s`）、`cargo test -j 8 --workspace --no-fail-fast` exit=0（`gate-task-0.log`）；测试 **92 个结果行、5123 通过、0 失败、13 忽略**，与派工给的基线（92 / 5123 / 0 / 13）差 0；没有 `STATUS_HEAP_CORRUPTION` / `STATUS_ACCESS_VIOLATION`，没有重跑，跑完没有留下 `rurge.exe` 孤儿。一台机器上的一次运行，不是 CI；需要参照服务端的互操作用例在本机立刻返回，算在 5123 里 | 本提交 |
| 19 | 0 | 协调者核过任务 0（`64fdf00`，第 1 ～ 18 行）：门禁的数（92 / 5123 / 0 / 13，没有崩溃与重跑）、计划的两张表只多不少、带本机账户名的文件 26、`git diff --check` 干净，都对得上；第 2、3、6、7、8、14、16、17 行引的本仓行号抽查过（如 `tests/corpus/valid/kitchen-sink.conf:168-174`、快照 `:97-107`、`crates/rurge-config/src/config.rs:937-953`、`crates/rurge-http/src/pool.rs:103`、`crates/rurge-engine/src/engine.rs:1516`、`crates/rurge-net/src/resource/mod.rs:138`），都对，只有第 8 行 ⑤ 的两处差一行：`crates/rurge-http/src/rewrite/mod.rs` 的那一句在 `:60-61`，`crates/rurge-api/src/routes/rurge_http.rs` 的在 `:26-27`（第 21 行用这两个）。第 8 行 ④ 先写"共六条"，列出来的与它句末说的都是七条（`warnings`、`ip_rewrite_is_read_in_order`、`replica_is_known_and_ignored`、`rewrite_sections_are_read_and_no_longer_deferred`、`wireguard_sections_are_typed`、`the_mitm_section_is_typed_and_no_longer_deferred`、`an_ssid_setting_section_is_read_and_no_longer_inactive`），以七条为准，结论（一条也不用改）不变 | 本提交 |
| 20 | 0 | **协调者的裁定**（trailer）：第 12 行读出的 hyper 1.11.1 的事实有一个计划没有料到的后果。它的 HTTP/1.1 编码器（服务端与客户端都是）只写消息头里 `Trailer` 列出的 trailer：头里没有 `Trailer` 时编码器是 `Kind::Chunked(None)`，`encode_trailers` 一个也不写（`hyper-1.11.1/src/proto/h1/encode.rs:163-210`，`role.rs:859-900`、`:984-988`、`:1415-1432`）；而引擎把 `Trailer` 当作逐跳的头去掉（`crates/rurge-http/src/headers.rs:14`，M1 设计第 8.7 节）。所以从 M1b 起，经引擎到 HTTP/1.1 客户端的 trailer 一个也不转，有没有重写规则都一样；兼容性清单第 570 行登记的"trailers 只在声明过时转发"比实际宽。① 任务 5 的 `an_unchanged_response_keeps_its_bytes_and_trailers` 改在 HTTP/2 的两端看 trailer：客户端讲 HTTP/2（`rig_tls_h2`），源站是 `h2_origin`，加一种在应答的数据之后带 trailer `x-t: 1` 的 `OriginScript`；一条配上而没有改动的应答规则 → 客户端收到同样的数据与那个 trailer、头不变、没有备注、`body` 是 0。HTTP/1.1 的那一半（源站 chunked 地答、带 trailer）只断言正文的字节与头不变、照旧 chunked 分帧，不断言 trailer。② 任务 9 在兼容性清单里订正第 570 行那一句：到 HTTP/1.1 客户端的 trailer 不转发（`Trailer` 是逐跳的头、被去掉，hyper 只写声明过的），两端都讲 HTTP/2 时照转（任务 5 的那条用例，经一条配上而没有改动的规则）。P9 的 `size_hint` 照旧：有 trailer 时不报确切的长度，HTTP/1.1 上因此照旧用 chunked 分帧 | 本提交 |
| 21 | 0 | **协调者的裁定**（过时的"until M3b"）：第 8 行 ⑤ 列的几处注释由这几个任务改，只改注释，不改形状与键：`crates/rurge-config/src/config.rs:250-252`（`rewrite` 字段的注释）由任务 1、2 改（任务 1 之后只剩 `[Map Local]` 还没读，任务 2 之后整句换成现在的样子）；`crates/rurge-http/src/rewrite/mod.rs:60-61`（`RewriteCounts` 的文档注释）由任务 4 改（计划已写）；`crates/rurge-http/src/engine.rs:6730-6731`（测试的 `counted` 的注释）与 `crates/rurge-api/src/routes/rurge_http.rs:26-27`（`RewriteJson` 的文档注释）由任务 5 改，那时两个数开始计——`engine.rs` 那一处覆盖第 8 行 ⑤ 说的"任务 4 改前两个"；`crates/rurge-api/src/routes/rurge_http.rs:21-22` 的 `rewrite` 字段的注释（"What the rewrites of `[URL Rewrite]` and `[Header Rewrite]` did"）也由任务 5 改成四个节。计划的「文件结构」没有列 `crates/rurge-api/src/routes/rurge_http.rs`，任务 5 只改这两处注释 | 本提交 |
| 22 | 1 | 一次性的测量（任务 1「实现」最后一条）：调试构建、一台机器上的一次运行（不是 CI）。临时的 `#[ignore]` 用例 `measure_the_worst_case` 以 `BODY_BACKTRACK_LIMIT` 编两个式子，各跑一次 `replace_all`；命令 `cargo test -j 8 --workspace --lib -- measure_the_worst_case --ignored --nocapture`，exit=0（`measure-task-1.log`）。`(?=a)b` 在 32 MiB 的 `a` 上 **15.0005125 s**、结果不是 `None`；`(a|b|ab)*(?=c)` 在 `"ab"` 重复 40 次的 80 字节上 **21.1897669 s**、结果是 `None`（回溯超限）；整条用例 36.22 s。用例跑完已删除，不在提交里 | b048276 |
| 23 | 1 | `regex` 这个依赖（任务 1 第 1 步）：`Cargo.toml` 的 `[workspace.dependencies]` 与 `crates/rurge-config/Cargo.toml` 各加一行；第一次编译用 `cargo check --offline --workspace --all-targets`，exit=0（`check-dep-task-1.log`），没有下载。`Cargo.lock` 的 `[[package]]` 仍是 **520**，`git diff Cargo.lock` 只有 `rurge-config` 的依赖列表多一行 `"regex",` | b048276 |
| 24 | 1 | 与计划对不上的：第 7 条用例的第二半在 `"ab"` 重复 **20** 次上就分开了（`with_backtrack_limit(.., 1_000_000)` 得 `None`、`BodyPattern::new` 得 `Some`），不需要再调重复次数，照计划写的 20 次跑 | 本提交 |
| 25 | 1 | 执行者的裁定：`parse_body` 里一对的式子（`BodyPattern::new`）编不了时与 URL 式子编不了同样处理——这一行整行跳过、只给一条 `W0012`，后面成对的项不再读（P3 只写了诊断都带这一行的 `span`）；每一行都先看项数、再编式子，所以 `http-response ^a b` 一类的形状错只给形状那一条 | 本提交 |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
