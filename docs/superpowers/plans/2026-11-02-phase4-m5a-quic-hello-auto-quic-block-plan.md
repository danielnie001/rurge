# 阶段 4 / M5a「读 QUIC 里的 ClientHello 与 `auto-quic-block`」实施计划

> **执行方式**：交给 Pi，由协调的会话逐个任务派工、逐个任务评审（设计第 2 节、M5-D1；做法同 M1a ～ M4b）。
>
> - 协调的会话每个任务开一个 Pi 会话，交给它这个任务的全文与要用的接口（派工文件）。Pi 做完把报告写进派工文件指定的路径，在本地提交。
> - 协调者读改动、重跑门禁、评审。有问题回到同一个 Pi 会话修正，再评审；过了才派下一个任务。
> - 每个任务：先写失败的用例、看到红；再实现到绿；跑门禁；对照本任务的「判据」与「全局约束」逐条自查；最后提交（**只在本地，不推送**）。钉住现有行为的用例（任务里写明"不要求先红"的）可以一写就绿。
> - 与计划的出入记进末尾的「执行期修正记录」，**不改计划的正文**。
>
> **停点**：没有。M5a 没有平台代码、没有只在 CI 上编译的代码。M5a 做完之后不停下来等推送（设计第 2 节）：接着对着那时的代码写 M5b 的计划；做完 M5b 由项目所有者推送（那时还没有推送的一起），推送之后读回那次 CI。

**目标**：FR-HTTP-04（`auto-quic-block`）、FR-RULE-07 / 08 拨号之前部分里 QUIC 的那一块、FR-HTTP-10 的备注（设计第 1 节的 M5a 部分）。

- 一条 UDP 流的第一个数据报是客户端的 QUIC Initial 时，从它与后面的数据报里的 Initial 包读出 CRYPTO 帧、按偏移拼出 ClientHello（设计第 4 节）；拼齐之前持住这条流，至多 300 毫秒、16 个数据报（第 5 节）；读出的 SNI 在求规则之前写进会话。
- `auto-quic-block` 生效：读到了 ClientHello、开关为真、回落之后的 TCP 会被嗅探并解密时阻断（第 6 节）。
- `GET /v1/rurge/http` 多 `quic` 四个数；两种备注、一种说明（第 5.4、7 节）。
- 手工清单加 M5a 的六项；M5a 的收尾文档。

**架构**：

- 不加包、不加工作区边、不改任何 `Cargo.toml`（P2）。
- `rurge-net` 的 `quic::initial` 读包、读帧、拼 CRYPTO、看握手消息的头；`quic::testing` 造 Initial 包、数据报与真实的 ClientHello（M5-D2）。`rurge-net` 不依赖 `rurge-http`：拼出的握手消息由引擎交给 `rurge_http::tls::parse_client_hello`。
- 引擎的 `udp.rs` 在 `run_flow` 里持住、写 SNI、计数与备注，`forward` 与拒绝带着持着的数据报（P5 ～ P7）；`serve.rs` 从 `sniffable_session` 抽出入口、名字、两张名单与端口那几步（`sniffable_target`），与新的 `auto_quic_blocks` 共用，后者再调 `decrypting`（P9，M5-D11）。
- `rurge-api` 的 `/v1/rurge/http` 读 `HttpCounts::quic`（P11）。

**技术栈**：都已在 `Cargo.lock` 里，源码在本机 cargo 注册表里。**不下载、不加新包**。

- rustls 0.23.43 的 `quic` 模块（`src/quic.rs`）：`Suite::keys`（第 892 行）、`Keys { local, remote }`（第 904 行）、`DirectionalKeys { header, packet }`（第 591 行，两个字段都是 `Box<dyn …>`，两个 trait 都要求 `Send + Sync`）、`HeaderProtectionKey::{encrypt_in_place, decrypt_in_place}`（第 679、707 行；只掩去首字节标出的包号长度那么多个字节）、`PacketKey::{encrypt_in_place, decrypt_in_place}`（第 727、760 行；后者交回明文的切片）、`ClientConnection::new_with_alpn`（第 179 行，要 TLS 1.3 与能用于 QUIC 的套件）、`write_hs`（第 430 行）。
- `rustls::pki_types::ServerName::IpAddress`：rustls 对地址不发 SNI，`client_hello(None, ..)` 靠它造不带 SNI 的 ClientHello。
- tokio：`tokio::time::Instant` 与 `sleep_until`（暂停的时钟之下按暂停的时钟走）；`#[tokio::test(start_paused = true)]`（引擎的 dev-dependencies 开着 `test-util`，`udp.rs` 已有这样的用例）。
- quinn：只在 `initial.rs` 的一条用例里当客户端，同现有的 `a_real_quinn_client_initial_is_one`。

**依据**：

- M5 细化设计（下面写"设计"指它）：`docs/superpowers/specs/2026-11-02-phase4-m5-auto-quic-block-bench-design.md`。
  - M5a 用到第 3 ～ 7、10、11、13 ～ 15 节。
  - **先读它第 16 节**：写本计划时改的三处。
  - 设计与本计划的「计划期决定」不一致的，以计划为准。
- `docs/status.md` 阶段 4 / M4b 一节的「以后的会话必须知道的事」整段适用，M4a、M2a、M1a 三节的同名段落也适用；阶段 3 / M5 一节（QUIC 的分类与 `block-quic`）。
- 写计划时对着 `2ca12b2`（分支 `phase4-m5`，比本地 `main` 多设计的那一个提交）核对过的事实写在「计划期决定」里。外部库的事实对着注册表里锁定版本的源码。

## 全局约束

**依赖**

- **不改任何 `Cargo.toml`**：引擎与 API 的 dev-dependencies 已开着 `rurge-net` 的 `testing` 特性（`crates/rurge-engine/Cargo.toml:35`、`crates/rurge-api/Cargo.toml:28`）。
- `Cargo.lock` 的 `[[package]]` 今天 520 个，每个任务之后照旧 520 个，锁文件一个字不变。`cargo` 要下载任何东西就停下报告。
- 依赖方向不变：`rurge-net` 不依赖 `rurge-http`（反过来才有）；`rurge-http` 不依赖 `rurge-engine`。
- `unsafe_code`：不加放开的函数，`crates/rurge-platform/tests/unsafe_functions.rs` 的 `LISTED` 仍是 23 项。
- `crates/rurge/tests/sockets.rs` 的 `LISTED` 仍是 26 行：M5a 的生产代码不建套接字、不问解析库、不建不带钩子的连接器；`quic::testing` 的新构造器也不建套接字。

**缺省的 `cargo test` 不碰系统**

- 同 M4b：不建网卡，不改系统的任何设置；不执行 `netsh`、`route`、`ipconfig`、`ip`、`resolvectl`、`networksetup`。
- **测试从不把任何 CA 装进信任库，也不读系统的信任库**：Profile 里有 `[MITM]` CA 的引擎一律给 `EngineShared.roots`（`tests/mitm.rs` 的 `harness_mitm`、`common` 的 `harness_trusting`）。不给的话 `crates/rurge-engine/src/runtime.rs` 的 `build_mitm` 会读系统的根。
- **QUIC 的用例只喂数据报、不完成 QUIC 握手**（总设计第 14 节第 5 条，设计第 10 节）：喂给 `read_first` / `HelloReader::read`、SOCKS5 的 UDP 关联与内存设备上的虚拟网卡；quinn 的真客户端只连一个只收不答的回环套接字。

**测试的约定**

`CLAUDE.md`「测试约定」整节适用。写用例之前读 `docs/testing.md`「测试约定（全文）」的阶段 3 / M5 与阶段 4 各节。与本计划直接相关的：

- 只用回环与端口 0。名字（`held.test`、`origin.test`，RFC 样例里的 `example.com`）经一行 `[Host]` 钉到 `127.0.0.1`，第二道防线是共用夹具的 `dns-server`（回环上的 `MockDns`）。收数据报的服务器是用例自己绑的回环套接字。
- 放行的 QUIC 流的数据报发往它的目标。目标是钉到 `127.0.0.1` 的名字的 443 或 8443 时，发往没有人收的回环端口（设计第 16 节第 2 条）：在回环上，可以；不放行到回环之外的地方。
- 等待都有时限。记录用有时限的轮询等（`wait_until`）；固定等待只许"断言什么都没发生"，注释写它等的是什么、余量多大。
- 持住的两个上限（300 毫秒、16 个）只在暂停时钟的单元用例里量边界；端到端只在"只发第一个数据报"的几例里等它过去（有界）。
- 新写的、带等待或并发的用例提交前单独循环跑二十次以上：任务 2 的 quinn 那一条、任务 3 与任务 4 的端到端。
- 随机的构造（Chrome 的排法、打散）用固定的种子，断言的信息里带种子。
- 抓日志的用例用 `LogCapture`，跑在当前线程运行时。
- 收数据报的缓冲给 64 KiB（测试服务器与从 `PacketSocket` 收包的都是）。
- **内联的测试模块放在文件的末尾**：`crates/rurge/tests/sockets.rs` 读生产代码，读到第一个 `#[cfg(test)] mod … {` 为止。

**门禁与这台开发机**（每个任务末尾一次；说"通过"之前必跑）

- 门禁：`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test -j 8 --workspace --no-fail-fast`。
- 本机 msvc 工具链没有 rustfmt。跑 `cargo fmt` 之前让环境变量 `RUSTFMT` 指向 gnu 工具链里同版本的 rustfmt：用户目录下的 `.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe`。不要为此安装或升级工具链。
- **门禁的日志用重定向写、再追加退出码，不用 `Tee-Object`**；单独重跑的结果也写进日志再引，引之前先看它在、看它写的是什么。
- 聚焦地跑用例也经整个工作区：
  - 单元测试：`cargo test -j 8 --workspace --lib -- <过滤词>`（如 `quic::initial`、`quic::testing`、`udp::tests`）；
  - 集成测试：`cargo test -j 8 --workspace --test <名字> -- <过滤词>`。
- **不用 `-p`**：它另编一份依赖，`target/` 涨得很快，写满过 D: 盘。**也不用 `--release`**（M5a 没有基准）。
- 跑全量之前 `df -h /d`：剩余不足约 15 GiB 就停下报告。
- 测试二进制以 `STATUS_HEAP_CORRUPTION`（本机管控软件注入的钩子 DLL）或 `STATUS_ACCESS_VIOLATION` 退出：单独重跑那一个二进制，报告里写明。
- `cli` 的用例偶发 `give_up` 20 秒超时：单独重跑 `cli`，把标准错误原样抄进报告。
- rustc 在增量编译里报内部编译器错误：删掉出错的那个目标在 `target/debug/incremental/` 下的目录再编。
- 回环的 TCP / UDP 7、9、13、17、19 号端口在这台机器上开着，CI 上关着：用例不假定任何固定端口的状态。
- 结果照实记：本机的全量运行是"一台机器上的一次运行，不是 CI"；需要参照服务端的互操作用例在本机立刻返回、算在通过的数里，要说出来。

**仓库的规矩**（`CLAUDE.md`「必须遵守的规矩」整节适用；与本计划直接相关的）

- **凭据**：M5a 不加带凭据的字段。SNI 与 TCP 一侧一样不是凭据。
- **日志的分级**：新加的只有两行 DEBUG（「表后的原文」），带目标的名字与端口：名字与地址只进 DEBUG。INFO 的会话日志行不变（设计第 5.4 节）。`decrypting` 的两行 DEBUG 照旧，文字不变。
- **诊断**：不设新码。
- **网络与进程**：会话的进程照旧读进程槽，M5a 不动它。
- **出站套接字只有一处建法**：M5a 不建套接字。
- 文档与报告用写文件 / 改文件的工具改。**永远不要用把一个文件的内容经管道写回同一个文件的命令**（2026-10-27 这样写满过 D: 盘）。
- 计划末尾两张表里已写下的行不改写、不删除。追加新行时**不要把已有的行当成锚点替换掉**（M2a 任务 9 这样删掉过一行）。

**写法**

- 注释、日志、错误文本与备注用英文，照所在文件的写法：文档注释写"为什么"与前提，句子完整。注释里提到设计或计划，写 `phase 4 M5 design 4.4`、`the phase 4 / M5a plan's P4`。
- 提交说明照 `git log` 的样子写，标题用中文：`feat(net): …`、`feat(engine): …`、`feat(api): …`、`test(net): …`、`test(engine): …`、`docs: …`。正文说做了什么、改了哪些现有用例的期望、为什么。Pi 的提交不带署名行。
- 只在本地提交：不 `push`、不 `pull`、不 `fetch` 远端、不 `amend`、不改写历史。

**不做的事**

- 只运行本仓库构建出来的二进制与 Pi 自己。不下载、不安装任何东西。不手动跑 `rurge run`、`rurge dns lookup`；`rurge service` 只用 `--dry-run`。
- 参考实现：GPL 的（sing-box、mihomo）只取事实，代码不抄；设计第 4.6 节的表就是要的事实。
- 不做设计第 1 节「不做」一栏的事，也不做这些：
  - 从没有拼齐的 ClientHello 里取 SNI、gQUIC、Retry 之后的第二把密钥、ECH（M5-D15）；
  - 按 SNI 换拨号的目标（总设计 P4-D5）；自适应的表按 SNI 记（M5-D13）；
  - 读到了 ClientHello 而没有阻断时写原因（M5-D10）；测试用的调时限的旋钮（M5-D7）；
  - M5b 的基准与遗留。

**计划没有写到、要做决定的地方**

- 能按设计与总设计的原则定的：自己定，记进修正记录，写"执行者的裁定"。
- 要停下来列出、交给协调者的：去向写着"项目所有者"的（设计第 15 节）；会改变 M5-D1 ～ D18 的。
- 现有用例变红：P14 的表点了名的，照计划改，在提交说明里说；没点名的，停下来，记进修正记录，不顺手改期望值。

## 评审要点

设计没写透、而最可能咬到使用者的几种输入，与负责它们的任务和用例：

1. **真实浏览器的第一批数据报**：Chrome 打散的 CRYPTO（乱序、切碎、夹 PING 与 PADDING）、跨包的排法（第一个包放开头与结尾，SNI 在第二个数据报里）、合并的包、尾部填零。
   - 期望：拼出的消息与 ClientHello 逐字节相同；第一个数据报之后答 `Partial`，第二个之后答 `Whole`。
   - 用例（任务 1、2）：`chrome_frames_put_the_start_and_the_end_first`、`chrome_arrangements_come_whole`、`chaos_in_one_packet_comes_whole`、`two_initials_in_one_datagram_come_whole`、`trailing_zeros_end_the_walk`、`a_real_quinn_client_hello_across_two_datagrams`。
2. **第二个数据报丢了，或客户端只发了一个**：
   - 期望：300 毫秒之后照样放出去，发往目标的数据报一个不少；记录没有 SNI、带 `incomplete` 那一句备注、`quic.incomplete` 加一；不阻断。
   - 用例（任务 3、4）：`holding_lets_go_at_300_ms`、`a_hello_that_never_completes_goes_out_after_300_ms`、`an_incomplete_hello_is_not_blocked`。
