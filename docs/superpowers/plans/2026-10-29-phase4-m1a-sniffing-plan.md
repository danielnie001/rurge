# 阶段 4 / M1a「嗅探与规则之前的 SNI」实施计划

> **执行方式**：交给 Pi，由协调的会话逐个任务派工、逐个任务评审（设计第 2 节、M1-D1；做法同阶段 3 的 M4d ～ M6）。
>
> - 协调的会话每个任务开一个 Pi 会话，交给它这个任务的全文与要用的接口（派工文件）。Pi 做完把报告写进派工文件指定的路径，在本地提交。
> - 协调者读改动、重跑门禁、评审。有问题回到同一个 Pi 会话修正，再评审；过了才派下一个任务。
> - 每个任务：先写失败的用例、看到红；再实现到绿；跑门禁；对照本任务的「判据」与「全局约束」逐条自查；最后提交（**只在本地，不推送**）。
> - 与计划的出入记进末尾的「执行期修正记录」，**不改计划的正文**。
>
> 没有等 CI 的停点：M1 没有平台代码（设计开头）。M1a 做完不停：协调者对着合并之后的代码写 M1b 的计划、接着做；M1b 做完由项目所有者推送一次，协调者读回那次 CI。

**目标**：80、443 与 `force-http-engine-hosts` 命中的 TCP 会话先答后读、先嗅探再求规则，于是 SNI 与协议在规则之前就有。具体是：

- 新 crate `rurge-http` 的第一部分：前缀流；完整的 TLS ClientHello 读取（记录的重组与握手消息的解析分开）；HTTP 开头的判断；带时限的嗅探。
- 三种入口（SOCKS5、HTTP 监听器的 `CONNECT`、虚拟网卡）对可嗅探的会话先答"成功"，再把连接交给 `Dialer::serve`。
- 引擎判定哪些会话可嗅探（端口、三个设置、预匹配、暂停、fake 地址）；嗅探之后带着 SNI 与协议求规则、拨号，把前缀一个字节不丢地转发。
- 规则集文件里一行上的 `extended-matching` 作用于整集（清单 L314）。
- 嗅探的五种结局计数，`GET /v1/rurge/http` 读出。
- 文档收尾。

判为 HTTP 的连接在 M1a 照原样转发、协议记 `HTTP`；进 HTTP 引擎是 M1b 的事。

**架构**：没有平台代码，全部在开发机上编译、运行。

- `rurge-http`（新）在 M1a 只依赖 `tokio`：`prefix`（`Prefixed<S>`）、`tls`（`ClientHello`、`parse_client_hello`、记录的重组）、`sniff`（`judge`、`sniff`、`Sniffed`、`SNIFF_WAIT`）。它不认识引擎、监听器与会话。
- `rurge-inbound` 的 `Dialer` 多两个带缺省实现的方法 `sniffable` 与 `serve`，另加 `ServeOpts` 与通用的停留 `hold_dropped`。SOCKS5 与 `CONNECT` 在 `sniffable` 为真时先答、再 `serve`。
- `rurge-tun` 的 `session::dial_and_relay` 在 `sniffable` 为真时把流交给 `serve`；`rurge-tun` 不依赖 `rurge-http`。
- 引擎的新模块 `serve.rs`：`sniffable` 的判定与 `serve` 的流程。流程是：嗅探与停止竞速 → 组带 SNI 与协议的会话 → 今天的 `dial` 原样做 → 前缀流与转发。计数在引擎里；`rurge-api` 经引擎读，照旧只依赖 `rurge-engine` 与 `rurge-config`。

**技术栈**：tokio（`io-util`、`time`）；rustls 0.23.43（ring，只在 `rurge-http` 的用例里产生真的 ClientHello）；tokio-rustls（引擎用例里的 TLS 客户端，已是引擎的 dev 依赖）。不加外部包。

**依据**：

- M1 细化设计（下面写"设计"指它）：`docs/superpowers/specs/2026-10-29-phase4-m1-session-entry-http-engine-design.md`。M1a 用到第 1 ～ 7、11 ～ 16 节与已决事项 M1-D1 ～ D11、D24、D26、D27。**先读它第 20 节"写计划时的订正"**：本计划改了设计正文的地方列在「计划期决定」里，以计划为准。
- 阶段 4 总设计：`docs/superpowers/specs/2026-10-29-phase4-http-engine-design.md` 第 5 节（嗅探）。
- 手册：
  - `profile/general.html`：三个设置；
  - `policies/reject.html`：REJECT 一族与预匹配，以及规则为什么要等第一个包；
  - `rules/ruleset.html`：一行上的 `extended-matching`；
  - `rules/protocol-and-network.html`：`PROTOCOL`。
  - 原话都在设计里。
- 写计划时对着 `3ec732e` 核对过的事实写在「计划期决定」里，包括设计第 16 节 V1 ～ V10、V15、V16 的结论。

## 全局约束

**依赖**

- 新 crate `rurge-http`（任务 1）：
  - 工作区成员是 `crates/*`，新 crate 自动加入。
  - 根 `Cargo.toml` 的 `[workspace.dependencies]` 加 `rurge-http = { path = "crates/rurge-http" }`。
  - 它的 `[dependencies]` 只有 `tokio`，`[dev-dependencies]` 只有 `rustls`，都是工作区依赖。设计第 3 节的四条边（`rurge-config`、`rurge-net`、`rurge-inbound`、`rurge-proto`）在 M1b 用到时再加（P2）。
- 新的依赖边只有 `rurge-engine → rurge-http`，在任务 6 加。`rurge-tun`、`rurge-inbound`、`rurge-api` 不依赖 `rurge-http`。
- 不加外部包。`Cargo.lock` 的包数在 `3ec732e` 是 519 个；任务 1 之后是 520 个（多的是 `rurge-http` 自己），以后每个任务照旧。
- `unsafe_code`：`rurge-http` 照工作区的 `[lints] workspace = true`（`forbid`）；不加放开的函数，`crates/rurge-platform/tests/unsafe_functions.rs` 的 `LISTED` 仍是 23 项。

**缺省的 `cargo test` 不碰系统**

- M1 没有平台代码、没有 L2 用例。不建网卡，不改系统的任何设置。
- 不执行 `netsh`、`route`、`ipconfig`、`ip`、`resolvectl`、`networksetup`，不读真实的网络环境。
- 虚拟网卡的用例只用 `rurge-tun` 的内存设备与引擎用例的 `Rig`（`crates/rurge-engine/tests/common/tun.rs`）。

**测试的约定**

`CLAUDE.md`「测试约定」整节适用。写 `rurge-tun` 的用例与经引擎的 TUN 用例之前，读 `docs/testing.md`「测试约定（全文）」里对应的一节（阶段 3 / M3a、M3b、M3c）。与本计划直接相关的：

- 只用回环与端口 0，失败时也不离开回环。用例绑不了 80、443（Linux 上要特权），所以"目标是 443"的会话这样走到回环上的服务器（P14）：
  - 规则把它送到一个 SOCKS5 策略。策略指向 `rurge_proto::testing::FakeSocks5`，它的 `Socks5Script.connect_to` 不管目标是什么，都连到用例给的回环地址（例子是 `tests/tun.rs` 的 `tcp_to_a_documentation_address_follows_the_rules`）。看哪一个 FakeSocks5 被问到、问的是什么，就知道哪条规则命中。
  - 或者用 `force-http-engine-hosts` 让端口 0 的服务器的端口可嗅探。
- 写了主机名的 Profile 另给第二道防线：
  - 经引擎的 TUN 用例用 `guarded`；
  - 监听器的用例里，`target.test` 与 `alt.test` 经 MockDns 落在 `127.0.0.1`，别的名字加一行 `[Host]` 钉到 `127.0.0.1`。
- 嗅探的等待在用例里调短：`EngineShared { sniff_wait: …, ..EngineShared::default() }`（任务 6 加的字段）。端到端用例用 300 毫秒；要证明"是停止、不是时限结束了等待"的用例用缺省的 15 秒。
- 等待都有时限。固定等待只许"断言什么都没发生"，注释写它等的是什么、余量多大。
- 新写的、带等待的用例提交前单独循环跑二十次以上：任务 2 的 `sniff` 用例、任务 4 的停留用例、任务 5 的虚拟网卡用例、任务 6 与 7 的引擎用例。
- 抓日志用 `LogCapture`，跑在当前线程的运行时上（普通的 `#[tokio::test]`）。
- **内联的测试模块放在文件的末尾**：`crates/rurge/tests/sockets.rs` 读生产代码，读到第一个 `#[cfg(test)] mod … {` 为止；那之后再出现顶格的条目会让它变红。

**门禁与这台开发机**（每个任务末尾一次；说"通过"之前必跑）

- 门禁：`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test -j 8 --workspace --no-fail-fast`。
- 本机 msvc 工具链没有 rustfmt。跑 `cargo fmt` 之前让环境变量 `RUSTFMT` 指向 gnu 工具链里同版本的 rustfmt：用户目录下的 `.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe`（同版本，输出与 CI 一致）。不要为此安装或升级工具链。
- 聚焦地跑用例也经整个工作区：
  - 单元测试：`cargo test -j 8 --workspace --lib -- <过滤词>`；
  - 集成测试：`cargo test -j 8 --workspace --test <名字> -- <过滤词>`。
- **不用 `-p`**：它另编一份依赖，`target/` 涨得很快，写满过 D: 盘。
- 跑全量之前 `df -h /d`：剩余不足约 15 GiB 就停下报告。
- 测试二进制以 `STATUS_HEAP_CORRUPTION`（本机管控软件注入的钩子 DLL）或 `STATUS_ACCESS_VIOLATION` 退出：单独重跑那一个二进制，报告里写明。
- rustc 在增量编译里报内部编译器错误：删掉出错的那个目标在 `target/debug/incremental/` 下的目录（可再生的缓存）再编。
- 回环的 TCP / UDP 7、9、13、17、19 号端口在这台机器上开着，CI 上关着：用例不假定任何固定端口的状态。"一个没有应答的地方"是用例自己持有的套接字。
- 结果照实记：
  - 本机的全量运行是"一台机器上的一次运行，不是 CI"；
  - 需要参照服务端的互操作用例在本机立刻返回、算在通过的数里，要说出来。

**仓库的规矩**（`CLAUDE.md`「必须遵守的规矩」整节适用；与本计划直接相关的）

- 日志（设计第 12 节；P8）：
  - 嗅探不另写日志；
  - 新的日志行只有一条：`CONNECT` 升级失败、还没有会话时的 DEBUG，不带地址；
  - 虚拟网卡上的名字与地址照旧只在 DEBUG。
- 凭据：没有新的带凭据的字段；`Proxy-Authorization` 照旧不进日志、错误文本与 API。
- 诊断：不加新码。
- 出站套接字只有一处建法：本计划的生产代码不新建套接字、不问系统解析库、不新建连接器。若改动让 `crates/rurge/tests/sockets.rs` 变红，停下报告。
- `rurge-tun`：
  - 停止取消正在转发的会话，不取消正在进行的拨号；
  - 结束一个会话靠取消它的令牌，不靠丢掉拨号或转发的 future；
  - 改 `rurge-tun` 之前（任务 5），读 `docs/status.md` 阶段 3 的 M3a、M3b、M3c 各节"必须知道的事"。
- 文档与报告用写文件 / 改文件的工具改。**永远不要用把一个文件的内容经管道写回同一个文件的命令**（`Get-Content x | Out-File x -Append` 一类；2026-10-27 这样写满过 D: 盘）。

**写法**

- 注释、日志与错误文本用英文，照所在文件的写法：文档注释写"为什么"与前提，句子完整。注释里提到设计或计划，写 `phase 4 M1 design 7.1`、`the phase 4 / M1a plan's execution record row N`。
- 提交说明照 `git log` 的样子写，标题用中文：`feat(http): …`、`feat(rules): …`、`feat(inbound): …`、`feat(tun): …`、`feat(engine): …`、`feat(api): …`、`test(engine): …`、`docs: …`。正文说做了什么、改了哪些现有用例的期望、为什么。Pi 的提交不带署名行。
- 只在本地提交：不 `push`、不 `pull`、不 `fetch` 远端、不 `amend`、不改写历史。

**不做的事**

- 只运行本仓库构建出来的二进制与 Pi 自己。不下载、不安装任何东西（含 `rustup target add`）。
- 不手动跑 `rurge run`、`rurge dns lookup`；`rurge service` 只用 `--dry-run`。
- 参考实现：GPL 的（sing-box、mihomo、Xray-core）只取事实，代码不抄。RFC 是公开的事实。

**计划没有写到、要做决定的地方**

- 能按设计与总设计的原则定的：自己定，记进修正记录，写"执行者的裁定"。
- 去向写着"项目所有者"的（设计第 19 节），或会改变 M1-D1 ～ D27 的：停下来列出，交给协调者。
- 现有用例变红：本计划点了名的照计划改（P9），在提交说明里说；没点名的停下来，记进修正记录，不顺手改期望值。

## 评审要点

设计没写、而最可能咬到使用者的几种输入，与负责它们的任务和用例：

1. **恶意或残缺的 ClientHello。** 任何客户端都能往 443 发任意字节，长度全由对方给。
   - 期望：不 panic、不越界；缓冲至多 64 KiB 加一次读的量；一个字节一个字节发来时，不每来一个字节就从头再解析一遍（`NeedMore` 带"至少要多少字节"，P3）。
   - 用例（任务 2）：`no_damaged_hello_panics`、`lengths_that_do_not_add_up_are_other`、`more_than_64_kib_is_other`、`a_slow_drip_is_not_judged_again_until_it_can_be`。
2. **前缀丢字节或乱了次序。**
   - 期望：转发给上游的头一批字节就是客户端发来的全部前缀，后面接着客户端以后发的。
   - 用例（任务 1、7）：`the_prefix_is_read_before_the_stream`、`the_prefix_reaches_the_upstream_whole`。
3. **客户端一个字节也不发**（443 上服务器先说话的协议，或者客户端挂着不动）。
   - 期望：等满时限按原样转发，不挂住；等的时候遇到停止（虚拟网卡停、守护进程退出）立刻结束。
   - 用例（任务 7）：`a_server_first_protocol_passes_after_the_wait`、`a_stop_during_the_wait_ends_a_tun_session_at_once`。
4. **先答后读改变了客户端看到的失败。**
   - 期望：只在可嗅探的会话上这样。预匹配会拒、引擎暂停、fake 地址背后没有名字时，照今天的路先拨号再答（P5、P6）。
   - 用例（任务 6、7）：`which_sessions_are_sniffable`、`pre_matching_still_refuses_before_the_answer`、`a_paused_engine_still_refuses_before_the_answer`；`tests/tun_dns.rs` 的 `a_socks5_session_to_an_unknown_fake_address_is_refused` 不改照过。
5. **同一个会话的规则命中数加两次**（`sniffable` 先问一次预匹配，拨号时规则又求一次）。
   - 期望：探询不计数。
   - 用例（任务 3）：`pre_refuses_answers_as_pre_match_and_counts_nothing`。
6. **现有用例里连 80、443 的。**
   - 期望：讲"拨号之后才答"的用例换成不嗅探的端口，断言不改；别的照过（P9）。

---

## 文件结构

| 文件 | 任务 | 职责 |
| --- | --- | --- |
| 根 `Cargo.toml`、`crates/rurge-http/{Cargo.toml,src/lib.rs,src/prefix.rs}`（新） | 1 | 新 crate；前缀流 |
| `crates/rurge-http/src/{tls.rs,sniff.rs}`（新）、`src/lib.rs` | 2 | ClientHello、判断、嗅探 |
| `crates/rurge-rules/src/{set.rs,engine.rs}` 与 `engine` 的用例模块 `pre_match` | 3 | L314；`pre_refuses` |
| `crates/rurge-inbound/src/{session.rs,hold.rs（新）,lib.rs,socks5.rs,http.rs,testing.rs}` | 4 | `ServeOpts`、`sniffable` / `serve`、`hold_dropped`；两个监听器先答后交 |
| `crates/rurge-tun/src/session.rs`、`src/tests/outcomes/…` | 5 | 虚拟网卡把可嗅探的会话交给 `serve` |
| `crates/rurge-engine/{Cargo.toml,src/serve.rs（新）,src/engine.rs,src/shared.rs,src/observe.rs,src/lib.rs}`、`tests/sniff.rs`（新）、P9 点名的现有用例 | 6 | 判定、嗅探之后的分派、记录的 SNI、计数 |
| `crates/rurge-engine/tests/sniff.rs` | 7 | 端到端 |
| `crates/rurge-api/src/{routes/rurge_http.rs（新）,routes/mod.rs,lib.rs}`、`tests/api.rs` | 8 | `GET /v1/rurge/http` |
| 文档（任务 9 列全） | 9 | 收尾 |

模块的名字与拆法可以照实际调整，调了记进修正记录。

---

## Task 0：开工前核对

**做什么**：对着当时的分支核对下面每一项。

- 有出入的，记进「执行期修正记录」，一项一行，写"计划写 X，代码是 Y，按 Y 做"；按记录调整后面的任务。
- **不改计划的正文。**
- 没有出入也记一行。

