# 阶段 4 / M2c「CA 工具、`http-api-tls` 与收尾」实施计划

> **执行方式**：交给 Pi，由协调的会话逐个任务派工、逐个任务评审（设计第 2 节、M2-D1；做法同 M1a、M1b、M2a、M2b）。
>
> - 协调的会话每个任务开一个 Pi 会话，交给它这个任务的全文与要用的接口（派工文件）。Pi 做完把报告写进派工文件指定的路径，在本地提交。
> - 协调者读改动、重跑门禁、评审。有问题回到同一个 Pi 会话修正，再评审；过了才派下一个任务。
> - 每个任务：先写失败的用例、看到红；再实现到绿；跑门禁；对照本任务的「判据」与「全局约束」逐条自查；最后提交（**只在本地，不推送**）。钉住现有行为的用例（任务里写明“不要求先红”的）可以一写就绿。
> - 与计划的出入记进末尾的「执行期修正记录」，**不改计划的正文**。
>
> **停点**：M2c 做完，M2 的三份计划就都做完了。之后由项目所有者推送一次（连同 M1），协调者读回那次 CI 的注解与任务摘要（M2-D1）。M2c 有只在 CI 上编译的 `#[cfg(unix)]` 分支：两个私密文件建成 `0600`，与验证它的用例。

**目标**：做完 M2 剩下的三件事，并收尾 M2。

- **CA 工具**：`rurge mitm ca generate --out <目录>` 生成一个新的 CA，写四个文件；`rurge mitm ca export -c <配置>` 读出配置里 CA 的证书。
- **`http-api-tls`**：开着时 API 讲 HTTPS，叶证书由 MITM 的 CA 按名字签发；没有可用的 CA 时 API 不启动，也不退回明文。`rurge status` / `reload` / `stop` 经 `-c` 找到 API 时走 HTTPS。
- **`W0040`**：`http-api-tls` 开着而没有可用的 CA。

具体是：

- `rurge-config`：`W0040`。
- `rurge-http`：
  - `mitm::generate`：新 CA 的四个文件的内容；
  - `mitm::api`：`ApiTls`（API 的 TLS：模板与解密的连接同一个，只给 `http/1.1`；叶证书从同一个 `Authority` 签、同一个缓存）与 `client_config`；
  - `check` 加 `W0040`。
- `rurge-engine`：`Engine::api_tls()`（取最后一个生效过的 `Authority`），再导出 `ApiTls`。
- `rurge-api`：TLS 的监听器（握手不占接受连接的循环、同时至多 64 个、每个 30 秒）与 `serve_tls`。
- 可执行文件：
  - `rurge mitm ca generate` / `export`；
  - 起 API 时按 `http-api-tls` 选 `serve` 或 `serve_tls`，没有 CA 时不起；
  - 重载时关于 API 的告警；
  - 命令行的 HTTPS 客户端。
- M2b 计划延后事项第 1 行：`pipeline.rs` 一条用例的等待。
- 手工验收清单 `docs/acceptance/phase4-manual.md`（新建，M1 与 M2 的行为）；M2 的收尾文档。

**架构**：

- 新的工作区边只有一条：`rurge`（可执行文件）→ `rurge-http`（M2-D2，设计第 3 节）。
- `rurge-api` 照旧只依赖 `rurge-engine` 与 `rurge-config`（`CLAUDE.md`「架构」）：
  - `ApiTls` 经 `rurge-engine` 再导出；
  - `rurge-api` 只在开发依赖里加 `rurge-http`，单元用例要造 `Authority`。
- 外部依赖都已在锁文件里，包数 520 不变：
  - `rurge-http` 加 `p12-keystore`，`base64` 挪进正式依赖；
  - `rurge-api` 与可执行文件加 `rustls`、`tokio-rustls`。
- 平台代码只有 `#[cfg(unix)]` 的 `0600`（标准库的 `OpenOptionsExt::mode`）。它不进 `rurge-platform`，也不加 `unsafe`。

**技术栈**：都已在 `Cargo.lock` 里，源码在本机 cargo 注册表里。**不下载、不加新包**（M2-D2）。

- rcgen 0.14.7：自签的 CA。
- p12-keystore 0.2.1：`KeyStore::writer`、`Pkcs12Writer::{encryption_algorithm, mac_algorithm}`。
- ring 0.17：随机数、SHA-1（PKCS#12 的本地密钥标识）。
- rustls 0.23.43 与 tokio-rustls 0.26.4：`server::Acceptor`、`LazyConfigAcceptor`、`StartHandshake`、`TlsConnector`。
- axum 0.8.9：`serve::Listener`。
- hyper 1.11.1：`client::conn::http1`。
- x509-parser 0.18.1：用例读回证书。

**依据**：

- M2 细化设计（下面写"设计"指它）：`docs/superpowers/specs/2026-10-30-phase4-m2-mitm-design.md`。
  - M2c 用到第 13、14、15 节，第 16 节，第 17 节第 6 条，第 18、22 节，与已决事项 M2-D20 ～ D22。
  - **先读它第 23 节**：第 22 ～ 27 条是本计划改了设计正文的地方（随本计划提交）。
  - 设计与本计划的「计划期决定」不一致的，以计划为准。
- M2a、M2b 计划的「计划期决定」与执行期修正记录：`Authority`、`server_config`、`MitmShared::last_live` 的来历。
- 装 CA 的命令照各平台工具自己的用法写：
  - Windows 的 `certutil -addstore`、macOS 的 `security add-trusted-cert`；
  - Debian 的 `update-ca-certificates`、Fedora 的 `update-ca-trust`、Arch 的 `trust anchor`；
  - NSS 的 `certutil -A`。
  - mitmproxy 的证书安装文档写的是同一类步骤，只取事实。
- 写计划时对着 `cb74b35`（M2b 合并之后的 `phase4-m2`）核对过的事实写在「计划期决定」里。外部库的事实对着注册表里锁定版本的源码。

## 全局约束

**依赖**

- `rurge-http`：任务 2 在 `[dependencies]` 加 `p12-keystore.workspace = true`；把 `base64.workspace = true` 从 `[dev-dependencies]` 挪到 `[dependencies]`。
- `rurge`（可执行文件）：
  - 任务 3 加 `rurge-http.workspace = true`；
  - 任务 6 加 `rustls.workspace = true`、`tokio-rustls.workspace = true`。
- `rurge-api`：任务 5 加 `rustls.workspace = true`、`tokio-rustls.workspace = true`，`[dev-dependencies]` 加 `rurge-http.workspace = true`。
- 不加外部包。
  - `Cargo.lock` 的 `[[package]]` 今天 520 个，每个任务之后照旧 520 个。锁文件里只有上面几个 crate 的依赖列表可以变。
  - 改了 `Cargo.toml` 之后，第一次编译用 `cargo check --offline --workspace --all-targets`。它失败就停下报告：那说明要下载，不许。
- 依赖方向：
  - `rurge-api` 的正式依赖里没有 `rurge-http`；
  - `rurge-tun`、`rurge-inbound`、`rurge-config` 不依赖 `rurge-http`；
  - `rurge-http` 不依赖 `rurge-engine`。
- `unsafe_code`：不加放开的函数，`crates/rurge-platform/tests/unsafe_functions.rs` 的 `LISTED` 仍是 23 项。

**缺省的 `cargo test` 不碰系统**

- 同 M2a、M2b：不建网卡，不改系统的任何设置；不执行 `netsh`、`route`、`ipconfig`、`ip`、`resolvectl`、`networksetup`。
- **测试从不把任何 CA 装进系统或浏览器的信任库**（总设计 P4-D26、M2-D24）：
  - `rurge mitm ca generate` 在用例里只写进用例的临时目录；
  - 它打印的安装命令，用例与 Pi 都不执行；
  - 测试的客户端是 rustls 的客户端，根只有测试自己的 CA。

**测试的约定**

`CLAUDE.md`「测试约定」整节适用。写用例之前读 `docs/testing.md`「测试约定（全文）」的阶段 4 各节（M1a、M1b、M2a、M2b）。与本计划直接相关的：

- 只用回环与端口 0。监听器绑 `127.0.0.1:0`，不绑 `0.0.0.0` 或 `::`：通配地址怎样映射到回环，由现有的 `connect_addr` 单元用例管。
- 起 `rurge` 二进制的用例一律经 `crates/rurge/tests/cli.rs` 的 `rurge_command()` 或 `rurge_process()`。
  - 给 `--data-dir`；`generate` 不读数据目录，也照样经 `rurge_command()`。
  - 起守护进程照 `mod run` 的 `rurge_run` 与 `spawn_daemon` 一族。
- 等待都有时限。固定等待只许"断言什么都没发生"，注释写它等的是什么、余量多大。
- 新写的、带等待的用例提交前单独循环跑二十次以上（任务 1、5、6）。
- **只在 Unix 上编译的代码与用例**（`#[cfg(unix)]`）在本机编译不到：
  - 写法照 `crates/rurge-dns/src/fakeip/file.rs` 的 `write_temporary`；
  - 用到的 `use` 也放在 `#[cfg(unix)]` 之下，免得 Windows 上多出没用的导入、Unix 上少了导入；
  - 报告里写明"这一段只有 CI 会编译"。
- **内联的测试模块放在文件的末尾**：`crates/rurge/tests/sockets.rs` 读生产代码，读到第一个 `#[cfg(test)] mod … {` 为止。
- 测试里手写的 HTTP/1.1 客户端把每个请求——请求头与正文——在一次写里发出（`CLAUDE.md`）。

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
- 结果照实记：
  - 本机的全量运行是"一台机器上的一次运行，不是 CI"；
  - 需要参照服务端的互操作用例在本机立刻返回、算在通过的数里，要说出来；
  - `#[cfg(unix)]` 的部分本机没有编译。

**仓库的规矩**（`CLAUDE.md`「必须遵守的规矩」整节适用；与本计划直接相关的）

- **凭据**（设计第 13、15 节）：
  - CA 的私钥、`rurge-ca.p12` 的内容、它的口令、`rurge-mitm.conf` 的内容，只写进那两个文件；
  - 它们不进标准输出、标准错误、日志、错误文本与 `Debug`；
  - `export` 与 `GET /v1/mitm/ca` 只给证书。
- **日志的分级**：
  - API 的握手失败、超时、名额满了，只在 DEBUG（带来源地址，来源地址只进 DEBUG）；
  - 没有 CA 而不起 API 的那一行、重载时关于 API 的几行在 WARN。
- **出站套接字只有一处建法**（`crates/rurge/tests/sockets.rs`）：
  - `rurge-api` 的生产代码里 `TcpListener::bind(` 只有一处，`serve` 与 `serve_tls` 共用（P11）；
  - 命令行的 HTTPS 客户端的 `TcpStream::connect` 在那张名单上加一行（任务 6，P14）；
  - 名单之外的原因让那条扫描用例变红，就停下报告。
- **诊断**：`W0040` 在 `codes::hint` 的表里做决定（缺省建议，M2-D22）。
- `rurge-tun` 不改。
- 文档与报告用写文件 / 改文件的工具改。**永远不要用把一个文件的内容经管道写回同一个文件的命令**（2026-10-27 这样写满过 D: 盘）。
- 计划末尾两张表里已写下的行不改写、不删除。追加新行时**不要把已有的行当成锚点替换掉**（M2a 任务 9 这样删掉过一行）。

**写法**

- 注释、日志、错误文本与命令行的输出用英文，照所在文件的写法：文档注释写"为什么"与前提，句子完整。注释里提到设计或计划，写 `phase 4 M2 design 14.1`、`the phase 4 / M2c plan's P10`。
- 提交说明照 `git log` 的样子写，标题用中文：`feat(config): …`、`feat(http): …`、`feat(api): …`、`feat(cli): …`、`test(engine): …`、`docs: …`。正文说做了什么、改了哪些现有用例的期望、为什么。Pi 的提交不带署名行。
- 只在本地提交：不 `push`、不 `pull`、不 `fetch` 远端、不 `amend`、不改写历史。

**不做的事**

- 只运行本仓库构建出来的二进制与 Pi 自己。不下载、不安装任何东西。
- 不手动跑 `rurge run`、`rurge dns lookup`；`rurge service` 只用 `--dry-run`。手动跑 `rurge mitm ca generate` 只许 `--out` 指向工作区里的临时目录，跑完删掉。
- 参考实现：GPL 的（sing-box、mihomo、Xray-core）只取事实，代码不抄。RFC 与各平台工具的文档是公开的事实。
- 不做设计第 1 节「不做」一栏的事，也不做这些：
  - 把 CA 装进任何信任库：总设计 Q5 归项目所有者，M2c 只打印步骤；
  - Windows 上给私密文件设 ACL（M2-D20）；
  - 给 `--remote` 加指定 CA 文件的参数（M2-D21）；
  - API 讲 HTTP/2；
  - 运行中按时检查 CA 的有效期：M2a 计划延后事项第 1 行，归所有者，任务 8 列进设计第 22 节；
  - M3 的事（重写、Map Local）。

**计划没有写到、要做决定的地方**

- 能按设计与总设计的原则定的：自己定，记进修正记录，写"执行者的裁定"。
- 要停下来列出、交给协调者的：
  - 去向写着"项目所有者"的（设计第 22 节）；
  - 会改变 M2-D1 ～ D25 的。
- 现有用例变红：
  - P15 的表点了名的，照计划改，在提交说明里说；
  - 没点名的，停下来，记进修正记录，不顺手改期望值。

## 评审要点

设计没写透、而最可能咬到使用者的几种输入，与负责它们的任务和用例：

1. **凭据漏出来**：`generate` 的标准输出与出错时的消息、`Generated` 的 `Debug`、`export` 与命令行读 CA 失败时的消息。
   - 期望：口令、PKCS#12 的 Base64、私钥都不出现。
   - 用例（任务 2、3、6）：`the_debug_of_a_generated_ca_shows_no_secret`、`mitm_ca_generate_writes_four_files_and_prints_no_secret`、`mitm_ca_export_refuses_a_profile_without_a_usable_ca`、`an_https_endpoint_needs_a_usable_ca`。
2. **覆盖文件，或留下半截**：输出目录里已经有同名文件、目录不在、写到一半失败、读回失败。
   - 期望：不覆盖，不建目录；失败时删掉这一次写出的文件，别的文件不动。
   - 用例（任务 3）：`mitm_ca_generate_does_not_overwrite_or_create_a_directory`、`the_files_written_are_removed_when_one_cannot_be`。
3. **接受连接的循环被握手卡住**：客户端连上不说话、一下子来很多、停止时还在握手。
   - 期望：别的连接照常握手；同时握手至多 64 个，多的直接关；30 秒到了关掉；停止时立刻放掉。
   - 用例（任务 5）：`a_silent_client_does_not_hold_up_the_next`、`handshakes_beyond_the_limit_are_closed`、`a_handshake_that_does_not_finish_in_time_is_closed`、`a_stop_lets_go_of_the_handshakes`。
4. **CA 换了或没了，API 与命令行对不上**：重载换了 CA、重载让 CA 不可用、启动时没有 CA。
   - 期望：新的连接用最后一个生效过的 CA 签，命令行读当前配置里的 CA；没有 CA 时 API 不起、有一行 WARN、不退回明文。
   - 用例（任务 4、6）：`api_tls_is_the_last_ca_a_generation_had`、`without_a_ca_the_api_is_not_started_and_rurge_runs`、`what_a_reload_says_about_the_api`。
5. **证书上的名字对不上**：客户端按 IP 连（rustls 不为 IP 发 SNI）、监听在通配地址时连进来的是 IPv4 映射的地址。
   - 期望：证书带客户端连的那个地址（映射的地址还原成 IPv4）的 SAN；带 SNI 时按 SNI。
   - 用例（任务 5）：`the_certificate_names_the_sni_or_the_local_address`、`a_mapped_local_address_is_named_as_ipv4`。

---

## 文件结构

| 文件 | 任务 | 职责 |
| --- | --- | --- |
| `crates/rurge-config/src/diagnostic.rs`、`crates/rurge-http/src/mitm/authority.rs`、`crates/rurge/tests/cli.rs`、`crates/rurge-engine/tests/pipeline.rs` | 1 | `W0040`；`pipeline.rs` 一条用例的等待 |
| `crates/rurge-http/src/mitm/{generate.rs（新）,mod.rs}`、`crates/rurge-http/Cargo.toml` | 2 | 新 CA 的四个文件的内容、读回的检查、PEM |
| `crates/rurge/src/{main.rs,cli/mod.rs,cli/mitm.rs（新）,cli/control.rs}`、`crates/rurge/Cargo.toml`、`crates/rurge/tests/cli.rs` | 3 | `rurge mitm ca generate` / `export` |
| `crates/rurge-http/src/mitm/{api.rs（新）,decrypt.rs,mod.rs}`、`crates/rurge-engine/src/{engine.rs,lib.rs}`、`crates/rurge-engine/tests/mitm.rs` | 4 | `ApiTls`、`client_config`、`Engine::api_tls` |
| `crates/rurge-api/src/{tls.rs（新）,linger.rs,lib.rs}`、`crates/rurge-api/Cargo.toml`、`crates/rurge-api/tests/api.rs`、`crates/rurge/tests/sockets.rs` | 5 | TLS 的监听器与 `serve_tls` |
| `crates/rurge/src/cli/{run.rs,control.rs,api_client.rs}`、`crates/rurge/Cargo.toml`、`crates/rurge/tests/{cli.rs,run/mitm.rs（新）,sockets.rs}` | 6 | 起 API、重载的告警、命令行的 HTTPS、端到端 |
| `docs/acceptance/phase4-manual.md`（新）、`docs/README.md` | 7 | 手工验收清单 |
| 文档（任务 8 列全） | 8 | M2 的收尾 |

模块的名字与拆法可以照实际调整，调了记进修正记录。

---

## Task 0：开工前核对

**做什么**：对着当时的分支核对下面每一项。有出入的记进「执行期修正记录」，一项一行，写"计划写 X，代码是 Y，按 Y 做"；按记录调整后面的任务。**不改计划的正文。** 没有出入也记一行。

1. **工作树**：`git status --short` 空；当前分支 `phase4-m2`；`cb74b35`（M2b 合并之后）在历史里，之后只多了本计划的提交。
2. **读**：
   - 设计第 23 节（先读）、第 13 ～ 16 节、第 17 节第 6 条、第 18、21、22 节；
   - 本计划的「计划期决定」与 P15 的表；
   - `docs/status.md` 阶段 4 / M2b 一节；
   - M2b 计划「延后事项」第 1 行（任务 1 做它）；M2a 计划「延后事项」第 1 行（任务 8 把它列进设计第 22 节）。
