# 阶段 3 / M5「识别、阻断与 `[IP Rewrite]`」实施计划

> **执行方式**：交给 Pi，由协调的会话逐个任务派工、逐个任务评审（设计第 2 节、M5-D1；做法同 M4d）。
>
> - 协调的会话每个任务开一个 Pi 会话，交给它这个任务的全文与要用的接口（派工文件）。Pi 做完把报告写进派工文件指定的路径，在本地提交。
> - 协调者读改动、重跑门禁、评审。有问题回到同一个 Pi 会话修正，再评审；过了才派下一个任务。
> - 每个任务：先写失败的用例、看到红；再实现到绿；跑门禁；对照本任务的「判据」与「全局约束」逐条自查；最后提交（**只在本地，不推送**）。
> - 与计划的出入记进末尾的「执行期修正记录」，**不改计划的正文**。
>
> 没有等 CI 的停点：M5 没有平台代码（设计第 1 节）。做完任务 10，由项目所有者推送，协调者读回那次 CI。

**目标**：做完阶段 3 剩下的识别与阻断。具体是：

- UDP 流在所有端口上识别 QUIC（包头筛过之后用 Initial 密钥解密来确认）与 STUN；
- `block-quic` 照手册 6.11 多一个 `adaptive`，`auto` 改成 6.11 的含义；
- `udp-priority`：写回虚拟网卡的 UDP 包不排在 TCP 后面；
- `[IP Rewrite]` 第一次生效（`reflect` / `reject` / `drop`）；
- 开着 `encrypted-dns-follow-outbound-mode` 时，DNS 服务器自己的连接带上它的主机名，按主机名写的规则第一次能匹配它；
- 手册 6.11 的 WireGuard 服务端写法能加载；
- 三个基准重跑、记数；文档收尾。

**架构**：没有平台代码，全部在开发机上编译、运行。

- 识别放在 `rurge-net` 的 `quic::initial`，引擎调用它；测试用的构造器在 `rurge-net` 的 `quic::testing`，几个 crate 的用例共用。
- `block-quic` 的判定是一个纯函数（四种全局 × 四种参数 × 直连与否 × 载不载 UDP），"自适应"交给引擎里一张 200 个名字的表。
- `[IP Rewrite]` 在 `rurge-config` 里读成一张按族分存的表，每一代经 `TunSettings` 交给虚拟网卡；收包的任务每批读一次设置，命中的包在 `rurge-tun` 里就地处理，不进协议栈、规则与请求记录。
- `udp-priority`：写任务两条有界队列，由发送的一方按这一代的设置选队列。
- DNS 会话带名字：`Connector` 两个带缺省实现的方法，`rurge-dns` 调它们，引擎的流水线实现它们；规则用跟着会话来的地址，不经引擎的解析器。

**技术栈**：rustls 0.23.43 的 `quic` 模块与 ring 的套件（工作区已开）；quinn（只在一条用例里截一个真的客户端 Initial）；ipnet 2.12.1；tokio。不加外部包，不加工作区依赖边。

**依据**：

- M5 设计（下面写"设计"指它）：`docs/superpowers/specs/2026-10-28-phase3-m5-recognition-blocking-ip-rewrite-design.md`，全文；已决事项 M5-D1 ～ D24。**本计划改了设计正文的地方列在「计划期决定」里（P8、P9、P15、P16），以计划为准。**
- 手册复查报告：`docs/reports/2026-10-28-manual-6.11-recheck.md`（`block-quic` 与 WireGuard 的原话）。
- 总设计第 8、10 节；M3 设计第 4.4、4.5 节（包的去向、写任务）。
- 写计划时对着 `9ee270f` 核对过的事实（设计第 16 节 V1 ～ V12）写在「计划期决定」里。

## 全局约束

**依赖**

- 不加外部包，不加工作区依赖边（M5-D23）。`Cargo.lock` 的包数在 `9ee270f` 是 519 个，每个任务做完照旧。
- 要用的边都已有：`rurge-net` → `rustls`（ring）、`quinn`；`rurge-dns` → `rurge-net`；`rurge-engine` → `rurge-net`（dev 依赖开着 `testing`）、`rurge-rules`；`rurge-tun` → `rurge-config`。
- `unsafe_code`：不加放开的函数。`crates/rurge-platform/tests/unsafe_functions.rs` 的 `LISTED` 仍是 23 项。

**缺省的 `cargo test` 不碰系统**

- M5 没有平台代码、没有 L2 用例。不建网卡，不改系统的任何设置。
- 不执行 `netsh`、`route`、`ipconfig`、`ip`、`resolvectl`、`networksetup`，不读真实的网络环境。
- 虚拟网卡的用例只用 `rurge-tun` 的内存设备（`MemoryDevice`、`GatedDevice`）与引擎用例的 `Rig`（`crates/rurge-engine/tests/common/tun.rs`）。

**测试的约定**

`CLAUDE.md`「测试约定」整节适用。写 `rurge-tun` 与经引擎的 TUN、DNS 用例之前，读 `docs/testing.md`「测试约定（全文）」里对应的一节。与本计划直接相关的：

- 只用回环与端口 0，失败时也不离开回环。
- 写了主机名的 Profile 另给第二道防线：一条按地址拒绝的规则（引擎的 TUN 用例用 `tests/common/tun.rs` 的 `guarded`），或一行把名字钉到 `127.0.0.1` 的 `[Host]`。
- 文档地址（`192.0.2.0/24`、`198.51.100.0/24`、`203.0.113.0/24`、`2001:db8::/32`）只作为 `[IP Rewrite]` 的行与内存设备上的包的地址，不作为可能被拨的目标。
- 等待都有时限。固定等待只许"断言什么都没发生"，注释写它等的是什么、余量多大。
- 新写的、带等待的用例提交前单独循环跑二十次以上：任务 1 的 quinn 截包、任务 5 与 6 的内存设备用例、任务 7 的引擎用例。
- 抓日志用 `LogCapture`，跑在当前线程的运行时上（普通的 `#[tokio::test]`）。`rurge-tun` 有自己的（`src/tests/log_capture.rs`）；引擎的在 `tests/common/log_capture.rs`。
- 数"服务器被问了几次"的用例用不重发的 `FanoutOpts`。
- quinn 截包的用例只在回环上，每一步给时限（设计第 14 节）。
- **内联的测试模块放在文件的末尾**：`crates/rurge/tests/sockets.rs` 读生产代码，读到第一个 `#[cfg(test)] mod … {` 为止；那之后再出现顶格的条目会让它变红。

**门禁与这台开发机**（每个任务末尾一次；说"通过"之前必跑）

- 门禁：`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test -j 8 --workspace --no-fail-fast`。
- 本机 msvc 工具链没有 rustfmt。跑 `cargo fmt` 之前让环境变量 `RUSTFMT` 指向 gnu 工具链里同版本的 rustfmt：用户目录下的 `.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe`（同版本，输出与 CI 一致）。不要为此安装或升级工具链。
- 聚焦地跑用例也经整个工作区：`cargo test -j 8 --workspace --lib -- <过滤词>`；集成测试 `cargo test -j 8 --workspace --test <名字> -- <过滤词>`。
- **不用 `-p`**：它另编一份依赖，`target/` 涨得很快，写满过 D: 盘。唯一的例外是任务 9 的基准命令。
- 跑全量之前 `df -h /d`：剩余不足约 15 GiB 就停下报告。
- 测试二进制以 `STATUS_HEAP_CORRUPTION`（本机管控软件注入的钩子 DLL）或 `STATUS_ACCESS_VIOLATION` 退出：单独重跑那一个二进制，报告里写明。
- rustc 在增量编译里报内部编译器错误：删掉出错的那个目标在 `target/debug/incremental/` 下的目录（可再生的缓存）再编。
- 回环的 TCP / UDP 7、9、13、17、19 号端口在这台机器上开着，CI 上关着：用例不假定任何固定端口的状态。"一个没有应答的地方"是用例自己持有的套接字。
- 结果照实记：本机的全量运行是"一台机器上的一次运行，不是 CI"；需要参照服务端的互操作用例在本机立刻返回、算在通过的数里，要说出来。

**仓库的规矩**（`CLAUDE.md`「必须遵守的规矩」整节适用；与本计划直接相关的）

- 日志：名字与地址只在 DEBUG（设计第 13 节）。
  - `ip_rewrite` 的丢弃逐包 DEBUG（带两个地址），从不 WARN；
  - `reflected` 不写日志；
  - 自适应的状态落定在 DEBUG，带名字；
  - QUIC / STUN 的识别不写日志。
- 凭据不进日志、错误文本、API 与 `Debug`。WireGuard 的新诊断只引可以引的键名，不引值（`wireguard.rs` 的 `quotable`、`said` 的做法）。
- 诊断：不加新码（M5-D22）。沿用的码与常数：
  - `W0012` `codes::W_INVALID_VALUE`；
  - `W0029` `codes::W_PARAM_NOT_EFFECTIVE`；
  - `W0007` `codes::W_PROTOCOL_NOT_IMPLEMENTED`；
  - `E0018` `codes::E_INVALID_POLICY_PARAM`；
  - `E0023` `codes::E_WIREGUARD_SECTION`。
- 出站套接字只有一处建法：本计划的生产代码不新建套接字、不问系统解析库、不新建不带钩子的连接器。若改动让 `crates/rurge/tests/sockets.rs` 变红，停下报告。
- `rurge-tun`：起 TUN 之前先 `TunOptions::check`；停止取消正在转发的会话，不取消正在进行的拨号。改 `rurge-tun` 的代码（任务 5、6）之前，读 `docs/status.md` M3a、M3b、M3c 各节"必须知道的事"。
- 文档与报告用写文件 / 改文件的工具改。**永远不要用把一个文件的内容经管道写回同一个文件的命令**（`Get-Content x | Out-File x -Append` 一类；2026-10-27 这样写满过 D: 盘）。

**写法**

- 注释、日志与错误文本用英文，照所在文件的写法：文档注释写"为什么"与前提，句子完整。
- 提交说明照 `git log` 的样子：`feat(net): …`、`feat(engine): …`、`feat(tun): …`、`feat(config): …`、`docs: …`，标题用中文。正文说做了什么、改了哪些现有用例的期望、为什么。Pi 的提交不带署名行。
- 只在本地提交：不 `push`、不 `pull`、不 `fetch` 远端、不 `amend`、不改写历史。

**不做的事**

- 只运行本仓库构建出来的二进制与 Pi 自己。不下载、不安装任何东西（含 `rustup target add`）。
- 不手动跑 `rurge run`、`rurge dns lookup`；`rurge service` 只用 `--dry-run`。
- 参考实现：GPL 的（sing-box、mihomo、Xray-core）只取事实，代码不抄。RFC 的测试向量是公开的事实，照抄进用例。

**计划没有写到、要做决定的地方**

- 能按设计与总设计的原则定的：自己定，记进修正记录，写"执行者的裁定"。
- 去向写着"项目所有者"的（设计第 19 节：总设计 Q8、B2 的保守读数），或会改变 M5-D1 ～ D24 的：停下来列出，交给协调者。
- 现有用例变红：本计划点了名的照计划改，在提交说明里说；没点名的停下来，记进修正记录，不顺手改期望值。

## 评审要点

设计没写、而最可能咬到使用者的几种输入，与负责它们的任务和用例：

1. **识别器读到坏的或恶意的数据报。** 任何应用都能往任意端口发一个 1200 字节、首字节 `0xc0`、版本 1 的数据报；变长整数、长度与偏移都由对方给。
   - 期望：不 panic、不越界、不为过不了包头筛的数据报分配内存；答 false。
   - 用例（任务 1）：`no_prefix_of_an_initial_is_one`、`no_bit_flip_in_the_first_64_bytes_makes_one`。
2. **`[IP Rewrite]` 拿回应去回应。** 被 `reject` 的地址发来 ICMP 差错、RST 或非首片，回应它就可能来回不停。
   - 期望：这三种不回，只计数。
   - 用例（任务 5）：`reject_answers_no_error_with_an_error`、`reject_answers_no_reset_with_a_reset`、`reject_answers_no_later_fragment`。
3. **把 rurge 自己的地址写进 `[IP Rewrite]`**（例如 `198.18.0.0/15 = drop` 挡掉 DNS 与所有 fake 地址）。
   - 期望：照写的办，按地址命中、不反查名字、不例外（设计第 8.3 节）。
   - 用例（任务 5）：`a_fake_address_is_matched_as_an_address`。
4. **写任务的一条队列关了，另一条还开着。**
   - 期望：不忙转；另一条照样写；两条都关了才结束。
   - 用例（任务 6）：`the_writer_goes_on_with_one_queue_closed`、`the_writer_ends_when_both_queues_have_closed`。
5. **DNS 会话的规则要一个地址**（`IP-CIDR` 排在 `DOMAIN` 之前）。
   - 期望：不把 DNS 服务器的名字交给引擎的解析器（那是自己解析自己，会挂住或绕回来）。
   - 用例（任务 7）：`a_dns_session_matches_rules_by_the_servers_name`：上游的名字只被引导问过；规则里先有一条 `IP-CIDR`；整个查找有 5 秒的时限。
6. **手册自己的 WireGuard 服务端例子。**
   - 期望：Profile 照常加载；用这个节的策略的连接立刻失败，失败的文字说明原因，不挂住。
   - 用例（任务 8）：`the_manuals_server_example_loads`、`a_policy_on_a_section_without_peers_fails_at_once`。

---

## 文件结构

