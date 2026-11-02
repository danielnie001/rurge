# 阶段 4 / M5b「基准与遗留」实施计划

> **执行方式**：交给 Pi，由协调的会话逐个任务派工、逐个任务评审（设计第 2 节、M5-D1；做法同 M5a）。
>
> - 协调的会话每个任务开一个 Pi 会话，交给它这个任务的全文与要用的接口（派工文件）。Pi 做完把报告写进派工文件指定的路径，在本地提交。
> - 协调者读改动、重跑门禁、评审。有问题回到同一个 Pi 会话修正，再评审；过了才派下一个任务。
> - 遗留的三条用例（任务 1）钉住今天的行为，写完就绿；"先红"在这里是变异：各做一次计划写的那一处改动，看它红、记下红的样子，再改回来。
> - 基准（任务 2 ～ 4）是量、不是门，没有"先红"：每个基准写完，在调试构建里用 `HTTP_BENCH_QUICK=1` 单独跑一次，看它跑完、它自己的断言都过、打出的数合理。守护读数解析的那一条用例照常先红。
> - 与计划的出入记进末尾的「执行期修正记录」，**不改计划的正文**。
>
> **停点**：一个，在 M5b 做完之后（设计第 2 节）。协调者把 `phase4-m5` 快进合并进本地 `main`，请项目所有者推送（那时 `main` 领先 `origin/main` 的全部提交，含 `f9994db` 起的五个），推送之后读回那次 CI。再往后是 M6（收尾与验收）的细化设计。

**目标**：NFR-01 的读数（写成数字，不作门禁）与 M1b 计划延后第 4 行（设计第 1 节的 M5b 部分、第 8、9 节）。

- 遗留：`Idle` 第一次 `poll_frame` 才起表、`Progress::restart`、`restart` 在重发之前的调用处，各有一条去掉就红的用例（第 9 节）。
- 基准，在一台 Windows 开发机上用 release 跑一遍，读数照实写进修正记录与 `docs/status.md`（第 8 节）：
  - B1：明文 HTTP/1.1 经引擎的吞吐与每请求延迟；
  - B2：解密之后，HTTP/1.1 与 `h2` 两行；
  - B3：一千条不命中的规则下每请求的开销；
  - B4：叶证书签发的耗时；
  - B5：每条连接、正文的缓冲、叶证书缓存三项内存；
  - Q1：读一个跨两个数据报的 ClientHello 的开销。
- M5b 的收尾文档。M5 至此做完。

**架构**：

- 生产代码一行不改。
- 遗留的两条单元用例在 `crates/rurge-http/src/engine.rs` 的测试模块，用暂停的时钟；`rurge-http` 的开发依赖给 tokio 开 `test-util`（P2）。端到端的一条在 `crates/rurge-engine/tests/http.rs`，用一个先拖住再断、换了连接再拖一会儿才答的源站（P4）。
- 基准都在一个新的用例文件 `crates/rurge-engine/tests/bench.rs`：忽略的用例，各自打印数字（P5 ～ P11）。
  - 源站是这个文件自己的 hyper 服务器（明文或 TLS）；客户端是 hyper 的连接级客户端。
  - 正文逐帧发、逐帧数，从不攒起来。
  - CI 的 `cargo test` 编译它们、不跑。

**技术栈**：都已在 `Cargo.lock` 里，源码在本机 cargo 注册表里。**不下载、不加新包**。

- hyper 1.11.1：
  - `hyper::client::conn::{http1, http2}` 的 `handshake` 与 `SendRequest`；`hyper::body::{Body, Frame, SizeHint, Incoming}`。
  - 连接级的 HTTP/1.1 客户端照请求的 URI 原样写请求行（`src/proto/h1/role.rs` 第 1215 行用 `{}` 写整个 `Uri`），所以绝对形式的 URI 写出去就是绝对形式；它不自己加 `Host`。
- hyper-util 0.1.20：`rt::{TokioExecutor, TokioIo}`、`server::conn::auto::Builder`。http-body-util：`BodyExt::frame`、`Full`、`combinators::BoxBody`。
- tokio-rustls 0.26 与 rustls 0.23.43：`TlsConnector`；`ClientConfig::resumption` 与 `client::Resumption::disabled()`（`src/client/client_conn.rs:513`）；`handshake_kind()` 与 `HandshakeKind`（`src/common_state.rs:188`、`:785`）；`peer_certificates`、`alpn_protocol`。
- tokio：`#[tokio::test]`（当前线程的运行时）；`#[tokio::test(start_paused = true)]` 与 `tokio::time::advance`（要 `test-util`）。

**依据**：

- M5 细化设计（下面写"设计"指它）：`docs/superpowers/specs/2026-11-02-phase4-m5-auto-quic-block-bench-design.md`。
  - M5b 用到第 1、2、8、9、11、12（V7 ～ V9）、15 节。
  - **先读它第 16 节**：第 1 ～ 3 条是写 M5a 计划时改的，第 4 条是执行 M5a 时改的，第 5 ～ 10 条是写本计划时改的。
  - 设计与本计划的「计划期决定」不一致的，以计划为准。
- M1b 计划（`docs/superpowers/plans/2026-10-29-phase4-m1b-http-engine-plan.md`）「延后事项」第 4 行；M1 细化设计第 8.6、8.10 节（重发与两个空闲时限）。
- `docs/status.md` 阶段 4 / M5a 一节的「以后的会话必须知道的事」整段适用，M4b、M4a、M2a、M1b 四节的同名段落也适用。
- 写计划时对着 `6d74039`（分支 `phase4-m5`，M5a 做完、评审完）核对过的事实写在「计划期决定」里。外部库的事实对着注册表里锁定版本的源码。

## 全局约束

**依赖**

- 只改一处 `Cargo.toml`：`crates/rurge-http/Cargo.toml` 的 `[dev-dependencies]` 加 `tokio = { workspace = true, features = ["test-util"] }`（P2）。别的 `Cargo.toml` 不改。
- `Cargo.lock` 的 `[[package]]` 今天 520 个，每个任务之后照旧 520 个，锁文件一个字不变（给锁里已有的包多开一个特性不改锁文件）。`cargo` 要下载任何东西就停下报告。
- 依赖方向不变：`rurge-http` 不依赖 `rurge-engine`。
- `unsafe_code`：不加放开的函数，`crates/rurge-platform/tests/unsafe_functions.rs` 的 `LISTED` 仍是 23 项。
- `crates/rurge/tests/sockets.rs` 的 `LISTED` 仍是 26 行：它只读 `crates/*/src` 的生产代码。M5b 不改生产代码；基准与用例建的套接字都在 `tests/` 与测试模块里。
- 生产代码（`crates/*/src` 里测试模块之外的部分）一行不改。

**缺省的 `cargo test` 不碰系统**

- 同 M5a：不建网卡，不改系统的任何设置；不执行 `netsh`、`route`、`ipconfig`、`ip`、`resolvectl`、`networksetup`。
- **测试从不把任何 CA 装进信任库，也不读系统的信任库**：
  - 引擎一律给 `EngineShared.roots`（`tests/mitm.rs` 的 `harness_mitm` 的写法）。不给的话 `crates/rurge-engine/src/runtime.rs` 的 `build_mitm` 会读系统的根。
  - 客户端的 rustls 配置只信给它的根。
- 基准读本进程的内存（P10 ①）：
  - Windows 上由用例自己起一个 `powershell` 读 `Get-Process` 的两个数（只读的系统工具，同 `crates/rurge-engine/src/observe/memory.rs`）；
  - Linux 读 `/proc/self/status`；macOS 不读。
- 任务 5 另用 PowerShell 的 `Get-CimInstance Win32_Processor` 读一次处理器的名字与核数（只读）。

**测试的约定**

`CLAUDE.md`「测试约定」整节适用。与本计划直接相关的：

- 只用回环与端口 0。
  - 名字（`origin.test`、`target.test`、`*.bench.test` 之下的名字）经 `[Host]` 钉到 `127.0.0.1`（`[Host]` 认 `*.` 开头的通配，见 `crates/rurge-dns/src/hosts.rs` 的用例）。
  - 第二道防线是共用夹具的 `dns-server`（回环上的 `MockDns`）（设计第 16 节第 10 条）。
- 等待都有时限；计数与记录用 `wait_until` 等。
- 新写的计时用例（任务 1 的端到端）提交前单独循环跑二十次以上。
- 做"改一行看用例变红"之前，先说清这一改让会话、查找与文件去哪里（`CLAUDE.md`「测试约定」）。本计划的变异只改下面几处，会话都在回环上，不查钉住的名字之外的东西，不写文件；做完立刻改回：
  - `rurge-http` 的 `Idle::new`、`Progress::restart`、`retry` 里的一行；
  - 基准用例自己的一行。
- 基准是忽略的用例，`#[ignore = "…"]` 的原文在「表后的原文」。CI 的 `cargo test` 与门禁都编译它们、不跑。
- **基准各自一个进程**：B5 读的是本进程的峰值（只涨不落），所以每个基准单独跑（`--exact <名字>`）。B5 的用例一开头查一个进程里的标记，同一个进程里跑到第二个 B5 就 panic（P10 ③）。
- **内联的测试模块放在文件的末尾**：`sockets.rs` 读到第一个 `#[cfg(test)] mod … {` 为止。任务 1 的两条单元用例加在 `engine.rs` 现有测试模块的末尾。

**门禁与这台开发机**（每个任务末尾一次；说"通过"之前必跑）