3. **名字与签名**：用 `git grep -n` 逐项找到、记下行号；与「计划期决定」写的不同的照实记。
   - `crates/rurge-config/src/diagnostic.rs`：
     - `codes::W_MITM_CA_UNUSABLE`；
     - `codes::hint` 与它的文档注释；
     - 测试模块的 `WITH_DEFAULT`、`PER_SITE`、`WITHOUT`、`every_code_is_decided_once`。
   - `crates/rurge-config/src/general.rs`：`http_api`、`http_api_tls`。
   - `crates/rurge-config/src/types.rs`：`HostName::parse`。
   - `crates/rurge-config/src/config.rs`：`load`、`Loaded`、`from_text`。
   - `MitmConfig` 的字段与 `Default`。
   - `crates/rurge-http/src/mitm/authority.rs`：
     - `Authority::{load, load_at, ca_der, leaf, sessions, usable_now, signature_count, issue}`；
     - `CaProblem`、`LeafError`、`NO_CA`、`BACKDATE_DAYS`；
     - `check`、`check_with`；
     - 测试模块里断言 `W0039` 的几条（第 1035、1086、1224 行一带）。
   - `crates/rurge-http/src/mitm/decrypt.rs`：
     - `server_config`、`NoCertificate`；
     - `Mitm::handshake` 里克隆模板、换 `cert_resolver` 的那几行；
     - `BuildError` 的来处。
   - `crates/rurge-http/src/mitm/{mod.rs,testing.rs}`：再导出；测试的 `NOW`、`at`、`make_ca`、`ca_p12`、`cfg_with_ca`、`generated`。
   - `crates/rurge-proto`：
     - `keystore::{decode_pkcs12, base64_bytes}`；
     - `testing::p12_base64` 的写法；
     - `testing::TlsFixture::{new, roots, ca_der, ca_p12_base64}`。
   - `crates/rurge-engine`：
     - `src/mitm.rs` 的 `MitmShared::{remember, last_live}`；
     - `src/shared.rs` 的 `mitm`；
     - `src/engine.rs` 的 `mitm_ca_der` 与 `remember` 的两处调用；
     - `src/lib.rs` 的再导出；
     - `tests/mitm.rs` 的 `harness_mitm`、`mitm_section`、`mitm_ca`、`swap_to`、`runtime`、`a_reload_with_another_ca_issues_from_it`。
   - `crates/rurge-engine/tests/pipeline.rs`：`a_dns_session_is_labelled_by_its_scheme_whatever_the_port` 的等待循环。
   - `crates/rurge-api/src/lib.rs`：`ApiContext`、`router`、`serve`、`ServerFuture`。
   - `crates/rurge-api/src/linger.rs`：
     - `Lingering::{new, with_limits, connections}` 与它的 `Listener` 实现；
     - `failed_to_accept`；`Closing`；
     - `Peer` 与它的 `Connected` 实现；
     - `LINGER_MAX`、`LINGER_TIME`。
   - `crates/rurge-api/tests/api.rs`：`Api`、`api_with_extra`、`serve_apart`、`the_mitm_ca_is_given_in_der`、`call_download`。
   - `crates/rurge/src/main.rs`：`enum Command` 与 `main` 的分派；`main` 把 `Err` 打成 `error: …`、以 2 退出。
   - `crates/rurge/src/cli/`：
     - `mod.rs`；`rule.rs`：嵌套子命令的写法；
     - `control.rs`：`ControlArgs`、`connect_addr`、`resolve_endpoint`、`client`、`expect_2xx`、`status_text`，与测试模块的两条 `resolve_endpoint` 用例；
     - `api_client.rs`：`ApiClient::{new, base, get, post, call}`、`ClientError`、`TIMEOUT`；
     - `run.rs`：重载用的 `struct Daemon`、重载里 `http-api changed` 的那一段、`let http_api =`、起 API 的那一段、`Daemon {` 的构造处；有没有测试模块。
   - `crates/rurge/tests/cli.rs`：
     - `rurge_command`、`rurge_process`、`check_names_an_unusable_mitm_ca`；
     - `mod run` 的 `Daemon`、`write_conf`、`rurge_run`、`spawn_daemon`、`spawn_command`、`wait_for_line`、`api_port`、`output_until_it_is_running`、`API_GENERAL`；
     - `rurge stop` 的用例等进程退出的写法；
     - `tests/run/enhanced_mode.rs` 怎样用 `super` 的东西。
   - `crates/rurge/tests/sockets.rs`：`PATTERNS`、`LISTED`（今天 25 行）与 `rurge-api`、`api_client.rs` 的两行。
   - `crates/rurge-dns/src/fakeip/file.rs`：`write_temporary`（`0600` 的写法）。
   - 外部库（注册表源码），行号写进记录：
     - p12-keystore 0.2.1 `src/keystore.rs`：`Certificate::from_der`、`PrivateKeyChain::new`、`KeyStore::{new, add_entry, writer}`、`KeyStoreEntry::PrivateKeyChain`、`Pkcs12Writer::{encryption_algorithm, mac_algorithm, write}` 与两个迭代次数的缺省、`EncryptionAlgorithm::PbeWithShaAnd3KeyTripleDesCbc`、`MacAlgorithm::HmacSha1`；
     - rcgen 0.14.7：`KeyPair::{generate_for, serialize_der}`、`PKCS_ECDSA_P256_SHA256`、`CertificateParams::{self_signed, is_ca, key_usages, not_before, not_after, serial_number, distinguished_name}`、`IsCa::Ca`、`BasicConstraints::Constrained`、`KeyUsagePurpose::{KeyCertSign, CrlSign}`、`DnType::{CommonName, OrganizationName}`；
     - rcgen 0.14.7 `src/certificate.rs`：CA 写 SKI、BasicConstraints 与 KeyUsage 关键的那几处；
     - time 0.3：`Date` 的 `Display` 是 `YYYY-MM-DD`；
     - ring 0.17：`digest::SHA1_FOR_LEGACY_USE_ONLY`、`rand::SystemRandom`；
     - tokio-rustls 0.26.4 `src/server.rs`：`LazyConfigAcceptor::new`、`StartHandshake::{client_hello, into_stream}`；`TlsConnector::connect`；
     - rustls 0.23.43：`server::Acceptor`、`ClientHello::server_name`；
     - axum 0.8.9 `src/serve/listener.rs` 的 `Listener`；`src/serve/mod.rs` 的 `WithGracefulShutdown::run` 在收到停止的信号之后先丢掉监听器、再等连接（第 297 行一带）；`IncomingStream::remote_addr`；
     - hyper 1.11.1：`client::conn::http1::handshake` 与 `SendRequest::send_request`。
4. **数**：
   - `Cargo.lock` 里 `[[package]]` 的个数（520）；
   - `unsafe_functions.rs` 的 `LISTED` 的项数（23）；
   - `sockets.rs` 的 `LISTED` 的行数（25）；
   - 带本机账户名的文件数：派工文件给出要数的那个名字，数出来是 26。
5. **现有用例**（P15 的表）：逐条找到、读它断言什么，把"预计要改 / 预计照过"逐条写进修正记录，一条用例一句。表里没有的照实补一行。另扫一遍：
   - `git grep -n 'http-api-tls'`；
   - `git grep -n 'resolve_endpoint('`；
   - `git grep -n 'ApiClient::new'`；
   - `git grep -n 'rurge_api::serve'`；
   - `git grep -n 'api on http'`。
6. **门禁**：跑一遍，记下测试二进制数、通过数、失败数与忽略数（M2b 收尾时是 91 / 5002 / 0 / 13，一台机器上的一次运行，不是 CI）。

**判据**：修正记录里有任务 0 的行；门禁的数字记下。

---

## Task 1：`W0040` 与 `pipeline.rs` 的等待

**改哪里**：

- `crates/rurge-config/src/diagnostic.rs`：常量、`hint`、测试的 `WITH_DEFAULT`。
- `crates/rurge-http/src/mitm/authority.rs`：`check_with` 与测试模块。
- `crates/rurge/tests/cli.rs`：顶层一条用例，放在 `check_names_an_unusable_mitm_ca` 之后。
- `crates/rurge-engine/tests/pipeline.rs`：一处等待。

**接口**：

- 产出 `codes::W_API_TLS_NO_CA`（`"W0040"`）。
- `rurge_http::mitm::check(config)` 在 P2 的条件下多给一条 `W0040`。

**用例（先红）**

- `authority.rs` 的测试模块，`an_api_that_cannot_start_is_named`。用 `from_text` 加载 Profile，再调 `check_with`，时钟照这个模块别的 `W0039` 用例的写法钉住。各种情形与期望：
  - `[General]` 有 `http-api = k@127.0.0.1:6171`、`http-api-tls = true`，没有 `[MITM]`：
    - 恰好一条诊断，码是 `W0040`、级别是警告；
    - 消息恰好是 P2 的那句，`span` 为 `None`；
    - `hint` 恰好是 P2 的那句。
  - 同上，另有 `[MITM]`，写着读不了的 `ca-p12 = QUJDRA` 与 `ca-passphrase = pw-7d1f`：两条，依次是 `W0039`、`W0040`；两条的消息里都没有 `QUJDRA` 与 `pw-7d1f`。
  - 同上，CA 来自 `[Keystore]` 里 Base64 坏了的一项（加载时已经报了 `E0021`，`CaProblem.reported` 为真）：`check` 只给 `W0040`，没有 `W0039`。
  - 同上，CA 可用（`cfg_with_ca` 一类的夹具）：没有 `W0040`。
  - `http-api-tls = true` 而没有 `http-api`：没有 `W0040`。
  - `http-api-tls = false`（或没写），没有 CA：没有 `W0040`。
- `crates/rurge/tests/cli.rs` 的 `check_says_the_api_does_not_start_without_a_usable_ca`：
  - Profile 是 `[General]\nhttp-api = k@127.0.0.1:0\nhttp-api-tls = true\n[Rule]\nFINAL,DIRECT\n`；
  - 执行 `rurge check -c <它> --data-dir <临时目录>`，退出码 0（只有警告）；
  - 标准输出里有 `W0040`、P2 的消息与缺省建议。
- `diagnostic.rs` 现有的 `every_code_is_decided_once`：加了常量而没有做决定时它变红，这就是它的"先红"。

**实现**

- 常量照 P2，放在 `W_MITM_CA_UNUSABLE` 之后，文档注释写它是什么、在哪里报（`phase 4 M2 design 14.1`）。
- `hint` 加一支，`WITH_DEFAULT` 加它。
- `check_with`：`W0039` 那一段照旧；之后按 P2 的条件推一条 `W0040`，不带 `span`。"CA 用不了"复用同一次 `load_with` 的结果，不再载入第二次。

**`pipeline.rs` 的等待**（M2b 计划延后事项第 1 行，P3）

- `a_dns_session_is_labelled_by_its_scheme_whatever_the_port` 的等待循环里，`find` 的条件从 `r.protocol == Some(protocol)` 改成 `r.protocol == Some(protocol) && r.rule.is_some() && !r.policy.is_empty()`。
- 循环之后的三条断言一个字不改。
- 不要求先红：这是测试自己的竞争。

**判据**

- 新用例先红后绿；
- `pipeline` 那一条单独循环跑二十次（`cargo test -j 8 --workspace --test pipeline -- a_dns_session_is_labelled_by_its_scheme_whatever_the_port`）全绿；
- `Cargo.lock` 520；
- 门禁绿。

---

## Task 2：`rurge_http::mitm::generate`

**改哪里**：

- `crates/rurge-http/Cargo.toml`：P5。
- `crates/rurge-http/src/mitm/generate.rs`：新文件，测试模块在末尾。
- `crates/rurge-http/src/mitm/mod.rs`：模块与再导出；模块的文档注释加一句 `generate`。

**接口**（产出，任务 3、6 用）

```rust
/// How long a generated CA lives: about ten years (phase 4 M2 design 13).
pub const CA_DAYS: i64 = 3650;

/// A new CA and the four files `rurge mitm ca generate` writes (phase 4 M2
/// design 13). `Debug` shows the common name and the end of validity alone.
pub struct Generated {
    pub pem: String,   // rurge-ca.pem
    pub der: Vec<u8>,  // rurge-ca.cer
    pub p12: Vec<u8>,  // rurge-ca.p12
    pub conf: String,  // rurge-mitm.conf
    pub common_name: String,
    pub not_after: time::OffsetDateTime,
    passphrase: String,
}
impl Generated {
    /// Whether `p12` — the file as read back — is a CA rurge can sign with:
    /// `Authority::load` with this CA's passphrase says `Ok(Some(_))`.
    pub fn check(&self, p12: &[u8]) -> Result<(), CaProblem>;
}
pub struct GenerateError(/* why */);  // Display: "cannot generate a CA: {why}"
pub fn generate() -> Result<Generated, GenerateError>;  // generate_at(now_utc)
#[cfg(test)] pub(crate) fn generate_at(now: OffsetDateTime) -> Result<Generated, GenerateError>;
/// The PEM text of a certificate: base64 lines of 64, LF line ends.
pub fn certificate_pem(der: &[u8]) -> String;
```

`mod.rs` 再导出 `generate::{CA_DAYS, GenerateError, Generated, certificate_pem, generate}`。

**用例（先红）**：`generate.rs` 的测试模块，时间用 `mitm::testing` 的 `NOW`。

- `a_generated_ca_is_p256_backdated_and_ten_years_long`：`generate_at(NOW)`，用 x509-parser 读 `der`：
  - subject 恰好是 CN `rurge MITM CA <NOW 的 UTC 日期>` 与 O `rurge`，issuer 与 subject 相同；
  - `not_before == NOW - 2 天`，`not_after == NOW + 3650 天`（秒为单位）；
  - BasicConstraints：关键、`ca` 真、`path_len_constraint == Some(0)`；
  - KeyUsage：关键，恰好 `keyCertSign` 与 `cRLSign`；
  - 有 SKI，没有 AKI；
  - 公钥是 EC P-256（曲线 OID `1.2.840.10045.3.1.7`），签名算法 ecdsa-with-SHA256；
  - 序列号为正，至多 16 字节；
  - `common_name` 与 `not_after` 两个字段与证书一致。
- `the_p12_is_3des_with_a_sha1_mac_and_reads_back`：
  - `p12` 里 3DES 的 PBE 的 OID（字节 `06 0A 2A 86 48 86 F7 0D 01 0C 01 03`）至少出现两处：证书包、密钥包各一处；
  - SHA-1 的 OID（`06 05 2B 0E 03 02 1A`）出现；PBES2 的 OID（`06 09 2A 86 48 86 F7 0D 01 05 0D`）不出现；
  - `g.check(&g.p12)` 是 `Ok`；
  - 把 `p12` 改掉一个字节，`check` 是 `Err`。
- `the_conf_is_a_mitm_section_that_loads`：
  - `conf` 恰好三行：`[MITM]`、`ca-p12 = <Base64>`、`ca-passphrase = <口令>`，以 `\n` 结尾；
  - 口令 24 个字符，都是 ASCII 字母或数字；
  - 把 `conf` 接上 `hostname = a.test\n[Rule]\nFINAL,DIRECT\n`，经 `from_text` 加载、`check`：没有诊断；
  - `Authority::load` 得到 `Some`，它的 `ca_der()` 等于 `der`。
- `a_generated_ca_serves_a_handshake`（设计第 17 节第 6 条）：
  - 用上一条载入的 `Authority` 的 `leaf(&HostName::Domain("a.test"))` 建一个服务端配置；
  - 在 `tokio::io::duplex` 上与一个 rustls 客户端握手，客户端的根只有 `der`、名字 `a.test`；
  - 握手成功。
- `two_cas_differ`：两次 `generate()` 的 `der`、口令与序列号都不同。
- `the_debug_of_a_generated_ca_shows_no_secret`：`format!("{g:?}")` 里没有口令、没有 `p12` 的 Base64 的前 40 个字符、没有 `PRIVATE KEY`；有 `common_name`。
- `certificate_pem_is_the_standard_text`：
  - `certificate_pem(&der)` 以 `-----BEGIN CERTIFICATE-----\n` 开头、以 `-----END CERTIFICATE-----\n` 结尾；
  - 中间每行至多 64 个字符；解回来等于 `der`；
  - `g.pem == certificate_pem(&g.der)`。

**实现**：P4。

**判据**

- 新用例先红后绿；
- `Cargo.lock` 520，锁文件里只有 `rurge-http` 一项的依赖列表变了；
- 门禁绿。

---

## Task 3：`rurge mitm ca generate` / `export`

**改哪里**：

- `crates/rurge/Cargo.toml`：加 `rurge-http.workspace = true`。
- `crates/rurge/src/main.rs`：`Command::Mitm(cli::mitm::MitmArgs)`。文档注释 `Tools for the CA that MITM signs with`。分派到 `cli::mitm::run(args)`。
- `crates/rurge/src/cli/mod.rs`：`pub mod mitm;`。
- `crates/rurge/src/cli/mitm.rs`：新文件，测试模块在末尾。
- `crates/rurge/src/cli/control.rs`：从 `resolve_endpoint` 抽出加载 Profile 的那一段（P7）。
- `crates/rurge/tests/cli.rs`：顶层的几条用例，放在 `check_says_the_api_does_not_start_without_a_usable_ca` 之后。

**接口**

- 消费：任务 2 的 `generate`、`Generated`、`certificate_pem`；`rurge_http::mitm::Authority::load`。
- 产出（任务 6 用）：

  ```rust
  /// The profile `-c` names, read the way the daemon reads it
  /// (`resolve_endpoint`, `rurge mitm ca export`).
  pub(crate) fn load_profile(
      config: &Path,
      platform: Option<Platform>,
      data_dir: Option<&Path>,
  ) -> anyhow::Result<rurge_config::config::Loaded>;
  ```

  `resolve_endpoint` 改成调它，行为不变。

**子命令**：照 `cli/rule.rs` 的嵌套写法。

```text
rurge mitm ca generate --out <DIR>
rurge mitm ca export -c <FILE> [--out <FILE>] [--der] [--platform <P>] [--data-dir <DIR>]
```

`--der` 用 clap 的 `requires = "out"`；`--data-dir` 带 `env = "RURGE_DATA_DIR"`，同 `ControlArgs`。

**用例（先红）**

- `cli/mitm.rs` 的测试模块，`the_files_written_are_removed_when_one_cannot_be`：
  - 给 P6 的 `write_new` 三个路径，第二个已经是一个目录；
  - 期望返回错误；第一个文件被删掉了，第三个没有建；那个目录还在。
