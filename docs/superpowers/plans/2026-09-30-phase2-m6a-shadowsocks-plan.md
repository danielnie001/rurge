# 阶段 2 / M6a「Shadowsocks」Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `ss` 出站：AEAD（`aes-128-gcm` `aes-192-gcm` `aes-256-gcm` `chacha20-ietf-poly1305` `xchacha20-ietf-poly1305`）、`none` 与 SS 2022（`2022-blake3-aes-128-gcm` / `-256-gcm`，含 `serverKey:userKey` 多用户）的 TCP 与 UDP，simple-obfs 的 `http` / `tls` 两种伪装，`udp-relay` / `udp-port`；流式旧方法只解析（`W0007` + REJECT）。

**Architecture:** `rurge-config` 新增 `spec::ss`（`SsSpec`）与供 Snell 复用的 `spec::obfs`（`ObfsOpts`），并把"未实现"的说明从 vmess 专用的标志推广成 `NotImplemented`。`rurge-proto` 新增传输层 `transport::obfs`（`Stack` 的一层：connect → shadow-tls → obfs → tls → ws）与协议模块 `shadowsocks`（`cipher` / `kdf` / `aead` 的分块流、`s2022` 的请求与应答头和身份头、`udp` 的按包载体），各有独立实现的回环假服务端。引擎的工厂装上 `ShadowsocksOutbound`，能力表翻转 `ss`。

**Tech Stack:** Rust 1.89 / edition 2024；新增依赖只有 `blake3` 1.8（SS 2022）；加密原语用依赖树里已有的 RustCrypto 版本（`aes-gcm` 0.11.1、`chacha20poly1305` 0.10.1、`hkdf` 0.13 + `sha1` 0.11、`md-5` 0.10、`aes` 0.8）。

**Spec:** `docs/superpowers/specs/2026-09-30-phase2-m6-ss-snell-h2-design.md`（M6-D1 ～ D8；第 3 节；第 6、7 节中 M6a 的部分；第 9 节 V1 ～ V3、V10；第 10 节 M6a 草图）与总设计 `docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`。与本计划「计划期决定」表不一致处，以该表为准；执行开始时一并写进设计文档新增的第 12 节。

## Global Constraints

- MSRV **1.89**，edition 2024。`unsafe_code`：全工作区 `forbid`；只有 `rurge-platform` 的两个函数例外（本计划不碰）。**本计划不新增任何 unsafe**。
- 依赖方向不变：`rurge-proto → rurge-net → rurge-config`；`rurge-engine → { rurge-inbound → rurge-proto, rurge-policy → rurge-proto, rurge-dns }`。**新增依赖只有 `blake3`**（Task 4）；不给依赖树里已有的 crate 再引入第二个大版本。
- **测试绝不碰公网**：只用回环 + 端口 0 + 有界等待；不用固定 sleep 当同步手段（轮询 + 截止时间；只有"断言这段时间里什么也没发生"时才等一段固定时间）。**任何带 `url-test` / `fallback` / `load-balance` / `smart` 组的测试配置，`proxy-test-url` 与 `internet-test-url` 都必须指向回环**——引擎用例的 `Profile::text` 已默认指向 `http://127.0.0.1:9/`，不要删掉。
- **测试与任何命令都不得修改本机的系统代理、注册表、网络设置，不得注册真实服务 / 计划任务**：不在 CLI 测试夹具之外运行 `rurge run --system-proxy`；不运行不带 `--dry-run` 的 `rurge service install | uninstall`。
- **不在本机下载或安装任何东西**（不装 shadowsocks-rust、sing-box，不 `rustup target add`、不 `cargo install`）。互操作用例在本机没有二进制时按既有约定跳过。`blake3` 由 cargo 按 `Cargo.lock` 取得。
- **口令、密钥与载荷永不外泄**：`password`、SS 2022 的各段密钥、派生出的子密钥与 salt、UDP 与 TCP 的载荷不进日志、错误文本与 `Debug`（配置里用 `Secret<T>`，出站与密钥对象不实现 `Debug`）。错误文本可以说"差了多少秒"，不能带时间戳、salt、密钥的原始字节。
- rurge 专有的运行时选项只经命令行与环境变量提供，不扩展 Surge 配置格式（FR-CFG-17）。与 Surge 的每一处行为差异都登记进 `docs/surge-compatibility-matrix.md`（Task 7）。
- 日志、错误文本、CLI 输出、代码注释用英文；文档与提交标题用中文。注释密度、命名、惯用法与相邻代码保持一致；注释里不写评审轮次的标签。GPL 参考实现（simple-obfs、Snell 的第三方实现）只取协议事实，不抄代码（M6-D3）。
- 提交信息结尾两行：`Co-Authored-By: <执行者自己的署名>` 与 `Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th`。**不 push、不 merge**，不 amend。
- 每个任务结束的门禁（全部通过才算完成）：

  ```bash
  RUSTFMT="C:\Users\SZV01065\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe" cargo fmt --all --check \
    && cargo clippy --all-targets -- -D warnings \
    && timeout 1500 cargo test --workspace --no-fail-fast
  ```

  `timeout` 不能省：`rurge-dns` 的一个用例曾让测试进程以 100% CPU 空转数小时（M3a「延后事项」#20）。测试二进制异常退出而没有失败用例时（`STATUS_ACCESS_VIOLATION`、`STATUS_HEAP_CORRUPTION` / `0xc0000374`、段错误——本机已知的既有问题，M3b 计划 P21），或整轮被 `timeout` 杀掉时，重跑一次并保留两次的日志，**不要在任务里去修它**。已知偶发失败的用例（`rurge-dns` 的 `a_partial_result_completes_aaaa_in_the_background` 与 `bootstrap::tests::stale_entries_are_served_and_refreshed_once`、`rurge` 的 `run::watch_reloads_rules_on_change` 与 `run::run_system_proxy_is_applied_switched_and_restored`、`rurge-engine` 的 `udp::a_closed_port_does_not_break_the_carrier`）同样重跑。**编译器（`rustc` / 链接器）自己崩溃、报 PDB 损坏或 "no space on device" 时多半是磁盘满了**：先看 `df -h /d`，删 `target/debug/incremental` 再重跑（Task 4 加入 `blake3` 后所有依赖 `rurge-proto` 的测试程序都会重新链接，占用会明显上升）。
- 第三方 crate 的 API 以本机源码为准：`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`。
- 本机的 bash 处理不了超过约 8 KB 或含反斜杠的 heredoc（`\\` 会被改写）：新文件与含反斜杠的改动一律用写文件的工具落盘，不用 heredoc。

## Review Focus

设计没有逐条写到、而最可能伤到使用者的五类输入或失败方式；每一条都在负责它的任务里配了用例。

1. **口令或方法写错**（最常见的配置错误）：连接期没有鉴权应答，服务端通常一声不吭地关掉——要以清楚的文字失败（`ss: the server closed the connection without answering` / `ss: the server's data failed to decrypt (wrong password or method?)`），不能挂到超时，也不能泄露口令。用例：Task 3 `a_wrong_password_is_a_connection_closed_without_an_answer`、`what_the_server_gets_wrong_is_an_error_that_quotes_nothing`；Task 4 `a_key_the_server_does_not_know_is_a_connection_closed_without_an_answer`。
2. **本机时钟偏差**（SS 2022 要求 ±30 秒）：服务端拒绝时要能看出原因；服务端时钟不对时报出差了多少秒。用例：Task 4 `a_client_clock_an_hour_off_is_refused_without_an_answer`、`an_answer_off_the_clock_or_for_another_request_is_an_error`；Task 6 `a_server_clock_an_hour_off_fails_the_session_with_the_reason`。
3. **SS 2022 的密钥写错**（Base64 不对、长度与方法不符、多用户的冒号写法）：加载时就是 `E0018`，文字指出第几段，绝不引用密钥本身。用例：Task 1 `a_bad_ss_2022_key_is_an_error_that_never_quotes_the_password`；Task 6 `check_knows_ss`。
4. **服务端先说话的协议与大块数据**（客户端不写、或一次写入超过一块）：SS 2022 没有首段负载时要按规范加填充，大数据按块上限切分。用例：Task 4 `a_silent_client_pads_its_request_and_hears_the_target_first`、`ss_2022_sends_names_and_crosses_chunks_of_0xffff`；Task 3 `a_large_payload_crosses_many_chunks_both_ways`。
5. **UDP 的回包被篡改、重放或来自别人**（游戏与语音经 ss）：坏包与重放包要静默丢弃、载体继续收；全锥下陌生来源的回包原样送回。用例：Task 5 `a_garbled_or_replayed_answer_is_dropped_and_the_next_one_arrives`、`udp_is_full_cone`；Task 6 `anyone_may_answer_through_ss`。

## 计划期决定

写计划时对照设计、协议规范（shadowsocks.org 的 AEAD 文档与 SIP022 / SIP023，2026-09-30 查阅）、参考实现（shadowsocks-rust v1.25.0，MIT；simple-obfs，GPL——只取事实）与本仓库源码核对后定下的事；与设计文档文字不同的，写进设计文档第 12 节。

**本计划里的代码不是凭空写的。** 全部 7 个任务的改动在仓库的一份副本上按任务顺序真实做了一遍（副本用自己的构建目录，不与本仓库的 `target/` 混用），最后一次全工作区门禁见 Task 7 的 Step 5。计划里新文件的全文取自副本上该任务的提交，修改处的"把 … 换成 …"由脚本从相邻两个任务提交的差异生成，并在拼好之后按计划的顺序套到开工前的源码上逐字核对过——计划文本与验证过的代码一字不差。每个任务 Step 2 的"预期失败"是只把该任务的用例块（及写明的前置改动）套到上一个任务的状态上、真实跑出来的。加密的已知答案由 Python 独立算出（`hashlib` / `hmac`、`cryptography` 45 的 AES-GCM / ChaCha20-Poly1305 / AES-ECB、按规范自写并以规范自带哈希校验过的 BLAKE3 与 HChaCha20；HKDF-SHA1 另对照 RFC 5869 A.4），Rust 代码第一次运行就与全部向量一致。

| # | 事项 | 决定与依据 |
| - | ---- | ---------- |
| P1 | V10：加密 crate | 全用 RustCrypto：AES-GCM 三种长度用 `aes-gcm` 0.11.1（aead 0.6；AES-192 为 `AesGcm<Aes192, U12>`）；ChaCha20 / XChaCha20-Poly1305 用依赖树里已有的 `chacha20poly1305` **0.10.1**（aead 0.5），不用 0.11——后者会给同一个 crate 引入第二个版本；代价是 `AeadCipher` 枚举内部混用两代 `aead` 接口（枚举把差异藏在里面）。HKDF 用 `hkdf` 0.13 + `sha1` 0.11，MD5 用已有的 `md-5` 0.10，AES 单块用 VMess 已直接依赖的 `aes` 0.8。`blake3` 1.8 是唯一的新依赖（Task 4；锁文件随之多了 `arrayvec`、`constant_time_eq`） |
| P2 | "未实现"的说明 | vmess 专用的 `SpecOutcome.legacy_vmess` 推广为 `SpecOutcome.not_implemented: Option<NotImplemented>`（`LegacyVmess` / `SsStreamCipher(&'static str)`，各带加载告警、会话日志与订阅导入的文字）；注册表看不到 `SpecOutcome`，原因经 `Config.not_implemented`（主配置）与 `Imported.not_implemented`（订阅）送到 `Entry::Unsupported`。流式方法的 `W0007` 每次加载每种方法一条；会话日志 `policy protocol not implemented: ss (<method>)`；vmess 的文字不变 |
| P3 | 分步落地的门 | Task 1 里 `to_spec` 已完整读取并检查 `ss` 行，但暂不产出 spec（`let built = policy.kind != PolicyKind::Shadowsocks;`），引擎工厂的 `ProtoSpec::Ss` 分支暂时返回 `BuildError`：否则注册表会去构建 `ss`、干构建把每条 `ss` 行变成加载错误、约 20 个以 `ss` 为"未实现协议"例子的用例一起变。Task 6 去掉这道门、装上真正的出站，并把那些用例的例子换成 M7 之前都不会实现的 `hysteria2` |
| P4 | `password` 与 SS 2022 的密钥 | `password` 只接受命名写法，除 `none` 外必填（`none` 写了也照 shadowsocks-rust 忽略）。2022 方法按冒号拆段、逐段标准 Base64 解码（有无填充都行），长度必须等于方法的密钥长度，解出的密钥另存进 `SsSpec.keys: Secret<Vec<Vec<u8>>>`（身份密钥在前、用户密钥在最后）；错误 `` key #N of `password` is not a Base64 key of L bytes, as `<method>` requires ``，不引用取值。`encrypt-method` 不区分大小写；`2022-blake3-chacha20-poly1305` 不在手册的方法表里，不接受 |
| P5 | 流式方法的名单 | libev 的 20 个名字（`rc4` `rc4-md5` `aes-128/192/256-cfb` `aes-128/192/256-ctr` `bf-cfb` `camellia-128/192/256-cfb` `cast5-cfb` `des-cfb` `idea-cfb` `rc2-cfb` `seed-cfb` `salsa20` `chacha20` `chacha20-ietf`）：合法、只解析，`W0007` + REJECT（M8） |
| P6 | V1 / V3：obfs 的缺省与伪装细节 | 手册没写 `obfs-host` / `obfs-uri` 的缺省：`obfs-host` 缺省取服务器主机名（不用参考实现的 `cloudfront.net`），`obfs-uri` 缺省 `/`。`http` 的 `Host` 在服务器端口不是 80 时带 `:port`（写了 `obfs-host` 也带，同参考客户端）；IPv6 服务器写成 `[v6]`。`obfs-uri` 与 `tls` 同写是 `W0028`；没有 `obfs` 时写的 `obfs-host` / `obfs-uri` 是 `W0028`。伪装的请求头与 ClientHello 模板照 simple-obfs 的协议事实自写（`http`：`GET`、`curl/7.x.y` 的 User-Agent、`Upgrade: websocket`、随机 `Sec-WebSocket-Key`、`Content-Length`；`tls`：217 + 负载 + 主机名字节的 ClientHello，首段负载在 session_ticket 扩展里） |
| P7 | obfs 的时机与读取 | 伪装头随第一次**非空**写出去；空写、写之前的 flush / shutdown 什么也不发（上面的协议总是先写自己的请求头）。首包最多带 16384 字节（`tls` 为 16384 − 217 − 主机名长度），其余随下一次写。读 `http` 时累积最多 8 KiB 的响应头，只认 `101` 或 2xx；读 `tls` 时逐个解析记录头（不做固定字节数的跳过），此后 0x17 记录的正文原样流过。服务端一个字节没发就关闭是普通 EOF（好让协议层报"没有应答"） |
| P8 | `LazyHead` 的位置 | `LazyHead` 在加密流之上（vmess 在下面）：地址必须加密在第一块里，于是请求头、首段负载与 salt 一次写出——走 obfs 时也就是同一个首包 |
| P9 | AEAD 的错误 | 服务端的 salt 一个字节没到就 EOF：`ss: the server closed the connection without answering`；任何标签校验失败：`ss: the server's data failed to decrypt (wrong password or method?)`；块中途断开：`ss: the connection ended in the middle of a chunk`；块长度超过上限：`ss: the server sent a chunk longer than the protocol allows`。都在转发时以读错误出现（同 vmess）。服务端发来的零长度块跳过（AEAD 没有结束标记） |
| P10 | V2：SS 2022 的请求 | 同一个 `AeadStream` 加 `new_2022`（不另写一种流）：首次写入 = salt ‖ 身份头 ‖ 固定长度头块 ‖ 变长头块（地址、填充、首段负载），随后是普通块（上限 0xFFFF）。有首段负载时不填充，没有时填充 1..=900 字节——**填充内容是 0**（它是加密的，规范只要求长度；设计写的"随机填充"只对长度成立）。首次写入最多带 0xFFFF − 2 字节的地址加负载 |
| P11 | SS 2022 的应答与时钟 | 在应答的固定长度头块处就校验：类型不是 1 或回显的请求 salt 不是自己的 → `ss: the server's answer is not for this request`；时间差超过 30 秒 → `ss: the server's clock differs from ours by N seconds (at most 30 are allowed)`（正好 30 秒接受）。时钟以 `now: fn() -> u64` 注入，测试专用 `with_clock` |
| P12 | V2：SS 2022 的 UDP | 包号**从 0 开始**（SIP022 原文；shadowsocks-rust 发的第一个是 1，两者都过对方的窗口）。防重放窗口照 wireguard-go 的过滤器（8128），只在包解密且类型、时间戳、回显的客户端 session id 都对上之后才前移；每个载体最多记 8 个服务端 session，满了挤掉最旧的。回包的分离头用**用户密钥**解（回包没有身份头）。回显的客户端 session id 要校验（shadowsocks-rust 似乎不查）。用不了的包静默丢弃并记一条不带载荷的 `debug!`，载体继续收；来源不是服务器地址的数据报在解密前就忽略（复用 socks5 中继的 `from_relay`） |
| P13 | UDP 的路径 | 必须写 `udp-relay=true`（否则 `Unsupported`，文字同 socks5 / external）；发往 `udp-port`（缺省主端口）；服务器地址在打开载体时解析一次。UDP 不经 Shadow TLS 与 obfs（它们是 TCP 层），经策略的连接器走 DIRECT 或 `underlying-proxy` 的链 |
| P14 | 重载 | 不需要改代码：注册表的指纹就是 `PolicySpec` 的相等比较，`SsSpec` 按值比较方法、口令、密钥、`udp-relay`、`udp-port` 与 obfs（Task 6 的引擎用例覆盖） |
| P15 | 互操作 | shadowsocks-rust v1.25.0 的 `ssserver`（三平台预编译，CI 按 SHA-256 安装，`RURGE_TEST_SSSERVER`）覆盖 `aes-128-gcm` `aes-256-gcm` `chacha20-ietf-poly1305` 与两种 2022（含多用户）；它的发布版不带 `aes-192-gcm` 与 `xchacha20-ietf-poly1305`（在 `aead-cipher-extra` 特性里），这两种改由 sing-box 的 `shadowsocks` 入站覆盖。obfs 没有可用的预编译参考服务端（simple-obfs 已停止维护，sing-box 不带插件），只有回环假服务端与手工验收。本机没有二进制，互操作由 CI 首跑证明 |
| P16 | 任务的切分 | 与设计第 10 节草图相同的 7 个任务 |

## 承接事项

| # | 来源 | 事项 | 处理 | 任务 |
| - | ---- | ---- | ---- | ---- |
| C1 | M6 设计第 10 节 | M6a 的全部内容 | 本计划 | 1–7 |
| C2 | M5a 延后事项（`udp-relay` 行） | `udp-relay` 对 Shadowsocks 生效 | 本计划（P13） | 5–7 |
| C3 | 总设计 Q7 | Snell 可依据的公开资料 | M6 设计第 4.1 节已关闭；总设计随 Task 7 同步 | 7 |
| C4 | M3b #7（P21） | 测试二进制偶发崩溃 | 照旧：门禁遇到就重跑 | — |

## File Structure

新建：

| 文件 | 职责 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-config/src/spec/ss.rs` | `SsMethod`、`SsSpec`、`read_ss`（与用例） | 1 |
| `crates/rurge-config/src/spec/obfs.rs` | `ObfsMode`、`ObfsOpts`、`read_obfs`（与 Snell 共用，与用例） | 1 |
| `crates/rurge-proto/src/transport/obfs/{mod.rs, http.rs, tls.rs}` | simple-obfs 客户端：`ObfsClient` 与字节流包装、`http` 请求头、`tls` 的 ClientHello 与记录 | 2 |
| `crates/rurge-proto/src/testing/obfs.rs` | 假服务端一侧的 obfs：`accept_obfs`、`ObfsHello` | 2 |
| `crates/rurge-proto/src/shadowsocks/{mod.rs, cipher.rs, kdf.rs, aead.rs}` | `ShadowsocksOutbound`、AEAD 原语与计数 nonce、口令与子密钥派生、分块流 | 3 |
| `crates/rurge-proto/src/testing/shadowsocks.rs` | `FakeShadowsocks`（AEAD / `none` → 2022 → UDP 逐任务加） | 3–5 |
| `crates/rurge-proto/src/shadowsocks/s2022.rs` | SS 2022 的请求 / 应答头、身份头、填充与时钟 | 4 |
| `crates/rurge-proto/src/shadowsocks/udp.rs` | UDP 载体 `SsUdp`：AEAD、`none`、2022（分离头、防重放窗口） | 5 |
| `crates/rurge-engine/tests/outbounds_shadowsocks.rs` | 经引擎的端到端用例 | 6 |
| `tests/interop/src/shadowsocks_rust.rs`、`tests/interop/tests/shadowsocks.rs` | shadowsocks-rust 夹具与互操作用例 | 7 |

修改：

| 文件 | 改动 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-config/src/{spec/mod.rs, config.rs, redact.rs}`、`tests/corpus/valid/kitchen-sink.conf` | `ProtoSpec::Ss`、`NotImplemented`、`Config.not_implemented`、脱敏名单加 `obfs-host`、语料库里的 2022 行 | 1、6 |
| `crates/rurge-policy/src/{assemble.rs, registry.rs}` | 订阅与注册表的"未实现"说明 | 1、6（用例） |
| `crates/rurge-proto/src/{lib.rs, transport/mod.rs, transport/stack.rs, testing/mod.rs, socks5.rs}`、`Cargo.toml`、`crates/rurge-proto/Cargo.toml` | 模块、`Stack::with_obfs`、依赖、`from_relay` 可见性 | 2–5 |
| `crates/rurge-engine/src/outbounds.rs`、`crates/rurge/src/capabilities.rs` | 工厂分支、能力表 | 1、6 |
| 以 `ss` 为"未实现协议"例子的用例（`rurge-api` / `rurge-engine` / `rurge-policy` / `rurge-config` / `rurge` 的测试） | 换成 `hysteria2` | 6 |
| `tests/interop/{src/lib.rs, README.md}`、`.github/workflows/ci.yml` | sing-box 的 `shadowsocks` 入站、shadowsocks-rust 的安装 | 7 |
| 文档（兼容性清单、手工验收、两份 README、`CLAUDE.md`、总设计） | 见 Task 7 | 7 |

## 任务一览

| 任务 | 交付物 | 依赖 |
| ---- | ------ | ---- |
| 1 | `SsSpec` / `ObfsOpts`、`NotImplemented`、流式方法的 `W0007` + REJECT、`obfs-host` 脱敏 | — |
| 2 | obfs 层（`http` / `tls`）与 `Stack` 的一层、假服务端一侧 | 1（`ObfsOpts`） |
| 3 | AEAD 与 `none` 的 TCP、`FakeShadowsocks` | 1、2 |
| 4 | SS 2022 的 TCP（含身份头） | 3 |
| 5 | UDP（AEAD、`none`、2022）与 `udp-port` | 3、4 |
| 6 | 引擎装配、能力表翻转、经引擎的用例 | 1–5 |
| 7 | 互操作（shadowsocks-rust、sing-box）与文档 | 1–6 |

---

### Task 1: `ss` 的配置与"未实现"说明

`SsSpec`（P4）与供 Snell 复用的 `ObfsOpts`（P6），流式方法只解析（P5），vmess 专用的"旧握手"标志推广成 `NotImplemented`（P2）；`obfs-host` 进 `GET /v1/profiles/current` 的脱敏名单。`ss` 行此时已被完整读取与检查，但暂不产出 spec（P3），引擎工厂先放一个到不了的 `BuildError` 分支。

**Files:**
- Create: `crates/rurge-config/src/spec/ss.rs`、`crates/rurge-config/src/spec/obfs.rs`（均自带用例）
- Modify: `crates/rurge-config/src/spec/mod.rs`、`src/config.rs`、`src/redact.rs`（与用例）、`tests/corpus/valid/kitchen-sink.conf`、`crates/rurge-policy/src/assemble.rs`、`src/registry.rs`（与用例）、`crates/rurge-engine/src/outbounds.rs`

**Interfaces:**
- Consumes: 既有的 `ParamReader`（`str` / `bool` / `choice` / `error` / `warn`）、`Secret<T>`、`read_common`、`read_shadow_tls`、`tls::refuse_tls`、诊断码 `E0018` / `W0007` / `W0028`。
- Produces:
  - `rurge_config::spec::obfs`：`pub enum ObfsMode { Http, Tls }`（`name()`）、`pub struct ObfsOpts { pub mode: ObfsMode, pub host: Option<String>, pub uri: String }`、`pub fn read_obfs(r: &mut ParamReader<'_>, allowed: &[ObfsMode]) -> Option<ObfsOpts>`
  - `rurge_config::spec::ss`：`pub enum SsMethod { None, Aes128Gcm, Aes192Gcm, Aes256Gcm, ChaCha20IetfPoly1305, XChaCha20IetfPoly1305, Blake3Aes128Gcm, Blake3Aes256Gcm }`（`name()`、`key_len()`（= salt 长度，`none` 为 0）、`is_2022()`）、`pub struct SsSpec { pub method, pub password: Secret<String>, pub keys: Secret<Vec<Vec<u8>>>, pub udp_relay: bool, pub udp_port: Option<u16>, pub obfs: Option<ObfsOpts> }`、`pub struct SsRead { pub spec: SsSpec, pub stream_cipher: Option<&'static str> }`、`pub fn read_ss(r: &mut ParamReader<'_>) -> SsRead`
  - `ProtoSpec::Ss(SsSpec)`；`pub enum NotImplemented { LegacyVmess, SsStreamCipher(&'static str) }`（`warning()` / `note()` / `imported()`）；`SpecOutcome.not_implemented: Option<NotImplemented>`（取代 `legacy_vmess`）；`Config.not_implemented: HashMap<String, NotImplemented>`；`rurge_policy` 的 `Imported.not_implemented`

- [ ] **Step 1: 先写用例**

配置层（`NotImplemented` 的加载告警、`ss` 行的检查、脱敏）：

`crates/rurge-config/src/config.rs`——把

```rust
        assert!(loaded.config.spec("Old1").is_none() && loaded.config.spec("Old2").is_none());
```

换成

```rust
        assert!(loaded.config.spec("Old1").is_none() && loaded.config.spec("Old2").is_none());
        assert_eq!(
            loaded.config.not_implemented.get("Old2"),
            Some(&NotImplemented::LegacyVmess)
        );
        assert_eq!(loaded.config.not_implemented.get("New"), None);
    }

    /// An `ss` stream cipher: `W0007` once per load and cipher, at the first
    /// line that uses it (phase 2 M6 design 3.1).
    #[test]
    fn ss_stream_ciphers_are_reported_once_per_load_and_cipher() {
        let loaded = load_text(
            "[Proxy]\nA = ss, a.test, 8388, encrypt-method=rc4-md5, password=pw\n\
B = ss, b.test, 8388, encrypt-method=aes-256-cfb, password=pw\n\
C = ss, c.test, 8388, encrypt-method=rc4-md5, password=pw\n\
D = ss, d.test, 8388, encrypt-method=aes-256-gcm, password=pw\n[Rule]\nFINAL,DIRECT\n",
        );
        let found: Vec<(&str, Option<u32>)> = loaded
            .diagnostics
            .iter()
            .filter(|d| d.code == codes::W_PROTOCOL_NOT_IMPLEMENTED)
            .map(|d| (d.message.as_str(), d.span.as_ref().map(|s| s.line)))
            .collect();
        assert_eq!(
            found,
            [
                (
                    "`ss` stream cipher `rc4-md5` is not implemented yet; such policies behave as REJECT",
                    Some(2)
                ),
                (
                    "`ss` stream cipher `aes-256-cfb` is not implemented yet; such policies behave as REJECT",
                    Some(3)
                ),
            ]
        );
        assert_eq!(
            loaded.config.not_implemented.get("C"),
            Some(&NotImplemented::SsStreamCipher("rc4-md5"))
        );
        assert_eq!(loaded.config.not_implemented.get("D"), None);
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
        let o = outcome(
            "SS",
            "ss, h, 8388, encrypt-method=aes-128-gcm, password=x, mystery=1",
        );
```

换成

```rust
        let o = outcome("H", "hysteria2, h, 443, password=x, mystery=1");
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
        assert!(!o.legacy_vmess);
```

换成

```rust
        assert_eq!(o.not_implemented, None);
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
        assert!(o.legacy_vmess);
```

换成

```rust
        assert_eq!(o.not_implemented, Some(NotImplemented::LegacyVmess));
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
        assert!(o.spec.is_none() && !o.legacy_vmess);
        assert_eq!(o.diagnostics.len(), 1);
```

换成

```rust
        assert!(o.spec.is_none() && o.not_implemented.is_none());
        assert_eq!(o.diagnostics.len(), 1);
    }

    /// An `ss` line is read and checked in full; it has no spec until the
    /// engine builds `ss` (M6a task 6), and a stream cipher says why.
    #[test]
    fn an_ss_line_is_checked_and_a_stream_cipher_is_not_implemented() {
        let o = outcome(
            "S",
            "ss, h.test, 8388, encrypt-method=aes-128-gcm, password=pw, udp-relay=true, udp-port=8389, obfs=tls, obfs-host=cdn.test, shadow-tls-password=st",
        );
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        assert!(o.inert.is_empty(), "{:?}", o.inert);
        assert_eq!(o.not_implemented, None);
        assert!(o.spec.is_none());

        let o = outcome("S", "ss, h.test, 8388, encrypt-method=RC4-MD5, password=pw");
        assert!(o.spec.is_none() && o.diagnostics.is_empty());
        let why = o.not_implemented.expect("a stream cipher");
        assert_eq!(why, NotImplemented::SsStreamCipher("rc4-md5"));
        assert_eq!(
            why.warning(),
            "`ss` stream cipher `rc4-md5` is not implemented yet; such policies behave as REJECT"
        );
        assert_eq!(why.note(), "ss (rc4-md5)");
        assert_eq!(why.imported(), "`ss` with the stream cipher `rc4-md5`");

        // a broken line is an error like any other, and not "not implemented"
        let o = outcome("S", "ss, h.test, 8388, encrypt-method=rc4-md5");
        assert!(o.not_implemented.is_none());
        assert_eq!(o.diagnostics[0].code, codes::E_INVALID_POLICY_PARAM);
        // no TLS under `ss`; mystery parameters are unknown as anywhere
        let o = outcome(
            "S",
            "ss, h.test, 8388, encrypt-method=none, sni=edge.test, mystery=1",
        );
        let found: Vec<(&str, &str)> = o
            .diagnostics
            .iter()
            .map(|d| (d.code, d.message.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `S`: `sni` does not apply to `ss` policies; ignored"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `S`: unknown parameter `mystery` ignored"
                ),
            ]
        );
```

`crates/rurge-config/src/redact.rs`——把

```rust
            "external, exec = \"/usr/bin/sshpass\", args = ***, args = ***, args=***, local-port = 1080"
        );
    }
}

```

换成

```rust
            "external, exec = \"/usr/bin/sshpass\", args = ***, args = ***, args=***, local-port = 1080"
        );
    }

    /// The camouflage host of an `ss` / `snell` line (phase 2 M6 design 6);
    /// `obfs-uri` and the method stay.
    #[test]
    fn an_obfs_line_loses_its_host() {
        assert_eq!(
            redact_definition(
                "ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=pw, obfs=http, obfs-host=my.cdn.test, obfs-uri=/x"
            ),
            "ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=***, obfs=http, obfs-host=***, obfs-uri=/x"
        );
    }
}

```

语料库里的 2022 行：密钥是 32 字节，方法要与之相符（密钥校验后原来那行会是 `E0018`）：

`tests/corpus/valid/kitchen-sink.conf`——把

```text
SS-2022 = ss, 192.0.2.12, 8388, encrypt-method=2022-blake3-aes-128-gcm, password=YctPZ6U7xPPcU+gp3u+0tx/tRizJN9K8y+uKlW2qjlI=
```

换成

```text
SS-2022 = ss, 192.0.2.12, 8388, encrypt-method=2022-blake3-aes-256-gcm, password=YctPZ6U7xPPcU+gp3u+0tx/tRizJN9K8y+uKlW2qjlI=
```

策略层（订阅导入的流式方法行、注册表的说明）：

`crates/rurge-policy/src/assemble.rs`——把

```rust
SS = ss, s.test, 8388, encrypt-method=aes-128-gcm, password=pw\n\
```

换成

```rust
SS = ss, s.test, 8388, encrypt-method=aes-128-gcm, password=pw\n\
Rc = ss, r.test, 8388, encrypt-method=rc4, password=pw\n\
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
        assert_eq!(members(&a, "G"), ["SS", "Old", "Good"]);
        let specs: Vec<bool> = a.imported.iter().map(|i| i.spec.is_some()).collect();
        assert_eq!(specs, [false, false, true]);
```

换成

```rust
        assert_eq!(members(&a, "G"), ["SS", "Rc", "Old", "Good"]);
        let specs: Vec<bool> = a.imported.iter().map(|i| i.spec.is_some()).collect();
        assert_eq!(specs, [false, false, false, true]);
        let why: Vec<Option<NotImplemented>> = a
            .imported
            .iter()
            .map(|i| i.not_implemented.clone())
            .collect();
        assert_eq!(
            why,
            [
                None,
                Some(NotImplemented::SsStreamCipher("rc4")),
                Some(NotImplemented::LegacyVmess),
                None
            ]
        );
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
                    "policy group `G`: imported policies of type `ss` are not implemented in this version; they behave as REJECT".to_string()
```

换成

```rust
                    "policy group `G`: imported policies of type `ss` are not implemented in this version; they behave as REJECT".to_string()
                ),
                (
                    codes::W_PROTOCOL_NOT_IMPLEMENTED,
                    "policy group `G`: imported policies of type `ss` with the stream cipher `rc4` are not implemented in this version; they behave as REJECT".to_string()
```

`crates/rurge-policy/src/registry.rs`——把

```rust
SS = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\n[Rule]\nFINAL,DIRECT\n";
```

换成

```rust
SS = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\n\
Stream = ss, 1.2.3.4, 8388, encrypt-method=rc4-md5, password=x\n[Rule]\nFINAL,DIRECT\n";
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        assert_eq!(ss.note, Some(Note::Unsupported("ss".into())));
```

换成

```rust
        assert_eq!(ss.note, Some(Note::Unsupported("ss".into())));
        let stream = registry.resolve(&PolicyRef::parse("Stream"));
        assert_eq!(stream.terminal, TerminalKind::Reject);
        assert_eq!(
            stream.note.clone().unwrap().to_string(),
            "policy protocol not implemented: ss (rc4-md5)"
        );
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-config --lib`
Expected: FAIL——`NotImplemented` 与 `not_implemented` 由 Step 3 引入，编译不过：

```text
error[E0609]: no field `not_implemented` on type `config::Config`
    --> crates\rurge-config\src\config.rs:1643:27
error[E0433]: failed to resolve: use of undeclared type `NotImplemented`
    --> crates\rurge-config\src\config.rs:1644:19
error[E0609]: no field `not_implemented` on type `config::Config`
    --> crates\rurge-config\src\config.rs:1646:34
error[E0609]: no field `not_implemented` on type `config::Config`
    --> crates\rurge-config\src\config.rs:1679:27
error[E0433]: failed to resolve: use of undeclared type `NotImplemented`
    --> crates\rurge-config\src\config.rs:1680:19
error[E0609]: no field `not_implemented` on type `config::Config`
    --> crates\rurge-config\src\config.rs:1682:34
error[E0609]: no field `not_implemented` on type `spec::SpecOutcome`
   --> crates\rurge-config\src\spec\mod.rs:711:22
error[E0609]: no field `not_implemented` on type `spec::SpecOutcome`
   --> crates\rurge-config\src\spec\mod.rs:730:22
error[E0433]: failed to resolve: use of undeclared type `NotImplemented`
   --> crates\rurge-config\src\spec\mod.rs:730:44
error[E0609]: no field `not_implemented` on type `spec::SpecOutcome`
   --> crates\rurge-config\src\spec\mod.rs:738:39
error[E0609]: no field `not_implemented` on type `spec::SpecOutcome`
   --> crates\rurge-config\src\spec\mod.rs:752:22
error[E0609]: no field `not_implemented` on type `spec::SpecOutcome`
   --> crates\rurge-config\src\spec\mod.rs:757:21
error[E0433]: failed to resolve: use of undeclared type `NotImplemented`
   --> crates\rurge-config\src\spec\mod.rs:758:25
error[E0609]: no field `not_implemented` on type `spec::SpecOutcome`
   --> crates\rurge-config\src\spec\mod.rs:768:19
Some errors have detailed explanations: E0433, E0609.
For more information about an error, try `rustc --explain E0433`.
error: could not compile `rurge-config` (lib test) due to 14 previous errors
exit 101
```

- [ ] **Step 3: 实现**

两个新模块（自带用例）：

新建 `crates/rurge-config/src/spec/obfs.rs`：

```rust
//! simple-obfs parameters (`obfs`, `obfs-host`, `obfs-uri`; manual:
//! Policies › Shadowsocks, Policies › Snell): a camouflage layer right below
//! the protocol (phase 2 M6 design 3.1 / 3.2).

use super::reader::ParamReader;
use crate::diagnostic::codes;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObfsMode {
    /// The first packet each way looks like an HTTP upgrade.
    Http,
    /// The first packets look like a TLS handshake, the rest like TLS records.
    Tls,
}

impl ObfsMode {
    pub fn name(self) -> &'static str {
        match self {
            ObfsMode::Http => "http",
            ObfsMode::Tls => "tls",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObfsOpts {
    pub mode: ObfsMode,
    /// `obfs-host`: the `Host` header of `http`, the SNI of `tls`. `None`:
    /// the policy's server host (a rurge default, the manual gives none).
    pub host: Option<String>,
    /// `obfs-uri`: the request path of `http`; `/` unless written.
    pub uri: String,
}

const KEYS: [&str; 2] = ["obfs-host", "obfs-uri"];

/// What goes into a request line or a header as is: printable ASCII, no
/// space.
fn printable(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_graphic())
}

/// `None` without `obfs`; `obfs-host` / `obfs-uri` are then `W0028`.
/// `allowed` is what the protocol supports (`http` / `tls` for `ss`); any
/// other value is `E0018`. After an error was reported the returned value is
/// meaningless: the caller checks `r.has_errors()`. Neither the host nor the
/// path is quoted in a diagnostic: the camouflage host can identify the user.
pub fn read_obfs(r: &mut ParamReader<'_>, allowed: &[ObfsMode]) -> Option<ObfsOpts> {
    if !r.has("obfs") {
        for key in KEYS {
            if r.has(key) {
                r.touch(key);
                r.warn(
                    codes::W_PARAM_NOT_APPLICABLE,
                    format!("`{key}` has no effect without `obfs`; ignored"),
                );
            }
        }
        return None;
    }
    let table: Vec<(&str, ObfsMode)> = allowed.iter().map(|m| (m.name(), *m)).collect();
    let Some(mode) = r.choice("obfs", &table) else {
        // `choice` reported the value; the other two are not unknown
        for key in KEYS {
            r.touch(key);
        }
        return None;
    };
    let mut host = None;
    if let Some(v) = r.str("obfs-host") {
        let v = v.trim();
        if printable(v) {
            host = Some(v.to_string());
        } else {
            r.error(
                codes::E_INVALID_POLICY_PARAM,
                "invalid `obfs-host` (expected a host name without space or control character)"
                    .to_string(),
            );
        }
    }
    let mut uri = "/".to_string();
    if let Some(v) = r.str("obfs-uri") {
        let v = v.trim();
        if mode == ObfsMode::Tls {
            r.warn(
                codes::W_PARAM_NOT_APPLICABLE,
                "`obfs-uri` has no effect with `obfs=tls`; ignored".to_string(),
            );
        } else if v.starts_with('/') && printable(v) {
            uri = v.to_string();
        } else {
            r.error(
                codes::E_INVALID_POLICY_PARAM,
                "invalid `obfs-uri` (expected an ASCII path that starts with `/` and holds no space or control character)"
                    .to_string(),
            );
        }
    }
    Some(ObfsOpts { mode, host, uri })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Diagnostic, codes};
    use crate::policy::parse_policy;
    use crate::span::Span;
    use std::path::Path;
    use std::sync::Arc;

    const BOTH: [ObfsMode; 2] = [ObfsMode::Http, ObfsMode::Tls];

    fn read(def: &str, allowed: &[ObfsMode]) -> (Option<ObfsOpts>, bool, Vec<Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        let opts = read_obfs(&mut r, allowed);
        let failed = r.has_errors();
        (opts, failed, r.finish())
    }

    fn messages(diags: &[Diagnostic]) -> Vec<(&str, &str)> {
        diags.iter().map(|d| (d.code, d.message.as_str())).collect()
    }

    #[test]
    fn the_manuals_examples_and_the_defaults() {
        let (opts, failed, diags) = read(
            "ss, h.test, 8388, obfs=http, obfs-host=bing.com, obfs-uri=/a/b?c=1",
            &BOTH,
        );
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(
            opts,
            Some(ObfsOpts {
                mode: ObfsMode::Http,
                host: Some("bing.com".into()),
                uri: "/a/b?c=1".into(),
            })
        );
        let (opts, failed, diags) = read("ss, h.test, 8388, obfs=TLS", &BOTH);
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(
            opts,
            Some(ObfsOpts {
                mode: ObfsMode::Tls,
                host: None,
                uri: "/".into(),
            })
        );
        let (opts, _, _) = read("ss, h.test, 8388", &BOTH);
        assert_eq!(opts, None);
    }

    #[test]
    fn the_other_two_without_obfs_are_not_applicable() {
        let (opts, failed, diags) =
            read("ss, h.test, 8388, obfs-host=cdn.test, obfs-uri=/x", &BOTH);
        assert!(opts.is_none() && !failed);
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `obfs-host` has no effect without `obfs`; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `obfs-uri` has no effect without `obfs`; ignored"
                ),
            ]
        );
    }

    #[test]
    fn a_path_with_tls_is_ignored() {
        let (opts, failed, diags) = read("ss, h.test, 8388, obfs=tls, obfs-uri=/x", &BOTH);
        assert!(!failed);
        assert_eq!(opts.unwrap().uri, "/");
        assert_eq!(
            messages(&diags),
            [(
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `P`: `obfs-uri` has no effect with `obfs=tls`; ignored"
            )]
        );
    }

    #[test]
    fn an_unknown_or_unsupported_mode_is_an_error() {
        let (opts, failed, diags) = read(
            "ss, h.test, 8388, obfs=websocket, obfs-host=cdn.test",
            &BOTH,
        );
        assert!(opts.is_none() && failed);
        assert_eq!(
            messages(&diags),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: invalid value `websocket` for `obfs` (expected http / tls)"
            )]
        );
        // a protocol that supports `http` only
        let (_, failed, diags) = read("snell, h.test, 443, obfs=tls", &[ObfsMode::Http]);
        assert!(failed);
        assert_eq!(
            diags[0].message,
            "policy `P`: invalid value `tls` for `obfs` (expected http)"
        );
    }

    #[test]
    fn a_bad_host_or_path_is_an_error_and_never_quoted() {
        for (def, text) in [
            (
                "ss, h.test, 8388, obfs=http, obfs-uri=s3cret",
                "policy `P`: invalid `obfs-uri` (expected an ASCII path that starts with `/` and holds no space or control character)",
            ),
            (
                "ss, h.test, 8388, obfs=http, obfs-uri=/s3cret path",
                "policy `P`: invalid `obfs-uri` (expected an ASCII path that starts with `/` and holds no space or control character)",
            ),
            (
                "ss, h.test, 8388, obfs=http, obfs-host=s3cret host",
                "policy `P`: invalid `obfs-host` (expected a host name without space or control character)",
            ),
            (
                "ss, h.test, 8388, obfs=tls, obfs-host=\"\"",
                "policy `P`: invalid `obfs-host` (expected a host name without space or control character)",
            ),
        ] {
            let (_, failed, diags) = read(def, &BOTH);
            assert!(failed, "{def}");
            assert_eq!(
                messages(&diags),
                [(codes::E_INVALID_POLICY_PARAM, text)],
                "{def}"
            );
            assert!(!diags[0].message.contains("s3cret"), "{def}");
        }
    }
}
```

新建 `crates/rurge-config/src/spec/ss.rs`：

```rust
//! `ss` policy parameters (manual: Policies › Shadowsocks; phase 2 M6
//! design 3.1).

use super::obfs::{ObfsMode, ObfsOpts, read_obfs};
use super::reader::ParamReader;
use super::secret::Secret;
use crate::diagnostic::codes;
use base64::Engine as _;
use base64::alphabet::STANDARD;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};

/// `encrypt-method`: the ciphers this version implements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SsMethod {
    /// No encryption: the address, then the payload as is.
    None,
    Aes128Gcm,
    Aes192Gcm,
    Aes256Gcm,
    ChaCha20IetfPoly1305,
    XChaCha20IetfPoly1305,
    /// SS 2022 (SIP022).
    Blake3Aes128Gcm,
    Blake3Aes256Gcm,
}

const METHODS: [(&str, SsMethod); 8] = [
    ("aes-128-gcm", SsMethod::Aes128Gcm),
    ("aes-192-gcm", SsMethod::Aes192Gcm),
    ("aes-256-gcm", SsMethod::Aes256Gcm),
    ("chacha20-ietf-poly1305", SsMethod::ChaCha20IetfPoly1305),
    ("xchacha20-ietf-poly1305", SsMethod::XChaCha20IetfPoly1305),
    ("2022-blake3-aes-128-gcm", SsMethod::Blake3Aes128Gcm),
    ("2022-blake3-aes-256-gcm", SsMethod::Blake3Aes256Gcm),
    ("none", SsMethod::None),
];

/// The stream ciphers of the original protocol: valid, but not implemented
/// before M8 (`W0007`, and the policy behaves as REJECT).
const STREAM_CIPHERS: [&str; 20] = [
    "rc4",
    "rc4-md5",
    "aes-128-cfb",
    "aes-192-cfb",
    "aes-256-cfb",
    "aes-128-ctr",
    "aes-192-ctr",
    "aes-256-ctr",
    "bf-cfb",
    "camellia-128-cfb",
    "camellia-192-cfb",
    "camellia-256-cfb",
    "cast5-cfb",
    "des-cfb",
    "idea-cfb",
    "rc2-cfb",
    "seed-cfb",
    "salsa20",
    "chacha20",
    "chacha20-ietf",
];

/// SS 2022 keys: the standard alphabet, padding optional (as
/// shadowsocks-rust reads them).
const KEY_BASE64: GeneralPurpose = GeneralPurpose::new(
    &STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

impl SsMethod {
    pub fn name(self) -> &'static str {
        METHODS
            .iter()
            .find(|(_, m)| *m == self)
            .map(|(name, _)| *name)
            .expect("every method is in the table")
    }

    /// The key length in bytes, which is also the salt length; 0 for `none`.
    pub fn key_len(self) -> usize {
        match self {
            SsMethod::None => 0,
            SsMethod::Aes128Gcm | SsMethod::Blake3Aes128Gcm => 16,
            SsMethod::Aes192Gcm => 24,
            SsMethod::Aes256Gcm
            | SsMethod::ChaCha20IetfPoly1305
            | SsMethod::XChaCha20IetfPoly1305
            | SsMethod::Blake3Aes256Gcm => 32,
        }
    }

    /// SS 2022: the password is Base64 keys, not a password.
    pub fn is_2022(self) -> bool {
        matches!(self, SsMethod::Blake3Aes128Gcm | SsMethod::Blake3Aes256Gcm)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SsSpec {
    pub method: SsMethod,
    /// As written; empty for `none` without one. The AEAD methods derive
    /// their key from it.
    pub password: Secret<String>,
    /// SS 2022: what `password` decodes to, each key of the method's length.
    /// The identity keys come first, outermost first (SIP023), and the user
    /// key last; a single-user server has the user key only. Empty for the
    /// other methods.
    pub keys: Secret<Vec<Vec<u8>>>,
    pub udp_relay: bool,
    /// `udp-port`: where UDP goes; `None`: the policy's port.
    pub udp_port: Option<u16>,
    pub obfs: Option<ObfsOpts>,
}

/// What an `ss` line says. With a stream cipher `stream_cipher` names it and
/// `spec` is meaningless: the caller makes no spec of it (phase 2 M6 design
/// 3.1).
pub struct SsRead {
    pub spec: SsSpec,
    pub stream_cipher: Option<&'static str>,
}

/// The keys of an SS 2022 password, or the 1-based position of the first
/// one that is not Base64 of `len` bytes.
fn decode_keys(password: &str, len: usize) -> Result<Vec<Vec<u8>>, usize> {
    password
        .split(':')
        .enumerate()
        .map(|(i, part)| match KEY_BASE64.decode(part.trim()) {
            Ok(key) if key.len() == len => Ok(key),
            _ => Err(i + 1),
        })
        .collect()
}

/// Everything `ss`-specific on the line. After an error was reported the
/// returned value is meaningless: the caller checks `r.has_errors()`.
///
/// The password is named-only (`password=`), as the manual writes it, and is
/// never quoted in a diagnostic; the method may be, it is no secret.
pub fn read_ss(r: &mut ParamReader<'_>) -> SsRead {
    let mut method = SsMethod::None;
    let mut stream_cipher = None;
    match r.str("encrypt-method").map(str::trim) {
        None => r.error(
            codes::E_INVALID_POLICY_PARAM,
            "`encrypt-method` is required".to_string(),
        ),
        Some(v) => {
            if let Some((_, m)) = METHODS.iter().find(|(n, _)| n.eq_ignore_ascii_case(v)) {
                method = *m;
            } else if let Some(name) = STREAM_CIPHERS.iter().find(|n| n.eq_ignore_ascii_case(v)) {
                stream_cipher = Some(*name);
            } else {
                let names: Vec<&str> = METHODS.iter().map(|(name, _)| *name).collect();
                r.invalid("encrypt-method", v, &names.join(" / "));
            }
        }
    }
    let password = r.str("password").unwrap_or_default();
    let needs_password = method != SsMethod::None || stream_cipher.is_some();
    let mut keys = Vec::new();
    if password.is_empty() {
        if needs_password {
            r.error(
                codes::E_INVALID_POLICY_PARAM,
                "`password` is required".to_string(),
            );
        }
    } else if method.is_2022() {
        let len = method.key_len();
        match decode_keys(password, len) {
            Ok(decoded) => keys = decoded,
            Err(position) => r.error(
                codes::E_INVALID_POLICY_PARAM,
                format!(
                    "key #{position} of `password` is not a Base64 key of {len} bytes, as `{}` requires",
                    method.name()
                ),
            ),
        }
    }
    let udp_relay = r.bool("udp-relay").unwrap_or(false);
    let udp_port = match r.number::<u16>("udp-port", "a port from 1 to 65535") {
        Some(0) => {
            r.invalid("udp-port", "0", "a port from 1 to 65535");
            None
        }
        port => port,
    };
    let obfs = read_obfs(r, &[ObfsMode::Http, ObfsMode::Tls]);
    SsRead {
        spec: SsSpec {
            method,
            password: password.into(),
            keys: Secret::new(keys),
            udp_relay,
            udp_port,
            obfs,
        },
        stream_cipher,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Diagnostic, codes};
    use crate::policy::parse_policy;
    use crate::span::Span;
    use std::path::Path;
    use std::sync::Arc;

    /// 16 and 32 bytes of Base64 (`openssl rand -base64 16` / `32`).
    const KEY16: &str = "tn6UbJ3OzpVCTU1RlQzm2g==";
    const KEY32: &str = "YctPZ6U7xPPcU+gp3u+0tx/tRizJN9K8y+uKlW2qjlI=";

    fn read(def: &str) -> (SsRead, bool, Vec<Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        let got = read_ss(&mut r);
        let failed = r.has_errors();
        (got, failed, r.finish())
    }

    fn messages(diags: &[Diagnostic]) -> Vec<&str> {
        diags.iter().map(|d| d.message.as_str()).collect()
    }

    #[test]
    fn the_manuals_example() {
        let (got, failed, diags) = read(
            "ss, 1.2.3.4, 8000, encrypt-method=chacha20-ietf-poly1305, password=abcd1234, obfs=http, obfs-host=bing.com, obfs-uri=/resource/file, udp-relay=true",
        );
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert!(got.stream_cipher.is_none());
        let spec = got.spec;
        assert_eq!(spec.method, SsMethod::ChaCha20IetfPoly1305);
        assert_eq!(spec.password.expose(), "abcd1234");
        assert!(spec.keys.expose().is_empty());
        assert!(spec.udp_relay);
        assert_eq!(spec.udp_port, None);
        let obfs = spec.obfs.expect("obfs");
        assert_eq!(
            (obfs.mode, obfs.host.as_deref(), obfs.uri.as_str()),
            (ObfsMode::Http, Some("bing.com"), "/resource/file")
        );
    }

    #[test]
    fn every_method_by_name_with_its_key_length() {
        for (name, method, len) in [
            ("aes-128-gcm", SsMethod::Aes128Gcm, 16),
            ("AES-192-GCM", SsMethod::Aes192Gcm, 24),
            ("aes-256-gcm", SsMethod::Aes256Gcm, 32),
            ("chacha20-ietf-poly1305", SsMethod::ChaCha20IetfPoly1305, 32),
            (
                "xchacha20-ietf-poly1305",
                SsMethod::XChaCha20IetfPoly1305,
                32,
            ),
            ("2022-blake3-aes-128-gcm", SsMethod::Blake3Aes128Gcm, 16),
            ("2022-blake3-aes-256-gcm", SsMethod::Blake3Aes256Gcm, 32),
            ("none", SsMethod::None, 0),
        ] {
            let password = match len {
                16 if method.is_2022() => KEY16,
                32 if method.is_2022() => KEY32,
                _ => "pw",
            };
            let (got, failed, diags) = read(&format!(
                "ss, h.test, 8388, encrypt-method={name}, password={password}"
            ));
            assert!(!failed && diags.is_empty(), "{name}: {diags:?}");
            assert_eq!(got.spec.method, method, "{name}");
            assert_eq!(method.key_len(), len, "{name}");
            assert_eq!(method.name(), name.to_ascii_lowercase());
        }
        // `none` needs no password
        let (got, failed, diags) = read("ss, h.test, 8388, encrypt-method=none");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(got.spec.password.expose(), "");
    }

    #[test]
    fn ss_2022_keys_are_decoded_last_one_the_user_key() {
        let (got, failed, diags) = read(&format!(
            "ss, h.test, 8388, encrypt-method=2022-blake3-aes-256-gcm, password={KEY32}"
        ));
        assert!(!failed && diags.is_empty(), "{diags:?}");
        let keys = got.spec.keys.expose();
        assert_eq!(keys.len(), 1);
        assert_eq!(&keys[0][..4], &[0x61, 0xcb, 0x4f, 0x67]);
        // an identity key, then the user key; padding is optional
        let bare = KEY16.trim_end_matches('=');
        let other = "AAECAwQFBgcICQoLDA0ODw";
        let (got, failed, diags) = read(&format!(
            "ss, h.test, 8388, encrypt-method=2022-blake3-aes-128-gcm, password={bare}:{other}"
        ));
        assert!(!failed && diags.is_empty(), "{diags:?}");
        let keys = got.spec.keys.expose();
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0][..2], [0xb6, 0x7e], "the identity key");
        assert_eq!(keys[1], (0u8..16).collect::<Vec<u8>>(), "the user key");
        assert!(keys.iter().all(|k| k.len() == 16));
    }

    #[test]
    fn a_bad_ss_2022_key_is_an_error_that_never_quotes_the_password() {
        for (method, password, text) in [
            (
                "2022-blake3-aes-128-gcm",
                KEY32.to_string(),
                "policy `P`: key #1 of `password` is not a Base64 key of 16 bytes, as `2022-blake3-aes-128-gcm` requires",
            ),
            (
                "2022-blake3-aes-256-gcm",
                KEY16.to_string(),
                "policy `P`: key #1 of `password` is not a Base64 key of 32 bytes, as `2022-blake3-aes-256-gcm` requires",
            ),
            (
                "2022-blake3-aes-128-gcm",
                "hunter2".to_string(),
                "policy `P`: key #1 of `password` is not a Base64 key of 16 bytes, as `2022-blake3-aes-128-gcm` requires",
            ),
            (
                "2022-blake3-aes-256-gcm",
                format!("{KEY16}:{KEY32}"),
                "policy `P`: key #1 of `password` is not a Base64 key of 32 bytes, as `2022-blake3-aes-256-gcm` requires",
            ),
            (
                "2022-blake3-aes-128-gcm",
                format!("{KEY16}:"),
                "policy `P`: key #2 of `password` is not a Base64 key of 16 bytes, as `2022-blake3-aes-128-gcm` requires",
            ),
        ] {
            let (_, failed, diags) = read(&format!(
                "ss, h.test, 8388, encrypt-method={method}, password={password}"
            ));
            assert!(failed, "{method} {password}");
            assert_eq!(messages(&diags), [text]);
            assert_eq!(diags[0].code, codes::E_INVALID_POLICY_PARAM);
            for part in password.split(':').filter(|p| !p.is_empty()) {
                assert!(!diags[0].message.contains(part), "{}", diags[0].message);
            }
        }
    }

    #[test]
    fn the_method_and_the_password_are_required() {
        let (_, failed, diags) = read("ss, h.test, 8388, password=pw");
        assert!(failed);
        assert_eq!(
            messages(&diags),
            ["policy `P`: `encrypt-method` is required"]
        );
        let (_, failed, diags) = read("ss, h.test, 8388, encrypt-method=aes-128-gcm, hunter2");
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [
                "policy `P`: `password` is required",
                "policy `P`: unexpected positional value #1 ignored"
            ]
        );
        assert!(diags.iter().all(|d| !d.message.contains("hunter2")));
        // a stream cipher needs one too
        let (_, failed, _) = read("ss, h.test, 8388, encrypt-method=rc4-md5");
        assert!(failed);
    }

    #[test]
    fn an_unknown_method_is_an_error_that_quotes_it() {
        let (_, failed, diags) = read("ss, h.test, 8388, encrypt-method=aes-512-gcm, password=pw");
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [
                "policy `P`: invalid value `aes-512-gcm` for `encrypt-method` (expected aes-128-gcm / aes-192-gcm / aes-256-gcm / chacha20-ietf-poly1305 / xchacha20-ietf-poly1305 / 2022-blake3-aes-128-gcm / 2022-blake3-aes-256-gcm / none)"
            ]
        );
    }

    #[test]
    fn a_stream_cipher_is_named_and_not_an_error() {
        for (written, name) in [
            ("rc4-md5", "rc4-md5"),
            ("AES-256-CFB", "aes-256-cfb"),
            ("chacha20-ietf", "chacha20-ietf"),
            ("camellia-128-cfb", "camellia-128-cfb"),
        ] {
            let (got, failed, diags) = read(&format!(
                "ss, h.test, 8388, encrypt-method={written}, password=pw"
            ));
            assert!(!failed && diags.is_empty(), "{written}: {diags:?}");
            assert_eq!(got.stream_cipher, Some(name));
        }
    }

    #[test]
    fn udp_parameters() {
        let (got, failed, diags) =
            read("ss, h.test, 8388, encrypt-method=none, udp-relay=true, udp-port=8389");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert!(got.spec.udp_relay);
        assert_eq!(got.spec.udp_port, Some(8389));
        for bad in ["0", "65536", "port"] {
            let (_, failed, diags) = read(&format!(
                "ss, h.test, 8388, encrypt-method=none, udp-port={bad}"
            ));
            assert!(failed, "{bad}");
            assert_eq!(
                messages(&diags),
                [format!(
                    "policy `P`: invalid value `{bad}` for `udp-port` (expected a port from 1 to 65535)"
                )
                .as_str()]
            );
        }
        let (_, failed, _) = read("ss, h.test, 8388, encrypt-method=none, udp-relay=maybe");
        assert!(failed);
    }

    #[test]
    fn the_spec_does_not_print_the_password_or_the_keys() {
        let (got, _, _) = read(&format!(
            "ss, h.test, 8388, encrypt-method=2022-blake3-aes-256-gcm, password={KEY32}"
        ));
        let printed = format!("{:?}", got.spec);
        assert!(printed.contains("Secret(***)"), "{printed}");
        assert!(!printed.contains("YctP"), "{printed}");
        // 0x61, the first key byte, as `Debug` would print it
        assert!(!printed.contains("97"), "{printed}");
    }
}
```

`spec/mod.rs`：模块、`ProtoSpec::Ss`、`NotImplemented`、`to_spec` 的 `ss` 分支（暂不产出 spec，P3）：

`crates/rurge-config/src/spec/mod.rs`——把

```rust
pub mod http;
```

换成

```rust
pub mod http;
pub mod obfs;
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
pub mod socks5;
```

换成

```rust
pub mod socks5;
pub mod ss;
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
pub use http::{HeaderPart, HeaderTemplate, HttpSpec};
```

换成

```rust
pub use http::{HeaderPart, HeaderTemplate, HttpSpec};
pub use obfs::{ObfsMode, ObfsOpts};
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
pub use socks5::Socks5Spec;
```

换成

```rust
pub use socks5::Socks5Spec;
pub use ss::{SsMethod, SsSpec};
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    External(ExternalSpec),
```

换成

```rust
    External(ExternalSpec),
    Ss(SsSpec),
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            | ProtoSpec::External(_) => None,
```

换成

```rust
            | ProtoSpec::External(_)
            | ProtoSpec::Ss(_) => None,
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    /// A `vmess` line without `vmess-aead=true`: valid, but it asks for the
    /// legacy handshake, so it has no spec. The caller reports it once per
    /// load (`W0007`, M2 design 4.3).
    pub legacy_vmess: bool,
```

换成

```rust
    /// A valid line that asks for something not implemented yet, so it has
    /// no spec. The caller reports each distinct one once per load (`W0007`).
    pub not_implemented: Option<NotImplemented>,
}

/// What a valid line asks for that this version does not implement: the
/// policy has no spec and behaves as REJECT.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NotImplemented {
    /// `vmess` without `vmess-aead=true`: the legacy handshake (M2 design 4.3).
    LegacyVmess,
    /// An `ss` stream cipher, by its name (phase 2 M6 design 3.1).
    SsStreamCipher(&'static str),
}

impl NotImplemented {
    /// The load warning (`W0007`).
    pub fn warning(&self) -> String {
        match self {
            NotImplemented::LegacyVmess => "`vmess` without `vmess-aead=true` uses the legacy handshake, which is not implemented yet".to_string(),
            NotImplemented::SsStreamCipher(method) => format!(
                "`ss` stream cipher `{method}` is not implemented yet; such policies behave as REJECT"
            ),
        }
    }

    /// What the session log says after "policy protocol not implemented: ".
    pub fn note(&self) -> String {
        match self {
            NotImplemented::LegacyVmess => "vmess (legacy handshake)".to_string(),
            NotImplemented::SsStreamCipher(method) => format!("ss ({method})"),
        }
    }

    /// How a warning about imported policies names the kind.
    pub fn imported(&self) -> String {
        match self {
            NotImplemented::LegacyVmess => {
                "`vmess` without `vmess-aead=true` (the legacy handshake)".to_string()
            }
            NotImplemented::SsStreamCipher(method) => {
                format!("`ss` with the stream cipher `{method}`")
            }
        }
    }
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    let mut legacy_vmess = false;
```

换成

```rust
    let mut not_implemented = None;
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            legacy_vmess = !read.aead;
            (common, ProtoSpec::Vmess(read.spec))
```

换成

```rust
            if !read.aead {
                not_implemented = Some(NotImplemented::LegacyVmess);
            }
            (common, ProtoSpec::Vmess(read.spec))
        }
        PolicyKind::Shadowsocks => {
            let common = read_common(&mut r, Applies::Proxy, &mut notes);
            tls::refuse_tls(&mut r);
            let read = ss::read_ss(&mut r);
            not_implemented = read.stream_cipher.map(NotImplemented::SsStreamCipher);
            (common, ProtoSpec::Ss(read.spec))
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    let spec = (!failed && !legacy_vmess).then(|| PolicySpec {
```

换成

```rust
    // `ss` is read and checked in full, but the engine builds it only from
    // M6a task 6 on: until then a valid line has no spec either (the
    // capability table's `W0007`, REJECT at run time)
    let built = policy.kind != PolicyKind::Shadowsocks;
    let spec = (!failed && built && not_implemented.is_none()).then(|| PolicySpec {
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
        legacy_vmess: legacy_vmess && !failed,
```

换成

```rust
        not_implemented: not_implemented.filter(|_| !failed),
```

`config.rs`：按 `NotImplemented` 去重的 `W0007` 与 `Config.not_implemented`：

`crates/rurge-config/src/config.rs`——把

```rust
use crate::spec::{GroupSpec, NameKind, PolicySpec, ProtoSpec, SpecEnv, to_group_spec, to_spec};
```

换成

```rust
use crate::spec::{
    GroupSpec, NameKind, NotImplemented, PolicySpec, ProtoSpec, SpecEnv, to_group_spec, to_spec,
};
```

`crates/rurge-config/src/config.rs`——把

```rust
    pub specs: Vec<PolicySpec>,
```

换成

```rust
    pub specs: Vec<PolicySpec>,
    /// Why a policy without errors has no spec, when its line asks for
    /// something not implemented yet (`W0007`); by policy name.
    pub not_implemented: HashMap<String, NotImplemented>,
```

`crates/rurge-config/src/config.rs`——把

```rust
        specs: Vec::new(),
```

换成

```rust
        specs: Vec::new(),
        not_implemented: HashMap::new(),
```

`crates/rurge-config/src/config.rs`——把

```rust
    let (specs, group_specs) = {
```

换成

```rust
    let (specs, not_implemented, group_specs) = {
```

`crates/rurge-config/src/config.rs`——把

```rust
        let mut legacy_seen = false;
```

换成

```rust
        let mut not_implemented = HashMap::new();
        let mut not_implemented_seen: HashSet<NotImplemented> = HashSet::new();
```

`crates/rurge-config/src/config.rs`——把

```rust
            if outcome.legacy_vmess && !legacy_seen {
                legacy_seen = true;
                diags.push(
                    Diagnostic::warning(
                        codes::W_PROTOCOL_NOT_IMPLEMENTED,
                        "`vmess` without `vmess-aead=true` uses the legacy handshake, which is not implemented yet".to_string(),
                    )
                    .at(p.span.clone()),
                );
```

换成

```rust
            if let Some(why) = outcome.not_implemented {
                if not_implemented_seen.insert(why.clone()) {
                    diags.push(
                        Diagnostic::warning(codes::W_PROTOCOL_NOT_IMPLEMENTED, why.warning())
                            .at(p.span.clone()),
                    );
                }
                not_implemented.insert(p.name.clone(), why);
```

`crates/rurge-config/src/config.rs`——把

```rust
        (specs, group_specs)
    };
    config.specs = specs;
```

换成

```rust
        (specs, not_implemented, group_specs)
    };
    config.specs = specs;
    config.not_implemented = not_implemented;
```

`crates/rurge-config/src/redact.rs`——把

```rust
/// policy's `args` often carry a password (`sshpass -p …`, M4-D12).
/// Over-redacting is the safe side for an endpoint whose purpose is safe
/// output.
const SECRET_PARAMS: [&str; 17] = [
```

换成

```rust
/// policy's `args` often carry a password (`sshpass -p …`, M4-D12). The
/// camouflage host of `obfs-host` can identify the user (phase 2 M6 design 6).
/// Over-redacting is the safe side for an endpoint whose purpose is safe
/// output.
const SECRET_PARAMS: [&str; 18] = [
```

`crates/rurge-config/src/redact.rs`——把

```rust
    "args",
```

换成

```rust
    "args",
    "obfs-host",
```

策略层：

`crates/rurge-policy/src/assemble.rs`——把

```rust
    GroupSpec, NameKind, PolicyPath, PolicySpec, ProtoSpec, SpecEnv, to_spec,
```

换成

```rust
    GroupSpec, NameKind, NotImplemented, PolicyPath, PolicySpec, ProtoSpec, SpecEnv, to_spec,
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
    pub spec: Option<PolicySpec>,
```

换成

```rust
    pub spec: Option<PolicySpec>,
    /// Why there is no spec, when the line asks for something not
    /// implemented yet.
    pub not_implemented: Option<NotImplemented>,
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
                        out.list.push((Imported { policy, spec: None }, g));
```

换成

```rust
                        out.list.push((
                            Imported {
                                policy,
                                spec: None,
                                not_implemented: None,
                            },
                            g,
                        ));
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
                let what = if outcome.legacy_vmess {
                    "`vmess` without `vmess-aead=true` (the legacy handshake)".to_string()
                } else {
                    format!("`{}`", imported.policy.kind.keyword())
```

换成

```rust
                let what = match &outcome.not_implemented {
                    Some(why) => why.imported(),
                    None => format!("`{}`", imported.policy.kind.keyword()),
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
                    ));
                }
            }
        }
        self.forget(&failed);
```

换成

```rust
                    ));
                }
            }
            imported.not_implemented = outcome.not_implemented;
        }
        self.forget(&failed);
```

`crates/rurge-policy/src/registry.rs`——把

```rust
use rurge_config::spec::{CommonOpts, GroupSpec, IpVersion, PolicySpec, ProtoSpec};
```

换成

```rust
use rurge_config::spec::{CommonOpts, GroupSpec, IpVersion, NotImplemented, PolicySpec, ProtoSpec};
```

`crates/rurge-policy/src/registry.rs`——把

```rust
    /// The protocol is not implemented yet: REJECT (W0007 at load).
    Unsupported { kind: PolicyKind },
```

换成

```rust
    /// The protocol is not implemented yet: REJECT (W0007 at load). `what`
    /// is what the session log says about it.
    Unsupported { kind: PolicyKind, what: String },
```

`crates/rurge-policy/src/registry.rs`——把

```rust
/// What the session log says about a policy that has no spec. Since M2b a
/// `vmess` policy of a profile that loaded is only ever without one for a
/// single reason: the line lacks `vmess-aead=true` (M2 design 4.3).
fn unsupported_text(kind: PolicyKind) -> String {
    match kind {
        PolicyKind::Vmess => "vmess (legacy handshake)".to_string(),
        other => other.keyword().to_string(),
    }
```

换成

```rust
/// What the session log says about a policy that has no spec: what its
/// line asks for that is not implemented, else its protocol.
fn unsupported_text(kind: PolicyKind, why: Option<&NotImplemented>) -> String {
    why.map_or_else(|| kind.keyword().to_string(), NotImplemented::note)
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        let policy_entry = |kind: PolicyKind, spec: Option<&PolicySpec>| {
            Ok::<Entry, BuildError>(match (alias_terminal(kind), spec) {
                (Some(Terminal::Direct), Some(spec)) if has_socket_opts(&spec.common) => {
                    outbound_entry(spec, false)?
                }
                (Some(terminal), _) => Entry::Alias(terminal),
                (None, Some(spec)) => outbound_entry(spec, true)?,
                // no spec: a protocol of a later milestone
                (None, None) => Entry::Unsupported { kind },
            })
        };
```

换成

```rust
        let policy_entry =
            |kind: PolicyKind, spec: Option<&PolicySpec>, why: Option<&NotImplemented>| {
                Ok::<Entry, BuildError>(match (alias_terminal(kind), spec) {
                    (Some(Terminal::Direct), Some(spec)) if has_socket_opts(&spec.common) => {
                        outbound_entry(spec, false)?
                    }
                    (Some(terminal), _) => Entry::Alias(terminal),
                    (None, Some(spec)) => outbound_entry(spec, true)?,
                    // no spec: a protocol of a later milestone
                    (None, None) => Entry::Unsupported {
                        kind,
                        what: unsupported_text(kind, why),
                    },
                })
            };
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            let entry = policy_entry(p.kind, spec)?;
```

换成

```rust
            let entry = policy_entry(p.kind, spec, cfg.not_implemented.get(&p.name))?;
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            match policy_entry(i.policy.kind, i.spec.as_ref()) {
```

换成

```rust
            match policy_entry(i.policy.kind, i.spec.as_ref(), i.not_implemented.as_ref()) {
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            Some(Entry::Unsupported { kind }) => {
                chain.push(format!("!unsupported:{}", kind.keyword()));
                self.rejected(chain, Some(Note::Unsupported(unsupported_text(*kind))))
```

换成

```rust
            Some(Entry::Unsupported { kind, what }) => {
                chain.push(format!("!unsupported:{}", kind.keyword()));
                self.rejected(chain, Some(Note::Unsupported(what.clone())))
```

引擎工厂的占位分支（Task 6 换成真正的出站）：

`crates/rurge-engine/src/outbounds.rs`——把

```rust
            ),
```

换成

```rust
            ),
            // the loader makes no spec of an `ss` line before M6a task 6
            ProtoSpec::Ss(_) => {
                return Err(BuildError::new(format!(
                    "policy `{}`: `ss` is not implemented yet",
                    spec.name
                )));
            }
```

要点：
- 2022 的密钥错误文字说第几段、要多少字节，绝不引用取值；`encrypt-method` 不认识时可以引用它（不是秘密）。
- 流式方法的 `W0007` 每次加载每种方法一条；在 `ss` 进能力表（Task 6）之前，通用的 "policy type `ss` is not implemented" 也会出现。
- vmess 的用例只把 `legacy_vmess` 字段改成 `not_implemented == Some(NotImplemented::LegacyVmess)`，文字不变。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-config` → 通过（`spec::obfs` 5 条、`spec::ss` 9 条、`spec::tests::an_ss_line_is_checked_and_a_stream_cipher_is_not_implemented`、`config::tests::ss_stream_ciphers_are_reported_once_per_load_and_cipher`、`redact::tests::an_obfs_line_loses_its_host`，语料库快照不变）。
Run: `cargo test -p rurge-policy` → 通过（`a_legacy_vmess_policy_says_why_it_rejects` 与 `an_imported_line_that_cannot_be_used_is_skipped_by_its_number` 多了流式方法的断言）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-config crates/rurge-policy crates/rurge-engine/src/outbounds.rs tests/corpus
git commit -m "feat(config): ss 的配置（SsSpec、共用的 ObfsOpts）；流式方法只解析；"未实现"说明推广为 NotImplemented"
```

### Task 2: simple-obfs 层

`transport::obfs`：`http` 与 `tls` 两种字节流包装（P6、P7），作为 `Stack` 的一层（connect → shadow-tls → **obfs** → tls → ws）。假服务端一侧的 `testing::accept_obfs` 独立实现，供 Task 3 起的 `FakeShadowsocks` 使用。本任务还没有协议用它，用例直接测这一层与 `Stack`。

**Files:**
- Create: `crates/rurge-proto/src/transport/obfs/mod.rs`、`http.rs`、`tls.rs`（均自带用例）、`crates/rurge-proto/src/testing/obfs.rs`
- Modify: `crates/rurge-proto/src/transport/mod.rs`、`src/transport/stack.rs`（与用例）、`src/testing/mod.rs`

**Interfaces:**
- Consumes: Task 1 的 `rurge_config::spec::{ObfsOpts, ObfsMode}`；既有的 `BoxedStream`、`Target`、`BuildError`、`Stack`。
- Produces:
  - `pub struct rurge_proto::transport::obfs::ObfsClient`：`ObfsClient::new(opts: &ObfsOpts, server: &Target) -> Result<ObfsClient, BuildError>`、`fn wrap(&self, stream: BoxedStream) -> BoxedStream`（同步，没有握手往返）
  - `Stack::with_obfs(self, obfs: ObfsClient) -> Stack`（`Stack::new` 的签名不变）
  - 测试设施：`pub struct ObfsHello { pub host: String, pub uri: Option<String>, pub user_agent: Option<String>, pub first_payload: Vec<u8> }`、`pub async fn accept_obfs(stream: BoxedStream, mode: ObfsMode) -> io::Result<(BoxedStream, ObfsHello)>`（返回的流先吐出首段负载；它等客户端先写）

- [ ] **Step 1: 先写用例（连同假服务端一侧）**

新建 `crates/rurge-proto/src/testing/obfs.rs`：

```rust
//! The server side of simple-obfs (`http` / `tls`), written independently of
//! the client (`transport::obfs`) so each checks the other: a fake server
//! wraps an accepted connection with `accept_obfs` and speaks its protocol
//! over the returned stream.

use crate::transport::prefixed;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rurge_config::spec::ObfsMode;
use rurge_net::connector::BoxedStream;
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// What the client's first packet said.
#[derive(Clone, Debug, Default)]
pub struct ObfsHello {
    /// The `Host` header of `http` (port included), the server name of `tls`.
    pub host: String,
    /// The request path (`http` only).
    pub uri: Option<String>,
    pub user_agent: Option<String>,
    /// The payload that rode in the first packet.
    pub first_payload: Vec<u8>,
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("fake obfs: {what}"))
}

/// The largest record the reference server accepts from a client.
const MAX_RECORD: usize = 16384;

/// Reads the client's first packet and returns a stream that yields the
/// first payload and then the rest of the client's data, and whose writes
/// are camouflaged: the first one behind the upgrade answer (`http`) or the
/// fake ServerHello and ChangeCipherSpec (`tls`), the later ones raw or in
/// application data records. A shutdown before any write sends nothing.
/// A client record over 16 KiB ends the connection (as the reference server).
pub async fn accept_obfs(
    mut stream: BoxedStream,
    mode: ObfsMode,
) -> io::Result<(BoxedStream, ObfsHello)> {
    let (hello, session_id) = match mode {
        ObfsMode::Http => (read_request(&mut stream).await?, [0u8; 32]),
        ObfsMode::Tls => read_client_hello(&mut stream).await?,
    };
    let (app, pump) = tokio::io::duplex(64 * 1024);
    let (from_client, to_client) = tokio::io::split(stream);
    let (from_app, to_app) = tokio::io::split(pump);
    tokio::spawn(async move {
        let _ = tokio::join!(
            inbound(mode, from_client, to_app),
            outbound(mode, session_id, from_app, to_client),
        );
    });
    let first = hello.first_payload.clone();
    Ok((prefixed::boxed(first, Box::new(app)), hello))
}

async fn read_request(stream: &mut BoxedStream) -> io::Result<ObfsHello> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > MAX_RECORD {
            return Err(bad("the request head is too long"));
        }
        stream.read_exact(&mut byte).await?;
        head.push(byte[0]);
    }
    let text = String::from_utf8(head).map_err(|_| bad("the request head is not text"))?;
    let mut lines = text.split("\r\n");
    let request = lines.next().unwrap_or_default();
    let uri = request
        .strip_prefix("GET ")
        .and_then(|rest| rest.strip_suffix(" HTTP/1.1"))
        .ok_or_else(|| bad("not a GET request"))?;
    let mut hello = ObfsHello {
        uri: Some(uri.to_string()),
        ..ObfsHello::default()
    };
    let mut upgrade = false;
    let mut length = None;
    for line in lines.filter(|l| !l.is_empty()) {
        let (name, value) = line.split_once(": ").ok_or_else(|| bad("a bad header"))?;
        match name.to_ascii_lowercase().as_str() {
            "host" => hello.host = value.to_string(),
            "user-agent" => hello.user_agent = Some(value.to_string()),
            "upgrade" => upgrade = value == "websocket",
            "content-length" => length = value.parse::<usize>().ok(),
            _ => {}
        }
    }
    if !upgrade {
        return Err(bad("no `Upgrade: websocket`"));
    }
    let length = length.ok_or_else(|| bad("no `Content-Length`"))?;
    hello.first_payload = vec![0u8; length];
    stream.read_exact(&mut hello.first_payload).await?;
    Ok(hello)
}

/// Takes `n` bytes off the front of `data`.
fn take<'a>(data: &mut &'a [u8], n: usize) -> io::Result<&'a [u8]> {
    if data.len() < n {
        return Err(bad("the ClientHello is cut short"));
    }
    let (head, rest) = data.split_at(n);
    *data = rest;
    Ok(head)
}

fn take_u16(data: &mut &[u8]) -> io::Result<usize> {
    let b = take(data, 2)?;
    Ok(usize::from(u16::from_be_bytes([b[0], b[1]])))
}

/// The hello and the session id the ServerHello echoes.
async fn read_client_hello(stream: &mut BoxedStream) -> io::Result<(ObfsHello, [u8; 32])> {
    let mut header = [0u8; 5];
    stream.read_exact(&mut header).await?;
    if header[..3] != [0x16, 0x03, 0x01] {
        return Err(bad("not a TLS handshake record"));
    }
    let mut body = vec![0u8; usize::from(u16::from_be_bytes([header[3], header[4]]))];
    stream.read_exact(&mut body).await?;
    let mut data = &body[..];
    let handshake = take(&mut data, 4)?;
    let length = usize::from(u16::from_be_bytes([handshake[2], handshake[3]]));
    if handshake[..2] != [0x01, 0x00] || length != data.len() {
        return Err(bad("not a ClientHello"));
    }
    take(&mut data, 2 + 4 + 28)?;
    if take(&mut data, 1)? != [32] {
        return Err(bad("the session id is not 32 bytes"));
    }
    let mut session_id = [0u8; 32];
    session_id.copy_from_slice(take(&mut data, 32)?);
    let suites = take_u16(&mut data)?;
    take(&mut data, suites)?;
    let methods = take(&mut data, 1)?[0];
    take(&mut data, usize::from(methods))?;
    let extensions = take_u16(&mut data)?;
    if extensions != data.len() {
        return Err(bad("the extensions do not fill the ClientHello"));
    }
    let mut hello = ObfsHello::default();
    let (mut ticket, mut name) = (false, false);
    while !data.is_empty() {
        let kind = take_u16(&mut data)?;
        let len = take_u16(&mut data)?;
        let mut ext = take(&mut data, len)?;
        match kind {
            0x0023 => {
                hello.first_payload = ext.to_vec();
                ticket = true;
            }
            0x0000 => {
                take_u16(&mut ext)?;
                if take(&mut ext, 1)? != [0] {
                    return Err(bad("the server name is not a host name"));
                }
                let len = take_u16(&mut ext)?;
                hello.host = String::from_utf8(take(&mut ext, len)?.to_vec())
                    .map_err(|_| bad("the server name is not text"))?;
                name = true;
            }
            _ => {}
        }
    }
    if !ticket || !name {
        return Err(bad("no session ticket or no server name"));
    }
    Ok((hello, session_id))
}

/// The client's data after its first packet, towards the fake's protocol.
async fn inbound(
    mode: ObfsMode,
    mut from: impl AsyncRead + Unpin,
    mut to: impl AsyncWrite + Unpin,
) -> io::Result<()> {
    match mode {
        ObfsMode::Http => {
            tokio::io::copy(&mut from, &mut to).await?;
        }
        ObfsMode::Tls => {
            let mut body = vec![0u8; MAX_RECORD];
            loop {
                let mut header = [0u8; 5];
                match from.read_exact(&mut header).await {
                    Ok(_) => {}
                    Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                    Err(e) => return Err(e),
                }
                let len = usize::from(u16::from_be_bytes([header[3], header[4]]));
                if header[..3] != [0x17, 0x03, 0x03] || len > MAX_RECORD {
                    return Err(bad("a bad client record"));
                }
                from.read_exact(&mut body[..len]).await?;
                to.write_all(&body[..len]).await?;
            }
        }
    }
    to.shutdown().await
}

fn server_hello(session_id: &[u8; 32]) -> Vec<u8> {
    let mut out = vec![
        0x16, 0x03, 0x01, 0x00, 0x5b, 0x02, 0x00, 0x00, 0x57, 0x03, 0x03,
    ];
    let mut random = [0u8; 32];
    let _ = getrandom::fill(&mut random);
    out.extend_from_slice(&random);
    out.push(32);
    out.extend_from_slice(session_id);
    // ECDHE-RSA-CHACHA20-POLY1305, no compression, and the extensions
    // length left at 0 although extensions follow (as the reference does)
    out.extend_from_slice(&[0xcc, 0xa8, 0x00, 0x00, 0x00]);
    out.extend_from_slice(&[0xff, 0x01, 0x00, 0x01, 0x00]);
    out.extend_from_slice(&[0x00, 0x17, 0x00, 0x00]);
    out.extend_from_slice(&[0x00, 0x0b, 0x00, 0x02, 0x01, 0x00]);
    // ChangeCipherSpec
    out.extend_from_slice(&[0x14, 0x03, 0x03, 0x00, 0x01, 0x01]);
    out
}

/// The fake's protocol data, camouflaged, towards the client.
async fn outbound(
    mode: ObfsMode,
    session_id: [u8; 32],
    mut from: impl AsyncRead + Unpin,
    mut to: impl AsyncWrite + Unpin,
) -> io::Result<()> {
    let mut buf = vec![0u8; MAX_RECORD];
    let mut first = true;
    loop {
        let n = from.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        let mut packet = Vec::with_capacity(n + 256);
        match mode {
            ObfsMode::Http if first => {
                let mut accept = [0u8; 16];
                let _ = getrandom::fill(&mut accept);
                packet.extend_from_slice(
                    format!(
                        "HTTP/1.1 101 Switching Protocols\r\n\
                         Server: nginx/1.18.0\r\n\
                         Date: Wed, 30 Sep 2026 00:00:00 GMT\r\n\
                         Upgrade: websocket\r\n\
                         Connection: Upgrade\r\n\
                         Sec-WebSocket-Accept: {}\r\n\r\n",
                        STANDARD.encode(accept)
                    )
                    .as_bytes(),
                );
            }
            ObfsMode::Http => {}
            ObfsMode::Tls => {
                let kind = if first {
                    packet.extend_from_slice(&server_hello(&session_id));
                    0x16
                } else {
                    0x17
                };
                packet.extend_from_slice(&[kind, 0x03, 0x03]);
                packet.extend_from_slice(&(n as u16).to_be_bytes());
            }
        }
        packet.extend_from_slice(&buf[..n]);
        to.write_all(&packet).await?;
        first = false;
    }
    to.shutdown().await
}
```

`crates/rurge-proto/src/testing/mod.rs`——把

```rust
mod http_proxy;
```

换成

```rust
mod http_proxy;
mod obfs;
```

`crates/rurge-proto/src/testing/mod.rs`——把

```rust
pub use http_proxy::{FakeHttpProxy, HttpProxyScript, RecordedHead};
```

换成

```rust
pub use http_proxy::{FakeHttpProxy, HttpProxyScript, RecordedHead};
pub use obfs::{ObfsHello, accept_obfs};
```

`Stack` 的用例：

`crates/rurge-proto/src/transport/stack.rs`——把

```rust
        Camouflage, FakeShadowTls, FakeWs, ShadowTlsScript, TlsFixture, WsScript,
```

换成

```rust
        Camouflage, FakeShadowTls, FakeWs, ShadowTlsScript, TlsFixture, WsScript, accept_obfs,
```

`crates/rurge-proto/src/transport/stack.rs`——把

```rust
    use rurge_config::spec::{Secret, ShadowTlsOpts, ShadowTlsVersion, TlsOpts, WsOpts};
```

换成

```rust
    use rurge_config::spec::{
        ObfsMode, ObfsOpts, Secret, ShadowTlsOpts, ShadowTlsVersion, TlsOpts, WsOpts,
    };
```

`crates/rurge-proto/src/transport/stack.rs`——把

```rust
    #[tokio::test]
    async fn each_layer_says_which_one_failed() {
```

换成

```rust
    #[tokio::test]
    async fn obfs_sits_on_the_connection() {
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let fake = tokio::spawn(async move {
                let (tcp, _) = listener.accept().await.unwrap();
                let (stream, hello) = accept_obfs(Box::new(tcp), ObfsMode::Tls).await.unwrap();
                let (mut rd, mut wr) = tokio::io::split(stream);
                tokio::io::copy(&mut rd, &mut wr).await.unwrap();
                wr.shutdown().await.unwrap();
                hello
            });
            let server = Target::new(HostName::Ip(addr.ip()), addr.port());
            let obfs = ObfsClient::new(
                &ObfsOpts {
                    mode: ObfsMode::Tls,
                    host: None,
                    uri: "/".into(),
                },
                &server,
            )
            .unwrap();
            let stack = Stack::new(
                Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
                server,
                None,
                None,
                None,
            )
            .with_obfs(obfs);
            let mut stream = stack.open(&ConnectOpts::default()).await.unwrap();
            stream.write_all(b"through obfs").await.unwrap();
            stream.shutdown().await.unwrap();
            let mut back = Vec::new();
            stream.read_to_end(&mut back).await.unwrap();
            assert_eq!(back, b"through obfs");
            let hello = fake.await.unwrap();
            // no `obfs-host`: the server's own name
            assert_eq!(hello.host, "127.0.0.1");
            assert_eq!(hello.first_payload, b"through obfs");
        })
        .await
        .expect("the round trip finished within the bound");
    }

    #[tokio::test]
    async fn each_layer_says_which_one_failed() {
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-proto --lib obfs`
Expected: FAIL——`ObfsClient` 与 `Stack::with_obfs` 由 Step 3 引入，编译不过：

```text
error[E0433]: failed to resolve: use of undeclared type `ObfsClient`
   --> crates\rurge-proto\src\transport\stack.rs:197:24
error[E0599]: no method named `with_obfs` found for struct `stack::Stack` in the current scope
   --> crates\rurge-proto\src\transport\stack.rs:213:14
Some errors have detailed explanations: E0433, E0599.
For more information about an error, try `rustc --explain E0433`.
error: could not compile `rurge-proto` (lib test) due to 2 previous errors
exit 101
```

- [ ] **Step 3: 实现**

新模块（自带用例）：

新建 `crates/rurge-proto/src/transport/obfs/mod.rs`：

```rust
//! simple-obfs (`obfs=http` / `obfs=tls`, phase 2 M6 design 3.2): a
//! camouflage layer between the connection (or Shadow TLS) and the protocol.
//! Only the look of the bytes changes; nothing is encrypted or authenticated.
//!
//! The first packet leaves with the first non-empty write: an empty write is
//! a no-op, and a flush or a shutdown before any write sends nothing. Every
//! protocol above this layer writes its own request header first (through
//! `LazyHead`, which sends it alone after its grace), so the camouflage head
//! never needs to go out without a payload. Neither mode puts more than
//! 16 KiB of payload into the first packet: the reference server reads the
//! whole first packet into a 16 KiB buffer.
//!
//! A server that closes before sending a single byte is an ordinary EOF, so
//! the protocol above can say what that means for it (`ss` reports "closed
//! the connection without answering"); a server that answers with anything
//! else than the camouflage is an error that never quotes the answer.

mod http;
mod tls;

use crate::BuildError;
use rurge_config::HostName;
use rurge_config::spec::{ObfsMode, ObfsOpts};
use rurge_net::connector::{BoxedStream, Target};
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

const NOT_OBFS: &str = "obfs: the server did not answer as an obfs server";
const MALFORMED: &str = "obfs: the server sent a malformed record";
const CUT_SHORT: &str = "obfs: the server closed the connection in the middle of a record";

/// The longest `obfs-host` written into a request or a server name.
const MAX_HOST: usize = 255;

#[derive(Clone, Debug)]
enum Kind {
    /// `host` already carries `:port` when the port is not 80.
    Http {
        uri: String,
        host: String,
    },
    Tls {
        host: String,
    },
}

/// Everything that can be prepared ahead of a connection.
pub struct ObfsClient {
    kind: Kind,
}

impl ObfsClient {
    /// `server` is the policy's own server: its name stands in for a missing
    /// `obfs-host`, and `http` appends its port unless it is 80 (whichever
    /// host is used, as the reference client does). No error text quotes the
    /// host or the path: both can identify the user.
    pub fn new(opts: &ObfsOpts, server: &Target) -> Result<ObfsClient, BuildError> {
        // the configuration layer checks both, but the fields are public and
        // a line break would end the request head early
        let printable = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_graphic());
        let host = match &opts.host {
            Some(host) if printable(host) && host.len() <= MAX_HOST => host.clone(),
            Some(_) => {
                return Err(BuildError::new(
                    "`obfs-host` cannot be written into the camouflage",
                ));
            }
            None => {
                let name = match (&server.host, opts.mode) {
                    // a server name carries no brackets
                    (HostName::Ip(ip), ObfsMode::Tls) => Some(ip.to_string()),
                    _ => crate::http::wire_host(server),
                };
                name.filter(|name| name.len() <= MAX_HOST).ok_or_else(|| {
                    BuildError::new("the server's host name cannot be written into the camouflage")
                })?
            }
        };
        let kind = match opts.mode {
            ObfsMode::Http => {
                if !opts.uri.starts_with('/') || !printable(&opts.uri) {
                    return Err(BuildError::new(
                        "`obfs-uri` cannot be written into the camouflage",
                    ));
                }
                let host = if server.port == 80 {
                    host
                } else {
                    format!("{host}:{}", server.port)
                };
                Kind::Http {
                    uri: opts.uri.clone(),
                    host,
                }
            }
            ObfsMode::Tls => Kind::Tls { host },
        };
        Ok(ObfsClient { kind })
    }

    pub fn wrap(&self, stream: BoxedStream) -> BoxedStream {
        Box::new(ObfsStream::new(stream, self.kind.clone()))
    }
}

/// Which record the server sends next (`obfs=tls`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    ServerHello,
    ChangeCipherSpec,
    /// The handshake record that carries the server's first data.
    FirstData,
    AppData,
}

impl Phase {
    fn kind(self) -> u8 {
        match self {
            Phase::ServerHello | Phase::FirstData => tls::HANDSHAKE,
            Phase::ChangeCipherSpec => tls::CHANGE_CIPHER_SPEC,
            Phase::AppData => tls::APPLICATION_DATA,
        }
    }

    fn next(self) -> Phase {
        match self {
            Phase::ServerHello => Phase::ChangeCipherSpec,
            Phase::ChangeCipherSpec => Phase::FirstData,
            Phase::FirstData | Phase::AppData => Phase::AppData,
        }
    }
}

enum ReadState {
    /// `obfs=http` before the end of the server's answer head.
    HttpHead,
    /// `obfs=tls`: every byte is inside some record.
    Records {
        /// The record whose header comes next.
        phase: Phase,
        header: [u8; tls::HEADER_LEN],
        /// How much of `header` is in.
        have: usize,
        /// What is left of the current record's body.
        left: usize,
        /// Whether the current body is data (or a record only skipped).
        deliver: bool,
    },
    /// `obfs=http` after the head: what is left in the buffer, then the
    /// stream itself.
    Raw,
}

/// Bytes through the camouflage (see the module comment).
struct ObfsStream {
    inner: BoxedStream,
    /// `Some` until the first packet is built.
    first: Option<Kind>,
    tls: bool,
    /// The packet being written and how much of it is out.
    out: Vec<u8>,
    sent: usize,
    /// The caller's bytes that `out` carries: reported once all of `out` is
    /// written, even when a flush finished it (see `poll_write`).
    accepted: usize,
    read: ReadState,
    rbuf: Vec<u8>,
    rpos: usize,
    rend: usize,
}

fn invalid(text: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, text)
}

impl ObfsStream {
    fn new(inner: BoxedStream, kind: Kind) -> ObfsStream {
        let (tls, read, buffer) = match kind {
            Kind::Http { .. } => (false, ReadState::HttpHead, http::MAX_RESPONSE_HEAD),
            Kind::Tls { .. } => (
                true,
                ReadState::Records {
                    phase: Phase::ServerHello,
                    header: [0; tls::HEADER_LEN],
                    have: 0,
                    left: 0,
                    deliver: false,
                },
                tls::MAX_RECORD,
            ),
        };
        ObfsStream {
            inner,
            first: Some(kind),
            tls,
            out: Vec::new(),
            sent: 0,
            accepted: 0,
            read,
            rbuf: vec![0; buffer],
            rpos: 0,
            rend: 0,
        }
    }

    /// Builds the next packet from a prefix of `data`; returns its length.
    fn encode(&mut self, data: &[u8]) -> usize {
        match self.first.take() {
            Some(Kind::Http { uri, host }) => {
                let n = data.len().min(tls::MAX_RECORD);
                self.out = http::request_head(&uri, &host, n);
                self.out.extend_from_slice(&data[..n]);
                n
            }
            Some(Kind::Tls { host }) => {
                let n = data.len().min(tls::max_first_payload(&host));
                self.out = tls::client_hello(&host, &data[..n]);
                n
            }
            None => {
                let n = data.len().min(tls::MAX_RECORD);
                tls::app_data(&mut self.out, &data[..n]);
                n
            }
        }
    }

    /// Drives `out` into the stream below.
    fn poll_out(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.sent < self.out.len() {
            let n = ready!(Pin::new(&mut self.inner).poll_write(cx, &self.out[self.sent..]))?;
            if n == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
            self.sent += n;
        }
        self.out.clear();
        self.sent = 0;
        Poll::Ready(Ok(()))
    }

    /// Reads more into `rbuf[rend..limit]`; 0 at EOF.
    fn poll_fill(&mut self, cx: &mut Context<'_>, limit: usize) -> Poll<io::Result<usize>> {
        let mut rb = ReadBuf::new(&mut self.rbuf[self.rend..limit]);
        ready!(Pin::new(&mut self.inner).poll_read(cx, &mut rb))?;
        let n = rb.filled().len();
        self.rend += n;
        Poll::Ready(Ok(n))
    }

    /// Hands buffered bytes over.
    fn take_buffered(&mut self, buf: &mut ReadBuf<'_>) {
        let n = (self.rend - self.rpos).min(buf.remaining());
        buf.put_slice(&self.rbuf[self.rpos..self.rpos + n]);
        self.rpos += n;
    }
}

impl AsyncRead for ObfsStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        loop {
            match &mut this.read {
                ReadState::Raw => {
                    if this.rpos < this.rend {
                        this.take_buffered(buf);
                        return Poll::Ready(Ok(()));
                    }
                    return Pin::new(&mut this.inner).poll_read(cx, buf);
                }
                ReadState::HttpHead => {
                    if let Some(end) = http::head_end(&this.rbuf[..this.rend]) {
                        if !http::is_upgrade_answer(&this.rbuf[..end]) {
                            return Poll::Ready(Err(invalid(NOT_OBFS)));
                        }
                        this.rpos = end;
                        this.read = ReadState::Raw;
                        continue;
                    }
                    if this.rend == this.rbuf.len() {
                        return Poll::Ready(Err(invalid(NOT_OBFS)));
                    }
                    let limit = this.rbuf.len();
                    if ready!(this.poll_fill(cx, limit))? == 0 {
                        if this.rend == 0 {
                            // closed without a word: the protocol above says why
                            return Poll::Ready(Ok(()));
                        }
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            NOT_OBFS,
                        )));
                    }
                }
                ReadState::Records {
                    phase,
                    header,
                    have,
                    left,
                    deliver,
                } => {
                    let avail = this.rend - this.rpos;
                    if *left > 0 && avail > 0 {
                        let n = (*left).min(avail);
                        if *deliver {
                            let n = n.min(buf.remaining());
                            buf.put_slice(&this.rbuf[this.rpos..this.rpos + n]);
                            this.rpos += n;
                            *left -= n;
                            return Poll::Ready(Ok(()));
                        }
                        this.rpos += n;
                        *left -= n;
                        continue;
                    }
                    if *left == 0 && avail > 0 {
                        let n = (tls::HEADER_LEN - *have).min(avail);
                        header[*have..*have + n]
                            .copy_from_slice(&this.rbuf[this.rpos..this.rpos + n]);
                        this.rpos += n;
                        *have += n;
                        if *have == tls::HEADER_LEN {
                            let Some(len) = tls::record_len(header, phase.kind()) else {
                                let why = if *phase == Phase::AppData {
                                    MALFORMED
                                } else {
                                    NOT_OBFS
                                };
                                return Poll::Ready(Err(invalid(why)));
                            };
                            *have = 0;
                            *left = len;
                            *deliver = matches!(*phase, Phase::FirstData | Phase::AppData);
                            *phase = phase.next();
                        }
                        continue;
                    }
                    // everything buffered is consumed: start over at the front
                    let at_start = *phase == Phase::ServerHello && *have == 0;
                    let at_boundary = *phase == Phase::AppData && *have == 0 && *left == 0;
                    // the server's first data record has not begun yet
                    let handshake = *phase != Phase::AppData;
                    this.rpos = 0;
                    this.rend = 0;
                    let limit = this.rbuf.len();
                    if ready!(this.poll_fill(cx, limit))? == 0 {
                        if at_start || at_boundary {
                            return Poll::Ready(Ok(()));
                        }
                        let why = if handshake { NOT_OBFS } else { CUT_SHORT };
                        return Poll::Ready(Err(io::Error::new(io::ErrorKind::UnexpectedEof, why)));
                    }
                }
            }
        }
    }
}

impl AsyncWrite for ObfsStream {
    /// Success means the packet carrying the bytes reached the stream below.
    /// A packet that the stream below took only in part is finished first;
    /// every caller here retries with the same buffer (see `WsByteStream`).
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        if self.out.is_empty() && self.accepted == 0 {
            if self.first.is_none() && !self.tls {
                return Pin::new(&mut self.inner).poll_write(cx, data);
            }
            self.accepted = self.encode(data);
        }
        ready!(self.poll_out(cx))?;
        // a flush may have finished the packet: the retry learns it here
        let n = self.accepted.min(data.len());
        self.accepted = 0;
        Poll::Ready(Ok(n))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        ready!(self.poll_out(cx))?;
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        ready!(self.poll_out(cx))?;
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{ObfsHello, accept_obfs};
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
    use tokio::net::{TcpListener, TcpStream};

    const BOTH: [ObfsMode; 2] = [ObfsMode::Http, ObfsMode::Tls];

    fn opts(mode: ObfsMode, host: Option<&str>, uri: &str) -> ObfsOpts {
        ObfsOpts {
            mode,
            host: host.map(str::to_string),
            uri: uri.to_string(),
        }
    }

    fn edge(port: u16) -> Target {
        Target::new(HostName::parse("edge.example"), port)
    }

    fn client(mode: ObfsMode) -> ObfsClient {
        ObfsClient::new(&opts(mode, Some("cdn.example"), "/path"), &edge(8388)).unwrap()
    }

    /// A wrapped client end and the raw server end.
    fn pipe(mode: ObfsMode, capacity: usize) -> (BoxedStream, DuplexStream) {
        let (near, far) = tokio::io::duplex(capacity);
        (client(mode).wrap(Box::new(near)), far)
    }

    /// The server's first packet (hand-written, independent of the fake).
    fn answer(mode: ObfsMode, first: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        match mode {
            ObfsMode::Http => out.extend_from_slice(
                b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\r\n",
            ),
            ObfsMode::Tls => {
                out.extend_from_slice(&[0x16, 0x03, 0x01, 0x00, 0x5b, 0x02, 0x00, 0x00, 0x57]);
                out.extend_from_slice(&[0u8; 87]);
                out.extend_from_slice(&[0x14, 0x03, 0x03, 0x00, 0x01, 0x01]);
                out.extend_from_slice(&[0x16, 0x03, 0x03]);
                out.extend_from_slice(&(first.len() as u16).to_be_bytes());
            }
        }
        out.extend_from_slice(first);
        out
    }

    fn record(data: &[u8]) -> Vec<u8> {
        let mut out = vec![0x17, 0x03, 0x03];
        out.extend_from_slice(&(data.len() as u16).to_be_bytes());
        out.extend_from_slice(data);
        out
    }

    fn pattern(len: usize) -> Vec<u8> {
        (0..len as u32).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn the_host_defaults_to_the_server_and_http_adds_the_port() {
        let host = |mode, host: Option<&str>, server: Target| match ObfsClient::new(
            &opts(mode, host, "/"),
            &server,
        )
        .unwrap()
        .kind
        {
            Kind::Http { host, .. } | Kind::Tls { host } => host,
        };
        let v6 = |port| Target::new(HostName::parse("2001:db8::1"), port);
        assert_eq!(host(ObfsMode::Http, None, edge(8388)), "edge.example:8388");
        assert_eq!(host(ObfsMode::Http, None, edge(80)), "edge.example");
        assert_eq!(host(ObfsMode::Http, Some("cdn.test"), edge(80)), "cdn.test");
        assert_eq!(
            host(ObfsMode::Http, Some("cdn.test"), edge(443)),
            "cdn.test:443"
        );
        assert_eq!(host(ObfsMode::Http, None, v6(80)), "[2001:db8::1]");
        assert_eq!(host(ObfsMode::Tls, None, v6(443)), "2001:db8::1");
        assert_eq!(host(ObfsMode::Tls, None, edge(8388)), "edge.example");
        assert_eq!(
            host(ObfsMode::Tls, Some("cdn.test"), edge(8388)),
            "cdn.test"
        );
    }

    #[test]
    fn what_cannot_be_written_is_a_build_error_that_quotes_nothing() {
        let long = "a".repeat(256);
        for (o, server, expected) in [
            (
                opts(ObfsMode::Http, Some("a b"), "/"),
                edge(80),
                "`obfs-host` cannot be written into the camouflage",
            ),
            (
                opts(ObfsMode::Tls, Some(&long), "/"),
                edge(80),
                "`obfs-host` cannot be written into the camouflage",
            ),
            (
                opts(ObfsMode::Http, None, "no-slash"),
                edge(80),
                "`obfs-uri` cannot be written into the camouflage",
            ),
            (
                opts(ObfsMode::Http, None, "/a\r\nX: 1"),
                edge(80),
                "`obfs-uri` cannot be written into the camouflage",
            ),
            (
                opts(ObfsMode::Tls, None, "/"),
                Target::new(HostName::Domain("a@b.test".into()), 443),
                "the server's host name cannot be written into the camouflage",
            ),
        ] {
            let err = ObfsClient::new(&o, &server).err().expect("refused");
            assert_eq!(err.message, expected);
        }
    }

    #[tokio::test]
    async fn http_the_first_write_carries_the_head_and_later_ones_are_raw() {
        let (mut stream, mut server) = pipe(ObfsMode::Http, 64 * 1024);
        stream.write_all(b"first").await.unwrap();
        let mut seen = Vec::new();
        while !seen.ends_with(b"\r\n\r\nfirst") {
            let mut byte = [0u8; 1];
            server.read_exact(&mut byte).await.unwrap();
            seen.push(byte[0]);
        }
        let head = String::from_utf8(seen).unwrap();
        assert!(
            head.starts_with("GET /path HTTP/1.1\r\nHost: cdn.example:8388\r\nUser-Agent: curl/7."),
            "{head}"
        );
        assert!(
            head.contains("\r\nContent-Length: 5\r\n\r\nfirst"),
            "{head}"
        );
        stream.write_all(b"second").await.unwrap();
        let mut raw = [0u8; 6];
        server.read_exact(&mut raw).await.unwrap();
        assert_eq!(&raw, b"second");
        // the answer head is taken off, whatever follows it is data
        server
            .write_all(&answer(ObfsMode::Http, b"hello"))
            .await
            .unwrap();
        server.write_all(b"world").await.unwrap();
        let mut back = [0u8; 10];
        stream.read_exact(&mut back).await.unwrap();
        assert_eq!(&back, b"helloworld");
        // a half-close reaches the server and the other way stays open
        stream.shutdown().await.unwrap();
        let mut rest = Vec::new();
        server.read_to_end(&mut rest).await.unwrap();
        assert!(rest.is_empty());
        server.write_all(b"late").await.unwrap();
        let mut late = [0u8; 4];
        stream.read_exact(&mut late).await.unwrap();
        assert_eq!(&late, b"late");
    }

    #[tokio::test]
    async fn tls_the_first_write_rides_in_the_ticket_and_later_ones_split_at_16384() {
        let (mut stream, mut server) = pipe(ObfsMode::Tls, 256 * 1024);
        stream.write_all(b"first").await.unwrap();
        let mut hello = vec![0u8; tls::HELLO_OVERHEAD + 5 + "cdn.example".len()];
        server.read_exact(&mut hello).await.unwrap();
        assert_eq!(&hello[138..142], &[0x00, 0x23, 0x00, 0x05]);
        assert_eq!(&hello[142..147], b"first");
        assert_eq!(&hello[156..167], b"cdn.example");
        let data = pattern(40_000);
        stream.write_all(&data).await.unwrap();
        let mut got = Vec::new();
        let mut lengths = Vec::new();
        while got.len() < data.len() {
            let mut header = [0u8; 5];
            server.read_exact(&mut header).await.unwrap();
            assert_eq!(&header[..3], &[0x17, 0x03, 0x03]);
            let len = usize::from(u16::from_be_bytes([header[3], header[4]]));
            let mut body = vec![0u8; len];
            server.read_exact(&mut body).await.unwrap();
            lengths.push(len);
            got.extend_from_slice(&body);
        }
        assert_eq!(lengths, [16384, 16384, 7232]);
        assert_eq!(got, data);
        stream.shutdown().await.unwrap();
        let mut rest = Vec::new();
        server.read_to_end(&mut rest).await.unwrap();
        assert!(rest.is_empty(), "no close_notify or anything else");
    }

    #[tokio::test]
    async fn the_first_packet_carries_at_most_16_kib_and_an_empty_write_sends_nothing() {
        for (mode, expected) in [
            (ObfsMode::Http, 16384),
            (ObfsMode::Tls, tls::max_first_payload("cdn.example")),
        ] {
            let (mut stream, _server) = pipe(mode, 256 * 1024);
            assert_eq!(stream.write(&[1u8; 20_000]).await.unwrap(), expected);
            let (mut idle, mut server) = pipe(mode, 1024);
            assert_eq!(idle.write(&[]).await.unwrap(), 0);
            idle.flush().await.unwrap();
            idle.shutdown().await.unwrap();
            let mut rest = Vec::new();
            server.read_to_end(&mut rest).await.unwrap();
            assert!(rest.is_empty(), "{mode:?}");
        }
    }

    #[tokio::test]
    async fn the_servers_answer_is_read_whatever_the_slicing() {
        let big = pattern(20_000);
        for mode in BOTH {
            let mut wire = answer(mode, b"hello ");
            match mode {
                ObfsMode::Http => {
                    wire.extend_from_slice(b"obfs ");
                    wire.extend_from_slice(&big);
                }
                ObfsMode::Tls => {
                    wire.extend_from_slice(&record(b""));
                    wire.extend_from_slice(&record(b"obfs "));
                    wire.extend_from_slice(&record(&big[..16384]));
                    wire.extend_from_slice(&record(&big[16384..]));
                }
            }
            let mut expected = b"hello obfs ".to_vec();
            expected.extend_from_slice(&big);
            // (bytes the pipe holds, bytes per read): one byte at a time,
            // odd slices, and reads larger than a record
            for (capacity, chunk) in [(1usize, 1usize), (7, 3), (4096, 20_000)] {
                let (mut stream, mut server) = pipe(mode, capacity);
                let to_send = wire.clone();
                let writer = tokio::spawn(async move {
                    server.write_all(&to_send).await.unwrap();
                    server.shutdown().await.unwrap();
                    server
                });
                let mut got = Vec::new();
                let mut buf = vec![0u8; chunk];
                loop {
                    let n = stream.read(&mut buf).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    got.extend_from_slice(&buf[..n]);
                }
                assert!(got == expected, "{mode:?} {capacity} {chunk}");
                drop(writer.await.unwrap());
            }
        }
    }

    #[tokio::test]
    async fn a_server_that_is_not_obfs_fails_without_quoting_it() {
        let mut after_hello = answer(ObfsMode::Tls, b"x");
        after_hello.extend_from_slice(&[0x15, 0x03, 0x03, 0x00, 0x02, b's', b'e']);
        let mut cut = answer(ObfsMode::Tls, b"x");
        // what arrived of a record cut short is still data
        cut.extend_from_slice(&[0x17, 0x03, 0x03, 0x00, 0x0a, b'a', b'b', b'c']);
        for (mode, wire, expected) in [
            (
                ObfsMode::Http,
                b"HTTP/1.1 400 Bad Request\r\n\r\nsecret".to_vec(),
                NOT_OBFS,
            ),
            (ObfsMode::Http, vec![b'a'; 9000], NOT_OBFS),
            (ObfsMode::Http, b"HTTP/1.1 101 OK\r\nsec".to_vec(), NOT_OBFS),
            (
                ObfsMode::Tls,
                b"HTTP/1.1 200 OK\r\n\r\nsecret".to_vec(),
                NOT_OBFS,
            ),
            (ObfsMode::Tls, vec![0x16, 0x03, 0x01, 0x40, 0x01], NOT_OBFS),
            (
                ObfsMode::Tls,
                vec![0x16, 0x03, 0x01, 0x00, 0x5b, 0x02],
                NOT_OBFS,
            ),
            (ObfsMode::Tls, after_hello, MALFORMED),
            (ObfsMode::Tls, cut, CUT_SHORT),
        ] {
            let (mut stream, mut server) = pipe(mode, 64 * 1024);
            server.write_all(&wire).await.unwrap();
            server.shutdown().await.unwrap();
            let mut got = Vec::new();
            let err = stream.read_to_end(&mut got).await.expect_err("not obfs");
            assert_eq!(err.to_string(), expected, "{mode:?}");
            assert!(!got.windows(3).any(|w| w == b"sec"), "{mode:?}");
        }
    }

    #[tokio::test]
    async fn a_server_that_closes_before_a_word_is_a_plain_eof() {
        for mode in BOTH {
            let (mut stream, mut server) = pipe(mode, 1024);
            server.shutdown().await.unwrap();
            let mut got = Vec::new();
            assert_eq!(stream.read_to_end(&mut got).await.unwrap(), 0, "{mode:?}");
        }
    }

    /// Accepts one connection, echoes through the fake, returns the hello.
    async fn fake_echo(mode: ObfsMode) -> (SocketAddr, tokio::task::JoinHandle<ObfsHello>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let (stream, hello) = accept_obfs(Box::new(tcp), mode).await.unwrap();
            let (mut rd, mut wr) = tokio::io::split(stream);
            tokio::io::copy(&mut rd, &mut wr).await.unwrap();
            wr.shutdown().await.unwrap();
            hello
        });
        (addr, task)
    }

    #[tokio::test]
    async fn round_trips_through_the_fake_server_with_a_half_close() {
        tokio::time::timeout(Duration::from_secs(30), async {
            for mode in BOTH {
                let (addr, fake) = fake_echo(mode).await;
                let tcp = TcpStream::connect(addr).await.unwrap();
                let stream = client(mode).wrap(Box::new(tcp));
                let payload = pattern(1 << 20);
                let (mut rd, mut wr) = tokio::io::split(stream);
                let to_send = payload.clone();
                let writer = tokio::spawn(async move {
                    for chunk in to_send.chunks(50_000) {
                        wr.write_all(chunk).await.unwrap();
                    }
                    wr.shutdown().await.unwrap();
                });
                let mut back = Vec::new();
                rd.read_to_end(&mut back).await.unwrap();
                writer.await.unwrap();
                assert!(back == payload, "{mode:?}: the echo differs");
                let hello = fake.await.unwrap();
                let first = match mode {
                    ObfsMode::Http => {
                        assert_eq!(hello.host, "cdn.example:8388");
                        assert_eq!(hello.uri.as_deref(), Some("/path"));
                        assert!(hello.user_agent.unwrap().starts_with("curl/7."));
                        16384
                    }
                    ObfsMode::Tls => {
                        assert_eq!(hello.host, "cdn.example");
                        tls::max_first_payload("cdn.example")
                    }
                };
                assert!(hello.first_payload == payload[..first], "{mode:?}");
            }
        })
        .await
        .expect("the round trips finished within the bound");
    }
}
```

新建 `crates/rurge-proto/src/transport/obfs/http.rs`：

```rust
//! `obfs=http`: the first client packet is an HTTP upgrade request with the
//! first payload as its body; the server's first packet starts with an
//! upgrade answer. Everything else is raw (phase 2 M6 design 3.2). The
//! template is our own, written from the protocol facts (M6-D3).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use std::sync::OnceLock;

/// The longest server answer head read before giving up.
pub(super) const MAX_RESPONSE_HEAD: usize = 8 * 1024;

/// `curl/7.<a>.<b>`: chosen once per process (`a` in 0..=50, `b` in 0..=1)
/// and reused by every connection, as the reference client does.
fn user_agent() -> &'static str {
    static AGENT: OnceLock<String> = OnceLock::new();
    AGENT.get_or_init(|| {
        let mut pick = [0u8; 2];
        // camouflage, not a secret: a fixed version is still a valid one
        let _ = getrandom::fill(&mut pick);
        format!("curl/7.{}.{}", pick[0] % 51, pick[1] % 2)
    })
}

/// The request head for a first payload of `len` bytes. `host` already
/// carries `:port` when the port is not 80.
pub(super) fn request_head(uri: &str, host: &str, len: usize) -> Vec<u8> {
    let mut key = [0u8; 16];
    let _ = getrandom::fill(&mut key);
    format!(
        "GET {uri} HTTP/1.1\r\n\
         Host: {host}\r\n\
         User-Agent: {}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {}\r\n\
         Content-Length: {len}\r\n\
         \r\n",
        user_agent(),
        STANDARD.encode(key)
    )
    .into_bytes()
}

/// Where the server's answer head ends (the index just past `\r\n\r\n`).
pub(super) fn head_end(data: &[u8]) -> Option<usize> {
    data.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
}

/// An upgrade (`101`) or any success; the reference client checks nothing,
/// but anything else is not an obfs server (a plain web server answering
/// `400` or `404`, say).
pub(super) fn is_upgrade_answer(head: &[u8]) -> bool {
    let line = head.split(|&b| b == b'\r').next().unwrap_or_default();
    let mut parts = line.splitn(3, |&b| b == b' ');
    let version = parts.next().unwrap_or_default();
    let code = parts.next().unwrap_or_default();
    let code = std::str::from_utf8(code)
        .ok()
        .and_then(|c| c.parse::<u16>().ok());
    version.starts_with(b"HTTP/1.") && matches!(code, Some(101 | 200..=299))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_head_has_the_facts_lines_in_order() {
        let head = String::from_utf8(request_head("/cdn?x=1", "edge.example:8388", 42)).unwrap();
        let lines: Vec<&str> = head.split("\r\n").collect();
        assert_eq!(lines[0], "GET /cdn?x=1 HTTP/1.1");
        assert_eq!(lines[1], "Host: edge.example:8388");
        let agent = lines[2].strip_prefix("User-Agent: curl/7.").unwrap();
        let (a, b) = agent.split_once('.').unwrap();
        assert!(a.parse::<u8>().unwrap() <= 50 && b.parse::<u8>().unwrap() <= 1);
        assert_eq!(lines[3], "Upgrade: websocket");
        assert_eq!(lines[4], "Connection: Upgrade");
        let key = lines[5].strip_prefix("Sec-WebSocket-Key: ").unwrap();
        assert_eq!(key.len(), 24);
        assert_eq!(STANDARD.decode(key).unwrap().len(), 16);
        assert_eq!(lines[6], "Content-Length: 42");
        assert_eq!(&lines[7..], ["", ""], "a blank line ends the head");
        // the agent is fixed for the process, the key is fresh every time
        let again = String::from_utf8(request_head("/", "h", 0)).unwrap();
        assert!(again.contains(lines[2]));
        assert!(!again.contains(lines[5]));
    }

    #[test]
    fn the_answer_head_ends_at_the_first_blank_line() {
        assert_eq!(head_end(b"HTTP/1.1 101 OK\r\n\r\nrest"), Some(19));
        assert_eq!(head_end(b"HTTP/1.1 101 OK\r\n\r"), None);
    }

    #[test]
    fn only_an_upgrade_or_a_success_is_an_obfs_answer() {
        for ok in [
            &b"HTTP/1.1 101 Switching Protocols\r\n\r\n"[..],
            b"HTTP/1.0 200 OK\r\n\r\n",
            b"HTTP/1.1 204\r\n\r\n",
        ] {
            assert!(is_upgrade_answer(ok), "{}", String::from_utf8_lossy(ok));
        }
        for no in [
            &b"HTTP/1.1 400 Bad Request\r\n\r\n"[..],
            b"HTTP/1.1 302 Found\r\n\r\n",
            b"SSH-2.0-OpenSSH\r\n\r\n",
            b"HTTP/1.1 1O1 x\r\n\r\n",
            b"\r\n\r\n",
        ] {
            assert!(!is_upgrade_answer(no), "{}", String::from_utf8_lossy(no));
        }
    }
}
```

新建 `crates/rurge-proto/src/transport/obfs/tls.rs`：

```rust
//! `obfs=tls`: the first client packet is a fake TLS 1.2 ClientHello with the
//! first payload in its session ticket; the server answers with a fake
//! ServerHello, a ChangeCipherSpec and a handshake record carrying its first
//! data; every later packet either way is an application data record (phase
//! 2 M6 design 3.2). The template is our own, written from the protocol
//! facts (M6-D3): nothing here is a real TLS handshake.

use std::time::{SystemTime, UNIX_EPOCH};

/// The largest record body either side accepts (the reference server
/// refuses a longer one).
pub(super) const MAX_RECORD: usize = 16 * 1024;
pub(super) const HEADER_LEN: usize = 5;

pub(super) const HANDSHAKE: u8 = 0x16;
pub(super) const CHANGE_CIPHER_SPEC: u8 = 0x14;
pub(super) const APPLICATION_DATA: u8 = 0x17;

/// The hello's size without the ticket and the host name.
pub(super) const HELLO_OVERHEAD: usize = 217;

/// What the reference server reads for the whole hello: the first payload
/// is cut to fit in it.
pub(super) fn max_first_payload(host: &str) -> usize {
    MAX_RECORD - HELLO_OVERHEAD - host.len()
}

/// 28 suites, the last one the renegotiation SCSV.
const CIPHER_SUITES: [u16; 28] = [
    0xc02c, 0xc030, 0x009f, 0xcca9, 0xcca8, 0xccaa, 0xc02b, 0xc02f, 0x009e, 0xc024, 0xc028, 0x006b,
    0xc023, 0xc027, 0x0067, 0xc00a, 0xc014, 0x0039, 0xc009, 0xc013, 0x0033, 0x009d, 0x009c, 0x003d,
    0x003c, 0x0035, 0x002f, 0x00ff,
];

/// ec_point_formats, supported_groups, signature_algorithms, encrypt_then_mac
/// and extended_master_secret: the extensions after the server name, fixed.
const TAIL_EXTENSIONS: [u8; 66] = [
    0x00, 0x0b, 0x00, 0x04, 0x03, 0x01, 0x00, 0x02, // ec_point_formats
    0x00, 0x0a, 0x00, 0x0a, 0x00, 0x08, 0x00, 0x1d, 0x00, 0x17, 0x00, 0x19, 0x00,
    0x18, // supported_groups: x25519, secp256r1, secp521r1, secp384r1
    0x00, 0x0d, 0x00, 0x20, 0x00, 0x1e, 0x06, 0x01, 0x06, 0x02, 0x06, 0x03, 0x05, 0x01, 0x05, 0x02,
    0x05, 0x03, 0x04, 0x01, 0x04, 0x02, 0x04, 0x03, 0x03, 0x01, 0x03, 0x02, 0x03, 0x03, 0x02, 0x01,
    0x02, 0x02, 0x02, 0x03, // signature_algorithms
    0x00, 0x16, 0x00, 0x00, // encrypt_then_mac
    0x00, 0x17, 0x00, 0x00, // extended_master_secret
];

fn put_u16(out: &mut Vec<u8>, n: usize) {
    out.extend_from_slice(&(n as u16).to_be_bytes());
}

/// The first packet: `ticket` is the first payload (at most
/// `max_first_payload(host)` bytes), `host` the server name.
pub(super) fn client_hello(host: &str, ticket: &[u8]) -> Vec<u8> {
    let total = HELLO_OVERHEAD + ticket.len() + host.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&[HANDSHAKE, 0x03, 0x01]);
    put_u16(&mut out, total - HEADER_LEN);
    // ClientHello, a 24-bit length whose top byte is always 0 here
    out.extend_from_slice(&[0x01, 0x00]);
    put_u16(&mut out, total - HEADER_LEN - 4);
    out.extend_from_slice(&[0x03, 0x03]);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or_default();
    out.extend_from_slice(&now.to_be_bytes());
    // 28 random bytes, then a 32-byte session id: camouflage, not secrets
    let mut random = [0u8; 28 + 32];
    let _ = getrandom::fill(&mut random);
    out.extend_from_slice(&random[..28]);
    out.push(32);
    out.extend_from_slice(&random[28..]);
    put_u16(&mut out, CIPHER_SUITES.len() * 2);
    for suite in CIPHER_SUITES {
        out.extend_from_slice(&suite.to_be_bytes());
    }
    // one compression method: null
    out.extend_from_slice(&[0x01, 0x00]);
    put_u16(&mut out, total - 138);
    // session_ticket
    out.extend_from_slice(&[0x00, 0x23]);
    put_u16(&mut out, ticket.len());
    out.extend_from_slice(ticket);
    // server_name: one host_name entry
    out.extend_from_slice(&[0x00, 0x00]);
    put_u16(&mut out, host.len() + 5);
    put_u16(&mut out, host.len() + 3);
    out.push(0x00);
    put_u16(&mut out, host.len());
    out.extend_from_slice(host.as_bytes());
    out.extend_from_slice(&TAIL_EXTENSIONS);
    debug_assert_eq!(out.len(), total);
    out
}

/// Appends one application data record carrying `data` (at most
/// `MAX_RECORD` bytes).
pub(super) fn app_data(out: &mut Vec<u8>, data: &[u8]) {
    out.extend_from_slice(&[APPLICATION_DATA, 0x03, 0x03]);
    put_u16(out, data.len());
    out.extend_from_slice(data);
}

/// A record header the client accepts: the expected type, a TLS 1.x
/// version and a body of at most `MAX_RECORD` bytes. Returns the body length.
pub(super) fn record_len(header: &[u8; HEADER_LEN], kind: u8) -> Option<usize> {
    let len = usize::from(u16::from_be_bytes([header[3], header[4]]));
    (header[0] == kind
        && header[1] == 0x03
        && (0x01..=0x04).contains(&header[2])
        && len <= MAX_RECORD)
        .then_some(len)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u16_at(data: &[u8], at: usize) -> usize {
        usize::from(u16::from_be_bytes([data[at], data[at + 1]]))
    }

    #[test]
    fn the_hello_has_the_facts_layout() {
        let host = "cdn.example";
        let ticket: Vec<u8> = (0..300u32).map(|i| i as u8).collect();
        let hello = client_hello(host, &ticket);
        let (p, h) = (ticket.len(), host.len());
        assert_eq!(hello.len(), 217 + p + h);
        assert_eq!(&hello[..3], &[0x16, 0x03, 0x01]);
        assert_eq!(u16_at(&hello, 3), hello.len() - 5);
        assert_eq!(&hello[5..7], &[0x01, 0x00]);
        assert_eq!(u16_at(&hello, 7), hello.len() - 9);
        assert_eq!(&hello[9..11], &[0x03, 0x03]);
        assert_eq!(hello[43], 32, "a 32-byte session id");
        assert_eq!(u16_at(&hello, 76), 0x38);
        assert_eq!(&hello[78..80], &[0xc0, 0x2c]);
        assert_eq!(
            &hello[132..134],
            &[0x00, 0xff],
            "the SCSV is the last suite"
        );
        assert_eq!(&hello[134..136], &[0x01, 0x00]);
        assert_eq!(u16_at(&hello, 136), 79 + p + h);
        // session_ticket first, carrying the payload verbatim
        assert_eq!(&hello[138..140], &[0x00, 0x23]);
        assert_eq!(u16_at(&hello, 140), p);
        assert_eq!(&hello[142..142 + p], &ticket[..]);
        // then server_name
        let sni = 142 + p;
        assert_eq!(&hello[sni..sni + 2], &[0x00, 0x00]);
        assert_eq!(u16_at(&hello, sni + 2), h + 5);
        assert_eq!(u16_at(&hello, sni + 4), h + 3);
        assert_eq!(hello[sni + 6], 0);
        assert_eq!(u16_at(&hello, sni + 7), h);
        assert_eq!(&hello[sni + 9..sni + 9 + h], host.as_bytes());
        // then the fixed extensions, in order, and nothing after them
        let mut at = sni + 9 + h;
        for (kind, len) in [
            (0x000b, 4),
            (0x000a, 10),
            (0x000d, 32),
            (0x0016, 0),
            (0x0017, 0),
        ] {
            assert_eq!(u16_at(&hello, at), kind);
            assert_eq!(u16_at(&hello, at + 2), len);
            at += 4 + len;
        }
        assert_eq!(at, hello.len());
    }

    #[test]
    fn an_empty_ticket_and_the_largest_one_fit() {
        let hello = client_hello("h", &[]);
        assert_eq!(hello.len(), 218);
        assert_eq!(u16_at(&hello, 140), 0);
        let big = vec![7u8; max_first_payload("h")];
        assert_eq!(client_hello("h", &big).len(), MAX_RECORD);
    }

    #[test]
    fn records_are_checked_by_type_version_and_length() {
        let mut out = Vec::new();
        app_data(&mut out, b"abc");
        assert_eq!(out, [0x17, 0x03, 0x03, 0x00, 0x03, b'a', b'b', b'c']);
        let header = |b: [u8; 5]| b;
        assert_eq!(
            record_len(&header([0x17, 3, 3, 0x40, 0]), 0x17),
            Some(16384)
        );
        assert_eq!(record_len(&header([0x16, 3, 1, 0, 0x5b]), 0x16), Some(91));
        assert_eq!(record_len(&header([0x17, 3, 3, 0x40, 1]), 0x17), None);
        assert_eq!(record_len(&header([0x16, 3, 3, 0, 1]), 0x17), None);
        assert_eq!(record_len(&header([0x17, 2, 0, 0, 1]), 0x17), None);
    }
}
```

`crates/rurge-proto/src/transport/mod.rs`——把

```rust
pub mod lazy_head;
```

换成

```rust
pub mod lazy_head;
pub mod obfs;
```

`Stack` 的一层：

`crates/rurge-proto/src/transport/stack.rs`——把

```rust
//! (phase 2 design §5.4): connect → shadow-tls → tls → ws.

use crate::OutboundError;
```

换成

```rust
//! (phase 2 design §5.4, phase 2 M6 design 3.2): connect → shadow-tls →
//! obfs → tls → ws. No protocol combines obfs with tls or ws.

use crate::OutboundError;
use crate::transport::obfs::ObfsClient;
```

`crates/rurge-proto/src/transport/stack.rs`——把

```rust
    shadow_tls: Option<ShadowTlsClient>,
    tls: Option<TlsClient>,
```

换成

```rust
    shadow_tls: Option<ShadowTlsClient>,
    obfs: Option<ObfsClient>,
    tls: Option<TlsClient>,
```

`crates/rurge-proto/src/transport/stack.rs`——把

```rust
            shadow_tls,
```

换成

```rust
            shadow_tls,
            obfs: None,
```

`crates/rurge-proto/src/transport/stack.rs`——把

```rust
            ws,
        }
    }
```

换成

```rust
            ws,
        }
    }

    /// Adds the simple-obfs layer (right above Shadow TLS).
    pub fn with_obfs(mut self, obfs: ObfsClient) -> Stack {
        self.obfs = Some(obfs);
        self
    }
```

`crates/rurge-proto/src/transport/stack.rs`——把

```rust
            stream = shadow_tls.wrap(stream).await?;
```

换成

```rust
            stream = shadow_tls.wrap(stream).await?;
        }
        if let Some(obfs) = &self.obfs {
            stream = obfs.wrap(stream);
```

要点：
- 写的成功只在整个包都交给了下层之后才报告（同 `WsByteStream`）；被 flush 推完的包报告给重试的那次写，不重新编码。
- `tls` 此后每次写一条记录，不超过 16384 字节；shutdown 原样传下去，不加 close_notify。
- 随机数（`Sec-WebSocket-Key`、hello 的随机数与 session id、User-Agent 的版本号）用 `getrandom::fill` 且忽略失败：这是伪装，不是秘密（同 shadow-tls 的填充）。
- 写不进伪装的 `obfs-host` / `obfs-uri` / 服务器名是 `BuildError`，文字不引用取值。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-proto --lib obfs` → 通过（`transport::obfs::http` 3 条、`transport::obfs::tls` 3 条、`transport::obfs::tests` 9 条，含经回环假服务端的 1 MiB 往返与半关闭；`transport::stack::tests::obfs_sits_on_the_connection`）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-proto
git commit -m "feat(proto): simple-obfs 层（http / tls）——Stack 的一层与假服务端一侧"
```

### Task 3: AEAD 与 `none` 的 TCP

`rurge_proto::shadowsocks`：方法与 AEAD 原语、口令与子密钥派生（`EVP_BytesToKey(MD5)`、HKDF-SHA1 `ss-subkey`）、分块流（负载每块不超过 0x3FFF、小端计数 nonce），`ShadowsocksOutbound` 的 TCP（P8、P9）。`FakeShadowsocks` 的解码与编码独立实现（只共用经向量测试过的原语与 KDF）。2022 方法在本任务里构建时报 `BuildError`（Task 4 去掉），UDP 仍是默认的 `Unsupported`（Task 5）。

**Files:**
- Create: `crates/rurge-proto/src/shadowsocks/mod.rs`、`cipher.rs`、`kdf.rs`、`aead.rs`（均自带用例）、`crates/rurge-proto/src/testing/shadowsocks.rs`
- Modify: `Cargo.toml`（工作区依赖）、`crates/rurge-proto/Cargo.toml`、`src/lib.rs`、`src/testing/mod.rs`

**Interfaces:**
- Consumes: Task 1 的 `SsSpec` / `SsMethod`；Task 2 的 `ObfsClient` / `Stack::with_obfs` / `accept_obfs`；既有的 `crate::addr::socks_addr`、`transport::lazy_head::LazyHead`、`Outbound`、`ShadowTlsOpts`。
- Produces:
  - `cipher.rs`：`pub(crate) const TAG: usize = 16`；`pub(crate) enum AeadKind`（`of(SsMethod) -> Option<AeadKind>`、`key_len()`、`nonce_len()`）；`pub(crate) enum AeadCipher`（`new(kind, key)`、`seal_in_place(&self, nonce, data) -> [u8; TAG]`、`open_in_place(&self, nonce, data, tag) -> bool`）；`pub(crate) struct CountingAead`（`new`、`seal(&mut self, plain, out: &mut Vec<u8>)`、`open(&mut self, sealed: &mut [u8]) -> Option<usize>`）；`pub(crate) struct MasterKey`（`from_password(kind, &str)`、`salt_len()`、`session(&self, salt) -> CountingAead`）
  - `kdf.rs`：`evp_bytes_to_key(password: &[u8], len) -> Vec<u8>`、`session_subkey(master, salt) -> Vec<u8>`
  - `aead.rs`：`pub(crate) const MAX_PAYLOAD: usize = 0x3FFF`、`seal_chunk`、`pub(crate) struct AeadStream`（`new(inner, key: Arc<MasterKey>, salt, max_payload)`）
  - `pub struct ShadowsocksOutbound`：`new(name: &str, server: Target, spec: &SsSpec, shadow_tls: Option<&ShadowTlsOpts>, roots: Arc<RootCertStore>, connector: Arc<dyn Connector>) -> Result<ShadowsocksOutbound, BuildError>`
  - 测试设施：`ShadowsocksScript { method, password, obfs: Option<ObfsMode>, connect_to }`（`ShadowsocksScript::new(method, &str)`）、`RecordedShadowsocks`、`FakeShadowsocks::spawn(script)` / `addr()` / `requests()` / `obfs_seen()` / `answer_salts()` / `largest_chunk()` / `connections()` / `rejected()`

- [ ] **Step 1: 先写假服务端**

假服务端独立实现（自己的分块、nonce 计数与地址解析），口令错时像真实服务端一样不作声地关掉：

新建 `crates/rurge-proto/src/testing/shadowsocks.rs`：

```rust
//! A scriptable Shadowsocks server: optionally simple-obfs in front, then
//! the AEAD stream (or, with `none`, the bytes as they are), the request
//! header, and a relay. Its framing, nonce counting and address parsing are
//! written apart from the client's (`crate::shadowsocks`), so each checks the
//! other; only the primitives (the ciphers and the key derivation, which
//! have vectors of their own) are shared. It never resolves a name.
//!
//! As real servers do, it never tells a client with a wrong password so:
//! it stops reading into the stream, closes its side and waits for the
//! client to go away.

use super::AbortOnDrop;
use super::obfs::{ObfsHello, accept_obfs};
use crate::shadowsocks::cipher::{AeadCipher, AeadKind, TAG};
use crate::shadowsocks::kdf;
use rurge_config::spec::{ObfsMode, SsMethod};
use rurge_net::connector::BoxedStream;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// The largest payload a client chunk may carry.
const MAX_CHUNK: usize = 0x3FFF;

#[derive(Clone, Debug)]
pub struct ShadowsocksScript {
    pub method: SsMethod,
    pub password: String,
    /// Expect this simple-obfs camouflage in front of the protocol.
    pub obfs: Option<ObfsMode>,
    /// Relay here whatever the client asked for (needed for a domain target).
    pub connect_to: Option<SocketAddr>,
}

impl ShadowsocksScript {
    pub fn new(method: SsMethod, password: &str) -> ShadowsocksScript {
        ShadowsocksScript {
            method,
            password: password.to_string(),
            obfs: None,
            connect_to: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedShadowsocks {
    /// The client's salt; empty for `none`.
    pub salt: Vec<u8>,
    pub atyp: u8,
    /// An IP literal, or the name exactly as it was on the wire.
    pub host: String,
    pub port: u16,
    /// Payload after the address: the rest of the first chunk (`none`: of
    /// the read that completed the address).
    pub early: Vec<u8>,
}

pub struct FakeShadowsocks {
    addr: SocketAddr,
    shared: Arc<Shared>,
    connections: Arc<AtomicUsize>,
    _task: AbortOnDrop,
}

#[derive(Default)]
struct Seen {
    requests: Mutex<Vec<RecordedShadowsocks>>,
    obfs: Mutex<Vec<ObfsHello>>,
    answer_salts: Mutex<Vec<Vec<u8>>>,
    rejected: AtomicUsize,
    largest_chunk: AtomicUsize,
}

struct Shared {
    script: ShadowsocksScript,
    seen: Seen,
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("fake ss: {what}"))
}

/// One direction's cipher, its nonce a little-endian counter from zero.
struct Direction {
    cipher: AeadCipher,
    count: u128,
    nonce_len: usize,
}

impl Direction {
    fn new(kind: AeadKind, password: &str, salt: &[u8]) -> Direction {
        let master = kdf::evp_bytes_to_key(password.as_bytes(), kind.key_len());
        Direction {
            cipher: AeadCipher::new(kind, &kdf::session_subkey(&master, salt)),
            count: 0,
            nonce_len: kind.nonce_len(),
        }
    }

    fn nonce(&mut self) -> [u8; 24] {
        let mut nonce = [0u8; 24];
        nonce[..16].copy_from_slice(&self.count.to_le_bytes());
        self.count += 1;
        nonce
    }

    fn seal(&mut self, plain: &[u8], out: &mut Vec<u8>) {
        let nonce = self.nonce();
        let mut data = plain.to_vec();
        let tag = self
            .cipher
            .seal_in_place(&nonce[..self.nonce_len], &mut data);
        out.extend_from_slice(&data);
        out.extend_from_slice(&tag);
    }

    fn open(&mut self, sealed: &[u8]) -> Option<Vec<u8>> {
        let nonce = self.nonce();
        let (data, tag) = sealed.split_at(sealed.len().checked_sub(TAG)?);
        let mut data = data.to_vec();
        let tag: [u8; TAG] = tag.try_into().ok()?;
        self.cipher
            .open_in_place(&nonce[..self.nonce_len], &mut data, &tag)
            .then_some(data)
    }
}

/// `Ok(false)`: the peer closed before the first byte.
async fn read_full<R: AsyncRead + Unpin>(r: &mut R, buf: &mut [u8]) -> io::Result<bool> {
    if r.read(&mut buf[..1]).await? == 0 {
        return Ok(false);
    }
    r.read_exact(&mut buf[1..]).await?;
    Ok(true)
}

/// The next chunk's payload; `Ok(None)` when the client closed between
/// chunks. An error when it does not authenticate or is too long.
async fn read_chunk<R: AsyncRead + Unpin>(
    r: &mut R,
    up: &mut Direction,
    seen: &Seen,
) -> io::Result<Option<Vec<u8>>> {
    let mut sealed_len = [0u8; 2 + TAG];
    if !read_full(r, &mut sealed_len).await? {
        return Ok(None);
    }
    let len = up
        .open(&sealed_len)
        .ok_or_else(|| bad("a length that does not authenticate"))?;
    let len = usize::from(u16::from_be_bytes([len[0], len[1]]));
    if len > MAX_CHUNK {
        return Err(bad("a chunk over 0x3FFF bytes"));
    }
    seen.largest_chunk.fetch_max(len, Ordering::SeqCst);
    let mut body = vec![0u8; len + TAG];
    r.read_exact(&mut body).await?;
    up.open(&body)
        .map(Some)
        .ok_or_else(|| bad("a payload that does not authenticate"))
}

/// `ATYP ADDR PORT` at the start of `buf`: the request and the bytes it
/// took, once whole.
fn parse_address(buf: &[u8]) -> Option<(RecordedShadowsocks, usize)> {
    let atyp = *buf.first()?;
    let (host, used) = match atyp {
        1 => {
            let b: [u8; 4] = buf.get(1..5)?.try_into().ok()?;
            (IpAddr::V4(Ipv4Addr::from(b)).to_string(), 5)
        }
        4 => {
            let b: [u8; 16] = buf.get(1..17)?.try_into().ok()?;
            (IpAddr::V6(Ipv6Addr::from(b)).to_string(), 17)
        }
        3 => {
            let len = usize::from(*buf.get(1)?);
            let name = buf.get(2..2 + len)?;
            (String::from_utf8_lossy(name).into_owned(), 2 + len)
        }
        _ => return None,
    };
    let port = u16::from_be_bytes(buf.get(used..used + 2)?.try_into().ok()?);
    Some((
        RecordedShadowsocks {
            salt: Vec::new(),
            atyp,
            host,
            port,
            early: buf[used + 2..].to_vec(),
        },
        used + 2,
    ))
}

impl Shared {
    fn record(&self, request: &RecordedShadowsocks) {
        self.seen
            .requests
            .lock()
            .expect("requests")
            .push(request.clone());
    }

    /// Where to relay a request; `None`: a name without `connect_to`.
    fn upstream(&self, request: &RecordedShadowsocks) -> Option<SocketAddr> {
        self.script.connect_to.or_else(|| {
            let ip = request.host.parse::<IpAddr>().ok()?;
            Some(SocketAddr::new(ip, request.port))
        })
    }

    /// A client it cannot understand: no word back, only the close.
    async fn reject(&self, mut stream: BoxedStream) -> io::Result<()> {
        self.seen.rejected.fetch_add(1, Ordering::SeqCst);
        stream.shutdown().await?;
        // read until the client goes: closing with unread data would reset
        let mut sink = [0u8; 4096];
        while stream.read(&mut sink).await? > 0 {}
        Ok(())
    }
}

async fn serve(tcp: TcpStream, shared: Arc<Shared>) -> io::Result<()> {
    let mut stream: BoxedStream = Box::new(tcp);
    if let Some(mode) = shared.script.obfs {
        let (inner, hello) = accept_obfs(stream, mode).await?;
        shared.seen.obfs.lock().expect("obfs").push(hello);
        stream = inner;
    }
    match AeadKind::of(shared.script.method) {
        None => serve_plain(stream, &shared).await,
        Some(kind) => serve_aead(stream, kind, &shared).await,
    }
}

async fn serve_plain(mut stream: BoxedStream, shared: &Shared) -> io::Result<()> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let request = loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some((request, _)) = parse_address(&buf) {
            break request;
        }
    };
    shared.record(&request);
    let Some(to) = shared.upstream(&request) else {
        return stream.shutdown().await;
    };
    let mut upstream = TcpStream::connect(to).await?;
    upstream.write_all(&request.early).await?;
    let _ = tokio::io::copy_bidirectional(&mut stream, &mut upstream).await;
    Ok(())
}

async fn serve_aead(mut stream: BoxedStream, kind: AeadKind, shared: &Shared) -> io::Result<()> {
    let password = shared.script.password.as_str();
    let mut salt = vec![0u8; kind.key_len()];
    stream.read_exact(&mut salt).await?;
    let mut up = Direction::new(kind, password, &salt);
    let first = match read_chunk(&mut stream, &mut up, &shared.seen).await {
        Ok(Some(first)) => first,
        Ok(None) => return Ok(()),
        Err(_) => return shared.reject(stream).await,
    };
    // the address rides in the first chunk
    let Some((mut request, _)) = parse_address(&first) else {
        return shared.reject(stream).await;
    };
    request.salt = salt;
    shared.record(&request);
    let Some(to) = shared.upstream(&request) else {
        return stream.shutdown().await;
    };
    let mut upstream = TcpStream::connect(to).await?;
    upstream.write_all(&request.early).await?;
    let (mut client_read, mut client_write) = tokio::io::split(stream);
    let (mut target_read, mut target_write) = upstream.into_split();
    let requests = async {
        while let Ok(Some(payload)) = read_chunk(&mut client_read, &mut up, &shared.seen).await {
            if target_write.write_all(&payload).await.is_err() {
                return;
            }
        }
        let _ = target_write.shutdown().await;
    };
    let answers = async {
        let mut answer_salt = vec![0u8; kind.key_len()];
        getrandom::fill(&mut answer_salt).expect("randomness");
        let mut down = Direction::new(kind, password, &answer_salt);
        // the salt goes out with the first payload, never alone
        let mut pending_salt = Some(answer_salt);
        let mut buf = vec![0u8; MAX_CHUNK];
        loop {
            let n = match target_read.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let mut frame = Vec::with_capacity(n + 64);
            if let Some(salt) = pending_salt.take() {
                shared
                    .seen
                    .answer_salts
                    .lock()
                    .expect("salts")
                    .push(salt.clone());
                frame.extend_from_slice(&salt);
            }
            down.seal(&(n as u16).to_be_bytes(), &mut frame);
            down.seal(&buf[..n], &mut frame);
            if client_write.write_all(&frame).await.is_err() {
                return;
            }
        }
        let _ = client_write.shutdown().await;
    };
    tokio::join!(requests, answers);
    Ok(())
}

impl FakeShadowsocks {
    pub async fn spawn(script: ShadowsocksScript) -> FakeShadowsocks {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let shared = Arc::new(Shared {
            script,
            seen: Seen::default(),
        });
        let connections = Arc::new(AtomicUsize::new(0));
        let (count, serving) = (connections.clone(), shared.clone());
        let task = tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                count.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(serve(tcp, serving.clone()));
            }
        });
        FakeShadowsocks {
            addr,
            shared,
            connections,
            _task: AbortOnDrop(task),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn requests(&self) -> Vec<RecordedShadowsocks> {
        self.shared.seen.requests.lock().expect("requests").clone()
    }

    /// What each connection's camouflage said, when the script expects one.
    pub fn obfs_seen(&self) -> Vec<ObfsHello> {
        self.shared.seen.obfs.lock().expect("obfs").clone()
    }

    /// The salts of the server's own streams, in the order they started.
    pub fn answer_salts(&self) -> Vec<Vec<u8>> {
        self.shared.seen.answer_salts.lock().expect("salts").clone()
    }

    /// The longest payload of any client chunk so far.
    pub fn largest_chunk(&self) -> usize {
        self.shared.seen.largest_chunk.load(Ordering::SeqCst)
    }

    /// TCP connections accepted so far.
    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    /// Connections given no answer because they did not decrypt.
    pub fn rejected(&self) -> usize {
        self.shared.seen.rejected.load(Ordering::SeqCst)
    }
}
```

`crates/rurge-proto/src/testing/mod.rs`——把

```rust
mod shadow_tls;
```

换成

```rust
mod shadow_tls;
mod shadowsocks;
```

`crates/rurge-proto/src/testing/mod.rs`——把

```rust
};
pub use socks5::{FakeSocks5, RecordedSocks5, Socks5Script};
```

换成

```rust
};
pub use shadowsocks::{FakeShadowsocks, RecordedShadowsocks, ShadowsocksScript};
pub use socks5::{FakeSocks5, RecordedSocks5, Socks5Script};
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-proto --lib shadowsocks`
Expected: FAIL——`crate::shadowsocks` 由 Step 3 引入，编译不过：

```text
error[E0433]: failed to resolve: unresolved import
  --> crates\rurge-proto\src\testing\shadowsocks.rs:14:12
error[E0432]: unresolved import `crate::shadowsocks`
  --> crates\rurge-proto\src\testing\shadowsocks.rs:15:12
Some errors have detailed explanations: E0432, E0433.
For more information about an error, try `rustc --explain E0432`.
error: could not compile `rurge-proto` (lib test) due to 2 previous errors
exit 101
```

- [ ] **Step 3: 实现**

依赖（P1；锁文件不新增任何包）：

`Cargo.toml`——把

```toml
md-5 = "0.10"
```

换成

```toml
md-5 = "0.10"
# Shadowsocks AEAD: AES-GCM of all three key sizes (aes-192-gcm has no type
# alias), ChaCha20- and XChaCha20-Poly1305 at the version already in the tree,
# HKDF-SHA1 for the session subkey
aes-gcm = { version = "0.11.1", default-features = false, features = ["aes"] }
chacha20poly1305 = { version = "0.10.1", default-features = false }
hkdf = "0.13"
sha1 = { version = "0.11", default-features = false }
```

`crates/rurge-proto/Cargo.toml`——把

```toml
md-5.workspace = true
```

换成

```toml
md-5.workspace = true
aes-gcm.workspace = true
chacha20poly1305.workspace = true
hkdf.workspace = true
sha1.workspace = true
```

`crates/rurge-proto/src/lib.rs`——把

```rust
pub mod reject;
```

换成

```rust
pub mod reject;
pub mod shadowsocks;
```

新模块（自带用例；已知答案由 Python 独立算出）：

新建 `crates/rurge-proto/src/shadowsocks/cipher.rs`：

```rust
//! The AEAD constructions of Shadowsocks (phase 2 M6 design 3.3): which
//! cipher a method uses, the cipher itself, and the counting nonce of a
//! stream. No associated data anywhere in the protocol.

use super::kdf;
use aes_gcm::aead::consts::U12;
use aes_gcm::aes::Aes192;
use aes_gcm::{AeadInOut, Aes128Gcm, Aes256Gcm, AesGcm, KeyInit};
use chacha20poly1305::aead::generic_array::GenericArray;
// the ChaCha ciphers are of the older `aead` generation, with traits of their own
use chacha20poly1305::{AeadInPlace, ChaCha20Poly1305, KeyInit as _, XChaCha20Poly1305};
use rurge_config::spec::SsMethod;

/// Every method's tag is 16 bytes.
pub(crate) const TAG: usize = 16;

/// The longest nonce (XChaCha20's).
const MAX_NONCE: usize = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AeadKind {
    Aes128Gcm,
    Aes192Gcm,
    Aes256Gcm,
    ChaCha20Poly1305,
    XChaCha20Poly1305,
}

impl AeadKind {
    /// `None` for `none`, which encrypts nothing. SS 2022 names the AES-GCM
    /// it seals with.
    pub(crate) fn of(method: SsMethod) -> Option<AeadKind> {
        match method {
            SsMethod::None => None,
            SsMethod::Aes128Gcm | SsMethod::Blake3Aes128Gcm => Some(AeadKind::Aes128Gcm),
            SsMethod::Aes192Gcm => Some(AeadKind::Aes192Gcm),
            SsMethod::Aes256Gcm | SsMethod::Blake3Aes256Gcm => Some(AeadKind::Aes256Gcm),
            SsMethod::ChaCha20IetfPoly1305 => Some(AeadKind::ChaCha20Poly1305),
            SsMethod::XChaCha20IetfPoly1305 => Some(AeadKind::XChaCha20Poly1305),
        }
    }

    /// The key length, which is also the salt length.
    pub(crate) fn key_len(self) -> usize {
        match self {
            AeadKind::Aes128Gcm => 16,
            AeadKind::Aes192Gcm => 24,
            AeadKind::Aes256Gcm | AeadKind::ChaCha20Poly1305 | AeadKind::XChaCha20Poly1305 => 32,
        }
    }

    pub(crate) fn nonce_len(self) -> usize {
        match self {
            AeadKind::XChaCha20Poly1305 => 24,
            _ => 12,
        }
    }
}

/// One keyed cipher. No `Debug`: it holds a key.
pub(crate) enum AeadCipher {
    Aes128Gcm(Aes128Gcm),
    Aes192Gcm(AesGcm<Aes192, U12>),
    Aes256Gcm(Aes256Gcm),
    ChaCha20Poly1305(ChaCha20Poly1305),
    XChaCha20Poly1305(XChaCha20Poly1305),
}

/// The GCM ciphers take a 12-byte nonce.
fn gcm_nonce(nonce: &[u8]) -> &aes_gcm::Nonce<U12> {
    nonce.try_into().expect("a 12-byte nonce")
}

impl AeadCipher {
    /// `key` is `kind.key_len()` bytes long: the callers derive it so.
    pub(crate) fn new(kind: AeadKind, key: &[u8]) -> AeadCipher {
        const LEN: &str = "a key of the method's length";
        match kind {
            AeadKind::Aes128Gcm => {
                AeadCipher::Aes128Gcm(Aes128Gcm::new_from_slice(key).expect(LEN))
            }
            AeadKind::Aes192Gcm => {
                AeadCipher::Aes192Gcm(AesGcm::<Aes192, U12>::new_from_slice(key).expect(LEN))
            }
            AeadKind::Aes256Gcm => {
                AeadCipher::Aes256Gcm(Aes256Gcm::new_from_slice(key).expect(LEN))
            }
            AeadKind::ChaCha20Poly1305 => {
                AeadCipher::ChaCha20Poly1305(ChaCha20Poly1305::new_from_slice(key).expect(LEN))
            }
            AeadKind::XChaCha20Poly1305 => {
                AeadCipher::XChaCha20Poly1305(XChaCha20Poly1305::new_from_slice(key).expect(LEN))
            }
        }
    }

    /// Encrypts `data` in place and returns its tag. `nonce` is the kind's
    /// nonce length.
    pub(crate) fn seal_in_place(&self, nonce: &[u8], data: &mut [u8]) -> [u8; TAG] {
        // fails only for inputs of gigabytes: a chunk or a datagram is far below
        const SIZE: &str = "a chunk is far below the AEAD's limit";
        let mut tag = [0u8; TAG];
        match self {
            AeadCipher::Aes128Gcm(c) => tag.copy_from_slice(
                &c.encrypt_inout_detached(gcm_nonce(nonce), &[], data.into())
                    .expect(SIZE),
            ),
            AeadCipher::Aes192Gcm(c) => tag.copy_from_slice(
                &c.encrypt_inout_detached(gcm_nonce(nonce), &[], data.into())
                    .expect(SIZE),
            ),
            AeadCipher::Aes256Gcm(c) => tag.copy_from_slice(
                &c.encrypt_inout_detached(gcm_nonce(nonce), &[], data.into())
                    .expect(SIZE),
            ),
            AeadCipher::ChaCha20Poly1305(c) => tag.copy_from_slice(
                &c.encrypt_in_place_detached(GenericArray::from_slice(nonce), &[], data)
                    .expect(SIZE),
            ),
            AeadCipher::XChaCha20Poly1305(c) => tag.copy_from_slice(
                &c.encrypt_in_place_detached(GenericArray::from_slice(nonce), &[], data)
                    .expect(SIZE),
            ),
        }
        tag
    }

    /// Decrypts `data` in place; `false` when `tag` does not authenticate it
    /// (`data` is then garbage).
    pub(crate) fn open_in_place(&self, nonce: &[u8], data: &mut [u8], tag: &[u8; TAG]) -> bool {
        match self {
            AeadCipher::Aes128Gcm(c) => c
                .decrypt_inout_detached(gcm_nonce(nonce), &[], data.into(), &(*tag).into())
                .is_ok(),
            AeadCipher::Aes192Gcm(c) => c
                .decrypt_inout_detached(gcm_nonce(nonce), &[], data.into(), &(*tag).into())
                .is_ok(),
            AeadCipher::Aes256Gcm(c) => c
                .decrypt_inout_detached(gcm_nonce(nonce), &[], data.into(), &(*tag).into())
                .is_ok(),
            AeadCipher::ChaCha20Poly1305(c) => c
                .decrypt_in_place_detached(
                    GenericArray::from_slice(nonce),
                    &[],
                    data,
                    GenericArray::from_slice(tag),
                )
                .is_ok(),
            AeadCipher::XChaCha20Poly1305(c) => c
                .decrypt_in_place_detached(
                    GenericArray::from_slice(nonce),
                    &[],
                    data,
                    GenericArray::from_slice(tag),
                )
                .is_ok(),
        }
    }
}

/// A cipher and the nonce of one direction of a stream: a little-endian
/// counter from zero, incremented after every operation. No `Debug`.
pub(crate) struct CountingAead {
    cipher: AeadCipher,
    nonce: [u8; MAX_NONCE],
    nonce_len: usize,
}

impl CountingAead {
    pub(crate) fn new(kind: AeadKind, key: &[u8]) -> CountingAead {
        CountingAead {
            cipher: AeadCipher::new(kind, key),
            nonce: [0; MAX_NONCE],
            nonce_len: kind.nonce_len(),
        }
    }

    /// Wraps at the nonce's width, which no stream reaches.
    fn advance(&mut self) {
        for byte in &mut self.nonce[..self.nonce_len] {
            *byte = byte.wrapping_add(1);
            if *byte != 0 {
                break;
            }
        }
    }

    /// Appends `plain` sealed, its tag last, to `out`.
    pub(crate) fn seal(&mut self, plain: &[u8], out: &mut Vec<u8>) {
        let start = out.len();
        out.extend_from_slice(plain);
        let tag = self
            .cipher
            .seal_in_place(&self.nonce[..self.nonce_len], &mut out[start..]);
        out.extend_from_slice(&tag);
        self.advance();
    }

    /// Opens `sealed` (its tag last) in place; the plaintext is
    /// `sealed[..n]`. `None`: it does not authenticate. The nonce moves on
    /// either way.
    pub(crate) fn open(&mut self, sealed: &mut [u8]) -> Option<usize> {
        let n = sealed.len().checked_sub(TAG)?;
        let (data, tag) = sealed.split_at_mut(n);
        let tag: &[u8; TAG] = (&*tag).try_into().expect("the last 16 bytes");
        let ok = self
            .cipher
            .open_in_place(&self.nonce[..self.nonce_len], data, tag);
        self.advance();
        ok.then_some(n)
    }
}

/// The key a method's streams derive their session keys from. No `Debug`.
pub(crate) struct MasterKey {
    kind: AeadKind,
    key: Vec<u8>,
}

impl MasterKey {
    /// The AEAD methods' key: `EVP_BytesToKey(MD5)` of the password.
    pub(crate) fn from_password(kind: AeadKind, password: &str) -> MasterKey {
        MasterKey {
            kind,
            key: kdf::evp_bytes_to_key(password.as_bytes(), kind.key_len()),
        }
    }

    pub(crate) fn salt_len(&self) -> usize {
        self.kind.key_len()
    }

    /// One direction of a stream that starts with `salt`: the session key
    /// is HKDF-SHA1 of the master key under that salt.
    pub(crate) fn session(&self, salt: &[u8]) -> CountingAead {
        CountingAead::new(self.kind, &kdf::session_subkey(&self.key, salt))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_method_has_its_cipher_and_sizes() {
        let cases = [
            (SsMethod::Aes128Gcm, AeadKind::Aes128Gcm, 16, 12),
            (SsMethod::Aes192Gcm, AeadKind::Aes192Gcm, 24, 12),
            (SsMethod::Aes256Gcm, AeadKind::Aes256Gcm, 32, 12),
            (
                SsMethod::ChaCha20IetfPoly1305,
                AeadKind::ChaCha20Poly1305,
                32,
                12,
            ),
            (
                SsMethod::XChaCha20IetfPoly1305,
                AeadKind::XChaCha20Poly1305,
                32,
                24,
            ),
            (SsMethod::Blake3Aes128Gcm, AeadKind::Aes128Gcm, 16, 12),
            (SsMethod::Blake3Aes256Gcm, AeadKind::Aes256Gcm, 32, 12),
        ];
        for (method, kind, key, nonce) in cases {
            assert_eq!(AeadKind::of(method), Some(kind));
            assert_eq!((kind.key_len(), kind.nonce_len()), (key, nonce));
            assert_eq!(kind.key_len(), method.key_len(), "{method:?}");
        }
        assert_eq!(AeadKind::of(SsMethod::None), None);
    }

    #[test]
    fn the_nonce_counts_little_endian_with_a_carry() {
        let mut aead = CountingAead::new(AeadKind::Aes128Gcm, &[0; 16]);
        aead.nonce[0] = 0xff;
        aead.advance();
        assert_eq!(aead.nonce[..3], [0, 1, 0]);
        aead.nonce[..12].fill(0xff);
        aead.advance();
        assert_eq!(aead.nonce, [0; MAX_NONCE], "wraps at its own width");
    }

    #[test]
    fn what_was_sealed_opens_once_and_a_flipped_bit_does_not() {
        for kind in [
            AeadKind::Aes128Gcm,
            AeadKind::Aes192Gcm,
            AeadKind::Aes256Gcm,
            AeadKind::ChaCha20Poly1305,
            AeadKind::XChaCha20Poly1305,
        ] {
            let key = vec![7u8; kind.key_len()];
            let mut up = CountingAead::new(kind, &key);
            let mut wire = Vec::new();
            up.seal(b"first", &mut wire);
            up.seal(b"second", &mut wire);
            let mut down = CountingAead::new(kind, &key);
            let (first, second) = wire.split_at_mut(5 + TAG);
            assert_eq!(down.open(first), Some(5));
            assert_eq!(&first[..5], b"first");
            second[0] ^= 1;
            assert_eq!(down.open(second), None, "{kind:?}");
            // out of step: the second nonce does not open the first chunk
            let mut again = Vec::new();
            CountingAead::new(kind, &key).seal(b"first", &mut again);
            let mut late = CountingAead::new(kind, &key);
            late.advance();
            assert_eq!(late.open(&mut again), None);
            assert_eq!(down.open(&mut [0u8; 3]), None, "shorter than a tag");
        }
    }
}
```

新建 `crates/rurge-proto/src/shadowsocks/kdf.rs`：

```rust
//! Key derivation of the AEAD methods (the Shadowsocks AEAD specification):
//! the master key from the password, and one session key per salt.

use hkdf::Hkdf;
use md5::{Digest, Md5};
use sha1::Sha1;

/// OpenSSL's `EVP_BytesToKey` with MD5, one round and no salt, key part
/// only: `D0 = MD5(password)`, `Di = MD5(Di-1 ‖ password)`, the first `len`
/// bytes of `D0 ‖ D1 ‖ …`.
pub(crate) fn evp_bytes_to_key(password: &[u8], len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len + 16);
    let mut previous: Option<[u8; 16]> = None;
    while out.len() < len {
        let mut md5 = Md5::new();
        if let Some(previous) = previous {
            md5.update(previous);
        }
        md5.update(password);
        let block: [u8; 16] = md5.finalize().into();
        out.extend_from_slice(&block);
        previous = Some(block);
    }
    out.truncate(len);
    out
}

/// HKDF-SHA1 (RFC 5869).
fn hkdf_sha1(ikm: &[u8], salt: &[u8], info: &[u8], okm: &mut [u8]) {
    Hkdf::<Sha1>::new(Some(salt), ikm)
        .expand(info, okm)
        // at most 255 × 20 bytes; a key is 32 at most
        .expect("a key-sized output");
}

/// The session key of a stream (or a datagram) that starts with `salt`:
/// `HKDF-SHA1(master, salt, "ss-subkey")`, as long as the master key.
pub(crate) fn session_subkey(master: &[u8], salt: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; master.len()];
    hkdf_sha1(master, salt, b"ss-subkey", &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vmess::vectors::hex;

    /// Computed with Python's `hashlib` from the definition above.
    #[test]
    fn evp_bytes_to_key_chains_md5_over_the_password() {
        let full = "5f4dcc3b5aa765d61d8327deb882cf992b95990a9151374abd8ff8c5a7a0fe08";
        // the first block is plain MD5("password")
        assert_eq!(evp_bytes_to_key(b"password", 16), hex(&full[..32]));
        assert_eq!(evp_bytes_to_key(b"password", 24), hex(&full[..48]));
        assert_eq!(evp_bytes_to_key(b"password", 32), hex(full));
        assert!(evp_bytes_to_key(b"password", 0).is_empty());
    }

    /// RFC 5869 appendix A.4 (test case 4, SHA-1).
    #[test]
    fn hkdf_sha1_is_the_rfcs() {
        let mut okm = [0u8; 42];
        hkdf_sha1(
            &[0x0b; 11],
            &hex("000102030405060708090a0b0c"),
            &hex("f0f1f2f3f4f5f6f7f8f9"),
            &mut okm,
        );
        assert_eq!(
            okm.to_vec(),
            hex(
                "085a01ea1b10f36933068b56efa5ad81a4f14b822f5b091568a9cdd4f155fda2c22e422478d305f3f896"
            )
        );
    }

    /// Computed with Python's `hmac` from RFC 5869 and the specification's
    /// info string: salt `00 01 … 1f`, the master key of "password".
    #[test]
    fn the_session_subkey_is_as_long_as_the_master_key() {
        let master = evp_bytes_to_key(b"password", 32);
        let salt: Vec<u8> = (0u8..32).collect();
        assert_eq!(
            session_subkey(&master, &salt),
            hex("ee187aed3f87574907a39db98606f60a526114831288097cac66054b33a9464f")
        );
        let master = evp_bytes_to_key(b"password", 16);
        let salt: Vec<u8> = (0u8..16).collect();
        assert_eq!(
            session_subkey(&master, &salt),
            hex("ed2a618d9490d1701de885d82aa80616")
        );
    }
}
```

新建 `crates/rurge-proto/src/shadowsocks/aead.rs`：

```rust
//! The TCP stream of the AEAD methods (the Shadowsocks AEAD specification):
//! each direction starts with its own random salt, then chunks of
//! `sealed(length, 2 bytes big-endian) ‖ sealed(payload)`, both sealed with
//! the direction's session key and the next value of its counting nonce.
//!
//! A write reports success only after its whole chunk has been handed to the
//! layer below, so the stream never depends on anyone calling `flush`. The
//! salt leaves in front of the first chunk, in the same write. There is no
//! end-of-stream chunk: a shutdown passes straight through.
//!
//! Two contracts on the caller, as for `VmessStream`: a write that returned
//! `Pending` must be retried with the same bytes (the parked chunk was
//! sealed from them and is what goes out), and a read error is final (the
//! nonce has moved on).

use super::cipher::{CountingAead, MasterKey, TAG};
use rurge_net::connector::BoxedStream;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// The largest payload of a chunk: the length's top two bits are reserved.
pub(crate) const MAX_PAYLOAD: usize = 0x3FFF;

/// The server says nothing before the first payload, so a wrong password (or
/// method) and a server that refuses for any other reason look the same:
/// the connection closes (phase 2 M6 design 3.3).
const NO_ANSWER: &str = "ss: the server closed the connection without answering";
const UNDECRYPTABLE: &str = "ss: the server's data failed to decrypt (wrong password or method?)";
const CUT_SHORT: &str = "ss: the connection ended in the middle of a chunk";
const TOO_LONG: &str = "ss: the server sent a chunk longer than the protocol allows";

/// Appends one chunk carrying `payload` (at most the stream's largest) to `out`.
pub(crate) fn seal_chunk(aead: &mut CountingAead, payload: &[u8], out: &mut Vec<u8>) {
    let len = u16::try_from(payload.len()).expect("a chunk's length fits two bytes");
    aead.seal(&len.to_be_bytes(), out);
    aead.seal(payload, out);
}

enum Reading {
    Salt {
        buf: Vec<u8>,
        filled: usize,
    },
    Len {
        buf: [u8; 2 + TAG],
        filled: usize,
    },
    Body {
        buf: Vec<u8>,
        filled: usize,
    },
    Payload {
        buf: Vec<u8>,
        pos: usize,
        end: usize,
    },
    Eof,
}

/// No `Debug`: it holds the connection's keys.
pub(crate) struct AeadStream {
    inner: BoxedStream,
    key: Arc<MasterKey>,
    max_payload: usize,
    /// Our salt until it is sealed into `out` in front of the first chunk.
    salt: Option<Vec<u8>>,
    up: CountingAead,
    /// Known once the server's salt has arrived.
    down: Option<CountingAead>,
    /// The chunk being written, how much of it is out, and how many payload
    /// bytes it carries.
    out: Vec<u8>,
    out_pos: usize,
    accepted: usize,
    reading: Reading,
}

fn invalid(text: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, text)
}

fn cut_short() -> io::Error {
    io::Error::new(io::ErrorKind::UnexpectedEof, CUT_SHORT)
}

/// Fills `buf[*filled..]`. `Ok(false)`: the peer closed before the first byte.
fn poll_fill(
    inner: &mut BoxedStream,
    cx: &mut Context<'_>,
    buf: &mut [u8],
    filled: &mut usize,
) -> Poll<io::Result<bool>> {
    while *filled < buf.len() {
        let mut space = ReadBuf::new(&mut buf[*filled..]);
        ready!(Pin::new(&mut *inner).poll_read(cx, &mut space))?;
        let n = space.filled().len();
        if n == 0 {
            return Poll::Ready(if *filled == 0 {
                Ok(false)
            } else {
                Err(cut_short())
            });
        }
        *filled += n;
    }
    Poll::Ready(Ok(true))
}

impl AeadStream {
    /// `salt` is fresh randomness of `key.salt_len()` bytes; `max_payload`
    /// bounds the chunks both ways.
    pub(crate) fn new(
        inner: BoxedStream,
        key: Arc<MasterKey>,
        salt: Vec<u8>,
        max_payload: usize,
    ) -> AeadStream {
        let salt_len = key.salt_len();
        AeadStream {
            up: key.session(&salt),
            down: None,
            inner,
            key,
            max_payload,
            salt: Some(salt),
            out: Vec::new(),
            out_pos: 0,
            accepted: 0,
            reading: Reading::Salt {
                buf: vec![0; salt_len],
                filled: 0,
            },
        }
    }

    fn poll_out(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.out_pos < self.out.len() {
            let n = ready!(Pin::new(&mut self.inner).poll_write(cx, &self.out[self.out_pos..]))?;
            if n == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
            self.out_pos += n;
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for AeadStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            match &mut this.reading {
                Reading::Salt { buf, filled } => {
                    if !ready!(poll_fill(&mut this.inner, cx, buf, filled))? {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            NO_ANSWER,
                        )));
                    }
                    this.down = Some(this.key.session(buf));
                    this.reading = Reading::Len {
                        buf: [0; 2 + TAG],
                        filled: 0,
                    };
                }
                Reading::Len { buf, filled } => {
                    if !ready!(poll_fill(&mut this.inner, cx, buf, filled))? {
                        this.reading = Reading::Eof;
                        continue;
                    }
                    let down = this.down.as_mut().expect("the salt came first");
                    if down.open(buf).is_none() {
                        return Poll::Ready(Err(invalid(UNDECRYPTABLE)));
                    }
                    let len = usize::from(u16::from_be_bytes([buf[0], buf[1]]));
                    if len > this.max_payload {
                        return Poll::Ready(Err(invalid(TOO_LONG)));
                    }
                    this.reading = Reading::Body {
                        buf: vec![0; len + TAG],
                        filled: 0,
                    };
                }
                Reading::Body { buf, filled } => {
                    if !ready!(poll_fill(&mut this.inner, cx, buf, filled))? {
                        return Poll::Ready(Err(cut_short()));
                    }
                    let down = this.down.as_mut().expect("the salt came first");
                    let Some(end) = down.open(buf) else {
                        return Poll::Ready(Err(invalid(UNDECRYPTABLE)));
                    };
                    this.reading = if end == 0 {
                        // nothing to hand out: on to the next chunk
                        Reading::Len {
                            buf: [0; 2 + TAG],
                            filled: 0,
                        }
                    } else {
                        Reading::Payload {
                            buf: std::mem::take(buf),
                            pos: 0,
                            end,
                        }
                    };
                }
                Reading::Payload { buf, pos, end } => {
                    let n = out.remaining().min(*end - *pos);
                    out.put_slice(&buf[*pos..*pos + n]);
                    *pos += n;
                    if pos == end {
                        this.reading = Reading::Len {
                            buf: [0; 2 + TAG],
                            filled: 0,
                        };
                    }
                    return Poll::Ready(Ok(()));
                }
                Reading::Eof => return Poll::Ready(Ok(())),
            }
        }
    }
}

impl AsyncWrite for AeadStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        if this.out_pos == this.out.len() {
            let n = data.len().min(this.max_payload);
            this.out.clear();
            this.out_pos = 0;
            if let Some(salt) = this.salt.take() {
                this.out.extend_from_slice(&salt);
            }
            seal_chunk(&mut this.up, &data[..n], &mut this.out);
            this.accepted = n;
        }
        ready!(this.poll_out(cx))?;
        Poll::Ready(Ok(this.accepted.min(data.len())))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_out(cx))?;
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        // a parked chunk first: it already consumed its nonce
        ready!(this.poll_out(cx))?;
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shadowsocks::cipher::AeadKind;
    use crate::vmess::vectors::hex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// `seal("hello")` then `seal(00 01 … 27)` under the password
    /// "password" and the salt `00 01 …`, computed with Python's
    /// `cryptography` (XChaCha20 through its HChaCha20 subkey) from the
    /// specification alone.
    const VECTORS: [(AeadKind, &str); 5] = [
        (
            AeadKind::Aes128Gcm,
            "5c2b27a26ad0cdf9cd7aa4f3c851b134b4b9947477b58a2f87d1affe84b78de924b5f222d5ec8cf7fff1aae7df1600106a3ca3d13ffa98f19b96d22705e6337c6f27ef7d7df12956ead1fdd856502e2aad6d8196189c21536caabf29450eb17c0122df377b5e82b6b195b78de510e5cde8",
        ),
        (
            AeadKind::Aes192Gcm,
            "863e97590d6f98066b78f274bdb5a4ad7f95f7102226764c34b2fcd7b95c2feba247473e8fd1a84e815c1195a2ece1e98b8324d252f1c1d681c8e74e4afa0c38e2bc45f99ed8ee1206199a625f2a0c5fb1f4fedeaa4c04f59adfb70f7e37f85645a7274d715413e5268a0f0e93112c5e78",
        ),
        (
            AeadKind::Aes256Gcm,
            "7ea089e1d8874f484867a34f5b648078a7379d45b3194573671c53431294750d0362127bcf86798eb8d4e434962dd61b1be92d84791ec1b1d31ff67ef4c1204f35e6ab270005b9a672651fffb9a410f2edd96ac4114ada92f823d528bf789d9c44f7f8f163e61c9d3e6938214dbc4fa77d",
        ),
        (
            AeadKind::ChaCha20Poly1305,
            "ad4d5c2599d42f6d9b26804b82a3b96dc584e8adc7498c0ff41f578989fe0c5ded753038d91134efbb23156ec73f2d258da9e30323b8caed59a798588d30c7d900092ff76b36e9d27f78881a2357bd6a2cfce948523b580a3fbac4e189be71a50e0cf17b4591cff3d261d862aa69be155b",
        ),
        (
            AeadKind::XChaCha20Poly1305,
            "7808afe0b13ac1c1e48139cc556091669eaa8a9e1627100727e6b7ad25261f8856a6c77c3e51635380cc7098a3c7212080a0a2d4009237d9780a17ff173a194e3df77e49e9dbb8eab3e5313c586119c3642563047150cfe12b6988b8c15c4bf2f979e32d185e7c674a407a1f1c0055c409",
        ),
    ];

    fn salt(kind: AeadKind) -> Vec<u8> {
        (0..kind.key_len() as u8).collect()
    }

    fn key(kind: AeadKind, password: &str) -> Arc<MasterKey> {
        Arc::new(MasterKey::from_password(kind, password))
    }

    fn long() -> Vec<u8> {
        (0u8..40).collect()
    }

    /// What a stream keyed with `password` reads from a peer that sends
    /// `wire` through a pipe of `capacity` bytes and then closes.
    async fn read_from(
        kind: AeadKind,
        password: &str,
        wire: Vec<u8>,
        capacity: usize,
    ) -> io::Result<Vec<u8>> {
        let (near, mut far) = tokio::io::duplex(capacity);
        tokio::spawn(async move {
            let _ = far.write_all(&wire).await;
            // dropping `far` is the close
        });
        let mut stream =
            AeadStream::new(Box::new(near), key(kind, password), salt(kind), MAX_PAYLOAD);
        let mut got = Vec::new();
        stream.read_to_end(&mut got).await.map(|_| got)
    }

    #[tokio::test]
    async fn every_cipher_writes_the_known_answer() {
        for (kind, vector) in VECTORS {
            let (near, mut far) = tokio::io::duplex(64 * 1024);
            let mut stream = AeadStream::new(
                Box::new(near),
                key(kind, "password"),
                salt(kind),
                MAX_PAYLOAD,
            );
            // an empty write seals nothing: an empty chunk is not a payload
            assert_eq!(stream.write(b"").await.unwrap(), 0);
            stream.write_all(b"hello").await.unwrap();
            stream.write_all(&long()).await.unwrap();
            stream.shutdown().await.unwrap();
            let mut wire = Vec::new();
            far.read_to_end(&mut wire).await.unwrap();
            let mut expected = salt(kind);
            expected.extend_from_slice(&hex(vector));
            assert_eq!(wire, expected, "{kind:?}");
        }
    }

    #[tokio::test]
    async fn the_server_stream_reads_back_whatever_the_slicing() {
        for (kind, vector) in VECTORS {
            // the answer has the request's form: its own salt, then chunks
            let mut wire = salt(kind);
            wire.extend_from_slice(&hex(vector));
            for capacity in [1, 7, 4096] {
                let got = read_from(kind, "password", wire.clone(), capacity)
                    .await
                    .unwrap();
                let mut expected = b"hello".to_vec();
                expected.extend_from_slice(&long());
                assert_eq!(got, expected, "{kind:?} through {capacity}");
            }
        }
    }

    #[tokio::test]
    async fn a_large_write_is_cut_into_chunks_of_at_most_0x3fff() {
        let kind = AeadKind::Aes128Gcm;
        let (near, mut far) = tokio::io::duplex(1 << 20);
        let mut stream = AeadStream::new(Box::new(near), key(kind, "pw"), salt(kind), MAX_PAYLOAD);
        let data = vec![0x55u8; MAX_PAYLOAD + 1];
        assert_eq!(stream.write(&data).await.unwrap(), MAX_PAYLOAD, "one chunk");
        stream.write_all(&data[MAX_PAYLOAD..]).await.unwrap();
        stream.shutdown().await.unwrap();
        let mut wire = Vec::new();
        far.read_to_end(&mut wire).await.unwrap();
        assert_eq!(
            wire.len(),
            16 + (2 + TAG + MAX_PAYLOAD + TAG) + (2 + TAG + 1 + TAG)
        );
        // the peer's view: two chunks, the first of the largest length
        let got = read_from(kind, "pw", wire, 4096).await.unwrap();
        assert_eq!(got, data);
    }

    #[tokio::test]
    async fn what_the_server_gets_wrong_is_an_error_that_quotes_nothing() {
        let kind = AeadKind::Aes256Gcm;
        let (_, vector) = VECTORS[2];
        let mut good = salt(kind);
        good.extend_from_slice(&hex(vector));
        let first_chunk = 32 + (2 + TAG) + (5 + TAG);
        let mut flipped = good.clone();
        flipped[32] ^= 1;
        // a length of 0x4000, sealed correctly
        let mut too_long = salt(kind);
        let mut aead = key(kind, "password").session(&salt(kind));
        aead.seal(&0x4000u16.to_be_bytes(), &mut too_long);
        let cases: [(&str, &str, Vec<u8>, io::ErrorKind, &str); 6] = [
            (
                "silence",
                "password",
                Vec::new(),
                io::ErrorKind::UnexpectedEof,
                NO_ANSWER,
            ),
            (
                "half a salt",
                "password",
                good[..20].to_vec(),
                io::ErrorKind::UnexpectedEof,
                CUT_SHORT,
            ),
            (
                "a flipped bit",
                "password",
                flipped,
                io::ErrorKind::InvalidData,
                UNDECRYPTABLE,
            ),
            (
                "another password",
                "other",
                good.clone(),
                io::ErrorKind::InvalidData,
                UNDECRYPTABLE,
            ),
            (
                "an oversized chunk",
                "password",
                too_long,
                io::ErrorKind::InvalidData,
                TOO_LONG,
            ),
            (
                "the middle of a chunk",
                "password",
                good[..first_chunk - 1].to_vec(),
                io::ErrorKind::UnexpectedEof,
                CUT_SHORT,
            ),
        ];
        for (case, password, wire, kind_of_error, text) in cases {
            let err = read_from(kind, password, wire, 4096).await.unwrap_err();
            assert_eq!(
                (err.kind(), err.to_string().as_str()),
                (kind_of_error, text),
                "{case}"
            );
        }
        // closed between two chunks: an ordinary end
        let got = read_from(kind, "password", good[..first_chunk].to_vec(), 4096)
            .await
            .unwrap();
        assert_eq!(got, b"hello");
    }
}
```

新建 `crates/rurge-proto/src/shadowsocks/mod.rs`：

```rust
//! `ss` outbound (manual: Policies › Shadowsocks; phase 2 M6 design 3.3):
//! optionally behind Shadow TLS and / or simple-obfs, the AEAD stream
//! (`aead`) — or, with `none`, the bytes as they are. The request header is
//! the target as a SOCKS5 address; it waits in a `LazyHead` above the
//! stream for the first payload, so both are sealed into the first chunk.
//!
//! The server never answers the header: a wrong password or method only
//! shows once the relay reads, as a connection closed without an answer or
//! as data that does not decrypt.

pub(crate) mod aead;
pub(crate) mod cipher;
pub(crate) mod kdf;

use crate::addr::{AddrError, socks_addr};
use crate::build::shadow_tls_client;
use crate::transport::Stack;
use crate::transport::lazy_head::LazyHead;
use crate::transport::obfs::ObfsClient;
use crate::{BuildError, Outbound, OutboundError};
use aead::AeadStream;
use cipher::{AeadKind, MasterKey};
use rurge_config::spec::{ShadowTlsOpts, SsSpec};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rustls::RootCertStore;
use std::sync::Arc;

/// No `Debug`: the master key is as good as the password.
pub struct ShadowsocksOutbound {
    name: String,
    stack: Stack,
    /// `None`: the method is `none`.
    key: Option<Arc<MasterKey>>,
}

impl ShadowsocksOutbound {
    pub fn new(
        name: &str,
        server: Target,
        spec: &SsSpec,
        shadow_tls: Option<&ShadowTlsOpts>,
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> Result<ShadowsocksOutbound, BuildError> {
        // error texts carry no policy name: the registry's `build_one` and the
        // dry build both prefix it
        if spec.method.is_2022() {
            return Err(BuildError::new(format!(
                "ss: `{}` is not supported yet",
                spec.method.name()
            )));
        }
        let key = match AeadKind::of(spec.method) {
            None => None,
            Some(_) if spec.password.expose().is_empty() => {
                return Err(BuildError::new("`password` is empty"));
            }
            Some(kind) => Some(Arc::new(MasterKey::from_password(
                kind,
                spec.password.expose(),
            ))),
        };
        // `ss` has no TLS of its own: the camouflage certificate is checked
        // against the server's name
        let shadow_tls = shadow_tls_client(shadow_tls, None, &server.host, roots)?;
        let obfs = spec
            .obfs
            .as_ref()
            .map(|obfs| ObfsClient::new(obfs, &server))
            .transpose()?;
        let mut stack = Stack::new(connector, server, shadow_tls, None, None);
        if let Some(obfs) = obfs {
            stack = stack.with_obfs(obfs);
        }
        Ok(ShadowsocksOutbound {
            name: name.to_string(),
            stack,
            key,
        })
    }
}

fn head(target: &Target) -> Result<Vec<u8>, OutboundError> {
    socks_addr(target).map_err(|e| {
        OutboundError::Proxy(
            match e {
                AddrError::Unsendable => "ss: the host name cannot be sent to the server",
                AddrError::TooLong => "ss: the host name is longer than 255 bytes",
            }
            .to_string(),
        )
    })
}

impl Outbound for ShadowsocksOutbound {
    fn name(&self) -> &str {
        &self.name
    }

    fn connect_tcp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            // never dial for a target whose name cannot be sent
            let head = head(target)?;
            let salt = match &self.key {
                None => Vec::new(),
                Some(key) => {
                    let mut salt = vec![0u8; key.salt_len()];
                    getrandom::fill(&mut salt).map_err(|_| {
                        OutboundError::Proxy("ss: no randomness available".to_string())
                    })?;
                    salt
                }
            };
            // one budget for the connection, Shadow TLS and obfs; the
            // server's first word comes with its first payload, in the relay
            let transport = match tokio::time::timeout(opts.timeout, self.stack.open(opts)).await {
                Ok(result) => result?,
                Err(_) => return Err(OutboundError::Timeout),
            };
            let stream: BoxedStream = match &self.key {
                None => transport,
                Some(key) => Box::new(AeadStream::new(
                    transport,
                    key.clone(),
                    salt,
                    aead::MAX_PAYLOAD,
                )),
            };
            Ok(Box::new(LazyHead::new(stream, head)) as BoxedStream)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::UdpSupport;
    use crate::testing::{FakeShadowsocks, ShadowsocksScript, echo_server};
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::shadow_tls::read_shadow_tls;
    use rurge_config::spec::ss::read_ss;
    use rurge_config::spec::{ObfsMode, ParamReader, Secret, SsMethod};
    use rurge_config::{HostName, Span};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const AEAD: [SsMethod; 5] = [
        SsMethod::Aes128Gcm,
        SsMethod::Aes192Gcm,
        SsMethod::Aes256Gcm,
        SsMethod::ChaCha20IetfPoly1305,
        SsMethod::XChaCha20IetfPoly1305,
    ];

    /// The outbound for `definition` (an `ss, host, port, ...` line).
    fn outbound(definition: &str) -> ShadowsocksOutbound {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let policy = parse_policy("S", definition, &span).unwrap();
        let mut r = ParamReader::new(&policy);
        let read = read_ss(&mut r);
        let shadow_tls = read_shadow_tls(&mut r);
        assert!(!r.has_errors(), "{:?}", r.finish());
        ShadowsocksOutbound::new(
            "S",
            Target::new(policy.server.clone().unwrap(), policy.port.unwrap()),
            &read.spec,
            shadow_tls.as_ref(),
            Arc::new(RootCertStore::empty()),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .unwrap()
    }

    fn target(addr: SocketAddr) -> Target {
        Target::new(HostName::Ip(addr.ip()), addr.port())
    }

    async fn roundtrip(stream: &mut BoxedStream, text: &[u8]) {
        // no explicit `flush()`: `write_all` alone must deliver
        stream.write_all(text).await.unwrap();
        let mut buf = vec![0u8; text.len()];
        tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut buf))
            .await
            .expect("the echo arrives within the bound")
            .unwrap();
        assert!(buf == text, "the echo differs");
    }

    #[tokio::test]
    async fn every_method_carries_the_head_with_the_first_payload() {
        let echo = echo_server().await;
        for method in AEAD.into_iter().chain([SsMethod::None]) {
            let fake = FakeShadowsocks::spawn(ShadowsocksScript::new(method, "pw")).await;
            let out = outbound(&format!(
                "ss, 127.0.0.1, {}, encrypt-method={}, password=pw",
                fake.addr().port(),
                method.name()
            ));
            assert_eq!(out.name(), "S");
            assert_eq!(out.udp(), UdpSupport::Unsupported);
            let mut stream = out
                .connect_tcp(&target(echo), &ConnectOpts::default())
                .await
                .unwrap();
            roundtrip(&mut stream, b"hello through ss").await;
            roundtrip(&mut stream, b"and again").await;
            let seen = fake.requests();
            assert_eq!(seen.len(), 1, "{method:?}");
            assert_eq!(
                (seen[0].atyp, seen[0].host.as_str(), seen[0].port),
                (1, "127.0.0.1", echo.port())
            );
            assert_eq!(seen[0].early, b"hello through ss", "{method:?}: one chunk");
            assert_eq!(seen[0].salt.len(), method.key_len());
        }
    }

    #[tokio::test]
    async fn each_connection_has_a_salt_of_its_own() {
        let echo = echo_server().await;
        let fake = FakeShadowsocks::spawn(ShadowsocksScript::new(SsMethod::Aes256Gcm, "pw")).await;
        let out = outbound(&format!(
            "ss, 127.0.0.1, {}, encrypt-method=aes-256-gcm, password=pw",
            fake.addr().port()
        ));
        for _ in 0..2 {
            let mut stream = out
                .connect_tcp(&target(echo), &ConnectOpts::default())
                .await
                .unwrap();
            roundtrip(&mut stream, b"x").await;
        }
        let seen = fake.requests();
        assert_ne!(seen[0].salt, seen[1].salt);
        assert_ne!(
            seen[0].salt,
            fake.answer_salts()[0],
            "and one per direction"
        );
    }

    #[tokio::test]
    async fn a_large_payload_crosses_many_chunks_both_ways() {
        let echo = echo_server().await;
        let fake =
            FakeShadowsocks::spawn(ShadowsocksScript::new(SsMethod::ChaCha20IetfPoly1305, "pw"))
                .await;
        let out = outbound(&format!(
            "ss, 127.0.0.1, {}, encrypt-method=chacha20-ietf-poly1305, password=pw",
            fake.addr().port()
        ));
        let stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        let data: Vec<u8> = (0..1 << 20).map(|i: u32| (i % 251) as u8).collect();
        let (mut read, mut write) = tokio::io::split(stream);
        let sent = data.clone();
        let writer = tokio::spawn(async move {
            write.write_all(&sent).await.unwrap();
        });
        let mut back = vec![0u8; data.len()];
        tokio::time::timeout(Duration::from_secs(30), read.read_exact(&mut back))
            .await
            .expect("the echo arrives within the bound")
            .unwrap();
        assert!(back == data, "the echo differs");
        writer.await.unwrap();
        // the fake refuses anything longer
        assert_eq!(fake.largest_chunk(), aead::MAX_PAYLOAD, "full chunks");
    }

    #[tokio::test]
    async fn names_go_out_as_a_labels_and_an_unsendable_name_never_dials() {
        let echo = echo_server().await;
        let fake = FakeShadowsocks::spawn(ShadowsocksScript {
            connect_to: Some(echo),
            ..ShadowsocksScript::new(SsMethod::Aes128Gcm, "pw")
        })
        .await;
        let out = outbound(&format!(
            "ss, 127.0.0.1, {}, encrypt-method=aes-128-gcm, password=pw",
            fake.addr().port()
        ));
        let mut stream = out
            .connect_tcp(
                &Target::new(HostName::Domain("bücher.example".into()), 443),
                &ConnectOpts::default(),
            )
            .await
            .unwrap();
        roundtrip(&mut stream, b"x").await;
        let seen = fake.requests();
        assert_eq!(
            (seen[0].atyp, seen[0].host.as_str(), seen[0].port),
            (3, "xn--bcher-kva.example", 443)
        );
        let before = fake.connections();
        for (name, expected) in [
            (
                "a@b.test".to_string(),
                "ss: the host name cannot be sent to the server",
            ),
            (
                "a".repeat(256),
                "ss: the host name is longer than 255 bytes",
            ),
        ] {
            let err = out
                .connect_tcp(
                    &Target::new(HostName::Domain(name), 443),
                    &ConnectOpts::default(),
                )
                .await
                .err()
                .expect("refused");
            assert!(
                matches!(&err, OutboundError::Proxy(m) if m == expected),
                "{err}"
            );
        }
        assert_eq!(fake.connections(), before, "nothing was dialled");
    }

    #[tokio::test]
    async fn a_wrong_password_is_a_connection_closed_without_an_answer() {
        let echo = echo_server().await;
        let fake =
            FakeShadowsocks::spawn(ShadowsocksScript::new(SsMethod::Aes256Gcm, "right")).await;
        let out = outbound(&format!(
            "ss, 127.0.0.1, {}, encrypt-method=aes-256-gcm, password=wrong",
            fake.addr().port()
        ));
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .expect("connecting succeeds");
        stream.write_all(b"hello").await.unwrap();
        let mut answer = Vec::new();
        let err = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut answer))
            .await
            .expect("the server closes")
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "ss: the server closed the connection without answering"
        );
        assert_eq!((fake.rejected(), fake.requests().len()), (1, 0));
    }

    #[tokio::test]
    async fn through_both_obfs_modes() {
        let echo = echo_server().await;
        for (mode, extra) in [
            (
                ObfsMode::Http,
                "obfs=http, obfs-host=cdn.example, obfs-uri=/a",
            ),
            (ObfsMode::Tls, "obfs=tls, obfs-host=cdn.example"),
        ] {
            let fake = FakeShadowsocks::spawn(ShadowsocksScript {
                obfs: Some(mode),
                ..ShadowsocksScript::new(SsMethod::Aes128Gcm, "pw")
            })
            .await;
            let port = fake.addr().port();
            let out = outbound(&format!(
                "ss, 127.0.0.1, {port}, encrypt-method=aes-128-gcm, password=pw, {extra}"
            ));
            let mut stream = out
                .connect_tcp(&target(echo), &ConnectOpts::default())
                .await
                .unwrap();
            roundtrip(&mut stream, b"behind the camouflage").await;
            let data = vec![0xa5u8; 100_000];
            roundtrip(&mut stream, &data).await;
            let hello = &fake.obfs_seen()[0];
            match mode {
                ObfsMode::Http => {
                    assert_eq!(hello.host, format!("cdn.example:{port}"));
                    assert_eq!(hello.uri.as_deref(), Some("/a"));
                }
                ObfsMode::Tls => assert_eq!(hello.host, "cdn.example"),
            }
            assert_eq!(fake.requests()[0].early, b"behind the camouflage");
        }
    }

    #[tokio::test]
    async fn a_half_close_reaches_the_target_and_the_answer_still_arrives() {
        // reads to the end, then answers with how much it got
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let counter = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut tcp, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut all = Vec::new();
                    if tcp.read_to_end(&mut all).await.is_ok() {
                        let _ = tcp.write_all(format!("got {}", all.len()).as_bytes()).await;
                    }
                });
            }
        });
        for method in [SsMethod::XChaCha20IetfPoly1305, SsMethod::None] {
            let fake = FakeShadowsocks::spawn(ShadowsocksScript::new(method, "pw")).await;
            let out = outbound(&format!(
                "ss, 127.0.0.1, {}, encrypt-method={}, password=pw",
                fake.addr().port(),
                method.name()
            ));
            let mut stream = out
                .connect_tcp(&target(counter), &ConnectOpts::default())
                .await
                .unwrap();
            stream.write_all(b"12345").await.unwrap();
            stream.shutdown().await.unwrap();
            let mut answer = Vec::new();
            tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut answer))
                .await
                .expect("the answer arrives")
                .unwrap();
            assert_eq!(answer, b"got 5", "{method:?}");
        }
    }

    #[test]
    fn what_cannot_be_built_is_a_build_error_that_quotes_nothing() {
        let build = |method: SsMethod, password: &str| {
            ShadowsocksOutbound::new(
                "S",
                Target::new(HostName::parse("127.0.0.1"), 8388),
                &SsSpec {
                    method,
                    password: Secret::from(password),
                    keys: Secret::default(),
                    udp_relay: false,
                    udp_port: None,
                    obfs: None,
                },
                None,
                Arc::new(RootCertStore::empty()),
                Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
            )
            .map(|_| ())
            .unwrap_err()
            .message
        };
        assert_eq!(build(SsMethod::Aes128Gcm, ""), "`password` is empty");
        assert_eq!(
            build(SsMethod::Blake3Aes128Gcm, "secret"),
            "ss: `2022-blake3-aes-128-gcm` is not supported yet"
        );
        // `none` needs no password
        assert!(
            ShadowsocksOutbound::new(
                "S",
                Target::new(HostName::parse("127.0.0.1"), 8388),
                &SsSpec {
                    method: SsMethod::None,
                    password: Secret::default(),
                    keys: Secret::default(),
                    udp_relay: false,
                    udp_port: None,
                    obfs: None,
                },
                None,
                Arc::new(RootCertStore::empty()),
                Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
            )
            .is_ok()
        );
    }
}
```

要点：
- 分块流的写：每次写封成不超过上限的一块，salt 与第一块在同一个缓冲里写出；整块交给下层之后才报告成功（同 `VmessStream`）；shutdown 原样传下去（AEAD 没有结束标记）。
- 读：salt → 派生 → 长度块 → 负载块；零长度块跳过。
- nonce 计数在每次 AEAD 操作之后前进（失败也前进）。
- 目标名的编码与字母表规则同 trojan / socks5；名字发不出去时不拨号。
- salt 取不到随机数时是 `ss: no randomness available`（同 vmess）。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-proto --lib shadowsocks` → 通过（`kdf` 3 条、`cipher` 3 条、`aead` 4 条、`shadowsocks::tests` 8 条：五种 AEAD 与 `none` 的往返、每条连接各自的 salt、1 MiB 跨块、IDN 名字与发不出去的名字、口令错、两种 obfs、半关闭、构建错误）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add Cargo.toml crates/rurge-proto
git commit -m "feat(proto): Shadowsocks 的 AEAD 与 none（TCP）——分块流、口令与子密钥派生、FakeShadowsocks"
```

### Task 4: SS 2022 的 TCP（含多用户身份头）

`2022-blake3-aes-128-gcm` / `-256-gcm`：子密钥 BLAKE3 `derive_key("shadowsocks 2022 session subkey", PSK ‖ salt)`、请求的固定长度头与变长头（P10）、应答的校验与时钟（P11）、SIP023 的身份头。`AeadStream` 加 `new_2022`，不另写一种流。`FakeShadowsocks` 加 2022 的服务端一侧（自己的头编解码、身份头解出用户、时间窗、请求 salt 防重放，可设错误的时钟或回显的 salt）。

**Files:**
- Create: `crates/rurge-proto/src/shadowsocks/s2022.rs`（自带用例）
- Modify: `Cargo.toml`、`crates/rurge-proto/Cargo.toml`（`blake3`）、`src/shadowsocks/cipher.rs`、`kdf.rs`、`aead.rs`、`mod.rs`（与用例）、`src/testing/shadowsocks.rs`

**Interfaces:**
- Consumes: Task 3 的 `AeadKind` / `CountingAead` / `MasterKey` / `AeadStream` / `ShadowsocksOutbound` / `FakeShadowsocks`；Task 1 的 `SsSpec.keys`。
- Produces:
  - `kdf.rs`：`session_subkey_2022(psk, salt) -> Vec<u8>`、`identity_subkey(ipsk, salt) -> Vec<u8>`、`identity_hash(key) -> [u8; 16]`
  - `cipher.rs`：`MasterKey::from_psk(kind, psk)`、`aes_encrypt_block(key, &mut [u8; 16])`、`aes_decrypt_block`（本任务只给测试与假服务端，Task 5 起客户端也用）
  - `s2022.rs`：`MAX_PAYLOAD = 0xFFFF`、`REQUEST_FIXED = 11`、`unix_now() -> u64`、`struct Identity`（`new(keys: &[Vec<u8>])`、`headers(salt) -> Vec<u8>`）、`request_fixed`、`request_variable`、`padding_len`、`response_fixed_len`、`check_response(plain, request_salt, now) -> io::Result<usize>`
  - `aead.rs`：`pub(crate) struct Request2022 { identity: Vec<u8>, addr_len: usize, now: fn() -> u64 }`、`AeadStream::new_2022(inner, key, salt, request)`
  - `ShadowsocksOutbound` 的 `#[cfg(test)] with_clock`
  - 测试设施：`ShadowsocksScript` 加 `users: Vec<String>`、`answer_skew: i64`、`wrong_request_salt: bool`；`RecordedShadowsocks` 加 `padding`、`user`

- [ ] **Step 1: 先写用例（连同假服务端的 2022）**

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
//! the AEAD stream (or, with `none`, the bytes as they are), the request
//! header, and a relay. Its framing, nonce counting and address parsing are
//! written apart from the client's (`crate::shadowsocks`), so each checks the
//! other; only the primitives (the ciphers and the key derivation, which
//! have vectors of their own) are shared. It never resolves a name.
//!
//! As real servers do, it never tells a client with a wrong password so:
//! it stops reading into the stream, closes its side and waits for the
//! client to go away.
```

换成

```rust
//! the AEAD stream (or, with `none`, the bytes as they are; or SS 2022 with
//! its headers and, for several users, identity headers), the request
//! header, and a relay. Its framing, nonce counting, headers and address
//! parsing are written apart from the client's (`crate::shadowsocks`), so
//! each checks the other; only the primitives (the ciphers and the key
//! derivations, which have vectors of their own) are shared. It never
//! resolves a name.
//!
//! As real servers do, it never tells a client with a wrong password (or
//! key, or clock, or a replayed salt) so: it stops reading into the stream,
//! closes its side and waits for the client to go away.
```

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
use crate::shadowsocks::cipher::{AeadCipher, AeadKind, TAG};
use crate::shadowsocks::kdf;
use rurge_config::spec::{ObfsMode, SsMethod};
use rurge_net::connector::BoxedStream;
```

换成

```rust
use crate::shadowsocks::cipher::{AeadCipher, AeadKind, TAG, aes_decrypt_block};
use crate::shadowsocks::kdf;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rurge_config::spec::{ObfsMode, SsMethod};
use rurge_net::connector::BoxedStream;
use std::collections::HashSet;
```

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
use std::sync::{Arc, Mutex};
```

换成

```rust
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
```

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
/// The largest payload a client chunk may carry.
const MAX_CHUNK: usize = 0x3FFF;
```

换成

```rust
/// The largest payload a client chunk may carry (AEAD, SS 2022).
const MAX_CHUNK: usize = 0x3FFF;
const MAX_CHUNK_2022: usize = 0xFFFF;
```

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
    pub password: String,
```

换成

```rust
    /// The password; SS 2022: the key in Base64, or with `users` the
    /// server's identity key.
    pub password: String,
    /// SS 2022 with identity headers: the users' keys in Base64. Empty: a
    /// single-user server, no identity header.
    pub users: Vec<String>,
```

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
    pub connect_to: Option<SocketAddr>,
```

换成

```rust
    pub connect_to: Option<SocketAddr>,
    /// SS 2022: seconds added to the timestamp of every answer.
    pub answer_skew: i64,
    /// SS 2022: answer naming a salt that is not the request's.
    pub wrong_request_salt: bool,
```

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
            obfs: None,
            connect_to: None,
```

换成

```rust
            users: Vec::new(),
            obfs: None,
            connect_to: None,
            answer_skew: 0,
            wrong_request_salt: false,
```

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
    /// the read that completed the address).
    pub early: Vec<u8>,
```

换成

```rust
    /// the read that completed the address; SS 2022: after the padding).
    pub early: Vec<u8>,
    /// SS 2022: the request's padding length.
    pub padding: usize,
    /// SS 2022 with users: which one's key the identity header named.
    pub user: Option<usize>,
```

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
    largest_chunk: AtomicUsize,
```

换成

```rust
    largest_chunk: AtomicUsize,
    /// SS 2022: every request salt accepted so far, to refuse replays.
    salts: Mutex<HashSet<Vec<u8>>>,
```

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
        Direction {
            cipher: AeadCipher::new(kind, &kdf::session_subkey(&master, salt)),
```

换成

```rust
        Direction::keyed(kind, &kdf::session_subkey(&master, salt))
    }

    fn new_2022(kind: AeadKind, psk: &[u8], salt: &[u8]) -> Direction {
        Direction::keyed(kind, &kdf::session_subkey_2022(psk, salt))
    }

    fn keyed(kind: AeadKind, session_key: &[u8]) -> Direction {
        Direction {
            cipher: AeadCipher::new(kind, session_key),
```

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
    up: &mut Direction,
```

换成

```rust
    up: &mut Direction,
    max: usize,
```

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
    if len > MAX_CHUNK {
        return Err(bad("a chunk over 0x3FFF bytes"));
```

换成

```rust
    if len > max {
        return Err(bad("a chunk over the largest payload"));
```

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
            early: buf[used + 2..].to_vec(),
```

换成

```rust
            early: buf[used + 2..].to_vec(),
            padding: 0,
            user: None,
```

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
        None => serve_plain(stream, &shared).await,
```

换成

```rust
        None => serve_plain(stream, &shared).await,
        Some(kind) if shared.script.method.is_2022() => serve_2022(stream, kind, &shared).await,
```

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
    let first = match read_chunk(&mut stream, &mut up, &shared.seen).await {
```

换成

```rust
    let first = match read_chunk(&mut stream, &mut up, MAX_CHUNK, &shared.seen).await {
```

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
        while let Ok(Some(payload)) = read_chunk(&mut client_read, &mut up, &shared.seen).await {
```

换成

```rust
        while let Ok(Some(payload)) =
            read_chunk(&mut client_read, &mut up, MAX_CHUNK, &shared.seen).await
        {
```

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
            down.seal(&(n as u16).to_be_bytes(), &mut frame);
```

换成

```rust
            down.seal(&(n as u16).to_be_bytes(), &mut frame);
            down.seal(&buf[..n], &mut frame);
            if client_write.write_all(&frame).await.is_err() {
                return;
            }
        }
        let _ = client_write.shutdown().await;
    };
    tokio::join!(requests, answers);
    Ok(())
}

fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("after 1970")
        .as_secs()
}

fn decode_key(text: &str) -> Vec<u8> {
    STANDARD.decode(text).expect("a Base64 key in the script")
}

/// A 2022 request's variable-length header: the address, the padding
/// behind its length, the initial payload. `None` when malformed, or when
/// it has neither padding nor payload (the specification's rule).
fn parse_variable(buf: &[u8]) -> Option<RecordedShadowsocks> {
    let (mut request, used) = parse_address(buf)?;
    let rest = &buf[used..];
    let padding = usize::from(u16::from_be_bytes(rest.get(..2)?.try_into().ok()?));
    let payload = rest.get(2 + padding..)?;
    if padding == 0 && payload.is_empty() {
        return None;
    }
    request.padding = padding;
    request.early = payload.to_vec();
    Some(request)
}

/// SS 2022 (SIP022 3.1, SIP023): salt, identity header when the script has
/// users, the fixed-length header chunk (type 0, a timestamp within 30
/// seconds, a salt never seen), the variable-length one; the answer opens
/// with its own header chunk.
async fn serve_2022(mut stream: BoxedStream, kind: AeadKind, shared: &Shared) -> io::Result<()> {
    let script = &shared.script;
    let mut salt = vec![0u8; kind.key_len()];
    stream.read_exact(&mut salt).await?;
    let server_key = decode_key(&script.password);
    let (psk, user) = if script.users.is_empty() {
        (server_key, None)
    } else {
        let mut block = [0u8; 16];
        stream.read_exact(&mut block).await?;
        aes_decrypt_block(&kdf::identity_subkey(&server_key, &salt), &mut block);
        let users: Vec<Vec<u8>> = script.users.iter().map(|u| decode_key(u)).collect();
        match users.iter().position(|u| kdf::identity_hash(u) == block) {
            Some(i) => (users[i].clone(), Some(i)),
            None => return shared.reject(stream).await,
        }
    };
    let mut up = Direction::new_2022(kind, &psk, &salt);
    let mut fixed = [0u8; 1 + 8 + 2 + TAG];
    stream.read_exact(&mut fixed).await?;
    let Some(fixed) = up.open(&fixed) else {
        return shared.reject(stream).await;
    };
    let time = u64::from_be_bytes(fixed[1..9].try_into().expect("8 bytes"));
    if fixed[0] != 0 || time.abs_diff(unix_time()) > 30 {
        return shared.reject(stream).await;
    }
    if !shared
        .seen
        .salts
        .lock()
        .expect("salts")
        .insert(salt.clone())
    {
        return shared.reject(stream).await;
    }
    let len = usize::from(u16::from_be_bytes([fixed[9], fixed[10]]));
    let mut variable = vec![0u8; len + TAG];
    stream.read_exact(&mut variable).await?;
    let Some(mut request) = up.open(&variable).as_deref().and_then(parse_variable) else {
        return shared.reject(stream).await;
    };
    request.salt = salt.clone();
    request.user = user;
    shared.record(&request);
    let Some(to) = shared.upstream(&request) else {
        return stream.shutdown().await;
    };
    let mut upstream = TcpStream::connect(to).await?;
    upstream.write_all(&request.early).await?;
    let (mut client_read, mut client_write) = tokio::io::split(stream);
    let (mut target_read, mut target_write) = upstream.into_split();
    let requests = async {
        while let Ok(Some(payload)) =
            read_chunk(&mut client_read, &mut up, MAX_CHUNK_2022, &shared.seen).await
        {
            if target_write.write_all(&payload).await.is_err() {
                return;
            }
        }
        let _ = target_write.shutdown().await;
    };
    let answers = async {
        let mut answer_salt = vec![0u8; kind.key_len()];
        getrandom::fill(&mut answer_salt).expect("randomness");
        let mut down = Direction::new_2022(kind, &psk, &answer_salt);
        let mut echoed = salt.clone();
        if script.wrong_request_salt {
            echoed[0] ^= 1;
        }
        // the salt and the header go out with the first payload, never alone
        let mut pending_salt = Some(answer_salt);
        let mut buf = vec![0u8; MAX_CHUNK_2022];
        loop {
            let n = match target_read.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let len = (n as u16).to_be_bytes();
            let mut frame = Vec::with_capacity(n + 128);
            if let Some(salt) = pending_salt.take() {
                shared
                    .seen
                    .answer_salts
                    .lock()
                    .expect("salts")
                    .push(salt.clone());
                frame.extend_from_slice(&salt);
                let time = unix_time().saturating_add_signed(script.answer_skew);
                let mut header = vec![1];
                header.extend_from_slice(&time.to_be_bytes());
                header.extend_from_slice(&echoed);
                header.extend_from_slice(&len);
                down.seal(&header, &mut frame);
            } else {
                down.seal(&len, &mut frame);
            }
```

KDF 的已知答案（BLAKE3 的期望值由按规范自写的 Python BLAKE3 算出，并以规范自带的空串与 `abc` 哈希校验过）：

`crates/rurge-proto/src/shadowsocks/kdf.rs`——把

```rust
            hex("ed2a618d9490d1701de885d82aa80616")
        );
    }
}

```

换成

```rust
            hex("ed2a618d9490d1701de885d82aa80616")
        );
    }

    /// Computed with a BLAKE3 written in Python from its specification
    /// (checked against the specification's hashes of "" and "abc"): key
    /// `00 01 …`, salt `80 81 …`, both as long as the method's key. 32 + 32
    /// bytes of material are exactly one block.
    #[test]
    fn ss_2022_subkeys_and_the_identity_hash_are_blake3s() {
        let cases = [
            (
                16,
                "722b3033c5d021365a8521bfb41157a3",
                "9b488f206a32316bf47ef417027b4242",
                "a6a492965517a830cb75fdb713465aa4",
            ),
            (
                32,
                "11289b9d205255930f83932405c2b0a38ec32be703fe33f290ff25ffeff402f9",
                "e3ba9438b4e97ed02d0c818020755598829161aaca5dc2b65fd46238ca2148ad",
                "e528e95798037df410543d9f31e396ec",
            ),
        ];
        for (len, session, identity, hash) in cases {
            let key: Vec<u8> = (0..len).collect();
            let salt: Vec<u8> = (0x80..0x80 + len).collect();
            assert_eq!(session_subkey_2022(&key, &salt), hex(session), "{len}");
            assert_eq!(identity_subkey(&key, &salt), hex(identity), "{len}");
            assert_eq!(identity_hash(&key).to_vec(), hex(hash), "{len}");
        }
    }
}

```

分块流与出站的用例：

`crates/rurge-proto/src/shadowsocks/aead.rs`——把

```rust
        assert_eq!(got, b"hello");
    }
}
```

换成

```rust
        assert_eq!(got, b"hello");
    }

    const TIME: u64 = 1_700_000_000;
    /// 127.0.0.1:8080 as a SOCKS5 address.
    const ADDR: [u8; 7] = [1, 127, 0, 0, 1, 0x1f, 0x90];

    /// SS 2022 known answers, computed with a BLAKE3 written in Python from
    /// its specification and `cryptography`'s AES-GCM / AES-ECB: the key
    /// sets, the request after its salt `80 81 …` (identity headers, the
    /// header chunks for `ADDR` ‖ "hello" at `TIME`, then a chunk "world"),
    /// and a response (salt `90 91 …`, header at `TIME` naming the request
    /// salt, first chunk "ok").
    struct Vector2022 {
        kind: AeadKind,
        /// The identity keys, then the user key.
        keys: Vec<Vec<u8>>,
        request: &'static str,
        response: &'static str,
    }

    fn vectors_2022() -> [Vector2022; 2] {
        [
            Vector2022 {
                kind: AeadKind::Aes128Gcm,
                keys: vec![(0u8..16).collect(), (0x20u8..0x30).collect()],
                request: "efa5909821ac85519cb2bac2aebde4c208e3db4c9c568afe00f79400b7de08b99705e60672e49140157f36cb37ee7cdbb68b6f05e5b0781929dd6faa3f98821068d0dd702f5c25c7a6e3660c43fe09c778270316fdca0040824985d1e5c085edf95a3d34c77e0ea6434d2bb376e9afee",
                response: "909192939495969798999a9b9c9d9e9f17be40f377e19922ad1161db151a79ab845e3736f8b62015b85aa6a7abae3321e93c787a398be13e55aa4697fc05eae3783a4f75ac3cf0252cb9df1daa",
            },
            Vector2022 {
                kind: AeadKind::Aes256Gcm,
                keys: vec![(0x20u8..0x40).collect()],
                request: "a74482c100c255a6eb2bd1f55f1988d1c550161c7275ca6428763b418e260e6df7a2f7cc9b0ba2611e87e691d74fb7c14fc48011bddcc78f5767ccab920e6a74aefcdb72a6e91368c4eadf6185c04dfc07fddcc1fcd229291349b11478f41af7",
                response: "909192939495969798999a9b9c9d9e9fa0a1a2a3a4a5a6a7a8a9aaabacadaeaf8d4a9fedac53d86bcfa93ab21983ac82b38c11611d5a54f53de08cc2b0f5d7aef4fd69c51150aab7a37b0feedb3351fc511674611d46d29627514852c2567450524bf016bf3e555405e12149ca",
            },
        ]
    }

    fn request_salt(kind: AeadKind) -> Vec<u8> {
        (0x80..0x80 + kind.key_len() as u8).collect()
    }

    fn stream_2022(
        inner: BoxedStream,
        kind: AeadKind,
        keys: &[Vec<u8>],
        now: fn() -> u64,
    ) -> AeadStream {
        let salt = request_salt(kind);
        let request = Request2022 {
            identity: s2022::Identity::new(keys).headers(&salt),
            addr_len: ADDR.len(),
            now,
        };
        let key = Arc::new(MasterKey::from_psk(kind, keys.last().unwrap()));
        AeadStream::new_2022(inner, key, salt, request)
    }

    /// What a 2022 stream reads from a server that sends `wire` through a
    /// pipe of `capacity` bytes and then closes, its clock at `now`.
    async fn read_2022(
        kind: AeadKind,
        keys: &[Vec<u8>],
        wire: Vec<u8>,
        capacity: usize,
        now: fn() -> u64,
    ) -> io::Result<Vec<u8>> {
        let (near, mut far) = tokio::io::duplex(capacity);
        tokio::spawn(async move {
            let _ = far.write_all(&wire).await;
        });
        let mut stream = stream_2022(Box::new(near), kind, keys, now);
        let mut got = Vec::new();
        stream.read_to_end(&mut got).await.map(|_| got)
    }

    #[tokio::test]
    async fn ss_2022_writes_the_known_answer() {
        for vector in vectors_2022() {
            let (near, mut far) = tokio::io::duplex(64 * 1024);
            let mut stream = stream_2022(Box::new(near), vector.kind, &vector.keys, || TIME);
            // the address and the first payload: one write, as a `LazyHead` does it
            let first = [&ADDR[..], b"hello"].concat();
            assert_eq!(stream.write(&first).await.unwrap(), first.len());
            stream.write_all(b"world").await.unwrap();
            stream.shutdown().await.unwrap();
            let mut wire = Vec::new();
            far.read_to_end(&mut wire).await.unwrap();
            let mut expected = request_salt(vector.kind);
            expected.extend_from_slice(&hex(vector.request));
            assert_eq!(wire, expected, "{:?}", vector.kind);
        }
    }

    #[tokio::test]
    async fn ss_2022_reads_the_known_answer_whatever_the_slicing() {
        for vector in vectors_2022() {
            for capacity in [1, 7, 4096] {
                let got = read_2022(
                    vector.kind,
                    &vector.keys,
                    hex(vector.response),
                    capacity,
                    || TIME + 30,
                )
                .await
                .unwrap();
                assert_eq!(got, b"ok", "{:?} through {capacity}", vector.kind);
            }
        }
    }

    #[tokio::test]
    async fn an_address_alone_is_padded_and_a_long_first_write_fills_one_header() {
        let kind = AeadKind::Aes128Gcm;
        let keys = vec![vec![3u8; 16]];
        // the target speaks first: the address goes out alone, padded
        let (near, mut far) = tokio::io::duplex(1 << 20);
        let mut stream = stream_2022(Box::new(near), kind, &keys, || TIME);
        stream.write_all(&ADDR).await.unwrap();
        stream.shutdown().await.unwrap();
        let mut wire = Vec::new();
        far.read_to_end(&mut wire).await.unwrap();
        let bare = 16 + (s2022::REQUEST_FIXED + TAG) + (ADDR.len() + 2 + TAG);
        assert!(
            (bare + 1..=bare + 900).contains(&wire.len()),
            "{} bytes",
            wire.len()
        );
        // with a payload, as much as the variable header holds, unpadded
        let (near, mut far) = tokio::io::duplex(1 << 20);
        let mut stream = stream_2022(Box::new(near), kind, &keys, || TIME);
        let first = [&ADDR[..], &vec![0x55; 100_000]].concat();
        let taken = stream.write(&first).await.unwrap();
        assert_eq!(taken, s2022::MAX_VARIABLE_HEADER - 2);
        stream.write_all(&first[taken..]).await.unwrap();
        stream.shutdown().await.unwrap();
        let mut wire = Vec::new();
        far.read_to_end(&mut wire).await.unwrap();
        let rest = first.len() - taken;
        assert_eq!(
            wire.len(),
            16 + (s2022::REQUEST_FIXED + TAG) + (0xFFFF + TAG) + (2 + TAG + rest + TAG)
        );
    }

    #[tokio::test]
    async fn a_2022_answer_that_is_not_ours_is_an_error_that_quotes_nothing() {
        let kind = AeadKind::Aes128Gcm;
        let keys = vec![(0x20u8..0x30).collect::<Vec<u8>>()];
        let answer_salt = vec![0x90u8; 16];
        // an answer sealed right, its header as given
        let answer = |header: &[u8]| {
            let key = MasterKey::from_psk(kind, &keys[0]);
            let mut aead = key.session(&answer_salt);
            let mut wire = answer_salt.clone();
            aead.seal(header, &mut wire);
            aead.seal(b"ok", &mut wire);
            wire
        };
        let header = |kind_byte: u8, time: u64, salt: &[u8]| {
            let mut out = vec![kind_byte];
            out.extend_from_slice(&time.to_be_bytes());
            out.extend_from_slice(salt);
            out.extend_from_slice(&2u16.to_be_bytes());
            out
        };
        let ours = request_salt(kind);
        let mut other = ours.clone();
        other[15] ^= 1;
        let good = answer(&header(1, TIME, &ours));
        let not_ours = "ss: the server's answer is not for this request";
        let cases: [(&str, Vec<u8>, io::ErrorKind, &str); 6] = [
            (
                "silence",
                Vec::new(),
                io::ErrorKind::UnexpectedEof,
                NO_ANSWER,
            ),
            (
                "a salt alone",
                answer_salt.clone(),
                io::ErrorKind::UnexpectedEof,
                CUT_SHORT,
            ),
            (
                "our request played back",
                answer(&header(0, TIME, &ours)),
                io::ErrorKind::InvalidData,
                not_ours,
            ),
            (
                "another request's salt",
                answer(&header(1, TIME, &other)),
                io::ErrorKind::InvalidData,
                not_ours,
            ),
            (
                "a clock 31 seconds ahead",
                answer(&header(1, TIME + 31, &ours)),
                io::ErrorKind::InvalidData,
                "ss: the server's clock differs from ours by 31 seconds (at most 30 are allowed)",
            ),
            (
                "another key",
                {
                    let mut wire = good.clone();
                    wire[16] ^= 1;
                    wire
                },
                io::ErrorKind::InvalidData,
                UNDECRYPTABLE,
            ),
        ];
        for (case, wire, kind_of_error, text) in cases {
            let err = read_2022(kind, &keys, wire, 4096, || TIME)
                .await
                .unwrap_err();
            assert_eq!(
                (err.kind(), err.to_string().as_str()),
                (kind_of_error, text),
                "{case}"
            );
        }
        let got = read_2022(kind, &keys, good, 4096, || TIME).await.unwrap();
        assert_eq!(got, b"ok");
    }
}
```

`crates/rurge-proto/src/shadowsocks/mod.rs`——把

```rust
    use crate::testing::{FakeShadowsocks, ShadowsocksScript, echo_server};
```

换成

```rust
    use crate::addr::socks_addr;
    use crate::testing::{FakeShadowsocks, ShadowsocksScript, echo_server};
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
```

`crates/rurge-proto/src/shadowsocks/mod.rs`——把

```rust
    #[test]
    fn what_cannot_be_built_is_a_build_error_that_quotes_nothing() {
        let build = |method: SsMethod, password: &str| {
```

换成

```rust
    const SS_2022: [SsMethod; 2] = [SsMethod::Blake3Aes128Gcm, SsMethod::Blake3Aes256Gcm];

    /// A Base64 key of `method`'s length, every byte `byte`.
    fn key_2022(method: SsMethod, byte: u8) -> String {
        STANDARD.encode(vec![byte; method.key_len()])
    }

    /// An outbound of `method` whose `password` is `password`.
    fn outbound_2022(
        fake: &FakeShadowsocks,
        method: SsMethod,
        password: &str,
    ) -> ShadowsocksOutbound {
        outbound(&format!(
            "ss, 127.0.0.1, {}, encrypt-method={}, password={password}",
            fake.addr().port(),
            method.name()
        ))
    }

    #[tokio::test]
    async fn ss_2022_round_trips_with_one_key_or_as_one_of_several_users() {
        let echo = echo_server().await;
        for method in SS_2022 {
            let (server, user) = (key_2022(method, 1), key_2022(method, 2));
            // a single key; the server's key and one of its users' behind it,
            // also behind obfs
            let setups = [
                (
                    ShadowsocksScript::new(method, &server),
                    server.clone(),
                    None,
                ),
                (
                    ShadowsocksScript {
                        users: vec![key_2022(method, 3), user.clone()],
                        ..ShadowsocksScript::new(method, &server)
                    },
                    format!("{server}:{user}"),
                    Some(1),
                ),
                (
                    ShadowsocksScript {
                        users: vec![user.clone()],
                        obfs: Some(ObfsMode::Tls),
                        ..ShadowsocksScript::new(method, &server)
                    },
                    format!("{server}:{user}, obfs=tls, obfs-host=cdn.example"),
                    Some(0),
                ),
            ];
            for (script, password, user) in setups {
                let fake = FakeShadowsocks::spawn(script).await;
                let out = outbound_2022(&fake, method, &password);
                let mut stream = out
                    .connect_tcp(&target(echo), &ConnectOpts::default())
                    .await
                    .unwrap();
                roundtrip(&mut stream, b"hello through ss 2022").await;
                roundtrip(&mut stream, b"and again").await;
                let seen = fake.requests();
                assert_eq!(seen.len(), 1, "{method:?} {password}");
                assert_eq!(
                    (seen[0].atyp, seen[0].host.as_str(), seen[0].port),
                    (1, "127.0.0.1", echo.port())
                );
                assert_eq!(seen[0].early, b"hello through ss 2022", "one write");
                assert_eq!((seen[0].padding, seen[0].user), (0, user));
                assert_eq!(seen[0].salt.len(), method.key_len());
            }
        }
    }

    #[tokio::test]
    async fn ss_2022_sends_names_and_crosses_chunks_of_0xffff() {
        let echo = echo_server().await;
        let method = SsMethod::Blake3Aes256Gcm;
        let key = key_2022(method, 7);
        let fake = FakeShadowsocks::spawn(ShadowsocksScript {
            connect_to: Some(echo),
            ..ShadowsocksScript::new(method, &key)
        })
        .await;
        let out = outbound_2022(&fake, method, &key);
        let stream = out
            .connect_tcp(
                &Target::new(HostName::Domain("bücher.example".into()), 443),
                &ConnectOpts::default(),
            )
            .await
            .unwrap();
        let data: Vec<u8> = (0..1 << 20).map(|i: u32| (i % 251) as u8).collect();
        let (mut read, mut write) = tokio::io::split(stream);
        let sent = data.clone();
        let writer = tokio::spawn(async move {
            write.write_all(&sent).await.unwrap();
        });
        let mut back = vec![0u8; data.len()];
        tokio::time::timeout(Duration::from_secs(30), read.read_exact(&mut back))
            .await
            .expect("the echo arrives within the bound")
            .unwrap();
        assert!(back == data, "the echo differs");
        writer.await.unwrap();
        let seen = fake.requests();
        assert_eq!(
            (seen[0].atyp, seen[0].host.as_str(), seen[0].port),
            (3, "xn--bcher-kva.example", 443)
        );
        assert_eq!(fake.largest_chunk(), s2022::MAX_PAYLOAD, "full chunks");
    }

    #[tokio::test]
    async fn a_silent_client_pads_its_request_and_hears_the_target_first() {
        // a target that speaks first, then echoes
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let greeter = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut tcp, _)) = listener.accept().await {
                tokio::spawn(async move {
                    tcp.write_all(b"220 ready").await?;
                    let (mut r, mut w) = tcp.split();
                    tokio::io::copy(&mut r, &mut w).await
                });
            }
        });
        let method = SsMethod::Blake3Aes128Gcm;
        let key = key_2022(method, 5);
        let fake = FakeShadowsocks::spawn(ShadowsocksScript::new(method, &key)).await;
        let out = outbound_2022(&fake, method, &key);
        let mut stream = out
            .connect_tcp(&target(greeter), &ConnectOpts::default())
            .await
            .unwrap();
        let mut greeting = [0u8; 9];
        tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut greeting))
            .await
            .expect("the greeting arrives")
            .unwrap();
        assert_eq!(&greeting, b"220 ready");
        roundtrip(&mut stream, b"HELO").await;
        let seen = fake.requests();
        assert!(seen[0].early.is_empty());
        assert!((1..=900).contains(&seen[0].padding), "{}", seen[0].padding);
    }

    #[tokio::test]
    async fn a_key_the_server_does_not_know_is_a_connection_closed_without_an_answer() {
        let echo = echo_server().await;
        let method = SsMethod::Blake3Aes128Gcm;
        let (server, user) = (key_2022(method, 1), key_2022(method, 2));
        let stranger = key_2022(method, 9);
        for (script, password) in [
            (ShadowsocksScript::new(method, &server), stranger.clone()),
            (
                ShadowsocksScript {
                    users: vec![user.clone()],
                    ..ShadowsocksScript::new(method, &server)
                },
                format!("{server}:{stranger}"),
            ),
            (
                ShadowsocksScript {
                    users: vec![user.clone()],
                    ..ShadowsocksScript::new(method, &server)
                },
                // a user key without the identity header
                user.clone(),
            ),
        ] {
            let fake = FakeShadowsocks::spawn(script).await;
            let out = outbound_2022(&fake, method, &password);
            let mut stream = out
                .connect_tcp(&target(echo), &ConnectOpts::default())
                .await
                .expect("connecting succeeds");
            stream.write_all(b"hello").await.unwrap();
            let mut answer = Vec::new();
            let err = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut answer))
                .await
                .expect("the server closes")
                .unwrap_err();
            assert_eq!(
                err.to_string(),
                "ss: the server closed the connection without answering",
                "{password}"
            );
            assert_eq!((fake.rejected(), fake.requests().len()), (1, 0));
        }
    }

    fn an_hour_behind() -> u64 {
        s2022::unix_now() - 3600
    }

    #[tokio::test]
    async fn a_client_clock_an_hour_off_is_refused_without_an_answer() {
        let echo = echo_server().await;
        let method = SsMethod::Blake3Aes256Gcm;
        let key = key_2022(method, 4);
        let fake = FakeShadowsocks::spawn(ShadowsocksScript::new(method, &key)).await;
        let out = outbound_2022(&fake, method, &key).with_clock(an_hour_behind);
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        stream.write_all(b"hello").await.unwrap();
        let mut answer = Vec::new();
        let err = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut answer))
            .await
            .expect("the server closes")
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "ss: the server closed the connection without answering"
        );
        assert_eq!(fake.rejected(), 1);
    }

    #[tokio::test]
    async fn a_replayed_request_salt_is_refused_without_an_answer() {
        let echo = echo_server().await;
        let method = SsMethod::Blake3Aes128Gcm;
        let key = vec![6u8; 16];
        let fake =
            FakeShadowsocks::spawn(ShadowsocksScript::new(method, &STANDARD.encode(&key))).await;
        let head = socks_addr(&target(echo)).unwrap();
        let salt = vec![0x42u8; 16];
        let mut outcomes = Vec::new();
        for _ in 0..2 {
            let tcp = tokio::net::TcpStream::connect(fake.addr()).await.unwrap();
            let request = Request2022 {
                identity: Vec::new(),
                addr_len: head.len(),
                now: s2022::unix_now,
            };
            let mut stream = AeadStream::new_2022(
                Box::new(tcp),
                Arc::new(MasterKey::from_psk(AeadKind::Aes128Gcm, &key)),
                salt.clone(),
                request,
            );
            stream
                .write_all(&[&head[..], b"once"].concat())
                .await
                .unwrap();
            let mut answer = [0u8; 4];
            let outcome =
                tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut answer))
                    .await
                    .expect("an answer or the close")
                    .map(|_| answer.to_vec())
                    .map_err(|e| e.to_string());
            outcomes.push(outcome);
        }
        assert_eq!(outcomes[0], Ok(b"once".to_vec()));
        assert_eq!(
            outcomes[1],
            Err("ss: the server closed the connection without answering".to_string())
        );
        assert_eq!((fake.requests().len(), fake.rejected()), (1, 1));
    }

    #[tokio::test]
    async fn an_answer_off_the_clock_or_for_another_request_is_an_error() {
        let echo = echo_server().await;
        let method = SsMethod::Blake3Aes128Gcm;
        let key = key_2022(method, 8);
        for script in [
            ShadowsocksScript {
                answer_skew: 3600,
                ..ShadowsocksScript::new(method, &key)
            },
            ShadowsocksScript {
                wrong_request_salt: true,
                ..ShadowsocksScript::new(method, &key)
            },
        ] {
            let skewed = script.answer_skew != 0;
            let fake = FakeShadowsocks::spawn(script).await;
            let out = outbound_2022(&fake, method, &key);
            let mut stream = out
                .connect_tcp(&target(echo), &ConnectOpts::default())
                .await
                .unwrap();
            stream.write_all(b"hello").await.unwrap();
            let mut answer = [0u8; 5];
            let err = tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut answer))
                .await
                .expect("the answer arrives")
                .unwrap_err();
            let text = err.to_string();
            if skewed {
                // a second may tick between the two clocks
                let seconds = text
                    .strip_prefix("ss: the server's clock differs from ours by ")
                    .and_then(|rest| rest.strip_suffix(" seconds (at most 30 are allowed)"))
                    .and_then(|n| n.parse::<u64>().ok());
                assert!(
                    seconds.is_some_and(|n| (3599..=3601).contains(&n)),
                    "{text}"
                );
            } else {
                assert_eq!(text, "ss: the server's answer is not for this request");
            }
            assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        }
    }

    #[test]
    fn what_cannot_be_built_is_a_build_error_that_quotes_nothing() {
        let build_keyed = |method: SsMethod, password: &str, keys: Vec<Vec<u8>>| {
```

`crates/rurge-proto/src/shadowsocks/mod.rs`——把

```rust
                    password: Secret::from(password),
                    keys: Secret::default(),
```

换成

```rust
                    password: Secret::from(password),
                    keys: Secret::new(keys),
```

`crates/rurge-proto/src/shadowsocks/mod.rs`——把

```rust
        assert_eq!(build(SsMethod::Aes128Gcm, ""), "`password` is empty");
        assert_eq!(
            build(SsMethod::Blake3Aes128Gcm, "secret"),
            "ss: `2022-blake3-aes-128-gcm` is not supported yet"
```

换成

```rust
        let build = |method: SsMethod, password: &str| build_keyed(method, password, Vec::new());
        assert_eq!(build(SsMethod::Aes128Gcm, ""), "`password` is empty");
        // SS 2022 takes its keys, not the password (the spec has none here)
        assert_eq!(
            build(SsMethod::Blake3Aes128Gcm, "secret"),
            "`password` is empty"
        );
        assert_eq!(
            build_keyed(
                SsMethod::Blake3Aes256Gcm,
                "x",
                vec![vec![0; 32], vec![0; 16]]
            ),
            "`password` is not Base64 keys of the length `2022-blake3-aes-256-gcm` requires"
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-proto --lib shadowsocks`
Expected: FAIL——`s2022`、`Request2022` 与 2022 的 KDF 由 Step 3 引入，编译不过（节选）：

```text
error[E0432]: unresolved import `crate::shadowsocks::cipher::aes_decrypt_block`
  --> crates\rurge-proto\src\testing\shadowsocks.rs:16:61
error[E0433]: failed to resolve: use of unresolved module or unlinked crate `s2022`
   --> crates\rurge-proto\src\shadowsocks\aead.rs:513:23
error[E0422]: cannot find struct, variant or union type `Request2022` in this scope
   --> crates\rurge-proto\src\shadowsocks\aead.rs:512:23
error[E0422]: cannot find struct, variant or union type `Request2022` in this scope
   --> crates\rurge-proto\src\shadowsocks\mod.rs:669:27
error[E0425]: cannot find function `session_subkey_2022` in module `kdf`
   --> crates\rurge-proto\src\testing\shadowsocks.rs:126:38
error[E0425]: cannot find function `identity_subkey` in module `kdf`
   --> crates\rurge-proto\src\testing\shadowsocks.rs:409:33
error[E0425]: cannot find function `identity_hash` in module `kdf`
   --> crates\rurge-proto\src\testing\shadowsocks.rs:411:46
error[E0599]: no function or associated item named `from_psk` found for struct `shadowsocks::cipher::MasterKey` in the current scope
   --> crates\rurge-proto\src\shadowsocks\aead.rs:517:39
   --> crates\rurge-proto\src\shadowsocks\cipher.rs:222:5
   --> /rustc/29483883eed69d5fb4db01964cdf2af4d86e9cb2\library\core\src\convert\mod.rs:588:5
error[E0599]: no function or associated item named `new_2022` found for struct `shadowsocks::aead::AeadStream` in the current scope
   --> crates\rurge-proto\src\shadowsocks\aead.rs:518:21
   --> crates\rurge-proto\src\shadowsocks\aead.rs:115:5
error[E0433]: failed to resolve: use of unresolved module or unlinked crate `s2022`
   --> crates\rurge-proto\src\shadowsocks\aead.rs:586:26
error[E0433]: failed to resolve: use of unresolved module or unlinked crate `s2022`
   --> crates\rurge-proto\src\shadowsocks\aead.rs:597:27
error[E0433]: failed to resolve: use of unresolved module or unlinked crate `s2022`
   --> crates\rurge-proto\src\shadowsocks\aead.rs:605:19
error[E0599]: no function or associated item named `from_psk` found for struct `shadowsocks::cipher::MasterKey` in the current scope
   --> crates\rurge-proto\src\shadowsocks\aead.rs:616:34
   --> crates\rurge-proto\src\shadowsocks\cipher.rs:222:5
```

- [ ] **Step 3: 实现**

依赖（唯一的新 crate）：

`Cargo.toml`——把

```toml
sha1 = { version = "0.11", default-features = false }
```

换成

```toml
sha1 = { version = "0.11", default-features = false }
# SS 2022: the session and identity subkeys (BLAKE3 derive_key), the identity hash
blake3 = "1.8"
```

`crates/rurge-proto/Cargo.toml`——把

```toml
sha1.workspace = true
```

换成

```toml
sha1.workspace = true
blake3.workspace = true
```

新模块（自带用例）：

新建 `crates/rurge-proto/src/shadowsocks/s2022.rs`：

```rust
//! The headers of SS 2022 over TCP (SIP022 3.1, SIP023; phase 2 M6 design
//! 3.3). The stream itself is `aead::AeadStream` in its 2022 form; these are
//! the pure pieces: the identity headers, the request's two header chunks
//! and the check of the response's header. Time is an argument, so the
//! tests pin it.

use super::cipher::aes_encrypt_block;
use super::kdf;
use std::io;
use std::time::{SystemTime, UNIX_EPOCH};

/// A payload chunk of this edition carries up to 0xFFFF bytes (no 0x3FFF cap).
pub(crate) const MAX_PAYLOAD: usize = 0xFFFF;
/// The request's variable-length header is a chunk too.
pub(crate) const MAX_VARIABLE_HEADER: usize = 0xFFFF;
/// Padding of a request without an initial payload: 1 to 900 bytes.
const MAX_PADDING: u16 = 900;
/// Timestamps further apart than this are a replay.
const TIME_WINDOW: u64 = 30;
const CLIENT_STREAM: u8 = 0;
const SERVER_STREAM: u8 = 1;
/// The request's fixed-length header: type, timestamp, the next chunk's length.
pub(crate) const REQUEST_FIXED: usize = 1 + 8 + 2;

const NOT_OURS: &str = "ss: the server's answer is not for this request";

/// Seconds since the Unix epoch, the protocol's timestamps.
pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The identity layers of a multi-user key (SIP023). No `Debug`: it holds
/// keys.
pub(crate) struct Identity {
    /// Each identity key and the hash of the key after it (the next
    /// identity key, or the user key for the last).
    layers: Vec<(Vec<u8>, [u8; 16])>,
}

impl Identity {
    /// `keys` as written: the identity keys outermost first, the user key
    /// last. A single key has no layers.
    pub(crate) fn new(keys: &[Vec<u8>]) -> Identity {
        Identity {
            layers: keys
                .windows(2)
                .map(|pair| (pair[0].clone(), kdf::identity_hash(&pair[1])))
                .collect(),
        }
    }

    /// The identity headers of a request that starts with `salt`, 16 bytes
    /// a layer: the next key's hash under one AES block keyed with the
    /// identity subkey of this layer's key and the salt.
    pub(crate) fn headers(&self, salt: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 * self.layers.len());
        for (key, next) in &self.layers {
            let mut block = *next;
            aes_encrypt_block(&kdf::identity_subkey(key, salt), &mut block);
            out.extend_from_slice(&block);
        }
        out
    }
}

/// The request's fixed-length header: type 0, `now`, and the length of the
/// variable-length header that follows.
pub(crate) fn request_fixed(now: u64, variable_len: usize) -> [u8; REQUEST_FIXED] {
    let len = u16::try_from(variable_len).expect("the variable header fits a chunk");
    let mut out = [0u8; REQUEST_FIXED];
    out[0] = CLIENT_STREAM;
    out[1..9].copy_from_slice(&now.to_be_bytes());
    out[9..].copy_from_slice(&len.to_be_bytes());
    out
}

/// The request's variable-length header: the address, `padding` bytes of
/// padding behind their length, then the initial payload. The padding is
/// zeros: it is sealed like the rest, only its length shows (as the chunk's).
pub(crate) fn request_variable(addr: &[u8], padding: usize, payload: &[u8]) -> Vec<u8> {
    let len = u16::try_from(padding).expect("at most 900 bytes of padding");
    let mut out = Vec::with_capacity(addr.len() + 2 + padding + payload.len());
    out.extend_from_slice(addr);
    out.extend_from_slice(&len.to_be_bytes());
    out.resize(out.len() + padding, 0);
    out.extend_from_slice(payload);
    out
}

/// How much padding a request carries: none with an initial payload, else
/// 1 to 900 bytes (the specification requires one or the other).
pub(crate) fn padding_len(payload_len: usize) -> Result<usize, getrandom::Error> {
    if payload_len > 0 {
        return Ok(0);
    }
    let mut random = [0u8; 2];
    getrandom::fill(&mut random)?;
    Ok(usize::from(u16::from_be_bytes(random) % MAX_PADDING + 1))
}

/// The length of the response's fixed-length header: type, timestamp, the
/// request's salt, the first chunk's length.
pub(crate) fn response_fixed_len(salt_len: usize) -> usize {
    1 + 8 + salt_len + 2
}

/// Checks the opened response header against the request that started with
/// `request_salt`; the length of the first payload chunk when it is ours.
/// The texts never carry a salt or a timestamp.
pub(crate) fn check_response(plain: &[u8], request_salt: &[u8], now: u64) -> io::Result<usize> {
    let invalid = |text: String| io::Error::new(io::ErrorKind::InvalidData, text);
    let salt_end = 9 + request_salt.len();
    debug_assert_eq!(plain.len(), response_fixed_len(request_salt.len()));
    // our own request played back would open too: its type tells
    if plain[0] != SERVER_STREAM || plain[9..salt_end] != *request_salt {
        return Err(invalid(NOT_OURS.to_string()));
    }
    let time = u64::from_be_bytes(plain[1..9].try_into().expect("8 bytes"));
    let skew = time.abs_diff(now);
    if skew > TIME_WINDOW {
        return Err(invalid(format!(
            "ss: the server's clock differs from ours by {skew} seconds (at most {TIME_WINDOW} are allowed)"
        )));
    }
    Ok(usize::from(u16::from_be_bytes([
        plain[salt_end],
        plain[salt_end + 1],
    ])))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vmess::vectors::hex;

    /// Computed with a BLAKE3 written in Python from its specification and
    /// `cryptography`'s AES-ECB: keys `00 …`, `n …`, `2n …` (n bytes each,
    /// counting up), salt `80 81 …`.
    #[test]
    fn two_identity_layers_are_two_headers_of_the_known_answer() {
        for (n, expected) in [
            (
                16u8,
                "2f80aef230d903eae2200a9b5411cfcf17ae493aaa1743d5a7042ec3fd99ea70",
            ),
            (
                32,
                "c4d59e980ad60a7751f47db7595cc79910038a40f080e8b0931372bfefdd7142",
            ),
        ] {
            let keys: Vec<Vec<u8>> = (0..3).map(|i| (i * n..(i + 1) * n).collect()).collect();
            let salt: Vec<u8> = (0x80..0x80 + n).collect();
            assert_eq!(Identity::new(&keys).headers(&salt), hex(expected), "{n}");
            // one layer fewer: the first header alone is not the same (it
            // hides the user key's hash now)
            let one = Identity::new(&[keys[0].clone(), keys[2].clone()]).headers(&salt);
            assert_eq!(one.len(), 16);
            assert_ne!(one, hex(expected)[..16]);
        }
        assert!(Identity::new(&[vec![7; 16]]).headers(&[0; 16]).is_empty());
    }

    #[test]
    fn the_request_headers_are_laid_out_as_the_specification() {
        let fixed = request_fixed(0x0102030405060708, 0x0a0b);
        assert_eq!(fixed, [0, 1, 2, 3, 4, 5, 6, 7, 8, 0x0a, 0x0b]);
        let addr = [1, 127, 0, 0, 1, 0x1f, 0x90];
        assert_eq!(
            request_variable(&addr, 0, b"hi"),
            [&addr[..], &[0, 0], b"hi"].concat()
        );
        let padded = request_variable(&addr, 3, b"");
        assert_eq!(padded, [&addr[..], &[0, 3, 0, 0, 0]].concat());
    }

    #[test]
    fn padding_only_without_a_payload_and_then_1_to_900_bytes() {
        assert_eq!(padding_len(1).unwrap(), 0);
        for _ in 0..1000 {
            let n = padding_len(0).unwrap();
            assert!((1..=900).contains(&n), "{n}");
        }
    }

    fn response(kind: u8, time: u64, salt: &[u8], len: u16) -> Vec<u8> {
        let mut out = vec![kind];
        out.extend_from_slice(&time.to_be_bytes());
        out.extend_from_slice(salt);
        out.extend_from_slice(&len.to_be_bytes());
        out
    }

    #[test]
    fn a_response_is_ours_only_with_its_type_our_salt_and_a_close_clock() {
        let salt = [9u8; 16];
        let now = 1_700_000_000;
        assert_eq!(response_fixed_len(16), 27);
        assert_eq!(response_fixed_len(32), 43);
        for time in [now - 30, now, now + 30] {
            let got = check_response(&response(1, time, &salt, 513), &salt, now).unwrap();
            assert_eq!(got, 513);
        }
        let texts = |plain: Vec<u8>| check_response(&plain, &salt, now).unwrap_err().to_string();
        assert_eq!(
            texts(response(0, now, &salt, 5)),
            NOT_OURS,
            "a played-back request"
        );
        assert_eq!(texts(response(1, now, &[8; 16], 5)), NOT_OURS);
        assert_eq!(
            texts(response(1, now - 31, &salt, 5)),
            "ss: the server's clock differs from ours by 31 seconds (at most 30 are allowed)"
        );
        assert_eq!(
            texts(response(1, now + 3600, &salt, 5)),
            "ss: the server's clock differs from ours by 3600 seconds (at most 30 are allowed)"
        );
    }
}
```

`crates/rurge-proto/src/shadowsocks/kdf.rs`——把

```rust
//! the master key from the password, and one session key per salt.
```

换成

```rust
//! the master key from the password, and one session key per salt; and of
//! SS 2022 (SIP022, SIP023): BLAKE3 `derive_key` of a key and a salt.
```

`crates/rurge-proto/src/shadowsocks/kdf.rs`——把

```rust
    hkdf_sha1(master, salt, b"ss-subkey", &mut out);
```

换成

```rust
    hkdf_sha1(master, salt, b"ss-subkey", &mut out);
    out
}

const SESSION_CONTEXT: &str = "shadowsocks 2022 session subkey";
const IDENTITY_CONTEXT: &str = "shadowsocks 2022 identity subkey";

/// `blake3::derive_key(context, key ‖ salt)`, as long as `key` (the first
/// 16 bytes of the output for a 16-byte key: its extendable output).
fn derive_2022(context: &str, key: &[u8], salt: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; key.len()];
    blake3::Hasher::new_derive_key(context)
        .update(key)
        .update(salt)
        .finalize_xof()
        .fill(&mut out);
    out
}

/// SS 2022: the session key of a stream that starts with `salt` (SIP022 2.2).
pub(crate) fn session_subkey_2022(psk: &[u8], salt: &[u8]) -> Vec<u8> {
    derive_2022(SESSION_CONTEXT, psk, salt)
}

/// SIP023: the key an identity header is encrypted with, from an identity
/// key and the stream's salt.
pub(crate) fn identity_subkey(ipsk: &[u8], salt: &[u8]) -> Vec<u8> {
    derive_2022(IDENTITY_CONTEXT, ipsk, salt)
}

/// SIP023: what an identity header says, the first 16 bytes of the next
/// layer's key's BLAKE3 hash.
pub(crate) fn identity_hash(key: &[u8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    out.copy_from_slice(&blake3::hash(key).as_bytes()[..16]);
```

`crates/rurge-proto/src/shadowsocks/cipher.rs`——把

```rust
use super::kdf;
```

换成

```rust
use super::kdf;
use aes::cipher::{BlockDecrypt, BlockEncrypt};
use aes::{Aes128, Aes256};
```

`crates/rurge-proto/src/shadowsocks/cipher.rs`——把

```rust
// the ChaCha ciphers are of the older `aead` generation, with traits of their own
```

换成

```rust
// the ChaCha ciphers (and the `aes` block ciphers) are of the older generation,
// with traits of their own
```

`crates/rurge-proto/src/shadowsocks/cipher.rs`——把

```rust
    key: Vec<u8>,
```

换成

```rust
    key: Vec<u8>,
    /// SS 2022: the key is a PSK and sessions derive with BLAKE3.
    edition_2022: bool,
```

`crates/rurge-proto/src/shadowsocks/cipher.rs`——把

```rust
            key: kdf::evp_bytes_to_key(password.as_bytes(), kind.key_len()),
```

换成

```rust
            key: kdf::evp_bytes_to_key(password.as_bytes(), kind.key_len()),
            edition_2022: false,
        }
    }

    /// SS 2022's key: the (user) PSK itself, `kind.key_len()` bytes.
    pub(crate) fn from_psk(kind: AeadKind, psk: &[u8]) -> MasterKey {
        debug_assert_eq!(psk.len(), kind.key_len());
        MasterKey {
            kind,
            key: psk.to_vec(),
            edition_2022: true,
```

`crates/rurge-proto/src/shadowsocks/cipher.rs`——把

```rust
    /// is HKDF-SHA1 of the master key under that salt.
    pub(crate) fn session(&self, salt: &[u8]) -> CountingAead {
        CountingAead::new(self.kind, &kdf::session_subkey(&self.key, salt))
```

换成

```rust
    /// is HKDF-SHA1 of the master key under that salt, or with SS 2022
    /// BLAKE3 `derive_key` of the PSK and the salt.
    pub(crate) fn session(&self, salt: &[u8]) -> CountingAead {
        let key = if self.edition_2022 {
            kdf::session_subkey_2022(&self.key, salt)
        } else {
            kdf::session_subkey(&self.key, salt)
        };
        CountingAead::new(self.kind, &key)
    }
}

/// One AES block encrypted in place with a 16- or 32-byte key (SS 2022's
/// identity headers and separate headers: ECB of a single block).
pub(crate) fn aes_encrypt_block(key: &[u8], block: &mut [u8; 16]) {
    let block = GenericArray::from_mut_slice(block);
    match key.len() {
        16 => Aes128::new_from_slice(key)
            .expect("16 bytes")
            .encrypt_block(block),
        _ => Aes256::new_from_slice(key)
            .expect("a 32-byte key")
            .encrypt_block(block),
    }
}

/// The inverse of `aes_encrypt_block` (the fake server's side).
#[cfg(any(test, feature = "testing"))]
pub(crate) fn aes_decrypt_block(key: &[u8], block: &mut [u8; 16]) {
    let block = GenericArray::from_mut_slice(block);
    match key.len() {
        16 => Aes128::new_from_slice(key)
            .expect("16 bytes")
            .decrypt_block(block),
        _ => Aes256::new_from_slice(key)
            .expect("a 32-byte key")
            .decrypt_block(block),
```

`crates/rurge-proto/src/shadowsocks/aead.rs`——把

```rust
//! the direction's session key and the next value of its counting nonce.
```

换成

```rust
//! the direction's session key and the next value of its counting nonce.
//!
//! SS 2022 (SIP022 3.1) is the same stream with larger chunks and headers:
//! the request puts the identity headers behind its salt and turns its first
//! write into a fixed-length header chunk and a variable-length one (the
//! address, padding, the initial payload); the response opens with a
//! fixed-length header chunk that names our salt and the length of its first
//! payload chunk.
```

`crates/rurge-proto/src/shadowsocks/aead.rs`——把

```rust
use super::cipher::{CountingAead, MasterKey, TAG};
```

换成

```rust
use super::cipher::{CountingAead, MasterKey, TAG};
use super::s2022;
```

`crates/rurge-proto/src/shadowsocks/aead.rs`——把

```rust
        filled: usize,
    },
    Len {
```

换成

```rust
        filled: usize,
    },
    /// SS 2022: the response's fixed-length header.
    Head {
        buf: Vec<u8>,
        filled: usize,
    },
    Len {
```

`crates/rurge-proto/src/shadowsocks/aead.rs`——把

```rust
    Eof,
```

换成

```rust
    Eof,
}

/// What the 2022 edition adds to a request stream.
pub(crate) struct Request2022 {
    /// The identity headers, between the salt and the first chunk.
    pub identity: Vec<u8>,
    /// The length of the address the first write starts with (a
    /// `LazyHead` above guarantees that it does).
    pub addr_len: usize,
    /// Seconds since the Unix epoch.
    pub now: fn() -> u64,
}

/// A 2022 request stream's state: its setup and its salt, which the
/// response must echo.
struct Edition2022 {
    request: Request2022,
    salt: Vec<u8>,
```

`crates/rurge-proto/src/shadowsocks/aead.rs`——把

```rust
    reading: Reading,
```

换成

```rust
    reading: Reading,
    /// `None`: the AEAD edition.
    edition_2022: Option<Edition2022>,
```

`crates/rurge-proto/src/shadowsocks/aead.rs`——把

```rust
    io::Error::new(io::ErrorKind::UnexpectedEof, CUT_SHORT)
```

换成

```rust
    io::Error::new(io::ErrorKind::UnexpectedEof, CUT_SHORT)
}

/// Seals the first write of a 2022 request into its two header chunks: the
/// address, padding when there is no payload, and as much of the payload as
/// the variable-length header holds. The bytes of `data` it took.
fn seal_first_2022(
    up: &mut CountingAead,
    request: &Request2022,
    data: &[u8],
    out: &mut Vec<u8>,
) -> io::Result<usize> {
    let addr_len = request.addr_len.min(data.len());
    let room = s2022::MAX_VARIABLE_HEADER - addr_len - 2;
    let payload = &data[addr_len..data.len().min(addr_len + room)];
    let padding = s2022::padding_len(payload.len())
        .map_err(|_| io::Error::other("ss: no randomness available"))?;
    let variable = s2022::request_variable(&data[..addr_len], padding, payload);
    out.extend_from_slice(&request.identity);
    up.seal(&s2022::request_fixed((request.now)(), variable.len()), out);
    up.seal(&variable, out);
    Ok(addr_len + payload.len())
```

`crates/rurge-proto/src/shadowsocks/aead.rs`——把

```rust
                filled: 0,
            },
        }
```

换成

```rust
                filled: 0,
            },
            edition_2022: None,
        }
    }

    /// An SS 2022 request stream: `key` is the user key's.
    pub(crate) fn new_2022(
        inner: BoxedStream,
        key: Arc<MasterKey>,
        salt: Vec<u8>,
        request: Request2022,
    ) -> AeadStream {
        let edition = Edition2022 {
            request,
            salt: salt.clone(),
        };
        AeadStream {
            edition_2022: Some(edition),
            ..AeadStream::new(inner, key, salt, s2022::MAX_PAYLOAD)
        }
```

`crates/rurge-proto/src/shadowsocks/aead.rs`——把

```rust
                    this.reading = Reading::Len {
                        buf: [0; 2 + TAG],
```

换成

```rust
                    this.reading = match &this.edition_2022 {
                        Some(edition) => Reading::Head {
                            buf: vec![0; s2022::response_fixed_len(edition.salt.len()) + TAG],
                            filled: 0,
                        },
                        None => Reading::Len {
                            buf: [0; 2 + TAG],
                            filled: 0,
                        },
                    };
                }
                Reading::Head { buf, filled } => {
                    if !ready!(poll_fill(&mut this.inner, cx, buf, filled))? {
                        return Poll::Ready(Err(cut_short()));
                    }
                    let down = this.down.as_mut().expect("the salt came first");
                    let Some(n) = down.open(buf) else {
                        return Poll::Ready(Err(invalid(UNDECRYPTABLE)));
                    };
                    let edition = this.edition_2022.as_ref().expect("a 2022 stream");
                    let len =
                        s2022::check_response(&buf[..n], &edition.salt, (edition.request.now)())?;
                    this.reading = Reading::Body {
                        buf: vec![0; len + TAG],
```

`crates/rurge-proto/src/shadowsocks/aead.rs`——把

```rust
            let n = data.len().min(this.max_payload);
            this.out.clear();
            this.out_pos = 0;
            if let Some(salt) = this.salt.take() {
                this.out.extend_from_slice(&salt);
            }
            seal_chunk(&mut this.up, &data[..n], &mut this.out);
            this.accepted = n;
```

换成

```rust
            this.out.clear();
            this.out_pos = 0;
            let first = this.salt.take();
            if let Some(salt) = &first {
                this.out.extend_from_slice(salt);
            }
            this.accepted = match (&first, &this.edition_2022) {
                (Some(_), Some(edition)) => {
                    match seal_first_2022(&mut this.up, &edition.request, data, &mut this.out) {
                        Ok(n) => n,
                        Err(e) => {
                            this.out.clear();
                            return Poll::Ready(Err(e));
                        }
                    }
                }
                _ => {
                    let n = data.len().min(this.max_payload);
                    seal_chunk(&mut this.up, &data[..n], &mut this.out);
                    n
                }
            };
```

出站：去掉 Task 3 的 2022 构建错误，加身份头与时钟：

`crates/rurge-proto/src/shadowsocks/mod.rs`——把

```rust
//! (`aead`) — or, with `none`, the bytes as they are. The request header is
//! the target as a SOCKS5 address; it waits in a `LazyHead` above the
//! stream for the first payload, so both are sealed into the first chunk.
```

换成

```rust
//! (`aead`) — in its SS 2022 form for the `2022-blake3-*` methods, with the
//! identity headers of a multi-user key (`s2022`) — or, with `none`, the
//! bytes as they are. The request header is the target as a SOCKS5 address;
//! it waits in a `LazyHead` above the stream for the first payload, so both
//! are sealed into the first chunk (SS 2022: the two header chunks).
```

`crates/rurge-proto/src/shadowsocks/mod.rs`——把

```rust
//! as data that does not decrypt.
```

换成

```rust
//! as data that does not decrypt. SS 2022's answer names the request it
//! belongs to and the server's time, which the stream checks.
```

`crates/rurge-proto/src/shadowsocks/mod.rs`——把

```rust
pub(crate) mod kdf;
```

换成

```rust
pub(crate) mod kdf;
pub(crate) mod s2022;
```

`crates/rurge-proto/src/shadowsocks/mod.rs`——把

```rust
use aead::AeadStream;
```

换成

```rust
use aead::{AeadStream, Request2022};
```

`crates/rurge-proto/src/shadowsocks/mod.rs`——把

```rust
use rustls::RootCertStore;
```

换成

```rust
use rustls::RootCertStore;
use s2022::Identity;
```

`crates/rurge-proto/src/shadowsocks/mod.rs`——把

```rust
    /// `None`: the method is `none`.
    key: Option<Arc<MasterKey>>,
```

换成

```rust
    /// `None`: the method is `none`. SS 2022: the user key's.
    key: Option<Arc<MasterKey>>,
    /// `Some` for SS 2022, with no layers for a single key.
    identity: Option<Identity>,
    /// Seconds since the Unix epoch, for SS 2022's timestamps.
    now: fn() -> u64,
```

`crates/rurge-proto/src/shadowsocks/mod.rs`——把

```rust
        if spec.method.is_2022() {
            return Err(BuildError::new(format!(
                "ss: `{}` is not supported yet",
                spec.method.name()
            )));
        }
        let key = match AeadKind::of(spec.method) {
            None => None,
```

换成

```rust
        let (key, identity) = match AeadKind::of(spec.method) {
            None => (None, None),
            Some(kind) if spec.method.is_2022() => {
                // the configuration checked them; a spec made by hand may not be
                let keys = spec.keys.expose();
                let Some(user) = keys.last() else {
                    return Err(BuildError::new("`password` is empty"));
                };
                if keys.iter().any(|key| key.len() != kind.key_len()) {
                    return Err(BuildError::new(format!(
                        "`password` is not Base64 keys of the length `{}` requires",
                        spec.method.name()
                    )));
                }
                (
                    Some(Arc::new(MasterKey::from_psk(kind, user))),
                    Some(Identity::new(keys)),
                )
            }
```

`crates/rurge-proto/src/shadowsocks/mod.rs`——把

```rust
            Some(kind) => Some(Arc::new(MasterKey::from_password(
                kind,
                spec.password.expose(),
            ))),
```

换成

```rust
            Some(kind) => (
                Some(Arc::new(MasterKey::from_password(
                    kind,
                    spec.password.expose(),
                ))),
                None,
            ),
```

`crates/rurge-proto/src/shadowsocks/mod.rs`——把

```rust
            key,
        })
```

换成

```rust
            key,
            identity,
            now: s2022::unix_now,
        })
    }

    /// Runs SS 2022's timestamps off another clock.
    #[cfg(test)]
    fn with_clock(self, now: fn() -> u64) -> ShadowsocksOutbound {
        ShadowsocksOutbound { now, ..self }
```

`crates/rurge-proto/src/shadowsocks/mod.rs`——把

```rust
            let stream: BoxedStream = match &self.key {
                None => transport,
                Some(key) => Box::new(AeadStream::new(
```

换成

```rust
            let stream: BoxedStream = match (&self.key, &self.identity) {
                (None, _) => transport,
                (Some(key), None) => Box::new(AeadStream::new(
```

`crates/rurge-proto/src/shadowsocks/mod.rs`——把

```rust
                )),
```

换成

```rust
                )),
                (Some(key), Some(identity)) => {
                    let request = Request2022 {
                        identity: identity.headers(&salt),
                        addr_len: head.len(),
                        now: self.now,
                    };
                    Box::new(AeadStream::new_2022(transport, key.clone(), salt, request))
                }
```

要点：
- 契约：写进 2022 流的第一次写入以完整的地址（`addr_len` 字节）开头——`LazyHead` 保证这一点；流自己把地址与负载分开、把填充放在两者之间。
- 用户密钥（最后一段）派生正文的子密钥；`Identity` 把每段身份密钥与下一段的哈希配对。
- 应答头之后读到 EOF 是 `ss: the connection ended in the middle of a chunk`。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-proto --lib shadowsocks` → 通过（新增 15 条：BLAKE3 子密钥与身份哈希、两层身份头、请求头布局、填充、应答校验、2022 的写与读的已知答案、单密钥与多用户的往返（含 obfs=tls）、IDN 与 0xFFFF 的块、客户端不说话时的填充、未知密钥、客户端时钟差一小时、重放的请求 salt、应答的时钟与 salt 不对）。

- [ ] **Step 5: 门禁与提交**

跑门禁（加入 `blake3` 后依赖 `rurge-proto` 的测试程序全部重新链接，先看 `df -h /d`）。

```bash
git add Cargo.toml Cargo.lock crates/rurge-proto
git commit -m "feat(proto): SS 2022 的 TCP——BLAKE3 子密钥、请求与应答头、多用户身份头、时钟校验"
```

### Task 5: UDP（AEAD、`none`、SS 2022）

`ShadowsocksOutbound` 的 `udp()` / `open_udp()`（P13）：`SsUdp` 是全锥的 `PacketSocket`，每个策略一份共享的包加解密（`Packets`），2022 每个载体一个随机 session id、包号从 0 起、防重放窗口（P12）。`FakeShadowsocks` 加 UDP（独立的编解码，每个客户端会话一个中继 socket，谁都可以回包）。

**Files:**
- Create: `crates/rurge-proto/src/shadowsocks/udp.rs`（自带用例）
- Modify: `crates/rurge-proto/src/shadowsocks/mod.rs`（与用例）、`cipher.rs`、`s2022.rs`、`src/socks5.rs`、`src/testing/mod.rs`、`src/testing/shadowsocks.rs`

**Interfaces:**
- Consumes: Task 3 / 4 的 `MasterKey`、`AeadCipher`、`aes_encrypt_block` / `aes_decrypt_block`、`Identity`、`padding_len`、`TIME_WINDOW`；M5a 的 `PacketSocket` / `Connector::open_udp`；socks5 的 `from_relay`。
- Produces:
  - `udp.rs`：`pub(crate) enum Packets { Plain, Aead(Arc<MasterKey>), S2022(Keys2022) }`、`pub(crate) struct Keys2022`（`new(kind, keys: &[Vec<u8>])`）、`pub(crate) struct SsUdp`（`async fn open(socket: BoxedPacketSocket, server: &Target, packets: Arc<Packets>, now: fn() -> u64) -> Result<SsUdp, OutboundError>`，实现 `PacketSocket`）
  - `s2022.rs`：`Identity::packet_headers(&self, separate: &[u8; 16]) -> Vec<u8>`；`TIME_WINDOW` 改为 `pub(crate)`
  - `socks5::from_relay` 改为 `pub(crate)`；`aes_decrypt_block` 不再只供测试
  - 测试设施：`ShadowsocksScript` 加 `udp_apart`、`udp_twice`、`udp_garbled_first`；`pub struct RecordedDatagram { target, payload, session, padding, user }`；`FakeShadowsocks::udp_addr()` / `datagrams()` / `udp_outside()` / `udp_rejected()`

- [ ] **Step 1: 先写用例（连同假服务端的 UDP）**

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
//! closes its side and waits for the client to go away.
```

换成

```rust
//! closes its side and waits for the client to go away.
//!
//! UDP: a loopback socket on the TCP port's number (or, with `udp_apart`, a
//! port of its own). Each client session (SS 2022: its session id; else the
//! client's address) relays through a socket of its own, and whatever
//! reaches that socket goes back with its source — full cone, as the
//! reference servers do. Packets it cannot use are dropped and counted.
```

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
use crate::shadowsocks::cipher::{AeadCipher, AeadKind, TAG, aes_decrypt_block};
```

换成

```rust
use crate::shadowsocks::cipher::{AeadCipher, AeadKind, TAG, aes_decrypt_block, aes_encrypt_block};
```

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
use std::collections::HashSet;
```

换成

```rust
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
```

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
use tokio::net::{TcpListener, TcpStream};
```

换成

```rust
use tokio::net::{TcpListener, TcpStream, UdpSocket};
```

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
    /// SS 2022: answer naming a salt that is not the request's.
    pub wrong_request_salt: bool,
```

换成

```rust
    /// SS 2022: answer naming a salt (UDP: a client session) that is not
    /// the request's.
    pub wrong_request_salt: bool,
    /// UDP on a port of its own rather than on the TCP port's number.
    pub udp_apart: bool,
    /// UDP: every answer goes out twice (SS 2022: the copy is a replay).
    pub udp_twice: bool,
    /// UDP: every answer goes after a copy of it with a bit flipped (with
    /// `none`, a different answer).
    pub udp_garbled_first: bool,
```

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
            wrong_request_salt: false,
```

换成

```rust
            wrong_request_salt: false,
            udp_apart: false,
            udp_twice: false,
            udp_garbled_first: false,
```

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
pub struct FakeShadowsocks {
    addr: SocketAddr,
```

换成

```rust
/// A client datagram the fake relayed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedDatagram {
    /// `host:port`, the name as it was on the wire.
    pub target: String,
    pub payload: Vec<u8>,
    /// SS 2022: the client's session id and the packet's id.
    pub session: Option<(u64, u64)>,
    /// SS 2022: the packet's padding length.
    pub padding: usize,
    /// SS 2022 with users: which one's key the identity header named.
    pub user: Option<usize>,
}

pub struct FakeShadowsocks {
    addr: SocketAddr,
    udp_addr: SocketAddr,
```

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
    _task: AbortOnDrop,
```

换成

```rust
    _task: AbortOnDrop,
    _udp: AbortOnDrop,
```

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
    salts: Mutex<HashSet<Vec<u8>>>,
```

换成

```rust
    salts: Mutex<HashSet<Vec<u8>>>,
    datagrams: Mutex<Vec<RecordedDatagram>>,
    /// Each UDP client session's own socket, in the order they opened.
    outside: Mutex<Vec<SocketAddr>>,
    udp_rejected: AtomicUsize,
```

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
impl FakeShadowsocks {
    pub async fn spawn(script: ShadowsocksScript) -> FakeShadowsocks {
```

换成

```rust
/// A TCP listener and a UDP socket on the same port number, or, `apart`, on
/// different ones.
async fn bind(apart: bool) -> (TcpListener, UdpSocket) {
    for _ in 0..64 {
```

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
        let addr = listener.local_addr().expect("local addr");
```

换成

```rust
        let port = listener.local_addr().expect("local addr").port();
        let udp = if apart {
            UdpSocket::bind("127.0.0.1:0")
                .await
                .ok()
                .filter(|udp| udp.local_addr().is_ok_and(|a| a.port() != port))
        } else {
            UdpSocket::bind(("127.0.0.1", port)).await.ok()
        };
        if let Some(udp) = udp {
            return (listener, udp);
        }
    }
    panic!("no loopback port for both TCP and UDP");
}

/// Which client session a datagram belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum ClientKey {
    Addr(SocketAddr),
    Session(u64),
}

/// What it takes to answer one client session.
#[derive(Clone)]
enum Answering {
    Plain,
    Aead(AeadKind),
    S2022 {
        kind: AeadKind,
        user: Vec<u8>,
        client: [u8; 8],
    },
}

/// A client datagram, opened.
struct Opened {
    key: ClientKey,
    answering: Answering,
    record: RecordedDatagram,
    host: String,
    port: u16,
}

/// `sealed` (its tag last) opened with `cipher` under `nonce`.
fn open_sealed(cipher: &AeadCipher, nonce: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
    let (data, tag) = sealed.split_at(sealed.len().checked_sub(TAG)?);
    let mut data = data.to_vec();
    let tag: [u8; TAG] = tag.try_into().ok()?;
    cipher.open_in_place(nonce, &mut data, &tag).then_some(data)
}

/// `plain` sealed with `cipher` under `nonce`, its tag last.
fn seal_plain(cipher: &AeadCipher, nonce: &[u8], plain: &[u8]) -> Vec<u8> {
    let mut data = plain.to_vec();
    let tag = cipher.seal_in_place(nonce, &mut data);
    data.extend_from_slice(&tag);
    data
}

/// The target and the payload of a datagram's plaintext.
fn target_of(plain: &[u8]) -> Option<(String, u16, Vec<u8>)> {
    let (request, _) = parse_address(plain)?;
    Some((request.host, request.port, request.early))
}

/// A client datagram from `from`; `None` for one it cannot use.
fn open_datagram(script: &ShadowsocksScript, packet: &[u8], from: SocketAddr) -> Option<Opened> {
    let (plain, answering) = match AeadKind::of(script.method) {
        None => (packet.to_vec(), Answering::Plain),
        Some(kind) if script.method.is_2022() => return open_datagram_2022(script, kind, packet),
        Some(kind) => {
            let salt = packet.get(..kind.key_len())?;
            let master = kdf::evp_bytes_to_key(script.password.as_bytes(), kind.key_len());
            let cipher = AeadCipher::new(kind, &kdf::session_subkey(&master, salt));
            let nonce = [0u8; 24];
            let plain = open_sealed(&cipher, &nonce[..kind.nonce_len()], &packet[salt.len()..])?;
            (plain, Answering::Aead(kind))
        }
    };
    let (host, port, payload) = target_of(&plain)?;
    Some(Opened {
        key: ClientKey::Addr(from),
        answering,
        record: RecordedDatagram {
            target: format!("{host}:{port}"),
            payload,
            session: None,
            padding: 0,
            user: None,
        },
        host,
        port,
    })
}

/// SS 2022 (SIP022 3.2, SIP023): the separate header under the server's
/// key, an identity header when the script has users, the body.
fn open_datagram_2022(script: &ShadowsocksScript, kind: AeadKind, packet: &[u8]) -> Option<Opened> {
    let server_key = decode_key(&script.password);
    let mut separate: [u8; 16] = packet.get(..16)?.try_into().ok()?;
    aes_decrypt_block(&server_key, &mut separate);
    let (psk, user, body_at) = if script.users.is_empty() {
        (server_key, None, 16)
    } else {
        let mut block: [u8; 16] = packet.get(16..32)?.try_into().ok()?;
        aes_decrypt_block(&server_key, &mut block);
        for (byte, mask) in block.iter_mut().zip(separate) {
            *byte ^= mask;
        }
        let users: Vec<Vec<u8>> = script.users.iter().map(|u| decode_key(u)).collect();
        let i = users.iter().position(|u| kdf::identity_hash(u) == block)?;
        (users[i].clone(), Some(i), 32)
    };
    let client: [u8; 8] = separate[..8].try_into().ok()?;
    let cipher = AeadCipher::new(kind, &kdf::session_subkey_2022(&psk, &client));
    let body = open_sealed(&cipher, &separate[4..], packet.get(body_at..)?)?;
    let time = u64::from_be_bytes(body.get(1..9)?.try_into().ok()?);
    if body[0] != 0 || time.abs_diff(unix_time()) > 30 {
        return None;
    }
    let padding = usize::from(u16::from_be_bytes(body.get(9..11)?.try_into().ok()?));
    let (host, port, payload) = target_of(body.get(11 + padding..)?)?;
    let session = u64::from_be_bytes(client);
    Some(Opened {
        key: ClientKey::Session(session),
        answering: Answering::S2022 {
            kind,
            user: psk,
            client,
        },
        record: RecordedDatagram {
            target: format!("{host}:{port}"),
            payload,
            session: Some((session, u64::from_be_bytes(separate[8..].try_into().ok()?))),
            padding,
            user,
        },
        host,
        port,
    })
}

/// `from` as `ATYP ADDR PORT`.
fn socks_address(from: SocketAddr) -> Vec<u8> {
    let mut out = Vec::with_capacity(19);
    match from.ip() {
        IpAddr::V4(ip) => {
            out.push(1);
            out.extend_from_slice(&ip.octets());
        }
        IpAddr::V6(ip) => {
            out.push(4);
            out.extend_from_slice(&ip.octets());
        }
    }
    out.extend_from_slice(&from.port().to_be_bytes());
    out
}

/// The answer `payload` from `from`: packet `packet` of server session
/// `server` (SS 2022).
fn seal_answer(
    script: &ShadowsocksScript,
    answering: &Answering,
    server: &[u8; 8],
    packet: u64,
    from: SocketAddr,
    payload: &[u8],
) -> Vec<u8> {
    let addr = socks_address(from);
    match answering {
        Answering::Plain => [&addr[..], payload].concat(),
        Answering::Aead(kind) => {
            let mut salt = vec![0u8; kind.key_len()];
            getrandom::fill(&mut salt).expect("randomness");
            let master = kdf::evp_bytes_to_key(script.password.as_bytes(), kind.key_len());
            let cipher = AeadCipher::new(*kind, &kdf::session_subkey(&master, &salt));
            let nonce = [0u8; 24];
            let sealed = seal_plain(
                &cipher,
                &nonce[..kind.nonce_len()],
                &[&addr[..], payload].concat(),
            );
            [salt, sealed].concat()
        }
        Answering::S2022 { kind, user, client } => {
            let mut separate = [0u8; 16];
            separate[..8].copy_from_slice(server);
            separate[8..].copy_from_slice(&packet.to_be_bytes());
            let mut echoed = *client;
            if script.wrong_request_salt {
                echoed[0] ^= 1;
            }
            let time = unix_time().saturating_add_signed(script.answer_skew);
            let mut body = vec![1];
            body.extend_from_slice(&time.to_be_bytes());
            body.extend_from_slice(&echoed);
            body.extend_from_slice(&[0, 0]);
            body.extend_from_slice(&addr);
            body.extend_from_slice(payload);
            let cipher = AeadCipher::new(*kind, &kdf::session_subkey_2022(user, server));
            let sealed = seal_plain(&cipher, &separate[4..], &body);
            // the answers' separate headers are under the user's key
            aes_encrypt_block(user, &mut separate);
            [&separate[..], &sealed].concat()
        }
    }
}

/// One client session's relay.
struct UdpClient {
    outside: Arc<UdpSocket>,
    /// Where the client last sent from: the answers go there.
    client: Arc<Mutex<SocketAddr>>,
    /// SS 2022: the packet ids seen.
    packets: HashSet<u64>,
    _answers: AbortOnDrop,
}

impl UdpClient {
    async fn open(
        socket: &Arc<UdpSocket>,
        shared: &Arc<Shared>,
        answering: Answering,
        from: SocketAddr,
    ) -> io::Result<UdpClient> {
        let outside = Arc::new(UdpSocket::bind("127.0.0.1:0").await?);
        shared
            .seen
            .outside
            .lock()
            .expect("outside")
            .push(outside.local_addr()?);
        let client = Arc::new(Mutex::new(from));
        let task = tokio::spawn(answer(
            socket.clone(),
            outside.clone(),
            client.clone(),
            shared.clone(),
            answering,
        ));
        Ok(UdpClient {
            outside,
            client,
            packets: HashSet::new(),
            _answers: AbortOnDrop(task),
        })
    }
}

/// Sends whatever reaches `outside`, from anyone, to the client.
async fn answer(
    socket: Arc<UdpSocket>,
    outside: Arc<UdpSocket>,
    client: Arc<Mutex<SocketAddr>>,
    shared: Arc<Shared>,
    answering: Answering,
) {
    let script = &shared.script;
    let mut server = [0u8; 8];
    getrandom::fill(&mut server).expect("randomness");
    let mut packet = 0u64;
    let mut buf = vec![0u8; 65536];
    loop {
        let (n, from) = match outside.recv_from(&mut buf).await {
            Ok(got) => got,
            // an ICMP "unreachable" for an earlier datagram (Windows)
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
            Err(_) => return,
        };
        let sealed = seal_answer(script, &answering, &server, packet, from, &buf[..n]);
        packet += 1;
        let to = *client.lock().expect("client");
        if script.udp_garbled_first {
            let mut garbled = sealed.clone();
            let last = garbled.len() - 1;
            garbled[last] ^= 1;
            let _ = socket.send_to(&garbled, to).await;
        }
        let _ = socket.send_to(&sealed, to).await;
        if script.udp_twice {
            let _ = socket.send_to(&sealed, to).await;
        }
    }
}

async fn serve_udp(socket: Arc<UdpSocket>, shared: Arc<Shared>) {
    let mut clients: HashMap<ClientKey, UdpClient> = HashMap::new();
    let mut buf = vec![0u8; 65536];
    loop {
        let (n, from) = match socket.recv_from(&mut buf).await {
            Ok(got) => got,
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
            Err(_) => return,
        };
        let seen = &shared.seen;
        let Some(opened) = open_datagram(&shared.script, &buf[..n], from) else {
            seen.udp_rejected.fetch_add(1, Ordering::SeqCst);
            continue;
        };
        let client = match clients.entry(opened.key) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let open = UdpClient::open(&socket, &shared, opened.answering, from);
                let Ok(client) = open.await else {
                    continue;
                };
                entry.insert(client)
            }
        };
        if let Some((_, packet)) = opened.record.session
            && !client.packets.insert(packet)
        {
            seen.udp_rejected.fetch_add(1, Ordering::SeqCst);
            continue;
        }
        *client.client.lock().expect("client") = from;
        seen.datagrams
            .lock()
            .expect("datagrams")
            .push(opened.record.clone());
        let to = match (opened.host.parse::<IpAddr>(), shared.script.connect_to) {
            (Ok(ip), _) => SocketAddr::new(ip, opened.port),
            (Err(_), Some(addr)) => addr,
            // never resolves: a name without `connect_to` is a dead end
            (Err(_), None) => continue,
        };
        let _ = client.outside.send_to(&opened.record.payload, to).await;
    }
}

impl FakeShadowsocks {
    pub async fn spawn(script: ShadowsocksScript) -> FakeShadowsocks {
        let (listener, udp) = bind(script.udp_apart).await;
        let addr = listener.local_addr().expect("local addr");
        let udp_addr = udp.local_addr().expect("local addr");
```

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
        FakeShadowsocks {
            addr,
```

换成

```rust
        let udp_task = tokio::spawn(serve_udp(Arc::new(udp), shared.clone()));
        FakeShadowsocks {
            addr,
            udp_addr,
```

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
            _task: AbortOnDrop(task),
```

换成

```rust
            _task: AbortOnDrop(task),
            _udp: AbortOnDrop(udp_task),
```

`crates/rurge-proto/src/testing/shadowsocks.rs`——把

```rust
        self.addr
```

换成

```rust
        self.addr
    }

    /// Where it takes UDP: the TCP port's number unless `udp_apart`.
    pub fn udp_addr(&self) -> SocketAddr {
        self.udp_addr
    }

    /// Every client datagram relayed so far.
    pub fn datagrams(&self) -> Vec<RecordedDatagram> {
        self.shared
            .seen
            .datagrams
            .lock()
            .expect("datagrams")
            .clone()
    }

    /// Each UDP client session's own socket: whatever reaches it goes back
    /// to that client.
    pub fn udp_outside(&self) -> Vec<SocketAddr> {
        self.shared.seen.outside.lock().expect("outside").clone()
    }

    /// Client datagrams dropped: not decrypted, malformed, off the clock or
    /// replayed.
    pub fn udp_rejected(&self) -> usize {
        self.shared.seen.udp_rejected.load(Ordering::SeqCst)
```

`crates/rurge-proto/src/testing/mod.rs`——把

```rust
pub use shadowsocks::{FakeShadowsocks, RecordedShadowsocks, ShadowsocksScript};
```

换成

```rust
pub use shadowsocks::{FakeShadowsocks, RecordedDatagram, RecordedShadowsocks, ShadowsocksScript};
```

出站的用例：

`crates/rurge-proto/src/shadowsocks/mod.rs`——把

```rust
    use crate::UdpSupport;
    use crate::addr::socks_addr;
    use crate::testing::{FakeShadowsocks, ShadowsocksScript, echo_server};
```

换成

```rust
    use crate::addr::socks_addr;
    use crate::testing::{FakeShadowsocks, ShadowsocksScript, echo_server, udp_echo_server};
```

`crates/rurge-proto/src/shadowsocks/mod.rs`——把

```rust
    use rurge_net::connector::{DirectConnector, SystemResolve};
```

换成

```rust
    use rurge_net::connector::{DirectConnector, PacketSocket, SystemResolve};
```

`crates/rurge-proto/src/shadowsocks/mod.rs`——把

```rust
            assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        }
    }

    #[test]
```

换成

```rust
            assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        }
    }

    async fn udp_answer(carrier: &dyn PacketSocket) -> (Vec<u8>, Target) {
        let mut buf = vec![0u8; 65536];
        let (n, from) = tokio::time::timeout(Duration::from_secs(5), carrier.recv_from(&mut buf))
            .await
            .expect("an answer within the bound")
            .unwrap();
        (buf[..n].to_vec(), from)
    }

    async fn udp_roundtrip(carrier: &dyn PacketSocket, to: SocketAddr, payload: &[u8]) {
        carrier.send_to(payload, &target(to)).await.unwrap();
        assert_eq!(udp_answer(carrier).await, (payload.to_vec(), target(to)));
    }

    /// Nothing comes back within a short while.
    async fn no_udp_answer(carrier: &dyn PacketSocket) {
        let mut buf = vec![0u8; 65536];
        let got =
            tokio::time::timeout(Duration::from_millis(300), carrier.recv_from(&mut buf)).await;
        assert!(got.is_err(), "no answer: {got:?}");
    }

    /// Every method: the script, the line's `password` and the user the
    /// fake should find.
    fn udp_setups() -> Vec<(ShadowsocksScript, String, Option<usize>)> {
        let mut setups: Vec<_> = AEAD
            .into_iter()
            .chain([SsMethod::None])
            .map(|method| (ShadowsocksScript::new(method, "pw"), "pw".to_string(), None))
            .collect();
        for method in SS_2022 {
            let (server, user) = (key_2022(method, 1), key_2022(method, 2));
            setups.push((
                ShadowsocksScript::new(method, &server),
                server.clone(),
                None,
            ));
            setups.push((
                ShadowsocksScript {
                    users: vec![key_2022(method, 3), user.clone()],
                    ..ShadowsocksScript::new(method, &server)
                },
                format!("{server}:{user}"),
                Some(1),
            ));
        }
        setups
    }

    fn udp_outbound(
        fake: &FakeShadowsocks,
        method: SsMethod,
        password: &str,
    ) -> ShadowsocksOutbound {
        outbound(&format!(
            "ss, 127.0.0.1, {}, encrypt-method={}, password={password}, udp-relay=true",
            fake.addr().port(),
            method.name()
        ))
    }

    /// A password for `method`: a key for SS 2022.
    fn udp_password(method: SsMethod) -> String {
        if method.is_2022() {
            key_2022(method, 1)
        } else {
            "pw".to_string()
        }
    }

    #[tokio::test]
    async fn udp_round_trips_with_every_method() {
        let (one, two) = (udp_echo_server().await, udp_echo_server().await);
        for (script, password, user) in udp_setups() {
            let method = script.method;
            let fake = FakeShadowsocks::spawn(script).await;
            let out = udp_outbound(&fake, method, &password);
            assert_eq!(out.udp(), UdpSupport::Native);
            let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
            udp_roundtrip(carrier.as_ref(), one, b"to one").await;
            udp_roundtrip(carrier.as_ref(), two, b"to two").await;
            // another carrier is another client session
            let other = out.open_udp(&ConnectOpts::default()).await.unwrap();
            udp_roundtrip(other.as_ref(), one, b"again").await;
            let seen = fake.datagrams();
            let targets: Vec<_> = seen.iter().map(|d| d.target.clone()).collect();
            assert_eq!(
                targets,
                [one.to_string(), two.to_string(), one.to_string()],
                "{method:?}"
            );
            assert_eq!(seen[1].payload, b"to two");
            if method.is_2022() {
                let ids: Vec<(u64, u64)> = seen.iter().map(|d| d.session.unwrap()).collect();
                assert_eq!(ids[0].0, ids[1].0, "one session");
                assert_eq!((ids[0].1, ids[1].1), (0, 1), "counting from 0");
                assert_ne!(ids[2].0, ids[0].0, "{method:?}");
                assert_eq!(ids[2].1, 0);
                assert!(seen.iter().all(|d| d.user == user && d.padding == 0));
            } else {
                assert!(seen.iter().all(|d| d.session.is_none()));
            }
        }
    }

    #[tokio::test]
    async fn udp_goes_to_udp_port_when_it_is_written() {
        let echo = udp_echo_server().await;
        for method in [SsMethod::Aes128Gcm, SsMethod::Blake3Aes256Gcm] {
            let password = udp_password(method);
            let fake = FakeShadowsocks::spawn(ShadowsocksScript {
                udp_apart: true,
                ..ShadowsocksScript::new(method, &password)
            })
            .await;
            assert_ne!(fake.udp_addr().port(), fake.addr().port());
            let out = outbound(&format!(
                "ss, 127.0.0.1, {}, encrypt-method={}, password={password}, udp-relay=true, udp-port={}",
                fake.addr().port(),
                method.name(),
                fake.udp_addr().port()
            ));
            let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
            udp_roundtrip(carrier.as_ref(), echo, b"to the other port").await;
            assert_eq!(fake.connections(), 0, "no TCP");
        }
    }

    /// Whoever sends to the server's socket for this client reaches it, with
    /// their own address.
    #[tokio::test]
    async fn udp_is_full_cone() {
        let echo = udp_echo_server().await;
        for method in [SsMethod::ChaCha20IetfPoly1305, SsMethod::Blake3Aes128Gcm] {
            let password = udp_password(method);
            let fake = FakeShadowsocks::spawn(ShadowsocksScript::new(method, &password)).await;
            let out = udp_outbound(&fake, method, &password);
            let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
            udp_roundtrip(carrier.as_ref(), echo, b"hello").await;
            let stranger = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
            stranger
                .send_to(b"from a stranger", fake.udp_outside()[0])
                .await
                .unwrap();
            assert_eq!(
                udp_answer(carrier.as_ref()).await,
                (
                    b"from a stranger".to_vec(),
                    target(stranger.local_addr().unwrap())
                ),
                "{method:?}"
            );
        }
    }

    #[tokio::test]
    async fn udp_sends_names_to_the_server() {
        let echo = udp_echo_server().await;
        let method = SsMethod::Blake3Aes256Gcm;
        let key = key_2022(method, 1);
        let fake = FakeShadowsocks::spawn(ShadowsocksScript {
            connect_to: Some(echo),
            ..ShadowsocksScript::new(method, &key)
        })
        .await;
        let out = udp_outbound(&fake, method, &key);
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        let name = Target::new(HostName::Domain("bücher.example".into()), 53);
        carrier.send_to(b"a query", &name).await.unwrap();
        // the answer names where it really came from
        assert_eq!(
            udp_answer(carrier.as_ref()).await,
            (b"a query".to_vec(), target(echo))
        );
        assert_eq!(fake.datagrams()[0].target, "xn--bcher-kva.example:53");
        let err = carrier
            .send_to(b"x", &Target::new(HostName::Domain("a@b.test".into()), 53))
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "ss: the host name cannot be sent to the server"
        );
        assert_eq!(fake.datagrams().len(), 1);
    }

    #[tokio::test]
    async fn a_garbled_or_replayed_answer_is_dropped_and_the_next_one_arrives() {
        let echo = udp_echo_server().await;
        let garbled = |method: SsMethod| ShadowsocksScript {
            udp_garbled_first: true,
            ..ShadowsocksScript::new(method, &udp_password(method))
        };
        let method = SsMethod::Blake3Aes128Gcm;
        for script in [
            garbled(SsMethod::Aes256Gcm),
            garbled(method),
            ShadowsocksScript {
                udp_twice: true,
                ..ShadowsocksScript::new(method, &udp_password(method))
            },
        ] {
            let method = script.method;
            let fake = FakeShadowsocks::spawn(script).await;
            let out = udp_outbound(&fake, method, &udp_password(method));
            let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
            for payload in [&b"one"[..], b"two", b"three"] {
                udp_roundtrip(carrier.as_ref(), echo, payload).await;
            }
        }
    }

    #[tokio::test]
    async fn a_2022_answer_off_the_clock_or_for_another_session_is_dropped() {
        let echo = udp_echo_server().await;
        let method = SsMethod::Blake3Aes256Gcm;
        let key = key_2022(method, 1);
        for script in [
            ShadowsocksScript {
                answer_skew: 3600,
                ..ShadowsocksScript::new(method, &key)
            },
            ShadowsocksScript {
                wrong_request_salt: true,
                ..ShadowsocksScript::new(method, &key)
            },
        ] {
            let fake = FakeShadowsocks::spawn(script).await;
            let out = udp_outbound(&fake, method, &key);
            let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
            carrier.send_to(b"hello", &target(echo)).await.unwrap();
            no_udp_answer(carrier.as_ref()).await;
            assert_eq!(fake.datagrams().len(), 1, "the server relayed it");
        }
    }

    #[tokio::test]
    async fn the_server_drops_a_wrong_key_or_a_clock_an_hour_off() {
        let echo = udp_echo_server().await;
        let method = SsMethod::Blake3Aes128Gcm;
        let key = key_2022(method, 1);
        let setups = [
            (
                ShadowsocksScript::new(method, &key),
                key_2022(method, 9),
                false,
            ),
            (ShadowsocksScript::new(method, &key), key.clone(), true),
            (
                ShadowsocksScript::new(SsMethod::Aes128Gcm, "right"),
                "wrong".to_string(),
                false,
            ),
        ];
        for (script, password, behind) in setups {
            let method = script.method;
            let fake = FakeShadowsocks::spawn(script).await;
            let mut out = udp_outbound(&fake, method, &password);
            if behind {
                out = out.with_clock(an_hour_behind);
            }
            let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
            carrier.send_to(b"hello", &target(echo)).await.unwrap();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            while fake.udp_rejected() == 0 {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "the server drops it"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert!(fake.datagrams().is_empty());
            no_udp_answer(carrier.as_ref()).await;
        }
    }

    /// Without `udp-relay=true` the policy carries no UDP (the manual: the
    /// server must allow it).
    #[tokio::test]
    async fn no_udp_without_udp_relay() {
        let fake = FakeShadowsocks::spawn(ShadowsocksScript::new(SsMethod::Aes128Gcm, "pw")).await;
        let out = outbound(&format!(
            "ss, 127.0.0.1, {}, encrypt-method=aes-128-gcm, password=pw",
            fake.addr().port()
        ));
        assert_eq!(out.udp(), UdpSupport::Unsupported);
        let err = out.open_udp(&ConnectOpts::default()).await.err().unwrap();
        assert_eq!(
            err.to_string(),
            "policy protocol not implemented: UDP without `udp-relay=true`"
        );
    }

    #[test]
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-proto --lib shadowsocks`
Expected: FAIL——`udp()` 与 `open_udp` 由 Step 3 引入，用例里的 `UdpSupport` 还没有导入，编译不过：

```text
error[E0433]: failed to resolve: use of undeclared type `UdpSupport`
   --> crates\rurge-proto\src\shadowsocks\mod.rs:250:35
error[E0433]: failed to resolve: use of undeclared type `UdpSupport`
   --> crates\rurge-proto\src\shadowsocks\mod.rs:862:35
error[E0433]: failed to resolve: use of undeclared type `UdpSupport`
    --> crates\rurge-proto\src\shadowsocks\mod.rs:1070:31
For more information about this error, try `rustc --explain E0433`.
error: could not compile `rurge-proto` (lib test) due to 3 previous errors
exit 101
```

- [ ] **Step 3: 实现**

新模块（自带用例；已知答案由 Python 独立算出）：

新建 `crates/rurge-proto/src/shadowsocks/udp.rs`：

```rust
//! Shadowsocks over UDP (phase 2 M6 design 3.3): every datagram goes to the
//! server by itself, sealed with the target's SOCKS5 address in front of
//! the payload; the server's answers name their source the same way. One
//! carrier to the server (`udp-port`, else the server's port) carries every
//! target of an association: full cone.
//!
//! - `none`: the address and the payload as they are.
//! - AEAD: a random salt of its own per packet, the session key under it,
//!   the nonce all zeros.
//! - SS 2022 (SIP022 3.2, SIP023): each carrier is a client session with a
//!   random id and packet ids counting from zero; the 16-byte separate
//!   header (session id, packet id) is one AES block under the first key,
//!   followed by the identity headers of a multi-user key, then the body
//!   sealed with a key of the user key and the session id, its nonce the
//!   last 12 bytes of the separate header. The server's packets come from
//!   sessions of its own and are checked for type, time, the client session
//!   they name and, per server session, replays.
//!
//! What does not decrypt or does not check out is dropped, as a socket
//! drops what it cannot use: a debug line, never the payload.

use super::cipher::{AeadCipher, AeadKind, MasterKey, TAG, aes_decrypt_block, aes_encrypt_block};
use super::kdf;
use super::s2022::{self, Identity};
use crate::OutboundError;
use crate::addr::{AddrError, parse_socks_addr, socks_addr};
use crate::socks5::from_relay;
use rurge_config::HostName;
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedPacketSocket, PacketSocket, Target};
use std::io;
use std::net::IpAddr;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const CLIENT_PACKET: u8 = 0;
const SERVER_PACKET: u8 = 1;
/// The separate header: session id and packet id.
const SEPARATE: usize = 16;
/// A server body before its address: type, timestamp, the client's session
/// id, the padding's length.
const SERVER_FIXED: usize = 1 + 8 + 8 + 2;
/// Server sessions whose replay windows a carrier keeps; a new one pushes
/// out the oldest (a server has one per client session, a new one after a
/// restart).
const SERVER_SESSIONS: usize = 8;

/// How a method seals datagrams: one per policy, shared by its carriers.
/// No `Debug`: it holds keys.
pub(crate) enum Packets {
    Plain,
    Aead(Arc<MasterKey>),
    S2022(Keys2022),
}

/// SS 2022's keys for UDP. No `Debug`.
pub(crate) struct Keys2022 {
    kind: AeadKind,
    /// The separate header's key: the first identity key, or the only key.
    first: Vec<u8>,
    /// The bodies' keys derive from it; the server's separate headers are
    /// under it.
    user: Vec<u8>,
    identity: Identity,
}

impl Keys2022 {
    /// `keys` as written: identity keys first, the user key last; never empty.
    pub(crate) fn new(kind: AeadKind, keys: &[Vec<u8>]) -> Keys2022 {
        Keys2022 {
            kind,
            first: keys[0].clone(),
            user: keys[keys.len() - 1].clone(),
            identity: Identity::new(keys),
        }
    }

    /// The cipher of the bodies of `session`'s packets.
    fn body_cipher(&self, session: &[u8; 8]) -> AeadCipher {
        AeadCipher::new(self.kind, &kdf::session_subkey_2022(&self.user, session))
    }

    /// Packet `packet` of client session `session` (its body cipher
    /// `cipher`) to `addr`, with `padding` bytes of padding.
    #[allow(clippy::too_many_arguments)]
    fn seal(
        &self,
        session: &[u8; 8],
        cipher: &AeadCipher,
        packet: u64,
        now: u64,
        padding: usize,
        addr: &[u8],
        payload: &[u8],
    ) -> Vec<u8> {
        let mut separate = [0u8; SEPARATE];
        separate[..8].copy_from_slice(session);
        separate[8..].copy_from_slice(&packet.to_be_bytes());
        let mut out = Vec::with_capacity(64 + padding + addr.len() + payload.len());
        let mut header = separate;
        aes_encrypt_block(&self.first, &mut header);
        out.extend_from_slice(&header);
        out.extend_from_slice(&self.identity.packet_headers(&separate));
        let body = out.len();
        out.push(CLIENT_PACKET);
        out.extend_from_slice(&now.to_be_bytes());
        let len = u16::try_from(padding).expect("at most 900 bytes of padding");
        out.extend_from_slice(&len.to_be_bytes());
        // zeros: sealed like the rest
        out.resize(out.len() + padding, 0);
        out.extend_from_slice(addr);
        out.extend_from_slice(payload);
        let tag = cipher.seal_in_place(&separate[4..], &mut out[body..]);
        out.extend_from_slice(&tag);
        out
    }
}

/// An AEAD packet for `addr` under `salt`.
fn seal_aead(key: &MasterKey, salt: &[u8], addr: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut plain = Vec::with_capacity(addr.len() + payload.len());
    plain.extend_from_slice(addr);
    plain.extend_from_slice(payload);
    let mut out = Vec::with_capacity(salt.len() + plain.len() + TAG);
    out.extend_from_slice(salt);
    // a fresh session per packet: its first nonce is all zeros
    key.session(salt).seal(&plain, &mut out);
    out
}

/// The source of a packet whose plaintext `plain` starts at `start` of the
/// packet, and where its payload lies in the packet.
fn source(plain: &[u8], start: usize) -> Result<(Target, Range<usize>), &'static str> {
    let (from, used) = parse_socks_addr(plain).ok_or("no address")?;
    Ok((from, start + used..start + plain.len()))
}

/// An AEAD packet opened in place.
fn open_aead(key: &MasterKey, packet: &mut [u8]) -> Result<(Target, Range<usize>), &'static str> {
    let salt_len = key.salt_len();
    if packet.len() < salt_len + TAG {
        return Err("too short");
    }
    let (salt, sealed) = packet.split_at_mut(salt_len);
    let n = key.session(salt).open(sealed).ok_or("does not decrypt")?;
    source(&sealed[..n], salt_len)
}

/// Packet ids already seen from one server session: the last 8128 of them
/// (SIP022 3.2.4; a ring of 64-bit blocks as WireGuard's replay filter).
struct Window {
    last: u64,
    ring: [u64; RING_BLOCKS],
}

const BLOCK_BITS: u64 = 64;
const RING_BLOCKS: usize = 128;
/// Ids this far behind the newest are still told apart.
const WINDOW: u64 = (RING_BLOCKS as u64 - 1) * BLOCK_BITS;

impl Window {
    fn new() -> Window {
        Window {
            last: 0,
            ring: [0; RING_BLOCKS],
        }
    }

    /// Whether `packet` is new (then it is seen from now on): not seen
    /// before, and not too far behind the newest.
    fn accept(&mut self, packet: u64) -> bool {
        let block = packet / BLOCK_BITS;
        if packet > self.last {
            // the blocks the window moves over are cleared
            let current = self.last / BLOCK_BITS;
            let moved = (block - current).min(RING_BLOCKS as u64);
            for i in 1..=moved {
                self.ring[((current + i) % RING_BLOCKS as u64) as usize] = 0;
            }
            self.last = packet;
        } else if self.last - packet > WINDOW {
            return false;
        }
        let index = (block % RING_BLOCKS as u64) as usize;
        let bit = 1u64 << (packet % BLOCK_BITS);
        let old = self.ring[index];
        self.ring[index] = old | bit;
        old & bit == 0
    }
}

/// One server session a carrier heard from.
struct ServerSession {
    id: [u8; 8],
    cipher: AeadCipher,
    window: Window,
}

/// A carrier's SS 2022 client session. No `Debug`.
struct Session {
    id: [u8; 8],
    cipher: AeadCipher,
    /// The next packet id. A session sends 2^64 packets long after its
    /// carrier is gone.
    next: AtomicU64,
    /// Oldest first.
    servers: Mutex<Vec<ServerSession>>,
}

impl Session {
    /// A server packet opened in place: checked, then counted against its
    /// session's replay window.
    fn open(
        &self,
        keys: &Keys2022,
        packet: &mut [u8],
        now: u64,
    ) -> Result<(Target, Range<usize>), &'static str> {
        if packet.len() < SEPARATE + SERVER_FIXED + TAG {
            return Err("too short");
        }
        let (header, sealed) = packet.split_at_mut(SEPARATE);
        let mut separate: [u8; SEPARATE] = (&*header).try_into().expect("16 bytes");
        // the server's separate headers are under the user key: no identity
        // headers come back
        aes_decrypt_block(&keys.user, &mut separate);
        let id: [u8; 8] = separate[..8].try_into().expect("8 bytes");
        let packet_id = u64::from_be_bytes(separate[8..].try_into().expect("8 bytes"));
        let (body, tag) = sealed.split_at_mut(sealed.len() - TAG);
        let tag: &[u8; TAG] = (&*tag).try_into().expect("16 bytes");
        let mut servers = self.servers.lock().expect("server sessions");
        let known = servers.iter().position(|s| s.id == id);
        let fresh = match known {
            Some(_) => None,
            None => Some(keys.body_cipher(&id)),
        };
        let cipher = match (known, &fresh) {
            (Some(i), _) => &servers[i].cipher,
            (None, fresh) => fresh.as_ref().expect("a new session's cipher"),
        };
        if !cipher.open_in_place(&separate[4..], body, tag) {
            return Err("does not decrypt");
        }
        if body[0] != SERVER_PACKET {
            return Err("not a server's packet");
        }
        let time = u64::from_be_bytes(body[1..9].try_into().expect("8 bytes"));
        if time.abs_diff(now) > s2022::TIME_WINDOW {
            return Err("the server's clock differs from ours by more than 30 seconds");
        }
        if body[9..17] != self.id {
            return Err("for another client session");
        }
        let padding = usize::from(u16::from_be_bytes([body[17], body[18]]));
        let start = SERVER_FIXED + padding;
        let plain = body.get(start..).ok_or("no address")?;
        let (from, range) = source(plain, SEPARATE + start)?;
        // only a packet that checked out moves the window
        let accepted = match known {
            Some(i) => servers[i].window.accept(packet_id),
            None => {
                if servers.len() == SERVER_SESSIONS {
                    servers.remove(0);
                }
                let mut window = Window::new();
                window.accept(packet_id);
                servers.push(ServerSession {
                    id,
                    cipher: fresh.expect("a new session's cipher"),
                    window,
                });
                true
            }
        };
        if !accepted {
            return Err("a replay");
        }
        Ok((from, range))
    }
}

/// One association's carrier to the server. No `Debug`.
pub(crate) struct SsUdp {
    /// The server as looked up when the carrier opened.
    server: Target,
    /// Its address, when the carrier resolved it to one: only datagrams from
    /// there are the server's. A chained carrier keeps names.
    server_ip: Option<IpAddr>,
    socket: BoxedPacketSocket,
    packets: Arc<Packets>,
    /// SS 2022's client session.
    session: Option<Session>,
    now: fn() -> u64,
}

fn no_randomness() -> OutboundError {
    OutboundError::Proxy("ss: no randomness available".to_string())
}

impl SsUdp {
    /// The carrier to `server` through `socket`. The server is looked up
    /// once, here.
    pub(crate) async fn open(
        socket: BoxedPacketSocket,
        server: &Target,
        packets: Arc<Packets>,
        now: fn() -> u64,
    ) -> Result<SsUdp, OutboundError> {
        let server = socket.resolve(server).await.map_err(|e| {
            OutboundError::Proxy(format!("ss: cannot look up the server for UDP: {e}"))
        })?;
        let server_ip = match server.host {
            HostName::Ip(ip) => Some(ip),
            HostName::Domain(_) => None,
        };
        let session = match &*packets {
            Packets::S2022(keys) => {
                let mut id = [0u8; 8];
                getrandom::fill(&mut id).map_err(|_| no_randomness())?;
                Some(Session {
                    id,
                    cipher: keys.body_cipher(&id),
                    next: AtomicU64::new(0),
                    servers: Mutex::new(Vec::new()),
                })
            }
            _ => None,
        };
        Ok(SsUdp {
            server,
            server_ip,
            socket,
            packets,
            session,
            now,
        })
    }

    fn seal(&self, addr: &[u8], payload: &[u8]) -> io::Result<Vec<u8>> {
        let random = |_| io::Error::other("ss: no randomness available");
        Ok(match (&*self.packets, &self.session) {
            (Packets::Aead(key), _) => {
                let mut salt = vec![0u8; key.salt_len()];
                getrandom::fill(&mut salt).map_err(random)?;
                seal_aead(key, &salt, addr, payload)
            }
            (Packets::S2022(keys), Some(session)) => {
                let packet = session.next.fetch_add(1, Ordering::Relaxed);
                let padding = s2022::padding_len(payload.len()).map_err(random)?;
                keys.seal(
                    &session.id,
                    &session.cipher,
                    packet,
                    (self.now)(),
                    padding,
                    addr,
                    payload,
                )
            }
            _ => [addr, payload].concat(),
        })
    }

    fn unseal(&self, packet: &mut [u8]) -> Result<(Target, Range<usize>), &'static str> {
        match (&*self.packets, &self.session) {
            (Packets::Aead(key), _) => open_aead(key, packet),
            (Packets::S2022(keys), Some(session)) => session.open(keys, packet, (self.now)()),
            _ => source(packet, 0),
        }
    }
}

impl PacketSocket for SsUdp {
    fn send_to<'a>(&'a self, buf: &'a [u8], to: &'a Target) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            let addr = socks_addr(to).map_err(|e| {
                io::Error::other(match e {
                    AddrError::Unsendable => "ss: the host name cannot be sent to the server",
                    AddrError::TooLong => "ss: the host name is longer than 255 bytes",
                })
            })?;
            let packet = self.seal(&addr, buf)?;
            self.socket.send_to(&packet, &self.server).await
        })
    }

    /// `buf` takes the whole packet, the payload's address and the
    /// protocol's headers too: give it 64 KiB.
    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
        Box::pin(async move {
            loop {
                let (n, sender) = self.socket.recv_from(buf).await?;
                if !from_relay(self.server_ip, &sender) {
                    continue;
                }
                match self.unseal(&mut buf[..n]) {
                    Ok((from, payload)) => {
                        let len = payload.len();
                        buf.copy_within(payload, 0);
                        return Ok((len, from));
                    }
                    Err(why) => {
                        tracing::debug!("ss: a UDP packet from the server was dropped: {why}")
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vmess::vectors::hex;

    const NOW: u64 = 1_700_000_000;

    /// 127.0.0.1:53, the targets' address in the vectors.
    const QUERY_TO: [u8; 7] = [1, 127, 0, 0, 1, 0, 53];

    fn eight_eight() -> Target {
        Target::new(HostName::parse("8.8.8.8"), 53)
    }

    /// Computed with Python's `hashlib` / `hmac` and `cryptography`'s
    /// AES-GCM and ChaCha20-Poly1305: password "password", salt `00 01 …`
    /// out and `40 41 …` back, the answer from 8.8.8.8:53.
    #[test]
    fn aead_packets_are_the_known_answers() {
        for (kind, request, answer) in [
            (
                AeadKind::Aes128Gcm,
                "000102030405060708090a0b0c0d0e0f5d51eb7401796b37cf59a6924a104e437bf11e7f91878ac86cd6979b",
                "404142434445464748494a4b4c4d4e4fd3aff9990f2d96949689928d39629a307b8f95ba59bcb26f84533ef88f",
            ),
            (
                AeadKind::Aes256Gcm,
                "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f7fda93a3e4da07282516f07cd043bdbe38981f72ebd4fa923ff5e529",
                "404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5fa1934dbc2a99e8a6b43db9be547e396ba796e9e9fd20ad591c2a321682",
            ),
            (
                AeadKind::ChaCha20Poly1305,
                "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1fac378ef1ffba4729e383117a3abb36719c11848971cb61c67ac52c96",
                "404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f560961fc5e043ae2f29829b22dbac6afccd63ae2aa2efeb6e1b2534f56",
            ),
        ] {
            let key = MasterKey::from_password(kind, "password");
            let salt: Vec<u8> = (0..kind.key_len() as u8).collect();
            assert_eq!(
                seal_aead(&key, &salt, &QUERY_TO, b"query"),
                hex(request),
                "{kind:?}"
            );
            let mut packet = hex(answer);
            let (from, payload) = open_aead(&key, &mut packet).unwrap();
            assert_eq!((from, &packet[payload]), (eight_eight(), &b"answer"[..]));
            let mut flipped = hex(answer);
            flipped[40] ^= 1;
            assert_eq!(
                open_aead(&key, &mut flipped).err(),
                Some("does not decrypt")
            );
            assert_eq!(open_aead(&key, &mut [0u8; 20]).err(), Some("too short"));
        }
    }

    /// Keys for the vectors: aes-128 with one identity key (`00 …`) before
    /// the user key (`20 …`), aes-256 with the single key `20 …`.
    fn keys_2022() -> [(AeadKind, Vec<Vec<u8>>); 2] {
        [
            (
                AeadKind::Aes128Gcm,
                vec![(0..16).collect(), (0x20..0x30).collect()],
            ),
            (AeadKind::Aes256Gcm, vec![(0x20..0x40).collect()]),
        ]
    }

    const CLIENT_SESSION: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

    fn session(keys: &Keys2022) -> Session {
        Session {
            id: CLIENT_SESSION,
            cipher: keys.body_cipher(&CLIENT_SESSION),
            next: AtomicU64::new(0),
            servers: Mutex::new(Vec::new()),
        }
    }

    /// Computed with a BLAKE3 written in Python from its specification and
    /// `cryptography`'s AES-ECB / AES-GCM: client session `01 … 08`, packet
    /// 5, no padding; the answer from server session `a1 … a8`, packet 9,
    /// three bytes of padding, from 8.8.8.8:53; time 1 700 000 000.
    #[test]
    fn ss_2022_packets_are_the_known_answers() {
        let vectors = [
            (
                "8e27435dfc522793dbd676d13a03be50130d1ed6480648b9bd16081cec953f96527df4ce550b12f9075b09f9b72cfa5a62cb9b6a916fe1ee1e6f8fb1f8937e9ae9ac17284a7566",
                "badae3d7318fe1f1771801dcec420eeefa394afd953ad174e0d8171afe4dd7cba7ea77116f0e9d0e64052fd7259c52598586497335744f7423071adb0dc72b2a2a63a0",
            ),
            (
                "a05d60bef664d31d3700189cd527895a5027f8a748e831207a32aef447d7782981382f558654abc07b36ecc39f961b53666bc4f6f2d3af",
                "a944fe53b01aac08fe3f5c095862f076bc6c1a168b81bfcc4b1eaeba3ab16bdc89bc12709667e2e81213fcb068e92bcd3d472c2a315bca128b932910bdbd28bc9d83de",
            ),
        ];
        for ((kind, keys), (request, answer)) in keys_2022().into_iter().zip(vectors) {
            let keys = Keys2022::new(kind, &keys);
            let session = session(&keys);
            let sealed = keys.seal(
                &CLIENT_SESSION,
                &session.cipher,
                5,
                NOW,
                0,
                &QUERY_TO,
                b"query",
            );
            assert_eq!(sealed, hex(request), "{kind:?}");
            let mut packet = hex(answer);
            let (from, payload) = session.open(&keys, &mut packet, NOW).unwrap();
            assert_eq!((from, &packet[payload]), (eight_eight(), &b"answer"[..]));
            // the same packet again is a replay
            let mut again = hex(answer);
            assert_eq!(session.open(&keys, &mut again, NOW).err(), Some("a replay"));
        }
    }

    #[test]
    fn a_2022_answer_is_checked_before_it_counts() {
        let (kind, keys) = keys_2022().into_iter().nth(1).unwrap();
        let keys = Keys2022::new(kind, &keys);
        let answer = "a944fe53b01aac08fe3f5c095862f076bc6c1a168b81bfcc4b1eaeba3ab16bdc89bc12709667e2e81213fcb068e92bcd3d472c2a315bca128b932910bdbd28bc9d83de";
        let open = |session: &Session, packet: &mut [u8], now| {
            session.open(&keys, packet, now).map(|_| ()).err()
        };
        let session = session(&keys);
        // off the clock: dropped, and the window has not moved
        assert_eq!(
            open(&session, &mut hex(answer), NOW + 31),
            Some("the server's clock differs from ours by more than 30 seconds")
        );
        assert_eq!(
            open(&session, &mut hex(answer), NOW - 30),
            None,
            "30 s is fine"
        );
        let mut flipped = hex(answer);
        flipped[20] ^= 1;
        assert_eq!(open(&session, &mut flipped, NOW), Some("does not decrypt"));
        assert_eq!(open(&session, &mut [0u8; 50], NOW), Some("too short"));
        // an answer for someone else's session
        let other = Session {
            id: [9; 8],
            ..self::session(&keys)
        };
        assert_eq!(
            open(&other, &mut hex(answer), NOW),
            Some("for another client session")
        );
        // our own request played back is no answer (a single key: its
        // separate header is under the key the answers use)
        let mut request = keys.seal(
            &[9; 8],
            &keys.body_cipher(&[9; 8]),
            1,
            NOW,
            0,
            &QUERY_TO,
            b"q",
        );
        assert_eq!(
            open(&other, &mut request, NOW),
            Some("not a server's packet")
        );
    }

    #[test]
    fn padding_and_identity_headers_take_their_places() {
        let (kind, keys) = keys_2022().into_iter().next().unwrap();
        let keys = Keys2022::new(kind, &keys);
        let session = session(&keys);
        let sealed = keys.seal(&CLIENT_SESSION, &session.cipher, 7, NOW, 3, &QUERY_TO, b"");
        // separate header, one identity header, body (11 + 3 + 7), tag
        assert_eq!(sealed.len(), 16 + 16 + 21 + TAG);
        let mut separate: [u8; 16] = sealed[..16].try_into().unwrap();
        aes_decrypt_block(&keys.first, &mut separate);
        assert_eq!(separate[..8], CLIENT_SESSION);
        assert_eq!(separate[8..], 7u64.to_be_bytes());
        let mut identity: [u8; 16] = sealed[16..32].try_into().unwrap();
        aes_decrypt_block(&keys.first, &mut identity);
        for (byte, mask) in identity.iter_mut().zip(separate) {
            *byte ^= mask;
        }
        assert_eq!(identity, kdf::identity_hash(&keys.user));
        let mut body = sealed[32..].to_vec();
        let (plain, tag) = body.split_at_mut(21);
        assert!(
            session
                .cipher
                .open_in_place(&separate[4..], plain, (&*tag).try_into().unwrap())
        );
        assert_eq!(plain[..1], [CLIENT_PACKET]);
        assert_eq!(plain[1..9], NOW.to_be_bytes());
        assert_eq!(plain[9..14], [0, 3, 0, 0, 0]);
        assert_eq!(plain[14..], QUERY_TO);
    }

    #[test]
    fn a_plain_packet_is_the_address_and_the_payload() {
        let mut packet = [&[1, 8, 8, 8, 8, 0, 53][..], b"answer"].concat();
        let (from, payload) = source(&packet, 0).unwrap();
        assert_eq!((from, &packet[payload]), (eight_eight(), &b"answer"[..]));
        packet[0] = 9;
        assert_eq!(source(&packet, 0).err(), Some("no address"));
    }

    #[test]
    fn the_window_takes_each_id_once_in_any_order() {
        let mut window = Window::new();
        assert!(window.accept(0), "the first id");
        assert!(!window.accept(0), "a duplicate");
        for id in 1..100 {
            assert!(window.accept(id));
        }
        // out of order, within the window
        assert!(window.accept(150));
        assert!(window.accept(120));
        assert!(!window.accept(120));
        assert!(window.accept(149));
        assert!(!window.accept(99));
    }

    #[test]
    fn the_window_forgets_what_falls_behind() {
        let mut window = Window::new();
        assert!(window.accept(10_000));
        assert!(window.accept(10_000 - WINDOW), "at the window's edge");
        assert!(!window.accept(10_000 - WINDOW - 1), "too old");
        assert!(!window.accept(0), "too old");
        // a jump far ahead clears the ring: ids that shared a block with
        // old ones are new
        assert!(window.accept(10_000 + 64 * 1000));
        assert!(window.accept(10_000 + 64 * 1000 - 64 * 3));
        assert!(!window.accept(10_000 + 64 * 1000 - 64 * 3));
        assert!(!window.accept(10_000), "far behind now");
    }

    #[test]
    fn a_window_jump_of_every_size_keeps_the_newest() {
        for jump in [1, 63, 64, 65, 127 * 64, 128 * 64, 129 * 64, u64::MAX / 2] {
            let mut window = Window::new();
            for id in 0..200 {
                assert!(window.accept(id));
            }
            let top = 199 + jump;
            assert!(window.accept(top), "{jump}");
            assert!(!window.accept(top), "{jump}");
            if jump <= WINDOW {
                assert!(!window.accept(199), "{jump}: still seen");
            }
            if jump > 1 {
                assert!(window.accept(top - 1), "{jump}");
            }
        }
    }
}
```

`crates/rurge-proto/src/shadowsocks/cipher.rs`——把

```rust
/// The inverse of `aes_encrypt_block` (the fake server's side).
#[cfg(any(test, feature = "testing"))]
```

换成

```rust
/// The inverse of `aes_encrypt_block`: the separate headers of the
/// server's UDP packets (and the fake server's side).
```

`crates/rurge-proto/src/shadowsocks/s2022.rs`——把

```rust
const TIME_WINDOW: u64 = 30;
```

换成

```rust
pub(crate) const TIME_WINDOW: u64 = 30;
```

`crates/rurge-proto/src/shadowsocks/s2022.rs`——把

```rust
            aes_encrypt_block(&kdf::identity_subkey(key, salt), &mut block);
```

换成

```rust
            aes_encrypt_block(&kdf::identity_subkey(key, salt), &mut block);
            out.extend_from_slice(&block);
        }
        out
    }

    /// The identity headers of a UDP packet whose separate header is
    /// `separate` (in the clear): the next key's hash, masked with the
    /// separate header so that it differs per packet, under one AES block
    /// keyed with this layer's key itself.
    pub(crate) fn packet_headers(&self, separate: &[u8; 16]) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 * self.layers.len());
        for (key, next) in &self.layers {
            let mut block = *next;
            for (byte, mask) in block.iter_mut().zip(separate) {
                *byte ^= mask;
            }
            aes_encrypt_block(key, &mut block);
```

`crates/rurge-proto/src/socks5.rs`——把

```rust
fn from_relay(relay: Option<IpAddr>, sender: &Target) -> bool {
```

换成

```rust
pub(crate) fn from_relay(relay: Option<IpAddr>, sender: &Target) -> bool {
```

出站：

`crates/rurge-proto/src/shadowsocks/mod.rs`——把

```rust
//! belongs to and the server's time, which the stream checks.
```

换成

```rust
//! belongs to and the server's time, which the stream checks.
//!
//! With `udp-relay=true` datagrams go to the server by themselves (`udp`),
//! past Shadow TLS and obfs, which are TCP's.
```

`crates/rurge-proto/src/shadowsocks/mod.rs`——把

```rust
pub(crate) mod s2022;
```

换成

```rust
pub(crate) mod s2022;
pub(crate) mod udp;
```

`crates/rurge-proto/src/shadowsocks/mod.rs`——把

```rust
use crate::{BuildError, Outbound, OutboundError};
```

换成

```rust
use crate::{BuildError, Outbound, OutboundError, UdpSupport};
```

`crates/rurge-proto/src/shadowsocks/mod.rs`——把

```rust
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
```

换成

```rust
use rurge_net::connector::{BoxedPacketSocket, BoxedStream, ConnectOpts, Connector, Target};
```

`crates/rurge-proto/src/shadowsocks/mod.rs`——把

```rust
use std::sync::Arc;
```

换成

```rust
use std::sync::Arc;
use udp::{Keys2022, Packets, SsUdp};
```

`crates/rurge-proto/src/shadowsocks/mod.rs`——把

```rust
    now: fn() -> u64,
```

换成

```rust
    now: fn() -> u64,
    /// `udp-relay=true`: where the datagrams go and how they are sealed.
    udp: Option<UdpRelay>,
    /// Where the datagrams leave from: the way the TCP connections go
    /// (DIRECT, or `underlying-proxy`).
    connector: Arc<dyn Connector>,
}

struct UdpRelay {
    /// `udp-port`, else the server's port.
    server: Target,
    packets: Arc<Packets>,
```

`crates/rurge-proto/src/shadowsocks/mod.rs`——把

```rust
        let mut stack = Stack::new(connector, server, shadow_tls, None, None);
```

换成

```rust
        let udp = spec.udp_relay.then(|| UdpRelay {
            server: Target::new(server.host.clone(), spec.udp_port.unwrap_or(server.port)),
            packets: Arc::new(match (&key, AeadKind::of(spec.method)) {
                (Some(_), Some(kind)) if spec.method.is_2022() => {
                    Packets::S2022(Keys2022::new(kind, spec.keys.expose()))
                }
                (Some(key), _) => Packets::Aead(key.clone()),
                (None, _) => Packets::Plain,
            }),
        });
        let mut stack = Stack::new(connector.clone(), server, shadow_tls, None, None);
```

`crates/rurge-proto/src/shadowsocks/mod.rs`——把

```rust
            now: s2022::unix_now,
        })
```

换成

```rust
            now: s2022::unix_now,
            udp,
            connector,
        })
```

`crates/rurge-proto/src/shadowsocks/mod.rs`——把

```rust
        })
    }
}
```

换成

```rust
        })
    }

    fn udp(&self) -> UdpSupport {
        if self.udp.is_some() {
            UdpSupport::Native
        } else {
            UdpSupport::Unsupported
        }
    }

    fn open_udp<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedPacketSocket, OutboundError>> {
        Box::pin(async move {
            let Some(relay) = &self.udp else {
                return Err(OutboundError::Unsupported(
                    "UDP without `udp-relay=true`".to_string(),
                ));
            };
            let open = async {
                let socket = self.connector.open_udp(opts).await?;
                let carrier = SsUdp::open(socket, &relay.server, relay.packets.clone(), self.now);
                Ok(Box::new(carrier.await?) as BoxedPacketSocket)
            };
            match tokio::time::timeout(opts.timeout, open).await {
                Ok(result) => result,
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }
}
```

要点：
- 丢包的原因（解不开、太短、类型不对、时钟不对、别的会话、地址坏、重放）只记 `debug!("ss: a UDP packet from the server was dropped: {why}")`，原因是固定文字，不带载荷与密钥。
- 客户端的填充：有负载时为 0，空数据报时 1..=900 个零字节（同 TCP 的 `padding_len`）。
- 服务器地址在打开载体时解析一次：`ss: cannot look up the server for UDP: …`；目标名发不出去时的文字同 TCP。
- 链上的载体保留名字时不按来源过滤（同 socks5 的中继）。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-proto --lib shadowsocks` → 通过（`udp` 8 条：AEAD 与 2022 包的已知答案、2022 回包先校验再计数、填充与身份头的位置、`none` 的包、窗口的三种情形；`shadowsocks::tests` 8 条：每种方法的往返、`udp-port`、全锥、名字、坏包与重放后下一个照常到达、时钟不对或别的会话的回包、服务端丢掉错的密钥或差一小时的时钟、没有 `udp-relay`）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-proto
git commit -m "feat(proto): Shadowsocks 的 UDP——AEAD、none 与 SS 2022（分离头、防重放窗口），全锥，udp-port"
```

### Task 6: 引擎装配与能力表

去掉 Task 1 的门（P3），引擎工厂装上 `ShadowsocksOutbound`（经直连连接器或 `underlying-proxy` 的链，同其它协议），能力表加入 `ss`。以 `ss` 为"未实现协议"例子的用例改用 `hysteria2`（M7 之前都不会实现，M6b / M6c 不必再改）。重载按指纹复用不需要改代码（P14），由用例覆盖。

**Files:**
- Create: `crates/rurge-engine/tests/outbounds_shadowsocks.rs`
- Modify: `crates/rurge-config/src/spec/mod.rs`（与用例）、`crates/rurge-engine/src/outbounds.rs`（与用例）、`crates/rurge/src/capabilities.rs`；用例：`crates/rurge-engine/tests/common/mod.rs`、`tests/pipeline.rs`、`src/observe.rs`、`crates/rurge-api/tests/api.rs`、`crates/rurge-config/tests/policy_spec.rs`、`crates/rurge-policy/src/assemble.rs`、`src/registry.rs`、`crates/rurge/tests/cli.rs`

**Interfaces:**
- Consumes: Task 1 ～ 5 的全部；既有的 `EngineFactory::build`、`server_of`、测试夹具 `harness` / `Profile` / `udp_associate` / `FakeSocks5`。
- Produces: 工厂的 `ProtoSpec::Ss` 分支；`PolicyKind::Shadowsocks` 进能力表；`tests/common` 再导出 `FakeShadowsocks` / `ShadowsocksScript`。

- [ ] **Step 1: 先写用例**

经引擎的端到端用例：

新建 `crates/rurge-engine/tests/outbounds_shadowsocks.rs`：

```rust
//! Sessions that leave through `ss` (phase 2 M6 design 3.3, 3.4): TCP and
//! UDP through the engine to the loopback `FakeShadowsocks` — the AEAD
//! methods, `none` and SS 2022 with identity headers, both obfs modes,
//! `udp-port`, `underlying-proxy`, reloads, and what is not implemented.

mod common;
use common::*;
use rurge_config::HostName;
use rurge_config::session::Transport;
use rurge_config::spec::{ObfsMode, SsMethod};
use rurge_engine::RequestRecord;
use rurge_net::connector::Target;

/// SS 2022 keys in Base64: 16 bytes (the server's identity key, two
/// users') and 32 bytes.
const SERVER_16: &str = "MDEyMzQ1Njc4OWFiY2RlZg==";
const USER_16: &str = "ZmVkY2JhOTg3NjU0MzIxMA==";
const OTHER_16: &str = "dGhlIG90aGVyIHVzZXIhIQ==";
const KEY_32: &str = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=";

fn udp_records(h: &Harness) -> Vec<RequestRecord> {
    h.engine
        .request_log()
        .recent(4096)
        .into_iter()
        .filter(|r| r.transport == Transport::Udp)
        .collect()
}

/// `S = ss, …` to `fake`, with `params` after the port.
fn ss_line(fake: &FakeShadowsocks, params: &str) -> String {
    format!("S = ss, 127.0.0.1, {}, {params}", fake.addr().port())
}

async fn through(proxies: &str) -> Harness {
    harness(Profile {
        proxies,
        rules: "DOMAIN,target.test,S\nIP-CIDR,127.0.0.1/32,S,no-resolve",
        ..Profile::default()
    })
    .await
}

/// The only session's record, once it has finished.
async fn the_record(h: &Harness) -> RequestRecord {
    let log = h.engine.request_log();
    wait_until("the session to finish", || !log.recent(10).is_empty()).await;
    log.recent(10)[0].clone()
}

/// An AEAD method, `none`, and SS 2022 as the second of two users: each
/// carries a tunnel to the echo, and the server is asked for the name.
#[tokio::test]
async fn a_connect_leaves_through_ss_with_every_kind_of_method() {
    let echo = rurge_proto::testing::echo_server().await;
    for (script, params, user) in [
        (
            ShadowsocksScript::new(SsMethod::Aes256Gcm, "s3same"),
            "encrypt-method=aes-256-gcm, password=s3same".to_string(),
            None,
        ),
        (
            ShadowsocksScript::new(SsMethod::None, ""),
            "encrypt-method=none".to_string(),
            None,
        ),
        (
            ShadowsocksScript {
                users: vec![OTHER_16.to_string(), USER_16.to_string()],
                ..ShadowsocksScript::new(SsMethod::Blake3Aes128Gcm, SERVER_16)
            },
            format!("encrypt-method=2022-blake3-aes-128-gcm, password={SERVER_16}:{USER_16}"),
            Some(1),
        ),
    ] {
        let fake = FakeShadowsocks::spawn(ShadowsocksScript {
            connect_to: Some(echo),
            ..script
        })
        .await;
        let h = through(&ss_line(&fake, &params)).await;
        let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
        echo_through(&mut tunnel, b"through ss").await;
        echo_through(&mut tunnel, &vec![0x5a; 100_000]).await;
        let seen = fake.requests();
        let first = seen.first().expect("the server never saw a request");
        assert_eq!(
            (first.atyp, first.host.as_str(), first.port, first.user),
            (3, "target.test", 7, user),
            "{params}: the server resolves the name"
        );
        assert!(h.dns.queries().is_empty(), "rurge never looked the name up");
        drop(tunnel);
        let record = the_record(&h).await;
        assert_eq!(record.policy, ["S"], "{params}");
        assert!(record.error.is_none(), "{params}: {:?}", record.error);
    }
}

#[tokio::test]
async fn both_obfs_modes_carry_the_session() {
    let echo = rurge_proto::testing::echo_server().await;
    for (mode, extra) in [
        (ObfsMode::Http, "obfs=http, obfs-host=cdn.test, obfs-uri=/a"),
        (ObfsMode::Tls, "obfs=tls, obfs-host=cdn.test"),
    ] {
        let fake = FakeShadowsocks::spawn(ShadowsocksScript {
            obfs: Some(mode),
            connect_to: Some(echo),
            ..ShadowsocksScript::new(SsMethod::ChaCha20IetfPoly1305, "s3same")
        })
        .await;
        let h = through(&ss_line(
            &fake,
            &format!("encrypt-method=chacha20-ietf-poly1305, password=s3same, {extra}"),
        ))
        .await;
        let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
        echo_through(&mut tunnel, b"behind the camouflage").await;
        let hello = &fake.obfs_seen()[0];
        match mode {
            ObfsMode::Http => {
                assert_eq!(hello.host, format!("cdn.test:{}", fake.addr().port()));
                assert_eq!(hello.uri.as_deref(), Some("/a"));
            }
            ObfsMode::Tls => assert_eq!(hello.host, "cdn.test"),
        }
    }
}

/// An SS 2022 answer whose clock is an hour off fails the session, and
/// the record says why (design 3.3).
#[tokio::test]
async fn a_server_clock_an_hour_off_fails_the_session_with_the_reason() {
    let echo = rurge_proto::testing::echo_server().await;
    let fake = FakeShadowsocks::spawn(ShadowsocksScript {
        connect_to: Some(echo),
        answer_skew: 3600,
        ..ShadowsocksScript::new(SsMethod::Blake3Aes256Gcm, KEY_32)
    })
    .await;
    let h = through(&ss_line(
        &fake,
        &format!("encrypt-method=2022-blake3-aes-256-gcm, password={KEY_32}"),
    ))
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    tunnel.write_all(b"what time is it").await.unwrap();
    let mut rest = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), tunnel.read_to_end(&mut rest))
        .await
        .expect("the tunnel closes within the bound");
    assert!(rest.is_empty(), "nothing of the answer is passed on");
    let record = the_record(&h).await;
    assert_eq!(record.status, RecordStatus::Failed, "{record:?}");
    let error = record.error.unwrap_or_default();
    assert!(
        error.starts_with("ss: the server's clock differs from ours by ")
            && error.ends_with(" seconds (at most 30 are allowed)"),
        "{error}"
    );
}

/// A stream cipher is parsed and rejects at run time, and the record names
/// the cipher (design 3.1).
#[tokio::test]
async fn a_stream_cipher_rejects_and_says_which() {
    let h = harness(Profile {
        proxies: "Rc = ss, 127.0.0.1, 9, encrypt-method=rc4-md5, password=s3same",
        rules: "DOMAIN,target.test,Rc",
        ..Profile::default()
    })
    .await;
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    s.write_all(b"CONNECT target.test:7 HTTP/1.1\r\nHost: target.test:7\r\n\r\n")
        .await
        .unwrap();
    let mut answer = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut answer)).await;
    assert!(
        !answer.starts_with(b"HTTP/1.1 200"),
        "{}",
        String::from_utf8_lossy(&answer)
    );
    let record = the_record(&h).await;
    assert_eq!(record.status, RecordStatus::Rejected("REJECT".into()));
    assert_eq!(
        record.error.as_deref(),
        Some("policy protocol not implemented: ss (rc4-md5)")
    );
}

/// `ss` over a SOCKS5 `underlying-proxy`: the entry is asked for the `ss`
/// server, TCP through its tunnel, UDP through its association.
#[tokio::test]
async fn ss_goes_through_an_underlying_socks5_proxy_for_tcp_and_udp() {
    let echo = rurge_proto::testing::echo_server().await;
    let fake = FakeShadowsocks::spawn(ShadowsocksScript {
        connect_to: Some(echo),
        ..ShadowsocksScript::new(SsMethod::Aes128Gcm, "s3same")
    })
    .await;
    let entry = FakeSocks5::spawn(Socks5Script::default()).await;
    let h = through(&format!(
        "Entry = socks5, 127.0.0.1, {}, udp-relay=true\n{}",
        entry.addr().port(),
        ss_line(
            &fake,
            "encrypt-method=aes-128-gcm, password=s3same, udp-relay=true, underlying-proxy=Entry"
        )
    ))
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, b"socks5 then ss").await;
    let first = entry.requests()[0].clone();
    assert_eq!(
        (first.command, first.host.as_str(), first.port),
        (1, "127.0.0.1", fake.addr().port())
    );
    assert_eq!(fake.requests()[0].host, "target.test");

    let (udp_echo_addr, _) = udp_echo().await;
    let association = udp_associate(h.socks()).await;
    association
        .send("127.0.0.1", udp_echo_addr.port(), b"socks5 then ss, by UDP")
        .await;
    assert_eq!(
        association.recv().await,
        (udp_echo_addr, b"socks5 then ss, by UDP".to_vec())
    );
    let commands: Vec<u8> = entry.requests().iter().map(|r| r.command).collect();
    assert_eq!(commands, [1, 3], "the tunnel, then the association");
    assert_eq!(
        entry.datagrams(),
        [Target::new(
            HostName::Ip(fake.udp_addr().ip()),
            fake.udp_addr().port()
        )],
        "the ss datagram went to the server through the entry"
    );
    assert_eq!(fake.datagrams()[0].payload, b"socks5 then ss, by UDP");
}

/// Three datagrams to two echoes through `S` over one association: every flow leaves
/// through `S` and ends with the association.
async fn udp_through(fake: &FakeShadowsocks, params: &str) {
    let h = through(&ss_line(fake, params)).await;
    let ((one, _), (two, _)) = (udp_echo().await, udp_echo().await);
    let association = udp_associate(h.socks()).await;
    for (echo, payload) in [(one, &b"one"[..]), (two, b"two"), (one, b"again")] {
        association.send("127.0.0.1", echo.port(), payload).await;
        assert_eq!(association.recv().await, (echo, payload.to_vec()));
    }
    drop(association);
    wait_until("both flows to finish", || udp_records(&h).len() == 2).await;
    for r in udp_records(&h) {
        assert_eq!(r.policy, ["S"], "{r:?}");
        assert_eq!(r.status, RecordStatus::Completed, "{r:?}");
    }
    assert_eq!(fake.datagrams().len(), 3, "{params}");
    assert_eq!(fake.connections(), 0, "{params}: no TCP");
}

#[tokio::test]
async fn udp_goes_through_ss_aead_and_2022() {
    let fake = FakeShadowsocks::spawn(ShadowsocksScript::new(SsMethod::Aes256Gcm, "s3same")).await;
    udp_through(
        &fake,
        "encrypt-method=aes-256-gcm, password=s3same, udp-relay=true",
    )
    .await;

    let fake = FakeShadowsocks::spawn(ShadowsocksScript {
        users: vec![OTHER_16.to_string(), USER_16.to_string()],
        ..ShadowsocksScript::new(SsMethod::Blake3Aes128Gcm, SERVER_16)
    })
    .await;
    udp_through(
        &fake,
        &format!(
            "encrypt-method=2022-blake3-aes-128-gcm, password={SERVER_16}:{USER_16}, udp-relay=true"
        ),
    )
    .await;
    let seen = fake.datagrams();
    assert!(seen.iter().all(|d| d.user == Some(1)), "{seen:?}");
    let session = seen[0].session.expect("an SS 2022 session").0;
    assert!(
        seen.iter().all(|d| d.session.map(|s| s.0) == Some(session)),
        "one carrier, one session: {seen:?}"
    );
}

/// `udp-port`: datagrams go to that port, not to the policy's.
#[tokio::test]
async fn udp_goes_to_udp_port() {
    let fake = FakeShadowsocks::spawn(ShadowsocksScript {
        udp_apart: true,
        ..ShadowsocksScript::new(SsMethod::Blake3Aes256Gcm, KEY_32)
    })
    .await;
    assert_ne!(fake.udp_addr().port(), fake.addr().port());
    udp_through(
        &fake,
        &format!(
            "encrypt-method=2022-blake3-aes-256-gcm, password={KEY_32}, udp-relay=true, udp-port={}",
            fake.udp_addr().port()
        ),
    )
    .await;
}

/// Full cone: whoever reaches the server's socket for this client reaches
/// the client, under its own address.
#[tokio::test]
async fn anyone_may_answer_through_ss() {
    let fake = FakeShadowsocks::spawn(ShadowsocksScript::new(
        SsMethod::ChaCha20IetfPoly1305,
        "s3same",
    ))
    .await;
    let h = through(&ss_line(
        &fake,
        "encrypt-method=chacha20-ietf-poly1305, password=s3same, udp-relay=true",
    ))
    .await;
    let (echo, _) = udp_echo().await;
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", echo.port(), b"hello").await;
    assert_eq!(association.recv().await, (echo, b"hello".to_vec()));
    let stranger = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    stranger
        .send_to(b"unasked", fake.udp_outside()[0])
        .await
        .unwrap();
    assert_eq!(
        association.recv().await,
        (stranger.local_addr().unwrap(), b"unasked".to_vec())
    );
}

/// Without `udp-relay=true` the policy carries no UDP:
/// `udp-policy-not-supported-behaviour` (REJECT by default) decides.
#[tokio::test]
async fn without_udp_relay_a_flow_is_rejected() {
    let fake = FakeShadowsocks::spawn(ShadowsocksScript::new(SsMethod::Aes128Gcm, "s3same")).await;
    let h = through(&ss_line(
        &fake,
        "encrypt-method=aes-128-gcm, password=s3same",
    ))
    .await;
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", 53, b"q").await;
    assert!(association.quiet_for(Duration::from_millis(300)).await);
    wait_until("the flow to finish", || !udp_records(&h).is_empty()).await;
    let records = udp_records(&h);
    assert_eq!(records[0].status, RecordStatus::Rejected("REJECT".into()));
    assert_eq!(
        records[0].error.as_deref(),
        Some("policy does not support UDP")
    );
    assert!(fake.datagrams().is_empty() && fake.connections() == 0);
}

/// The outbound is kept across a reload that leaves the line alone, and
/// rebuilt when its method, password, obfs or `udp-port` changes (design 6).
#[tokio::test]
async fn a_reload_keeps_an_unchanged_ss_policy_and_rebuilds_a_changed_one() {
    let echo = rurge_proto::testing::echo_server().await;
    let fake = FakeShadowsocks::spawn(ShadowsocksScript {
        connect_to: Some(echo),
        ..ShadowsocksScript::new(SsMethod::Aes128Gcm, "s3same")
    })
    .await;
    let base = "encrypt-method=aes-128-gcm, password=s3same, udp-relay=true";
    let proxies = |params: &str, extra: &str| format!("{}\n{extra}", ss_line(&fake, params));
    let h = through(&proxies(base, "")).await;
    let reload = |params: &str, extra: &str| {
        let next = Profile {
            proxies: &proxies(params, extra),
            rules: "DOMAIN,target.test,S",
            ..Profile::default()
        }
        .text(h.dns.addr());
        let dir = h.dir.path().to_path_buf();
        let shared = h.engine.shared();
        async move { runtime(&dir, &next, shared).await }
    };
    let before = outbound_now(&h, "S");
    h.engine
        .swap_runtime(reload(base, "Other = http, other.example, 8080").await);
    assert!(
        Arc::ptr_eq(&before, &outbound_now(&h, "S")),
        "S was rebuilt"
    );
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, b"after the reload").await;

    let mut previous = before;
    for changed in [
        "encrypt-method=aes-256-gcm, password=s3same, udp-relay=true",
        "encrypt-method=aes-256-gcm, password=0ther, udp-relay=true",
        "encrypt-method=aes-256-gcm, password=0ther, udp-relay=true, obfs=http",
        "encrypt-method=aes-256-gcm, password=0ther, udp-relay=true, obfs=http, udp-port=9999",
    ] {
        h.engine.swap_runtime(reload(changed, "").await);
        let now = outbound_now(&h, "S");
        assert!(!Arc::ptr_eq(&previous, &now), "kept after: {changed}");
        previous = now;
    }
}
```

`crates/rurge-engine/tests/common/mod.rs`——把

```rust
    AnyTlsScript, FakeAnyTls, FakeHttpProxy, FakeSocks5, FakeTrojan, FakeVmess, HttpProxyScript,
    Socks5Script, TlsFixture, TrojanScript, VmessScript,
```

换成

```rust
    AnyTlsScript, FakeAnyTls, FakeHttpProxy, FakeShadowsocks, FakeSocks5, FakeTrojan, FakeVmess,
    HttpProxyScript, ShadowsocksScript, Socks5Script, TlsFixture, TrojanScript, VmessScript,
```

工厂与干构建：

`crates/rurge-engine/src/outbounds.rs`——把

```rust
    use rurge_config::diagnostic::codes;
```

换成

```rust
    use rurge_config::diagnostic::{Severity, codes};
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
SSH = ssh, proxy.test, 22, username=u, password=pw\n\
```

换成

```rust
SSH = ssh, proxy.test, 22, username=u, password=pw\n\
SS = ss, proxy.test, 8388, encrypt-method=aes-128-gcm, password=pw, obfs=http, udp-relay=true\n\
SK = ss, proxy.test, 8388, encrypt-method=2022-blake3-aes-128-gcm, password=MDEyMzQ1Njc4OWFiY2RlZg==:MDEyMzQ1Njc4OWFiY2RlZg==, shadow-tls-password=st\n\
SN = ss, proxy.test, 8388, encrypt-method=none\n\
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
            ("SSH", "SSH"),
```

换成

```rust
            ("SSH", "SSH"),
            ("SS", "SS"),
            ("SK", "SK"),
            ("SN", "SN"),
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
                "policy `S` cannot be built: keystore item `key1` is protected by a passphrase, which rurge cannot use; remove the passphrase"
            ]
        );
    }

    #[test]
```

换成

```rust
                "policy `S` cannot be built: keystore item `key1` is protected by a passphrase, which rurge cannot use; remove the passphrase"
            ]
        );
    }

    /// A sound `ss` line passes the dry build; an SS 2022 key of the wrong
    /// length is already a load error at its line, never quoted (phase 2 M6
    /// design 3.1).
    #[test]
    fn an_ss_policy_passes_the_dry_build_and_a_bad_2022_key_fails_the_load() {
        let cfg = config(
            "[Proxy]\nS = ss, proxy.test, 8388, encrypt-method=2022-blake3-aes-256-gcm, \
password=MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=, obfs=tls, udp-relay=true, udp-port=8389\n\
[Rule]\nFINAL,DIRECT\n",
        );
        assert!(dry_build(&cfg).is_empty());
        let loaded = from_text(
            "[Proxy]\nS = ss, proxy.test, 8388, encrypt-method=2022-blake3-aes-256-gcm, \
password=MDEyMzQ1Njc4OWFiY2RlZg==\n[Rule]\nFINAL,DIRECT\n",
            Path::new("t.conf"),
            &LoadOptions::for_tests(),
        );
        let errors: Vec<(&str, u32, &str)> = loaded
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| {
                let line = d.span.as_ref().map(|s| s.line).unwrap_or(0);
                (d.code, line, d.message.as_str())
            })
            .collect();
        assert_eq!(
            errors,
            [(
                codes::E_INVALID_POLICY_PARAM,
                2,
                "policy `S`: key #1 of `password` is not a Base64 key of 32 bytes, as `2022-blake3-aes-256-gcm` requires"
            )]
        );
        assert!(loaded.config.spec("S").is_none());
    }

    #[test]
```

配置层（`ss` 行有了 spec；`ss` 进"每种 TCP 协议都能叠 Shadow TLS"的名单）：

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            "anytls, h.test, 443, password=p",
```

换成

```rust
            "anytls, h.test, 443, password=p",
            "ss, h.test, 8388, encrypt-method=aes-128-gcm, password=p",
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    /// An `ss` line is read and checked in full; it has no spec until the
    /// engine builds `ss` (M6a task 6), and a stream cipher says why.
```

换成

```rust
    /// An `ss` line is read and checked in full; a stream cipher has no
    /// spec and says why.
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
        assert_eq!(o.not_implemented, None);
        assert!(o.spec.is_none());
```

换成

```rust
        assert_eq!(o.not_implemented, None);
        let spec = o.spec.expect("an AEAD line has a spec");
        let ProtoSpec::Ss(ss) = &spec.proto else {
            panic!("{:?}", spec.proto)
        };
        assert_eq!(
            (ss.method, ss.udp_relay, ss.udp_port),
            (SsMethod::Aes128Gcm, true, Some(8389))
        );
        assert_eq!(ss.obfs.as_ref().map(|o| o.mode), Some(ObfsMode::Tls));
        assert!(spec.shadow_tls.is_some());
```

`crates/rurge-config/tests/policy_spec.rs`——把

```rust
        "A = http, a.example, 80, ip-version=v6-only\nSS = ss, s.example, 8388, encrypt-method=aes-128-gcm, password=x\nB = socks5, b.example, 1080\nC = direct, interface=eth0",
```

换成

```rust
        "A = http, a.example, 80, ip-version=v6-only\nHy = hysteria2, h.example, 443, password=x\nB = socks5, b.example, 1080\nC = direct, interface=eth0",
```

`crates/rurge-config/tests/policy_spec.rs`——把

```rust
    assert_eq!(names, ["A", "B", "C"], "ss has no spec yet");
```

换成

```rust
    assert_eq!(names, ["A", "B", "C"], "hysteria2 has no spec yet");
```

`crates/rurge-config/tests/policy_spec.rs`——把

```rust
    assert!(loaded.config.spec("SS").is_none() && loaded.config.spec("nope").is_none());
```

换成

```rust
    assert!(loaded.config.spec("Hy").is_none() && loaded.config.spec("nope").is_none());
```

以 `ss` 为"未实现协议"例子的用例改用 `hysteria2`（api / pipeline 的夹具改为从测试能力表里去掉 `Hysteria2`，好让它们仍然得到加载时的 `W0007`）：

`crates/rurge-api/tests/api.rs`——把

```rust
/// The binary does not declare Shadowsocks (yet), so dropping it from the
```

换成

```rust
/// The binary does not declare Hysteria 2 (yet), so dropping it from the
```

`crates/rurge-api/tests/api.rs`——把

```rust
        .remove(&rurge_config::PolicyKind::Shadowsocks);
```

换成

```rust
        .remove(&rurge_config::PolicyKind::Hysteria2);
```

`crates/rurge-api/tests/api.rs`——把

```rust
[Proxy]\nHK = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\nBlock = reject-tinygif\n\
```

换成

```rust
[Proxy]\nHK = hysteria2, 1.2.3.4, 443, password=x\nBlock = reject-tinygif\n\
```

`crates/rurge-api/tests/api.rs`——把

```rust
        "W0007 for the unsupported ss policy: {body}"
```

换成

```rust
        "W0007 for the unsupported hysteria2 policy: {body}"
```

`crates/rurge-api/tests/api.rs`——把

```rust
    assert_eq!(members[0]["typeDescription"], "ss");
```

换成

```rust
    assert_eq!(members[0]["typeDescription"], "hysteria2");
```

`crates/rurge-api/tests/api.rs`——把

```rust
    assert!(detail.starts_with("ss, 1.2.3.4, 8388"), "{detail}");
```

换成

```rust
    assert!(detail.starts_with("hysteria2, 1.2.3.4, 443"), "{detail}");
```

`crates/rurge-engine/tests/pipeline.rs`——把

```rust
/// does not declare Shadowsocks (`crates/rurge/src/capabilities.rs`), so
```

换成

```rust
/// does not declare Hysteria 2 (`crates/rurge/src/capabilities.rs`), so
```

`crates/rurge-engine/tests/pipeline.rs`——把

```rust
        .remove(&rurge_config::PolicyKind::Shadowsocks);
```

换成

```rust
        .remove(&rurge_config::PolicyKind::Hysteria2);
```

`crates/rurge-engine/tests/pipeline.rs`——把

```rust
[Proxy]\nHK = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\nBlock = reject-tinygif\n\
```

换成

```rust
[Proxy]\nHK = hysteria2, 1.2.3.4, 443, password=x\nBlock = reject-tinygif\n\
```

`crates/rurge-engine/tests/pipeline.rs`——把

```rust
        "unsupported ss policy warned at load: {:?}",
```

换成

```rust
        "unsupported hysteria2 policy warned at load: {:?}",
```

`crates/rurge-engine/tests/pipeline.rs`——把

```rust
    assert!(String::from_utf8_lossy(&body).contains("!unsupported:ss"));
```

换成

```rust
    assert!(String::from_utf8_lossy(&body).contains("!unsupported:hysteria2"));
```

`crates/rurge-engine/tests/pipeline.rs`——把

```rust
                Some("policy protocol not implemented: ss")
```

换成

```rust
                Some("policy protocol not implemented: hysteria2")
```

`crates/rurge-engine/tests/pipeline.rs`——把

```rust
[Proxy]\nHK = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\n[Proxy Group]\nPick = select, HK, DIRECT\n\
```

换成

```rust
[Proxy]\nHK = hysteria2, 1.2.3.4, 443, password=x\n[Proxy Group]\nPick = select, HK, DIRECT\n\
```

`crates/rurge-engine/src/observe.rs`——把

```rust
            "!unsupported:ss".into(),
```

换成

```rust
            "!unsupported:hysteria2".into(),
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
SS = ss, s.test, 8388, encrypt-method=aes-128-gcm, password=pw\n\
```

换成

```rust
Hy = hysteria2, h.test, 443, password=pw\n\
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
        assert_eq!(members(&a, "G"), ["SS", "Rc", "Old", "Good"]);
```

换成

```rust
        assert_eq!(members(&a, "G"), ["Hy", "Rc", "Old", "Good"]);
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
                    "policy group `G`: imported policies of type `ss` are not implemented in this version; they behave as REJECT".to_string()
```

换成

```rust
                    "policy group `G`: imported policies of type `hysteria2` are not implemented in this version; they behave as REJECT".to_string()
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
Corp = direct, interface=eth9\nBlock = reject\nSS = ss, s.test, 8388, encrypt-method=aes-128-gcm, password=pw",
            "Inner = select, A\n\
G = select, A, Corp, Block, DIRECT, Inner, SS, policy-path=https://sub.test/g, underlying-proxy=Relay",
```

换成

```rust
Corp = direct, interface=eth9\nBlock = reject\nHy = hysteria2, h.test, 443, password=pw",
            "Inner = select, A\n\
G = select, A, Corp, Block, DIRECT, Inner, Hy, policy-path=https://sub.test/g, underlying-proxy=Relay",
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
                "SS",
```

换成

```rust
                "Hy",
```

`crates/rurge-policy/src/registry.rs`——把

```rust
HK = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\n\
```

换成

```rust
HK = hysteria2, 1.2.3.4, 443, password=x\n\
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        assert_eq!(chain(&hk), vec!["HK", "!unsupported:ss", "REJECT"]);
```

换成

```rust
        assert_eq!(chain(&hk), vec!["HK", "!unsupported:hysteria2", "REJECT"]);
```

`crates/rurge-policy/src/registry.rs`——把

```rust
                Some(Note::Unsupported("ss".into()))
```

换成

```rust
                Some(Note::Unsupported("hysteria2".into()))
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            vec!["Pick", "HK", "!unsupported:ss", "REJECT"]
```

换成

```rust
            vec!["Pick", "HK", "!unsupported:hysteria2", "REJECT"]
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            vec!["Auto", "HK", "!unsupported:ss", "REJECT"]
```

换成

```rust
            vec!["Auto", "HK", "!unsupported:hysteria2", "REJECT"]
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        let ss = registry.resolve(&PolicyRef::parse("SS"));
        assert_eq!(ss.note, Some(Note::Unsupported("ss".into())));
```

换成

```rust
        // an AEAD `ss` is a proxy; only a stream cipher is not implemented
        let ss = registry.resolve(&PolicyRef::parse("SS"));
        assert_eq!((ss.terminal, ss.note), (TerminalKind::Proxy, None));
```

`crates/rurge-policy/src/registry.rs`——把

```rust
[Proxy]\nU = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\nA = http, a.example, 80\nB = http, b.example, 80\n\
```

换成

```rust
[Proxy]\nU = hysteria2, 1.2.3.4, 443, password=x\nA = http, a.example, 80\nB = http, b.example, 80\n\
```

`crates/rurge-policy/src/registry.rs`——把

```rust
        assert_eq!(only.chain, ["OnlyU", "U", "!unsupported:ss", "REJECT"]);
```

换成

```rust
        assert_eq!(
            only.chain,
            ["OnlyU", "U", "!unsupported:hysteria2", "REJECT"]
        );
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            profile += &format!(
                "{name} = ss, {name}.example, 8388, encrypt-method=aes-128-gcm, password=x\n"
            );
```

换成

```rust
            profile += &format!("{name} = hysteria2, {name}.example, 443, password=x\n");
```

`crates/rurge/tests/cli.rs`——把

```rust
const PROXIES: &str = "[General]\n[Proxy]\nH = http, proxy.test, 8080\nS = socks5-tls, proxy.test, 443\nOld = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\n[Rule]\nFINAL,DIRECT\n";
```

换成

```rust
const PROXIES: &str = "[General]\n[Proxy]\nH = http, proxy.test, 8080\nS = socks5-tls, proxy.test, 443\nOld = hysteria2, 1.2.3.4, 443, password=x\n[Rule]\nFINAL,DIRECT\n";
```

`crates/rurge/tests/cli.rs`——把

```rust
    assert!(out.contains("`ss`"), "{out}");
```

换成

```rust
    assert!(out.contains("`hysteria2`"), "{out}");
```

`crates/rurge/tests/cli.rs`——把

```rust
const TROJAN: &str = "[General]\n[Proxy]\nT = trojan, proxy.test, 443, password=s3same, ws=true, ws-path=/w\nOld = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\n[Rule]\nFINAL,DIRECT\n";
```

换成

```rust
const TROJAN: &str = "[General]\n[Proxy]\nT = trojan, proxy.test, 443, password=s3same, ws=true, ws-path=/w\nOld = hysteria2, 1.2.3.4, 443, password=x\n[Rule]\nFINAL,DIRECT\n";
```

`crates/rurge/tests/cli.rs`——把

```rust
    // `ss` is still a later milestone; `trojan` is not
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(out.contains("`ss`") && !out.contains("`trojan`"), "{out}");
```

换成

```rust
    // `hysteria2` is still a later milestone; `trojan` is not
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("`hysteria2`") && !out.contains("`trojan`"),
        "{out}"
    );
```

`crates/rurge/tests/cli.rs`——把

```rust
S = ssh, proxy.test, 22, username=u, password=s3same, idle-timeout=60\n\
Old = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\n[Rule]\nFINAL,DIRECT\n";
```

换成

```rust
S = ssh, proxy.test, 22, username=u, password=s3same, idle-timeout=60\n\
Old = hysteria2, 1.2.3.4, 443, password=x\n[Rule]\nFINAL,DIRECT\n";
```

`crates/rurge/tests/cli.rs`——把

```rust
    // `ss` is still a later milestone; `ssh` is not
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(out.contains("`ss`") && !out.contains("`ssh`"), "{out}");
```

换成

```rust
    // `hysteria2` is still a later milestone; `ssh` is not
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("`hysteria2`") && !out.contains("`ssh`"),
        "{out}"
    );
```

`crates/rurge/tests/cli.rs`——把

```rust
Old = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\n[Rule]\nFINAL,DIRECT\n\
```

换成

```rust
Old = hysteria2, 1.2.3.4, 443, password=x\n[Rule]\nFINAL,DIRECT\n\
```

`crates/rurge/tests/cli.rs`——把

```rust
    // `ss` is still a later milestone; `wireguard` is not
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("`ss`") && !out.contains("`wireguard`"),
```

换成

```rust
    // `hysteria2` is still a later milestone; `wireguard` is not
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("`hysteria2`") && !out.contains("`wireguard`"),
```

`crates/rurge/tests/cli.rs`——把

```rust
X = external, exec = \"/usr/bin/sshpass\", args = -p, args = hunter2, args = ssh, local-port = 1080\n\
Old = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\n[Rule]\nFINAL,DIRECT\n";
```

换成

```rust
X = external, exec = \"/usr/bin/sshpass\", args = -p, args = hunter2, args = ssh, local-port = 1080\n\
Old = hysteria2, 1.2.3.4, 443, password=x\n[Rule]\nFINAL,DIRECT\n";
```

`crates/rurge/tests/cli.rs`——把

```rust
    // `ss` is still a later milestone; `external` is not
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(out.contains("`ss`") && !out.contains("`external`"), "{out}");
```

换成

```rust
    // `hysteria2` is still a later milestone; `external` is not
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("`hysteria2`") && !out.contains("`external`"),
        "{out}"
    );
```

`crates/rurge/tests/cli.rs`——把

```rust
        ));
```

换成

```rust
        ));
}

const SS: &str = "[General]\n[Proxy]\n\
A = ss, proxy.test, 8388, encrypt-method=chacha20-ietf-poly1305, password=s3same, obfs=http\n\
K = ss, proxy.test, 8388, encrypt-method=2022-blake3-aes-128-gcm, password=MDEyMzQ1Njc4OWFiY2RlZg==, udp-relay=true\n\
Rc1 = ss, proxy.test, 8388, encrypt-method=rc4-md5, password=s3same\n\
Rc2 = ss, proxy.test, 8388, encrypt-method=rc4-md5, password=s3same\n[Rule]\nFINAL,DIRECT\n";
const SS_BAD_KEY: &str = "[General]\n[Proxy]\n\
K = ss, proxy.test, 8388, encrypt-method=2022-blake3-aes-128-gcm, password=c2VjcmV0IGtleSBtYXRlcmlhbA==\n\
[Rule]\nFINAL,DIRECT\n";

/// `rurge check` knows `ss` (phase 2 M6 design 6): only a stream cipher is
/// still "not implemented", once however many lines use it; an SS 2022 key
/// of the wrong length is an error, named and not quoted (design 3.1).
#[test]
fn check_knows_ss() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "ss.conf", SS))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("ss.conf:5") && out.contains("`ss` stream cipher `rc4-md5`"),
        "{out}"
    );
    assert!(!out.contains("policy type `ss`"), "{out}");
    assert!(!out.contains("s3same") && !out.contains("MDEyMz"), "{out}");

    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "bad.conf", SS_BAD_KEY))
        .assert()
        .code(2)
        .stdout(predicate::str::contains("E0018"))
        .stdout(predicate::str::contains(
            "bad.conf:3: policy `K`: key #1 of `password` is not a Base64 key of 16 bytes, as `2022-blake3-aes-128-gcm` requires",
        ))
        .stdout(predicate::str::contains("c2VjcmV0").not());
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-engine --test outbounds_shadowsocks`
Expected: FAIL——`ss` 行还没有 spec、工厂还不会构建，经它的会话都被拒绝（`common/mod.rs:175` 是 HTTP 入站的 CONNECT 等不到应答；`:392` 是重载用例的断言）：

```text
test a_reload_keeps_an_unchanged_ss_policy_and_rebuilds_a_changed_one ... FAILED
test ss_goes_through_an_underlying_socks5_proxy_for_tcp_and_udp ... FAILED
test a_connect_leaves_through_ss_with_every_kind_of_method ... FAILED
test a_server_clock_an_hour_off_fails_the_session_with_the_reason ... FAILED
test both_obfs_modes_carry_the_session ... FAILED
test without_udp_relay_a_flow_is_rejected ... FAILED
test anyone_may_answer_through_ss ... FAILED
test udp_goes_to_udp_port ... FAILED
test udp_goes_through_ss_aead_and_2022 ... FAILED
thread 'a_reload_keeps_an_unchanged_ss_policy_and_rebuilds_a_changed_one' panicked at crates\rurge-engine\tests\outbounds_shadowsocks.rs:392:5:
thread 'ss_goes_through_an_underlying_socks5_proxy_for_tcp_and_udp' panicked at crates\rurge-engine\tests\common\mod.rs:175:9:
thread 'a_connect_leaves_through_ss_with_every_kind_of_method' panicked at crates\rurge-engine\tests\common\mod.rs:175:9:
thread 'a_server_clock_an_hour_off_fails_the_session_with_the_reason' panicked at crates\rurge-engine\tests\common\mod.rs:175:9:
thread 'both_obfs_modes_carry_the_session' panicked at crates\rurge-engine\tests\common\mod.rs:175:9:
thread 'without_udp_relay_a_flow_is_rejected' panicked at crates\rurge-engine\tests\outbounds_shadowsocks.rs:358:5:
assertion `left == right` failed
  left: Some("policy protocol not implemented: ss")
 right: Some("policy does not support UDP")
thread 'anyone_may_answer_through_ss' panicked at crates\rurge-engine\tests\common\mod.rs:277:18:
thread 'udp_goes_to_udp_port' panicked at crates\rurge-engine\tests\common\mod.rs:277:18:
thread 'udp_goes_through_ss_aead_and_2022' panicked at crates\rurge-engine\tests\common\mod.rs:277:18:
test result: FAILED. 1 passed; 9 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.05s
error: test failed, to rerun pass `-p rurge-engine --test outbounds_shadowsocks`
exit 101
```

- [ ] **Step 3: 实现**

去掉门：

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    // `ss` is read and checked in full, but the engine builds it only from
    // M6a task 6 on: until then a valid line has no spec either (the
    // capability table's `W0007`, REJECT at run time)
    let built = policy.kind != PolicyKind::Shadowsocks;
    let spec = (!failed && built && not_implemented.is_none()).then(|| PolicySpec {
```

换成

```rust
    let spec = (!failed && not_implemented.is_none()).then(|| PolicySpec {
```

工厂分支（trojan 的参数去掉 keystore）：

`crates/rurge-engine/src/outbounds.rs`——把

```rust
use rurge_proto::http::HttpOutbound;
```

换成

```rust
use rurge_proto::http::HttpOutbound;
use rurge_proto::shadowsocks::ShadowsocksOutbound;
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
            // the loader makes no spec of an `ss` line before M6a task 6
            ProtoSpec::Ss(_) => {
                return Err(BuildError::new(format!(
                    "policy `{}`: `ss` is not implemented yet",
                    spec.name
                )));
            }
```

换成

```rust
            ProtoSpec::Ss(ss) => Arc::new(ShadowsocksOutbound::new(
                &spec.name,
                server_of(spec)?,
                ss,
                spec.shadow_tls.as_ref(),
                self.roots.clone(),
                connector,
            )?),
```

能力表：

`crates/rurge/src/capabilities.rs`——把

```rust
//! (phase 2 M4c), `select` groups, `url-test` / `fallback` /
```

换成

```rust
//! (phase 2 M4c), `ss` with the AEAD, `none` and 2022 methods (phase 2
//! M6a), `select` groups, `url-test` / `fallback` /
```

`crates/rurge/src/capabilities.rs`——把

```rust
            PolicyKind::External,
```

换成

```rust
            PolicyKind::External,
            PolicyKind::Shadowsocks,
```

要点：
- 流式方法的行仍然没有 spec，照旧 `W0007` + REJECT。
- `CORE_VERSION` 不动（之前的能力翻转都没动它）。
- 订阅导入的 `ss` 行走同一个 `to_spec`，不需要额外处理。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-engine --test outbounds_shadowsocks` → 通过（10 条：三类方法的连接、两种 obfs、服务端时钟差一小时、流式方法被拒并说明、经 `underlying-proxy`（socks5）的 TCP 与 UDP、AEAD 与 2022 的 UDP、`udp-port`、全锥、没有 `udp-relay` 时按 `udp-policy-not-supported-behaviour` 拒绝、重载时参数没变的沿用、变了的重建）。
Run: `cargo test -p rurge --test cli check_knows` → 通过（新增 `check_knows_ss`）。
Run: `cargo test -p rurge-api` 与 `cargo test -p rurge-engine --test pipeline` → 通过。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates
git commit -m "feat(engine): 装配 ss 出站、能力表翻转 ss；"未实现协议"的用例例子改用 hysteria2"
```

### Task 7: 互操作与文档

shadowsocks-rust v1.25.0 的 `ssserver` 夹具与 sing-box 的 `shadowsocks` 入站（P15）；兼容性清单、手工验收、两份 README、`CLAUDE.md` 与总设计跟上 M6a。

**Files:**
- Create: `tests/interop/src/shadowsocks_rust.rs`（自带用例）、`tests/interop/tests/shadowsocks.rs`
- Modify: `tests/interop/src/lib.rs`、`tests/interop/README.md`、`.github/workflows/ci.yml`、`docs/surge-compatibility-matrix.md`、`docs/acceptance/phase2-manual.md`、`README.md`、`README_en.md`、`CLAUDE.md`、`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`

**Interfaces:**
- Consumes: 互操作夹具既有的 `Reference`、`roundtrip` / `roundtrip_big` / `udp_roundtrip`、`echo_server` / `udp_echo_server`、`outbound(profile, name, fixture)`。
- Produces: `rurge_interop::shadowsocks_rust::{BINARY_ENV, locate, ssserver_or_skip, SsInbound, render, Ssserver}`；sing-box 夹具的 `InboundKind::Shadowsocks { method }`。

- [ ] **Step 1: 互操作夹具与用例**

sing-box 夹具加 `shadowsocks` 入站：

`tests/interop/src/lib.rs`——把

```rust
//! `auto_route`; a WireGuard endpoint runs in user space).

pub mod sshd;
```

换成

```rust
//! `auto_route`; a WireGuard endpoint runs in user space).

pub mod shadowsocks_rust;
pub mod sshd;
```

`tests/interop/src/lib.rs`——把

```rust
        detour: usize,
    },
}
```

换成

```rust
        detour: usize,
    },
    /// `users[0]` holds the password (the name is ignored); the further
    /// users are 2022 users, told apart by the identity header, and the
    /// password is then the server's identity key.
    Shadowsocks {
        method: &'static str,
    },
}
```

`tests/interop/src/lib.rs`——把

```rust
                    InboundKind::ShadowTls { .. } => "shadowtls",
```

换成

```rust
                    InboundKind::ShadowTls { .. } => "shadowtls",
                    InboundKind::Shadowsocks { .. } => "shadowsocks",
```

`tests/interop/src/lib.rs`——把

```rust
                v["detour"] = json!(format!("in-{detour}"));
```

换成

```rust
                v["detour"] = json!(format!("in-{detour}"));
            } else if let InboundKind::Shadowsocks { method } = inbound.kind {
                let (_, password) = inbound.users.first().expect("a Shadowsocks password");
                v["method"] = json!(method);
                v["password"] = json!(password);
                if inbound.users.len() > 1 {
                    v["users"] = inbound.users[1..]
                        .iter()
                        .map(|(u, p)| json!({ "name": u, "password": p }))
                        .collect();
                }
```

`tests/interop/src/lib.rs`——把

```rust
            ),
        ]
```

换成

```rust
            ),
            (
                Inbound {
                    kind: InboundKind::Shadowsocks {
                        method: "aes-256-gcm",
                    },
                    users: vec![("ignored".into(), "s3same".into())],
                    tls: None,
                    ws_path: None,
                },
                1009,
            ),
            (
                Inbound {
                    kind: InboundKind::Shadowsocks {
                        method: "2022-blake3-aes-128-gcm",
                    },
                    users: vec![
                        ("ignored".into(), "MDEyMzQ1Njc4OWFiY2RlZg==".into()),
                        ("u".into(), "ZmVkY2JhOTg3NjU0MzIxMA==".into()),
                    ],
                    tls: None,
                    ws_path: None,
                },
                1010,
            ),
        ]
```

`tests/interop/src/lib.rs`——把

```rust
        assert_eq!(v2["detour"], "in-3");
```

换成

```rust
        assert_eq!(v2["detour"], "in-3");
        // one password; the further users are 2022 users under the server's key
        let ss = &config["inbounds"][8];
        assert_eq!(
            (&ss["type"], &ss["method"], &ss["password"]),
            (
                &json!("shadowsocks"),
                &json!("aes-256-gcm"),
                &json!("s3same")
            )
        );
        assert!(ss.get("users").is_none() && ss.get("network").is_none());
        let multi = &config["inbounds"][9];
        assert_eq!(multi["password"], "MDEyMzQ1Njc4OWFiY2RlZg==");
        assert_eq!(
            multi["users"],
            json!([{ "name": "u", "password": "ZmVkY2JhOTg3NjU0MzIxMA==" }])
        );
```

shadowsocks-rust 夹具（配置只监听 `127.0.0.1`，由单元用例断言）：

新建 `tests/interop/src/shadowsocks_rust.rs`：

```rust
//! A shadowsocks-rust `ssserver` child process on the loopback, the reference
//! for the `ss` outbound (phase 2 M6 design, M6-D5). The same rules as for
//! sing-box apply: nothing is downloaded or installed here, and every server
//! of the rendered configuration listens on 127.0.0.1 only — its TCP port and
//! the UDP port of the same number (`tcp_and_udp`).

use crate::{REQUIRED_ENV, Reference, free_port};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const BINARY_ENV: &str = "RURGE_TEST_SSSERVER";

/// `RURGE_TEST_SSSERVER`, else the first `ssserver` on `PATH`.
pub fn locate() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(BINARY_ENV).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let name = if cfg!(windows) {
        "ssserver.exe"
    } else {
        "ssserver"
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// The binary — or `None` after saying why `test` is skipped. With
/// `RURGE_INTEROP_REQUIRED=1` (CI) a missing binary is a failure instead.
pub fn ssserver_or_skip(test: &str) -> Option<PathBuf> {
    if let Some(path) = locate() {
        return Some(path);
    }
    if std::env::var(REQUIRED_ENV).as_deref() == Ok("1") {
        panic!("{REQUIRED_ENV}=1 but no ssserver was found ({BINARY_ENV} or PATH)");
    }
    eprintln!("skipping {test}: no ssserver ({BINARY_ENV} or PATH); see tests/interop/README.md");
    None
}

/// One server of the configuration.
pub struct SsInbound {
    pub method: &'static str,
    /// For a 2022 method the Base64 key — with `users`, the server's
    /// identity key (SIP023).
    pub password: String,
    /// 2022 users (name, Base64 key of the method's length), told apart by
    /// the identity header.
    pub users: Vec<(String, String)>,
}

/// The whole configuration for `servers`, each on its loopback port.
pub fn render(servers: &[(SsInbound, u16)]) -> Value {
    let rendered: Vec<Value> = servers
        .iter()
        .map(|(server, port)| {
            let mut v = json!({
                "server": "127.0.0.1",
                "server_port": port,
                "method": server.method,
                "password": server.password,
                "mode": "tcp_and_udp",
            });
            if !server.users.is_empty() {
                v["users"] = server
                    .users
                    .iter()
                    .map(|(name, password)| json!({ "name": name, "password": password }))
                    .collect();
            }
            v
        })
        .collect();
    json!({ "servers": rendered })
}

/// A running ssserver; killed and reaped on drop.
pub struct Ssserver(Reference);

impl Ssserver {
    /// Writes the configuration into `dir`, starts `binary` there and waits
    /// until every server accepts TCP connections.
    pub fn spawn(binary: &Path, dir: &Path, servers: Vec<SsInbound>) -> Ssserver {
        let with_ports: Vec<(SsInbound, u16)> =
            servers.into_iter().map(|s| (s, free_port())).collect();
        let ports: Vec<u16> = with_ports.iter().map(|(_, p)| *p).collect();
        let config = dir.join("ssserver.json");
        std::fs::write(&config, render(&with_ports).to_string()).expect("write the config");
        let mut command = Command::new(binary);
        command.arg("-c").arg(&config).current_dir(dir);
        Ssserver(Reference::start(
            "ssserver",
            command,
            ports,
            dir.join("ssserver.log"),
        ))
    }

    /// The loopback port (TCP and UDP) of the `index`-th server.
    pub fn port(&self, index: usize) -> u16 {
        self.0.port(index)
    }

    pub fn log_text(&self) -> String {
        self.0.log_text()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn both() -> Vec<(SsInbound, u16)> {
        vec![
            (
                SsInbound {
                    method: "aes-128-gcm",
                    password: "s3same".into(),
                    users: Vec::new(),
                },
                3001,
            ),
            (
                SsInbound {
                    method: "2022-blake3-aes-128-gcm",
                    password: "MDEyMzQ1Njc4OWFiY2RlZg==".into(),
                    users: vec![("u".into(), "ZmVkY2JhOTg3NjU0MzIxMA==".into())],
                },
                3002,
            ),
        ]
    }

    #[test]
    fn the_configuration_stays_on_the_loopback() {
        let config = render(&both());
        let text = config.to_string();
        for forbidden in [
            "0.0.0.0",
            "::",
            "local_address",
            "locals",
            "manager",
            "plugin",
            "outbound_",
            "acl",
        ] {
            assert!(!text.contains(forbidden), "`{forbidden}` in {text}");
        }
        let top: Vec<&String> = config.as_object().unwrap().keys().collect();
        assert_eq!(top, ["servers"]);
        for server in config["servers"].as_array().unwrap() {
            assert_eq!(server["server"], "127.0.0.1");
            assert_eq!(server["mode"], "tcp_and_udp");
        }
    }

    #[test]
    fn servers_are_rendered_as_ssserver_spells_them() {
        let config = render(&both());
        assert_eq!(
            config["servers"][0],
            json!({
                "server": "127.0.0.1",
                "server_port": 3001,
                "method": "aes-128-gcm",
                "password": "s3same",
                "mode": "tcp_and_udp",
            })
        );
        let multi = &config["servers"][1];
        assert_eq!(multi["password"], "MDEyMzQ1Njc4OWFiY2RlZg==");
        assert_eq!(
            multi["users"],
            json!([{ "name": "u", "password": "ZmVkY2JhOTg3NjU0MzIxMA==" }])
        );
    }
}
```

用例：

新建 `tests/interop/tests/shadowsocks.rs`：

```rust
//! rurge's `ss` outbound against shadowsocks-rust's `ssserver` and sing-box's
//! `shadowsocks` inbound (phase 2 M6 design 3.4, M6-D5): TCP with one chunk
//! and with many, and UDP, for the AEAD and the 2022 methods, the latter
//! also as one of several users. Every target is a loopback IP literal.

mod common;

use common::*;
use rurge_interop::shadowsocks_rust::{SsInbound, Ssserver, ssserver_or_skip};

const SERVER_16: &str = "MDEyMzQ1Njc4OWFiY2RlZg==";
const USER_16: &str = "ZmVkY2JhOTg3NjU0MzIxMA==";
const SERVER_32: &str = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=";
const USER_32: &str = "ZmVkY2JhOTg3NjU0MzIxMGZlZGNiYTk4NzY1NDMyMTA=";
const OTHER_16: &str = "dGhlIG90aGVyIHVzZXIhIQ==";
const OTHER_32: &str = "dGhlIG90aGVyIHVzZXIgaGFzIDMyIGJ5dGVzLCB0b28=";

/// `name = ss, 127.0.0.1, <port>, …` with UDP on.
fn line(name: &str, port: u16, method: &str, password: &str) -> String {
    format!(
        "{name} = ss, 127.0.0.1, {port}, encrypt-method={method}, password={password}, udp-relay=true\n"
    )
}

/// Each policy of `profile`: one small and one large TCP round trip, then UDP.
async fn every_way(profile: &str, names: &[&str]) {
    let (echo, udp_echo) = (echo_server().await, udp_echo_server().await);
    for name in names {
        let out = outbound(profile, name, None);
        roundtrip(&out, echo).await;
        roundtrip_big(&out, echo).await;
        udp_roundtrip(&out, udp_echo).await;
    }
}

fn server(method: &'static str, password: &str) -> SsInbound {
    SsInbound {
        method,
        password: password.into(),
        users: Vec::new(),
    }
}

/// The AEAD methods of ssserver's release build (`aes-192-gcm` and
/// `xchacha20-ietf-poly1305` are not in it; sing-box covers them below).
#[tokio::test]
async fn aead_methods_against_ssserver() {
    let Some(bin) = ssserver_or_skip("aead_methods_against_ssserver") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let methods = ["aes-128-gcm", "aes-256-gcm", "chacha20-ietf-poly1305"];
    let ss = Ssserver::spawn(
        &bin,
        dir.path(),
        methods.iter().map(|m| server(m, "s3same")).collect(),
    );
    let profile = format!(
        "[Proxy]\n{}{}{}[Rule]\nFINAL,DIRECT\n",
        line("A128", ss.port(0), methods[0], "s3same"),
        line("A256", ss.port(1), methods[1], "s3same"),
        line("Chacha", ss.port(2), methods[2], "s3same"),
    );
    every_way(&profile, &["A128", "A256", "Chacha"]).await;
}

/// Both 2022 methods with a single key, and `2022-blake3-aes-256-gcm` as the
/// second of two users (`serverKey:userKey`, one identity header).
#[tokio::test]
async fn ss_2022_against_ssserver_with_one_key_and_as_a_user() {
    let Some(bin) = ssserver_or_skip("ss_2022_against_ssserver_with_one_key_and_as_a_user") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let ss = Ssserver::spawn(
        &bin,
        dir.path(),
        vec![
            server("2022-blake3-aes-128-gcm", SERVER_16),
            server("2022-blake3-aes-256-gcm", SERVER_32),
            SsInbound {
                method: "2022-blake3-aes-256-gcm",
                password: SERVER_32.into(),
                users: vec![
                    ("other".into(), OTHER_32.into()),
                    ("u".into(), USER_32.into()),
                ],
            },
        ],
    );
    let profile = format!(
        "[Proxy]\n{}{}{}[Rule]\nFINAL,DIRECT\n",
        line("B128", ss.port(0), "2022-blake3-aes-128-gcm", SERVER_16),
        line("B256", ss.port(1), "2022-blake3-aes-256-gcm", SERVER_32),
        line(
            "User",
            ss.port(2),
            "2022-blake3-aes-256-gcm",
            &format!("{SERVER_32}:{USER_32}")
        ),
    );
    every_way(&profile, &["B128", "B256", "User"]).await;
}

fn ss_inbound(method: &'static str, password: &str, users: &[&str]) -> Inbound {
    let mut all = vec![("ignored".to_string(), password.to_string())];
    all.extend(
        users
            .iter()
            .enumerate()
            .map(|(i, key)| (format!("u{i}"), key.to_string())),
    );
    Inbound {
        kind: InboundKind::Shadowsocks { method },
        users: all,
        tls: None,
        ws_path: None,
    }
}

/// sing-box's `shadowsocks` inbound: the two AEAD methods ssserver's release
/// build lacks, a 2022 method with a single key and one as the second of two users.
#[tokio::test]
async fn ss_against_sing_box_aead_2022_and_a_user() {
    let Some(bin) = sing_box_or_skip("ss_against_sing_box_aead_2022_and_a_user") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let sb = SingBox::spawn(
        &bin,
        dir.path(),
        vec![
            ss_inbound("aes-192-gcm", "s3same", &[]),
            ss_inbound("xchacha20-ietf-poly1305", "s3same", &[]),
            ss_inbound("2022-blake3-aes-256-gcm", SERVER_32, &[]),
            ss_inbound("2022-blake3-aes-128-gcm", SERVER_16, &[OTHER_16, USER_16]),
        ],
    );
    let profile = format!(
        "[Proxy]\n{}{}{}{}[Rule]\nFINAL,DIRECT\n",
        line("A192", sb.port(0), "aes-192-gcm", "s3same"),
        line("XChacha", sb.port(1), "xchacha20-ietf-poly1305", "s3same"),
        line("B256", sb.port(2), "2022-blake3-aes-256-gcm", SERVER_32),
        line(
            "User",
            sb.port(3),
            "2022-blake3-aes-128-gcm",
            &format!("{SERVER_16}:{USER_16}")
        ),
    );
    every_way(&profile, &["A192", "XChacha", "B256", "User"]).await;
}
```

- [ ] **Step 2: 运行**

Run: `cargo test -p rurge-interop`
Expected: 本机没有 `ssserver` 与 sing-box 时，三条 Shadowsocks 用例各打印一行 `skipping …` 后通过，夹具的单元用例（`the_configuration_stays_on_the_loopback`、`servers_are_rendered_as_ssserver_spells_them`、sing-box 渲染的扩展断言）照常断言并通过。互操作由首次推送后的 CI 证明。

- [ ] **Step 3: CI 与文档**

CI 安装 shadowsocks-rust（SHA-256 取自 GitHub 发布资产的 `digest` 字段）并设 `RURGE_TEST_SSSERVER`：

`.github/workflows/ci.yml`——把

```yaml
          echo "RURGE_TEST_XRAY=$bin" >> "$GITHUB_ENV"
```

换成

```yaml
          echo "RURGE_TEST_XRAY=$bin" >> "$GITHUB_ENV"
      - name: Install shadowsocks-rust for the Shadowsocks interoperability tests
        shell: bash
        run: |
          set -euo pipefail
          version=1.25.0
          case "$RUNNER_OS" in
            Linux)   asset="shadowsocks-v$version.x86_64-unknown-linux-gnu.tar.xz"; sha=874f817fcf3e6d7681ec715a1c13c686c6eaae936524d102639b38364f3966ae ;;
            Windows) asset="shadowsocks-v$version.x86_64-pc-windows-msvc.zip";     sha=882151ea5c52941d4a3360ebd12c74c6d6bd1b599596089ef1811b379c705666 ;;
            macOS)   asset="shadowsocks-v$version.aarch64-apple-darwin.tar.xz";    sha=58e0caf0cc9266c4ea226f38aa20fb28c1be12efc87a73cf5903197867555208 ;;
            *) echo "unexpected runner OS: $RUNNER_OS"; exit 1 ;;
          esac
          cd "$RUNNER_TEMP"
          curl -fsSL --retry 3 --retry-all-errors -o "$asset" "https://github.com/shadowsocks/shadowsocks-rust/releases/download/v$version/$asset"
          if command -v sha256sum >/dev/null 2>&1; then
            actual=$(sha256sum "$asset" | cut -d' ' -f1)
          else
            actual=$(shasum -a 256 "$asset" | cut -d' ' -f1)
          fi
          if [ "$actual" != "$sha" ]; then
            echo "shadowsocks-rust checksum mismatch: expected $sha, got $actual"
            exit 1
          fi
          mkdir -p shadowsocks-rust
          case "$asset" in
            *.zip) unzip -q "$asset" -d shadowsocks-rust ;;
            *)     tar -xJf "$asset" -C shadowsocks-rust ;;
          esac
          bin=$(find "$PWD/shadowsocks-rust" -type f \( -name ssserver -o -name ssserver.exe \) | head -n 1)
          [ -n "$bin" ] || { echo "no ssserver binary in $asset"; exit 1; }
          chmod +x "$bin"
          if [ "$RUNNER_OS" = "Windows" ]; then bin=$(cygpath -w "$bin"); fi
          echo "RURGE_TEST_SSSERVER=$bin" >> "$GITHUB_ENV"
```

`tests/interop/README.md`——把

```markdown
`rurge-interop` 是仅供测试使用的工作区成员（`publish = false`），不对外发布，也不是任何其它 crate 的依赖。它把 [sing-box](https://sing-box.sagernet.org/) 与 [xray](https://github.com/XTLS/Xray-core) 作为参照实现，以回环子进程的方式拉起来，驱动 rurge 的 `http` / `https` / `socks5` / `trojan` / `vmess` / `anytls` / `wireguard` 出站，以及包在 Shadow TLS 里的 `trojan`，去连它们，验证 rurge 与真实的第三方实现互通。xray 只用来跑 `vmess`：VMess 协议由 xray 所在的这一脉实现定义，sing-box 的实现是重写，手写的编解码需要两个独立参照互相印证（M2 设计 M2-D5）。
```

换成

```markdown
`rurge-interop` 是仅供测试使用的工作区成员（`publish = false`），不对外发布，也不是任何其它 crate 的依赖。它把 [sing-box](https://sing-box.sagernet.org/)、[xray](https://github.com/XTLS/Xray-core) 与 [shadowsocks-rust](https://github.com/shadowsocks/shadowsocks-rust) 的 `ssserver` 作为参照实现，以回环子进程的方式拉起来，驱动 rurge 的 `http` / `https` / `socks5` / `trojan` / `vmess` / `anytls` / `wireguard` / `ss` 出站，以及包在 Shadow TLS 里的 `trojan`，去连它们，验证 rurge 与真实的第三方实现互通。xray 只用来跑 `vmess`：VMess 协议由 xray 所在的这一脉实现定义，sing-box 的实现是重写，手写的编解码需要两个独立参照互相印证（M2 设计 M2-D5）。
```

`tests/interop/README.md`——把

```markdown
本地默认不安装 sing-box 与 xray：`cargo test -p rurge-interop` 会正常通过，sing-box 的十四个互操作用例与 xray 的两个互操作用例各打印一行 `skipping …` 后直接返回（两个夹具自身的单元测试——配置渲染、"配置绝不碰本机"的安全守卫、取空闲端口——照常运行并断言）。要在本机真正跑 sing-box 的用例，二选一：
```

换成

```markdown
本地默认不安装 sing-box、xray 与 shadowsocks-rust：`cargo test -p rurge-interop` 会正常通过，sing-box 的十五个互操作用例、xray 的两个与 shadowsocks-rust 的两个互操作用例各打印一行 `skipping …` 后直接返回（各夹具自身的单元测试——配置渲染、"配置绝不碰本机"的安全守卫、取空闲端口——照常运行并断言）。要在本机真正跑 sing-box 的用例，二选一：
```

`tests/interop/README.md`——把

```markdown
xray 的本机运行方式同理，见下面「xray」一节。
```

换成

```markdown
xray 与 shadowsocks-rust 的本机运行方式同理，见下面「xray」「shadowsocks-rust」两节。
```

`tests/interop/README.md`——把

```markdown
- `RURGE_INTEROP_REQUIRED=1`：找不到二进制时让用例直接失败，而不是打印 `skipping …` 后跳过；对 sing-box 与 xray 两个夹具都生效。CI 会设置它，本机一般不需要。
```

换成

```markdown
- `RURGE_TEST_SSSERVER`：shadowsocks-rust 的 `ssserver` 可执行文件的路径，优先于 `PATH` 查找。
- `RURGE_INTEROP_REQUIRED=1`：找不到二进制时让用例直接失败，而不是打印 `skipping …` 后跳过；对 sing-box、xray、shadowsocks-rust 与 sshd 各夹具都生效。CI 会设置它，本机一般不需要。
```

`tests/interop/README.md`——把

```markdown
- WireGuard（`tests/sing_box_wireguard.rs`）：sing-box 的 WireGuard 端点（`endpoints`，sing-box 1.11 起；`system: false`，在用户态运行，不建网卡、不改路由）以 rurge 为唯一的 peer，给发往它的每个报文写上保留字节 `1/2/3`；rurge 的 `wireguard` 出站带 `client-id = 1/2/3` 与它握手，经隧道连 sing-box 自己的隧道地址 `10.9.0.1` 上的 echo 端口（节里 `allowed-ips = 10.9.0.1/32`）：sing-box 把发往端点自身地址的连接改写到它的回环 `127.0.0.1` / `::1`，echo 就听在那里（sing-box 1.14.1 `protocol/wireguard/endpoint.go` 的 `NewConnectionEx`）——直接发往 `127.0.0.1` 的目标经隧道进来，可能被它的用户态协议栈丢弃；隧道里出来的连接交给 `direct`。覆盖单块与跨多块的往返、原生测速（强制握手），以及经隧道往返一个 UDP 回显（阶段 2 / M5c；发往隧道地址 `10.9.0.1` 上回显的端口，同样被改写到回环）。rurge 收到的报文里 sing-box 写的保留字节必须先清零，否则 boringtun 认不出报文类型、握手不成。端点没有监听地址这一项，**它的 UDP 端口开在所有地址上**；就绪与否看同一份配置里一个只听 `127.0.0.1` 的 `mixed` 入站（UDP 端口无从探测）。

**不覆盖 `socks5-tls`**：sing-box 的 `socks` 与 `mixed` 入站没有 `tls` 字段（参见 sing-box 文档 `configuration/inbound/{socks,mixed}`），无法用它搭建一个会说 TLS 的 SOCKS5 服务端。`socks5-tls` 的覆盖仍由 M1a 的回环假上游（`rurge_proto::testing::FakeSocks5`）承担。
```

换成

```markdown
- WireGuard（`tests/sing_box_wireguard.rs`）：sing-box 的 WireGuard 端点（`endpoints`，sing-box 1.11 起；`system: false`，在用户态运行，不建网卡、不改路由）以 rurge 为唯一的 peer，给发往它的每个报文写上保留字节 `1/2/3`；rurge 的 `wireguard` 出站带 `client-id = 1/2/3` 与它握手，经隧道连 sing-box 自己的隧道地址 `10.9.0.1` 上的 echo 端口（节里 `allowed-ips = 10.9.0.1/32`）：sing-box 把发往端点自身地址的连接改写到它的回环 `127.0.0.1` / `::1`，echo 就听在那里（sing-box 1.14.1 `protocol/wireguard/endpoint.go` 的 `NewConnectionEx`）——直接发往 `127.0.0.1` 的目标经隧道进来，可能被它的用户态协议栈丢弃；隧道里出来的连接交给 `direct`。覆盖单块与跨多块的往返、原生测速（强制握手），以及经隧道往返一个 UDP 回显（阶段 2 / M5c；发往隧道地址 `10.9.0.1` 上回显的端口，同样被改写到回环）。rurge 收到的报文里 sing-box 写的保留字节必须先清零，否则 boringtun 认不出报文类型、握手不成。端点没有监听地址这一项，**它的 UDP 端口开在所有地址上**；就绪与否看同一份配置里一个只听 `127.0.0.1` 的 `mixed` 入站（UDP 端口无从探测）。

- Shadowsocks（`tests/shadowsocks.rs` 里的 `ss_against_sing_box_aead_2022_and_a_user`，阶段 2 / M6a）：sing-box 的 `shadowsocks` 入站，`aes-192-gcm` 与 `xchacha20-ietf-poly1305` 两种 AEAD 方法（shadowsocks-rust 的发布包不带这两种，见下面「shadowsocks-rust」一节）、单密钥的 `2022-blake3-aes-256-gcm`，以及 `2022-blake3-aes-128-gcm` 两个用户里的第二个（`password=服务端密钥:用户密钥`，一层身份头）；每种都做单块与跨多块（100 000 字节）的 TCP 往返，以及经 `udp-relay=true` 往返一个回环 UDP 回显两次。

**不覆盖 `socks5-tls`**：sing-box 的 `socks` 与 `mixed` 入站没有 `tls` 字段（参见 sing-box 文档 `configuration/inbound/{socks,mixed}`），无法用它搭建一个会说 TLS 的 SOCKS5 服务端。`socks5-tls` 的覆盖仍由 M1a 的回环假上游（`rurge_proto::testing::FakeSocks5`）承担。
```

`tests/interop/README.md`——把

```markdown
渲染出的配置（`rurge_interop::xray::render`）只有 `log` / `inbounds` / `outbounds` 三个顶层键：每个入站是一个只监听 `127.0.0.1` 的 `vmess`（可选 `ws` 传输），唯一的出站是 `freedom`。

## sshd
```

换成

```markdown
渲染出的配置（`rurge_interop::xray::render`）只有 `log` / `inbounds` / `outbounds` 三个顶层键：每个入站是一个只监听 `127.0.0.1` 的 `vmess`（可选 `ws` 传输），唯一的出站是 `freedom`。

## shadowsocks-rust

互操作测试固定 shadowsocks-rust **v1.25.0**（2026-08-26 发布），只用其中的 `ssserver`。CI 下载并校验以下三个发布包（SHA-256 取自 GitHub 发布页每个资产的 `digest`）：

| 平台 | 资产 | SHA-256 |
| ---- | ---- | ------- |
| Linux (amd64) | `shadowsocks-v1.25.0.x86_64-unknown-linux-gnu.tar.xz` | `874f817fcf3e6d7681ec715a1c13c686c6eaae936524d102639b38364f3966ae` |
| Windows (amd64) | `shadowsocks-v1.25.0.x86_64-pc-windows-msvc.zip` | `882151ea5c52941d4a3360ebd12c74c6d6bd1b599596089ef1811b379c705666` |
| macOS (arm64) | `shadowsocks-v1.25.0.aarch64-apple-darwin.tar.xz` | `58e0caf0cc9266c4ea226f38aa20fb28c1be12efc87a73cf5903197867555208` |

`tests/shadowsocks.rs` 里对 `ssserver` 的两个用例覆盖 `ss`：

- `aead_methods_against_ssserver`：`aes-128-gcm`、`aes-256-gcm`、`chacha20-ietf-poly1305`。
- `ss_2022_against_ssserver_with_one_key_and_as_a_user`：单密钥的 `2022-blake3-aes-128-gcm` 与 `2022-blake3-aes-256-gcm`，以及 `2022-blake3-aes-256-gcm` 两个用户里的第二个（`ssserver` 配置的 `users`，SIP023 的身份头）。

每种方法都做单块与跨多块（100 000 字节）的 TCP 往返，以及经 `udp-relay=true` 往返一个回环 UDP 回显两次（`ssserver` 的 `mode` 是 `tcp_and_udp`，UDP 端口与 TCP 端口同号）。

`ssserver` 的发布包（默认的 `full` 特性）不带 `aes-192-gcm` 与 `xchacha20-ietf-poly1305`（它们在 `aead-cipher-extra` 特性里），这两种由 sing-box 的 `shadowsocks` 入站覆盖（见上面「覆盖范围」），另有回环假服务端（`rurge_proto::testing::FakeShadowsocks`）。`none` 只由假服务端覆盖。

**不覆盖 obfs**：simple-obfs 已停止维护，sing-box 不带插件，`http` / `tls` 两种 obfs 都没有可用的预编译参考服务端；它们由假服务端（`rurge_proto::testing::accept_obfs`）与手工验收（`docs/acceptance/phase2-manual.md` 的 M6a 一节）覆盖。

本机不安装 shadowsocks-rust：`RURGE_TEST_SSSERVER`（优先于 `PATH` 查找）没有指向可执行文件、`PATH` 上也找不到 `ssserver` / `ssserver.exe` 时，用例打印一行 `skipping …` 后直接返回；这个 crate 不会下载或安装它。互操作由首次推送后的 CI 证明（CI 安装 shadowsocks-rust v1.25.0 并设置 `RURGE_TEST_SSSERVER` 与 `RURGE_INTEROP_REQUIRED=1`）。

渲染出的配置（`rurge_interop::shadowsocks_rust::render`）只有 `servers` 一个顶层键：每个服务端只监听 `127.0.0.1`，`mode` 为 `tcp_and_udp`，没有 `manager`、`locals`、插件、ACL 与 `outbound_*` 这些键（夹具的单元用例 `the_configuration_stays_on_the_loopback` 断言这一点）。

## sshd
```

`tests/interop/README.md`——把

```markdown
- 夹具渲染出的 sing-box 配置只有 `log` / `inbounds` / `outbounds` 三个顶层键（WireGuard 的配置另有 `endpoints`）；每个入站只监听 `127.0.0.1`；唯一的出站是 `direct`。WireGuard 端点在用户态运行（`system: false`），它的 UDP 端口开在所有地址上（端点没有监听地址这一项；`rurge_interop::render_wireguard` 的单元测试 `the_wireguard_configuration_never_touches_the_machine` 断言其余各项）。xray 配置同样只有这三个顶层键，唯一的出站是 `freedom`。任何地方都不出现 `set_system_proxy`、`tun`、`auto_route` 这些键（`rurge_interop::render` 与 `rurge_interop::xray::render` 的单元测试 `the_configuration_never_touches_the_machine` 各自断言这一点）；`shadowtls` 入站的 `handshake.server` 恒为 `127.0.0.1`（夹具的单元用例断言）。
- 每个用例的连接目标都是回环 IP 字面量（`127.0.0.1` 上的 echo / 测试服务器；WireGuard 用例在隧道里连的是 sing-box 自己的隧道地址 `10.9.0.1`，由 sing-box 映射到它的 `127.0.0.1`），sing-box 与 xray 因此既不解析域名也不会访问公网。
- 这个 crate 本身不下载、不安装任何东西；本机是否装有 sing-box 或 xray 由项目所有者决定，没装就跳过。
```

换成

```markdown
- 夹具渲染出的 sing-box 配置只有 `log` / `inbounds` / `outbounds` 三个顶层键（WireGuard 的配置另有 `endpoints`）；每个入站只监听 `127.0.0.1`；唯一的出站是 `direct`。WireGuard 端点在用户态运行（`system: false`），它的 UDP 端口开在所有地址上（端点没有监听地址这一项；`rurge_interop::render_wireguard` 的单元测试 `the_wireguard_configuration_never_touches_the_machine` 断言其余各项）。xray 配置同样只有这三个顶层键，唯一的出站是 `freedom`。`ssserver` 的配置只有 `servers`，每个服务端只听 `127.0.0.1`（TCP 与同号的 UDP）。任何地方都不出现 `set_system_proxy`、`tun`、`auto_route` 这些键（`rurge_interop::render` 与 `rurge_interop::xray::render` 的单元测试 `the_configuration_never_touches_the_machine` 各自断言这一点）；`shadowtls` 入站的 `handshake.server` 恒为 `127.0.0.1`（夹具的单元用例断言）。
- 每个用例的连接目标都是回环 IP 字面量（`127.0.0.1` 上的 echo / 测试服务器；WireGuard 用例在隧道里连的是 sing-box 自己的隧道地址 `10.9.0.1`，由 sing-box 映射到它的 `127.0.0.1`），sing-box、xray 与 `ssserver` 因此既不解析域名也不会访问公网。
- 这个 crate 本身不下载、不安装任何东西；本机是否装有 sing-box、xray 或 shadowsocks-rust 由项目所有者决定，没装就跳过。
```

兼容性清单（4.2 的 `ss` 行与"未实现协议"行、4.5 的 `udp-relay` / `udp-port`、4.6 的三条 `ss` 参数行、`GET /v1/profiles/current` 的脱敏行）：

`docs/surge-compatibility-matrix.md`——把

```markdown
| `ss` | Shadowsocks | 全部 | ✅ | 2 | 加密方法见 4.6 |
```

换成

```markdown
| `ss` | Shadowsocks | 全部 | ✅ | 2 | 加密方法见 4.6。M6a（阶段 2）已实现 TCP 与 UDP：AEAD（`aes-128-gcm` `aes-192-gcm` `aes-256-gcm` `chacha20-ietf-poly1305` `xchacha20-ietf-poly1305`；主密钥 `EVP_BytesToKey`，每个方向一个随机 salt，子密钥 HKDF-SHA1，每块负载至多 0x3FFF 字节）、`none`（只写地址、不加密；写了口令照收不用）与 SS 2022（`2022-blake3-aes-128-gcm` `2022-blake3-aes-256-gcm`，SIP022：BLAKE3 子密钥，每块至多 0xFFFF 字节；请求带时间戳，应答须是类型 1、时间戳与本机相差不超过 30 秒、回显的请求 salt 与自己的一致，否则以 `ss: the server's clock differs from ours by <n> seconds (at most 30 are allowed)` 或 `ss: the server's answer is not for this request` 失败；多用户 `password=身份密钥:…:用户密钥`，每层一个 SIP023 身份头）。请求头（SOCKS5 地址）与首段负载合并成一次写出，客户端 100 ms 内不发数据时（服务端先说话的协议）请求头单独发出、这类协议的首字节因此晚 100 ms；SS 2022 这时补 1–900 字节的填充（内容为零，SIP022 只要求长度随机），带首段负载时不填充。口令错在连接期无法识别（协议没有鉴权应答，服务端一般一直不回或直接关闭），第一次读到 EOF 时是 `ss: the server closed the connection without answering`，与服务端的其它问题分辨不出；解不开的应答是 `ss: the server's data failed to decrypt (wrong password or method?)`。`obfs=http` / `obfs=tls`（simple-obfs）：伪装层在 Shadow TLS 之上、协议之下；`obfs-host` 缺省为服务器主机名，`obfs-uri` 缺省为 `/`（两者手册都没写缺省值，这是 rurge 的决定；simple-obfs 自己的缺省主机名是 `cloudfront.net`）；`http` 的 `Host` 在服务器端口不是 80 时带上 `:端口`（写了 `obfs-host` 也带，与 simple-obfs 客户端一致）；`tls` 的首个包（伪造的 ClientHello，首段负载放在 session ticket 扩展里）不超过 16384 字节，首段负载多出的部分随下一个记录发出；没有 obfs 的预编译参考服务端，兼容性靠手工验收。流式旧方法到 M8 才实现：加载时每种方法每次加载一条 `W0007`，运行时 `REJECT`（见 4.6 与 `W0007` 行）。UDP（`udp-relay=true`）：发往 `udp-port`（缺省主端口），全锥；AEAD 每个包一个随机 salt；SS 2022 每个载体一个随机 session id、包号从 0 递增，回包校验类型、时间戳（±30 秒）与回显的客户端 session id，按服务端 session 维护 8128 个包号的防重放窗口（每个载体至多记 8 个服务端 session，最旧的先忘），不合格的回包静默丢弃（`debug` 日志只写原因）；只收源 IP 是服务器的包（经链、服务器仍是主机名时不检查）；Shadow TLS 与 obfs 只作用于 TCP，UDP 直接发往 `udp-port`；有 `underlying-proxy` 时经底层策略的 UDP 载体；目标主机名的字母表规则同 http / socks5 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| 策略指向 rurge 尚未实现的协议类型 | Surge 原生支持全部协议 | 全部 | 🟡 | 1 / 2 | 加载时告警 `W0007`；运行时该策略按 `REJECT` 处理，会话日志 `error = policy protocol not implemented: <type>`；随阶段 2 各里程碑逐协议移除：M1 已移除 `http` `https` `socks5` `socks5-tls`；M2a 已移除 `trojan`；M2b 已移除 `vmess`（写了 `vmess-aead=true` 的行）与 `anytls`；M4a 已移除 `ssh`；M4b 已移除 `wireguard`；M4c 已移除 `external`。没写 `vmess-aead=true` 的 `vmess` 行是唯一例外：仍按 `W0007` 处理，但走专门的诊断文本 `` `vmess` without `vmess-aead=true` uses the legacy handshake, which is not implemented yet ``（每次加载一条，不是每行一条）与专门的会话日志文本 `policy protocol not implemented: vmess (legacy handshake)`，不是这里的通用 `<type>` 模板 |
```

换成

```markdown
| 策略指向 rurge 尚未实现的协议类型 | Surge 原生支持全部协议 | 全部 | 🟡 | 1 / 2 | 加载时告警 `W0007`；运行时该策略按 `REJECT` 处理，会话日志 `error = policy protocol not implemented: <type>`；随阶段 2 各里程碑逐协议移除：M1 已移除 `http` `https` `socks5` `socks5-tls`；M2a 已移除 `trojan`；M2b 已移除 `vmess`（写了 `vmess-aead=true` 的行）与 `anytls`；M4a 已移除 `ssh`；M4b 已移除 `wireguard`；M4c 已移除 `external`；M6a 已移除 `ss`（流式旧方法除外）。例外有两个：没写 `vmess-aead=true` 的 `vmess` 行仍按 `W0007` 处理，但走专门的诊断文本 `` `vmess` without `vmess-aead=true` uses the legacy handshake, which is not implemented yet ``（每次加载一条，不是每行一条）与专门的会话日志文本 `policy protocol not implemented: vmess (legacy handshake)`；流式旧方法的 `ss` 行同样如此，诊断文本 `` `ss` stream cipher `<method>` is not implemented yet; such policies behave as REJECT ``（每种方法每次加载一条），会话日志 `policy protocol not implemented: ss (<method>)`；两者都不是这里的通用 `<type>` 模板 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `udp-relay`（布尔；默认 false） | 适用 SOCKS5 / SOCKS5-TLS / Shadowsocks / External / HTTP/2 CONNECT（RFC 9298） | ✅ M5a：`socks5` / `socks5-tls` / `external` 已生效；Shadowsocks 与 HTTP/2 CONNECT 随 M6 | 2 |
| `udp-port`（端口；默认主端口） | 适用 Shadowsocks / Snell | ✅ | 2 |
```

换成

```markdown
| `udp-relay`（布尔；默认 false） | 适用 SOCKS5 / SOCKS5-TLS / Shadowsocks / External / HTTP/2 CONNECT（RFC 9298） | ✅ M5a：`socks5` / `socks5-tls` / `external` 已生效；M6a：Shadowsocks 已生效；HTTP/2 CONNECT 随 M6c | 2 |
| `udp-port`（端口；默认主端口） | 适用 Shadowsocks / Snell | ✅ M6a：Shadowsocks 已生效（1–65535，其它取值 `E0018`；没写 `udp-relay=true` 时不用）；Snell 随 M6b | 2 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `ss` | `encrypt-method`：AEAD 2022 `2022-blake3-aes-128-gcm` `2022-blake3-aes-256-gcm`；AEAD `aes-128-gcm` `aes-192-gcm` `aes-256-gcm` `chacha20-ietf-poly1305` `xchacha20-ietf-poly1305`；`none` | ✅ | 2 | 2022 方法密码为 Base64 密钥（16/32 字节），支持 `serverKey:userKey` |
| `ss` | 流式旧方法 `rc4` `rc4-md5` `aes-128/192/256-cfb` `aes-128/192/256-ctr` `salsa20` `chacha20` `chacha20-ietf` | 🟡 | 2 | 低优先级，加载时告警"不推荐" |
| `ss` | `password` `udp-relay` `udp-port` `obfs`（`http` / `tls`）`obfs-host` `obfs-uri` | ✅ | 2 | |
```

换成

```markdown
| `ss` | `encrypt-method`：AEAD 2022 `2022-blake3-aes-128-gcm` `2022-blake3-aes-256-gcm`；AEAD `aes-128-gcm` `aes-192-gcm` `aes-256-gcm` `chacha20-ietf-poly1305` `xchacha20-ietf-poly1305`；`none` | ✅ | 2 | M6a 已实现。`encrypt-method` 必填、不区分大小写，不认识的方法 `E0018`；`2022-blake3-chacha20-poly1305` 不在 Surge 的方法表里，不接受。2022 方法的 `password` 按冒号分段，每段是标准字母表的 Base64 密钥（填充可有可无）、长度须与方法相符（16 / 32 字节），不合法 `E0018`（`` key #<n> of `password` is not a Base64 key of <len> bytes, as `<method>` requires ``，不引用取值）；最后一段是用户密钥，前面各段是逐层的身份密钥（`serverKey:userKey`，SIP023）。`none` 不要求 `password` |
| `ss` | 流式旧方法 `rc4` `rc4-md5` `aes-128/192/256-cfb` `aes-128/192/256-ctr` `bf-cfb` `camellia-128/192/256-cfb` `cast5-cfb` `des-cfb` `idea-cfb` `rc2-cfb` `seed-cfb` `salsa20` `chacha20` `chacha20-ietf` | 🟡 | 2（M8） | 只解析：加载时 `W0007`（`` `ss` stream cipher `<method>` is not implemented yet; such policies behave as REJECT ``，每种方法每次加载一条），运行时 `REJECT`，会话日志 `policy protocol not implemented: ss (<method>)`；`password` 仍必填；订阅导入的这类行跳过并告警；实现排在 M8 |
| `ss` | `password` `udp-relay` `udp-port` `obfs`（`http` / `tls`）`obfs-host` `obfs-uri` | ✅ | 2 | M6a 已实现。`password` 只读命名写法（与 trojan 相同），除 `none` 外必填（`E0018`）；`udp-relay` 缺省 false；`udp-port` 1–65535、缺省主端口；`obfs` 其它取值 `E0018`；`obfs-host` 缺省为服务器主机名、须是不含空格与控制字符的 ASCII（否则 `E0018`，不引用取值）；`obfs-uri` 缺省 `/`、须以 `/` 开头（否则 `E0018`），`obfs=tls` 时写了 `W0028` 并忽略；没写 `obfs` 时这两个参数 `W0028` 并忽略（缺省值是 rurge 的决定，手册没写）；TLS 参数（`sni` 等）不适用于 `ss`（`W0028`）；可叠 Shadow TLS 与 `underlying-proxy` |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `GET /v1/profiles/current?sensitive=0` | 当前配置文本（可脱敏） | 全部 | ✅ | 1 | M4a 已实现；`sensitive=0`（默认）脱敏：独立密钥行 `password` / `ca-passphrase` / `ca-p12` / `private-key` / `psk` / `pre-shared-key` / `token`；内联参数 `password` / `psk` / `private-key` / `pre-shared-key` / `base64` / `token` / `uuid` / `username` / `headers` / `ws-headers` / `ws-path` / `shadow-tls-password` / `policy-path` / `external-policy-modifier`（后两个自 M3a 起：订阅链接常带 token，修饰列表能设任何参数）/ `test-url`（自 M3b 起：订阅行设的测试 URL 可能带 token）/ `args`（自 M4c 起：`external` 的参数里常带口令）；`http-api` / `external-controller-access` / `http-listen` / `socks5-listen` 的 `key@` 前缀与 `wifi-access-http-auth` 口令；所有写作 `type, server, port` 的代理类型（`http` `https` `h2-connect` `socks5` `socks5-tls` `ss` `snell` `vmess` `trojan` `tuic` `tuic-v5` `hysteria2` `masque` `anytls` `trust-tunnel` `ssh`）策略行第 4 个起、凡不是 `name=value` 具名参数的 token（最常见的是根本不含 `=` 的裸 token，即位置传递的凭据；含 `=` 但值为空或全是 `=` 的也算，例如带填充的 base64。除前四种外这个位置本就是多余参数 `W0001`，按偏安全一侧抹掉）。取值的结尾按解析器自己的规则找**第一个顶层逗号**：`"` 或 `'` 在值里任何位置都会开启一段引号（`"` 内 `\` 转义下一个字符），`(` / `)` 分组，引号内与括号内的逗号都属于值（`password="p,w"`、`password=ab"c,d"`、`password=a(b,c)d` 各是一整个值）；引号或括号未闭合时抹到行尾。名单以外不脱敏；其余内容与行数、行尾 CRLF 原样保留 |
```

换成

```markdown
| `GET /v1/profiles/current?sensitive=0` | 当前配置文本（可脱敏） | 全部 | ✅ | 1 | M4a 已实现；`sensitive=0`（默认）脱敏：独立密钥行 `password` / `ca-passphrase` / `ca-p12` / `private-key` / `psk` / `pre-shared-key` / `token`；内联参数 `password` / `psk` / `private-key` / `pre-shared-key` / `base64` / `token` / `uuid` / `username` / `headers` / `ws-headers` / `ws-path` / `shadow-tls-password` / `policy-path` / `external-policy-modifier`（后两个自 M3a 起：订阅链接常带 token，修饰列表能设任何参数）/ `test-url`（自 M3b 起：订阅行设的测试 URL 可能带 token）/ `args`（自 M4c 起：`external` 的参数里常带口令）/ `obfs-host`（自 M6a 起：伪装域名可能是用户的指纹）；`http-api` / `external-controller-access` / `http-listen` / `socks5-listen` 的 `key@` 前缀与 `wifi-access-http-auth` 口令；所有写作 `type, server, port` 的代理类型（`http` `https` `h2-connect` `socks5` `socks5-tls` `ss` `snell` `vmess` `trojan` `tuic` `tuic-v5` `hysteria2` `masque` `anytls` `trust-tunnel` `ssh`）策略行第 4 个起、凡不是 `name=value` 具名参数的 token（最常见的是根本不含 `=` 的裸 token，即位置传递的凭据；含 `=` 但值为空或全是 `=` 的也算，例如带填充的 base64。除前四种外这个位置本就是多余参数 `W0001`，按偏安全一侧抹掉）。取值的结尾按解析器自己的规则找**第一个顶层逗号**：`"` 或 `'` 在值里任何位置都会开启一段引号（`"` 内 `\` 转义下一个字符），`(` / `)` 分组，引号内与括号内的逗号都属于值（`password="p,w"`、`password=ab"c,d"`、`password=a(b,c)d` 各是一整个值）；引号或括号未闭合时抹到行尾。名单以外不脱敏；其余内容与行数、行尾 CRLF 原样保留 |
```

手工验收：

`docs/acceptance/phase2-manual.md`——把

```markdown
- [ ] `dns-follow-interface`：`direct` 策略写 `interface=<第二块网卡>, dns-follow-interface=true`，`dns-server` 写一个只经第二块网卡可达的 DNS 服务器（或用抓包确认），规则把某个域名分到这条策略，访问它：抓包看到 DNS 查询从第二块网卡发出；配了 `encrypted-dns-server` 时查询照常走加密 DNS，日志有一条 `dns-follow-interface: the encrypted DNS servers are asked as usual` 的说明。

```

换成

```markdown
- [ ] `dns-follow-interface`：`direct` 策略写 `interface=<第二块网卡>, dns-follow-interface=true`，`dns-server` 写一个只经第二块网卡可达的 DNS 服务器（或用抓包确认），规则把某个域名分到这条策略，访问它：抓包看到 DNS 查询从第二块网卡发出；配了 `encrypted-dns-server` 时查询照常走加密 DNS，日志有一条 `dns-follow-interface: the encrypted DNS servers are asked as usual` 的说明。

## M6a　Shadowsocks

前置：同 M5a 一节的 SOCKS5 UDP 客户端；自己的 Shadowsocks 节点（记下服务端实现与版本，如 shadowsocks-rust、shadowsocks-libev、sing-box、Xray），至少一个 AEAD 方法（如 `aes-256-gcm` 或 `chacha20-ietf-poly1305`）与一个 2022 方法（`2022-blake3-aes-128-gcm` 或 `2022-blake3-aes-256-gcm`），都开 UDP；一个配了多用户（SIP023 身份头）的 2022 节点；一个带 simple-obfs 的节点（服务端插件 `obfs-server`，`http` 与 `tls` 两种模式各一次）。每份配置 `[Rule]` 里 `FINAL,<策略名>`，`rurge check -c <配置>` 零错误（没有 `W0007`）。

- [ ] AEAD 的 TCP：`ss` 策略写 `encrypt-method=<方法>, password=<口令>`，`curl -x http://127.0.0.1:<http-listen 端口> https://example.com/ -I` 与 `curl --socks5-hostname 127.0.0.1:<socks5-listen 端口> https://example.com/ -I` 都返回 200，请求记录里策略链是该策略、`error` 为空；服务端先说话的协议（经代理连一个 SMTP / SSH 主机）能看到对端的欢迎行。
- [ ] 2022 的 TCP：`encrypt-method=2022-blake3-…, password=<Base64 密钥>`，同上；本机时钟准确。
- [ ] UDP：两个节点都加 `udp-relay=true`，经 SOCKS5 UDP 发 DNS 查询（如 Proxifier 代理 `nslookup example.com 8.8.8.8`）得到回答；经它进行一次语音通话或联机游戏；用 NAT 类型检测工具（STUN）检测，结果是 Full Cone（节点的出口须是全锥）。去掉 `udp-relay=true` 后 UDP 按 `udp-policy-not-supported-behaviour` 处理（默认 REJECT，记录写 `policy does not support UDP`）。
- [ ] 多用户：`password=<服务端密钥>:<用户密钥>` 连多用户节点，TCP 与 UDP 都能往返；服务端日志（若有）认出的是这个用户。把用户密钥换成节点不认识的一把，连接得不到应答（`ss: the server closed the connection without answering`）。
- [ ] obfs：`obfs=http, obfs-host=<伪装域名>` 与 `obfs=tls, obfs-host=<伪装域名>` 各连一次带 `obfs-server` 的节点，TCP 都能往返；抓包看到 `http` 模式的首个包是带 `Host: <伪装域名>:<端口>`（端口为 80 时不带端口）与 `Upgrade: websocket` 的 `GET` 请求，`tls` 模式的首个包是 SNI 为伪装域名的 ClientHello；不写 `obfs-host` 时伪装域名是服务器主机名。
- [ ] `udp-port`：服务端的 UDP 另开在一个端口上（或经端口转发把 UDP 转到另一个端口），策略写 `udp-port=<该端口>`，UDP 照常往返；抓包确认 UDP 发往 `udp-port`、TCP 仍发往主端口。
- [ ] 口令错误：把 AEAD 节点的 `password` 改错一位，重载后访问 `https://example.com/`：请求失败，会话记录的 `error` 是 `ss: the server closed the connection without answering`（或转发阶段的错误；服务端一直不回时由空闲超时结束），与服务端的其它问题分辨不出（已知差异，见兼容性清单 `ss` 一行）；**错误文本与日志都不含口令**。2022 节点把密钥换成另一把合法长度的 Base64 密钥，表现相同；把密钥写成不合法的 Base64，`rurge check` 报 `E0018` 且不引用取值。
- [ ] 时钟偏差：2022 节点，把本机时钟拨偏 2 分钟（超出 30 秒的窗口），重复 TCP 一项：服务端不应答，会话记录的 `error` 同上一项；改回时钟后恢复正常。
- [ ] 脱敏：`GET /v1/policies/detail?policy_name=<策略名>` 与 `GET /v1/profiles/current` 里 `password` 与 `obfs-host` 都是 `***`。

```

README：

`README.md`——把

```markdown
> **阶段 1 功能齐备：M1 ～ M4b**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）进行中：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；M3b（测速与自动组）已完成——`url-test` / `fallback` / `load-balance` 组按连通性测试（经各节点在同一条连接上两次 `HEAD`，HTTPS 测试 URL 在已建立的 TLS 连接上测第二次）自动选择，`interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` 与策略的 `test-url` / `test-timeout` 生效，自动组可经 `POST /v1/policy_groups/select` 临时覆盖，新增 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 三个端点；M3c（`smart`）已完成——`smart` 组按真实会话的首字节耗时与测速结果打分（失败罚分、`policy-priority` 因子、约 1 小时的站点记忆），选中的成员连不上时自动换下一个（最多再试 2 个），请求记录增加 `connectMs` / `firstByteMs`；M4a（SSH）已完成——`ssh` 策略（TCP；每个策略一条 SSH 会话、每个连接一个通道；口令或 `[Keystore]` 里的 OpenSSH 私钥登录；`server-fingerprint` 校验主机密钥；按 `idle-timeout` 断开空闲会话、30 秒一次保活）已可用；M4b（WireGuard）已完成——`wireguard` 策略（TCP；用户态隧道，不建虚拟网卡；多个 peer 按 `allowed-ips` 选路；`client-id`（WARP 的保留字节）；配了 `dns-server` 时目标域名经隧道查询，否则在本机解析；没有 `dns-server` 也没写 `test-url` 时以握手测速）已可用；M4c（external）已完成——`external` 策略（TCP；第一次用到时拉起外部程序、经它在本机端口上的 SOCKS5 转发；程序退出后下次用到时再拉起；输出写进数据目录；rurge 退出时连同它启动的进程一起停掉——Unix 用进程组，Windows 用 Job Object；三平台都支持，Surge 只在 Mac 上有）已可用，订阅导入的 `external` 一律跳过；M5a（UDP 地基）已完成——SOCKS5 监听支持 UDP ASSOCIATE，UDP 按规则分流到 DIRECT、REJECT 或 `socks5` / `socks5-tls` / `external`（`udp-relay=true`，含 `underlying-proxy` 链），全锥 NAT，每条 UDP 流一条请求记录，`block-quic` 与 `udp-policy-not-supported-behaviour` 生效；M5b（TLS 族的 UDP）已完成——`trojan`（UDP ASSOCIATE）、`anytls`（UDP over TCP v2）全锥，`vmess`（命令 2，每个目标一条连接）对称型；M5c（WireGuard 的 UDP 与其余）已完成——`wireguard` 的 UDP（全锥）与经 `underlying-proxy` 的隧道、`test-udp` / `proxy-test-udp`、`smart` 组计入 UDP、`dns-follow-interface`；其余出站协议仍在阶段 2 后续里程碑，`subnet` 策略组在阶段 3。
```

换成

```markdown
> **阶段 1 功能齐备：M1 ～ M4b**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）进行中：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；M3b（测速与自动组）已完成——`url-test` / `fallback` / `load-balance` 组按连通性测试（经各节点在同一条连接上两次 `HEAD`，HTTPS 测试 URL 在已建立的 TLS 连接上测第二次）自动选择，`interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` 与策略的 `test-url` / `test-timeout` 生效，自动组可经 `POST /v1/policy_groups/select` 临时覆盖，新增 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 三个端点；M3c（`smart`）已完成——`smart` 组按真实会话的首字节耗时与测速结果打分（失败罚分、`policy-priority` 因子、约 1 小时的站点记忆），选中的成员连不上时自动换下一个（最多再试 2 个），请求记录增加 `connectMs` / `firstByteMs`；M4a（SSH）已完成——`ssh` 策略（TCP；每个策略一条 SSH 会话、每个连接一个通道；口令或 `[Keystore]` 里的 OpenSSH 私钥登录；`server-fingerprint` 校验主机密钥；按 `idle-timeout` 断开空闲会话、30 秒一次保活）已可用；M4b（WireGuard）已完成——`wireguard` 策略（TCP；用户态隧道，不建虚拟网卡；多个 peer 按 `allowed-ips` 选路；`client-id`（WARP 的保留字节）；配了 `dns-server` 时目标域名经隧道查询，否则在本机解析；没有 `dns-server` 也没写 `test-url` 时以握手测速）已可用；M4c（external）已完成——`external` 策略（TCP；第一次用到时拉起外部程序、经它在本机端口上的 SOCKS5 转发；程序退出后下次用到时再拉起；输出写进数据目录；rurge 退出时连同它启动的进程一起停掉——Unix 用进程组，Windows 用 Job Object；三平台都支持，Surge 只在 Mac 上有）已可用，订阅导入的 `external` 一律跳过；M5a（UDP 地基）已完成——SOCKS5 监听支持 UDP ASSOCIATE，UDP 按规则分流到 DIRECT、REJECT 或 `socks5` / `socks5-tls` / `external`（`udp-relay=true`，含 `underlying-proxy` 链），全锥 NAT，每条 UDP 流一条请求记录，`block-quic` 与 `udp-policy-not-supported-behaviour` 生效；M5b（TLS 族的 UDP）已完成——`trojan`（UDP ASSOCIATE）、`anytls`（UDP over TCP v2）全锥，`vmess`（命令 2，每个目标一条连接）对称型；M5c（WireGuard 的 UDP 与其余）已完成——`wireguard` 的 UDP（全锥）与经 `underlying-proxy` 的隧道、`test-udp` / `proxy-test-udp`、`smart` 组计入 UDP、`dns-follow-interface`；M6（Shadowsocks / Snell / HTTP/2 族）进行中：M6a（Shadowsocks）已完成——`ss` 的 AEAD、`none` 与 SS 2022（含多用户身份头）方法、simple-obfs（`http` / `tls`），TCP 与 UDP（`udp-relay=true`、`udp-port`，全锥），流式旧方法在 M8 之前按 REJECT 处理；其余出站协议仍在阶段 2 后续里程碑，`subnet` 策略组在阶段 3。
```

`README.md`——把

```markdown
| 出站协议       | DIRECT / REJECT 系列 / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / 外部程序（DIRECT 与 REJECT 系四种已实现，M3a；HTTP / SOCKS5（含 `socks5-tls`）TCP 上游与 `underlying-proxy` 链已实现，阶段 2 / M1；Trojan（TCP；WebSocket 传输）已实现，阶段 2 / M2a；VMess（AEAD 握手，TCP；可叠加 TLS / WebSocket）与 AnyTLS（TCP，会话复用）已实现，阶段 2 / M2b；SSH（TCP，会话复用）已实现，阶段 2 / M4a；WireGuard（TCP，用户态隧道）已实现，阶段 2 / M4b；外部程序（TCP，三平台）已实现，阶段 2 / M4c） | 2     |
```

换成

```markdown
| 出站协议       | DIRECT / REJECT 系列 / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / 外部程序（DIRECT 与 REJECT 系四种已实现，M3a；HTTP / SOCKS5（含 `socks5-tls`）TCP 上游与 `underlying-proxy` 链已实现，阶段 2 / M1；Trojan（TCP；WebSocket 传输）已实现，阶段 2 / M2a；VMess（AEAD 握手，TCP；可叠加 TLS / WebSocket）与 AnyTLS（TCP，会话复用）已实现，阶段 2 / M2b；SSH（TCP，会话复用）已实现，阶段 2 / M4a；WireGuard（TCP，用户态隧道）已实现，阶段 2 / M4b；外部程序（TCP，三平台）已实现，阶段 2 / M4c；Shadowsocks（AEAD / 2022、obfs，TCP 与 UDP）已实现，阶段 2 / M6a） | 2     |
```

`README_en.md`——把

```markdown
> **Phase 1 feature-complete: M1 through M4b** (profile parsing, rule engine, rule sets, GeoIP, external resource management, DNS client, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`; M3b added the request log and traffic stats, SNI recording, idle timeout, REJECT auto-escalation, a 502 for failed CONNECT dials, graceful shutdown, hot reload (SIGHUP / `--watch`), `--log-file`, and `encrypted-dns-follow-outbound-mode`; M4a added a Surge-compatible HTTP API (phase-1 endpoints, `X-Key` auth with banning), outbound mode / global policy persisted to `state.json`, and `rurge reload` / `stop` / `status`; M4b added the system proxy (Windows registry + a WinINet notification, macOS `networksetup`, Linux GNOME / KDE, `--system-proxy` and `POST /v1/features/system_proxy`, automatic recovery on exit or crash, and following listener changes across a reload) and `rurge service install / uninstall [--dry-run]` (systemd / launchd / a Windows scheduled task)) — the phase-1 acceptance criteria that need a real desktop environment (the three-platform system proxy, service install) have not been through manual acceptance yet; see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md) for the checklist. The dashboard is phase 6. Phase 2 (outbound protocols and policy groups) is in progress: M1 (outbound foundation and HTTP / SOCKS5 upstreams) is done — `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections by rule through DIRECT / REJECT or `http` / `https` / `socks5` / `socks5-tls` upstreams (TCP, including two-or-more-hop `underlying-proxy` chains), and a `select` group's choice can be read and switched over the HTTP API (see [docs/api/phase2.md](docs/api/phase2.md)); M2a (the TLS family, Trojan first) is done — the `trojan` policy (TCP, TLS always on, optionally over WebSocket) is usable; M2b (VMess / AnyTLS) is done — the `vmess` (AEAD handshake, TCP, optionally over TLS / WebSocket) and `anytls` (TCP, session reuse) policies are usable, and a reload reuses outbounds by fingerprint without disturbing connection pools still in use; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); M3a (member assembly and subscriptions) is done — `policy-path` subscriptions (a local file or a URL, in Surge format; an update rebuilds only the policy table, without disturbing unrelated connections), `include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`, the group-level `underlying-proxy` (derived `Name (via Relay)` members), group cycle warnings (`W0030`) and the empty-group fallback (DIRECT by default, REJECT with `--empty-group-reject`) are usable; M3b (connectivity tests and automatic groups) is done — `url-test` / `fallback` / `load-balance` groups choose by connectivity tests (two `HEAD`s over one connection through each member, the second one on the established TLS connection for an HTTPS test URL), `interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` and the policies' `test-url` / `test-timeout` take effect, an automatic group takes a temporary override through `POST /v1/policy_groups/select`, and `POST /v1/policies/test`, `GET /v1/policy_groups/test_results` and `POST /v1/policy_groups/test` are available; M3c (`smart`) is done — a `smart` group scores its members by the first-byte time of real sessions and by the tests (failure penalties, `policy-priority` factors, per-site memory for about an hour) and moves on to the next member when the one it picked does not connect (up to two more), and the request log gains `connectMs` / `firstByteMs`; M4a (SSH) is done — the `ssh` policy (TCP; one SSH session per policy and a channel per connection; login with a password or an OpenSSH private key from `[Keystore]`; host keys checked against `server-fingerprint`; idle sessions closed after `idle-timeout`, a keepalive every 30 seconds) is usable; M4b (WireGuard) is done — the `wireguard` policy (TCP; a user-space tunnel, no virtual interface; several peers chosen by `allowed-ips`; `client-id` (WARP's reserved bytes); destination names looked up through the tunnel when the section has a `dns-server`, on this machine otherwise; tested by a handshake when there is neither a `dns-server` nor a `test-url`) is usable; M4c (external) is done — the `external` policy (TCP; the program is started on first use and reached as a SOCKS5 proxy on its local port; a program that exited is started again on the next use; its output goes to the data directory; when rurge exits, the program is stopped together with whatever it started — a process group on Unix, a Job Object on Windows; all three platforms, where Surge has it on the Mac only) is usable, and subscriptions never bring `external` policies in; M5a (the UDP foundation) is done — the SOCKS5 listener takes UDP ASSOCIATE, and UDP is routed by rule to DIRECT, REJECT or `socks5` / `socks5-tls` / `external` (`udp-relay=true`, `underlying-proxy` chains included), full-cone NAT, one request record per UDP flow, and `block-quic` and `udp-policy-not-supported-behaviour` take effect; M5b (UDP over the TLS family) is done — `trojan` (UDP ASSOCIATE) and `anytls` (UDP over TCP v2) with full-cone NAT, `vmess` (command 2, one connection per target) symmetric; M5c (UDP over WireGuard and the rest) is done — UDP over `wireguard` (full cone) and tunnels over an `underlying-proxy`, `test-udp` / `proxy-test-udp`, UDP in `smart` groups, and `dns-follow-interface`; the remaining outbound protocols are later phase-2 milestones, and the `subnet` group comes in phase 3.
```

换成

```markdown
> **Phase 1 feature-complete: M1 through M4b** (profile parsing, rule engine, rule sets, GeoIP, external resource management, DNS client, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`; M3b added the request log and traffic stats, SNI recording, idle timeout, REJECT auto-escalation, a 502 for failed CONNECT dials, graceful shutdown, hot reload (SIGHUP / `--watch`), `--log-file`, and `encrypted-dns-follow-outbound-mode`; M4a added a Surge-compatible HTTP API (phase-1 endpoints, `X-Key` auth with banning), outbound mode / global policy persisted to `state.json`, and `rurge reload` / `stop` / `status`; M4b added the system proxy (Windows registry + a WinINet notification, macOS `networksetup`, Linux GNOME / KDE, `--system-proxy` and `POST /v1/features/system_proxy`, automatic recovery on exit or crash, and following listener changes across a reload) and `rurge service install / uninstall [--dry-run]` (systemd / launchd / a Windows scheduled task)) — the phase-1 acceptance criteria that need a real desktop environment (the three-platform system proxy, service install) have not been through manual acceptance yet; see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md) for the checklist. The dashboard is phase 6. Phase 2 (outbound protocols and policy groups) is in progress: M1 (outbound foundation and HTTP / SOCKS5 upstreams) is done — `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections by rule through DIRECT / REJECT or `http` / `https` / `socks5` / `socks5-tls` upstreams (TCP, including two-or-more-hop `underlying-proxy` chains), and a `select` group's choice can be read and switched over the HTTP API (see [docs/api/phase2.md](docs/api/phase2.md)); M2a (the TLS family, Trojan first) is done — the `trojan` policy (TCP, TLS always on, optionally over WebSocket) is usable; M2b (VMess / AnyTLS) is done — the `vmess` (AEAD handshake, TCP, optionally over TLS / WebSocket) and `anytls` (TCP, session reuse) policies are usable, and a reload reuses outbounds by fingerprint without disturbing connection pools still in use; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); M3a (member assembly and subscriptions) is done — `policy-path` subscriptions (a local file or a URL, in Surge format; an update rebuilds only the policy table, without disturbing unrelated connections), `include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`, the group-level `underlying-proxy` (derived `Name (via Relay)` members), group cycle warnings (`W0030`) and the empty-group fallback (DIRECT by default, REJECT with `--empty-group-reject`) are usable; M3b (connectivity tests and automatic groups) is done — `url-test` / `fallback` / `load-balance` groups choose by connectivity tests (two `HEAD`s over one connection through each member, the second one on the established TLS connection for an HTTPS test URL), `interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` and the policies' `test-url` / `test-timeout` take effect, an automatic group takes a temporary override through `POST /v1/policy_groups/select`, and `POST /v1/policies/test`, `GET /v1/policy_groups/test_results` and `POST /v1/policy_groups/test` are available; M3c (`smart`) is done — a `smart` group scores its members by the first-byte time of real sessions and by the tests (failure penalties, `policy-priority` factors, per-site memory for about an hour) and moves on to the next member when the one it picked does not connect (up to two more), and the request log gains `connectMs` / `firstByteMs`; M4a (SSH) is done — the `ssh` policy (TCP; one SSH session per policy and a channel per connection; login with a password or an OpenSSH private key from `[Keystore]`; host keys checked against `server-fingerprint`; idle sessions closed after `idle-timeout`, a keepalive every 30 seconds) is usable; M4b (WireGuard) is done — the `wireguard` policy (TCP; a user-space tunnel, no virtual interface; several peers chosen by `allowed-ips`; `client-id` (WARP's reserved bytes); destination names looked up through the tunnel when the section has a `dns-server`, on this machine otherwise; tested by a handshake when there is neither a `dns-server` nor a `test-url`) is usable; M4c (external) is done — the `external` policy (TCP; the program is started on first use and reached as a SOCKS5 proxy on its local port; a program that exited is started again on the next use; its output goes to the data directory; when rurge exits, the program is stopped together with whatever it started — a process group on Unix, a Job Object on Windows; all three platforms, where Surge has it on the Mac only) is usable, and subscriptions never bring `external` policies in; M5a (the UDP foundation) is done — the SOCKS5 listener takes UDP ASSOCIATE, and UDP is routed by rule to DIRECT, REJECT or `socks5` / `socks5-tls` / `external` (`udp-relay=true`, `underlying-proxy` chains included), full-cone NAT, one request record per UDP flow, and `block-quic` and `udp-policy-not-supported-behaviour` take effect; M5b (UDP over the TLS family) is done — `trojan` (UDP ASSOCIATE) and `anytls` (UDP over TCP v2) with full-cone NAT, `vmess` (command 2, one connection per target) symmetric; M5c (UDP over WireGuard and the rest) is done — UDP over `wireguard` (full cone) and tunnels over an `underlying-proxy`, `test-udp` / `proxy-test-udp`, UDP in `smart` groups, and `dns-follow-interface`; M6 (Shadowsocks / Snell / the HTTP/2 family) is in progress: M6a (Shadowsocks) is done — `ss` with the AEAD, `none` and SS 2022 (multi-user identity headers included) methods, simple-obfs (`http` / `tls`), TCP and UDP (`udp-relay=true`, `udp-port`, full cone), with the legacy stream ciphers behaving as REJECT until M8; the remaining outbound protocols are later phase-2 milestones, and the `subnet` group comes in phase 3.
```

`README_en.md`——把

```markdown
| Outbound            | DIRECT / REJECT family / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / external program (DIRECT and the four REJECT flavours implemented, M3a; HTTP / SOCKS5 (including `socks5-tls`) TCP upstreams and `underlying-proxy` chains implemented, phase 2 / M1; Trojan (TCP; WebSocket transport) implemented, phase 2 / M2a; VMess (AEAD handshake, TCP; optionally TLS / WebSocket) and AnyTLS (TCP, session reuse) implemented, phase 2 / M2b; SSH (TCP, session reuse) implemented, phase 2 / M4a; WireGuard (TCP, user-space tunnel) implemented, phase 2 / M4b; external program (TCP, all three platforms) implemented, phase 2 / M4c) | 2     |
```

换成

```markdown
| Outbound            | DIRECT / REJECT family / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / external program (DIRECT and the four REJECT flavours implemented, M3a; HTTP / SOCKS5 (including `socks5-tls`) TCP upstreams and `underlying-proxy` chains implemented, phase 2 / M1; Trojan (TCP; WebSocket transport) implemented, phase 2 / M2a; VMess (AEAD handshake, TCP; optionally TLS / WebSocket) and AnyTLS (TCP, session reuse) implemented, phase 2 / M2b; SSH (TCP, session reuse) implemented, phase 2 / M4a; WireGuard (TCP, user-space tunnel) implemented, phase 2 / M4b; external program (TCP, all three platforms) implemented, phase 2 / M4c; Shadowsocks (AEAD / 2022, obfs, TCP and UDP) implemented, phase 2 / M6a) | 2     |
```

`CLAUDE.md`（当前状态、文档清单、常用命令）：

`CLAUDE.md`——把

```markdown
阶段 1 功能已齐备（M1 ～ M4b）；阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`。M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。M3b（测速与自动组）已完成——`[General] test-timeout` 区分未设置（`General::test_target`；策略的 `test-url` / `test-timeout` 生效，`W0029` 对它们退役）；`rurge-policy` 的 `probe`（经策略自己的出站在一条连接上两次 HEAD，HTTPS 在已建立的 TLS 连接上测第二次，根证书取 `OutboundFactory::roots`）、`testbook`（`TestBook`：结果按策略与定义保存、同一策略同时只测一次、最多 8 个并发、`TestObserver`、一次性的 `test_once`）与 `auto`（`url-test` / `fallback` / `load-balance` 三种选法、`SelectCtx`、`AutoGroups`：临时覆盖、`url-test` 保持的成员、每组上一轮测试的时间与测试请求）；注册表按测试结果选成员（只有拨号触发测试、`evaluate-before-use` 的 `Resolution.pending`、嵌套组的分数、`round_timeout`）、`resolve_relay`（空组当中继一律拒绝）、订阅行不能动用主配置的 `client-cert` 与策略（`W0023`）；`rurge-engine` 的测试调度任务、测试会话进请求记录（规则 `policy test`）、`evaluate-before-use` 的有界等待（`policy group evaluation failed`）、重载时保留未变组的覆盖、`Engine::snapshot`（运行时与注册表在代际锁下成对读取）与 DNS 会话在链底为 REJECT 时的旁路；`rurge-api` 的 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 与 `POST /v1/policy_groups/select` 对自动组的临时覆盖；能力表翻转 `url-test` / `fallback` / `load-balance`（`W0008` 只剩 `smart` 与 `subnet`）。M3c（`smart`）已完成——`rurge-policy::smart`（`SmartBook`：按策略的时间加权首字节分数与失败罚分、健康 / 未知 / 失败三种状态、站点记忆、使用计数；`rank` 给出选中的成员与重试列表；`sample` 给大组的常规轮抽样）、注册表的 `smart` 成员过滤与 `policy-priority` 因子、`Resolution.smart`（`SmartPick`）、`resolve_member`、`test_round`，以及每代预存的测试查表（`TestBook::outcome`，M3b 延后事项 #14）；`rurge-inbound` 的 `SessionHandle` 两个时刻（出站就绪、首字节）、可挂多个的结束钩子与首字节钩子、`mark_upstream_failed`；`rurge-engine` 的 `smart` 模块（拨号时按重试列表换成员，单次时限"剩余时间 ÷ 剩余尝试次数"；质量回报：首字节、3 秒无响应、上游先断）、请求记录的 `connectMs` / `firstByteMs`、`automatic()` 纳入 `smart`、每次发布注册表时 `SmartBook::retain`；能力表翻转 `smart`（`W0008` 只剩 `subnet`）。M4（WireGuard / SSH / external）按三份计划推进（M4a SSH → M4b WireGuard → M4c external）：M4a（SSH）已完成——`rurge-config::spec::SshSpec`（`server-fingerprint` 解析为 `HostKeyPin`、`idle-timeout`、`private-key` 的 `E0020`）与 `ProtoSpec::keystore_item`（重载的指纹含 SSH 私钥）；新 crate `rurge-proto-ssh`（russh 0.63.3，`ring` 后端）：Keystore 私钥解码（Ed25519 / ECDSA / RSA；带口令与 DSA 为 `E0022`）、主机密钥比对、`SshOutbound`（每个策略一条会话、单飞握手、认证失败等持久失败后按 60 秒起翻倍至 10 分钟退避、钉住的主机密钥算法优先协商、`direct-tcpip` 通道、会话断了重建一次、按通道计的空闲断开、30 秒 × 3 的保活、没配指纹的一次性告警、RSA 只用 SHA-2 签名）与 `testing::FakeSsh`；`rurge-engine` 的工厂分支；订阅行自己的 `private-key=` 进订阅安全门（`W0023`）；`tests/interop` 的临时 `sshd`（`RURGE_TEST_SSHD`，只在 Unix）；能力表翻转 `ssh`。M4b（WireGuard）已完成——`rurge-config` 的 `[WireGuard <name>]` 类型化（`WireGuardSection`，`E0023`，重名的节 `W0020`）与 `spec::WireGuardSpec`（spec 带着节的内容；`underlying-proxy` 为 `W0029` 且策略 REJECT；订阅行自己的 `section-name=` 进订阅安全门）；`rurge-net` 的 `Datagram`、`Connector::connect_udp` 与 `DirectConnector::connect_udp`（按 `ip-version`、`set_tos`、7 MiB 收发缓冲）；新 crate `rurge-proto-wireguard`（boringtun 0.7.1 的 sans-IO `Tunn` 加 smoltcp 0.12：`routes` 最长前缀选 peer、`wire` 的 `client-id`、`stack`（锁内只做内存操作，每次推进都 poll 到发完，Reno 拥塞控制）、`device`（每条隧道一个任务、批量收、握手 TOS 0x88、5 分钟重拨与 endpoint 跟随、发送时本机地址或路由失效或握手无回应时换新载体（网络变化后自愈）、网络变化入口、同一私钥与 peer 只留一条隧道，节与载体设置都相同的策略共用）、`stream`、`dns`（隧道内 DNS 与缓存；每个问题 2 秒内重发、先等隧道第一次握手）、`outbound`（`WireGuardOutbound`：没有 `dns-server` 时在本机解析、含 `[Host]`；原生握手测速）与 `testing::FakeWgPeer`）；`rurge-proto` 的 `Outbound::native_test`；`rurge-policy` 的 `TestMode`（URL 与原生两种测速，`wireguard` 另加 10 秒）；`rurge-engine` 的工厂分支、拨号时 `Unsupported` 带说明、DNS 会话防环扩展到 endpoint 写成域名的 `wireguard`；bin 不输出 boringtun 自己的日志；`tests/interop` 对 sing-box WireGuard 端点（带保留字节）的互操作用例；能力表翻转 `wireguard`。M4c（external）已完成——`rurge-config::spec::ExternalSpec`（`args` 是 `Secret`；两个 `external` 策略写了同一个 `local-port` 是 `E0018`；`args` 进内联参数的脱敏名单）、订阅导入的 `external` 行一律跳过（`W0023`）；`rurge-platform::process`（Unix 进程组与 `killpg`，Windows Job Object）；`rurge_proto::external`（`ExternalOutbound`：第一次用到时拉起、输出写 `<数据目录>/external/<策略名>.log` 且超过 1 MiB 时在拉起前轮转、子进程环境去掉代理变量并设 `NO_PROXY=*`、程序退出后下次用到时再拉起且两次拉起至少隔 2 秒、每 500 ms 连一次本机端口最多 6 次；`ProcessHook` / `ProcessGroup` 与 `NoProcessGroups`；程序自己退出时连同它启动的进程一起结束）；`rurge-engine` 的工厂分支、`EngineShared.processes` / `externals` 与 `Engine::stop_external_programs`；bin 的 `PlatformProcesses` 与退出流程最后停掉全部外部程序；只用于测试的工作区成员 `tests/external`（`rurge-external-tests`：极小的 SOCKS5 辅助程序 `socks-helper` 与真实拉起它的用例）；能力表翻转 `external`（`W0007` 不再因 `external` 出现）。M4 至此完成。M5（UDP 路径）按三份计划推进（M5a 地基 → M5b TLS 族 → M5c WireGuard 与其余）：M5a 已完成——`rurge_net::connector::PacketSocket`（按包收发、带地址的 UDP 载体）与 `Connector::open_udp`（`DirectConnector`：每个地址族一个未连接的 socket，忽略 Windows 的 ICMP 不可达报错）；`Outbound::udp()` / `open_udp()` 与 `UdpSupport`；DIRECT 与 `socks5` / `socks5-tls` / `external` 的 UDP（`udp-relay`，`W0029` 退役）；`ChainConnector::open_udp`（链式 UDP 载体）；`rurge-inbound` 的 SOCKS5 UDP ASSOCIATE（`UdpClient`、`UdpAdmission`、`Dialer::admit_udp` / `associate`）；`rurge-engine` 的 UDP 流水线（`udp` 模块：一条流 = 关联 + 目标、按"关联 × 出站"共用载体的全锥、60 秒 / DNS 10 秒回收、1024 流 / 4096 关联的上限）、请求记录与 API 的 `transport`、`block-quic`（`W0029` 退役）与 `udp-policy-not-supported-behaviour`、QUIC Initial 识别、`PROTOCOL` 规则按传输层匹配 `TCP` / `UDP`；`FakeSocks5` 与 `tests/external` 辅助程序的 UDP ASSOCIATE；对 sing-box `socks` 入站的 UDP 互操作用例。M5b（TLS 族的 UDP）已完成——`rurge-proto` 的 `stream_udp`（一条字节流上按包收发：请求头随第一个包发出、写那个包的目标；`trojan` 的 UDP ASSOCIATE 与 `anytls` 的 UDP over TCP v2 两种封装）、`trojan` / `anytls` 的 `open_udp`（全锥）、`vmess` 的命令 2（`vmess::udp`：每个目标一条 VMess 连接、随它的第一个包建立、每个数据报一个分块，对称型）；`FakeTrojan` / `FakeAnyTls` / `FakeVmess` 的 UDP 与 `rurge_proto::testing::udp_echo_server`；经引擎的端到端用例（`tests/udp_tls_family.rs`）；对 sing-box（三种）与 xray（vmess）的 UDP 互操作用例。M5c（WireGuard 的 UDP 与其余）已完成——`rurge-proto-wireguard` 的 `TunnelUdp`（隧道里每个地址族一个 UDP socket、第一次发往该族时绑定、全锥，目标名经隧道 DNS 或本机解析）与 `Stack::udp_bind` / `check`；`rurge_net::packet_datagram`（把 `PacketSocket` 变成一条到固定目标的 `Datagram`）与 `ChainConnector::connect_udp`，`wireguard` 的载体经 `underlying-proxy`（底层策略不载 UDP 时拨号失败，`W0029` 退役）；启动时 peer 连不上的告警每个策略的每个 peer 5 分钟至多一次（M4b 延后事项 #15）；`rurge_policy::udp_probe`（经策略的 UDP 向 `hostname@ipv4` 问一次 A 记录）、`Engine::test_udp` 与 `POST /v1/policies/test` 结果里的 `udp` 键（`test-udp` / `proxy-test-udp`，不保存、不参与组的选择）；`smart` 计入 UDP（载体打不开算失败、第一个回包算首字节、3 秒无回包只在 53 / 443 端口算失败、UDP 不换成员）；`dns-follow-interface`（`rurge_net::connector::Via` / `ResolveVia`、`Resolver::lookup_via`：策略自己的解析经它的 `interface` 问普通 DNS 服务器、答案另存，配了加密 DNS 时不跟随；没有 `interface` 时 `W0028`）；对 sing-box WireGuard 端点的 UDP 互操作。M5 至此完成。
```

换成

```markdown
阶段 1 功能已齐备（M1 ～ M4b）；阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`。M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。M3b（测速与自动组）已完成——`[General] test-timeout` 区分未设置（`General::test_target`；策略的 `test-url` / `test-timeout` 生效，`W0029` 对它们退役）；`rurge-policy` 的 `probe`（经策略自己的出站在一条连接上两次 HEAD，HTTPS 在已建立的 TLS 连接上测第二次，根证书取 `OutboundFactory::roots`）、`testbook`（`TestBook`：结果按策略与定义保存、同一策略同时只测一次、最多 8 个并发、`TestObserver`、一次性的 `test_once`）与 `auto`（`url-test` / `fallback` / `load-balance` 三种选法、`SelectCtx`、`AutoGroups`：临时覆盖、`url-test` 保持的成员、每组上一轮测试的时间与测试请求）；注册表按测试结果选成员（只有拨号触发测试、`evaluate-before-use` 的 `Resolution.pending`、嵌套组的分数、`round_timeout`）、`resolve_relay`（空组当中继一律拒绝）、订阅行不能动用主配置的 `client-cert` 与策略（`W0023`）；`rurge-engine` 的测试调度任务、测试会话进请求记录（规则 `policy test`）、`evaluate-before-use` 的有界等待（`policy group evaluation failed`）、重载时保留未变组的覆盖、`Engine::snapshot`（运行时与注册表在代际锁下成对读取）与 DNS 会话在链底为 REJECT 时的旁路；`rurge-api` 的 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 与 `POST /v1/policy_groups/select` 对自动组的临时覆盖；能力表翻转 `url-test` / `fallback` / `load-balance`（`W0008` 只剩 `smart` 与 `subnet`）。M3c（`smart`）已完成——`rurge-policy::smart`（`SmartBook`：按策略的时间加权首字节分数与失败罚分、健康 / 未知 / 失败三种状态、站点记忆、使用计数；`rank` 给出选中的成员与重试列表；`sample` 给大组的常规轮抽样）、注册表的 `smart` 成员过滤与 `policy-priority` 因子、`Resolution.smart`（`SmartPick`）、`resolve_member`、`test_round`，以及每代预存的测试查表（`TestBook::outcome`，M3b 延后事项 #14）；`rurge-inbound` 的 `SessionHandle` 两个时刻（出站就绪、首字节）、可挂多个的结束钩子与首字节钩子、`mark_upstream_failed`；`rurge-engine` 的 `smart` 模块（拨号时按重试列表换成员，单次时限"剩余时间 ÷ 剩余尝试次数"；质量回报：首字节、3 秒无响应、上游先断）、请求记录的 `connectMs` / `firstByteMs`、`automatic()` 纳入 `smart`、每次发布注册表时 `SmartBook::retain`；能力表翻转 `smart`（`W0008` 只剩 `subnet`）。M4（WireGuard / SSH / external）按三份计划推进（M4a SSH → M4b WireGuard → M4c external）：M4a（SSH）已完成——`rurge-config::spec::SshSpec`（`server-fingerprint` 解析为 `HostKeyPin`、`idle-timeout`、`private-key` 的 `E0020`）与 `ProtoSpec::keystore_item`（重载的指纹含 SSH 私钥）；新 crate `rurge-proto-ssh`（russh 0.63.3，`ring` 后端）：Keystore 私钥解码（Ed25519 / ECDSA / RSA；带口令与 DSA 为 `E0022`）、主机密钥比对、`SshOutbound`（每个策略一条会话、单飞握手、认证失败等持久失败后按 60 秒起翻倍至 10 分钟退避、钉住的主机密钥算法优先协商、`direct-tcpip` 通道、会话断了重建一次、按通道计的空闲断开、30 秒 × 3 的保活、没配指纹的一次性告警、RSA 只用 SHA-2 签名）与 `testing::FakeSsh`；`rurge-engine` 的工厂分支；订阅行自己的 `private-key=` 进订阅安全门（`W0023`）；`tests/interop` 的临时 `sshd`（`RURGE_TEST_SSHD`，只在 Unix）；能力表翻转 `ssh`。M4b（WireGuard）已完成——`rurge-config` 的 `[WireGuard <name>]` 类型化（`WireGuardSection`，`E0023`，重名的节 `W0020`）与 `spec::WireGuardSpec`（spec 带着节的内容；`underlying-proxy` 为 `W0029` 且策略 REJECT；订阅行自己的 `section-name=` 进订阅安全门）；`rurge-net` 的 `Datagram`、`Connector::connect_udp` 与 `DirectConnector::connect_udp`（按 `ip-version`、`set_tos`、7 MiB 收发缓冲）；新 crate `rurge-proto-wireguard`（boringtun 0.7.1 的 sans-IO `Tunn` 加 smoltcp 0.12：`routes` 最长前缀选 peer、`wire` 的 `client-id`、`stack`（锁内只做内存操作，每次推进都 poll 到发完，Reno 拥塞控制）、`device`（每条隧道一个任务、批量收、握手 TOS 0x88、5 分钟重拨与 endpoint 跟随、发送时本机地址或路由失效或握手无回应时换新载体（网络变化后自愈）、网络变化入口、同一私钥与 peer 只留一条隧道，节与载体设置都相同的策略共用）、`stream`、`dns`（隧道内 DNS 与缓存；每个问题 2 秒内重发、先等隧道第一次握手）、`outbound`（`WireGuardOutbound`：没有 `dns-server` 时在本机解析、含 `[Host]`；原生握手测速）与 `testing::FakeWgPeer`）；`rurge-proto` 的 `Outbound::native_test`；`rurge-policy` 的 `TestMode`（URL 与原生两种测速，`wireguard` 另加 10 秒）；`rurge-engine` 的工厂分支、拨号时 `Unsupported` 带说明、DNS 会话防环扩展到 endpoint 写成域名的 `wireguard`；bin 不输出 boringtun 自己的日志；`tests/interop` 对 sing-box WireGuard 端点（带保留字节）的互操作用例；能力表翻转 `wireguard`。M4c（external）已完成——`rurge-config::spec::ExternalSpec`（`args` 是 `Secret`；两个 `external` 策略写了同一个 `local-port` 是 `E0018`；`args` 进内联参数的脱敏名单）、订阅导入的 `external` 行一律跳过（`W0023`）；`rurge-platform::process`（Unix 进程组与 `killpg`，Windows Job Object）；`rurge_proto::external`（`ExternalOutbound`：第一次用到时拉起、输出写 `<数据目录>/external/<策略名>.log` 且超过 1 MiB 时在拉起前轮转、子进程环境去掉代理变量并设 `NO_PROXY=*`、程序退出后下次用到时再拉起且两次拉起至少隔 2 秒、每 500 ms 连一次本机端口最多 6 次；`ProcessHook` / `ProcessGroup` 与 `NoProcessGroups`；程序自己退出时连同它启动的进程一起结束）；`rurge-engine` 的工厂分支、`EngineShared.processes` / `externals` 与 `Engine::stop_external_programs`；bin 的 `PlatformProcesses` 与退出流程最后停掉全部外部程序；只用于测试的工作区成员 `tests/external`（`rurge-external-tests`：极小的 SOCKS5 辅助程序 `socks-helper` 与真实拉起它的用例）；能力表翻转 `external`（`W0007` 不再因 `external` 出现）。M4 至此完成。M5（UDP 路径）按三份计划推进（M5a 地基 → M5b TLS 族 → M5c WireGuard 与其余）：M5a 已完成——`rurge_net::connector::PacketSocket`（按包收发、带地址的 UDP 载体）与 `Connector::open_udp`（`DirectConnector`：每个地址族一个未连接的 socket，忽略 Windows 的 ICMP 不可达报错）；`Outbound::udp()` / `open_udp()` 与 `UdpSupport`；DIRECT 与 `socks5` / `socks5-tls` / `external` 的 UDP（`udp-relay`，`W0029` 退役）；`ChainConnector::open_udp`（链式 UDP 载体）；`rurge-inbound` 的 SOCKS5 UDP ASSOCIATE（`UdpClient`、`UdpAdmission`、`Dialer::admit_udp` / `associate`）；`rurge-engine` 的 UDP 流水线（`udp` 模块：一条流 = 关联 + 目标、按"关联 × 出站"共用载体的全锥、60 秒 / DNS 10 秒回收、1024 流 / 4096 关联的上限）、请求记录与 API 的 `transport`、`block-quic`（`W0029` 退役）与 `udp-policy-not-supported-behaviour`、QUIC Initial 识别、`PROTOCOL` 规则按传输层匹配 `TCP` / `UDP`；`FakeSocks5` 与 `tests/external` 辅助程序的 UDP ASSOCIATE；对 sing-box `socks` 入站的 UDP 互操作用例。M5b（TLS 族的 UDP）已完成——`rurge-proto` 的 `stream_udp`（一条字节流上按包收发：请求头随第一个包发出、写那个包的目标；`trojan` 的 UDP ASSOCIATE 与 `anytls` 的 UDP over TCP v2 两种封装）、`trojan` / `anytls` 的 `open_udp`（全锥）、`vmess` 的命令 2（`vmess::udp`：每个目标一条 VMess 连接、随它的第一个包建立、每个数据报一个分块，对称型）；`FakeTrojan` / `FakeAnyTls` / `FakeVmess` 的 UDP 与 `rurge_proto::testing::udp_echo_server`；经引擎的端到端用例（`tests/udp_tls_family.rs`）；对 sing-box（三种）与 xray（vmess）的 UDP 互操作用例。M5c（WireGuard 的 UDP 与其余）已完成——`rurge-proto-wireguard` 的 `TunnelUdp`（隧道里每个地址族一个 UDP socket、第一次发往该族时绑定、全锥，目标名经隧道 DNS 或本机解析）与 `Stack::udp_bind` / `check`；`rurge_net::packet_datagram`（把 `PacketSocket` 变成一条到固定目标的 `Datagram`）与 `ChainConnector::connect_udp`，`wireguard` 的载体经 `underlying-proxy`（底层策略不载 UDP 时拨号失败，`W0029` 退役）；启动时 peer 连不上的告警每个策略的每个 peer 5 分钟至多一次（M4b 延后事项 #15）；`rurge_policy::udp_probe`（经策略的 UDP 向 `hostname@ipv4` 问一次 A 记录）、`Engine::test_udp` 与 `POST /v1/policies/test` 结果里的 `udp` 键（`test-udp` / `proxy-test-udp`，不保存、不参与组的选择）；`smart` 计入 UDP（载体打不开算失败、第一个回包算首字节、3 秒无回包只在 53 / 443 端口算失败、UDP 不换成员）；`dns-follow-interface`（`rurge_net::connector::Via` / `ResolveVia`、`Resolver::lookup_via`：策略自己的解析经它的 `interface` 问普通 DNS 服务器、答案另存，配了加密 DNS 时不跟随；没有 `interface` 时 `W0028`）；对 sing-box WireGuard 端点的 UDP 互操作。M5 至此完成。M6（Shadowsocks / Snell / HTTP/2 族）按三份计划推进（M6a Shadowsocks → M6b Snell → M6c HTTP/2 族）：M6a（Shadowsocks）已完成——`rurge-config::spec` 的 `SsSpec`（`SsMethod`：AEAD 五种、`none`、SS 2022 两种；2022 的 `password` 按冒号拆成逐层的 Base64 密钥、最后一段是用户密钥，不合法是 `E0018` 且不引用取值；`udp-relay`、`udp-port`）与 `ObfsOpts`（与 Snell 共用；`obfs-host` 缺省服务器主机名、`obfs-uri` 缺省 `/`）、`NotImplemented`（取代 `legacy_vmess` 标记：vmess 旧握手与 `ss` 流式旧方法都是 `W0007` + REJECT，流式方法每种每次加载一条，会话日志 `policy protocol not implemented: ss (<method>)`）、`obfs-host` 进内联参数的脱敏名单；`rurge_proto::transport::obfs`（simple-obfs 的 `http` / `tls`，自写模板，`Stack` 的一层：connect → shadow-tls → obfs → tls → ws）；`rurge_proto::shadowsocks`（`ShadowsocksOutbound`：AEAD 分块流与 `none`、SS 2022（BLAKE3 子密钥、请求头块与填充、应答的类型 / 时间戳 / 回显 salt 校验、SIP023 多用户身份头）、UDP（`udp-relay=true` 发往 `udp-port`，全锥；2022 的分离头、按服务端 session 的防重放窗口））；新依赖只有 `blake3`；`rurge_proto::testing` 的 `FakeShadowsocks`（AEAD、2022 含身份头、UDP、两种 obfs）与 `accept_obfs`；`rurge-engine` 的工厂分支与经引擎的端到端用例（`tests/outbounds_shadowsocks.rs`）；能力表翻转 `ss`（流式旧方法除外）；测试里"未实现的协议"的例子从 `ss` 换成 `hysteria2`；`tests/interop` 对 shadowsocks-rust `ssserver`（固定版本 v1.25.0，`RURGE_TEST_SSSERVER`）与 sing-box `shadowsocks` 入站的互操作用例。
```

`CLAUDE.md`——把

```markdown
- `docs/superpowers/plans/2026-09-30-phase2-m5c-udp-wireguard-rest-plan.md`：阶段 2 / M5c（WireGuard 的 UDP 与其余）实施计划（6 个任务）。开头「计划期决定」表记录核对 smoltcp / 本仓库与手册得出的结论和与设计文字不同的决定（隧道里每个地址族一个 UDP socket、链上的 UDP 经 `packet_datagram`、底层策略不载 UDP 时拨号失败而不是 REJECT、M4b #24 已无对象、UDP 测试另走 `Engine::test_udp` 且只加 `udp` 键、`smart` 的 3 秒只在 53 / 443、`dns-follow-interface` 覆盖策略自己的全部解析但不跟随加密 DNS 等）与「承接事项」；末尾「执行期修正记录」与「延后事项」两张表。
```

换成

```markdown
- `docs/superpowers/plans/2026-09-30-phase2-m5c-udp-wireguard-rest-plan.md`：阶段 2 / M5c（WireGuard 的 UDP 与其余）实施计划（6 个任务）。开头「计划期决定」表记录核对 smoltcp / 本仓库与手册得出的结论和与设计文字不同的决定（隧道里每个地址族一个 UDP socket、链上的 UDP 经 `packet_datagram`、底层策略不载 UDP 时拨号失败而不是 REJECT、M4b #24 已无对象、UDP 测试另走 `Engine::test_udp` 且只加 `udp` 键、`smart` 的 3 秒只在 53 / 443、`dns-follow-interface` 覆盖策略自己的全部解析但不跟随加密 DNS 等）与「承接事项」；末尾「执行期修正记录」与「延后事项」两张表。
- `docs/superpowers/specs/2026-09-30-phase2-m6-ss-snell-h2-design.md`：阶段 2 / M6 细化设计（Shadowsocks / Snell / HTTP/2 族），细化总设计的 M6 里程碑、不一致处以它为准，并关闭总设计的开放问题 Q7。三份计划的拆分（M6a Shadowsocks → M6b Snell → M6c `h2-connect` / `trust-tunnel`）；已决事项 M6-D1 ～ D8（四种协议都做、Snell 只实现 v4 / v5 而 `version` 缺省为 1 的行按 v1 拒绝、GPL 参考实现只取协议事实、新依赖只有 `blake3`、各协议的互操作参考、sing-box 升到有 `snell` 入站的 1.14.x、v5 的动态帧大小只影响发送方、`h2-connect` 的 UDP 每个目标一条流）；各协议的配置、线上格式、错误与三层测试；第 7 节是需登记的差异，第 9 节 V1 ～ V10 是写各份计划时必须核对的事项，第 10 节是三份计划的任务草图。
- `docs/superpowers/plans/2026-09-30-phase2-m6a-shadowsocks-plan.md`：阶段 2 / M6a（Shadowsocks）实施计划（7 个任务）。开头「计划期决定」表记录核对参考实现、SIP022 / SIP023 原文与本仓库得出的结论和与设计文字不同的决定（加密只用 RustCrypto 且 `chacha20poly1305` 沿用依赖树里的 0.10、`LazyHead` 在加密流之上、SS 2022 与 AEAD 共用一种流且在应答头块校验、填充内容为零只有长度随机、UDP 包号从 0 开始并校验回显的客户端 session id、每个载体至多记 8 个服务端 session、obfs 的 `Host` 总带非 80 的端口与首包 16 KiB 上限、不接受 `2022-blake3-chacha20-poly1305`、测试里"未实现的协议"的例子换成 `hysteria2`、sing-box 覆盖 ssserver 发布包没有的 `aes-192-gcm` / `xchacha20-ietf-poly1305` 等）与「承接事项」；末尾「执行期修正记录」与「延后事项」两张表。
```

`CLAUDE.md`——把

```markdown
cargo test -p rurge-external-tests              # external：真实拉起测试辅助程序（socks-helper）——按需拉起、参数顺序与代理变量、再拉起与 2 秒间隔、连接被拒的重试、日志、整棵进程树的停止、经引擎的端到端、检查不拉起、重载沿用与替换
cargo test -p rurge-interop                     # 对 sing-box（全部协议）、xray（只测 vmess）与 OpenSSH sshd（只测 ssh，只在 Unix）的互操作测试；没装就跳过（RURGE_TEST_SING_BOX / RURGE_TEST_XRAY / RURGE_TEST_SSHD / RURGE_INTEROP_REQUIRED=1）
```

换成

```markdown
cargo test -p rurge-proto shadowsocks           # shadowsocks：KDF 与分块的向量、SS 2022 头与身份头、UDP 包与防重放窗口，出站对回环假服务端（FakeShadowsocks，含两种 obfs）
cargo test -p rurge-proto obfs                  # simple-obfs：http 请求头与 tls ClientHello 的布局、应答的读取，对回环假服务端的往返
cargo test -p rurge-engine --test outbounds_shadowsocks   # 经 ss 出站的端到端用例：各种方法、两种 obfs、时钟偏差、流式方法的 REJECT、经 underlying-proxy、UDP 与 udp-port、全锥、重载沿用与替换
cargo test -p rurge-external-tests              # external：真实拉起测试辅助程序（socks-helper）——按需拉起、参数顺序与代理变量、再拉起与 2 秒间隔、连接被拒的重试、日志、整棵进程树的停止、经引擎的端到端、检查不拉起、重载沿用与替换
cargo test -p rurge-interop                     # 对 sing-box（全部协议）、xray（只测 vmess）、shadowsocks-rust ssserver（只测 ss）与 OpenSSH sshd（只测 ssh，只在 Unix）的互操作测试；没装就跳过（RURGE_TEST_SING_BOX / RURGE_TEST_XRAY / RURGE_TEST_SSSERVER / RURGE_TEST_SSHD / RURGE_INTEROP_REQUIRED=1）
```

总设计（第 6 节 `snell` 一行、第 13 节 M6 的参考实现、开放问题 Q7）：

`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`——把

```markdown
| `snell` v1 ～ v4 | `rurge-proto` | M6 | TCP（+ obfs），v4 reuse | v3+ 自动、`udp-port` |
```

换成

```markdown
| `snell` v4 / v5 | `rurge-proto` | M6（v1 ～ v3：M8，需要时） | TCP（+ obfs `http`），reuse | 自动（UDP over TCP）、`udp-port` |
```

`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`——把

```markdown
| M6 | shadowsocks-rust（含 2022）、sing-box；Snell 官方二进制只有 Linux，互操作只在 Linux CI 跑 |
```

换成

```markdown
| M6 | 按 M6 细化设计 M6-D5：shadowsocks-rust v1.25.0（含 2022 与多用户，三平台）与 sing-box 的 `shadowsocks` 入站：`ss`；sing-box 的 `snell` 入站（1.14.0 起，只支持 v5 / v6）：`snell`，三平台；官方 snell-server v5.0.1 只有 Linux 版，只在 Linux CI 跑；sing-box 的 `http` 入站（HTTP/2 over TLS）：`h2-connect` 的普通 CONNECT；TrustTunnel endpoint v1.1.0 只有 Linux / macOS 版，只在这两个平台的 CI 跑；CONNECT-UDP over HTTP/2 与 obfs 没有可用的预编译参考服务端，只有回环假服务端与手工验收 |
```

`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`——把

```markdown
| Q7 | Snell 各版本可依据的公开资料 | M6 细化设计时确认 |
```

换成

```markdown
| Q7 | Snell 各版本可依据的公开资料 | 已决（2026-09-30，M6 细化设计第 4.1 节）：v4 / v5 的 TCP 线上格式依据 missuo/opensnell 与 SagerNet/sing-snell 的公开描述（均为 GPL，只取协议事实，M6-D3），sing-box 1.14.0 起的 `snell` 入站实现 v5 / v6；v1 ～ v3 另有 icpz/open-snell；v6 细节未公开，不做 |
```

- [ ] **Step 4: 核对**

- `README.md` 与 `README_en.md` 的状态一段与特性表内容一致。
- 兼容性清单里不再有说 `ss`"M6 实现"一类的前瞻文字：`grep -n "ss.*M6\b" docs/surge-compatibility-matrix.md` 只剩写明"M6a 已实现"的行。

- [ ] **Step 5: 门禁与提交**

跑门禁。副本上最后一次全工作区门禁：fmt、clippy 通过，`cargo test --workspace --no-fail-fast` 1367 通过、0 失败、2 忽略。

```bash
git add tests/interop .github docs README.md README_en.md CLAUDE.md
git commit -m "docs: M6a Shadowsocks——兼容性清单、手工验收、README 与 CLAUDE.md；shadowsocks-rust 与 sing-box 的互操作"
```

## 验收对照（设计第 1 节与第 3 节，M6a 部分）

| # | 验收项 | 由谁保证 |
| - | ------ | -------- |
| 1 | `ss` 对参考实现的 TCP 转发通过 | Task 7：`aead_methods_against_ssserver`、`ss_2022_against_ssserver_with_one_key_and_as_a_user`、`ss_against_sing_box_aead_2022_and_a_user`（CI）；Task 3 / 4 的回环往返 |
| 2 | UDP 转发通过 | Task 7 同上三条的 `udp_roundtrip`（CI）；Task 5 `udp_round_trips_with_every_method`；Task 6 `udp_goes_through_ss_aead_and_2022` |
| 3 | SS 2022 与多用户 | Task 4 `ss_2022_round_trips_with_one_key_or_as_one_of_several_users`；Task 5 的 2022 UDP；Task 7 的多用户互操作 |
| 4 | obfs `http` / `tls` | Task 2 的回环往返；Task 3 `through_both_obfs_modes`；Task 6 `both_obfs_modes_carry_the_session`；真实服务端靠手工验收（P15） |
| 5 | `udp-relay` / `udp-port` | Task 5 `no_udp_without_udp_relay`、`udp_goes_to_udp_port_when_it_is_written`；Task 6 `udp_goes_to_udp_port`、`without_udp_relay_a_flow_is_rejected` |
| 6 | 流式方法 `W0007` + REJECT | Task 1 `ss_stream_ciphers_are_reported_once_per_load_and_cipher`；Task 6 `a_stream_cipher_rejects_and_says_which`、`check_knows_ss` |
| 7 | 能力表不再为 `ss` 出 `W0007` | Task 6 `check_knows_ss` |
| 8 | 门禁全绿 | 各任务的门禁 |
| 9 | 需要真实节点的项目进手工验收清单 | Task 7：`docs/acceptance/phase2-manual.md` 的 M6a 一节 |

## 执行期修正记录

| # | 任务 | 与计划的出入 | 原因 |
| - | ---- | ------------ | ---- |
| 1 | Task 7 | 兼容性清单 4.2 的 `ss` 行多写了一句：格式不对的 `ss` 行（`encrypt-method` 缺失或不认识、缺 `password`、2022 密钥不合法）现在是加载错误 `E0018`，不再静默按 `REJECT` 处理 | 裁定：Task 1 起的用户可见变化（Task 1 评审的延后记录），清单必须记录 |
| 2 | 终审 | （#2–#7 均在 ef5701b，文档在其后的文档提交）SS 2022 的 UDP 载体：`SsUdp` 的 `packets` + `Option<Session>` 换成每个载体自己的 `Sealing`（`Plain` / `Aead` / `S2022 { keys, session }`），`Packets::S2022` 改持 `Arc<Keys2022>`；"2022 策略却没有 session"的状态不再可表示，`seal` / `unseal` 没有了会按明文收发的兜底分支 | 终审 M1：按计划的兜底 `_` 分支也匹配 `(S2022, None)`，一旦出现这种状态就会以明文收发。无效状态已不可表示，没有单独的用例，由既有的 UDP 用例覆盖 |
| 3 | 终审 | 兼容性清单 `ss` 行补上：`obfs=http` 只接受 `101` 或 2xx 的应答头（跨读取累积、至多 8 KiB），参考客户端什么都不检查、并假定应答头在第一次读到的数据里；`obfs=tls` 逐个解析服务端记录的头而不是跳过固定长度；4.6 的 `ss` 参数行补上 `obfs-host` 至多 255 字节 | 终审 M2：这两处与参考实现的差异此前没有登记 |
| 4 | 终审 | `AeadStream` 的请求 salt 改为一直保留（`salt` + `salt_sent`）：2022 的首次写在封装任何东西之前先取填充长度的随机数，失败时本次写报错、salt 仍待发出；`Edition2022` 不再另存一份 salt | 终审 M3：按计划 salt 先被取走，取随机数失败后再写就不带 salt。随机数失败无法在测试里触发，没有单独的用例 |
| 5 | 终审 | `obfs-host` 超过 255 字节在配置层是 `E0018`（`` invalid `obfs-host` (expected at most 255 bytes) ``，不引用取值）；出站层的 `BuildError` 保留为兜底（新用例 `a_host_longer_than_255_bytes_is_an_error_and_never_quoted`） | 终审 M4：此前只在构建出站时失败 |
| 6 | 终审 | 旧式 AEAD 读到的服务端 salt 与自己的请求 salt 相同（流被反射）时以 `UNDECRYPTABLE` 失败；读侧用例的请求 salt 改用与应答不同的 `0xee…`（新用例 `a_reflected_stream_does_not_decrypt`，去掉这条检查时它失败） | 终审 M5：反射的流两个方向子密钥与 nonce 序列相同，照样能解开 |
| 7 | 终审 | `outbounds_shadowsocks` 重载用例的断言文字改为 "an unrelated reload rebuilt S" | Task 6 评审的延后记录：原文字读反了 |
| 8 | 门禁 | 最终门禁（终审修正 ef5701b 之后）：fmt、clippy 通过；第一轮 `cargo test --workspace --no-fail-fast` 因磁盘满编译失败（`os error 112`），删掉 `target/debug/incremental` 后以 `CARGO_INCREMENTAL=0` 重跑：55 个测试二进制 1369 通过 / 0 失败 / 2 忽略 | 终审修正加了 #5、#6 的两个用例 |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | 流式旧方法（P5） | M8 |
| 2 | obfs 没有可用的预编译参考服务端，真实兼容性只能手工验收（P15） | 手工验收；有可用的参考服务端时补互操作 |
| 3 | 没有"口令错"的互操作用例（`ssserver` 对错误首块的行为未核对） | 需要时另议 |
| 4 | 假服务端的多用户只有一层身份密钥（两层只由已知答案覆盖）；它记住的请求 salt 不过期（规范是 60 秒） | 接受（测试设施） |
| 5 | `2022-blake3-chacha20-poly1305`（SIP022 可选，手册不列） | 手册列出时 |
| 6 | README 路线图一行停在 M4b；总设计第 1.4 节与第 2 节的 Snell 仍写 v1 ～ v4（M6 设计已取代） | M6b 的文档任务一并订正 |
| 7 | 配置层：`udp-port` 没写 `udp-relay` 时静默接受（可以是 `W0028`）；`ObfsOpts` 的 `Debug` 打印 `obfs-host`（只要求 API 与 `rurge check` 脱敏） | 以后顺手改 |
| 8 | 写路径的"同一缓冲重试"约定：obfs 层与 `AeadStream` 的 `poll_write` 在 `Pending` 之后依赖调用方以同一缓冲重试（与 `WsByteStream` / `VmessStream` 相同），其间插入 flush 可能重复数据 | 接受 |
| 9 | `AeadStream` 的细节：`Payload` 分支对零长度的 `ReadBuf` 返回 `Ok(0)`（理论上）；`seal_first_2022` 截断 `addr_len` 而不是断言 `LazyHead` 的约定；AES 单块函数的 `_ => Aes256` 分支是隐含的 | 以后顺手改 |
| 10 | 密钥材料（主密钥、子密钥、2022 的密钥）不清零 | 需要时另议 |
| 11 | SS 2022 的 UDP：服务端 session 按先进先出淘汰而不是 LRU；未知的服务端 session 先做 BLAKE3 派生再验 tag（伪造的包也花一次派生）；来源过滤只比 IP（与 socks5 相同） | 有用户报告再说 |
| 12 | 测试：`ss.rs` 用例的 `!printed.contains("97")` 脆弱；假 obfs `tls` 服务端把截断的记录头当作干净的 EOF（测试设施） | 以后顺手改 |
| 13 | 手工验收：时钟偏差一项可以一并写出客户端侧的 `ss: the server's clock differs from ours by <n> seconds` 文本；`ssserver` 互操作的配置只在 CI 上验证过（发布包的 SHA-256 取自 GitHub API 的 digest 字段） | 以后顺手改；CI 首跑时核对 |