| 文件 | 任务 | 职责 |
| --- | --- | --- |
| `crates/rurge-net/src/quic/initial.rs`（新）、`quic/mod.rs`、`quic/testing.rs` | 1 | `is_client_initial`；造客户端 Initial 的构造器 |
| `crates/rurge-engine/src/{sniff.rs,udp.rs}`、`tests/common/mod.rs`、`tests/udp.rs` | 2 | 所有端口上的 QUIC 与 STUN 标签；用例里的真 Initial |
| `crates/rurge-config/src/spec/{common.rs,mod.rs}`、`crates/rurge-engine/src/{udp.rs,quic_adaptive.rs（新）,engine.rs,lib.rs}`、`tests/udp.rs` | 3 | `BlockQuic`；`quic_verdict`；自适应的表与客户端 TCP 会话的通知 |
| `crates/rurge-config/src/{ip_rewrite.rs（新）,lib.rs,config.rs,deferred.rs}` | 4 | `[IP Rewrite]` 的解析与 `Config.ip_rewrite` |
| `crates/rurge-tun/src/{hooks.rs,packet.rs,dispatch.rs,rewrite.rs（新）,context.rs,lib.rs,stats.rs}`、`src/tests/…`、`crates/rurge-engine/src/runtime.rs`、`tests/tun.rs`、`crates/rurge-api/src/routes/features.rs`、`tests/api.rs`、`crates/rurge/tests/run/enhanced_mode.rs` | 5 | 进包的去向、三种动作、计数与 API |
| `crates/rurge-tun/src/{writer.rs,context.rs,hooks.rs,lib.rs,udp/client.rs,dns/udp.rs,icmp.rs,rewrite.rs}`、`src/tests/…`、`crates/rurge-engine/src/runtime.rs` | 6 | 写任务两条队列 |
| `crates/rurge-net/src/{connector.rs,quic/mod.rs}`、`crates/rurge-dns/src/{bootstrap.rs,upstream/session.rs,resolver.rs}`、`crates/rurge-engine/src/{dns_pipeline.rs,engine.rs}`、`tests/pipeline.rs` | 7 | 内部 DNS 会话带名字 |
| `crates/rurge-config/src/{wireguard.rs,spec/mod.rs}`、`crates/rurge-engine/tests/outbounds_wireguard.rs`（或已有的 WireGuard 用例文件） | 8 | 手册 6.11 的 WireGuard 写法 |
| 本计划「执行期修正记录」 | 9 | 基准的读数 |
| 文档（任务 10 列全） | 10 | 收尾 |

模块的名字与拆法可以照实际调整，调了记进修正记录。

---

## Task 0：开工前核对

**做什么**：对着当时的 `main` 核对下面每一项。

- 有出入的，记进「执行期修正记录」，一项一行，写"计划写 X，代码是 Y，按 Y 做"；按记录调整后面的任务。
- **不改计划的正文。**
- 没有出入也记一行。

1. **工作树**：`git status` 干净；`9ee270f` 在历史里，之后只多了本计划的提交。
2. **读**：设计全文（尤其第 4.2、6.2 ～ 6.3、7.2、8.3 ～ 8.5、9.2 ～ 9.4、10.2 节）与本计划的「计划期决定」。
3. **名字与签名**：用 `git grep -n` 逐项找到，签名与「计划期决定」写的相同。
   - `rurge-net`：`quic::testing::{TestCert, endpoint}`、`TestCert::client_tls`、`quic::{client_config, Transport, Path}`、`Transport::STANDARD`、`Path::open`、`connector::{Connector, ConnectOpts, Target}`。
   - `rurge-dns`：`bootstrap::BootstrapConnector` 与它的 `each_address`、`upstream/session.rs` 的 `Dialer::connect_to`、`testing::MockDns` 的 `spawn`、`set`、`queries`、`query_count`；`resolver.rs` 的用例 `a_named_server_reaches_its_connector_as_the_address_the_bootstrap_gave`。
   - `rurge-config`：`spec::common` 的 `Tristate`、`TRISTATES`、`CommonOpts.block_quic` 与读它的那一行（`r.choice("block-quic", &TRISTATES)`）；`general::BlockQuicGlobal`；`deferred.rs` 的 `DEFERRED`；`config.rs` 的用例 `ip_rewrite_is_a_known_section_kept_for_later` 与读 `[SSID Setting]` 的那一段；`wireguard.rs` 的 `Report`、`peer`、`parse_section`；`spec::NotImplemented`；上面「仓库的规矩」列的五个诊断常数。
   - `rurge-rules`：`engine::FixedResolve`、`ResolvedAddrs`。
   - `rurge-engine`：`sniff::is_quic_initial`；`udp.rs` 的 `quic_blocked`、`open`、定协议标签的那一行（`asked.port == 443 && …`）；`engine.rs` 的 `impl Dialer for Engine` 的 `dial`、`choose_policy`、`route_internal`、`dial_internal`、`connect_internal`、`dial_internal_udp`、`open_internal_udp`、`bypass_to_direct`；`dns_pipeline.rs` 的 `PipelineConnector`、`Face`；`runtime.rs` 建 `TunSettings` 的那一处；用例的 `tests/common/mod.rs` 的 `quic_initial`、`tests/common/tun.rs` 的 `Rig`、`guarded`，`tests/tun.rs` 的 `prohibited_from`。
   - `rurge-tun`：`hooks::{Hooks, TunSettings}`；`packet` 的 `classify`、`segment`、`rst_for`、`Reason`、`Dropped`、`Parsed`；`dispatch::{run, route}`；`context::Context`；`writer::run`；`icmp` 的 `prohibited`、`refuse`、`limiter`；`tcp::admission::Admission` 的 `allow_reset`、`count`；`stats::Stats::drop`；`Counters`；`testing::hooks::FakeHooks::set`；`src/tests/support.rs` 的 `rig_extras`、`Extras`、`settings`、`GatedDevice`。
   - `rurge-api`：`routes/features.rs` 的 `enhanced_json`；`tests/api.rs` 里写全 `TunCounters` 字段的两处。
   - `rurge`：`tests/run/enhanced_mode.rs` 里 `dropped.len()` 是 10 的那一句。
4. **数**：`Cargo.lock` 里 `[[package]]` 的个数（519）；`LISTED` 的项数（23）。
5. **门禁**：跑一遍，记下测试二进制数、通过数、失败数与忽略数。

**判据**：修正记录里有任务 0 的行；门禁的数字记下。

---

## Task 1：QUIC Initial 的识别与构造器（`rurge-net`）

**改哪里**（设计第 4.2 ～ 4.4、4.6 节；M5-D2 ～ D4；P2 ～ P5）

- 新文件 `crates/rurge-net/src/quic/initial.rs`；`quic/mod.rs` 加 `pub mod initial;`，不带 `cfg`。
- `crates/rurge-net/src/quic/testing.rs` 加构造器。

**接口**（后面的任务用）

```rust
// crates/rurge-net/src/quic/initial.rs
/// Whether `datagram` begins with a client's QUIC Initial packet (design 4.2):
/// the first packet in it, as far as its Length reaches, has a long header
/// with the fixed bit, a version this recognises (v1, v2, drafts 29–32), the
/// Initial type of that version, connection IDs of lengths QUIC allows, and
/// opens with the client Initial keys of its own Destination Connection ID.
/// A datagram the header checks turn away costs no allocation and no key.
pub fn is_client_initial(datagram: &[u8]) -> bool
```

```rust
// crates/rurge-net/src/quic/testing.rs
/// A client's first datagram: one Initial packet of `version` (1,
/// 0x6b3343cf or a draft 0xff00001d–0xff000020) to the Destination
/// Connection ID `dcid` (any length, so that a test can make a wrong one),
/// sealed with the client Initial keys of that DCID and version, header
/// protection applied (RFC 9001 §5), the datagram 1200 bytes long (plan P5).
/// Panics on any other version.
pub fn client_initial(version: u32, dcid: &[u8]) -> Vec<u8>
```

判法是设计第 4.2 节的九步；写法上的细节见 P4，构造器的布局见 P5。

**用例**（`initial.rs` 末尾的 `mod tests`）

正例：

- `the_rfc_9001_client_initial_is_one`：下面的 v1 向量。
- `the_rfc_9369_client_initial_is_one`：下面的 v2 向量。
- `what_the_builder_makes_is_one`：版本 `1`、`0x6b3343cf`、`0xff00001d`、`0xff000020` 各一，DCID 为 `8394c8f03e515708`、1 个字节、20 个字节。
- `a_coalesced_datagram_is_judged_by_its_first_packet`：v1 向量后面接 100 个字节的 `0xaa`。
- `a_real_quinn_client_initial_is_one`（`#[tokio::test]`）：
  1. 用例自己在回环上绑一个 UDP 套接字，只收、不答；
  2. 用 `TestCert::new().client_tls(&["h3"])` 与 `client_config(…, &Transport::STANDARD)` 建 quinn 客户端（`quinn::Endpoint::client` 绑 `127.0.0.1:0`），`connect` 到那个套接字，名字 `localhost`；
  3. 套接字 5 秒内收到的第一个数据报答 true；
  4. 丢掉端点，不等握手。

反例（都答 false）：

- `a_changed_payload_byte_is_not_one`：v1 向量改第 600 个字节；另改最后一个字节。
- `a_changed_dcid_byte_is_not_one`：v1 向量改 DCID 的第一个字节（数据报的第 7 个字节，下标 6）。
- `headers_that_are_not_an_initials_are_not_ones`，逐条：
  - 1199 字节（构造器的 v1 截短）；
  - 短包头（首字节去掉 `0x80`）；
  - 固定位是 0（去掉 `0x40`）；
  - 版本 0；未知版本 `0x0a0a0a0a`；草案 28（版本改成 `0xff00001c`）；
  - v2 的类型写成 `0b00`；v1 的类型写成 `0b01`（首字节第 4、5 位，不受头部保护）；
  - DCID 长 0 与长 21（构造器造）；
  - Length 大于数据报能装下的（构造器的包把两字节的 Length 改成 `0x7fff`）；
  - 旧的假包：`0xc3 0 0 0 1` 补零到 1200 字节。
- `no_prefix_of_an_initial_is_one`：v1 向量的每个前缀，长 0 ～ 1199。
- `no_bit_flip_in_the_first_64_bytes_makes_one`：v1 向量的前 64 个字节，逐字节逐位翻转，每一种都答 false、不 panic。

识别器认得 RFC 的两个包，也认得构造器的包：构造器的正确性由这两件一起钉住。

**测试向量**（RFC 9001 与 RFC 9369 附录 A.2 "The resulting protected packet is:" 之后的十六进制。两份都是 1200 字节，DCID `8394c8f03e515708`、包号 2、Length `0x449e`；字节串的 SHA-256 写在后面，用来核对抄得对不对。用例里写成常数，解十六进制的小函数自己写，跳过空白。）

```rust
// RFC 9001 A.2, SHA-256 of the 1200 bytes:
// 73fa0210cb4a5a17dc10b9dc98e5cc359ba1c20fe7c0e93a9e1dcb473c37e097
const RFC9001_CLIENT_INITIAL: &str = concat!(
    "c000000001088394c8f03e5157080000449e7b9aec34d1b1c98dd7689fb8ec11",
    "d242b123dc9bd8bab936b47d92ec356c0bab7df5976d27cd449f63300099f399",
    "1c260ec4c60d17b31f8429157bb35a1282a643a8d2262cad67500cadb8e7378c",
    "8eb7539ec4d4905fed1bee1fc8aafba17c750e2c7ace01e6005f80fcb7df6212",
    "30c83711b39343fa028cea7f7fb5ff89eac2308249a02252155e2347b63d58c5",
    "457afd84d05dfffdb20392844ae812154682e9cf012f9021a6f0be17ddd0c208",
    "4dce25ff9b06cde535d0f920a2db1bf362c23e596d11a4f5a6cf3948838a3aec",
    "4e15daf8500a6ef69ec4e3feb6b1d98e610ac8b7ec3faf6ad760b7bad1db4ba3",
    "485e8a94dc250ae3fdb41ed15fb6a8e5eba0fc3dd60bc8e30c5c4287e53805db",
    "059ae0648db2f64264ed5e39be2e20d82df566da8dd5998ccabdae053060ae6c",
    "7b4378e846d29f37ed7b4ea9ec5d82e7961b7f25a9323851f681d582363aa5f8",
    "9937f5a67258bf63ad6f1a0b1d96dbd4faddfcefc5266ba6611722395c906556",
    "be52afe3f565636ad1b17d508b73d8743eeb524be22b3dcbc2c7468d54119c74",
    "68449a13d8e3b95811a198f3491de3e7fe942b330407abf82a4ed7c1b311663a",
    "c69890f4157015853d91e923037c227a33cdd5ec281ca3f79c44546b9d90ca00",
    "f064c99e3dd97911d39fe9c5d0b23a229a234cb36186c4819e8b9c5927726632",
    "291d6a418211cc2962e20fe47feb3edf330f2c603a9d48c0fcb5699dbfe58964",
    "25c5bac4aee82e57a85aaf4e2513e4f05796b07ba2ee47d80506f8d2c25e50fd",
    "14de71e6c418559302f939b0e1abd576f279c4b2e0feb85c1f28ff18f58891ff",
    "ef132eef2fa09346aee33c28eb130ff28f5b766953334113211996d20011a198",
    "e3fc433f9f2541010ae17c1bf202580f6047472fb36857fe843b19f5984009dd",
    "c324044e847a4f4a0ab34f719595de37252d6235365e9b84392b061085349d73",
    "203a4a13e96f5432ec0fd4a1ee65accdd5e3904df54c1da510b0ff20dcc0c77f",
    "cb2c0e0eb605cb0504db87632cf3d8b4dae6e705769d1de354270123cb11450e",
    "fc60ac47683d7b8d0f811365565fd98c4c8eb936bcab8d069fc33bd801b03ade",
    "a2e1fbc5aa463d08ca19896d2bf59a071b851e6c239052172f296bfb5e724047",
    "90a2181014f3b94a4e97d117b438130368cc39dbb2d198065ae3986547926cd2",
    "162f40a29f0c3c8745c0f50fba3852e566d44575c29d39a03f0cda721984b6f4",
    "40591f355e12d439ff150aab7613499dbd49adabc8676eef023b15b65bfc5ca0",
    "6948109f23f350db82123535eb8a7433bdabcb909271a6ecbcb58b936a88cd4e",
    "8f2e6ff5800175f113253d8fa9ca8885c2f552e657dc603f252e1a8e308f76f0",
    "be79e2fb8f5d5fbbe2e30ecadd220723c8c0aea8078cdfcb3868263ff8f09400",
    "54da48781893a7e49ad5aff4af300cd804a6b6279ab3ff3afb64491c85194aab",
    "760d58a606654f9f4400e8b38591356fbf6425aca26dc85244259ff2b19c41b9",
    "f96f3ca9ec1dde434da7d2d392b905ddf3d1f9af93d1af5950bd493f5aa731b4",
    "056df31bd267b6b90a079831aaf579be0a39013137aac6d404f518cfd4684064",
    "7e78bfe706ca4cf5e9c5453e9f7cfd2b8b4c8d169a44e55c88d4a9a7f9474241",
    "e221af44860018ab0856972e194cd934",
);

// RFC 9369 A.2, SHA-256 of the 1200 bytes:
// b0b766fe7e762577072d3365cc4f9635809210cba301a67f8457f5e6fdac5c68
const RFC9369_CLIENT_INITIAL: &str = concat!(
    "d76b3343cf088394c8f03e5157080000449ea0c95e82ffe67b6abcdb4298b485",
    "dd04de806071bf03dceebfa162e75d6c96058bdbfb127cdfcbf903388e99ad04",
    "9f9a3dd4425ae4d0992cfff18ecf0fdb5a842d09747052f17ac2053d21f57c5d",
    "250f2c4f0e0202b70785b7946e992e58a59ac52dea6774d4f03b55545243cf1a",
    "12834e3f249a78d395e0d18f4d766004f1a2674802a747eaa901c3f10cda5500",
    "cb9122faa9f1df66c392079a1b40f0de1c6054196a11cbea40afb6ef5253cd68",
    "18f6625efce3b6def6ba7e4b37a40f7732e093daa7d52190935b8da58976ff33",
    "12ae50b187c1433c0f028edcc4c2838b6a9bfc226ca4b4530e7a4ccee1bfa2a3",
    "d396ae5a3fb512384b2fdd851f784a65e03f2c4fbe11a53c7777c023462239dd",
    "6f7521a3f6c7d5dd3ec9b3f233773d4b46d23cc375eb198c63301c21801f6520",
    "bcfb7966fc49b393f0061d974a2706df8c4a9449f11d7f3d2dcbb90c6b877045",
    "636e7c0c0fe4eb0f697545460c806910d2c355f1d253bc9d2452aaa549e27a1f",
    "ac7cf4ed77f322e8fa894b6a83810a34b361901751a6f5eb65a0326e07de7c12",
    "16ccce2d0193f958bb3850a833f7ae432b65bc5a53975c155aa4bcb4f7b2c4e5",
    "4df16efaf6ddea94e2c50b4cd1dfe06017e0e9d02900cffe1935e0491d77ffb4",
    "fdf85290fdd893d577b1131a610ef6a5c32b2ee0293617a37cbb08b847741c3b",
    "8017c25ca9052ca1079d8b78aebd47876d330a30f6a8c6d61dd1ab5589329de7",
    "14d19d61370f8149748c72f132f0fc99f34d766c6938597040d8f9e2bb522ff9",
    "9c63a344d6a2ae8aa8e51b7b90a4a806105fcbca31506c446151adfeceb51b91",
    "abfe43960977c87471cf9ad4074d30e10d6a7f03c63bd5d4317f68ff325ba3bd",
    "80bf4dc8b52a0ba031758022eb025cdd770b44d6d6cf0670f4e990b22347a7db",
    "848265e3e5eb72dfe8299ad7481a408322cac55786e52f633b2fb6b614eaed18",
    "d703dd84045a274ae8bfa73379661388d6991fe39b0d93debb41700b41f90a15",
    "c4d526250235ddcd6776fc77bc97e7a417ebcb31600d01e57f32162a8560cacc",
    "7e27a096d37a1a86952ec71bd89a3e9a30a2a26162984d7740f81193e8238e61",
    "f6b5b984d4d3dfa033c1bb7e4f0037febf406d91c0dccf32acf423cfa1e70710",
    "10d3f270121b493ce85054ef58bada42310138fe081adb04e2bd901f2f13458b",
    "3d6758158197107c14ebb193230cd1157380aa79cae1374a7c1e5bbcb80ee23e",
    "06ebfde206bfb0fcbc0edc4ebec309661bdd908d532eb0c6adc38b7ca7331dce",
    "8dfce39ab71e7c32d318d136b6100671a1ae6a6600e3899f31f0eed19e3417d1",
    "34b90c9058f8632c798d4490da4987307cba922d61c39805d072b589bd52fdf1",
    "e86215c2d54e6670e07383a27bbffb5addf47d66aa85a0c6f9f32e59d85a44dd",
    "5d3b22dc2be80919b490437ae4f36a0ae55edf1d0b5cb4e9a3ecabee93dfc6e3",
    "8d209d0fa6536d27a5d6fbb17641cde27525d61093f1b28072d111b2b4ae5f89",
    "d5974ee12e5cf7d5da4d6a31123041f33e61407e76cffcdcfd7e19ba58cf4b53",
    "6f4c4938ae79324dc402894b44faf8afbab35282ab659d13c93f70412e85cb19",
    "9a37ddec600545473cfb5a05e08d0b209973b2172b4d21fb69745a262ccde96b",
    "a18b2faa745b6fe189cf772a9f84cbfc",
);
```