- `crates/rurge/tests/cli.rs`：
  - `mitm_ca_generate_writes_four_files_and_prints_no_secret`：
    - `rurge mitm ca generate --out <临时目录>/ca` 成功，目录事先建好；四个文件都在；
    - 标准输出里有四个文件的路径，有 P6 输出里的这几段：
      - `certutil -user -addstore Root`、`security add-trusted-cert`；
      - `update-ca-certificates`、`update-ca-trust`、`trust anchor`、`sql:$HOME/.pki/nssdb`；
      - `Firefox`、`Android`、`iOS`；
    - 从 `rurge-mitm.conf` 里读出口令：标准输出与标准错误里都没有它，也没有 `ca-p12` 那一行 Base64 的前 40 个字符，没有 `PRIVATE KEY`；
    - `rurge-ca.pem` 解出来等于 `rurge-ca.cer`；
    - 把 `rurge-mitm.conf` 接进一份 Profile（另加 `hostname = a.test` 与 `[Rule]`），`rurge check` 成功，输出里没有 `W0039`。
  - `mitm_ca_generate_does_not_overwrite_or_create_a_directory`：
    - `--out` 指向不存在的目录：以 2 退出；标准错误里有这个路径；目录仍不存在。
    - 目录里已有一个 `rurge-ca.p12`（内容 `keep`）：以 2 退出；标准错误里有它的路径；另三个文件没有建；它的内容仍是 `keep`。
  - `#[cfg(unix)] mitm_ca_generate_keeps_the_secrets_private`：`rurge-ca.p12` 与 `rurge-mitm.conf` 的 `mode() & 0o777` 都是 `0o600`。只有 CI 会编译它。
  - `mitm_ca_export_prints_the_certificate`：先 `generate`，再写一份接进了 `rurge-mitm.conf` 的 Profile。
    - `export -c <它> --data-dir <临时目录>`：标准输出是 PEM，解出来等于 `rurge-ca.cer`；
    - `--out <临时目录>/a.pem`：文件内容与上面的标准输出相同；
    - `--out <临时目录>/a.cer --der`：文件字节等于 `rurge-ca.cer`；
    - 再 `--out <临时目录>/a.pem`：以 2 退出，文件不变；
    - `--der` 不带 `--out`：以 2 退出（clap 的用法错误）。
  - `mitm_ca_export_refuses_a_profile_without_a_usable_ca`：
    - 没有 `[MITM]` 的 Profile：以 2 退出，标准错误里有 `names no CA`；
    - `ca-p12 = QUJDRA`、`ca-passphrase = pw-7d1f` 的 Profile：以 2 退出，标准错误里有 `MITM is off`，没有 `QUJDRA` 与 `pw-7d1f`。

**实现**：P6、P7。

**判据**

- 新用例先红后绿（`#[cfg(unix)]` 那一条照实写"本机没有编译"）；
- `Cargo.lock` 520；
- 门禁绿；
- 本任务手动跑过的 `generate` 的输出目录已删掉。

---

## Task 4：`ApiTls` 与 `Engine::api_tls`

**改哪里**：

- `crates/rurge-http/src/mitm/api.rs`：新文件，测试模块在末尾。
- `crates/rurge-http/src/mitm/decrypt.rs`：`server_config` 改成 `pub(crate)`，别的不动。
- `crates/rurge-http/src/mitm/mod.rs`：`pub mod api; pub use api::ApiTls;`，模块的文档注释加一句。
- `crates/rurge-engine/src/engine.rs`：`Engine::api_tls`，放在 `mitm_ca_der` 之后。
- `crates/rurge-engine/src/lib.rs`：`pub use rurge_http::mitm::ApiTls;`。
- `crates/rurge-engine/tests/mitm.rs`：末尾一条用例。

**接口**（产出，任务 5、6 用）

```rust
// rurge_http::mitm::api
/// The TLS of the API (`http-api-tls`, phase 4 M2 design 14.1).
#[derive(Clone)]
pub struct ApiTls { authority: Arc<Authority>, template: Arc<ServerConfig> }
impl ApiTls {
    pub fn new(authority: Arc<Authority>) -> Result<ApiTls, BuildError>;
    /// The configuration of one connection: the template with the
    /// certificate of `name`, from the authority's cache (phase 4 M2 design 6).
    pub async fn config(&self, name: &HostName) -> Result<Arc<ServerConfig>, LeafError>;
    /// The CA certificate (DER) the API's certificates chain to.
    pub fn ca_der(&self) -> &[u8];
}
impl fmt::Debug for ApiTls { /* the authority's Debug alone */ }
/// What a client of the API trusts: this CA alone (`rurge status` over
/// HTTPS, and the tests).
pub fn client_config(ca_der: &[u8]) -> Result<Arc<rustls::ClientConfig>, BuildError>;

// rurge_engine
impl Engine {
    /// The TLS of the API: the authority of the last published generation
    /// that had one (`MitmShared::last_live`), or `None` when none had.
    pub fn api_tls(&self) -> Option<ApiTls>;
}
```

**用例（先红）**

- `api.rs` 的测试模块（CA 用 `mitm::testing` 的夹具）：
  - `the_api_is_served_a_certificate_for_its_name_or_address`：
    - `config(&HostName::Ip(127.0.0.1))` 的配置在 `tokio::io::duplex` 上与 `client_config(ca_der)` 的客户端握手，名字是 `ServerName::IpAddress(127.0.0.1)`：成功；
    - 协商到 `http/1.1`；对端证书的 SAN 是 IP `127.0.0.1`；
    - `config(&HostName::Domain("api.test"))` 对名字 `api.test`：成功，SAN 是 `api.test`。
  - `the_api_template_offers_http1_alone_and_no_early_data`：模板的 `alpn_protocols == [b"http/1.1"]`、`max_early_data_size == 0`。
  - `a_client_trusting_another_ca_is_refused`：客户端信的是另一个 CA，握手失败。
  - `the_api_and_the_decrypted_connections_share_the_leaves`：同一个 `Authority` 上，`ApiTls::config(a)` 两次与 `authority.leaf(a)` 一次之后，`signature_count() == 1`。
  - `client_config_wants_a_certificate`：`client_config(b"not a certificate")` 是 `Err`。
- `crates/rurge-engine/tests/mitm.rs` 的 `api_tls_is_the_last_ca_a_generation_had`：
  - 引擎从带 CA A 的 Profile 建起：`api_tls().map(|t| t.ca_der().to_vec())` 是 A；
  - `swap_to` 一份没有 CA 的 Profile：`mitm_ca_der()` 是 `None`，`api_tls()` 仍是 A；
  - 再 `swap_to` 一份带 CA B 的：是 B；
  - 另起一个从一开始就没有 CA 的引擎：`api_tls()` 是 `None`。

**实现**：P8、P9。

**判据**

- 新用例先红后绿；
- `decrypt.rs` 现有的用例照过；
- `Cargo.lock` 520；
- 门禁绿。

---

## Task 5：API 的 TLS 监听器与 `serve_tls`

**改哪里**：

- `crates/rurge-api/Cargo.toml`：P5。
- `crates/rurge-api/src/tls.rs`：新文件，测试模块在末尾。
- `crates/rurge-api/src/linger.rs`：`Closing::local_addr`。
- `crates/rurge-api/src/lib.rs`：`mod tls;`、共用的绑定、`serve_tls`。
- `crates/rurge-api/tests/api.rs`：一个 `Api` 的方法与两条用例。
- `crates/rurge/tests/sockets.rs`：只改 `rurge-api` 那一行的理由，P11。

**接口**

- 消费：任务 4 的 `ApiTls`（经 `rurge_engine::ApiTls`）、`Engine::api_tls`。单元用例经开发依赖 `rurge_http::mitm::{Authority, api::client_config}`。
- 产出（任务 6 用）：

  ```rust
  /// `serve`, with TLS around every connection (`http-api-tls`, phase 4 M2
  /// design 14.1). The caller has checked `Engine::api_tls` first: without a
  /// CA every connection fails its handshake, and there is no plain HTTP.
  pub async fn serve_tls(
      addr: SocketAddr,
      key: String,
      ctx: ApiContext,
      shutdown: CancellationToken,
  ) -> io::Result<(SocketAddr, ServerFuture)>;
  ```

**用例（先红）**

- `tls.rs` 的测试模块：
  - 监听器建在绑了 `127.0.0.1:0` 的 `Lingering::new` 上；
  - `source` 给一个测试 CA 的 `ApiTls`（`TlsFixture::new(&[]).ca_p12_base64("pw")` 经 `Authority::load`）；
  - 一个任务循环调 `accept`，把拿到的流送进通道；
  - 客户端是 `TcpStream` 加 `tokio_rustls::TlsConnector`（`client_config(ca_der)`）；
  - 每一步都有 5 秒的上限。

  各条用例：
  - `a_connection_is_handed_over_once_its_handshake_is_done`：客户端握手、写 `ping`；通道里来的流读到 `ping`，`Addr` 等于客户端的本地地址。
  - `a_silent_client_does_not_hold_up_the_next`：先连一个不说话的客户端，再连一个握手的；通道里来的是第二个，而第一个还连着。
  - `handshakes_beyond_the_limit_are_closed`：`with_limits(.., max: 2, time: 600 秒)`；两个不说话的客户端先连上；第三个客户端读到 EOF（服务端关了它）。
  - `a_handshake_that_does_not_finish_in_time_is_closed`：`time: 300 毫秒`；不说话的客户端读到 EOF，从连上算起不少于 250 毫秒。
  - `a_stop_lets_go_of_the_handshakes`：`time: 600 秒`；不说话的客户端连上之后取消 `stop`，它读到 EOF。
  - `the_certificate_names_the_sni_or_the_local_address`：
    - 按 `ServerName::IpAddress(127.0.0.1)` 连（rustls 不发 SNI）：成功，证书的 SAN 是 IP `127.0.0.1`；
    - 按 `api.test` 连：成功，SAN 是 `api.test`。
  - `a_mapped_local_address_is_named_as_ipv4`：
    - `name_of(None, "[::ffff:127.0.0.1]:6171")` 是 `HostName::Ip(127.0.0.1)`；
    - `name_of(Some("API.Test"), …)` 是 `HostName::Domain("api.test")`。
  - `without_a_source_the_connection_is_closed`：`source` 返回 `None`，客户端的握手失败。
- `crates/rurge-api/tests/api.rs`：
  - `Api::serve_tls_apart(&self) -> SocketAddr`：同 `serve_apart`，调 `serve_tls`，把服务器的 future `tokio::spawn` 出去。
  - `the_api_is_served_over_tls_with_the_mitm_ca`：引擎照 `the_mitm_ca_is_given_in_der` 带 `TlsFixture` 的 CA；测试自己的 HTTPS 客户端（tokio-rustls 加 hyper 的 `client::conn::http1`）：
    - `GET /v1/outbound`（带 `X-Key`）：200，正文有 `mode`；
    - 协商到 `http/1.1`；对端证书的 SAN 是 IP `127.0.0.1`，签发者是测试 CA；
    - 经 HTTPS 的 `GET /v1/mitm/ca`：DER 等于 CA；
    - 对同一个端口发明文的 `GET /v1/outbound`：读不到以 `HTTP/` 开头的应答（EOF 或别的字节）。
  - `serve_tls_without_a_ca_answers_nothing`：引擎没有 `[MITM]`；`serve_tls` 照样绑上，但客户端的握手失败，也没有明文的应答。

**实现**：P10、P11。

**判据**

- 新用例先红后绿；带等待的单独循环二十次（`cargo test -j 8 --workspace --lib -- tls::`、`cargo test -j 8 --workspace --test api -- tls`）；
- `linger.rs` 与 `api.rs` 现有的用例照过；
- `sockets.rs` 照过，`LISTED` 仍是 25 行；
- `Cargo.lock` 520；
- 门禁绿。

---

## Task 6：起 API、重载的告警、命令行的 HTTPS 与端到端

**改哪里**：

- `crates/rurge/Cargo.toml`：P5。
- `crates/rurge/src/cli/run.rs`：P12、P13；`ApiAtBoot` 与两个函数的单元用例放在测试模块里（没有测试模块就在文件末尾建）。
- `crates/rurge/src/cli/control.rs`：`Endpoint`、`resolve_endpoint`、`client`；改现有的两条用例。
- `crates/rurge/src/cli/api_client.rs`：`ApiClient::with_tls`。
- `crates/rurge/tests/sockets.rs`：`LISTED` 加一行，P14。
- `crates/rurge/tests/cli.rs`：`mod run` 里加 `mod mitm;`。
- `crates/rurge/tests/run/mitm.rs`：新文件，端到端。

**接口**

- 消费：
  - 任务 4 的 `Engine::api_tls`、`rurge_http::mitm::api::client_config`；
  - 任务 5 的 `rurge_api::serve_tls`；
  - 任务 3 的 `load_profile`；
  - `rurge_http::mitm::Authority::load`。
- 产出：

  ```rust
  // cli/control.rs
  /// Where the daemon's API is and how to reach it.
  pub struct Endpoint {
      pub addr: SocketAddr,
      pub key: String,
      /// The MITM CA's certificate (DER) when `http-api-tls` is on: the one
      /// root the API's certificate is checked against (phase 4 M2 design 14.2).
      pub ca: Option<Vec<u8>>,
  }
  pub fn resolve_endpoint(args: &ControlArgs) -> anyhow::Result<Endpoint>;

  // cli/api_client.rs
  impl ApiClient {
      /// HTTPS to `addr`, trusting `ca_der` alone and checking the
      /// certificate against the IP of `addr`.
      pub fn with_tls(addr: SocketAddr, key: String, ca_der: &[u8]) -> anyhow::Result<ApiClient>;
  }
  ```

**用例（先红）**

- `run.rs` 的测试模块，`what_a_reload_says_about_the_api`：P13 的两个函数，表里每一行一个断言，句子逐字比较。
- `control.rs` 的测试模块，`an_https_endpoint_needs_a_usable_ca`。CA 来自 `rurge_http::mitm::generate()`，`conf` 接进 Profile：
  - `http-api-tls = true` 且有可用的 CA：`ca` 是 `Some(der)`；
  - 同一份 Profile，`http-api-tls` 关着：`ca` 是 `None`；
  - `http-api-tls = true`、没有 CA：`Err`，消息是 `W0040` 的那句；
  - `http-api-tls = true`、`ca-p12 = QUJDRA`、`ca-passphrase = pw-7d1f`：`Err`，消息有 `MITM is off`，没有 `QUJDRA` 与 `pw-7d1f`；
  - `--remote` 加 `--key`：`ca` 是 `None`。
- `crates/rurge/tests/run/mitm.rs`。CA 都由 `rurge mitm ca generate --out <临时目录>` 生成；Profile 是 `write_conf(dir, API_GENERAL)` 接上 `http-api-tls = true` 与 `rurge-mitm.conf` 的内容。
  - `status_and_stop_reach_an_api_served_over_https`：
    - 起守护进程，等 `api on https://` 那一行，取端口；
    - `rurge status -c <conf> --data-dir <data>`：成功，标准输出的第一行是 `rurge at https://127.0.0.1:<端口>`；
    - `rurge status --remote 127.0.0.1:<端口> --key k`：以 1 退出，标准错误里有 `cannot reach rurge at http://127.0.0.1:`；
    - `rurge stop -c <conf> --data-dir <data>`：成功；守护进程在有界的时间里退出（照现有 `stop` 用例的等法）。
  - `without_a_ca_the_api_is_not_started_and_rurge_runs`：
    - Profile 开着 `http-api-tls`、没有 `[MITM]`；
    - `output_until_it_is_running` 读到的行里：没有以 `api on` 开头的；有一行带 P12 的那句 WARN；最后是正常的运行摘要。

**实现**：P12、P13、P14。

**判据**

- 新用例先红后绿；`run::mitm` 的用例单独循环二十次（`cargo test -j 8 --workspace --test cli -- run::mitm`）；
- P15 表里任务 6 的几条照表；
- `sockets.rs` 的 `LISTED` 是 26 行；
- `Cargo.lock` 520；
- 门禁绿。

---

## Task 7：手工验收清单

**改哪里**：

- `docs/acceptance/phase4-manual.md`：新建。
- `docs/README.md`：`docs/acceptance/` 那一段加一项。

**写法**：照 `docs/acceptance/phase3-manual.md` 的结构与口气，骨架见 P17。

- 每一项都是一行表格，七栏：`#`、操作、期望、怎么看、结果、日期、系统版本。后三栏留空。
- 期望里引的日志行、API 字段、命令的输出与退出码，逐字取自 `docs/api/phase4.md` 与代码。写进去之前用 `git grep` 核过；核不到的不写，在报告里列出。
- 会改系统设置的项（装 CA、增强模式、系统代理）写明恢复的命令，集中在「改系统设置的项与恢复」一节。
- 不写测试已经证明了的事；写只有真机、真浏览器、真网站、真应用才看得到的。

**判据**

- 骨架里的每一项都在；
- 引的每一句日志、每一个字段在代码或 `docs/api/phase4.md` 里找得到（报告附 `git grep` 的命中）；
- 门禁不用跑（只改文档），报告写明。

---

## Task 8：M2 的收尾文档

**改哪里**（设计第 16、18 节，P18）。照 M2a 任务 9、M2b 任务 6 的写法与规矩：引的提交、用例名与数字对着仓库核过；历史的段落不改写；本地提交，CI 没有见过。

- `docs/api/phase4.md`：加「M2c　CA 工具与 `http-api-tls`」一节。
- `docs/testing.md`：
  - 「命令」加 M2c 的过滤词；
  - 「测试约定（全文）」加阶段 4 / M2c 一节：
    - CA 只在临时目录里生成；
    - 命令行的端到端用 `rurge mitm ca generate` 的输出；
    - `0600` 的用例只在 CI 上编译与运行；
    - API 的 TLS 用例的客户端只信测试的 CA；
    - 握手的名额与时限用 `with_limits` 调小。
- `docs/surge-compatibility-matrix.md`：
  - 第 54 行（1.2 节 `[MITM]`）、第 166 行（2.1 节 `http-api-tls`）、第 585 行（7.2 节"CA 生成"）、第 860 行（10.3 节"rurge 专有"）、第 904 行（10.x 节 API 的 `http-api-tls`）；
  - 第 16 节登记 M2c 的取舍（P18 列着）；
  - 状态标记照规矩，有登记的差异才改：预计不改；
  - 附录照规则整张重数；第 5 行的「最近一次」改成 M2c。
- `README.md`、`README_en.md`（两份一致）：
  - 「当前状态」加阶段 4 的一段：M1 与 M2 做完，本地提交，CI 没有见过，手工验收没有做；
  - 特性表"HTTP 处理"一行；
  - 路线图阶段 4 一行。
- `docs/requirements.md` 第 7 节阶段 4 的「状态」一句：M2 做完的日期、M2 的设计与三份计划的链接；第 7 节的五条验收标准一条也没有验过（手工清单没有做）。FR 的字面不改。
- `docs/status.md`：
  - 阶段 4 下加「### M2c（CA 工具、`http-api-tls` 与收尾）」一节，照 M2b 一节的四段；
  - 第四段"去向写着项目所有者的事"汇总 M2a、M2b、M2c 三份计划「延后事项」里去向是项目所有者的行，与设计第 22 节；
  - 依赖附录加 `rurge → rurge-http`。
- `CLAUDE.md`：只改「现在在哪里」阶段 4 那一条与 CI 那一条：
  - M2c 做完，M2 做完；
  - M1 与 M2 的提交都在本地，CI 都没有见过；
  - 下一步：项目所有者推送（M1 与 M2 一起），读回那次 CI；之后是 M3（重写与 Map Local）的细化设计。
- 设计：
  - 第 22 节的表加一行：M2a 计划延后事项第 1 行，运行中 CA 到期；
  - 第 23 节在"写 M2c 计划时"那几条之后加"执行 M2c 时"：修正记录里改了设计正文意思的行，一条一段；没有就写一句"M2c 执行期没有改设计正文的意思"。