3. **一个客户端往同一个目标连发许多数据报，或在持住时走掉**：
   - 期望：至多持 16 个，其余照旧排在流的队列里、放了之后照常转发；关联在持住时结束，这条流不留记录、不计数。
   - 用例（任务 3）：`holding_lets_go_at_16_datagrams`、`holding_ends_with_the_association`、`a_flow_whose_association_ends_while_held_leaves_no_record`。
4. **名单命中、回落之后的 TCP 却不会被解开的 QUIC**：开关关着、没有可用的 CA、来源不允许、名单排除、`always-raw-tcp-hosts`、不带端口的项在 TCP 一侧不嗅探的端口上、没有 SNI 而端口不是 443。
   - 期望：不阻断（阻断只让客户端慢，回落之后也不解开）；`quic.blocked` 不变。
   - 用例（任务 4）：`auto_quic_block_off_lets_quic_through`、`with_mitm_off_quic_goes_through`、`without_a_usable_ca_quic_goes_through`、`a_source_mitm_does_not_take_lets_quic_through`、`a_negated_entry_lets_quic_through`、`a_raw_tcp_host_is_not_blocked`、`a_bare_entry_does_not_block_quic_on_8443`、`a_hello_without_sni_is_judged_by_its_target`。
5. **损坏或恶意的 Initial**：重叠处不同的字节、33 段、超过 64 KiB 的偏移与长度、截断的帧、Initial 不许有的帧、CONNECTION_CLOSE、后来的数据报里每一位的翻转与每一个前缀。
   - 期望：不 panic；读不成的答 `GaveUp`（记录写 `unreadable` 那一句、`quic.unreadable` 加一、照旧按没有 SNI 求规则）；打不开的包、别的版本、短包头不算读不成。
   - 用例（任务 2、3）：`the_ways_to_give_up`、`thirty_two_pieces_are_read_and_the_thirty_third_gives_up`、`no_bit_flip_in_a_later_datagram_panics`、`no_prefix_of_a_later_datagram_panics`、`an_unreadable_hello_is_noted`。

---

## 文件结构

| 文件 | 任务 | 职责 |
| --- | --- | --- |
| `crates/rurge-net/src/quic/testing.rs`；`crates/rurge-net/src/quic/initial.rs` 的测试模块（只改 `use`、挪走样例） | 1 | 造 Initial 包、数据报与真实的 ClientHello；两份 RFC 样例 |
| `crates/rurge-net/src/quic/initial.rs` | 2 | `read_first`、`HelloReader`、`Hello`、`MAX_HELLO`；`is_client_initial` 由 `read_first` 定 |
| `crates/rurge-engine/src/{udp.rs,serve.rs,engine.rs,lib.rs}`、`crates/rurge-engine/tests/{common/mod.rs,udp.rs,sniff.rs}` | 3 | 持住、SNI、`forward` 与拒绝带着持着的数据报、计数与备注 |
| `crates/rurge-engine/src/{serve.rs,udp.rs}`、`crates/rurge-engine/tests/{mitm.rs,tun.rs}` | 4 | `sniffable_target`、`auto_quic_blocks`、`open` 里的阻断 |
| `crates/rurge-api/src/routes/rurge_http.rs`、`crates/rurge-api/tests/api.rs` | 5 | `/v1/rurge/http` 的 `quic` |
| `docs/api/phase4.md`、`docs/acceptance/phase4-manual.md`、`docs/testing.md`、`docs/surge-compatibility-matrix.md` | 6a | 写行为的文档 |
| `docs/status.md`、`README.md`、`README_en.md`、`docs/requirements.md`、`CLAUDE.md`、设计第 16 节 | 6b | 写进度的文档 |

模块的名字与拆法可以照实际调整，调了记进修正记录。

---

## Task 0：开工前核对

**做什么**：对着当时的分支核对下面每一项。有出入的记进「执行期修正记录」，一项一行，写"计划写 X，代码是 Y，按 Y 做"；按记录调整后面的任务。**不改计划的正文。** 没有出入也记一行。

1. **工作树**：`git status --short` 空；当前分支 `phase4-m5`；`2ca12b2` 在历史里，之后只多了本计划的提交（本计划、`docs/README.md`、`CLAUDE.md`）。
2. **读**：设计第 16 节（先读）、第 3 ～ 7、10、11、13 ～ 15 节；本计划的「计划期决定」、「表后的原文」与 P14 的表；`docs/status.md` 阶段 4 / M4b、M4a、M2a、M1a 四节的「以后的会话必须知道的事」与阶段 3 / M5 一节。
3. **名字与签名**（设计第 12 节 V1 ～ V6、V8）：用 `git grep -n` 逐项找到、记下行号；与「计划期决定」写的不同的照实记。
   - `rurge-net` 的 `quic/initial.rs`：`MIN_DATAGRAM`、`SAMPLE`、`MIN_LENGTH`、`is_client_initial`、`Header`、`quic_version`、`initial_kind`、`initial_suite`、`varint`、`header`、`opens`；测试模块的十三条用例、`DCID`、`hex_digit`、`unhex` 与两个样例常量。`quic/mod.rs` 里 `testing` 的 `cfg`。`quic/testing.rs` 的 `TestCert`（`new`、`client_tls` 的提供者与版本）、`client_initial`、`client_initial_with_quic_bit`。
   - rustls 0.23.43（注册表里锁定版本的 `src/quic.rs`）：「技术栈」列的各项与行号；`new_with_alpn` 的参数与它对配置的要求；`HeaderProtectionKey` 掩去几个包号字节。
   - `rurge-http` 的 `tls.rs`：`ClientHello`（`sni: Option<String>`、`alpn: Vec<Vec<u8>>`）、`parse_client_hello`（要恰好一个握手消息）；`lib.rs` 的 `pub mod tls`。
   - `rurge-engine` 的 `udp.rs`（V1）：
     - `RETRY_AFTER`、`QUEUE`、`QUEUED_BYTES`、`Queued`、`Association`；
     - `serve`（drop guard、`flows`、`run_flow` 的调用处）；
     - `run_flow`（`turn_back`、协议的三支、`new_handle_with`、`Turned`、`follow(open(..))`、`Ok` 与 `Err` 两支、`refused` 的调用处、`drop(first)`、`done`）；
     - `open`（`is_suspended`、`snapshot`、`choose_policy`、`SelectCtx`、`resolve_ready`、REJECT 那一处、`direct`、`carries_udp`、`setting`、QUIC 那一段）、`forward`、`drain`；
     - 测试模块的 `queued`、`stuck`、`NoClient`、`Hears`、`HearsOnce` 与调 `forward` 的两条用例。
   - `rurge-engine` 的 `serve.rs`、`engine.rs`、`runtime.rs`、`lib.rs`（V3、V4）：
     - `raw_listed`、`SniffCounts`、`SniffedCounts`、`HttpCounts`；
     - `sniffable_session` 的七步、`named_at_a_proxy_port`、`named_target`、`widened_by_mitm`、`decrypting`（次序与两行 DEBUG）、`reaches_the_rules`；
     - `rurge_http::mitm::source_of`、`Mitm::widens`；
     - `engine.rs` 的 `sniff_counts`、`mitm_counts` 与它们的初始化、`http_counts`、`runtime`、`mitm_on`、`set_mitm`、`is_suspended`；
     - `runtime.rs` 的 `Runtime::{config, mitm}`、`build_mitm`（`roots` 为 `None` 时读系统的根）；
     - `lib.rs` 的 `pub use serve::{…}`；`git grep -n 'HttpCounts {'` 列出的全部字面量。
   - `rurge-config`：`MitmConfig::auto_quic_block`（`crates/rurge-config/src/mitm.rs`）、`Config::mitm`；`General` 的 `block_quic`、`always_raw_tcp_hosts`。
   - `rurge-api`（V4）：`routes/rurge_http.rs` 的 `HttpJson` 与它的几个 `…Json`、`http()`；`tests/api.rs` 的 `rurge_http_counts_start_at_zero`。
   - 用例的夹具（V5、V6）：
     - `tests/common/mod.rs` 的 `Profile`、`harness`、`harness_trusting`、`harness_with`、`udp_associate`、`UdpAssociation`、`udp_records`、`wait_until`、`quic_initial`、`lookalike_quic_initial`、`udp_echo`；
     - `tests/udp.rs` 的 `routed`、`finished`、`OwnClient`、`own_association`、`OwnAssociation` 的 `write`、`heard`、`refused`；
     - `tests/mitm.rs` 的 `mitm_section`、`mitm_ca`、`harness_mitm`、`set_mitm` 的用法、`client-source-address` 的写法；
     - `tests/common/tun.rs` 的 `Rig::{new, on}`、`guarded`、`profile`；`tests/tun.rs` 的 `a_blocked_quic_flow_answers_with_icmp` 与 `prohibited_from`；
     - `tests/sniff.rs` 的 `HttpCounts` 字面量；`LogCapture` 的用法。
4. **数**：
   - `Cargo.lock` 里 `[[package]]` 的个数（520）；
   - `unsafe_functions.rs` 的 `LISTED` 的项数（23）；
   - `sockets.rs` 的 `LISTED` 的行数（26）；
   - 带本机账户名的文件数：派工文件给出要数的那个名字，数出来是 26。
5. **现有用例**（P14 的表）：逐条找到、读它断言什么，把"预计要改 / 预计照过"逐条写进修正记录，一条用例一句。表里没有的照实补一行。另扫一遍：`git grep -n 'is_client_initial\|client_initial\|quic_initial()\|HttpCounts {\|"rewrite":'`。
6. **门禁**：跑一遍，记下测试二进制数、通过数、失败数与忽略数（M4b 任务 11 评审时是 93 / 5380 / 0 / 17，一台机器上的一次运行，不是 CI）。

**判据**：修正记录里有任务 0 的行；门禁的数字记下。

---

## Task 1：`quic::testing` 的构造器

**改哪里**：

- `crates/rurge-net/src/quic/testing.rs`：P3 的全部；文件末尾一个 `#[cfg(test)] mod tests`；开头的模块注释加一句（Initial 包、数据报与真实的 ClientHello 的构造器）。
- `crates/rurge-net/src/quic/initial.rs`：测试模块里删掉 `hex_digit`、`unhex` 与两个样例常量（连同它们前面的注释挪到 `testing.rs`），`use` 改成从 `crate::quic::testing` 取。本地的 `DCID` 换成 `RFC_DCID`，或留着而值不变，二选一，写进报告。用例的正文不改。

**接口**

- 消费：`initial::{quic_version, initial_suite}`（`pub(crate)`），`TestCert::client_tls` 的提供者与版本（ring、只有 TLS 1.3）。
- 产出（P3）：

```rust
// rurge-net::quic::testing
pub const RFC9001_CLIENT_INITIAL: &str; // 原样挪来
pub const RFC9369_CLIENT_INITIAL: &str; // 原样挪来
pub const RFC_DCID: &[u8];              // 两份样例的目的连接 ID，0x8394c8f03e515708
pub fn unhex(hex: &str) -> Vec<u8>;     // 原样挪来

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    Padding(usize),
    Ping,
    Ack { largest: u64, delay: u64, first_range: u64, ranges: Vec<(u64, u64)>, ecn: Option<[u64; 3]> },
    Crypto { offset: u64, data: Vec<u8> },
    ConnectionClose { error: u64, frame_type: u64, reason: Vec<u8> },
    Raw(Vec<u8>),
}
impl Frame {
    pub fn encode(&self, out: &mut Vec<u8>);
}

#[derive(Clone, Debug)]
pub struct Initial {
    pub version: u32,
    pub dcid: Vec<u8>,
    pub keys_from: Vec<u8>,
    pub number: u64,
    pub number_len: usize,
    pub frames: Vec<Frame>,
}
impl Initial {
    pub fn new(version: u32, dcid: &[u8], number: u64, frames: Vec<Frame>) -> Initial;
    pub fn seal(&self, size: Option<usize>) -> Vec<u8>;
}

pub fn long_packet(version: u32, kind: u8, dcid: &[u8], payload: usize) -> Vec<u8>;
pub fn datagram(packets: &[Vec<u8>], pad_to: usize) -> Vec<u8>;
pub fn client_hello(server_name: Option<&str>, alpn: &[&[u8]]) -> Vec<u8>;
pub fn big_client_hello(server_name: Option<&str>) -> Vec<u8>;
pub fn hello_initial(dcid: &[u8], hello: &[u8]) -> Vec<u8>;
pub fn hello_initials(dcid: &[u8], hello: &[u8], at: usize) -> [Vec<u8>; 2];
pub fn chaos_frames(pieces: &[(u64, &[u8])], seed: u64) -> Vec<Frame>;
pub fn chrome_frames(hello: &[u8], seed: u64) -> [Vec<Frame>; 2];
```

**用例（先红）**：在 `testing.rs` 末尾的测试模块里。拆开一个包用测试模块里的一个小函数（去掉包头保护、解密、交回明文）：它是对 `seal` 的独立检验，不调 `initial.rs`。

- `every_frame_reads_back_as_written`：一个带 `Ack`（`ranges` 一对、`ecn` 三个计数）、`Ping`、`Padding(3)`、`Crypto { offset: 5, data: b"hello" }`、`ConnectionClose` 的 `Initial`。
  - `seal(None)` 与 `seal(Some(1200))` 各一次：拆开的明文以这些帧的写法开头；`Some(1200)` 的包恰好 1200 字节，明文其余是零。
  - v1、v2（`0x6b3343cf`）、草案 `0xff00001d` 各一次；`number_len` 1 ～ 4 各一次。
- `a_packet_sealed_with_other_keys_opens_only_with_them`：`keys_from` 不同于 `dcid` 的包，用 `dcid` 的密钥拆不开，用 `keys_from` 的拆得开。
- `the_hello_rustls_writes_is_one_client_hello`：
  - `client_hello(Some("held.test"), &[b"h3"])` 的第一个字节是 1、24 位的长度加 4 等于总长、含 `b"held.test"`；
  - `client_hello(None, &[b"h3"])` 的形状相同。
- `a_big_hello_needs_two_packets`：`big_client_hello(Some("held.test"))` 的长度在 1500 ～ 1900 之间，含 `b"held.test"`。
- `hello_initials_carry_the_two_halves`：`hello_initials(RFC_DCID, &hello, 600)` 两个数据报各 1200 字节，拆开之后的 CRYPTO 帧分别是 `[0, 600)` 与 `[600, len)`，包号 0 与 1。
- `chrome_frames_put_the_start_and_the_end_first`：种子 0 ～ 31。
  - 第一个包的 CRYPTO 帧合起来是 `[0, k)` 与 `[len - e, len)`，`55 <= k <= 86`；第二个包的是 `[k, len - e)`；两包合起来逐字节等于 ClientHello。
  - 每个包至少一个 `Ping`；两个包（包号 0、1）都封得进 1200 字节。
  - 断言的信息里带种子。
