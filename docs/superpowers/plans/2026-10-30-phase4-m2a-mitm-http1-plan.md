# 阶段 4 / M2a「MITM：HTTP/1.1 上的解密」实施计划

> **执行方式**：交给 Pi，由协调的会话逐个任务派工、逐个任务评审（设计第 2 节、M2-D1；做法同 M1a、M1b）。
>
> - 协调的会话每个任务开一个 Pi 会话，交给它这个任务的全文与要用的接口（派工文件）。Pi 做完把报告写进派工文件指定的路径，在本地提交。
> - 协调者读改动、重跑门禁、评审。有问题回到同一个 Pi 会话修正，再评审；过了才派下一个任务。
> - 每个任务：先写失败的用例、看到红；再实现到绿；跑门禁；对照本任务的「判据」与「全局约束」逐条自查；最后提交（**只在本地，不推送**）。
> - 与计划的出入记进末尾的「执行期修正记录」，**不改计划的正文**。
>
> 没有等 CI 的停点：M2a 没有平台代码（设计开头）。M2 的三份计划都做完之后由项目所有者推送一次（连同 M1），协调者读回那次 CI（M2-D1）。

**目标**：`[MITM]` 生效。判为 TLS 的可嗅探会话在开关开着、有可用的 CA、来源允许、名单命中、ALPN 合得上时，rurge 用 `[MITM]` 的 CA 当场签一张叶证书、与客户端完成 TLS 握手，把解密之后的 HTTP/1.1 请求交给 M1b 的引擎：每个请求照旧各自求规则、选策略、借或拨上游连接、各有一条记录，只是上游一侧多一层 TLS、URL 以 `https://` 开头、协议是 `HTTPS`。具体是：

- `rurge-config`：`[MITM]` 九个键类型化（生效的名单、来源名单），`[MITM]` 不再是"解析了但不生效"的节；诊断码 `W0039`。
- `rurge-proto` / `rurge-net`：PKCS#12 的解码拆出来共用；解密的上游的客户端配置；系统 ∪ `webpki-roots` 的根。
- `rurge-http` 的新模块 `mitm`：CA 的载入与六条检查（`Authority`）、叶证书的签发与缓存、对客户端的服务端配置与握手、何时解密（`Mitm`）。`engine.rs`：解密的连接上的请求（上游的 TLS、`https://`、`HTTPS`）。
- 引擎：
  - 每一代的 `Mitm`，同样的材料沿用同一个 `Authority`；
  - `mitm` 开关与 `state.json` 第 2 版；
  - 可嗅探的会话随名单扩大，解密的分派；
  - 握手失败与证书固定的提示：日志、限频、计数、承载连接的记录；
  - `load_checked` 加 CA 的检查。
- API：`/v1/features/mitm` 生效、`GET /v1/mitm/ca`、`/v1/rurge/http` 的 `mitm`。
- 端到端用例；文档收尾（M2a 的部分）。

**架构**：没有平台代码，全部在开发机上编译、运行。

- 依赖方向不加新的工作区边。`rurge-http → rurge-proto`、`rurge-engine → rurge-http` 在 M1 就有。
- `rurge-http` 加的外部依赖全是锁文件里已有的包：rcgen（开 `x509-parser` 特性）、x509-parser、time、lru、rustls（从开发依赖挪到正式依赖）、tokio-rustls、sha2、ring。
- `rurge-http` 照旧不认识引擎。解密与否、握手、签证书在 `rurge-http::mitm` 里；日志、计数、记录、开关与"用哪一代"在引擎里。
- `Authority` 活得比一代配置长：由引擎的 `EngineShared` 记着最后一个生效过的那个。`Mitm` 每一代新建。
- `rurge-api` 照旧只依赖 `rurge-engine` 与 `rurge-config`：`GET /v1/mitm/ca` 经 `Engine::mitm_ca_der` 取证书。

**技术栈**：都已在 `Cargo.lock` 里、源码在本机 cargo 注册表里（设计开头「调研」一行）。**不下载、不加新包**（M2-D2）。

- rcgen 0.14.7：`Issuer::from_ca_cert_der`、`CertificateParams`、`KeyPair`、`SanType`、`SerialNumber`、`CustomExtension`。
- x509-parser 0.18.1、time 0.3.45、lru 0.18.4、p12-keystore 0.2.1、ring 0.17（只用随机数）、hyper 1.11.1。
- rustls 0.23.43：`ServerConfig`、`server::ServerSessionMemoryCache`、`sign::SingleCertAndKey`、`crypto::ring::sign::any_ecdsa_type`、`client::WebPkiServerVerifier`。
- tokio-rustls 0.26.4：`TlsAcceptor`、`TlsConnector`。

**依据**：

- M2 细化设计（下面写"设计"指它）：`docs/superpowers/specs/2026-10-30-phase4-m2-mitm-design.md`。
  - M2a 用到第 3 ～ 9、11、12、15 ～ 17 节与已决事项 M2-D2 ～ D13、D18、D19、D22 ～ D24。
  - **先读它第 20、23 节**：第 20 节是对总设计的订正，第 23 节是本计划改了设计正文的地方（随本计划提交）。
  - 设计与本计划的「计划期决定」不一致的，以计划为准。
- M1 细化设计第 6 ～ 10 节与 M1b 计划的「计划期决定」：解密的连接跑在 M1b 的引擎上，M1b 的规矩照旧。
- 阶段 4 总设计第 7 节（MITM）、第 10.1 节（开关）。
- 手册的原话在设计里（`http/mitm.html`、`profile/host-list.html`、`profile/keystore.html`、`tools/http-api.html`、`profile/general.html`）。
- 写计划时对着 `25b2bbc`（M2 设计提交之后的 `phase4-m2`）核对过的事实写在「计划期决定」里，外部库的事实对着注册表里锁定版本的源码。

## 全局约束

**依赖**

- 工作区 `Cargo.toml` 的 `[workspace.dependencies]` 加两行：`x509-parser = "0.18"`、`time = "0.3"`（今天是传递依赖，锁到 0.18.1 与 0.3.45）。
- `rurge-http` 的 `[dependencies]`（任务 2）加：
  - `rcgen = { workspace = true, features = ["x509-parser"] }`；
  - `x509-parser`、`time`、`lru`、`tokio-rustls`、`sha2`、`ring`，都写 `*.workspace = true`；
  - `rustls` 从 `[dev-dependencies]` 挪过来。
- `rurge-http` 的 `[dev-dependencies]` 加 `rurge-proto = { workspace = true, features = ["testing"] }` 与 `base64.workspace = true`。
- 不加外部包。`Cargo.lock` 的 `[[package]]` 今天 520 个，每个任务之后照旧 520 个。改了 `Cargo.toml` 之后第一次编译用 `cargo check --offline --workspace --all-targets`：它失败就停下报告——那说明要下载，不许。
- 依赖方向：
  - `rurge-tun`、`rurge-inbound`、`rurge-api`、`rurge-config` 不依赖 `rurge-http`；
  - `rurge-http` 不依赖 `rurge-engine`。
- `unsafe_code`：不加放开的函数，`crates/rurge-platform/tests/unsafe_functions.rs` 的 `LISTED` 仍是 23 项。

**缺省的 `cargo test` 不碰系统**

- 同 M1b：不建网卡、不改系统的任何设置；不执行 `netsh`、`route`、`ipconfig`、`ip`、`resolvectl`、`networksetup`；虚拟网卡的用例只用 `rurge-tun` 的内存设备与引擎用例的 `Rig`。
- **测试从不把任何 CA 装进系统或浏览器的信任库**（总设计 P4-D26、M2-D24）。
  - 测试的客户端是 rustls 的客户端，根只有测试自己的 MITM CA。
  - 源站是回环上的 TLS 测试服务器，证书来自测试自己的另一个 CA。要引擎信任它时，经 `EngineShared.roots` 交给引擎（`harness_trusting`）。
- 测试的 CA 与 PKCS#12 在内存里生成：rcgen 与任务 2 加的 `rurge_proto::testing::p12_base64`。
- rcgen 的 ring 后端生成不了的五种夹具，由**协调者**在派任务 2 时用本机的 OpenSSL 生成、以 Base64 写进派工文件，Pi 把它们写成常量（P6）。**Pi 不运行 OpenSSL**，也不读写用例目录之外的证书文件。

**测试的约定**

`CLAUDE.md`「测试约定」整节适用。写经引擎的用例之前读 `docs/testing.md`「测试约定（全文）」：阶段 3 / M3b、M3c 两节，与阶段 4 的 M1a、M1b 两节。与本计划直接相关的：

- 只用回环与端口 0。用例绑不了 443：
  - 会话的目标是 443 时照 M1b 的 P14，规则把它送到一个 SOCKS5 策略（`FakeSocks5`，`Socks5Script.connect_to` 指向用例的回环源站）；
  - 要源站自己的端口进来的用例（可嗅探的扩大）在 `[MITM] hostname` 里写明那个端口。
- 写了主机名的 Profile 另给第二道防线：`[Host]` 把名字钉到 `127.0.0.1`，另加一条按地址的规则；经引擎的 TUN 用例用 `guarded`。
- 嗅探的等待调短到 300 毫秒（`EngineShared.sniff_wait`）。TLS 的客户端连上就发 ClientHello，嗅探在它到齐时就判出 `Tls`，用例不必等满 300 毫秒。
- 等待都有时限。固定等待只许"断言什么都没发生"，注释写它等的是什么、余量多大。
- 新写的、带等待的用例提交前单独循环跑二十次以上（任务 3、5、6、8）。
- 抓日志用 `LogCapture`（`crates/rurge-engine/tests/common/log_capture.rs`），跑在当前线程的运行时上（普通的 `#[tokio::test]`）。
- 测试里手写的 HTTP 客户端把每个请求——请求头与正文——在一次写里发出（`CLAUDE.md`）；经 TLS 时是一次 `write_all`。
- **内联的测试模块放在文件的末尾**：`crates/rurge/tests/sockets.rs` 读生产代码，读到第一个 `#[cfg(test)] mod … {` 为止。只装测试常量的文件以 `#[cfg(test)] mod fixtures;` 声明。

**门禁与这台开发机**（每个任务末尾一次；说"通过"之前必跑）

- 门禁：`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test -j 8 --workspace --no-fail-fast`。
- 本机 msvc 工具链没有 rustfmt。跑 `cargo fmt` 之前让环境变量 `RUSTFMT` 指向 gnu 工具链里同版本的 rustfmt：用户目录下的 `.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe`。不要为此安装或升级工具链。
- 聚焦地跑用例也经整个工作区：单元测试 `cargo test -j 8 --workspace --lib -- <过滤词>`；集成测试 `cargo test -j 8 --workspace --test <名字> -- <过滤词>`。
- **不用 `-p`**：它另编一份依赖，`target/` 涨得很快，写满过 D: 盘。
- 跑全量之前 `df -h /d`：剩余不足约 15 GiB 就停下报告。
- 测试二进制以 `STATUS_HEAP_CORRUPTION`（本机管控软件注入的钩子 DLL）或 `STATUS_ACCESS_VIOLATION` 退出：单独重跑那一个二进制，报告里写明。`cli` 的用例偶发 `give_up` 20 秒超时：单独重跑 `cli`，把标准错误原样抄进报告。
- rustc 在增量编译里报内部编译器错误：删掉出错的那个目标在 `target/debug/incremental/` 下的目录再编。
- 回环的 TCP / UDP 7、9、13、17、19 号端口在这台机器上开着，CI 上关着：用例不假定任何固定端口的状态。
- 结果照实记：本机的全量运行是"一台机器上的一次运行，不是 CI"；需要参照服务端的互操作用例在本机立刻返回、算在通过的数里，要说出来。

**仓库的规矩**（`CLAUDE.md`「必须遵守的规矩」整节适用；与本计划直接相关的）

- 凭据（设计第 4.4、15 节）：
  - 下面这些不进日志、错误文本、诊断、API 输出与 `Debug`：`ca-p12`、`ca-passphrase`、`[Keystore]` 条目的 Base64 与口令、CA 的私钥。
  - `W0039` 的消息里没有它们。
  - `Authority` 的 `Debug` 只写 CA 证书的 subject 与证书 DER 的 SHA-256 的前 8 个十六进制字符。
  - 材料的指纹（P8）不进 `Debug`、日志与 API。
- 日志的分级（设计第 8.2、15 节、M2-D23）：
  - 经 HTTP 与 SOCKS5 监听器来的会话：握手失败与提示的行带名字（SNI 或连接的目标）与端口；
  - 经虚拟网卡来的会话：WARN / INFO 的行不带名字与端口，名字另起一行 DEBUG；
  - 都不带来源地址与进程；
  - 解密之后的请求的日志行照 M1b：路径只在 DEBUG、不写查询串，头与正文不进日志。
- 诊断：新码只有 `W0039`（M2-D22），在 `codes::hint` 的表里放进 `WITHOUT`、写理由。
- 出站套接字只有一处建法：对上游的连接一律经 `Upstreams::dial`（引擎的选路与拨号）建。`rurge-http` 只在拨好的流上做 TLS 与 hyper，不建套接字、不问系统解析库、不建连接器。改动让 `crates/rurge/tests/sockets.rs` 变红就停下报告。
- `rurge-tun` 不改。M1b 的规矩照旧：停止取消正在转发的会话，不取消正在进行的拨号。与客户端的 TLS 握手还没有握着任何句柄，可以与停止竞速（设计第 8.2 节）。
- 文档与报告用写文件 / 改文件的工具改。**永远不要用把一个文件的内容经管道写回同一个文件的命令**（2026-10-27 这样写满过 D: 盘）。

**写法**

- 注释、日志与错误文本用英文，照所在文件的写法：文档注释写"为什么"与前提，句子完整。注释里提到设计或计划，写 `phase 4 M2 design 6`、`the phase 4 / M2a plan's P9`。
- 提交说明照 `git log` 的样子写，标题用中文：`feat(config): …`、`feat(proto): …`、`feat(net): …`、`feat(http): …`、`feat(engine): …`、`feat(api): …`、`feat(cli): …`、`test(engine): …`、`docs: …`。正文说做了什么、改了哪些现有用例的期望、为什么。Pi 的提交不带署名行。
- 只在本地提交：不 `push`、不 `pull`、不 `fetch` 远端、不 `amend`、不改写历史。

**不做的事**

- 只运行本仓库构建出来的二进制与 Pi 自己（OpenSSL 由协调者运行）。不下载、不安装任何东西。
- 不手动跑 `rurge run`、`rurge dns lookup`；`rurge service` 只用 `--dry-run`。
- 参考实现：GPL 的（sing-box、mihomo、Xray-core）只取事实，代码不抄。RFC 是公开的事实。
- 不做设计第 1 节「不做」一栏的事，也不做 M2b、M2c 的事：
  - HTTP/2：M2a 的 ALPN 只认 `http/1.1`；
  - `rurge mitm ca generate` / `export`、`http-api-tls`、`W0040`、手工清单；
  - `auto-quic-block` 的效果；`rewrite` 与 `capture` 开关；
  - 模块里的 `%APPEND%` / `%INSERT%`（阶段 5）；
  - `p12-keystore` 升级（所有者的 Q4）。

**计划没有写到、要做决定的地方**

- 能按设计与总设计的原则定的：自己定，记进修正记录，写"执行者的裁定"。
- 去向写着"项目所有者"的（设计第 22 节），或会改变 M2-D1 ～ D25 的：停下来列出，交给协调者。
- 现有用例变红：P15 的表点了名的照计划改，在提交说明里说；没点名的停下来，记进修正记录，不顺手改期望值。

## 评审要点

设计没写透、而最可能咬到使用者的几种输入，与负责它们的任务和用例：

1. **凭据漏出去**：CA 的 Base64、口令或私钥进了 `Debug`、诊断、日志或 API。
   - 期望：哪儿都没有；`GET /v1/mitm/ca` 只给证书。
   - 用例（任务 1、2、7）：`the_debug_of_a_loaded_profile_holds_no_secret`（加 `[MITM]` 的断言）、`every_ca_problem_is_named_and_quotes_no_secret`、`the_debug_of_an_authority_shows_the_certificate_not_the_material`、`the_mitm_ca_is_given_in_der`。
2. **不该解密的被解密了**：排除项、`hostname-disabled`、来源不允许、开关关着、客户端只给 `h2`。
   - 期望：按原样转发，客户端看到的是源站自己的证书。
   - 用例（任务 3、5、8）：`decides_in_the_design_order`、`with_the_switch_off_the_client_sees_the_origin_certificate`、`a_client_offering_only_h2_is_passed_through`、`exclusions_disabled_entries_and_sources_pass_through`。
3. **挂着的握手、开着的记录**：客户端握手到一半不动、停止落在握手里、同一个名字的连接同时到。
   - 期望：握手 30 秒为限；停止时不留记录、不计数；同一个名字只签一次；每个失败的握手有一条承载连接的记录。
   - 用例（任务 3、5、6）：`a_stalled_handshake_times_out`、`a_stop_during_the_handshake_leaves_no_record`、`concurrent_requests_for_one_name_sign_once`、`a_handshake_the_client_abandons_is_failed_at_info`。
4. **对上游的校验松了**：证书不受信的源站、`skip-server-cert-verify`、SNI。
   - 期望：不受信就 502、记录带 rustls 的原因；`skip-server-cert-verify` 只放过链与名字；对上游的 SNI 是客户端的 SNI，IP 不发。
   - 用例（任务 4、8）：`an_origin_the_roots_do_not_trust_is_a_502_with_the_reason`、`skip_server_cert_verify_reaches_an_untrusted_origin`、`the_origin_hears_the_client_sni_and_an_ip_hears_none`。
5. **重载**：CA 没变、变了、变坏了、过期了。
   - 期望：没变时叶证书与会话缓存跟着同一个 `Authority`；变了换 CA；变坏了新的连接不解密、已经解密的连接照常；过期的 `Authority` 不沿用。
   - 用例（任务 2、5、8）：`an_expired_authority_is_not_usable_now`、`a_reload_with_the_same_material_keeps_the_leaves`、`a_reload_with_another_ca_issues_from_it`、`a_reload_that_breaks_the_ca_leaves_open_connections_decrypted`。
6. **非标准端口**：名单让哪些会话可嗅探。
   - 期望：写明的端口让那个端口可嗅探；`:0` 只扩它命中的目标；不带端口的项不扩。
   - 用例（任务 3、5）：`widens_on_written_ports_and_on_targets_the_list_takes`、`a_written_port_makes_its_port_sniffable`、`a_bare_entry_does_not_sniff_another_port`、`a_zero_port_entry_widens_its_own_target_only`。
7. **`state.json`**：第 1 版的 `"mitm": false`、阶段 3 的代码读第 2 版的文件。
   - 期望：第 1 版的四个开关读作没设过（`mitm` 因此开着）；第 2 版只写设过的、都是布尔，旧代码读得懂、不把文件改名成 `.broken`。
   - 用例（任务 5）：`a_version_1_file_leaves_the_four_switches_unset`、`a_version_2_file_is_what_phase_3_reads`、`the_switch_is_restored_from_state_json`。
8. **刷屏，与虚拟网卡上的名字**。
   - 期望：每种提示每个名字一小时至多一行 WARN；经虚拟网卡来的会话 WARN / INFO 里没有名字；请求头读超时不当成证书固定。
   - 用例（任务 6）：`an_untrusting_client_is_warned_once_an_hour_and_counted_each_time`、`through_the_virtual_interface_the_warnings_name_no_host`、`a_request_head_that_times_out_is_no_pinning_hint`。

---

## 文件结构