1. **工作树**：`git status --short` 空；当前分支 `phase4-m1a`；`3ec732e` 在历史里，之后只多了 M1 的设计与本计划的提交。
2. **读**：设计全文（尤其第 4 ～ 7、13、14、16、20 节）、总设计第 5 节、本计划的「计划期决定」。
3. **名字与签名**：用 `git grep -n` 逐项找到，签名与「计划期决定」写的相同。
   - `rurge-inbound`：
     - `session.rs`：`trait Dialer`（`dial`、`relay`、`admit_udp`、`associate`）、`Dialed`、`DialError`、`SessionHandle::token`；
     - `socks5.rs`：`handle`、`reply`、`REP_SUCCESS`；
     - `http.rs`：`connect`、`session_for`、`Ctx`（`tracker`、`opts`）；
     - `listener.rs`：`ListenerOpts.drop_hold`；
     - `testing.rs`：`FakeDialer`（`new`、`sessions`）与 `echo_server`；
     - `lib.rs` 的导出。
   - `rurge-tun`：
     - `session.rs`：`dial_and_relay`、`tcp_session`、`Serving`（`dialer`、`holds`）、`Holds`（`drop_hold`、`linger`）；
     - `lib.rs`：`Life::session_stop`、`Life::stopped`、`TunHandle::stop`、`TunHandle::join`；
     - `src/tests/outcomes/stop.rs` 的 `Gated`：任务 5 的包装照它写。
   - `rurge-rules`：
     - `set.rs`：`CompiledSet` 的字段、`compile`、`eval`、用例 `extended_matching_applies_to_domain_entries`；
     - `engine.rs`：`RuleEngine::pre_match`、`pre_pass`、`pre_matches`、`CompiledRule` 的 `hits`、用例模块 `mod pre_match`；
     - `matcher.rs`：`CompiledSubRule`、`compile_subrule`；
     - `rurge-config` 的 `RuleKind` 的五种域名规则。
   - `rurge-engine`：
     - `engine.rs`：`Engine` 的字段 `sessions_root`、`shared`、`fake_pool`、`tracker`；`impl Dialer for Engine` 的 `dial` 与 `relay`；`is_suspended`、`suspend`、`mode`、`global_policy`、`registry`、`policy_known`、`runtime`；改出站模式的公开方法（照 `tests/pipeline.rs` 的用法找）；
     - `fake_address.rs`：`turn_back`；
     - `shared.rs`：`EngineShared` 与 `new`；
     - `observe.rs`：`record_of`；
     - `lib.rs` 的导出；
     - `tests/common/mod.rs`：`Harness`、`Profile`、`harness_with`、`connect_via_http`、`connect_via_socks5`、`the_records`、`wait_until`；
     - `tests/common/tun.rs`：`Rig`（`new`、`on`、`fakes`）、`guarded`、`profile`、`tun_records`、`WITHIN`。
   - `rurge-dns`：`fakeip::in_space`、`FakeIpPool::name_of`。
   - `rurge-config`：
     - `General` 的 `force_http_engine_hosts`、`always_raw_tcp_hosts`、`always_raw_tcp_keywords`；
     - `HostList::matches`；
     - `SessionInfo`、`ListenerKind`、`Transport`、`ProtocolKind`（`Http`、`Https`）。
   - `rurge-proto`：`testing::{FakeSocks5, Socks5Script, TlsFixture, echo_server}`、`FakeSocks5::requests` 的元素类型与字段。
   - `rurge-net`：`http::tls_client_config`、`testing::TestServer`。
   - `rurge-api`：`lib.rs` 的 `router`、`routes/mod.rs`、`tests/api.rs` 的 `api()` 与 `get`。
4. **数**：`Cargo.lock` 里 `[[package]]` 的个数（519）；`LISTED` 的项数（23）。
5. **现有用例**（P9）：P9 列的每一条都找到，读它断言什么。把"预计要改 / 预计照过"逐条写进修正记录，一条用例一句；与 P9 的预计不同的，写理由。
6. **门禁**：跑一遍，记下测试二进制数、通过数、失败数与忽略数。

**判据**：修正记录里有任务 0 的行；门禁的数字记下。

---

## Task 1：`rurge-http` 与前缀流

**改哪里**（设计第 3、6.1 节；M1-D26；P2、P13）

1. 根 `Cargo.toml`：`[workspace.dependencies]` 里照别的工作区 crate 的写法加 `rurge-http = { path = "crates/rurge-http" }`，放在 `rurge-inbound` 那一行附近。
2. `crates/rurge-http/Cargo.toml`（新），照 `crates/rurge-inbound/Cargo.toml` 的样子：

   ```toml
   [package]
   name = "rurge-http"
   description = "HTTP engine for rurge: protocol sniffing, plain HTTP/1.1 and MITM"
   version.workspace = true
   edition.workspace = true
   rust-version.workspace = true
   license.workspace = true
   repository.workspace = true

   [dependencies]
   tokio.workspace = true

   [dev-dependencies]
   rustls.workspace = true

   [lints]
   workspace = true
   ```

3. `src/lib.rs`（新）：
   - crate 的文档注释（英文）：阶段 4 的 HTTP 引擎；M1a 有前缀流与嗅探；不认识引擎、监听器与会话。
   - `pub mod prefix;` 与 `pub use prefix::Prefixed;`。
4. `src/prefix.rs`（新）：

   ```rust
   /// The most one `Prefixed::fill` reads (P13).
   pub const FILL: usize = 16 * 1024;

   /// A stream of which the first bytes were read to be looked at (phase 4
   /// M1 design 6.1). They stay in a buffer, and reads give them back before
   /// anything more of the stream, so whoever reads it next gets every byte
   /// the peer sent, in order. Writes and the shutdown go straight through.
   pub struct Prefixed<S> { /* inner: S, buf: Vec<u8>, pos: usize */ }

   impl<S> Prefixed<S> {
       pub fn new(inner: S) -> Prefixed<S>;
       /// What was read and has not been read back yet.
       pub fn buffered(&self) -> &[u8];
   }

   impl<S: AsyncRead + Unpin> Prefixed<S> {
       /// Reads the stream once more, at most `FILL` bytes, onto the end of
       /// the buffer: how many came, 0 at the end of the stream.
       pub async fn fill(&mut self) -> io::Result<usize>;
   }

   impl<S: AsyncRead + Unpin> AsyncRead for Prefixed<S> { /* … */ }
   impl<S: AsyncWrite + Unpin> AsyncWrite for Prefixed<S> { /* … */ }
   ```

   - 读：缓冲里还有就先给缓冲里的（一次至多给读的一方那么多），给完了再读底下的流。缓冲读完之后放掉它的内存（`Vec::new()`），不让 64 KiB 跟着连接活下去。
   - 写：`poll_write`、`poll_flush`、`poll_shutdown`、`poll_write_vectored` 与 `is_write_vectored` 直通。
   - `Prefixed<S>` 在 `S: Unpin` 时是 `Unpin`；`Box::new(Prefixed<BoxedStream>)` 要能当 `BoxedStream` 用（`AsyncRead + AsyncWrite + Unpin + Send`）。

**用例**（`prefix.rs` 末尾的 `#[cfg(test)] mod tests`，用 `tokio::io::duplex`）

- `the_prefix_is_read_before_the_stream`：对端写 `hello`，`fill` 到 `buffered()` 是 `hello`；对端再写 `world` 并关闭；`read_to_end` 得 `helloworld`。
- `a_small_read_takes_the_prefix_in_pieces`：缓冲里有 10 个字节；每次用 3 个字节的缓冲读，依次得 3、3、3、1 个字节，然后才读到对端后写的。
- `writes_and_the_shutdown_go_straight_through`：经 `Prefixed` 写的，对端读得到；`shutdown` 之后对端读到 EOF。
- `fill_at_the_end_reads_nothing`：对端关闭之后 `fill` 是 `Ok(0)`，`buffered()` 不变。

**判据**

- 用例全绿；
- `Cargo.lock` 的 `[[package]]` 是 520 个，多的是 `rurge-http`；
- `crates/rurge-http/Cargo.toml` 的 `[dependencies]` 只有 `tokio`；
- 门禁绿。

---

## Task 2：ClientHello、判断与嗅探（`rurge-http`）

**改哪里**（设计第 6.2 ～ 6.4 节；M1-D5、D6；P3、P4、P17、P18）

1. `src/tls.rs`（新，`pub mod tls`）：

   ```rust
   /// What the engine takes from a TLS ClientHello (phase 4 M1 design 6.2).
   #[derive(Clone, Debug, Default, PartialEq, Eq)]
   pub struct ClientHello {
       /// The first `host_name` of `server_name`, lowercased. `None` when the
       /// extension is not there, or its name is empty or not all ASCII.
       pub sni: Option<String>,
       /// Every protocol name of `application_layer_protocol_negotiation`, in
       /// the client's order; empty without the extension.
       pub alpn: Vec<Vec<u8>>,
   }

   /// Reads a ClientHello from one whole handshake message — its type (1),
   /// its 24-bit length and its body — as TLS records carry it once they are
   /// put together, and as QUIC's CRYPTO frames do (phase 4 M5). `None` when
   /// the message is not a ClientHello or its lengths do not add up.
   pub fn parse_client_hello(message: &[u8]) -> Option<ClientHello>;
   ```

   - 记录的重组写成 crate 内的函数，由 `judge` 调用：把前缀开头的 TLS 记录的负载接成握手的字节，读法见下面的第 2 条。
   - `parse_client_hello` 的读法照 P17。
2. `src/sniff.rs`（新，`pub mod sniff`）：

   ```rust
   /// How long a sniffable session's first bytes are waited for (phase 4 M1
   /// design 6.3, M1-D5).
   pub const SNIFF_WAIT: Duration = Duration::from_secs(15);
   /// The most of a prefix that is read to judge it (design 6.2).
   pub const MAX_PREFIX: usize = 64 * 1024;

   /// What the bytes at the start of a connection are (design 6.2).
   #[derive(Debug, PartialEq, Eq)]
   pub enum Judged {
       /// Not enough to tell. Telling needs at least this many bytes in all,
       /// always more than there are and never more than `MAX_PREFIX` (P3).
       NeedMore(usize),
       Tls(ClientHello),
       Http,
       Other,
   }

   pub fn judge(prefix: &[u8]) -> Judged;

   /// How a sniff ended (design 6.3).
   #[derive(Debug, PartialEq, Eq)]
   pub enum Sniffed {
       Tls(ClientHello),
       Http,
       Other,
       /// The wait ran out first.
       Timeout,
       /// The peer closed its side before the bytes could be judged.
       Closed,
       /// Reading failed: the peer reset the connection, say (P4).
       Failed,
   }

   /// Reads `stream` until its first bytes can be judged, the peer closes
   /// its side, reading fails, or `wait` runs out. What was read stays in
   /// `stream`, to be read back by whoever reads it next.
   pub async fn sniff<S: AsyncRead + Unpin>(stream: &mut Prefixed<S>, wait: Duration) -> Sniffed;
   ```

   `judge` 的规则：
   - 空的前缀：`NeedMore(1)`。
   - 第一个字节是 `0x16`：TLS 一支。
     1. 逐个记录看头（5 个字节）。头没到齐时，已到的头字节都合格就是 `NeedMore(这个记录的开头 + 5)`，否则 `Other`。合格是指：第 2 个字节是 `0x03`，第 3 个字节不大于 `0x04`。
     2. 头齐了，下面任何一条成立就是 `Other`：类型不是 `0x16`；版本不合格；长度是 0 或大于 16384。
     3. 各记录的负载依次接起来就是握手的字节；负载只到了一部分的，到的那部分也接上。
     4. 握手的字节够 4 个时：类型不是 1，`Other`；`4 + 长度` 大于 `MAX_PREFIX`，`Other`。
     5. 握手的字节够 `4 + 长度` 个时，交给 `parse_client_hello`（只交前 `4 + 长度` 个字节）：`Some` 是 `Tls`，`None` 是 `Other`。
     6. 握手的字节还不够时：
        - 当前记录没有到齐，`NeedMore(n)`。`n` 取两者中小的那个：当前记录的末尾，与握手消息在这个记录里结束的位置（还不知道长度时，是握手的头在这个记录里凑齐 4 个字节的位置）。
        - 当前记录到齐了，就看下一个记录，同上。
     7. `NeedMore` 的数大于 `MAX_PREFIX` 时改判 `Other`：前缀不会超过 `MAX_PREFIX` 加一次 `fill` 的量（P3）。
   - 否则：HTTP 一支，逐字节看。
     - 开头 1 ～ 20 个字节都是 `A-Z`、`-` 或 `_`，接着一个空格：这个记号是 `PRI` 时 `Other`，否则 `Http`。
     - 第一个不合格的字节就是 `Other`。第 21 个记号字节算不合格，开头就是空格也算。
     - 字节用完了、到目前为止都合格：`NeedMore(已有的 + 1)`。

   `sniff` 的流程：
   - 截止时刻是现在加上 `wait`。
   - 已有的字节不少于上一次 `NeedMore` 给的数时才再判（第一次直接判）。判出 `Tls`、`Http`、`Other` 就返回。
   - 否则在截止之前 `fill` 一次：
     - 到了截止：`Timeout`；
     - `Ok(0)`：`Closed`；
     - 读出错：`Failed`；
     - 读到了字节：回到上一步。
3. `src/lib.rs`：`pub mod sniff; pub mod tls;`，导出 `ClientHello`、`Judged`、`Sniffed`、`judge`、`sniff`、`SNIFF_WAIT`、`MAX_PREFIX`。

**用例**（`tls.rs` 与 `sniff.rs` 末尾的测试模块；产生 ClientHello 的办法见 P18）

- 读 ClientHello：
  - `a_whole_hello_is_tls_with_its_sni_and_alpn`：rustls 对 `sni.test`、ALPN `h2` 与 `http/1.1` 产生的 ClientHello → `Tls`，SNI 是 `sni.test`，ALPN 两项、次序不变。
  - `every_cut_of_a_hello_needs_more`：同一个 ClientHello 的每个前缀（0 ～ 全长减一）都是 `NeedMore(n)`，`n` 大于前缀的长度、不大于全长。
  - `a_hello_over_several_records_is_put_together`：把它的握手消息重新切成负载为 1、7、100 与其余的四个记录 → 同样的 `Tls`；这样切出来的每个前缀也都是 `NeedMore`。
  - `a_hello_larger_than_a_record_is_read_to_its_end`：手工造、带 20 KiB 填充扩展的 ClientHello，分两个记录 → `Tls`，SNI 照读。
  - `an_upper_case_sni_is_lowered`：字节里的 `sni.test` 原地换成 `SNI.TEST` → SNI 是 `sni.test`。
  - `a_hello_without_a_name_has_no_sni`：rustls 对一个 IP 地址产生的（不带 `server_name`）→ `Tls`，SNI 是 `None`。
  - `a_name_that_is_not_ascii_is_no_sni`：手工造、`host_name` 里有 `0xC3 0xA9` 的 → `Tls`，SNI 是 `None`。
  - `alpn_lists_are_read_whole`：不带 ALPN（空）、一项、三项。
- 判不出、读不懂的：
  - `more_than_64_kib_is_other`：一个握手头说长度是 65536 → 头的 9 个字节一到就 `Other`；一串负载都是 1 个字节的记录，凑不到消息的末尾就超过 `MAX_PREFIX` → `Other`。
  - `another_record_type_midway_is_other`：第一个记录是半个 ClientHello，第二个记录的类型是 `0x17` → `Other`。
  - `lengths_that_do_not_add_up_are_other`：手工造的，逐条都判 `Other`：
    - 扩展块比余下的长；
    - 会话号长度 33；
    - 密码套件的长度是奇数；
    - 没有压缩方法；
    - `server_name` 列表的长度不对；
    - 记录长度 0；
    - 记录长度 16385；
    - 记录版本 `0x03 0x05`；
    - 握手类型 2。
  - `no_damaged_hello_panics`：真 ClientHello 的每个字节逐个换成 `0x00`、`0xff` 与它异或 `0x80` 的值，每种都判一次，不 panic、结果不论。
  - `a_slow_drip_is_not_judged_again_until_it_can_be`：用真 ClientHello 的单个记录（记录不比消息长）。
    - 记录的头的 3 个字节 → `NeedMore(5)`；
    - 头加上一个负载字节 → `NeedMore(9)`（握手的头在这个记录里凑齐的位置）；
    - 头加上握手的头 → `NeedMore(5 + 记录长度)`，即这个记录的末尾。
    - `sniff` 不到这个数不再判，由评审读代码看，不另加只为数次数的东西。
- HTTP 开头：
  - `method_tokens_are_http`：`GET `、`POST `、`M-SEARCH `、`PROPFIND `、`X_CUSTOM `，以及 20 个字符的记号加空格。
  - `pri_is_other`：`PRI * HTTP/2.0\r\n`。
  - `what_is_not_a_method_is_other`：小写的 `get `、21 个字符的记号、开头的空格、`GET\t`。
  - `half_a_method_needs_more`：空、`G`、`POS` → `NeedMore`。
- `sniff`（用 `tokio::io::duplex`；时限用真的定时器，工作区的 tokio 没有开 `test-util`）：
  - `sniff_judges_once_enough_has_come`：ClientHello 分三次写、中间各隔 20 毫秒 → `Tls`。
  - `sniff_returns_at_once_on_other`：`hello` → `Other`。
  - `sniff_gives_up_at_the_wait`：什么也不写，`wait` 200 毫秒 → `Timeout`；用时不少于 200 毫秒、少于 5 秒；`buffered()` 是空的。
  - `sniff_sees_the_peer_close`：写 `GE` 再关闭写端 → `Closed`，`buffered()` 是 `GE`。
  - `sniff_sees_a_read_error`：用例里一个读出错（`ConnectionReset`）的流 → `Failed`。
  - `the_prefix_survives_the_sniff`：判出 `Tls` 之后读到底，得 ClientHello 的全部字节，接着是对端随后写的。

**判据**

- 用例全绿；
- `no_damaged_hello_panics` 在一台机器上一秒以内跑完；
- 生产代码里对外来字节的每个下标都先查长度，外来的长度不 `unwrap`、不直接做下标；
- 门禁绿。

---

## Task 3：规则集一行上的 `extended-matching` 与预匹配的探询（`rurge-rules`）

**改哪里**（设计第 4.3、7.3 节；M1-D3、D10；P6、P7）

1. `crates/rurge-rules/src/set.rs`
   - `CompiledSet` 加字段 `extended: bool`。文档注释：一行域名规则带了 `extended-matching`，整集的域名规则都带（手册 `rules/ruleset.html`："if any domain rule inside the set file already carries the flag, it is applied to all domain rules of that set"）。`empty()` 给 `false`。
   - `compile`：建索引之前先扫一遍 `parsed.lines`，看有没有一行 `SetLine::Rule(sub)` 满足 `sub.extended_matching`、且 `sub.kind` 是五种域名规则之一（P7）。有就是整集的标志。
     - 进了逐条列表的域名规则（`DOMAIN-KEYWORD`、`DOMAIN-WILDCARD`、`DOMAIN-SET`），编出的 `CompiledSubRule.extended` 是 `sub.extended_matching || 整集`。
     - 别的规则照旧是 `sub.extended_matching`。
   - `eval`：域名索引那一段比 SNI 与 `http_host` 的条件，从 `extended` 改成 `extended || self.extended`；逐条的照旧 `extended || rule.extended`，因为 `rule.extended` 已经含了整集的标志。
   - 域名集文件（`SetLine::Domain`）的行没有参数，照旧只看引用它的那一行。
2. `crates/rurge-rules/src/engine.rs`：

   ```rust
   /// Whether `pre_match(s, PreMatchLayer::Syn)` would refuse `s`, without
   /// counting a hit. The engine asks it before it answers a session at a
   /// proxy port that it would sniff (phase 4 M1 design 4.3); the session's
   /// evaluation counts the hit when the rule refuses it there.
   pub fn pre_refuses(&self, s: &SessionInfo) -> bool;
   ```

   与 `pre_match` 共用判断，写法自定（提出一个带"计不计数"参数的私有函数，或别的）；`pre_match` 的行为不变。

**用例**