**判据**

- 上面的用例全绿；
- `rurge-net` 的 `Cargo.toml` 不动，`Cargo.lock` 的包数不变；
- 门禁绿。

---

## Task 2：所有端口上的 QUIC 与 STUN 标签（引擎）

**改哪里**（设计第 4.4、4.5、5 节；M5-D2、D21；P6）

1. `crates/rurge-engine/src/sniff.rs`
   - 去掉 `is_quic_initial`、`QUIC_V2` 与它们的用例（`mod quic_tests`）。`parse_sni` 不动。
   - 文件头的注释随之改。
   - 加：

   ```rust
   /// Whether `datagram` is a STUN message (RFC 5389 / 8489 §5, §6): at
   /// least the 20 bytes of the header, the two top bits of the first byte
   /// clear, the magic cookie 0x2112A442 in bytes 4–7, and a message length
   /// (bytes 2–3) that is a multiple of 4 and is exactly what follows the
   /// header — over UDP one datagram is one message. Classic STUN (RFC 3489,
   /// no cookie) and TURN ChannelData are not recognised.
   pub fn is_stun(datagram: &[u8]) -> bool
   ```

2. `crates/rurge-engine/src/udp.rs`：定协议标签的那一行（今天 `asked.port == 443 && crate::sniff::is_quic_initial(…)`）不再看端口：
   - `rurge_net::quic::initial::is_client_initial(第一个数据报)` → `ProtocolKind::Quic`；
   - 否则 `crate::sniff::is_stun(第一个数据报)` → `ProtocolKind::Stun`；
   - 否则 `ProtocolKind::Udp`。
   - 两种判据互斥，次序无所谓。`silence_tells` 不改。附近的注释随之改。

3. `crates/rurge-engine/tests/common/mod.rs`
   - `quic_initial()` 改成返回真的 Initial：`rurge_net::quic::testing::client_initial(1, &[0x83, 0x94, 0xc8, 0xf0, 0x3e, 0x51, 0x57, 0x08])`。文档注释随之改。
   - 旧的假包改名 `lookalike_quic_initial()`。文档注释："looks like an Initial to a glance at its header; no key opens it"。
   - 用到 `quic_initial()` 的五处（`tests/udp.rs` 的 `block_quic_drops_quic_and_nothing_else`、`per_policy_block_quic_follows_the_terminal_policy`、`protocol_rules_see_udp_and_quic` 与约第 1194 行那一条，`tests/tun.rs` 的 `a_blocked_quic_flow_answers_with_icmp`）照旧调用它，期望不变。

**用例**

- `sniff.rs` 的单元用例：
  - `stun_messages_are_recognised`：
    - Binding 请求，消息长度 0：`00 01 00 00 21 12 a4 42` 加 12 个字节的事务号；
    - 带一个属性的 Binding 请求：消息长度 8，属性 SOFTWARE（`0x8022`）、长度 4、值 4 个字节。
  - `what_is_not_stun_is_not`，逐条：
    - 魔数错一位；
    - 消息长度 6、后面跟 6 个字节（不是 4 的倍数）；
    - 消息长度 8、后面只跟 4 个字节（与数据报对不上）；
    - 首字节 `0x80`（RTP）；
    - TURN ChannelData（首字节 `0x40`）；
    - 19 个字节；
    - 空的数据报。
- `crates/rurge-engine/tests/udp.rs`（经 SOCKS5 UDP，照这个文件已有用例的写法）：
  - `quic_on_any_port_is_quic`：`PROTOCOL,QUIC,REJECT` 拒掉端口 8443 上的真 Initial；记录的 `protocol` 是 `Quic`。
  - `block_quic_blocks_quic_on_any_port`：`block-quic = all`，端口 8443 上的真 Initial 的记录写 `QUIC blocked`。
  - `a_lookalike_of_a_quic_initial_is_plain_udp`：`block-quic = all`，端口 443 上的 `lookalike_quic_initial()` 不被挡，记录的 `protocol` 是 `Udp`。目标照 `block_quic_drops_quic_and_nothing_else` 里 `alt.test` 的写法。
  - `stun_flows_are_labelled_stun`：
    - 端口 9000 上的 Binding 请求被 `PROTOCOL,STUN,REJECT` 拒，记录的 `protocol` 是 `Stun`；
    - 只有 `PROTOCOL,UDP,REJECT-NO-DROP` 时同样被拒：规则引擎按传输层覆盖 STUN。

**判据**

- 新用例与五处原有的 QUIC 用例全绿；
- `git grep -n "is_quic_initial"` 在 `crates/` 下什么也没有；
- 门禁绿。

---

## Task 3：`block-quic` 的 `adaptive` 与 6.11 的 `auto`

**改哪里**（设计第 6.2 ～ 6.4 节；M5-D5、D6；P7、P8）

1. **配置**：`crates/rurge-config/src/spec/common.rs` 与 `spec/mod.rs`
   - 新类型：

   ```rust
   /// `block-quic` of a policy (manual: Policies › Parameters, 6.11).
   #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
   pub enum BlockQuic {
       #[default]
       Auto,
       On,
       Off,
       Adaptive,
   }
   ```

   - 读法：`const BLOCK_QUIC: [(&str, BlockQuic); 6]`，依次是 `auto`、`on`、`off`、`adaptive`、`true`（`On`）、`false`（`Off`）。`r.choice("block-quic", &BLOCK_QUIC)` 换掉今天的 `&TRISTATES`。
   - `CommonOpts.block_quic` 的类型换成 `BlockQuic`。`ecn` 仍用 `Tristate`。
   - `spec/mod.rs` 导出 `BlockQuic`；`CommonOpts` 的 `Debug` 与第 302 行附近用例里的字面量随之改。

2. **判定**：`crates/rurge-engine/src/udp.rs`
   - `quic_blocked` 换成：

   ```rust
   #[derive(Clone, Copy, Debug, PartialEq, Eq)]
   pub(crate) enum QuicVerdict {
       Allow,
       Block,
       /// Blocked or let through by the table of the engine (design 6.3).
       Adaptive,
   }

   pub(crate) fn quic_verdict(
       global: BlockQuicGlobal,
       policy: BlockQuic,
       direct: bool,
       carries_udp: bool,
   ) -> QuicVerdict
   ```

   - 判定表见 P7，逐格照它。文档注释改成 6.11 的读法。
   - `open()` 里：
     - `direct` 照今天取；
     - `carries_udp` 是 `resolution.outbound.udp() != UdpSupport::Unsupported`（终端出站；在 `udp-policy-not-supported-behaviour` 换出站之前取）；
     - `Adaptive` 时问引擎的表：`engine.quic_adaptive().on_quic(&destination.host.to_string(), Instant::now())`；
     - 被挡的照今天写 `QUIC blocked`、`Rejected(Reject)`。

3. **自适应的表**：新文件 `crates/rurge-engine/src/quic_adaptive.rs`（`lib.rs` 加 `mod quic_adaptive;`）

   ```rust
   /// The hostnames the table remembers (manual: "up to the 200 most recently
   /// used hostnames").
   pub(crate) const REMEMBERED: usize = 200;
   /// How long after a blocked QUIC flow a TCP session to the same host still
   /// counts as its fallback.
   pub(crate) const WINDOW: Duration = Duration::from_secs(1);

   #[derive(Clone, Copy, Debug, PartialEq, Eq)]
   pub(crate) enum Adaptive {
       /// QUIC was blocked at this moment; waiting to see a TCP session.
       Trying(Instant),
       /// A TCP session came within the window: QUIC stays blocked.
       FellBack,
       /// None came: QUIC goes through from now on.
       Allowed,
   }

   pub(crate) struct QuicAdaptive { /* Mutex<…> */ }

   impl QuicAdaptive {
       pub(crate) fn new() -> QuicAdaptive;
       /// A QUIC flow to `host` that the verdict left to the table, at `now`:
       /// whether it is blocked.
       pub(crate) fn on_quic(&self, host: &str, now: Instant) -> bool;
       /// A client TCP session to `host`, at `now`.
       pub(crate) fn on_tcp(&self, host: &str, now: Instant);
       #[cfg(test)]
       pub(crate) fn state(&self, host: &str) -> Option<Adaptive>;
       #[cfg(test)]
       pub(crate) fn set(&self, host: &str, state: Adaptive);
   }
   ```

   - 状态与事件照设计第 6.3 节的表；"1 秒之内"是 `now.saturating_duration_since(t) <= WINDOW`。
   - 键：`host` 去掉末尾的点、转小写。`on_quic` 刷新"最近用到"，`on_tcp` 不刷新。
   - 第 201 个名字进来时，淘汰最久没用到的那一个（线性找）。
   - 状态落定时各 DEBUG 一行（P8 的原文）。
   - 锁不跨 `await`（这里没有 `await`）。

4. **接线**：`crates/rurge-engine/src/engine.rs`
   - `Engine` 加字段 `quic_adaptive: QuicAdaptive`，在 `Engine::new` 里建；加 `pub(crate) fn quic_adaptive(&self) -> &QuicAdaptive`。表跨重载保留，网络变化不清（`network_changed` 不碰它）。
   - `impl Dialer for Engine` 的 `dial`：在 `match turned`（`Unknown` 回失败）之后、`connect_session` 之前，`self.quic_adaptive.on_tcp(&handle.session().dst_host.to_string(), Instant::now())`（P8）。

**用例**

- `rurge-config`（`spec` 的用例里）：
  - `block_quic_reads_four_values_and_two_spellings`：`auto`、`on`、`off`、`adaptive`、`true`、`false` 各读成对的值；不写时是 `Auto`。
  - `an_unknown_block_quic_value_is_e0018`：`block-quic=maybe` 是 `E0018`，消息里列着 `adaptive`。