| 文件 | 任务 | 职责 |
| --- | --- | --- |
| `crates/rurge-config/src/{mitm.rs（新）,hostlist.rs,general.rs,config.rs,deferred.rs,diagnostic.rs,lib.rs}`、`tests/secrets.rs`、`tests/snapshots/corpus__corpus__kitchen-sink.snap` | 1 | `[MITM]` 类型化；`W0039` |
| `Cargo.toml`、`crates/rurge-http/Cargo.toml`、`crates/rurge-proto/src/{keystore.rs,transport/tls.rs,testing/tls.rs}`、`crates/rurge-net/src/tls.rs`、`crates/rurge-http/src/{lib.rs,mitm/mod.rs（新）,mitm/authority.rs（新）,mitm/fixtures.rs（新）}`、`crates/rurge-engine/src/outbounds.rs`、`crates/rurge/src/cli/run.rs`、`crates/rurge/tests/cli.rs` | 2 | 依赖；PKCS#12 的解码、上游的客户端配置、两种根；CA 的载入与检查；`load_checked`；重载的检查挪进阻塞线程 |
| `crates/rurge-http/src/mitm/{authority.rs,decrypt.rs（新）,mod.rs}` | 3 | 叶证书与缓存；`Mitm`：何时解密、服务端配置、握手、`Watch` |
| `crates/rurge-http/src/engine.rs`；`Conn` 的构造处（`crates/rurge-engine/src/{serve.rs,http.rs}`） | 4 | 解密的连接上的请求 |
| `crates/rurge-engine/src/{mitm.rs（新）,shared.rs,runtime.rs,engine.rs,serve.rs,state.rs,lib.rs}`、`crates/rurge/src/cli/run.rs`、`crates/rurge-engine/tests/mitm.rs`（新） | 5 | 每一代的 `Mitm`、开关与 `state.json` 第 2 版、可嗅探的扩大、解密的分派 |
| `crates/rurge-engine/src/{mitm.rs,serve.rs,engine.rs}`、`tests/{mitm.rs,sniff.rs}` | 6 | 握手失败、证书固定的提示、限频、计数、承载连接的记录 |
| `crates/rurge-api/src/{routes/features.rs,routes/mitm.rs（新）,routes/rurge_http.rs,routes/mod.rs,lib.rs}`、`tests/api.rs`、`crates/rurge/tests/cli.rs` | 7 | API；启动时还原开关的用例 |
| `crates/rurge-engine/tests/mitm.rs` | 8 | 端到端 |
| 文档（任务 9 列全） | 9 | 收尾 |

模块的名字与拆法可以照实际调整，调了记进修正记录。

---

## Task 0：开工前核对

**做什么**：对着当时的分支核对下面每一项。有出入的记进「执行期修正记录」，一项一行，写"计划写 X，代码是 Y，按 Y 做"；按记录调整后面的任务。**不改计划的正文。** 没有出入也记一行。

1. **工作树**：`git status --short` 空；当前分支 `phase4-m2`；`10d5925`（M1b 合并之后的 `main`）在历史里，之后只多了 `25b2bbc`（M2 设计）与本计划的提交。
2. **读**：
   - 设计第 3 ～ 9、11、12、15 ～ 17、19 ～ 23 节（第 20、23 节先读）；
   - 总设计第 7 节；
   - 本计划的「计划期决定」与 P15 的表；
   - `docs/status.md` 阶段 4 / M1b 一节。
3. **名字与签名**：设计第 19 节 V1 ～ V15 与 V19（V16 ～ V18 属于 M2b、M2c）。用 `git grep -n` 逐项找到、记下行号；与「计划期决定」写的不同的照实记。
   - `rurge-config`：
     - `general.rs`：`parse_general`、`bool_or`、`invalid`、`parse_host_list`、`is_key_name`、未知键的那一支（"unknown [General] key"）；
     - `value.rs`：`split_definition`、`split_list`、`parse_bool`；
     - `text/mod.rs`：`Profile::section`、`Section::active_entries`、`KEY_VALUE_SECTIONS`；
     - `hostlist.rs` 全部；`types.rs` 的 `HostName::{parse, canonical}`；
     - `config.rs`：`Config`（它的 `derive`）、`from_profile` 里 `parse_general`、`[Keystore]` 与 `W0016` 的位置、`ConfigSummary`；`E0021` 让不让加载失败；
     - `deferred.rs`、`keystore.rs`（`KeystoreItem`、`KeystoreType`、它的 `Debug`）；
     - `diagnostic.rs`：末尾的码与 `hint` 的三张表；`redact.rs` 的 `SECRET_KEYS` 里的 `ca-p12`、`ca-passphrase`。
   - `rurge-proto`：
     - `keystore.rs`：`decode_base64`、`decode_p12` 与它们的全部调用处；
     - `transport/tls.rs`：`ClientIdentity`、`Mode`、`Verifier`、`client_config`；
     - `testing/tls.rs`：`TlsFixture`（`issuer` 与 `ca` 两个字段、`client_p12_base64`、`acceptor`、`seen`）。
   - `rurge-net`：`tls.rs` 的 `root_store`；`connector.rs` 的 `BoxedStream`、`AsyncStream`；`testing.rs` 的 `TestServer::{spawn_tls, spawn_tls_with, url, connections}`。
   - `rurge-http`：
     - `engine.rs`：`Conn` 与它的全部构造处、`prepare`、`connect`、`redial`、`DialError`、`failed`（它调不调 `mark_upstream_failed`）、`promote`（两支）、`form_of`、`origin_authority`、`Served`、`serve_connection`；
     - 测试模块的 `Fake`、`Rig`、`rig_head_wait`、`a_request_without_host_is_named_by_its_target`；
     - `tls.rs` 的 `ClientHello`；`prefix.rs` 的 `Prefixed`。
   - `rurge-engine`：
     - `serve.rs`：`sniffable_session`、`named_at_a_proxy_port`、`serve_http`、`serve_sniffed`、`HttpCounts`、`SniffCounts`；
     - `shared.rs`：`EngineShared` 与全部 `EngineShared {` 字面量（含用例）；
     - `runtime.rs`：`Runtime`、`Runtime::build`（异步；`opts.shared.roots` 的用法）；
     - `engine.rs`：`Engine` 的字段、`Engine::new`、`publish_generation`、`attach_state`、`set_mode`、`http_counts`、`runtime()`、`fake_pool`、`turn_back`、`look_up_owner`、`attach_observers`、`HANDSHAKE_TIMEOUT`；
     - `reload.rs` 的 `swap_runtime`；
     - `state.rs` 全部与 `Features` 的全部用处；
     - `outbounds.rs` 的 `load_checked`、`dry_build`；`load_checked` 的全部调用处逐个看在不在阻塞线程上（P12）。
   - 可执行文件：`cli/run.rs` 的 `reload`（调 `load_checked` 的那一行）、`attach_state` 与它附近拿到 `State` 的地方。
   - `rurge-api`：`routes/features.rs`、`routes/rurge_http.rs`、路由表、`error.rs` 的 `not_found`、`auth.rs` 对 `?x-key=` 的处理；`tests/api.rs` 的 `api_built`（P27）、`Api.store`、`state_path`。
   - 用例夹具：
     - `crates/rurge-engine/tests/common/mod.rs`：`Profile`（`sections`、`keystore` 两个字段）、`harness_trusting`、`harness_with`、`harness_timed`、`connect_via_socks5`、`connect_via_http`、`the_records`、`wait_until`、`origin_addr`；
     - `common/tun.rs` 的 `Rig`（`on`、`fakes`）、`guarded`、`tun_records`；`common/log_capture.rs` 的 `LogCapture`；
     - `rurge-tun` 测试用的 `AppTcp`（实现 `AsyncRead` / `AsyncWrite`）；
     - `crates/rurge/tests/cli.rs` 的 `rurge_command`、`rurge_process`。
   - 外部库（注册表源码）：
     - rcgen 0.14.7：`Issuer::from_ca_cert_der`；`DistinguishedName::from_name`（多值 RDN 返回 `CouldNotParseCertificate`）与 `push`（同一类型后写的覆盖前写的）；`KeyPair::try_from(&PrivateKeyDer)`（丢掉 ring 的原因，P1）；`CertificateParams` 的字段与它的 `Default`（带一个 CN，P9）；`SerialNumber::from_slice`；`CustomExtension`。
     - x509-parser 0.18.1：读 BasicConstraints、KeyUsage、SKI、有效期、subject 的接口。
     - rustls 0.23.43：`ServerConfig`（`Clone`、`cert_resolver`、`session_storage`、`alpn_protocols`、`max_early_data_size`）、`ServerSessionMemoryCache::new`、`sign::SingleCertAndKey`、`crypto::ring::sign::any_ecdsa_type`、`Error::AlertReceived` 与 `AlertDescription::{UnknownCA, BadCertificate, CertificateUnknown}`、`ClientConnection::handshake_kind`。
     - tokio-rustls 0.26.4：`TlsAcceptor::accept` 与它的错误的形状（`io::Error` 里包着 `rustls::Error`）。
4. **数**：`Cargo.lock` 里 `[[package]]` 的个数（520）；`LISTED` 的项数（23）；带本机账户名的文件数（派工文件给出要数的那个名字；数出来是 26）。
5. **现有用例**（P15 的表）：
   - 逐条找到、读它断言什么，把"预计要改 / 预计照过"逐条写进修正记录，一条用例一句。
   - 另扫一遍：`git grep -n '\[MITM\]'`、`git grep -n 'features'`（各 crate 的 `tests/` 与 `src` 里的测试模块）、`git grep -n 'EngineShared {'`、`git grep -n 'Conn {'`、`git grep -n 'HttpCounts {'`、`git grep -n 'STATE_VERSION\|"version"'`。表里没有的照实补一行。
6. **门禁**：跑一遍，记下测试二进制数、通过数、失败数与忽略数（M1b 收尾时是 90 / 4867 / 0 / 13，一台机器上的一次运行，不是 CI）。

**判据**：修正记录里有任务 0 的行；门禁的数字记下。

---

## Task 1：`rurge-config`：`[MITM]` 类型化、`W0039`

**改哪里**（设计第 4 节；M2-D3、D22；P2、P3、P25）

1. `hostlist.rs` 的 `impl HostList` 加三个方法（文档注释写依据）：
   - `pub fn matches_sni(&self, host: &HostName, port: u16) -> Option<bool>`：同 `matches`，只是 `PortSpec::Default` 的项不看端口（`phase 4 M2 design 7.3`）。
   - `pub fn without(&self, other: &HostList) -> HostList`：`self` 里去掉"文字与 `other` 的某一项相同"的项，次序与 `default_port` 不变，`invalid` 照抄 `self` 的。"文字相同"见 P3。
   - `pub fn explicit_ports(&self) -> Vec<u16>`：不带 `-` 的项里写明的端口（`PortSpec::Port`，不含 `:0`），升序、去重（设计第 7.6 节）。
2. `general.rs`：`bool_or`、`parse_host_list`、`is_key_name` 改成 `pub(crate)`，别的不动。
3. `mitm.rs`（新）：

   ```rust
   #[derive(Clone, PartialEq, Eq)]
   pub struct MitmConfig {
       pub hostname: HostList,          // default port 443
       pub hostname_disabled: HostList, // default port 443
       pub effective: HostList,         // hostname.without(&hostname_disabled)
       pub ca_p12: Option<String>,
       pub ca_passphrase: Option<String>,
       pub ca_keystore_name: Option<String>,
       pub skip_server_cert_verify: bool, // false
       pub h2: bool,                      // false; read only until M2b
       pub client_source_address: Option<SourceList>, // None: every source
       pub auto_quic_block: bool,         // true; read only until M5
       pub spans: MitmSpans,
   }
   #[derive(Clone, Debug, Default, PartialEq, Eq)]
   pub struct MitmSpans {
       pub hostname: Option<Span>,
       pub ca_p12: Option<Span>,
       pub ca_keystore_name: Option<Span>,
   }
   #[derive(Clone, Debug, PartialEq, Eq)]
   pub struct SourceList { pub entries: Vec<SourceEntry> }
   #[derive(Clone, Debug, PartialEq, Eq)]
   pub struct SourceEntry { pub negate: bool, pub kind: SourceKind, pub raw: String }
   #[derive(Clone, Debug, PartialEq, Eq)]
   pub enum SourceKind { Net(IpNet), Mac([u8; 6]) }

   impl Default for MitmConfig { /* empty lists with default port 443, the defaults above */ }
   impl fmt::Debug for MitmConfig { /* ca_p12 / ca_passphrase: whether set, never the value */ }
   impl SourceList {
       /// The list and the entries that are neither an IP, a CIDR nor a MAC.
       pub fn parse(text: &str) -> (SourceList, Vec<String>);
       pub fn has_mac(&self) -> bool;
       /// design 4.3: the first entry that matches decides; none matches → false.
       /// `ip` is canonicalized first; a MAC entry matches nothing until phase 7.
       pub fn allows(&self, ip: IpAddr) -> bool;
   }
   pub fn parse_mitm(section: Option<&Section>, diags: &mut Diagnostics) -> MitmConfig;
   ```

   `parse_mitm` 照 `parse_general` 的循环写；每个键怎样读、读不懂时给什么，见设计第 4.1 节的表与 P2。

4. `config.rs`：`Config` 加 `pub mitm: MitmConfig`，在 `from_profile` 里紧跟 `parse_general` 由 `parse_mitm(profile.section("MITM"), &mut diags)` 填。`ConfigSummary` 不加东西。
5. `deferred.rs`：`DEFERRED` 去掉 `"MITM"`。
6. `diagnostic.rs`：
   - `pub const W_MITM_CA_UNUSABLE: &str = "W0039";`，文档注释："`[MITM]` names a CA that cannot be used, or lists hosts with no CA: MITM is off (phase 4 M2 design 5.2)"；
   - 放进测试里的 `WITHOUT`；
   - `hint` 的文档注释在"the other codes without one"那一段加一句：`W0039` 的每一种问题在消息里说自己的改法。
7. `lib.rs`：`pub mod mitm;`，照现有的导出风格导出 `MitmConfig`、`SourceList`。
8. 语料库快照 `corpus__corpus__kitchen-sink.snap`：手改两处（P25）——`deferred:` 下去掉 `- MITM` 一行；`W0016` 那条消息去掉 `[MITM], `。

**用例**

- `mitm.rs`（文件末尾的测试模块）：
  - `the_nine_keys_are_read_and_default`：
    - 一节写全九个键，各字段是写的值；
    - 一节什么也不写，各字段是缺省（两张名单空、缺省端口 443；`auto_quic_block` 真；`h2` 与 `skip_server_cert_verify` 假；`client_source_address` 是 `None`）；
    - 没有 `[MITM]` 节同上。
  - `a_key_written_twice_takes_the_later_value_and_names_ignore_case`：`H2 = true` 后跟 `h2 = false` → 假。
  - `bad_values_warn_and_keep_the_default`：
    - `h2 = maybe`、`skip-server-cert-verify = 2` 各一条 `W0012`，取假；`auto-quic-block = x` 一条 `W0012`，取真；
    - `hostname = a.com, [bad` 一条 `W0005`，消息是 ``"`hostname`: invalid entry `[bad` ignored"``，`a.com` 留着。
  - `unknown_keys_are_w0001_tcp_connection_included`：`tcp-connection = true` → `W0001`，消息 ``"unknown [MITM] key `tcp-connection` ignored"``。
  - `a_line_without_equals_is_an_error_that_quotes_nothing`：`ca-passphrase hunter2` → 一条 `E0017`（`expected \`key = value\``），消息与它的提示里都没有 `hunter2`。
  - `hostname_disabled_removes_entries_written_the_same`：
    - `hostname = *.Example.com, example.com, -a.com, b.com:443, c.com`，`hostname-disabled = *.example.com, example.com:443, a.com, d.com`；
    - 生效的名单依次是 `example.com`、`-a.com`、`b.com:443`、`c.com`；
    - `d.com` 不告警。
  - `the_source_list_reads_ips_cidrs_exclusions_and_macs`：
    - `client-source-address = -192.168.1.5, 192.168.1.0/24, ::1, 10.0.0.1, AA:BB:CC:DD:EE:FF, aa-bb-cc-dd-ee-ff`；
    - `allows`：`192.168.1.5` 假（排除项在前）、`192.168.1.9` 真、`10.0.0.1` 真、`::ffff:10.0.0.1` 真（先还原成 IPv4）、`10.0.0.2` 假（一项也不命中）；
    - 整个键恰好一条 `W0029`，消息 ``"`client-source-address`: MAC address entries match no client in this version"``。
  - `an_empty_source_list_is_as_if_unwritten`：`client-source-address =` → `None`、没有诊断；`client-source-address = bogus` → `Some`、一条 `W0005`，`allows` 对任何地址都假。
  - `an_empty_passphrase_is_kept`：`ca-passphrase =` → `Some("")`。
  - `the_debug_shows_neither_the_p12_nor_the_passphrase`：`ca-p12 = SECRETP12`、`ca-passphrase = SECRETPW`，`{:?}` 与 `{:#?}` 里都没有这两个词，有 `ca_p12` 这个字段名。
- `hostlist.rs`：
  - `matches_sni_ignores_the_port_of_a_bare_entry`：
    - `example.com`：`matches_sni(example.com, 8443)` 是 `Some(true)`，`matches` 同样的参数是 `None`；
    - `example.com:8443`：两个方法在 8443 都是 `Some(true)`、在 443 都是 `None`；
    - `example.com:0`：两个方法在任何端口都命中；
    - `-bad.example.com, *.example.com`：`bad.example.com` 两个方法都是 `Some(false)`。
  - `explicit_ports_are_the_ports_of_entries_that_include`：`a.com:8443, -b.com:9443, c.com, d.com:0, e.com:8443, f.com:25` → `[25, 8443]`。
- `deferred.rs`、`config.rs` 里的现有用例照 P15 改。
- `config.rs`：`the_mitm_section_is_typed_and_no_longer_deferred`：`[MITM]\nhostname = a.com\nca-p12 = AAAA\n` → `config.mitm.hostname` 有 `a.com`、`ca_p12` 是 `Some`；没有 `W0016`。
- `tests/secrets.rs`：照文件里查 `Debug` 的写法，断言 `[MITM]` 那两处标记不在 `Config` 的 `{:#?}` 里。`#!MACOS-ONLY` 前缀的那两行照文件里现有的期望。
- `diagnostic.rs` 的 `every_code_is_decided_once` 照过。

**判据**

- 用例全绿；快照只改了那两处，`cargo test -j 8 --workspace --test corpus` 绿；
- `Cargo.lock` 520；
- 门禁绿。

---

## Task 2：依赖、PKCS#12 与根、CA 的载入与检查、`load_checked`

**改哪里**（设计第 3、5、9.2、9.3 节；M2-D2、D4、D13、D22；P1、P4 ～ P8、P12、P26）

1. 依赖：照「全局约束」改工作区与 `rurge-http` 的 `Cargo.toml`，先 `cargo check --offline --workspace --all-targets`。
2. `rurge-proto/src/keystore.rs`（P4）：
   - `pub fn base64_bytes(text: &str) -> Option<Vec<u8>>`：先 `STANDARD`、再 `STANDARD_NO_PAD`。`decode_base64` 改为调它，消息不变。
   - `pub fn decode_pkcs12(label: &str, der: &[u8], password: &str) -> Result<ClientIdentity, BuildError>`：今天 `decode_p12` 里 Base64 之后的那一段，消息里的 ``keystore item `{name}` `` 换成 `{label}`。
   - `decode_p12(item)` 改为 `decode_pkcs12(&format!("keystore item `{}`", item.name), &decode_base64(item)?, item.password.as_deref().unwrap_or(""))`：今天的消息逐字不变。
3. `rurge-proto/src/transport/tls.rs`：
   - 签名：`pub fn origin_client_config(roots: Arc<RootCertStore>, insecure: bool, alpn: &[&[u8]]) -> Result<Arc<ClientConfig>, BuildError>`。
   - 做法：
     - ring 的 provider；TLS 1.3 与 1.2；
     - 校验器用私有的 `Verifier`：`insecure` 时 `Mode::Insecure`，否则 `Mode::Standard { inner: WebPkiServerVerifier::builder_with_provider(roots, provider).build()?, verify_name: None }`；
     - 不带客户端证书；`enable_sni = true`；`alpn_protocols` 取 `alpn`。
   - 文档注释（`phase 4 M2 design 9.2`）：
     - 解密的连接对源站用它，校验器照旧私有；
     - `insecure` 不看链与名字，握手里的签名照常校验；
     - 名字在连接时给（`TlsConnector::connect`），IP 的名字不发 SNI。
4. `rurge-proto/src/testing/tls.rs`（P26）：
   - `pub fn p12_base64(chain: &[Vec<u8>], key_pkcs8: &[u8], password: &str) -> String`：p12-keystore 的写法同今天的 `client_p12_base64`（别名 `rurge`、本地密钥编号 `[1, 2, 3, 4]`、缺省的加密）。`client_p12_base64` 改为调它。
   - `TlsFixture::ca_p12_base64(&self, password: &str) -> String`：fixture 自己的 CA 证书与它的私钥（`self.issuer.key().serialize_der()`）。这样 fixture 的 CA 就是一个 MITM CA，`roots()` 是客户端要信的根。
   - 照 `testing` 模块现有的导出写法导出。
