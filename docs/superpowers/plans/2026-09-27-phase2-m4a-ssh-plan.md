# 阶段 2 / M4a「SSH 出站」Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 实现 `ssh` 出站（SSH 动态转发）：每个策略一条 SSH 会话、每个连接一个 `direct-tcpip` 通道；口令或 `[Keystore]` 里的 OpenSSH 私钥认证；`server-fingerprint` 校验主机密钥；空闲断开（按打开的通道计）与 30 秒 × 3 的保活；经引擎的端到端与测速；对 OpenSSH `sshd` 的互操作；订阅行自己写的 `private-key=` 进订阅安全门；能力表翻转 `ssh`。

**Architecture:** `rurge-config` 新增 `spec::ssh`（`SshSpec`、`HostKeyPin`、`read_ssh`），`to_spec` 多一个 `ssh` 分支，`ProtoSpec::keystore_item` 让重载的指纹认得 SSH 私钥；`rurge-policy` 的订阅安全门与指纹随之认 `private-key`。新 crate `rurge-proto-ssh`（`→ rurge-proto → rurge-net → rurge-config`，只被 `rurge-engine` 依赖）：`keys`（构建期解码 Keystore 私钥）、`pins`（主机密钥比对）、`outbound`（`SshOutbound`：经 M2c 的 `Stack` 连到服务器（可叠 Shadow TLS）→ russh `connect_stream` 握手 → 先密钥后口令认证；tokio 锁单飞建会话；每次拨号开一个 `direct-tcpip` 通道；通道计数加一个看门任务实现空闲断开）与 `testing`（`FakeSsh`：russh 自己的服务端，只在回环）。`rurge-engine` 的工厂多一个分支；`tests/interop` 在 Unix 上起一个只听回环的临时 `sshd`。

**Tech Stack:** Rust 1.89 / edition 2024；新依赖 **russh 0.63.3**（`default-features = false`，开 `ring` 与 `rsa`；随之带进 `ssh-key 0.7.0-rc.11`、`rsa 0.10.0-rc.18` 等，`Cargo.lock` 从 371 个包变为 474 个）；`rand` 0.10（`Cargo.lock` 里已有，只供 `testing` 特性现场生成密钥）；其余用工作区已有的 `tokio`、`tracing`、`base64`、`rustls`。

**Spec:** `docs/superpowers/specs/2026-09-27-phase2-m4-wireguard-ssh-external-design.md`（第 2 节 M4-D1、D2、D8、D10、D13；第 4.2、4.5 ～ 4.8 节中 `ssh` 的部分；第 5 节；第 8 ～ 12 节中 SSH 的部分；第 15 节 V1、V2、V12、V13；第 16 节 M4a 草图）；总设计 `docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md` 第 16 节 Q5；M3b 计划 `docs/superpowers/plans/2026-09-25-phase2-m3b-testing-auto-groups-plan.md` 末尾「延后事项」#10。与本计划「计划期决定」表不一致处，以该表为准，并由 Task 7 写回设计文档新增的第 17 节。

## Global Constraints

- MSRV **1.89**，edition 2024。`unsafe_code`：全工作区 `forbid`；只有 `rurge-platform` 是 `deny` + 唯一一个 `#[allow(unsafe_code)]` 函数。**本计划不新增任何 unsafe**（新 crate 用 `[lints] workspace = true` 继承 `forbid`）。
- 依赖方向：`rurge-proto-ssh → rurge-proto → rurge-net → rurge-config`；`rurge-engine` 依赖 `rurge-proto-ssh`；`rurge-policy` 不依赖任何协议实现；`rurge-inbound` 不依赖 `rurge-policy`；平台代码只在 `rurge-platform`（本计划不碰它）。**新第三方依赖只有 russh 0.63.3**：`default-features = false, features = ["ring", "rsa"]`——不用默认的 `aws-lc-rs`（Windows 上编译要 cmake 与 NASM），不开 `des`、`dsa`、`flate2`（M4-D2）。
- **测试绝不碰公网**：只用回环 + 端口 0 + 有界等待；不用固定 sleep 当同步手段（轮询 + 截止时间；只有"断言这段时间里什么也没发生"时才等一段固定时间）。SSH 服务端只用回环上的 `FakeSsh`（russh 服务端）或 `tests/interop` 的临时 `sshd`（只监听 `127.0.0.1`）。**任何带 `url-test` / `fallback` / `load-balance` / `smart` 组的测试配置，`proxy-test-url` 与 `internet-test-url` 都必须指向回环**——引擎用例的 `Profile::text` 已默认指向 `http://127.0.0.1:9/`，不要删掉。
- **测试与任何命令都不得修改本机的系统代理、注册表、网络设置，不得注册真实服务 / 计划任务**：不在 CLI 测试夹具之外运行 `rurge run --system-proxy`；不运行不带 `--dry-run` 的 `rurge service install | uninstall`。**不读写用户的 `~/.ssh`**：密钥现场生成（`testing::random_key`）或用 `testing` 里标明只供测试的固定密钥；`sshd` 的主机密钥、`authorized_keys` 与配置都写进临时目录。
- **不在本机下载或安装任何东西**（不装 OpenSSH Server / `sshd`、sing-box、xray，不 `rustup target add`、不 `cargo install`）。唯一的例外：首次构建时 cargo 从 crates.io 下载 russh 及其依赖（项目所有者已同意，M4-D2）。
- **凭据及其派生物永不外泄**：`username`、`password`、私钥内容与 Keystore 的 `base64` 不进日志、错误文本、API 输出与 `Debug`（`SshSpec` 的两个凭据字段是 `Secret`）；错误文本是固定说法，不引用服务器发来的原文（设计 5.6）；`server-fingerprint` 的解析错误只报序号、不引用取值（订阅行也可能写它）；没配指纹的告警只带策略名。
- rurge 专有的运行时选项只经命令行与环境变量提供，不扩展 Surge 配置格式（FR-CFG-17）。与 Surge 的每一处行为差异都登记进 `docs/surge-compatibility-matrix.md`（Task 7）。
- 日志、错误文本、CLI 输出、代码注释用英文；文档与提交标题用中文。注释密度、命名、惯用法与相邻代码保持一致；注释里不写评审轮次的标签。
- 提交信息结尾两行：`Co-Authored-By: <执行者自己的署名>` 与 `Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th`。**不 push、不 merge**，不 amend。
- 每个任务结束的门禁（全部通过才算完成）：

  ```bash
  RUSTFMT="C:\Users\SZV01065\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe" cargo fmt --all --check \
    && cargo clippy --all-targets -- -D warnings \
    && timeout 1500 cargo test --workspace --no-fail-fast
  ```

  `timeout` 不能省：`rurge-dns` 的一个用例曾让测试进程以 100% CPU 空转数小时（M3a「延后事项」#20）。测试二进制异常退出而没有失败用例时（`STATUS_ACCESS_VIOLATION`、`STATUS_HEAP_CORRUPTION` / `0xc0000374`、段错误——本机已知的既有问题，M3b 计划 P21；写本计划时新 crate `rurge-proto-ssh` 的测试二进制也遇到过一次），或整轮被 `timeout` 杀掉时，重跑一次并保留两次的日志，**不要在任务里去修它**。已知偶发失败的计时类用例（`rurge-dns` 的 `a_partial_result_completes_aaaa_in_the_background`、`rurge` 的 `run::watch_reloads_rules_on_change`）同样重跑。
- 第三方 crate 的 API 以本机源码为准：`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`（`russh-0.63.3`、`ssh-key-0.7.0-rc.11`）。
- 本机的 bash 处理不了超过约 8 KB 或引号复杂的 heredoc：新文件一律用写文件的工具落盘，不用 heredoc。
- Windows 上连接回环里一个没人监听的端口，要一两秒才报"拒绝连接"：用到它的用例因此各要几秒，这是正常的。

## Review Focus

设计没有逐条写到、而最可能伤到使用者的五类输入或失败方式；每一条都在负责它的任务里配了用例。

1. **服务器断开了会话**（`sshd` 重启、网络切换、NAT 超时）：下一个连接要自己重建会话再试一次，而不是一直报"会话已关"。用例：Task 3 `a_session_the_server_ended_is_replaced`。
2. **浏览器同时开几十个连接**：只握手、登录一次，共用一条会话（否则会撞上服务器的 `MaxStartups` 或 fail2ban 之类的封禁）。用例：Task 3 `one_session_carries_every_connection`。
3. **长时间没有流量的连接**（WebSocket、SSH 里再套一层 SSH）：不能被 `idle-timeout` 切断；最后一个连接关掉之后才开始计时。用例：Task 4 `an_open_channel_keeps_the_session_past_the_idle_timeout`、`a_session_without_channels_closes_after_the_idle_timeout`。
4. **口令、私钥、指纹写错或私钥用不了**：固定说法、不带凭据与服务器原文；用不了的私钥（带口令、DSA、不是私钥）在 `rurge check` 就报出来。用例：Task 2 `keys_rurge_cannot_use_are_named_not_quoted`；Task 3 `a_wrong_password_fails_without_repeating_it`、`a_host_key_outside_server_fingerprint_is_refused`；Task 7 `check_knows_ssh`。
5. **在 `[Keystore]` 里换了私钥（条目名不变）后重载**：策略按新私钥重建，而不是沿用旧出站继续用旧私钥。用例：Task 5 `a_changed_ssh_private_key_rebuilds_the_policy`。

另有两条同样配了用例、但不那么常见的：只认 `ssh-rsa`（SHA-1）签名的老服务器——rurge 的 RSA 私钥绝不用 SHA-1 签名（Task 3 `an_rsa_key_never_signs_with_sha1`）；订阅行想用主配置里的私钥——整行跳过（Task 1 `a_subscription_ssh_line_may_not_use_the_profiles_private_keys`）。

## 计划期决定

写计划时对照设计、Surge 手册（`policies/ssh.html`、`profile/keystore.html`，2026-09-27 读取）、russh 0.63.3 / ssh-key 0.7.0-rc.11 源码与本仓库源码核对后定下的事；与设计文档文字不同的，由 Task 7 写回设计文档第 17 节。

**本计划里的代码不是凭空写的。** 全部 7 个任务的改动在仓库的一份副本上按任务顺序真实做了一遍（副本用自己的构建目录，不与本仓库的 `target/` 混用），每个任务之后跑一次全工作区门禁：最后一次是 **45 个测试二进制，1029 通过 / 0 失败 / 1 忽略**（本计划开工前的 main 是 41 个、992 通过）。计划里新文件的全文取自副本上该任务的提交，修改处的"把 … 换成 …"由脚本从相邻两个任务提交的差异生成，并在拼好之后按计划的顺序套到开工前的源码上逐字核对过——计划文本与验证过的代码一字不差。每个任务 Step 2 的"预期失败"是只把该任务的用例块（及写明的前置改动）套到上一个任务的状态上、真实跑出来的。

| # | 事项 | 决定与依据 |
| - | ---- | ---------- |
| P1 | V1：依赖与特性 | 工作区 `russh = { version = "0.63.3", default-features = false, features = ["ring", "rsa"] }`，在 Windows、MSRV 1.89 上编译通过；`Cargo.lock` 从 371 个包变为 474 个（首次构建需要联网下载）。`rand = "0.10"` 进工作区依赖：`PrivateKey::random` 要 rand_core 0.10 的随机源，版本已在 `Cargo.lock` 里；`rurge-proto-ssh` 只在 `testing` 特性与 dev 依赖里用它 |
| P2 | V1 / 设计 5.5：算法 | russh 0.63.3 的 `Preferred::DEFAULT`：kex 为 `mlkem768x25519-sha256`、`curve25519-sha256`（含 `@libssh.org`）、`diffie-hellman-group-exchange-sha256`、DH group 14 ～ 18（SHA-2），MAC 为 `hmac-sha2-512/256`（含 ETM）——都没有 SHA-1；**加密列表没有 `aes128-gcm@openssh.com`**（只有 chacha20-poly1305、aes256-gcm 与三种 ctr），而 Surge 手册要求它：`preferred()` 把它补在 aes256-gcm 之后；主机密钥算法去掉 `Rsa { hash: None }`（`ssh-rsa`，SHA-1 签名），其余照默认 |
| P3 | V1 / 设计 5.3：RSA 私钥的签名哈希 | russh 的 `best_supported_rsa_hash()` 在服务器的 `server-sig-algs` 只列 `ssh-rsa` 时给出 SHA-1（`Some(None)`）。改用纯函数 `rsa_hash`：服务器列了 rsa-sha2-512 就用它，否则一律 rsa-sha2-256——服务器只列 `ssh-rsa` 或什么都没说时也是；只认 SHA-1 的老服务器（OpenSSH 7.2 以前）因此登录不了，登记为差异 |
| P4 | V1 / V2：主机密钥比对 | `Handler::check_server_key(&mut self, &PublicKeyOrCertificate)`：`server-fingerprint` 为空时一律接受；否则拿服务器公钥的线上编码（开头就是算法名）与每一项逐字节比对，主机证书取证书里被认证的那把公钥（`Certificate::public_key()` 的 `KeyData`）。返回 `false` 时 russh 以 `Error::UnknownKey` 结束握手 |
| P5 | V1 / 设计 5.1：开通道的失败形态 | `channel_open_direct_tcpip(host, port as u32, "127.0.0.1", 0)`：`Error::ChannelOpenFailure(reason)` 是服务器拒绝——按 RFC 4254 的原因名报错，会话保留；其余错误都当作会话已断（russh 此时给的是发送失败一类的错误，分不出更细）——丢弃会话、重建一次、再试一次。取会话时另查 `Handle::is_closed()`。通道经 `Channel::into_stream()` 交给引擎，流被丢弃时通道关闭 |
| P6 | V1 / 设计 5.4：保活与空闲 | 保活用 russh 客户端 `Config` 的 `keepalive_interval = 30 秒`、`keepalive_max = 3`。`inactivity_timeout` 按流量计，与"空闲 = 没有打开的通道"（M4-D10）不同，不用（保持客户端默认的 `None`）；空闲断开由每条会话一个看门任务实现：通道计数（`OpenChannel` 守卫随流存亡）与 `Notify`，计数为 0 持续 `idle-timeout` 就 `disconnect`。看门任务只持有会话的 `Weak`，会话被替换或丢弃时随之结束 |
| P7 | V2 / 设计 4.5：私钥解码 | `russh::keys::decode_secret_key(text, None)`：带口令的私钥返回 `Error::KeyIsEncrypted`，据此给专门的文本 ``keystore item `k` is protected by a passphrase, which rurge cannot use; remove the passphrase``（设计的示例是 ``key1 is encrypted; …``；改为与既有 Keystore 报错同样以 "keystore item `名字`" 开头，并说明怎么办）。**不开 `dsa` 特性时 DSA 私钥照样能解码**（只是签不了名）：解码后按算法只收 Ed25519 / ECDSA / RSA，其余（DSA、要硬件的 `sk-*` 安全密钥）一律 ``… is not an Ed25519, ECDSA or RSA key``。文本都只点名条目，不引用内容 |
| P8 | V2：`server-fingerprint` 的写法 | 手册的写法是 `ssh-keyscan` 那样的 `算法 base64`，多个以逗号分隔、整值加引号。`rurge-config` 不依赖 ssh-key：只做 base64 解码，并读出公钥编码开头的算法名与写的算法名比对（不一致、base64 不对、少一段都是 `E0018`，只报第几项） |
| P9 | V13：`private-key` 的 `E0020` | M1 的 Keystore 引用校验只针对 `client-cert`（`spec/tls.rs`）；`read_ssh` 自己查：条目不存在、条目是 p12 都是 `E0020`，文本与 `client-cert` 的两条对称 |
| P10 | 重载按指纹复用出站 | M2b 的指纹里"引用的 Keystore 条目"只取 TLS 的 `client-cert`。新增 `ProtoSpec::keystore_item()`：`ssh` 取 `private-key`，其余取 TLS 的 `client-cert`；注册表的指纹改用它。否则在 `[Keystore]` 里换了私钥（条目名不变）的重载会沿用旧出站、继续用旧私钥 |
| P11 | 订阅安全门（M3b #10，设计 4.8） | `reaches_into_profile` 里 `client-cert` 的检查抽成 `only_the_modifier_sets(policy, modifier_values, key)`，对 `client-cert`、`private-key` 各跑一次；文本沿用 `client-cert` 的两条（键名换成 `private-key`）。`wireguard` 的 `section-name=` 留给 M4b |
| P12 | 设计 5.2：没配指纹的告警 | `tracing::warn!(policy = %name, "ssh: no server-fingerprint; the server's host key is not verified")`：结构化字段加固定消息，与 `registry.rs` 的 `policy cannot be built` 同一写法（设计写的是把策略名嵌进消息）。**按出站对象计一次**（`AtomicBool`），在第一次建好会话时记：重载时参数没变的策略沿用原出站、不再告警，参数变了而重建的出站再告警一次（设计写的是"每个策略每个进程一次"，那要另设一张全局表） |
| P13 | 设计 5.1：单飞与时限 | 会话槽是 tokio `Mutex<Option<Arc<Session>>>`，锁只包住"取或建"：同时进来的拨号在锁上等同一次握手。`connect_tcp` 用 `tokio::time::timeout(opts.timeout, …)` 包住整个拨号（取会话、握手、认证、开通道与那一次重建），超时是 `OutboundError::Timeout` |
| P14 | 设计 5.6：错误文本 | 只有"没有共同算法"带括号说明（`ssh: the handshake failed (no algorithm in common with the server)`）；其余握手失败（对端不是 SSH、握手中的 I/O 错误等）都是 `ssh: the handshake failed`——russh 的错误分不出握手的阶段；认证失败 `ssh: authentication failed`；主机密钥不在列表里 `ssh: the server's host key is not one of server-fingerprint`；开通道被拒 `ssh: the server refused the channel (<原因名>)`；会话已断而重建后仍开不了通道 `ssh: the session closed` |
| P15 | V12：对 `sshd` 的互操作 | 非 root 的 `sshd` 只让运行它的用户登录：用例用现场生成的 Ed25519 密钥登录（用户名取 `USER`），只在 Unix 上编译（`#![cfg(unix)]`）。渲染出的 `sshd_config` 只监听 `127.0.0.1`、只认那一把公钥、关掉口令 / PAM / 终端、只允许本地转发；主机密钥写成 0600（否则 `sshd` 拒绝）。本机（Windows）没有 `sshd`：这两个用例只在 CI 上真正跑，CI 在 Linux / macOS 上设 `RURGE_TEST_SSHD=/usr/sbin/sshd`（Linux 上缺它时先装 `openssh-server`） |
| P16 | 设计 §10："空闲断开（暂停的时钟）" | 空闲断开的用例用真实时钟（`idle-timeout` 取 1 秒，轮询等待最多 5 秒）；保活只断言 `session_config()` 的取值。回环上的真实连接与暂停的时钟不能共存：运行时空闲时会自动拨快时钟，russh 自己的计时（等 `server-sig-algs` 的 1 秒、保活）随之提前到期 |
| P17 | `FakeSsh` | russh 服务端（`server::run_stream`），只听回环：口令与公钥登录、可拒绝全部通道（`connect failed`）、`surge_minimum`（只开 `curve25519-sha256` 与 `aes128-gcm`）、`connect_to`（把所有通道接到一个固定地址：引擎用例里目标名不在本地解析）；记录成功登录次数、在线会话数、通道请求的目标，`end_sessions()` 断开全部会话（像服务器重启） |
| P18 | Shadow TLS | `SshOutbound` 用 M2c 的 `Stack`（connect → shadow-tls），不带 TLS / WebSocket 层；`shadow_tls_client(opts, None, &server.host, roots)`：没有里层 TLS，不写 `shadow-tls-sni` 时伪装握手的证书按服务器主机名校验（与不带 TLS 的 `socks5` 同一规则）。Task 5 有一条经 Shadow TLS v3 的端到端用例 |
| P19 | 任务的切分 | 与设计第 16 节草图相同的 7 个任务；经 Shadow TLS 的端到端用例放在 Task 5（引擎）；Task 7 是能力表翻转与文档（草图如此，两次提交） |

## 承接事项

之前计划「延后事项」表里标给 M4（SSH 部分）的条目，及仍然有效的既有现象。

| # | 来源 | 事项 | 处理 | 任务 |
| - | ---- | ---- | ---- | ---- |
| C1 | M3b #10 | `ssh` 的 `private-key=` 与 WireGuard 的 `section-name=` 同样是按名字用到主配置的材料，要加进 `reaches_into_profile` | `ssh` 的 `private-key=` 在本计划（P11）；`section-name=` 在 M4b | 1 |
| C2 | M3a #25 | 订阅导入的 `external` 一律跳过 | 不在本计划：M4c（M4-D1、D8） | — |
| C3 | M3c #4 | `CORE_VERSION` 仍报告 20 | 不在本计划：阶段 2 收尾（M8）由项目所有者决定 | — |
| C4 | M3b #7（P21） | 测试二进制偶发崩溃 | 照旧：门禁遇到就重跑 | — |

## File Structure

新建：

| 文件 | 职责 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-config/src/spec/ssh.rs` | `SshSpec`、`HostKeyPin`、`read_ssh`、`DEFAULT_IDLE_TIMEOUT`，与用例 | 1 |
| `crates/rurge-proto-ssh/Cargo.toml`、`src/lib.rs` | 新 crate 的清单与入口 | 2（3 加 `outbound`） |
| `crates/rurge-proto-ssh/src/keys.rs` | `decode_private_key`，与用例 | 2 |
| `crates/rurge-proto-ssh/src/pins.rs` | `host_key_allowed`，与用例 | 2 |
| `crates/rurge-proto-ssh/src/testing/mod.rs` | 测试密钥与辅助函数；`FakeSsh` 的出口 | 2、3、5、6 |
| `crates/rurge-proto-ssh/src/outbound.rs` | `SshOutbound`：会话、通道、认证、错误文本（3）；空闲与保活、告警（4），与用例 | 3、4 |
| `crates/rurge-proto-ssh/src/testing/server.rs` | `FakeSsh`（russh 服务端） | 3、4、5 |
| `crates/rurge-engine/tests/outbounds_ssh.rs` | 经引擎的端到端用例 | 5 |
| `tests/interop/src/sshd.rs`、`tests/interop/tests/sshd.rs` | 临时 `sshd` 的夹具与互操作用例 | 6 |

修改：

| 文件 | 改动 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-config/src/spec/mod.rs` | `pub mod ssh`（1）；`ProtoSpec::Ssh`、`keystore_item`、`to_spec` 的 `ssh` 分支（5） | 1、5 |
| `crates/rurge-policy/src/assemble.rs` | 订阅安全门认 `private-key` | 1 |
| `Cargo.toml` | 工作区依赖：`rurge-proto-ssh`、`russh`、`rand` | 2 |
| `crates/rurge-policy/src/registry.rs` | 指纹改用 `keystore_item` | 5 |
| `crates/rurge-engine/{Cargo.toml, src/outbounds.rs, tests/common/mod.rs}` | 依赖、工厂分支、`Profile.keystore` | 5 |
| `tests/interop/{Cargo.toml, src/lib.rs, README.md}`、`.github/workflows/ci.yml` | 互操作夹具的接线与 CI | 6 |
| `crates/rurge/src/capabilities.rs`、`crates/rurge/tests/cli.rs` | 能力表翻转与用例 | 7 |
| 文档（兼容性清单、两份 README、`CLAUDE.md`、手工验收、M4 设计第 17 节、总设计 Q5） | 见 Task 7 | 7 |

## 任务一览

| 任务 | 交付物 | 依赖 |
| ---- | ------ | ---- |
| 1 | 配置层：`SshSpec` 与 `server-fingerprint` 解析；订阅安全门认 `private-key`（承接 C1） | — |
| 2 | `rurge-proto-ssh` 骨架：依赖、Keystore 私钥解码、主机密钥比对、测试密钥 | 1 |
| 3 | `SshOutbound`：会话、单飞、通道、断线重建、认证、错误文本；`FakeSsh` | 2 |
| 4 | 空闲断开与保活；没配指纹的一次性告警 | 3 |
| 5 | 接入：`ProtoSpec::Ssh`、重载指纹、引擎工厂；端到端、经 Shadow TLS 与测速 | 4 |
| 6 | 互操作：OpenSSH `sshd` | 5 |
| 7 | 能力表翻转 `ssh` 与文档 | 6 |

---

### Task 1: 配置层——`SshSpec` 与 `server-fingerprint`；订阅安全门认 `private-key`（承接 C1）

`ssh` 策略行的参数读成 `SshSpec`（设计 4.2）：`username` 必填；`password`（`Secret`）与 `private-key`（`[Keystore]` 条目名）至少一个；`idle-timeout`（秒，至少 1，默认 180）；`server-fingerprint`（`算法 base64`，多个以逗号分隔）解析成 `HostKeyPin`。TLS 参数不适用（`refuse_tls`，`W0028`），Shadow TLS 与其余通用参数照常由 `read_common` 处理（Task 5 接上）。本任务只提供 `read_ssh`；`to_spec` 的 `ssh` 分支在 Task 5 接上（那时才有出站可以构建，能力表在 Task 7 翻转，之前 `ssh` 行照旧 `W0007`）。另外，订阅行自己写的 `private-key=` 指向的是主配置 `[Keystore]` 里的私钥：像 `client-cert=` 一样，只认 `external-policy-modifier` 设上的值，否则整行跳过（P11，设计 4.8）。

**Files:**
- Create: `crates/rurge-config/src/spec/ssh.rs`（`DEFAULT_IDLE_TIMEOUT`、`SshSpec`、`HostKeyPin`、`read_ssh`、`parse_pin`，与用例）
- Modify: `crates/rurge-config/src/spec/mod.rs`（`pub mod ssh;` 与导出）
- Modify: `crates/rurge-policy/src/assemble.rs`（`only_the_modifier_sets`，与用例）