- `set.rs` 末尾的测试模块（照 `extended_matching_applies_to_domain_entries` 的写法，`SetKind::RuleSet`）：
  - `a_flag_on_one_domain_line_applies_to_every_domain_line`：集合是下面几行：

    ```text
    DOMAIN,exact.com
    DOMAIN-SUFFIX,suffix.com
    DOMAIN-KEYWORD,kw
    DOMAIN-WILDCARD,*.wild.com
    DOMAIN,flag.com,extended-matching
    ```

    目标是 `1.2.3.4`，参数 `extended` 为假。SNI 是 `exact.com`、`x.suffix.com`、`akwb.com`、`a.wild.com` 时各命中对应的那一行。
  - `without_a_flag_a_rule_set_ignores_the_sni`：同一个集合去掉最后一行 → 四个 SNI 都 `NoMatch`。
  - `a_flag_on_a_rule_that_is_not_a_domain_rule_does_not_spread`：集合是 `DOMAIN,exact.com` 与 `URL-REGEX,^http://x\.com/,extended-matching` 两行 → SNI 是 `exact.com` 时 `NoMatch`。
- `engine.rs` 的 `mod pre_match`：`pre_refuses_answers_as_pre_match_and_counts_nothing`。
  - 规则是 `DOMAIN,ads.test,REJECT,pre-matching`。
  - `pre_refuses`：`ads.test:443` 为真，`other.test:443` 为假；问过之后那一行的 `hits()` 是 0。
  - 再 `pre_match(ads.test:443, Syn)` 一次，`hits()` 是 1。

**判据**

- 新用例全绿；
- `set.rs` 现有的用例与 `tests/golden` 照过；
- 门禁绿。

---

## Task 4：`Dialer` 的 `sniffable` 与 `serve`、两个监听器先答后交（`rurge-inbound`）

**改哪里**（设计第 3、5.1、5.2、7.4 节；M1-D4、D11；P8、P10、P11、P15）

1. `src/session.rs`：

   ```rust
   /// How `Dialer::serve` holds and stops a session it was handed (phase 4 M1
   /// design 5, 7.4).
   #[derive(Clone, Debug)]
   pub struct ServeOpts {
       /// Cancelled when whoever handed the connection over stops (the TUN).
       /// `None` from the listeners: their sessions end with the engine's.
       pub stop: Option<CancellationToken>,
       /// REJECT-DROP: how long the connection is held.
       pub drop_hold: Duration,
       /// REJECT-DROP: after the hold, the longest the peer is waited for to
       /// close its side, what it sends being read and discarded. 0 from the
       /// listeners.
       pub linger: Duration,
   }
   ```

   `trait Dialer` 加两个方法：

   ```rust
   /// Whether `session` is to be answered "connected" first and dialed only
   /// after its first bytes are read (phase 4 M1 design 4). Its entry then
   /// answers at once and hands the connection to `serve`. The default:
   /// never.
   fn sniffable(&self, session: &SessionInfo) -> bool {
       let _ = session;
       false
   }

   /// Serves a session whose client was answered "connected" already, to
   /// its end (phase 4 M1 design 5, 7). The default dials it and relays,
   /// cancelling the session at `opts.stop`; REJECT-DROP holds the
   /// connection (`hold_dropped`); any other rejection or failure lets it go.
   fn serve<'a>(
       &'a self,
       session: SessionInfo,
       client: BoxedStream,
       opts: ServeOpts,
   ) -> BoxFuture<'a, ()> { /* … */ }
   ```

   缺省的 `serve` 是这样的：
   - `dial`：
     - `Ok` → `relay`。`opts.stop` 有的话，转发与它竞速：停止先到，就取消会话的令牌（`handle.token()`），再等转发结束（同 `rurge-tun` 的 `dial_and_relay`）。
     - `Reject { kind: Drop }` → `hold_dropped(client, &opts)`。
     - 别的错误 → 放掉流。
2. `src/hold.rs`（新）：

   ```rust
   /// REJECT-DROP for a session that was answered "connected" already (phase
   /// 4 M1 design 7.4): what the peer sends is read and discarded until it
   /// closes its side, reading fails, `opts.stop` fires or `drop_hold` runs
   /// out; then this side is shut down, and what the peer sends is read and
   /// discarded for `linger` at the most, until it closes its side. The stop
   /// ends it at once at any point.
   pub async fn hold_dropped(stream: BoxedStream, opts: &ServeOpts);
   ```

   读的缓冲在堆上（`vec![0; 4096]`），不让 future 大出 4 KiB。`shutdown` 也与停止竞速。
3. `src/lib.rs`：`mod hold;`，导出 `ServeOpts` 与 `hold_dropped`。
4. `src/socks5.rs` 的 `handle`：组好 `session` 之后、`dialer.dial(session)` 之前，加先答后交的一支：

   ```rust
   if dialer.sniffable(&session) {
       // answered first, dialed after the client's first bytes (phase 4 M1
       // design 5.1); a failure to answer ends it here, as below
       stream.write_all(&reply(REP_SUCCESS)).await?;
       let opts = ServeOpts {
           stop: None,
           drop_hold: opts.drop_hold,
           linger: Duration::ZERO,
       };
       dialer.serve(session, Box::new(stream), opts).await;
       return Ok(());
   }
   ```

   写应答失败时照今天返回错误，由监听器已有的 DEBUG 行记下（P8）。
5. `src/http.rs` 的 `connect`：`session_for` 之后、`ctx.dialer.dial(session)` 之前，`sniffable` 为真时：
   - `let upgrade = hyper::upgrade::on(&mut req);`
   - 照下面今天那一支的写法，在 `ctx.tracker` 上起任务、里面再 `tokio::spawn` 一层接 panic。任务里等升级：
     - 升级成功：`dialer.serve(session, Box::new(TokioIo::new(upgraded)), ServeOpts { stop: None, drop_hold: ctx.opts.drop_hold, linger: Duration::ZERO })`；
     - 升级失败：`tracing::debug!(listener = "http", error = %e, "CONNECT upgrade failed before the session began")`，什么也不记（还没有句柄）。
   - 立刻 `return Ok(responses::connect_established())`。
   - 端口照旧必须写明（否则 400）、鉴权照旧在前。
6. `src/testing.rs` 的 `FakeDialer`：加 `pub sniff: AtomicBool`（`new` 给 `false`），`sniffable` 返回它的值；`serve` 用缺省实现（P15）。

**用例**（各文件末尾已有的测试模块，照它们起监听器与 `FakeDialer` 的写法）

- `hold.rs`（用 `tokio::io::duplex`）：
  - `the_hold_ends_at_the_stop`：停止之后一秒以内返回。
  - `the_hold_ends_when_the_peer_closes`：对端写几个字节、再关写端 → 远早于 `drop_hold`（30 秒）返回，对端读到 EOF。
  - `the_hold_lasts_drop_hold_and_then_shuts_down`：`drop_hold` 200 毫秒、`linger` 0 → 用时不少于 200 毫秒、少于 5 秒，对端读到 EOF。
  - `the_linger_reads_until_the_peer_closes`：`drop_hold` 100 毫秒、`linger` 30 秒；对端在读到 EOF 之后再写一些、再关写端 → 远早于 `linger` 返回。
- `socks5.rs`（`dialer.sniff` 设为真）：
  - `a_sniffable_session_is_answered_before_it_is_dialed`：目标 `fail.test:443` → 应答码 `0x00`，随后读到 EOF；`FakeDialer.sessions()` 有一个以失败结束的句柄。
  - `a_sniffable_session_is_relayed_after_the_answer`：目标 `echo.test:443` → `0x00`，回显照常。
  - `a_dropped_sniffable_session_is_held_after_the_answer`：目标 `reject.test:444`（DROP）→ `0x00`；客户端写几个字节，连接不关；客户端关写端之后，五秒以内读到 EOF。
- `http.rs`（`dialer.sniff` 设为真）：
  - `a_sniffable_connect_is_answered_before_it_is_dialed`：`CONNECT fail.test:443` → 200，随后读到 EOF。
  - `a_sniffable_connect_is_relayed_after_the_answer`：`CONNECT echo.test:443` → 200，回显照常。
- `session.rs`：`the_default_serve_cancels_the_session_at_the_stop`。
  - 用 `FakeDialer` 的缺省 `serve` 服务一个到 `echo.test` 的会话；
  - 取消 `opts.stop` 之后，`FakeDialer.sessions()[0].token()` 被取消了；
  - 用例再关客户端，让 `FakeDialer` 的转发结束（它不看令牌）。

**判据**

- 新用例全绿；
- `rurge-inbound` 现有的用例照过（`sniff` 缺省为假）；
- 门禁绿（引擎还没有实现 `sniffable`，引擎的用例照过）。

---

## Task 5：虚拟网卡把可嗅探的会话交给 `serve`（`rurge-tun`）

**改哪里**（设计第 5.3、5.4 节；P12、P15）

1. `crates/rurge-tun/src/session.rs` 的 `dial_and_relay`：`let session = tcp_session(…)` 之后，加：

   ```rust
   if dialer.sniffable(&session) {
       // answered already, the handshake is over: the dialer reads the app's
       // first bytes before it dials (phase 4 M1 design 5.3)
       let opts = ServeOpts {
           stop: Some(life.session_stop()),
           drop_hold: serving.holds.drop_hold,
           linger: serving.holds.linger,
       };
       dialer.serve(session, Box::new(stream), opts).await;
       return;
   }
   ```

   - fake 地址的租约（开头绑定的 `_lease`）一直握到函数结束。
   - 文件头的 `//!` 注释补一句：可嗅探的会话整个交给拨号器的 `serve`。
   - `use rurge_inbound::{DialError, Dialer, ServeOpts};`。

**用例**（`src/tests/outcomes/` 下新的 `serve.rs`，在 `outcomes/mod.rs` 里登记；照同一目录 `stop.rs` 的写法与夹具）

一个包装的拨号器（照 `stop.rs` 的 `Gated`，P15）：`sniffable` 为真；`serve` 把拿到的 `ServeOpts` 存起来，再用里面那个 `FakeDialer` 的 `dial` 与 `relay` 做缺省实现做的事。

- `a_sniffable_connection_is_served_with_the_tuns_holds`：
  - 应用连上之后回显照常；
  - `serve` 被调了一次，`opts.stop` 是 `Some`，`drop_hold` 与 `linger` 等于起 TUN 时的 `TunOptions`（用例给两个不同于缺省的值）。
- `the_stop_reaches_a_served_connection`：
  - 包装的 `serve` 只等 `opts.stop` 被取消；
  - 停 TUN 之后它返回，`join` 在用例的时限之内结束。
- `a_served_connection_keeps_its_fake_address_leased`：
  - 到 fake 地址的连接在 `serve` 里时，池里这个地址是租出的；
  - `serve` 返回之后不再租出。
  - 看法：
    - 起 TUN 用 `stop.rs` 用的 `tun_dialing`，`Extras` 的 `pool` 给用例自己的池；
    - 到 fake 地址的连法照 `src/tests/fake.rs` 的 `a_connection_to_a_fake_address_dials_its_name`；
    - 租着没有，看 `FakeIpPool::leased()`：`serve` 里时是 1，返回之后在用例的时限之内变成 0。

**判据**

- 新用例全绿；
- `rurge-tun` 现有的用例照过（`FakeDialer` 不嗅探）；
- 门禁绿。

---

## Task 6：引擎：判定、嗅探之后、记录与计数（`rurge-engine`）

**改哪里**（设计第 4、6.3、7 节；M1-D2、D5、D7、D8、D9、D11、D24；P4 ～ P6、P9、P10、P16、P19）

1. `Cargo.toml`：`[dependencies]` 加 `rurge-http.workspace = true`。
2. `src/shared.rs`：`EngineShared` 加一个字段，`new` 里给 `rurge_http::sniff::SNIFF_WAIT`：

   ```rust
   /// How long a sniffable session's first bytes are waited for (phase 4 M1
   /// design 6.3): `rurge_http::sniff::SNIFF_WAIT`. Only tests shorten it.
   pub sniff_wait: Duration,
   ```

3. `src/serve.rs`（新）。文件头注释：先答后读的会话——哪些是、嗅探、嗅探之后。
   - `pub(crate) fn sniffable_session(&self, session: &SessionInfo) -> bool`（`impl Engine`），依次判，任何一条不成立就是假：
     1. `session.transport` 是 `Tcp`，`session.listener` 是 `Http`、`Socks5` 或 `Tun`；
     2. 引擎没有暂停（`is_suspended()`）；
     3. 探询用的会话（P5）：
        - 经 HTTP 与 SOCKS5 监听器来的、目标是 rurge 地址空间里的 IP（`in_space`）、引擎有 fake 池（`fake_pool.load_full()`）时，`name_of(ip.to_canonical())`：
          - 有名字：探询用的会话是一份目标换成这个名字的拷贝；
          - 没有名字：假。
        - 别的会话就用它自己；
     4. 设置读自 `self.runtime()` 的 `config.general`。`always_raw_tcp_hosts.matches(主机, 端口)` 是 `Some(true)`：假；
     5. 主机是名字、`always_raw_tcp_keywords` 里任何一个非空的关键字（比较时转小写）是它的子串：假；
     6. 端口不是 80、443，而 `force_http_engine_hosts.matches(主机, 端口)` 不是 `Some(true)`：假；
     7. 经 HTTP 与 SOCKS5 监听器来的会话、且拨号时会走到规则时（P6），`runtime().rules.pre_refuses(&探询用的会话)` 为真：假。"会走到规则"是：模式 `Mode::Rule`，或 `Mode::Proxy` 而全局策略不可用；
     8. 其余：真。
   - `pub(crate) async fn serve_sniffed(&self, session: SessionInfo, client: BoxedStream, opts: ServeOpts)`：

     ```rust
     let mut client = Prefixed::new(client);
     // The sniff holds no handle and makes no record: a stop ends it at
     // once and lets the stream go (a reset on the TUN).
     let sniffed = tokio::select! {
         biased;
         _ = self.sessions_root.cancelled() => return,
         _ = stopped(opts.stop.as_ref()) => return,
         sniffed = rurge_http::sniff(&mut client, self.shared.sniff_wait) => sniffed,
     };
     self.sniff_counts.count(&sniffed);
     let mut session = session;
     match sniffed {
         Sniffed::Tls(hello) => {
             session.sni = hello.sni;
             session.protocol = Some(ProtocolKind::Https);
         }
         Sniffed::Http => session.protocol = Some(ProtocolKind::Http),
         Sniffed::Other | Sniffed::Timeout | Sniffed::Closed => {}
         // the client is gone: nothing to dial for, nothing to record
         Sniffed::Failed => return,
     }
     let client: BoxedStream = Box::new(client);
     // Not raced with the stop: the dial holds the session's handle, and a
     // dial dropped half way would leave its record open (as on the TUN).
     match self.dial(session).await {
         Ok(dialed) => {
             let token = dialed.handle.token().clone();
             let mut relay = self.relay(client, dialed.stream, dialed.handle);
             tokio::select! {
                 biased;
                 _ = stopped(opts.stop.as_ref()) => {
                     token.cancel();
                     relay.await;
                 }
                 _ = &mut relay => {}
             }
         }
         Err(DialError::Reject { kind: RejectKind::Drop, .. }) => {
             tokio::select! {
                 biased;
                 _ = self.sessions_root.cancelled() => {}
                 _ = hold_dropped(client, &opts) => {}
             }
         }
         // any other rejection and a failed dial let the stream go
         Err(_) => {}
     }
     ```

     - `stopped(None)` 永不完成，`stopped(Some(t))` 等 `t` 被取消。
     - 名字照实际调整，语义照上面。
   - 计数：
     - `SniffCounts`（`pub(crate)`，五个 `AtomicU64`）：`count(&Sniffed)`，`Closed` 与 `Failed` 都算 `closed`（P4）；`snapshot() -> SniffedCounts`。
     - `HttpCounts` 与 `SniffedCounts` 是 `pub`（P16）。
4. `src/engine.rs`：
   - `Engine` 加字段 `sniff_counts: SniffCounts`，在 `new` 里建；
   - `impl Dialer for Engine` 加 `sniffable`（调 `sniffable_session`）与 `serve`（`Box::pin(self.serve_sniffed(…))`）；
   - 加 `pub fn http_counts(&self) -> HttpCounts`。
5. `src/observe.rs` 的 `record_of`：`sni` 改成 `s.sni.clone().or_else(|| h.sni())`。注释写理由：嗅探读出的完整 SNI 在会话信息里，侧槽是 `relay` 的首块嗅探写的，至多 8 KiB；首块嗅探给不可嗅探的会话记 SNI，不动它（设计第 7.2 节）。
6. `src/lib.rs`：`mod serve;`，`pub use serve::{HttpCounts, SniffedCounts};`。
7. 现有用例：照 P9 改；改了哪些写进提交说明与报告。

**用例**（`tests/sniff.rs`（新），开头 `mod common;`，照 `tests/tun.rs` 的引用写法；夹具照 P14）

- `which_sessions_are_sniffable`：
  - Profile 的 `[General]`：

    ```text
    force-http-engine-hosts = forced.test, *.anyport.test:0
    always-raw-tcp-hosts = raw.test
    always-raw-tcp-keywords = RawKw
    ```

  - 逐条 `h.engine.sniffable(&SessionInfo { listener, transport, ..SessionInfo::tcp(主机, 端口) })`：

    | 入口 | 传输 | 主机 | 端口 | 期望 |
    | --- | --- | --- | --- | --- |
    | SOCKS5 | TCP | `target.test` | 443 | 真 |
    | SOCKS5 | TCP | `target.test` | 80 | 真 |
    | SOCKS5 | TCP | `target.test` | 8443 | 假 |
    | HTTP | TCP | `forced.test` | 8080 | 假（缺省端口 80） |
    | SOCKS5 | TCP | `a.anyport.test` | 8080 | 真 |
    | 虚拟网卡 | TCP | `raw.test` | 443 | 假 |
    | SOCKS5 | TCP | `xrawkwx.test` | 443 | 假（关键字不分大小写） |
    | SOCKS5 | TCP | `10.1.2.3` | 443 | 真 |
    | SOCKS5 | UDP | `target.test` | 443 | 假 |
    | 内部 | TCP | `target.test` | 443 | 假 |
    | 端口转发 | TCP | `target.test` | 443 | 假 |

- `a_paused_engine_sniffs_nothing`：`suspend(Some(…))` 之后 `target.test:443` 为假；`suspend(None)` 之后为真。
- `a_session_that_pre_matching_refuses_is_not_sniffed`：
  - 规则 `DOMAIN,ads.test,REJECT,pre-matching`；
  - SOCKS5 的 `ads.test:443` 为假，虚拟网卡的为真（P6）；
  - 出站模式改成直连之后，SOCKS5 的也为真。