5. `rurge-net/src/tls.rs`：`pub fn union_root_store() -> Arc<rustls::RootCertStore>`：系统的根（`add_parsable_certificates`）∪ `webpki-roots`，`OnceLock` 读一次；文档注释照设计第 9.3 节写理由。`root_store` 不动。
6. `rurge-http/src/mitm/`（新模块；`lib.rs` 加 `pub mod mitm;`，`//!` 补一句 M2 的 `mitm`）：
   - `mod.rs`：声明 `authority`（与任务 3 的 `decrypt`），`#[cfg(test)] mod fixtures;`，导出公开的名字。
   - `authority.rs`：
     - `pub struct CaProblem { pub message: String, pub span: Option<Span>, pub reported: bool }`：`message` 以 `MITM is off: ` 开头，文字见 P5。`reported` 只在 `[Keystore]` 条目的 Base64 解不开时为真：加载时已经有它的 `E0021`，`check` 不再报（P5 第 4 条）。
     - `pub struct Authority`，握着：
       - CA 证书的 DER、PKCS#12 的链（与私钥配对的那张起）、`rcgen::Issuer<'static, KeyPair>`；
       - CA 有没有 SKI；CA 的 `notBefore`、`notAfter`（`time::OffsetDateTime`）；
       - CA 证书的 subject（字符串）与证书 DER 的 SHA-256；材料的指纹（P8）；时钟（P7）。
     - `impl Authority`：
       - `pub fn load(cfg: &MitmConfig, keystore: &[KeystoreItem]) -> Result<Option<Authority>, CaProblem>`：设计第 5.1、5.2 节，次序与消息见 P5。没写 CA 时 `Ok(None)`。
       - `pub fn same_material(&self, cfg: &MitmConfig, keystore: &[KeystoreItem]) -> bool`：算出 `cfg` 所用来源的材料指纹、与自己的比（P8）。`cfg` 没写 CA，或 `ca-keystore-name` 指向的条目不在，是假。
       - `pub fn usable_now(&self) -> bool`：时钟的现在在 CA 的有效期里（P16）。
       - `pub fn ca_der(&self) -> &[u8]`。
     - `impl fmt::Debug for Authority`：只写 `subject` 与 `certificate_sha256`（前 8 个十六进制字符）。
     - `pub fn check(config: &Config) -> Diagnostics`：
       - `Authority::load(&config.mitm, &config.keystore)` 是 `Err(p)` → `p.reported` 为假时一条 `W0039`，消息 `p.message`，在 `p.span`；为真时什么也没有；
       - 是 `Ok(None)` 而 `config.mitm.effective` 不空 → 一条 `W0039`（P5 第 1 条），在 `spans.hostname`；
       - 是 `Ok(Some(_))` → 什么也没有。
     - 试签一张（设计第 5.2 节第 6 条）调签发叶证书的那个私有函数，任务 3 原样接着用（P9）。
   - `fixtures.rs`（`#[cfg(test)]`）：协调者在派工文件里给的五组 Base64（P6），写成 `pub(crate) const`（`&str`），每个常量一行文档注释说它是什么。
7. 引擎 `outbounds.rs` 的 `load_checked`：在 `dry_build` 之后 `loaded.diagnostics.extend(rurge_http::mitm::check(&loaded.config));`，文档注释补"and the MITM CA's checks (phase 4 M2 design 5.3)"。
8. `crates/rurge/src/cli/run.rs` 的 `reload`：把 `load_checked` 挪进 `tokio::task::spawn_blocking`（`LoadOptions` 是 `Clone`，路径克隆一份）；`JoinError` 当作加载失败处理，消息照那里现有的写法（P12）。

**用例**

- `rurge-proto`：
  - `keystore.rs` 现有的用例照过（消息不变）；
  - `decode_pkcs12_names_its_label`：口令错，`label` 给 ``"`ca-p12`"`` → 消息以 ``"`ca-p12` cannot be decoded"`` 开头、没有口令；
  - `origin_client_config_verifies_unless_insecure`（`transport/tls.rs` 末尾的测试模块）：
    - 用 `TlsFixture::spawn_echo` 起一个回环的 TLS 回显；
    - 信 fixture 的根、名字对 → 握手成功，ALPN 是给的那个；
    - 信另一个 fixture 的根 → 握手失败，错误里有 `UnknownIssuer`；
    - `insecure` 加另一个 fixture 的根 → 成功。
- `rurge-net`：`the_union_holds_every_webpki_root_and_is_shared`：`webpki_roots::TLS_SERVER_ROOTS` 的每一个 subject 都在结果里；两次调用 `Arc::ptr_eq`。只读系统存储，同现有的 `the_store_is_never_empty_and_is_shared`。
- `rurge-http`（`authority.rs` 末尾的测试模块；用 P7 的时钟，夹具的用例把现在钉在 2027-01-01）：
  - `good_cas_of_every_supported_key_load`：
    - 四种都是 `Ok(Some(a))`、`a.ca_der()` 等于那张 CA：rcgen 生成的 P-256、P-384、Ed25519，与以 `RSA_2048_KEY` 自签的 RSA CA；
    - P-256 那一种同样经 `[Keystore]` 条目再载入一次。
  - `a_keystore_item_wins_and_does_not_fall_back`：
    - `ca-keystore-name` 指向好的 p12 条目、`ca-p12` 坏 → `Ok`；
    - `ca-keystore-name` 指向不存在的条目、`ca-p12` 好 → `Err`，消息是 P5 第 2 条。
  - `a_keystore_item_that_is_not_pkcs12_is_a_problem`：条目的类型是 `OpensshPrivateKey` → P5 第 3 条。
  - `a_keystore_item_with_bad_base64_is_left_to_e0021`：`ca-keystore-name` 指向一个 `base64` 解不开的 p12 条目 → `Authority::load` 是 `Err`、`reported` 为真；`check` 没有诊断；从同样的文字加载 Profile 恰好一条 `E0021`。
  - `an_empty_passphrase_opens_a_p12_written_with_one`：p12 以空口令写成；`ca-passphrase` 不写 → `Ok`；写成空 → `Ok`。
  - `every_ca_problem_is_named_and_quotes_no_secret`：一张表，每行一种问题。
    - 每行经 `check` 恰好得到一条 `W0039`，消息以 `MITM is off: ` 开头、含 P5 那一条的关键短语；
    - 所有消息里都没有那一行的 Base64 文字与口令（口令用一个不会碰巧出现的词，例如 `pw-7d1f`）。
    - 行：Base64 坏、口令错、没有私钥（只有证书的 p12）、不是 CA、KeyUsage 没有 `keyCertSign`、过期、未生效、`P521`、`NO_PUBLIC`、`MULTI_RDN`、`DUP_ATTR`、私钥与证书不配对（以 A 的私钥配 B 的证书）。
  - `no_ca_warns_only_when_hosts_are_listed`：
    - 有 `hostname = a.test`、没有 CA → 一条 `W0039`（P5 第 1 条）；
    - `hostname` 空或全被 `hostname-disabled` 去掉 → 没有诊断。
  - `the_debug_of_an_authority_shows_the_certificate_not_the_material`：`{:?}` 里有 subject 与证书指纹的前 8 位；没有 Base64、口令、材料指纹的十六进制。
  - `same_material_compares_the_base64_and_the_passphrase`：
    - 同样的 Base64 与口令 → 真；换口令 → 假；
    - `[Keystore]` 条目带同样的 Base64 与口令 → 真；
    - 没写 CA → 假。
  - `an_expired_authority_is_not_usable_now`：CA 的 `notAfter` 是 T，现在在 T 之前 → 真；时钟拨到 T 之后 → 假。
- 引擎：`load_checked_reports_an_unusable_mitm_ca`（`outbounds.rs` 末尾的测试模块）：临时文件里 `[MITM]\nhostname = a.test\nca-p12 = QUJDRA\nca-passphrase = pw-7d1f` → 结果里有 `W0039`，消息里没有 `QUJDRA` 与 `pw-7d1f`。
- 命令行：`check_names_an_unusable_mitm_ca`（`crates/rurge/tests/cli.rs`）：
  - 同样的配置交给 `rurge check -c <文件>`（经 `rurge_command()`、给 `--data-dir`）；
  - 输出里有 `W0039` 与 `MITM is off`，没有 `QUJDRA` 与 `pw-7d1f`；
  - 退出码照告警时现有的约定（任务 0 读出来）。

**判据**

- 用例全绿；`Cargo.lock` 520；`git diff Cargo.lock` 为空，或只有 `rurge-http` 依赖列表里的变化；
- 门禁绿。

---

## Task 3：叶证书与缓存；`Mitm`：何时解密、服务端配置、与客户端的握手

**改哪里**（设计第 5.5、6、7.1 ～ 7.6、8.1、8.2 节；M2-D6 ～ D10；P7、P9 ～ P11）

1. `authority.rs`：
   - 常量：`pub const LEAF_SLOTS: usize = 1000;`、`pub const SESSION_SLOTS: usize = 1024;`、`pub const LEAF_DAYS: i64 = 47;`、`pub const BACKDATE_DAYS: i64 = 2;`。
   - `Authority` 加两个字段：
     - 叶证书缓存 `Mutex<LruCache<HostName, Arc<Slot>>>`，`Slot` 是 `tokio::sync::OnceCell<Arc<Leaf>>`，`Leaf { key: Arc<CertifiedKey>, not_before, not_after }`；
     - 会话缓存 `Arc<ServerSessionMemoryCache>`（`SESSION_SLOTS` 条）。
   - `pub async fn leaf(&self, name: &HostName) -> Result<Arc<CertifiedKey>, LeafError>`：
     - 锁里取槽：没有就放一个空的；有、而槽里的叶证书剩余寿命不足总长的三分之一，就换一个空槽。锁只在这时持有。
     - 锁外 `slot.get_or_try_init(...)`：签名与叶证书 P-256 密钥的生成在同一个 `tokio::task::spawn_blocking` 里（M2-D7）。
     - 失败时槽留空。`LeafError` 的 `Display` 是 `cannot issue a certificate for {name}: {why}`。
   - `pub(crate) fn sessions(&self) -> Arc<ServerSessionMemoryCache>`。
   - 签发的参数见 P9；试签（任务 2）与 `leaf` 共用同一个私有函数。
2. `decrypt.rs`（新）：

   ```rust
   pub enum Decision { Decrypt, Pass, AlpnMismatch }
   pub struct OriginTls { pub config: Arc<rustls::ClientConfig>, pub name: ServerName<'static>, pub wait: Duration }
   pub struct Accepted { pub stream: BoxedStream, pub watch: Arc<Watch>, pub origin: OriginTls }
   pub struct Watch { /* AtomicU64 read, AtomicBool closed */ }
   impl Watch { pub fn read(&self) -> u64; pub fn closed_by_client(&self) -> bool; }
   pub enum HandshakeFailure { Untrusted { alert: &'static str }, Failed { reason: String } }
   pub struct Mitm { /* Arc<Authority>, effective, sources, explicit ports, server template, origin config */ }
   impl Mitm {
       pub fn build(cfg: &MitmConfig, authority: Arc<Authority>, roots: Arc<RootCertStore>) -> Result<Mitm, BuildError>;
       pub fn authority(&self) -> &Arc<Authority>;
       pub fn decides(&self, source: IpAddr, sni: Option<&HostName>, target: &HostName, port: u16, alpn: &[Vec<u8>]) -> Decision;
       pub fn widens(&self, source: IpAddr, target: &HostName, port: u16) -> bool;
       pub async fn accept(&self, client: Prefixed<BoxedStream>, name: &HostName, wait: Duration) -> Result<Accepted, HandshakeFailure>;
   }
   /// design 7.2: what a client's address is compared as.
   pub fn source_of(listener: ListenerKind, src: SocketAddr) -> IpAddr;
   /// P11: what a failed `TlsAcceptor::accept` was.
   pub(crate) fn classify(error: &io::Error) -> HandshakeFailure;
   ```

   - `build`：
     - 名单与来源从 `cfg` 克隆；`explicit_ports` 从 `cfg.effective` 取；
     - 服务端配置的模板见 P10；
     - 对上游的配置 `origin_client_config(roots, cfg.skip_server_cert_verify, &[b"http/1.1"])`。
   - `decides`，依次（设计第 7.1、7.3、7.4 节）：
     - 来源不允许（`sources` 有、`allows` 为假）→ `Pass`；
     - 有 SNI 时 `effective.matches_sni(sni, port) != Some(true)`、没有 SNI 时 `effective.matches(target, port) != Some(true)` → `Pass`；
     - 要签的名字（SNI，没有时 `target`）是域名而 `ServerName::try_from` 不收 → `Pass`；
     - ALPN 非空且不含 `http/1.1` → `AlpnMismatch`；
     - 其余 `Decrypt`。
   - `widens`（设计第 7.6 节）：来源允许，并且 `explicit_ports` 含 `port`、或 `effective.matches(target, port) == Some(true)`。
   - `accept`，整个过程在 `wait` 之内：
     - `authority.leaf(name)`；
     - 克隆模板、换上只给出这张叶证书的 `SingleCertAndKey`；
     - `TlsAcceptor::accept(client)`；
     - 成功时包上 `Watched`（P11），装进 `BoxedStream`；`origin` 的名字由 `name` 转来（IP 用 `ServerName::IpAddress`）。
   - 失败的分类见 P11：时限到是 `Failed { reason: "timed out" }`；签不出叶证书是 `Failed { reason: <LeafError 的 Display> }`。
   - `source_of`：
     - `ListenerKind::Tun` → `127.0.0.1`；
     - 地址先 `to_canonical()`，是回环（`127.0.0.0/8`、`::1`）→ `127.0.0.1`；
     - 其余照原样。
3. `mod.rs` 导出：`Authority`、`CaProblem`、`LeafError`、`Mitm`、`Decision`、`Accepted`、`Watch`、`HandshakeFailure`、`OriginTls`、`source_of`、`check`，与常量。

**用例**（`authority.rs` 与 `decrypt.rs` 末尾的测试模块；握手用 `tokio::io::duplex` 两头，客户端是 tokio-rustls 的 `TlsConnector`）

- `a_leaf_says_what_the_design_says`：x509-parser 读回下面各项。
  - 域名 `site.test`：
    - SAN 只有一个 DNS 名；CN 是名字；
    - EKU 只有 serverAuth；KU 只有 digitalSignature；
    - AKI 等于 CA 的 SKI；没有 SKI、没有 BasicConstraints；
    - 序列号 16 字节、最高位为 0。
  - IP `127.0.0.1`：SAN 是 IP。
  - 65 个字符的域名：没有 CN。
  - 签名算法随 CA：P-384 的 CA → ecdsa-with-SHA384；RSA 的 CA → sha256WithRSAEncryption；Ed25519 → Ed25519。
- `a_leaf_lives_47_days_within_its_ca`：
  - 时钟钉在 T：`notBefore` 是 T − 2 天，`notAfter` 是 `notBefore` + 47 天；
  - CA 的 `notBefore` 是 T − 1 天 → 叶的 `notBefore` 等于它；
  - CA 的 `notAfter` 是 T + 10 天 → 叶的 `notAfter` 等于它。
- `a_ca_without_a_subject_key_identifier_gives_leaves_without_an_authority_key_identifier`：CA 照 P6 最后一段造。
- `one_name_one_leaf_until_a_third_of_its_life_is_left`：
  - 同一个名字两次是同一个 `Arc`，签名计数 1；
  - 时钟拨到剩余寿命不足三分之一 → 新的一张，计数 2。
- `the_thousand_and_first_name_pushes_out_the_oldest`：1001 个名字之后第一个名字再取 → 计数加一。
- `concurrent_requests_for_one_name_sign_once`：多线程运行时上 16 个任务同时取同一个名字 → 计数 1，16 个都是同一个 `Arc`。
- `decides_in_the_design_order`：
  - 来源：`client-source-address = 10.0.0.0/8`，来源 `127.0.0.1` → `Pass`；
  - 名单：`-bad.test, *.test`：`bad.test` → `Pass`、`ok.test` → `Decrypt`；
  - 有 SNI 时不带端口的项在 8443 也解密，没有 SNI 时不带端口的项只在 443；
  - SNI 写成 IP、名单有 `127.0.0.1` → `Decrypt`；
  - ALPN：空、`[http/1.1]`、`[h2, http/1.1]` → `Decrypt`；`[h2]` → `AlpnMismatch`。
- `widens_on_written_ports_and_on_targets_the_list_takes`：
  - `a.test:8443` 让 `x.test:8443` 扩大；
  - `b.test:0` 让 `b.test:25` 扩大，不让 `c.test:25`；
  - 不带端口的 `d.test` 不让 `d.test:8443`；
  - `-e.test:9443` 不让 9443；`*:0` 让任何目标；
  - 来源不允许时都不扩大。
- `source_of_maps_loopback_and_the_virtual_interface_to_127_0_0_1`：`127.0.0.2`、`::1`、`::ffff:127.0.0.1`、TUN 来的任何地址 → `127.0.0.1`；`192.168.1.2` 照原样。
- `a_client_is_served_http1_or_nothing`：客户端不给 ALPN → 握手成功、没有协商的协议；给 `[h2, http/1.1]` → `http/1.1`。
- `without_sni_the_leaf_names_the_target`：名字 `127.0.0.1`，客户端以 `ServerName::IpAddress` 连 → 握手成功（叶证书的 SAN 是那个 IP）。
- `a_client_that_does_not_trust_the_ca_sends_unknown_ca`：客户端的根是另一个 CA → `Untrusted { alert: "unknown_ca" }`。
- `the_three_certificate_alerts_are_untrusted_and_the_rest_failed`：直接调分类的函数，`io::Error::new(InvalidData, rustls::Error::AlertReceived(...))` 三种 → `Untrusted`，各自的名字；`HandshakeFailure` 警报、`UnexpectedEof` → `Failed`。
- `a_client_that_leaves_mid_handshake_is_failed`：客户端只写一个 ClientHello（rustls 的 `ClientConnection::write_tls` 生成）就关 → `Failed`。
- `a_stalled_handshake_times_out`：同上但不关，`wait` 300 毫秒 → `Failed { reason: "timed out" }`，在 2 秒之内返回。
- `the_watch_counts_plaintext_and_sees_the_client_close`：
  - 握手之后客户端发 `close_notify` 并关 → `read()` 0、`closed_by_client()` 真；
  - 另一条：客户端写 `GET`，服务端读到 → `read()` 3、`closed_by_client()` 假。
- `a_second_connection_resumes_its_session`：同一个 `Authority`、同一个名字、同一份客户端配置连两次 → 第二次客户端的 `handshake_kind()` 是 `Resumed`。TLS 1.3 的票据在握手之后才到，第一条连接上客户端要读到服务端关闭为止，票据才被处理。

**判据**

- 用例全绿；带等待的循环二十次（`cargo test -j 8 --workspace --lib -- mitm::`）；
- 门禁绿。

---

## Task 4：`rurge-http`：解密的连接上的请求

**改哪里**（设计第 9 节；M2-D12；P13、P14）

1. `engine.rs`：
   - `Conn` 加 `pub origin: Option<OriginTls>`。文档注释：`Some` 时这是解密的连接，请求的 URL 用 `https://`、协议是 `HTTPS`，`connect` 在拨好的流上先做对上游的 TLS（`phase 4 M2 design 9`）。全部构造处给 `None`：引擎 `serve.rs` 的 `serve_http`、`http.rs` 的监听器、测试的 `rig_head_wait`。
   - `prepare`：`session.protocol` 在 `origin` 是 `Some` 时是 `ProtocolKind::Https`；调 `form_of` 时给 scheme（P13）。
   - `form_of(req, fallback: (&HostName, u16), scheme: &str)`：三条路都用 `scheme`；兜底的那一条调 `origin_authority(host, port, default_port)`，`default_port` 是 80（`http`）或 443（`https`）。
   - `connect`（P14）：
     - 拨号与令牌的检查不变；
     - `origin` 是 `None` → 照今天；
     - 是 `Some(tls)` → `tokio::time::timeout(tls.wait, TlsConnector::from(tls.config.clone()).connect(tls.name.clone(), Metered::new(upstream.stream, meter.clone())))`，再在 TLS 流上做 hyper 的握手。
     - 握手与把连接放上跟踪器的那一段提成一个泛型的私有函数，两支共用。
     - 失败：时限到 → `DialError::Tls("timed out")`；其余 → `DialError::Tls(<错误的 Display>)`。
   - `DialError` 加 `Tls(String)`。`redial` 把它映射成 `self.failed(guard, pages, format!("upstream TLS handshake failed: {e}"), format!("Upstream TLS handshake failed: {e}"))`，**不** `mark_upstream_failed`。
   - `promote`：代码不改。在按 `TokioIo<Metered<BoxedStream>>` 取回上游的那一支前加一句注释：解密的连接对不上它，走通用的一支，升级之后的字节由 `relay` 记（`phase 4 M2 design 9.5`）。