- `udp.rs` 的单元用例：`which_quic_flows_are_blocked` 改写成 `every_cell_of_the_quic_verdict`，逐格对 P7 的表（4 × 4 × 2 × 2 = 64 格）。
- `quic_adaptive.rs` 的单元用例（显式的时刻：`let t0 = Instant::now();`，之后都是 `t0 + Duration::from_millis(…)`，不睡）：
  - `a_first_quic_flow_is_blocked_and_tried`：`on_quic` 答 true，状态 `Trying(t0)`。
  - `tcp_within_the_window_keeps_quic_blocked`：`t0 + 1000ms` 的 `on_tcp` → `FellBack`；之后的 `on_quic` 答 true。
  - `tcp_after_the_window_allows_quic`：`t0 + 1001ms` 的 `on_tcp` → `Allowed`；之后的 `on_quic` 答 false。
  - `a_quiet_window_allows_the_next_quic_flow`：`t0 + 1000ms` 的 `on_quic` 答 true、状态仍是 `Trying(t0)`；`t0 + 1001ms` 的答 false、状态 `Allowed`。
  - `settled_states_stay`：`FellBack` 与 `Allowed` 之后的 `on_quic`、`on_tcp` 都不改状态。
  - `tcp_without_an_entry_records_nothing`。
  - `names_are_compared_without_case_or_a_final_dot`：`Example.COM.` 与 `example.com` 是同一项。
  - `the_201st_name_evicts_the_least_recently_used`：
    1. `a0` ～ `a199` 依次 `on_quic`；
    2. 再 `on_quic("a0")`，又 `on_tcp("a1")`；
    3. `on_quic("a200")` 之后：`a1` 没了（`on_tcp` 不算用到），`a0` 还在。
  - `settling_is_said_at_debug_with_the_name`：两行 DEBUG 照 P8 的原文、带名字。抓日志照 `tests/common/log_capture.rs` 的做法，在用例里就地装一个只管本线程的订阅者（`tracing-subscriber` 是 dev 依赖）。
- `engine.rs` 的单元用例 `a_client_tcp_session_tells_the_table`：
  - 照 `dns_pipeline.rs` 用例的写法（`Runtime::build`）建一个引擎，Profile 的规则是 `FINAL,REJECT`：不拨任何东西。
  - `engine.quic_adaptive().set("x.test", Adaptive::Trying(Instant::now() + Duration::from_secs(3600)))`：一个小时之内都"在窗口里"，与机器快慢无关。
  - 经 `Dialer::dial` 拨一个到 `x.test:80` 的 SOCKS5 会话（被规则拒）；之后状态是 `FellBack`。
- `crates/rurge-engine/tests/udp.rs`：
  - `per_policy_block_quic_follows_the_terminal_policy` 的期望不变（载 UDP 的代理上 `auto` 自适应，第一条照样被挡）；文档注释改成 6.11 的读法。
  - `auto_blocks_quic_through_a_proxy_that_carries_no_udp`：`NoUdp = socks5, 127.0.0.1, {port}`（不写 `udp-relay`），真 Initial 的记录写 `QUIC blocked`，不是 `policy does not support UDP`。
  - `adaptive_on_a_direct_alias_blocks_the_first_quic_flow`：`[Proxy] D = direct, block-quic=adaptive`，`DOMAIN,d.test,D`；第一条 QUIC 的记录写 `QUIC blocked`。

**判据**

- 上面的用例全绿；
- 原有的 `block-quic` 用例全绿（`per_policy_…` 只改了注释）；
- `git grep -n "quic_blocked"` 在 `crates/` 下什么也没有；
- 门禁绿。

---

## Task 4：`[IP Rewrite]` 的解析（配置）

**改哪里**（设计第 8.1 节；M5-D9；P9）

1. 新文件 `crates/rurge-config/src/ip_rewrite.rs`（`lib.rs` 加 `pub mod ip_rewrite;`，照 `lib.rs` 的写法重新导出两个类型）：

   ```rust
   /// What `[IP Rewrite]` does with a packet to a matching address.
   #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
   pub enum IpRewriteAction {
       Reflect,
       Reject,
       Drop,
   }

   /// `[IP Rewrite]` (manual: Features › IP Rewrite): the lines as written,
   /// kept by family — an address can match only lines of its own family, so
   /// "the first line from the top" holds within one family.
   #[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
   pub struct IpRewrite { /* v4: Vec<(Ipv4Net, IpRewriteAction)>, v6: Vec<(Ipv6Net, IpRewriteAction)> */ }

   impl IpRewrite {
       /// The action of the first line of `dst`'s family whose prefix holds it.
       pub fn action(&self, dst: IpAddr) -> Option<IpRewriteAction>;
       pub fn is_empty(&self) -> bool;
   }

   /// Reads the section's active lines in order: `<address or CIDR> = <action>`.
   pub fn parse<'a>(
       entries: impl IntoIterator<Item = &'a Entry>,
       diags: &mut Diagnostics,
   ) -> IpRewrite
   ```

   - 一行用 `split_definition` 拆成左右两边。
   - 左边照 `IP-CIDR` 规则的读法（V6）：先读成 `Ipv4Net` / `Ipv6Net`（带主机位的照收，按前缀匹配），读不出再读成单个地址（`/32`、`/128`）；前缀长度越界是读不出。
   - 右边去掉首尾空白之后，不分大小写地是 `reflect`、`reject`、`drop` 之一。
   - 写坏的行（没有 `=`、左边读不出、右边不是这三个词）：一条 `W0012`（`codes::W_INVALID_VALUE`），位置是那一行的 span；这一行跳过，其余照常。消息不引用行的内容：

     ```
     [IP Rewrite]: a line that is not `<address or CIDR> = reflect | reject | drop` is ignored
     ```

2. `crates/rurge-config/src/config.rs`：
   - `Config` 加字段 `pub ip_rewrite: IpRewrite`，放在 `ssid_settings` 之后；
   - 照读 `[SSID Setting]` 的写法读 `profile.section("IP Rewrite")` 的有效行；
   - 第 956 行附近的字面量随之加。
3. `crates/rurge-config/src/deferred.rs`：`DEFERRED` 去掉 `"IP Rewrite"`。`text/mod.rs` 的 `ORDERED_SECTIONS` 里照旧有它，节不是未知的节。
4. `redact.rs` 与 `value.rs` 不动（这一节没有凭据，也不新加一行的拆法）。

**用例**（`config.rs` 的用例里）

- 删掉 `ip_rewrite_is_a_known_section_kept_for_later`，加：
  - `ip_rewrite_is_read_in_order`：
    - 两种写法的节头 `[IP Rewrite]`、`[ip rewrite]`，各三行 `10.0.0.1 = reject`、`10.0.0.0/8 = drop`、`fd00::/8 = reflect`；
    - 没有错误，没有 `W0016`、`W0001`、`W0012`，没有未知的节，`config.deferred.get("IP Rewrite")` 是 `None`；
    - `action(10.0.0.1)` 是 `Reject`、`action(10.9.9.9)` 是 `Drop`、`action(fd00::1)` 是 `Reflect`、`action(192.0.2.1)` 是 `None`。
  - `ip_rewrite_actions_ignore_case`：`REFLECT`、`Drop`、`rEjEcT`。
  - `ip_rewrite_host_bits_are_read_as_ip_cidr_reads_them`：`10.1.2.3/8 = drop` 命中 `10.200.0.1`；`2001:db8::1/32 = drop` 命中 `2001:db8:ffff::1`。
  - `a_single_address_is_a_slash_32_or_a_slash_128`：`192.0.2.1` 不命中 `192.0.2.2`；`2001:db8::1` 不命中 `2001:db8::2`。
  - `bad_ip_rewrite_lines_are_w0012_and_skipped`：逐行各一条 `W0012`，位置在那一行，消息里不含那一行的文字；同一节里的好行照样读进表：
    - `10.0.0.1 drop`
    - `10.0.0.300 = drop`
    - `10.0.0.0/33 = drop`
    - `fd00::/129 = drop`
    - `10.0.0.1 = bounce`
    - `10.0.0.1 = drop, now`
    - `= drop`
    - `10.0.0.1 =`
  - `the_first_line_of_its_family_decides`：`10.0.0.1 = reflect` 在 `10.0.0.0/8 = drop` 之前时，`10.0.0.1` 是 `Reflect`、`10.0.0.2` 是 `Drop`；两行对调之后 `10.0.0.1` 是 `Drop`。
  - `no_section_is_an_empty_table`：`is_empty()`；`action` 一律 `None`。

**判据**

- 上面的用例全绿；
- `rurge-config` 的语料库快照没有变化；有变化时停下报告，不自己接受；
- 门禁绿。

---

## Task 5：`[IP Rewrite]` 在虚拟网卡上（`rurge-tun`、引擎、API）

**改哪里**（设计第 8.2 ～ 8.9 节；M5-D10 ～ D14；P10 ～ P12、P18）

1. **设置**：`crates/rurge-tun/src/hooks.rs`
   - `TunSettings` 加：

   ```rust
   /// `[IP Rewrite]` of this generation: asked with the destination of each
   /// packet whose IP header passed its own checks and that is no martian
   /// (design 8.3). The receive task reads it once for each batch.
   pub ip_rewrite: Arc<IpRewrite>,
   ```

   - `TunSettings` 的字面量都随之加：`crates/rurge-engine/src/runtime.rs`（取 `Arc::new(config.ip_rewrite.clone())`）、`rurge-tun` 的 `src/tests/dns/mod.rs`、`src/tests/dns/udp.rs`、`src/tests/support.rs` 的 `settings`，以及编译器指出的其余几处。用例里一律给空表（`Arc::default()`）。

2. **包的去向**：`crates/rurge-tun/src/packet.rs`（P10、P11）
   - 新的原因 `Reason::IpRewrite`，`Display` 是 `ip_rewrite`；`impl Reason` 加 `is_wanted()`，只对它成立。
   - `classify` 的次序改成：版本 → 首部 → 怪地址 → `[IP Rewrite]` → 分片 → 传输层（设计第 8.3 节的表）。
     - 今天 `read_ipv4` / `read_ipv6` 里分片在怪地址之前，两步对调。
     - `segment()` 与 `rst_for()` 不问 `[IP Rewrite]`。
   - 新的形状：

   ```rust
   pub(crate) fn classify(
       packet: &[u8],
       ipv6: bool,
       rewrite: &IpRewrite,
   ) -> Result<Classified, Dropped>

   pub(crate) enum Classified {
       /// Every check passed, as today.
       Passed(Parsed),
       /// `[IP Rewrite]` has a line for the destination: what is past the IP
       /// header was not read.
       Rewrite(Hit),
   }

   #[derive(Clone, Copy, Debug, PartialEq, Eq)]
   pub(crate) struct Hit {
       pub(crate) action: IpRewriteAction,
       pub(crate) src: IpAddr,
       pub(crate) dst: IpAddr,
       /// Where the IP packet ends, as `Parsed::len`.
       pub(crate) len: usize,
       /// The IP header's length: IPv4's IHL × 4, IPv6's 40.
       pub(crate) header_len: usize,
   }

   /// What a packet `reject` matched is answered with (design 8.5).
   #[derive(Clone, Copy, Debug, PartialEq, Eq)]
   pub(crate) enum Refusal {
       Nothing,
       Reset,
       Prohibited,
   }

   pub(crate) fn refusal(packet: &[u8]) -> Refusal

   /// `classify` with an empty `[IP Rewrite]`, for the tests of the checks.
   #[cfg(test)]
   pub(crate) fn checks(packet: &[u8], ipv6: bool) -> Result<Parsed, Dropped>
   ```

   - `classify` 的文档注释随新次序改。
   - 用例里的 `classify(` 换成 `checks(`：`src/tests/filter.rs` 十一处、`src/tests/fixture/test_packets.rs` 两处。

3. **收包的任务**：`crates/rurge-tun/src/dispatch.rs`

   ```rust
   pub(crate) enum Routed {
       /// A UDP datagram or an echo request, with what `classify` read of it.
       Local(Parsed, Vec<u8>),
       /// A packet `[IP Rewrite]` matched, cut to its length.
       Rewrite(Hit, Vec<u8>),
   }

   pub(crate) fn route(
       packet: Vec<u8>,
       ipv6: bool,
       rewrite: &IpRewrite,
       shards: &[mpsc::Sender<Ingress>],
       stats: &Stats,
       queue_bytes: &Arc<QueuedBytes>,
       shard_key: &RandomState,
   ) -> Option<Routed>
   ```

   - `run`：每次 `device.recv` 之后读一次 `ctx.hooks.settings()`，这一批都用它的 `ip_rewrite`；`Routed::Rewrite` 交给 `rewrite::handle`。
   - `route` 的调用方随之改：`src/tests/dispatch.rs` 一处，`src/tests/filter.rs` 四处（给空表，匹配 `Routed::Local`）。

4. **三种动作**：新文件 `crates/rurge-tun/src/rewrite.rs`

   ```rust
   /// Does what `[IP Rewrite]` asks with `packet` (design 8.4 – 8.6). Never
   /// waits: what it writes is offered to the writer's queue, and what finds
   /// the queue full is dropped and counted `queue_full`.
   pub(crate) fn handle(ctx: &Context, hit: &Hit, packet: Vec<u8>)
   ```

   - `reflect`：对调两个地址（IPv4 第 12..16 与 16..20 字节，IPv6 第 8..24 与 24..40 字节），其余字节一个不动，交给写任务，不等（这时只有一条队列：`ctx.writer.try_send`；任务 6 把它归到普通的一类）；写任务收下了才计 `reflected`，队列满了计 `queue_full`。
   - `reject`：
     - 命中的包计 `ip_rewrite`（`Stats::drop`，两个地址是包的）；
     - 再按 `refusal(&packet)`：
       - `Reset`：先问 `ctx.admission.allow_reset(Instant::now())`；准了就把 `packet::rst_for(&packet)` 交给写任务；不准就 `ctx.admission.count(Reason::RstLimited, hit.src, hit.dst)`；
       - `Prohibited`：先问 `ctx.icmp_limit`（照 `UdpClient::refused` 的写法）；准了就 `icmp::refuse(ctx, hit.dst, hit.src, &packet[..quote])`，`quote` 是 `min(hit.len, hit.header_len + 8)`；不准就计 `icmp_limited`，两个地址照 `UdpClient::refused` 的写法（消息的来处与去处）；
       - `Nothing`：什么也不回。
   - `drop`：计 `ip_rewrite`。

5. **`Context`**：`crates/rurge-tun/src/context.rs` 加 `pub(crate) admission: Arc<Admission>`；`Tun::start`（`lib.rs`）建 `Context` 时给各片共用的那一个。