- `chaos_frames_cover_their_pieces`：种子 0 ～ 63，两段输入。
  - 交回的 CRYPTO 帧合起来逐字节等于输入的两段；CRYPTO 帧的个数在 段数 + 2 ～ 段数 + 10 之间；`Ping` 有 2 ～ 10 个。
  - 断言的信息里带种子。
- `datagram_pads_with_zeros`：`datagram(&[p], 1300)` 长 1300，以 `p` 开头，其余是零。
- `a_long_packet_ends_where_its_length_says`：`long_packet(1, 0b01, RFC_DCID, 100)` 的 Length 是 100，包恰好在 Length 之后的 100 个字节处结束。

**实现**：照 P3。

**判据**

- 上面的用例都绿；`initial.rs` 的十三条现有用例照过（只改了 `use`）。
- `testing.rs` 不建套接字（`sockets.rs` 的扫描照过）。
- 门禁绿。

---

## Task 2：读 ClientHello

**改哪里**：`crates/rurge-net/src/quic/initial.rs`。模块注释的最后一段改写（帧照 `phase 4 M5 design 4` 读）；`opens` 由 P4 ② 的 `open` 取代；`is_client_initial` 改由 `read_first` 定；新的用例加在测试模块里。

**接口**

- 消费：任务 1 的构造器与样例。
- 产出（P4）：

```rust
// rurge-net::quic::initial
pub const MAX_HELLO: usize = 64 * 1024;

#[derive(Debug)]
pub enum Hello {
    /// type 1, the 24-bit length and the body: 4 + length bytes
    Whole(Vec<u8>),
    Partial(HelloReader),
    /// why, for DEBUG only
    GaveUp(&'static str),
}

pub fn read_first(datagram: &[u8]) -> Option<Hello>;

pub struct HelloReader { /* 私有：P4 ⑦ */ }
impl HelloReader {
    pub fn read(self, datagram: &[u8]) -> Hello;
}

pub fn is_client_initial(datagram: &[u8]) -> bool; // = read_first(datagram).is_some()
```

**用例（先红）**

下面的 `hello` 是 `big_client_hello(Some("held.test"))`，`small` 是 `client_hello(Some("held.test"), &[b"h3"])`，"d1 / d2"是 `hello_initials(RFC_DCID, &hello, 600)` 的两个数据报；读者交回 `Partial` 时用 `read` 接着读。

- **两份样例**：
  - `the_rfc_9001_client_initial_reads_whole`：`read_first(&unhex(RFC9001_CLIENT_INITIAL))` 是 `Whole(m)`，`m.len() == 241`，`m[..4] == [0x01, 0x00, 0x00, 0xed]`，`m` 含 `b"example.com"`（RFC 9001 附录 A.2 的 CRYPTO 帧：偏移 0、长度 241，包号 2）。
  - `the_rfc_9369_client_initial_reads_whole`：RFC 9369 的样例读出与上一条相同的 241 个字节（两份附录 A.2 的 CRYPTO 帧相同）。
- **跨数据报与排法**：
  - `a_hello_across_two_datagrams_comes_whole`：d1 → `Partial`，d2 → `Whole(m)`，`m == hello`。
  - `the_second_half_first`：第一个数据报带 `[600, len)`、第二个带 `[0, 600)`（包号 0、1，各封到 1200）→ `Partial`、`Whole(hello)`。
  - `chrome_arrangements_come_whole`：种子 0 ～ 31，`chrome_frames` 的两个包（包号 0、1，各封到 1200）→ `Partial`、`Whole(hello)`；断言的信息带种子。
  - `chaos_in_one_packet_comes_whole`：种子 0 ～ 63，`chaos_frames(&[(0, &small)], seed)` 封在一个 1200 字节的包里 → 直接 `Whole(small)`。
  - `two_initials_in_one_datagram_come_whole`：`datagram(&[包号 0 带 small 的 [0, 100) 的 seal(None), 包号 1 带 [100, len) 的 seal(None)], 1200)` → `Whole(small)`。
  - `an_initial_then_a_0rtt_packet`：
    - `[带整个 small 的 Initial, long_packet(1, 0b01, RFC_DCID, 100)]` 填到 1200 → `Whole`；
    - `[带 [0, 100) 的 Initial, 0-RTT 的长包, 带 [100, len) 的 Initial（包号 1）]` 填到 1200 → `Whole`（按 Length 跳过 0-RTT，读到第三个包）。
  - `trailing_zeros_end_the_walk`：`datagram(&[带 [0, 600) 的 Initial 的 seal(None)], 1300)` → `Partial`；再读 d2 → `Whole(hello)`。
  - `a_later_datagram_with_another_dcid_still_opens`：第二个数据报的 Initial 的 `dcid` 是 `[0x42; 8]`、`keys_from` 是 `RFC_DCID` → `Whole(hello)`。
- **重发与重叠**：
  - `a_retransmitted_piece_is_taken`：d1 之后再读一遍 d1 → `Partial`（相同的字节，不算读不成）；之后 d2 → `Whole(hello)`。
  - `overlapping_pieces_that_agree_merge`：第一个带 `[0, 700)`、第二个带 `[600, len)` → `Whole(hello)`。
- **段数**：`thirty_two_pieces_are_read_and_the_thirty_third_gives_up`：第一个数据报一个 Initial 带 32 个一字节的 CRYPTO 帧，偏移 10、12、…、72（互不相接，不含 0）→ `Partial`；第二个带偏移 200 的一字节 → `GaveUp("CRYPTO data in more than 32 pieces")`。
- **读不成**：`the_ways_to_give_up`，每一例的读者从 d1 起（第一个字节与长度那两例从第一个数据报起），断言原因的字符串（「表后的原文」）：
  - 第二个数据报带 `[500, 700)` 而其中一个字节与 `hello` 不同 → "CRYPTO data that changed when it came again"；
  - 带 `ConnectionClose` → "the client closed the connection"；
  - 带 `Raw(vec![0x08, 0x00, 0x01, 0x61])`（STREAM）→ "a frame an Initial may not carry"；
  - 截断的帧，三种：`Raw(vec![0x06, 0x00])`（偏移之后没有长度）、`Raw(vec![0x06, 0x00, 0x10, 0x01])`（长度 16 而只有 1 个字节）、`Raw(vec![0x02, 0x05, 0x00])`（ACK 缺两项）→ 都是 "a frame cut short"。截断的帧前面放一个 `Ping`、`number_len = 4`、`seal(None)`，让明文恰好以它结尾（不然 PADDING 的零会被读成长度）；
  - 第一个数据报带 `Crypto { offset: 0, data: [2, 0, 0, 4, 0, 0, 0, 0] }` → `read_first` 答 `GaveUp("the first handshake message is no ClientHello")`，而 `is_client_initial` 照旧是 `true`（读不成不改变"是 QUIC"，设计第 4.3 节）；
  - 第一个数据报带 `Crypto { offset: 0, data: [1, 0x01, 0x00, 0x00] }`（4 + 65536 > 64 KiB）→ "a ClientHello longer than 64 KiB"；
  - 第二个数据报带 `Crypto { offset: 65_530, data: vec![0; 10] }` → "CRYPTO data beyond 64 KiB"。
- **不算读不成**（答 `Partial`）：
  - `an_initial_that_does_not_open_is_skipped`：第二个数据报的 Initial 由 `keys_from = [0x42; 8]` 封 → `Partial`；第三个是 d2 → `Whole(hello)`。
  - `a_packet_of_another_version_ends_the_walk`：第二个数据报是 v2 的 Initial（v2 的密钥封、带后半）→ `Partial`。
  - `a_short_header_datagram_is_nothing`：第二个数据报以 `0x40` 开头、1200 字节 → `Partial`。
- **包号**：
  - `packet_numbers_decode_as_rfc_9000_a3`：`packet_number(Some(0xa82f30ea), 0x9b32, 2) == 0xa82f9b32`；`packet_number(None, 2, 4) == 2`；`packet_number(Some(0), 1, 1) == 1`；`packet_number(Some(255), 0, 1) == 256`。
  - `a_second_packet_with_a_one_byte_number_opens`：第一个包号 0、`number_len = 4`，第二个包号 1、`number_len = 1` → `Whole(hello)`。
- **quinn 的真客户端**：`a_real_quinn_client_hello_across_two_datagrams`（设计第 16 节第 3 条）。
  - 照 `a_real_quinn_client_initial_is_one` 建：`TestCert::new().client_tls(..)` 给 64 个生成的 ALPN 名字，`connect_with(.., "localhost")`。
  - 从只收不答的回环套接字一个一个收数据报（64 KiB 的缓冲，每个 5 秒为限，至多 4 个），第一个交给 `read_first`、之后的交给 `read`，直到 `Whole`。
  - 断言：第一个数据报读完是 `Partial`；拼出的消息第一个字节是 1、含 `b"localhost"`。
  - 不完成握手；`Connecting` 留到收完再丢。
- **不 panic**：
  - `no_bit_flip_in_a_later_datagram_panics`：d2 的前 64 个字节的每一位各翻一次，每次用一个新读者（`read_first(&d1)`）读：不 panic，答什么都行。
  - `no_prefix_of_a_later_datagram_panics`：d2 的每一个前缀（0 ～ 1199 字节）各用一个新读者读：都是 `Partial`（包被截断，走到它就停）。
- `the_reader_crosses_await`：编译期断言 `HelloReader` 与 `Hello` 是 `Send`。
- **分类**：十三条现有用例原样通过（不要求先红；P14）。

**实现**：照 P4。

**判据**

- 上面的用例都绿；十三条现有用例原样通过（这是分类没有变的证据，设计第 4.3 节）。
- `read_first` 对 `header` 交回 `None` 的数据报不分配、不导出密钥（同今天）。
- `quinn` 那一条单独循环跑二十次以上，全过。
- 门禁绿。

---

## Task 3：持住、SNI、计数与备注

**改哪里**：

- `crates/rurge-engine/src/udp.rs`：P5 ～ P7；测试模块里的单元用例；P14 点名的两条用例只改参数。
- `crates/rurge-engine/src/serve.rs`、`engine.rs`、`lib.rs`：P8（`count_blocked` 留给任务 4）。
- `crates/rurge-engine/tests/common/mod.rs`：`quic_initial()`（P14）。
- `crates/rurge-engine/tests/udp.rs`：端到端与 `udp_sink()`（P12）。
- `crates/rurge-engine/tests/sniff.rs`：`HttpCounts` 字面量补 `quic`（P14）。

**接口**

- 消费：任务 2 的 `read_first`、`HelloReader::read`、`Hello`；`rurge_http::tls::parse_client_hello`；任务 1 的构造器（只在用例里）。
- 产出：

```rust
// rurge-engine::serve（P8）
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QuicSniffed { pub hello: u64, pub incomplete: u64, pub unreadable: u64, pub blocked: u64 }
pub(crate) enum HelloRead { Read, Incomplete, Unreadable }
#[derive(Default)]
pub(crate) struct QuicCounts { /* 四个 AtomicU64 */ }
impl QuicCounts {
    pub(crate) fn count(&self, read: HelloRead);
    pub(crate) fn snapshot(&self) -> QuicSniffed;
}
pub struct HttpCounts { /* 已有的四个 */ pub quic: QuicSniffed }

// rurge-engine::engine
pub(crate) quic_counts: crate::serve::QuicCounts,

// rurge-engine::lib
pub use serve::{HttpCounts, QuicSniffed, SniffedCounts};

// rurge-engine::udp（私有，P5、P7）
const HELLO_WAIT: Duration = Duration::from_millis(300);
const HELLO_DATAGRAMS: usize = 16;
enum Held { Whole(Vec<u8>), Incomplete, GaveUp(&'static str), Ended }
async fn hold(
    association: &Association,
    reader: HelloReader,
    held: &mut VecDeque<Queued>,
    waiting: &mut mpsc::Receiver<Queued>,
) -> Held;
async fn forward(
    association: &Association,
    flow: &Flow,
    carrier: &Carrier,
    send_to: &Target,
    held: VecDeque<Queued>,
    waiting: &mut mpsc::Receiver<Queued>,
) -> SessionOutcome;
```

**用例（先红）**

单元用例在 `udp.rs` 的测试模块里，都是 `#[tokio::test(start_paused = true)]`。关联照 `stuck()` 建；读者是 `read_first(&d1)` 交回的 `Partial`；`d1`、`d2` 是 `hello_initials(RFC_DCID, &big_client_hello(Some("held.test")), 600)`；`hold` 的 future 用 `tokio::pin!` 固定，再用 `tokio::time::timeout(.., &mut fut)` 推。

- `holding_lets_go_once_the_hello_is_whole`：队列里放 d2 → `Held::Whole(m)`，`m` 等于那个 ClientHello；`held` 依次是 d1、d2。
- `holding_lets_go_at_300_ms`：队列里什么也不放。`timeout(299 ms, &mut fut)` 答 `Err`（还持着）；再给 2 毫秒答 `Ok(Held::Incomplete)`；`held.len() == 1`。
- `holding_lets_go_at_16_datagrams`：队列里放 16 份 d1（相同的字节，没有进展）→ `Held::Incomplete`，暂停的时钟没有走到 `HELLO_WAIT`；`held.len() == 16`（第一个加 15 个）；`waiting.try_recv()` 还拿得到 1 个。
- `holding_ends_with_the_association`：两例。
  - `association.ended.cancel()` → `Held::Ended`；
  - 丢掉队列的发送端 → `Held::Ended`。
- `holding_gives_up_with_the_reader`：第二个数据报带 `ConnectionClose` → `Held::GaveUp("the client closed the connection")`，`held.len() == 2`。

端到端在 `tests/udp.rs`（P12）。`d1`、`d2` 同上，目标是 `127.0.0.1:<udp_sink 的端口>`，`hosts` 写 `held.test = 127.0.0.1`。

- `a_hello_across_two_datagrams_is_routed_by_its_sni`：规则 `DOMAIN,held.test,REJECT,extended-matching`；经 `own_association` 连着写 d1、d2（中间不等）。
  - 记录结束：状态 `Rejected("REJECT")`、`sni == Some("held.test")`、协议 `QUIC`、`notes` 为空；
  - 客户端被告知两次（`refused(2)`：两个持着的数据报各一次）；
  - `http_counts().quic == QuicSniffed { hello: 1, ..Default::default() }`；
  - 收数据报的服务器什么也没收到（被拒绝的流从不发，不用等）。
- `without_extended_matching_the_sni_does_not_match`：规则 `DOMAIN,held.test,REJECT`（不带 `extended-matching`）；经 `udp_associate` 发 d1、d2。
  - 收数据报的服务器依次收到 d1、d2，逐字节相同；
  - 记录（等到 `up > 0`）的 `sni == Some("held.test")`，状态不是拒绝；`quic.hello == 1`。