- 门禁：`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test -j 8 --workspace --no-fail-fast`。
- 本机 msvc 工具链没有 rustfmt。跑 `cargo fmt` 之前让环境变量 `RUSTFMT` 指向 gnu 工具链里同版本的 rustfmt：用户目录下的 `.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe`。不要为此安装或升级工具链。
- **门禁的日志用重定向写、再追加退出码，不用 `Tee-Object`**；单独重跑的结果也写进日志再引，引之前先看它在、看它写的是什么。
- 聚焦地跑用例也经整个工作区：
  - 单元测试：`cargo test -j 8 --workspace --lib -- engine::tests::<名字>`；
  - 集成测试：`cargo test -j 8 --workspace --test http -- <名字>`；
  - 基准在调试构建里试跑：`HTTP_BENCH_QUICK=1 cargo test -j 8 --workspace --test bench -- --ignored --exact <名字> --nocapture`。
- **不用 `-p`、不用 `--release`，只有任务 5 例外**：任务 5 照设计第 8.1 节用 `cargo test -p rurge-engine --release`，只编引擎与它的依赖的一份 release（同 `relay_throughput` 与阶段 3 的 `rurge-tun` 基准，设计第 16 节第 5 条）。
- 磁盘：
  - 跑全量之前 `df -h /d`：剩余不足约 15 GiB 就停下报告。
  - 任务 5 开始之前剩不到 20 GiB 就停下报告（设计第 8.1 节；2026-11-03 剩 34 GiB）。
- 测试二进制以 `STATUS_HEAP_CORRUPTION`（本机管控软件注入的钩子 DLL）或 `STATUS_ACCESS_VIOLATION` 退出：单独重跑那一个二进制，报告里写明。
- `cli` 的用例偶发 `give_up` 20 秒超时：单独重跑 `cli`，把标准错误原样抄进报告。
- rustc 在增量编译里报内部编译器错误：删掉出错的那个目标在 `target/debug/incremental/` 下的目录再编。
- 回环的 TCP / UDP 7、9、13、17、19 号端口在这台机器上开着，CI 上关着：用例不假定任何固定端口的状态。
- 结果照实记：
  - 本机的全量运行是"一台机器上的一次运行，不是 CI"；
  - 需要参照服务端的互操作用例在本机立刻返回、算在通过的数里，要说出来；
  - 基准的读数是"一台 Windows 11 x86-64 开发机上 release 的一次运行，不是 CI"。

**仓库的规矩**（`CLAUDE.md`「必须遵守的规矩」整节适用；与本计划直接相关的）

- **凭据**：M5b 不加带凭据的字段。`ca-p12` 与口令只在用例的 Profile 里，同 `tests/mitm.rs`。
- **日志的分级**、**诊断**、**网络与进程**、**出站套接字只有一处建法**：M5b 不改生产代码，都不涉及。
- 文档与报告用写文件 / 改文件的工具改。**永远不要用把一个文件的内容经管道写回同一个文件的命令**（2026-10-27 这样写满过 D: 盘）。
- 计划末尾两张表里已写下的行不改写、不删除。追加新行时**不要把已有的行当成锚点替换掉**（M2a 任务 9 这样删掉过一行）。

**写法**

- 注释、断言与打印的文字用英文，照所在文件的写法：文档注释写"为什么"与前提，句子完整。注释里提到设计或计划，写 `phase 4 M5 design 8.2`、`the phase 4 / M5b plan's P6`。
- 提交说明照 `git log` 的样子写，标题用中文：`test(http): …`、`test(engine): …`、`docs: …`。
  - 正文说做了什么、改了哪些现有用例或夹具、为什么。
  - Pi 的提交不带署名行。
- 只在本地提交：不 `push`、不 `pull`、不 `fetch` 远端、不 `amend`、不改写历史。

**不做的事**

- 只运行本仓库构建出来的二进制与 Pi 自己。不下载、不安装任何东西。不手动跑 `rurge run`、`rurge dns lookup`；`rurge service` 只用 `--dry-run`。
- 不做设计第 1 节「不做」一栏的事：
  - 基准不作门禁；
  - 不重跑阶段 3 的 B1、B2、b3（第 8.8 节）；
  - 第 9 节"不收"的遗留。
- 不为让读数好看改生产代码：读数只记下来，NFR-01 的线怎样用在经引擎的路径上归所有者（设计第 15 节第 6 条）。
- 不为基准放开 `Authority::issue`（设计第 8.5 节）。

**计划没有写到、要做决定的地方**

- 能按设计与总设计的原则定的：自己定，记进修正记录，写"执行者的裁定"。
- 要停下来列出、交给协调者的：
  - 去向写着"项目所有者"的（设计第 15 节）；
  - 会改变 M5-D16 ～ D18 的；
  - 要改生产代码才能做下去的。
- 现有用例变红：P14 的表点了名的，照计划改，在提交说明里说；没点名的，停下来，记进修正记录，不顺手改期望值。

## 评审要点

设计没写透、而最可能让读数骗人或让用例平白就绿的几处，与负责它们的任务和用例：

1. **量错了路**：B1 的请求没有进引擎、B2 没有解密或上游讲的不是那一行的版本、B3 的规则没有在请求上求值、B4 的第二遍是会话恢复而不是完整握手、B5 的几种连接走的不是各自的路。
   - 期望：每个基准自己断言走的是那条路，路错了就 panic，不打印一个错的数。
   - 用例（任务 2 ～ 4）：
     - `requests`、`sniffed.tls`、`mitm.decrypted` 的增量；
     - 源站按版本数的请求；
     - B3 开跑前的 `/sanity` 记录；
     - B4 每次握手的 `handshake_kind()` 与叶证书的 DER；
     - 任务 2、3 各一次让断言红的变异（P6、P9）。
2. **客户端或源站把正文攒起来**：256 MiB 的正文被 `collect()`、`to_bytes()` 或一个 `Vec` 攒住，量的就成了内存拷贝，B5 的读数也被污染。
   - 期望：两侧逐帧数字节、不留；上行与下行的正文都是 `Repeat`，每帧是同一块 `Bytes` 的切片，不分配。
   - 检查（任务 2、4）：评审读代码；`b5_body_streamed` 的读数（正文流过 rurge 不该让峰值多出一份正文）。
3. **内存读数被别的东西污染**：几个 B5 在同一个进程里跑、读 `before` 之前没有预热、请求记录随握手增长。
   - 期望：每个 B5 单独一个进程，同一个进程里的第二个 B5 panic；`before` 在预热之后读；叶证书一项在请求记录装满之后读 `before`（P10）。
   - 用例（任务 4）：`MEASURED` 标记与它的那一次试跑；任务 5 一个用例一个进程。
4. **遗留的用例平白就绿**：去掉被测的那一行之后照样绿（M1b 延后第 4 行说的正是这个）。
   - 期望：三条用例各有一处变异让它红，红的样子记进修正记录。端到端还断言第二个请求从发出到答完超过整个时限——不然没有测到"在新连接上有整个时限"。
   - 用例（任务 1）：`idle_starts_its_clock_at_the_first_poll`、`restart_gives_the_resent_request_its_whole_wait`、`a_request_sent_again_waits_its_whole_idle_on_the_new_connection`。
5. **计时的用例在慢机器上偶发**：端到端用真实的时钟，要在 CI 的三个平台上跑。
   - 期望：绿的一侧每一处至少 1 秒的余量；提交前单独循环二十次以上；两条单元用例用暂停的时钟，不靠真实时间。
   - 用例（任务 1）：端到端的三个常数（P4 ③）。

---

## 文件结构

| 文件 | 任务 | 职责 |
| --- | --- | --- |
| `crates/rurge-http/Cargo.toml`（开发依赖）；`crates/rurge-http/src/engine.rs` 的测试模块 | 1 | 两条暂停时钟的单元用例 |
| `crates/rurge-engine/tests/http.rs` | 1 | `slow_cut_off_origin` 与端到端 |
| `crates/rurge-engine/tests/bench.rs`（新） | 2 | 基准的夹具（规模、统计、`Repeat`、源站、客户端）、B1、B3 |
| 同上 | 3 | B2、B4 |
| 同上 | 4 | `Resident` 与它的守护用例、B5、Q1 |
| 本计划的「执行期修正记录」 | 5 | release 下的读数 |
| `docs/testing.md`、`docs/status.md`、`README.md`、`README_en.md`、`docs/requirements.md`、`CLAUDE.md`、设计第 16 节 | 6 | 收尾文档 |

模块与辅助函数的名字可以照实际调整，调了记进修正记录。

---

## Task 0：开工前核对

**做什么**：对着当时的分支核对下面每一项。有出入的记进「执行期修正记录」，一项一行，写"计划写 X，代码是 Y，按 Y 做"；按记录调整后面的任务。**不改计划的正文。** 没有出入也记一行。

1. **工作树**：`git status --short` 空；当前分支 `phase4-m5`；`6d74039` 在历史里，之后只多了本计划的提交（本计划、`docs/README.md`、`CLAUDE.md`、设计第 16 节）。
2. **读**：
   - 设计第 16 节（先读）、第 8、9、12、15 节；
   - 本计划的「计划期决定」、「表后的原文」与 P14 的表；
   - `docs/status.md` 阶段 4 / M5a、M4b、M4a、M2a、M1b 五节的「以后的会话必须知道的事」；
   - M1 细化设计第 8.6、8.10 节。