2. `Upstreams` 不改。
3. 不以绝对形式交给 HTTP 代理（设计第 9.1 节）不要新代码：解密的连接从嗅探进来，`Conn.entry` 是 `Entry::Sniffed`，`prepare` 给的 `forwardable` 已经为假。任务 5 建 `Conn` 时照这样给；任务 8 的第 10 条用例钉住它。

**用例**（`engine.rs` 末尾的测试模块）

- 夹具：加一个 `rig_tls(origin: OriginTls, ...)`（照 `rig_head_wait`，`Conn.origin` 给 `Some`）。源站是 `TestServer::spawn_tls_with(fixture.acceptor(false))`，fixture 为 `target.test` 签；`OriginTls` 用 `origin_client_config(fixture.roots(), false, &[b"http/1.1"])`、名字 `target.test`、`wait` 5 秒。
- `a_decrypted_request_goes_to_the_origin_over_tls`：
  - `GET /a`（`Host: target.test`）→ 200，源站收到；
  - 记录的 `url` 是 `https://target.test/a`、`protocol` 是 `Https`；
  - `up` 与 `down` 都大于 0。
- `a_decrypted_url_leaves_out_port_443_only`（`#[test]`，直接调 `form_of`）：
  - `Host: target.test` → `https://target.test/`；`Host: target.test:8443` → `https://target.test:8443/`；`Host: target.test:443` → `https://target.test:443/`（照 M1b，`Host` 写了端口就留着）；
  - 没有 `Host`、目标 443 → `https://target.test/`；目标 8443 → `https://target.test:8443/`；
  - 绝对形式 `http://other.test/x` 在解密的连接上 → `https://other.test/x`（P13）。
- `an_origin_the_roots_do_not_trust_is_a_502_with_the_reason`：
  - 源站用另一个 fixture → 502、页面里有 `Upstream TLS handshake failed`；
  - 记录 `Failed`，原因以 `upstream TLS handshake failed: ` 开头、含 `UnknownIssuer`；
  - 句柄的 `upstream_failed()` 为假。
- `skip_server_cert_verify_reaches_an_untrusted_origin`：同上的源站，`OriginTls` 用 `insecure = true` 的配置 → 200。
- `the_origin_hears_the_client_sni_and_an_ip_hears_none`：fixture 的 `seen()`——名字 `target.test` → `sni` 是 `Some("target.test")`；名字是 `127.0.0.1`（fixture 为这个 IP 签）→ `None`。
- `a_slow_origin_handshake_is_a_502_after_its_wait`：用例自己的回环监听器，接受了不答；`wait` 300 毫秒 → 502，原因 `upstream TLS handshake failed: timed out`，在 3 秒之内。
- `a_websocket_upgrade_goes_through_tls_and_is_counted_once`：
  - 用例自己的 TLS 源站：fixture 的 `acceptor` 包住回环连接，答 101，之后回显；
  - 升级之后写 1000 字节、读回；
  - 记录 `status_code` 101，`up` 与 `down` 都小于 3000（没有翻倍）。
- `a_request_without_host_is_named_by_its_target`（现有）：照 P15 改。

**判据**

- 用例全绿；带等待的循环二十次（`cargo test -j 8 --workspace --lib -- engine::tests`）；
- 门禁绿。

---

## Task 5：引擎：每一代的 `Mitm`、开关与 `state.json` 第 2 版、可嗅探的扩大、解密的分派

**改哪里**（设计第 5.3、5.4、7、11 节；M2-D5、D8、D9、D18；P16 ～ P20）

1. `rurge-engine/src/mitm.rs`（新）：

   ```rust
   /// What outlives a generation (phase 4 M2 design 5.4): the last `Authority` a published generation had.
   pub struct MitmShared { /* Mutex<Option<Arc<Authority>>> */ }
   impl MitmShared {
       pub fn new() -> MitmShared;
       pub fn remember(&self, authority: &Arc<Authority>);
       pub fn last_live(&self) -> Option<Arc<Authority>>;
   }
   ```

2. `shared.rs`：`EngineShared` 加 `pub mitm: Arc<MitmShared>`，`EngineShared::new` 建一个。
3. `runtime.rs`：`Runtime` 加 `pub(crate) mitm: Option<Arc<Mitm>>`，在 `build` 的末尾照 P16 建。
4. `engine.rs`：
   - `Engine` 加 `mitm_on: AtomicBool`，初值真（总设计 P4-D19）。
   - `pub fn mitm_on(&self) -> bool`。
   - `pub async fn set_mitm(&self, on: bool)`：存进原子量；挂了 `StateStore` 时 `store.update(|s| s.features.mitm = Some(on))`，同 `set_mode`。
   - `pub fn restore_switches(&self, features: &Features)`：`features.mitm` 是 `Some(on)` 时存进原子量；不写盘。
   - `pub fn mitm_ca_der(&self) -> Option<Vec<u8>>`：当前这一代的 `Mitm` 的 `authority().ca_der()`。
   - `publish_generation`：`next.mitm` 有 `Mitm` 时 `self.shared.mitm.remember(m.authority())`（P17）。
5. `serve.rs`（P18）：
   - `fn named_target(&self, session: &SessionInfo) -> Option<HostName>`：同 `named_at_a_proxy_port`，但对所有监听器都探 fake 地址的池（只问不租）；是 rurge 的地址而没有名字时 `None`。
   - `sniffable_session` 的端口那一条加一种（设计第 7.6 节）：`mitm_on()`、这一代有 `Mitm`、`named_target` 有、`mitm.widens(source_of(...), &target, port)`。
   - `serve_sniffed` 判为 TLS 的一支：设好 `sni` 与 `HTTPS` 之后先问 `fn decrypting(&self, session, hello) -> Option<(Arc<Mitm>, HostName)>`，有就交给 `serve_mitm`、返回。
   - `async fn serve_mitm(&self, session, client: Prefixed<BoxedStream>, mitm: Arc<Mitm>, name: HostName, opts: ServeOpts)`：
     1. `turn_back`、`look_up_owner`、承载连接的句柄，同 `serve_http`；
     2. `mitm.accept(client, &name, HANDSHAKE_TIMEOUT)` 与引擎的会话根令牌、`opts.stop` 竞速——停止时直接返回：不记录、不计数、不写日志；
     3. 失败 → 承载连接一条记录，`Failed`，原因是 P21 的记录文字（不信任与别的失败两种；日志、计数与限频在任务 6）；
     4. 成功 → 与 `serve_http` 共用的那一段（见下），`Conn.origin` 给 `accepted.origin`，客户端的流是 `accepted.stream`；
     5. `routed == 0` 时的承载记录照 `serve_http`（任务 6 加证书固定的判断）。
   - `serve_http` 里从建 `Conn` 到等 `serve_connection` 结束的那一段，提成一个私有函数两处共用：`serve_http` 的行为不变，它的现有用例照过。
6. `state.rs`：第 2 版（P19）。
7. `crates/rurge/src/cli/run.rs`：`engine.attach_state(store)` 之后 `engine.restore_switches(&state.features)`。
8. `lib.rs`：照现有的导出风格导出 `mitm::MitmShared`。

**用例**

- `state.rs`（末尾的测试模块）：
  - `loads_defaults_selections_and_tolerates_garbage`：照 P15 改。
  - `a_version_1_file_leaves_the_four_switches_unset`：第 1 版的文件里写 `"mitm":false,"capture":true` → 四个都是 `None`，`version` 读进来之后是 2。
  - `only_the_switches_that_were_set_are_written`：`mitm` 设成 `Some(false)`、别的没设 → 写出的 `features` 对象恰好是 `{"system_proxy":false,"enhanced_mode":false,"mitm":false}`。
  - `a_version_2_file_is_what_phase_3_reads`：
    - 用例里抄一份阶段 3 的 `State` 与 `Features`（布尔、`#[serde(default)]`）；
    - 第 2 版写出的文字能被它读出来，`mitm` 是写进去的值。
- `crates/rurge-engine/tests/mitm.rs`（新；夹具见下）：
  - `a_listed_host_is_decrypted_through_socks5`：
    - 客户端只信 MITM 的 CA，握手成功；
    - 拿到的叶证书的 SAN 是 `origin.test`；
    - `GET /a` 拿到源站的 200；
    - 记录：`protocol` 是 `https`、`url` 是 `https://origin.test/a`、`sni` 是 `origin.test`。
  - `with_the_switch_off_the_client_sees_the_origin_certificate`：
    - `set_mitm(false)` 之后，只信 MITM 的 CA 的客户端握手失败（`UnknownIssuer`）；
    - 只信源站的 CA 的客户端成功（按原样转发）；
    - 切回 `true` 之后新的连接又解密。
  - `a_client_offering_only_h2_is_passed_through`（`LogCapture`）：
    - 客户端的 ALPN 只有 `h2` → 看到源站的证书；
    - 有一行 DEBUG 带 `origin.test` 与 `h2`。
  - `without_sni_the_target_decides_on_443_only`：
    - 客户端不发 SNI（rustls 客户端 `enable_sni = false`），经 SOCKS5 连 `origin.test:443` → 解密，叶证书的 SAN 是 `origin.test`；
    - 同样的客户端连 `origin.test:8443`（`hostname` 写 `origin.test`、`force-http-engine-hosts` 让 8443 可嗅探）→ 不解密。
  - `a_written_port_makes_its_port_sniffable`：`hostname = origin.test:<源站端口>`，经 SOCKS5 直接连源站的端口（不经 `FakeSocks5`）→ 解密。
  - `a_bare_entry_does_not_sniff_another_port`：`hostname = origin.test`，同样连源站的端口 → 看到源站的证书。
  - `a_zero_port_entry_widens_its_own_target_only`：
    - `hostname = origin.test:0`：`origin.test:<端口>` 解密；
    - `other.test:<另一个源站的端口>` 看到源站的证书。
  - `a_stop_during_the_handshake_leaves_no_record`：
    - 经虚拟网卡的 `Rig`（停法照 M1b 的 `stopping_the_adapter_ends_its_http_connections`）；
    - 客户端只写一个 ClientHello 就不动，停掉适配器；
    - 再等 300 毫秒（注释照规矩写），一条记录也没有。
  - `a_reload_with_the_same_material_keeps_the_leaves`：
    - 第一条连接拿到叶证书 L；`swap_runtime`（同样的 Profile 文字）；
    - 第二条连接拿到的叶证书与 L 的 DER 相同；
    - `shared.mitm.last_live()` 与新的这一代的 `Authority` 是同一个 `Arc`。
  - `the_switch_is_restored_from_state_json`：
    - 挂上 `StateStore`，`set_mitm(false)` → 文件里 `version` 是 2、`features.mitm` 是 `false`；
    - 新的 `StateStore` 读同一个文件，`restore_switches` 到一个新的引擎上 → `mitm_on()` 是假。
- 夹具（写在 `tests/mitm.rs` 里；`common` 里有的不重写）：
  - MITM 的 CA：`TlsFixture::new(&[])` 的 `ca_p12_base64("pw")`，客户端信它的 `roots()`；
  - 源站：`TlsFixture::new(&["origin.test"])` 与 `TestServer::spawn_tls_with`；
  - 引擎：`harness_trusting` 把源站的根交给引擎，嗅探等待 300 毫秒；
  - 规则把 `origin.test` 送到一个指向源站的 `FakeSocks5`。

**判据**

- 用例全绿；带等待的循环二十次（`cargo test -j 8 --workspace --test mitm`）；`sniff`、`http` 两个测试二进制照过；
- 门禁绿。

---

## Task 6：握手失败、证书固定的提示、限频、计数、承载连接的记录

**改哪里**（设计第 8.2 ～ 8.5、15 节；M2-D11、D19、D23；P21 ～ P23）

1. `rurge-engine/src/mitm.rs`：
   - `MitmShared` 加提示的限频（P22）：`pub(crate) fn warn_now(&self, kind: Hint, name: &str, now: Instant) -> bool`，`Hint` 有两种：`Untrusted`、`Pinning`。
   - 计数（P23）：`MitmCounters`（四个 `AtomicU64`）放在 `Engine` 里；`#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)] pub struct MitmCounts { pub decrypted: u64, pub untrusted: u64, pub failed: u64, pub no_request: u64 }`。
2. `serve.rs` 的 `serve_mitm`：
   - 握手失败：照 P21 写日志（经虚拟网卡来的会话用另一种写法）、计数；承载连接的记录在任务 5 已经写了；
   - 握手成功：`decrypted` 加一；
   - `serve_connection` 返回之后，`routed == 0` 时照 P21 判断证书固定：成立 → 提示（限频）、`no_request` 加一、记录写提示的那一句；不成立 → 照 M1b。
3. `HttpCounts` 加 `pub mitm: MitmCounts`；`Engine::http_counts` 填它。
4. `tests/sniff.rs` 的 `the_sniffs_are_counted`：照 P15 改。

**用例**（`crates/rurge-engine/tests/mitm.rs`；抓日志的用例用 `LogCapture`、当前线程的运行时）

- `an_untrusting_client_is_warned_once_an_hour_and_counted_each_time`：客户端只信另一个 CA，连两次。
  - 第一次一行 WARN，带 `origin.test` 与端口；第二次没有 WARN、有一行带 `origin.test` 的 DEBUG；
  - `untrusted` 是 2、`decrypted` 是 0；
  - 两条承载记录 `Failed`，原因是 P21 的那一句（`alert unknown_ca`）。
- `a_client_that_closes_after_the_handshake_gets_the_pinning_hint`：客户端握手完成、`shutdown()` 之后关。
  - 一行 WARN，以 `origin.test is likely protected by certificate pinning` 开头；
  - `decrypted` 1、`no_request` 1；
  - 一条承载记录，原因 `no request after the TLS handshake (likely certificate pinning)`；
  - 同样再连一次：没有新的 WARN、一行带 `origin.test` 的 DEBUG；`no_request` 2。
- `a_request_head_that_times_out_is_no_pinning_hint`：
  - `harness_timed` 300 毫秒，客户端握手完成之后什么也不发、也不关；
  - 承载记录 `Failed`，原因不是提示的那一句；`no_request` 0；没有提示的 WARN。
- `a_handshake_the_client_abandons_is_failed_at_info`：
  - 客户端只写一个 ClientHello 就关；
  - 一行 INFO，带 `origin.test`；`failed` 1；
  - 承载记录以 `TLS handshake with the client failed: ` 开头。
- `through_the_virtual_interface_the_warnings_name_no_host`：经 `Rig`，不信任的客户端与握手之后就关的客户端各一次。
  - 两行 WARN 里都没有 `origin.test`、没有端口，有 `virtual interface`；
  - 各有一行 DEBUG 带 `origin.test`；
  - 两条承载记录的 `dst` 里有 `origin.test`（记录照旧带名字）。
- `a_stop_during_the_handshake_counts_nothing`：接着任务 5 的那一条，四个数都是 0。
- `the_mitm_counts_survive_a_reload`：解密一次，`swap_runtime`，`decrypted` 仍是 1。

**判据**

- 用例全绿；带等待的循环二十次（`cargo test -j 8 --workspace --test mitm`、`--test sniff -- the_sniffs_are_counted`）；
- 门禁绿。

---

## Task 7：API

**改哪里**（设计第 12 节；M2-D19；P20、P27）

1. `routes/features.rs`：
   - `mitm` 的 `GET` 给 `{"enabled": app.engine.mitm_on()}`；
   - `POST` 调 `app.engine.set_mitm(body.enabled).await`，INFO 一行 `mitm switched via http-api`（字段 `enabled`，同系统代理那一行）；
   - `capture`、`rewrite`、`scripting` 照旧；模块的 `//!` 改成 M2a 之后的样子。
2. `routes/mitm.rs`（新）：
   - `GET /v1/mitm/ca`：`app.engine.mitm_ca_der()` 是 `Some(der)` → 200、正文 `der`、`Content-Type: application/x-x509-ca-cert`、`Content-Disposition: attachment; filename="rurge-ca.cer"`；
   - 是 `None` → `ApiError::not_found("no usable MITM CA")`；
   - 路由照 `lib.rs` 的写法挂上（鉴权同别的路由，`?x-key=` 照旧认）。
3. `routes/rurge_http.rs`：`HttpJson` 在 `requests` 之后加 `mitm: MitmJson { decrypted, untrusted, failed, no_request }`（`no_request` 序列化成 `noRequest`）；模块的 `//!` 补一句。
4. `tests/api.rs` 的 `api_built` 加一个参数（P27）。

**用例**

- `tests/api.rs`：
  - `the_mitm_switch_is_read_switched_and_kept`：
    - `GET /v1/features/mitm` 是 `{"enabled":true}`；
    - `POST {"enabled":false}` 得 `{}`；再 `GET` 是假；
    - `state.json` 里 `version` 2、`features.mitm` 是 `false`；
    - `POST` 真之后 `GET` 是真。
  - `the_mitm_ca_is_given_in_der`：
    - Profile 的 `[MITM]` 用一个 `TlsFixture` 的 `ca_p12_base64`；
    - 正文逐字节等于那张 CA 的 DER，x509-parser 把它整个读成一张证书、没有剩余的字节；
    - 两个头如上；只用 `?x-key=`、不带头的请求也拿到 200。
  - `the_mitm_ca_is_a_404_without_a_usable_ca`：没有 `[MITM]` → 404、`{"error":"no usable MITM CA"}`。
  - `features_collections_stop_and_unknown_paths`、`rurge_http_counts_start_at_zero`：照 P15 改。
- `crates/rurge/tests/cli.rs`：`a_daemon_restores_the_mitm_switch_from_state_json`。
  - 数据目录里先写一个 `state.json`：`{"version":2,"features":{"mitm":false}}`；
  - 经 `rurge_process()` 起守护进程（`--data-dir` 指它），等它说准备好；
  - `GET /v1/features/mitm` 是 `{"enabled":false}`。

**判据**

- 用例全绿；`/v1/rurge/http` 的原文字节恰好是 P23 的那一串；
- 门禁绿。

---

## Task 8：端到端

**改哪里**：只改 `crates/rurge-engine/tests/mitm.rs`（设计第 17 节第 3 条）。产品代码不改：照计划写了之后红、而原因在产品代码的，停下写进报告。

**用例**（夹具同任务 5）

1. `every_entry_decrypts_a_listed_host`：
   - SOCKS5、HTTP 监听器的 `CONNECT`、虚拟网卡（`Rig`、`guarded`）各一条连接，每条上两个请求；
   - 各两条记录：`listener` 是那个入口，`protocol` 是 `https`，`method`、`url`、`status_code` 都在。
2. `two_requests_of_one_decrypted_connection_follow_their_own_rules`：
   - 规则 `URL-REGEX,^https://origin\.test/blocked,REJECT` 在前，`show-error-page-for-reject = true`；
   - 同一条解密的连接上 `GET /blocked` 拿到 TLS 里的 403 页，`GET /ok` 拿到 200；
   - 两条记录的 `rule` 各是那一条。
3. `an_untrusted_origin_is_a_502_and_skip_server_cert_verify_lets_it_through`：
   - 引擎不信源站的 CA（`harness`，不用 `harness_trusting`；源站的根不在系统的存储里）→ 502，记录的原因含 `UnknownIssuer`；
   - Profile 加 `skip-server-cert-verify = true` → 200。