6. **计数与日志**：`crates/rurge-tun/src/stats.rs` 与 `lib.rs`（P12）
   - `Stats` 加 `ip_rewrite`、`reflected` 两个计数，加 `pub(crate) fn reflected(&self)`。
   - `Stats::drop` 对 `is_wanted()` 的原因从不 WARN，每次都 DEBUG（`tun: packet dropped`，带原因与两个地址）。
   - 公开的 `Counters` 加两个字段，文档注释照它的写法：
     - `ip_rewrite`：Packets that `[IP Rewrite]` rejected or dropped: what the profile asked for, so never warned about.
     - `reflected`：Packets that `[IP Rewrite]` sent back with their two addresses swapped. No drop, and not logged.

7. **引擎、API 与可执行文件**
   - `crates/rurge-api/src/routes/features.rs` 的 `enhanced_json`：`dropped` 里加 `"ipRewrite"`，顶层加 `"reflected"`（与 `stackPanics` 并列）。
   - `crates/rurge-api/tests/api.rs` 写全 `TunCounters` 字段的两处随之加，期望的 JSON 加这两个键。
   - `crates/rurge/tests/run/enhanced_mode.rs`：`dropped.len()` 从 10 改成 11（"eleven reasons"），并断言 `body["reflected"] == 0`。

**用例**

`rurge-tun`：新文件 `src/tests/rewrite.rs`（`src/tests/mod.rs` 加 `mod rewrite;`），内存设备上（`rig_extras` 加 `FakeHooks`，换代用 `FakeHooks::set`）。地址用 `203.0.113.0/24` 与 `2001:db8::/32`。

- `reflect_sends_a_syn_back_with_the_addresses_swapped`：
  - 写回的包等于原包对调两个地址；
  - IPv4 首部校验和与 TCP 校验和都对（smoltcp 的 `verify_checksum`）；
  - `reflected == 1`；拨号器什么也没收到。
- `reflect_sends_udp_echo_ipv6_and_fragments_back`：UDP 数据报、回显请求、IPv6 的 UDP（`ipv6` 开着）、一个 IPv4 分片（`with_fragment_field`），各自写回、地址对调、其余字节不变；`reflected == 4`。
- `reject_answers_a_syn_with_a_reset`：RST 来自原包的目的端，确认号是 SYN 的序号加一；`ip_rewrite == 1`。
- `reject_answers_the_rest_with_prohibited`：
  - UDP → ICMPv4 3/13，来自原包的目的地址，引用原包的 IP 首部与其后 8 个字节；
  - 带 ACK 的 TCP 段、回显请求、TCP 的首片 → 同样；
  - IPv6 的 UDP → ICMPv6 1/1。
- `reject_answers_no_error_with_an_error`：发往 `reject` 地址的 ICMPv4 类型 3、ICMPv6 类型 1，什么也不写回；`ip_rewrite` 照计。
- `reject_answers_no_reset_with_a_reset`：RST 段什么也不写回。
- `reject_answers_no_later_fragment`：偏移不为 0 的 IPv4 分片、偏移不为 0 的 IPv6 分片（带分片首部），什么也不写回。
- `reject_answers_nothing_to_an_unreadable_tcp_header`：TCP 首部被截短的不分片的包，什么也不写回。
- `reject_resets_share_the_tuns_limit`：同一个窗口里 101 个 SYN → 100 个 RST，`rst_limited == 1`。
- `reject_messages_share_the_icmp_limit`：同一个窗口里 101 个 UDP → 100 个 ICMP，`icmp_limited == 1`。
- `drop_drops_and_counts`：什么也不写回、不拨号；`ip_rewrite == 1`。
- `the_first_line_decides`：`10.0.0.1 = reflect` 在 `10.0.0.0/8 = drop` 之前：`10.0.0.1` 被送回，`10.0.0.2` 被丢。
- `ipv6_lines_never_match_while_ipv6_is_off`：`ipv6` 关着，表里有 `2001:db8::/32 = drop`，IPv6 的包计 `unsupported`，不计 `ip_rewrite`。
- `a_fake_address_is_matched_as_an_address`：表里有 `198.18.0.0/15 = drop`，发往一个名字的 fake 地址的 UDP 计 `ip_rewrite`；不开会话，不问名字。
- `martians_come_before_ip_rewrite`：表里有 `0.0.0.0/0 = drop`，发往 `224.0.0.1` 的包计 `martian`。
- `fragments_come_after_ip_rewrite`：命中的分片照动作办；不命中的照旧计 `fragment`。
- `a_new_generation_brings_its_table_with_the_next_batch`：`FakeHooks::set` 换一张表之后，下一个包按新表。
- `ip_rewrite_drops_are_said_at_debug_never_warned`（`LogCapture`）：两个被丢的包，没有 WARN 行，有两行带 `ip_rewrite` 的 DEBUG。
- `reflected_packets_are_not_logged`（`LogCapture`）。
- 原有的用例里，若有断言"既是分片又是怪地址的包计 `fragment`"的，改成计 `martian`（设计第 8.3 节），提交说明里说。

引擎：`crates/rurge-engine/tests/tun.rs` 加 `ip_rewrite_in_the_profile_acts_on_the_tun_and_a_reload_replaces_it`。

1. `Profile.sections` 是 `[IP Rewrite]\n203.0.113.9 = drop\n`，规则用 `guarded("")`（漏过去的包会被规则拒，不会被拨）。
2. 应用发一个到 `203.0.113.9:9000` 的 UDP：等到 `rig.tun.counters().ip_rewrite == 1`；没有 UDP 的请求记录。
3. 照 `a_ping_to_a_fake_address_goes_to_the_names_address` 的做法 `swap_runtime`，新的 `[IP Rewrite]` 是 `203.0.113.9 = reject`。
4. 再发一个：应用收到来自 `203.0.113.9` 的"管理上禁止"（`prohibited_from`）；`ip_rewrite == 2`；仍然没有请求记录。

另在 `the_tuns_settings_carry_the_dns_settings_of_the_profile` 里加一句：没有这一节时 `settings.ip_rewrite.is_empty()`。

**判据**

- 上面的用例全绿；
- 原有的 `rurge-tun` 用例全绿（只有上面点了名的那一处改期望）；
- 门禁绿。

---

## Task 6：`udp-priority`（`rurge-tun`）

**改哪里**（设计第 7 节；M5-D8；P13、P18）

1. **设置**
   - `TunSettings` 加 `pub udp_priority: bool`。文档注释：`udp-priority`: the UDP datagrams the TUN writes back go ahead of the rest at the writer (design 7.2); nothing else is reordered。
   - `crates/rurge-engine/src/runtime.rs` 取 `config.general.udp_priority`；用例里的字面量给 `true`（缺省）。
2. **写任务**：`crates/rurge-tun/src/writer.rs`

   ```rust
   /// Which of the writer's two queues a batch is for (design 7.2).
   #[derive(Clone, Copy, Debug, PartialEq, Eq)]
   pub(crate) enum Class {
       /// UDP datagrams written back: a session's replies (their fragments
       /// too), the responder's answers over UDP.
       Udp,
       /// Everything else: TCP segments and resets, ICMP messages, echo
       /// replies, what `[IP Rewrite]` sends back.
       Other,
   }

   /// The sending ends of the writer's two queues.
   #[derive(Clone)]
   pub(crate) struct Outlet {
       pub(crate) priority: mpsc::Sender<Vec<Vec<u8>>>,
       pub(crate) normal: mpsc::Sender<Vec<Vec<u8>>>,
   }

   pub(crate) async fn run(
       device: Arc<dyn Device>,
       priority: mpsc::Receiver<Vec<Vec<u8>>>,
       normal: mpsc::Receiver<Vec<Vec<u8>>>,
       limit: usize,
       life: Arc<Life>,
   )
   ```

   - 每一轮：等到任一条开着的队列有东西；然后两条都取（各至多 `limit` 批，不等）；优先队列的包排在这一次设备写的最前面。
   - 一条队列关了（发送端都没了、里面也空了）之后不再等它，不忙转；两条都关了才结束。
   - 停止之后的 `STOP_GRACE` 照旧。
   - 文件头的注释随之改。
3. **`Context`**：`crates/rurge-tun/src/context.rs`
   - `writer` 的类型换成 `Outlet`；加两个方法：

   ```rust
   /// Sends `batch` to the writer's queue for `class` in this generation,
   /// waiting for room: the priority queue for `Class::Udp` while
   /// `udp_priority` is on, the normal one otherwise.
   pub(crate) async fn send(&self, class: Class, batch: Vec<Vec<u8>>)
       -> Result<(), mpsc::error::SendError<Vec<Vec<u8>>>>;
   /// The same without waiting.
   pub(crate) fn try_send(&self, class: Class, batch: Vec<Vec<u8>>)
       -> Result<(), mpsc::error::TrySendError<Vec<Vec<u8>>>>;
   ```

   - 每次发送读一次 `self.hooks.settings().udp_priority`。
4. **`Tun::start`**（`lib.rs`）：建两条 `mpsc::channel(writer_queue)`；各片照旧只拿普通队列的发送端；写任务拿两个接收端。
5. **发送者**（V7），照下表改，等与不等的写法不变：

   | 发送者 | 位置 | 类 |
   | --- | --- | --- |
   | UDP 会话的回包 | `udp/client.rs`，约第 128 行，`send().await` | `Class::Udp` |
   | 应答器经 UDP 的应答 | `dns/udp.rs`，约第 100 行，`try_send` | `Class::Udp` |
   | `icmp::offer` | `icmp.rs`，约第 86 行，`try_send` | `Class::Other` |
   | 回显应答 | `icmp.rs`，约第 315 行，`send` | `Class::Other` |
   | `[IP Rewrite]` 写回的包（任务 5） | `rewrite.rs` | `Class::Other` |
   | 各片 | `tcp/shard.rs` | 普通队列的发送端，不经 `Context` |

**用例**

写任务：新文件 `src/tests/writer.rs`。用一个只记录每次 `send` 收到的包的设备；需要时用 `GatedDevice` 让写任务先停住。

- `priority_batches_go_first_in_the_next_write`：写任务起来之前，普通队列放 `[n1]`、`[n2]`、`[n3]`，优先队列放 `[p1]`；第一次设备写是 `[p1, n1, n2, n3]`。
- `one_queue_alone_keeps_the_order_of_arrival`：全放在普通队列里，次序不变。
- `a_full_normal_queue_leaves_room_in_the_priority_queue`：普通队列满着（写任务停在门上），优先队列的 `try_send` 照样成功。
- `the_writer_goes_on_with_one_queue_closed`：丢掉优先队列的全部发送端之后，再往普通队列放的两批都写出去；写任务还在。
- `the_writer_ends_when_both_queues_have_closed`：两条的发送端都丢掉，写任务 5 秒内结束。
- `the_stop_grace_holds_for_both_queues`：停止之后两条队列里的东西在 `STOP_GRACE` 之内照样写；照原有的宽限用例的写法。

发送的一方（`GatedDevice` 停住写任务，打开之后看第一次设备写的次序）：

- `udp_replies_go_ahead_of_tcp_when_udp_priority_is_on`：先让一条 TCP 连接有段排着，再让一个 UDP 会话的回包排着；打开之后 UDP 的包在前。
- `udp_replies_keep_their_turn_when_udp_priority_is_off`：同上，`udp_priority` 关着：先来先写。
- `responder_answers_are_udp_and_icmp_is_not`：开着时，应答器经 UDP 的应答进优先队列；"管理上禁止"与回显应答进普通队列。

原有的、把 `writer_queue` 设成 1 的用例（`src/tests/dns/udp.rs`、`src/tests/icmp/writer_queue.rs`、`src/tests/udp/writer_queue.rs`、`src/tests/dispatch.rs`）必须照旧绿。若哪一条是靠一类填满队列、再看另一类的发送找不到空位，给它的设置 `udp_priority: false`（等于今天的一条队列），提交说明里说。

**判据**

- 上面的用例全绿；
- 新的计时用例单独循环跑二十次都绿；
- 门禁绿。

---

## Task 7：内部 DNS 会话带上主机名

**改哪里**（设计第 9 节；M5-D15、D16；P14 ～ P16）

1. **`rurge-net`**：`crates/rurge-net/src/connector.rs` 的 `Connector` 加两个带缺省实现的方法：

   ```rust
   /// `target` is an address `name` resolved to (M5 design 9.2). A connector
   /// that tells its sessions by name (the engine's DNS pipeline) takes the
   /// name along; any other dials `target` as `connect` would.
   fn connect_resolved<'a>(
       &'a self,
       name: &'a str,
       target: &'a Target,
       opts: &'a ConnectOpts,
   ) -> BoxFuture<'a, io::Result<BoxedStream>> {
       let _ = name;
       self.connect(target, opts)
   }

   /// The same for a UDP flow.
   fn connect_udp_resolved<'a>(
       &'a self,
       name: &'a str,
       target: &'a Target,
       opts: &'a ConnectOpts,
   ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
       let _ = name;
       self.connect_udp(target, opts)
   }
   ```

   - `ConnectOpts` 不加字段。别的连接器一个不改。
   - `crates/rurge-net/src/quic/mod.rs` 的 `Path` 加 `open_resolved`（P15）：

   ```rust
   /// `open` with no port hopping and quinn's own ECN, for a server whose
   /// address `server` the name `name` resolved to: over a connector that is
   /// not direct the carrier is asked of `connect_udp_resolved`.
   pub async fn open_resolved(
       connector: &dyn Connector,
       name: &str,
       server: &Target,
       opts: &ConnectOpts,
   ) -> io::Result<Path>
   ```

   与 `open` 共用一个内部函数，不复制分支。
2. **`rurge-dns`**
   - `crates/rurge-dns/src/bootstrap.rs` 的 `BootstrapConnector`：目标是名字时，解析出的每个地址都调内层的 `connect_resolved` / `connect_udp_resolved`，名字就是目标里的那个名字（`HostName::Domain` 的字符串），原样传。目标本来就是地址时，照旧调 `connect` / `connect_udp`。
   - `crates/rurge-dns/src/upstream/session.rs` 的 `Dialer::connect_to`：`self.host` 是名字时用 `Path::open_resolved(…, &self.host, server, &opts)`，是地址时照旧 `Path::open`。