3. **名字与签名**（设计第 12 节 V7、V9）：用 `git grep -n` 逐项找到、记下行号；与「计划期决定」写的不同的照实记。
   - `rurge-http` 的 `engine.rs`：
     - `Progress`（`new`、`since_started`、`restart`、`moved`、`waiting`、`is_waiting`、`remaining`）、`head_deadline`；
     - `Idle`（字段、`new`、`poll_frame` 里 `deadline` 的 `get_or_insert_with` 与先看 `is_end_stream`）、`Stalled`；
     - `retry` 里的 `progress.restart();`、`timed_out`、`Pages`；
     - 文件开头的 `use`（`Meter`、`HostName`、`SessionInfo`、`SessionHandle`、`BodyExt`）；测试模块的开头与末尾。
   - `rurge-http` 的其余：`pool.rs` 的 `Meter::new`；`mitm/authority.rs` 的 `LEAF_SLOTS`（1000）、`SESSION_SLOTS`（1024）、`Authority::leaf` 里的 `spawn_blocking`；`mitm/decrypt.rs` 的 `server_config` 与对上游的两份配置（`h2` 开着给 `h2` 与 `http/1.1`、关着只给 `http/1.1`）；`rewrite/body.rs` 的 `REQUEST`（33,554,432 字节，"over" 是大于）。
   - `rurge-inbound` 的 `SessionHandle::new`；`rurge-config` 的 `SessionInfo::tcp`、`HostName::parse`。
   - `rurge-engine`：
     - `src/http.rs` 里 HTTP 监听器的请求进引擎的那一路、`request_count`、`Pages` 取 `show-error-page`；
     - `Engine::{http_counts, request_log}`、`HttpCounts`、`SniffedCounts`、`MitmCounts`（`decrypted` 在与客户端的握手完成时加）、`EngineShared::{roots, sniff_wait}`；
     - `relay.rs` 的 `relay_throughput`（`RELAY_BENCH_MIB`）；`observe/memory.rs` 的读法。
   - 用例的夹具：
     - `tests/common/mod.rs` 的 `Profile`、`Profile::text`、`harness`、`harness_with`、`harness_timed`、`Harness::{http, socks, engine}`、`connect_via_socks5`、`wait_until`、`the_records`；
     - `tests/http.rs` 的 `sniffing`、`request`、`read_head`、`socks5_connect`、`cut_off_origin`、`an_idempotent_request_cut_off_is_sent_again_through_the_engine`；
     - `tests/mitm.rs` 的 `mitm_section`、`mitm_ca`、`client_config`、`handshake`、`harness_mitm`；
     - `rurge-proto` 的 `testing::TlsFixture`（`new`、`with_ca_name`、`roots`、`acceptor`、`acceptor_with_alpn`、`ca_p12_base64`）；
     - `rurge-net` 的 `quic::testing`（`big_client_hello`、`chrome_frames`、`Initial::{new, seal}`、`RFC_DCID`）与 `quic::initial`（`read_first`、`Hello`、`HelloReader::read`）。
   - 注册表里锁定版本的源码：hyper 1.11.1 `src/proto/h1/role.rs` 写请求行的那几行；rustls 0.23.43 的 `Resumption::disabled`、`handshake_kind` 与 `HandshakeKind` 从哪里引。
4. **数**：
   - `Cargo.lock` 里 `[[package]]` 的个数（520）；
   - `unsafe_functions.rs` 的 `LISTED` 的项数（23）；`sockets.rs` 的 `LISTED` 的行数（26）；
   - 带本机账户名的文件数：派工文件给出要数的那个名字，数出来是 26；
   - `crates/rurge-engine/tests/` 下没有叫 `bench` 的文件，`crates/rurge-engine/Cargo.toml` 没有叫 `bench` 的目标（`[[bench]]` 只有 `relay`）。
5. **磁盘**：`df -h /d`，记下剩多少。
6. **现有用例**（P14 的表）：逐条找到、读它断言什么，把"预计照过"逐条写进修正记录，一条用例一句。
7. **门禁**：跑一遍，记下测试二进制数、通过数、失败数与忽略数（M5a 任务 6b 评审时是 93 / 5449 / 0 / 17，一台机器上的一次运行，不是 CI）。

**判据**：修正记录里有任务 0 的行；门禁的数字记下。

---

## Task 1：遗留的三条用例

**改哪里**：

- `crates/rurge-http/Cargo.toml`：`[dev-dependencies]` 加 `tokio = { workspace = true, features = ["test-util"] }`（P2）。
- `crates/rurge-http/src/engine.rs` 的测试模块末尾：一个永不出帧的正文与两条单元用例（P3）。
- `crates/rurge-engine/tests/http.rs`：`slow_cut_off_origin`，`cut_off_origin` 改成调它；新的端到端（P4）。

**接口**

- 消费：`Progress`、`Idle::new`、`Stalled` 的文字、`Meter::new`、`SessionHandle::new`；`harness_timed`、`sniffing`、`request`、`socks5_connect`、`FakeSocks5`、`the_records`。
- 产出：

```rust
// crates/rurge-engine/tests/http.rs（P4 ①）
async fn slow_cut_off_origin(
    hold: Duration,
    delay: Duration,
) -> (SocketAddr, Arc<std::sync::atomic::AtomicUsize>);
```

**用例**（钉住今天的行为；各做一次变异看它红）

- `idle_starts_its_clock_at_the_first_poll`（`#[tokio::test(start_paused = true)]`，P3 ②）。
- `restart_gives_the_resent_request_its_whole_wait`（`#[tokio::test(start_paused = true)]`，P3 ③）。
- `a_request_sent_again_waits_its_whole_idle_on_the_new_connection`（`#[tokio::test]`，P4 ②）。

**判据**

- 三条用例绿；三处变异各让对应的用例红，红的样子（哪一条断言、信息）记进修正记录。
- 端到端单独循环跑二十次以上，全过；`an_idempotent_request_cut_off_is_sent_again_through_the_engine` 照过。
- 门禁绿：通过比基线多 3 条（预计 93 / 5452 / 0 / 17）。

---

## Task 2：基准的夹具、B1 与 B3

**改哪里**：新建 `crates/rurge-engine/tests/bench.rs`（P5 ～ P7）。

**接口**

- 消费：`common` 的 `Profile`、`harness`、`Harness`、`RecordStatus`、`wait_until`；`h.engine.http_counts()`、`h.engine.request_log()`。
- 产出（任务 3、4 用；签名可照实际调整，调了记进修正记录）：

```rust
struct Scale {
    rounds: usize,
    body: u64,
    requests: usize,
    warm_up: usize,
    names: usize,
    connections: usize,
    iterations: usize,
    quick: bool,
}
fn scale() -> Scale;

struct Latency {
    median: Duration,
    p99: Duration,
    max: Duration,
}
fn latency(samples: &mut [Duration]) -> Latency;
fn median(values: &[f64]) -> f64;
fn gbit_per_s(bytes: u64, took: Duration) -> f64;

/// `len` bytes of `b'a'`, handed on as slices of one 64 KiB block.
struct Repeat {
    left: u64,
    chunk: Bytes,
}
impl Repeat {
    fn new(len: u64) -> Repeat;
}
impl hyper::body::Body for Repeat {
    type Data = Bytes;
    type Error = std::convert::Infallible;
    // poll_frame, is_end_stream, size_hint: P5 ⑤
}

/// The origin of these benchmarks and the requests it served, by version.
struct Origin {
    addr: SocketAddr,
    h1: AtomicU64,
    h2: AtomicU64,
}
async fn origin(tls: Option<tokio_rustls::TlsAcceptor>, down: u64) -> Arc<Origin>;

enum Client {
    H1(hyper::client::conn::http1::SendRequest<Repeat>),
    H2(hyper::client::conn::http2::SendRequest<Repeat>),
}
async fn h1_client<I>(io: I) -> Client; // I: AsyncRead + AsyncWrite + Unpin + Send + 'static
async fn h2_client<I>(io: I) -> Client;
async fn plain_stream(addr: SocketAddr) -> TcpStream;

fn get(uri: &str, host: &str) -> Request<Repeat>;
fn post(uri: &str, host: &str, len: u64) -> Request<Repeat>;
async fn fetch(client: &mut Client, req: Request<Repeat>) -> (StatusCode, u64);
async fn upload(client: &mut Client, req: Request<Repeat>) -> String;
async fn timed_gets(
    client: &mut Client,
    uri: &str,
    host: &str,
    warm_up: usize,
    count: usize,
) -> Vec<Duration>;
fn thousand_rules() -> String;
```

**基准**（P6、P7）：`b1_plain_throughput`、`b1_plain_latency`、`b3_a_thousand_rules_latency`。

**判据**

- 三个基准在调试构建里用 `HTTP_BENCH_QUICK=1` 各单独跑一次：跑完、断言都过、打印的行照 P5 ⑧。输出抄进报告。
- 变异（P6 ③）：`b1_plain_latency` 在 `requests` 增量的断言上红；改回来。
- 门禁绿：测试二进制多一个（`bench`），忽略多 3（预计 94 / 5452 / 0 / 20）。

---

## Task 3：B2 与 B4

**改哪里**：`crates/rurge-engine/tests/bench.rs`（P8、P9）。

**接口**

- 消费：任务 2 的全部；`TlsFixture`；`connect_via_socks5`；`EngineShared`、`harness_with`。
- 产出：

```rust
fn mitm_section(hostname: &str, ca_p12: &str) -> String; // as tests/mitm.rs writes it
fn client_tls(
    roots: Arc<rustls::RootCertStore>,
    alpn: &[&[u8]],
    resume: bool,
) -> Arc<rustls::ClientConfig>;
async fn tls_stream(
    tcp: TcpStream,
    config: Arc<rustls::ClientConfig>,
    name: &str,
) -> tokio_rustls::client::TlsStream<TcpStream>;
```

**基准**（P8、P9）：`b2_decrypted_throughput`、`b2_decrypted_latency`、`b4_leaf_certificates`。

**判据**

- 三个基准用 `HTTP_BENCH_QUICK=1` 各单独跑一次：跑完、断言都过；输出抄进报告。
- 变异（P9 ⑤）：`b4_leaf_certificates` 在 `handshake_kind` 的断言上红；改回来。
- 门禁绿：忽略再多 3（预计 94 / 5452 / 0 / 23）。

---

## Task 4：B5 与 Q1

**改哪里**：`crates/rurge-engine/tests/bench.rs`（P10、P11）。

**接口**

- 消费：任务 2、3 的全部；`rurge_net::quic::{initial, testing}`。
- 产出：

```rust
struct Resident {
    peak: u64,
    now: u64,
}
fn resident() -> Option<Resident>;
fn parse_pair(output: &str) -> Option<(u64, u64)>;
fn parse_status(status: &str) -> Option<(u64, u64)>;
static MEASURED: AtomicBool;
```