**Interfaces:**
- Produces:
  - `rurge_config::spec::ssh::DEFAULT_IDLE_TIMEOUT: Duration`（180 秒）
  - `pub struct SshSpec { pub username: Secret<String>, pub password: Option<Secret<String>>, pub private_key: Option<String>, pub idle_timeout: Duration, pub host_keys: Vec<HostKeyPin> }`（`Clone + Debug + PartialEq + Eq`；`Debug` 不显示两个凭据）
  - `pub struct HostKeyPin { pub algorithm: String, pub blob: Vec<u8> }`——`blob` 是公钥的 SSH 线上编码（base64 解码后的字节），开头是算法名
  - `pub fn read_ssh(r: &mut ParamReader<'_>, keystore: &[KeystoreItem]) -> SshSpec`（报错后返回值无意义，调用方看 `r.has_errors()`）
  - `rurge_config::spec::{SshSpec, HostKeyPin}` 的导出

- [ ] **Step 1: 先写用例**

`crates/rurge-policy/src/assemble.rs`——把

```rust
                ),
            ]
        );
    }
}

```

换成

```rust
                ),
            ]
        );
    }

    /// An `ssh` line's `private-key` names an item of the profile's
    /// `[Keystore]`: like `client-cert`, only the user's modifier may set it
    /// (phase 2 M4 design 4.8).
    #[test]
    fn a_subscription_ssh_line_may_not_use_the_profiles_private_keys() {
        let cfg = profile(
            "Corp = http, corp.test, 80",
            "G = select, policy-path=https://sub.test/g\n\
H = select, policy-path=https://sub.test/h, external-policy-modifier=\"private-key=key1\"\n\
[Keystore]\nkey1 = type=openssh-private-key, base64=QUJD",
        );
        let a = assemble(
            &cfg,
            &snapshots(
                &cfg,
                &[
                    (
                        "G",
                        "Own = ssh, s.test, 22, username=u, private-key=key1\n\
Plain = ssh, p.test, 22, username=u, password=pw",
                    ),
                    ("H", "Mod = ssh, m.test, 22, username=u"),
                ],
            ),
        );
        assert_eq!(members(&a, "G"), ["Plain"]);
        assert_eq!(members(&a, "H"), ["Mod"]);
        let skipped: Vec<_> = warnings(&a)
            .into_iter()
            .filter(|(code, _)| *code == codes::W_SET_LINES_SKIPPED)
            .collect();
        assert_eq!(
            skipped,
            [(
                codes::W_SET_LINES_SKIPPED,
                "policy group `G`: `policy-path` line 1: a subscription line's own `private-key` is not honoured (only `external-policy-modifier` may set it); skipped".to_string()
            )]
        );
    }
}

```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-policy a_subscription_ssh_line_may_not_use_the_profiles_private_keys`
Expected: FAIL——订阅行自己写的 `private-key=` 还没被挡住：

```text
test assemble::tests::a_subscription_ssh_line_may_not_use_the_profiles_private_keys ... FAILED
thread 'assemble::tests::a_subscription_ssh_line_may_not_use_the_profiles_private_keys' panicked at crates\rurge-policy\src\assemble.rs:1580:9:
assertion `left == right` failed
  left: ["Own", "Plain"]
 right: ["Plain"]
```

- [ ] **Step 3: 实现（新模块自带用例）**

新建 `crates/rurge-config/src/spec/ssh.rs`：

```rust
//! `ssh` policy parameters (manual: Policies › SSH).

use super::reader::ParamReader;
use super::secret::Secret;
use super::tls::refuse_tls;
use crate::diagnostic::codes;
use crate::keystore::{KeystoreItem, KeystoreType};
use base64::Engine as _;
use std::time::Duration;

/// `idle-timeout` when the line has none (manual: 180 seconds).
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshSpec {
    pub username: Secret<String>,
    pub password: Option<Secret<String>>,
    /// The name of an `openssh-private-key` item of `[Keystore]`.
    pub private_key: Option<String>,
    /// How long the session may have no open channel before it is closed.
    pub idle_timeout: Duration,
    /// `server-fingerprint`: the server's host key must be one of these;
    /// when there are none, any key is accepted (with a warning).
    pub host_keys: Vec<HostKeyPin>,
}

/// One entry of `server-fingerprint`: a public key the way `ssh-keyscan`
/// prints it, `<algorithm> <base64>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostKeyPin {
    pub algorithm: String,
    /// The key in the SSH wire encoding (the decoded base64).
    pub blob: Vec<u8>,
}

/// Everything `ssh`-specific on the line. After an error was reported the
/// returned value is meaningless: the caller checks `r.has_errors()`.
///
/// The credentials are named-only (`username=`, `password=`), as the manual
/// writes them: a positional value stays unread and is reported as an extra
/// positional value, never quoted.
pub fn read_ssh(r: &mut ParamReader<'_>, keystore: &[KeystoreItem]) -> SshSpec {
    refuse_tls(r);
    let username = r.str("username").unwrap_or_default();
    if username.is_empty() {
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            "`username` is required".to_string(),
        );
    }
    let password = r
        .str("password")
        .filter(|p| !p.is_empty())
        .map(Secret::from);
    let mut private_key = None;
    if let Some(v) = r.str("private-key") {
        let name = v.trim();
        match keystore.iter().find(|k| k.name == name) {
            None => r.error(
                codes::E_KEYSTORE_REF,
                format!("`private-key` references unknown keystore item `{name}`"),
            ),
            Some(item) if item.kind != KeystoreType::OpensshPrivateKey => r.error(
                codes::E_KEYSTORE_REF,
                format!("`private-key` needs an `openssh-private-key` keystore item, but `{name}` is `p12`"),
            ),
            Some(_) => private_key = Some(name.to_string()),
        }
    } else if password.is_none() {
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            "either `password` or `private-key` is required".to_string(),
        );
    }
    let idle_timeout = match r.str("idle-timeout") {
        None => DEFAULT_IDLE_TIMEOUT,
        Some(v) => match v.trim().parse::<u64>() {
            Ok(secs) if secs > 0 => Duration::from_secs(secs),
            _ => {
                r.invalid("idle-timeout", v, "seconds, at least 1");
                DEFAULT_IDLE_TIMEOUT
            }
        },
    };
    let mut host_keys = Vec::new();
    if let Some(v) = r.str("server-fingerprint") {
        for (i, entry) in v.split(',').map(str::trim).enumerate() {
            match parse_pin(entry) {
                Ok(pin) => host_keys.push(pin),
                // never echoed: the line may come from a subscription
                Err(why) => r.error(
                    codes::E_INVALID_POLICY_PARAM,
                    format!("`server-fingerprint` entry {}: {why}", i + 1),
                ),
            }
        }
    }
    SshSpec {
        username: username.into(),
        password,
        private_key,
        idle_timeout,
        host_keys,
    }
}