4. `exclusions_disabled_entries_and_sources_pass_through`：下面三种各看到源站的证书。
   - `hostname = -origin.test, *.test`；
   - `hostname = origin.test`、`hostname-disabled = origin.test`；
   - `client-source-address = 10.0.0.0/8`（本机的会话按 `127.0.0.1` 比）。
5. `the_local_machine_is_compared_as_127_0_0_1`：
   - `client-source-address = 127.0.0.1` → 经 SOCKS5 与经虚拟网卡都解密；
   - `client-source-address = ::1` → 都不解密。
6. `a_reload_with_another_ca_issues_from_it`：重载换成另一个 fixture 的 CA → 新的连接只信新 CA 就握手成功，只信旧 CA 就失败。
7. `a_reload_that_breaks_the_ca_leaves_open_connections_decrypted`：
   - 连接 A 解密、第一个请求 200；
   - 重载成 `ca-p12` 坏的 Profile；
   - A 上第二个请求照样 200；
   - 新的连接 B 看到源站的证书。
8. `a_websocket_echoes_through_a_decrypted_connection`：源站答 101 之后回显（用例自己的 TLS 源站，同任务 4），升级之后写、读回。
9. `plain_http_is_untouched_by_mitm`：`[MITM] hostname = *` 时，到 80 的明文请求照 M1b 进引擎、记录 `http`。
10. `a_decrypted_request_tunnels_through_an_http_proxy`（设计第 9.1 节）：
    - 规则把 `origin.test` 送到一个 HTTP 代理策略：`rurge_proto::testing` 的 `FakeHttpProxy`，`HttpProxyScript.connect_to` 指向源站；
    - 解密的 `GET /a` 拿到源站的 200，不是代理自己答的 `forwarded`；
    - 代理记下的请求行以 `CONNECT origin.test:443` 开头。

**判据**

- 用例全绿；循环二十次（`cargo test -j 8 --workspace --test mitm`）；
- 先看到红：本任务不改产品代码，红靠临时改期望看（照 M1b 任务 7 的做法），报告里写三四处；
- 门禁绿。

---

## Task 9：文档收尾（M2a 的部分）

**改哪里**（设计第 16、18 节）。照 M1b 任务 8 的写法与规矩：引的提交、用例名与数字对着仓库核过；历史的段落不改写；本地提交、CI 没有见过。

- `docs/api/phase4.md` 加「M2a　MITM（HTTP/1.1）」一节：
  - `/v1/features/mitm`（缺省开、写进 `state.json`）、`GET /v1/mitm/ca`（头、404）、`/v1/rurge/http` 的 `mitm` 四个数（各自何时加一、被停止打断的不计）；
  - 解密的请求的记录（`https`、`https://` 的 URL、`sni`、承载连接的三种记录与原因）；
  - 握手失败与提示的日志行：哪一级、哪几种写法、限频、虚拟网卡上的写法；
  - `state.json` 第 2 版。
- `docs/testing.md`：
  - 「命令」加 `mitm::` 的单元用例与 `--test mitm` 的过滤词；
  - 「测试约定（全文）」加阶段 4 / M2a 一节：
    - 不碰信任库；
    - 测试的 CA 用 `TlsFixture::ca_p12_base64` 与 `p12_base64`；
    - 源站经 `harness_trusting`；
    - OpenSSL 的五组夹具由协调者生成（`crates/rurge-http/src/mitm/fixtures.rs`），以后要新的照 P6 的命令生成；
    - 时钟注入；
    - 夹具的用例把现在钉在 2027-01-01。
- `docs/surge-compatibility-matrix.md`：
  - L54（`[MITM]`）、L565（HTTPS 默认不透明）、7.2 节各行、L871（功能开关）、L901（`GET /v1/mitm/ca`）；
  - 设计第 16 节 M2a 的那几条登记（第 1 ～ 9、11、14、15 条；第 14 条照 P1 改写）写进对应行的备注；
  - 状态标记照 M1a 任务 9 的规矩（有登记的差异才改）；
  - 附录的统计用脚本整张重数；文件开头"最近一次"那一句。
- `docs/status.md`：
  - 阶段 4 下加「### M2a（MITM：HTTP/1.1 上的解密）」一节，照 M1b 一节的四段；
  - 依赖的附录接一句：阶段 4 / M2a 给 `rurge-http` 加的外部依赖（rcgen 的 `x509-parser` 特性、x509-parser、time、lru、rustls、tokio-rustls、sha2、ring），工作区依赖表加的两行。
- `CLAUDE.md`：只改「现在在哪里」阶段 4 那一条，写 M2a 做完（本地提交、CI 没有见过）、下一步 M2b 的计划。
- 设计第 23 节：在"写 M2a 计划时"那几条之后，加任务 0 ～ 9 的修正记录里改了设计正文意思的行，一条一段（写清改了正文哪一处、以哪一行为准）；没有就写一句"M2a 执行期没有改设计正文的意思"。
- `docs/README.md`：本计划那一项末尾接一句做完。
- `README.md`、`README_en.md`、`docs/requirements.md` 不改（M2c 收尾时改，设计第 18 节）。

**判据**

- 文档里引的提交、用例名与数字对着仓库核过；
- 门禁绿。

---

## 计划期决定

写计划时对着 `25b2bbc` 核对过下面引的名字与行号。行号会随前面任务的改动漂移，以名字为准；名字不同就照实改、记进修正记录，不改语义。标了"订正设计"的，已随本计划写进设计第 23 节。

| # | 事项 | 决定 |
| - | ---- | ---- |
| P1 | 钥匙签不了时的消息 | rcgen 0.14.7 的 `KeyPair::try_from(&PrivateKeyDer)`（`src/key_pair.rs`）依次试 Ed25519、P-256、P-384、RSA，全不成时只返回 `Error::CouldNotParseKeyPair`，ring 的原因丢了。所以设计第 5.2 节第 5 条"消息带 ring 给的原因"做不到：消息改为列出 rurge 签得了的几种（P5 第 10 条），其中写明"EC 的 PKCS#8 要带公钥"。订正设计 |
| P2 | `[MITM]` 的读法 | 照 `parse_general`，取第一个 `[MITM]` 节、`active_entries()`、`split_definition`、键名转小写。补充的几处如下。<br>① 没有 `=` 的行一条 `E0017`（`expected \`key = value\``），不引行里的任何东西（同 `[General]`：可能是口令）。<br>② 未知键 `W0001`，消息 ``unknown [MITM] key `{key}` ignored``，键名只在 `is_key_name` 时写出。<br>③ 两张 Host List 用 `parse_host_list(diags, span, key, value, Some(443))`；`client-source-address` 的非法项同样是 `W0005`，消息 ``"`client-source-address`: invalid entry `{bad}` ignored"``。<br>④ 有 MAC 项时整个键一条 `W0029`，消息见任务 1 的用例。<br>⑤ 值去掉空白之后为空：`client-source-address` 按没写（`None`），`ca-passphrase` 是 `Some("")`，`ca-p12` 与 `ca-keystore-name` 按没写（`None`）。<br>⑥ `effective` 在循环之后算，键的先后不影响它。<br>⑦ 各键所在的行记进 `spans`。<br>⑧ 模块（`.sgmodule`）里的 `%APPEND%` 不在 M2a：主 Profile 里这样写，整项读不懂，是 `W0005` |
| P3 | 名单的三个方法 | `without` 的"文字相同"：两项的 `negate` 相同，`raw` 去掉开头的 `-` 再去掉首尾空白之后按 ASCII 不分大小写相同（设计第 4.2 节；端口是文字的一部分，`example.com` 与 `example.com:443` 不同）。`matches_sni` 与 `matches` 共用一个私有函数，差别只在 `PortSpec::Default` 怎样算。`explicit_ports` 只取不带 `-` 的 `PortSpec::Port` |
| P4 | PKCS#12 的解码 | 设计第 3 节写 `decode_pkcs12(label, base64, password)`，改成收 DER：`decode_pkcs12(label, der, password)`，另加 `base64_bytes(text)`。Base64 那一步由各来源自己做，这样 `[Keystore]` 条目今天的两种消息逐字不变，`ca-p12` 的 Base64 错是 P5 第 4 条。订正设计 |
| P5 | `W0039` 的消息 | 都以 `MITM is off: ` 开头。`{src}` 是 ``the CA in `ca-p12` ``，或 ``the CA in keystore item `{name}` ``；"证书"指 PKCS#12 里与私钥配对的那一张。检查依次如下，用例里认的关键短语加了引号。<br>1 没有 CA 而生效的名单不空：``…`hostname` lists hosts to decrypt but `[MITM]` sets no CA (`ca-p12` or `ca-keystore-name`)``，在 `hostname` 那一行（"lists hosts to decrypt"）。<br>2 ``…`ca-keystore-name` names `{name}`, which [Keystore] does not have``（"which [Keystore] does not have"）。<br>3 ``…`ca-keystore-name` names `{name}`, which is not a PKCS#12 item``（"not a PKCS#12 item"）。<br>4 ``…`ca-p12` is not valid Base64``（"not valid Base64"）。`[Keystore]` 条目的 Base64 解不开时消息照 `decode_base64` 的原话、`reported` 为真，`check` 不报：加载时已经有它的 `E0021`（设计第 5.2 节第 1 条）。<br>5 `decode_pkcs12` 的三种原话（"cannot be decoded" / "holds no private key" / "holds no certificate"）。<br>6 x509-parser 读不了那张证书：`…the certificate of {src} cannot be read: {e}`。<br>7 不是 CA：`…the certificate of {src} is not a CA (no basic constraints with CA:TRUE)`（"is not a CA"）。<br>8 写了 KeyUsage 而没有 `keyCertSign`：`…the certificate of {src} may not sign certificates (its key usage lacks keyCertSign)`（"lacks keyCertSign"）。<br>9 过期：`…{src} expired on {YYYY-MM-DD}`（"expired on"）。<br>10 未生效：`…{src} is not valid until {YYYY-MM-DD}`（"not valid until"）。<br>11 钥匙：`…rurge cannot sign with the private key of {src}: it signs with ECDSA P-256 or P-384 (a PKCS#8 key that carries its public key), RSA of 2048 to 8192 bits, or Ed25519`（"cannot sign with the private key"）。<br>12 签发者：`…rurge cannot issue certificates under {src} ({e}); a subject with several attributes in one RDN is one cause`（"cannot issue certificates under"）。<br>13 试签不过：`…a certificate rurge issued under {src} does not verify: {e}`（"does not verify"）。<br>第 2、3 条在 `ca-keystore-name` 那一行；其余在所用来源的那一行。日期用 `time::Date` 的 `Display`。第 12 条接住 rcgen 的 `from_ca_cert_der`：多值 RDN 在那里就报错（`DistinguishedName::from_name`）。第 13 条接住 webpki：重复的属性类型被 rcgen 合并、签发者名字与 subject 不同，与证书私钥不配对、签名算法不认一样在这里查出。这样设计第 5.2 节第 6 条那几种坏法分在两处报，订正设计 |
| P6 | OpenSSL 的夹具 | 下面五组由协调者派任务 2 时用本机的 OpenSSL 3.2.4（`/mingw64/bin/openssl`）生成；Git Bash 里要 `MSYS_NO_PATHCONV=1`，否则 `-subj` 被当成路径改写。都是 DER 的 Base64，不带 PEM 的头尾。<br>CA 都用 `req -x509 -config <只有 [req] 与 [ext] 的文件> -extensions ext -newkey ec -pkeyopt ec_paramgen_curve:<曲线> -nodes -days 3650` 生成，`[ext]` 是 `basicConstraints=critical,CA:TRUE`、`keyUsage=critical,keyCertSign,cRLSign`、`subjectKeyIdentifier=hash`；私钥用 `pkey -outform DER` 转成 PKCS#8。五组：<br>① `RSA_2048_KEY`：`genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -outform DER`；用例以它用 rcgen 自签 RSA 的 CA。<br>② `P521_CA`、`P521_KEY`：曲线 P-521，subject `/CN=rurge test P-521 CA`。<br>③ `NO_PUBLIC_CA`、`NO_PUBLIC_KEY`：曲线 P-256；私钥经 `ec -no_public` 再 `pkcs8 -topk8 -nocrypt -outform DER`，内层 `ECPrivateKey` 里没有 `[1]`。<br>④ `MULTI_RDN_CA`、`MULTI_RDN_KEY`：曲线 P-256，`-subj "/O=rurge+CN=multi"`，一个 RDN 里两个属性。<br>⑤ `DUP_ATTR_CA`、`DUP_ATTR_KEY`：曲线 P-256，`-subj "/OU=a/OU=b/CN=dup"`。<br>CA 的有效期从生成的那一刻起十年，用这些夹具的用例把时钟钉在 2027-01-01（P7）。rcgen 造得出的不用夹具：P-256、P-384、Ed25519 的 CA；不是 CA、没有 `keyCertSign`、过期与未生效（设 `not_before` / `not_after`）的证书；不配对的一对。**不带 SKI 的 CA** 也用 rcgen：rcgen 给 `IsCa::Ca` 一定写 SKI，所以写 `IsCa::NoCa`，`custom_extensions` 放一个关键的 BasicConstraints（OID 2.5.29.19，内容 `30 03 01 01 ff`），`key_usages` 写 `KeyCertSign`。设计第 17 节第 2 条说多值 RDN 用 `yasna` 手写，订正设计 |
| P7 | 时钟与签名计数 | `Authority` 握着一个时钟 `Arc<dyn Fn() -> OffsetDateTime + Send + Sync>`，缺省是 `OffsetDateTime::now_utc`。有效期的检查、试签与 webpki 的"现在"、叶证书的有效期、剩余寿命都读它。只供测试：<br>`#[cfg(test)] pub(crate) fn load_at(cfg, keystore, clock)`；<br>`#[cfg(test)] pub(crate) fn set_clock(&self, clock)`（拨时钟）；<br>`#[cfg(test)]` 的签名计数（`AtomicUsize`，`leaf` 真签名时加一，试签不算）。生产代码里没有能改时钟的入口 |
| P8 | 材料的指纹 | SHA-256（所用来源的 Base64 文字的字节 ‖ `0x00` ‖ 口令的字节）。`ca-p12` 的来源是 `ca_p12` 与 `ca_passphrase`（没有时为空），`[Keystore]` 的来源是条目的 `base64` 与 `password`（没有时为空）。只用来比较，不进 `Debug`、日志与 API（设计第 4.4、5.4 节） |
| P9 | 叶证书的参数 | `CertificateParams::default()` 带一个 CN `rcgen self signed cert`，先换成空的 `DistinguishedName`。各项：<br>名字不超过 64 个字符时写 CN；<br>SAN：域名是一个 `SanType::DnsName`，IP 是一个 `SanType::IpAddress`；<br>`is_ca = IsCa::NoCa`；`key_usages = [DigitalSignature]`；`extended_key_usages = [ServerAuth]`；<br>`use_authority_key_identifier_extension` 只在 CA 有 SKI 时为真（rcgen 的 `from_ca_cert_der` 照抄 CA 的 SKI，M2-D6）；<br>序列号：ring 的 `SystemRandom` 取 16 字节，`bytes[0] &= 0x7f`；<br>有效期见设计第 6 节的表，时钟见 P7；<br>叶证书的密钥是 `KeyPair::generate()`（P-256），签名用 `params.signed_by(&leaf_key, &issuer)`，算法随 CA 的密钥。<br>`CertifiedKey` 的链是叶 + PKCS#12 的整条链（设计第 5.5 节），私钥经 `rustls::crypto::ring::sign::any_ecdsa_type` 载入 |
| P10 | 服务端配置 | 模板每一代一份（`Mitm::build`）：<br>`ServerConfig::builder_with_provider(Arc::new(ring::default_provider())).with_protocol_versions(&[&TLS13, &TLS12])?.with_no_client_auth().with_cert_resolver(<总是 None 的解析器>)`；<br>`alpn_protocols = [b"http/1.1"]`；<br>`session_storage = authority.sessions()`；<br>`max_early_data_size` 留 0（不收 0-RTT）。<br>每条连接 `clone()` 模板，把 `cert_resolver` 换成 `Arc::new(SingleCertAndKey::from(leaf))`；克隆共享会话缓存的 `Arc`（设计第 8.1 节） |
| P11 | 握手的结局与 `Watch` | 分类：`io::Error` 里包着 `rustls::Error::AlertReceived(d)`、`d` 是 `UnknownCA`、`BadCertificate`、`CertificateUnknown` 之一 → `Untrusted { alert }`，`alert` 是 `"unknown_ca"`、`"bad_certificate"`、`"certificate_unknown"`；其余 → `Failed { reason }`，`reason` 是错误的 `Display`（包着 `rustls::Error` 时取它的）。<br>`Watched<S>` 包住服务端的 TLS 流，读写照转，并记两件事：<br>一、`poll_read` 给出 n > 0 个新字节时 `read += n`；<br>二、给出 `Ok` 而一个新字节也没有（缓冲还有空位），或给出 `UnexpectedEof`、`ConnectionReset`、`ConnectionAborted`、`BrokenPipe` 的错误时，`closed = true`。<br>tokio-rustls 收到 `close_notify` 之后读到 `Ok(0)`；客户端不发 `close_notify` 就断开时给 `UnexpectedEof`。这两件是设计第 8.3 节第 2、3 条要的"读到的明文字节数"与"客户端关闭" |
| P12 | `load_checked` 在哪个线程上 | 设计时核过：<br>`profile/boot.rs` 经 `spawn_blocking`；<br>`profile/verdict.rs` 在阻塞线程池上；<br>`cli/run.rs` 的 `run()` 是同步的；<br>`cli/check.rs` 是同步的；<br>**`cli/run.rs` 的 `reload` 是异步函数、直接调**——任务 2 挪进 `spawn_blocking`（`LoadOptions` 是 `Clone`）。<br>`crates/rurge-engine/src/subscriptions.rs:305` 的调用处没有核过，任务 0 看：在异步的工作线程上就一并挪，记进修正记录。测试里的调用不管 |
| P13 | 解密的连接上的 URL | 每个请求的 URL 都是 `https://`，包括客户端在解密的连接里发来的绝对形式请求：请求行里的 scheme 不看，请求照 M1b 去连接的目标。`Host` 写了端口的照写（M1b 的规则）；只有兜底的连接目标在 443 时省略端口。设计第 9.1 节只说了源站形式。订正设计 |
| P14 | 对上游的 TLS | 在 `connect` 里、计量之外做（`Metered` → TLS → hyper），所以计量记的是线上的字节（设计第 9.2 节）。明文的一支类型不变（`TokioIo<Metered<BoxedStream>>`），`promote` 的取回照旧对得上；TLS 的一支类型是 `TokioIo<tokio_rustls::client::TlsStream<Metered<BoxedStream>>>`，升级时走通用的一支。上游的 TLS 失败不 `mark_upstream_failed`：那不是代理的错 |
| P15 | 现有的用例 | 见下面的表。表里"预计照过"的若红，停下报告，不改期望 |
| P16 | 每一代建 `Mitm` | 在 `Runtime::build` 的末尾。`config.mitm` 没写 CA（`ca_keystore_name` 与 `ca_p12` 都是 `None`）→ `None`。否则：<br>一、`opts.shared.mitm.last_live()` 有，且 `same_material` 与 `usable_now` 都真 → 沿用那个 `Arc<Authority>`。设计第 5.4 节只说"材料相同就沿用"，这里另加"且现在仍在有效期里"：过期的不沿用，重新载入就得到"过期"的问题。订正设计。<br>二、不沿用 → `spawn_blocking` 里 `Authority::load`：`Ok(Some)` 用它；`Ok(None)` 或 `Err` → `None`，`Err` 时 DEBUG 一行（`W0039` 在加载时已经报过，不再告警）。<br>三、有了 `Authority` → 根是 `opts.shared.roots`，没有时在 `spawn_blocking` 里取 `union_root_store()`（第一次要读系统的存储）。然后 `Mitm::build`：成功 `Some(Arc::new(..))`；失败 WARN 一行 `MITM is off: cannot set up TLS: {e}`、`None` |
| P17 | 记住生效过的 `Authority` | 在 `publish_generation` 里记：它在 `Engine::new` 与 `swap_runtime` 里都被调，正好是"换上时"（设计第 5.4 节：不在建时记）。没有 `Mitm` 的一代不动记着的那个 |
| P18 | 引擎里的判断 | `decrypting(session, hello)` 依次：<br>`mitm_on()` 假 → `None`；<br>`named_target(session)` 没有 → `None`（反查不到名字的 fake 地址照 M1 按原样转发，拨号时失败）；<br>SNI 转成 `HostName`（`parse` 再 `canonical`）；<br>这一代没有 `Mitm` → 名单（`config.mitm.effective`，有 SNI 时 `matches_sni`，否则 `matches`）命中时 DEBUG 一行 `{name} is listed for MITM but this generation has no usable CA; passing it through`，返回 `None`；<br>有 `Mitm` → `decides(source_of(session.listener, session.src), sni, &target, session.dst_port, &hello.alpn)`：`Decrypt` → `Some((mitm, sni 或 target))`；`AlpnMismatch` → DEBUG 一行 `{name} is listed for MITM but the client offers no protocol rurge decrypts ({alpn}); passing it through`（`alpn` 用逗号连起、按 UTF-8 宽松转），返回 `None`；`Pass` → `None`。<br>条件取会话判为 TLS 那一刻的 `runtime()` 与开关。`sniffable_session` 的扩大用同一个 `named_target` 与 `source_of` |
| P19 | `state.json` 第 2 版 | `STATE_VERSION = 2`。`Features` 的 `mitm`、`capture`、`rewrite`、`scripting` 改成 `Option<bool>`，各带 `#[serde(skip_serializing_if = "Option::is_none")]`；`system_proxy`、`enhanced_mode` 照旧是布尔。`read_state` 读出之后若 `version < 2`，把这四个设成 `None`、`version` 改成 2（内存里；下一次写盘时写成第 2 版）。没有 `version` 的文件经 `#[serde(default)]` 取 `State::default()` 的版本，即第 2 版：只有手写的文件会这样，它写了什么就是什么。阶段 3 的代码读第 2 版的文件：`Features` 带 `#[serde(default)]`，缺的键是 `false`、在的都是布尔，读得懂（设计第 11.2 节） |
| P20 | 开关的接口 | `Engine::mitm_on`、`set_mitm`（异步，写盘）、`restore_switches`（同步，只设原子量）、`mitm_ca_der`。设计第 3 节写 `switch` / `set_switch`：M2a 只有一个能切的开关，按名字写，`rewrite`、`capture` 在 M3、M4 各加自己的 |
| P21 | 日志与记录的文字 | `{name}` 是 SNI 或连接的目标，日志行另带字段 `port`（只在经监听器来的会话上）。<br>**不信任**（`untrusted`）：<br>经监听器 WARN `the client does not trust the certificate rurge issued for {name}: trust rurge's MITM CA on the client, or exclude the host with -{name} in [MITM] hostname`（字段 `alert`）；<br>经虚拟网卡 WARN `the client does not trust the certificate rurge issued for a host reached through the virtual interface (the name is in the DEBUG log and the request record): trust rurge's MITM CA on the client, or exclude the host in [MITM] hostname`，另 DEBUG 一行带名字；<br>记录 `the client does not trust the certificate rurge issued (alert {alert})`。<br>**证书固定**（`noRequest`）：<br>经监听器 WARN `{name} is likely protected by certificate pinning: the client finished the TLS handshake and closed without a request; exclude it with -{name} in [MITM] hostname`；<br>经虚拟网卡 WARN `a host reached through the virtual interface is likely protected by certificate pinning (the name is in the DEBUG log and the request record): the client finished the TLS handshake and closed without a request`，另 DEBUG 一行带名字；<br>记录 `no request after the TLS handshake (likely certificate pinning)`。<br>**别的失败**（`failed`）：<br>经监听器 INFO `the TLS handshake with the client failed for {name}: {reason}`；<br>经虚拟网卡 INFO `the TLS handshake with the client failed for a host reached through the virtual interface: {reason}`，另 DEBUG 一行带名字；<br>记录 `TLS handshake with the client failed: {reason}`。<br>两种 WARN 被限频挡下时，同样的话（经监听器的写法，带名字）写在 DEBUG。<br>证书固定的条件（设计第 8.3 节）：握手完成、`serve_connection` 返回、`routed == 0`、`watch.read() == 0`、`watch.closed_by_client()`、没有被停止（`opts.stop` 与连接的令牌都没有取消）。都成立才算；不成立时承载记录照 M1b |
| P22 | 限频 | `MitmShared` 里 `Mutex<HashMap<(Hint, String), Instant>>`：<br>同一个键上一次写 WARN 不到 3600 秒 → 这次写 DEBUG；否则写 WARN、记下现在；<br>记下之前表里已有 1024 个键就整个清空；<br>用 `std::time::Instant`。计数与记录不限 |
| P23 | 计数 | `decrypted`：握手完成时；`untrusted`、`failed`：握手失败时，按 P11 的分类；`no_request`：P21 的条件成立时。停止打断的握手不计。自进程启动起累计，重载不清零（计数器在 `Engine` 里，不在 `Runtime` 里）。<br>`/v1/rurge/http` 的原文：`{"sniffed":{"tls":0,"http":0,"other":0,"timeout":0,"closed":0},"requests":0,"mitm":{"decrypted":0,"untrusted":0,"failed":0,"noRequest":0}}` |
| P24 | 任务的拆法 | 设计第 2 节的任务表把引擎的接入写成一个任务 5。这里拆成两个：<br>任务 5：每一代的 `Mitm`、开关与 `state.json`、可嗅探的扩大、解密的分派；<br>任务 6：握手失败、提示、限频、计数、承载连接的记录。<br>之后的顺延：API 是 7，端到端是 8，文档是 9。订正设计 |
| P25 | 语料库快照 | 手改 `.snap` 的两处，不用 `cargo insta`：本机不一定装了，评审也只要看这两处 |
| P26 | 测试用的 p12 | `p12_base64` 与 `TlsFixture::ca_p12_base64` 放在 `rurge-proto` 的 `testing` 里：p12-keystore 的写法已经在那里（`client_p12_base64`），引擎的用例经 `rurge-proto` 的 `testing` 特性本来就用 `TlsFixture`。`rurge-http` 的单元用例经开发依赖用它们 |
| P27 | API 用例的 Profile | `crates/rurge-api/tests/api.rs` 的 `api_built` 加一个参数 `extra: &str`，接在 Profile 的末尾（`[Rule]` 之后，整节的文字，如 `[MITM]\n…`）；现有的调用处给 `""`，行为不变 |