**用例（先红）**：`the_readings_are_parsed_from_what_the_system_prints`（不忽略，P10 ②）。

**基准**（P10、P11）：九个 B5 与 `q1_reading_a_quic_client_hello`。

**判据**

- 守护用例先红后绿。
- 十个基准用 `HTTP_BENCH_QUICK=1` 各单独跑一次（每个一个进程）：跑完、断言都过；输出抄进报告。
- 再把两个 B5 放在同一次运行里（`--ignored b5_body_ --test-threads=1`），看第二个以 `MEASURED` 的信息 panic；报告里抄那一行。
- 门禁绿：通过多 1、忽略再多 10（预计 94 / 5453 / 0 / 33）。

---

## Task 5：release 下跑一遍、记数

**做什么**：照 P12。不改代码，不跑门禁。

**判据**

- 十六个基准与 `relay_throughput` 都在 release 下跑过，每个一个进程；日志在派工文件给的目录里。
- 读数照 P12 ⑥ 写进修正记录；没有一处写成"通过"或"不通过"。
- 提交只有本计划的修正记录。

---

## Task 6：收尾文档

**改哪里**：`docs/testing.md`、`docs/status.md`、`README.md`、`README_en.md`、`docs/requirements.md`、`CLAUDE.md`、设计第 16 节，内容照 P13。

**判据**

- 各文档照 P13 改完；状态、README 与需求文档里的每一个数（提交号、用例数、四个数、读数、日期）都能在修正记录、`git log` 或文件里找到出处。
- 中文、引号照各文件自己的（设计只用直引号；`docs/status.md`、两份 README、需求文档、`CLAUDE.md` 与 `docs/testing.md` 照文件里已有的写法）。
- 行尾照旧：`CLAUDE.md`、`docs/status.md`、`docs/testing.md`、两份 README、`docs/requirements.md` 在工作区是 CRLF，设计是 LF。
- 带本机账户名的文件数照旧 26；`git diff --check` 干净。

---

## 计划期决定

写计划时对着 `6d74039` 核对过下面引的名字与行号。行号会随前面任务的改动漂移，以名字为准；名字不同就照实改、记进修正记录，不改语义。标了"订正设计"的，已随本计划写进设计第 16 节。