- `a_hello_that_never_completes_goes_out_after_300_ms`：只发 d1，发之前记下时刻。
  - 收数据报的服务器收到 d1，收到的时刻减去发的时刻至少 300 毫秒（5 秒为限）；
  - 记录（等到 `up > 0`）的 `sni == None`、`notes == ["No SNI: the QUIC ClientHello did not arrive whole"]`；
  - `quic.incomplete == 1`、`quic.hello == 0`。
- `an_unreadable_hello_is_noted`：同一个关联上两条流，两个收数据报的服务器。
  - 一条发 d1 与一个带 `ConnectionClose` 的第二个数据报；另一条发 `hello_initial(RFC_DCID, &[1, 0, 0, 4, 0xff, 0xff, 0xff, 0xff])`（拼得齐，`parse_client_hello` 读不出）。
  - 两条记录的 `notes == ["No SNI: the QUIC ClientHello could not be read"]`、`sni == None`；`quic.unreadable == 2`；第一个服务器依次收到那两个数据报。
  - 用 `LogCapture` 断言 DEBUG 里有"the QUIC ClientHello could not be read"那一行，带着 "the client closed the connection" 与 "a ClientHello that does not parse"。
- `a_flow_whose_association_ends_while_held_leaves_no_record`：经 `udp_associate` 先发 d1（目标 A），再发一个普通的数据报（目标 B，另一个收数据报的服务器）；等 B 的流有了策略（`routed` 的写法），丢掉这个关联。
  - 固定等 600 毫秒（`HELLO_WAIT` 加一倍余量：断言"A 什么也没留下"，注释写明）；
  - 进行中与结束的记录里都没有 A 的目标；`quic` 四个数都是 0。
- `the_rfc_sample_flow_has_its_sni`：`hosts` 加 `example.com = 127.0.0.1`；`unhex(RFC9001_CLIENT_INITIAL)` 发往收数据报的服务器 → 记录（等到 `up > 0`）的 `sni == Some("example.com")`。

**实现**：照 P5 ～ P8。

**判据**

- 上面的用例都绿；P14 点名的现有用例照表改、其余照过。
- 端到端单独循环跑二十次以上，全过。
- 门禁绿。

---

## Task 4：`auto-quic-block`

**改哪里**：

- `crates/rurge-engine/src/serve.rs`：`sniffable_target`、`sniffable_session` 改由它定、`auto_quic_blocks`、`QuicCounts::count_blocked`（P8、P9）。
- `crates/rurge-engine/src/udp.rs`：`open` 的新参数与阻断（P10）；`run_flow` 传参数。
- `crates/rurge-engine/tests/mitm.rs`：经 SOCKS5 的端到端；`crates/rurge-engine/tests/tun.rs`：经虚拟网卡的一条（P12）。

**接口**

- 消费：任务 3 的 `HelloRead`、`QuicCounts`；`decrypting`、`named_at_a_proxy_port`、`raw_listed`、`widened_by_mitm`。
- 产出：

```rust
// rurge-engine::serve（P9）
fn sniffable_target<'s>(&self, session: &'s SessionInfo, runtime: &Runtime) -> Option<Cow<'s, SessionInfo>>;
pub(crate) fn auto_quic_blocks(&self, session: &SessionInfo) -> bool;
impl QuicCounts { pub(crate) fn count_blocked(&self); }

// rurge-engine::udp（P10）
const AUTO_QUIC_BLOCKED: &str = "QUIC blocked by auto-quic-block";
async fn open(
    engine: &Engine,
    association: &Association,
    handle: &Arc<SessionHandle>,
    destination: &Target,
    asked: &Target,
    hello_read: bool,
) -> Result<(Arc<Flow>, Arc<Carrier>, Target), SessionOutcome>;
```

**用例（先红）**

经 SOCKS5 的在 `tests/mitm.rs`。缺省的 Profile：

- `sections` 是 `mitm_section("origin.test", &ca.ca_p12_base64("pw"))`（`ca = mitm_ca()`），`hosts` 是 `origin.test = 127.0.0.1`，没有额外的规则（`FINAL,DIRECT`）；
- 引擎用 `harness_mitm(p, ca.roots())`；
- 数据报是 `hello_initial(RFC_DCID, &client_hello(Some("origin.test"), &[b"h3"]))`，经 `udp_associate` 发往 `origin.test:443`。

"被阻断"与"不阻断"照 P12 断言。各例只写与缺省不同的地方：

- `auto_quic_block_blocks_quic_to_a_listed_host`：被阻断——状态 `Rejected("REJECT")`、说明 `QUIC blocked by auto-quic-block`、`sni == Some("origin.test")`、策略链不空；`quic.blocked == 1`、`quic.hello == 1`。
- 不阻断的八例：
  - `auto_quic_block_off_lets_quic_through`：`[MITM]` 节多一行 `auto-quic-block = false`；
  - `with_mitm_off_quic_goes_through`：`h.engine.set_mitm(false).await` 之后发；
  - `without_a_usable_ca_quic_goes_through`：`[MITM]` 节只有 `hostname = origin.test`；
  - `a_source_mitm_does_not_take_lets_quic_through`：`[MITM]` 节多一行 `client-source-address = 10.0.0.0/8`（照这个文件里的现有写法）；
  - `a_negated_entry_lets_quic_through`：`hostname = *.test, -origin.test`；
  - `a_raw_tcp_host_is_not_blocked`：`general` 是 `always-raw-tcp-hosts = origin.test`；
  - `a_bare_entry_does_not_block_quic_on_8443`：发往 `origin.test:8443`；
  - `an_incomplete_hello_is_not_blocked`：只发 `hello_initials(RFC_DCID, &big_client_hello(Some("origin.test")), 600)` 的第一个——记录还多 `incomplete` 那一句备注（等到 `up > 0`，最多约 300 毫秒之后）。
- `a_port_entry_blocks_only_its_port`：`hostname = origin.test:8443`；发往 8443 的被阻断，发往 443 的不阻断（同一个引擎、两条流）。
- `always_allow_does_not_stop_auto_quic_block`：`general` 是 `block-quic = always-allow` → 被阻断。
- `with_block_quic_too_the_record_names_auto_quic_block`：`general` 是 `block-quic = all` → 被阻断，说明是 `QUIC blocked by auto-quic-block`（它先判，设计第 6.3 节）。
- `a_reject_rule_comes_before_auto_quic_block`：规则 `DOMAIN,origin.test,REJECT` → 状态 `Rejected("REJECT")`，说明不是 `QUIC blocked by auto-quic-block`；`quic.blocked == 0`、`quic.hello == 1`。
- `a_hello_without_sni_is_judged_by_its_target`：数据报换成 `client_hello(None, &[b"h3"])` 的；发往 443 的被阻断（`sni == None`），发往 8443 的不阻断（设计第 6.4 节）。

经虚拟网卡的在 `tests/tun.rs`：

- `auto_quic_block_answers_each_held_datagram_with_icmp`：
  - 规则 `guarded("DOMAIN,origin.test,DIRECT")`，`hosts` 与 `[MITM]` 节同上（节在用例里照 `mitm_section` 的样子写，CA 是 `TlsFixture::with_ca_name("rurge test MITM CA", &[])`），`Rig::on(harness_trusting(p, ca.roots()).await)`；
  - 照 `a_blocked_quic_flow_answers_with_icmp`：`divert_icmp`、`fakes("origin.test")`，把 `hello_initials(RFC_DCID, &big_client_hello(Some("origin.test")), 600)` 的两个数据报一前一后注入到 `(fake, 443)`；
  - 断言：连着两次 `prohibited_from(&mut icmp, fake)`（每个持着的数据报一个 ICMP"管理上禁止"）；记录的说明是 `QUIC blocked by auto-quic-block`；`quic.blocked == 1`。

**实现**：照 P8 ～ P10。

**判据**

- 上面的用例都绿；`tests/sniff.rs`、`tests/mitm.rs`、`tests/http.rs` 的 TCP 嗅探与解密的现有用例照过（`sniffable_session` 的行为不变）。
- 端到端单独循环跑二十次以上，全过。
- 门禁绿。

---

## Task 5：`/v1/rurge/http` 的 `quic`

**改哪里**：`crates/rurge-api/src/routes/rurge_http.rs`（P11；文件末尾加测试模块）；`crates/rurge-api/tests/api.rs` 的 `rurge_http_counts_start_at_zero`（P14）。

**接口**

- 消费：`rurge_engine::{HttpCounts, QuicSniffed}`。
- 产出：`QuicJson { hello, incomplete, unreadable, blocked }`、`impl From<QuicSniffed> for QuicJson`、`HttpJson` 在 `rewrite` 之后多 `quic: QuicJson`。

**用例（先红）**

- `the_quic_counts_go_out_under_their_names`：`serde_json::to_string(&QuicJson::from(QuicSniffed { hello: 1, incomplete: 2, unreadable: 3, blocked: 4 }))` 恰好是 `{"hello":1,"incomplete":2,"unreadable":3,"blocked":4}`。
- `rurge_http_counts_start_at_zero`（P14，要改）：期望的 JSON 与逐字节的串在 `"rewrite":{…}` 之后加 `"quic":{"hello":0,"incomplete":0,"unreadable":0,"blocked":0}`。

**实现**：照 P11。

**判据**：两条用例绿；门禁绿。

---

## Task 6a：写行为的文档

**改哪里**：`docs/api/phase4.md`、`docs/acceptance/phase4-manual.md`、`docs/testing.md`、`docs/surge-compatibility-matrix.md`，内容照 P13 ①～④。

**判据**

- 四份文档照 P13 改完；`docs/api/phase4.md` 与手工清单里每一个说法都出自设计、本计划或任务 1 ～ 5 的代码与用例。
- 中文、引号照各文件自己的（本计划与设计只用直引号；`docs/testing.md`、兼容性清单用弯引号，照文件里已有的写法）；行尾照旧（`git ls-files --eol` 看过：`docs/testing.md`、兼容性清单与 `docs/api/phase4.md` 在工作区是 CRLF，手工清单是 LF）。
- 带本机账户名的文件数照旧 26；`git diff --check` 干净。

---

## Task 6b：写进度的文档

**改哪里**：`docs/status.md`、`README.md`、`README_en.md`、`docs/requirements.md`、`CLAUDE.md`、设计第 16 节，内容照 P13 ⑤。

**判据**

- 各文档照 P13 ⑤ 改完；行尾照旧（`CLAUDE.md`、`docs/status.md`、两份 README、`docs/requirements.md` 在工作区是 CRLF，设计是 LF）。
- 带本机账户名的文件数照旧 26；`git diff --check` 干净。

---

## 计划期决定

写计划时对着 `2ca12b2` 核对过下面引的名字与行号。行号会随前面任务的改动漂移，以名字为准；名字不同就照实改、记进修正记录，不改语义。标了"订正设计"的，已随本计划写进设计第 16 节。