### P15 的表：要改的、要留意的现有用例

| 用例 | 为什么 | 任务 | 预计 |
| ---- | ------ | ---- | ---- |
| `crates/rurge-config/src/deferred.rs` 里断言 `is_deferred("MITM")` 的那条（今天第 67 行一带） | MITM 不再是"不生效的节" | 1 | 改：断言 `!is_deferred("MITM")`，别的节照旧 |
| 同文件 `collect_keeps_only_active_entries_and_get_is_case_insensitive`（83） | 拿 `[MITM]` 当不生效的节的例子 | 1 | 改：换成 `[Testing]`，两行键照样写，讲的不变 |
| `crates/rurge-config/src/config.rs` 第 2156 行一带（`[Nowhere]\nx = 1\n[MITM]\nx = 1`） | `[MITM]` 里的 `x = 1` 起 `W0001`，不再进 `W0016` | 1 | 改：期望照新的读法（任务 0 读出它断言什么，写进修正记录） |
| 同文件第 2603 行一带（`[MITM]\nhostname = *` 在一串"不生效"的节里） | 同上 | 1 | 改：不生效的节的名单与 `W0016` 的消息去掉 `[MITM]` |
| 同文件第 6248 ～ 6258 行一带（`W0016` 的消息 `[MITM]`） | 同上 | 1 | 改：把 `[MITM]` 换成一个仍不生效的节（例如 `[Script]`），讲的不变 |
| `crates/rurge-config/tests/snapshots/corpus__corpus__kitchen-sink.snap` | 同上 | 1 | 改（P25） |
| `crates/rurge-config/tests/secrets.rs` 的 `the_debug_of_a_loaded_profile_holds_no_secret` 与 `#!MACOS-ONLY` 的那两行（539、540） | `[MITM]` 的值进了 `Config` | 1 | 照过；加断言 |
| `crates/rurge-proto/src/keystore.rs` 的现有用例 | `decode_p12` 拆开 | 2 | 照过（消息不变） |
| `crates/rurge-proto/src/testing/tls.rs:375` 一带（`decode_p12` 读回自己写的 p12） | `client_p12_base64` 改为调 `p12_base64` | 2 | 照过 |
| `crates/rurge-engine/src/outbounds.rs` 的 `load_checked` 两条（848、858） | `load_checked` 加了检查 | 2 | 照过 |
| `crates/rurge-http/src/engine.rs` 的 `a_request_without_host_is_named_by_its_target`（3348） | `form_of` 多一个参数 | 4 | 改：原断言照旧、给 `"http"`；`https` 的几句写进新用例 |
| 同文件测试模块的 `rig_head_wait`（`Conn` 字面量） | `Conn` 多一个字段 | 4 | 改：`origin: None` |
| `crates/rurge-engine/src/state.rs` 的 `loads_defaults_selections_and_tolerates_garbage`（178） | 第 1 版文件里的 `"mitm":true` 读作没设过 | 5 | 改：断言 `features.mitm` 与 `features.capture` 是 `None`；往返那句照过 |
| 同文件 `store_opens_updates_atomically_and_renames_garbage` 与其余 | `version` 变成 2 | 5 | 任务 0 逐条读；预计照过，断言了版本号的改成 2 |
| 全部 `EngineShared {` 字面量里不带 `..` 的 | 新字段 | 5 | 任务 0 扫出；预计没有 |
| `crates/rurge-engine/tests/sniff.rs` 的 `the_sniffs_are_counted`（958 的字面量） | `HttpCounts` 多 `mitm` | 6 | 改：加 `mitm: MitmCounts::default()` |
| `crates/rurge-api/tests/api.rs` 的 `features_collections_stop_and_unknown_paths`（540） | `mitm` 缺省开、能切换 | 7 | 改：读作关、`POST` 答 501 的那两段换成 `capture`；`mitm` 的断言在新用例里 |
| 同文件 `rurge_http_counts_start_at_zero`（1113） | 多 `mitm` | 7 | 改：两处期望加 `mitm` 对象，原文字节照 P23 |
| `crates/rurge/tests/cli.rs` 里读或写 `state.json` 的用例 | 第 2 版 | 5 | 任务 0 扫出；预计照过（它们读的是 `outbound_mode`、选择与备份） |
| `crates/rurge-engine/tests/{sniff.rs,http.rs}` 全部 | 判为 TLS 的分支先问 MITM | 5 | 照过：它们的 Profile 没有 `[MITM]`，这一代没有 `Mitm` |

---

## 执行期修正记录

下面写"执行者的裁定"的，是 Pi 在设计与计划的原则下定的；写"协调者的裁定"的，是协调的会话代项目所有者定的。项目所有者都可以改。计划的正文不改：正文里说得不对的地方，以这张表的行为准。