- `docs/README.md`：本计划那一项末尾接一句做完。

**判据**

- 文档里引的提交、用例名与数字对着仓库核过；
- 两份 README 的内容一致；
- 门禁绿。

---

## 计划期决定

写计划时对着 `cb74b35` 核对过下面引的名字与行号。行号会随前面任务的改动漂移，以名字为准；名字不同就照实改、记进修正记录，不改语义。标了"订正设计"的，已随本计划写进设计第 23 节。

| # | 事项 | 决定 |
| - | ---- | ---- |
| P1 | 任务的拆法 | 设计第 2 节的 M2c 大纲（核对；`generate` / `export`；`http-api-tls` 的服务端；命令行的 HTTPS；`W0040`；手工清单；收尾）拆成九个任务：<br>0 核对；<br>1 `W0040`，另带 M2b 延后第 1 行；<br>2 `generate` 的内容（`rurge-http`）；<br>3 两个子命令（可执行文件）；<br>4 `ApiTls` 与引擎；<br>5 API 的监听器；<br>6 起 API、重载、命令行的 HTTPS 与端到端；<br>7 手工清单；<br>8 收尾。<br>`rurge-http`、`rurge-api`、可执行文件各自能单独被拒，所以分开 |
| P2 | `W0040`（订正设计） | 常量 `codes::W_API_TLS_NO_CA = "W0040"`。<br>消息：`http-api-tls is on but there is no usable MITM CA: the API does not start with this profile`。<br>缺省建议：`configure a usable MITM CA, or turn http-api-tls off`（M2-D22），进 `WITH_DEFAULT`。<br>条件：`general.http_api_tls && general.http_api.is_some()`，且 `Authority::load` 是 `Err(_)`（不论 `reported`）或 `Ok(None)`。没有 `http-api` 就没有要起的 API，不报。<br>不带 `span`：`GeneralConfig` 不记 `http-api-tls` 的行，为一条诊断加一个 span 字段不值。<br>设计第 14.1 节的原文是 "…the MITM CA cannot be used: the API will not start"，它漏了"没有写 CA"这一种（`Ok(None)`）；重载时这一条也会报出来，而那时 API 还在用之前的 CA，所以写成"不会以这份 Profile 启动" |
| P3 | `pipeline.rs` 的等待 | M2b 计划延后事项第 1 行。<br>等待的条件加上 `r.rule.is_some() && !r.policy.is_empty()`，断言不改。<br>规则写坏了的回归照样红：等到规则写进去之后断言它的值；规则一直不写进去，就等到时限、报"no … session"。<br>不要求先红 |
| P4 | `generate` 的内容 | **钥匙**：`KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)`。<br>**证书**：`CertificateParams::default()` 之后：<br>• `distinguished_name` 换成新的，只有 CN `rurge MITM CA {now 的 UTC 日期}`（`Date` 的 `Display`）与 O `rurge`；<br>• `is_ca = IsCa::Ca(BasicConstraints::Constrained(0))`；<br>• `key_usages = [KeyCertSign, CrlSign]`。rcgen 0.14.7 因此写关键的 BasicConstraints、关键的 KeyUsage 与 SKI（核过 `src/certificate.rs`）；AKI 不写，`use_authority_key_identifier_extension` 照缺省为假；<br>• `not_before = now - BACKDATE_DAYS 天`（2，同叶证书），`not_after = now + CA_DAYS 天`（3650）；<br>• 序列号照 `Authority::issue`：16 个随机字节，最高位清零；<br>• `self_signed(&key)`。<br>**PKCS#12**：<br>• `PrivateKeyChain::new(key.serialize_der(), 本地密钥标识, [Certificate::from_der(der)])`；<br>• 本地密钥标识是证书 DER 的 SHA-1（`ring::digest::SHA1_FOR_LEGACY_USE_ONLY`，与 OpenSSL 写的一样）；<br>• 别名 `rurge MITM CA`，`KeyStoreEntry::PrivateKeyChain`；<br>• `writer(&口令).encryption_algorithm(EncryptionAlgorithm::PbeWithShaAnd3KeyTripleDesCbc).mac_algorithm(MacAlgorithm::HmacSha1).write()`。<br>• 迭代次数照库的缺省（两个都是 10000，`keystore.rs:232-242`）。<br>• 3DES 与 SHA-1 是设计第 13 节为老的导入方定的。<br>**口令**：24 个字符，取自 `A-Z a-z 0-9` 这 62 个。`ring::rand::SystemRandom` 一次填一块字节，丢掉 ≥ 248 的字节（拒绝采样，248 = 62 × 4），其余对 62 取余。<br>**`conf`**：`format!("[MITM]\nca-p12 = {Base64}\nca-passphrase = {口令}\n")`，Base64 是标准字母表、一行。<br>**PEM**：`certificate_pem` 自己写——标准字母表的 Base64、每行 64 个字符、`\n` 结尾；不用 rcgen 的 `pem()`，因为 `export` 只有 DER，两处用同一个函数。<br>**`check`**：用一份 `MitmConfig { ca_p12: Some(Base64(p12)), ca_passphrase: Some(口令), ..Default::default() }` 调 `Authority::load`，`Ok(Some(_))` 是 `Ok`。`Ok(None)` 不会出现，按 `Err` 处理。<br>**错误**：rcgen、p12-keystore 与随机数的错误都成 `GenerateError`，消息只带库给的原因（不含口令与材料）。<br>**`Debug`**：只有 `common_name` 与 `not_after` |
| P5 | 依赖（订正设计） | 设计第 13 节末条说 `generate` 放在 `rurge-http` 是因为"rcgen 与 p12-keystore 都在那里"，这不准：今天 `rurge-http` 不依赖 `p12-keystore`（`decode_pkcs12` 在 `rurge-proto`，测试的 `p12_base64` 在 `rurge-proto` 的 `testing` 特性里），`base64` 只是它的开发依赖。<br>所以：<br>• `rurge-http` 正式依赖 `p12-keystore`，`base64` 从开发依赖挪进正式依赖（任务 2）；<br>• `rurge-api` 依赖 `rustls`、`tokio-rustls`，开发依赖 `rurge-http`（任务 5）；<br>• 可执行文件依赖 `rurge-http`（任务 3）、`rustls`、`tokio-rustls`（任务 6）。<br>都在锁文件里，包数 520 不变。`rurge-api` 的正式依赖仍只有 `rurge-engine` 与 `rurge-config` 两条工作区边 |
| P6 | `rurge mitm ca generate`（订正设计） | **文件名**：`rurge-ca.pem`、`rurge-ca.cer`、`rurge-ca.p12`、`rurge-mitm.conf`，常量放在 `cli/mitm.rs`。<br>**次序**：<br>① `--out` 不是一个已存在的目录 → `anyhow` 的错误 `{dir} is not a directory; rurge does not create it`；<br>② 四个路径里有任何一个已存在 → `{path} already exists; rurge does not overwrite it`；<br>③ `generate()`；<br>④ `write_new` 依次写 pem、cer、p12、conf；<br>⑤ 读回 `rurge-ca.p12` 交给 `Generated::check`；<br>⑥ 打印。<br>`write_new(files: &[(PathBuf, &[u8], bool)]) -> io::Result<()>`：每个文件 `OpenOptions::new().write(true).create_new(true)`，第三项为真（p12 与 conf）时在 Unix 上另加 `.mode(0o600)`；任何一个写失败，删掉这一次已经写出的，返回那个错误。<br>**读回用 `Authority::load`**，不只 `decode_pkcs12`：设计第 13 节写的是 `decode_pkcs12`，`Authority::load` 解码之外还查它是不是 CA、能不能签，多查几样。<br>**退出码**：<br>• 用法与前提（①②）以 2 退出（`anyhow` 的错误，`main` 打 `error: …`）；<br>• 做的过程中失败（③ ～ ⑤）以 1 退出：先删掉写出的，再 `eprintln!("error: …")`；<br>• ⑤ 的消息是 `{path} does not read back as a CA rurge can sign with: {CaProblem 的消息}; the files written are removed`。<br>**输出**：下面的文字逐字照写，`<…>` 换成值，路径用 `Path::display`；只有 `#[cfg(unix)]` / `#[cfg(not(unix))]` 两句二选一。<br>`wrote a new MITM CA, "<CN>", valid until <not_after 的 UTC 日期>:`<br>`  <dir>/rurge-ca.pem     the certificate (PEM), to be trusted`<br>`  <dir>/rurge-ca.cer     the same certificate (DER)`<br>`  <dir>/rurge-ca.p12     the certificate and its private key (PKCS#12)`<br>`  <dir>/rurge-mitm.conf  a [MITM] section with that PKCS#12 and its passphrase`<br>（空行）<br>Unix：`rurge-ca.p12 and rurge-mitm.conf hold the private key and its passphrase: they were made readable by you alone (0600); keep them that way.`<br>Windows：`rurge-ca.p12 and rurge-mitm.conf hold the private key and its passphrase: rurge sets no permissions of its own on Windows, so keep them in a folder only you can read.`<br>（空行）<br>`to use it: copy the lines of rurge-mitm.conf into your profile (they are a whole [MITM] section; if the profile has one, put ca-p12 and ca-passphrase in it instead), list the hosts to decrypt in hostname, and reload.`<br>（空行）<br>`to trust it (rurge does not install it; run these where the files are):`<br>`  Windows, this user:  certutil -user -addstore Root rurge-ca.cer`<br>`  macOS, every user:   sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain rurge-ca.pem`<br>`  Debian, Ubuntu:      sudo cp rurge-ca.pem /usr/local/share/ca-certificates/rurge-ca.crt && sudo update-ca-certificates`<br>`  Fedora, RHEL:        sudo cp rurge-ca.pem /etc/pki/ca-trust/source/anchors/rurge-ca.pem && sudo update-ca-trust`<br>`  Arch Linux:          sudo trust anchor --store rurge-ca.pem`<br>`  NSS (Chrome and others on Linux): certutil -d sql:$HOME/.pki/nssdb -A -t "C,," -n "rurge MITM CA" -i rurge-ca.pem`<br>`  Firefox: Settings > Privacy & Security > Certificates > View Certificates > Authorities > Import rurge-ca.pem, and trust it to identify websites`<br>`  Android: Settings > Security > Encryption & credentials > Install a certificate > CA certificate > rurge-ca.cer (apps trust it only if they opt in to user CAs)`<br>`  iOS, iPadOS: open rurge-ca.cer on the device, install the profile in Settings, then turn on full trust in Settings > General > About > Certificate Trust Settings`<br>不打印口令、Base64 与私钥（设计第 13、15 节） |
| P7 | `rurge mitm ca export`（订正设计） | **参数**：`-c <FILE>`（必需）、`--out <FILE>`、`--der`（`requires = "out"`）、`--platform`、`--data-dir`。后两个同 `ControlArgs`：托管的 Profile 的 `[MITM]` 在数据目录里的副本里，`export` 与 `status` 读同一份文本。设计第 13 节没有列这两个参数。<br>**读**：`load_profile`（从 `resolve_endpoint` 抽出来的那一段：`LoadOptions` 带 `disk_remote(&data_dir)`），然后 `Authority::load(&config.mitm, &config.keystore)`：<br>• `Ok(Some(a))` → `a.ca_der()`；<br>• `Ok(None)` → 错误 `the profile's [MITM] names no CA (ca-p12 or ca-keystore-name)`；<br>• `Err(p)` → 错误 `p.message`（`W0039` 的消息，设计第 13 节）。<br>Profile 别处的错误不管：只读 `[MITM]` 与 `[Keystore]`。<br>**写**：<br>• 没有 `--out` → 把 `certificate_pem(der)` 打到标准输出，别的什么也不打；<br>• 有 `--out` → 先看它是否已存在（是 → 错误 `{path} already exists; rurge does not overwrite it`）；再以 `create_new` 写 PEM（或 `--der` 时写 DER），之后打印一行 `wrote the CA certificate to {path}`。<br>**退出码**：读不了 Profile、没有 CA、CA 用不了、目标已存在，以 2 退出（`anyhow`）；写失败以 1 退出 |
| P8 | `ApiTls` 与 `client_config` | `ApiTls::new` 的模板就是 `server_config(&authority, false)`：ring 的提供方、TLS 1.3 与 1.2、ALPN 只有 `http/1.1`、`session_storage = authority.sessions()`、不收 0-RTT，与 `h2` 关着时解密的连接同一个。<br>`config(name)`：`authority.leaf(name).await?`，再克隆模板、`cert_resolver = SingleCertAndKey::from(leaf)`，同 `Mitm::handshake`。<br>所以 API 与解密的连接共用同一个叶证书缓存与会话缓存（设计第 14.1 节"同一个 `Authority` 的缓存"）。<br>`client_config(ca_der)`：<br>• `RootCertStore` 只放这一张（`add` 失败 → `BuildError`）；<br>• `ClientConfig::builder_with_provider(ring)`，TLS 1.3 与 1.2，`with_root_certificates(roots)`，`with_no_client_auth()`；<br>• ALPN `http/1.1` |
| P9 | `Engine::api_tls`（订正设计） | `self.shared.mitm.last_live()` 有值时 `ApiTls::new(它)`。`Err` 时 DEBUG 一行 `the TLS of the API cannot be set up: {e}`，返回 `None`。<br>**每条新连接问一次**（P10 的 `TlsSource`），不在启动时取一次存下来：<br>• 重载换了 CA 之后，新的连接用新的 CA 签——命令行经 `-c` 读的正是新的 CA；<br>• 重载让 CA 不可用时，`last_live()` 仍是之前的，API 照旧（设计第 14.1 节）。<br>设计第 14.1 节只说"重载让 CA 不可用时沿用之前的"，没有说换了 CA 时怎样。<br>每条连接建一次模板，不过是一次 `ServerConfig` 的构造，比握手本身便宜得多 |
| P10 | TLS 的监听器 | `crates/rurge-api/src/tls.rs`：<br>• `pub(crate) type TlsSource = Arc<dyn Fn() -> Option<ApiTls> + Send + Sync>;`<br>• `pub(crate) const TLS_HANDSHAKES: usize = LINGER_MAX;`（64）<br>• `pub(crate) const TLS_HANDSHAKE_TIME: Duration = Duration::from_secs(30);`（M2-D21）<br>• `pub(crate) struct TlsListener { inner: Lingering, source: TlsSource, room: Arc<Semaphore>, time: Duration, stop: CancellationToken, tracker: TaskTracker, ready_tx, ready_rx }`<br>• 构造：`new(inner, source, stop)` 用上面两个数；`with_limits(inner, source, stop, max, time)` 给用例。`tracker` 是 `inner.connections()`。`ready_tx` / `ready_rx` 是 `mpsc::channel::<(TlsStream<Closing>, SocketAddr)>(TLS_HANDSHAKES)`。<br>**`accept`**：一个循环里 `tokio::select!`（`biased`）两支：<br>① `Some(ready) = self.ready_rx.recv()` → 返回它；<br>② `(closing, peer) = self.inner.accept()` → `self.room.clone().try_acquire_owned()`：<br>　◦ 拿不到 → DEBUG `http-api: too many TLS handshakes at once; a connection is closed`（带 `%peer`），丢掉 `closing`（照 `Closing` 的规矩逗留之后关）；<br>　◦ 拿到 → 在 `tracker` 上起一个握手的任务。<br>两支都是可取消的：`mpsc::Receiver::recv` 可取消；`Lingering::accept` 等的是 `TcpListener::accept`，也可取消。<br>**握手的任务**（持着名额，直到流交出去或丢掉）：<br>• `local = closing.local_addr()`；<br>• `select!` 停止（`stop.cancelled()` → 什么也不写，丢掉）与 `timeout(time, …)`；<br>• `timeout` 里依次：`LazyConfigAcceptor::new(rustls::server::Acceptor::default(), closing).await` → `name = name_of(start.client_hello().server_name(), local)` → `(self.source)()`（`None` → 失败，原因 `no MITM CA to serve TLS with`）→ `tls.config(&name).await` → `start.into_stream(config).await`；<br>• 成功 → `select!` 停止与 `ready_tx.send((stream, peer))`，送不出去（监听器已丢）就丢掉；<br>• 失败 → DEBUG `http-api: a TLS handshake failed`（带 `%peer`、`error = %e`）；<br>• 超时 → DEBUG `http-api: a TLS handshake did not finish in time`（带 `%peer`）。<br>axum 0.8.9 收到停止的信号之后先丢掉监听器、再等连接（`serve/mod.rs:297`），所以停止时 `ready_tx.send` 失败，握手的任务也随停止的令牌结束，`connections().wait()` 等得到它们。<br>**`name_of(sni: Option<&str>, local: SocketAddr) -> HostName`**：有 SNI → `HostName::parse(sni)`；没有 → `HostName::Ip(local.ip().to_canonical())`（监听在 `::` 时 IPv4 的客户端连进来是映射的地址）。<br>**`local_addr`**：`self.inner.local_addr()`。<br>**`Peer`**：在 `tls.rs` 里另实现 `Connected<IncomingStream<'_, TlsListener>>`，取 `*stream.remote_addr()`，同 `Lingering` 那一份。<br>**`Closing::local_addr(&self) -> io::Result<SocketAddr>`**：流已交给逗留的任务时返回 `io::ErrorKind::NotConnected`。<br>丢掉 TLS 流时不发 `close_notify`（设计第 14.1 节）；hyper 平静地关连接时经 `poll_shutdown` 发它，照 tokio-rustls |
| P11 | `serve_tls`（订正设计） | 设计第 3 节只说 `rurge-api` 有"`http-api-tls` 的监听器"。本计划：<br>• `serve` 的签名与行为不变；<br>• 另加 `pub async fn serve_tls(addr, key, ctx, shutdown)`，`TlsSource` 是 `{ let engine = ctx.engine.clone(); Arc::new(move \|\| engine.api_tls()) }`，`stop` 是 `shutdown` 的克隆；<br>• 绑定与"不是回环"的 WARN 抽成一个私有的 `async fn bind(addr) -> io::Result<(TcpListener, SocketAddr)>`，两个函数共用，所以生产代码里 `TcpListener::bind(` 仍只有一处；<br>• 之后两个函数各自建监听器、`axum::serve(…).with_graceful_shutdown(…)`、等连接，同今天的 `serve`。<br>`sockets.rs` 里 `rurge-api` 那一行的理由改成 `` `serve` and `serve_tls`: the HTTP API's listener, which does not go out ``。<br>`serve_tls` 自己不查有没有 CA：调用方先问 `Engine::api_tls`（P12）。没有 CA 时它照样绑上，每条连接的握手都失败，从不讲明文 |
| P12 | 起 API | `run.rs` 在 `let http_api = …` 旁边记 `let http_api_tls = cfg.general.http_api_tls;`。起 API 的那一段：<br>• `http_api_tls` 且 `engine.api_tls().is_none()` → `tracing::warn!("http-api-tls is on but there is no usable MITM CA: the API is not started (it never falls back to plain HTTP)")`，不打印 `api on`，照常运行；<br>• `http_api_tls` 且有 CA → `serve_tls`；<br>• 否则 → `serve`，同今天。<br>起来了：打印 `api on https://{addr}` 或 `api on http://{addr}`，INFO 的 `http-api listening` 多一个字段 `tls = http_api_tls`。<br>绑不上照旧 `eprintln!` 并以 1 退出。<br>`api_running` 记下 API 起没起，交给重载（P13） |
| P13 | 重载时说的话（订正设计） | `struct ApiAtBoot { access: Option<ControllerAccess>, tls: bool, running: bool }`。重载用的 `Daemon` 的 `http_api` 换成 `api: &'a ApiAtBoot`。<br>`fn api_changes(boot: &ApiAtBoot, access: &Option<ControllerAccess>, tls: bool) -> Vec<&'static str>`，在今天那一段 `http-api changed` 的位置调，每一句 WARN 一行：<br>• `access` 变了：<br>　◦ `running` → `http-api changed in the profile; the API keeps its current address and key until rurge restarts`（今天的句子）；<br>　◦ `!running` 且 `boot.access` 有 → `http-api changed in the profile; the API is not started until rurge restarts`；<br>　◦ `boot.access` 没有 → `http-api was added to the profile; the API is not started until rurge restarts`（今天的句子）；<br>• `boot.access` 有且 `tls` 变了：<br>　◦ `running` 且 `boot.tls` → `http-api-tls changed in the profile; the API keeps serving HTTPS until rurge restarts`；<br>　◦ `running` 且 `!boot.tls` → `http-api-tls changed in the profile; the API keeps serving plain HTTP until rurge restarts`；<br>　◦ `!running` → `http-api-tls changed in the profile; the API is not started until rurge restarts`。<br>`fn api_ca_note(boot: &ApiAtBoot, has_ca: bool) -> Option<&'static str>`，在新的一代换上之后调，`has_ca = engine.mitm_ca_der().is_some()`：<br>• `running && tls && !has_ca` → `http-api-tls is on but this profile has no usable MITM CA: the API goes on with the last CA it had`；<br>• `!running && boot.access.is_some() && boot.tls && has_ca` → `the profile has a usable MITM CA now: the API that http-api-tls kept from starting starts when rurge restarts`；<br>• 别的 → `None`。<br>今天"`d.http_api.is_some()` 就是 API 在跑"的前提在 `http-api-tls` 没有 CA 时不再成立，所以改看 `running`。<br>设计第 14.1 节只写了"重载让 CA 不可用时 WARN 一行" |
| P14 | 命令行的 HTTPS | `resolve_endpoint`：<br>• `--remote` → `ca: None`（设计第 14.3 节）；<br>• 否则 `load_profile`，`http_api` 照旧；<br>• `general.http_api_tls` 为真时 `Authority::load(&config.mitm, &config.keystore)`：<br>　◦ `Ok(Some(a))` → `ca: Some(a.ca_der().to_vec())`；<br>　◦ `Ok(None)` → 错误，消息是 P2 那句；<br>　◦ `Err(p)` → 错误，消息是 `p.message`。<br>　这几种错误以 2 退出，同 `NOT_CONFIGURED`。<br>`client(args)`：`ca` 有 → `ApiClient::with_tls(addr, key, &ca)`，否则 `ApiClient::new`。<br>`ApiClient` 里加一个私有的 `enum Transport { Plain(Client<HttpConnector, Full<Bytes>>), Tls { addr: SocketAddr, connector: TlsConnector, name: ServerName<'static> } }`：<br>• `base` 是 `https://{addr}`；`name` 是 `ServerName::IpAddress(addr.ip().into())`——rustls 不为 IP 发 SNI，服务端按本地地址签（P10）；<br>• `connector` 来自 `rurge_http::mitm::api::client_config(ca_der)`；<br>• `call` 的请求：明文照旧用绝对形式的 URI；TLS 用源站形式（`path`），另加 `Host: {addr}`；<br>• TLS 的一次调用：`TcpStream::connect(addr)` → `connector.connect(name, tcp)` → `hyper::client::conn::http1::handshake(TokioIo::new(tls))` → 连接的 future `tokio::spawn` → `send_request`。<br>• 一次调用一条连接：命令行一条命令只有几次调用，不值得复用。<br>• 5 秒的总时限与 `seen_status` 照旧包住整个交换（连接、握手与读正文都在里面），错误照旧映射成 `ClientError`。<br>`sockets.rs` 的 `LISTED` 加一行：`("crates/rurge/src/cli/api_client.rs", "TcpStream::connect", "the process of `rurge status` / `reload` / `stop` over `http-api-tls`, not the daemon")`，按字母序放，常量的长度 25 → 26。提交说明里写这一行与理由（`CLAUDE.md`「出站套接字只有一处建法」） |
| P15 | 现有的用例 | 见下面的表。表里"预计照过"的若红，停下报告，不改期望 |
| P16 | 测试的 CA、客户端与源站 | • `rurge-http` 的单元用例：CA 用 `mitm::testing` 的夹具，或任务 2 的 `generate_at`。<br>• `rurge-api` 与引擎的用例：CA 用 `TlsFixture::new(&[])` 的 `ca_p12_base64("pw")` 与 `ca_der()`。<br>• 命令行的用例：CA 用 `rurge mitm ca generate` 写进临时目录的文件；`control.rs` 的单元用例直接调 `rurge_http::mitm::generate()`。<br>• 客户端一律是 rustls 的，根只有那一个 CA。在内存里握手用 `tokio::io::duplex`，经网络的只用回环 |
| P17 | 手工清单的骨架 | 照 `docs/acceptance/phase3-manual.md`：<br>**标题与引言**：覆盖 M1 与 M2，M3 ～ M6 的项随各自的里程碑加。<br>**`## 怎么用`**：<br>• `### 先做哪一节`：M1 在前；M2 先做"生成与安装 CA"；<br>• `### 共用的前置条件`：`cargo build --release`、一份带 `http-listen`、`socks5-listen`、`http-api`、`loglevel` 的 Profile 模板、要用的浏览器与 `curl`；增强模式的项照阶段 3 清单的前置条件；<br>• `### 改系统设置的项与恢复`：装 CA 的各平台命令（P6 的输出）与卸掉的命令，例如：<br>　◦ Windows `certutil -user -delstore Root "<CN>"`；<br>　◦ macOS `sudo security delete-certificate -c "<CN>" /Library/Keychains/System.keychain`；<br>　◦ Debian 删掉 `/usr/local/share/ca-certificates/rurge-ca.crt` 再 `sudo update-ca-certificates --fresh`；<br>　◦ Fedora 删掉 anchors 里那个文件再 `sudo update-ca-trust`；<br>　◦ Arch `sudo trust anchor --remove rurge-ca.pem`；<br>　◦ NSS `certutil -d sql:$HOME/.pki/nssdb -D -n "rurge MITM CA"`；<br>　◦ Firefox、Android、iOS 在设置里删。<br>　另有增强模式与系统代理的恢复，指到阶段 3 清单；<br>• `### 第 7 节的五条验收标准各由哪几项证明`：只有"主流站点 MITM 解密成功且证书固定站点给出提示"有 M2 的项，其余四条写"M3 ～ M6 之后才有项"。<br>**`## M1　会话入口与明文 HTTP 引擎`**：前置条件；三个平台各几项与"与平台无关"：<br>• 真浏览器经 HTTP 代理、SOCKS5、增强模式三种入口上明文 HTTP 的站点：每个请求一条记录，`method`、`url`、`statusCode`；`GET /v1/rurge/http` 的计数；<br>• 443 上按 SNI 分流（`DOMAIN` 规则对 HTTPS 站点生效，不解密）；<br>• 同一条 keep-alive 连接上两个请求按 `URL-REGEX` 各走各的策略（`curl` 一次给两个 URL）；<br>• `force-http-engine-hosts`；<br>• 非标准端口的明文 HTTP 被嗅探到。<br>**`## M2　MITM`**：<br>• 前置条件：用 `rurge mitm ca generate` 生成，按它打印的步骤装 CA，`[MITM] hostname` 写几个真站点；<br>• 三个平台各一组：装 CA；真浏览器（Windows 的 Edge 或 Chrome、macOS 的 Safari、Linux 的 Firefox 与 Chrome）经三种入口打开解密的真站点：记录是 `https://` 的 URL、协议 `HTTPS`、浏览器里看到的证书由 rurge 的 CA 签发；`h2 = true` 时浏览器开发者工具里协议是 `h2`，`h2 = false` 时是 `http/1.1`；`curl --http1.1` 与 `--http2` 各一次；从浏览器打开 `http://127.0.0.1:6171/v1/mitm/ca?x-key=…` 下载到 `rurge-ca.cer`；`http-api-tls = true` 时 `rurge status -c` 走 HTTPS、`--remote` 连不上；卸掉 CA；<br>• "与平台无关"：<br>　◦ 证书固定：`curl --pinnedpubkey` 加上站点真实公钥的哈希，经 rurge 打开解密的站点 → curl 拒绝；rurge 的 WARN 提示与记录的 `noRequest`；<br>　◦ 源站证书不受信：`expired.badssl.com`、`self-signed.badssl.com` → 502 与原因；`skip-server-cert-verify = true` 时通；<br>　◦ `hostname-disabled`、排除项、`client-source-address`、`:0` 与写明的端口；<br>　◦ Android 与 iOS 装 CA，经局域网里 rurge 的 HTTP 代理上网；<br>　◦ `rurge check` 对用不了的 CA 给 `W0039`、对 `http-api-tls` 而没有 CA 给 `W0040`。<br>编号：Windows `W1…`、macOS `M1…`、Linux `L1…`、与平台无关 `P1…`。M1 与 M2 两节各自从 1 编起，前面加节名区分，照阶段 3 清单 |
| P18 | 收尾文档的清单 | 见任务 8。第 16 节要登记的 M2c 的取舍：<br>• Windows 上私密文件不设 ACL（M2-D20）；<br>• `--remote` 对开了 `http-api-tls` 的实例连不上（M2-D21）；<br>• 丢掉 TLS 流时不发 `close_notify`；<br>• API 讲 HTTPS 时 CA 随重载换（P9）；<br>• `rurge mitm ca generate` 不装 CA（总设计 Q5）；<br>• API 的叶证书与解密的连接共用缓存（P8）。<br>两份 README 与需求文档照设计第 18 节在 M2c 收尾时改 |