| # | 事项 | 决定 |
| - | ---- | ---- |
| P1 | 任务的拆法（订正设计，第 16 节第 1 条） | 设计第 2 节的 M5a 任务 0 ～ 6 拆成 0 ～ 5、6a、6b：0 核对；1 构造器；2 读 ClientHello；3 持住、SNI、计数与备注；4 `auto-quic-block`；5 API；6a 写行为的文档；6b 写进度的文档。`open` 的新参数与 `count_blocked` 随任务 4 加：任务 3 不留没人用的参数与函数（`-D warnings`） |
| P2 | 依赖 | 不改任何 `Cargo.toml`：引擎与 API 的 dev-dependencies 已开着 `rurge-net/testing`，`quic::testing` 用的 rustls、quinn 是 `rurge-net` 的正式依赖。锁文件一个字不变（520 个包）；`unsafe_functions.rs` 的 `LISTED`（23 项）、`sockets.rs` 的 `LISTED`（26 行）、带本机账户名的文件数（26）都照旧 |
| P3 | `quic::testing` 的构造器（任务 1） | ① **`Frame`**：`Padding(n)` 写 n 个零字节；`Ping` 写 `0x01`；`Ack` 在 `ecn` 是 `None` 时写类型 `0x02`，否则 `0x03` 并在末尾写三个计数，`ranges` 每项是（间隙, 长度），RFC 9000 第 19.3 节；`Crypto` 写 `0x06`、偏移、长度、数据；`ConnectionClose` 写 `0x1c`、错误码、帧类型、原因的长度与原因；`Raw` 原样写出（Initial 不许有的帧、截断的帧）。变长整数都用最短的写法（一个私有的函数）。<br>② **`Initial`**：`new` 让 `keys_from` 等于 `dcid`、`number_len` 是装得下 `number` 的最短长度（1 ～ 4）。`seal(size)` 照 `client_initial_with_quic_bit` 的做法（那个函数不改）：<br>• 长包头 `0x80 \| 0x40 \| 这个版本的 Initial 类型 << 4 \| (number_len - 1)`、版本、目的连接 ID、空的源连接 ID、空的 Token、两个字节的 Length（`0x4000 \| n`）、包号的低 `number_len` 个字节；<br>• 明文是各帧的写法，不够 `4 - number_len` 个字节时在末尾补 PADDING（包头保护的样本总在包号起点之后第 4 个字节，RFC 9001 第 5.4.2 节）；<br>• `size` 是 `Some(n)` 时在明文末尾补 PADDING，让整个包恰好 n 个字节，装不下就 panic（信息写差多少）；<br>• 用 `initial_suite().keys(keys_from, Side::Client, 版本)` 的 `local` 以 `number` 封包、加包头保护。<br>③ **`long_packet(version, kind, dcid, payload)`**：类型位是 `kind`（调用方按版本的编号给，v1 的 0-RTT 是 `0b01`）、空的源连接 ID、**没有 Token 一项**、两个字节的 Length 等于 `payload`、之后 `payload` 个 `0x5a`；不封、不加保护：读的一侧按 Length 跳过它。<br>④ **`datagram(packets, pad_to)`**：依次接起来，短于 `pad_to` 时在末尾补零。<br>⑤ **`client_hello(server_name, alpn)`**：`rustls::quic::ClientConnection::new_with_alpn(config, Version::V1, name, params, alpn)` 之后 `write_hs` 写出的字节，恰好一个握手消息。`config` 照 `TestCert::client_tls` 的提供者与版本（ring、只有 TLS 1.3），根可以是空的；`name` 是 `Some(n)` 时 `ServerName::try_from(n)`，`None` 时 `ServerName::IpAddress(127.0.0.1)`；`params` 是一串写死的传输参数（例如只有 `initial_max_data` 一项，照 RFC 9000 第 18 节编码）。<br>⑥ **`big_client_hello(server_name)`**：同 ⑤，ALPN 是一串生成的名字（`rurge-test-alpn-00` 起），长度落在 1500 ～ 1900 字节：一个 1200 字节的数据报装不下（设计第 4.2 节），上限让 `chrome_frames` 的两个包都装得下。<br>⑦ **`hello_initial(dcid, hello)`**：v1、包号 0、一个 `Crypto { offset: 0, data: hello }`，封到 1200 字节。**`hello_initials(dcid, hello, at)`**：两个数据报，包号 0 与 1，第一个带 `[0, at)`、第二个带 `[at, len)`，各封到 1200 字节。<br>⑧ **随机**：一个私有的 SplitMix64（从 `seed` 起），不加包。<br>• **`chaos_frames(pieces, seed)`**（QUICHE `quic_chaos_protector.cc` 的做法，只取事实，设计第 4.2 节）：每段在随机的位置切开，CRYPTO 帧比段多 2 ～ 10 个（段短到切不开的照实少切）；加 2 ～ 10 个 `Ping`；在 1 ～ 4 个随机的位置插 1 ～ 8 字节的 `Padding`；全部帧随机排序。<br>• **`chrome_frames(hello, seed)`**（`quic_packet_creator.cc` 的 `MultiPacketChaosProtect`，第 4.2 节）：`k` 取 55 ～ 86；`e` 取 `(len - k).saturating_sub(1000)` 再加 0 ～ 64（中间那段不为空）；第一个包是 `chaos_frames(&[(0, &hello[..k]), (len - e, &hello[len - e..])], seed)`，第二个包是 `chaos_frames(&[(k, &hello[k..len - e])], seed ^ 1)`。<br>⑨ **样例**：`RFC9001_CLIENT_INITIAL`、`RFC9369_CLIENT_INITIAL`、`unhex`（连同它用的 `hex_digit`，私有）从 `initial.rs` 的测试模块原样挪来、改成 `pub`，前面的 SHA-256 注释一起挪 |
| P4 | 读 ClientHello（任务 2） | ① **入口**：`read_first(datagram)`：<br>• `header(datagram)?`：今天的便宜判据，不变，不分配、不导出密钥；<br>• 用它的目的连接 ID 与版本导出 `initial_suite().keys(dcid, Side::Server, version)`，留 `remote`（客户端一侧的 `DirectionalKeys`）；<br>• 第一个包用 ② 的 `open` 打开，打不开答 `None`（与今天 `opens` 答 `false` 是同一件事）；<br>• 打得开就建读者（线上的版本四字节、`Version`、密钥、`largest = Some(这个包的包号)`、空的段），读这个包的帧（④），再从这个包的末尾（`pn_offset + length`）起走完数据报的其余部分（③），按 ⑥ 答 `Some(..)`。<br>`HelloReader::read(self, datagram)`：从数据报的开头起走（③，第一个包不另判），按 ⑥ 答。<br>② **`open`**：`fn open(packet: &[u8], pn_offset: usize, keys: &DirectionalKeys, largest: Option<u64>) -> Option<(u64, Vec<u8>)>`（名字计划定）。第一个包与后来的 Initial 共用，`opens` 由它取代。<br>• `packet` 是从包的首字节到它的 Length 末尾；Length 小于 `MIN_LENGTH`（20）答 `None`（取不到样本，当作打不开）；<br>• 复制一份、去掉包头保护（样本在 `pn_offset + 4`，同今天），得到 `number_len` 与截断的包号；<br>• 照 RFC 9000 附录 A.3 还原：`fn packet_number(largest: Option<u64>, truncated: u64, len: usize) -> u64`，`largest` 为 `None` 时期望 0；算术不溢出（`expected - hwin` 为负的那一支不成立）；<br>• 解密，答（包号, 明文）。<br>③ **走一个数据报**（设计第 4.4 节的 1 ～ 6 步）：从 `at` 起，直到末尾：<br>• 首字节的 `0x80` 是 0 → 停；<br>• 版本（线上的四字节）不是第一个包的 → 停；<br>• 类型是这个版本的 Retry（v1 `0b11`、v2 `0b00`）→ 停；<br>• 读目的连接 ID 的长度（≤ 20）与它、源连接 ID 的长度（≤ 20）与它、**只有 Initial 有的** Token 的长度与 Token、Length：任一项超出数据报、或长度超过 20 → 停；<br>• 不是 Initial（0-RTT、Handshake）→ 按 Length 跳过；<br>• 是 Initial → `open`：打不开就按 Length 跳过；打得开就让 `largest` 取大的那个，读它的帧（④）。<br>"停"都不是读不成。后来的数据报不要求 1200 字节，目的连接 ID 不与第一个比（M5-D4）。<br>④ **帧**：类型读一个字节。<br>• `0x00` 跳过一个字节；`0x01` 跳过；<br>• `0x02` / `0x03` 读最大确认、延迟、段数、第一段，再逐个读段数那么多对（间隙, 长度）——不按段数预先分配，读到明文末尾还没读完就是截断——`0x03` 再读三个计数；<br>• `0x06` 读偏移、长度、数据，交给 ⑤；<br>• `0x1c` → 读不成 "the client closed the connection"；<br>• 别的 → 读不成 "a frame an Initial may not carry"；<br>• 任何一项在明文末尾之前截断 → 读不成 "a frame cut short"。<br>⑤ **`Pieces`**（私有）：按起点排好、互不重叠、互不相接的几段 `Vec<(u64, Vec<u8>)>`。加一段 `(offset, data)`：<br>• `offset + len` 溢出或超过 `MAX_HELLO` → 读不成 "CRYPTO data beyond 64 KiB"；<br>• 长度为 0 的什么也不做；<br>• 与已有的段重叠的部分逐字节比，不同 → 读不成 "CRYPTO data that changed when it came again"；<br>• 与重叠或相接的段合成一段；合并之后多于 32 段 → 读不成 "CRYPTO data in more than 32 pieces"。<br>每加一段之后看头（第一段起点是 0、至少 4 个字节时）：第一个字节不是 1 → 读不成 "the first handshake message is no ClientHello"；4 加 24 位的长度超过 `MAX_HELLO` → 读不成 "a ClientHello longer than 64 KiB"；第一段有至少 4 加长度个字节 → 拼齐了（记下，读完这个数据报再交）。<br>⑥ **一个数据报的答复**：读不成的那一刻就停，答 `GaveUp(原因)`；读完了、拼齐过 → `Whole(第一段的前 4 + 长度个字节)`；否则 `Partial(读者)`。即同一个数据报里读不成优先于拼齐，拼齐优先于还缺。<br>⑦ **`HelloReader`**：私有字段；`Send`（引擎持着它跨过 `await`）；`Debug` 手写，只写版本、`largest`、段数与字节数，不写密钥。`Hello` 派生 `Debug`。<br>⑧ **`is_client_initial`** 改成 `read_first(datagram).is_some()`；文档注释照旧说判据，加一句"the frames are read by `read_first`"；模块注释改写最后一段 |
| P5 | 持住（任务 3） | `HELLO_WAIT`（300 毫秒）与 `HELLO_DATAGRAMS`（16）是 `udp.rs` 的常数，文档注释写设计第 5.2 节的理由；不设旋钮（M5-D7）。`hold`：<br>• 进来时 `held` 里只有第一个数据报；期限是进来那一刻（`tokio::time::Instant::now()`）加 `HELLO_WAIT`。`run_flow` 读完第一个数据报就调它，所以就是"从第一个数据报起"。<br>• 循环里 `select!` 三样：`association.ended` 取消 → `Ended`；期限到（`sleep_until`）→ `Incomplete`；`waiting.recv()`：`None` → `Ended`；`Some(q)` → `reader.read(&q.datagram)`，`q` 推进 `held` 的末尾，`Whole` / `GaveUp` 照答，`Partial` 时 `held.len() >= HELLO_DATAGRAMS` → `Incomplete`、否则接着等。<br>• 读完第 16 个还没有拼齐才算凑不齐；第 17 个起照旧排在 `waiting` 里。<br>• 持着的数据报都是 `Queued`，照旧记在关联的 `QUEUED_BYTES` 里（设计第 4.5 节末段） |
| P6 | `run_flow`（任务 3） | ① `turn_back` 与 `destination` 之后：`let mut held = VecDeque::from([first]);`，再 `read_first(&held[0].datagram)`：<br>• `None` → 照旧判 STUN 与 UDP；<br>• `Some(Whole(m))` → `Held::Whole(m)`；`Some(GaveUp(w))` → `Held::GaveUp(w)`；`Some(Partial(r))` → `hold(..).await`。协议都是 `QUIC`。<br>② `Held::Ended`：`let _ = association.done.send((asked, id)); return;`——不建句柄、不留记录、不计数（设计第 5.1 节）；`turned` 里的租约随之放掉。<br>③ `Held::Whole(m)` → `rurge_http::tls::parse_client_hello(&m)`：`Some(h)` 时 `session.sni = h.sni`，读成；`None` 时读不成，原因 "a ClientHello that does not parse"（只进 DEBUG）。`Incomplete` → 凑不齐；`GaveUp(w)` → 读不成，原因 `w`。<br>④ 然后才 `new_handle_with`（会话之后不再变）；租约照旧。对 QUIC 流：`engine.quic_counts.count(..)` 恰好一次；凑不齐与读不成各 `add_note` 那一句、写一行 DEBUG（「表后的原文」）；读成的不写备注（M5-D10）。<br>⑤ `Turned::Unknown` 的流同样读、同样计数：它照旧失败，记录多了 SNI（设计第 5.1 节） |
| P7 | `forward` 与拒绝（任务 3） | `forward` 的 `first: Queued` 换成 `held: VecDeque<Queued>`：先按次序发完 `held`（每个照今天发一个数据报的那一段：超时、`add_up`、`touch`、`lost_alone`），再像今天一样收 `waiting`。`Err` 那一支：`refused` 是 `Some((target, kind))` 时对 `held` 里的每个数据报各调一次 `association.client.refused(target, kind)`（今天对 `first` 调一次），之后 `drop(held)`；`Failed` 的照旧什么也不告诉客户端；`drain` 不变；`RETRY_AFTER` 不变。注释照今天那一段改 |
| P8 | 计数（任务 3、4） | **`serve.rs`**：`QuicSniffed`、`HelloRead`、`QuicCounts` 照「任务 3 的接口」；`QuicCounts` 与 `SniffCounts` 并排，计数用 `Relaxed`（同它）。<br>**`HttpCounts`**：末尾多 `pub quic: QuicSniffed`，文档注释说它是读到的 QUIC ClientHello 与 `auto-quic-block` 阻断的流（`phase 4 M5 design 5.4`）。<br>**`Engine`**：多字段 `pub(crate) quic_counts: crate::serve::QuicCounts`，紧跟 `sniff_counts`，注释同它（重载不清）；初始化为缺省；`http_counts` 填 `quic: self.quic_counts.snapshot()`。<br>**`lib.rs`**：`pub use serve::{HttpCounts, QuicSniffed, SniffedCounts};`。<br>`count_blocked` 在任务 4 加 |
| P9 | `sniffable_target` 与 `auto_quic_blocks`（任务 4） | ① **`sniffable_target`**：`sniffable_session` 里入口（`Http`、`Socks5`、`Tun`）、`named_at_a_proxy_port`、`raw_listed`、端口（80、443、`force-http-engine-hosts`、`widened_by_mitm(session, runtime.mitm.as_deref())`）这几步原样搬过来，注释跟着走；交回 `probed`。不看传输：UDP 流也问它。<br>② **`sniffable_session`** 改成：传输是 TCP → 引擎没有暂停 → `let runtime = self.runtime();` → `let Some(probed) = self.sniffable_target(session, &runtime) else { return false };` → 预匹配的探测照旧（`at_a_proxy_port && self.reaches_the_rules() && runtime.rules.pre_refuses(&probed)`）。先后只有"入口"挪到了"暂停"之后：两者都只读、都答 `false`，结果不变。文档注释说明抽出的几步与 `auto_quic_blocks` 共用。<br>③ **`auto_quic_blocks(session)`**：`let runtime = self.runtime();`，`runtime.config.mitm.auto_quic_block && self.sniffable_target(session, &runtime).is_some() && self.decrypting(session, &ClientHello { sni: session.sni.clone(), alpn: Vec::new() }).is_ok()`。文档注释写设计第 6.1 节的条件、"同一段代码，两侧的条件不会分叉"与 ALPN 为空的理由（QUIC 的 ALPN 是 `h3`，回落之后客户端给 `h2` 与 `http/1.1`，那时由 TCP 一侧照常判断）。暂停与预匹配不问（设计第 6.1 节） |
| P10 | `open` 里的位置（任务 4） | `open` 多最后一个参数 `hello_read: bool`（这条流的 ClientHello 读成了没有），`run_flow` 给 `HelloRead::Read` 时的 `true`。在 `if resolution.terminal == TerminalKind::Reject { … }` 之后、`let direct = …` 之前：<br>`if hello_read && engine.auto_quic_blocks(handle.session()) { handle.set_error(AUTO_QUIC_BLOCKED); engine.quic_counts.count_blocked(); return Err(SessionOutcome::Rejected(RejectKind::Reject)); }`<br>注释写：规则与策略照旧记在记录上（设计第 6.2 节）；不碰自适应的表、`always-allow` 不放过它（M5-D13）。`block-quic` 那一段不改 |
| P11 | API（任务 5） | `QuicJson` 照这个文件里 `SniffedJson` 的派生与写法（`#[derive(Serialize)]`，四个 `u64`）；`impl From<QuicSniffed> for QuicJson`；`HttpJson` 在 `rewrite` 之后多 `quic: QuicJson`，`http()` 用 `From` 填它。这个文件今天没有测试模块：加在文件末尾 |
| P12 | 端到端的做法（任务 3、4；订正设计，第 16 节第 2 条） | • 任务 3 的端到端在 `tests/udp.rs`：`own_association` 数得出每个数据报的拒绝。收数据报的服务器是新的本地辅助 `udp_sink()`：回环上绑 0 号、64 KiB 的缓冲，把收到的每个数据报连同收到的时刻（`tokio::time::Instant`）送进一个无界的通道，不答。<br>• 任务 4 经 SOCKS5 的在 `tests/mitm.rs`（`mitm_section`、`mitm_ca`、`harness_mitm` 在那里），数据报经 `common` 的 `udp_associate` 发；经虚拟网卡的一条在 `tests/tun.rs`（`prohibited_from` 在那里）。<br>• 目标：任务 3 多是 `127.0.0.1:<udp_sink 的端口>`（地址，不查名字），SNI 是另一个名字（`held.test`），这样"按 SNI 命中"与"按目标命中"分得开；任务 4 是钉到 `127.0.0.1` 的 `origin.test` 的 443 或 8443。<br>• **被阻断的流**：等它的记录结束（拒绝的流在 `drain` 之前就 `finish`），断言状态 `Rejected("REJECT")`、说明、SNI、策略链不空、`quic.blocked`。<br>• **放行的流**：等到它的记录（进行中或结束的）`up > 0`——第一个数据报发出去了，说明 `open` 已经走过阻断的判断——再断言说明不是 `QUIC blocked by auto-quic-block`、状态不是拒绝、`quic.blocked == 0`。不断言说明为空：发往没有人收的回环端口，Windows 上可能写上一个"连接被重置"一类的说明。<br>• 读计数用 `h.engine.http_counts().quic`（经虚拟网卡的用 `rig.h`） |
| P13 | 收尾文档（任务 6a、6b） | ① **`docs/api/phase4.md`**（6a）：新的一节「M5a　QUIC 里的 SNI 与 `auto-quic-block`」，放在 M4b 一节之后。<br>• `/v1/rurge/http` 的 `quic` 与四个数（设计第 5.4 节）；两种备注、一种说明与 DEBUG 的两行（「表后的原文」）；<br>• `/v1/requests/*` 的形状不变：QUIC 流的 `sni` 有了值，`notes` 可能多一句；<br>• `auto-quic-block` 的条件（第 6.1 节）、位置与 `block-quic` 的关系（第 6.2、6.3 节）、没有 SNI 时（第 6.4 节）；持住的两个上限（第 5.2 节）。<br>② **`docs/acceptance/phase4-manual.md`**（6a）：新的一节 M5，六项（W13、W14、M13、M14、L13、L14），开头的项数 62 → 68，"清单随里程碑逐节增加"与「怎么用」里的次序各补一句。<br>• 13 号：增强模式、装 CA，`[MITM] hostname` 写一个讲 HTTP/3 的站点；Chrome 与 Firefox 打开它——记录里那个站点的 QUIC 流被阻断（说明 `QUIC blocked by auto-quic-block`）、页面照样打开、之后的 TCP 会话被解密，`quic.blocked` 涨；做完卸 CA、关增强模式。<br>• 14 号：`auto-quic-block = false` 时 HTTP/3 照通，记录里 QUIC 流带 `sni`；浏览器自己解析（目标是地址）时 `[Rule]` 的 `DOMAIN,<站点>,REJECT,extended-matching` 按 SNI 拒绝它的 QUIC 流。<br>• 照这份清单已有的写法：操作、期望、怎么看，macOS 与 Linux 的照 Windows 的做、写不同的地方。<br>③ **`docs/testing.md`**（6a）：新的一节（阶段 4 / M5a）：QUIC 的用例只喂数据报、不完成握手；`quic::testing` 的构造器有哪些；持住的上限只在暂停时钟的单元用例里量，端到端只在只发第一个数据报的几例里等 300 毫秒；放行的 QUIC 流发往没有人收的回环端口，断言不看说明（P12）。<br>④ **`docs/surge-compatibility-matrix.md`**（6a）：L584、L249、L265、L177 四行（行号是 `a262d4c` 的，按内容找）。<br>• L584（`auto-quic-block`）：状态与备注——生效；跟着 TCP 一侧，`always-raw-tcp-hosts` 与不带端口的项的差异（第 6.1 节）；没有 SNI 按目标（第 6.4 节）；`always-allow` 不管它（第 6.3 节）；不完整的 ClientHello 不取 SNI、gQUIC 不认（第 4.7 节）。<br>• L249（`PROTOCOL`）、L265（`extended-matching`）加 QUIC 的 SNI；L177（全局的 `block-quic`）写 `always-allow` 不管 `auto-quic-block`。<br>• 开头"最近一次"的括号。<br>⑤ **进度的文档**（6b）：<br>• `docs/status.md`：阶段 4 下新的一节「M5a（读 QUIC 里的 ClientHello 与 `auto-quic-block`）」，照 `CLAUDE.md`「工作流约定」的「进度」那一条写：做了什么；没做的与不要当成已经有的（第 4.7 节的四件、真浏览器没有验过）；以后的会话必须知道的事（持住的两个常数与不设旋钮、`quic_initial()` 现在是完整的 ClientHello 而 `client_initial` 只有 PADDING 给分类的用例用、`sniffable_target` 两处共用、放行的 QUIC 流的说明可能有"连接被重置"）；去向写着项目所有者的事（设计第 15 节第 1 ～ 5 条、推送、手工验收）。<br>• `README.md` 与 `README_en.md`：状态与特性表（`auto-quic-block`、QUIC 的 SNI），两份一致。<br>• `docs/requirements.md` 第 7 节阶段 4 的状态。<br>• `CLAUDE.md` 只改「现在在哪里」：M5a 做完；之后是 M5b 的计划（不等推送）；M5 的提交都在本地、CI 没有见过。<br>• 设计第 16 节："执行 M5a 时"之下，执行期改了设计正文意思的地方一条一段（没有就写"没有"），每条指到修正记录的行 |
| P14 | 现有的用例 | 见下面的表。表里"预计照过"的若红，停下报告，不改期望 |