| # | 任务 | 内容 | 提交 |
| - | ---- | ---- | ---- |
| 1 | 0 | 第 1 项与第 2 项合成一行：`git status --short` 空；分支 `phase4-m2`，`HEAD` = `b1a562c`；`git log --oneline 10d5925..HEAD` 两行——`25b2bbc`（M2 细化设计、`docs/README.md` 的一项、`CLAUDE.md`「现在在哪里」）与 `b1a562c`（本计划、设计第 23 节、`docs/README.md` 的一项、`CLAUDE.md`「现在在哪里」），`10d5925` 在历史里，其后没有别的提交。读过：设计第 3 ～ 9、11、12、15 ～ 17、19 ～ 23 节（第 20、23 节先读）、总设计第 7 节、本计划「计划期决定」P1 ～ P27 与 P15 的表、`docs/status.md` 阶段 4 / M1b 一节。无出入 | 本提交 |
| 2 | 0 | 第 3 项（`rurge-config`）：名字都在，语义与 V1 ～ V3 写的一样，签名抄给后面的任务用。`general.rs`：`invalid(diags, span, key, value)`（`:501`，`W0012`，消息带值）、`bool_or(diags, span, key, value, current: bool) -> bool`（`:559`）、`parse_host_list(diags, span, key, value, default_port: Option<u16>) -> HostList`（`:748`，逐项 `W0005`）、`is_key_name(key: &str) -> bool`（`:851`，ASCII 字母数字与 `-`/`_`、至多 64）、`pub fn parse_general(section: Option<&Section>, diags: &mut Diagnostics) -> General`（`:859`），它的循环是 `:865` ～ `:1174`（`for e in section.active_entries()` `:865`、`split_definition` `:868`、没有 `=` 时 `E0017` `:869` ～ `:876`、键名小写 `:877`、`match key` 到 `:1170`、未知键那一支 `:1148` ～ `:1170`，消息是 `unknown [General] key{told} ignored`，`told` 只在 `is_key_name` 为真时写成键名、`IOS_ONLY_KEYS` 的追加 `:1172` ～ `:1174`）。`value.rs`：`split_list`（`:175`）、`split_definition(raw) -> Option<(&str, &str)>`（`:314`）、`parse_bool(s) -> Option<bool>`（`:328`）。`text/mod.rs`：`Profile::section(&self, name: &str) -> Option<&Section>`（`:148`）、`Section::active_entries`（`:87`）、`KEY_VALUE_SECTIONS`（`:169`，`[MITM]` 已在其中）；`Entry` 的 `Debug` 自己走 `redact::redact_line`（`:47` ～ `:64`）。`hostlist.rs` 全部：`HostPattern`（`:10`）、`PortSpec`（`:20`：`Default` / `Any` / `Port(u16)`）、`HostListEntry`（`:27`：`negate`、`pattern`、`port`、`raw`）、`HostList`（`:35`：`entries`、`default_port`、`invalid`），`parse(text: &str, default_port: Option<u16>) -> HostList`（`:50`）、`matches(&self, host: &HostName, port: u16) -> Option<bool>`（`:70`，`PortSpec::Default` 走 `self.default_port.is_none_or(…)`），私有的 `pattern_matches`（`:85`）、`split_host_port`（`:98`）、`parse_port`（`:116`，`0` 读成 `Any`）、`parse_entry`（`:125`）。`types.rs` 的 `HostName::parse`（`:14`）与 `canonical`（`:53`）。`config.rs`：`Config` 是 `#[derive(Clone, Debug, PartialEq, Eq)]`（`:70` 起，`deferred: DeferredSections` 在字段表里），`from_profile` 里 `parse_general(profile.section("General"), &mut diags)` 在 `:667`，`[Keystore]` 的循环 `:862` ～ `:889`（未知字段 `W0001`，`E0021` 在 `:871` ～ `:884`：`!is_base64(&k.base64)` 时 `Diagnostic::error`，**错误级**，而且无论报不报错 `keystore.push(k)` 都执行），`ConfigSummary` 在 `:269`。`deferred.rs`：`DEFERRED` 名单 `:6` ～ `:19`（`MITM` 在 `:7`）、`is_deferred` `:21`、`DeferredSections::collect` `:48`。`keystore.rs`：`KeystoreType`（`:10`：`P12` / `OpensshPrivateKey`）、`KeystoreItem`（`:16`：`name`、`kind`、`base64`、`password`、`unknown`、`span`）、它的 `Debug`（`:28` 起）只写 `name`、`kind`、有没有 `password`、`unknown` 的条数与 `span`。`diagnostic.rs`：`codes` 模块里 `E0021` 在 `:194`、`W0016` 在 `:218`，`codes::hint` 的表 `:325` 起，三张测试表 `WITH_DEFAULT`（`:415`）、`PER_SITE`（`:455`）、`WITHOUT`（`:462` 起），`every_code_is_decided_once`（`:517`）读源码的 `pub const` 逐个比对——`W0039` 要进 `WITHOUT` 并写理由。`redact.rs` 的 `SECRET_KEYS`（`:26` 起的 14 项）里已有 `ca-passphrase`（`:34`）与 `ca-p12`（`:35`）。`tests/secrets.rs` 查 `Debug` 的写法：`the_debug_of_a_loaded_profile_holds_no_secret`（`:285`）是让 `format!` 的整份 `Config` `Debug` 过一遍 `leaks`（`:132`，按 `MARKER`、`UUID` 与 `KEY` 的前 12 个字符逐行找）；`profile()` 的 `[MITM]` 在 `:107` ～ `:109`（两个凭据都带 `MARKER`，**没有** `#!` 前缀，在这台机器上也读），`:539` ～ `:540` 那一对 `#!MACOS-ONLY` 属于另一个用例（`:515`），在这台机器上是停用的 | 本提交 |
| 3 | 0 | 第 3 项（`rurge-proto`）与 P4 的一处出入。`keystore.rs`：`pub fn decode_base64(item: &KeystoreItem) -> Result<Vec<u8>, BuildError>`（`:12`，消息是 `` keystore item `{name}`: `base64` is not valid Base64 ``，先 `STANDARD` 再 `STANDARD_NO_PAD`）、`pub fn decode_p12(item: &KeystoreItem) -> Result<ClientIdentity, BuildError>`（`:26` ～ `:59`，三条消息分别是 cannot be decoded (wrong password or unsupported PKCS#12) / holds no private key with a certificate / holds no certificate，链是 `private_key_chain().chain()` 的原序、私钥是 `PrivateKeyDer::Pkcs8`）。**出入**：两个函数今天都收 `&KeystoreItem`，不是 `(label, base64, password)`——P4 写的 `decode_pkcs12(label, der, password)` 与 `base64_bytes(text)` 是**新**的两个函数，`decode_p12` 改成先 `decode_base64(item)` 取 DER 再 `decode_pkcs12(&item.name, &der, item.password.as_deref().unwrap_or(""))`，三条消息逐字不变（`[Keystore]` 条目的两种消息因此照旧）。`decode_p12` 的调用处只有 `build.rs:89` 与 `testing/tls.rs:375`（用例里读回自己写的 p12），`decode_base64` 另有一处 `rurge-proto-ssh/src/keys.rs:14`，改成 `base64_bytes` 时要一并看。`transport/tls.rs`：`ClientIdentity`（`:20`：`chain` 与 `key`，`pub`）、`enum Mode`（`:25`，私有：`Standard { inner, verify_name }` / `Pinned([u8; 32])` / `Insecure`）、`struct Verifier`（`:37`，私有）、`impl ServerCertVerifier for Verifier`（`:48`）的四个方法——`verify_server_cert`（`:49`）、`verify_tls12_signature`（`:80`）、`verify_tls13_signature`（`:94`）、`supported_verify_schemes`（`:108`）；`fn client_config(opts, server, default_alpn, identity, roots, versions)`（`:122` ～ `:169`，私有，`TlsClient::build` 在 `:179`、`QuicTls::build` 在 `:231` 调它）——设计第 20 节第 3 条要的 `origin_client_config` 是新增的公开函数，照它的样子写、`skip-server-cert-verify` 走 `Mode::Insecure`。`testing/tls.rs`：`TlsFixture`（`:48`：`issuer`、`ca`、`leaf`、`leaf_key` 与 `seen`）、`new(names: &[&str]) -> Arc<TlsFixture>`（`:59`）、`seen(&self) -> Vec<SeenHandshake>`（`:260` 一带，`SeenHandshake { sni, alpn, client_cert }` 在 `:42`）、`roots`（`:90`）、`ca_pem` / `leaf_pem` / `leaf_key_pem`（`:103`、`:107`、`:112`）、`client_p12_base64(&self, common_name: &str) -> String`（`:118` ～ `:130`：`issue_client` 造一对、`PrivateKeyChain::new(key, [1,2,3,4], [cert])`、`store.writer("pw").write()`、`STANDARD` 编码）、`issue_client`（`:133`）、`acceptor(&self, require_client_cert: bool) -> TlsAcceptor`（`:145`）、`acceptor_with_alpn`（`:151`）、`impostor_acceptor`（`:196`）、`accept`（`:221`）、`accept_patiently`（`:234`）、`record`（`:260` 一带，`Box::new(tls)` 就是今天的 `BoxedStream`）、`spawn_echo(self: &Arc<Self>, require_client_cert: bool) -> SocketAddr`（`:318`）；`testing` 由 `#[cfg(any(test, feature = "testing"))] pub mod testing;`（`lib.rs:22` ～ `:23`）带进来 | 本提交 |
| 4 | 0 | 第 3 项（`rurge-net`、`rurge-http`）与两处出入。`rurge-net`：`tls.rs` 的 `pub fn root_store() -> Arc<rustls::RootCertStore>`（`:6` ～ `:26`，`OnceLock`，`rustls_native_certs::load_native_certs()`、`add_parsable_certificates`，`added == 0` 时 WARN 并改用 `webpki_roots::TLS_SERVER_ROOTS`）；`connector.rs:21` 的 `pub trait AsyncStream: AsyncRead + AsyncWrite + Unpin + Send` 与 `:23` 的 `BoxedStream`；`testing.rs` 的 `TestServer::spawn_tls_with(acceptor: TlsAcceptor) -> TestServer`（`:82` 一带，转 `start(Some(acceptor))`，接受之后 `a.accept(stream)` 再 `auto::Builder` 服务，连接数在交给任务之前加一）。`rurge-http/src/engine.rs`：`Conn`（`:56` ～ `:86`：`session`、`entry`、`idle`、`head_wait`、`drop_hold`、`abort`、`token`、`tracker`，八项）**今天有三处构造**——`crates/rurge-engine/src/serve.rs:237`、`crates/rurge-engine/src/engine.rs:1946`（监听器的绝对形式，`plain_requests`）与测试里的 `crates/rurge-http/src/engine.rs:2099`；**出入**：计划「文件结构」与任务 4 写的是 `crates/rurge-engine/src/{serve.rs,http.rs}`，`http.rs` 里没有 `Conn` 字面量，第三处在 `engine.rs:1946`。按代码做：任务 4 给这三处各加 `origin: None`（`http.rs` 不用改）。`prepare`（`:511` 起，`session.protocol = Some(ProtocolKind::Http)` 写死在一处）、`failed(&self, guard: &Guard, pages: Pages, record: String, page: String)`（`:623`，**不**调 `mark_upstream_failed`）、`timed_out`（`:640`）、`page`（`:658`）、`connect(&self, handle, route)`（`:686` ～ `:736`：`upstreams.dial`、令牌已取消时 `Stopped`、`Metered` 包流、`hyper::client::conn::http1::Builder` 握手、`tracker.spawn` 两层）、`redial(&self, guard, pages, route)`（`:738` ～ `:767`，三支分别是 `Refused` 转 `answer`、`Stopped` 转 `Close`、`Handshake(e)` 转 `failed` 带 upstream handshake failed 与原文）、`DialError`（`:959`：`Refused(Refusal)` / `Stopped` / `Handshake(String)`）、`upstream_failed`（`:796`，`if !e.is_user() { handle.mark_upstream_failed() }`，`failed` 自己不调）、`promote`（`:833` ～ `:955`）：`tunnel == true` 那一支 `:844` ～ `:848`（`strip(…, Keep::NONE)`、去掉 `CONTENT_LENGTH`），`else` 那一支 `:849` ～ `:857`（`Keep { upgrade: true, proxy_authorization: false }`），之后是取回（`upstream.downcast::<TokioIo<Metered<BoxedStream>>>()` 成功的一支与通用的一支）与转发、`form_of<B>(req: &Request<B>, fallback: (&HostName, u16)) -> (String, Option<String>)`（`:1064` ～ `:1094`，`http://` 写死三处）、`origin_authority(host: &HostName, port: u16) -> String`（`:1101` ～ `:1110`，只省略 80）、`Served { routed: u64, error: Option<String> }`（`:1664` ～ `:1672`）、`pub async fn serve_connection<U: Upstreams>(client: BoxedStream, requests: Requests<U>) -> Served`（`:1675` 起）。测试模块：`Fake`（`:1774`，`struct Fake(Arc<Inner>)`）、`Rig { client, fake, token, served }`（`:2045`）、`connected`（`:2067`）、`rig_timed`（`:2076`）、`rig_head_wait(entry, target, idle, drop_hold, head_wait) -> Rig`（`:2089` ～ `:2118`，`Conn` 字面量在 `:2099`）、`a_request_without_host_is_named_by_its_target`（`:3348` ～ `:3362`，四组断言 `form_of` 的 `http://` URL 与 `http_host == None`） | 本提交 |
| 5 | 0 | 第 3 项（`rurge-engine`）：名字都在。`serve.rs`：`SniffCounts`（`:30`）、`SniffedCounts`（`:64`）、`HttpCounts { sniffed, requests }`（`:80`，`Clone + Copy + Debug + Default + PartialEq + Eq`）、`sniffable_session`（`:94` ～ `:145`）、`named_at_a_proxy_port`（`:161` ～ `:181`）、`forces_http_engine`（`:184` ～ `:195`）、`serve_http`（`:201` ～ `:282`）**分段**：`self_arc` `:207`；`turn_back` `:210`；`look_up_owner` `:211`；承载连接的句柄 `:212` ～ `:222`（`session_token` `:215`、`SessionHandle::new_with_slot` `:216` ～ `:221`）、`lease` 与 `Turned::Unknown` 那一支 `:223` ～ `:232`；`quic_adaptive().on_tcp` `:234` ～ `:235`；`let runtime = self.runtime()` `:236` 与 `Conn` 字面量 `:237` ～ `:247`；`ConnUpstreams::new(…)` `:249` ～ `:256`；`serve_connection` 的 future `:257` ～ `:260`；与停止竞速的 `select`（`biased`，`stopped(opts.stop)` 先取消令牌再等同一个 future）`:266` ～ `:273`；`served.routed == 0` 那一支 `:274` ～ `:281`。`serve_sniffed`（`:294` ～ `:364`），判为 TLS 的那一支是 `:317` ～ `:322`（`session.sni = hello.sni`、`session.protocol = Some(ProtocolKind::Https)`，`into_engine` 保持 false）；`Sniffed::Http` 那一支 `:323` ～ `:326`；`reaches_the_rules`（`:366` ～ `:379`）；`stopped`（`:383` ～ `:390`）。`shared.rs`：`EngineShared` 的字段（`:172` ～ `:209`：`cell`、`selections`、`resolver`、`roots`、`empty_group`、`auto`、`processes`、`externals`、`network`、`owners`、`sniff_wait`）与 `new(initial: GroupSelections)`（`:212` ～ `:228`）。`runtime.rs`：`RuntimeOptions`（`:17` ～ `:25`）、`Runtime` 的字段（`:27` ～ `:57`）、`pub async fn build(config: Config, mut opts: RuntimeOptions) -> anyhow::Result<Runtime>`（`:62` 起，`match &opts.shared.roots` 在 `:119` 一带）。`engine.rs`：`HANDSHAKE_TIMEOUT`（`:50`）、`Engine` 的字段（`:174` ～ `:238`，`sniff_counts` `:203`、`request_count` `:207`、`state` `:208`、`shared` `:209`）、`Engine::new(mut runtime: Runtime) -> Arc<Engine>`（`:244` 起）、`session_token`（`:465`）、`look_up_owner`（`:498`）、`attach_observers`（`:551`）、`publish_generation(&self, next: &mut Runtime)`（`:631` ～ `:659`）、`attach_state(&self, store: Arc<StateStore>)`（`:690` ～ `:692`）、`set_mode`（`:703` ～ `:713`，先存原子量再 `store.update`）、`http_counts`（`:1411` ～ `:1417`，字面量在 `:1412`）、`plain_requests` 里第三处 `rurge_http::Conn`（`:1946`）；`reload.rs` 的 `swap_runtime(self: &Arc<Self>, mut next: Runtime) -> bool`（`:47` ～ `:73`）。`fake_address.rs` 的 `turn_back(&self, session: &mut SessionInfo) -> Turned`（`:56`）。`state.rs`：`STATE_VERSION = 1`（`:13`）、`Features`（`:17` ～ `:24`，六个 `bool`）、`State`（`:27` ～ `:41`）、`State::default`（`:43` ～ `:56`）、`StateStore` 的 `open` / `snapshot` / `update` / `try_update`（`:79`、`:91`、`:98`、`:119`）、`read_state`（`:136` ～ `:157`）；`Features` 的用处全在 `cli/enhanced`（`enhanced_mode`）与 `cli/sysproxy`（`system_proxy`）里，都是布尔 | 本提交 |
| 6 | 0 | 第 3 项（可执行文件、`rurge-api`、用例夹具）。可执行文件：`cli/run.rs` 的 `async fn reload(d: &Daemon<'_>, listeners: &mut Vec<(ListenerSpec, Running)>) -> ReloadReport`（`:487` 起），`rurge_engine::load_checked(d.config, d.load_opts)` 在 `:489` 是**直接调**（异步函数里，见第 7 行）；`attach_state` 的唯一调用在 `:1727`，`StateStore::open` 在 `:1649`（`let (store, state) = …`，`state` 就在手边），另两处 `:1315`、`:1352` 只取 `store`。`rurge-api`：`routes/features.rs` 全部（`FEATURES` 六项 `:12` ～ `:19`、`known` `:21`、`get_feature` `:56` 的 `_ => json!({ enabled: false })`、`set_feature` `:71` 的 `_ => not_implemented`）、`routes/rurge_http.rs` 全部（`HttpJson { sniffed, requests }` `:12`、`SniffedJson` 五个键 `:20`、`http` `:28` 逐字段拷）、路由表 `lib.rs:79` ～ `:80` 与 `:100`、`ApiError::not_found(message: impl Into<String>)`（`error.rs:34`）与 `IntoResponse`（`:48` ～ `:51`，正文是带 error 一个键的对象）、`?x-key=` 由 `auth.rs` 的 `presented_key`（`:106` ～ `:120`）认：先 `x-key` 头，再 URI 的 query 里 `strip_prefix("x-key=")`。用例夹具：`tests/common/mod.rs` 的 `Profile`（`:200` ～ `:212`，七个字段 `general`、`proxies`、`groups`、`hosts`、`rules`、`keystore`、`sections`，`text` 把 `sections` 放在 `FINAL,DIRECT` 之后）、`harness_trusting(p, roots) -> Harness`（`:236`）、`harness_with`（`:244`）、`harness_timed`（`:260`）、`connect_via_http(proxy: SocketAddr, authority: &str) -> TcpStream`（`:367`）、`connect_via_socks5(proxy: SocketAddr, host: &str, port: u16) -> TcpStream`（`:393`）、`wait_until(what: &str, check: impl FnMut() -> bool)`（`:428`，五秒）、`the_records(h: &Harness, n: usize) -> Vec<RequestRecord>`（`:446`）、`origin_addr(origin: &TestServer) -> SocketAddr`（`:695`）；`common/tun.rs` 的 `listener_rules(upstream_port: u16) -> String`（`:45`）、`Rig { h, tun, app, pinger }`（`:53`，`on(h: Harness) -> Rig` 在 `:65`、`fakes(&self, name: &str) -> (IpAddr, IpAddr)` 在 `:84`）、`guarded(rules: &str) -> String`（`:37` 一带）、`tun_records(h: &Harness, n: usize) -> Vec<RequestRecord>`（`:151`）；`LogCapture` 的公开方法 `start`（`:37`）、`everything`（`:43`）、`lines`（`:67`）；`rurge-tun` 的 `AppTcp`（`testing/app.rs:266`）；`crates/rurge/tests/cli.rs` 的 `rurge_command`（`:60`）与 `rurge_process`（`:75`）；`crates/rurge-api/tests/api.rs` 的 `api_built(rules: &str, groups: &str, geo: Option<&TestServer>, seed: impl FnOnce(&Path)) -> Api`（`:211` ～ `:280` 一带，profile 用 `format!` 拼，P27 的 `extra` 接在 `FINAL,DIRECT` 之后）、`Api` 的字段（`:97` ～ `:109`，`state_path` 在 `:100`） | 本提交 |
| 7 | 0 | 第 3 项第 1 件（P12：`load_checked` 的每个调用处在哪个线程上，逐个看过）：`profile/boot.rs:162` 在 `tokio::task::spawn_blocking` 里；`profile/verdict.rs:464`（`judge`）经 `:558` 等处的 `spawn_blocking` 进；`crates/rurge-engine/src/subscriptions.rs:305` 在 `pub fn check_profile(…)`（同步函数）里，它的两个调用处——`cli/check.rs:65`（同步的 `run`）与 `rurge-api/src/routes/profiles.rs:132`（`spawn_blocking`）——都在阻塞线程上，**不用挪**（P12 说没核过的那一处，核过了）；`cli/check.rs` 与 `cli/run.rs:1570`（`run()` 里）是同步的；`cli/run.rs:489` 的 `reload` 是异步函数里直接调——任务 2 照计划挪进 `spawn_blocking`，`d.config` 与 `d.load_opts` 都是借来的引用，要先把前者克隆成 `PathBuf`、后者克隆一份再进闭包（`LoadOptions` 是 `Clone`）。测试里的四处调用不管 | 本提交 |
| 8 | 0 | 第 3 项第 2、3 件（任务 2 的两条前提）。`E0021`（`codes::E_KEYSTORE_BASE64`）是**错误级**；有它时 `Config` 照样建出来，那个条目**照样留在 `config.keystore` 里**（`config.rs:871` ～ `:885`：报错之后才 `keystore.push(k)`），但 `rurge run` 在 `has_errors()` 为真时打印诊断并以退出码 2 结束（`cli/run.rs:1588` ～ `:1592`），重载同样拒绝并保留当前配置（`cli/run.rs:495` ～ `:514`）。所以任务 2 的 `a_keystore_item_with_bad_base64_is_left_to_e0021` 要断言的是：`check` 不再多报一条 `W0039`（`reported` 为真）、`E0021` 只有 `[Keystore]` 那一条、`decode_base64` 的原话不变。`rurge check` 的退出码（`cli/check.rs:93` ～ `:99`）：有错误是 2；只有告警时 `--strict` 才是 1，否而是 0。任务 2 的 `check_names_an_unusable_mitm_ca` 因此看 0 与那一条 `W0039`（不加 `--strict`），`check` 的文本是 errors / warnings / notes 三数那一行 | 本提交 |
| 9 | 0 | 第 3 项第 4 件（rcgen 0.14.7，路径相对 `rcgen-0.14.7/src/`，逐条读过）：`KeyPair::try_from(&PrivateKeyDer)`（`key_pair.rs:533` 起）只要 `PrivateKeyDer::Pkcs8` 一支（PKCS#1 直接 `CouldNotParseKeyPair`），依次试 Ed25519、P-256、P-384、RSA，全不成时 `return Err(Error::CouldNotParseKeyPair)`——**ring 的原因丢了**（P1 成立）。`DistinguishedName::from_name`（`lib.rs:523` 起）在同一个 RDN 里还有第二个属性时 `return Err(Error::CouldNotParseCertificate)`（多值 RDN 就在 `Issuer::from_ca_cert_der`（`lib.rs:245` ～ `:256`）里报错，P5 第 12 条的同一个 `e`）；`DistinguishedName::push`（`lib.rs:508` ～ `:514`）同一类型是**覆盖**（`entries.insert`）。`CertificateParams::default()`（`certificate.rs:81` ～ `:104`）带一个 CN `rcgen self signed cert`，`not_before` 1975-01-01、`not_after` 4096-01-01，`serial_number` 是 `None`，`use_authority_key_identifier_extension` 是 `false`（P9 要先换掉 CN 成立）。字段类型（`certificate.rs:56` ～ `:76`）：`not_before` / `not_after` 是 `time::OffsetDateTime`，`serial_number: Option<SerialNumber>`，`key_identifier_method: KeyIdMethod`。`SerialNumber::from_slice(bytes: &[u8]) -> SerialNumber`（`lib.rs:878`）。`CustomExtension::from_oid_content(oid: &[u64], content: Vec<u8>)`（`certificate.rs:685`，内容要已是 DER、`critical` 缺省 `false`）与 `set_criticality(&mut self, bool)`（`:693`）——P6 的「不带 SKI 的 CA」就靠这两个。`CertificateParams::new(subject_alt_names)`（`certificate.rs:111`）把能 `parse::<IpAddr>()` 的签成 `SanType::IpAddress`、其余 `SanType::DnsName`。特性：`[features]` 里没有显式的 `x509-parser` 项，它是可选依赖 `x509-parser = 0.18` 带来的隐式特性（`ring` 特性另带 `x509-parser?/verify`），所以 `features = [x509-parser]` 可用；它要的包（`x509-parser 0.18.1`、`der-parser 10.0.0`、`asn1-rs 0.7.2`、`oid-registry 0.8.1`、`rusticata-macros 4.1.0`、`nom 7.1.3`、`yasna 0.5.2`、`data-encoding 2.11.1`、`ring 0.17.14`）在 `Cargo.lock` 里都有，包数不变 | 本提交 |
| 10 | 0 | 第 3 项第 5 件（x509-parser 0.18.1，路径相对 `x509-parser-0.18.1/src/`）：`X509Certificate::subject(&self) -> &X509Name`（`certificate.rs:315`）、`validity(&self) -> &Validity`（`:327`，`Validity { not_before: ASN1Time, not_after: ASN1Time }` 在 `:712` ～ `:715`，`ASN1Time::timestamp() -> i64` 在 `time.rs:70`）、`basic_constraints()`（`certificate.rs:396`，`BasicConstraints { ca: bool, path_len_constraint: Option<u32> }` 在 `extensions/mod.rs:289`）、`key_usage()`（`:414`，`KeyUsage { flags: u16 }` 与 `key_cert_sign()` 在 `extensions/keyusage.rs:10`、`:30`）。**SKI 没有 `subject_key_identifier()` 这样的便捷方法**：只能 `iter_extensions()` 之后按 `parsed_extension()` 匹配 `ParsedExtension::SubjectKeyIdentifier(KeyIdentifier)`（`extensions/mod.rs:194`，文档示例在 `:72` 一带）。任务 2 的检查照这几处写（`BasicConstraints` 缺失时是 `None`，`key_usage` 写了才查 `keyCertSign`） | 本提交 |
| 11 | 0 | 第 3 项第 6 件（rustls 0.23.43 与 tokio-rustls 0.26.4，路径相对各自的 `src/`）：`ServerConfig` 是 `#[derive(Clone)]` 的 `pub struct`（`server/server_conn.rs:297` 起），`session_storage`（`:323`）、`cert_resolver`（`:334`）、`alpn_protocols`（`:338`）、`max_early_data_size`（`:366`）都是 `pub`；`ServerSessionMemoryCache::new(size: usize) -> Arc<Self>`（`server/handy.rs:48`）；`sign::SingleCertAndKey(Arc<CertifiedKey>)`（`crypto/signer.rs:96`）有 `From<CertifiedKey>`（`:98`）与 `From<Arc<CertifiedKey>>`（`:104`），并实现 `ResolvesServerCert`（`:124`）；`crypto::ring::sign::any_ecdsa_type(der: &PrivateKeyDer) -> Result<Arc<dyn SigningKey>, Error>`（`crypto/ring/sign.rs:45`）；`Error::AlertReceived(AlertDescription)`（`error.rs:69`），`AlertDescription` 在 `enums.rs`（`UnknownCA => 0x30`、`BadCertificate => 0x2a`、`CertificateUnknown => 0x2e`）；`CommonState::handshake_kind() -> Option<HandshakeKind>`（`common_state.rs:188`，`HandshakeKind::Resumed` 在 `:785` 起），`ServerConnection` 经 `Deref<Target = ConnectionCommon<…>>`（`server/server_conn.rs:761` 起）取到它；`WebPkiServerVerifier::builder_with_provider`（`webpki/server_verifier.rs:168`），今天的 `transport/tls.rs:148` 已经在用。`ServerConnection` 是不是 `Send`：源码里没有负向 impl，字段都 `Send + Sync`；**今天已有证据**——`rurge-proto/src/testing/tls.rs` 的 `record` 把 `tokio_rustls::server::TlsStream<TcpStream>` 装进 `BoxedStream`（`Box::new(tls)`），那个 `BoxedStream` 要 `Send`，所以服务端的 TLS 流今天就能装进 `BoxedStream`（`AsyncStream` 只要求 `Send`，不要求 `Sync`）。tokio-rustls：`TlsAcceptor::accept` 的错误是 `io::Error`，rustls 的错经 `io::Error::new(io::ErrorKind::InvalidData, rustls_error)` 包出来（`server.rs:161` 与 `:237`），所以 `err.get_ref()` / `err.downcast_ref::<rustls::Error>()` 取得到那个 `rustls::Error`，`kind()` 是 `InvalidData`（不是 `Other`）；任务 3 的 `classify` 照这个写。`poll_read` 的两端（P11 成立）：`reader().into_first_chunk()` 在握手已结束而没收到 `close_notify` 时给 `io::ErrorKind::UnexpectedEof`（`conn.rs:188`），收到 `close_notify` 之后给 `Ok(&[])`（`conn.rs:201` ～ `:210`），就这样 `tokio-rustls` 的 `poll_fill_buf`（`common/mod.rs:190` 一带）读成空的一块、`poll_read` 交出 0 字节 | 本提交 |
| 12 | 0 | 第 3 项第 8、9 件。工作区的 tokio（根 `Cargo.toml:28`）开了 `rt-multi-thread`、`net`、`time`、`fs`、`sync`、`macros`、`io-util`、`signal`——`sync`（`OnceCell`）与 `spawn_blocking`（要 `rt`，由 `rt-multi-thread` 带来）都够用（V19 的第 8 件）。`ListenerKind` 在 `crates/rurge-config/src/session.rs:10` ～ `:18`（`Http` / `Socks5` / `Tun` / `Forward` / `Internal`）。**出入**：引擎里**没有**一处现成的、按 `ListenerKind::Tun` 分两种写法的 WARN / INFO 日志行可照抄——现有的按监听器分叉只有两处：`observe.rs:336` ～ `:351` 的流量分桶（不是日志）与 `engine.rs:1455` ～ `:1481` 的会话日志（`Rejected` / `Failed` 两行在 INFO 里对所有监听器都带 `dst`，正是 M3c 计划延后第 6 行登记的那一处例外）。任务 6 的那三对文字（P21）要自己写这个分叉：按 `matches!(session.listener, ListenerKind::Tun)` 决定写哪一句、名字另起一行 DEBUG | 本提交 |
| 13 | 0 | 第 3 项第 10、11、12 件。`?x-key=`：认，在 `crates/rurge-api/src/auth.rs` 的 `presented_key`（`:106` ～ `:120`）：先 `x-key` 头，再 URI 的 query 里按 `strip_prefix("x-key=")` 取（不做百分号解码）；404 的正文是带 `error` 一个键的对象（`error.rs:48` ～ `:51` 的 `IntoResponse`），`ApiError::not_found` 只定状态码（`:34`）。`TlsFixture::acceptor(bool)` 的参数是 `require_client_cert`（`testing/tls.rs:145`，真时用 `WebPkiClientVerifier` 只放行本夹具 CA 签的客户端证书）；`seen(&self) -> Vec<SeenHandshake>` 给出每次握手的 `sni`（`Option<String>`）、`alpn`（`Option<String>`）、`client_cert`（`bool`）——**有 SNI**；`new(names)` 为每个名字签一张叶证书，能 `parse::<IpAddr>()` 的签成 IP 的 SAN、其余签成 DNS 名（rcgen 的 `CertificateParams::new`），所以 `new(&["127.0.0.1"])` 得到 IP 的 SAN。`HostList::default()` 的 `default_port` 是 `None`（derive 的 `Default`，`hostlist.rs:34` ～ `:39`），`HostList::parse(text, Some(443))` 才让不带端口的项只认 443——任务 1 的 `MitmConfig::default()` 要自己写 `HostList::parse("", Some(443))`（或等价的 `HostList { default_port: Some(443), ..Default::default() }`） | 本提交 |
| 14 | 0 | 第 4 项（数）：`Cargo.lock` 里 `[[package]]` 520 个；`crates/rurge-platform/tests/unsafe_functions.rs:18` 是 `const LISTED: [(&str, &str); 23] = [`，数组长度 23、数下来也是 23 项；带本机账户名的文件 26 个。都与计划写的一样 | 本提交 |
| 15 | 0 | 第 5 项（P15 的表，任务 1 与任务 2 的行，逐条找到读过）。任务 1：`deferred.rs` 的 `prefix_and_membership_matching`（`:66`，`assert!(is_deferred("MITM"))` 在 `:67`）——照计划改成 `!is_deferred("MITM")`，其余照旧；同文件 `collect_keeps_only_active_entries_and_get_is_case_insensitive`（`:83` ～ `:96`）拿 `[MITM]` 当不生效的节的例子（`:85`，断言 `deferred.sections.len() == 1`、`get("mitm")` 的 `entries.len() == 1`）——`[MITM]` 生效之后 `DeferredSections::collect` 收不到它，换 `[Testing]`，两行键照写；`config.rs` 的 `a_load_gives_each_diagnostic_the_hint_of_its_code`（`:2153`，profile 里的 `[Nowhere]` 与 `[MITM]` 那两行在 `:2156`）今天断言的是每条诊断的 `hint` 与 `codes::hint` 一致，并且那张 `(code, has_one)` 表要求每个码**至少出现一次**、`W_DEFERRED_SECTION` 也在其中（`:2175`）——今天的 `W0016` 就是 `[MITM]` 产生的；任务 1 之后 `[MITM]` 读到键，`x = 1` 起 `W0001`（有缺省建议，不影响那张表的 `has_one`），但 `W0016` 一条也没有了 → `assert!(!found.is_empty(), …)` 变红。照计划改期望之外还要**在那份 profile 里留一个仍不生效的节**（例如 `[Map Local]`），否则这一条过不去——计划只写了期望照新的读法，这一句补上；`warnings`（`:2601`，profile 的 `[MITM]` 与 `hostname = *` 在 `:2603`）断言 `W0016` 恰好 1 条与 `l.config.deferred.sections.len() == 2`（`[MITM]` 与 `[Script]`）——任务 1 之后 `[MITM]` 是生效的节、`hostname = *` 合法（没有 `W0001`），`W0016` 仍是一条（一条消息列全部），**第二个数从 2 改成 1**；`an_ssid_setting_section_is_read_and_no_longer_inactive`（`:6246`，profile 在 `:6248`）断言 `inactive` 恰是 `sections parsed but inactive in this version: [MITM]`（`:6258`）——任务 1 之后那份 profile 里没有不生效的节，消息成空；照计划把 `[MITM]` 换成 `[Script]`（消息随之变成 `[Script]`）；语料库快照 `corpus__corpus__kitchen-sink.snap` 是 `(loaded.config.summary(), diags)`，`Config.mitm` 进不了它，只有两处变——`:98` 一带的 `deferred` 列表少一行 `MITM`、`:113` 的 `W0016` 消息少 `[MITM]`（P25 的两处对；语料 `tests/corpus/valid/kitchen-sink.conf:152` 的 `[MITM]` 四个键都认得，不产生 `W0001`）。`tests/secrets.rs`：`the_debug_of_a_loaded_profile_holds_no_secret`（`:285`）断言 `Config` 的 `Debug` 里没有秘密，`profile()` 的 `[MITM]`（`:107` ～ `:109`，两个凭据都带 `MARKER`）今天靠 `DeferredSections` 里 `Entry` 的 `Debug`（`redact_line`）躲过——**任务 1 把两个值搬进 `MitmConfig` 之后，这一条会因为 `Config` 的 `Debug` 而变红，直到 `MitmConfig` 的 `Debug` 只写有没有**：所以它不是照过，是照过的前提为 `MitmConfig` 的 `Debug` 写对；计划说的加断言是在此之上再加（例如断言 `MitmConfig` 的 `Debug` 里没有那两个 `MARKER` 值）；`:539` ～ `:540` 那一对 `#!MACOS-ONLY` 在另一个用例里、在这台机器上是停用的，不覆盖 `MitmConfig`。任务 2：`rurge-proto/src/keystore.rs` 末尾的用例断言 OpenSSL 现代与 `-legacy` 两种 p12 解出的链、指纹与三条错误消息——拆成 `decode_pkcs12` 之后照过（消息逐字不变，见第 3 行）；`testing/tls.rs:375`（读回自己写的 p12）改成 `p12_base64` 之后照过；`outbounds.rs` 的 `load_checked_is_load_plus_the_dry_build`（`:844` ～ `:859`，两处 `load_checked` 在 `:848`、`:858`）断言 `has_errors()`、`E_POLICY_BUILD` 恰好 2 条、缺文件时 `is_err()`——那份 profile 与 `BROKEN` 都没有 `[MITM]`，加进去的 `check` 不产生 `W0039`，照过 | 本提交 |
| 16 | 0 | 第 5 项（P15 的表，任务 4 ～ 7，逐条找到读过）。任务 4：`rurge-http/src/engine.rs` 的 `a_request_without_host_is_named_by_its_target`（`:3348` ～ `:3362`）四组断言 `form_of(&req, (&HostName::parse(host), port))` 的两个返回值（URL 与 `http_host == None`）——`form_of` 多一个 scheme 参数之后，原断言照旧、给 `http`，`https` 的几句写进新用例（照计划）；`rig_head_wait` 的 `Conn` 字面量（`:2099` ～ `:2109`）要补 `origin: None`（照计划），另加第 4 行说的 `rurge-engine/src/engine.rs:1946` 那一处。任务 5：`state.rs` 的 `loads_defaults_selections_and_tolerates_garbage`（`:176` ～ `:194`）写一份版本 1 加 `mitm: true` 的文件，断言 `state.features.mitm && !state.features.capture`——第 2 版把那四个改成 `Option<bool>` 之后，第 1 版文件读作没设过，断言要改成 `features.mitm` 与 `features.capture` 都是 `None`（照计划）；末尾的往返（`to_string` 再 `from_str` 相等）照过；`store_opens_updates_atomically_and_renames_garbage`（`:199` ～ `:230`）断言 `State::default()`、原子写、`.broken`、重开读回，**不断言版本号**，照过；`enhanced_backup_round_trips_and_an_old_file_reads_without_it`（`:260` 起）读版本 1 只断言 `enhanced_backup`，照过；`try_update_*` 两条照过。`EngineShared {` 的字面量共 9 处，**全部带 `..EngineShared::default()`**（`tests/common/mod.rs:237`、`http.rs:21`、`sniff.rs:224`、`outbounds_hysteria2.rs:48`、`outbounds_masque.rs:51`、`outbounds_tuic.rs:90`、`process.rs:205`、`subscriptions.rs:368`；`shared.rs:213` 是定义本身），表里预计没有成立，任务 5 加字段不用改用例。任务 6：`tests/sniff.rs` 的 `the_sniffs_are_counted`（`:884`，`HttpCounts` 字面量在 `:957` ～ `:970`）断言 `h.engine.http_counts()` 与 `HttpCounts { sniffed: …, requests: 1 }` 相等——`HttpCounts` 加 `mitm` 之后要加 `mitm: MitmCounts::default()`（照计划；`HttpCounts` 是 `PartialEq` 的 `Copy` 结构，`MitmCounts` 也要 `Default + PartialEq`），生产侧的字面量在 `engine.rs:1412` 一处。任务 7：`rurge-api/tests/api.rs` 的 `features_collections_stop_and_unknown_paths`（`:540` 起）断言 `GET /v1/features/mitm` 是 `enabled: false`、`POST` 答 501——照计划改成读作关、`POST` 501 的两段换成 `capture`，`mitm` 的断言进新用例；`rurge_http_counts_start_at_zero`（`:1113` ～ `:1136`）断言解析后的 JSON 与**原文逐字节**（`sniffed` 对象加 `requests`）——两处都加 `mitm` 对象，原文照 P23 那一串（键次序固定） | 本提交 |
| 17 | 0 | 第 5 项的补扫（扫的是 `[MITM]`、`features`、`EngineShared {`、`Conn {`、`HttpCounts {`、`STATE_VERSION` 与 `version` 在 `crates/` 与 `tests/` 下的全部命中，逐个读过）。表上没有、找到的四件：(1) `crates/rurge/tests/cli.rs` 的 `a_start_with_the_recorded_profile_leaves_state_json_as_it_is`（`:5347` ～ `:5365`）先写一份版本 1 的手写文件，再起守护进程、`POST /v1/stop` 之后断言文件**逐字节没动**——第 2 版只在写盘时升级，启动时不写盘就照过；这是一条会盯着「启动不要顺手写 `state.json`」的用例，任务 5、7 要守住（`restore_switches` 照 P20 只设原子量）。同文件另三处写版本 1（`:4880`、`:4908`、`:5055`）与 `tests/run/enhanced_mode.rs:65` 都只断言 `system_proxy_backup` / `outbound_mode` / `current_profile` / `enhanced_mode`，且都断言失败启动不改文件，同上。(2) `crates/rurge-api/tests/api.rs` 的 `api_built` 里有一句 `assert!(!loaded.diagnostics.has_errors(), …)`（`:238`）——任务 2 给 `load_checked` 加的是 `W0039`（告警），不影响；任务 7 的 `extra` 参数如果让某个新用例写进不可用的 CA，`W0039` 仍是告警。(3) `crates/rurge-config/src/redact.rs:1329` 与 `:1340` 的用例（`[MITM]` 两个凭据的行级脱敏）断言的是**行**的脱敏，`[MITM]` 类型化之后照样过（脱敏不经过 `Config`）。(4) `crates/rurge-config/tests/secrets.rs:1037` 的 `a_line_in_the_wrong_section_is_neither_quoted_nor_kept` 里那一段 `[MITM] # my certificate`（节头带注释，总设计 Q3 的那一种）与 `[MITM]` 的类型化无关，照过。`[MITM]` 的全部命中除上面这些就是 `config.rs:2156`、`:2603`、`:6248`、`:6258` 与 `deferred.rs:68`、`:85`、语料与快照——都在表里。`features` 的其余用处全在 `cli/enhanced`（`enhanced_mode`）与 `cli/sysproxy`（`system_proxy`）里，都是布尔，不受第 2 版影响。没有找到表外的第三条 `HttpCounts` 字面量，也没有不带 `..` 的 `EngineShared` 字面量 | 本提交 |
| 18 | 0 | 第 6 项（门禁，一台机器上的一次运行，不是 CI）：`df -h /d` 剩余 117 GiB（先跑）；`cargo fmt --all --check` 退出码 0；`cargo clippy --all-targets -- -D warnings` 退出码 0、0 个警告；`cargo test -j 8 --workspace --no-fail-fast` **90 个结果行、4867 通过、0 失败、13 忽略**，与基线（M1b 任务 8 的 90 / 4867 / 0 / 13）逐个相同，没有二进制以 `STATUS_HEAP_CORRUPTION` 或 `STATUS_ACCESS_VIOLATION` 退出，`cli` 的 `give_up` 与两条 DoQ 用例这次也没有出现，因此没有重跑。本任务只改文档、0 条用例，这个数就是本计划的基线。需要参照服务端的互操作用例在本机立刻返回、算在通过的数里。日志 `gate-task-0.log` 在工作区（git 忽略） | 本提交 |
| 19 | 1 | **协调者的裁定**：任务 1 里两处现有用例的期望。(1) `crates/rurge-config/src/config.rs` 的 `a_load_gives_each_diagnostic_the_hint_of_its_code`（`0fb1ca4` 上 `:2156`）：profile 里的 `[MITM]\nx = 1` 换成 `[Script]\nx = 1`。`[Script]` 仍不生效，`W0016` 照样出现，那张要求 `W_DEFERRED_SECTION` 至少出现一次的码表照过；用例的意思不变。(2) 同文件 `warnings`（`0fb1ca4` 上 `:2601`）：`deferred.sections.len()` 的期望从 2 改成 1；`[MITM]` 与 `hostname = *` 那两行现在是生效的节里的合法键，`W0016` 仍是一条（来自 `[Script]`）。这条用例没有断言 `W0016` 的文字（文字在新加的 `the_mitm_section_is_typed_and_no_longer_deferred` 覆盖的那一侧，即 `an_ssid_setting_section_is_read_and_no_longer_inactive`，照 P15 表换成 `[Script]`） | 本提交 |
| 20 | 1 | **执行者的裁定**：`MitmSpans` 只有 `hostname`、`ca_p12`、`ca_keystore_name` 三个字段，这是任务 1 的短文里写的形状；P2 ⑦ 说“各键所在的行记进 `spans`”，只做到这三处——它们是任务 2 的 `W0039` 要报在的那三行（`hostname` 的两条名单里只有它报，`ca-keystore-name` 的两条报在它自己那一行）。其余六个键的行不记：没有一条诊断报在那一行，记下来只有无人读的字段 | 本提交 |
| 21 | 1 | **执行者的裁定**：`crates/rurge-config/src/hostlist.rs` 的 `matches_sni_ignores_the_port_of_a_bare_entry` 里，`-bad.example.com, *.example.com` 那一组对 `matches` 用的是 443（`matches_sni` 用的是 8443）。不带端口的项在 `matches` 里只认缺省端口 443，在 8443 上是 `None`；短文说“两个方法都是 `Some(false)`”只在 443 上成立，它没有给端口 | 本提交 |
| 22 | 1 | **执行者的裁定**：`client-source-address` 的诊断次序是先逐项的 `W0005`（按项在值里的次序）、再整键的一条 `W0029`（设计只说“有这样的项时整个键一条 `W0029`”，没有说先后）；值去掉空白之后为空时先算没写、返回 `None`，因此也不报 `W0029` | 本提交 |
| 23 | 2 | **协调者的裁定**：计划 P6 写私钥用 `pkey -outform DER` 转成 PKCS#8、RSA 的用 `genpkey … -outform DER`。本机 OpenSSL 3.2.4 这样写出的是传统格式（EC 是 SEC1、RSA 是 PKCS#1），ring 只收 PKCS#8。协调者改用 `openssl pkcs8 -topk8 -nocrypt -inform DER -in <私钥>.der -outform DER` 转出 RSA、P-521、多值 RDN、重复属性四把私钥；不带公钥的那把本来就是这样写的。四张 CA 与各自私钥的公钥逐对核过相同。以后要新的夹具照这个命令 | 本提交 |
| 24 | 2 | **执行者的裁定**：签发叶证书的那一个函数是 `crates/rurge-http/src/mitm/authority.rs` 的 `pub(crate) fn issue(&self, name: &HostName, now: OffsetDateTime) -> Result<(CertificateDer<'static>, PrivatePkcs8KeyDer<'static>), rcgen::Error>`：每次新生成一把 P-256 的 `KeyPair`，参数照 P9（名字不超过 64 个字符才写 CN、SAN 按域名或 IP、`IsCa::NoCa`、`DigitalSignature`、`ServerAuth`、`use_authority_key_identifier_extension` 取 CA 有没有 SKI、`SystemRandom` 取 16 字节并清最高位、`notBefore` 不早于 CA 的 `notBefore`、`notAfter` 不晚于 CA 的 `notAfter`），`params.signed_by(&key, &issuer)`；试签（P5 第 13 条）与任务 3 的 `leaf` 都调它，任务 3 再把返回的证书与私钥装成 `CertifiedKey` 并放进阻塞线程 | 本提交 |
| 25 | 2 | **执行者的裁定**：P26 只写了 `p12_base64` 与 `TlsFixture::ca_p12_base64` 两个测试帮手；「没有私钥（只有证书的 p12）」那一行还要写出一个只有证书包的 PKCS#12，所以 `rurge-proto` 的 `testing` 里另加 `p12_cert_base64(cert, password)`。p12-keystore 0.2.1 写得出来（`KeyStoreEntry::Certificate` 是一条证书包），读回时 `private_key_chain()` 是 `None`，因此落到 `decode_pkcs12` 的「holds no private key with a certificate」 | 本提交 |
| 26 | 2 | **执行者的裁定**：`Authority` 的时钟放在 `RwLock<Clock>` 里，`set_clock(&self)` 才能拨它（P7 写的是 `&self` 上的 `set_clock`）。`check` 拆成 `check(config)` 与私有的 `check_with(config, clock)`，用例经后者把时钟钉在 2027-01-01：`every_ca_problem_is_named_and_quotes_no_secret` 里过期与未生效两行的日期因此不取决于跑用例的那一天 | 本提交 |
| 27 | 2 | **执行者的裁定**：`origin_client_config_verifies_unless_insecure` 里「信另一个 CA 的根」那一半，根是自己在用例里用 rcgen 造的另一张 CA（subject 不同），不是第二个 `TlsFixture` 的根：`TlsFixture` 的 CA subject 都叫 `rurge test CA`，webpki 会把它当作签发者再比签名，错误是 `BadSignature` 而不是 `UnknownIssuer`（短文要的那一句） | 本提交 |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