- `fake_addresses_at_a_proxy_port_are_judged_by_their_names`：
  - `Rig`，`[General]` 写 `always-raw-tcp-hosts = raw.test`；
  - SOCKS5 的会话：`raw.test` 的 fake 地址的 443 为假，`target.test` 的为真；`198.19.255.254`（没有名字）的 443 为假。
- `extended_matching_rules_a_tls_session_by_its_sni`（经 SOCKS5，最主要的一条）：
  - 策略 `A`、`B` 各指向一个 `FakeSocks5`，都连到 `TlsFixture` 的回显服务器；
  - 规则 `DOMAIN,sni.test,B,extended-matching` 与 `DOMAIN,target.test,A`；
  - 客户端经 SOCKS5 连 `target.test:443`，用 SNI `sni.test` 握手、回显一次；
  - `B` 被问了一次（`target.test`、443），`A` 一次也没有；
  - 记录：规则是那条 `DOMAIN,sni.test,…`，策略 `["B"]`，`sni` 是 `sni.test`，`protocol` 是 `Https`。

**判据**

- 新用例全绿；
- P9 的改动照列；
- 门禁绿。

---

## Task 7：引擎的端到端用例

**用例**（都在 `tests/sniff.rs`；夹具照 P14；`sniff_wait` 300 毫秒，另有写明的除外）

1. `extended_matching_rules_a_tls_session_by_its_sni_on_every_entry`：同任务 6 那一条，经 `CONNECT` 与虚拟网卡各一次。
   - 虚拟网卡上，应用连 `target.test` 的 fake 地址的 443，规则用 `guarded`；
   - 记录的 `listener` 各是 `http` 与 `tun`。
2. `without_extended_matching_the_sni_decides_nothing`：规则 `DOMAIN,sni.test,B`（不带标志）→ `A`。
3. `protocol_https_refuses_before_the_dial`：
   - 规则 `PROTOCOL,HTTPS,REJECT` 排在 `DOMAIN,target.test,A` 之前；
   - 经 SOCKS5：应答码 `0x00`，TLS 握手在五秒以内失败；`A`、`B` 都没有被问；
   - 记录：以 `REJECT` 被拒，`protocol` 是 `Https`，`sni` 是 `sni.test`；
   - 经 `CONNECT`：200，握手同样失败。
4. `a_plain_http_session_is_marked_http`：
   - `CONNECT target.test:80`，`A` 连到 `TestServer`；客户端发一个 `GET`，拿到 200；
   - 记录的 `protocol` 是 `Http`；
   - 另起一个引擎，规则加一条 `PROTOCOL,HTTP,B`（排在前面）→ `B` 被问。
5. `always_raw_tcp_hosts_keep_the_dial_first`：
   - `A` 的 `FakeSocks5` 拒绝（`reply: 5`）；
   - `always-raw-tcp-hosts = target.test` 时，经 SOCKS5 连 `target.test:443` 的应答码是 `0x05`；
   - 没有这个设置时是 `0x00`，客户端发几个字节之后读到 EOF。
6. `always_raw_tcp_keywords_ignore_case`：`always-raw-tcp-keywords = TARGET` → 同上的 `0x05`。
7. `force_http_engine_hosts_sniff_another_port`：同样的 `A`；经 SOCKS5 连 `target.test:8080`：
   - `force-http-engine-hosts = target.test:8080` 时 `0x00`；
   - `force-http-engine-hosts = target.test`（缺省端口 80）时 `0x05`；
   - 没有设置时 `0x05`。
8. `pre_matching_still_refuses_before_the_answer`：
   - 规则 `DOMAIN,ads.test,REJECT,pre-matching`，`[Host]` 把 `ads.test` 钉到 `127.0.0.1`；
   - SOCKS5 连 `ads.test:443` → `0x02`；`CONNECT ads.test:443` → 连接关闭、没有应答（同 `tests/pipeline.rs` 今天的断言）；
   - 同样的规则去掉 `pre-matching`：SOCKS5 → `0x00`。
9. `a_paused_engine_still_refuses_before_the_answer`：`suspend(Some(…))` 之后经 SOCKS5 连 `target.test:443`。
   - 应答码是今天暂停时给的那一个（任务 0 读出是哪一个），不是 `0x00`；
   - 记录照今天。
10. `a_server_first_protocol_passes_after_the_wait`：
    - `A` 连到一个回环上的服务器，它一接受就写 `220 hello\r\n`；
    - 客户端经 SOCKS5 连 `target.test:443`，什么也不写，读到 `220 hello`；
    - 用时不少于 300 毫秒、少于五秒；
    - 记录的 `protocol` 是 `null`。
11. `the_prefix_reaches_the_upstream_whole`：`A` 连到 TCP 回显服务器。
    - 客户端写 `GE`，隔 100 毫秒，再写 `T / HTTP/1.1\r\nHost: x\r\n\r\n` 与 10 KiB 的字节；
    - 读回的与写的逐字节相同。
    - 那 100 毫秒是为了让嗅探分两次读到；它不是断言什么都没发生的等待，所以注释写清它的用意。
12. `a_client_that_closes_during_the_wait_is_still_dialed`：
    - 客户端写 `GE` 再关写端，读回 `GE` 与 EOF；
    - 记录以完成结束，`protocol` 是 `null`。
13. `a_flag_on_one_line_of_a_rule_set_matches_the_sni`（L314 经引擎）：
    - 内联的 `[Ruleset sni]` 节写 `DOMAIN,sni.test` 与 `DOMAIN,flag.test,extended-matching` 两行；
    - 规则 `RULE-SET,sni,B`；
    - 任务 6 那样的 TLS 会话 → `B`。
14. `the_sniffs_are_counted`：同一个引擎上五个经 SOCKS5 的会话。
    - 五个会话依次是：TLS 握手、`GET / HTTP/1.1\r\n\r\n`、`hello`、什么也不写、`GE` 再关写端；
    - 等它们都结束，`h.engine.http_counts().sniffed` 是各 1。
15. `a_stop_during_the_wait_ends_a_tun_session_at_once`：
    - `Rig`，`sniff_wait` 用缺省的 15 秒；
    - 应用连 `target.test` 的 fake 地址的 443，什么也不写；
    - 停 TUN 之后，应用的读在 `WITHIN` 之内以错误或 EOF 结束；
    - 这个会话没有记录。这一句是"什么都没发生"的断言，注释写它等的是什么、余量多大。
16. `a_dropped_tun_session_ends_when_the_app_closes`：
    - `Rig`，规则 `DOMAIN,drop.test,REJECT-DROP`（`guarded`）；
    - 应用连 `drop.test` 的 fake 地址的 443，写 `hello`，再关写端；
    - 应用在 `WITHIN` 之内读到 EOF（缺省的 `drop_hold` 是 30 秒）。

**判据**

- 用例全绿；
- 带等待的十条单独循环跑二十次以上，全过；
- 门禁绿。

---

## Task 8：`GET /v1/rurge/http`（`rurge-api`）

**改哪里**（设计第 11 节；M1-D24；P16）

1. `src/routes/rurge_http.rs`（新）：
   - `GET /v1/rurge/http` 读 `app.engine.http_counts()`；
   - 答 `{"sniffed":{"tls":0,"http":0,"other":0,"timeout":0,"closed":0}}` 这个形状，键序照写，`serde` 的结构体；
   - 文件头注释：rurge 专有的端点，`/v1/rurge/…` 是给 rurge 专有接口的前缀（阶段 4 总设计），M1b 加 `requests`。
2. `src/routes/mod.rs` 加 `pub mod rurge_http;`；`src/lib.rs` 的 `router` 加 `.route("/v1/rurge/http", get(routes::rurge_http::http))`。

**用例**（`tests/api.rs`）

- `rurge_http_counts_start_at_zero`：
  - 应答与 `{"sniffed":{"tls":0,"http":0,"other":0,"timeout":0,"closed":0}}` 逐字段相同；
  - 不带钥匙时同别的端点一样被拒（照这个文件已有的写法）。

**判据**

- 用例全绿；
- `rurge-api` 的 `Cargo.toml` 不变；
- 门禁绿。

---

## Task 9：文档收尾（设计第 13、15 节）

1. `docs/surge-compatibility-matrix.md`：
   - L131：`force-http-engine-hosts` 的缺省端口 80 在运行时由嗅探的判定读。
   - L172 ～ L174：三个设置生效，写 rurge 的读法：
     - 名单的第一个命中项定、排除项照 Host List；
     - 关键字不分大小写、只比名字（IP 不是 hostname）；
     - M1a 里 `force-http-engine-hosts` 命中的会话被嗅探、照原样转发，M1b 起非 TLS 的进 HTTP 引擎。
   - L265：
     - 可嗅探的会话（80、443、强制的）在规则之前就有 SNI 与协议，`extended-matching` 比 SNI；
     - 不可嗅探的端口上 SNI 照旧只进记录；
     - `Host` 随 M1b。
   - L314：做完；写做法（五种域名规则、`DOMAIN-SET` 文件的行照旧）。
   - 7.1 节 L562 ～ L565：嗅探、等待 15 秒、先答后读、判为 HTTP 的在 M1a 照原样转发；L570 不动（M1b）。
   - 设计第 13 节第 1 ～ 3 条登记在对应行的备注里。
   - 状态标记照图例：有登记的差异才从 ✅ 改成 🟡；改了的写进附录。附录的统计照这一份的规则重数（有脚本的话说明脚本不在仓库里）。
   - 文件开头"最近一次"的那一句随之改。
2. `docs/api/phase4.md`（新）：
   - `GET /v1/rurge/http`：形状、各键的意思、自进程启动起累计、重载不清零、M1b 加 `requests`；
   - 请求记录：可嗅探的会话的 `sni` 与 `protocol` 在规则之前就定了；不可嗅探的、判为别的、超时与关闭的 `protocol` 是 `null`；
   - 日志：嗅探不另写日志；`CONNECT` 升级失败那一行 DEBUG。
3. `docs/testing.md`：
   - 「命令」里加 `rurge-http` 与 `--test sniff` 的过滤词；
   - 「测试约定（全文）」加一节「阶段 4 / M1a 起：嗅探」：
     - 绑不了 80、443 时怎样让会话走到回环上的服务器（P14）；
     - `EngineShared.sniff_wait`；
     - 讲"拨号之后才答"的用例用不嗅探的端口（P9）。
   - `CLAUDE.md`「测试约定」里没有要随之改的句子；若有，两处一起改。
4. `docs/status.md`：
   - 在「## 附：依赖方向一条的原文」之前加「## 阶段 4（HTTP 引擎）」与「### M1a（嗅探与规则之前的 SNI）」一节：做了什么、没做的与不要当成已经有的、以后的会话必须知道的事、去向写着项目所有者的事。细节指到本计划的两张表。
   - 依赖的附录加一行：阶段 4 / M1a 加了 `rurge-engine → rurge-http`；`rurge-http` 只依赖 `tokio`。
5. `CLAUDE.md`：
   - 「现在在哪里」阶段 4 那一条改成 M1a 做完、下一步 M1b；
   - 「架构」的一句照 P20 改。
6. `docs/README.md`：加 `docs/api/phase4.md` 一项。
7. `README.md`、`README_en.md`、`docs/requirements.md` 不改：设计第 15 节把它们放在 M1b 收尾。

**判据**

- 文档里引的提交、用例名与数字对着仓库核过；
- 门禁绿（只改文档也跑）。

---

## 计划期决定