3. **引擎**（P14）
   - `crates/rurge-engine/src/dns_pipeline.rs`：`Face` 实现 `connect_resolved` / `connect_udp_resolved`。会话的目标是 `HostName::Domain(name)`、端口是 `target` 的；地址是 `target` 的地址，经 `dial_internal` / `dial_internal_udp` 新加的参数带给引擎。目标是名字而不带地址的（`connect_as` 今天的回落）照旧不进流水线。
   - `crates/rurge-engine/src/engine.rs`：

   ```rust
   pub async fn dial_internal(
       &self,
       session: SessionInfo,
       address: Option<IpAddr>,
       fallback: …, // as today
   ) -> …

   pub async fn dial_internal_udp(
       &self,
       session: SessionInfo,
       address: Option<IpAddr>,
       fallback: …, // as today
   ) -> …

   /// `choose_policy` with the IP rules asking `resolver`.
   pub(crate) async fn choose_policy_with(
       &self,
       rt: &Runtime,
       registry: &PolicyRegistry,
       handle: &SessionHandle,
       resolver: &dyn LazyResolver,
   ) -> Chosen
   ```

   - `choose_policy` 委托给 `choose_policy_with`，传 `rt.stack.resolver`。
   - 带地址的内部会话：
     - 规则用 `rurge_rules::engine::FixedResolve`（`ResolvedAddrs` 里只有这一个地址），**不经引擎的解析器**；
     - 拨号的目标一律是 `Target::new(HostName::Ip(address), port)`：DIRECT、代理、两条防环的回落（`bypass_to_direct`）都是它。
   - `SelectCtx.host` 是会话的目标（名字）。
   - 其余调用方（`tests/process.rs` 约第 484 行）传 `None`。

**用例**

- `rurge-net`：`connector.rs` 的单元用例 `the_resolved_calls_default_to_the_plain_ones`：一个只实现 `connect` 与 `connect_udp` 的连接器，经 `connect_resolved` / `connect_udp_resolved` 调到它们，目标不变。
- `rurge-dns`：
  - `bootstrap.rs`：`connector_dials_the_resolved_address_and_names_it`（照已有的 `connector_dials_the_resolved_address`）。内层是记录每次调用的连接器：
    - 目标是 `dns.test` 时，它收到 `connect_resolved("dns.test", 127.0.0.1:<端口>)`；
    - 目标本来就是地址时，它收到 `connect`。
  - `resolver.rs`：`a_named_quic_server_reaches_its_connector_with_its_name`（照 `a_named_server_reaches_its_connector_as_the_address_the_bootstrap_gave` 的写法，暂停的时钟、内存里的引导）：`quic://dns.example:4` 与 `h3://dns.example:5/dns-query` 的连接器收到 `connect_udp_resolved("dns.example", 127.0.0.1:端口)`。已有的那一条照旧绿：目标仍是地址。
- 引擎：`crates/rurge-engine/tests/pipeline.rs`（P16）
  - 公共部分：
    - 两个 `MockDns`：`boot` 作 `dns-server = 127.0.0.1:<boot>`，答 `dns-up.test → 127.0.0.1`；`up` 作 `encrypted-dns-server = tcp://dns-up.test:<up>`，答 `target.test → 127.0.0.1`；
    - `encrypted-dns-follow-outbound-mode = true`；
    - `[Proxy] Named = direct`；
    - 每次查找给 5 秒的时限。
  - `a_dns_session_matches_rules_by_the_servers_name`：
    - 规则依次是 `IP-CIDR,10.0.0.0/8,REJECT`（让规则先要一个地址）、`DOMAIN,dns-up.test,Named`、`PROTOCOL,DNS,REJECT`、`FINAL,DIRECT`；
    - 查 `target.test` 成功；
    - 内部的那条记录：`dst` 是 `dns-up.test:<up>`，`rule` 是 `DOMAIN,dns-up.test`，`policy` 的第一个是 `Named`，`remote_addr` 是 `127.0.0.1:<up>`，`dns_ms` 是 `None`；
    - `up.query_count("dns-up.test", A) == 0`：引擎的解析器没有拿上游的名字去问上游；`boot` 被问过 `dns-up.test`。
  - `an_ip_cidr_rule_still_matches_a_dns_session_by_its_address`：规则 `IP-CIDR,127.0.0.1/32,Named` 在前，记录的 `rule` 是它。
  - 原有的 `dns_upstream_follows_the_pipeline_as_an_internal_session` 与两条防环的用例（`a_dns_session_bypasses_a_proxy_configured_by_host_name` 等）照旧绿。

**判据**

- 上面的用例全绿；
- `crates/rurge/tests/sockets.rs` 绿（没有新的建套接字或解析的地方）；
- 门禁绿。

---

## Task 8：手册 6.11 的 WireGuard 写法能加载（配置）

**改哪里**（设计第 10.2 节；M5-D17；P17）

1. `crates/rurge-config/src/wireguard.rs`
   - `parse_section` 先扫一遍这一节的有效行，看有没有 `listen-port`（键不分大小写），再逐行读。
   - `Report` 加：

   ```rust
   /// What is read and has no effect in this version: `W0029`.
   fn inert(&mut self, span: &Span, message: impl fmt::Display)
   ```

   消息与 `error` / `warn` 一样以 `[WireGuard <名字>]: ` 开头。
   - 新认识的两个键（文字照 P17）：
     - `listen-port`：什么值都是一条 `W0029`；
     - `accept-incoming`：值读成 `false` 时什么也不说，其余（`true`、读不出的值）一条 `W0029`。
   - `peer(…)` 多一个参数 `listening: bool`。不带 `endpoint` 的 peer：
     - `listening` 时一条 `W0029`（文字照 P17），这个 peer 不进节，不算错误；
     - 不 `listening` 时照旧 `E0023`（`peer {n}: `endpoint` is required`）。
   - "at least one `peer` is required" 照旧按写了几个 peer 数：写了、都被留在节外的，节照常加载，`peers` 是空的。
2. `crates/rurge-config/src/spec/mod.rs`
   - `NotImplemented` 加 `WireGuardNoPeers`：
     - `warning()`：`` `wireguard` policies whose section has no peer to dial are not supported in this version (it only lists peers that connect to it); such policies behave as REJECT ``；
     - `diagnostic()`：`W0007`；
     - `note()`：`wireguard (no peer to dial)`。
   - `PolicyKind::WireGuard` 一支：读出的节没有 peer 时，`not_implemented = Some(NotImplemented::WireGuardNoPeers)`。
   - 别处对 `NotImplemented` 的 `match` 随之加这一支（编译器会指出）。

**用例**

- `wireguard.rs` 的用例：
  - `the_manuals_server_example_loads`：手册 `policies/wireguard.html` 的 `[WireGuard home]` 例子——`private-key`、`self-ip = 10.8.0.1`、`listen-port = 51820`、两个只有 `public-key` 与 `allowed-ips`（`10.8.0.2/32`、`10.8.0.3/32`）的 peer——密钥换成本文件用例里的 `PUBLIC` / `PRIVATE`。节读得出，`peers` 是空的；诊断恰好是三条 `W0029`（`listen-port`、peer 1、peer 2），没有错误。
  - `peers_with_and_without_an_endpoint_mix_when_listening`：带 `endpoint` 的 peer 照常进节，不带的一条 `W0029`、留在节外。
  - `a_peer_without_an_endpoint_is_an_error_without_listen_port`：照旧 `E0023`，消息 `peer 1: `endpoint` is required`。
  - `accept_incoming_false_says_nothing_and_anything_else_is_w0029`：`false`、`true`、`maybe`。
  - 诊断的消息里不含密钥（照本文件已有用例的查法）。
- `spec` 的用例 `a_policy_on_a_section_without_peers_is_not_implemented`：`W0007`，`spec` 是 `None`，`not_implemented` 是 `Some(WireGuardNoPeers)`。
- 引擎的用例 `a_policy_on_a_section_without_peers_fails_at_once`（照 `tests/outbounds_snell.rs` 里 snell v6 那一条的写法）：
  - HTTP CONNECT 到一个规则指向这个策略的名字，5 秒内收到的不是 `200`；
  - 记录是 `Rejected("REJECT")`，`error` 是 `policy protocol not implemented: wireguard (no peer to dial)`。

**判据**

- 上面的用例全绿；
- 原有的 WireGuard 用例全绿（`what_a_section_must_have` 的期望不变）；
- 门禁绿。

---

## Task 9：基准重跑（设计第 12 节；M5-D20；P19）

**做什么**：任务 1 ～ 8 都提交之后，在开发机上各跑一次三个基准，照 `docs/testing.md` 的命令（release，忽略的用例，不作门禁；只有它们用 `-p`）。`[IP Rewrite]` 是空表（缺省），`udp-priority` 开着（缺省）。

```bash
TUN_BENCH_FAR=memory cargo test -p rurge-tun --release --lib -- --ignored --exact tests::bench::b1_single_stream_throughput --nocapture
TUN_BENCH_FAR=memory cargo test -p rurge-tun --release --lib -- --ignored --exact tests::bench::b2_ten_thousand_connections --nocapture
TUN_BENCH_FAR=memory cargo test -p rurge-tun --release --lib -- --ignored --exact tests::bench::b3_fake_answer_latency --nocapture
```

- 跑之前 `df -h /d`，剩余不足约 20 GiB 就停下报告（release 另编一份）。
- 读数各记一行进「执行期修正记录」：中位数、要的分位数、对着 NFR-01 的 70% 的线，与上一次的读数（P19）并列。照实写是一台机器上的一次运行。
- 读数低于线，或比上一次差出一半以上：不改代码，记下来，停下报告。B2 的保守读数归项目所有者，照旧（设计第 19 节）。

**判据**：修正记录里有三行读数。

---

## Task 10：文档收尾（设计第 15 节；M5-D18、D24）

**改哪里**（行号是 `cdd011d` 的，以内容为准）

1. `docs/surge-compatibility-matrix.md`
   - 文首的手册基线：改成 2026-10-28 的复查（83 页不变、4 页改过，对应 Mac 6.11.0 / iOS 5.24.0），指向复查报告。
   - L61 与 9.10 节：`[IP Rewrite]` 生效。逐项写 rurge 的做法：`W0012`、在包的去向里的位置、三种动作、计数、重载。
   - L176（`udp-priority`）：尽力而为，只在写回虚拟网卡时生效。
   - L177、L375（`block-quic`）：所有端口、以解密确认、少认的版本（gQUIC、草案 28 以前）、`adaptive` 与 6.11 的 `auto`、阻断之前已在的 TCP 不算。
   - L249（`PROTOCOL`）：QUIC 所有端口；STUN 做了；`DNS` 那半句照手册订正。
   - L502：`.local` 带作用域号的 IPv6，去向远期（M5-D19）。
   - L520：DNS 会话带名字；经代理时代理收到的是地址（M5-D16）。
   - L584：`auto-quic-block` 的阶段改成 4（M5-D7）。
   - L66、L356 与 `tailscale` 一行：6.11 的键（`listen-port`、`accept-incoming`；Tailscale 的三个键）。
   - 10.4 节：`GET /v1/policies?include_external=true`，阶段 6。
   - 附录的统计随改动的行。
2. `docs/api/phase3.md` 加 M5 一节：
   - `ipRewrite` 与 `reflected`；
   - `protocol` 的 `quic` / `stun` 不再限端口；
   - 内部 DNS 会话的目标是名字；
   - 两行新的 DEBUG（自适应）与 `ip_rewrite` 的 DEBUG。
3. `docs/testing.md`：M5 的用例过滤词；测试向量的出处；基准的读数（任务 9）。
4. `docs/acceptance/phase3-manual.md` 加 M5 一节，每项写清楚怎样做、看什么：
   - `[IP Rewrite]` 三种动作：`reflect` 用 `ping`（注意 Windows 防火墙缺省挡入站的回显请求）；`reject` 的 TCP 立刻被拒、UDP 收到 ICMP；`drop` 超时。地址要先被路由进虚拟网卡（手册：被 `tun-excluded-routes` 或局域网路由盖住的，写进 `tun-included-routes`）；
   - 443 以外端口上的 QUIC 被挡；
   - `PROTOCOL,STUN` 挡掉 WebRTC 的 STUN；
   - `curl --http3-only` 经载 UDP 的代理：第一次失败，1 秒之后成功（自适应）；
   - 按 DNS 服务器名写的规则在请求记录里生效；
   - `udp-priority` 不设手工项：看不出来，照实写一句。
5. `README.md` 与 `README_en.md`：状态、特性表、路线图，两份一致。
6. `docs/requirements.md`：第 7 节阶段 3 的状态一句；FR 的字面不改（总设计 Q8 归项目所有者）。
7. `docs/status.md`：阶段 3 下新的 M5 一节——做了什么、没做的与不要当成已经有的、以后的会话必须知道的事、去向写着项目所有者的；细节指到本计划的两张表。
8. `CLAUDE.md`：
   - 「现在在哪里」：M5 做完；下一步是 M6（阶段 3 的需求逐句对照代码、验收文档、手工验收）；推送与读 CI 归项目所有者；
   - 「工作流约定」里手册基线那一句改成 2026-10-28 的复查（M5-D18）。
9. 本计划的「延后事项」补上执行期新出现的。

**判据**

- 上面每一份都改了；
- `README.md` 与 `README_en.md` 的状态一致；
- 只改文档，门禁照旧绿（只跑 `cargo fmt --all --check` 与 `git diff --stat` 确认没有动代码也可以，记一行说明）。

---

## 计划期决定