fn parse_pin(entry: &str) -> Result<HostKeyPin, &'static str> {
    let mut parts = entry.split_whitespace();
    let (Some(algorithm), Some(key), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err("expected `<algorithm> <base64 key>`");
    };
    let blob = base64::engine::general_purpose::STANDARD
        .decode(key)
        .map_err(|_| "the key is not valid base64")?;
    // the encoded key starts with its algorithm name, as an SSH string
    let named = blob
        .get(..4)
        .and_then(|n| <[u8; 4]>::try_from(n).ok())
        .map(|n| u32::from_be_bytes(n) as usize)
        .and_then(|n| blob.get(4..4 + n));
    if named != Some(algorithm.as_bytes()) {
        return Err("the key is not of the named algorithm");
    }
    Ok(HostKeyPin {
        algorithm: algorithm.to_string(),
        blob,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::Diagnostic;
    use crate::policy::parse_policy;
    use crate::span::Span;
    use std::path::Path;
    use std::sync::Arc;

    /// The manual's three example host keys (`policies/ssh.html`).
    const ED25519: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBk2No6KBq2m9VTCcHXXJBX4/A3RNr+L+yDBl5+TF9qz";
    const ECDSA: &str = "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBLdhR3D2BvyD7FTXfx0CrjZF2tVgoVRFi1poGKoX0eXc9OlpiaqNos4niiN0GWyoT4mL724cgvaL+vHW8sTZE5A=";

    fn item(name: &str, kind: KeystoreType) -> KeystoreItem {
        KeystoreItem {
            name: name.into(),
            kind,
            base64: "QUJD".into(),
            password: None,
            unknown: Vec::new(),
            span: Span::new(Arc::from(Path::new("p.conf")), 9),
        }
    }

    fn read(def: &str) -> (SshSpec, bool, Vec<Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let keystore = [
            item("key1", KeystoreType::OpensshPrivateKey),
            item("cert1", KeystoreType::P12),
        ];
        let mut r = ParamReader::new(&p);
        let spec = read_ssh(&mut r, &keystore);
        let failed = r.has_errors();
        (spec, failed, r.finish())
    }

    fn errors(def: &str) -> Vec<(&'static str, String)> {
        let (_, failed, diags) = read(def);
        assert!(failed, "{def}");
        diags.into_iter().map(|d| (d.code, d.message)).collect()
    }

    #[test]
    fn the_manuals_examples() {
        let (spec, failed, diags) = read("ssh, 1.2.3.4, 22, username=root, password=pw");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.username.expose(), "root");
        assert_eq!(
            spec.password.as_ref().map(|p| p.expose().as_str()),
            Some("pw")
        );
        assert_eq!(spec.private_key, None);
        assert_eq!(spec.idle_timeout, DEFAULT_IDLE_TIMEOUT);
        assert!(spec.host_keys.is_empty());

        let (spec, failed, _) = read("ssh, 1.2.3.4, 22, username=root, private-key=key1");
        assert!(!failed);
        assert_eq!(spec.private_key.as_deref(), Some("key1"));
        assert_eq!(spec.password, None);

        let (spec, failed, diags) = read(&format!(
            "ssh, 1.2.3.4, 22, username=root, password=pw, idle-timeout=60, server-fingerprint=\"{ED25519},{ECDSA}\""
        ));
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.idle_timeout, Duration::from_secs(60));
        let algorithms: Vec<&str> = spec
            .host_keys
            .iter()
            .map(|k| k.algorithm.as_str())
            .collect();
        assert_eq!(algorithms, ["ssh-ed25519", "ecdsa-sha2-nistp256"]);
        assert_eq!(spec.host_keys[0].blob.len(), 51, "4 + 11 + 4 + 32 bytes");
    }

    #[test]
    fn both_credentials_are_kept() {
        let (spec, failed, _) = read("ssh, h.test, 22, username=u, password=pw, private-key=key1");
        assert!(!failed);
        assert!(spec.password.is_some() && spec.private_key.is_some());
    }

    #[test]
    fn a_username_and_one_credential_are_required() {
        assert_eq!(
            errors("ssh, h.test, 22, password=pw"),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: `username` is required".to_string()
            )]
        );
        assert_eq!(
            errors("ssh, h.test, 22, username=u, password="),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: either `password` or `private-key` is required".to_string()
            )]
        );
    }

    #[test]
    fn the_private_key_must_be_an_openssh_keystore_item() {
        assert_eq!(
            errors("ssh, h.test, 22, username=u, private-key=nope"),
            [(
                codes::E_KEYSTORE_REF,
                "policy `P`: `private-key` references unknown keystore item `nope`".to_string()
            )]
        );
        assert_eq!(
            errors("ssh, h.test, 22, username=u, private-key=cert1"),
            [(
                codes::E_KEYSTORE_REF,
                "policy `P`: `private-key` needs an `openssh-private-key` keystore item, but `cert1` is `p12`".to_string()
            )]
        );
    }

    #[test]
    fn a_bad_idle_timeout_is_an_error() {
        let found = errors("ssh, h.test, 22, username=u, password=pw, idle-timeout=0");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, codes::E_INVALID_POLICY_PARAM);
        assert!(found[0].1.contains("`idle-timeout`"), "{found:?}");
    }

    /// A malformed entry is named by its position, never quoted: the line may
    /// come from a subscription.
    #[test]
    fn a_bad_server_fingerprint_entry_is_named_by_position() {
        let found = errors(&format!(
            "ssh, h.test, 22, username=u, password=pw, server-fingerprint=\"{ED25519},ssh-ed25519,ssh-rsa !!!,ssh-rsa AAAAC3NzaC1lZDI1NTE5AAAAIBk2No6KBq2m9VTCcHXXJBX4/A3RNr+L+yDBl5+TF9qz\""
        ));
        let messages: Vec<&str> = found.iter().map(|(_, m)| m.as_str()).collect();
        assert_eq!(
            messages,
            [
                "policy `P`: `server-fingerprint` entry 2: expected `<algorithm> <base64 key>`",
                "policy `P`: `server-fingerprint` entry 3: the key is not valid base64",
                "policy `P`: `server-fingerprint` entry 4: the key is not of the named algorithm",
            ]
        );
    }

    #[test]
    fn tls_parameters_do_not_apply() {
        let (_, failed, diags) = read("ssh, h.test, 22, username=u, password=pw, sni=x.test");
        assert!(!failed);
        assert_eq!(
            (diags[0].code, diags[0].message.as_str()),
            (
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `P`: `sni` does not apply to `ssh` policies; ignored"
            )
        );
    }

    #[test]
    fn the_password_never_shows_in_debug_output() {
        let (spec, _, _) = read("ssh, h.test, 22, username=root, password=hunter2");
        let shown = format!("{spec:?}");
        assert!(
            !shown.contains("hunter2") && !shown.contains("root"),
            "{shown}"
        );
    }
}
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
pub mod socks5;
```

换成

```rust
pub mod socks5;
pub mod ssh;
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
pub use socks5::Socks5Spec;
```

换成

```rust
pub use socks5::Socks5Spec;
pub use ssh::{HostKeyPin, SshSpec};
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
    // Check client-cert first.
    if let Some(mod_value) = modifier_values.get("client-cert") {
        // Modifier sets it: all values in the parsed policy must be the modifier's value exactly.
        let values = policy.params.get_all("client-cert");
        if values.is_empty() || !values.iter().all(|v| v == mod_value) {
            return Some(
                "`external-policy-modifier` cannot set `client-cert` on this line".to_string(),
            );
        }
    } else {
        // Modifier does not set it: no value allowed.
        let values = policy.params.get_all("client-cert");
        if !values.is_empty() {
            return Some(
                "a subscription line's own `client-cert` is not honoured (only `external-policy-modifier` may set it)".to_string(),
            );
```

换成

```rust
    // Keystore items: a client certificate, an SSH private key.
    for key in ["client-cert", "private-key"] {
        if let Some(why) = only_the_modifier_sets(policy, &modifier_values, key) {
            return Some(why);
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
    None
}
```

换成

```rust
    None
}

/// A parameter that names material of the profile: the line may carry it
/// only as the user's own modifier sets it.
fn only_the_modifier_sets(
    policy: &ProxyPolicy,
    modifier_values: &HashMap<String, String>,
    key: &str,
) -> Option<String> {
    let values = policy.params.get_all(key);
    match modifier_values.get(key) {
        // all values in the parsed policy must be the modifier's value exactly
        Some(mod_value) => (values.is_empty() || !values.iter().all(|v| v == mod_value))
            .then(|| format!("`external-policy-modifier` cannot set `{key}` on this line")),
        None => (!values.is_empty()).then(|| {
            format!(
                "a subscription line's own `{key}` is not honoured (only `external-policy-modifier` may set it)"
            )
        }),
    }
}
```

要点：
- 凭据只认命名写法（`username=`、`password=`，手册如此）：位置参数不读，照常作为多余的位置参数报告，不引用取值。`password=` 为空等于没写。
- `private-key` 在读取时就查 `[Keystore]`（P9）：条目不存在、条目是 p12 都是 `E0020`；两个都写时都保留（Task 3 先试密钥、再试口令）。
- `server-fingerprint` 的每一项必须是 `算法 base64` 两段，base64 解出的公钥编码开头的算法名必须与写的一致（P8）；错误只报第几项，不引用取值——订阅行也可能写它。
- `only_the_modifier_sets` 就是原来 `client-cert` 那段检查，参数化了键名；循环对 `client-cert`、`private-key` 各跑一次，文本不变（只换键名）。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-config spec::ssh` → 8 passed。
Run: `cargo test -p rurge-policy assemble` → 24 passed（新增 `a_subscription_ssh_line_may_not_use_the_profiles_private_keys`）。

- [ ] **Step 5: 门禁与提交**

跑门禁（41 个测试二进制，1001 通过 / 1 忽略）。

```bash
git add crates/rurge-config/src/spec/ssh.rs crates/rurge-config/src/spec/mod.rs crates/rurge-policy/src/assemble.rs
git commit -m "feat(config): ssh 策略参数 SshSpec 与 server-fingerprint 解析；订阅行自己的 private-key 进订阅安全门"
```

### Task 2: `rurge-proto-ssh` 骨架——依赖、Keystore 私钥解码、主机密钥比对、测试密钥

新 crate `rurge-proto-ssh`（设计第 3 节、M4-D2）：本任务是两个纯函数——构建期把 `[Keystore]` 的 `openssh-private-key` 条目解码成私钥（设计 4.5，P7），以及拿服务器的主机密钥与 `server-fingerprint` 比对（设计 5.2，P4）——外加供本 crate 与下游测试用的测试密钥（`testing` 特性）。出站本身在 Task 3。

**首次构建要联网**：cargo 从 crates.io 下载 russh 0.63.3 及其依赖（`Cargo.lock` 从 371 个包变为 474 个）；项目所有者已同意（M4-D2）。不要为此改用别的源或 `--offline`。

**Files:**
- Modify: `Cargo.toml`（工作区依赖：`rurge-proto-ssh`、`russh`、`rand`）
- Create: `crates/rurge-proto-ssh/Cargo.toml`、`crates/rurge-proto-ssh/src/lib.rs`
- Create: `crates/rurge-proto-ssh/src/keys.rs`（`decode_private_key`，与用例）
- Create: `crates/rurge-proto-ssh/src/pins.rs`（`host_key_allowed`，与用例）
- Create: `crates/rurge-proto-ssh/src/testing/mod.rs`（测试密钥与辅助函数）
- `Cargo.lock` 由 cargo 自己更新

**Interfaces:**
- Consumes: Task 1 的 `rurge_config::spec::HostKeyPin`；`rurge_config::{KeystoreItem, keystore::KeystoreType}`；`rurge_proto::BuildError`。
- Produces:
  - `rurge_proto_ssh::decode_private_key(item: &KeystoreItem) -> Result<PrivateKey, BuildError>`（`PrivateKey` 即 `russh::keys::PrivateKey`）
  - `rurge_proto_ssh::host_key_allowed(pins: &[HostKeyPin], key: &PublicKeyOrCertificate) -> bool`
  - 特性 `testing`（下游的 dev 依赖开启）：`rurge_proto_ssh::testing::{RSA_KEY, ED25519_WITH_PASSPHRASE, DSA_KEY}`（`ssh-keygen` 生成、只供测试的私钥文本）、`keystore_item(name: &str, key_text: &str) -> KeystoreItem`、`random_key(algorithm: Algorithm) -> PrivateKey`

- [ ] **Step 1: 依赖、crate 骨架与测试密钥**

`Cargo.toml`——把

```toml
crc32fast = "1"
```

换成

```toml
crc32fast = "1"
rurge-proto-ssh = { path = "crates/rurge-proto-ssh" }
# ring, not the default aws-lc-rs (cmake and NASM on Windows); no des, no dsa
russh = { version = "0.63.3", default-features = false, features = ["ring", "rsa"] }
rand = "0.10"
```

新建 `crates/rurge-proto-ssh/Cargo.toml`：

```toml
[package]
name = "rurge-proto-ssh"
description = "The ssh outbound of rurge: SSH dynamic forwarding, one session per policy"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
rurge-config.workspace = true
rurge-net.workspace = true
rurge-proto.workspace = true
russh.workspace = true
tokio.workspace = true
tracing.workspace = true
base64.workspace = true
rand = { workspace = true, optional = true }

[features]
# Loopback SSH server and test keys (`rurge_proto_ssh::testing`); enabled by dependants' dev-dependencies.
testing = ["dep:rand"]

[dev-dependencies]
rand.workspace = true
tokio = { workspace = true, features = ["test-util"] }

[lints]
workspace = true
```

新建 `crates/rurge-proto-ssh/src/lib.rs`：

```rust
//! The `ssh` outbound (phase 2 M4 design §5): SSH dynamic forwarding, one
//! session per policy with a `direct-tcpip` channel per connection.

pub mod keys;
pub mod pins;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use keys::decode_private_key;
pub use pins::host_key_allowed;
```

新建 `crates/rurge-proto-ssh/src/testing/mod.rs`：

```rust
//! Keys and a loopback SSH server for the tests of this crate and of its
//! dependants (feature `testing`).
//!
//! The private keys below were made with `ssh-keygen` for this test suite
//! only; they guard nothing.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rurge_config::KeystoreItem;
use rurge_config::keystore::KeystoreType;
use rurge_config::span::Span;
use russh::keys::PrivateKey;
use russh::keys::ssh_key::Algorithm;
use std::path::Path;
use std::sync::Arc;

/// An RSA key (2048 bits): making one in a debug build takes too long.
pub const RSA_KEY: &str = r"-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAABFwAAAAdzc2gtcn
NhAAAAAwEAAQAAAQEA1g1TZD0u2/ZxHgzZF13blq+aKGB4mmR/q7nmbN8vpltGszI2PV+G
vviYK7S12l4NbwRn3zJtGExapyfQ/U+dPpwTd8avQEKRG/vO+Nkv/xoVSb+Yh2bmILyWrE
HETCM/aaXQitcOUd8WwyKL1si3y18x+xCx+9rkUtKz8xU8JnMQReIxk0eucvtex4OVJQDf
BEeqMiEsn32zKRwgOrKgrX25e/jEQDKWrpRC2GKBRk5IEHrx9kPHYfDtjDT7ulztlab6R0
NZOR8dSmil/4YN9RY3FF0fMXV2BBlpJZy2kFYFV7YMmEYWy+k/in4gicXCAEPcgsOfoxvl
3dS0xSlv2wAAA8DvOb8O7zm/DgAAAAdzc2gtcnNhAAABAQDWDVNkPS7b9nEeDNkXXduWr5
ooYHiaZH+rueZs3y+mW0azMjY9X4a++JgrtLXaXg1vBGffMm0YTFqnJ9D9T50+nBN3xq9A
QpEb+8742S//GhVJv5iHZuYgvJasQcRMIz9ppdCK1w5R3xbDIovWyLfLXzH7ELH72uRS0r
PzFTwmcxBF4jGTR65y+17Hg5UlAN8ER6oyISyffbMpHCA6sqCtfbl7+MRAMpaulELYYoFG
TkgQevH2Q8dh8O2MNPu6XO2VpvpHQ1k5Hx1KaKX/hg31FjcUXR8xdXYEGWklnLaQVgVXtg
yYRhbL6T+KfiCJxcIAQ9yCw5+jG+Xd1LTFKW/bAAAAAwEAAQAAAQAURCy6F+Tg5KNvIe5H
/RX2XWfuHLwuegdwfehoNHVxfcDi5IUoKGw8lpLpyHFTXIZPFY60HjUgENKgcu+hnDEaJX
Lea0xafDL7AEtnWkDmGVUcp2xMnZx6SwDFDHEGeGvfl9h33Ma5T7L7BMFSs6xbMAcuazU+
0Em/4b0x7bfFOAFky/5T91eR0qLSvEPZb3fivbKMWYI99h77ovMficF2AKkP4NNNhgYt47
/VmFgt+qQaFNwghcv8zSQfz4oVA10jcPGHkNIJc1l+Od3tnHH0dwBX6c6yEVBZ7RUCdrCU
aGQfwH8iycz+QKDRvDpFZvhBZhwtkQ+d7pd4a+yP/Qy5AAAAgCQTHCFW2ANFGgOSQhcPDW
EWuPGM2Qz6xStCuQ0kQzPLwD/+IP8WBnO7rKlqTrcsnSYQE0igcPyXXckET08fSTzym3Xo
tG5D5Ny4+kZ6ZTzcChWg9a/dqd/hScvoA0BZLjGhAZX5jy7I3Kv+fa3rhsxqKhxwB2i94w
MmZK25IxVAAAAAgQDs8GpkMyUhLb3/U4eXo6n0bIWfeHgu0xNK1jX7pUOcJOX+2T4GQGiP
vJPjAJXgkAabCiBd6Y2s9lmhnfdlpVVmdqbIhdk+biXYbgEIBEMerA89v0BGjMViB5mccL
GGJaiUOKZ/I+T0c+eek64buaD0texTfJPntOPOsCN8xH5e7wAAAIEA50WRavAYLJkweVjf
890KdC0qvYgGibkXpe48DMyujbklsR/TUuw4Rzo0SYI5jkG8eKPK1ITFwoSg9C3KUYgJIW
41/qiFo7a95wTH0ntj9VAAYXgCilezpnBfVT66yvYrNnKpxqqYA9pFPwE/qP15ronnaoHd
fFB8G1hLZaLyvdUAAAAKcnVyZ2UtdGVzdAE=
-----END OPENSSH PRIVATE KEY-----
";

/// An Ed25519 key protected by the passphrase `pw`.
pub const ED25519_WITH_PASSPHRASE: &str = r"-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAACmFlczI1Ni1jdHIAAAAGYmNyeXB0AAAAGAAAABBgagZIBd
zn7HTsSPluzWLtAAAAAQAAAAEAAAAzAAAAC3NzaC1lZDI1NTE5AAAAIJ6BQgWnxd54MYop
XpKyvebv6xD3l1QmX3/slN+Cnn7wAAAAkCmr6vNBiyz/Hg05H08eYsxM5hgpg9PEoxWkFH
G2rm9daIM1Th+HDfi/QfxAtnDIja0lJ6YAg1913VDICr5F25kZYznc58gDaiefgLb/V9o6
hOGIFUfm3s1e1GZ/Wg6eoFb/F/EU4YH4Zqqfkd5hehJI4VGGU9iDhYBxmgZp293DLuG89C
mICxruBuMF6RghVg==
-----END OPENSSH PRIVATE KEY-----
";

/// A DSA key (1024 bits), which rurge does not accept.
pub const DSA_KEY: &str = r"-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAABsgAAAAdzc2gtZH
NzAAAAgQDibpJx8ewUMR81iEGJkOCSb/Y0/RClXE/LEKoctha1r9R03XT/US9pD7Y3krUg
zewzBoL5fsGmyHoe6x75+hJlTj7dAb0uYWy5cg269LG0/B8rtIHXIbd1xgrbFKWleFIsMG
8gRTbvPGlbL6Nd9vr2lq2R1iAKLeUmIuEaulOZ+wAAABUA6hnzf4B17PNL6w1MB1HjlnF6
PmcAAACARJn1yli87Zb8qA7C4ZUj+bsxZGka4Rkl769jDSbPhRB9bQRIcUrPe59A1cEi70
hazvQMVuJJtoa7S+cKUi0wFnJ5djKi/5QboaHhfjweN6Je9wKOFZqdTJrqS4UJ+1kov1lI
06B6VQkwxorBTwPXnlHWP+Anz+m6qwN2crHOADIAAACBAJDUZ8Tc2Y9z0KOXJTcCIJz/9p
4vNfjiQFoag0L2GV1wndwXjEerjcyzZrHsDgzIwF3bvnuihEhhHU2izbQwoDr8k4Z8NYhp
SpGZK6nS/pF6JWjgwz5eK1Z2+ypGEqaS36F4O96/NpXqROWoU827SUX2xzId+6WFuceO0w
25Fw2uAAAB6LeBvuq3gb7qAAAAB3NzaC1kc3MAAACBAOJuknHx7BQxHzWIQYmQ4JJv9jT9
EKVcT8sQqhy2FrWv1HTddP9RL2kPtjeStSDN7DMGgvl+wabIeh7rHvn6EmVOPt0BvS5hbL
lyDbr0sbT8Hyu0gdcht3XGCtsUpaV4UiwwbyBFNu88aVsvo132+vaWrZHWIAot5SYi4Rq6
U5n7AAAAFQDqGfN/gHXs80vrDUwHUeOWcXo+ZwAAAIBEmfXKWLztlvyoDsLhlSP5uzFkaR
rhGSXvr2MNJs+FEH1tBEhxSs97n0DVwSLvSFrO9AxW4km2hrtL5wpSLTAWcnl2MqL/lBuh
oeF+PB43ol73Ao4Vmp1MmupLhQn7WSi/WUjToHpVCTDGisFPA9eeUdY/4CfP6bqrA3Zysc
4AMgAAAIEAkNRnxNzZj3PQo5clNwIgnP/2ni81+OJAWhqDQvYZXXCd3BeMR6uNzLNmsewO
DMjAXdu+e6KESGEdTaLNtDCgOvyThnw1iGlKkZkrqdL+kXolaODDPl4rVnb7KkYSppLfoX
g73r82lepE5ahTzbtJRfbHMh37pYW5x47TDbkXDa4AAAAVAJ95JG/htpe16yIM7dVN9roj
zkgUAAAACnJ1cmdlLXRlc3QBAgMEBQYH
-----END OPENSSH PRIVATE KEY-----
";

/// A keystore item holding `key_text` the way `[Keystore]` does: the whole
/// key file, in Base64.
pub fn keystore_item(name: &str, key_text: &str) -> KeystoreItem {
    KeystoreItem {
        name: name.into(),
        kind: KeystoreType::OpensshPrivateKey,
        base64: STANDARD.encode(key_text),
        password: None,
        unknown: Vec::new(),
        span: Span::new(Arc::from(Path::new("t.conf")), 1),
    }
}

/// A fresh Ed25519 or ECDSA key (for RSA, `RSA_KEY`).
pub fn random_key(algorithm: Algorithm) -> PrivateKey {
    PrivateKey::random(&mut rand::rng(), algorithm).expect("a random key")
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-proto-ssh`
Expected: FAIL，编译错误（首次运行会先下载并编译 russh 一族，要几分钟）——

```text
error[E0583]: file not found for module `keys`
 --> crates\rurge-proto-ssh\src\lib.rs:4:1
error[E0583]: file not found for module `pins`
 --> crates\rurge-proto-ssh\src\lib.rs:5:1
error: could not compile `rurge-proto-ssh` (lib) due to 2 previous errors
```

- [ ] **Step 3: 实现（新模块自带用例）**

新建 `crates/rurge-proto-ssh/src/keys.rs`：

```rust
//! `[Keystore]` OpenSSH private keys, decoded at build time (phase 2 M4
//! design 4.5).

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use rurge_config::KeystoreItem;
use rurge_proto::BuildError;
use russh::keys::ssh_key::{self, Algorithm};
use russh::keys::{Error, PrivateKey, decode_secret_key};

/// Decodes the private key an `ssh` policy names. The error names the item;
/// it never repeats any of the material.
pub fn decode_private_key(item: &KeystoreItem) -> Result<PrivateKey, BuildError> {
    let name = &item.name;
    let file = STANDARD
        .decode(&item.base64)
        .or_else(|_| STANDARD_NO_PAD.decode(&item.base64))
        .map_err(|_| {
            BuildError::new(format!(
                "keystore item `{name}`: `base64` is not valid Base64"
            ))
        })?;
    let not_a_key = || {
        BuildError::new(format!(
            "keystore item `{name}` is not an OpenSSH private key"
        ))
    };
    let unsupported = || {
        BuildError::new(format!(
            "keystore item `{name}` is not an Ed25519, ECDSA or RSA key"
        ))
    };
    let text = String::from_utf8(file).map_err(|_| not_a_key())?;
    let key = decode_secret_key(&text, None).map_err(|e| match e {
        // the manual's `password` is for p12 files only
        Error::KeyIsEncrypted => BuildError::new(format!(
            "keystore item `{name}` is protected by a passphrase, which rurge cannot use; remove the passphrase"
        )),
        Error::UnsupportedKeyType { .. }
        | Error::UnknownAlgorithm(_)
        | Error::SshKey(
            ssh_key::Error::AlgorithmUnsupported { .. } | ssh_key::Error::AlgorithmUnknown,
        ) => unsupported(),
        _ => not_a_key(),
    })?;
    // a DSA key decodes (only signing with one needs the `dsa` feature), and
    // so does a security key that needs its hardware
    match key.algorithm() {
        Algorithm::Ed25519 | Algorithm::Ecdsa { .. } | Algorithm::Rsa { .. } => Ok(key),
        _ => Err(unsupported()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{DSA_KEY, ED25519_WITH_PASSPHRASE, RSA_KEY, keystore_item, random_key};
    use russh::keys::ssh_key::{EcdsaCurve, LineEnding};

    #[test]
    fn ed25519_ecdsa_and_rsa_keys_decode() {
        for algorithm in [
            Algorithm::Ed25519,
            Algorithm::Ecdsa {
                curve: EcdsaCurve::NistP256,
            },
            Algorithm::Ecdsa {
                curve: EcdsaCurve::NistP384,
            },
            Algorithm::Ecdsa {
                curve: EcdsaCurve::NistP521,
            },
        ] {
            let key = random_key(algorithm.clone());
            let text = key.to_openssh(LineEnding::LF).unwrap();
            let decoded = decode_private_key(&keystore_item("key1", &text)).unwrap();
            assert_eq!(decoded.algorithm(), algorithm);
            assert_eq!(decoded.public_key(), key.public_key());
        }
        let rsa = decode_private_key(&keystore_item("key1", RSA_KEY)).unwrap();
        assert!(matches!(rsa.algorithm(), Algorithm::Rsa { .. }));
    }

    /// The failures name the item and what is wrong with it, never the
    /// material.
    #[test]
    fn keys_rurge_cannot_use_are_named_not_quoted() {
        let cases = [
            (
                keystore_item("key1", ED25519_WITH_PASSPHRASE),
                "keystore item `key1` is protected by a passphrase, which rurge cannot use; remove the passphrase",
            ),
            (
                keystore_item("key1", DSA_KEY),
                "keystore item `key1` is not an Ed25519, ECDSA or RSA key",
            ),
            (
                keystore_item("key1", "hello, world"),
                "keystore item `key1` is not an OpenSSH private key",
            ),
        ];
        for (item, expected) in cases {
            let message = decode_private_key(&item).unwrap_err().message;
            assert_eq!(message, expected);
            assert!(!message.contains(&item.base64[..16]));
        }
        let mut bad = keystore_item("key1", RSA_KEY);
        bad.base64 = "!!not base64!!".into();
        assert_eq!(
            decode_private_key(&bad).unwrap_err().message,
            "keystore item `key1`: `base64` is not valid Base64"
        );
    }
}
```

新建 `crates/rurge-proto-ssh/src/pins.rs`：

```rust
//! `server-fingerprint` (phase 2 M4 design 5.2).

use rurge_config::spec::HostKeyPin;
use russh::keys::PublicKeyOrCertificate;
use russh::keys::ssh_key::encoding::Encode as _;

/// Whether the server's host key is one of `pins`; with no pins, every key
/// is. A host certificate is judged by the key it certifies.
pub fn host_key_allowed(pins: &[HostKeyPin], key: &PublicKeyOrCertificate) -> bool {
    if pins.is_empty() {
        return true;
    }
    let blob = match key {
        PublicKeyOrCertificate::PublicKey { key, .. } => key.to_bytes(),
        PublicKeyOrCertificate::Certificate(cert) => cert
            .public_key()
            .encode_vec()
            .map_err(russh::keys::ssh_key::Error::from),
    };
    blob.is_ok_and(|blob| pins.iter().any(|pin| pin.blob == blob))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::random_key;
    use russh::keys::ssh_key::certificate::{Builder, CertType};
    use russh::keys::ssh_key::{Algorithm, PublicKey};

    fn pin_of(key: &PublicKey) -> HostKeyPin {
        HostKeyPin {
            algorithm: key.algorithm().to_string(),
            blob: key.to_bytes().unwrap(),
        }
    }

    fn plain(key: &PublicKey) -> PublicKeyOrCertificate {
        PublicKeyOrCertificate::PublicKey {
            key: key.clone(),
            hash_alg: None,
        }
    }

    #[test]
    fn a_pinned_key_passes_and_any_other_does_not() {
        let server = random_key(Algorithm::Ed25519);
        let other = random_key(Algorithm::Ed25519);
        let pins = [pin_of(other.public_key()), pin_of(server.public_key())];
        assert!(host_key_allowed(&pins, &plain(server.public_key())));
        assert!(!host_key_allowed(&pins[..1], &plain(server.public_key())));
    }

    #[test]
    fn without_pins_every_key_passes() {
        let server = random_key(Algorithm::Ed25519);
        assert!(host_key_allowed(&[], &plain(server.public_key())));
    }

    /// A host certificate is judged by the key it certifies, whoever signed
    /// it.
    #[test]
    fn a_host_certificate_is_judged_by_its_key() {
        let host = random_key(Algorithm::Ed25519);
        let ca = random_key(Algorithm::Ed25519);
        let mut builder = Builder::new_with_random_nonce(
            &mut rand::rng(),
            host.public_key().key_data().clone(),
            0,
            u64::MAX,
        )
        .unwrap();
        builder.cert_type(CertType::Host).unwrap();
        builder.valid_principal("s.test").unwrap();
        let cert = builder.sign(&ca).unwrap();
        let shown = PublicKeyOrCertificate::Certificate(cert);
        assert!(host_key_allowed(&[pin_of(host.public_key())], &shown));
        assert!(!host_key_allowed(&[pin_of(ca.public_key())], &shown));
    }
}
```

要点：
- Keystore 的 `base64` 是整个私钥文件的 Base64（带不带填充都收）；解出来不是 UTF-8 或不是 OpenSSH 私钥格式时 ``… is not an OpenSSH private key``。
- 带口令的私钥靠 `decode_secret_key(text, None)` 返回的 `Error::KeyIsEncrypted` 认出来，给专门的文本（P7）；解码成功后再按算法只收 Ed25519 / ECDSA / RSA——**不开 `dsa` 特性时 DSA 私钥照样能解码**，要硬件的 `sk-*` 安全密钥也能，二者都在这里挡掉。
- 所有文本只点名条目，不引用任何内容；用例断言文本里没有 `base64` 的前 16 个字符。
- `host_key_allowed`：`server-fingerprint` 为空一律接受；否则拿服务器公钥的线上编码（开头就是算法名）与每一项的 `blob` 逐字节比；主机证书取它认证的那把公钥（`encode_vec`），与签发它的 CA 无关。
- 测试用的 RSA 私钥是固定的（debug 构建里现场生成 RSA 太慢）；Ed25519 / ECDSA 现场生成（`random_key`，随机源来自 `rand` 0.10）。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-proto-ssh` → 5 passed（`keys::tests` 2 条、`pins::tests` 3 条）。
Run: `cargo tree -p rurge-proto-ssh -e features -i russh` → russh 只开了 `ring` 与 `rsa` 两个特性（没有 `aws-lc-rs`、`des`、`dsa`、`flate2`）。

- [ ] **Step 5: 门禁与提交**

跑门禁（43 个测试二进制，1006 通过 / 1 忽略）。

```bash
git add Cargo.toml Cargo.lock crates/rurge-proto-ssh
git commit -m "feat(proto-ssh): 新 crate rurge-proto-ssh——Keystore 私钥解码、主机密钥比对与测试密钥（russh 0.63.3，ring 后端）"
```

### Task 3: `SshOutbound`——会话、单飞、通道、断线重建、认证与错误文本；`FakeSsh`

每个策略一条 SSH 会话，每次拨号在上面开一个 `direct-tcpip` 通道（设计 5.1）。建会话：经策略自己的连接器（`interface`、`underlying-proxy` 都在这里生效）连到服务器 → 配了 Shadow TLS 就先叠上（M2c 的 `Stack`，P18）→ russh `connect_stream` 握手（主机密钥按 Task 2 的 `host_key_allowed` 判断，P4）→ 先密钥、后口令认证（RSA 私钥的签名哈希见 P3）。同时进来的拨号在会话槽的锁上等同一次握手（P13）；会话已断时丢弃它、重建一次、再试一次（P5）。协商的算法见 P2，错误文本见 P14。空闲断开、保活与没配指纹的告警在 Task 4。

`FakeSsh` 是 russh 自己的服务端，只听回环（P17）：口令与公钥登录，`direct-tcpip` 通道接到它所请求的回环地址。

**Files:**
- Modify: `crates/rurge-proto-ssh/src/testing/mod.rs`（`mod server;` 与导出）
- Create: `crates/rurge-proto-ssh/src/testing/server.rs`（`FakeSsh`、`FakeSshOpts`）
- Modify: `crates/rurge-proto-ssh/src/lib.rs`（`pub mod outbound;` 与导出）、`crates/rurge-proto-ssh/Cargo.toml`（`rustls`）
- Create: `crates/rurge-proto-ssh/src/outbound.rs`（`SshOutbound`，与用例）

**Interfaces:**
- Consumes: Task 1 的 `SshSpec`、`HostKeyPin`；Task 2 的 `decode_private_key`、`host_key_allowed`、`testing::{random_key, keystore_item, RSA_KEY}`；`rurge_proto::transport::Stack`、`rurge_proto::build::shadow_tls_client`、`rurge_proto::{Outbound, OutboundError, BuildError}`、`rurge_net::connector::{Connector, ConnectOpts, Target, BoxedStream}`、`rurge_config::spec::ShadowTlsOpts`。
- Produces:
  - `SshOutbound::new(name: &str, server: Target, ssh: &SshSpec, shadow_tls: Option<&ShadowTlsOpts>, keystore: &[KeystoreItem], roots: Arc<RootCertStore>, connector: Arc<dyn Connector>) -> Result<SshOutbound, BuildError>`——构建期就解码私钥（`rurge check` 的干构建因此能报出用不了的私钥）；`impl Outbound for SshOutbound`
  - `rurge_proto_ssh::SshOutbound` 的导出
  - `testing::{FakeSsh, FakeSshOpts}`：`FakeSshOpts { user: String, password: Option<String>, keys: Vec<PublicKey>, refuse_channels: bool, surge_minimum: bool }`（`Clone + Debug + Default`）；`FakeSsh::start(opts).await`、字段 `addr: SocketAddr`、`host_key: PublicKey`，方法 `logins() -> usize`、`end_sessions().await`（Task 4、5 再加字段与方法）

- [ ] **Step 1: 先写假服务端，并声明新模块**

`crates/rurge-proto-ssh/src/testing/mod.rs`——把

```rust
use std::sync::Arc;
```

换成

```rust
use std::sync::Arc;

mod server;

pub use server::{FakeSsh, FakeSshOpts};
```

新建 `crates/rurge-proto-ssh/src/testing/server.rs`：

```rust
//! `FakeSsh`: a loopback SSH server for the tests (russh's own server).

use super::random_key;
use russh::keys::PublicKey;
use russh::keys::ssh_key::Algorithm;
use russh::server::{self, Auth, ChannelOpenHandle, Msg, run_stream};
use russh::{Channel, ChannelOpenFailure, Disconnect, Preferred, cipher, kex};
use std::borrow::Cow;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// Who may log in, and how the server behaves.
#[derive(Clone, Debug, Default)]
pub struct FakeSshOpts {
    pub user: String,
    pub password: Option<String>,
    /// Public keys that may log in.
    pub keys: Vec<PublicKey>,
    /// Refuse every `direct-tcpip` channel (`connect failed`).
    pub refuse_channels: bool,
    /// Offer only what Surge's manual requires: `curve25519-sha256` and
    /// `aes128-gcm@openssh.com`.
    pub surge_minimum: bool,
}

struct State {
    opts: FakeSshOpts,
    logins: AtomicUsize,
    sessions: Mutex<Vec<server::Handle>>,
}

/// Serves SSH on a loopback port: password and public-key logins, and
/// `direct-tcpip` channels bridged to the loopback address they name.
pub struct FakeSsh {
    pub addr: SocketAddr,
    /// Its host key.
    pub host_key: PublicKey,
    state: Arc<State>,
    task: JoinHandle<()>,
}

impl FakeSsh {
    pub async fn start(opts: FakeSshOpts) -> FakeSsh {
        let host = random_key(Algorithm::Ed25519);
        let host_key = host.public_key().clone();
        let mut preferred = Preferred::default();
        if opts.surge_minimum {
            preferred.kex = Cow::Owned(vec![
                kex::CURVE25519,
                kex::EXTENSION_SUPPORT_AS_SERVER,
                kex::EXTENSION_OPENSSH_STRICT_KEX_AS_SERVER,
            ]);
            preferred.cipher = Cow::Owned(vec![cipher::AES_128_GCM]);
        }
        let config = Arc::new(server::Config {
            keys: vec![host],
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            inactivity_timeout: None,
            preferred,
            ..Default::default()
        });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = Arc::new(State {
            opts,
            logins: AtomicUsize::new(0),
            sessions: Mutex::new(Vec::new()),
        });
        let accepting = state.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let peer = Peer {
                    state: accepting.clone(),
                };
                let config = config.clone();
                let state = accepting.clone();
                tokio::spawn(async move {
                    if let Ok(running) = run_stream(config, stream, peer).await {
                        state.sessions.lock().unwrap().push(running.handle());
                        let _ = running.await;
                    }
                });
            }
        });
        FakeSsh {
            addr,
            host_key,
            state,
            task,
        }
    }

    /// Logins that succeeded so far: one per session.
    pub fn logins(&self) -> usize {
        self.state.logins.load(Ordering::SeqCst)
    }

    /// Ends every session, as a server restart would.
    pub async fn end_sessions(&self) {
        let sessions = std::mem::take(&mut *self.state.sessions.lock().unwrap());
        for session in sessions {
            let _ = session
                .disconnect(Disconnect::ByApplication, String::new(), String::new())
                .await;
        }
    }
}

impl Drop for FakeSsh {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Peer {
    state: Arc<State>,
}

impl Peer {
    fn verdict(&self, ok: bool) -> Auth {
        if ok {
            self.state.logins.fetch_add(1, Ordering::SeqCst);
            Auth::Accept
        } else {
            Auth::reject()
        }
    }
}

impl server::Handler for Peer {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        let opts = &self.state.opts;
        Ok(self.verdict(user == opts.user && opts.password.as_deref() == Some(password)))
    }

    async fn auth_publickey(&mut self, user: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
        let opts = &self.state.opts;
        let known = opts.keys.iter().any(|k| k.key_data() == key.key_data());
        Ok(self.verdict(user == opts.user && known))
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host_to_connect: &str,
        port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        let target = u16::try_from(port_to_connect)
            .ok()
            .filter(|_| !self.state.opts.refuse_channels);
        let tcp = match target {
            Some(port) => TcpStream::connect((host_to_connect, port)).await.ok(),
            None => None,
        };
        match tcp {
            Some(mut tcp) => {
                reply.accept().await;
                tokio::spawn(async move {
                    let mut channel = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut channel, &mut tcp).await;
                });
            }
            None => reply.reject(ChannelOpenFailure::ConnectFailed).await,
        }
        Ok(())
    }
}
```

`crates/rurge-proto-ssh/src/lib.rs`——把

```rust
pub mod keys;
```

换成

```rust
pub mod keys;
pub mod outbound;
```

`crates/rurge-proto-ssh/src/lib.rs`——把

```rust
pub use keys::decode_private_key;
```

换成

```rust
pub use keys::decode_private_key;
pub use outbound::SshOutbound;
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-proto-ssh`
Expected: FAIL，编译错误——

```text
error[E0583]: file not found for module `outbound`
 --> crates\rurge-proto-ssh\src\lib.rs:5:1
error: could not compile `rurge-proto-ssh` (lib) due to 1 previous error
error: could not compile `rurge-proto-ssh` (lib test) due to 1 previous error
```

- [ ] **Step 3: 实现（新模块自带用例）**

`crates/rurge-proto-ssh/Cargo.toml`——把

```toml
russh.workspace = true
```

换成

```toml
russh.workspace = true
rustls.workspace = true
```

新建 `crates/rurge-proto-ssh/src/outbound.rs`：

```rust
//! `SshOutbound` (phase 2 M4 design §5): one SSH session per policy, a
//! `direct-tcpip` channel per connection.