### 表后的原文（照抄）

每个代码块里的文字就是要的那一串，没有为 Markdown 加的转义。

**备注**（任务 3，`add_note`）

```text
No SNI: the QUIC ClientHello did not arrive whole
No SNI: the QUIC ClientHello could not be read
```

**说明**（任务 4，`set_error`；常数 `AUTO_QUIC_BLOCKED`）

```text
QUIC blocked by auto-quic-block
```

**DEBUG 的两行**（任务 3；`destination` 是这条流的目标，`why` 是读不成的原因）

```rust
tracing::debug!(host = %destination.host, port = destination.port, "udp: the QUIC ClientHello did not come whole within 300 ms or 16 datagrams; the rules see no SNI");
tracing::debug!(host = %destination.host, port = destination.port, why, "udp: the QUIC ClientHello could not be read; the rules see no SNI");
```

**读不成的原因**（任务 2 的八种，任务 3 的一种）

```text
the client closed the connection
a frame an Initial may not carry
a frame cut short
CRYPTO data beyond 64 KiB
CRYPTO data that changed when it came again
CRYPTO data in more than 32 pieces
the first handshake message is no ClientHello
a ClientHello longer than 64 KiB
a ClientHello that does not parse
```

最后一种是任务 3 的：拼齐了而 `parse_client_hello` 读不出。

**JSON**（任务 5，紧跟 `"rewrite":{…}` 之后）

```text
"quic":{"hello":0,"incomplete":0,"unreadable":0,"blocked":0}
```

### P14 的表：要改的、要留意的现有用例

| 用例 | 为什么 | 任务 | 预计 |
| ---- | ------ | ---- | ---- |
| `crates/rurge-net/src/quic/initial.rs` 的十三条现有用例（`the_rfc_9001_client_initial_is_one` ～ `no_bit_flip_in_the_first_64_bytes_makes_one`） | 样例与 `unhex` 挪到 `quic::testing`；`is_client_initial` 改由 `read_first` 定。它们原样通过就是分类没有变的证据（设计第 4.3 节） | 1、2 | 照过（只改 `use`） |
| `crates/rurge-net/src/quic/` 下别的用例（`dial.rs`、`h3/`、`socket.rs`）与用 `quic::testing` 的别的 crate（`rurge-dns`、`rurge-policy`、`rurge-proto-quic`）的用例 | `testing.rs` 只多了东西 | 1 | 照过 |
| `crates/rurge-http/src/tls.rs` 的用例 | 不改 | — | 照过 |
| `crates/rurge-engine/src/udp.rs` 的 `a_send_that_hangs_fails_the_flow_and_its_carrier`、`the_end_of_the_association_cuts_a_send_short` | `forward` 收持着的队列（P7） | 3 | 改：只把 `queued(..)` 换成 `VecDeque::from([queued(..)])`，期望不变 |
| 同一文件的其余用例（`every_cell_of_the_quic_verdict`、`told_of_three_drops`、`the_flows_waiting_on_an_opening_that_fails_fail_with_it`、`the_far_end_replaces_what_the_carriers_dial_noted` 等） | 第一个数据报不是 QUIC 时不持；`open` 多一个参数只影响它唯一的调用处 | 3、4 | 照过 |
| `crates/rurge-engine/tests/common/mod.rs` 的 `quic_initial()` | 只有 PADDING 的 Initial 会被持 300 毫秒、多一句备注（设计第 10 节第 3 条） | 3 | 改：`hello_initial(RFC_DCID, &client_hello(None, &[b"h3"]))`（一个数据报装得下、不带 SNI 的完整 ClientHello），文档注释跟着改 |
| `crates/rurge-engine/tests/udp.rs` 用 `quic_initial()` 的八条：`block_quic_drops_quic_and_nothing_else`、`per_policy_block_quic_follows_the_terminal_policy`、`auto_blocks_quic_through_a_proxy_that_carries_no_udp`、`adaptive_on_a_direct_alias_blocks_the_first_quic_flow`、`protocol_rules_see_udp_and_quic`、`quic_on_any_port_is_quic`、`block_quic_blocks_quic_on_any_port`、`a_refused_flow_tells_the_client_for_every_datagram` | 完整的 ClientHello 不持、不写备注、没有 SNI；它们的 Profile 没有 `[MITM]`，`auto-quic-block` 不阻断 | 3、4 | 照过 |
| 同一文件的 `a_lookalike_of_a_quic_initial_is_plain_udp` | `read_first` 答 `None` | 2、3 | 照过 |
| `crates/rurge-engine/tests/tun.rs` 的 `a_blocked_quic_flow_answers_with_icmp` | 同上面那八条 | 3、4 | 照过 |
| `crates/rurge-engine/tests/sniff.rs` 的 `HttpCounts` 字面量（第 958 行附近） | 多一个字段 | 3 | 改：补 `quic: QuicSniffed::default()`，期望不变 |
| `crates/rurge-engine/tests/sniff.rs`、`tests/mitm.rs`、`tests/http.rs` 的 TCP 嗅探与解密的现有用例 | `sniffable_session` 抽出 `sniffable_target`，行为不变（P9 ②） | 4 | 照过 |
| `crates/rurge-api/tests/api.rs` 的 `rurge_http_counts_start_at_zero` | 多 `quic` | 5 | 改：期望的 JSON 与逐字节的串在 `rewrite` 之后加「表后的原文」那一串 |
| `crates/rurge/tests/sockets.rs` 的扫描用例 | 不建套接字 | 全部 | 照过 |
| `crates/rurge-platform/tests/unsafe_functions.rs` | 不加放开的函数 | 全部 | 照过 |
| `crates/rurge-config/tests/secrets.rs` | 不加带凭据的写法 | 全部 | 照过 |

---

## 执行期修正记录

下面写"执行者的裁定"的，是 Pi 在设计与计划的原则下定的；写"协调者的裁定"的，是协调的会话代项目所有者定的。项目所有者都可以改。计划的正文不改：正文里说得不对的地方，以这张表的行为准。