| # | 事项 | 决定 |
| - | ---- | ---- |
| P1 | 引用的名字与行号 | 对着 `9ee270f` 核对过（下面各行）。行号会随前面任务的改动漂移，以名字为准；名字不同就照实改、记进修正记录，不改语义 |
| P2 | rustls 0.23.43 的接口（V1，读本机 cargo 注册表的源码） | `rustls::crypto::ring::cipher_suite::TLS13_AES_128_GCM_SHA256` 是 `SupportedCipherSuite`；`.tls13()` 给 `Option<&'static Tls13CipherSuite>`，再 `.quic_suite()` 给 `Option<rustls::quic::Suite>`。`Suite::keys(&self, client_dst_connection_id: &[u8], side: rustls::Side, version: rustls::quic::Version) -> Keys`；`Keys { local, remote }`：`Side::Server` 时 `remote` 是客户端的 Initial 密钥（识别用），`Side::Client` 时 `local` 是（构造器用）。`Version` 带 `#[non_exhaustive]`：`V1Draft`（草案 29 ～ 32 的 salt）、`V1`、`V2`。`DirectionalKeys { header: Box<dyn HeaderProtectionKey>, packet: Box<dyn PacketKey> }`。`HeaderProtectionKey::decrypt_in_place(&self, sample: &[u8], first: &mut u8, packet_number: &mut [u8])`：包号的长度取自去掉保护之后的首字节，传 4 个字节的切片可以、只解开那么长；`encrypt_in_place` 同形；`sample_len()` 是 16。`PacketKey::decrypt_in_place(&self, packet_number: u64, header: &[u8], payload: &mut [u8]) -> Result<&[u8], Error>`；`encrypt_in_place(…) -> Result<Tag, Error>`。`rurge-net` 已依赖 `rustls`（ring、std、tls12、logging）与 quinn，`testing` 特性开着 `dep:rcgen`；`rurge-engine`、`rurge-dns` 的 dev 依赖开着 `rurge-net/testing` |
| P3 | 测试向量（V2） | RFC 9001 与 RFC 9369 的附录 A.2，取自 rfc-editor.org 的文本（2026-10-28）。两份都是 1200 字节，DCID `8394c8f03e515708`、包号 2、Length `0x449e`。写计划时用脚本把十六进制拼成连续的串，核对了长度与开头结尾（v1 开头 `c000000001088394c8f03e515708…`、结尾 `…e221af44860018ab0856972e194cd934`；v2 开头 `d76b3343cf088394c8f03e515708…`、结尾 `…a18b2faa745b6fe189cf772a9f84cbfc`），SHA-256 写在任务 1 里 |
| P4 | 识别的写法 | 第 1 ～ 6 步只读切片、不分配。包号的偏移是 `1 + 4 + 1 + dcid + 1 + scid + Token 长度的变长整数 + Token + Length 的变长整数`；变长整数照 RFC 9000 第 16 节（首两位给长度 1 / 2 / 4 / 8）。过了筛：把 `datagram[..偏移 + Length]` 复制进一个 `Vec`；样本是偏移加 4 处起的 16 个字节；首字节与 4 个包号字节复制出来去掉保护，写回副本；包号长度是首字节低两位加一；包号取这几个字节的大端值（第一个包，截短的就是完整的，同 sing-box 与 Xray 的做法）；附加数据是副本的 `[..偏移 + 包号长度]`，密文是其后到 `偏移 + Length` 为止（`split_at_mut`）。首字节的保留位不另查：解得开就是。版本到 rustls 的 `Version`：`1` → `V1`，`0x6b3343cf` → `V2`，`0xff00001d` ～ `0xff000020` → `V1Draft` |
| P5 | 构造器的布局 | 首字节 `0xc0 \| 类型 << 4 \| 3`（4 字节的包号；v1 与草案的类型 0，v2 的 1，所以是 `0xc3` / `0xd3`）；版本；DCID 长度与 DCID（任何长度，用例要造 0 与 21 字节的）；SCID 长度 0；Token 长度 0（一个字节）；Length 写成两字节的变长整数（`0x4000 \| len`），让整个数据报恰好 1200 字节；包号 0（4 个字节）；明文全是零（PADDING 帧）；AEAD 封好、补上 16 字节的标签之后，从包号偏移加 4 处取样本加头部保护。识别器不看帧，所以明文里不放 CRYPTO 帧 |
| P6 | STUN（M5-D21） | 判据照设计第 5 节，写在引擎的 `sniff.rs`；所有端口，只看流的第一个数据报。标签的次序：QUIC → STUN → UDP（两者互斥） |
| P7 | `block-quic` 的判定表 | `quic_verdict(global, policy, direct, carries_udp)`：`always-allow` 一律 `Allow`；`all` 一律 `Block`；`all-proxy`：不是直连类的一律 `Block`，直连类按它自己的参数（`on` → `Block`、`off` → `Allow`、`adaptive` → `Adaptive`、`auto` → `Allow`）；`per-policy`：`on` → `Block`、`off` → `Allow`、`adaptive` → `Adaptive`；`auto`：直连类 `Allow`，不载 UDP 的代理 `Block`，载 UDP 的代理 `Adaptive`。直连类不看 `carries_udp`。"直连类"即终端是 `TerminalKind::Direct`（DIRECT 与 `direct` 别名）；"载 UDP"即终端出站的 `udp()` 不是 `UdpSupport::Unsupported`。没有 spec 的终端（内置的 DIRECT）参数是 `Auto` |
| P8 | 自适应的表与 TCP 的通知（V3） | 表在 `rurge-engine` 的新模块 `quic_adaptive.rs`，每个引擎一个（`Engine` 的字段），跨重载保留，网络变化不清。键：`HostName` 的 `to_string()`，去掉末尾的点、转小写；UDP 一侧取流的目标，TCP 一侧取会话的 `dst_host`（fake 地址已反查成名字）。通知放在 `impl Dialer for Engine` 的 `dial` 里 `match turned` 之后、`connect_session` 之前：HTTP、SOCKS5、虚拟网卡的 TCP 会话都经这里；`Internal` 会话走 `dial_internal`，从不经过这里，所以"不是 `Internal` 的"由结构保证，不另判。两行 DEBUG 的原文：`quic: {host} falls back to TCP; QUIC stays blocked` 与 `quic: {host}: no TCP within 1s; QUIC allowed`（后一行在懒判定落定为 `Allowed` 时说，不论是下一条 QUIC 还是一个来晚了的 TCP 让它落定）。**用例不靠墙钟**：表的单元用例用显式的时刻；"客户端 TCP 会话通知了表"放在 crate 里的单元用例，预置一项 `Trying(now + 1 小时)`。**订正设计第 6.4 节**："第一条 QUIC 被挡、紧接着一个 TCP 会话之后第二条仍被挡"的集成用例不写：两步之间超过 1 秒（机器忙时）第二条就会被放行，用例会时红时绿；它要证的两件分别由上面两种单元用例证 |
| P9 | `[IP Rewrite]` 的解析（M5-D9、V6） | 类型与函数见任务 4。CIDR 照 `IP-CIDR` 的读法：`parse::<Ipv4Net>()`，读不出再 `parse::<Ipv4Addr>()` 当 `/32`（IPv6 同理当 `/128`）；主机位保留，匹配按 `network()` 到 `broadcast()` 的范围，主机位不影响；前缀长度越界是读不出。**订正设计第 8.1 节**：代码里没有模块的叠加（`sgmodule` 在 `crates/` 下出现 0 次，模块在阶段 5），"模块里的这一节……文本层已经这样做"不成立；"模块插在顶部的次序"的用例不写，等模块做的时候一起做 |
| P10 | `classify` 的形状 | 见任务 5。分出 `Classified::Rewrite(Hit)` 而不是把动作塞进 `Parsed`：命中的包没有读传输层，`Parsed.transport` 无从填起。给检查的用例留一个 `#[cfg(test)] checks(packet, ipv6)`（空表），`src/tests/filter.rs` 的几十处调用只换名字。`route` 多带表这一个参数，返回 `Routed` |
| P11 | `refusal` 的判法（设计第 8.5 节的表，V12） | 自上而下，第一条成立的定：（1）非首片（IPv4 片偏移不为 0；IPv6 的下一个首部是 44、分片首部里的偏移不为 0）→ `Nothing`；（2）ICMP 差错：IPv4 协议 1、首部之后第一个字节的类型是 3、4、5、11、12（首片也看），IPv6 的下一个首部是 58 时第 40 字节、或下一个首部是 44 且偏移为 0、分片首部的下一个首部是 58 时第 48 字节，类型小于 128 → `Nothing`；ICMP 的类型那个字节不在包里 → `Nothing`（读不到就不知道是不是差错，不回）；（3）不分片的 TCP：`packet::segment(packet)` 是 `None`（首部读不出、端口 0）→ `Nothing`；带 RST → `Nothing`；带 SYN、不带 ACK → `Reset`；（4）其余 → `Prohibited`。`rst_for` 的前提是 `segment()` 读得出（它里面 `expect`），由（3）保证。引用原包的长度是 `min(len, 首部长 + 8)`，IPv4 的选项不另截（`QUOTE_MAX` 是 60 + 8，同 UDP 被拒时） |
| P12 | 计数与日志（M5-D13） | `Reason::IpRewrite` 的 `is_wanted()`：`Stats::drop` 从不为它 WARN，每次 DEBUG（同别的原因的 DEBUG 行）；`warnings` 的 16 位图不为它占一位也放得下（原因一共 11 种）。`reflected` 在写任务收下那一包时才计；队列满了计 `queue_full`、不计 `reflected`。`rst_limited` 的两个地址是被拒的 SYN 的（`Admission::count` 的写法），`icmp_limited` 的是那条消息的来处与去处（`UdpClient::refused` 的写法）。API 的键：`dropped.ipRewrite`、顶层 `reflected` |
| P13 | 写任务（M5-D8、V7） | 见任务 6。`Class` 与 `Outlet` 放在 `writer.rs`；`Context::send` / `try_send` 每次发送读一次 `udp_priority`（`Hooks::settings()` 是一次 `ArcSwap::load_full` 与一次 `Arc` 克隆）。各片只拿普通队列的发送端，照旧不经 `Context`。一条队列关了之后不再等它（`select!` 的分支带开着与否的条件），两条都关了才结束 |
| P14 | 内部会话的形状（V4） | `dial_internal` / `dial_internal_udp` 多一个 `address: Option<IpAddr>`，不加 `SessionInfo` 的字段。规则：`choose_policy_with` 带一个 `&dyn LazyResolver`，带地址的内部会话给它 `FixedResolve(ResolvedAddrs { v4 / v6 里只有这一个地址 })`。请求记录的 `dst` 是名字与端口；`remote_addr` 照今天取 `trail().peer()`（TCP 是拨到的地址，UDP 是 `None`）；没有查找，`dns_ms` 是 `None`。`smart` 组给这类会话记的站点从地址变成名字（登记） |
| P15 | DoQ / DoH3 也带名字 | 设计第 9.2 节只写了 `BootstrapConnector`；DoQ 与 DoH3 不经它，而是自己经引导解析、在地址上开 QUIC 的路径（`upstream/session.rs` 的 `Dialer::connect_to` → `Path::open`）。为让设计第 9.1 节说的"QUIC 载体"同样带名字，`Path` 加 `open_resolved`，`connect_to` 在服务器写成名字时用它。**补设计第 9.2 节** |
| P16 | 引擎的 DNS 会话用例用 `tcp://` | 设计第 9.6 节写"回环上的 DoT / DoH 测试服务器"；`MockDns` 的 TLS 证书只给 `localhost` 与 `127.0.0.1`，名字 `dns-up.test` 过不了校验。`tcp://` 上游走同一个 `BootstrapConnector`、同一个 `Face`，证的是同一件事；DoT / DoH / DoQ / DoH3 带名字由 `rurge-dns` 的两条用例证。**订正设计第 9.6 节的写法** |
| P17 | WireGuard 6.11 的写法（V9） | 先扫一遍节里有没有 `listen-port`，再逐行读（`peer` 可能写在 `listen-port` 之前）。三句 `W0029` 的原文：`` `listen-port` has no effect: this version accepts no incoming WireGuard peers ``；`` `accept-incoming` has no effect: this version accepts no incoming WireGuard peers ``；`` peer {n}: no `endpoint`; this version accepts no incoming WireGuard peers, so the peer is left out ``。节里一个 peer 也不剩时节照常加载，策略那一侧是 `NotImplemented::WireGuardNoPeers`（`W0007`，运行时 `Entry::Unsupported`：按 REJECT，记录的 `error` 是 `policy protocol not implemented: wireguard (no peer to dial)`）。`peers[0]` 的两处用法（`rurge-policy/src/registry.rs`、`testing.rs`）只在有 spec 时走得到，没有 peer 的节没有 spec |
| P18 | `TunSettings` 的两项分两个任务加 | `ip_rewrite` 在任务 5、`udp_priority` 在任务 6，各自把字面量补齐（设计第 3 节把两项写在一处） |
| P19 | 基准（V11） | 命令见任务 9（`docs/testing.md` 第 133 ～ 135 行）。上一次的读数（一台 Windows 开发机，M3 与 M4 的计划记录）：B1 中位数上行约 7.2、下行约 5.5 Gbit/s（线：每个方向 0.7 Gbit/s）；B2 门槛读数 402.5 MB（线 429 MB；保守读数超线，归项目所有者，M3a 计划延后第 15 行）；b3 中位数 0.9 µs、p99 3.2 µs、最大 5.8 ms（线 0.2 ms） |
| P20 | 出处 | RFC 9000 第 14.1、16、17.2.2 节，RFC 9001 第 5 节与附录 A.2，RFC 9369 第 3 节与附录 A.2，RFC 5389 / 8489 第 5、6 节，RFC 1122 第 3.2.2 节，RFC 4443 第 2.4 节；rustls 0.23.43 的 `quic` 模块（本机注册表）；sing-box、Xray-core、mihomo 的 QUIC 嗅探（只取事实：都以解密确认、认的版本相同）；手册 2026-10-28 的 `policies/parameters.html`、`policies/wireguard.html`（复查报告）与 `features/ip-rewrite.html`（基线文本，复查时没有变） |

## 执行期修正记录

下面写"执行者的裁定"的，是 Pi 在设计与计划的原则下定的；写"协调者的裁定"的，是协调的会话代项目所有者定的。项目所有者都可以改。计划的正文不改：正文里说得不对的地方，以这张表的行为准。