use crate::keys::decode_private_key;
use crate::pins::host_key_allowed;
use rurge_config::KeystoreItem;
use rurge_config::spec::{HostKeyPin, ShadowTlsOpts, SshSpec};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rurge_proto::build::shadow_tls_client;
use rurge_proto::transport::Stack;
use rurge_proto::{BuildError, Outbound, OutboundError};
use russh::client::{self, Handle};
use russh::keys::ssh_key::Algorithm;
use russh::keys::{HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::{ChannelOpenFailure, Error, Preferred, cipher};
use rustls::RootCertStore;
use std::borrow::Cow;
use std::sync::Arc;
use tokio::sync::Mutex;

/// The session's side of the conversation: the server's host key is checked
/// against `server-fingerprint`.
struct Client {
    pins: Arc<[HostKeyPin]>,
}

impl client::Handler for Client {
    type Error = Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Error> {
        Ok(host_key_allowed(&self.pins, key))
    }
}

/// russh's defaults, with the one cipher Surge's manual requires added
/// (`aes128-gcm@openssh.com`, "Algorithm Requirements") and the SHA-1 host
/// key signature (`ssh-rsa`) left out.
fn preferred() -> Preferred {
    Preferred {
        cipher: Cow::Owned(vec![
            cipher::CHACHA20_POLY1305,
            cipher::AES_256_GCM,
            cipher::AES_128_GCM,
            cipher::AES_256_CTR,
            cipher::AES_192_CTR,
            cipher::AES_128_CTR,
        ]),
        key: Cow::Owned(
            Preferred::DEFAULT
                .key
                .iter()
                .filter(|algorithm| !matches!(algorithm, Algorithm::Rsa { hash: None }))
                .cloned()
                .collect(),
        ),
        ..Preferred::DEFAULT
    }
}

struct Session {
    handle: Handle<Client>,
}

enum ChannelError {
    /// The session is gone: another one may do.
    Gone,
    Refused(ChannelOpenFailure),
}

impl ChannelError {
    fn into_outbound(self) -> OutboundError {
        match self {
            ChannelError::Gone => OutboundError::Proxy("ssh: the session closed".to_string()),
            // the reason code only: the server's own text is not repeated
            ChannelError::Refused(reason) => OutboundError::Proxy(format!(
                "ssh: the server refused the channel ({})",
                match reason {
                    ChannelOpenFailure::AdministrativelyProhibited =>
                        "administratively prohibited".to_string(),
                    ChannelOpenFailure::ConnectFailed => "connect failed".to_string(),
                    ChannelOpenFailure::UnknownChannelType => "unknown channel type".to_string(),
                    ChannelOpenFailure::ResourceShortage => "resource shortage".to_string(),
                    ChannelOpenFailure::Other { code, .. } => format!("code {code}"),
                }
            )),
        }
    }
}

pub struct SshOutbound {
    name: String,
    stack: Stack,
    user: String,
    password: Option<String>,
    key: Option<Arc<PrivateKey>>,
    pins: Arc<[HostKeyPin]>,
    /// One handshake at a time: dials that come in meanwhile wait for it.
    session: Mutex<Option<Arc<Session>>>,
}

impl SshOutbound {
    /// Decodes the private key now, so that `rurge check` reports a key rurge
    /// cannot use.
    pub fn new(
        name: &str,
        server: Target,
        ssh: &SshSpec,
        shadow_tls: Option<&ShadowTlsOpts>,
        keystore: &[KeystoreItem],
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> Result<SshOutbound, BuildError> {
        let key = match &ssh.private_key {
            None => None,
            Some(item) => {
                let item = keystore.iter().find(|k| &k.name == item).ok_or_else(|| {
                    BuildError::new(format!("keystore item `{item}` does not exist"))
                })?;
                Some(Arc::new(decode_private_key(item)?))
            }
        };
        let shadow_tls = shadow_tls_client(shadow_tls, None, &server.host, roots)?;
        Ok(SshOutbound {
            name: name.to_string(),
            stack: Stack::new(connector, server, shadow_tls, None, None),
            user: ssh.username.expose().clone(),
            password: ssh.password.as_ref().map(|p| p.expose().clone()),
            key,
            pins: ssh.host_keys.clone().into(),
            session: Mutex::new(None),
        })
    }

    async fn dial(
        &self,
        target: &Target,
        opts: &ConnectOpts,
    ) -> Result<BoxedStream, OutboundError> {
        let session = self.session(opts).await?;
        match open_channel(&session, target).await {
            Err(ChannelError::Gone) => {
                // the session ended since it was last used: one new one
                self.forget(&session).await;
                let session = self.session(opts).await?;
                open_channel(&session, target)
                    .await
                    .map_err(ChannelError::into_outbound)
            }
            other => other.map_err(ChannelError::into_outbound),
        }
    }

    async fn session(&self, opts: &ConnectOpts) -> Result<Arc<Session>, OutboundError> {
        let mut slot = self.session.lock().await;
        if let Some(session) = slot.as_ref().filter(|s| !s.handle.is_closed()) {
            return Ok(session.clone());
        }
        let session = Arc::new(self.establish(opts).await?);
        *slot = Some(session.clone());
        Ok(session)
    }

    async fn forget(&self, dead: &Arc<Session>) {
        let mut slot = self.session.lock().await;
        if slot.as_ref().is_some_and(|s| Arc::ptr_eq(s, dead)) {
            *slot = None;
        }
    }

    async fn establish(&self, opts: &ConnectOpts) -> Result<Session, OutboundError> {
        let stream = self.stack.open(opts).await?;
        let config = Arc::new(client::Config {
            preferred: preferred(),
            ..Default::default()
        });
        let client = Client {
            pins: self.pins.clone(),
        };
        let mut handle = client::connect_stream(config, stream, client)
            .await
            .map_err(handshake_error)?;
        self.authenticate(&mut handle).await?;
        Ok(Session { handle })
    }

    /// The key first, then the password, as the OpenSSH client does.
    async fn authenticate(&self, handle: &mut Handle<Client>) -> Result<(), OutboundError> {
        if let Some(key) = &self.key {
            let hash = if matches!(key.algorithm(), Algorithm::Rsa { .. }) {
                let listed = handle
                    .best_supported_rsa_hash()
                    .await
                    .map_err(handshake_error)?;
                Some(rsa_hash(listed))
            } else {
                None
            };
            let key = PrivateKeyWithHashAlg::new(key.clone(), hash);
            let result = handle
                .authenticate_publickey(&self.user, key)
                .await
                .map_err(handshake_error)?;
            if result.success() {
                return Ok(());
            }
        }
        if let Some(password) = &self.password {
            let result = handle
                .authenticate_password(&self.user, password)
                .await
                .map_err(handshake_error)?;
            if result.success() {
                return Ok(());
            }
        }
        Err(OutboundError::Proxy(
            "ssh: authentication failed".to_string(),
        ))
    }
}

async fn open_channel(session: &Session, target: &Target) -> Result<BoxedStream, ChannelError> {
    let opened = session
        .handle
        .channel_open_direct_tcpip(
            target.host.to_string(),
            u32::from(target.port),
            "127.0.0.1",
            0,
        )
        .await;
    match opened {
        Ok(channel) => Ok(Box::new(channel.into_stream())),
        Err(Error::ChannelOpenFailure(reason)) => Err(ChannelError::Refused(reason)),
        Err(_) => Err(ChannelError::Gone),
    }
}

/// The hash an RSA key signs with: SHA-512 when the server lists it in
/// `server-sig-algs`, else SHA-256 — also when the server lists nothing.
/// Never SHA-1 (`ssh-rsa`), not even for a server that lists only that.
fn rsa_hash(listed: Option<Option<HashAlg>>) -> HashAlg {
    match listed {
        Some(Some(HashAlg::Sha512)) => HashAlg::Sha512,
        _ => HashAlg::Sha256,
    }
}

/// Fixed texts: what the server sent is not repeated. The connection is up
/// by now, so an I/O error too is the handshake failing.
fn handshake_error(e: Error) -> OutboundError {
    match e {
        Error::UnknownKey => OutboundError::Proxy(
            "ssh: the server's host key is not one of server-fingerprint".to_string(),
        ),
        Error::NoCommonAlgo { .. } => OutboundError::Proxy(
            "ssh: the handshake failed (no algorithm in common with the server)".to_string(),
        ),
        _ => OutboundError::Proxy("ssh: the handshake failed".to_string()),
    }
}

impl Outbound for SshOutbound {
    fn name(&self) -> &str {
        &self.name
    }

    fn connect_tcp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            match tokio::time::timeout(opts.timeout, self.dial(target, opts)).await {
                Ok(result) => result,
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeSsh, FakeSshOpts, RSA_KEY, keystore_item, random_key};
    use rurge_config::HostName;
    use rurge_config::spec::ssh::DEFAULT_IDLE_TIMEOUT;
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use russh::keys::PublicKey;
    use russh::keys::ssh_key::{EcdsaCurve, LineEnding};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A loopback server that echoes what it reads.
    async fn echo() -> Target {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let (mut r, mut w) = s.split();
                    let _ = tokio::io::copy(&mut r, &mut w).await;
                });
            }
        });
        Target::new(HostName::Ip(addr.ip()), addr.port())
    }

    fn spec(password: Option<&str>, key: Option<&str>, host_keys: Vec<HostKeyPin>) -> SshSpec {
        SshSpec {
            username: "u".into(),
            password: password.map(Into::into),
            private_key: key.map(str::to_string),
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            host_keys,
        }
    }

    fn pin(key: &PublicKey) -> HostKeyPin {
        HostKeyPin {
            algorithm: key.algorithm().to_string(),
            blob: key.to_bytes().unwrap(),
        }
    }

    fn outbound_to(
        addr: std::net::SocketAddr,
        ssh: &SshSpec,
        keystore: &[KeystoreItem],
    ) -> SshOutbound {
        SshOutbound::new(
            "S",
            Target::new(HostName::Ip(addr.ip()), addr.port()),
            ssh,
            None,
            keystore,
            Arc::new(RootCertStore::empty()),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .unwrap()
    }

    async fn fake(opts: FakeSshOpts) -> FakeSsh {
        FakeSsh::start(FakeSshOpts {
            user: "u".into(),
            ..opts
        })
        .await
    }

    async fn round_trip(outbound: &SshOutbound, target: &Target) -> Result<(), OutboundError> {
        let mut stream = outbound
            .connect_tcp(target, &ConnectOpts::default())
            .await?;
        stream.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");
        Ok(())
    }

    #[tokio::test]
    async fn password_and_key_logins_forward_a_connection() {
        let ed25519 = random_key(Algorithm::Ed25519);
        let ecdsa = random_key(Algorithm::Ecdsa {
            curve: EcdsaCurve::NistP256,
        });
        let rsa = crate::decode_private_key(&keystore_item("rsa", RSA_KEY)).unwrap();
        let server = fake(FakeSshOpts {
            password: Some("pw".into()),
            keys: vec![
                ed25519.public_key().clone(),
                ecdsa.public_key().clone(),
                rsa.public_key().clone(),
            ],
            ..Default::default()
        })
        .await;
        let keystore = [
            keystore_item("ed", &ed25519.to_openssh(LineEnding::LF).unwrap()),
            keystore_item("ec", &ecdsa.to_openssh(LineEnding::LF).unwrap()),
            keystore_item("rsa", RSA_KEY),
        ];
        let target = echo().await;
        for ssh in [
            spec(Some("pw"), None, vec![]),
            spec(None, Some("ed"), vec![]),
            spec(None, Some("ec"), vec![]),
            spec(None, Some("rsa"), vec![]),
        ] {
            round_trip(&outbound_to(server.addr, &ssh, &keystore), &target)
                .await
                .unwrap();
        }
        assert_eq!(server.logins(), 4);
    }

    /// The manual's "Algorithm Requirements": `curve25519-sha256` and
    /// `aes128-gcm@openssh.com` are enough.
    #[tokio::test]
    async fn a_server_with_only_surges_algorithms_is_reached() {
        let server = fake(FakeSshOpts {
            password: Some("pw".into()),
            surge_minimum: true,
            ..Default::default()
        })
        .await;
        let ssh = outbound_to(server.addr, &spec(Some("pw"), None, vec![]), &[]);
        round_trip(&ssh, &echo().await).await.unwrap();
    }

    #[tokio::test]
    async fn one_session_carries_every_connection() {
        let server = fake(FakeSshOpts {
            password: Some("pw".into()),
            ..Default::default()
        })
        .await;
        let ssh = Arc::new(outbound_to(
            server.addr,
            &spec(Some("pw"), None, vec![]),
            &[],
        ));
        let target = echo().await;
        let mut dials = tokio::task::JoinSet::new();
        for _ in 0..5 {
            let (ssh, target) = (ssh.clone(), target.clone());
            dials.spawn(async move { round_trip(&ssh, &target).await });
        }
        while let Some(dialed) = dials.join_next().await {
            dialed.unwrap().unwrap();
        }
        assert_eq!(server.logins(), 1);
    }

    #[tokio::test]
    async fn a_session_the_server_ended_is_replaced() {
        let server = fake(FakeSshOpts {
            password: Some("pw".into()),
            ..Default::default()
        })
        .await;
        let ssh = outbound_to(server.addr, &spec(Some("pw"), None, vec![]), &[]);
        let target = echo().await;
        round_trip(&ssh, &target).await.unwrap();
        server.end_sessions().await;
        round_trip(&ssh, &target).await.unwrap();
        assert_eq!(server.logins(), 2);
    }

    #[tokio::test]
    async fn a_host_key_outside_server_fingerprint_is_refused() {
        let server = fake(FakeSshOpts {
            password: Some("pw".into()),
            ..Default::default()
        })
        .await;
        let other = random_key(Algorithm::Ed25519);
        let target = echo().await;
        let pinned_elsewhere = spec(Some("pw"), None, vec![pin(other.public_key())]);
        let refused = round_trip(&outbound_to(server.addr, &pinned_elsewhere, &[]), &target)
            .await
            .unwrap_err();
        assert_eq!(
            refused.to_string(),
            "ssh: the server's host key is not one of server-fingerprint"
        );
        let pinned = spec(
            Some("pw"),
            None,
            vec![pin(other.public_key()), pin(&server.host_key)],
        );
        round_trip(&outbound_to(server.addr, &pinned, &[]), &target)
            .await
            .unwrap();
        assert_eq!(server.logins(), 1);
    }

    #[tokio::test]
    async fn a_wrong_password_fails_without_repeating_it() {
        let server = fake(FakeSshOpts {
            password: Some("pw".into()),
            ..Default::default()
        })
        .await;
        let ssh = outbound_to(server.addr, &spec(Some("hunter2"), None, vec![]), &[]);
        let failed = round_trip(&ssh, &echo().await).await.unwrap_err();
        assert_eq!(failed.to_string(), "ssh: authentication failed");
        assert_eq!(server.logins(), 0);
    }

    #[test]
    fn an_rsa_key_never_signs_with_sha1() {
        assert_eq!(rsa_hash(Some(Some(HashAlg::Sha512))), HashAlg::Sha512);
        assert_eq!(rsa_hash(Some(Some(HashAlg::Sha256))), HashAlg::Sha256);
        // a server that lists only `ssh-rsa`, and one that lists nothing
        assert_eq!(rsa_hash(Some(None)), HashAlg::Sha256);
        assert_eq!(rsa_hash(None), HashAlg::Sha256);
    }

    /// The reason code is named; the session stays for the next connection.
    #[tokio::test]
    async fn a_refused_channel_names_the_reason_and_keeps_the_session() {
        let server = fake(FakeSshOpts {
            password: Some("pw".into()),
            refuse_channels: true,
            ..Default::default()
        })
        .await;
        let ssh = outbound_to(server.addr, &spec(Some("pw"), None, vec![]), &[]);
        let target = echo().await;
        for _ in 0..2 {
            let refused = round_trip(&ssh, &target).await.unwrap_err();
            assert_eq!(
                refused.to_string(),
                "ssh: the server refused the channel (connect failed)"
            );
        }
        assert_eq!(server.logins(), 1);
    }

    #[tokio::test]
    async fn a_server_that_does_not_speak_ssh_fails_the_handshake() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                let _ = s.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").await;
            }
        });
        let ssh = outbound_to(addr, &spec(Some("pw"), None, vec![]), &[]);
        let failed = round_trip(&ssh, &echo().await).await.unwrap_err();
        assert_eq!(failed.to_string(), "ssh: the handshake failed");
    }
}
```

要点：
- `preferred()`：russh 的默认加密列表没有 Surge 要求的 `aes128-gcm@openssh.com`，补在 aes256-gcm 之后；主机密钥算法去掉 `Rsa { hash: None }`（`ssh-rsa`，SHA-1 签名）。kex 与 MAC 的默认列表本来就没有 SHA-1（P2）。`a_server_with_only_surges_algorithms_is_reached` 用只开 `curve25519-sha256` + `aes128-gcm` 的 `FakeSsh` 证明这一点。
- 会话槽是 tokio `Mutex<Option<Arc<Session>>>`，锁只包住"取或建"（P13）：同时进来的拨号在锁上等同一次握手；取的时候跳过已关闭的会话（`Handle::is_closed()`）。
- 开通道：`Error::ChannelOpenFailure(reason)` 是服务器拒绝——报原因名，会话保留；其余错误当作会话已断——`forget` 只在槽里仍是这条会话时清空（别的拨号可能已经换上了新会话），然后重建一次再试（P5）。
- 认证先试密钥、再试口令，都不行就是固定的 `ssh: authentication failed`。RSA 私钥的哈希由纯函数 `rsa_hash` 决定：服务器列了 rsa-sha2-512 用它，否则 rsa-sha2-256，绝不用 SHA-1（P3）。
- 握手期间的错误都经 `handshake_error` 变成固定说法，不带服务器原文（P14）；`connect_tcp` 用 `ConnectOpts.timeout` 包住整个拨号，超时是 `OutboundError::Timeout`。
- 用例里的目标都是回环上的回显服务；`FakeSsh` 的 `end_sessions()` 模拟服务器重启。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-proto-ssh` → 14 passed（新增 `outbound::tests` 9 条：口令与 Ed25519 / ECDSA / RSA 三种密钥登录、只开 Surge 最低算法的服务器、一条会话承载全部连接、被服务器断开的会话重建、主机密钥不在 `server-fingerprint` 里、口令错误、通道被拒、对端不是 SSH、RSA 绝不用 SHA-1 签名）。

- [ ] **Step 5: 门禁与提交**

跑门禁（43 个测试二进制，1015 通过 / 1 忽略）。

```bash
git add crates/rurge-proto-ssh
git commit -m "feat(proto-ssh): SshOutbound——每个策略一条会话、单飞握手、direct-tcpip 通道、断线重建、先密钥后口令认证；FakeSsh"
```

### Task 4: 空闲断开与保活；没配指纹的一次性告警

设计 5.4（M4-D10）：`idle-timeout` 的"空闲"指会话上**没有打开的通道**——长时间没有流量、但连接还开着（挂着的 WebSocket）时不会被切断；最后一个通道关掉之后持续 `idle-timeout` 才断开会话，下次拨号重建。会话存在期间每 30 秒一次 SSH keepalive，连续 3 次无回应判定会话已断（P6）。设计 5.2：没配 `server-fingerprint` 时照 Surge 接受任何主机密钥，但告警一次、只带策略名（P12）。

**Files:**
- Modify: `crates/rurge-proto-ssh/src/outbound.rs`（`KEEPALIVE` / `KEEPALIVE_MAX`、`session_config`、`OpenChannel`、`SshStream`、`watch_idle`、`first_unpinned`，与用例）
- Modify: `crates/rurge-proto-ssh/src/testing/server.rs`（在线会话计数 `live_sessions()`）

**Interfaces:**
- Consumes: Task 3 的 `SshOutbound`、`FakeSsh`。
- Produces:
  - `FakeSsh::live_sessions(&self) -> usize`（握手完成、尚未结束的会话数）
  - crate 内部：`session_config() -> client::Config`（`keepalive_interval = Some(30 秒)`、`keepalive_max = 3`）、`SshOutbound::first_unpinned(&self) -> bool`（没配指纹时只在第一次返回 `true`）

- [ ] **Step 1: 先写用例**

`crates/rurge-proto-ssh/src/outbound.rs`——把

```rust
        assert_eq!(server.logins(), 1);
    }

    #[tokio::test]
    async fn a_server_that_does_not_speak_ssh_fails_the_handshake() {
```

换成

```rust
        assert_eq!(server.logins(), 1);
    }

    fn with_idle(mut ssh: SshSpec, secs: u64) -> SshSpec {
        ssh.idle_timeout = Duration::from_secs(secs);
        ssh
    }

    /// Polls `check` until it holds, for at most five seconds.
    async fn eventually(check: impl Fn() -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !check() {
            assert!(tokio::time::Instant::now() < deadline, "timed out");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn a_session_without_channels_closes_after_the_idle_timeout() {
        let server = fake(FakeSshOpts {
            password: Some("pw".into()),
            ..Default::default()
        })
        .await;
        let ssh = outbound_to(
            server.addr,
            &with_idle(spec(Some("pw"), None, vec![]), 1),
            &[],
        );
        let target = echo().await;
        round_trip(&ssh, &target).await.unwrap();
        eventually(|| server.live_sessions() == 0).await;
        round_trip(&ssh, &target).await.unwrap();
        assert_eq!(server.logins(), 2);
    }

    /// Idle means no open channel, not no traffic: a quiet connection keeps
    /// its session.
    #[tokio::test]
    async fn an_open_channel_keeps_the_session_past_the_idle_timeout() {
        let server = fake(FakeSshOpts {
            password: Some("pw".into()),
            ..Default::default()
        })
        .await;
        let ssh = outbound_to(
            server.addr,
            &with_idle(spec(Some("pw"), None, vec![]), 1),
            &[],
        );
        let mut held = ssh
            .connect_tcp(&echo().await, &ConnectOpts::default())
            .await
            .unwrap();
        // the window being observed: twice the idle timeout
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(server.live_sessions(), 1);
        held.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        held.read_exact(&mut buf).await.unwrap();
        drop(held);
        eventually(|| server.live_sessions() == 0).await;
    }

    #[test]
    fn the_session_is_kept_alive_every_thirty_seconds() {
        let config = session_config();
        assert_eq!(
            (config.keepalive_interval, config.keepalive_max),
            (Some(Duration::from_secs(30)), 3)
        );
    }

    #[test]
    fn a_policy_without_server_fingerprint_is_warned_about_once() {
        let addr = "127.0.0.1:9".parse().unwrap();
        let unpinned = outbound_to(addr, &spec(Some("pw"), None, vec![]), &[]);
        assert!(unpinned.first_unpinned());
        assert!(!unpinned.first_unpinned());
        let key = random_key(Algorithm::Ed25519);
        let pinned = outbound_to(
            addr,
            &spec(Some("pw"), None, vec![pin(key.public_key())]),
            &[],
        );
        assert!(!pinned.first_unpinned());
    }

    #[tokio::test]
    async fn a_server_that_does_not_speak_ssh_fails_the_handshake() {
```

`crates/rurge-proto-ssh/src/testing/server.rs`——把

```rust
    logins: AtomicUsize,
```

换成

```rust
    logins: AtomicUsize,
    /// Sessions whose handshake finished and that have not ended.
    live: AtomicUsize,
```

`crates/rurge-proto-ssh/src/testing/server.rs`——把

```rust
            logins: AtomicUsize::new(0),
```

换成

```rust
            logins: AtomicUsize::new(0),
            live: AtomicUsize::new(0),
```

`crates/rurge-proto-ssh/src/testing/server.rs`——把

```rust
                        state.sessions.lock().unwrap().push(running.handle());
                        let _ = running.await;
```

换成

```rust
                        state.live.fetch_add(1, Ordering::SeqCst);
                        state.sessions.lock().unwrap().push(running.handle());
                        let _ = running.await;
                        state.live.fetch_sub(1, Ordering::SeqCst);
```

`crates/rurge-proto-ssh/src/testing/server.rs`——把

```rust
        self.state.logins.load(Ordering::SeqCst)
```

换成

```rust
        self.state.logins.load(Ordering::SeqCst)
    }

    /// Sessions that are up now.
    pub fn live_sessions(&self) -> usize {
        self.state.live.load(Ordering::SeqCst)
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-proto-ssh`
Expected: FAIL，编译错误（`Duration` 要到 Step 3 才在模块顶部导入，所以也报出来）——

```text
error[E0433]: failed to resolve: use of undeclared type `Duration`
   --> crates\rurge-proto-ssh\src\outbound.rs:524:28
error[E0425]: cannot find function `session_config` in this scope
   --> crates\rurge-proto-ssh\src\outbound.rs:586:22
error[E0599]: no method named `first_unpinned` found for struct `outbound::SshOutbound` in the current scope
   --> crates\rurge-proto-ssh\src\outbound.rs:597:26
error: could not compile `rurge-proto-ssh` (lib test) due to 9 previous errors
```

- [ ] **Step 3: 实现**

`crates/rurge-proto-ssh/src/outbound.rs`——把

```rust
use russh::{ChannelOpenFailure, Error, Preferred, cipher};
use rustls::RootCertStore;
use std::borrow::Cow;
use std::sync::Arc;
use tokio::sync::Mutex;
```

换成

```rust
use russh::{ChannelOpenFailure, ChannelStream, Disconnect, Error, Preferred, cipher};
use rustls::RootCertStore;
use std::borrow::Cow;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{Mutex, Notify};

/// A keepalive every 30 seconds while the session is up; three without an
/// answer end it (M4-D10).
const KEEPALIVE: Duration = Duration::from_secs(30);
const KEEPALIVE_MAX: usize = 3;
```

`crates/rurge-proto-ssh/src/outbound.rs`——把

```rust
struct Session {
    handle: Handle<Client>,
```

换成

```rust
fn session_config() -> client::Config {
    client::Config {
        preferred: preferred(),
        keepalive_interval: Some(KEEPALIVE),
        keepalive_max: KEEPALIVE_MAX,
        ..Default::default()
    }
}

struct Session {
    handle: Handle<Client>,
    /// Channels handed out and not yet dropped.
    open: Arc<AtomicUsize>,
    /// Woken when a channel opens or closes.
    activity: Arc<Notify>,
}

/// Counts a channel as open while it lives.
struct OpenChannel {
    open: Arc<AtomicUsize>,
    activity: Arc<Notify>,
}

impl OpenChannel {
    fn new(session: &Session) -> OpenChannel {
        session.open.fetch_add(1, Ordering::SeqCst);
        session.activity.notify_one();
        OpenChannel {
            open: session.open.clone(),
            activity: session.activity.clone(),
        }
    }
}

impl Drop for OpenChannel {
    fn drop(&mut self) {
        self.open.fetch_sub(1, Ordering::SeqCst);
        self.activity.notify_one();
    }
}

/// A channel as the engine sees it.
struct SshStream {
    channel: ChannelStream<client::Msg>,
    _open: OpenChannel,
}

impl AsyncRead for SshStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().channel).poll_read(cx, buf)
    }
}

impl AsyncWrite for SshStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().channel).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().channel).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().channel).poll_shutdown(cx)
    }
}

/// Ends the session once no channel has been open on it for `idle`
/// (`idle-timeout`: idle means no channel, not no traffic, M4-D10). Holds
/// the session weakly: a session replaced or dropped ends the watch.
fn watch_idle(session: Weak<Session>, idle: Duration) {
    tokio::spawn(async move {
        loop {
            let Some((open, activity)) = session
                .upgrade()
                .map(|s| (s.open.clone(), s.activity.clone()))
            else {
                return;
            };
            if open.load(Ordering::SeqCst) > 0 {
                activity.notified().await;
                continue;
            }
            if tokio::time::timeout(idle, activity.notified())
                .await
                .is_ok()
            {
                // a channel opened or closed meanwhile: look again
                continue;
            }
            if open.load(Ordering::SeqCst) == 0 {
                if let Some(session) = session.upgrade() {
                    let _ = session
                        .handle
                        .disconnect(Disconnect::ByApplication, "", "")
                        .await;
                }
                return;
            }
        }
    });
```

`crates/rurge-proto-ssh/src/outbound.rs`——把

```rust
    /// One handshake at a time: dials that come in meanwhile wait for it.
    session: Mutex<Option<Arc<Session>>>,
```

换成

```rust
    idle_timeout: Duration,
    /// One handshake at a time: dials that come in meanwhile wait for it.
    session: Mutex<Option<Arc<Session>>>,
    unpinned_warned: AtomicBool,
```

`crates/rurge-proto-ssh/src/outbound.rs`——把

```rust
            session: Mutex::new(None),
```

换成

```rust
            idle_timeout: ssh.idle_timeout,
            session: Mutex::new(None),
            unpinned_warned: AtomicBool::new(false),
```

`crates/rurge-proto-ssh/src/outbound.rs`——把

```rust
        let session = Arc::new(self.establish(opts).await?);
```

换成

```rust
        let session = Arc::new(self.establish(opts).await?);
        watch_idle(Arc::downgrade(&session), self.idle_timeout);
```

`crates/rurge-proto-ssh/src/outbound.rs`——把

```rust
        let stream = self.stack.open(opts).await?;
        let config = Arc::new(client::Config {
            preferred: preferred(),
            ..Default::default()
        });
```

换成

```rust
        let stream = self.stack.open(opts).await?;
```

`crates/rurge-proto-ssh/src/outbound.rs`——把

```rust
        let mut handle = client::connect_stream(config, stream, client)
```

换成

```rust
        let mut handle = client::connect_stream(Arc::new(session_config()), stream, client)
```