| # | 事项 | 决定 |
| - | ---- | ---- |
| P1 | 引用的名字与行号 | 对着 `3ec732e` 核对过（下面各行）。行号会随前面任务的改动漂移，以名字为准；名字不同就照实改、记进修正记录，不改语义 |
| P2 | `rurge-http` 在 M1a 的依赖 | 设计第 3 节写的四条边（`rurge-config`、`rurge-net`、`rurge-inbound`、`rurge-proto`）是 M1b 的 HTTP 引擎要用的。M1a 的三个模块只用 tokio 的 `AsyncRead` / `AsyncWrite` 与定时器，所以 M1a 只加 `tokio`（`[dependencies]`）与 `rustls`（`[dev-dependencies]`，产生真的 ClientHello），四条边在 M1b 用到时再加。`rurge-engine → rurge-http` 在任务 6 第一次用到时加。`Cargo.lock` 从 519 个包变成 520 个，多的是 `rurge-http` 自己 |
| P3 | `Judged::NeedMore` 带字节数 | 设计第 6.2 节的 `NeedMore` 不带东西。一个字节一个字节发来的客户端（15 秒里能发很多段）会让 `sniff` 每来一段就从头解析一遍，最坏是 64 KiB 的平方。所以 `judge` 给出"至少要多少字节"（总长），`sniff` 不到这个数不再判。TLS 时这个数是当前记录的末尾与握手消息末尾所在位置中小的那个，HTTP 时是已有的加一。这个数大于 `MAX_PREFIX` 时改判 `Other`，缓冲因此不超过 `MAX_PREFIX` 加一次 `fill` 的量（设计第 6.2 节"读的总量超过 64 KiB，判为 Other"） |
| P4 | 读出错与计数 | 设计第 6.3 节说读出错时 `sniff` 返回 `Closed` 并带上"出错"。这里写成第六个变体 `Sniffed::Failed`，计数算在 `closed` 里（设计第 7.5 节："含读出错"）。嗅探被停止打断时没有结局，不计数 |
| P5 | fake 地址背后没有名字 | 设计第 4.1 节没有说经监听器来的 fake 地址背后没有名字时怎样。这样的会话拨号时一定以 `fake address not known` 失败（`turn_back` 的 `Unknown`）；先答"成功"只会把今天的失败应答（SOCKS5 的 `0x05`、`CONNECT` 的 502 页）换成一个随即关闭的隧道。所以不嗅探，照今天的路走：`tests/tun_dns.rs` 的 `a_socks5_session_to_an_unknown_fake_address_is_refused` 不改照过。有名字时，第 4 ～ 7 条的名单、关键字与预匹配都对名字比 |
| P6 | 预匹配的探询 | 设计第 4.3 节说"规则模式下"问预匹配。写细几点：（一）只对经 HTTP 监听器与 SOCKS5 来的会话问；虚拟网卡的会话在 SYN 时问过（阶段 3 / M3c）。（二）只在拨号时会走到规则时问："会走到规则"是模式 `Mode::Rule`，或 `Mode::Proxy` 而全局策略不可用（`global_policy()` 是 `None`，或 `policy_known` 为假），与 `choose_policy_with` 里的条件相同；暂停时前一条已经不嗅探。（三）探询看的会话是 fake 地址换成名字之后的那一份（P5）。（四）`choose_policy_with` 里先于模式的"外部程序的会话一律直连"一支，探询时不认：它要等进程，而 `sniffable` 是同步的。探询拒了这样的会话，结果只是不嗅探它，拨号时照今天直连 |
| P7 | 整集的标志算哪些行 | 设计第 7.3 节列了四种域名规则。规则集文件里的 `DOMAIN-SET` 一行也是域名规则（`RuleKind::supports_pre_matching` 与 `matcher.rs` 的域名一族都把它算在内），也算进来：它带标志时整集都带；整集带时它也带，求值时把标志传给它引用的域名集，同今天的 `rule.extended`。所以是五种：`DOMAIN`、`DOMAIN-SUFFIX`、`DOMAIN-KEYWORD`、`DOMAIN-WILDCARD`、`DOMAIN-SET`。带标志的若是别的规则（如 `URL-REGEX`），不扩到整集。域名集文件（`SetLine::Domain`）的行没有参数，照旧只看引用它的那一行 |
| P8 | 两条日志 | 设计第 12 节说"SOCKS5 写应答失败（都还没有句柄）各一行 DEBUG，不带地址"。SOCKS5 的 `handle` 今天把写应答的错误返回给监听器，监听器已有一行 DEBUG（`socks5 session ended with an error`，带 `source`：经监听器来的会话的来源地址在 DEBUG 今天就有），所以照旧返回错误，不另写一行。`CONNECT` 升级失败、还没有会话时加一行：`tracing::debug!(listener = "http", error = %e, "CONNECT upgrade failed before the session began")`，不带地址 |
| P9 | 现有用例 | 任务 6 之后 `Engine::sniffable` 真的返回真，现有用例里经 SOCKS5、`CONNECT` 或虚拟网卡连 80、443 的会话改走先答后读。做法照设计第 14 节第 6 条：（一）讲"拨号之后才答"的（应答码、错误页、拒绝时的重置），与客户端不先发字节就等结果的：目标端口改成不嗅探的（443 → 8443，80 → 8080），断言里写到这个端口的地方（`dst`、规则）随之改，其余断言不改；改的地方加一行英文注释，例如 `// 8443 is not sniffed (phase 4 M1 design 4.1): the answer still waits for the dial`。（二）客户端先发字节的照过，不改。写计划时 `git grep` 出来的（对着 `3ec732e`）——**预计要改**：`tests/outbounds.rs` 的 `a_refusing_upstream_is_a_502_that_quotes_the_proxy`、`a_broken_hop_is_named_in_the_error`；`tests/pipeline.rs` 的 `reject_rules_close_serve_gifs_or_render_pages`（`CONNECT` 与 SOCKS5 两处）、`dns_failure_and_ip_rules`（`CONNECT nx.test:443`）；`tests/process.rs` 的 `a_record_shows_a_process_found_after_the_session_ended`；`tests/tun.rs` 的 `a_rejected_tcp_connection_is_reset`（应用只读不写，嗅探要等满 15 秒）。**预计照过**：`tests/network.rs` 的 `visit` 那一处；`tests/outbounds.rs` 的 `a_local_host_item_reaches_the_proxy_as_an_ip`、`a_local_host_item_with_several_addresses_gives_the_proxy_one_at_random`、`a_pinned_host_turns_forwarding_into_a_tunnel`、`a_selection_applies_to_the_next_connection_and_survives_a_restart`、`a_subnet_group_follows_the_network_the_engine_is_on`；`tests/outbounds_ssh.rs` 的 `a_keystore_key_logs_in_to_a_pinned_server`、`an_ssh_session_runs_inside_shadow_tls`；`tests/smart.rs` 的 `the_first_byte_back_is_reported`、`three_seconds_without_an_answer_count_against_the_member`；`tests/tun.rs` 的 `tcp_to_a_documentation_address_follows_the_rules`；`tests/tun_dns.rs` 的 `pre_matching_at_the_syn_layer_refuses_an_address_too`（SYN 时就拒了，不到 `serve`）与 `a_socks5_session_to_an_unknown_fake_address_is_refused`（P5）。任务 0 逐条读一遍，把预计写进修正记录；任务 6 以跑出来的为准：预计照过而变红的，按上面的两类判断，判断不了的停下来报告；名单之外变红的同样处理。不为让用例变绿改别的期望值 |
| P10 | 停止 | 设计第 7.1 节说嗅探与 `opts.stop`、会话根令牌竞速。还有两处：`hold_dropped` 在引擎里也与会话根令牌竞速，守护进程退出时不再停到 `drop_hold`；`rurge-inbound` 缺省的 `serve` 没有会话根令牌，只看 `opts.stop`。转发照旧由会话的令牌结束，它是会话根令牌的孩子 |
| P11 | 监听器给的 `ServeOpts` | `ServeOpts { stop: None, drop_hold: ListenerOpts.drop_hold, linger: Duration::ZERO }`。监听器的会话随引擎结束（会话根令牌） |
| P12 | 虚拟网卡给的 `ServeOpts` | `ServeOpts { stop: Some(life.session_stop()), drop_hold: serving.holds.drop_hold, linger: serving.holds.linger }`。`dial_and_relay` 开头绑定的 `_lease` 一直握到 `serve` 返回（函数结束）。DNS 的连接（`Purpose::Dns`）不经这里。`serve` 返回装箱的 future，`dial_and_relay` 的 future 不因此变大多少（文件里关于 future 大小的注释照旧适用）。`linger` 到时应用仍开着它那一侧的，流被放掉时重置，同 `close_and_wait_for_the_app`（设计第 5.4 节） |
| P13 | `Prefixed` | `fill` 每次至多读 `FILL`（16 KiB）个字节，接到缓冲的末尾。读的一方把缓冲读完之后放掉它的内存（`Vec::new()`），不让 64 KiB 跟着连接活下去。写、`flush`、关写端、`poll_write_vectored` 与 `is_write_vectored` 直通 |
| P14 | 端到端用例的夹具 | 绑不了 80、443：规则把会话送到 SOCKS5 策略 `A` / `B`，各指向一个 `FakeSocks5`（`Socks5Script { connect_to: Some(回环上的服务器), ..Socks5Script::default() }`）；看哪一个被问到（`requests()` 的 `command`、`host`、`port`）就知道哪条规则命中；要它拒绝时 `reply: 5`。TLS：`TlsFixture::new(&["sni.test"])` 与 `spawn_echo(false)`；客户端用 `rurge_net::http::tls_client_config(true)`（不校验，同 `tests/pipeline.rs` 的 `connect_tunnel_carries_tls_to_the_target`）与 `tokio_rustls::TlsConnector`，`ServerName` 是 `sni.test`。虚拟网卡上 `AppTcp` 实现了 `AsyncRead` / `AsyncWrite`，同样能跑 TLS 客户端。明文：`rurge_proto::testing::echo_server()` 与 `TestServer::spawn()`。等待：`EngineShared { sniff_wait: Duration::from_millis(300), ..EngineShared::default() }` 交给 `harness_with`。名字：`target.test` 经 MockDns 落在 `127.0.0.1`；`sni.test` 只在 ClientHello 里，不被解析；别的名字在监听器的用例里加一行 `[Host]` 钉到 `127.0.0.1`，在虚拟网卡的用例里用 `guarded` |
| P15 | 测试拨号器 | `rurge-inbound` 的 `FakeDialer`（`src/testing.rs`，`cfg(test)`）加 `pub sniff: AtomicBool`（缺省假），`sniffable` 返回它，`serve` 用缺省实现，现有用例不碰它。`rurge-tun` 的 `testing::FakeDialer`（`testing` 特性，引擎的用例也用）不改：任务 5 的用例另写一个包装的拨号器（照 `src/tests/outcomes/stop.rs` 的 `Gated`），`sniffable` 为真，`serve` 记下拿到的 `ServeOpts`，再用里面那个 `FakeDialer` 的 `dial` 与 `relay` 做缺省实现做的事 |
| P16 | API 与类型 | M1a 的 `GET /v1/rurge/http` 只有 `sniffed`（设计第 11 节）：`{"sniffed":{"tls":0,"http":0,"other":0,"timeout":0,"closed":0}}`，M1b 加 `requests`。`HttpCounts { sniffed: SniffedCounts }` 与 `SniffedCounts { tls, http, other, timeout, closed }`（都是 `u64`，`Clone, Copy, Debug, Default, PartialEq, Eq`）定义在 `rurge-engine` 的 `serve.rs`，由它的 `lib.rs` 导出，`Engine::http_counts()` 读。`rurge-api` 不认识 `rurge-http` 的类型。路由的文件是 `routes/rurge_http.rs` |
| P17 | ClientHello 的读法 | TLS 记录头：`0x16`、`0x03`、`0x00..=0x04`，长度 1 ～ 16384。握手消息：类型 1、24 位长度，`4 + 长度` 大于 64 KiB 判 `Other`。ClientHello 的体依次是：版本 2 字节、随机数 32 字节、会话号（长度不超过 32）、密码套件（长度是偶数且至少 2）、压缩方法（长度至少 1）；之后没有字节，就是没有扩展；有的话，扩展块的长度必须恰好填满余下的字节。`server_name`（类型 0）取第一个名字类型为 0（`host_name`）的项：空的或不全是 ASCII 当作没有；转小写；末尾的点不去（RFC 6066 不许它出现）。ALPN（类型 16）取全部协议名，每个长度至少 1，存成 `alpn: Vec<Vec<u8>>`；M1a 不用它，M2 的 MITM 用。两个扩展内部的长度对不上时，整个 ClientHello 读不懂（`Other`）。重复的扩展取第一个 |
| P18 | 真的与手工造的 ClientHello（V8） | 本机 rustls 0.23.43 只开了 `ring`（工作区 `Cargo.toml`）。`rustls::crypto::ring::DEFAULT_KX_GROUPS` 就是 `ALL_KX_GROUPS`：X25519、secp256r1、secp384r1，没有 ML-KEM，所以 rustls 产生的 ClientHello 只有几百字节。用例的办法：（一）真的 ClientHello 由 rustls 产生：`ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))`，加空的 `RootCertStore` 与 ALPN，再 `ClientConnection::new(…)` 与 `write_tls(&mut Vec<u8>)`。（二）跨记录的，由真 ClientHello 的握手消息重新切成几个记录。（三）大于一个记录的手工造，加一个类型 21 的填充扩展。（四）没有 SNI 的，用 IP 地址作 `ServerName`（rustls 不为 IP 发 SNI）。（五）大写的 SNI：把字节里的 `sni.test` 原地换成 `SNI.TEST`，长度不变，别处的长度都不用改 |
| P19 | 引擎的新模块名 | `serve.rs`。引擎里已有 `sniff.rs`（`parse_sni`、`is_stun`，给 `relay` 的首块嗅探与 UDP 用），不动它 |
| P20 | `CLAUDE.md`「架构」的一句 | "`rurge-http`（HTTP 引擎 / MITM / 重写 / 抓包）与 `rurge-script` 到后面的阶段才建"在 M1a 之后不再对。任务 9 把它改成：`rurge-http` 从阶段 4 / M1a 起有（M1a 是前缀流与嗅探），`rurge-script` 到后面的阶段才建；依赖方向一条加 `rurge-engine → rurge-http`、`rurge-tun` 不依赖它。这是事实的订正，不是进度 |
| P21 | 写计划时核过的事实（对着 `3ec732e`） | **V1**：`Dialer` 的实现有引擎、`rurge-inbound` 的 `FakeDialer` 与 `UdpEcho`（`socks5.rs` 的用例里）、`rurge_tun::testing::FakeDialer`，以及 `rurge-tun` 用例里的 `Ends` 与 `Gated`；两个新方法都有缺省实现，它们不用改。**V2**：SOCKS5 的成功应答是 `reply(REP_SUCCESS)` = `[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]`；`handle` 组好 `session`（`listener`、`transport`、`local`）之后调 `dialer.dial`；`CONNECT`（`http.rs` 的 `connect`）在 `session_for` 之后 `dial`，通了才 `hyper::upgrade::on(&mut req)`，在 `ctx.tracker` 上起任务、里面再 `tokio::spawn` 一层接 panic；`ListenerOpts.drop_hold` 缺省 30 秒，引擎在 `Engine::listener_opts` 里给。**V3**：`rurge-tun` 的 `session::dial_and_relay(stream, serving, tuple, name, life)` 开头绑 `(name, _lease)`；`tcp_session` 组会话；`Serving` 有 `dialer` 与 `holds`（`Holds { drop_hold, linger }`，来自 `TunOptions`，缺省 30 秒与 60 秒）；`Life::session_stop()` 给停止令牌的孩子。**V4**：`FakeIpPool::name_of(&self, ip) -> Option<String>`（刷新、不租用），`lease(self: &Arc<Self>, ip) -> Option<(String, Lease)>`；`rurge_dns::fakeip::in_space(ip)`；引擎的 `fake_pool: ArcSwapOption<FakeIpPool>`（`load_full()`）；`turn_back` 只对 `ListenerKind::Http` 与 `Socks5`、目标是 `in_space` 的 IP 时做，查之前 `to_canonical()`。**V5**：`RuleEngine::pre_match(&self, s, layer) -> Option<PreMatch>` 命中时加 `rule.hits`，不看出站模式；引擎的 `mode()` 给 `Mode::{Direct, Proxy, Rule}`；`is_suspended()` 是 crate 内的。**V6**：`CompiledSet::compile` 把规则集文件的 `DOMAIN` / `DOMAIN-SUFFIX` 行送进域名索引，不带它们的 `extended_matching`；`eval` 的索引只看参数 `extended`，逐条的用 `extended \|\| rule.extended`；现有用例 `extended_matching_applies_to_domain_entries` 在 `set.rs` 约第 1246 行（域名集）。**V7**：`HostList::matches(&self, host: &HostName, port: u16) -> Option<bool>`：第一个命中的项定，`Some(!negate)`，没有命中是 `None`；`General.force_http_engine_hosts`（缺省端口 80）、`always_raw_tcp_hosts`、`always_raw_tcp_keywords: Vec<String>`（照写的，没有转小写）；今天运行时没有谁读这三个设置。**V9**：`EngineShared` 的结构体字面量（`tests/common/mod.rs`、`outbounds_hysteria2.rs`、`outbounds_masque.rs`、`outbounds_tuic.rs`、`process.rs`、`subscriptions.rs`、`tun.rs`、`tun_dns.rs`）都带 `..EngineShared::default()`，加字段不用改它们。**V10**：`record_of` 今天是 `sni: h.sni()`、`protocol: h.protocol().or(s.protocol)`；`relay::pump` 的首块嗅探写 `set_sni` 与 `set_protocol(Https)`。**V15**：`TlsFixture`（`rurge-proto` 的 `testing` 特性）、`FakeSocks5`、`Socks5Script`（`Default`）、`echo_server`；引擎的 dev 依赖开着 `rurge-proto/testing` 与 `rurge-tun/testing`，有 `tokio-rustls`。**V16**：`Engine::tracker()` 是 `pub`；会话根令牌 `sessions_root` 是私有字段，`session_token()`（`pub(crate)`）给它的孩子，`cancel_sessions()` 取消它 |

## 执行期修正记录

下面写"执行者的裁定"的，是 Pi 在设计与计划的原则下定的；写"协调者的裁定"的，是协调的会话代项目所有者定的。项目所有者都可以改。计划的正文不改：正文里说得不对的地方，以这张表的行为准。