### P15 的表：要改的、要留意的现有用例

| 用例 | 为什么 | 任务 | 预计 |
| ---- | ------ | ---- | ---- |
| `crates/rurge-config/src/diagnostic.rs` 的 `every_code_is_decided_once` 与查缺省建议的那两条 | 多一个码 | 1 | 改：`WITH_DEFAULT` 加 `W0040`（先红） |
| `crates/rurge-http/src/mitm/authority.rs` 测试模块里断言 `W0039` 的几条（第 1035、1086、1224 行一带） | `check_with` 多一条 `W0040` | 1 | 照过：它们的 Profile 没开 `http-api-tls` |
| `crates/rurge/tests/cli.rs` 的 `check_names_an_unusable_mitm_ca`、`check_says_what_to_do_about_a_problem` | 同上 | 1 | 照过 |
| `rurge-config` 的语料库快照（`tests/corpus/valid/kitchen-sink.conf` 写着 `http-api-tls = false`） | 同上 | 1 | 照过：`W0040` 在 `rurge-http` 里，不在加载器里，开关也关着 |
| `crates/rurge-engine/tests/pipeline.rs` 的 `a_dns_session_is_labelled_by_its_scheme_whatever_the_port` | M2b 延后第 1 行 | 1 | 改：只改等待的条件，断言不改 |
| `crates/rurge-http/src/mitm/decrypt.rs` 测试模块的全部 | `server_config` 改成 `pub(crate)` | 4 | 照过 |
| `crates/rurge-engine/tests/mitm.rs` 里重载换 CA 的几条（`a_reload_with_another_ca_issues_from_it`、`a_reload_that_breaks_the_ca_leaves_open_connections_decrypted`） | 引擎只多一个方法 | 4 | 照过 |
| `crates/rurge-api/src/linger.rs` 测试模块的全部 | `Closing::local_addr` | 5 | 照过 |
| `crates/rurge-api/tests/api.rs` 的全部 | `serve` 改用共用的 `bind` | 5 | 照过 |
| `crates/rurge/tests/sockets.rs` 的 `every_outbound_socket_is_made_in_one_place` | `rurge-api` 仍一处绑定；命令行的 HTTPS 客户端多一处 `TcpStream::connect` | 5、6 | 任务 5：照过，只改那一行的理由；任务 6：改，`LISTED` 加一行（25 → 26） |
| `crates/rurge/src/cli/control.rs` 的 `endpoint_resolution_prefers_flags_then_config`、`a_managed_profile_names_its_api_in_the_copy_in_the_data_directory` | `resolve_endpoint` 返回 `Endpoint` | 6 | 改：元组的解构改成字段，断言的值不变 |
| `control.rs` 里 `status_text` 的用例（`rurge at http://127.0.0.1:6171`） | `base` | 6 | 照过：明文照旧 `http://` |
| `crates/rurge/tests/cli.rs` 里读 `api on http://` 的 `api_port` 与用它的用例、`output_until_it_is_running` 的用例（第三行是 `api on`） | 明文的 API 照旧打印 `api on http://` | 6 | 照过 |
| `cli.rs` 里 `rurge status` / `reload` / `stop` 的用例 | `ApiClient::new` 不变 | 6 | 照过 |
| `cli.rs` 里看重载告警的用例（若有） | `http-api changed` 的条件改看 `running` | 6 | 照过：明文的 API 起来时 `running` 为真，句子不变 |

---

## 执行期修正记录

下面写"执行者的裁定"的，是 Pi 在设计与计划的原则下定的；写"协调者的裁定"的，是协调的会话代项目所有者定的。项目所有者都可以改。计划的正文不改：正文里说得不对的地方，以这张表的行为准。