`crates/rurge-proto-ssh/src/outbound.rs`——把

```rust
        Ok(Session { handle })
```

换成

```rust
        if self.first_unpinned() {
            // the policy name only
            tracing::warn!(
                policy = %self.name,
                "ssh: no server-fingerprint; the server's host key is not verified"
            );
        }
        Ok(Session {
            handle,
            open: Arc::new(AtomicUsize::new(0)),
            activity: Arc::new(Notify::new()),
        })
    }

    /// True once, at the first session of a policy without
    /// `server-fingerprint` (manual: a one-time security warning).
    fn first_unpinned(&self) -> bool {
        self.pins.is_empty() && !self.unpinned_warned.swap(true, Ordering::Relaxed)
```

`crates/rurge-proto-ssh/src/outbound.rs`——把

```rust
        Ok(channel) => Ok(Box::new(channel.into_stream())),
```

换成

```rust
        Ok(channel) => Ok(Box::new(SshStream {
            channel: channel.into_stream(),
            _open: OpenChannel::new(session),
        })),
```

要点：
- 保活交给 russh：`client::Config` 的 `keepalive_interval` / `keepalive_max`；`inactivity_timeout` 按流量计、与"没有通道"不同，保持默认的 `None`（P6）。
- 每个交出去的流带一个 `OpenChannel` 守卫：创建时计数加一，随流丢弃时减一，两次都唤醒 `activity`。`SshStream` 只是把读写转给 russh 的 `ChannelStream`，外加这个守卫。
- 每建一条会话起一个 `watch_idle` 任务：计数不为 0 就等下一次唤醒；为 0 时等 `idle-timeout`，期间有通道开关就重新看，否则 `disconnect`。它只持有会话的 `Weak`：会话被替换或丢弃时任务随之结束。断开之后 `Handle::is_closed()` 为真，下一次拨号重建（Task 3 的路径）。
- 告警在第一次建好会话（认证通过）之后记，`AtomicBool` 保证每个出站对象只记一次；日志只有策略名这一个字段（P12）。
- 空闲用例用真实时钟：`idle-timeout` 取 1 秒，轮询等待最多 5 秒；"开着的通道让会话活过空闲时限"要观察 2 秒内什么也没发生，所以那里等一段固定时间。回环上的真实连接与暂停的时钟不能共存（P16）。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-proto-ssh` → 18 passed（新增 `a_session_without_channels_closes_after_the_idle_timeout`、`an_open_channel_keeps_the_session_past_the_idle_timeout`、`the_session_is_kept_alive_every_thirty_seconds`、`a_policy_without_server_fingerprint_is_warned_about_once`；两条空闲用例各要一到三秒）。

- [ ] **Step 5: 门禁与提交**

跑门禁（43 个测试二进制，1019 通过 / 1 忽略）。

```bash
git add crates/rurge-proto-ssh/src/outbound.rs crates/rurge-proto-ssh/src/testing/server.rs
git commit -m "feat(proto-ssh): 按通道计的空闲断开、30 秒 × 3 的保活、没配 server-fingerprint 时告警一次"
```

### Task 5: 接入——`ProtoSpec::Ssh`、重载指纹、引擎工厂；端到端、经 Shadow TLS 与测速

把 Task 1 ～ 4 接起来（设计 8.2）：`to_spec` 的 `ssh` 分支产出 `ProtoSpec::Ssh`；重载按指纹复用出站时认得 SSH 私钥（P10）；引擎的工厂构建 `SshOutbound`。经引擎的端到端用例：目标主机名交给 SSH 服务器解析（rurge 自己不查 DNS）、Keystore 私钥加 `server-fingerprint`、经 Shadow TLS v3、经 SSH 的连通性测试。能力表仍未翻转（Task 7），所以这些用例之外的 `ssh` 行照旧 `W0007`；但 `rurge check` 的干构建从本任务起就会构建 `ssh` 策略，报出用不了的私钥。

**Files:**
- Modify: `crates/rurge-config/src/spec/mod.rs`（`ProtoSpec::Ssh`、`keystore_item`、`to_spec` 的 `ssh` 分支，与用例）
- Modify: `crates/rurge-policy/src/registry.rs`（指纹改用 `keystore_item`，与用例）
- Modify: `crates/rurge-proto-ssh/src/testing/mod.rs`（`keystore_base64`、`fingerprint_of` 与导出）、`crates/rurge-proto-ssh/src/testing/server.rs`（`connect_to`、`requested()`）
- Modify: `crates/rurge-engine/Cargo.toml`（依赖与 dev 依赖）、`crates/rurge-engine/src/outbounds.rs`（工厂分支，与用例）、`crates/rurge-engine/tests/common/mod.rs`（`Profile.keystore`、导出 SSH 的测试辅助）
- Create: `crates/rurge-engine/tests/outbounds_ssh.rs`
- `Cargo.lock` 由 cargo 自己更新（`rurge-engine` 多一条依赖）

**Interfaces:**
- Consumes: Task 1 的 `SshSpec` / `read_ssh`；Task 3、4 的 `SshOutbound::new`、`FakeSsh`；`rurge_proto::build::server_of`；`rurge_proto::testing::{Camouflage, FakeShadowTls, ShadowTlsScript, TlsFixture}`。
- Produces:
  - `ProtoSpec::Ssh(SshSpec)`；`ProtoSpec::tls()` 对它返回 `None`
  - `ProtoSpec::keystore_item(&self) -> Option<&str>`：`ssh` 取 `private-key`，其余取 TLS 的 `client-cert`
  - `FakeSshOpts::connect_to: Option<SocketAddr>`；`FakeSsh::requested(&self) -> Vec<(String, u32)>`
  - `testing::keystore_base64(key: &PrivateKey) -> String`、`testing::fingerprint_of(key: &PublicKey) -> String`（`算法 base64`），并导出 `Algorithm`、`PrivateKey`、`PublicKey`
  - 引擎用例的 `Profile { keystore: &str, .. }`（`[Keystore]` 节的内容，默认空）

- [ ] **Step 1: 先写配置层与注册表的用例**

`crates/rurge-config/src/spec/mod.rs`——把

```rust
        assert!(o.spec.is_none() && o.diagnostics.is_empty() && o.inert.is_empty());
```

换成

```rust
        assert!(o.spec.is_none() && o.diagnostics.is_empty() && o.inert.is_empty());
    }

    #[test]
    fn an_ssh_line_gets_a_spec() {
        let o = outcome(
            "S",
            "ssh, h.test, 22, username=u, password=pw, idle-timeout=60",
        );
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        let spec = o.spec.expect("an ssh spec");
        assert_eq!(
            (spec.server, spec.port),
            (Some(HostName::Domain("h.test".into())), Some(22))
        );
        let ProtoSpec::Ssh(ssh) = &spec.proto else {
            panic!("{:?}", spec.proto);
        };
        assert_eq!(ssh.idle_timeout, std::time::Duration::from_secs(60));
    }

    /// What a reload compares besides the line (M2 design 7.1): the keystore
    /// item the policy uses, whatever the protocol.
    #[test]
    fn the_keystore_item_is_a_client_certificate_or_an_ssh_key() {
        let https = outcome("H", "https, h.test, 443, client-cert=cert1");
        assert_eq!(https.spec.unwrap().proto.keystore_item(), Some("cert1"));
        let plain = outcome("P", "http, h.test, 80");
        assert_eq!(plain.spec.unwrap().proto.keystore_item(), None);
        let ssh = ProtoSpec::Ssh(SshSpec {
            username: "u".into(),
            password: None,
            private_key: Some("key1".into()),
            idle_timeout: ssh::DEFAULT_IDLE_TIMEOUT,
            host_keys: Vec::new(),
        });
        assert_eq!(ssh.keystore_item(), Some("key1"));
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            );
        }
    }

    const SUBSCRIBED: &str = "[Proxy]\nRelay = http, r.example, 80\nA = http, a.example, 80\n\
```

换成

```rust
            );
        }
    }

    /// A new key under the same keystore name rebuilds the `ssh` policy:
    /// the old session would go on logging in with the old key.
    #[test]
    fn a_changed_ssh_private_key_rebuilds_the_policy() {
        let text = "[Proxy]\nS = ssh, s.example, 22, username=u, private-key=key1\n\
[Keystore]\nkey1 = type=openssh-private-key, base64=QUJD\n[Rule]\nFINAL,DIRECT\n";
        let factory = FakeFactory::new();
        let first = generation(text, &factory, None);
        let same = generation(text, &factory, Some(&first));
        assert!(Arc::ptr_eq(
            &outbound_of(&first, "S"),
            &outbound_of(&same, "S")
        ));
        let changed = generation(
            &text.replace("base64=QUJD", "base64=QUJE"),
            &factory,
            Some(&first),
        );
        assert!(!Arc::ptr_eq(
            &outbound_of(&first, "S"),
            &outbound_of(&changed, "S")
        ));
    }

    const SUBSCRIBED: &str = "[Proxy]\nRelay = http, r.example, 80\nA = http, a.example, 80\n\
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-config spec::tests`
Expected: FAIL，编译错误——

```text
error[E0599]: no variant or associated item named `Ssh` found for enum `spec::ProtoSpec` in the current scope
   --> crates\rurge-config\src\spec\mod.rs:422:24
error[E0599]: no method named `keystore_item` found for enum `spec::ProtoSpec` in the current scope
   --> crates\rurge-config\src\spec\mod.rs:433:46
error: could not compile `rurge-config` (lib test) due to 4 previous errors
```

- [ ] **Step 3: 实现配置层**

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    AnyTls(AnyTlsSpec),
```

换成

```rust
    AnyTls(AnyTlsSpec),
    Ssh(SshSpec),
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            ProtoSpec::Direct | ProtoSpec::Reject(_) => None,
```

换成

```rust
            ProtoSpec::Direct | ProtoSpec::Reject(_) | ProtoSpec::Ssh(_) => None,
        }
    }

    /// The `[Keystore]` item the protocol uses: a TLS client certificate, or
    /// an `ssh` private key.
    pub fn keystore_item(&self) -> Option<&str> {
        match self {
            ProtoSpec::Ssh(ssh) => ssh.private_key.as_deref(),
            _ => self.tls().and_then(|tls| tls.client_cert.as_deref()),
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            (common, ProtoSpec::AnyTls(anytls))
        }
        _ => return SpecOutcome::default(),
```

换成

```rust
            (common, ProtoSpec::AnyTls(anytls))
        }
        PolicyKind::Ssh => {
            let common = read_common(&mut r, Applies::Proxy, &mut notes);
            let ssh = ssh::read_ssh(&mut r, env.keystore);
            (common, ProtoSpec::Ssh(ssh))
        }
        _ => return SpecOutcome::default(),
```

Run: `cargo test -p rurge-config spec::tests` → 12 passed（新增 `an_ssh_line_gets_a_spec`、`the_keystore_item_is_a_client_certificate_or_an_ssh_key`）。

`ProtoSpec` 多了一个变体，`rurge-engine` 的工厂（`outbounds.rs` 里对 `ProtoSpec` 的 `match`）要到 Step 7 才补上分支：在那之前 `rurge-engine` 编译不过，属预期。

注册表的用例此时仍失败——指纹还只认 TLS 的 `client-cert`：

Run: `cargo test -p rurge-policy a_changed_ssh_private_key_rebuilds_the_policy`
Expected: FAIL——

```text
test registry::tests::a_changed_ssh_private_key_rebuilds_the_policy ... FAILED
thread 'registry::tests::a_changed_ssh_private_key_rebuilds_the_policy' panicked at crates\rurge-policy\src\registry.rs:1643:9:
assertion failed: !Arc::ptr_eq(&outbound_of(&first, "S"), &outbound_of(&changed, "S"))
```

- [ ] **Step 4: 实现注册表的指纹**

`crates/rurge-policy/src/registry.rs`——把

```rust
    /// `client-cert`'s keystore item, by content.
```

换成

```rust
    /// The keystore item the policy uses (`client-cert`, an `ssh`
    /// `private-key`), by content.
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        .tls()
        .and_then(|tls| tls.client_cert.as_ref())
        .and_then(|name| cfg.keystore.iter().find(|item| &item.name == name))
```

换成

```rust
        .keystore_item()
        .and_then(|name| cfg.keystore.iter().find(|item| item.name == name))
```

Run: `cargo test -p rurge-policy registry::tests` → 42 passed（新增 `a_changed_ssh_private_key_rebuilds_the_policy`）。

- [ ] **Step 5: 先写引擎的用例**

`crates/rurge-proto-ssh/src/testing/mod.rs`——把

```rust
use russh::keys::PrivateKey;
use russh::keys::ssh_key::Algorithm;
```

换成

```rust
use russh::keys::ssh_key::LineEnding;
```

`crates/rurge-proto-ssh/src/testing/mod.rs`——把

```rust
mod server;

pub use server::{FakeSsh, FakeSshOpts};
```

换成

```rust
mod server;

pub use russh::keys::ssh_key::Algorithm;
pub use russh::keys::{PrivateKey, PublicKey};
pub use server::{FakeSsh, FakeSshOpts};
```

`crates/rurge-proto-ssh/src/testing/mod.rs`——把

```rust
        span: Span::new(Arc::from(Path::new("t.conf")), 1),
    }
}

/// A fresh Ed25519 or ECDSA key (for RSA, `RSA_KEY`).
```

换成

```rust
        span: Span::new(Arc::from(Path::new("t.conf")), 1),
    }
}

/// `key` the way a `[Keystore]` item's `base64=` holds it.
pub fn keystore_base64(key: &PrivateKey) -> String {
    STANDARD.encode(key.to_openssh(LineEnding::LF).expect("an OpenSSH key file"))
}

/// `key` the way `server-fingerprint` takes it: `<algorithm> <base64>`.
pub fn fingerprint_of(key: &PublicKey) -> String {
    format!(
        "{} {}",
        key.algorithm(),
        STANDARD.encode(key.to_bytes().expect("an encoded key"))
    )
}

/// A fresh Ed25519 or ECDSA key (for RSA, `RSA_KEY`).
```

`crates/rurge-proto-ssh/src/testing/server.rs`——把

```rust
    pub surge_minimum: bool,
```

换成

```rust
    pub surge_minimum: bool,
    /// Connect every channel here instead of where it asks to go.
    pub connect_to: Option<SocketAddr>,
```

`crates/rurge-proto-ssh/src/testing/server.rs`——把

```rust
    sessions: Mutex<Vec<server::Handle>>,
```

换成

```rust
    sessions: Mutex<Vec<server::Handle>>,
    /// Where the channels asked to go, in order.
    requested: Mutex<Vec<(String, u32)>>,
```

`crates/rurge-proto-ssh/src/testing/server.rs`——把

```rust
            sessions: Mutex::new(Vec::new()),
```

换成

```rust
            sessions: Mutex::new(Vec::new()),
            requested: Mutex::new(Vec::new()),
```

`crates/rurge-proto-ssh/src/testing/server.rs`——把

```rust
        self.state.logins.load(Ordering::SeqCst)
```

换成

```rust
        self.state.logins.load(Ordering::SeqCst)
    }

    /// Where the channels asked to go: host and port, as sent.
    pub fn requested(&self) -> Vec<(String, u32)> {
        self.state.requested.lock().unwrap().clone()
```

`crates/rurge-proto-ssh/src/testing/server.rs`——把

```rust
    ) -> Result<(), Self::Error> {
```

换成

```rust
    ) -> Result<(), Self::Error> {
        self.state
            .requested
            .lock()
            .unwrap()
            .push((host_to_connect.to_string(), port_to_connect));
```

`crates/rurge-proto-ssh/src/testing/server.rs`——把

```rust
        let tcp = match target {
            Some(port) => TcpStream::connect((host_to_connect, port)).await.ok(),
            None => None,
```

换成

```rust
        let tcp = match (target, self.state.opts.connect_to) {
            (None, _) => None,
            (Some(_), Some(addr)) => TcpStream::connect(addr).await.ok(),
            (Some(port), None) => TcpStream::connect((host_to_connect, port)).await.ok(),
```

`crates/rurge-engine/Cargo.toml`——把

```toml
rurge-proto.workspace = true
```

换成

```toml
rurge-proto.workspace = true
rurge-proto-ssh.workspace = true
```

`crates/rurge-engine/Cargo.toml`——把

```toml
rurge-proto = { workspace = true, features = ["testing"] }
```

换成

```toml
rurge-proto = { workspace = true, features = ["testing"] }
rurge-proto-ssh = { workspace = true, features = ["testing"] }
```

`crates/rurge-engine/tests/common/mod.rs`——把

```rust
    Socks5Script, TlsFixture, TrojanScript, VmessScript,
```

换成

```rust
    Socks5Script, TlsFixture, TrojanScript, VmessScript,
};
pub use rurge_proto_ssh::testing::{
    Algorithm, FakeSsh, FakeSshOpts, fingerprint_of, keystore_base64, random_key,
```

`crates/rurge-engine/tests/common/mod.rs`——把

```rust
    pub rules: &'a str,
```

换成

```rust
    pub rules: &'a str,
    pub keystore: &'a str,
```

`crates/rurge-engine/tests/common/mod.rs`——把

```rust
[Proxy]\n{}\n[Proxy Group]\n{}\n[Host]\n{}\n[Rule]\n{}\nFINAL,DIRECT\n",
            self.general, self.proxies, self.groups, self.hosts, self.rules
```

换成

```rust
[Proxy]\n{}\n[Proxy Group]\n{}\n[Host]\n{}\n[Keystore]\n{}\n[Rule]\n{}\nFINAL,DIRECT\n",
            self.general, self.proxies, self.groups, self.hosts, self.keystore, self.rules
```

新建 `crates/rurge-engine/tests/outbounds_ssh.rs`：

```rust
//! Sessions that leave through the `ssh` outbound: profile text → Runtime →
//! Engine → loopback listeners → `FakeSsh` → `TestServer` (phase 2 M4
//! design §5).

mod common;

use common::*;
use rurge_config::spec::ShadowTlsVersion;
use rurge_proto::testing::{Camouflage, FakeShadowTls, ShadowTlsScript};

async fn ssh_server(origin: &TestServer, opts: FakeSshOpts) -> FakeSsh {
    FakeSsh::start(FakeSshOpts {
        user: "u".into(),
        connect_to: Some(origin_addr(origin)),
        ..opts
    })
    .await
}

#[tokio::test]
async fn a_connect_leaves_through_ssh_with_the_name_unresolved() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let server = ssh_server(
        &origin,
        FakeSshOpts {
            password: Some("pw".into()),
            ..FakeSshOpts::default()
        },
    )
    .await;
    let h = harness(Profile {
        proxies: &format!(
            "S = ssh, 127.0.0.1, {}, username=u, password=pw",
            server.addr.port()
        ),
        rules: "DOMAIN,target.test,S",
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
    let response = get(&mut tunnel, "target.test", "/hello").await;
    assert!(response.ends_with("hi there"), "{response}");
    // the SSH server resolves the name: rurge never looked it up
    assert_eq!(server.requested(), [("target.test".to_string(), 8080)]);
    assert!(h.dns.queries().is_empty());
    drop(tunnel);
    let log = h.engine.request_log();
    wait_until("the session to finish", || !log.recent(10).is_empty()).await;
    assert_eq!(log.recent(10)[0].policy, ["S"]);
}

/// A key from `[Keystore]`, and a server pinned by `server-fingerprint`.
#[tokio::test]
async fn a_keystore_key_logs_in_to_a_pinned_server() {
    let origin = TestServer::spawn().await;
    origin.set("/k", "keyed");
    let key = random_key(Algorithm::Ed25519);
    let server = ssh_server(
        &origin,
        FakeSshOpts {
            keys: vec![key.public_key().clone()],
            ..FakeSshOpts::default()
        },
    )
    .await;
    let h = harness(Profile {
        proxies: &format!(
            "S = ssh, 127.0.0.1, {}, username=u, private-key=key1, server-fingerprint=\"{}\"",
            server.addr.port(),
            fingerprint_of(&server.host_key)
        ),
        keystore: &format!(
            "key1 = type=openssh-private-key, base64={}",
            keystore_base64(&key)
        ),
        rules: "DOMAIN,target.test,S",
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:80").await;
    let response = get(&mut tunnel, "target.test", "/k").await;
    assert!(response.ends_with("keyed"), "{response}");
    assert_eq!(server.logins(), 1);
}

/// The SSH session runs inside Shadow TLS like any TCP protocol's.
#[tokio::test]
async fn an_ssh_session_runs_inside_shadow_tls() {
    let origin = TestServer::spawn().await;
    origin.set("/st", "wrapped");
    let server = ssh_server(
        &origin,
        FakeSshOpts {
            password: Some("pw".into()),
            ..FakeSshOpts::default()
        },
    )
    .await;
    let fixture = TlsFixture::new(&["site.test"]);
    let site = Camouflage::spawn(&fixture, &[&rustls::version::TLS13], 2).await;
    let front = FakeShadowTls::spawn(ShadowTlsScript::new(
        ShadowTlsVersion::V3,
        "st-pw",
        site.addr(),
        server.addr,
    ))
    .await;
    let h = harness_trusting(
        Profile {
            proxies: &format!(
                "S = ssh, 127.0.0.1, {}, username=u, password=pw, shadow-tls-password=st-pw, shadow-tls-version=3, shadow-tls-sni=site.test",
                front.addr().port()
            ),
            rules: "DOMAIN,target.test,S",
            ..Profile::default()
        },
        fixture.roots(),
    )
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:80").await;
    let response = get(&mut tunnel, "target.test", "/st").await;
    assert!(response.ends_with("wrapped"), "{response}");
    assert!(front.sessions()[0].authenticated);
    assert_eq!(server.logins(), 1);
}

/// A connectivity test goes through the SSH session like any connection.
#[tokio::test]
async fn a_policy_test_goes_through_the_ssh_session() {
    let origin = TestServer::spawn().await;
    let server = ssh_server(
        &origin,
        FakeSshOpts {
            password: Some("pw".into()),
            ..FakeSshOpts::default()
        },
    )
    .await;
    let h = harness(Profile {
        proxies: &format!(
            "S = ssh, 127.0.0.1, {}, username=u, password=pw",
            server.addr.port()
        ),
        ..Profile::default()
    })
    .await;
    let results = h
        .engine
        .test_policies(&["S".to_string()], Some(origin.url("/")))
        .await
        .unwrap();
    let (name, result) = &results[0];
    assert_eq!(name, "S");
    let result = result.as_ref().expect("an ssh policy can be tested");
    assert!(result.outcome.is_ok(), "{:?}", result.outcome);
    assert_eq!(server.requested().len(), 1, "one connection, two HEADs");
}
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
    use rurge_proto::testing::{FakeSocks5, Socks5Script, echo_server};
```

换成

```rust
    use rurge_proto::testing::{FakeSocks5, Socks5Script, echo_server};
    use rurge_proto_ssh::testing::ED25519_WITH_PASSPHRASE;
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
A = anytls, proxy.test, 443, password=pw\n\
```

换成

```rust
A = anytls, proxy.test, 443, password=pw\n\
SSH = ssh, proxy.test, 22, username=u, password=pw\n\
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
            ("A", "A"),
```

换成

```rust
            ("A", "A"),
            ("SSH", "SSH"),
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
                "{m}"
            );
        }
    }

    #[test]
```

换成

```rust
                "{m}"
            );
        }
    }

    /// `rurge check` names the item and what is wrong with it, never the
    /// key (phase 2 M4 design 4.5).
    #[test]
    fn an_ssh_key_rurge_cannot_use_is_a_load_error() {
        let cfg = config(&format!(
            "[Proxy]\nS = ssh, proxy.test, 22, username=u, private-key=key1\n\
[Keystore]\nkey1 = type=openssh-private-key, base64={}\n[Rule]\nFINAL,DIRECT\n",
            rurge_proto_ssh::testing::keystore_item("key1", ED25519_WITH_PASSPHRASE).base64
        ));
        let diags = dry_build(&cfg);
        let messages: Vec<&str> = diags.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "policy `S` cannot be built: keystore item `key1` is protected by a passphrase, which rurge cannot use; remove the passphrase"
            ]
        );
    }

    #[test]
```

- [ ] **Step 6: 运行，确认失败**

Run: `cargo test -p rurge-engine --test outbounds_ssh`
Expected: FAIL，编译错误——

```text
error[E0004]: non-exhaustive patterns: `&ProtoSpec::Ssh(_)` not covered
   --> crates\rurge-engine\src\outbounds.rs:134:43
error: could not compile `rurge-engine` (lib) due to 1 previous error
```

- [ ] **Step 7: 实现工厂分支**

`crates/rurge-engine/src/outbounds.rs`——把

```rust
use rurge_proto::{Direct, OutboundRef};
```

换成

```rust
use rurge_proto::{Direct, OutboundRef};
use rurge_proto_ssh::SshOutbound;
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
            )?),
        };
```

换成

```rust
            )?),
            ProtoSpec::Ssh(ssh) => Arc::new(SshOutbound::new(
                &spec.name,
                server_of(spec)?,
                ssh,
                spec.shadow_tls.as_ref(),
                &self.keystore,
                self.roots.clone(),
                connector,
            )?),
        };