| # | 事项 | 决定 |
| - | ---- | ---- |
| P1 | 任务的拆法 | 照设计第 2 节的大纲：0 核对与磁盘；1 遗留的用例；2 基准的夹具与 B1、B3；3 B2 与 B4；4 B5 与 Q1；5 release 下跑一遍、记数；6 文档收尾。任务 5 不改代码：基准的代码在任务 2 ～ 4 里各自在调试构建里试跑过 |
| P2 | 依赖 | 只有 `crates/rurge-http/Cargo.toml` 的 `[dev-dependencies]` 多一行 `tokio = { workspace = true, features = ["test-util"] }`：`start_paused` 与 `advance` 要它。<br>• 今天 `rurge-http` 的开发依赖只有 `rurge-net`（`testing`）、`rurge-proto`（`testing`）与 `tempfile`；另外七个 crate（`rurge`、`rurge-dns`、`rurge-engine`、`rurge-net`、`rurge-proto`、`rurge-proto-ssh`、`rurge-tun`）已这样写。<br>• 工作区的解析器是 3，`cargo test --workspace` 本来就带着 `test-util` 编 tokio：不多编一份 tokio，锁文件不变（520 个包）。<br>• `unsafe_functions.rs` 的 `LISTED`（23 项）、`sockets.rs` 的 `LISTED`（26 行）、带本机账户名的文件数（26）照旧 |
| P3 | 两条单元用例（任务 1） | 都在 `crates/rurge-http/src/engine.rs` 测试模块的末尾，`use super::*` 已带来 `Idle`、`Progress`、`Meter`、`SessionHandle`、`SessionInfo`、`HostName`、`BodyExt`。<br>① **永不出帧的正文** `Never`：`impl Body for Never { type Data = Bytes; type Error = std::convert::Infallible; }`，`poll_frame` 总答 `Poll::Pending`，`is_end_stream` 用缺省的 `false`——`Idle::poll_frame` 先看它，为真就答 `None`、不起表。名字与测试模块里已有的冲突就换一个，记进修正记录。<br>② **`idle_starts_its_clock_at_the_first_poll`**：<br>• `IDLE` 5 秒；`SessionHandle::new(1, SessionInfo::tcp(HostName::parse("origin.test"), 80))`；`Idle::new(Never, IDLE, Progress::new(), Meter::new(), None, handle)`；<br>• 建好之后、第一次 poll 之前 `tokio::time::advance(IDLE * 2).await`；<br>• 记下 `first_poll = tokio::time::Instant::now()`，用 `std::future::poll_fn(\|cx\| Poll::Ready(Pin::new(&mut body).poll_frame(cx))).await` poll 一次，断言 `is_pending()`（信息写：表从第一次 poll 起，10 秒前建的正文还不算迟）；<br>• `body.frame().await`（暂停的时钟在运行时没有别的事可做时走到下一个定时器）：断言答的是 `Some(Err(e))` 且 `e.to_string() == "request body stalled"`，再断言 `first_poll.elapsed() >= IDLE`。<br>• 变异：把 `Idle::new` 里的 `deadline: None` 临时改成 `deadline: Some(Box::pin(tokio::time::sleep(idle)))` → 第一次 poll 就答 `Err`，`is_pending` 的断言红。<br>③ **`restart_gives_the_resent_request_its_whole_wait`**：<br>• `IDLE` 10 秒；`let progress = Progress::new();`；`advance(3 s)`；`progress.moved()`；`advance(9 s)`；`progress.waiting()`；<br>• 断言 `progress.remaining(IDLE) == Duration::from_secs(1)` 与 `progress.is_waiting()`——先钉住重发之前的样子：上一次进展在第 3 秒，已过 12 秒，剩 1 秒；<br>• `progress.restart()`；断言 `progress.remaining(IDLE) == IDLE` 与 `!progress.is_waiting()`。<br>• 暂停的时钟下 `elapsed()` 精确到纳秒，`started` 与 `last` 按毫秒存，3、9、12 秒都是整毫秒，所以用 `==`。<br>• 变异：把 `restart` 的正文临时清空 → `remaining` 仍是 1 秒，第二组断言红 |
| P4 | 端到端（任务 1） | ① **`slow_cut_off_origin(hold, delay)`**：`cut_off_origin` 的正文搬进来，多两处：<br>• 第一条连接读完第二个请求之后，先 `tokio::time::sleep(hold)` 再走（今天是立刻走）；<br>• 后来的连接每读完一个请求，先 `sleep(delay)` 再答 `two`；<br>• `hold`、`delay` 为零时不睡。`cut_off_origin()` 改成 `slow_cut_off_origin(Duration::ZERO, Duration::ZERO)`，它的文档注释照旧，新函数的文档注释补这两处。<br>② **`a_request_sent_again_waits_its_whole_idle_on_the_new_connection`**：<br>• 源站 `slow_cut_off_origin(HOLD, DELAY)`；`FakeSocks5`（`connect_to` 是源站）与 Profile 同 `an_idempotent_request_cut_off_is_sent_again_through_the_engine`；引擎 `harness_timed(Profile { .. }, sniffing(), IDLE)`；<br>• `socks5_connect(h.socks(), "target.test", 80)`；第一个请求 `/a`：200、以 `one` 结尾；<br>• 记下 `let sent = std::time::Instant::now();`，第二个请求 `/b`：断言 200、以 `two` 结尾；断言 `sent.elapsed() > IDLE`（信息写：从发出到答完超过整个时限，只有在新连接上重新起表才答得上）；<br>• 两条记录都是 `Completed`、200；源站接受了两条连接。<br>③ **常数与时间**：`IDLE` 2,500 毫秒、`HOLD` 1,500 毫秒、`DELAY` 1,500 毫秒。<br>• 第一次发送在 1.5 秒时被断，离它的时限（2.5 秒）还有 1 秒；<br>• 重发在约 1.5 秒时起表，时限到约 4.0 秒；源站在约 3.0 秒时答，余 1 秒；<br>• `read_head` 每次读的时限是 5 秒，应答在发出之后约 3 秒到；<br>• 去掉调用：时限仍是 2.5 秒，源站 3.0 秒才答——差 0.5 秒，答 504。<br>④ **变异**：把 `retry` 里的 `progress.restart();`（今天 `engine.rs:2019`）临时删掉 → 第二个请求得到 `HTTP/1.1 504`（"Gateway timeout"；`show-error-page` 缺省开着，`crates/rurge-engine/src/http.rs` 取它），用例红，那条记录写 `upstream response timed out`。<br>⑤ 它用掉约 3 秒的真实时间。这是排得出"第一条连接用掉大半个时限才失败、在新连接上重发"那一刻的写法（设计第 9 节、V9）：只有借来的连接才会重发，所以第一条连接要先答一个请求 |
| P5 | 基准文件的骨架（任务 2） | ① **文件**：`crates/rurge-engine/tests/bench.rs`，`mod common;`。<br>② **模块的文档注释**（`//!`）：<br>• 它是什么：设计第 8 节的六项，量、不是门；CI 编译、不跑；<br>• 怎么跑：「表后的原文」的命令；`-p` 的例外与先看磁盘；<br>• 每个基准一个进程；`HTTP_BENCH_QUICK` 的用处。<br>③ **规模** `scale()`：<br>• 缺省是设计的数：`rounds` 5、`body` 256 MiB、`requests` 20,000、`warm_up` 200、`names` 1,000、`connections` 1,000、`iterations` 100,000；<br>• 环境变量 `HTTP_BENCH_QUICK` 设成任何非空的值时：1、8 MiB、500、20、50、50、2,000，`quick` 为真；<br>• B5 的 32 MiB 减 1 字节不随规模变。<br>④ **统计**：<br>• `latency`：排序；中位数取 `samples[n / 2]`（偶数个时取上中位）；p99 取最近秩 `samples[ceil(0.99 × n) − 1]`；最大取最后一个；<br>• `median`（各轮的吞吐）：同样取上中位；<br>• `gbit_per_s(bytes, took) = bytes × 8 / took.as_secs_f64() / 1e9`。<br>⑤ **正文 `Repeat`**：<br>• 一块 64 KiB、全是 `b'a'` 的 `Bytes`，建一次；<br>• `poll_frame` 每次交出 `chunk.slice(..min(left, 64 KiB))`、`left` 减掉；`left` 为零时答 `None`；<br>• `is_end_stream` 是 `left == 0`；`size_hint` 是 `SizeHint::with_exact(left)`（hyper 照它写 `Content-Length`）；<br>• 上行与下行都用它；`GET` 用 `Repeat::new(0)`。<br>⑥ **源站** `origin(tls, down)`：<br>• 绑 `127.0.0.1:0`；每条接受的连接 `set_nodelay(true)`，有 `tls` 就先握手，再交给 `hyper_util::server::conn::auto::Builder::new(TokioExecutor::new()).serve_connection(TokioIo::new(io), service)`，各在自己的任务里；<br>• 不给 builder 设定时器：hyper 的读头时限缺省 30 秒、没有定时器时不生效（hyper 1.11.1 `src/common/time.rs` 的 `check`），B5 的空闲连接不能被源站关掉；<br>• `GET /down`：200，正文 `Repeat::new(down)`；<br>• `GET /small`：200，正文是 16 字节的 `0123456789abcdef`；<br>• `POST /up`：逐帧读完请求的正文，数字节、记第一个字节；答 200，正文 `"{字节数} {第一个字节的十进制}"`，没有正文时第一个字节写 `-`；<br>• 别的：404；<br>• 每个请求按 `req.version()` 在 `h1` 或 `h2` 上加一；<br>• 应答的正文类型是 `BoxBody<Bytes, Infallible>`。<br>⑦ **客户端**：<br>• `plain_stream(addr)`：`TcpStream::connect` 之后 `set_nodelay(true)`（同 `relay_throughput` 与 rurge 自己的监听器）；经 SOCKS5 的由 `connect_via_socks5` 拿到之后同样 `set_nodelay(true)`；<br>• `h1_client` / `h2_client`：`http1::handshake(TokioIo::new(io))` / `http2::handshake(TokioExecutor::new(), TokioIo::new(io))`，连接在自己的任务里跑；<br>• 请求：经 HTTP 监听器的用绝对形式的 URI `http://origin.test:{port}{path}`；直连与经 SOCKS5 的 HTTP/1.1 用相对形式 `{path}`；HTTP/2 的用 `https://origin.test:{port}{path}`（hyper 的 HTTP/2 客户端从 URI 取 `:scheme` 与 `:authority`），不带 `Host`；HTTP/1.1 的都带 `Host: origin.test:{port}`；都带 `User-Agent: rurge-bench/1`；<br>• `fetch`：发，用 `BodyExt::frame` 一帧一帧地数 `data_ref().len()`、不留，交回状态码与字节数；<br>• `upload`：发 `POST`，把源站答的那一行读成字符串交回（它只有十几个字节）；<br>• `timed_gets`：先发 `warm_up` 个不计时的 `GET /small`，再发 `count` 个计时的，每个从发出之前到读完正文用 `std::time::Instant` 量；每个都断言 200 与 16 字节。<br>⑧ **打印**：<br>• 第一行：名字、构建（`cfg!(debug_assertions)` 为真是 debug，否则 release）、`quick` 与规模；<br>• 每轮一行；最后一行是汇总；<br>• 吞吐写到小数点后两位的 Gbit/s，延迟写到 0.1 微秒；<br>• 汇总写出经 rurge 与它的基线的比或差，和要对照的线（NFR-01 的 1 Gbit/s、50 µs） |
| P6 | B1（任务 2） | ① **`b1_plain_throughput`**：<br>• 源站 `origin(None, scale.body)`；引擎 `harness(Profile { hosts: "origin.test = 127.0.0.1", ..Profile::default() })`，规则只有 `FINAL,DIRECT`；<br>• 每轮 via 与 direct 各一次，次序轮流（偶数轮 via 先）；一次 = 新开一条连接，`GET /down`（断言 200、字节数等于 `body`）再 `POST /up`（断言答的是 `"{body} 97"`），各自计时；<br>• via 经 HTTP 监听器（`h.http()`、绝对形式）；direct 直连源站；<br>• 每轮打印四个 Gbit/s；汇总打印各项各轮的中位数、via 与 direct 的比、NFR-01 的 1 Gbit/s；<br>• 最后断言 `requests` 的增量是 `2 × rounds`（只有 via 的请求经引擎）。<br>② **`b1_plain_latency`**：<br>• 同样的源站与引擎；每轮 via 与 direct 轮流，各在一条新连接上 `timed_gets(warm_up, requests)`；<br>• 每轮打印那一轮的两个中位数；各轮的样本按 via、direct 各并在一起算 `latency`，打印两行与两个中位数之差（引擎多出的）；<br>• 断言 `requests` 的增量是 `rounds × (warm_up + requests)`。<br>③ **变异**（任务 2 的判据）：把 `b1_plain_latency` 的 via 一路临时改成直连源站（`plain_stream(origin.addr)`、相对形式的 URI）→ `requests` 增量的断言红 |
| P7 | B3（任务 2） | ① **规则** `thousand_rules()`，i 从 0 到 249、按这个次序、都是 `REJECT`：<br>• 250 行 `DOMAIN-SUFFIX,nomatch-{i:03}.test,REJECT`；<br>• 250 行 `DOMAIN-KEYWORD,nomatch-kw-{i:03},REJECT`；<br>• 250 行 `URL-REGEX,^http://nomatch-{i:03}\.test/,REJECT`；<br>• 250 行 `USER-AGENT,nomatch-agent-{i:03}*,REJECT`。<br>没有一行命中 `origin.test` 或 `rurge-bench/1`。<br>② **两个引擎**：`plain`（同 B1）与 `ruled`（`rules: &thousand_rules()`），同一个源站。B3 在同一个进程里量自己的 `plain` 一行，不拿 B1 那一次运行的数比（设计第 16 节第 9 条）。<br>③ **开跑之前查规则真的在请求上求值**：<br>• 在 `ruled` 上新开一条连接，发 `GET http://origin.test:{port}/sanity`、`User-Agent: nomatch-agent-007/1`；客户端看到什么都不看（连接可能被关，`Result` 丢掉）；<br>• `wait_until` 等 `request_log().recent(10)` 里有 `url` 以 `/sanity` 结尾的记录：断言它的 `status` 是 `RecordStatus::Rejected("REJECT".into())`、`rule` 含 `nomatch-agent-007`。<br>④ **量**：每轮 `plain` 与 `ruled` 轮流，各在一条新连接上 `timed_gets(warm_up, requests)`；两组样本各自并起来算 `latency`；打印两行、中位数之差与 p99 之差（一千条规则在每个请求上的开销）。对照 NFR-01 的"10 万条规则集下单次规则匹配 p99 < 50 µs"：那一条量的是 `rurge-rules` 的单次匹配，这里是引擎里一个请求实际付的（设计第 8.4 节）。<br>⑤ 断言两个引擎各自的 `requests` 增量（`ruled` 多 `/sanity` 那 1 条） |
| P8 | B2（任务 3） | ① **源站**：`let cert = TlsFixture::new(&["origin.test"]);`，`origin(Some(cert.acceptor(false)), body)`（ALPN `h2`、`http/1.1`）。<br>② **MITM 的 CA**：`TlsFixture::with_ca_name("rurge test MITM CA", &[])`（同 `tests/mitm.rs` 的 `mitm_ca`：主题与源站的 CA 不同）。<br>③ **两行，各建一个引擎**：<br>• `h1`：`[MITM]` 节 `mitm_section(&format!("origin.test:{port}"), &ca.ca_p12_base64("pw"))`——照 `tests/mitm.rs` 的写法抄进本文件；带端口的项让那个端口可嗅探；`h2` 缺省关着，对上游只给 `http/1.1`；<br>• `h2`：同一节多一行 `h2 = true`；<br>• 引擎 `harness_with(Profile { hosts: "origin.test = 127.0.0.1", sections, .. }, EngineShared { roots: Some(cert.roots()), sniff_wait: Duration::from_millis(300), ..EngineShared::default() })`（同 `harness_mitm`）。<br>④ **客户端的 TLS** `client_tls(roots, alpn, resume)`：照 `tests/mitm.rs` 的 `client_config`（ring 的提供者、只信给的根、发 SNI）；ALPN 只给这一行的（`http/1.1` 或 `h2`）；`resume` 为假时 `resumption = Resumption::disabled()`（B2 给真）。<br>• via 只信 MITM 的 CA，经 `connect_via_socks5(h.socks(), "origin.test", port)`；direct 只信源站的 CA，直连源站；<br>• 握手之后断言协商出的 ALPN 是这一行的。<br>⑤ **量**：`b2_decrypted_throughput` 同 B1 的吞吐，`b2_decrypted_latency` 同 B1 的延迟；每个基准先跑 `h1` 一行再跑 `h2` 一行，每行各轮 via 与 direct 轮流。<br>⑥ **断言**：<br>• 每行 `mitm.decrypted` 的增量等于这一行经 via 的连接数，`requests` 的增量等于经 via 的请求数；<br>• 源站按版本数的请求：`h1` 一行全是 HTTP/1.1，`h2` 一行全是 HTTP/2（via 与 direct 都是） |
| P9 | B4（任务 3） | ① **每轮一个新的引擎**（叶证书的缓存是空的）：<br>• `[Host]` 写 `*.bench.test = 127.0.0.1`；`[MITM]` 节 `hostname = *.bench.test`（不带端口：只在 443 上嗅探），CA 同 B2；<br>• `EngineShared { roots: Some(TlsFixture::new(&[]).roots()), sniff_wait: 300 ms, .. }`：B4 里引擎不连上游（MITM 先与客户端握手、再按请求连上游，总设计 P4-D7），给 `roots` 只为不读系统的根。<br>② **客户端**：`client_tls(ca.roots(), &[], false)`——只信 MITM 的 CA、不带 ALPN、不恢复会话（设计第 16 节第 7 条）。<br>③ **两遍**：名字 `n0000.bench.test` 起、共 `names` 个。<br>• 第一遍：每个名字经 `connect_via_socks5(h.socks(), name, 443)` 握一次，只握手、不发请求，握完就关；从连 SOCKS5 之前到握手完成计时；<br>• 第二遍：同样的名字、同样的次序再握一次。<br>④ **断言**：<br>• 每次握手 `handshake_kind() == Some(HandshakeKind::Full)`；<br>• 第一遍的叶证书（`peer_certificates()[0]` 的 DER）两两不同；<br>• 第二遍每个名字的 DER 与第一遍的相同（从缓存拿的）；<br>• 一轮结束 `wait_until` 等 `mitm.decrypted` 的增量到 `2 × names`。<br>⑤ **变异**（任务 3 的判据）：`client_tls` 临时不关会话恢复 → 第二遍在 `handshake_kind` 的断言上红（MITM 存着 `SESSION_SLOTS` 1,024 个会话）。<br>⑥ **打印**：每轮两遍各自的中位数、p99、最大；汇总把各轮的样本按遍并起来算 `latency`，打印两遍中位数之差（签一张叶证书）。签发在 tokio 的阻塞线程池里（`Authority::leaf` 的 `spawn_blocking`），这个差是那条线程上的签发加两次交接（设计第 16 节第 6 条） |
| P10 | B5（任务 4） | ① **读数** `resident() -> Option<Resident>`，照 `crates/rurge-engine/src/observe/memory.rs` 的三支 `#[cfg]`，文档注释写从那里照抄、多读当前的一个数：<br>• Windows：起一次 `powershell -NoProfile -Command`，命令是「表后的原文」那一行（`{pid}` 换成 `std::process::id()`），`parse_pair` 读最后一个非空行的两个数——峰值是 `PeakWorkingSet64`，当前是 `WorkingSet64`；<br>• Linux：`parse_status` 读 `/proc/self/status` 的 `VmHWM` 与 `VmRSS`（KiB 乘 1024）；<br>• 别处 `None`。<br>② **守护用例** `the_readings_are_parsed_from_what_the_system_prints`（不忽略、先红）：<br>• `parse_pair("\r\n123 456\r\n") == Some((123, 456))`；`parse_pair("x") == None`；`parse_pair("1") == None`；<br>• `parse_status` 读一段写好的 status（含 `VmHWM:\t    2048 kB` 与 `VmRSS:\t    1024 kB` 两行，与别的行）得 `Some((2097152, 1048576))`；少 `VmRSS` 一行时 `None`。<br>③ **一个进程一个**：`static MEASURED: AtomicBool`；每个 B5 用例一开头 `assert!(!MEASURED.swap(true, Ordering::SeqCst), …)`，信息是「表后的原文」那一句。`resident()` 是 `None` 时打印「表后的原文」的那一句并返回（不失败）。<br>④ **空闲连接**：每个用例建好源站与引擎（要的话）；预热一条（同样的路、一个 `GET /small`、关掉）；读 `before`；开 `connections` 条连接，每条走完一个 `GET /small`（断言 200）之后留着不关；读 `after`；打印 `before`、`after` 与峰值、当前各自的 `(after − before) / connections`。五个用例：<br>• `b5_idle_connections_relayed_tls`：TLS 的源站（`cert.acceptor_with_alpn(false, &[b"http/1.1"])`）；引擎 `general: &format!("force-http-engine-hosts = origin.test:{port}")`（让那个端口可嗅探），没有 `[MITM]` 节，`EngineShared` 同 B2（`roots` 是源站 CA 的根、`sniff_wait` 300 毫秒）；客户端经 SOCKS5、只信源站的 CA、HTTP/1.1。断言 `sniffed.tls` 的增量是 `connections + 1`（含预热）、`mitm.decrypted` 不变。<br>• `b5_idle_connections_plain`：明文源站；经 HTTP 监听器（同 B1）。断言 `requests` 的增量是 `connections + 1`。<br>• `b5_idle_connections_decrypted`：同 B2 的 `h1` 一行。断言 `mitm.decrypted` 与 `requests` 的增量各是 `connections + 1`。<br>• `b5_idle_connections_tls_direct`：TLS 的源站，直连，HTTP/1.1。<br>• `b5_idle_connections_plain_direct`：明文源站，直连。<br>经 rurge 的每条连接多占的，是"经 rurge 的那一项减去它的直连基线"，由协调者在任务 5 的评审里用两次运行的数算（各在自己的进程里，设计第 16 节第 8 条）。<br>⑤ **正文的缓冲**：`BIG` 是 32 MiB 减 1 字节（`REQUEST` 的上限是"大于 32 MiB 才不改"）；每个用例读 `before`，一条连接发 `POST /up`、正文 `Repeat::new(BIG)`，读 `after`，打印峰值的 `(after − before) / BIG`（缓冲有几份）。<br>• `b5_body_rewritten`：经 HTTP 监听器，`sections` 是 `[Body Rewrite]` 一节、一行 `http-request ^http://origin\.test:{port}/up ^a b`；断言源站答 `"{BIG} 98"`（第一个字节换成了 `b`）。<br>• `b5_body_streamed`：经 HTTP 监听器，没有那一节；断言 `"{BIG} 97"`。<br>• `b5_body_direct`：直连源站；断言 `"{BIG} 97"`。<br>⑥ **叶证书的缓存** `b5_leaf_cache`：引擎同 B4。<br>• 先用同一个名字 `one.bench.test` 握 `names` 次：请求记录装到缺省的 1,000 条，之后每条新记录挤掉一条旧的；缓存里只有一张；读 `before`；<br>• 再用 `n0000.bench.test` 起的 `names` 个不同的名字各握一次（缓存装满，`LEAF_SLOTS` 是 1,000）；读 `after`；打印 `(after − before) / names`；<br>• 断言 `mitm.decrypted` 的增量是 `2 × names`。<br>⑦ quick 时 `connections` 与 `names` 是 50：读数没有意义，只看跑不跑得通 |
| P11 | Q1（任务 4） | `q1_reading_a_quic_client_hello`（`#[test]`，不经引擎、不建套接字）：<br>• `let hello = big_client_hello(Some("held.test")); let [first, second] = chrome_frames(&hello, 7);`，两个数据报 `Initial::new(1, RFC_DCID, 0, first).seal(Some(1200))` 与 `Initial::new(1, RFC_DCID, 1, second).seal(Some(1200))`，在计时之外造好；<br>• 每次计时 `read_first(&d1)`（断言 `Some(Hello::Partial(r))`）加 `r.read(&d2)`（断言 `Hello::Whole(m)` 且 `m.len() == hello.len()`），输入与结果用 `std::hint::black_box` 包住；<br>• 先不计时地跑 `iterations / 100` 次，再计时 `iterations` 次；<br>• 打印中位数、p99、最大（微秒），对照阶段 3 / M5 设计第 4.2 节估的"十微秒"量级 |
| P12 | release 下跑一遍（任务 5） | ① **开始之前**：<br>• `df -h /d`，剩不到 20 GiB 就停下报告；<br>• PowerShell `Get-CimInstance Win32_Processor \| Select-Object Name, NumberOfCores, NumberOfLogicalProcessors`，记下处理器；`rustc -V` 记下编译器。<br>② **编一次**：`cargo test -p rurge-engine --release --test bench --no-run`，日志写进派工文件给的目录；之后 `df -h /d` 再记一次。<br>③ **逐个跑**，每个一个进程，次序照下面，每条带 `timeout 1800`，输出各写进一个日志：「表后的原文」的命令，名字依次是<br>`b1_plain_throughput`、`b1_plain_latency`、`b3_a_thousand_rules_latency`、`b2_decrypted_throughput`、`b2_decrypted_latency`、`b4_leaf_certificates`、`b5_idle_connections_plain_direct`、`b5_idle_connections_plain`、`b5_idle_connections_tls_direct`、`b5_idle_connections_relayed_tls`、`b5_idle_connections_decrypted`、`b5_body_direct`、`b5_body_streamed`、`b5_body_rewritten`、`b5_leaf_cache`、`q1_reading_a_quic_client_hello`。<br>④ **`relay_throughput`**：`RELAY_BENCH_MIB=64 timeout 3600 cargo test -p rurge-engine --release --lib relay_throughput -- --ignored --nocapture`。`pump` 经 TCP 的那几行是 B1 的对照（设计第 8.2 节）；64 MiB 把整张表的时间缩到约四分之一（`docs/testing.md` 记着 256 MiB 时约 26 ～ 27 分钟）。<br>⑤ **失败**（panic、超时、heap corruption）：单独重跑一次；再失败就停下报告，不改代码。<br>⑥ **记数**：<br>• 每个基准的第一行与汇总行原样抄进修正记录，一组一行（B1 与 B3、B2、B4、B5 的空闲连接、B5 的正文与叶证书、Q1、`relay_throughput`）；<br>• 写明是一台 Windows 11 x86-64 开发机上 release 的一次运行、不是 CI，处理器与编译器；<br>• 对照的线写成数字（例如"B1 via 下行 X Gbit/s，是 NFR-01 那条线的 Y 倍"），不写"通过"、"不通过"；<br>• B5 的"经 rurge 减直连"由协调者在评审里算，Pi 只抄数。<br>⑦ 不改代码、不跑门禁；提交只有修正记录 |
| P13 | 收尾文档（任务 6） | ① **`docs/testing.md`**（CRLF）：<br>• 命令一节 `relay_throughput` 那一行之后加一行：「表后的原文」的命令，注释照那一节已有行的写法用中文写——阶段 4 / M5b 的基准（B1 ～ B5、Q1，十六个忽略的用例）、不作门禁、每个单独一个进程、跑之前看 D: 剩 20 GiB 以上、`HTTP_BENCH_QUICK=1` 在调试构建里缩小规模只看跑不跑得通、读数在 `docs/status.md` 的阶段 4 / M5b 一节；<br>• 「测试约定（全文）」阶段 4 的部分加一条：基准是量不是门；`-p` 的例外现在有三处（`rurge-tun` 的三个基准、`relay_throughput`、`--test bench`）；内存的读数一个进程一个（`MEASURED`）；遗留的端到端用真实的时钟、约 3 秒、余量 1 秒。<br>② **`docs/status.md`**（CRLF）：阶段 4 / M5a 一节之后、`## 附：依赖方向一条的原文` 之前，新的一节 `### M5b（基准与遗留）`，结构照 M5a 一节：<br>• 开头一段；做了什么；各任务（提交、评审提交、用例增量、全量的四个数）；<br>• 读数：每个基准的汇总、经 rurge 减直连的差、对照的线写成数字；<br>• 没做的与不要当成已经有的：基准不是门、一台机器上的一次运行、macOS 没有内存的读数、Linux 的读法只在守护用例里对过写好的文本；<br>• 以后必须知道的事；去向写着项目所有者的（设计第 15 节第 6 条：NFR-01 的线怎样用在经引擎的路径上；推送）；<br>• 依赖附录：`rurge-http` 的开发依赖开了 tokio 的 `test-util`，锁文件不变；<br>• 下一步：M5 做完；项目所有者推送；之后是 M6 的细化设计。<br>③ **`README.md` 与 `README_en.md`**（CRLF，两份意思一致）：阶段 4 的状态引用、「现在：」那一段、路线图第 5 条——M5 做完、阶段 4 剩 M6，提交在本地、CI 没有见过，没有手工验收。特性表不改（M5b 没有新的特性）。<br>④ **`docs/requirements.md`**（CRLF）：第 7 节阶段 4 的状态行按日期接一句：M5b 做完（计划路径、任务 0 ～ 6）；读数的去处；M5 设计的路径（M5a 评审留下的一处，那一行今天没有它）；下一步。第 4 节的需求文字不改。<br>⑤ **`CLAUDE.md`**（CRLF）：只改「现在在哪里」的阶段 4 一条，接一句：**M5b 做完了**（日期、任务 0 ～ 6）、**M5 至此做完**；提交在本地、CI 没有见过；下一步是项目所有者推送、之后读回那次 CI，再之后是 M6 的细化设计；细节在 `docs/status.md` 的阶段 4 / M5b 一节。CI 一条与别的条目不改。<br>⑥ **设计第 16 节**（LF）：另起一段 `**执行 M5b 时**：`，从第 11 条起，照第 4 条的写法：改了正文哪一处（引原文）、改成什么、以修正记录哪一行为准；没有就写"没有"。<br>⑦ 收尾的提交之后，协调者评审；评审过了是停点（本计划开头） |
| P14 | 现有的用例 | 见下面的表。表里"预计照过"的若红，停下报告，不改期望 |