| # | 任务 | 修正 | 提交 |
| - | ---- | ---- | ---- |
| 1 | 0 | 第 1 项（工作树）与第 2 项（读）合并：`git status --short` 空；分支 `phase3-m5`，`HEAD` = `e6666c1`（本计划的提交），父提交 `9ee270f`（M5 的设计），`git log --oneline 9ee270f..HEAD` 只有 `e6666c1` 一行，`9ee270f` 之后只多了本计划的提交；`origin/main` 仍是 `3a76b80`（本分支未推送）。设计全文读过（第 4.2、6.2 ～ 6.3、7.2、8.3 ～ 8.5、9.2 ～ 9.4、10.2 节与第 20 节的五处订正），本计划的「计划期决定」P1 ～ P20 读过。无出入 | 本提交 |
| 2 | 0 | 第 3 项（`rurge-net`）：名字都在，签名与 P2、P15 写的相同。`connector.rs` 的 `Target`（`:89`，`host: HostName`、`port: u16`）、`ConnectOpts`（`:101`，只有 `timeout: Duration`）、`Connector`（`:116`，`connect` / `connect_udp` 各回 `BoxFuture<'a, io::Result<BoxedStream / BoxedDatagram>>`）；`quic/dial.rs` 的 `Transport`（`:20`）、`Transport::STANDARD`（`:54`）、`client_config(tls: Arc<rustls::ClientConfig>, transport: &Transport) -> io::Result<ClientConfig>`（`:117`）；`quic/mod.rs` 的 `Path`（`:37`，`pub socket`、`pub remote`，`carrier` 私有）与 `Path::open(connector: &dyn Connector, server: &Target, hopping: Option<&PortHopping>, ecn: Tristate, opts: &ConnectOpts) -> io::Result<Path>`（`:54`，`pub async fn`，任务 7 的 `open_resolved` 加在它旁边）；`quic/testing.rs` 的 `TestCert`（`:25`）、`TestCert::client_tls(&self, alpn: &[&str]) -> Arc<rustls::ClientConfig>`（`:78`）、`endpoint(config: ServerConfig, socket: Arc<dyn AsyncUdpSocket>) -> Endpoint`（`:94`）。`TestCert` 另有 `new`（`:33`）、`for_names(&[&str])`（`:38`）、`roots`（`:57`）、`server_config`（`:65`） | 本提交 |
| 3 | 0 | 第 3 项（`rurge-dns`）：`BootstrapConnector`（`bootstrap.rs:163`，字段 `inner: Arc<dyn Connector>`、`bootstrap: Arc<Bootstrap>`）、`new`（`:169`）、私有的 `each_address<T, F>(&self, target: &Target, dial: impl Fn(Target) -> F) -> io::Result<T>`（`:176`，`F: Future<Output = io::Result<T>>`；今天对每个地址调 `dial(Target::new(HostName::Ip(ip), port))`，任务 7 在这里改调内层的 `*_resolved` 并带上 `target` 的名字）；`upstream/session.rs` 的 `Dialer` 是结构体（`:96`，字段 `host: String`、`port: u16`、`connector`、`bootstrap`、`config`），`connect_to(&self, server: &Target, budget: Duration) -> io::Result<Connection>`（`:170`，`pub(super) async fn`，里面 `Path::open(..., None, Tristate::Auto, &opts)` 再 `quic::connect`，任务 7 按 P15 换成 `open_resolved`）；`testing.rs` 的 `MockDns`：`pub async fn spawn() -> MockDns`（`:242`）、`spawn_tls`（`:246`）、`set(&self, name: &str, v4: &[&str], v6: &[&str], ttl: u32)`（`:396`）、`queries() -> Vec<(Question, String)>`（`:458`）、`query_count(&self, name: &str, qtype: Qtype) -> usize`（`:468`）；`resolver.rs` 的 `a_named_server_reaches_its_connector_as_the_address_the_bootstrap_gave`（`:3979`）。P16 的前提也核过：`MockDns` 的证书只给 `localhost` 与 `127.0.0.1`（`testing.rs:541`） | 本提交 |
| 4 | 0 | 第 3 项（`rurge-config`）：`spec/common.rs` 的 `Tristate { Auto, On, Off }`（`:20`）、`TRISTATES: [(&str, Tristate); 5]`（`:106`，`true` / `false` 读作 `On` / `Off`）、`CommonOpts`（`:73`）的 `pub block_quic: Tristate`（`:82`）与读它的 `r.choice("block-quic", &TRISTATES).unwrap_or_default()`（`:181`）；`general.rs` 的 `BlockQuicGlobal`（`:125`）；`deferred.rs` 的 `DEFERRED`（`:6`）里仍有 `"IP Rewrite"`（`:15`，任务 4 拿掉）；`config.rs` 的 `ip_rewrite_is_a_known_section_kept_for_later`（`:2623`，今天断言的是 `W_DEFERRED_SECTION`）与读 `[SSID Setting]` 的那一段（`:906` ～ `:912`）；`wireguard.rs` 的 `Report<'a>`（`:83`，私有，`name` / `diags` / `failed`，`error` 走 `E0023`、`warn` 走 `W0001`）、`peer(report: &mut Report<'_>, span: &Span, item: &str, n: usize) -> Option<WireGuardPeer>`（`:255`）、`parse_section(section: &Section, diags: &mut Diagnostics) -> Option<WireGuardSection>`（`:356`）；`spec/mod.rs` 的 `NotImplemented`（`:192`）；五个诊断常数都在 `diagnostic.rs`：`E0018`（`:188`）、`E0023`（`:199`）、`W0007`（`:209`）、`W0012`（`:214`）、`W0029`（`:248`） | 本提交 |
| 5 | 0 | 第 3 项（`rurge-rules` 与 `rurge-engine`）：`FixedResolve(pub ResolvedAddrs)`（`crates/rurge-rules/src/engine.rs:54`）、`ResolvedAddrs { pub v4: Vec<Ipv4Addr>, pub v6: Vec<Ipv6Addr> }`（`matcher.rs:40`）。`sniff.rs` 的 `is_quic_initial(datagram: &[u8]) -> bool`（`:12`，任务 2 换成 `rurge_net::quic::initial` 并加 `is_stun`）；`udp.rs` 的 `quic_blocked(global: BlockQuicGlobal, policy: Tristate, direct: bool) -> bool`（`:62`，任务 3 换成 `quic_verdict`）、定标签那一行 `asked.port == 443 && crate::sniff::is_quic_initial(&first.datagram)`（`:489`）、`async fn open(engine: &Engine, association: &Association, handle: &Arc<SessionHandle>, destination: &Target, asked: &Target) -> Result<(Arc<Flow>, Arc<Carrier>, Target), SessionOutcome>`（`:564`）；`engine.rs` 的 `choose_policy(&self, rt: &Runtime, registry: &PolicyRegistry, handle: &SessionHandle) -> Chosen`（`:788`，今天没有 `choose_policy_with` 与解析器参数，任务 7 按 P14 加）、`bypass_to_direct(handle: Arc<SessionHandle>, fallback: &Arc<dyn Connector>, target: &Target, opts: &ConnectOpts, why: &str) -> io::Result<BoxedStream>`（`:942`）、`route_internal(&self, rt: &Runtime, registry: &PolicyRegistry, handle: &SessionHandle) -> InternalRoute`（`:1022`）、`pub async fn dial_internal(&self, session: SessionInfo, fallback: &Arc<dyn Connector>) -> io::Result<BoxedStream>`（`:1078`）、`connect_internal(&self, rt: Arc<Runtime>, registry: Arc<PolicyRegistry>, handle: Arc<SessionHandle>, fallback: &Arc<dyn Connector>) -> io::Result<BoxedStream>`（`:1093`）、`pub async fn dial_internal_udp(&self, session: SessionInfo, fallback: &Arc<dyn Connector>) -> io::Result<BoxedDatagram>`（`:1164`）、`open_internal_udp`（`:1179`，同形，`rt` / `registry` / `handle` / `fallback`）、`impl Dialer for Engine`（`:1586`）的 `fn dial<'a>(&'a self, mut session: SessionInfo) -> BoxFuture<'a, Result<Dialed, DialError>>`（`:1587`，P8 的通知加在 `match turned` 之后、`connect_session` 之前）；`dns_pipeline.rs` 的 `PipelineConnector`（`:29`）与私有的 `Face`（`:122`，`impl Connector for Face` 在 `:127`）；`runtime.rs` 建 `TunSettings` 的那一处（`:99`，今天六个字段）。用例：`tests/common/mod.rs` 的 `quic_initial() -> Vec<u8>`（`:575`）、`tests/common/tun.rs` 的 `guarded(rules: &str) -> String`（`:31`）与 `Rig`（`:44`）、`tests/tun.rs` 的 `prohibited_from`（`:197`） | 本提交 |
| 6 | 0 | 第 3 项（`rurge-tun`、`rurge-api` 与 `rurge`）：`hooks.rs` 的 `Hooks`（`:17`）、`fn settings(&self) -> Arc<TunSettings>`（`:19`）、`TunSettings`（`:29`，今天六个字段，任务 5、6 各加一项）；`packet.rs` 的 `Reason`（`:40`）、`Dropped`（`:111`）、`Parsed`（`:138`）、`classify(packet: &[u8], ipv6: bool) -> Result<Parsed, Dropped>`（`:193`）、`segment(packet: &[u8]) -> Option<Segment>`（`:205`）、`rst_for(syn: &[u8]) -> Vec<u8>`（`:223`）；`dispatch.rs` 的 `run`（`:25`）与 `route`（`:94`）；`context.rs` 的 `Context`（`:25`）；`writer.rs` 的 `run`（`:25`）；`icmp.rs` 的 `limiter()`（`:40`）、`prohibited(src: IpAddr, dst: IpAddr, quoted: &[u8]) -> Vec<u8>`（`:50`）、`refuse(ctx: &Context, src: IpAddr, dst: IpAddr, quoted: &[u8])`（`:78`）；`tcp/admission.rs` 的 `allow_reset(&self, now: Instant) -> bool`（`:123`）与 `count(&self, reason: Reason, src: IpAddr, dst: IpAddr)`（`:137`）；`stats.rs` 的 `Stats::drop(&self, reason: Reason, src: Option<IpAddr>, dst: Option<IpAddr>)`（`:51`）；`lib.rs` 的 `Counters`（`:669`）；`testing/hooks.rs` 的 `FakeHooks::set(&self, settings: TunSettings)`（`:41`）；`src/tests/support.rs` 的 `Extras`（`:42`）、`settings(resolver: Arc<dyn Resolve>, icmp_forwarding: bool) -> TunSettings`（`:75`）、`rig_extras(options: TunOptions, extras: Extras) -> (Rig, Extras)`（`:94`）、`GatedDevice`（`:171`）；`rurge-api` 的 `enhanced_json(status: EnhancedStatus) -> Value`（`routes/features.rs:33`）；`crates/rurge/tests/run/enhanced_mode.rs` 的 `assert_eq!(dropped.len(), 10, "ten reasons: {body}")`（`:475`） | 本提交 |
| 7 | 0 | 第 3 项的一处出入：计划写 `crates/rurge-api/tests/api.rs` 里写全 `TunCounters` 字段的两处，代码是一个 `TunCounters { … }` 字面量（`:652`）加同一条用例里的两个期望 JSON（关上时 `:631` ～ `:647`、打开时 `:665` ～ `:678`），按代码做：任务 5 加 `ipRewrite` 与 `reflected` 时三处都要改 | 本提交 |
| 8 | 0 | R1（协调者的裁定，接任务 5）：任务 5 在 `rurge-tun` 的用例里造 `[IP Rewrite]` 的表，照 `crates/rurge-tun/src/tests/dns/mod.rs:185` 从文字造 `HostList` 的做法——用 `rurge_config::text::parse_str("[IP Rewrite]\n…", …)` 取出那一节的 `entries`，交给 `rurge_config::ip_rewrite::parse`，在用例模块里写一个小函数；任务 4 的接口照计划（`parse`、`IpRewrite`、`action`、`is_empty`、`Config.ip_rewrite`），不为用例加只给用例用的公开构造函数。补的是计划「文件结构」与任务 4、5 之间的一处缺口 | 本提交 |
| 9 | 0 | R2（协调者的裁定，接任务 3）：任务 3 的 `QuicAdaptive` 记"最近用到"用一个递增的序号（一条 QUIC 流被记或刷新时加一），不用 `Instant`、不比较时刻；同一时刻的几次调用按调用的次序排，淘汰时取序号最小的那一项。`the_201st_name_evicts_the_least_recently_used` 的调用不带不同的时刻，按 `Instant` 排会并列 | 本提交 |
| 10 | 0 | 第 4 项：`Cargo.lock` 的 `[[package]]` 519 个；`crates/rurge-platform/tests/unsafe_functions.rs` 的 `LISTED` 是 `[(&str, &str); 23]`（`:18` ～ `:42`），数下来 23 项。都与计划写的一样 | 本提交 |
| 11 | 0 | 第 5 项（门禁，一台机器上的一次运行，不是 CI）：`cargo fmt --all --check` 通过（退出码 0）；`cargo clippy --all-targets -- -D warnings` 通过（0 个警告，日志工作区的 `gate-task-0-clippy.log`）；`cargo test -j 8 --workspace --no-fail-fast` 第一次 85 个结果行、4630 通过、0 失败、13 忽略，`outbounds_hysteria2` 的可执行文件以 `STATUS_HEAP_CORRUPTION`（`0xc0000374`）退出（本机管控软件的钩子 DLL，`CLAUDE.md`「这台开发机」），单独重跑 `cargo test -j 8 --workspace --test outbounds_hysteria2` 得 1 个结果行、11 通过、0 失败、0 忽略；两次合起来 86 个结果行、4641 通过、0 失败、13 忽略，与基线 86 / 4641 / 0 / 13 相同。日志 `gate-task-0.log` 与 `gate-task-0-rerun.log` 在工作区（git 忽略） | 本提交 |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | **手工验收**：M5 一节（任务 10 写），连同阶段 1、2、3 一项也没有做过的各节 | 项目所有者（手工验收） |
| 2 | **推送与读 CI**：M5 的提交都在本地 | 项目所有者推送；之后的会话读回那次运行的注解与任务摘要 |
| 3 | 总设计 Q8：需求文档第 4 节要不要为 `[IP Rewrite]` 另立一行；FR-OUT-11 的"UDP 443"按本计划（所有端口）改不改字面 | 项目所有者（M5-D24） |
| 4 | B2 的保守读数超线（M3a 计划延后第 15 行） | 项目所有者；任务 9 照旧记数 |
| 5 | `[MITM]` 的 `auto-quic-block` | 阶段 4（M5-D7） |
| 6 | QUIC 里的 SNI、跨几个包的 ClientHello、按 SNI 匹配规则 | 阶段 4 |
| 7 | 经代理的 DNS 会话：代理收到的是引导解析出的地址，不是名字 | 登记（M5-D16）；改成交名字要先改"引导先解析"的次序 |
| 8 | 自适应：阻断之前已经在的 TCP 连接不算回落（照手册的字面读，没有观察过 Surge） | 登记（M5-D6） |
| 9 | WireGuard 与 Tailscale 接受连入（`listen-port`、`accept-incoming`、`advertise-*`） | 阶段 7 / 远期 |
| 10 | `GET /v1/policies?include_external=true` | 阶段 6 |
| 11 | `.local` 答案里带作用域号的 IPv6 | 远期（M5-D19） |
| 12 | `[IP Rewrite]` 在模块里的叠加 | 阶段 5，与模块一起（P9） |