```

要点：
- `to_spec` 的 `ssh` 分支与其它代理协议一样先 `read_common(.., Applies::Proxy, ..)`（`interface`、`underlying-proxy`、`test-url` 等照常，Shadow TLS 由 `to_spec` 末尾统一读），再 `read_ssh`。
- 工厂分支与 `trojan` / `vmess` / `anytls` 同一形状：`server_of(spec)` 给出服务器，`spec.shadow_tls` 与根证书库交给出站（P18）。干构建（`dry_build`）因此会解码私钥：带口令的私钥在 `rurge check` 时就是带行号的 `E0022`（`an_ssh_key_rurge_cannot_use_is_a_load_error`）。
- 端到端用例里 `FakeSsh` 用 `connect_to` 把通道接到回环的 `TestServer`，并记下通道请求的目标：`target.test:8080` 原样到了 SSH 服务器，rurge 的 DNS 一次也没查。
- 引擎用例的 `Profile::text` 多了一个 `[Keystore]` 节（默认空）；`proxy-test-url` / `internet-test-url` 仍指向回环，不要动。

- [ ] **Step 8: 运行，确认通过**

Run: `cargo test -p rurge-engine --test outbounds_ssh` → 4 passed（`a_connect_leaves_through_ssh_with_the_name_unresolved`、`a_keystore_key_logs_in_to_a_pinned_server`、`an_ssh_session_runs_inside_shadow_tls`、`a_policy_test_goes_through_the_ssh_session`）。
Run: `cargo test -p rurge-engine --lib outbounds::tests` → 13 passed（`every_implemented_protocol_builds` 多了一行 `ssh`；新增 `an_ssh_key_rurge_cannot_use_is_a_load_error`）。

- [ ] **Step 9: 门禁与提交**

跑门禁（44 个测试二进制，1027 通过 / 1 忽略）。

```bash
git add Cargo.lock crates/rurge-config/src/spec/mod.rs crates/rurge-policy/src/registry.rs crates/rurge-proto-ssh/src/testing crates/rurge-engine/Cargo.toml crates/rurge-engine/src/outbounds.rs crates/rurge-engine/tests/common/mod.rs crates/rurge-engine/tests/outbounds_ssh.rs
git commit -m "feat(engine): 接入 ssh 出站——ProtoSpec::Ssh、重载指纹认 SSH 私钥、工厂分支；端到端、经 Shadow TLS 与测速的用例"
```

### Task 6: 互操作——OpenSSH `sshd`

设计 §10 第 3 层、V12（P15）：`ssh` 出站对真实的 OpenSSH `sshd`——OpenSSH 的默认算法，以及只开 Surge 手册要求的 `curve25519-sha256` 与 `aes128-gcm@openssh.com` 两种。`sshd` 以普通用户运行、只听回环临时端口，配置、主机密钥与 `authorized_keys` 都在临时目录里；非 root 的 `sshd` 只让运行它的用户登录，所以用例用现场生成的 Ed25519 密钥登录（用户名取 `USER`），只在 Unix 上编译。

**本机（Windows）没有 `sshd`，也不要装**：这两条用例在本机不编译，在没有 `sshd` 的 Unix 上只打印 `skipping …` 后返回，由 CI 证明（CI 在 Linux / macOS 上设 `RURGE_TEST_SSHD=/usr/sbin/sshd`，Linux 上缺它时先装 `openssh-server`）。夹具自身的单元用例（渲染出的配置只听回环、只认一把公钥）照常在本机运行。

**Files:**
- Modify: `crates/rurge-proto-ssh/src/testing/mod.rs`（`openssh_text`）
- Modify: `tests/interop/Cargo.toml`（`rurge-proto-ssh` 的 dev 依赖；`Cargo.lock` 随之多一行）、`tests/interop/src/lib.rs`（`pub mod sshd;`）
- Create: `tests/interop/src/sshd.rs`（查找 `sshd`、渲染配置、起停子进程，与单元用例）
- Create: `tests/interop/tests/sshd.rs`
- Modify: `.github/workflows/ci.yml`、`tests/interop/README.md`

**Interfaces:**
- Consumes: Task 5 的 `testing::{fingerprint_of, keystore_base64, random_key, Algorithm}`；夹具库里已有的 `Reference`（起子进程、等端口就绪、随 drop 回收）、`free_port`、`REQUIRED_ENV`；`tests/interop/tests/common` 的 `outbound`、`roundtrip`、`echo_server`。
- Produces:
  - `testing::openssh_text(key: &PrivateKey) -> String`（OpenSSH 私钥文件的文本）
  - `rurge_interop::sshd::{BINARY_ENV, locate() -> Option<PathBuf>, sshd_or_skip(test: &str) -> Option<PathBuf>, Algorithms { Default, SurgeMinimum }, render(dir: &Path, port: u16, algorithms: Algorithms) -> String, Sshd}`；`Sshd::spawn(binary, dir, host_key: &str, authorized_key: &str, algorithms) -> Sshd`、`.port()`、`.log_text()`（随 drop 杀掉并回收子进程）

- [ ] **Step 1: 夹具**

`crates/rurge-proto-ssh/src/testing/mod.rs`——把

```rust
/// `key` the way a `[Keystore]` item's `base64=` holds it.
pub fn keystore_base64(key: &PrivateKey) -> String {
    STANDARD.encode(key.to_openssh(LineEnding::LF).expect("an OpenSSH key file"))
```

换成

```rust
/// `key` as an OpenSSH private key file.
pub fn openssh_text(key: &PrivateKey) -> String {
    key.to_openssh(LineEnding::LF)
        .expect("an OpenSSH key file")
        .to_string()
}

/// `key` the way a `[Keystore]` item's `base64=` holds it.
pub fn keystore_base64(key: &PrivateKey) -> String {
    STANDARD.encode(openssh_text(key))
```

`tests/interop/Cargo.toml`——把

```toml
rurge-proto = { workspace = true, features = ["testing"] }
```

换成

```toml
rurge-proto = { workspace = true, features = ["testing"] }
rurge-proto-ssh = { workspace = true, features = ["testing"] }
```

`tests/interop/src/lib.rs`——把

```rust
//! that touches the machine (`set_system_proxy`, `tun`, `auto_route`).

pub mod xray;
```

换成

```rust
//! that touches the machine (`set_system_proxy`, `tun`, `auto_route`).

pub mod sshd;
pub mod xray;
```

新建 `tests/interop/src/sshd.rs`：

```rust
//! An OpenSSH `sshd` child process on the loopback, for the `ssh`
//! interoperability tests (phase 2 M4 design §10). Nothing is installed or
//! configured outside a temporary directory: the rendered `sshd_config`
//! listens on 127.0.0.1 only, lets in nothing but the one key the test made,
//! and allows local port forwarding only. A `sshd` that is not run as root
//! logs in only the user running it, so the tests log in with a key.

use crate::{REQUIRED_ENV, Reference, free_port};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const BINARY_ENV: &str = "RURGE_TEST_SSHD";

/// `RURGE_TEST_SSHD`, else `/usr/sbin/sshd`, else the first `sshd` on `PATH`.
pub fn locate() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(BINARY_ENV).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let usual = Path::new("/usr/sbin/sshd");
    if usual.is_file() {
        return Some(usual.to_path_buf());
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join("sshd"))
        .find(|candidate| candidate.is_file())
}

/// The binary — or `None` after saying why `test` is skipped. With
/// `RURGE_INTEROP_REQUIRED=1` (CI) a missing binary is a failure instead.
pub fn sshd_or_skip(test: &str) -> Option<PathBuf> {
    if let Some(path) = locate() {
        return Some(path);
    }
    if std::env::var(REQUIRED_ENV).as_deref() == Ok("1") {
        panic!("{REQUIRED_ENV}=1 but no sshd was found ({BINARY_ENV}, /usr/sbin/sshd or PATH)");
    }
    eprintln!(
        "skipping {test}: no sshd ({BINARY_ENV}, /usr/sbin/sshd or PATH); see tests/interop/README.md"
    );
    None
}

/// What the server offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Algorithms {
    /// OpenSSH's defaults.
    Default,
    /// Only what Surge's manual requires: `curve25519-sha256` and
    /// `aes128-gcm@openssh.com`.
    SurgeMinimum,
}

/// The `sshd_config` of a server on `port`, its files in `dir`.
pub fn render(dir: &Path, port: u16, algorithms: Algorithms) -> String {
    let mut text = format!(
        "Port {port}\nListenAddress 127.0.0.1\nHostKey {host}\nAuthorizedKeysFile {keys}\n\
PidFile none\nUsePAM no\nStrictModes no\nPasswordAuthentication no\n\
KbdInteractiveAuthentication no\nPubkeyAuthentication yes\nAllowTcpForwarding local\n\
AllowAgentForwarding no\nX11Forwarding no\nPermitTTY no\nPermitTunnel no\nLogLevel ERROR\n",
        host = dir.join("host_key").display(),
        keys = dir.join("authorized_keys").display(),
    );
    if algorithms == Algorithms::SurgeMinimum {
        text.push_str("KexAlgorithms curve25519-sha256\nCiphers aes128-gcm@openssh.com\n");
    }
    text
}

/// A running `sshd`; killed and reaped on drop.
pub struct Sshd(Reference);

impl Sshd {
    /// Writes the host key (owner-only, or `sshd` refuses it), the one
    /// authorized key and the configuration into `dir`, starts `binary` in
    /// the foreground and waits until it accepts connections.
    pub fn spawn(
        binary: &Path,
        dir: &Path,
        host_key: &str,
        authorized_key: &str,
        algorithms: Algorithms,
    ) -> Sshd {
        let port = free_port();
        let host = dir.join("host_key");
        std::fs::write(&host, host_key).expect("write the host key");
        owner_only(&host);
        std::fs::write(dir.join("authorized_keys"), format!("{authorized_key}\n"))
            .expect("write the authorized key");
        let config = dir.join("sshd_config");
        std::fs::write(&config, render(dir, port, algorithms)).expect("write the config");
        let mut command = Command::new(binary);
        // in the foreground, logging to stderr: the log file
        command.arg("-D").arg("-e").arg("-f").arg(&config);
        Sshd(Reference::start(
            "sshd",
            command,
            vec![port],
            dir.join("sshd.log"),
        ))
    }

    pub fn port(&self) -> u16 {
        self.0.port(0)
    }

    pub fn log_text(&self) -> String {
        self.0.log_text()
    }
}

#[cfg(unix)]
fn owner_only(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .expect("make the host key owner-only");
}

#[cfg(not(unix))]
fn owner_only(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// The configuration reaches no further than the loopback and the one
    /// key: no password, no PAM, no terminal, no remote forwarding.
    #[test]
    fn the_configuration_stays_on_the_loopback_with_one_key() {
        let dir = Path::new("/tmp/rurge-sshd");
        let text = render(dir, 2222, Algorithms::Default);
        for line in [
            "Port 2222",
            "ListenAddress 127.0.0.1",
            "UsePAM no",
            "PasswordAuthentication no",
            "KbdInteractiveAuthentication no",
            "AllowTcpForwarding local",
            "PermitTTY no",
        ] {
            assert!(text.lines().any(|l| l == line), "{line} missing:\n{text}");
        }
        assert!(!text.contains("Ciphers") && !text.contains("KexAlgorithms"));
        let minimum = render(dir, 2222, Algorithms::SurgeMinimum);
        assert!(
            minimum.ends_with("KexAlgorithms curve25519-sha256\nCiphers aes128-gcm@openssh.com\n"),
            "{minimum}"
        );
    }
}
```

- [ ] **Step 2: 运行夹具的单元用例**

Run: `cargo test -p rurge-interop --lib sshd` → 1 passed（`the_configuration_stays_on_the_loopback_with_one_key`：`Port`、`ListenAddress 127.0.0.1`、`UsePAM no`、`PasswordAuthentication no`、`KbdInteractiveAuthentication no`、`AllowTcpForwarding local`、`PermitTTY no` 都在；默认算法时不写 `Ciphers` / `KexAlgorithms`，最低算法时恰好写那两行）。

- [ ] **Step 3: 用例**

新建 `tests/interop/tests/sshd.rs`：

```rust
//! The `ssh` outbound against OpenSSH's `sshd` (phase 2 M4 design §10): with
//! OpenSSH's default algorithms, and with only the two Surge's manual
//! requires. Unix only: a `sshd` that is not run as root logs in only the
//! user running it, with a key.

#![cfg(unix)]

mod common;

use common::*;
use rurge_interop::sshd::{Algorithms, Sshd, sshd_or_skip};
use rurge_proto_ssh::testing::{
    Algorithm, fingerprint_of, keystore_base64, openssh_text, random_key,
};

async fn forward_through(algorithms: Algorithms, test: &str) {
    let Some(binary) = sshd_or_skip(test) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let host = random_key(Algorithm::Ed25519);
    let client = random_key(Algorithm::Ed25519);
    let sshd = Sshd::spawn(
        &binary,
        dir.path(),
        &openssh_text(&host),
        &fingerprint_of(client.public_key()),
        algorithms,
    );
    let user = std::env::var("USER").expect("USER names the user sshd logs in");
    let profile = format!(
        "[Proxy]\nS = ssh, 127.0.0.1, {}, username={user}, private-key=key1, server-fingerprint=\"{}\"\n\
[Keystore]\nkey1 = type=openssh-private-key, base64={}\n[Rule]\nFINAL,DIRECT\n",
        sshd.port(),
        fingerprint_of(host.public_key()),
        keystore_base64(&client)
    );
    let out = outbound(&profile, "S", None);
    roundtrip(&out, echo_server().await).await;
}

#[tokio::test]
async fn ssh_forwards_through_openssh() {
    forward_through(Algorithms::Default, "ssh_forwards_through_openssh").await;
}

#[tokio::test]
async fn ssh_reaches_an_openssh_offering_only_surges_algorithms() {
    forward_through(
        Algorithms::SurgeMinimum,
        "ssh_reaches_an_openssh_offering_only_surges_algorithms",
    )
    .await;
}
```

- [ ] **Step 4: CI 与说明**

`.github/workflows/ci.yml`——把

```yaml
          echo "RURGE_TEST_XRAY=$bin" >> "$GITHUB_ENV"
```

换成

```yaml
          echo "RURGE_TEST_XRAY=$bin" >> "$GITHUB_ENV"
      - name: Point the SSH interoperability tests at OpenSSH's sshd
        if: runner.os != 'Windows'
        shell: bash
        run: |
          if [ ! -x /usr/sbin/sshd ]; then
            sudo apt-get update -q
            sudo apt-get install -y -q --no-install-recommends openssh-server
          fi
          echo "RURGE_TEST_SSHD=/usr/sbin/sshd" >> "$GITHUB_ENV"
```

`tests/interop/README.md`——把

```markdown
渲染出的配置（`rurge_interop::xray::render`）只有 `log` / `inbounds` / `outbounds` 三个顶层键：每个入站是一个只监听 `127.0.0.1` 的 `vmess`（可选 `ws` 传输），唯一的出站是 `freedom`。

## 安全约束
```

换成

```markdown
渲染出的配置（`rurge_interop::xray::render`）只有 `log` / `inbounds` / `outbounds` 三个顶层键：每个入站是一个只监听 `127.0.0.1` 的 `vmess`（可选 `ws` 传输），唯一的出站是 `freedom`。

## sshd

`tests/sshd.rs` 用 OpenSSH 的 `sshd` 验证 `ssh` 出站（阶段 2 M4 设计第 10 节）：OpenSSH 的默认算法，以及只开 Surge 手册要求的 `curve25519-sha256` 与 `aes128-gcm@openssh.com` 两种。不以 root 运行的 `sshd` 只能让运行它的用户登录，所以用例用密钥登录（用户名取环境变量 `USER`），并且只在 Unix 上编译。

查找顺序：`RURGE_TEST_SSHD`，然后 `/usr/sbin/sshd`，然后 `PATH` 上的 `sshd`；都没有就打印 `skipping …` 后返回（`RURGE_INTEROP_REQUIRED=1` 时改为失败）。CI 在 Linux 与 macOS 上设置 `RURGE_TEST_SSHD=/usr/sbin/sshd`，Linux 上缺它时先装 `openssh-server`；Windows 上不编译这两个用例。

渲染出的 `sshd_config`（`rurge_interop::sshd::render`）只监听 `127.0.0.1`，只认用例现场生成的那一把公钥，关掉口令、PAM 与终端，只允许本地端口转发（夹具的单元用例 `the_configuration_stays_on_the_loopback_with_one_key` 断言这一点）；主机密钥同样现场生成，写进临时目录。

## 安全约束
```

- [ ] **Step 5: 运行**

Run: `cargo test -p rurge-interop`
Expected: 本机（Windows）上 `tests/sshd.rs` 编译成没有用例的测试二进制（`0 passed`），夹具库的单元用例 6 passed，其余与 Task 5 之后相同（没有 sing-box / xray 的用例照旧打印 `skipping …`）。**不要安装 `sshd` 来"验证"它们。**

要点：
- 查找顺序：`RURGE_TEST_SSHD`，然后 `/usr/sbin/sshd`，然后 `PATH` 上的 `sshd`；都没有时打印 `skipping …` 后返回，`RURGE_INTEROP_REQUIRED=1`（CI）时改为失败。
- `sshd -D -e -f <配置>`：前台运行、日志写 stderr（落到临时目录的日志文件），`Reference::start` 等它开始接受连接；用例结束时随 drop 杀掉并回收。
- 主机密钥文件设成 0600（`sshd` 拒绝别人可读的主机密钥）；`StrictModes no` 让它不检查临时目录的属主与权限。
- 客户端这一侧的配置是 rurge 的普通策略行：`private-key` 指向 `[Keystore]` 里现场生成的密钥，`server-fingerprint` 写 `sshd` 的主机公钥——互操作同时验证了指纹比对。

- [ ] **Step 6: 门禁与提交**

跑门禁（45 个测试二进制，1028 通过 / 1 忽略）。

```bash
git add Cargo.lock crates/rurge-proto-ssh/src/testing/mod.rs tests/interop .github/workflows/ci.yml
git commit -m "test(interop): ssh 出站对 OpenSSH sshd 的互操作（默认算法与 Surge 最低算法，只在 Unix），CI 设置 RURGE_TEST_SSHD"
```

### Task 7: 能力表翻转 `ssh` 与文档

`ssh` 的全部行为已经就位（Task 1 ～ 6），翻转 bin 的能力表：此后 `W0007` 不再因 `ssh` 出现（设计 8.4、验收第 4 条）。翻转前核对设计承诺的行为都已存在（M2 设计第 8 节的教训）：参数与 `server-fingerprint`（Task 1）、私钥解码与主机密钥比对（Task 2）、会话 / 单飞 / 通道 / 重建 / 认证 / 错误文本（Task 3）、空闲断开 / 保活 / 告警（Task 4）、工厂与重载指纹（Task 5）、互操作（Task 6）。然后是文档：兼容性清单（设计第 12 节的 `ssh` 部分）、两份 README、`CLAUDE.md`、手工验收清单的 M4a 一节、M4 设计新增的第 17 节（本计划与设计文字不同的地方）、总设计第 16 节 Q5 与技术选型表的 SSH 一行。本计划末尾的「执行期修正记录」与「延后事项」两张表，由控制者在派发本任务时给出要补的行（执行中的偏差、各任务门禁的实际数字、执行中新发现的延后事项），一并写入。

两次提交：能力表一次，文档一次。

**Files:**
- Modify: `crates/rurge/src/capabilities.rs`
- Test: `crates/rurge/tests/cli.rs`（`check_knows_ssh`）
- Modify: `docs/surge-compatibility-matrix.md`、`README.md`、`README_en.md`、`CLAUDE.md`、`docs/acceptance/phase2-manual.md`、`docs/superpowers/specs/2026-09-27-phase2-m4-wireguard-ssh-external-design.md`、`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`
- Modify: 本计划文件末尾两张表（按控制者给的行）

**Interfaces:**
- Consumes: Task 1 ～ 6。
- Produces: 无新接口；`capabilities::current()` 的 `policy_kinds` 多了 `PolicyKind::Ssh`。

- [ ] **Step 1: 先写用例**

`crates/rurge/tests/cli.rs`——把

```rust
        .stdout(predicate::str::contains("s3cretnotauuid").not());
```

换成

```rust
        .stdout(predicate::str::contains("s3cretnotauuid").not());
}

const SSH: &str = "[General]\n[Proxy]\n\
S = ssh, proxy.test, 22, username=u, password=s3same, idle-timeout=60\n\
Old = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\n[Rule]\nFINAL,DIRECT\n";
const SSH_BAD_KEY: &str = "[General]\n[Proxy]\nS = ssh, proxy.test, 22, username=u, private-key=key1\n\
[Keystore]\nkey1 = type=openssh-private-key, base64=c2VjcmV0IGtleSBtYXRlcmlhbA==\n[Rule]\nFINAL,DIRECT\n";

/// `rurge check` builds `ssh` policies: a key rurge cannot use is an error,
/// named and not quoted (M4 design 4.5).
#[test]
fn check_knows_ssh() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "ssh.conf", SSH))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    // `ss` is still a later milestone; `ssh` is not
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(out.contains("`ss`") && !out.contains("`ssh`"), "{out}");
    assert!(!out.contains("s3same"), "{out}");

    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "bad.conf", SSH_BAD_KEY))
        .assert()
        .code(2)
        .stdout(predicate::str::contains("E0022"))
        .stdout(predicate::str::contains("bad.conf:3"))
        .stdout(predicate::str::contains(
            "keystore item `key1` is not an OpenSSH private key",
        ))
        .stdout(predicate::str::contains("c2VjcmV0").not());
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge --test cli check_knows_ssh`
Expected: FAIL——`ssh` 仍报 `W0007`（临时目录名每次不同）：

```text
test check_knows_ssh ... FAILED
thread 'check_knows_ssh' panicked at crates\rurge\tests\cli.rs:289:5:
assertion `left == right` failed: warning[W0007] …\ssh.conf:3: policy type `ssh` is not implemented in this version; such policies behave as REJECT
warning[W0007] …\ssh.conf:4: policy type `ss` is not implemented in this version; such policies behave as REJECT
…\ssh.conf: 0 error(s), 2 warning(s), 0 note(s)
  left: 2
 right: 1
```

- [ ] **Step 3: 实现**

`crates/rurge/src/capabilities.rs`——把

```rust
//! M2b), `select` groups, `url-test` / `fallback` / `load-balance` groups
//! (phase 2 M3b), and `smart` groups (phase 2 M3c).
```

换成

```rust
//! M2b), `ssh` (phase 2 M4a), `select` groups, `url-test` / `fallback` /
//! `load-balance` groups (phase 2 M3b), and `smart` groups (phase 2 M3c).
```

`crates/rurge/src/capabilities.rs`——把

```rust
            PolicyKind::AnyTls,
```

换成

```rust
            PolicyKind::AnyTls,
            PolicyKind::Ssh,
```

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge --test cli check_knows` → 5 passed（`W0007` 只报 `ss`，输出里没有口令；带不是私钥的 `[Keystore]` 条目的配置退出 2，`E0022` 落在 `ssh` 策略那一行，文本点名条目、不含它的 Base64）。

- [ ] **Step 5: 门禁与提交（能力表）**

跑门禁（45 个测试二进制，1029 通过 / 1 忽略）。

```bash
git add crates/rurge/src/capabilities.rs crates/rurge/tests/cli.rs
git commit -m "feat(rurge): 能力表翻转 ssh（W0007 不再因 ssh 出现）"
```

- [ ] **Step 6: 兼容性清单**

`docs/surge-compatibility-matrix.md`——把

```markdown
| `[Keystore]` 条目 `name = type=, base64=, password=` | `type` 为 `p12` 或 `openssh-private-key`，可省略推断 | ✅ | 2 | M1 已实现：加载期校验 Base64（`E0021`）与引用（`E0020`），干构建期解码（`E0022`）；OpenSSL 3 默认加密与 `-legacy`（RC2-40 + 3DES）两种都能读 |
| 引用点：`client-cert` / `ca-keystore-name` / `private-key` | | ✅ | 2 / 4 | |
```

换成

```markdown
| `[Keystore]` 条目 `name = type=, base64=, password=` | `type` 为 `p12` 或 `openssh-private-key`，可省略推断 | ✅ | 2 | M1 已实现：加载期校验 Base64（`E0021`）与引用（`E0020`），干构建期解码（`E0022`）；OpenSSL 3 默认加密与 `-legacy`（RC2-40 + 3DES）两种都能读；`openssh-private-key` 自 M4a 起由 `ssh` 的 `private-key` 使用：支持 Ed25519、ECDSA（P-256 / P-384 / P-521）与 RSA；带口令的私钥（手册的 `password` 只用于 p12）与 DSA 私钥不支持，干构建期 `E0022`（文本只点名条目，不引用内容）；`ssh` 引用的条目不存在或是 p12 时 `E0020` |
| 引用点：`client-cert` / `ca-keystore-name` / `private-key` | | ✅ | 2 / 4 | `client-cert`（M1）与 `ssh` 的 `private-key`（M4a）已实现，`ca-keystore-name` 属阶段 4；条目内容变了（名字没变）的重载会重建引用它的策略 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `ssh` | SSH 动态转发 | 全部 | ✅ | 2 | |
```

换成

```markdown
| `ssh` | SSH 动态转发 | 全部 | ✅ | 2 | M4a（阶段 2）已实现（TCP）：每个策略一条 SSH 会话，每个连接一个 `direct-tcpip` 通道，同时进来的连接共用一次握手；会话断了（服务器断开、保活无回应）时下一个连接重建一次会话；目标主机名交给 SSH 服务器解析；可叠加 Shadow TLS、可经 `underlying-proxy` 连出去，也可以当别的策略的 `underlying-proxy`；不支持 UDP（与 Surge 一致）。参数与差异见 4.6 节 `ssh` 行 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| 策略指向 rurge 尚未实现的协议类型 | Surge 原生支持全部协议 | 全部 | 🟡 | 1 / 2 | 加载时告警 `W0007`；运行时该策略按 `REJECT` 处理，会话日志 `error = policy protocol not implemented: <type>`；随阶段 2 各里程碑逐协议移除：M1 已移除 `http` `https` `socks5` `socks5-tls`；M2a 已移除 `trojan`；M2b 已移除 `vmess`（写了 `vmess-aead=true` 的行）与 `anytls`。没写 `vmess-aead=true` 的 `vmess` 行是唯一例外：仍按 `W0007` 处理，但走专门的诊断文本 `` `vmess` without `vmess-aead=true` uses the legacy handshake, which is not implemented yet ``（每次加载一条，不是每行一条）与专门的会话日志文本 `policy protocol not implemented: vmess (legacy handshake)`，不是这里的通用 `<type>` 模板 |
```