### 表后的原文（照抄）

每个代码块里的文字就是要的那一串，没有为 Markdown 加的转义。

**忽略的理由**（每个基准的 `#[ignore = …]`）

```text
a measurement, not a gate: run it alone, in release, with --nocapture
```

**跑一个基准的命令**（模块的文档注释、任务 5、`docs/testing.md`）

```text
cargo test -p rurge-engine --release --test bench -- --ignored --exact <name> --nocapture --test-threads=1
```

**读内存的 PowerShell 命令**（P10 ①，`{pid}` 换成本进程的号）

```text
$p = Get-Process -Id {pid}; "$($p.PeakWorkingSet64) $($p.WorkingSet64)"
```

**两句信息**（P10 ③）

```text
one B5 measurement per process: run it alone with --exact
no reading on this platform
```

**客户端的 `User-Agent`**（P5 ⑦）

```text
rurge-bench/1
```

### P14 的表：要改的、要留意的现有用例

| 用例 | 为什么 | 任务 | 预计 |
| ---- | ------ | ---- | ---- |
| `crates/rurge-engine/tests/http.rs` 的 `an_idempotent_request_cut_off_is_sent_again_through_the_engine` | `cut_off_origin` 改成调 `slow_cut_off_origin(Duration::ZERO, Duration::ZERO)`，零时不睡 | 1 | 照过 |
| `crates/rurge-http` 的全部用例 | 开发依赖多开 tokio 的 `test-util`，生产代码不变 | 1 | 照过 |
| `crates/rurge-engine/tests/` 的其余用例 | `bench.rs` 是新的目标，不碰别的文件；`common` 不改 | 2 ～ 4 | 照过 |
| `crates/rurge/tests/sockets.rs` 的扫描用例 | 只读 `crates/*/src` 的生产代码；M5b 不改它 | 全部 | 照过 |
| `crates/rurge-platform/tests/unsafe_functions.rs` | 不加放开的函数 | 全部 | 照过 |
| `crates/rurge-config/tests/secrets.rs` | 不加带凭据的写法 | 全部 | 照过 |