| # | 任务 | 内容 | 提交 |
| - | ---- | ---- | ---- |
| 1 | 0 | 开工前核对（第 1、2 项）：`git status --short` 空；分支 `phase4-m2`；`HEAD` 是 `290b310`；`git log --oneline cb74b35..HEAD` 只有 `290b310 docs(plan): 阶段 4 / M2c（CA 工具、http-api-tls 与收尾）实施计划`，与派工写的一致，`cb74b35` 是它的祖先。读了设计第 23 节（先读）、第 13 ～ 16 节、第 17 节第 6 条、第 18、21、22 节，本计划的 P1 ～ P18 与 P15 的表，`docs/status.md` 阶段 4 / M2b 一节，M2b 计划「延后事项」第 1 行（任务 1 做它）与 M2a 计划「延后事项」第 1 行（任务 8 把它列进设计第 22 节） | 本提交 |
| 2 | 0 | 名字与签名（第 3 项）对得上的，`rurge-config`：`diagnostic.rs:278` 的 `W_MITM_CA_UNUSABLE = "W0039"`（文档注释在 `:276-277`）、`:330` 的 `pub fn hint(code: &str) -> Option<&'static str>`（`W0039` 在 `WITHOUT` 里，`:491`）、测试模块的 `WITH_DEFAULT`（`:420`）、`PER_SITE`（`:461`）、`WITHOUT`（`:468`）与 `every_code_is_decided_once`（`:523`，它读 `diagnostic.rs` 里 `pub mod codes {` 到第一个 `#[cfg(test)]` 之间的常量）；`general.rs:193` 的 `http_api: Option<ControllerAccess>`、`:194` 的 `http_api_tls: bool`；`types.rs:14` 的 `HostName::parse(s: &str) -> HostName`（去首尾空白、剥一层方括号、转小写、去掉末尾的点；`::ffff:` 的还原靠 `canonical`）；`config.rs:159` 的 `LoadOptions`（`environment`、`platform`、`capabilities`、`remote: Option<Arc<dyn RemoteSource>>`）、`:430` 的 `Loaded { pub config, pub diagnostics }`、`:450` 的 `load(path: &Path, opts: &LoadOptions) -> Result<Loaded, LoadError>`、`:497` 的 `from_text(text: &str, path: &Path, opts: &LoadOptions) -> Loaded`；`mitm.rs:25` 的 `MitmConfig` 字段与 `:62` 的 `Default`（三张名单都以 443 起，`auto_quic_block` 缺省真） | 本提交 |
| 3 | 0 | 名字与签名对得上的，`rurge-http` 与 `rurge-proto`：`authority.rs:139` 的 `pub fn load(cfg: &MitmConfig, keystore: &[KeystoreItem]) -> Result<Option<Authority>, CaProblem>`（内部调 `default_clock()`）、`:148` 的 `load_at`（`#[cfg(test)] pub(crate)`）、`:342` 的 `usable_now`、`:348` 的 `ca_der`、`:405` 的 `sessions`、`:431` 的 `signature_count`（`#[cfg(test)]`）、`:438` 的 `pub(crate) fn issue(&self, name: &HostName, now: OffsetDateTime) -> Result<Issued, rcgen::Error>`（序列号的五行在 `:462-471`：16 个空字节、`SystemRandom::fill`、`serial[0] &= 0x7f`、`SerialNumber::from_slice`）、`:66` 的 `BACKDATE_DAYS: i64 = 2`、`:69` 的 `NO_CA`、`:75` 的 `pub(crate) type Clock`、`:86` 的 `CaProblem { message, span, reported }`（没有 `Display`，只有 `Debug` 与 `.message`）、`:525` 的 `LeafError`（`:536` 的 `Display` 是 `cannot issue a certificate for {name}: {why}`）、`:685` 的 `pub fn check(config: &Config) -> Diagnostics` 与 `:688` 的 `check_with(config, clock)`；`decrypt.rs:340` 的 `fn server_config(authority: &Arc<Authority>, h2: bool) -> Result<Arc<ServerConfig>, BuildError>`（今天还是私有的 `fn`）、`:362` 的 `struct NoCertificate`、`:237` 的 `async fn handshake`（克隆模板与换 `cert_resolver` 在 `:252-254`）、`BuildError` 来自 `:20` 的 `use rurge_proto::BuildError`；`mitm/mod.rs` 的再导出（`authority::{Authority, BACKDATE_DAYS, CaProblem, LEAF_DAYS, LEAF_SLOTS, LeafError, SESSION_SLOTS, check}` 与 `decrypt::{Accepted, Decision, HandshakeFailure, Mitm, OriginTls, Watch, source_of}`）；`mitm/testing.rs` 的 `NOW`（`:22`，`1_798_761_600`）、`DAY`（`:23`）、`at`（`:25`）、`clock_at`（`:29`）、`TestCa`（`:33`）、`ca_params`（`:39`）、`make_ca`（`:48`）、`generated`（`:56`）、`fixture_key`（`:63`）、`ca_p12`（`:72`）、`cfg_with_ca`（`:81`），都是 `pub(crate)`；`rurge-proto/src/keystore.rs:23` 的 `base64_bytes(text: &str) -> Option<Vec<u8>>`、`:43` 的 `decode_pkcs12(label: &str, der: &[u8], password: &str) -> Result<ClientIdentity, BuildError>`；`rurge-proto/src/testing/tls.rs:361` 的 `p12_base64(chain: &[Vec<u8>], key_pkcs8: &[u8], password: &str) -> String`（别名 `client`、本地密钥编号 `[1,2,3,4]`、缺省的 PBES2/AES-256 与 SHA-256 MAC，所以任务 2 不能照抄它）、`TlsFixture::{new, with_ca_name, roots, ca_der, ca_p12_base64}`（`:59`、`:67`、`:98`、`:125`、`:138`） | 本提交 |
| 4 | 0 | 名字与签名对得上的，`rurge-engine`、`rurge-api` 与可执行文件：`engine/src/mitm.rs:38` 的 `MitmShared`、`:51` 的 `remember`、`:59` 的 `last_live`；`shared.rs:213` 的 `pub mitm: Arc<MitmShared>`；`engine.rs:769` 的 `mitm_ca_der`、`:270` 与 `:675` 的两处 `shared.mitm.remember(...)`；`lib.rs:39` 的 `pub use mitm::{MitmCounts, MitmShared}`；`outbounds.rs:409` 的 `rurge_http::mitm::check(&loaded.config)`（在 `:406` 起的 `load_checked` 里）；`engine/tests/mitm.rs:24` 的 `mitm_section`、`:33` 的 `mitm_ca`、`:132` 的 `harness_mitm`、`:146` 的 `leaf_of`、`:1869` 的 `swap_to`、`:1878` 的 `a_reload_with_another_ca_issues_from_it`；`engine/tests/common/mod.rs:82` 的 `runtime(dir, profile, shared)`；`engine/tests/pipeline.rs:2155` 的等待循环（`:2232-2248`）；`rurge-api/src/lib.rs:27` 的 `ApiContext { engine, control, load_options, sources }`、`:51` 的 `ServerFuture`、`:57` 的 `pub(crate) fn router(key: String, ctx: ApiContext) -> Router`、`:133` 的 `serve`（`:139` 是 `rurge-api` 唯一一处 `TcpListener::bind(`，`:148` 的 `Lingering::new`）；`linger.rs:88` 的 `Lingering`、`:94` 的 `new`、`:98` 的 `with_limits`、`:114` 的 `connections`、`:119` 的 `impl Listener`、`:156` 的 `failed_to_accept`、`:171` 的 `Closing`、`:179` 的 `stream()`、`:230` 的 `Drop`、`:294` 的 `Peer` 与 `:296` 的 `Connected`、`:36`/`:40`/`:45` 的 `LINGER_TIME`/`LINGER_BYTES`/`LINGER_MAX`；`main.rs:19` 的 `enum Command` 与 `:40` 的 `main`（`Err` 打成 `error: {e}` 后 `ExitCode::from(2)`）；`cli/rule.rs:27` 的 `RuleArgs`、`:33` 的 `RuleCommand`、`:197` 的 `run`；`cli/service.rs:96` 的 `carry_out`（`:108` 以 1 退出）；`cli/control.rs:17` 的 `ControlArgs`（`--platform` 是 `Option<Platform>`，`--data-dir` 带 `env = "RURGE_DATA_DIR"`）、`:45` 的 `StatusArgs`、`:55` 的 `connect_addr`、`:65` 的 `resolve_endpoint`（今天返回 `(SocketAddr, String)`）、`:100` 的 `client`、`:106` 的 `expect_2xx`、`:199` 的 `status_text`、`:278`/`:297`/`:350` 的三条用例；`cli/api_client.rs` 的 `TIMEOUT`（`:16`）、`ApiClient`（`:18`）、`ClientError`（`:24`）、`new`（`:55`）、`base`（`:63`）、`get`/`post`/`call`（`:67`/`:71`/`:75`）；`cli/run.rs:467` 的 `struct Daemon`、`:533-543` 的 `http-api changed` 那一段、`:1648` 的 `let http_api =`、`:1819-1838` 起 API 的那一段、`:1913` 的 `Daemon {` 构造处 | 本提交 |
| 5 | 0 | 名字与签名对不上的，照代码做、不改语义：① 没有 `KeystoreConfig` 这个类型，`Config` 里是 `pub keystore: Vec<KeystoreItem>`（`crates/rurge-config/src/config.rs:242`），`Authority::load` 的第二个参数就是 `&[KeystoreItem]`（任务 3、4、6 照这个写）。② `server_config` 今天还是私有的 `fn`（`decrypt.rs:340`），任务 4 照 P15 改成 `pub(crate)`。③ `Authority::leaf` 的接收者是 `self: &Arc<Self>`（`authority.rs:366`，设计第 23 节第 12 条），任务 4 的 `ApiTls::config` 因此要持 `Arc<Authority>`。④ `Lingering::with_limits` 是私有的 `fn`（`linger.rs:98`）；任务 5 的 `TlsListener::with_limits` 是它自己的，不共用。⑤ `crates/rurge/src/cli/run.rs` 今天没有测试模块（文件 2544 行，一处 `#[cfg(test)]` 也没有），任务 6 在文件末尾新建。⑥ `crates/rurge-api/tests/api.rs` 今天的客户端是 hyper-util 的 `legacy::Client`（`call_raw`、`call_download` 在 `:363`），没有 `client::conn::http1` 的辅助，任务 5 要自己写一个。⑦ `crates/rurge-http/src/mitm/decrypt.rs` 的测试模块只有 `tokio::io::duplex` 上的 `handshake`（`:512`），没有取对端证书、读 SAN 的辅助（那两样在 `authority.rs` 的 `leaf_of` 与 `extension`、以及 `engine/tests/mitm.rs:146` 的 `leaf_of` 里），任务 4 的用例要自己写 | 本提交 |
| 6 | 0 | 数（第 4 项）：`Cargo.lock` 的 `[[package]]` 520 个；`crates/rurge-platform/tests/unsafe_functions.rs:18` 的 `LISTED` 声明 23、实数 23；`crates/rurge/tests/sockets.rs:58` 的 `LISTED` 声明 25、实数 25（`rurge-api/src/lib.rs` 的 `TcpListener::bind(` 一行在首位，`rurge/src/cli/api_client.rs` 的 `build_http(` 一行在末段）；带本机账户名的文件 26 个（数出来只用于这个数，这个名字没有写进仓库里的任何文件） | 本提交 |
| 7 | 0 | 现有用例（第 5 项，P15 的表），任务 1 与任务 4 的：`diagnostic.rs:523` 的 `every_code_is_decided_once` 今天把 `WITH_DEFAULT`/`PER_SITE`/`WITHOUT` 与 `codes` 里声明的常量逐一对照，`W0040` 不在三张表里时它红，所以先红、再进 `WITH_DEFAULT`（`:585` 的 `a_default_hint_is_one_plain_sentence` 要求建议句小写开头、不带句号、单行、ASCII，P2 的那句满足）；`authority.rs:896` 的 `every_ca_problem_is_named_and_quotes_no_secret`（`:1035` 断言只有一条）、`:1063` 的 `no_ca_warns_only_when_hosts_are_listed`（`:1086` 断言条数等于 `usize::from(warned)`）、`:1214` 的 `a_problem_is_reported_at_the_line_of_the_key_that_names_the_ca`（`:1224` 断言码表恰是 `[W0039]`）三条的 Profile 都没有 `[General] http-api`，`W0040` 不会多出来，照过；`crates/rurge/tests/cli.rs:251` 的 `check_names_an_unusable_mitm_ca` 与 `:284` 的 `check_says_what_to_do_about_a_problem` 只用 `contains` 定位 `W0039` 与 `W0001`，照过；语料库快照的 `tests/corpus/valid/kitchen-sink.conf:26` 写着 `http-api-tls = false`，`W0040` 在 `rurge-http` 里、不在加载器里，照过；`decrypt.rs` 测试模块的全部在 `server_config` 改成 `pub(crate)` 时照过 | 本提交 |
| 8 | 0 | 现有用例，任务 5 与任务 6 的：`engine/tests/mitm.rs:1878` 与 `:1938` 两条重载用例只经 `Engine`，引擎只多一个方法，照过；`linger.rs` 测试模块的全部在加 `Closing::local_addr` 时照过；`rurge-api/tests/api.rs` 的全部在 `serve` 抽出共用的 `bind` 时照过（`serve` 的签名与行为不变）；`sockets.rs` 的 `every_outbound_socket_is_made_in_one_place` 任务 5 只改 `rurge-api` 那一行的理由、`LISTED` 仍 25 行，任务 6 加 `api_client.rs` 的 `TcpStream::connect` 一行（25 → 26）；`control.rs:297` 的 `endpoint_resolution_prefers_flags_then_config` 与 `:350` 的 `a_managed_profile_names_its_api_in_the_copy_in_the_data_directory` 各解构两次元组（`:316`、`:327`、`:332`、`:369`），任务 6 换成字段、断言的值不变；`control.rs:278` 的 `status_text_matches_the_design` 传的是 `http://127.0.0.1:6171`，明文照旧，照过；`cli.rs:2824` 的 `api_port` 等 `api on http://`（用它的有 `:3775` 起那一族与 `:5866` 的 `up`/`:5872` 的 `api_up`）、`:3351` 断言 `api on http://127.0.0.1:`，明文的 API 照旧打印，照过；`cli.rs` 里 `rurge status`/`reload`/`stop` 的用例经 `ApiClient::new`，不变，照过；看重载告警的用例一条也没有（`http-api changed` 与 `http-api was added` 两句今天只出现在 `run.rs:538`、`:540`，没有用例断言它们），P15 那一行的“若有”是没有 | 本提交 |
| 9 | 0 | 补扫（第 5 项，表里没有的）：`git grep -n 'http-api-tls'` 只命中 `general.rs:1041`、`:1457`、`engine/src/mitm.rs:57`（注释）与 `tests/corpus/valid/kitchen-sink.conf:26`（`false`），没有语料库 Profile 写着 `true`（语料库在仓库根的 `tests/corpus`，由 `crates/rurge-config/tests/corpus.rs` 读，不在 `crates/rurge-config/tests` 下）。`resolve_endpoint(` 的解构只有 `control.rs:101`（`client`）与那三条用例；`ApiClient::new` 只有 `control.rs:102`。`rurge_api::serve(` 只有 `run.rs:1827`（`rurge-api/tests/api.rs` 里直接调 `serve`，在 `:116`、`:138`、`:286`）。`Lingering::new(` 只有 `rurge-api/src/lib.rs:148`。`Daemon {` 不带 `..` 的构造只有 `run.rs:1913`（`cli.rs` 里同名的结构是测试自己的）。`crates/rurge-engine/src/outbounds.rs:871` 的 `load_checked_reports_an_unusable_mitm_ca` 断言诊断码表恰是 `[W0039]`，它的 Profile 没有 `http-api`，`W0040` 不会出现，照过（这一条 P15 的表里没有，任务 1 别把它弄红） | 本提交 |
| 10 | 0 | 协调者的裁定（任务 0 的派工单）：测试的时刻与真的时钟。核查过：`mitm::testing::NOW` 是 `1_798_761_600`，即 2027-01-01T00:00:00Z，在今天之后；`Authority::load` 用 `default_clock()`（`authority.rs:139-144`），`check` 也用（`:685-687`），`check_with` 的时钟由调用方给（测试给 `clock_at(NOW)`）。所以 `generate_at(NOW)` 生成的 CA 其 `notBefore` 是 2026-12-30，用真的时钟读回会得到 `is not valid until 2026-12-30`（`authority.rs:277-282` 的检查）。任务 2 因此：只有钉日期的 `a_generated_ca_is_p256_backdated_and_ten_years_long` 用 `generate_at(NOW)`（它只读 `der`、不 `load`）；`the_p12_is_3des_with_a_sha1_mac_and_reads_back`、`the_conf_is_a_mitm_section_that_loads`、`a_generated_ca_serves_a_handshake`、`two_cas_differ`、`the_debug_of_a_generated_ca_shows_no_secret`、`certificate_pem_is_the_standard_text` 一律用 `generate()` | 本提交 |
| 11 | 0 | p12-keystore 0.2.1（任务 2）：`p12-keystore-0.2.1/src/keystore.rs` 的 `Certificate::from_der(der: &[u8]) -> Result<Self>`（`:28`）、`PrivateKeyChain::new<K: AsRef<[u8]>, D: AsRef<[u8]>, I: IntoIterator<Item = Certificate>>(key, local_key_id, chain) -> Self`（`:74`）、`KeyStore::new()`（`:141`）、`add_entry(&mut self, alias: &str, entry: KeyStoreEntry)`（`:261`）、`writer<'a, 'b>(&'a self, password: &'b str) -> Pkcs12Writer<'a, 'b>`（`:232`）、`KeyStoreEntry::PrivateKeyChain(PrivateKeyChain)`（`:114`）、`Pkcs12Writer::{encryption_algorithm(EncryptionAlgorithm) -> Self, encryption_iterations(u64) -> Self, mac_algorithm(MacAlgorithm) -> Self, mac_iterations(u64) -> Self, write(self) -> Result<Vec<u8>>}`（`:329`、`:335`、`:341`、`:347`、`:353`）。两个迭代次数的缺省都是 10000（`:238`、`:240`）；`writer` 的缺省算法是 PBES2/AES-256 与 SHA-256 MAC，所以 P4 那两个显式设置是必须的。`EncryptionAlgorithm::PbeWithShaAnd3KeyTripleDesCbc`（`:296`）与 `MacAlgorithm::HmacSha1`（`:313`）都在，都不需要另外打开什么；写 3DES 要库的 `pbes1` 特性（`Cargo.toml` 的 `[features] default = ["pbes1"]`；`src/codec.rs:182-205` 的 `encrypt` 那一支在 `#[cfg(feature = "pbes1")]` 下，`#[cfg(not(feature = "pbes1"))]` 时落成 `UnsupportedEncryptionScheme`），工作区的依赖是 `p12-keystore = "0.2"`（根 `Cargo.toml:77`）、缺省特性开着。写 PKCS#12 时选的算法同时用在证书包（`src/codec.rs:482` 的 `cert_bags_to_auth_safe` 造 `EncryptedData`）与密钥包（`src/codec.rs:442` 的 `private_key_to_safe_bag` 造 `ShroudedKeyBag`），所以 3DES 的 PBE OID 至少两处、PBES2 的 OID 不出现、SHA-1 的 OID 出现在 MAC 上——任务 2 的三条断言成立 | 本提交 |
| 12 | 0 | rcgen 0.14.7（任务 2）：`KeyPair::generate_for(alg: &'static SignatureAlgorithm) -> Result<Self, Error>`（`rcgen-0.14.7/src/key_pair.rs:96`，`generate()` 就是它对 `PKCS_ECDSA_P256_SHA256`）、`serialize_der(&self) -> Vec<u8>`（`:438`）；`CertificateParams` 的 `not_before`、`not_after`（都是 `OffsetDateTime`）、`serial_number: Option<SerialNumber>`、`distinguished_name: DistinguishedName`、`is_ca: IsCa`、`key_usages: Vec<KeyUsagePurpose>`、`use_authority_key_identifier_extension: bool`（`src/certificate.rs:56-78`），它的 `Default` 是 `not_before` 1975-01-01、`not_after` 4096-01-01、`is_ca: NoCa`、`key_usages` 空、`use_authority_key_identifier_extension` 假、`distinguished_name` 里有一个 CN `rcgen self signed cert`（`:81-108`；P4 的“换成新的”因此是必须的；`TlsFixture` 用 `CertificateParams::new` 造 CA，有效期就是 1975 ～ 4096，任务 5 的用例用真的时钟也读得回）；`DistinguishedName::{new, push(ty: DnType, s: impl Into<DnValue>)}`（`src/lib.rs:479`、`:508`）、`DnType::{CommonName, OrganizationName}`（`src/certificate.rs:713-728`）；`SerialNumber::from_slice(bytes: &[u8]) -> SerialNumber`（`src/lib.rs:878`）；`self_signed(&self, signing_key: &impl SigningKey) -> Result<Certificate, Error>`（`src/certificate.rs:154`）；CA 写 SKI（不关键，`:543-553`）、BasicConstraints（关键，`:558`）、KeyUsage（关键，`:222`） | 本提交 |
| 13 | 0 | ring 0.17 与 time 0.3（任务 2）：`ring::digest::digest(algorithm: &'static Algorithm, data: &[u8]) -> Digest`（`ring-0.17.14/src/digest.rs:290`）、`SHA1_FOR_LEGACY_USE_ONLY`（`:403`）、`rand::SystemRandom::new()`（`src/rand.rs:114`）与 `SecureRandom::fill(&self, dest: &mut [u8]) -> Result<(), error::Unspecified>`（`:25`）；`time::Date` 的 `Display` 是 `YYYY-MM-DD`（年份至少 4 位，`time-0.3.45/src/date.rs:1409-1445` 的 `SmartDisplay`） | 本提交 |
| 14 | 0 | tokio-rustls 0.26.4 与 rustls 0.23.43（任务 4、5、6）：`LazyConfigAcceptor::new(acceptor: rustls::server::Acceptor, io: IO) -> Self`（`tokio-rustls-0.26.4/src/server.rs:81`），它作为 future 的输出是 `Result<StartHandshake<IO>, io::Error>`（`:135-139`）；`StartHandshake::{client_hello(&self) -> rustls::server::ClientHello<'_>, into_stream(self, config: Arc<ServerConfig>) -> Accept<IO>}`（`:217`、`:221`），`Accept` 的输出是 `io::Result<TlsStream<IO>>`（`:282`）；`TlsConnector` 由 `impl From<Arc<ClientConfig>>` 建（`src/client.rs:116`），`connect<IO>(&self, domain: ServerName<'static>, stream: IO) -> Connect<IO>`（`:40`），名字的类型是 `ServerName<'static>`；`rustls::server::Acceptor` 有 `Default`（`rustls-0.23.43/src/server/server_conn.rs:827-835`），`ClientHello::server_name(&self) -> Option<&str>`（`:157`）；`ServerConfig` 的 `session_storage`（`:323`）、`cert_resolver`（`:334`）、`alpn_protocols`（`:338`）、`max_early_data_size`（`:366`）四个字段都是 `pub`；`RootCertStore::add(&mut self, der: CertificateDer<'_>) -> Result<(), Error>`（`src/webpki/anchors.rs:70`）；`ClientConfig::builder_with_provider(...).with_protocol_versions(...)` 的现成写法在 `crates/rurge-net/src/http.rs:255-257`；`rustls::pki_types::ServerName::IpAddress(IpAddr)` 收的是 `pki_types::IpAddr`，从 `std::net::IpAddr` 由 `impl From` 转（`rustls-pki-types-1.15.1/src/server_name.rs:420`），仓库里现成的写法是 `ServerName::IpAddress((*ip).into())` | 本提交 |
| 15 | 0 | axum 0.8.9（任务 5）：`src/serve/listener.rs` 的 `pub trait Listener: Send + 'static`，`type Io: AsyncRead + AsyncWrite + Unpin + Send + 'static`、`type Addr: Send`、`fn accept(&mut self) -> impl Future<Output = (Self::Io, Self::Addr)> + Send`、`fn local_addr(&self) -> io::Result<Self::Addr>`（`:11-27`）；`WithGracefulShutdown::run` 收到停止的信号之后先 `drop(close_rx)`、再 `drop(listener)`、之后才等连接（`src/serve/mod.rs:296`、`:297`、`:303`），P10 的“先丢掉监听器、再等连接”成立；`IncomingStream::remote_addr(&self) -> &L::Addr`（`:442`）；`pub trait Connected<T>: Clone + Send + Sync + 'static { fn connect_info(stream: T) -> Self }`（`src/extract/connect_info.rs:80-83`） | 本提交 |
| 16 | 0 | hyper 1.11.1（任务 5、6）：`hyper::client::conn::http1::handshake<T, B>(io: T) -> crate::Result<(SendRequest<B>, Connection<T, B>)>`（`hyper-1.11.1/src/client/conn/http1.rs:148`），`T: Read + Write + Unpin`、`B: Body + 'static`、`B::Data: Send`、`B::Error: Into<Box<dyn StdError + Send + Sync>>`；`SendRequest::send_request(&mut self, req: Request<B>) -> impl Future<Output = crate::Result<Response<IncomingBody>>>`（`:228`）；`hyper_util::rt::TokioIo` 在工作区开的特性下可用（根 `Cargo.toml:29` 的 hyper 开了 `client`、`server`、`http1`、`http2`，`:30` 的 hyper-util 开了 `tokio`），现成的用法在 `crates/rurge-policy/src/probe.rs:110` | 本提交 |
| 17 | 0 | 依赖的现状（第 8 项）：`rurge-http` 的 `[dependencies]` 已有 rcgen（`x509-parser` 特性）、x509-parser、time、lru、rustls、tokio-rustls、sha2、ring、h2，`base64` 只在 `[dev-dependencies]`，没有 `p12-keystore`——与 P5 说的一致；`rurge-api` 的 `[dependencies]` 没有 rustls 与 tokio-rustls、`[dev-dependencies]` 没有 rurge-http，而有 hyper、hyper-util、http-body-util、bytes、x509-parser 与带 `testing` 的 rurge-proto——任务 5 要加的三项都在计划里；可执行文件 `rurge` 的 `[dependencies]` 没有 rurge-http、rustls、tokio-rustls（任务 3、6 加），hyper、hyper-util、http-body-util、bytes 在 `[dependencies]` 里（集成用例也够用），`x509-parser` 一处也没有。计划和派工都没有写到、要另加的一项：`base64` 不是 `rurge` 的依赖，而任务 3 在 `crates/rurge/tests/cli.rs` 要解 PEM（`mitm_ca_generate_writes_four_files_and_prints_no_secret` 比较 `rurge-ca.pem` 与 `rurge-ca.cer`、`mitm_ca_export_prints_the_certificate` 比较标准输出与文件），要么在用例里手写解码（`cli.rs` 里已有手写的 Base64 编码 `basic_credentials`，`:2790` 一带），要么给 `rurge` 加一条 `base64` 的开发依赖——后者只让锁文件里 `rurge` 那一项的依赖列表多一行，包数仍是 520。任务 6 的用例按任务 6 的说明只跑二进制、不读证书，所以不用 `x509-parser` | 本提交 |
| 18 | 0 | 门禁（第 6 项）：`cargo fmt --all --check` 退出码 0（`RUSTFMT` 指向 gnu 工具链的 rustfmt.exe，日志 `gate-task-0-fmt.log`）、`cargo clippy --all-targets -- -D warnings` 退出码 0（`gate-task-0-clippy.log`）、`cargo test -j 8 --workspace --no-fail-fast` 退出码 0（`gate-task-0.log`）：91 个结果行、5002 通过、0 失败、13 忽略，与派工给的基线（91 / 5002 / 0 / 13）逐项相同；没有 `STATUS_HEAP_CORRUPTION` 或 `STATUS_ACCESS_VIOLATION`，没有重跑。这是一台 Windows 11 x86-64 开发机上的一次运行，不是 CI；需要参照服务端的互操作用例在本机立刻返回、算在通过的数里 | 本提交 |
| 19 | 1 | 协调者的裁定（任务 1 的派工单）：`W0040` 的那句消息只写一处，常量 `API_TLS_NO_CA` 放在 `crates/rurge-http/src/mitm/authority.rs` 的 `NO_CA` 旁边，`mitm/mod.rs` 的 `pub use authority::{…}` 照字母序加上它；任务 6 的命令行经 `rurge_http::mitm::API_TLS_NO_CA` 用同一句，不另写一份。`check` 的文档注释与 `authority.rs`、`mitm/mod.rs` 的模块注释同时补上 `W0040`（P2） | 本提交 |
| 20 | 1 | 执行者的裁定：`an_api_that_cannot_start_is_named` 的“CA 可用”那一种，断言失败时打的是一个固定的标签（`a profile whose CA loads warns about nothing`），不是派工单要的那份 Profile 的文字——那份文字里有 `ca-p12`，而它是这个用例当场生成、装着私钥的 PKCS#12。另外五种情形的文字里只有 `QUJDRA` 与 `pw-7d1f` 两个假值，照派工单带 `{text}` | 本提交 |
| 21 | 1 | 门禁（任务 1）：`cargo fmt --all --check` 退出码 0（`gate-task-1-fmt.log`）、`cargo clippy --all-targets -- -D warnings` 退出码 0（`gate-task-1-clippy.log`）、`cargo test -j 8 --workspace --no-fail-fast` 退出码 101（`gate-task-1.log`）：90 个结果行、4810 通过、0 失败、13 忽略，只有一个目标失败——`-p rurge --test cli` 的二进制以 `0xc0000374`（`STATUS_HEAP_CORRUPTION`）退出，`CLAUDE.md`「这台开发机」记的那一种；单独重跑 `cargo test -j 8 --workspace --test cli`（`gate-task-1-cli-rerun.log`）退出码 0、194 通过、0 失败、0 忽略，两次数合起来是 91 个结果行、5004 通过、0 失败、13 忽略（基线 91 / 5002 / 0 / 13 加本任务的两条）。`Cargo.lock` 仍 520（本任务不改任何 `Cargo.toml`） | 本提交 |
| 22 | 2 | 协调者的记录（任务 2 的派工单）：任务 1 的全量里 `cli` 的测试二进制崩溃之后留下四个 `rurge run` 守护进程，占着输出管道，`cargo test … > 日志` 那条命令在 cargo 结束之后迟迟不返回；协调者结束了它们，`dispatch-rules.md` 的「门禁」一节加了认出与结束它们的办法 | 本提交 |
| 23 | 2 | 协调者的裁定（任务 2 的派工单）：`generate()` 取 `OffsetDateTime::now_utc()` 并 `replace_nanosecond(0)` 再交给同一段实现，证书里的时刻只到秒，`Generated.not_after` 与证书里的逐秒相等；`Generated::check` 的 `Authority::load` 若答 `Ok(None)`（不会出现），用结构体字面量造一个 `CaProblem`，消息 `MITM is off: the PKCS#12 holds no CA`，不为此改 `CaProblem::new` 的可见性（P4） | 本提交 |
| 24 | 2 | 执行者的裁定：任务 2 的接口写 `generate()` 调 `generate_at`，而 `generate_at` 是 `#[cfg(test)]` 的，非测试构建里调不到；于是两者都调同一个私有的 `new_ca(now)`（照 `Authority::load` 与 `load_at` 的样子），`generate_at` 仍是测试用的入口、签名不变 | 本提交 |
| 25 | 2 | 门禁（任务 2）：`cargo fmt --all --check` 退出码 0（`gate-task-2-fmt.log`）、`cargo clippy --all-targets -- -D warnings` 退出码 0（`gate-task-2-clippy.log`）、`cargo test -j 8 --workspace --no-fail-fast` 退出码 101（`gate-task-2.log`）：90 个结果行、5000 通过、0 失败、13 忽略，只有一个目标失败——`-p rurge-engine --test outbounds_hysteria2` 的二进制在 11 条都打印 `ok` 之后以 `0xc0000374`（`STATUS_HEAP_CORRUPTION`）退出，`CLAUDE.md`「这台开发机」记的那一种；单独重跑 `cargo test -j 8 --workspace --test outbounds_hysteria2`（`gate-task-2-hysteria2-rerun.log`）退出码 0、11 通过、0 失败、0 忽略，两次数合起来是 91 个结果行、5011 通过、0 失败、13 忽略（基线 91 / 5004 / 0 / 13 加本任务的七条）。`Cargo.lock` 仍 520，锁文件里只有 `rurge-http` 那一项的依赖列表多一行 `"p12-keystore"`；全量跑完之后没有留下 `rurge.exe` 进程 | 本提交 |
| 26 | 3 | 协调者的裁定（任务 3 的派工单）：`base64` 进 `rurge` 的**开发依赖**（`tests/cli.rs` 要解 PEM，不手写解码器）；`generate` ⑥ 的 Unix 与 Windows 两句用 `cfg!(unix)` 二选一，两支本机都编译得到（所以下面只列 `restrict` 与那条用例为“只有 CI 会编译”）；④⑤ 的两句消息写成 `cannot write {path}: {e}` 与 `{path} cannot be read back: {e}; the files written are removed`（⑤ 的 `check` 不过那一句照 P6） | 本提交 |
| 27 | 3 | 执行者的裁定：① P6 的 `write_new` 返回 `io::Result<()>`，而 `io::Error` 不带路径，所以失败时返回的错误是 `io::Error::new(e.kind(), "{path}: {e}")`，调用方打 `error: cannot write {e}`，得到 P6 的 `cannot write {path}: {e}`；这个包装写在私有的 `named(path, e)` 里。② 另外两个私有辅助：`remove_files(&[&Path])`（删除时的错误被忽略）与 `announce(&Generated, &Path)`（⑥ 的那段输出，`cfg!(unix)` 两支都编译）。③ `mitm.rs` 末尾的测试模块里只有 `the_files_written_are_removed_when_one_cannot_be`（派工单没要 `write_new` 的其它用例） | 本提交 |
| 28 | 3 | 门禁（任务 3）：`cargo fmt --all --check` 退出码 0（`gate-task-3-fmt.log`）、`cargo clippy --all-targets -- -D warnings` 退出码 0（`gate-task-3-clippy.log`）、`cargo test -j 8 --workspace --no-fail-fast` 退出码 0（`gate-task-3.log`）：91 个结果行、5016 通过、0 失败、13 忽略（基线 91 / 5011 / 0 / 13 加本任务的五条：`--bins` 的一条与 `tests/cli.rs` 的四条）；没有 `STATUS_HEAP_CORRUPTION`/`STATUS_ACCESS_VIOLATION`，没有重跑，跑完没有留下 `rurge.exe`。`Cargo.lock` 仍 520 个包，只有 `rurge` 那一项的依赖列表多 `base64` 与 `rurge-http` 两行。本机没有编译的：`crates/rurge/src/cli/mitm.rs:264-270` 的 `#[cfg(unix)] fn restrict`（`options.mode(0o600)`）与 `crates/rurge/tests/cli.rs:474` 的 `#[cfg(unix)] mitm_ca_generate_keeps_the_secrets_private`，第一次编译它们的是 CI。手动跑过一次 `rurge mitm ca generate --out <工作区里的临时目录>`，输出逐行对过 P6，之后删掉了那个目录（它打印的安装命令一条也没有执行） | 本提交 |
| 29 | 4 | 执行者的裁定（P8 没有写的）：`client_config` 里 `ClientConfig::builder_with_provider(...).with_protocol_versions(...)` 失败时的 `BuildError` 消息用 `cannot set up TLS: {e}`，与同一个 crate 的 `server_config` 的写法一致；`RootCertStore::add` 失败的消息照 P8 的 `the MITM CA cannot be trusted: {e}` | 本提交 |
| 30 | 4 | 门禁（任务 4）：`cargo fmt --all --check` 退出码 0（`gate-task-4-fmt.log`）、`cargo clippy --all-targets -- -D warnings` 退出码 0（`gate-task-4-clippy.log`）。第一次 `cargo test -j 8 --workspace --no-fail-fast` 退出码 101（`gate-task-4.log`）：`-p rurge-engine --test http` 那个测试二进制以 `STATUS_HEAP_CORRUPTION`（0xc0000374）退出、一行结果也没有写（本机管控软件注入的钩子 DLL，`CLAUDE.md`「这台开发机」），单独重跑它得到 36 通过。之后整个门禁又跑了一遍，退出码 0（`gate-task-4-retry.log`）：91 个结果行、5022 通过、0 失败、13 忽略（基线 91 / 5016 / 0 / 13 加本任务的五条单元用例与一条引擎用例），没有 `STATUS_HEAP_CORRUPTION`/`STATUS_ACCESS_VIOLATION`，跑完没有留下 `rurge.exe`。`Cargo.lock` 仍 520 个包，本任务不改 `Cargo.toml`。本机没有编译的代码：没有（本任务没有 `#[cfg(unix)]` 的东西） | 本提交 |
| 31 | 5 | 执行者的裁定：① P11 说两份相同的收尾可以抽成一个私有的小函数、也可以各写一遍——抽了，但只抽 `stopped(result, connections)`（ERROR 那一行、`connections.close()` 与 `wait()`）；`axum::serve(…).with_graceful_shutdown(…)` 那三行两个函数各写一遍，因为抽它要给泛型的 `Listener` 写一面 `L: Listener + …`、`L::Addr: Debug` 的约束墙，比重复三行难读。② `tls.rs` 测试模块的 `Rig` 用 `Lingering::connections()` 这个跟踪器当“这条连接已经被接受了”的信号：它从一个连接被接受起就数它，而握手任务也记在同一个跟踪器上，所以一条连接让 `len()` 长 2；用例因此写 `!is_empty()` / `>= n`，不写 `== n`（第一版写 `== 1`，三条用例因此等满 5 秒变红）。③ `name_of` 是私有的：只有一个调用方与它自己的单元用例；`TLS_HANDSHAKES` 与 `TLS_HANDSHAKE_TIME` 照 P10 落在 `tls.rs`。④ `without_a_source_the_connection_is_closed` 起初在空壳下是绿的（`accept` 里的 `todo!()` 让对端的握手也失败，与这条用例要盖的分支不是一回事），改成先等连接被接受、再做握手之后，空壳下才红 | 本提交 |
| 32 | 5 | 门禁（任务 5）：`cargo fmt --all --check` 退出码 0（`gate-task-5-fmt.log`）、`cargo clippy --all-targets -- -D warnings` 退出码 0（`gate-task-5-clippy.log`）、`cargo test -j 8 --workspace --no-fail-fast` 退出码 0（`gate-task-5.log`）：91 个结果行、5032 通过、0 失败、13 忽略（基线 91 / 5022 / 0 / 13 加 `tls.rs` 的八条与 `api.rs` 的两条），没有 `STATUS_HEAP_CORRUPTION`/`STATUS_ACCESS_VIOLATION`，跑完没有留下 `rurge.exe`。`Cargo.lock` 仍 520 个包，`rurge-api` 的依赖列表只多 `rurge-http`、`rustls`、`tokio-rustls` 三行。本机没有编译的代码：没有（本任务没有 `#[cfg(unix)]` 的东西）。带等待的五条（`tls.rs`）与 `--test api` 的两条各循环跑二十次全过（`task-5-loop-tls.log`、`task-5-loop-api.log`） | 本提交 |
| 33 | 5 | 修正轮 1（协调者评审任务 5 之后）：覆盖第 31 行 ② 的“用例因此写 `!is_empty()` / `>= n`”。① `TLS_HANDSHAKES` 的文档改照实：它限的是同时在握手的连接，超出的当场关，然后照 `Closing` 的规矩逗留（至多 `LINGER_MAX` 条、每条至多 `LINGER_TIME`），所以一个反复连接、从不握完手的客户端同时占 `TLS_HANDSHAKES + LINGER_MAX` 条连接（今天 64 + 64 = 128），不是 `TLS_HANDSHAKES` 一条；与 `LINGER_MAX` 同数就写着它的来历（P10），不编理由；握完手交给 `axum` 的连接不在这个数里。② `handshakes_beyond_the_limit_are_closed`：跟踪器对一条正在握手的连接恰好数两个（`accept` 给的 token 与同一次 `accept` 里同步起的握手任务，两者之间没有 `await`），所以第二条之后等 `len() >= 4`（“both connections to be in their handshake”），第三条之后不再等（两个名额都占着，它被当场关掉，`assert_closed` 自己有 5 秒的上限）；第一处照旧写 `!rig.connections.is_empty()`（等价于 `>= 2`），这个取舍记在这里。旧的第二处 `>= 2` 在第一处之后就已成立、第三处 `>= 3` 在第二条被接受之后就已成立，都是空等；`until` 的文档也照实改了，删掉“a case asks for at least as many as it has made” | 本提交 |
| 34 | 5 | 门禁（任务 5 修正轮 1）：`cargo fmt --all --check` 退出码 0（`gate-task-5-fix-1-fmt.log`）、`cargo clippy --all-targets -- -D warnings` 退出码 0（`gate-task-5-fix-1-clippy.log`）、`cargo test -j 8 --workspace --no-fail-fast` 退出码 0（`gate-task-5-fix-1.log`）：91 个结果行、5032 通过、0 失败、13 忽略，与第 32 行相同（这一轮不改行为，也不加用例）。带等待的五条再循环跑二十次全过（`task-5-fix-1-loop-tls.log`），`handshakes_beyond_the_limit_are_closed` 每次都在里面。`Cargo.lock` 仍 520 个包。本机没有编译的代码：没有 | 本提交 |
| 35 | 6 | 协调者的裁定（任务 6 的派工单）：① `crates/rurge/tests/sockets.rs` 的新行 `("crates/rurge/src/cli/api_client.rs", "TcpStream::connect", …)` 放在同一文件 `build_http(` 那一行的**前面**：清单按 (文件, 模式) 的字节序排，`T` 是 0x54、`b` 是 0x62；这订正任务 0 报告第 764 行的“插在 `api_client.rs` 那一行的后面”（第 764 行说的是 `api_client.rs` 那一行、不是 `build_http(` 那一行，两种读法在排序上相反）。② 端到端用 `free_tcp_port()` 订正任务 6 的 brief：`API_GENERAL` 的 `http-api` 是 `127.0.0.1:0`，`rurge status -c` 从 Profile 里只读得到一个 0 号端口，连不上，所以 `status_and_stop_reach_an_api_served_over_https` 照 `a_profile_that_is_only_a_pointer_runs_on_the_downloaded_copy` 的写法自己取号、把端口写进 Profile。③ `W0040`（加载时的诊断）与 P12 的 WARN（运行时的动作）会同时出现，两行各说一件事、都要；用例只断言 P12 的那一句。④ 本任务一个提交 | 本提交 |
| 36 | 6 | 执行者的裁定：① `crates/rurge/src/cli/run.rs` 今天**有**测试模块（`#[cfg(test)] mod tests`，文件末尾 `:2214`，本任务之前二十条用例），覆盖任务 0 报告第 5 项 ⑤（“今天没有测试模块，任务 6 在文件末尾新建”）与派工单“前面留下的”同一句：`what_a_reload_says_about_the_api` 加在那个模块里，不新建第二个。② `Endpoint` 装着 API 的钥匙，所以不给它 `Debug`（凭据不进 `Debug`）；三条现有断言里的 `unwrap_err()` 改成 `.err().expect(…)`（`Result::unwrap_err` 要 `T: Debug`），断言的值不变，P15 表里“元组的解构改成字段”那一条的两处只改解构。③ `Transport::Plain` 里那个 `Client` 装箱成 `Box<Client<…>>`：不装箱时 clippy 的 `large_enum_variant` 让 `-D warnings` 变红（最大的变体至少 288 字节、次大的 72 字节），仓库里没有别的 `#[allow(clippy::…)]`，所以装箱而不是加一条 allow。④ `api_changes` 的 `http-api-tls` 那一支以 `boot.access.is_some()` 为前提（启动时 Profile 里没有 `http-api`，`http-api-tls` 的改动不是一件要说的事），与派工单的用例表一致；`reload` 里调 `api_changes` 的地方没有外层 `if`，两个函数自己判，位置不变；`api_ca_note` 紧接在 `let surface_changed = d.engine.swap_runtime(next);` 之后调。⑤ 起 API 的一段：`ctx` 在 `http_api_tls && engine.api_tls().is_none()` 这一支里用不到（不绑、不打印 `api on`），但仍在 `if let Some(api)` 里先建好，两种分支共用一句 `match`（`serve_tls` 与 `serve` 的参数与返回相同） | 本提交 |
| 37 | 6 | 门禁（任务 6）：`cargo fmt --all --check` 退出码 0（`gate-task-6-fmt.log`）、`cargo clippy --all-targets -- -D warnings` 退出码 0（`gate-task-6-clippy.log`）、`cargo test -j 8 --workspace --no-fail-fast` 退出码 0（`gate-task-6.log`）：91 个结果行、5036 通过、0 失败、13 忽略（基线 91 / 5032 / 0 / 13 加本任务的四条：`run.rs` 与 `control.rs` 各一条单元用例、`run::mitm` 的两条），没有 `STATUS_HEAP_CORRUPTION`/`STATUS_ACCESS_VIOLATION`，没有重跑，跑完没有留下 `rurge.exe`；`cli` 那个二进制这一次 200 条全过（与任务 5 时同数，本任务只加两条新用例、`mod mitm;` 不挪别的）。`Cargo.lock` 仍 520 个包，只有 `rurge` 那一项的依赖列表多 `rustls` 与 `tokio-rustls` 两行；`sockets.rs` 的 `LISTED` 声明 26、实数 26。`run::mitm` 两条循环跑二十次全过（`task-6-loop-mitm.log`）。跑前 `df -h /d` 剩 114 GiB、跑后同数；本机没有编译的代码：没有（本任务没有 `#[cfg(unix)]` 的东西） | 本提交 |
| 38 | 6 | 修正轮 1（协调者评审任务 6 之后，不改任何行为）：① 覆盖第 36 行 ① 的“本任务之前二十条用例”：`git show 9d4f05a:crates/rurge/src/cli/run.rs` 里 `#[test]` 有 19 个、测试模块在 `:2127`；本任务之后 `cargo test -j 8 --workspace --bins -- --list` 里 `cli::run::tests::` 有 20 条（19 加 `what_a_reload_says_about_the_api`）、模块在 `:2214`。以本行为准；`0dabdff` 曾把第 36 行原地改写，这违反「已写下的行不改写」，第 36 行从此不再动（报告 `task-6-report.md` 的「自查」第 3 条与 `0dabdff` 的提交说明里的“二十条”也不 amend，同样以本行为准）。② 覆盖第 37 行的“`cli` 那个二进制这一次 200 条全过（与任务 5 时同数……）”：任务 5 修正轮 1 的全量里 `cli` 是 198 条（`gate-task-5-fix-1.log` 第 589 行），本任务是 200 条（`gate-task-6.log` 第 594 行），即 198 加 `run::mitm` 的两条。③ 第 36 行没写到的一件：`endpoint_resolution_prefers_flags_then_config` 多了一句 `assert_eq!(e.ca, None)`，而派工单说现有用例“不加新断言”；协调者的裁定是接受（它钉住没有 `http-api-tls` 的 Profile 不带 CA）。④ 派工单要一个提交，本任务成了两个（`6cd1d89` 与 `0dabdff`）；本行的提交是修正轮 1 的那一个。⑤ `crates/rurge/src/cli/api_client.rs` 的 `call` 里两句注释照实：给两支的 URI 那段改成说“交给各客户端的是什么，不是上线的样子”——明文给绝对形式，旧客户端从它取拨号的地址与 `Host`、再把 URI 写成源站形式（hyper-util 0.1.20 的 `set_host` 与 `origin_form`，`hyper-util-0.1.20/src/client/legacy/client.rs:298-318`）；TLS 给源站形式另加 `Host`，因为 `client::conn` 照给它的写。TLS 那一支上方那句“a pool would keep each of them for five seconds at least”删掉（旧客户端的连接池缺省空闲 90 秒，`client.rs:1044`；TLS 这一支根本没有池），理由照 P14 写：“a command makes only a handful of requests, which is not worth reusing a connection for” | 本提交 |
| 39 | 6 | 门禁（任务 6 修正轮 1）：`cargo fmt --all --check` 退出码 0（`gate-task-6-fix-1-fmt.log`）、`cargo clippy --all-targets -- -D warnings` 退出码 0（`gate-task-6-fix-1-clippy.log`）、`cargo test -j 8 --workspace --no-fail-fast` 退出码 0（`gate-task-6-fix-1.log`）：91 个结果行、5036 通过、0 失败、13 忽略，与第 37 行相同（这一轮只改注释与记录，不改行为、不改用例）；`cli` 那个二进制 200 条全过（`gate-task-6-fix-1.log` 第 594 行）。没有 `STATUS_HEAP_CORRUPTION`/`STATUS_ACCESS_VIOLATION`，没有重跑；只改注释，不循环。`Cargo.lock` 仍 520 个包，本任务不改 `Cargo.toml`；`LISTED` 仍 26。跑前 `df -h /d` 剩 114 GiB、跑后同数。本机没有编译的代码：没有 | 本提交 |
| 40 | 7 | 执行者的裁定（P17 没有写到的、与代码对不上的，照实写）：① 「三种入口」在 M1 一节拆成每个平台两条（W1 / W2 一类：HTTP 代理与 SOCKS5 一条、增强模式一条）——增强模式要管理员 / root 与一次性的机器，与代理那两条不是同一类操作。② 证书固定（P17 的「`curl --pinnedpubkey` … → curl 拒绝」）写成两种并都进 M2 一节的 P1：`curl --pinnedpubkey` 在证书校验那一层就拒绝（`unknown_ca` / `bad_certificate` 一类警报），rurge 因此记的是「不信任」与 `mitm.untrusted`（设计第 8.2 节：这几种警报分不清「没装 CA」与「证书固定」）；`noRequest` 与「likely protected by certificate pinning」那一句要一个握手完成之后才关掉的客户端，清单用 `openssl s_client … </dev/null`。③ P17 写「记录的 `noRequest`」：`noRequest` 是 `GET /v1/rurge/http` 的 `mitm.noRequest` 计数，记录里写的是原因 `no request after the TLS handshake (likely certificate pinning)`（`crates/rurge-engine/src/serve.rs:592`），清单两处都写。④ P17 写「从浏览器打开 … 下载到 `rurge-ca.cer`」：文件名照 `GET /v1/mitm/ca` 的 `Content-Disposition`（`crates/rurge-api/src/routes/mitm.rs:23`）写。⑤ 清单里的命令一条也没有执行（装 / 卸 CA、`certutil`、`security`、`openssl`、到真站点的 `curl`、`rurge run`、增强模式都是给项目所有者的），`rurge mitm ca generate` 也没有跑 | 本提交 |
| 41 | 7 | 门禁不跑（只改文档）：`git diff --check` 无输出（退出码 0）；`git grep -l <账户名>` 数出来的文件数仍是 26；`cargo fmt` / `clippy` / `test` 三步一次也没有跑。清单 30 项（M1 一节 9 项：Windows 2、macOS 2、Linux 2、与平台无关 3；M2 一节 21 项：Windows 5、macOS 5、Linux 5、与平台无关 6），`docs/README.md` 加了一项；清单里引的日志行、字段与路径逐句用 `git grep -n` 核过（清单 → 命中的表在任务 7 的报告里） | 本提交 |
| 42 | 7 | 修正轮 1（协调者评审任务 7 之后）：覆盖第 40 行 ② —— 那个前提不成立。curl 核 pin 在 TLS 握手**完成之后**：libcurl 的 `CURLOPT_PINNEDPUBLICKEY` 说 pin 不符时 "aborts the connection before sending or receiving any data"；OpenSSL / LibreSSL 版的 curl 连证书链也是握手完成之后才核（给 OpenSSL 设 `SSL_VERIFY_NONE`，握手成功之后才看链的结果、再核 pin，`curl` 的 `lib/vtls/openssl.c` 的 `ossl_connect_step3` / `Curl_ossl_check_peer_cert` / `ossl_check_pinned_key`）。所以 rurge 看到的是「握手完成、读到 0 个明文字节、客户端关了」，正是证书固定提示的三个条件（`crates/rurge-engine/src/serve.rs:575` ～ `:595`；`closed_by_client` 包括 `close_notify`、EOF 与重置，见 `crates/rurge-http/src/mitm/decrypt.rs:89` ～ `:93`）；`curl --pinnedpubkey` 得到的是那条提示（`serve.rs:638`）、`mitm.noRequest` 与记录的原因（`serve.rs:592`），不是「不信任」与 `untrusted`。M2 一节的 P1 因此改成两种客户端（curl 与 `openssl s_client`）看同一条提示，删掉「curl 报的是不信任」那一段，写明不信 CA 的 OpenSSL 版 curl 也一样（第二个误报来源，已进「延后事项」第 1 行）；W5 / M5 / L5 写明用浏览器、不用 curl。其余照评审改的：① P6 的 (c)：CA 是坏的时 `check` 给 `W0039` 与 `W0040` 两条（`authority.rs` 的 `check_with`；`CaProblem::new` 的 `MITM is off: ` 前缀在 `:103`），`status -c` 给的是 `MITM is off: …`（`control.rs` 的 `resolve_endpoint` 的 `Err(problem)`），`W0040` 那一句只在 `[MITM]` 一个 CA 也没写时（`Ok(None)`）出现——为此加了 (d) 那一步。② 日志写在标准输出、诊断与 `error:` 写在标准错误、`api on` 写在标准输出（`run.rs` 的 `init_logging` 的 `stdout_layer`、`:1910`；`crates/rurge/src/cli/rule.rs:190`；`crates/rurge/src/main.rs`）。③ 浏览器的代理改成不改系统设置的写法（Edge / Chrome / Chromium 的 `--proxy-server`，Firefox 自己的设置），Safari 只能用系统代理并进了「改系统设置的项与恢复」（出处：Chromium 的 "Configuring a SOCKS proxy server in Chrome" 与 "Run Chromium with flags"）。④ 明文站点改写 `neverssl.com` 一类与浏览器的 HTTPS 升级（Chrome 115 起，blink-dev 的 HTTPS Upgrades；被升级的导航在开发者工具里是 `307 Internal Redirect`）。⑤ P2、W3 的 curl 补了 `-x`，W3 补了 curl 信不信 CA；`-o NUL` / `</dev/null` 的平台写法集中在共用前置条件。 | 本提交 |
| 43 | 7 | 门禁不跑（只改文档）：`git diff --check` 无输出（退出码 0）；`git grep -l <账户名>` 数出来的文件数仍是 26；`cargo fmt` / `clippy` / `test` 三步一次也没有跑。清单仍是 30 项（P6 多了一步 (d)，不多一项，其余是改同一项的文字） | 本提交 |
| 44 | 7 | 修正轮 2（协调者评审修正轮 1 之后）：覆盖第 42 行的两处——① 里“`check` 给 `W0039` 与 `W0040` 两条”没有说次序与位置，写得像是 `warning[W0039]: …`；而 `rurge check` 的每条诊断带 `<文件>:<行>` 的位置（`crates/rurge-config/src/span.rs:20` ～ `:24` 的 `Display`，诊断的一行在 `crates/rurge-config/src/diagnostic.rs:71` ～ `:92`），且没有位置的 `W0040` 排在前面（`diagnostic.rs:123` ～ `:130` 的 `sorted` 按 `span` 排、`None` 在 `Some` 前）；② 里“诊断与 `error:` 写在标准错误”只对 `rurge run` 成立：`rurge check` 的诊断与末尾那行计数都打在标准输出（`crates/rurge/src/cli/check.rs:85` ～ `:91`，用例 `crates/rurge/tests/cli.rs:251` 读的是 `stdout`），`rurge run` 才是 `print_diagnostics` 的 `eprintln!` 且不带位置（`crates/rurge/src/cli/rule.rs:183` ～ `:195`；启动那一段用 `sorted()`，`run.rs:1673`）。这两处是协调者的修正单写错、执行者照做的。另三件：一、M1 一节的 P1、P2 的 curl 改成 `-sS`（它们要求“把错误原话记下来”，`-s` 会把错误吞掉）；二、Windows 装 CA 写明用平时的账户、不以管理员身份运行（`certutil -user` 装进当前用户的库，提权到别的账户时装进那个账户的库）与那一个安全警告，卸 CA 一样；第 42 行原来把“Windows 要管理员”也说给了装 CA。三、修正轮 1 报告“顾虑”第 1、2 条：M2 一节的 P2 补上 `--cacert` 那一句与“看到证书固定提示而不是 502 时先查 curl 信不信 CA”，P1 的 `openssl s_client` 补上“没有 `-proxy` 选项时跳过这一半”。 | 本提交 |
| 45 | 7 | 门禁不跑（只改文档）：`git diff --check` 无输出（退出码 0）；`git grep -l <账户名>` 数出来的文件数仍是 26；`cargo fmt` / `clippy` / `test` 三步一次也没有跑。清单仍是 30 项（这轮只改同一项的文字） | 本提交 |
| 46 | 8 | 执行者的裁定：① 派工第 7a 条的两处小字照它的原文改（清单第 32 行改成“每条诊断一行（有位置的带 `<文件>:<行>`，有 `hint` 的下面另起一行 `  hint: …`）”，第 194 行 P6 (c) 括号里两条的先后对调）。② 设计第 22 节加了两行（M2a 计划「延后事项」第 1 行、本计划「延后事项」第 1 行），都归项目所有者；第 23 节按派工逐条核了候选，**只算两条**（第 28 条：`rurge run` 起来时也随诊断打出 `W0040`，设计第 14.1 节读来只有 `rurge check`；第 29 条：证书固定提示的第二个误报来源，设计第 8.3 节只列了浏览器的预连接），其余候选不算，逐条的理由在报告的「设计第 22、23 节」一节。③ 兼容性清单 1.2 节的 `[MITM]` 一行不改写“目前（阶段 4 / M2a）”那一段，只在后面补一句说它写于 M2b、M2c 之前（历史的段落不改写）；`rurge-api` 对 `rurge-http` 的开发依赖写进了依赖附录（附录原本就连开发依赖一起记）。④ `docs/requirements.md` 阶段 4 的「状态」一句整句改写：它是状态，不是历史段落；FR 的字面没有动。⑤ 两份 README 的状态段落、特性表与路线图两处各写一遍，逐段对过。 | 本提交 |
| 47 | 8 | 门禁（任务 8）：`cargo fmt --all --check` 退出码 0（`gate-task-8-fmt.log`）、`cargo clippy --all-targets -- -D warnings` 退出码 0（`gate-task-8-clippy.log`）、`cargo test -j 8 --workspace --no-fail-fast` 退出码 0（`gate-task-8.log`）：91 个结果行、5036 通过、0 失败、13 忽略（与任务 6 的全量相同，本任务只改文档）。一台 Windows 11 x86-64 开发机上的一次运行，不是 CI；需要参照服务端的互操作用例在本机立刻返回、算在通过的数里 | 本提交 |
| 48 | 8 | 修正轮 1（协调者评审任务 8 之后）：① 设计第 23 节：补回第 27 条与“**执行 M2c 时**”之间被删掉的空行、去掉第 28、29 条之间的空行（其余各条紧排）；任务 8 加的四行里 8 对弯引号换成直引号，此后整个文件里没有弯引号。② 兼容性清单第 585 行：任务 8 把旧备注整句改写了，违反“历史的段落不改写”——恢复 `eedd9b1` 的原文、新文接在它后面；那一句“这一段只有 CI 编译过”不实（CI 还没有见过阶段 4 的提交），改成“`0600` 那一支是 `#[cfg(unix)]`，这台 Windows 开发机编译不到，写这一行时 CI 也还没有编译过它”。③ `docs/status.md`：任务 6 的提交补上 `0dabdff`；门禁一句改成“除任务 7 外”（第 41、43、45 行）；循环那一句照实录五圈、补上任务 1 那一圈；`0600` 那一句改成 CI 的三个平台没有编译过、这台机器编译并运行过、只有那两处编译不到；“协调者的裁定”的行号改成第 10、19、23、26、35 行与第 38 行 ③（订正第 46 行 ⑤ 没有提到的 `docs/status.md` 的行号；第 22 行是“协调者的记录”，不是裁定）。④ `CLAUDE.md`：阶段 4 那一条补上“推送与读回那次 CI 之后是 M3 的细化设计”；CI 那一条去掉重复的“与 M1 一起”。⑤ `docs/testing.md` 的「命令」补两行 M2c 之后的汇总（`--lib -- mitm` 61 条、`--test mitm` 37 条） | 本提交 |
| 49 | 8 | 门禁不跑（只改文档）：`git diff --check` 无输出（退出码 0）；`git grep -l <账户名>` 数出来的文件数仍是 26；`cargo fmt` / `clippy` / `test` 三步一次也没有跑 | 本提交 |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | 证书固定的提示会把「不信 CA 的客户端」也报成证书固定：rurge 靠客户端在握手里发的警报分辨「不信任」（设计第 8.2 节），而握手完成之后才核证书的客户端（OpenSSL / LibreSSL 版的 curl 等）不信 CA 时，满足第 8.3 节证书固定的三个条件，被说成证书固定；提示的办法（在 `hostname` 里写 `-<名字>` 排除）对它们不对，它们要的是在客户端信任 CA。这是第二个误报来源（第一个是浏览器的预连接，设计第 8.3 节已写）。提示要不要改措辞（例如加上「或客户端不信 rurge 的 CA」）、要不要另想分辨的办法 | 项目所有者 |