换成

```markdown
| 策略指向 rurge 尚未实现的协议类型 | Surge 原生支持全部协议 | 全部 | 🟡 | 1 / 2 | 加载时告警 `W0007`；运行时该策略按 `REJECT` 处理，会话日志 `error = policy protocol not implemented: <type>`；随阶段 2 各里程碑逐协议移除：M1 已移除 `http` `https` `socks5` `socks5-tls`；M2a 已移除 `trojan`；M2b 已移除 `vmess`（写了 `vmess-aead=true` 的行）与 `anytls`；M4a 已移除 `ssh`。没写 `vmess-aead=true` 的 `vmess` 行是唯一例外：仍按 `W0007` 处理，但走专门的诊断文本 `` `vmess` without `vmess-aead=true` uses the legacy handshake, which is not implemented yet ``（每次加载一条，不是每行一条）与专门的会话日志文本 `policy protocol not implemented: vmess (legacy handshake)`，不是这里的通用 `<type>` 模板 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `shadow-tls-password` | 字符串；设置即启用 Shadow TLS | ✅ | 2 | M2c 已实现：所有 TCP 类出站（`http` `https` `socks5` `socks5-tls` `trojan` `vmess` `anytls`）；顺序是 connect → shadow-tls → tls → ws → 协议。伪装握手照常校验证书（出站的根证书库），**不受本节其余六个 TLS 参数影响**（它们只作用于里层的真实 TLS）；伪装握手不带 ALPN。口令为空 `E0018`；没有口令时另外两个参数各报一条 `W0028` |
```

换成

```markdown
| `shadow-tls-password` | 字符串；设置即启用 Shadow TLS | ✅ | 2 | M2c 已实现：所有 TCP 类出站（`http` `https` `socks5` `socks5-tls` `trojan` `vmess` `anytls`，M4a 起还有 `ssh`）；顺序是 connect → shadow-tls → tls → ws → 协议。伪装握手照常校验证书（出站的根证书库），**不受本节其余六个 TLS 参数影响**（它们只作用于里层的真实 TLS）；伪装握手不带 ALPN。口令为空 `E0018`；没有口令时另外两个参数各报一条 `W0028` |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `ssh` | `username` `password` \| `private-key`（Keystore 名）`idle-timeout`（默认 180）`server-fingerprint`（多指纹逗号分隔） | ✅ | 2 | Surge 仅 `curve25519-sha256` + `aes128-gcm`；rurge 至少支持这两者（可为超集）；未配置指纹时一次性安全告警 |
```

换成

```markdown
| `ssh` | `username` `password` \| `private-key`（Keystore 名）`idle-timeout`（默认 180）`server-fingerprint`（多指纹逗号分隔） | ✅ | 2 | M4a 已实现。`username` 必填，`password` 与 `private-key` 至少一个（`E0018`），都写时先试密钥、再试口令；认证失败一律是 `ssh: authentication failed`（不带用户名与凭据）。`private-key` 的条目见 1.6 节 Keystore 行；RSA 私钥只用 rsa-sha2-512 / rsa-sha2-256 签名（服务器的 `server-sig-algs` 列了 512 就用它，否则用 256），只认 `ssh-rsa`（SHA-1）的老服务器（OpenSSH 7.2 以前）用 RSA 私钥登录不了。`idle-timeout`（秒，至少 1）：会话上**没有打开的通道**持续这么久才断开会话，下次连接时重建——连接还开着、只是很久没有流量时不会被切断（手册没说"空闲"怎么算，rurge 自定）；会话存在期间每 30 秒一次 keepalive，连续 3 次无回应判定会话已断（手册未写，rurge 自定）。`server-fingerprint`：按手册写成 `算法 base64公钥`，多个以逗号分隔、整值加引号，写错是 `E0018`；服务器的主机密钥（主机证书则取证书里的公钥）按"算法 + 公钥"比对，不在列表里时握手失败（`ssh: the server's host key is not one of server-fingerprint`）；没配时照 Surge 接受，每个出站在第一次建会话时告警一次（`ssh: no server-fingerprint; the server's host key is not verified`，日志只带策略名；重载时参数没变的策略沿用原出站，不再告警）。算法：russh 的默认列表（kex 含 `mlkem768x25519-sha256` 与 `curve25519-sha256`）加上 Surge 要求的 `aes128-gcm@openssh.com`（russh 默认没有它），去掉 SHA-1 的主机密钥签名 `ssh-rsa`，是 Surge 要求的超集。握手失败是 `ssh: the handshake failed`（没有共同算法时带说明），开通道被拒是 `ssh: the server refused the channel (<RFC 4254 的原因名>)`，都不引用服务器的原文。TLS 参数不适用（`W0028`）；订阅行自己写的 `private-key=` 不生效（见 5.2 节 `policy-path` 行） |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `policy-path` | 除 subnet 外 | 文件路径或 URL；内容为策略行列表或含 `[Proxy]` 的完整配置；远程缓存并定期更新。M3a 已实现，差异：只接受 Surge 格式（Clash / base64 解析不出策略时告警）；下载经 rurge 自己的直连，不经代理（未与 Surge 核对）；下载请求的 `User-Agent` 是 `rurge/<版本> (Surge-compatible)`：按 UA 里有没有 "surge" 选格式的机场面板（如 V2Board）因此返回 Surge 格式（未在真实面板上核对）；不认这个 UA 的面板可能返回非 Surge 格式的正文（组按空组兜底并提示"可能不是 Surge 格式"），订阅链接自带的格式参数（如 V2Board 的 `flag=surge`）能避开；值在 `profiles/current` 与 `policies/detail` 里脱敏，日志只写组名、不写 URL；单个订阅最多 10 000 条策略、只读前 100 000 行；跳过的行只逐条报前 20 条，其余合计一条；首次下载不阻塞启动（组先按空组兜底），已有缓存时启动与重载同步载入；订阅更新只重建策略表（没变的成员沿用原出站），不打断无关的连接；坏行、重名、与配置同名的行跳过并告警（只报行号与原因）；订阅行自己写的 `client-cert=` 与指向主配置策略或组的 `underlying-proxy=` 不生效，该行跳过并 `W0023`（M3b；只有 `external-policy-modifier` 能给导入行设这两项，订阅内部导入行之间的 `underlying-proxy` 照常可用；检查的是套用修饰之后的整行，行的写法（带引号的整项、未闭合的引号）让修饰设的值落空时同样跳过）——Surge 未这样限制，rurge 不让订阅作者动用主配置里的证书与代理 | 🟡 | 2 |
```

换成

```markdown
| `policy-path` | 除 subnet 外 | 文件路径或 URL；内容为策略行列表或含 `[Proxy]` 的完整配置；远程缓存并定期更新。M3a 已实现，差异：只接受 Surge 格式（Clash / base64 解析不出策略时告警）；下载经 rurge 自己的直连，不经代理（未与 Surge 核对）；下载请求的 `User-Agent` 是 `rurge/<版本> (Surge-compatible)`：按 UA 里有没有 "surge" 选格式的机场面板（如 V2Board）因此返回 Surge 格式（未在真实面板上核对）；不认这个 UA 的面板可能返回非 Surge 格式的正文（组按空组兜底并提示"可能不是 Surge 格式"），订阅链接自带的格式参数（如 V2Board 的 `flag=surge`）能避开；值在 `profiles/current` 与 `policies/detail` 里脱敏，日志只写组名、不写 URL；单个订阅最多 10 000 条策略、只读前 100 000 行；跳过的行只逐条报前 20 条，其余合计一条；首次下载不阻塞启动（组先按空组兜底），已有缓存时启动与重载同步载入；订阅更新只重建策略表（没变的成员沿用原出站），不打断无关的连接；坏行、重名、与配置同名的行跳过并告警（只报行号与原因）；订阅行自己写的 `client-cert=`、`private-key=`（`ssh`，M4a）与指向主配置策略或组的 `underlying-proxy=` 不生效，该行跳过并 `W0023`（M3b；只有 `external-policy-modifier` 能给导入行设这几项，订阅内部导入行之间的 `underlying-proxy` 照常可用；检查的是套用修饰之后的整行，行的写法（带引号的整项、未闭合的引号）让修饰设的值落空时同样跳过）——Surge 未这样限制，rurge 不让订阅作者动用主配置里的证书、私钥与代理 | 🟡 | 2 |
```

- [ ] **Step 7: 两份 README 与 `CLAUDE.md`**

`README.md`——把

```markdown
> **阶段 1 功能齐备：M1 ～ M4b**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）进行中：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；M3b（测速与自动组）已完成——`url-test` / `fallback` / `load-balance` 组按连通性测试（经各节点在同一条连接上两次 `HEAD`，HTTPS 测试 URL 在已建立的 TLS 连接上测第二次）自动选择，`interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` 与策略的 `test-url` / `test-timeout` 生效，自动组可经 `POST /v1/policy_groups/select` 临时覆盖，新增 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 三个端点；M3c（`smart`）已完成——`smart` 组按真实会话的首字节耗时与测速结果打分（失败罚分、`policy-priority` 因子、约 1 小时的站点记忆），选中的成员连不上时自动换下一个（最多再试 2 个），请求记录增加 `connectMs` / `firstByteMs`；其余出站协议仍在阶段 2 后续里程碑，`subnet` 策略组在阶段 3。
```

换成

```markdown
> **阶段 1 功能齐备：M1 ～ M4b**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）进行中：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；M3b（测速与自动组）已完成——`url-test` / `fallback` / `load-balance` 组按连通性测试（经各节点在同一条连接上两次 `HEAD`，HTTPS 测试 URL 在已建立的 TLS 连接上测第二次）自动选择，`interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` 与策略的 `test-url` / `test-timeout` 生效，自动组可经 `POST /v1/policy_groups/select` 临时覆盖，新增 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 三个端点；M3c（`smart`）已完成——`smart` 组按真实会话的首字节耗时与测速结果打分（失败罚分、`policy-priority` 因子、约 1 小时的站点记忆），选中的成员连不上时自动换下一个（最多再试 2 个），请求记录增加 `connectMs` / `firstByteMs`；M4a（SSH）已完成——`ssh` 策略（TCP；每个策略一条 SSH 会话、每个连接一个通道；口令或 `[Keystore]` 里的 OpenSSH 私钥登录；`server-fingerprint` 校验主机密钥；按 `idle-timeout` 断开空闲会话、30 秒一次保活）已可用；其余出站协议仍在阶段 2 后续里程碑，`subnet` 策略组在阶段 3。
```

`README.md`——把

```markdown
| 出站协议       | DIRECT / REJECT 系列 / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / 外部程序（DIRECT 与 REJECT 系四种已实现，M3a；HTTP / SOCKS5（含 `socks5-tls`）TCP 上游与 `underlying-proxy` 链已实现，阶段 2 / M1；Trojan（TCP；WebSocket 传输）已实现，阶段 2 / M2a；VMess（AEAD 握手，TCP；可叠加 TLS / WebSocket）与 AnyTLS（TCP，会话复用）已实现，阶段 2 / M2b） | 2     |
```

换成

```markdown
| 出站协议       | DIRECT / REJECT 系列 / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / 外部程序（DIRECT 与 REJECT 系四种已实现，M3a；HTTP / SOCKS5（含 `socks5-tls`）TCP 上游与 `underlying-proxy` 链已实现，阶段 2 / M1；Trojan（TCP；WebSocket 传输）已实现，阶段 2 / M2a；VMess（AEAD 握手，TCP；可叠加 TLS / WebSocket）与 AnyTLS（TCP，会话复用）已实现，阶段 2 / M2b；SSH（TCP，会话复用）已实现，阶段 2 / M4a） | 2     |
```

`README.md`——把

```markdown
3. **阶段 2** 出站协议全集、策略组、策略订阅（进行中：M1 出站地基与 HTTP / SOCKS5 上游已完成；M2a（Trojan，TCP + WebSocket）已完成；M2b（VMess / AnyTLS）已完成；M2c（Shadow TLS）已完成；M3a（成员装配与订阅）已完成；M3b（测速与自动组）已完成；M3c（`smart`）已完成）
```

换成

```markdown
3. **阶段 2** 出站协议全集、策略组、策略订阅（进行中：M1 出站地基与 HTTP / SOCKS5 上游已完成；M2a（Trojan，TCP + WebSocket）已完成；M2b（VMess / AnyTLS）已完成；M2c（Shadow TLS）已完成；M3a（成员装配与订阅）已完成；M3b（测速与自动组）已完成；M3c（`smart`）已完成；M4a（SSH）已完成）
```

`README.md`——把

```markdown
> `rurge check`、`rurge rule match`、`rurge dns lookup` 与 `rurge run`（HTTP / SOCKS5 代理，DIRECT / REJECT，以及 `http` / `https` / `socks5` / `socks5-tls` / `trojan` / `vmess` / `anytls` 上游——均可叠加 Shadow TLS，含 `underlying-proxy` 链）已可用；`select` / `url-test` / `fallback` / `load-balance` / `smart` 组已可用，`subnet` 在阶段 3。HTTP API 与 `rurge reload` / `stop` / `status` 已可用（见 [docs/api/phase1.md](docs/api/phase1.md)，阶段 2 新增端点见 [docs/api/phase2.md](docs/api/phase2.md)）。`rurge run --system-proxy` 可以把系统代理指向 rurge，退出时恢复、崩溃后在下次启动时恢复；`rurge service install | uninstall [--user] [--dry-run]` 可以注册 / 移除开机自启（systemd / launchd / Windows 计划任务）。macOS 经 `networksetup` 设置，通常需要管理员账户；是否需要 `sudo` 尚未在真机验证（见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)），失败时 rurge 原样报出工具的错误。`rurge run` 另支持 `--idle-timeout`、`--request-log-size`、`--watch`（配置热重载）、`--log-file`（按天滚动）、`--empty-group-reject`（没有成员的策略组拒绝而不是直连）等 rurge 专有运行时选项，只经命令行参数 / 环境变量提供，不写入 Surge 配置文件。
```

换成

```markdown
> `rurge check`、`rurge rule match`、`rurge dns lookup` 与 `rurge run`（HTTP / SOCKS5 代理，DIRECT / REJECT，以及 `http` / `https` / `socks5` / `socks5-tls` / `trojan` / `vmess` / `anytls` / `ssh` 上游——均可叠加 Shadow TLS，含 `underlying-proxy` 链）已可用；`select` / `url-test` / `fallback` / `load-balance` / `smart` 组已可用，`subnet` 在阶段 3。HTTP API 与 `rurge reload` / `stop` / `status` 已可用（见 [docs/api/phase1.md](docs/api/phase1.md)，阶段 2 新增端点见 [docs/api/phase2.md](docs/api/phase2.md)）。`rurge run --system-proxy` 可以把系统代理指向 rurge，退出时恢复、崩溃后在下次启动时恢复；`rurge service install | uninstall [--user] [--dry-run]` 可以注册 / 移除开机自启（systemd / launchd / Windows 计划任务）。macOS 经 `networksetup` 设置，通常需要管理员账户；是否需要 `sudo` 尚未在真机验证（见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)），失败时 rurge 原样报出工具的错误。`rurge run` 另支持 `--idle-timeout`、`--request-log-size`、`--watch`（配置热重载）、`--log-file`（按天滚动）、`--empty-group-reject`（没有成员的策略组拒绝而不是直连）等 rurge 专有运行时选项，只经命令行参数 / 环境变量提供，不写入 Surge 配置文件。
```

`README_en.md`——把

```markdown
> **Phase 1 feature-complete: M1 through M4b** (profile parsing, rule engine, rule sets, GeoIP, external resource management, DNS client, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`; M3b added the request log and traffic stats, SNI recording, idle timeout, REJECT auto-escalation, a 502 for failed CONNECT dials, graceful shutdown, hot reload (SIGHUP / `--watch`), `--log-file`, and `encrypted-dns-follow-outbound-mode`; M4a added a Surge-compatible HTTP API (phase-1 endpoints, `X-Key` auth with banning), outbound mode / global policy persisted to `state.json`, and `rurge reload` / `stop` / `status`; M4b added the system proxy (Windows registry + a WinINet notification, macOS `networksetup`, Linux GNOME / KDE, `--system-proxy` and `POST /v1/features/system_proxy`, automatic recovery on exit or crash, and following listener changes across a reload) and `rurge service install / uninstall [--dry-run]` (systemd / launchd / a Windows scheduled task)) — the phase-1 acceptance criteria that need a real desktop environment (the three-platform system proxy, service install) have not been through manual acceptance yet; see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md) for the checklist. The dashboard is phase 6. Phase 2 (outbound protocols and policy groups) is in progress: M1 (outbound foundation and HTTP / SOCKS5 upstreams) is done — `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections by rule through DIRECT / REJECT or `http` / `https` / `socks5` / `socks5-tls` upstreams (TCP, including two-or-more-hop `underlying-proxy` chains), and a `select` group's choice can be read and switched over the HTTP API (see [docs/api/phase2.md](docs/api/phase2.md)); M2a (the TLS family, Trojan first) is done — the `trojan` policy (TCP, TLS always on, optionally over WebSocket) is usable; M2b (VMess / AnyTLS) is done — the `vmess` (AEAD handshake, TCP, optionally over TLS / WebSocket) and `anytls` (TCP, session reuse) policies are usable, and a reload reuses outbounds by fingerprint without disturbing connection pools still in use; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); M3a (member assembly and subscriptions) is done — `policy-path` subscriptions (a local file or a URL, in Surge format; an update rebuilds only the policy table, without disturbing unrelated connections), `include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`, the group-level `underlying-proxy` (derived `Name (via Relay)` members), group cycle warnings (`W0030`) and the empty-group fallback (DIRECT by default, REJECT with `--empty-group-reject`) are usable; M3b (connectivity tests and automatic groups) is done — `url-test` / `fallback` / `load-balance` groups choose by connectivity tests (two `HEAD`s over one connection through each member, the second one on the established TLS connection for an HTTPS test URL), `interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` and the policies' `test-url` / `test-timeout` take effect, an automatic group takes a temporary override through `POST /v1/policy_groups/select`, and `POST /v1/policies/test`, `GET /v1/policy_groups/test_results` and `POST /v1/policy_groups/test` are available; M3c (`smart`) is done — a `smart` group scores its members by the first-byte time of real sessions and by the tests (failure penalties, `policy-priority` factors, per-site memory for about an hour) and moves on to the next member when the one it picked does not connect (up to two more), and the request log gains `connectMs` / `firstByteMs`; the remaining outbound protocols are later phase-2 milestones, and the `subnet` group comes in phase 3.
```

换成

```markdown
> **Phase 1 feature-complete: M1 through M4b** (profile parsing, rule engine, rule sets, GeoIP, external resource management, DNS client, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`; M3b added the request log and traffic stats, SNI recording, idle timeout, REJECT auto-escalation, a 502 for failed CONNECT dials, graceful shutdown, hot reload (SIGHUP / `--watch`), `--log-file`, and `encrypted-dns-follow-outbound-mode`; M4a added a Surge-compatible HTTP API (phase-1 endpoints, `X-Key` auth with banning), outbound mode / global policy persisted to `state.json`, and `rurge reload` / `stop` / `status`; M4b added the system proxy (Windows registry + a WinINet notification, macOS `networksetup`, Linux GNOME / KDE, `--system-proxy` and `POST /v1/features/system_proxy`, automatic recovery on exit or crash, and following listener changes across a reload) and `rurge service install / uninstall [--dry-run]` (systemd / launchd / a Windows scheduled task)) — the phase-1 acceptance criteria that need a real desktop environment (the three-platform system proxy, service install) have not been through manual acceptance yet; see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md) for the checklist. The dashboard is phase 6. Phase 2 (outbound protocols and policy groups) is in progress: M1 (outbound foundation and HTTP / SOCKS5 upstreams) is done — `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections by rule through DIRECT / REJECT or `http` / `https` / `socks5` / `socks5-tls` upstreams (TCP, including two-or-more-hop `underlying-proxy` chains), and a `select` group's choice can be read and switched over the HTTP API (see [docs/api/phase2.md](docs/api/phase2.md)); M2a (the TLS family, Trojan first) is done — the `trojan` policy (TCP, TLS always on, optionally over WebSocket) is usable; M2b (VMess / AnyTLS) is done — the `vmess` (AEAD handshake, TCP, optionally over TLS / WebSocket) and `anytls` (TCP, session reuse) policies are usable, and a reload reuses outbounds by fingerprint without disturbing connection pools still in use; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); M3a (member assembly and subscriptions) is done — `policy-path` subscriptions (a local file or a URL, in Surge format; an update rebuilds only the policy table, without disturbing unrelated connections), `include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`, the group-level `underlying-proxy` (derived `Name (via Relay)` members), group cycle warnings (`W0030`) and the empty-group fallback (DIRECT by default, REJECT with `--empty-group-reject`) are usable; M3b (connectivity tests and automatic groups) is done — `url-test` / `fallback` / `load-balance` groups choose by connectivity tests (two `HEAD`s over one connection through each member, the second one on the established TLS connection for an HTTPS test URL), `interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` and the policies' `test-url` / `test-timeout` take effect, an automatic group takes a temporary override through `POST /v1/policy_groups/select`, and `POST /v1/policies/test`, `GET /v1/policy_groups/test_results` and `POST /v1/policy_groups/test` are available; M3c (`smart`) is done — a `smart` group scores its members by the first-byte time of real sessions and by the tests (failure penalties, `policy-priority` factors, per-site memory for about an hour) and moves on to the next member when the one it picked does not connect (up to two more), and the request log gains `connectMs` / `firstByteMs`; M4a (SSH) is done — the `ssh` policy (TCP; one SSH session per policy and a channel per connection; login with a password or an OpenSSH private key from `[Keystore]`; host keys checked against `server-fingerprint`; idle sessions closed after `idle-timeout`, a keepalive every 30 seconds) is usable; the remaining outbound protocols are later phase-2 milestones, and the `subnet` group comes in phase 3.
```

`README_en.md`——把

```markdown
| Outbound            | DIRECT / REJECT family / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / external program (DIRECT and the four REJECT flavours implemented, M3a; HTTP / SOCKS5 (including `socks5-tls`) TCP upstreams and `underlying-proxy` chains implemented, phase 2 / M1; Trojan (TCP; WebSocket transport) implemented, phase 2 / M2a; VMess (AEAD handshake, TCP; optionally TLS / WebSocket) and AnyTLS (TCP, session reuse) implemented, phase 2 / M2b) | 2     |
```

换成

```markdown
| Outbound            | DIRECT / REJECT family / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / external program (DIRECT and the four REJECT flavours implemented, M3a; HTTP / SOCKS5 (including `socks5-tls`) TCP upstreams and `underlying-proxy` chains implemented, phase 2 / M1; Trojan (TCP; WebSocket transport) implemented, phase 2 / M2a; VMess (AEAD handshake, TCP; optionally TLS / WebSocket) and AnyTLS (TCP, session reuse) implemented, phase 2 / M2b; SSH (TCP, session reuse) implemented, phase 2 / M4a) | 2     |
```

`README_en.md`——把

```markdown
3. **Phase 2** All outbound protocols, policy groups, subscriptions (in progress: M1, the outbound foundation and HTTP / SOCKS5 upstreams, is done; M2a, Trojan (TCP + WebSocket), is done; M2b, VMess / AnyTLS, is done; M2c, Shadow TLS, is done; M3a, member assembly and subscriptions, is done; M3b, connectivity tests and automatic groups, is done; M3c, `smart` groups, is done)
```

换成

```markdown
3. **Phase 2** All outbound protocols, policy groups, subscriptions (in progress: M1, the outbound foundation and HTTP / SOCKS5 upstreams, is done; M2a, Trojan (TCP + WebSocket), is done; M2b, VMess / AnyTLS, is done; M2c, Shadow TLS, is done; M3a, member assembly and subscriptions, is done; M3b, connectivity tests and automatic groups, is done; M3c, `smart` groups, is done; M4a, SSH, is done)
```

`README_en.md`——把

```markdown
> `rurge check`, `rurge rule match`, `rurge dns lookup` and `rurge run` (HTTP / SOCKS5 proxy, DIRECT / REJECT, and `http` / `https` / `socks5` / `socks5-tls` / `trojan` / `vmess` / `anytls` upstreams — all optionally wrapped in Shadow TLS — including `underlying-proxy` chains) work today; `select` / `url-test` / `fallback` / `load-balance` / `smart` groups work, `subnet` arrives in phase 3. The HTTP API and `rurge reload` / `stop` / `status` are available (see [docs/api/phase1.md](docs/api/phase1.md); phase-2 additions in [docs/api/phase2.md](docs/api/phase2.md)). `rurge run --system-proxy` points the system proxy at rurge and restores it on exit, or at the next start after a crash; `rurge service install | uninstall [--user] [--dry-run]` registers or removes automatic startup (systemd / launchd / a Windows scheduled task). On macOS, `networksetup` usually needs an administrator account; whether `sudo` is required has not been verified on real hardware yet (see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)) — on failure, rurge passes the tool's error through as-is. `rurge run` also takes rurge-specific runtime options — `--idle-timeout`, `--request-log-size`, `--watch` (hot reload), `--log-file` (daily rotation), `--empty-group-reject` (a policy group without members rejects instead of going direct) — as CLI flags / env vars only, never written into the Surge profile.
```

换成