| # | 任务 | 内容 | 提交 |
| - | ---- | ---- | ---- |
| 1 | 0 | 开工前核对第 1、2 项：`git status --short` 空，分支 `phase4-m5`，`HEAD` `9c2b55b`，`git log --oneline 2ca12b2..HEAD` 只有 `9c2b55b` 一个提交；`git diff --stat a9ef85b HEAD -- crates tests Cargo.lock .github` 是空的，基线对着的就是这棵代码树。读了设计第 16 节（先）与第 3 ～ 7、10、11、13 ～ 15 节、本计划的计划期决定与表后的原文与 P14 的表，以及 `docs/status.md` 阶段 4 / M4b、M4a、M2a、M1a 四节的「以后的会话必须知道的事」与阶段 3 / M5 一节。只改文档。 | 本提交 |
| 2 | 0 | 名字与签名逐项对过：`initial.rs` 的 `MIN_DATAGRAM` 25、`SAMPLE` 29、`MIN_LENGTH` 30、`is_client_initial` 39、`Header` 49、`quic_version` 63、`initial_kind` 74、`initial_suite` 82、`varint` 91、`header` 107、`opens` 161，`quic/mod.rs` 的 `pub mod initial` 21 与 `#[cfg(any(test, feature = "testing"))] pub mod testing` 23 ～ 24，`quic/testing.rs` 的 `TestCert` 26、`new` 38、`client_tls` 77、`client_initial` 465、`client_initial_with_quic_bit` 472（文件里没有测试模块），以及 `udp.rs`、`serve.rs`、`engine.rs`、`runtime.rs`、`tls.rs`、`mitm/decrypt.rs`、`mitm.rs`、`matcher.rs`、`rurge_http.rs` 与用例的夹具：名字都在，签名与计划写的一致。**一处不同**：计划写 `initial.rs` 有十三条现有用例，代码是 **11 条**（226、231、238、256、273、284、309、323、339、393、404）；任务 1、2 的「原样通过」按 11 条算，其余照计划。 | 本提交 |
| 3 | 1、2 | rustls 0.23.43 的事实（任务 1、2 用的）：「技术栈」的九处行号都对得上（`rustls-0.23.43/src/quic.rs` 的 892、904、591、679、707、727、760、179、430，`Version` 在 980）。`HeaderProtectionKey::decrypt_in_place` 的实现在 `rustls-0.23.43/src/crypto/ring/quic.rs:20-70`：长包头掩去首字节的低 4 位，包号掩去（去保护后的首字节 `& 0x03`）加 1 个字节（1 ～ 4）；`PacketKey::decrypt_in_place` 答 `Result<&'a [u8], Error>`（明文切片），`tag_len()` 在 788。`new_with_alpn`（171 ～ 214）要求配置支持 TLS 1.3 与 QUIC 的套件，不然答 `Error::General("TLS 1.3 support is required for QUIC")` 或 `"at least one ciphersuite must support QUIC"`，不看 `enable_early_data`；`write_hs` 是 `pub fn write_hs(&mut self, buf: &mut Vec<u8>) -> Option<KeyChange>`（430），把握手字节追加进 `buf`。`ServerName::IpAddress` 不发 SNI（`src/client/hs.rs:260-278`）。 | 本提交 |
| 4 | 1、2、3 | tokio 1.53.1，`rurge-engine` 的开发依赖开着 `test-util`（`crates/rurge-engine/Cargo.toml:48`）；`udp.rs` 已有四处 `#[tokio::test(start_paused = true)]`（960、1369、1390、1500）；暂停的时钟之下 `tokio::time::Instant::now()` 是 `src/time/instant.rs:48` 与 `src/time/clock.rs:370-381`，没有可跑的任务时运行时把时钟推到最近的定时器（所以「299 毫秒还持着」要先 `sleep(299ms)` 把时钟推过去，不能靠真实等待）。**与计划写法的一处不同**：`udp.rs` 今天的 `Instant` 是 std 的（32 行），`now()`（117）是 `tokio::time::Instant::now().into_std()`，`drain` 睡的是 `sleep_until(end.into())`（855）；持住的期限照这条路写，不另起一口钟（语义不变）。quinn 0.11.12 / quinn-proto 0.11.19：客户端把装不下的 CRYPTO 帧切开、余下的推回队列（`quinn-proto-0.11.19/src/connection/mod.rs:3263-3301`），带 Initial 的数据报补到 `min_mtu`（`:790-793`、`:920-922`），所以两个 Initial 包在两个 1200 字节的数据报里。 | 本提交 |
| 5 | 3、4 | 内部的几件都对得上。`run_flow` 的 `session` 在 `new_handle_with` 之前是可改的本地值（503 ～ 528），`protocol` 在 521 设，`turn_back` 在 517；`Turned` 的三种是 `Unchanged`、`Name(lease)`、`Unknown`，租约在 `handle.on_finish(..)` 的回调里 drop（534 ～ 538）；`drain` 对后来的每个数据报各调一次 `refused`（846 ～ 851）；SOCKS5 的 `Socks5UdpClient` 不实现 `refused`（缺省什么也不做，`crates/rurge-inbound/src/session.rs:589`），虚拟网卡的 `refused` 回 ICMP「管理上禁止」、每 3 秒 100 个（`crates/rurge-tun/src/udp/client.rs:139`、`crates/rurge-tun/src/icmp.rs:31-32`，`Drop` 不回、`NoDrop` 不限频）；`open` 里 `REJECT` 在 638、`let direct` 在 641、`block-quic` 那一段在 656 ～ 673（P10 的位置成立）；`sniffable_session` 的七步次序是 180 ～ 187、190 ～ 192、198 ～ 201、204 ～ 212、216 ～ 222、227 ～ 229、230；`domain_targets`（`crates/rurge-rules/src/matcher.rs:530-537`）把 `s.sni` 接在目标之后、不限传输，所以 UDP 流带上 `sni` 之后 `DOMAIN,…,extended-matching` 会按它命中（任务 3 的 `a_hello_across_two_datagrams_is_routed_by_its_sni` 靠这一点）；`Engine::runtime()` 是 `Arc<Runtime>`，`Runtime.config` 是 `Arc<Config>`（`runtime.rs:34`），P9 ③ 的 `config.mitm.auto_quic_block` 路径成立（`crates/rurge-config/src/mitm.rs:45`，缺省真 65，解析 268）；`decrypting` 的次序与两行 DEBUG 的原文核对无改。 | 本提交 |
| 6 | 0 | 四个数：`Cargo.lock` 的 `[[package]]` 520 个；`unsafe_functions.rs` 的 `LISTED` 是 `(&str, &str); 23`、23 项；`sockets.rs` 的 `LISTED` 是 `(&str, &str, &str); 26`、26 行；带本机账户名的文件 26 个。与 P2 写的相同。 | 本提交 |
| 7 | 1 ～ 5 | P14 的表逐条找到、读过它断言什么：`initial.rs` 的用例是 11 条而不是十三条，换成 `read_first(..).is_some()` 之后仍绿（帧读不成答 `GaveUp`，仍 `Some`）；`tests/common/mod.rs` 的 `quic_initial()`（602 ～ 605）只有 PADDING，用它的八条在 `tests/udp.rs`（306、341、373、392、410、450、470、1370）与 `tests/tun.rs` 的 `a_blocked_quic_flow_answers_with_icmp`（231 ～ 258），其中 `block_quic_drops_quic_and_nothing_else` 与 `tun` 那一条把 `error` 断言成恰好 `"QUIC blocked"`，所以 `quic_initial()` 必须换成没有备注的完整 ClientHello；`HttpCounts {` 的字面量只有 `tests/sniff.rs:952-969` 一处；`/v1/rurge/http` 的逐字节用例只有 `crates/rurge-api/tests/api.rs:1612`；`open(` 的调用处只有 `udp.rs:546`，`forward(` 的是 `udp.rs:552`、1373、1395。补扫（`client_initial(`、`.sni`、`HttpCounts {`、`"rewrite":`、`open(`、`forward(`、`v1/rurge/http`）没有找到表以外要改的用例。 | 本提交 |
| 8 | 0 | 门禁（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过（退出码 0）；`cargo test -j 8 --workspace --no-fail-fast` 的 `rurge-http` 单元测试二进制以 `STATUS_HEAP_CORRUPTION`（0xc0000374）退出，第一次 92 个结果行、5093 通过、0 失败、15 忽略；单独重跑那一个二进制 287 通过、0 失败、2 忽略；合起来 93 / 5380 / 0 / 17，与本计划的基线（M4b 任务 11 评审时）相同。 | 本提交 |
| 9 | 0 | 协调者核过任务 0（`af6fd9a`，第 1 ～ 8 行）：门禁的数我从日志重数过（全量 92 个结果行、5093 通过、0 失败、15 忽略，`rurge-http` 的单元测试二进制以 `STATUS_HEAP_CORRUPTION` 退出、单独重跑 287 通过、2 忽略，合起来 93 / 5380 / 0 / 17；一台机器上的一次运行，不是 CI；日志里 `cannot bind http-api on 127.0.0.1:…` 是 `cli` 一条用例自己的日志行，不是失败）。第 2 行对：`initial.rs` 测试模块里 `#[test]` 与 `#[tokio::test]` 共 11 个，计划正文与 P14 的表写"十三条"是写计划时数错了，以 11 条为准。第 4 行照准：持住的期限写成 `now() + HELLO_WAIT`（`now()` 在 `udp.rs:117`，读的是 tokio 的钟，暂停的时钟之下照样走），睡 `tokio::time::sleep_until(期限.into())`，与 P5 写的 `tokio::time::Instant::now()` 等价。第 3、5、7 行抽查过：`now()`、`open` 的 638、641 与 `block-quic` 一段、`matcher.rs` 的 `domain_targets`（第 530 ～ 537 行）、`drain` 对每个数据报各调一次 `refused`，都对得上。**协调者的裁定（任务 1）**：`big_client_hello` 的长度除了 1500 ～ 1900（P3 ⑥），还要让 `hello_initials(dcid, &hello, 600)` 的第二个包封得进 1200 字节：dcid 8 个字节、包号 1 时一个 1200 字节的 Initial 里一个 CRYPTO 帧（偏移 600、两个字节的长度）最多装 1200 - 18 - 1 - 16 - 5 = 1160 个字节，所以长度不超过 1760；取 ALPN 名字的个数，让 `Some("held.test")` 与 `None` 两种都落在 1550 ～ 1700，任务 1 在它自己的行里记下量出的两个长度。 | 本提交 |
| 10 | 1 | `quic::testing` 的构造器（P3 的九项）与末尾的测试模块，九条用例全绿；`initial.rs` 的测试模块只改了 `use`（多引 `RFC9001_CLIENT_INITIAL`、`RFC9369_CLIENT_INITIAL`、`unhex` 三个名字），十一条用例的正文一字未改、原样通过，本地的 `DCID` 留着（值就是 `RFC_DCID`）。挪来的两个样例、`unhex` 与 `hex_digit` 放在 `testing.rs` 末尾、`client_initial_with_quic_bit` 之后（`RFC_DCID` 也在那里），新构造器在它之前、测试模块在最后。三处数值与事实照实记：① `big_client_hello` 的 ALPN 名字取 74 个（`rurge-test-alpn-00` 起），量出 `Some("held.test")` 是 **1601** 字节、`None` 是 **1583** 字节，都在 1550 ～ 1700 之内（一个名字时是 214 与 196，每个名字加 19 字节），`hello_initials(.., 600)` 的第二个包是 1001 字节的 CRYPTO，仍封得进 1200；② `long_packet` 按 P3 ③ 没有 Token 一项，所以 8 字节 dcid 的包 Length 字段在偏移 `7 + dcid.len()`（`a_long_packet_ends_where_its_length_says` 照这个算；一开始照 `initial.rs` 的 `length_too_long` 写成 `8 + …`，红过一次，是用例算错、不是构造器错）；③ `Initial::seal` 只把包头保护掩过的 `number_len` 个包号字节写回，后面的是密文、不能动。门禁（一台 Windows 11 开发机上的一次运行，不是 CI）：fmt 与 clippy 退出码 0；`cargo test -j 8 --workspace --no-fail-fast` 的两个测试二进制（`outbounds_http2`、`smart`）以 `STATUS_HEAP_CORRUPTION` 退出（本机管控软件的钩子 DLL，不是失败），全量 91 个结果行 / 5351 通过 / 0 失败 / 17 忽略，单独重跑那两个 16 与 22 通过、0 失败，合起来 **93 / 5389 / 0 / 17**，与基线加九条相符。与计划的其余出入：没有。 | 本提交 |
| 11 | 1 | 协调者核过任务 1（`b2bcb4a`、`66aea6b`，第 10 行）：门禁的数我从日志重数过（全量 91 个结果行、5351 通过、0 失败、17 忽略，`outbounds_http2` 与 `smart` 两个测试二进制以 `STATUS_HEAP_CORRUPTION` 退出、单独重跑 16 与 22 通过，合起来 93 / 5389 / 0 / 17，比基线多九条；一台机器上的一次运行，不是 CI）。构造器逐项对过 P3：`Frame::encode` 的 ACK、CRYPTO、CONNECTION_CLOSE 的字段次序与 RFC 9000 第 19.3、19.6、19.19 节相同，变长整数取最短的写法；`Initial::seal` 的长度算术、首字节的包号长度、包头保护只写回 `number_len` 个字节；`chaos_frames` 的切法（多出 2 ～ 10 个 CRYPTO 帧、2 ～ 10 个 PING、1 ～ 4 处 1 ～ 8 字节的 PADDING、Fisher–Yates 打乱）与 `chrome_frames` 的 `k`、`e` 照 P3 ⑧；测试模块的 `unseal` 自己拆包，不借 `header` 与 `opens`。**两处小毛病留给任务 2 顺手改（协调者的裁定：不为它们单跑一轮门禁）**：① `long_packet` 的 `payload` 只检查到 65535，`0x4000 | length` 在 `payload` 不小于 16384 时写出的不是两个字节的变长整数——改成那时 panic；② `chaos_frames_cover_their_pieces` 的文档注释把 "assertion" 拼成了 "asserion"。 | 本提交 |
| 12 | 2 | 读 ClientHello（P4 ①～⑧）：`is_client_initial` 改由 `read_first` 定，`opens` 换成 `open`（参数 `packet`、`pn_offset`、`keys`、`largest`，包号照 RFC 9000 §A.3 由 `packet_number` 还原，`packet.len()` 不足 `pn_offset + MIN_LENGTH` 时自己挡）；新的 `Pieces` 按偏移存 CRYPTO 的字节，重叠与相接的合并、重叠处逐字节比，64 KiB 与 32 段的上限在加之前查；`read_frames` 读 PADDING、PING、ACK（不按段数预先分配）、CRYPTO、CONNECTION_CLOSE（读不成）；`read_packets` 一个数据报一个包地走（长包头位、版本、Retry、两个连接 ID 的长度、只有 Initial 有的 Token、Length，打不开的 Initial 按 Length 跳过，读不成才停）；`Hello` 的 `Whole`、`Partial`、`GaveUp` 与 `HelloReader`（手写 `Debug`，不写密钥）。新加 30 条用例（`quic::initial` 的单元测试二进制里 41 条，原 11 条一字未改、原样通过，是分类没有变的证据）：P4 的 23 条与另加的 `Pieces`、读帧的 7 条单元用例。与计划的出入：① 计划与 P14 表写"十三条现有用例"，实际 11 条（修正记录第 2、9 行）；② `Pieces::add` 先查上限、空段之后什么也不做，照计划的次序，单元用例补了 64 KiB 与 32 段的边界；③ 没有别的出入。quinn 那一条（`a_real_quinn_client_hello_across_two_datagrams`）单独循环跑 22 次全过。门禁（一台 Windows 11 开发机上的一次运行，不是 CI）：fmt 与 clippy 退出码 0；全量 93 个结果行 / 5419 通过 / 0 失败 / 17 忽略，比基线多 30 条。`long_packet` 与拼写的两处小毛病在 `test(net)` 的提交 `748ed1b` 里（修正记录第 11 行）。 | 本提交 |
| 13 | 2 | 修正轮 1（协调者的评审）：① `Pieces` 的段缓冲改成 `VecDeque<u8>`，`add` 用二分找出碰到的段，最长的一段留着自己的缓冲、只在它前后补写其余段留下的字节，整个落在一段里面时不分配不拷贝；`adding_costs_what_arrives` 这条用例里 65 010 个字节送进去，旧写法往缓冲里写 1 000 075 020 个字节，新写法 45 020 个；`Pieces` 的文档注释写明为什么开销是线性的。② `see_head` 先看 0 起的第一段是否至少 4 个字节，再看第一个字节与 24 位长度（P4 ⑤、设计 4.5）；新用例 `the_head_is_read_once_four_bytes_are_here`。③ 整段比较 ClientHello 的断言改成 `assert!` 并只写两边的长度（派工单第 5 节）。门禁（一台 Windows 11 开发机上的一次运行，不是 CI）：fmt 与 clippy 退出码 0；全量 93 个结果行 / 5421 通过 / 0 失败 / 17 忽略（比修正前多两条）。 | 本提交 |
| 14 | 2 | 协调者核过任务 2（`748ed1b`、`7836738`，第 12 行）与它的修正轮 1（`6423a3a`，第 13 行）。门禁：两次全量的日志我数过——修正之前 93 个结果行、5419 通过、0 失败、17 忽略，修正之后 93 / 5421 / 0 / 17，两次 fmt 与 clippy 的退出码都是 0，没有崩溃与重跑（一台机器上的一次运行，不是 CI）；修正之后我在 `6423a3a` 上另跑了 `quic::initial` 的 43 条，全过。十一条旧用例与辅助的 `length_too_long` 逐个与 `9a50a3c` 比过，一字未改。读法逐项对过 P4 ① ～ ⑧：`header` 在最前，答 `None` 的数据报不分配、不导出密钥；后来的数据报一个包一个包地走，版本按线上的四个字节比，Retry 在读连接 ID 之前停，只有 Initial 读 Token，每一项都用 `get` 与 `checked_add`，"停"都不是读不成；包号照 RFC 9000 附录 A.3 还原，没有会下溢的减法；帧只认五种。**评审发现、修正轮 1 改了的三处**：① 拼接每加一段都新建一个"合并之后那么长"的缓冲、把碰到的段整段拷进去——这是派工单第 4 节协调者写的做法，错在协调者。经 SOCKS5 的 UDP 关联能送进 64 KiB 的数据报（`udp.rs` 的 `DATAGRAM`），里面约九千个一字节的 CRYPTO 帧，每个落在一段约 48 KiB 的段里或贴着它的一头，一个数据报就是约 1 GB 的拷贝，而读是在流的任务里同步做的。**协调者的裁定（代项目所有者）**：段的缓冲换成 `VecDeque<u8>`；整个落在一段里面的只比、不拷贝；碰到几段时最长的一段留着它的缓冲，只把前后要补的字节并进去（P4 ⑤ 只换了段的类型，语义不变）。`adding_costs_what_arrives` 钉住开销：送进去 65 010 个字节，旧写法往缓冲里写了 1 000 075 020 个，新写法 45 020 个。② 看头照 P4 ⑤ 与设计第 4.5 节，等偏移 0 起的四个字节到了再看第一个字节（`the_head_is_read_once_four_bytes_are_here`）。③ 整段比较 ClientHello 的 14 处断言改成只打印长度（与种子）。另外，协调者写了一个只放在本机临时目录、不进仓库的对照程序：把 `Pieces` 的源码抽出来，与一个逐字节的模型比，随机的偏移与长度、重发、改过的字节、越界与 `u64` 的末尾，每一步比答复、段与 `whole`。在 `6423a3a` 上 120 000 个序列、4 776 935 次 `add`，没有一步不同；在 `7836738` 上 2 万个序列也没有不同（那时按旧的看头时机比）。**留给以后的**：`VecDeque` 按倍数增长，一段的容量至多是它的字节数的两倍，所以设计第 4.5 节"一个读者至多多占持着的字节那么多"应当是至多两倍——由任务 6b 写进设计第 16 节。`Pieces` 文档注释里的 "about a dozen times at most"，按每并一次至少长一倍算，64 KiB 之内是至多 16 次，量级没有错，不为它另开修正轮。 | 本提交 |
| 15 | 3 | 持住、SNI、计数与备注（P5 ～ P8、P14）：`serve.rs` 多 `QuicCounts`（四个 `AtomicU64`，`Relaxed`）、`HelloRead`（`Read` / `Incomplete` / `Unreadable`）、`QuicSniffed`，`HttpCounts` 末尾多 `quic`；`engine.rs` 多 `quic_counts`（紧跟 `sniff_counts`，重载不清）、`http_counts` 填 `quic`；`lib.rs` 多导出 `QuicSniffed`。`udp.rs` 多 `HELLO_WAIT`（300 ms）与 `HELLO_DATAGRAMS`（16）、`Held`（`Whole` / `Incomplete` / `GaveUp` / `Ended`，不派生 `Debug`：`Whole` 装的是 ClientHello 的字节）、`hold`（期限 `now() + HELLO_WAIT`，`sleep_until(until.into())` 在 `select!` 外面 `pin!`，关联结束与队列关上都是 `Ended`）；`run_flow` 在 `turn_back` 与 `destination` 之后 `read_first(&held[0].datagram)`，`Held::Ended` 时送回 `done` 就返回（不建句柄、不留记录、不计数），`Whole` 交给 `parse_client_hello` 之后才 `new_handle_with`，计数、两句备注与两行 DEBUG（「表后的原文」逐字）在它之后、`open` 之前、一条流恰好一次，`Turned::Unknown` 的流也一样；`forward` 的 `first` 换成 `VecDeque<Queued>`，先按次序发完再进 `select!`；`Err` 那一支先记 `held.len()`、再 `drop(held)`，拒绝时对记下的次数各调一次 `refused`。`tests/common/mod.rs` 的 `quic_initial()` 换成 `hello_initial(RFC_DCID, &client_hello(None, &[b"h3"]))`、文档注释跟着改；`tests/sniff.rs` 的 `HttpCounts` 字面量补 `quic: QuicSniffed::default()`（P14 的两处，期望都不变）；`tests/udp.rs` 多 `udp_sink()`、`flowing()` 与六条 e2e（`a_hello_across_two_datagrams_is_routed_by_its_sni`、`without_extended_matching_the_sni_does_not_match`、`a_hello_that_never_completes_goes_out_after_300_ms`、`an_unreadable_hello_is_noted`、`a_flow_whose_association_ends_while_held_leaves_no_record`、`the_rfc_sample_flow_has_its_sni`），`udp.rs` 的测试模块多六条暂停时钟的单元用例（`holding_lets_go_once_the_hello_is_whole`、`holding_lets_go_at_300_ms`、`holding_lets_go_at_16_datagrams`、`holding_ends_with_the_association`、`holding_ends_when_the_queue_closes`、`holding_gives_up_with_the_reader`）与两个辅助（`hello_over_two`、`closes`）。**执行者的裁定**：`udp.rs` 里原来私有一个 `struct Held`（`Carriers` 的 `Slot` 里那个槽状态，字段 `carrier` 与 `why`），与 P5 要的 `enum Held` 撞名，把它改名为 `Opening`（只改类型名和它的文档注释，行为不变）。**执行期抓到的一个真错**：`forward` 改到一半时发完 `pop_front` 出来的那一个没有 `continue`，落进 `select!` 等下一个数据报，`held` 里其余的不再发；两条 `forward` 单元用例只有一个数据报、照过，两条「两条数据报」的 e2e 在 5 秒里收不到而红（`the datagram arrives within five seconds: Elapsed(())`），补上 `continue` 之后全过。与计划的其余出入：没有。新加 12 条（6 条单元、6 条 e2e）。端到端（整个 `udp` 二进制的 45 条）与暂停时钟的单元用例各单独循环跑 20 次，20 次全过。门禁（一台 Windows 11 开发机上的一次运行，不是 CI）：fmt 与 clippy 退出码 0；全量 93 个结果行 / 5433 通过 / 0 失败 / 17 忽略，比基线（93 / 5421 / 0 / 17）多 12 条，没有崩溃与重跑。 | 本提交 |
| 16 | 3 | P14 点名的现有用例照表改了两处、其余照过：`crates/rurge-engine/tests/common/mod.rs` 的 `quic_initial()` 换成完整的、不带 SNI 的 ClientHello（`hello_initial(RFC_DCID, &client_hello(None, &[b"h3"]))`），`crates/rurge-engine/tests/sniff.rs` 的 `HttpCounts` 字面量补 `quic: QuicSniffed::default()`，两处的期望都没动；`udp.rs` 的两条 `forward` 单元用例只把 `queued(..)` 换成 `VecDeque::from([queued(..)])`；用 `quic_initial()` 的八条 `tests/udp.rs` 用例、`a_lookalike_of_a_quic_initial_is_plain_udp` 与 `tests/tun.rs` 的 `a_blocked_quic_flow_answers_with_icmp` 都照过（`--test udp` 45 条、`--test tun` 30 条、`--test sniff` 22 条全过）。 | 本提交 |
| 17 | 3 | 修正轮 1（协调者的评审）：① 交上来时本任务有六处 `assert_eq!` 比整个数据报或整个 ClientHello，失败时两边的字节都会打出来，与派工单第 115 行不合（任务 2 的修正轮 ③ 是同一件事）——`udp.rs` 的 `holding_lets_go_once_the_hello_is_whole` 那一处，与 `tests/udp.rs` 的 `without_extended_matching_the_sni_does_not_match`、`a_hello_that_never_completes_goes_out_after_300_ms`、`an_unreadable_hello_is_noted` 里共五处，全改成 `assert!(a == b, "…{} bytes, not {}", …)`，信息只写情形与两边的长度。② `holding_lets_go_once_the_hello_is_whole` 补上计划写的「`held` 依次是 d1、d2」：长度之后加次序的断言（同样只写情形）。③ `HELLO_WAIT` 与 `HELLO_DATAGRAMS` 的文档注释照设计第 5.2 节的「理由」改写：300 毫秒只管凑不齐的、拼齐了立刻放；同 sing-box 的缺省，在 Xray 的 200 毫秒与 mihomo 的 3 秒之间，远小于客户端的 1 秒 PTO；不设旋钮（M5-D7）。16 个数据报装得下 16 KiB 的 ClientHello（quic-go 与 BoringSSL 的上限，每个约 1158 字节要 15 个），也挡住一个只发别的包的客户端让持着的队列一直长，`QUEUED_BYTES` 照旧管着全部。④ 去掉两处与本任务无关的改动：`run_flow` 第一行改回 `let mut session = association.template.clone();`，`const QUEUE` 之后多出来的空行去掉。⑤ `HelloRead::Read` 的文档注释改成「the flow's session has the SNI it named, if it named one」——没有 `server_name` 的 ClientHello 也读得成、也计进 `hello`（设计 5.4 节的表），那时会话的 SNI 是 `None`。⑥ `holding_ends_when_the_queue_closes` 的失败信息改成说队列关了。与计划的出入（第 15 行的「其余出入：没有」只指那一条）：计划里 `holding_ends_with_the_association` 的两例拆成了两条用例，第二条叫 `holding_ends_when_the_queue_closes`。没有改等待与次序，所以不再循环二十次；聚焦的 `udp::tests::holding` 6 条与 `--test udp` 45 条全过。门禁（一台 Windows 11 开发机上的一次运行，不是 CI）：fmt 与 clippy 退出码 0；全量 93 个结果行 / 5433 通过 / 0 失败 / 17 忽略，与第 15 行相同，没有崩溃与重跑。 | 本提交 |
| 18 | 3 | 协调者核过任务 3（`02a0332`，第 15、16 行）与它的修正轮 1（`7de770d`，第 17 行）。门禁：两次全量的日志我数过——修正之前与之后都是 93 个结果行、5433 通过、0 失败、17 忽略，两次 fmt 与 clippy 的退出码都是 0，没有崩溃与重跑（一台机器上的一次运行，不是 CI）。对过 P5 ～ P8：`hold` 的三样（关联结束与队列关上都是 `Ended`、期限、收数据报）与第 16 个；`run_flow` 里 `Ended` 不建句柄、不留记录、不计数，SNI 在 `new_handle_with` 之前写进会话，计数、两句备注与两行 DEBUG 一条 QUIC 流恰好一次、与「表后的原文」逐字相同；`forward` 先按次序发完持着的；`Err` 那一支对持着的每个数据报告诉客户端一次。P14 的两处与两条 `forward` 单元用例只换了参数。`Opening` 的改名（第 15 行，执行者的裁定）只动了类型名与一行注释，我认可。**评审发现、修正轮 1 改了的六处**（第 17 行）：① 六处 `assert_eq!` 比整个数据报或 ClientHello，失败时会打印字节（第 17 行写的"派工单第 115 行"是工作区 `dispatch-rules.md` 的第 115 行），报告"自查"里说没有的那一句也照实改了；② 持住的用例补"依次是 d1、d2"；③ `HELLO_WAIT` 与 `HELLO_DATAGRAMS` 的注释写设计第 5.2 节的理由（原来写的 1500 ～ 1900 字节是测试构造器的范围）；④ 去掉与本任务无关的类型标注与空行；⑤ `HelloRead::Read` 的注释（没有 SNI 的也算读成）；⑥ 拆出的 `holding_ends_when_the_queue_closes` 的失败信息与记成出入。另外，我在 `7de770d` 上跑了 `--test udp` 的 45 条一次，全过；又把六条端到端循环：六条一起（并行）跑 100 次，有 4 次以 `STATUS_HEAP_CORRUPTION`（0xC0000374）退出、没有失败的用例，都停在刚开跑时；每条单独各跑 50 次（共 300 次）、六条一起以 `--test-threads=1` 跑 60 次，都是 0 次；对照：`tests/udp.rs` 里八条旧的 QUIC 与拒绝用例一起并行跑 60 次，0 次。这与 `CLAUDE.md`「这台开发机」第一条、阶段 2 / M8 设计第 3.5 节查明的征状相同（注入的网络钩子在几个线程同时开始用套接字时出错，`--test-threads=1` 不出现），记作环境，不为它改代码；新的六条起得快、套接字密，一起跑时撞上的机会多些。CI 的 Windows 上若也出现，再另查。 | 本提交 |
| 19 | 4 | `sniffable_target`（P9 ① ②）：`sniffable_session` 里入口（`Http`、`Socks5`、`Tun`）、`named_at_a_proxy_port`、`raw_listed`、端口（80、443、`force-http-engine-hosts`、`widened_by_mitm`）这几步原样搬过去、注释跟着走，交回 `probed`；`sniffable_target` 不看传输（UDP 流也问它），文档注释写明它与 `auto_quic_blocks` 共用、`sniffable_session` 另看传输、暂停与预匹配。`sniffable_session` 改成先看 TCP 与暂停，再问 `sniffable_target`，`at_a_proxy_port` 仍在它里面算（预匹配那一步要它）。只搬、不改意思：抽出之后、加阻断之前跑过 `--test sniff`（30 条）、`--test mitm`（43 条）与 `--test http`（36 条），全过。 | 本提交 |
| 20 | 4 | `auto-quic-block`（P9 ③、P10、P12）：`serve.rs` 多 `auto_quic_blocks`（`[MITM]` 的开关 && `sniffable_target` && `decrypting`，`ClientHello` 的 ALPN 为空）与 `QuicCounts::count_blocked`，`decrypting` 原样调、没有另加日志；`udp.rs` 多常数 `AUTO_QUIC_BLOCKED`（「表后的原文」逐字），`open` 多最后一个参数 `hello_read`，在 `REJECT` 之后、`block-quic` 之前阻断（`set_error`、`count_blocked`、`Rejected(RejectKind::Reject)`），`run_flow` 传 `read == Some(HelloRead::Read)`。用例：`tests/mitm.rs` 多 14 条经 SOCKS5 的（被阻断一条；不阻断八条：`auto-quic-block = false`、`set_mitm(false)`、没有可用的 CA、`client-source-address` 不收、否定的项、`always-raw-tcp-hosts`、不带端口的项在 8443、不完整的 ClientHello；`a_port_entry_blocks_only_its_port`；`always-allow_does_not_stop_auto_quic_block`；`with_block_quic_too_the_record_names_auto_quic_block`；`a_reject_rule_comes_before_auto_quic_block`；`a_hello_without_sni_is_judged_by_its_target`），`tests/tun.rs` 多 1 条经虚拟网卡的（`auto_quic_block_answers_each_held_datagram_with_icmp`，两个数据报各一个 ICMP「管理上禁止」）。`tests/udp.rs` 的 `flowing()` 挪进 `tests/common/mod.rs`（改成 `pub async fn`，调用处不变），`tests/udp.rs` 删掉它。与计划的出入：没有；自己定的一件事记在下面。写用例时把否定的项写在通配项后面（`*.test, -origin.test`），用例在等第一个数据报时超时——`HostList` 是第一条命中的项决定，`*.test` 先命中，后面的否定不再看；仓库里现成的那条用例把否定写在前面，照它改成 `-origin.test, *.test` 之后过，行为是既有的，没有改代码。RED：把阻断那一行临时改成 `if false && hello_read && …`，`--test mitm` 的 `auto_quic_block_blocks_quic_to_a_listed_host` 以等待记录结束超时变红（6.02 秒），`--test tun` 的那条以等不到给应用的包变红（`a packet for the app in time: Elapsed(())`）；改回来之后 `--test mitm` 57 条、`--test tun` 23 条全过。端到端单独循环：`mitm` 整个二进制与 `tun` 整个二进制各连跑 20 次，20/20 全过，没有 `STATUS_HEAP_CORRUPTION`。门禁（一台 Windows 11 开发机上的一次运行，不是 CI）：fmt 与 clippy 退出码 0；全量 93 个结果行 / 5448 通过 / 0 失败 / 17 忽略，比基线（93 / 5433 / 0 / 17）多 15 条，没有崩溃与重跑。 | 本提交 |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