---

## 执行期修正记录

下面写"执行者的裁定"的，是 Pi 在设计与计划的原则下定的；写"协调者的裁定"的，是协调的会话代项目所有者定的。项目所有者都可以改。计划的正文不改：正文里说得不对的地方，以这张表的行为准。

| # | 任务 | 内容 | 提交 |
| - | ---- | ---- | ---- |
| 1 | 0 | 开工前核对第 1、2 项：`git status --short` 空，分支 `phase4-m5`，`HEAD` `ab54a6f`，`git log --oneline 6d74039..HEAD` 只有 `ab54a6f` 这一个；`git diff --stat 52e3590 HEAD -- crates tests Cargo.lock .github` 是空的，基线对着的就是这棵代码树。读了设计第 16 节（先）与第 8、9、10、11、12、15 节、本计划的计划期决定与表后的原文与 P14 的表，以及 `docs/status.md` 阶段 4 / M5a、M4b、M4a、M2a、M1b 五节的「以后的会话必须知道的事」，与 M1 细化设计第 8.6、8.10 节。只改文档。 | 本提交 |
| 2 | 0 | 名字与签名逐项对过（设计第 12 节 V7、V9）：`engine.rs` 的 `Progress`（2648）与 `impl`（2663）的 `new` 2664、`since_started` 2674、`restart` 2683、`moved` 2691、`waiting` 2698、`is_waiting` 2702、`remaining` 2707，`head_deadline` 2717，`Idle` 2737、`new` 2759、`poll_frame` 2788（先看 `is_end_stream` 2792，`deadline` 的 `get_or_insert_with` 2796）、`is_end_stream` 2836，`Stalled` 2632，`retry` 1969、`progress.restart();` 在 **2019**，`timed_out` 1715（`pages.error` 时 504 标题 `Gateway timeout`），`Pages` 152；`pool.rs` 的 `Meter::new` 106；`mitm/authority.rs` 的 `LEAF_SLOTS` 59（1000）、`SESSION_SLOTS` 63（1024）、`Authority::leaf` 374（`spawn_blocking` 402）；`mitm/decrypt.rs` 的 `server_config` 357 与对上游的两份配置（`h2` 开给 `h2` 与 `http/1.1`、关只给 `http/1.1`，`Mitm::build` 137 ～ 149）；`rewrite/body.rs` 的 `REQUEST` 47（33,554,432，33,554,431 不算 over，196、228 都是 `>`）；`rurge-inbound` 的 `SessionHandle::new(id, session) -> Arc` 129；`rurge-config` 的 `SessionInfo::tcp(dst_host, dst_port)` 90、`HostName::parse(s) -> HostName`（不用 `unwrap`）types.rs 14；`rurge-engine` 的 `http.rs` 的 `request_count` 加一 127、`Pages` 取 `show-error-page` 135 ～ 137，`Engine::request_log` 1573、`http_counts` 1581，`HttpCounts` serve.rs 201、`SniffedCounts` 187、`MitmCounts` mitm.rs 128（`decrypted` 在 serve.rs 652 加）；`EngineShared` shared.rs 173（`roots` 184、`sniff_wait` 209）；`relay.rs` 的 `relay_throughput` 913（`RELAY_BENCH_MIB` 977、`set_nodelay` 929）；`observe/memory.rs`；用例夹具 `common/mod.rs` 的 `Profile` 200、`text` 215、`harness` 231、`harness_with` 244、`harness_timed` 260、`Harness` 44（`http` 57、`socks` 60）、`connect_via_socks5` 393、`wait_until` 428、`the_records` 446；`tests/http.rs` 的 `sniffing` 20、`request` 53、`read_head` 61、`socks5_connect` 30、`cut_off_origin` 2630；`tests/mitm.rs` 的 `mitm_section` 27、`mitm_ca` 36、`client_config` 42、`handshake` 61、`harness_mitm` 135；`rurge-proto` 的 `TlsFixture`（tls.rs 的 48、`new` 59、`with_ca_name` 67、`roots` 98、`ca_p12_base64` 138、`acceptor` 156、`acceptor_with_alpn` 162）；`rurge-net` 的 `quic::testing`（`big_client_hello` 843、`chrome_frames` 985、`Initial::new` 647、`Initial::seal` 669、`RFC_DCID` 1006）与 `quic::initial`（`read_first` 85、`Hello` 69、`HelloReader::read` 122）。**没有一条不同**；计划未写的具体数（见下面第 5、6、7 行）照实记。 | 本提交 |
| 3 | 2、3 | 外部库的事实（任务 2、3 用的）：① **hyper 一族**：`rurge-engine` 的 `[dependencies]` 没有 `bytes`、`http`，两者从 hyper 引——`hyper-1.11.1/src/body/mod.rs:67` 是 `pub use bytes::{Buf, Bytes}`，`:68-72` 是 `http_body::{Body, Frame, SizeHint}` 与 `Incoming`；`hyper-1.11.1/src/lib.rs:121` 是 `pub use http::{header, HeaderMap, Method, Request, Response, StatusCode, Uri, Version}`（`:115` 另有 `pub use http`）。`hyper-1.11.1/src/proto/h1/role.rs:1215` 用 `write!(FastWrite(dst), "{} ", msg.head.subject.1)` 写整个 `Uri`。`hyper-1.11.1/src/common/time.rs:70` 的 `check`：缺省时限配 `Time::Empty` 时不生效（`warn` 后 `None`）。② **一处与计划写法不同**：计划与 P5 ⑦写「hyper 的 HTTP/2 客户端从 URI 取 `:scheme` 与 `:authority`」，实际取它的是 h2 包——`h2-0.4.19/src/client.rs:1659` `Pseudo::request(method, uri, protocol)`，`:1661` 起按版本处理、`:1674` 起缺两者且版本是 HTTP/2 就答 `MissingUriSchemeAndAuthority`；`hyper-1.11.1/src/proto/h2/client.rs` 里没有 `uri`。**按代码做**：B2 的 HTTP/2 请求照旧写绝对形式 `https://origin.test:{port}{path}`、不带 `Host`（语义不变，只是把出处写成 h2）。`http-body-1.1.0/src/lib.rs` 的 `Body`：`poll_frame`（62，无缺省）、`is_end_stream`（77，缺省 `false`）、`size_hint`（85，缺省）。③ **rustls 0.23.43**：`Resumption::disabled` 在 `src/client/client_conn.rs:513`（`resumption` 字段同一文件 191）、`handshake_kind` 在 `src/common_state.rs:188`（`HandshakeKind` 在 785）；从 crate 根引是 `rustls-0.23.43/src/lib.rs:546` `pub use crate::common_state::{CommonState, HandshakeKind, IoState, Side}`；`peer_certificates` 139、`alpn_protocol` 148；`tokio_rustls::client::TlsStream::get_ref` 在 `tokio-rustls-0.26.4/src/client.rs:217`（交回 `(&IO, &ClientConnection)`）。与计划写的一行号相同。 | 本提交 |
| 4 | 1 ～ 4 | 引擎与夹具的其余事实（后面的任务照它们做）：`progress.restart();` 在 `crates/rurge-http/src/engine.rs:2019`；`Idle::new` 里 `deadline: None` 在 `:2770`（`Idle {` 在 2767）；`Progress::restart` 的正文 2683 ～ 2689（重置 `started`、`last`、`waiting_on_source`）；`timed_out` 在 `pages.error` 时答 504（1715 ～ 1733）；**`show-error-page` 缺省为真在 `crates/rurge-config/src/general.rs:446`**，引擎在 `crates/rurge-engine/src/http.rs:135-137` 取它（P3 ④ 的那一行在配置 crate 里，不在引擎 crate 里）。`FakeSocks5` 重导出在 `crates/rurge-engine/tests/common/mod.rs:23-25`、定义在 `crates/rurge-proto/src/testing/socks5.rs:52`（`spawn` 264）。`tests/common/mod.rs:5` 有 `#![allow(dead_code, unused_imports)]`，`bench.rs` 只用其中一部分不会多出警告。请求记录的条数上限缺省 1,000 由命令行定：`crates/rurge/src/cli/run.rs:1769`（`unwrap_or(1000).max(1)`），引擎侧是 `crates/rurge-engine/src/observe.rs:201` 的 `RequestLog::new(capacity)`。`rurge-engine` 的 `[[bench]]` 只有 `relay`（`Cargo.toml:52-54`）。 | 本提交 |
| 5 | 0 | 四个数与磁盘：`Cargo.lock` 的 `[[package]]` 520 个；`crates/rurge-platform/tests/unsafe_functions.rs:18` 的 `LISTED` 是 `(&str, &str); 23`、实际 23 项；`crates/rurge/tests/sockets.rs:58` 的 `LISTED` 是 `(&str, &str, &str); 26`、实际 26 行；带本机账户名的文件 26 个；`crates/rurge-engine/tests/` 下没有叫 `bench` 的文件。`df -h /d`：剩 34G（同设计第 8.1 节记的 34 GiB），够任务 5 的 20 GiB 与门禁的 15 GiB 下限。都与计划写的相同。 | 本提交 |
| 6 | 1 | 现有用例（P14 的表，任务 1）：`crates/rurge-engine/tests/http.rs:2680` 的 `an_idempotent_request_cut_off_is_sent_again_through_the_engine` 今天断言 `/a` 200 以 `one` 结尾、`/b` 200 以 `two` 结尾、两条记录 `Completed` 且 200、源站 `accepted == 2`。P4 ① 把 `cut_off_origin`（2630）改成 `slow_cut_off_origin(Duration::ZERO, Duration::ZERO)`，零时不睡、行为不变，**照过**站得住。`cut_off_origin(` 的调用处只有这一处（2630 是定义、2681 是这一条）。`crates/rurge-http` 的 `#[test]`/`#[tokio::test]` 共 **288 条**，任务 1 只给它的开发依赖多开 `test-util`、只在 `engine.rs` 的 `mod tests`（3852 之后）加用例，生产代码不变，**照过**站得住。`crates/rurge-http` 里今天没有 `start_paused`/`advance`/`pause`（与开发依赖没有 `test-util` 相符）。 | 本提交 |
| 7 | 2 ～ 4 | 现有用例（P14 的表，任务 2 ～ 4）：`crates/rurge-engine/tests/` 的其余用例共 **678 条**，分布在 31 个文件里（含 `common/log_capture.rs`；`bench.rs` 是新的目标，不碰别的文件、`common/mod.rs` 也不改），**照过**站得住。`crates/rurge-engine/tests/` 下今天没有 `HTTP_BENCH_QUICK`、也没有叫 `bench` 的模块。`.github/workflows/ci.yml` 跑测试的一步（`Run the tests`，ci.yml:356-365）是 `cargo test --workspace --no-fail-fast`，**没有** `--ignored`/`--include-ignored`，所以忽略的基准在 CI 上不被跑（只被编译）；`bench` 作业（ci.yml:378-387）只跑 `cargo bench -p rurge-rules --no-run` 与 `cargo bench -p rurge-dns --no-run`。 | 本提交 |
| 8 | 全部 | 现有用例（P14 的表，写「全部」的三条）：`crates/rurge/tests/sockets.rs:192` 的 `every_outbound_socket_is_made_in_one_place` 只读 `crates/*/src` 的生产代码，`LISTED` 仍 26 行；`crates/rurge-platform/tests/unsafe_functions.rs:45` 的 `every_unsafe_exception_is_on_the_list`，M5b 不加放开的函数、`LISTED` 仍 23 项；`crates/rurge-config/tests/secrets.rs`，M5b 不加带凭据的字段或写法。三条**照过**站得住。 | 本提交 |
| 9 | 0 | 门禁（一台 Windows 11 x86-64 开发机上的一次运行，**不是 CI**）：`cargo fmt --all --check` 退出码 0（`gate-task-0-fmt.log`）；`cargo clippy --all-targets -- -D warnings` 退出码 0（`gate-task-0-clippy.log`）；`cargo test -j 8 --workspace --no-fail-fast` 的两个测试二进制 `outbounds_shadow_tls` 与 `outbounds_snell` 以 `STATUS_HEAP_CORRUPTION`（0xc0000374）退出，第一次 **91 个结果行、5423 通过、0 失败、17 忽略**；单独重跑那两个（`gate-task-0-rerun-outbounds_shadow_tls.log` 6 通过、`gate-task-0-rerun-outbounds_snell.log` 20 通过，各 0 失败），合起来 **93 / 5449 / 0 / 17**，与基线相同。另：`cli` 那一条日志行 `reload failed, keeping current config` 是用例自己的输出，不是失败；没有 `LNK1104`，没有留下 `rurge.exe` 孤儿进程。 | 本提交 |
| 10 | 0 | 协调者核过任务 0（`b8073ea`，第 1 ～ 9 行）。门禁的日志重数过：全量 91 / 5423 / 0 / 17，加上单独重跑的 `outbounds_shadow_tls`（6 通过）与 `outbounds_snell`（20 通过），合起来 93 / 5449 / 0 / 17，与基线相同（一台机器上的一次运行，不是 CI）；fmt 与 clippy 的退出码是 0。第 2 ～ 4 行抽查过 `SessionHandle::new`（交回 `Arc<SessionHandle>`）、`HostName::parse`（交回 `HostName`）、`hyper-1.11.1/src/lib.rs:121` 的 `pub use http::{…}`、`crates/rurge-config/src/general.rs:446` 的 `show_error_page: true`。订正第 4 行两处，不开修正轮：① `#![allow(dead_code, unused_imports)]` 在 `crates/rurge-engine/tests/common/mod.rs` 的第 6 行，不是第 5 行；② 第 4 行引的请求记录上限是命令行的缺省（`crates/rurge/src/cli/run.rs:1769`），基准经共用夹具建引擎（`harness_at` 调 `runtime_timed`），条数是夹具写死的 `request_log_size: 1000`（`crates/rurge-engine/tests/common/mod.rs:127`）：P10 ⑥ 的"装到缺省的 1,000 条"以夹具的这个数为准，数值相同。另记一件后面的任务要用的：`wait_until` 的时限是 5 秒（`common/mod.rs:428`），等更久的用 `wait_until_within`。只改文档，没有跑门禁。 | 本提交 |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