```markdown
> `rurge check`, `rurge rule match`, `rurge dns lookup` and `rurge run` (HTTP / SOCKS5 proxy, DIRECT / REJECT, and `http` / `https` / `socks5` / `socks5-tls` / `trojan` / `vmess` / `anytls` / `ssh` upstreams — all optionally wrapped in Shadow TLS — including `underlying-proxy` chains) work today; `select` / `url-test` / `fallback` / `load-balance` / `smart` groups work, `subnet` arrives in phase 3. The HTTP API and `rurge reload` / `stop` / `status` are available (see [docs/api/phase1.md](docs/api/phase1.md); phase-2 additions in [docs/api/phase2.md](docs/api/phase2.md)). `rurge run --system-proxy` points the system proxy at rurge and restores it on exit, or at the next start after a crash; `rurge service install | uninstall [--user] [--dry-run]` registers or removes automatic startup (systemd / launchd / a Windows scheduled task). On macOS, `networksetup` usually needs an administrator account; whether `sudo` is required has not been verified on real hardware yet (see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)) — on failure, rurge passes the tool's error through as-is. `rurge run` also takes rurge-specific runtime options — `--idle-timeout`, `--request-log-size`, `--watch` (hot reload), `--log-file` (daily rotation), `--empty-group-reject` (a policy group without members rejects instead of going direct) — as CLI flags / env vars only, never written into the Surge profile.
```

`CLAUDE.md`——把

```markdown
阶段 1 功能已齐备（M1 ～ M4b）；阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`。M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。M3b（测速与自动组）已完成——`[General] test-timeout` 区分未设置（`General::test_target`；策略的 `test-url` / `test-timeout` 生效，`W0029` 对它们退役）；`rurge-policy` 的 `probe`（经策略自己的出站在一条连接上两次 HEAD，HTTPS 在已建立的 TLS 连接上测第二次，根证书取 `OutboundFactory::roots`）、`testbook`（`TestBook`：结果按策略与定义保存、同一策略同时只测一次、最多 8 个并发、`TestObserver`、一次性的 `test_once`）与 `auto`（`url-test` / `fallback` / `load-balance` 三种选法、`SelectCtx`、`AutoGroups`：临时覆盖、`url-test` 保持的成员、每组上一轮测试的时间与测试请求）；注册表按测试结果选成员（只有拨号触发测试、`evaluate-before-use` 的 `Resolution.pending`、嵌套组的分数、`round_timeout`）、`resolve_relay`（空组当中继一律拒绝）、订阅行不能动用主配置的 `client-cert` 与策略（`W0023`）；`rurge-engine` 的测试调度任务、测试会话进请求记录（规则 `policy test`）、`evaluate-before-use` 的有界等待（`policy group evaluation failed`）、重载时保留未变组的覆盖、`Engine::snapshot`（运行时与注册表在代际锁下成对读取）与 DNS 会话在链底为 REJECT 时的旁路；`rurge-api` 的 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 与 `POST /v1/policy_groups/select` 对自动组的临时覆盖；能力表翻转 `url-test` / `fallback` / `load-balance`（`W0008` 只剩 `smart` 与 `subnet`）。M3c（`smart`）已完成——`rurge-policy::smart`（`SmartBook`：按策略的时间加权首字节分数与失败罚分、健康 / 未知 / 失败三种状态、站点记忆、使用计数；`rank` 给出选中的成员与重试列表；`sample` 给大组的常规轮抽样）、注册表的 `smart` 成员过滤与 `policy-priority` 因子、`Resolution.smart`（`SmartPick`）、`resolve_member`、`test_round`，以及每代预存的测试查表（`TestBook::outcome`，M3b 延后事项 #14）；`rurge-inbound` 的 `SessionHandle` 两个时刻（出站就绪、首字节）、可挂多个的结束钩子与首字节钩子、`mark_upstream_failed`；`rurge-engine` 的 `smart` 模块（拨号时按重试列表换成员，单次时限"剩余时间 ÷ 剩余尝试次数"；质量回报：首字节、3 秒无响应、上游先断）、请求记录的 `connectMs` / `firstByteMs`、`automatic()` 纳入 `smart`、每次发布注册表时 `SmartBook::retain`；能力表翻转 `smart`（`W0008` 只剩 `subnet`）。M4（WireGuard / SSH / external）细化设计已写好，按 M4a SSH → M4b WireGuard → M4c external 三份计划推进，尚未开始实施。
```

换成

```markdown
阶段 1 功能已齐备（M1 ～ M4b）；阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`。M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。M3b（测速与自动组）已完成——`[General] test-timeout` 区分未设置（`General::test_target`；策略的 `test-url` / `test-timeout` 生效，`W0029` 对它们退役）；`rurge-policy` 的 `probe`（经策略自己的出站在一条连接上两次 HEAD，HTTPS 在已建立的 TLS 连接上测第二次，根证书取 `OutboundFactory::roots`）、`testbook`（`TestBook`：结果按策略与定义保存、同一策略同时只测一次、最多 8 个并发、`TestObserver`、一次性的 `test_once`）与 `auto`（`url-test` / `fallback` / `load-balance` 三种选法、`SelectCtx`、`AutoGroups`：临时覆盖、`url-test` 保持的成员、每组上一轮测试的时间与测试请求）；注册表按测试结果选成员（只有拨号触发测试、`evaluate-before-use` 的 `Resolution.pending`、嵌套组的分数、`round_timeout`）、`resolve_relay`（空组当中继一律拒绝）、订阅行不能动用主配置的 `client-cert` 与策略（`W0023`）；`rurge-engine` 的测试调度任务、测试会话进请求记录（规则 `policy test`）、`evaluate-before-use` 的有界等待（`policy group evaluation failed`）、重载时保留未变组的覆盖、`Engine::snapshot`（运行时与注册表在代际锁下成对读取）与 DNS 会话在链底为 REJECT 时的旁路；`rurge-api` 的 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 与 `POST /v1/policy_groups/select` 对自动组的临时覆盖；能力表翻转 `url-test` / `fallback` / `load-balance`（`W0008` 只剩 `smart` 与 `subnet`）。M3c（`smart`）已完成——`rurge-policy::smart`（`SmartBook`：按策略的时间加权首字节分数与失败罚分、健康 / 未知 / 失败三种状态、站点记忆、使用计数；`rank` 给出选中的成员与重试列表；`sample` 给大组的常规轮抽样）、注册表的 `smart` 成员过滤与 `policy-priority` 因子、`Resolution.smart`（`SmartPick`）、`resolve_member`、`test_round`，以及每代预存的测试查表（`TestBook::outcome`，M3b 延后事项 #14）；`rurge-inbound` 的 `SessionHandle` 两个时刻（出站就绪、首字节）、可挂多个的结束钩子与首字节钩子、`mark_upstream_failed`；`rurge-engine` 的 `smart` 模块（拨号时按重试列表换成员，单次时限"剩余时间 ÷ 剩余尝试次数"；质量回报：首字节、3 秒无响应、上游先断）、请求记录的 `connectMs` / `firstByteMs`、`automatic()` 纳入 `smart`、每次发布注册表时 `SmartBook::retain`；能力表翻转 `smart`（`W0008` 只剩 `subnet`）。M4（WireGuard / SSH / external）按三份计划推进（M4a SSH → M4b WireGuard → M4c external）：M4a（SSH）已完成——`rurge-config::spec::SshSpec`（`server-fingerprint` 解析为 `HostKeyPin`、`idle-timeout`、`private-key` 的 `E0020`）与 `ProtoSpec::keystore_item`（重载的指纹含 SSH 私钥）；新 crate `rurge-proto-ssh`（russh 0.63.3，`ring` 后端）：Keystore 私钥解码（Ed25519 / ECDSA / RSA；带口令与 DSA 为 `E0022`）、主机密钥比对、`SshOutbound`（每个策略一条会话、单飞握手、`direct-tcpip` 通道、会话断了重建一次、按通道计的空闲断开、30 秒 × 3 的保活、没配指纹的一次性告警、RSA 只用 SHA-2 签名）与 `testing::FakeSsh`；`rurge-engine` 的工厂分支；订阅行自己的 `private-key=` 进订阅安全门（`W0023`）；`tests/interop` 的临时 `sshd`（`RURGE_TEST_SSHD`，只在 Unix）；能力表翻转 `ssh`。M4b（WireGuard）、M4c（external）尚未开始。
```

`CLAUDE.md`——把

```markdown
- `docs/superpowers/specs/2026-09-27-phase2-m4-wireguard-ssh-external-design.md`：阶段 2 / M4 细化设计（WireGuard / SSH / external 出站），细化总设计的 M4 里程碑、不一致处以它为准。三份计划的拆分（M4a SSH → M4b WireGuard → M4c external）；已决事项 M4-D1 ～ D13（russh 0.63.3 + `ring` 后端、smoltcp 0.12.0 以保住 MSRV 1.89、boringtun 0.7.1 只用 sans-IO 的 `Tunn`、协议栈"共享锁 + waker"、Windows 用 Job Object 清理外部进程树（`rurge-platform` 第二个 unsafe 例外）、只做 TCP、`wireguard` 经 `underlying-proxy` 到 M5、订阅安全门等）；`[WireGuard <name>]` 类型化与新错误码 `E0023`；三种出站的语义、WireGuard 测速的原生模式、需登记的差异；第 15 节 V1–V14 是写各份计划时必须核对的事项，第 16 节是三份计划的任务草图。
```

换成

```markdown
- `docs/superpowers/specs/2026-09-27-phase2-m4-wireguard-ssh-external-design.md`：阶段 2 / M4 细化设计（WireGuard / SSH / external 出站），细化总设计的 M4 里程碑、不一致处以它为准。三份计划的拆分（M4a SSH → M4b WireGuard → M4c external）；已决事项 M4-D1 ～ D13（russh 0.63.3 + `ring` 后端、smoltcp 0.12.0 以保住 MSRV 1.89、boringtun 0.7.1 只用 sans-IO 的 `Tunn`、协议栈"共享锁 + waker"、Windows 用 Job Object 清理外部进程树（`rurge-platform` 第二个 unsafe 例外）、只做 TCP、`wireguard` 经 `underlying-proxy` 到 M5、订阅安全门等）；`[WireGuard <name>]` 类型化与新错误码 `E0023`；三种出站的语义、WireGuard 测速的原生模式、需登记的差异；第 15 节 V1–V14 是写各份计划时必须核对的事项，第 16 节是三份计划的任务草图，第 17 节是 M4a 计划期的订正。
- `docs/superpowers/plans/2026-09-27-phase2-m4a-ssh-plan.md`：阶段 2 / M4a（SSH）实施计划（7 个任务）。开头「计划期决定」表（P1–P19）记录核对 russh 源码与手册得出的结论和与设计文字不同的决定（russh 默认没有 `aes128-gcm` 要补上、去掉 `ssh-rsa` 主机密钥签名、RSA 私钥只用 SHA-2 签名、DSA 私钥能解码所以解码后按算法拒绝、重载的指纹要含 SSH 私钥、没配指纹的告警按出站对象计一次、空闲断开按通道计数、`sshd` 互操作只在 Unix 与 CI 上跑等）与「承接事项」；末尾「执行期修正记录」与「延后事项」两张表。
```

`CLAUDE.md`——把

```markdown
cargo test -p rurge-interop                     # 对 sing-box（全部协议）与 xray（只测 vmess）的互操作测试；没装就跳过（RURGE_TEST_SING_BOX / RURGE_TEST_XRAY / RURGE_INTEROP_REQUIRED=1）
```

换成

```markdown
cargo test -p rurge-proto-ssh                   # ssh：私钥解码、主机密钥比对、会话与通道、断线重建、空闲断开与保活，出站对回环假服务端（FakeSsh）
cargo test -p rurge-engine --test outbounds_ssh   # 经 ssh 出站的端到端用例：目标名交给服务器解析、Keystore 私钥与指纹、经 Shadow TLS、经 SSH 的测速
cargo test -p rurge-interop                     # 对 sing-box（全部协议）、xray（只测 vmess）与 OpenSSH sshd（只测 ssh，只在 Unix）的互操作测试；没装就跳过（RURGE_TEST_SING_BOX / RURGE_TEST_XRAY / RURGE_TEST_SSHD / RURGE_INTEROP_REQUIRED=1）
```

- [ ] **Step 8: 手工验收与两份设计文档**

`docs/acceptance/phase2-manual.md`——把

```markdown
- [ ] 用一份混有尚未实现协议的节点（如 `ss`）、总数超过 12 个的订阅建 `smart` 组（`policy-path=<订阅>`）：请求照常，会话的 `policy` 链里从不出现 `ss` 节点；全部能测的节点都有了结论之后（连不上的节点要连续三次测试失败才算），`GET /v1/requests/recent` 里规则为 `policy test` 的会话大约每 5 分钟一批，不会接连不断。

```

换成

```markdown
- [ ] 用一份混有尚未实现协议的节点（如 `ss`）、总数超过 12 个的订阅建 `smart` 组（`policy-path=<订阅>`）：请求照常，会话的 `policy` 链里从不出现 `ss` 节点；全部能测的节点都有了结论之后（连不上的节点要连续三次测试失败才算），`GET /v1/requests/recent` 里规则为 `policy test` 的会话大约每 5 分钟一批，不会接连不断。

## M4a　SSH

需要一台自己的 SSH 服务器（OpenSSH `sshd`，允许 TCP 转发），自动化测试（只用回环与假服务端）覆盖不了。

- [ ] 口令登录：`S = ssh, <服务器>, 22, username=<用户>, password=<口令>`，经 `S` 浏览几个网站正常；日志里有一条 `ssh: no server-fingerprint; the server's host key is not verified`（只带 `policy=S`），之后再用多少次都不再出现。
- [ ] 密钥登录：`ssh-keygen -t ed25519 -N "" -f key` 生成一把不带口令的密钥，`key.pub` 加进服务器的 `authorized_keys`，把 `key` 文件整个 Base64 编码后写进 `[Keystore]`（`key1 = type=openssh-private-key, base64=<…>`），`S = ssh, <服务器>, 22, username=<用户>, private-key=key1`：经 `S` 正常。换成 RSA 密钥（`ssh-keygen -t rsa -b 3072 -N ""`）同样正常，服务器日志（`LogLevel VERBOSE`；`journalctl -u ssh` 或 `/var/log/auth.log`）里记下的签名算法是 `rsa-sha2-512` 或 `rsa-sha2-256`。
- [ ] 带口令的私钥（`ssh-keygen -t ed25519 -N pw`）：`rurge check` 报 `E0022`，文本是 ``keystore item `key1` is protected by a passphrase, which rurge cannot use; remove the passphrase``，输出里没有私钥内容。
- [ ] 主机密钥校验：`ssh-keyscan -t ed25519 <服务器>` 的输出去掉开头的主机名，写成 `server-fingerprint="ssh-ed25519 AAAA…"`：连接正常，也没有上面那条告警；换成另一台机器的公钥后 `rurge reload`，经 `S` 的会话失败，请求记录的错误是 `ssh: the server's host key is not one of server-fingerprint`。
- [ ] 会话复用：同时开几个经 `S` 的下载，服务器上（`ss -tnp | grep sshd` 或 `last`）只看到 rurge 的一次登录、一条连接。
- [ ] 空闲断开：`idle-timeout=30`，最后一个经 `S` 的连接关掉 30 秒后，服务器上那条连接消失，再访问时重新登录；一个开着但没有流量的连接（如网页上的 WebSocket）不会因为 `idle-timeout` 被断开。
- [ ] 断线重建：让服务器断开 rurge 的会话（在服务器上结束那次登录对应的 `sshd` 进程，或重启服务器）之后，下一个经 `S` 的连接正常（重新登录）；断网两分钟再恢复之后同样正常（旧会话在 3 次保活无回应后判定已断）。
- [ ] 口令写错：会话失败，错误是 `ssh: authentication failed`；日志与 `GET /v1/requests/recent` 里没有用户名与口令。
- [ ] 日志（含 `--log-level verbose`）里搜不到口令与私钥内容。

```

`docs/superpowers/specs/2026-09-27-phase2-m4-wireguard-ssh-external-design.md`——把

```markdown
5. 能力表翻转 `external` 与文档（含 `CLAUDE.md` 的 unsafe 规则）。

```

换成

```markdown
5. 能力表翻转 `external` 与文档（含 `CLAUDE.md` 的 unsafe 规则）。

## 17. 计划期的订正

写 M4a 计划（`docs/superpowers/plans/2026-09-27-phase2-m4a-ssh-plan.md`）时核对 russh 0.63.3 与本仓库源码得出、与上文不同的地方；P 编号是该计划「计划期决定」表的编号。

| 本文原文 | 计划 | 依据 |
| -------- | ---- | ---- |
| 4.5 带口令私钥的报错示例 `key1 is encrypted; rurge cannot use a passphrase-protected key` | ``keystore item `key1` is protected by a passphrase, which rurge cannot use; remove the passphrase``（P7） | 与既有 Keystore 报错同样以 "keystore item `名字`" 开头，并说明怎么办 |
| 4.5 DSA 私钥不支持（russh 标为不安全） | 不开 `dsa` 特性时 DSA 私钥照样能解码（只是签不了名）：解码后按算法只收 Ed25519 / ECDSA / RSA，其余（DSA、要硬件的 `sk-*` 安全密钥）同为 `E0022`（P7） | russh / ssh-key 源码 |
| 4.2 / V13 `private-key` 的 `E0020` | M1 的 Keystore 引用校验只针对 `client-cert`；`read_ssh` 自己查（条目不存在、条目是 p12）（P9） | `spec/tls.rs` |
| — | M2b 重载指纹里"引用的 Keystore 条目"只取 TLS 的 `client-cert`；新增 `ProtoSpec::keystore_item`，`ssh` 取 `private-key`（P10） | 否则换了私钥（名字不变）的重载沿用旧出站、仍用旧私钥 |
| 5.1 会话已断的判断"开通道失败且会话已关闭" | 开通道时除"服务器拒绝"（`ChannelOpenFailure`）以外的错误都当作会话已断（P5） | russh 在会话已关时给的是发送失败一类的错误，分不出更细 |
| 5.2 告警文本 ``ssh: policy `P` has no server-fingerprint; …``，"每个策略每个进程只告警一次" | 结构化字段 `policy` 加固定消息 `ssh: no server-fingerprint; the server's host key is not verified`；按出站对象计一次：重载时参数没变的策略沿用原出站、不再告警，参数变了而重建的出站再告警一次（P12） | 与 `registry.rs` 里 `policy cannot be built` 的写法一致；按进程记要另设一张全局表 |
| 5.3 RSA 按 `server-sig-algs` 选 rsa-sha2-256 / 512 | 服务器列了 rsa-sha2-512 用它，否则一律 rsa-sha2-256——服务器只列 `ssh-rsa` 或什么都没说时也是（P3） | russh 的 `best_supported_rsa_hash` 在服务器只列 `ssh-rsa` 时给出 SHA-1 |
| 5.5 "用 russh 的默认列表（覆盖 … `aes128-gcm@openssh.com`）" | russh 0.63.3 的默认加密列表没有 `aes128-gcm@openssh.com`，补在 `aes256-gcm` 之后；主机密钥算法去掉 `ssh-rsa`；kex 与 MAC 的默认列表本来就没有 SHA-1（P2） | `negotiation.rs` |
| 5.6 ``ssh: the handshake failed (<阶段>)`` | 只有"没有共同算法"带括号说明；其余握手失败（对端不是 SSH、握手中的 I/O 错误等）都是 `ssh: the handshake failed`；会话已断而重建后仍开不了通道时是 `ssh: the session closed`（P14） | russh 的错误分不出握手的阶段 |
| §10 SSH 第 2 层"空闲断开（暂停的时钟）" | 空闲断开用真实时钟（`idle-timeout` 取 1 秒、有界等待）；保活只断言 `session_config()` 的取值（P16） | 回环上的真实连接与暂停的时钟不能共存：运行时空闲时自动拨快时钟，russh 自己的计时随之提前到期 |
| §10 SSH 第 3 层"本机有 `sshd` 时起临时实例" | 只在 Unix 上编译与运行；非 root 的 `sshd` 只让运行它的用户登录，用例用现场生成的 Ed25519 密钥登录（P15） | Windows 上没有可用的 `sshd`；CI 在 Linux / macOS 上跑 |

```

`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`——把

```markdown
| SSH | `russh` 客户端，动态转发用 direct-tcpip 通道；OpenSSH 私钥解析用 `ssh-key` | M4 | 是否覆盖 `curve25519-sha256` + `aes128-gcm`（清单 4.6 的下限） |
```

换成

```markdown
| SSH | `russh` 客户端，动态转发用 direct-tcpip 通道；OpenSSH 私钥解析用 `ssh-key` | M4 | 已验证（M4a）：russh 0.63.3（`ring` 后端）两者都支持，`aes128-gcm@openssh.com` 不在它的默认列表里，由 rurge 显式加上（见 Q5） |
```

`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`——把

```markdown
| Q5 | `russh` 的算法覆盖 | M4 细化设计时验证 |
```

换成

```markdown
| Q5 | `russh` 的算法覆盖 | 已决（2026-09-27，M4-D2 与 M4 设计第 17 节）：russh 0.63.3 支持 `curve25519-sha256` 与 `aes128-gcm@openssh.com`；后者不在默认列表里，rurge 显式加上，并去掉 SHA-1 的 `ssh-rsa` 主机密钥签名 |
```

- [ ] **Step 9: 本计划末尾的两张表**

把控制者给出的行写进本计划末尾的「执行期修正记录」与「延后事项」两张表（没有要补的行时保持原样）。

- [ ] **Step 10: 门禁与提交（文档）**

跑门禁（只改了文档：45 个测试二进制，1029 通过 / 1 忽略）。

```bash
git add docs README.md README_en.md CLAUDE.md
git commit -m "docs: M4a SSH——兼容性清单、README、CLAUDE.md、手工验收、M4 设计第 17 节与总设计 Q5"
```

---

## 验收对照（设计第 11 节，SSH 部分）

| # | 验收项 | 由谁保证 |
| - | ------ | -------- |
| 1 | SSH 经 `FakeSsh` 动态转发（口令与三种密钥），主机密钥校验按 5.2；经引擎的端到端通过 | Task 3：`password_and_key_logins_forward_a_connection`（口令、Ed25519、ECDSA、RSA）、`a_host_key_outside_server_fingerprint_is_refused`；Task 2：`pins::tests`；Task 4：`a_policy_without_server_fingerprint_is_warned_about_once`；Task 5：`outbounds_ssh` 四条 |
| 2 | WireGuard 与带 `client-id` 的端点 | 不在本计划（M4b） |
| 3 | `external` 的再拉起与进程树清理 | 不在本计划（M4c） |
| 4 | `W0007` 不再因 `ssh` 出现；订阅安全门有用例 | Task 7：`check_knows_ssh`；Task 1：`a_subscription_ssh_line_may_not_use_the_profiles_private_keys`（`section-name=` 与 `external` 在 M4b / M4c） |
| 5 | 门禁全绿（fmt / clippy 零警告 / `cargo test --workspace`） | 各任务的门禁 |
| 6 | 需要真实环境的项目进手工验收清单 | Task 7：`docs/acceptance/phase2-manual.md` 的 M4a 一节（真实 `sshd`、RSA 与 Ed25519 密钥、指纹），由项目所有者用自己的服务器验收 |
| — | 第 10 节第 3 层：对 OpenSSH `sshd` 的互操作 | Task 6，由首次推送后的 CI 证明（本机没有 `sshd`） |

## 执行期修正记录

| 任务 | 计划原文 | 实际做法 | 原因 | 提交 |
| ---- | -------- | -------- | ---- | ---- |
| 3 | Step 5 的提交命令 `git add crates/rurge-proto-ssh` | 连同 `Cargo.lock` 一起提交 | 给 `rurge-proto-ssh` 加上 `rustls` 依赖后，cargo 在锁文件里该 crate 的条目下多写一行 `"rustls",`，计划的提交命令漏了它 | ffbafea |
| 3 | 会话槽的单飞只合并成功的握手（`session()` 里 `establish` 失败时直接返回） | 握手失败时，等在锁上的拨号共享这次失败（同一种错误、同一句固定文本），不各自重新登录；失败之后才来的拨号照常再试；新增 `FakeSsh::attempts()` 与用例 `concurrent_dials_share_one_failed_attempt_and_a_later_dial_retries` | 任务评审：口令写错时，浏览器一次开几十个连接会变成连续几十次失败登录，正好触发服务器的 fail2ban；设计 5.1"同时进来的拨号共用这一次握手"本就包括失败的结果 | d95a6e0 |
| 4 | 导入块，与 `let session = Arc::new(self.establish(opts).await?);` 那一块 | 改写成适配 Task 3 修正后的代码：导入列表并入 `AtomicU64` 与 `Mutex as StdMutex`；`watch_idle` 在 `session()` 的 `Ok` 分支里、`let session = Arc::new(session);` 之后启动 | Task 3 的修正改掉了这两处锚点 | db81786 |
| 5 ～ 7 | 各任务门禁的预期数字 | 实际：Task 1 41 个二进制 / 1001 通过；Task 2 43 / 1006；Task 3 43 / 1016（修正前 1015）；Task 4 43 / 1020；Task 5 44 / 1028；Task 6 45 / 1029；Task 7 45 / 1030（均 1 忽略） | Task 3 的修正多了一条用例 | — |
| 7 | 兼容性清单 `[Keystore]` 行、4.2 节 `ssh` 行与手工验收"口令写错"一项的原文 | 各多一句：解码器也收未加密的 PuTTY / PKCS#1 / PKCS#8 / SEC1 私钥文件；握手失败时等着的连接一起得到这次失败；验收时看服务器日志里每批只有一次失败登录 | Task 2 评审发现解码器按内容识别格式；Task 3 的修正 | 本任务的文档提交 |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | WireGuard 的 `section-name=` 进订阅安全门（M3b #10 的另一半） | M4b |
| 2 | 没配 `server-fingerprint` 的告警按出站对象计一次：重载时参数变了而重建的出站会再告警一次（P12） | 有用户报告再说 |
| 3 | 只认 `ssh-rsa`（SHA-1）签名的老服务器用 RSA 私钥登录不了（P3）；DSA 私钥与带口令的私钥不支持（P7） | 有用户报告再说（登记为差异） |
| 4 | 对 OpenSSH `sshd` 的互操作只在 CI（Linux / macOS）上真正运行，本机验证不了（P15） | 首次推送后看 CI |