| # | 任务 | 修正 | 提交 |
| - | ---- | ---- | ---- |
| 1 | 0 | 第 1 项（工作树）与第 2 项（读）合并：`git status --short` 空；分支 `phase4-m1a`，`HEAD` = `4650845`；`git log --oneline 3ec732e..HEAD` 只有两行——`60bd0f8`（M1 的设计与本计划）与 `4650845`（派工之前对任务 2 的 `NeedMore` 的订正），`3ec732e` 在历史里；本地 `main` 在 `60bd0f8`（本分支未推送）。设计全文读过（第 1 ～ 7、13、14、16、18、20 节与第 21 节「执行期的订正」），总设计第 5 节读过，本计划的「计划期决定」P1 ～ P21 读过。无出入 | 本提交 |
| 2 | 0 | 第 3 项（`rurge-inbound`）：名字都在，签名与 P21 的 V1、V2 写的相同。`session.rs`：`pub trait Dialer: Send + Sync`（`:450`）的四个方法今天的完整签名——`fn dial<'a>(&'a self, session: SessionInfo) -> BoxFuture<'a, Result<Dialed, DialError>>`（`:451`）、`fn relay<'a>(&'a self, client: BoxedStream, upstream: BoxedStream, handle: Arc<SessionHandle>) -> BoxFuture<'a, ()>`（`:452`）、`fn admit_udp(&self) -> UdpAdmission`（`:459`，缺省 `NotSupported`）与 `fn associate<'a>(&'a self, client: Arc<dyn UdpClient>, session: SessionInfo, closed: CancellationToken) -> BoxFuture<'a, ()>`（`:467`）——任务 4 加的两个带缺省实现的方法不影响它们；`Dialed`（`:388`：`stream`、`handle`、`forward: Option<Vec<(String, String)>>`）、`DialError`（`:398`：`Reject { kind, rule, handle }`、`Failed { kind, message, rule, handle }`）、`SessionHandle::token(&self) -> &CancellationToken`（`:116`）。`socks5.rs`：`async fn handle(mut stream: TcpStream, peer: SocketAddr, local: SocketAddr, dialer: Arc<dyn Dialer>, opts: Arc<ListenerOpts>) -> io::Result<()>`（`:176`）、`fn reply(code: u8) -> [u8; 10]`（`:64`）、`pub const REP_SUCCESS: u8 = 0x00`（`:26`）。`http.rs`：`async fn connect(mut req: Request<Incoming>, ctx: Arc<Ctx>) -> Result<Response<ResponseBody>, HandlerError>`（`:170`）、`fn session_for(ctx: &Ctx, host: HostName, port: u16) -> SessionInfo`（`:160`）、`struct Ctx`（`:33`，字段 `dialer: Arc<dyn Dialer>`、`opts: Arc<ListenerOpts>`、`local`、`here`、`peer`、`tracker: TaskTracker`）。`listener.rs` 的 `ListenerOpts.drop_hold: Duration`（`:47`，缺省 30 秒在 `:61`）。`testing.rs`：`pub(crate) struct FakeDialer`（`:23`）、`pub(crate) fn new(echo: SocketAddr, target: Option<SocketAddr>) -> Arc<FakeDialer>`（`:33`）、`pub(crate) fn sessions(&self) -> Vec<Arc<SessionHandle>>`（`:42`）、`pub(crate) async fn echo_server() -> SocketAddr`（`:147`）；这个模块是 `#[cfg(test)] pub(crate) mod testing;`（`lib.rs:13`）。`lib.rs` 的导出在 `:17` ～ `:24`，任务 4 加 `ServeOpts` 与 `hold_dropped` 时一起加 | 本提交 |
| 3 | 0 | 第 3 项（`rurge-tun`）：名字都在。`session.rs`：`async fn dial_and_relay(stream: TunStream, serving: &Serving, tuple: Tuple, name: Option<(String, Lease)>, life: &Life)`（`:157`，开头就把名字与租约绑住，`:164`）、`pub(crate) fn tcp_session(tuple: &Tuple, name: Option<&str>) -> SessionInfo`（`:91`）、`pub(crate) struct Serving`（`:44`，字段 `dialer: Arc<dyn Dialer>`、`responder: Arc<Responder>`、`holds: Holds`、`dns_idle: Duration`，都是模块私有——任务 5 在 `session.rs` 里改，不用动可见性）、`pub(crate) struct Holds`（`:22`，`pub(crate) drop_hold`（`:25`）与 `pub(crate) linger`（`:28`），`From<&TunOptions>` 在 `:32`）。`lib.rs`：`Life::session_stop(&self) -> CancellationToken`、`Life::stopped(&self) -> WaitForCancellationFuture<'_>`、`TunHandle::stop(&self)`、`TunHandle::join(&self) -> Ending` 都在。`src/tests/outcomes/stop.rs` 的 `Gated`（`:168`，`inner: Arc<FakeDialer>`、`open: watch::Sender<bool>`；`impl Dialer` 在 `:173`，只写了 `dial` 与 `relay`）——任务 5 的包装照它写。一处位置不同：`rurge_tun::testing::FakeDialer` 在 `src/testing/dialer.rs`（`pub struct FakeDialer` 在 `:137`、`pub fn new() -> Arc<FakeDialer>` 在 `:144`、`pub fn script(&self, dst: SocketAddr, script: Script)` 在 `:260`），`testing` 是一个目录而不是单个 `testing.rs`；模块路径不变，P15 照旧 | 本提交 |
| 4 | 0 | 第 3 项（`rurge-rules`）：名字都在，签名与 V5、V6 写的相同。`set.rs`：`pub struct CompiledSet`（`:18`，公开的只有 `name`、`kind`、`needs_dns`、`version`，其余私有——`domains: DomainIndex`、`ips: IpIndex`、`ips_resolve`、`asns: HashMap<u32, u32>`、`asns_resolve`、`linear: Vec<(u32, CompiledSubRule)>`、`entries: Vec<Box<str>>`；任务 3 的整集标志另加一个私有字段）、`pub fn compile(name: &str, kind: SetKind, parsed: &ParsedSet, sets: &dyn SetLookup, version: u64, …) -> CompiledSet`（`:63`）、`pub fn eval(&self, s: &SessionInfo, ctx: &mut EvalCtx<'_>, no_resolve: bool, extended: bool) -> SetVerdict`（`:194`；域名索引在 `:201` ～ `:213` 只按参数 `extended` 决定要不要看 `sni` 与 `http_host`，逐条的用 `extended \|\| rule.extended`（`:220`））、用例 `extended_matching_applies_to_domain_entries`（`:1246`）。`engine.rs`：`pub fn pre_match(&self, s: &SessionInfo, layer: PreMatchLayer) -> Option<PreMatch>`（`:553`，命中时 `rule.hits.fetch_add(1, …)` 在 `:563`）、`fn pre_pass(&self, s: &SessionInfo, ctx: &mut EvalCtx<'_>, notes: &mut Vec<String>, mut trace: Option<&mut Vec<TraceStep>>) -> Option<Decision>`（`:458`）、`fn pre_matches(rule: &CompiledRule, s: &SessionInfo, ctx: &mut EvalCtx<'_>) -> bool`（`:497`）、`pub struct CompiledRule`（`:86`，`hits: AtomicU64` 是私有字段（`:94`），读数走 `pub fn hits(&self) -> u64`（`:99`））、用例模块 `mod pre_match`（`:591`）。`matcher.rs`：`pub struct CompiledSubRule`（`:246`，`pub matcher`、`pub no_resolve`、`pub extended`、`pub raw`）、`pub fn compile_subrule(sub: &SubRule, sets: &dyn SetLookup) -> CompiledSubRule`（`:253`，`extended: sub.extended_matching` 在 `:257`）。`rurge-config` 的 `RuleKind` 五种域名规则是 `Domain`、`DomainSuffix`、`DomainKeyword`、`DomainWildcard`、`DomainSet`（`rule.rs:253` ～ `:257`）；`supports_pre_matching` 在 `rule.rs:337`，`RuleSet` 也在为真的那一支（`:356`）——R1 的前提 | 本提交 |
| 5 | 0 | 第 3 项（`rurge-engine`）：名字都在，签名与 V4、V5、V9、V10、V16 写的相同。`engine.rs`：`Engine` 的字段 `sessions_root: CancellationToken`（`:178`）、`tracker: TaskTracker`（`:183`）、`pub(crate) fake_pool: ArcSwapOption<FakeIpPool>`（`:195`）、`shared: EngineShared`（`:202`）；`impl Dialer for Engine`（`:1662`）的 `fn dial<'a>(&'a self, mut session: SessionInfo) -> BoxFuture<'a, Result<Dialed, DialError>>`（`:1663`）与 `fn relay<'a>(&'a self, client: BoxedStream, upstream: BoxedStream, handle: Arc<SessionHandle>) -> BoxFuture<'a, ()>`（`:1693`）——两个都没有同名的固有方法，所以 `serve.rs` 里要 `use rurge_inbound::Dialer`；`pub(crate) fn is_suspended(&self) -> bool`（`:702`）、`pub fn suspend(&self, why: Option<String>)`（`:692`）、`pub fn mode(&self) -> Mode`（`:664`）、`pub fn global_policy(&self) -> Option<String>`（`:678`）、`pub fn registry(&self) -> Arc<PolicyRegistry>`（`:306`）、`pub fn runtime(&self) -> Arc<Runtime>`（`:300`）、`pub fn tracker(&self) -> &TaskTracker`（`:1351`）、`pub fn cancel_sessions(&self)`（`:1348`）、`pub(crate) fn session_token(&self) -> CancellationToken`（`:455`）；`policy_known` 是自由函数 `pub(crate) fn policy_known(registry: &PolicyRegistry, name: &str) -> bool`（`:897`），不在 `impl Engine` 里。`fake_address.rs` 的 `pub(crate) fn turn_back(&self, session: &mut SessionInfo) -> Turned`（`:33`）。`shared.rs` 的 `pub struct EngineShared`（`:171`）、`pub fn new(initial: GroupSelections) -> EngineShared`（`:208`）、`impl Default for EngineShared`（`:224`，转调 `EngineShared::new(GroupSelections::new())`）——P16 的 `sniff_wait` 加在这里，V9 列的八个带 `..EngineShared::default()` 的字面量都不用改。`observe.rs` 的 `fn record_of(h: &SessionHandle, outcome: Option<&SessionOutcome>) -> RequestRecord`（`:91`，`sni: h.sni()`（`:121`）、`protocol: h.protocol().or(s.protocol)`（`:122`），与 V10 相同）。`lib.rs` 的导出在 `:30` ～ `:46`。用例夹具：`tests/common/mod.rs` 的 `Harness`（`:44`）、`Profile`（`:187`）、`harness_with`（`:231`）、`connect_via_http(proxy: SocketAddr, authority: &str) -> TcpStream`（`:339`）、`connect_via_socks5(proxy: SocketAddr, host: &str, port: u16) -> TcpStream`（`:365`）、`the_records(h: &Harness, n: usize) -> Vec<RequestRecord>`（`:418`）、`wait_until(what: &str, check: impl FnMut() -> bool)`（`:400`）；`tests/common/tun.rs` 的 `WITHIN`（`:18`，5 秒）、`guarded(rules: &str) -> String`（`:31`）、`profile(rules: &str) -> Profile<'_>`（`:35`）、`Rig`（`:44`）的 `new`（`:52`）、`on`（`:57`）与 `fakes(&self, name: &str) -> (IpAddr, IpAddr)`（`:75`）、`tun_records(h: &Harness, n: usize) -> Vec<RequestRecord>`（`:142`） | 本提交 |
| 6 | 0 | 计划说「改出站模式的公开方法（照 `tests/pipeline.rs` 的用法找）」，代码是 `pub async fn set_mode(&self, mode: Mode)`（`engine.rs:668`，异步：先 `self.mode.store(Arc::new(mode))`，再往 `state` 上 `update(…)`），按它做；任务 6、7 的用例照今天的用法 `.set_mode(…).await` 写。配套的读法是同文件的 `pub fn mode(&self) -> Mode`（`:664`） | 本提交 |
| 7 | 0 | 任务 7 第 9 条要读出来的事实（引擎暂停时，可嗅探的会话今天拿到什么）：`connect_session` 对暂停的会话 `fail(handle, FailKind::Other, REFUSED_STRICT)`（`engine.rs:1491`，`REFUSED_STRICT` 的定义在 `:65`）；SOCKS5 把 `FailKind::Other` 跟 `Connect`、`Timeout` 一起映成 `REP_CONNECTION_REFUSED`，即 **`0x05`**（`socks5.rs:241`）；`CONNECT` 走 `failure_response`（`http.rs:335`）——`show-error-page` 为真（缺省真）时答 **502** 错误页（`StatusCode::BAD_GATEWAY`，标题 `Connection failed`，正文按 `FailKind` 的三句之一），关着时不答就关（`Err(HandlerError::Close)`）。任务 6 之后暂停时不嗅探，所以这两种应答照旧在答复之前到 | 本提交 |
| 8 | 0 | 第 3 项（`rurge-dns` 与 `rurge-config`）：名字都在。`rurge-dns` 的 `fakeip/mod.rs`：`pub fn in_space(ip: IpAddr) -> bool`（`:98`）、`pub fn name_of(&self, ip: IpAddr) -> Option<String>`（`:441`，刷新、不租用）、`pub fn lease(self: &Arc<Self>, ip: IpAddr) -> Option<(String, Lease)>`（`:452`），`pub fn leased(&self) -> usize` 在 `:481`。`rurge-config`：`General` 的 `pub force_http_engine_hosts: HostList`（`general.rs:202`）、`pub always_raw_tcp_hosts: HostList`（`:203`）、`pub always_raw_tcp_keywords: Vec<String>`（`:204`）——与 V7 相同，今天没有谁在运行时读它们；`HostList::matches(&self, host: &HostName, port: u16) -> Option<bool>`（`hostlist.rs:70`）；`SessionInfo`（`session.rs:54`）、`ListenerKind`（`:10`：`Http` / `Socks5` / `Tun` / `Forward` / `Internal`）、`Transport`（`:20`：`Tcp` / `Udp`）、`ProtocolKind`（`rule.rs:208`，`Http`（`:209`）与 `Https`（`:210`）是前两个） | 本提交 |
| 9 | 0 | 第 3 项（`rurge-proto`、`rurge-net` 与 `rurge-api`）：名字都在。`rurge-proto` 的 `testing/socks5.rs`：`pub struct Socks5Script`（`:14`，`Default`；`reply: u8`（`:19`）与 `connect_to: Option<SocketAddr>`（`:24`）都在）、`pub struct FakeSocks5`（`:52`）、`pub fn requests(&self) -> Vec<RecordedSocks5>`（`:314`），元素 `pub struct RecordedSocks5`（`:41`）的字段是 `methods: Vec<u8>`、`credentials: Option<(String, String)>`、`command: u8`（1 = CONNECT、3 = UDP ASSOCIATE）、`atyp: u8`、`host: String`、`port: u16`——P14 要的 `command`、`host`、`port` 都在；`testing/tls.rs` 的 `pub struct TlsFixture`（`:48`）与 `pub fn new(names: &[&str]) -> Arc<TlsFixture>`（`:59`）；`testing/mod.rs` 的 `pub async fn echo_server() -> SocketAddr`（`:64`）。`rurge-net` 的 `pub fn tls_client_config(skip_verify: bool) -> Result<Arc<rustls::ClientConfig>, HttpError>`（`http.rs:281`）、`pub struct TestServer`（`testing.rs:64`）与 `pub async fn spawn() -> TestServer`（`:72`，另有 `spawn_tls`（`:76`）与 `spawn_tls_with`（`:82`））。`rurge-api` 的 `pub(crate) fn router(key: String, ctx: ApiContext) -> Router`（`lib.rs:57`，crate 内可见，任务 8 在同一 crate 里加路由）、`src/routes/mod.rs` 现在有十二个 `pub mod`（`dns`、`features`、`geoip`、`log`、`misc`、`outbound`、`policies`、`policy_groups`、`profiles`、`requests`、`resources`、`traffic`；任务 8 加 `rurge_http`）、`tests/api.rs` 的 `async fn api() -> Api`（`:181`）与 `async fn get(api: &Api, path: &str) -> (u16, Value)`（`:368`） | 本提交 |
| 10 | 0 | R1（协调者的裁定，接任务 6，补 P6）：探询（`pre_refuses`）的时候手里没有会话的进程——求规则时的 `pre_pass` 有（`choose_policy_with` 经进程槽把它传进去）。能带 `pre-matching` 的规则自己都不是 `PROCESS-NAME`（`rule.rs:337` 的为真一支里没有它），但带这个标志的 `RULE-SET` 引用的集合里可以有（`RuleSet` 在 `:356` 为真）：这样的会话探询时不拒、照可嗅探的路先答「成功」，求规则时才被拒，客户端看到的是一个随即关闭的隧道（设计第 13 节第 1 条）。为了让探询等进程而把 `sniffable` 变成异步的、让每个代理端口上的会话都多等一次进程查找，不值得。所以不改，照设计做；任务 9 登记差异时把这个差别并进设计第 13 节第 1 条那一句 | 本提交 |
| 11 | 0 | 第 4 项（数）：`Cargo.lock` 里 `[[package]]` 519 个；`crates/rurge-platform/tests/unsafe_functions.rs` 的 `LISTED` 是 `const LISTED: [(&str, &str); 23]`（`:18` ～ `:42`），数下来也是 23 项。都与计划写的一样 | 本提交 |
| 12 | 0 | 第 5 项（P9 里「预计要改」的六条，逐条找到读过；任务 6 照这一行改，改的地方各加一行英文注释）：`tests/outbounds.rs` 的 `a_refusing_upstream_is_a_502_that_quotes_the_proxy`（`:162`）断言 `CONNECT target.test:443` 拿到的应答以 `HTTP/1.1 502` 开头、记录里的错误是 `http proxy answered 407 Proxy Authentication Required`——`443` 改 `8443`（`Host` 头一起改），断言不改；`a_broken_hop_is_named_in_the_error`（`:429`）同样断言 `HTTP/1.1 502` 与错误以 `via Entry: ` 开头——`443` 改 `8443`，断言不改；`tests/pipeline.rs` 的 `reject_rules_close_serve_gifs_or_render_pages`（`:414`）有两处：`CONNECT ads.test:443 HTTP/1.1` 断言应答**空**（被 REJECT 时今天不答就关，`Err(HandlerError::Close)`，与 `show-error-page-for-reject` 无关——任务 7 第 8 条照的就是这一条），SOCKS5 连 `ads.test:443` 断言应答码 `0x02`（`not allowed by ruleset`）——两处 `443` 都改 `8443`，断言不改；`dns_failure_and_ip_rules`（`:1068`）的 `CONNECT nx.test:443` 断言 `HTTP/1.1 502` 且正文含 `DNS lookup failed`——`443` 改 `8443`，断言不改；`tests/process.rs` 的 `a_record_shows_a_process_found_after_the_session_ended`（`:593`）的 `CONNECT ads.test:443` 要在 5 秒内读到连接结束、记录是 `Rejected`——`443` 改 `8443`（在 443 上嗅探会先答 200 再等满 15 秒，5 秒的界先到）；`tests/tun.rs` 的 `a_rejected_tcp_connection_is_reset`（`:150`）断言应用的第一次读拿到 `ErrorKind::ConnectionReset`（虚拟网卡，应用只读不写）——`fake:80` 改 `fake:8080`，断言不改 | 本提交 |
| 13 | 0 | 第 5 项（P9 里「预计照过」的十二条，逐条找到读过；理由都在客户端先发了字节、或根本不经过嗅探）：`tests/network.rs` 的 `visit`（`:80`）`connect_via_http` 之后立刻 `get`，客户端先发 `GET /hello HTTP/1.1`；`tests/outbounds.rs` 的 `a_local_host_item_reaches_the_proxy_as_an_ip`（`:261`）与 `a_local_host_item_with_several_addresses_gives_the_proxy_one_at_random`（`:296`）同样先 `get`（两条用例里只 `connect_via_http` 就放掉的会话，放掉发的 FIN 让嗅探读到 EOF 就往下走）；`a_pinned_host_turns_forwarding_into_a_tunnel`（`:343`）是绝对形式请求，走监听器自己的 `forward()`，不经 `CONNECT`；`a_selection_applies_to_the_next_connection_and_survives_a_restart`（`:518`）与 `a_subnet_group_follows_the_network_the_engine_is_on`（`:575`）都是 `connect_via_http` 之后立刻 `get`；`tests/outbounds_ssh.rs` 的 `a_keystore_key_logs_in_to_a_pinned_server`（`:55`）与 `an_ssh_session_runs_inside_shadow_tls`（`:89`）同样先 `get`；`tests/smart.rs` 的 `the_first_byte_back_is_reported`（`:315`）先 `get`，`three_seconds_without_an_answer_count_against_the_member`（`:380`）`connect_via_http` 之后自己 `write_all` 一个 `GET / HTTP/1.1`；`tests/tun.rs` 的 `tcp_to_a_documentation_address_follows_the_rules`（`:110`）应用 `echoes` 先写；`tests/tun_dns.rs` 的 `pre_matching_at_the_syn_layer_refuses_an_address_too`（`:566`）在 SYN 就被协议栈拒，到不了 `dial_and_relay`，`a_socks5_session_to_an_unknown_fake_address_is_refused`（`:670`）是 P5 的那一条——fake 地址背后没有名字，不嗅探，照今天的路拿到 `0x05`。与 P9 的预计不同的：无 | 本提交 |
| 14 | 0 | 第 6 项（门禁，一台机器上的一次运行，不是 CI）：`cargo fmt --all --check` 退出码 0；`cargo clippy --all-targets -- -D warnings` 0 个警告（退出码 0）；`cargo test -j 8 --workspace --no-fail-fast` 86 个结果行、4725 通过、0 失败、13 忽略——与基线（86 / 4725 / 0 / 13）逐项相同；没有二进制以 `STATUS_HEAP_CORRUPTION` 或 `STATUS_ACCESS_VIOLATION` 退出，因此没有重跑。日志 `gate-task-0.log` 在工作区（git 忽略）。需要参照服务端的互操作用例在本机立刻返回，算在通过的数里 | 本提交 |
| 15 | 1 | 第 1 ～ 4 项与计划、派工相同：根 `Cargo.toml` 在 `rurge-inbound` 那一行之后加 `rurge-http = { path = "crates/rurge-http" }`（`members` 不动，`crates/*` 自动纳入）；`crates/rurge-http/Cargo.toml` 的 `[package]` 五个 `*.workspace = true` 照 `rurge-inbound`，`[dependencies]` 只有 `tokio`、`[dev-dependencies]` 只有 `rustls`、`[lints] workspace = true`；`src/lib.rs` 只有 crate 文档（英文，说 M1a 有前缀流与嗅探、不认识引擎与监听器与会话）与 `pub mod prefix;` 加 `pub use prefix::Prefixed;`；`src/prefix.rs` 照派工给的写法——`inner` / `buf` / `pos` 三个字段，`buffered()` 是 `&buf[pos..]`，`fill` 先 `reserve(FILL)` 再 `(&mut self.inner).take(FILL as u64).read_buf(&mut self.buf)`（不在 async 函数里放 16 KiB 的栈数组），`poll_read` 先给缓冲、给完 `Vec::new()` 放掉内存、缓冲空了才转给 `inner`，写的五个方法经 `get_mut()` 直通，没有 pin 投影宏。与计划的出入：无 | 本提交 |
| 16 | 1 | 执行者的裁定：用例里用一个 `fill_until(prefix, n)` 循环 `fill` 到至少 `n` 个字节——一次读可能短于对端写下的量（`tokio::io::duplex` 与真的流都如此），按单次 `fill` 写会让用例偶发变红。另：判据里「`Box::new(Prefixed<BoxedStream>)` 能当 `BoxedStream` 用（`AsyncRead + AsyncWrite + Unpin + Send`）」不在四条用例里，用一条临时用例验证过（本地写 `trait Both: AsyncRead + AsyncWrite + Unpin + Send` 与 `Box<dyn Both>`，正是 `rurge-net` 的 `AsyncStream` 的界，`Box::new(Prefixed::new(duplex 的一端))` 能交给它），验完删掉，让这个 crate 的用例恰好是派工的四条；真正的用法在任务 6，那时由引擎的代码钉住。`rurge-http` 在 M1a 不依赖 `rurge-net`（P2），所以这条性质没有留在用例里 | 本提交 |
| 17 | 1 | 判据（一台机器上的一次运行，不是 CI）：`cargo fmt --all --check` 退出码 0；`cargo clippy --all-targets -- -D warnings` 0 个警告（退出码 0）；`cargo test -j 8 --workspace --no-fail-fast` 88 个结果行、4729 通过、0 失败、13 忽略——比任务 0 的基线（86 / 4725 / 0 / 13）多 2 个结果行（`rurge-http` 的单元测试与文档测试各一行）、多 4 个通过（本任务的四条用例）；没有二进制以 `STATUS_HEAP_CORRUPTION` 或 `STATUS_ACCESS_VIOLATION` 退出，因此没有重跑。`Cargo.lock` 的 `[[package]]` 从 519 变成 520（多的是 `rurge-http`），提交里带着它；`crates/rurge-platform/tests/unsafe_functions.rs` 的 `LISTED` 仍是 23 项（没有新的放开）。日志 `gate-task-1.log` 在工作区（git 忽略）。需要参照服务端的互操作用例在本机立刻返回，算在通过的数里 | 本提交 |
| 18 | 2 | 执行者的裁定（模块的拆法，计划「文件结构」允许调整并记下）：记录的重组是 `tls::reassemble(prefix, limit) -> Reassembled`，`Reassembled` 是 `tls` 里的 `pub(crate)` 枚举（`Hello` / `NeedMore(usize)` / `Other`），`sniff::judge` 把它映成 `Judged`。这样 `tls` 不需要认识 `sniff::Judged`，也不需要反过来依赖 `sniff` 的 `MAX_PREFIX`：`judge` 把 `MAX_PREFIX` 当 `limit` 参数传进去（`need` 与握手的总长都用它比）。`parse_client_hello` 仍是任务要求的样子：只吃一整条握手消息，`tls` 之外的代码经 `rurge_http::tls::parse_client_hello` 用它（M5）。用例按主题分：与 ClientHello、记录、长度有关的十条在 `tls.rs`（`tls::tests` 里另有产生真 ClientHello、切记录、造手工 ClientHello 的辅助函数，`pub(crate)`，`sniff.rs` 的 `a_slow_drip…` 与 `another_record_type…` 用它们），HTTP 开头与 `sniff` 的十三条在 `sniff.rs` | 本提交 |
| 19 | 2 | 判据（一台机器上的一次运行，不是 CI）：`cargo fmt --all --check` 退出码 0；`cargo clippy --all-targets -- -D warnings` 0 个警告（退出码 0）；`cargo test -j 8 --workspace --no-fail-fast` 88 个结果行、4752 通过、0 失败、13 忽略——比任务 1 的基线（88 / 4729 / 0 / 13）多 23 个通过、结果行数不变（本任务新写的二十三条用例都在 `rurge-http` 已有的单元测试那一行里）；没有二进制以 `STATUS_HEAP_CORRUPTION` 或 `STATUS_ACCESS_VIOLATION` 退出，因此没有重跑。`Cargo.lock` 的 `[[package]]` 仍是 520（没有新的包），`LISTED` 仍是 23 项。日志 `gate-task-2.log` 与 `clippy-task-2.log` 在工作区（git 忽略）。带等待的十三条 `sniff::tests` 提交前另单独循环跑了二十次，全过（`loop-task-2.log`）。`no_damaged_hello_panics` 的输入是 253 个字节的三种替换（0x00、0xff、异或 0x80），共 759 种，单个用例一次跑 0.01 秒。需要参照服务端的互操作用例在本机立刻返回，算在通过的数里 | 本提交 |
| 20 | 2 | 协调者的裁定（修正轮 1，改计划任务 2 的规则 1 与规则 6，以这一行为准）：`NeedMore(n)` 的 `n` 从「当前记录的末尾与消息末尾中小的那个」改成「握手消息最早能齐的位置」——还不知道长度时是握手的头（4 个字节），知道之后是整条消息（`4 + 24 位长度`）；记录的头没到齐时是 `at + 5 + (wanted - got)`（第一个记录因此是 9，用例 `a_slow_drip_is_not_judged_again_until_it_can_be` 的第一句从 `NeedMore(5)` 改成 `NeedMore(9)`），当前记录没到齐时是 `at + 5 + (wanted - before)`，这个位置超过当前记录的末尾就再加 5（后面至少还有一个记录头）；`wanted` 是一个小函数（`handshake.get(..4)`，取到了算 `4 + 长度`，取不到就是 4）。P3（`NeedMore` 带一个数）的用意不变，只是数取这个下界。旧规则在一串只带很小负载的记录上，每到一个记录边界就把前缀从头接一遍，判的次数随记录数线性增长、总量是记录数的平方：一台机器上的一次运行（release 编译、只依赖 `rurge-http` 的小程序，不是 CI）量到——负载 1 字节 / 消息 10918 字节：21846 次、776 毫秒（新规则 49 次、2.2 毫秒）；负载 2 / 18720：18726 次、586 毫秒（28 次、1.1 毫秒）；负载 8 / 40324：10085 次、172 毫秒（12 次、0.24 毫秒）；负载 64 / 60732：1901 次、9.8 毫秒（7 次、0.03 毫秒）。代价是后面某个记录的头不合格时要到那个位置才判 `Other`（结论不变，照旧原样转发；真实的客户端不会先发半个 ClientHello 再发别的记录然后等着），写在 `reassemble` 与 `Judged::NeedMore` 的文档注释里。新增用例 `a_drip_of_tiny_records_is_judged_a_few_times`（两种字节各判 29 次与 23 次，断言不超过 40；旧规则下前者 480 次、后者约 13000 次）。订正 `d0f7914` 的提交说明里「总量最坏与读到的字节数成正比」一句：那句不对，旧规则下是平方，新规则下判的次数随消息长度成对数增长 | 本提交 |
| 21 | 2 | 判据（一台机器上的一次运行，不是 CI）：`cargo fmt --all --check` 退出码 0；`cargo clippy --all-targets -- -D warnings` 0 个警告（退出码 0）；`cargo test -j 8 --workspace --no-fail-fast` 88 个结果行、4753 通过、0 失败、13 忽略——比修正轮之前的 88 / 4752 / 0 / 13 多 1 个通过（新的判次数用例），结果行数不变；没有二进制以 `STATUS_HEAP_CORRUPTION` 或 `STATUS_ACCESS_VIOLATION` 退出，因此没有重跑。带等待的 `sniff::tests`（14 条）提交前单独循环跑了二十次，全过。日志 `gate-task-2-fix-1.log`、`loop-task-2-fix-1.log` 在工作区（git 忽略） | 本提交 |
| 22 | 3 | 第 1、2 项做了；与计划的出入：无。`set.rs`：`CompiledSet` 加私有字段 `extended`（`:29`，文档注释引手册 `rules/ruleset.html` 那句，并说明域名集文件的行 `SetLine::Domain` 没有参数、只看引用它的那一行），`empty()`（`:74`）与 `compile` 末尾（`:174`）两处字面量给值；私有 `fn is_domain_rule(kind: &RuleKind) -> bool`（`:56`）认五种（`Domain`、`DomainSuffix`、`DomainKeyword`、`DomainWildcard`、`DomainSet`，P7）；`compile` 在循环之前扫一遍 `parsed.lines`（`:102` ～ `:104`）；`other` 支里先 `compile_subrule`、再在 `extended && is_domain_rule` 时把 `rule.extended` 置真（`:159` ～ `:162`），`compile_subrule` 不改；`eval` 的域名索引改成 `if extended \|\| self.extended`（`:236`），逐条那段照旧。`engine.rs`：私有 `fn first_pre_hit(&self, s, layer) -> Option<(&CompiledRule, Option<SubRuleHit>)>`（`:575`）是两者共用的判断（建 `EvalCtx::with_network`、设 `before_connection`、每条先清 `ctx.sub_hit`、判完清 `ctx.notes`），`pre_match`（`:553`）在它上面加 `rule.hits`、返回 `PreMatch`，`pre_refuses`（`:568`）只问 `is_some()`；`pre_pass` 不动（协调者裁定 R1，修正记录第 10 行）。用例五条：`set.rs` 的 `a_flag_on_one_domain_line_applies_to_every_domain_line`（`:1307`）、`without_a_flag_a_rule_set_ignores_the_sni`（`:1335`）、`a_flag_on_a_rule_that_is_not_a_domain_rule_does_not_spread`（`:1359`）、`a_domain_set_line_takes_and_gives_the_flag`（`:1375`，协调者在派工时加的：P7 把 `DOMAIN-SET` 算进五种，计划的三条没有一条覆盖它），`engine/tests/pre_match.rs` 的 `pre_refuses_answers_as_pre_match_and_counts_nothing`（`:449`）。执行者的裁定：`URL-REGEX` 那一行写成转义过的普通字符串（`"URL-REGEX,^http://x\\.com/,extended-matching"`），与文件里别的用例的写法一致，不用原始字符串 | 本提交 |
| 23 | 3 | 判据（一台机器上的一次运行，不是 CI）：`cargo fmt --all --check` 退出码 0；`cargo clippy --all-targets -- -D warnings` 0 个警告（退出码 0）；`cargo test -j 8 --workspace --no-fail-fast` 88 个结果行、4758 通过、0 失败、13 忽略——比任务 2 修正轮的基线（88 / 4753 / 0 / 13）多 5 个通过（本任务的五条用例），结果行数不变。前三次全量各有一个与本任务无关的偶发，每个都单独重跑通过：第一次 `cli` 的 `run::enhanced_mode::an_unknown_enhanced_backend_ends_the_start_with_2` 20 秒内没退出（它的 stderr 已是期望的那句；单独重跑通过，第二 ～ 四次全量也通过）；第二次 `rurge-engine` 的 `outbounds_masque` 以 `STATUS_HEAP_CORRUPTION` 退出（单独重跑 11 passed，第一次全量里它是过的）；第三次 `rurge-api` 的单元测试二进制以 `STATUS_HEAP_CORRUPTION` 退出（`--lib` 重跑全过，第一、二次全量里它是过的）。`Cargo.lock` 的 `[[package]]` 仍是 520，`LISTED` 仍是 23 项（没有新的放开）。没有新增计时用例，因此没有循环跑二十次。日志 `gate-task-3.log`、`gate-task-3-rerun.log`、`gate-task-3-green.log`、`gate-task-3-green2.log` 与 `clippy-task-3.log` 在工作区（git 忽略）。需要参照服务端的互操作用例在本机立刻返回，算在通过的数里 | 本提交 |
| 24 | 4 | 第 1 ～ 6 项都做了；与计划的出入：无。`session.rs`：`ServeOpts`（`:451`，三个字段与计划写的相同）、`Dialer::sniffable`（`:477`，缺省 false）、`Dialer::serve`（`:486`，`Box::pin` 里照 `rurge-tun` 的 `dial_and_relay` 写：`dial` 不与停止竞速，`Ok` 时转发与 `opts.stop` 竞速、停止先到则取消 `handle.token()` 再 `relay.await`，`Reject { kind: Drop }` 交 `hold_dropped`，别的错误放掉流）。`hold.rs`（新）：`hold_dropped`（`:15`）三段——读写丢弃到对端关它那一侧、读出错、停止或 `drop_hold` 到（一个 `sleep` pin 住、不在读的循环里重新计时；停止立刻返回）；`shutdown` 与停止竞速、出错忽略；`linger` 为 0 时第三段不读，否则读了就扔至多 `linger` 直到对端关它那一侧、读出错或停止。停止写成一个 pin 住的 future（`opts.stop` 为 `None` 时是 `pending`），各段都 `biased;` 先看它；读的缓冲 `vec![0u8; 4096]` 在堆上、三段共用。`lib.rs`：`mod hold;`（`:6`）、`pub use hold::hold_dropped;`（`:18`）、`ServeOpts` 进 `pub use session::{…}`。`socks5.rs` 的 `handle`（`:212`）：`session.transport = Transport::Tcp;` 之后、`dial(session)` 之前，`sniffable` 为真时先写 `reply(REP_SUCCESS)`（写失败照旧 `?` 返回给监听器的 DEBUG 行，P8）、再 `serve(session, Box::new(stream), ServeOpts { stop: None, drop_hold: opts.drop_hold, linger: ZERO }).await`、返回。`http.rs` 的 `connect`（`:184`）：`session_for` 之后、`dial` 之前，`sniffable` 为真时 `hyper::upgrade::on(&mut req)`、在 `ctx.tracker` 上起任务、里面再 `tokio::spawn` 一层接 panic（同下面今天那一支）；升级成功 `serve(session, Box::new(TokioIo::new(upgraded)), ServeOpts { stop: None, drop_hold, linger: ZERO })`，失败一行 DEBUG `CONNECT upgrade failed before the session began`（不带地址），随即答 `connect_established()`。端口与鉴权的两处检查都在这一支之前，不动。`testing.rs`：`FakeDialer` 加 `pub sniff: AtomicBool`（`:24`，`new` 给假），`sniffable` 读它，不写 `serve`（P15）。执行者的裁定：SOCKS5 里新的局部变量叫 `serve_opts`（派工说两个名字都行）；缺省的 `serve` 调 `crate::hold::hold_dropped`，`hold` 模块对 crate 外只经 `pub use`。用例里两条（`a_sniffable_session_is_relayed_after_the_answer`、`a_sniffable_connect_is_relayed_after_the_answer`）在改动之前就是绿的：先答后交与先拨号后答在通了的路由上看不出差别，它们钉住的是「先答之后转发照旧」，分辨新旧两条路的是另外三条 | 本提交 |
| 25 | 4 | 判据（一台机器上的一次运行，不是 CI）：`cargo fmt --all --check` 退出码 0；`cargo clippy --all-targets -- -D warnings` 0 个警告（退出码 0）；`cargo test -j 8 --workspace --no-fail-fast` 88 个结果行、4768 通过、0 失败、13 忽略——比任务 3 的基线（88 / 4758 / 0 / 13）多 10 个通过（本任务的十条用例，都在 `rurge-inbound` 已有的单元测试那一行里）、结果行数不变。没有二进制以 `STATUS_HEAP_CORRUPTION` 或 `STATUS_ACCESS_VIOLATION` 退出，因此没有重跑。`Cargo.lock` 的 `[[package]]` 仍是 520（没有新的包），`LISTED` 仍是 23 项（没有新的放开）。带等待的五条（`hold::tests` 四条与 SOCKS5 的 `a_dropped_sniffable_session_is_held_after_the_answer`）提交前各单独循环跑了二十次，全过；`the_default_serve_cancels_the_session_at_the_stop` 也循环跑了二十次，全过。日志 `gate-task-4.log`、`loop-task-4-hold.log`、`loop-task-4-socks5-drop.log`、`loop-task-4-serve.log` 在工作区（git 忽略）。需要参照服务端的互操作用例在本机立刻返回，算在通过的数里 | 本提交 |
| 26 | 5 | 第 1、2 项做了；与计划的出入：无。`crates/rurge-tun/src/session.rs`：`use rurge_inbound::{DialError, Dialer, ServeOpts};`（`:18`）；`dial_and_relay` 在 `let session = tcp_session(…)` 与 `let dialer = &serving.dialer;` 之后、`match dialer.dial(session).await` 之前加一支（`:173` ～ `:186`）：`dialer.sniffable(&session)` 为真时照 P12 组 `ServeOpts { stop: Some(life.session_stop()), drop_hold: serving.holds.drop_hold, linger: serving.holds.linger }`，`dialer.serve(session, Box::new(stream), opts).await` 之后 `return`；这一支不在这里与停止竞速（`serve` 自己在里面与 `opts.stop` 竞速，缺省实现与引擎都是），也不把 `serve` 的 future 包进 `select!` 丢掉（丢掉会让句柄不结束）；`_lease` 仍是函数开头绑的那一个，握到 `serve` 返回（函数结束）；文件头 `//!` 补一句可嗅探的会话整个交给 `serve`。`crates/rurge-tun/src/tests/outcomes/mod.rs`：`mod serve;`（`:14`，按字母次序放在 `rejects` 与 `shard_task` 之间），头上一段 `//!` 补一句。`crates/rurge-tun/src/tests/outcomes/serve.rs`（新，三条用例）：包装的拨号器 `Sniffing`（`:39`，照 `stop.rs` 的 `Gated`）——`dial` 与 `relay` 转给里面的 `FakeDialer`，`sniffable` 恒真（`:87`），`serve`（`:91`）先把 `opts.clone()` 记进 `Mutex<Vec<ServeOpts>>`，再照 `Then`（`:25`）做：`Serve` 调 `self.inner.serve(…)`（`rurge-tun` 的 `FakeDialer` 不覆写 `serve`，就是缺省实现）、`UntilStop` 等 `opts.stop` 被取消、`UntilOpen` 等一个 `watch` 开门；`taken()`（`:55`）读记下的选项，`open()`（`:63`）开 `UntilOpen` 的门。三条用例（`:123`、`:162`、`:193`）的等待都用 `support.rs` 的 `within` / `eventually` / `in_five_seconds` / `WITHIN`，运行时都是 `#[tokio::test(flavor = "multi_thread", worker_threads = 4)]`。执行者的裁定：`UntilOpen` 的门由 `Sniffing::open` 开（派工说写法自定）；第一条里多看一句 `fake.dialed().len() == 1`，证明缺省 `serve` 真的拨了号 | 本提交 |
| 27 | 5 | 判据（一台机器上的一次运行，不是 CI）：`cargo fmt --all --check` 退出码 0；`cargo clippy --all-targets -- -D warnings` 0 个警告（退出码 0）；`cargo test -j 8 --workspace --no-fail-fast` 88 个结果行、4771 通过、0 失败、13 忽略——比任务 4 的基线（88 / 4768 / 0 / 13）多 3 个通过（本任务的三条用例，在 `rurge-tun` 已有的单元测试那一行里），结果行数不变。这一遍全量里有两个与本任务无关的偶发，都是 `STATUS_HEAP_CORRUPTION`（本机管控软件注入的钩子 DLL），各自单独重跑通过：`outbounds_masque`（单独重跑 11 passed）、`udp`（单独重跑 38 passed），两次重跑的数（11 + 38 = 49 个通过、2 个结果行）与第一次的数合起来是上面的 88 / 4771。先红：不加 `dial_and_relay` 的那一支时三条用例都红（`serve` 没被调，`a_sniffable_connection_is_served_with_the_tuns_holds` 的 `taken` 是 `[]`，另两条等不到「the connection to be served」）。带等待的三条提交前单独循环跑了二十次（`cargo test -j 8 --workspace --lib -- outcomes::serve`），全过；`rurge-tun` 现有的用例照过（两边的 `FakeDialer` 都不嗅探）。`Cargo.lock` 的 `[[package]]` 仍是 520（没有新的包），`LISTED` 仍是 23 项（没有新的放开）。日志 `task-5-gate-test.log`、`task-5-rerun.log`、`task-5-loop.log` 在工作区（git 忽略）。需要参照服务端的互操作用例在本机立刻返回，算在通过的数里 | 本提交 |
| 28 | 5 | 修正第 1 轮（评审）：`dial_and_relay` 那一支的注释原来说 `serve` 让拨号与转发都与停止竞速，改成只有转发（引擎另有嗅探）竞速、拨号不竞速，同下面那段注释与 `CLAUDE.md`「`rurge-tun`」一条；只改注释，门禁跑了 fmt、clippy 与 `outcomes::serve`（3 通过），没有跑全量（协调者的决定） | 本提交 |
| 29 | 6 | 第 1 ～ 7 项都做了；与计划的出入：无（写法自定的三处见下）。`crates/rurge-engine/Cargo.toml`：`rurge-http.workspace = true` 放在 `rurge-inbound` 之后（`:21`）；`Cargo.lock` 只多 `rurge-engine` 依赖 `rurge-http` 的一条边。`src/shared.rs`：`use std::time::Duration;`；`EngineShared.sniff_wait: Duration` 在 `owners` 之后（`:206` ～ `:208`），文档注释照计划，`EngineShared::new` 给 `rurge_http::sniff::SNIFF_WAIT`（`:224`）；别的九处 `..EngineShared::default()` 的字面量照旧。`src/serve.rs`（新）：`SniffCounts`（`:25`，五个 `AtomicU64`，`count`（`:37`）用 `Ordering::Relaxed`，`Closed` 与 `Failed` 都加在 `closed` 上，`snapshot`（`:48`））；`SniffedCounts`（`:62`）与 `HttpCounts`（`:74`）是 `pub`、`derive(Clone, Copy, Debug, Default, PartialEq, Eq)`，文档注释说它们是 `GET /v1/rurge/http` 读的（P16）；`sniffable_session`（`:86`）是一串提前返回假的判断，每条前面一行英文注释指向设计第 4.1 ～ 4.3 节：第 1、2 条（传输与入口、暂停），探询用的会话写成 `Cow<'_, SessionInfo>`（P5：经 HTTP 与 SOCKS5 来、目标是 `in_space` 的 IP 时先看有没有池，没有池就用它自己，有池而 `name_of` 没有名字就返回假，有名字就换成 `HostName::Domain`），第 4 条 `always_raw_tcp_hosts.matches(主机, 端口) == Some(true)`，第 5 条名字里含一个非空关键字（关键字转小写，名字本来就是小写的；IP 不比），第 6 条端口不是 80、443 而 `force_http_engine_hosts.matches` 不是 `Some(true)`，第 7 条只在 `at_a_proxy_port && self.reaches_the_rules()` 时问 `runtime.rules.pre_refuses(&probed)`（P6），最后真；设置读 `self.runtime()` 的 `config.general`。`serve_sniffed`（`:168`）照计划的代码块，三处照派工换：先 `let root = self.session_token();`（`:175`）再在 `select!` 里 `root.cancelled()`（`:178`）、`self.sniff_wait()`（`:180`）、`hold_dropped` 那一支同样与 `root.cancelled()` 竞速（`:222` ～ `:223`）；嗅探被停止或会话根打断时直接 `return`，不计数、不建句柄；拨号不与停止竞速，转发与 `opts.stop` 竞速、停止先到就 `token.cancel()` 再 `relay.await`（不丢掉转发的 future）；`Sniffed::Failed` 直接 `return`。`reaches_the_rules`（`:235`）与 `choose_policy_with` 的条件相同（`Mode::Rule` 真、`Mode::Proxy` 而 `global_policy()` 是 `None` 或 `policy_known` 为假、`Mode::Direct` 假），不用 `snapshot()`；`stopped`（`:248`）`Some(t)` 等 `t.cancelled()`、`None` 是 `std::future::pending()`。`src/engine.rs`：字段 `pub(crate) sniff_counts: crate::serve::SniffCounts`（`:203`，在 `fake_pool` 之后）与 `new` 里的初值（`:266`）；`pub(crate) fn sniff_wait`（`:552`，在 `shared()` 旁边，只读字段、不克隆 `EngineShared`）；`pub fn http_counts`（`:1376`，`traffic()` 之后同一个 `impl` 块）；`impl Dialer for Engine` 的 `sniffable`（`:1719`，转调 `sniffable_session`）与 `serve`（`:1723`，`Box::pin(self.serve_sniffed(…))`），`ServeOpts` 进 `rurge_inbound` 的 import（`:23`）。`src/observe.rs`：`sni: s.sni.clone().or_else(|| h.sni())`（`:125`），上面三行英文注释说理由（设计第 7.2 节：嗅探读出的完整 SNI 在会话信息里，侧槽是 `relay` 的首块 8 KiB 嗅探写的），`protocol` 那一行不动。`src/lib.rs`：`mod serve;`（`:20`，在 `pub mod runtime;` 与 `pub mod shared;` 之间）与 `pub use serve::{HttpCounts, SniffedCounts};`（`:45`）。执行者的裁定三处：（一）`sniffable` 的判定本体叫 `sniffable_session`，`impl Dialer` 里的方法只有一行转调（同 `fake_address.rs` 的写法）；（二）`reaches_the_rules` 是 `Engine` 的私有方法、放在 `serve.rs` 的 `impl Engine` 块里；（三）字段与返回类型写全路径 `crate::serve::…`，不为它们加 `use`。用例 `tests/sniff.rs`（新，五条，都用普通的 `#[tokio::test]`，等待用 `common` 的有界写法）：`which_sessions_are_sniffable`（`:26`，派工表的十一行逐行断言，断言信息带入口、传输、主机与端口）、`a_paused_engine_sniffs_nothing`（`:70`）、`a_session_that_pre_matching_refuses_is_not_sniffed`（`:89`，先 `Mode::Direct` 再问）、`fake_addresses_at_a_proxy_port_are_judged_by_their_names`（`:118`，`Rig` 与 `rig.fakes`，`198.19.255.254` 没有名字）、`extended_matching_rules_a_tls_session_by_its_sni`（`:150`）。执行者的裁定：表写成 `use ListenerKind::{Forward, Http, Internal, Socks5, Tun}; use Transport::{Tcp, Udp};` 之后逐行写 `(Socks5, Tcp, "target.test", 443, true)`，否则 rustfmt 把五行拆成三十多行；`extended_matching…` 另断言 `record.dst` 是 `target.test:443`。P9 照修正记录第 12 行改了六处（只改端口 443 → 8443、80 → 8080，写到这个端口的地方一起改，断言不改，每处一行英文注释）：`tests/outbounds.rs` 的 `a_refusing_upstream_is_a_502_that_quotes_the_proxy`（`:178`、`:181`）与 `a_broken_hop_is_named_in_the_error`（`:442`、`:445`）；`tests/pipeline.rs` 的 `reject_rules_close_serve_gifs_or_render_pages`（`:437` ～ `:447`，`CONNECT` 与 SOCKS5 两处）与 `dns_failure_and_ip_rules`（`:1083`、`:1085`）；`tests/process.rs` 的 `a_record_shows_a_process_found_after_the_session_ended`（`:599`、`:601`）；`tests/tun.rs` 的 `a_rejected_tcp_connection_is_reset`（`:154` ～ `:158`） | 本提交 |
| 30 | 6 | 判据（一台机器上的一次运行，不是 CI）。先红：只写好 `tests/sniff.rs`、`impl Dialer` 还没接 `sniffable` 时五条全红——`which_sessions_are_sniffable` 红在第一行（`Socks5 Tcp target.test:443` 期望真、得到假）、`a_paused_engine_sniffs_nothing` 红在第一句、`a_session_that_pre_matching_refuses_is_not_sniffed` 红在「the adapter's sessions asked pre-matching at the SYN」、`fake_addresses_at_a_proxy_port_are_judged_by_their_names` 红在第二个断言（假地址没有被换成名字）、`extended_matching_rules_a_tls_session_by_its_sni` 拿到的规则是 `DOMAIN,target.test,A`（SNI 根本没被看见）。`cargo fmt --all --check` 退出码 0；`cargo clippy --all-targets -- -D warnings` 0 个警告（退出码 0）；`cargo test -j 8 --workspace --no-fail-fast` 88 个结果行、4738 通过、0 失败、13 忽略——这一遍里 `rurge-engine` 的 `udp` 那个二进制以 `STATUS_HEAP_CORRUPTION` 退出（本机管控软件注入的钩子 DLL，与本任务无关，第一次全量里别的二进制都过），单独重跑 `cargo test -j 8 --workspace --test udp` 得 38 passed；两次的数合起来是 89 / 4776 / 0 / 13，比任务 5 的基线（88 / 4771 / 0 / 13）多 1 个结果行（新的 `sniff` 二进制）与 5 个通过（本任务的五条用例），失败与忽略不变。P9 的六条改端口之后照过（`tests/outbounds.rs` 19、`tests/pipeline.rs` 78、`tests/process.rs` 14、`tests/tun.rs` 22），修正记录第 13 行「预计照过」的十二条也照过（`tests/network.rs` 7、`tests/outbounds_ssh.rs` 4、`tests/smart.rs` 22、`tests/tun_dns.rs` 23 都在里面），名单之外没有变红的。带等待的两条（`fake_addresses_at_a_proxy_port_are_judged_by_their_names` 与 `extended_matching_rules_a_tls_session_by_its_sni`）提交前把整个 `sniff` 二进制单独循环跑了二十次（`for i in 1..20: cargo test -j 8 --workspace --test sniff`），二十次全 5 passed。`Cargo.lock` 的 `[[package]]` 仍是 520（没有新的包），`LISTED` 仍是 23 项（没有新的放开）。日志 `gate-task-6.log`、`rerun-task-6-udp.log`、`loop-task-6.log` 在工作区（git 忽略）。需要参照服务端的互操作用例在本机立刻返回，算在通过的数里 | 本提交 |
| 31 | 6 | 修正第 1 轮（评审）：三处，行为不变。（一）`serve.rs:132` 关键字那一段的注释把设计第 4.1 节的条目引错了（关键字属于 (4)，不是 (5)），`(5)` 改成 `(4)`，别的字不动；它前后的 `:128` 的 `(4)` 与 `:142` 的 `(5)` 本来是对的。（二）`tests/tun.rs:154` ～ `:156` 的注释原来说「拒绝仍在答复之前」，而虚拟网卡上没有这个次序（握手在求规则之前就完成了），改成：8080 不嗅探，规则在握手一完就求、不等 app 的第一批字节（这条用例从不写）；换成 8080 的理由仍是为这个。（三）`tests/sniff.rs` 的 `extended_matching_rules_a_tls_session_by_its_sni` 里握手（`:172` ～ `:177`）、`tls.write_all(b"ping")`（`:178` ～ `:181`）与 `tls.read_exact`（`:184` ～ `:186`）三步原来都没有时限，嗅探或转发坏了会把这一个测试二进制挂住；各包上 `tokio::time::timeout(Duration::from_secs(5), …)`，超时一句话说出等的是什么（`Duration` 由 `common::*` 带进来，没另加 `use`），断言、次序与别的步骤不动。门禁（协调者的决定：不跑全量）：`cargo fmt --all --check` 退出码 0；`cargo clippy --all-targets -- -D warnings` 0 个警告（退出码 0）；`cargo test -j 8 --workspace --test sniff` 5 passed；`cargo test -j 8 --workspace --test tun -- a_rejected_tcp_connection_is_reset` 1 passed；改过等待的那一条单独循环跑了二十次（`cargo test -j 8 --workspace --test sniff -- extended_matching_rules_a_tls_session_by_its_sni`），二十行都是 `1 passed`（日志 `loop-task-6-fix-1.log` 在工作区，git 忽略）。没有重跑，没有 `STATUS_HEAP_CORRUPTION` | 本提交 |
| 32 | 7 | 第 1 ～ 16 条用例都写在 `tests/sniff.rs`，现有的五条不动；`use` 只在 `:17` 从 `rurge_engine::Mode` 改成 `rurge_engine::{HttpCounts, Mode, SniffedCounts}`。夹具五处（执行者的裁定：都是文件末尾、新用例之前的同一个小节，名字与派工的不同之处见下）：`sniffing()`（`:223`，`EngineShared { sniff_wait: Duration::from_millis(300), ..EngineShared::default() }`，交给 `harness_with`）、`handshake(stream)`（`:233`，`rurge_net::http::tls_client_config(true)` 与 `tokio_rustls::TlsConnector`，`ServerName` 是 `sni.test`，握手包在 5 秒的 `timeout` 里；泛型 `S: AsyncRead + AsyncWrite + Unpin`，所以 SOCKS5 与 CONNECT 的 `TcpStream` 与虚拟网卡的 `AppTcp` 用同一个）、`ping_over`（`:249`，一次 TLS 往返，写与读各有 5 秒）、`socks5_connect`（`:271`，照 `tests/pipeline.rs` 的同名函数按名字写，返回整个应答与流）、`two_policies(to)`（`:294`，两个 `FakeSocks5` 与 `[Proxy]` 文本，取代每个用例各写一遍）。逐条：1 `:315` 一个引擎两种入口（先 `connect_via_http`，再 `Rig::on(h)` 与 `rig.fakes`），两条记录的 `listener` 各是 `Http` 与 `Tun`、`dst` 都是 `target.test:443`（TUN 上是名字不是 fake 地址）、`B` 被问两次、`A` 没被问；2 `:369` 不带标志时 `DOMAIN,target.test,A` 命中，SNI 仍记在记录上；3 `:401` SOCKS5 的 0x00 之后握手失败、CONNECT 的 200 之后同样失败，两条记录都是 `Rejected("REJECT")`、`rule` 是 `PROTOCOL,HTTPS,REJECT`、SNI 与 `Https` 都在，`A` 与 `B` 都没被问；4 `:450` `TestServer` 与 `get` 拿到 200、记录是 `Http`，另起的引擎上 `PROTOCOL,HTTP,B` 先命中；5 `:499` 无设置时 0x00 之后写 `hello` 读到空 EOF、有 `always-raw-tcp-hosts` 时 0x05；6 `:546` `TARGET` 对上 `target.test` 给 0x05；7 `:571` 端口、缺省端口、无设置三种各一条；8 `:616` 预匹配的 SOCKS5 0x02、CONNECT 读到空、去掉标志后 0x00；9 `:668` 暂停时 0x05、记录 `Failed` + `REFUSED_STRICT`、`A` 没被问；10 `:700` 服务器先说的 300 毫秒等待与 `null` 协议；11 `:757` `GE` 与其余字节分两次写、10 KiB 全量回读；12 `:798` 写 `GE` 后关写端、读回 `GE` 与 EOF、记录以完成结束；13 `:842` 内联 `[Ruleset sni]` 的两行与 `RULE-SET,sni,B`；14 `:869` 五个会话五种结局各计一次；15 `:956` 缺省 15 秒、停 TUN 之后应用立刻被放掉、300 毫秒后没有记录也没有在跑的会话；16 `:998` `REJECT-DROP` 经虚拟网卡、应用关写端后读到 EOF。执行者的裁定三处：（一）第 8 条 CONNECT 那一半用有界的 `read_to_end` 断言读到的为空（`tests/pipeline.rs` 的 `http_exchange` 是这个文件的私有函数，`sniff.rs` 里没有），与那一处的断言同义；（二）第 15 条的规则要给一个存在的策略，所以给它一个 `FakeSocks5` 的 `A`（派工里 `guarded("DOMAIN,target.test,A")` 的 `A` 在 `[Proxy]` 里不存在，`harness_with` 会以 `rule references unknown policy` 报错），并多看一句 `a.requests()` 为空，钉住「停止之后不拨号」；（三）第 10 条的用时从 `socks5_connect` 之前量起，量到读完 `220 hello\r\n`，所以含连接与嗅探的等待 | 本提交 |
| 33 | 7 | 判据（一台机器上的一次运行，不是 CI）。**先红**（本任务不改产品代码，红靠临时改期望，四处各改一处、看到红之后改回，都不是让会话离开回环的改动）：「`extended_matching_rules_a_tls_session_by_its_sni_on_every_entry` 的第二条记录的 `listener` 期望临时由 `Tun` 改成 `Http`，失败是 `assertion left == right failed: left: Tun right: Http`」「`protocol_https_refuses_before_the_dial` 的 SOCKS5 应答码期望临时由 `0x00` 改成 `0x05`，失败是 `left: 0 right: 5`，带出的应答是 `[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]`」「`the_sniffs_are_counted` 的期望临时由 `tls: 1` 改成 `tls: 2`，失败是左边 `SniffedCounts { tls: 1, http: 1, other: 1, timeout: 1, closed: 1 }`、右边 `{ tls: 2, … }`」「`always_raw_tcp_hosts_keep_the_dial_first` 的后一半期望临时由 `0x05` 改成 `0x00`，失败是 `left: 5 right: 0`，带出的应答是 `[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]`」。带等待的十六条提交前把整个 `sniff` 二进制（21 条）单独循环跑了二十次（PowerShell 的 `for ($i = 1; $i -le 20; $i++) { cargo test -j 8 --workspace --test sniff }`，每轮要求 `test result: ok. 21 passed`），二十次全过（日志 `loop-task-7.log` 在工作区，git 忽略）。`cargo fmt --all --check` 退出码 0；`cargo clippy --all-targets -- -D warnings` 0 个警告（退出码 0）；`cargo test -j 8 --workspace --no-fail-fast` 89 个结果行、4792 通过、0 失败、13 忽略——结果行数与任务 6 的基线（89 / 4776 / 0 / 13）相同（`sniff` 这个二进制已经有了），通过数多 16（本任务的十六条），失败与忽略不变；没有二进制以 `STATUS_HEAP_CORRUPTION` 或 `STATUS_ACCESS_VIOLATION` 退出，因此没有重跑。`Cargo.lock` 的 `[[package]]` 仍是 520（没有新的包），`LISTED` 仍是 23 项（没有新的放开）。需要参照服务端的互操作用例在本机立刻返回，算在通过的数里。日志 `gate-task-7.log` 在工作区（git 忽略） | 本提交 |
| 34 | 7 | 修正第 1 轮（评审）：三处，行为不变。（一）`tests/sniff.rs` 的 `extended_matching_rules_a_tls_session_by_its_sni_on_every_entry` 在 CONNECT 那一半的 `ping_over` 之后、`Rig::on(h)` 之前加了 `the_records(&h, 1).await;`（上面两句英文注释说它的用意）：`the_records` 按写完记录的先后返回，而 CONNECT 那一半的收尾要等客户端 EOF、上游关闭与回显服务器关闭，原先两条记录的次序靠的是「收尾比后一半快」，全量并行跑时没有东西保证它；先等第一条写完，`records[0]` 是 `Http`、`records[1]` 是 `Tun` 就由用例自己钉住了，后面按下标的断言不动。（二）`force_http_engine_hosts_sniff_another_port` 第一段的 `read_to_end` 之后补一句 `assert!(back.is_empty(), "nothing was echoed: {back:?}")`（`A` 拒绝、拨号失败、隧道关掉，读到的是空，照第 5 条的写法）。（三）`pre_matching_still_refuses_before_the_answer` 第二段同样补一句 `assert!(back.is_empty(), "the refusal answers nothing: {back:?}")`。门禁（协调者的决定：只改了测试文件里的一次等待与两句断言、产品代码不变，不跑全量）：`cargo fmt --all --check` 退出码 0；`cargo clippy --all-targets -- -D warnings` 0 个警告（退出码 0）；`cargo test -j 8 --workspace --test sniff` 21 passed；整个 `sniff` 二进制循环跑了二十次，二十行都是 `21 passed`（日志 `loop-task-7-fix-1.log` 在工作区，git 忽略）；没有二进制以 `STATUS_HEAP_CORRUPTION` 或 `STATUS_ACCESS_VIOLATION` 退出，因此没有重跑 | 本提交 |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | **M1b 的计划**：M1a 合并之后，对着那时的代码写（设计第 2 节） | 协调的会话 |
| 2 | **推送与读 CI**：M1a 与 M1b 的提交都在本地，M1b 做完推送一次 | 项目所有者推送；之后的会话读回那次运行的注解与任务摘要 |
| 3 | **手工验收**：M1 的行为（浏览器经三种入口上网、443 上按 SNI 分流）随 M2 的那一份阶段 4 手工清单写进去；阶段 1、2、3 的清单照旧一项也没有做过 | 项目所有者（手工验收） |
| 4 | **所有者可以改的裁定与开放问题**：设计第 19 节（M1-D3、D5、D7、D11 等；总设计 Q1、Q6） | 项目所有者 |
