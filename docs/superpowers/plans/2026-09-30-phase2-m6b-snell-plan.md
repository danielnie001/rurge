# 阶段 2 / M6b「Snell v4 / v5」Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `snell` 出站的 v4 / v5：TCP（Connect / ConnectV2）、`reuse` 的连接复用池、UDP over TCP、`obfs=http`、`udp-port`；`version` 1–3 与 6 只解析（`W0007` + REJECT）。

**Architecture:** `rurge-config` 新增 `spec::snell`（`SnellSpec`），`NotImplemented` 加 `SnellVersion`。`rurge-proto` 新增 `snell` 模块：`kdf`（Argon2id）、`record`（按记录收发的 `SnellStream`：每方向的 salt、7 字节头与负载分别封、首个数据记录带交错的填充、空记录即本方向结束、复用时计数继续）、`tunnel`（请求头与应答、复用、陈旧连接的一次重试）、`pool`（空闲连接池）、`udp`（按记录收发的 UDP 载体），外加独立实现的 `FakeSnell`；`LazyHead` 泛化成 `LazyHead<S>`。引擎的工厂装上 `SnellOutbound`，能力表翻转 `snell`。互操作对 sing-box 1.14.2 的 `snell` 入站（三平台）与官方 snell-server v5.0.1（只在 Linux CI）。

**Tech Stack:** Rust 1.89 / edition 2024；**不新增 crate**：`argon2` 0.6（`default-features = false`、`alloc`，已经经 `ssh-key` 在锁文件里）、`aes-gcm` 0.11（M6a）；复用 M6a 的 `shadowsocks::cipher::CountingAead` / `AeadCipher` 与 `transport::obfs`。

**Spec:** `docs/superpowers/specs/2026-09-30-phase2-m6-ss-snell-h2-design.md`（M6-D1 ～ D8；第 4 节；第 6、7 节中 M6b 的部分；第 9 节 V4 ～ V6、V9；第 10 节 M6b 草图；第 12、13 节 M6a 的订正）与总设计。与本计划「计划期决定」表不一致处，以该表为准；执行开始时一并写进设计文档新增的第 14 节。

## Global Constraints

- MSRV **1.89**，edition 2024。`unsafe_code`：全工作区 `forbid`；只有 `rurge-platform` 的两个函数例外（本计划不碰）。**本计划不新增任何 unsafe**。
- 依赖方向不变：`rurge-proto → rurge-net → rurge-config`；`rurge-engine → { rurge-inbound → rurge-proto, rurge-policy → rurge-proto, rurge-dns }`。**不新增 crate**，也不给已有 crate 引入第二个大版本（`argon2` 只是给 `rurge-proto` 加一条依赖边）。
- **测试绝不碰公网**：只用回环 + 端口 0 + 有界等待；不用固定 sleep 当同步手段（轮询 + 截止时间；只有"断言这段时间里什么也没发生"时才等一段固定时间）。**任何带 `url-test` / `fallback` / `load-balance` / `smart` 组的测试配置，`proxy-test-url` 与 `internet-test-url` 都必须指向回环**——引擎用例的 `Profile::text` 已默认指向 `http://127.0.0.1:9/`，不要删掉。
- **测试与任何命令都不得修改本机的系统代理、注册表、网络设置，不得注册真实服务 / 计划任务**：不在 CLI 测试夹具之外运行 `rurge run --system-proxy`；不运行不带 `--dry-run` 的 `rurge service install | uninstall`。
- **不在本机下载或安装任何东西**——哪怕只是为了算一个校验和也不行（不装 sing-box、snell-server，不 `rustup target add`、不 `cargo install`）。互操作用例在本机没有二进制时按既有约定跳过。
- **口令、密钥与载荷永不外泄**：`psk`、派生出的密钥与 salt、TCP 与 UDP 的载荷不进日志、错误文本与 `Debug`（配置里用 `Secret<T>`，持有密钥的对象不实现 `Debug`）。服务端拒绝时的说明文字只保留可打印 ASCII、最多 200 字节。
- Argon2id 要算几毫秒：**只在阻塞线程上算**（`spawn_blocking`），不在运行时线程上算。
- rurge 专有的运行时选项只经命令行与环境变量提供，不扩展 Surge 配置格式（FR-CFG-17）。与 Surge 的每一处行为差异都登记进 `docs/surge-compatibility-matrix.md`（Task 6）。
- 日志、错误文本、CLI 输出、代码注释用英文；文档与提交标题用中文。注释密度、命名、惯用法与相邻代码保持一致；注释里不写评审轮次的标签。Snell 的公开实现（SagerNet/sing-snell、missuo/opensnell）是 GPL：只取协议事实，不抄代码（M6-D3）。
- 提交信息结尾两行：`Co-Authored-By: <执行者自己的署名>` 与 `Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th`。**不 push、不 merge**，不 amend。
- 每个任务结束的门禁（全部通过才算完成）：

  ```bash
  RUSTFMT="C:\Users\SZV01065\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe" cargo fmt --all --check \
    && cargo clippy --all-targets -- -D warnings \
    && timeout 1500 cargo test --workspace --no-fail-fast
  ```

  `timeout` 不能省：`rurge-dns` 的一个用例曾让测试进程以 100% CPU 空转数小时（M3a「延后事项」#20）。测试二进制异常退出而没有失败用例时（`STATUS_ACCESS_VIOLATION`、`STATUS_HEAP_CORRUPTION` / `0xc0000374`、段错误——本机已知的既有问题，M3b 计划 P21），或整轮被 `timeout` 杀掉时，重跑一次并保留两次的日志，**不要在任务里去修它**。已知偶发失败的用例（`rurge-dns` 的 `a_partial_result_completes_aaaa_in_the_background` 与 `bootstrap::tests::stale_entries_are_served_and_refreshed_once`、`rurge` 的 `run::watch_reloads_rules_on_change` 与 `run::run_system_proxy_is_applied_switched_and_restored`、`rurge-engine` 的 `udp::a_closed_port_does_not_break_the_carrier`）同样重跑。**编译器或链接器报 PDB 损坏（如 LNK1285）或 "no space on device" 时是磁盘满了**：先看 `df -h /d`，删 `target/debug/incremental`（与损坏的 `.pdb`）再重跑。
- 第三方 crate 的 API 以本机源码为准：`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`（`argon2-0.6.0`、`aes-gcm-0.11.1`）。
- 本机的 bash 处理不了超过约 8 KB 或含反斜杠的 heredoc（`\\` 会被改写）：新文件与含反斜杠的改动一律用写文件的工具落盘，不用 heredoc。

## Review Focus

设计没有逐条写到、而最可能伤到使用者的五类输入或失败方式；每一条都在负责它的任务里配了用例。

1. **`version` 没写**（Surge 缺省是 1，而 rurge 只做 4 / 5）：加载时要告警并说明缺省值，运行时 REJECT 并在请求记录里写清楚，而不是用 v4 格式去连一个 v1 服务器、表现成莫名其妙的超时。用例：Task 1 `snell_versions_are_reported_once_per_load_and_version`；Task 5 `check_knows_snell`、`snell_v1_rejects_and_says_which`。
2. **复用池里的连接已被服务端关掉**（官方服务端据报几次请求后就关连接）：下一个请求要在新连接上透明地重试一次，不能让用户看到失败。用例：Task 3 `a_pooled_connection_the_server_retired_is_retried_on_a_fresh_one`。
3. **psk 或版本不对**：连接期没有鉴权应答，要以清楚的文字失败（`snell: the server closed the connection without answering` / `snell: the server's data failed to decrypt (wrong psk or version?)`），不泄露 psk。用例：Task 2 `what_the_server_gets_wrong_is_an_error_that_quotes_nothing`；Task 3 `a_wrong_psk_is_a_connection_closed_without_an_answer`；Task 5 `a_wrong_psk_fails_the_session_closed_without_an_answer`。
4. **服务端拒绝目标**（目标连不上、名字解析不了）：拒绝的说明要作为会话的失败原因出现，且只保留可打印字符、有长度上限。用例：Task 3 `the_servers_refusal_is_the_error_and_the_connection_is_not_reused`；Task 5 `the_servers_refusal_is_the_sessions_failure`。
5. **UDP 数据报太大或回包不是数据报**：发不出去的数据报要报错而不是截断；看不懂的回包要丢掉、载体继续收。用例：Task 4 `a_datagram_longer_than_a_record_is_refused_and_nothing_is_sent`、`what_is_no_datagram_is_dropped_and_the_next_arrives`。

## 计划期决定

写计划时对照设计、Snell 的公开资料（SagerNet/sing-snell 与 missuo/opensnell 的协议描述、sing-box 1.14 的 `snell` 入站文档，2026-09-30 查阅；只取事实）与本仓库源码核对后定下的事；与设计文档文字不同的，写进设计文档第 14 节。

**本计划里的代码不是凭空写的。** 全部 6 个任务的改动在仓库的一份副本上按任务顺序真实做了一遍（副本用自己的构建目录），最后一次全工作区门禁见 Task 6 的 Step 5。计划里新文件的全文取自副本上该任务的提交，修改处的"把 … 换成 …"由脚本从相邻两个任务提交的差异生成，并在拼好之后按计划的顺序套到开工前的源码上逐字核对过。每个任务 Step 2 的"预期失败"是只把该任务的用例块（及写明的前置改动）套到上一个任务的状态上、真实跑出来的。Argon2id 与整条记录的已知答案由 Python 独立算出（`cryptography`），与公开资料里的向量一致。

| # | 事项 | 决定与依据 |
| - | ---- | ---------- |
| P1 | `version` 与 obfs 随版本 | `version` 去空格后按整数解析，1..=6，缺省 1（Surge）；其它是 `E0018`（引用原文）。4 / 5 实现；1–3 与 6 合法但未实现：`NotImplemented::SnellVersion(n)`，加载时每种版本一条 `W0007`（v1 的文字说明 `version` 缺省为 1），会话日志 `policy protocol not implemented: snell v<n>`。obfs：4 / 5 只允许 `http`（`tls` 为 `E0018`）；1–3 允许 `http` / `tls`（只解析）；6 的三个 obfs 键 `W0028`。`mode` 只对 6 校验，其它版本 `W0028`。`reuse` 缺省 false。snell 行上写 `udp-relay` 按未知参数（`W0001`）——Snell v3+ 的 UDP 是自动的 |
| P2 | 分步落地的门 | 同 M6a 的 P3：Task 1 里 `to_spec` 读完并检查 `snell` 行但暂不产出 spec，引擎工厂先放一个 `BuildError` 分支；Task 5 去掉 |
| P3 | V5：记录格式 | 每方向一个 16 字节随机 salt；密钥 = Argon2id(psk 的 UTF-8, salt, t=3, m=8 KiB, p=1) 的 32 字节取前 16 字节（AES-128-GCM）；7 字节头 `04 00 00 padlen(u16 BE) paylen(u16 BE)` 以 nonce n 封、负载以 n+1 封；nonce 是 12 字节小端计数，**跨复用继续、从不归零**（直接复用 M6a 的 `CountingAead`）。只有本方向第一个**数据**记录带 256..=511 字节随机填充（空记录永不带），填充与负载密文按"偶数下标交换"交错。空负载的记录（裸头）= 本方向结束。接收方只要求版本字节是 4，不检查保留字节，接受任意 u16 长度 |
| P4 | V6 / M6-D7：v5 的动态记录大小 | 只影响发送方：rurge 发送时每条记录固定最多 0x3FFF 字节，不做 Surge 的逐次放大（首条 894..1149、每次写 +1421、空闲 31 秒回落）；接收方照常。登记为差异 |
| P5 | 服务端密钥不阻塞运行时 | 服务端只在目标先发数据时才回应，所以不能"先读完服务端 salt 再包装"；服务端 salt 在 `poll_read` 里到达时，用 `spawn_blocking` 派生密钥，读操作轮询它的 `JoinHandle`（有用例证明不在运行时线程上算）。上行密钥在构造前算好（`SnellStream::open`） |
| P6 | V5：请求与应答 | 请求头 `01 cmd 00 hostlen host port(BE) [首段负载]`：client-id 恒为空；IP 字面量也按文本发（IPv6 不带方括号），IDN 转 A-label，发不出去的名字不拨号。**命令字**：`reuse=false` 用 Connect `0x01`，`reuse=true` 用 ConnectV2 `0x05`（公开资料有冲突：据报 Surge 总是发 `0x05`，opensnell 在不复用时发 `0x01`；登记为差异）。应答是服务端第一条记录负载的第一个字节，只在目标先发数据时才到：`00` 隧道；`02 code len msg` → `snell: the server refused: <msg>`（`ConnectionRefused`，只留可打印 ASCII、最多 200 字节）；其它 → `snell: the server answered with an unknown reply`；应答前 EOF → `snell: the server closed the connection without answering` |
| P7 | 复用 | 请求结束（双方都发过空记录）后连接回到每个出站的池（最多 8 条空闲，取最新的；空闲 60 秒回收）。回池在 `SnellTunnel` 的 `Drop` 里：只在读到 `00` 且从未出错时；对方还没结束时由后台任务发出我方的结束、读掉服务端剩下的数据（最多 0x80001 字节、10 秒），干净结束才回池。取出时先做一次非阻塞读探测，已被关掉的连接丢弃。`LazyHead` 泛化成 `LazyHead<S = BoxedStream>` 并加 `get_mut` / `into_inner`，好在复用的连接上只发空记录并交还流 |
| P8 | 陈旧连接的重试（设计没写到） | 只对从池里取出的连接：在收到应答的第一个字节之前写、flush、结束失败或读到错误 / EOF 时，在新连接上透明地重试一次，重发请求头与已接受的负载（最多 64 KiB）；应用已关闭写方向的连同结束一起重发 |
| P9 | V4：obfs=http | 复用 M6a 的 `ObfsClient`，不改：没写 `obfs-host` 时用服务器主机名（端口不是 80 时带 `:port`）。公开资料说 Surge 发 `bing.com`（不带端口），官方服务端完全不看请求头；登记为差异。Snell 的 http 伪装与 simple-obfs 的字节不完全相同（头的顺序、User-Agent），但服务端都只找 `\r\n\r\n` |
| P10 | V5：UDP over TCP | v4 / v5 自动支持（`udp()` 为 `Native`）。每个载体一条新连接（不进也不出复用池），请求 `01 06 00`，服务端立即回 `00`（同一条记录里 `00` 之后的字节是第一个数据报）。每条记录一个数据报：客户端 `01` + (`hostlen host` \| `00 04/06 ip`) + port(BE) + 负载；服务端 `04/06 ip port 负载`，其它首字节的记录丢掉并记 `debug!`（公开资料有冲突：Surge / sing-snell 丢，opensnell 报错）。一个数据报放不进一条记录（0x3FFF）时发送报 `snell: a datagram longer than N bytes`、什么也不发。连接结束即载体结束（`snell: the server closed the UDP connection`）。不沿用 `stream_udp`：Snell 的数据报没有长度，记录边界就是数据报边界，另写 `snell/udp.rs`（在 `SnellStream::poll_record` 之上） |
| P11 | `udp-port` 对 Snell 的含义（手册没写清） | 写了时是 **UDP 会话那条 TCP 连接的端口**（经同样的 Shadow TLS 与 obfs；obfs 的 `Host` 写这个端口），没写用主端口。如果 Surge 的 `udp-port` 其实是给 v5 的 QUIC 模式用的 UDP 端口，写了它的用户会连到一个多半不通的 TCP 端口——登记为差异 |
| P12 | V9：互操作 | sing-box 固定版本从 1.14.1 升到 **1.14.2**（`snell` 入站从 1.14.0 起；它的 `version: 5` 也接受 v4 客户端），三平台；官方 snell-server v5.0.1 只有 Linux 版，只在 Linux CI 按 zip 的 SHA-256 安装（`RURGE_TEST_SNELL_SERVER`，`RURGE_INTEROP_REQUIRED=1` 对它只在 Linux 上生效）。互操作只能证明复用连接上的请求互通，数不了连接数（连接数由 `FakeSnell` 与引擎用例覆盖）。snell-server 是否默认开 UDP 资料未确认，由 CI 首跑证明 |
| P13 | 任务的切分 | 设计第 10 节草图的 5 个任务拆成 6 个：1 配置；2 KDF 与记录；3 TCP、复用池与 `FakeSnell`；4 UDP；5 引擎装配；6 互操作与文档 |

## 承接事项

| # | 来源 | 事项 | 处理 | 任务 |
| - | ---- | ---- | ---- | ---- |
| C1 | M6 设计第 10 节 | M6b 的全部内容 | 本计划 | 1–6 |
| C2 | M6a 延后事项 #6 | README 路线图停在 M4b；总设计第 1.4 节与第 2 节的 Snell 仍写 v1 ～ v4 | 本计划订正 | 6 |
| C3 | M6-D6 | sing-box 升到有 `snell` 入站的 1.14.x | 1.14.2（P12） | 6 |
| C4 | M3b #7（P21） | 测试二进制偶发崩溃 | 照旧：门禁遇到就重跑 | — |

## File Structure

新建：

| 文件 | 职责 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-config/src/spec/snell.rs` | `SnellVersion`、`SnellSpec`、`read_snell`（与用例） | 1 |
| `crates/rurge-proto/src/snell/{mod.rs, kdf.rs, record.rs}` | 模块、Argon2id、`SnellStream`（与用例） | 2 |
| `crates/rurge-proto/src/snell/{pool.rs, tunnel.rs}` | 空闲连接池；请求头、应答、复用与陈旧连接的重试 | 3 |
| `crates/rurge-proto/src/testing/snell.rs` | `FakeSnell`（TCP、复用、obfs，Task 4 加 UDP） | 3、4 |
| `crates/rurge-proto/src/snell/udp.rs` | UDP 载体 `SnellUdp` | 4 |
| `crates/rurge-engine/tests/outbounds_snell.rs` | 经引擎的端到端用例 | 5 |
| `tests/interop/src/snell_server.rs`、`tests/interop/tests/snell.rs` | snell-server 夹具与互操作用例 | 6 |

修改：

| 文件 | 改动 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-config/src/{spec/mod.rs, config.rs, redact.rs}` | `ProtoSpec::Snell`、`NotImplemented::SnellVersion`、用例 | 1、5 |
| `crates/rurge-policy/src/{assemble.rs, registry.rs}` | 用例 | 1 |
| `Cargo.toml`、`crates/rurge-proto/{Cargo.toml, src/lib.rs, src/transport/lazy_head.rs, src/testing/mod.rs}` | `argon2`、模块、`LazyHead<S>` | 2、3 |
| `crates/rurge-engine/src/outbounds.rs`、`crates/rurge/src/capabilities.rs`、`crates/rurge-engine/tests/common/mod.rs`、`crates/rurge/tests/cli.rs` | 工厂分支、能力表、用例 | 1、5 |
| `tests/interop/{src/lib.rs, README.md}`、`.github/workflows/ci.yml` | sing-box 1.14.2 与 `snell` 入站、snell-server 的安装 | 6 |
| 文档（兼容性清单、手工验收、两份 README、`CLAUDE.md`、总设计） | 见 Task 6 | 6 |

## 任务一览

| 任务 | 交付物 | 依赖 |
| ---- | ------ | ---- |
| 1 | `SnellSpec`、`NotImplemented::SnellVersion` | — |
| 2 | Argon2id 与 `SnellStream` | — |
| 3 | `SnellOutbound` 的 TCP、复用池、陈旧连接重试、`FakeSnell` | 1、2 |
| 4 | UDP over TCP | 3 |
| 5 | 引擎装配、能力表翻转、经引擎的用例 | 1–4 |
| 6 | 互操作（sing-box 1.14.2、snell-server）与文档 | 1–5 |

---

### Task 1: `snell` 的配置

`SnellSpec` 与版本、obfs、`mode` 的规则（P1）；`NotImplemented` 加 `SnellVersion`；`to_spec` 读完并检查 `snell` 行但暂不产出 spec，引擎工厂先放一个到不了的 `BuildError` 分支（P2）。

**Files:**
- Create: `crates/rurge-config/src/spec/snell.rs`（自带用例）
- Modify: `crates/rurge-config/src/spec/mod.rs`（与用例）、`src/config.rs`（用例）、`src/redact.rs`（用例）、`crates/rurge-policy/src/assemble.rs`（用例）、`src/registry.rs`（用例）、`crates/rurge-engine/src/outbounds.rs`

**Interfaces:**
- Consumes: M6a 的 `read_obfs` / `ObfsMode` / `ObfsOpts`、`NotImplemented`、`Secret<T>`、`ParamReader`。
- Produces: `rurge_config::spec::snell`：`pub enum SnellVersion { V4, V5 }`、`pub struct SnellSpec { pub version: SnellVersion, pub psk: Secret<String>, pub reuse: bool, pub udp_port: Option<u16>, pub obfs: Option<ObfsOpts> }`、`pub struct SnellRead { pub spec: SnellSpec, pub not_implemented_version: Option<u8> }`、`pub fn read_snell(r: &mut ParamReader<'_>) -> SnellRead`；`ProtoSpec::Snell(SnellSpec)`；`NotImplemented::SnellVersion(u8)`

- [ ] **Step 1: 先写用例**

`crates/rurge-config/src/spec/mod.rs`——把

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

    /// A `snell` line is read and checked in full; it has no spec until the
    /// engine builds `snell` (M6b task 5), and a version other than 4 and 5
    /// says why (M6-D2).
    #[test]
    fn a_snell_line_is_checked_and_other_versions_are_not_implemented() {
        let o = outcome(
            "N",
            "snell, h.test, 443, psk=pw, version=5, reuse=true, udp-port=8443, obfs=http, obfs-host=cdn.test, shadow-tls-password=st, shadow-tls-version=3, shadow-tls-sni=site.test",
        );
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        assert!(o.inert.is_empty(), "{:?}", o.inert);
        assert_eq!(o.not_implemented, None);
        assert!(o.spec.is_none());

        // without `version`: Surge's default, 1
        let o = outcome("N", "snell, h.test, 443, psk=pw");
        assert!(o.spec.is_none() && o.diagnostics.is_empty());
        let why = o.not_implemented.expect("version 1");
        assert_eq!(why, NotImplemented::SnellVersion(1));
        assert_eq!(
            why.warning(),
            "`snell` version 1 (the default when `version` is not written) is not implemented; rurge supports versions 4 and 5 (`version` must match the server); such policies behave as REJECT"
        );
        assert_eq!(why.note(), "snell v1");
        assert_eq!(why.imported(), "`snell` version 1");
        let o = outcome("N", "snell, h.test, 443, psk=pw, version=6");
        let why = o.not_implemented.expect("version 6");
        assert_eq!(
            why.warning(),
            "`snell` version 6 is not implemented; rurge supports versions 4 and 5 (`version` must match the server); such policies behave as REJECT"
        );
        assert_eq!(why.note(), "snell v6");

        // a broken line is an error like any other, and not "not implemented"
        let o = outcome("N", "snell, h.test, 443, version=2");
        assert!(o.not_implemented.is_none());
        assert_eq!(o.diagnostics[0].code, codes::E_INVALID_POLICY_PARAM);
        // no TLS under `snell`; the PSK is never quoted
        let o = outcome(
            "N",
            "snell, h.test, 443, psk=s3cretPsk, version=4, sni=edge.test, mystery=1",
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
                    "policy `N`: `sni` does not apply to `snell` policies; ignored"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `N`: unknown parameter `mystery` ignored"
                ),
            ]
        );
        assert_eq!(
            ProtoSpec::Snell(SnellSpec {
                version: SnellVersion::V4,
                psk: "pw".into(),
                reuse: false,
                udp_port: None,
                obfs: None,
            })
            .keystore_item(),
            None
        );
    }
}

```

`crates/rurge-config/src/config.rs`——把

```rust
        assert_eq!(loaded.config.not_implemented.get("D"), None);
    }

    /// Two `external` policies on one `local-port`: the second is an error
```

换成

```rust
        assert_eq!(loaded.config.not_implemented.get("D"), None);
    }

    /// A `snell` version other than 4 and 5: `W0007` once per load and
    /// version, at the first line that uses it; a line without `version` is
    /// version 1 (phase 2 M6 design 4.2).
    #[test]
    fn snell_versions_are_reported_once_per_load_and_version() {
        let loaded = load_text(
            "[Proxy]\nA = snell, a.test, 443, psk=pw\nB = snell, b.test, 443, psk=pw, version=6\n\
C = snell, c.test, 443, psk=pw, version=1\nD = snell, d.test, 443, psk=pw, version=5\n\
E = snell, e.test, 443, psk=pw, version=3\n[Rule]\nFINAL,DIRECT\n",
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
                    "`snell` version 1 (the default when `version` is not written) is not implemented; rurge supports versions 4 and 5 (`version` must match the server); such policies behave as REJECT",
                    Some(2)
                ),
                (
                    "`snell` version 6 is not implemented; rurge supports versions 4 and 5 (`version` must match the server); such policies behave as REJECT",
                    Some(3)
                ),
                (
                    "`snell` version 3 is not implemented; rurge supports versions 4 and 5 (`version` must match the server); such policies behave as REJECT",
                    Some(6)
                ),
            ]
        );
        assert_eq!(
            loaded.config.not_implemented.get("C"),
            Some(&NotImplemented::SnellVersion(1))
        );
        assert_eq!(loaded.config.not_implemented.get("D"), None);
    }

    /// Two `external` policies on one `local-port`: the second is an error
```

`crates/rurge-config/src/redact.rs`——把

```rust
            "ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=***, obfs=http, obfs-host=***, obfs-uri=/x"
        );
    }
```

换成

```rust
            "ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=***, obfs=http, obfs-host=***, obfs-uri=/x"
        );
        assert_eq!(
            redact_definition(
                "snell, 1.2.3.4, 8000, psk=s3cretPsk, version=5, obfs=http, obfs-host=my.cdn.test"
            ),
            "snell, 1.2.3.4, 8000, psk=***, version=5, obfs=http, obfs-host=***"
        );
    }
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
Rc = ss, r.test, 8388, encrypt-method=rc4, password=pw\n\
```

换成

```rust
Rc = ss, r.test, 8388, encrypt-method=rc4, password=pw\n\
Sn = snell, n.test, 443, psk=pw\n\
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
        assert_eq!(members(&a, "G"), ["Hy", "Rc", "Old", "Good"]);
        let specs: Vec<bool> = a.imported.iter().map(|i| i.spec.is_some()).collect();
        assert_eq!(specs, [false, false, false, true]);
```

换成

```rust
        assert_eq!(members(&a, "G"), ["Hy", "Rc", "Sn", "Old", "Good"]);
        let specs: Vec<bool> = a.imported.iter().map(|i| i.spec.is_some()).collect();
        assert_eq!(specs, [false, false, false, false, true]);
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
                Some(NotImplemented::SsStreamCipher("rc4")),
```

换成

```rust
                Some(NotImplemented::SsStreamCipher("rc4")),
                Some(NotImplemented::SnellVersion(1)),
```

`crates/rurge-policy/src/assemble.rs`——把

```rust
                    "policy group `G`: imported policies of type `ss` with the stream cipher `rc4` are not implemented in this version; they behave as REJECT".to_string()
```

换成

```rust
                    "policy group `G`: imported policies of type `ss` with the stream cipher `rc4` are not implemented in this version; they behave as REJECT".to_string()
                ),
                (
                    codes::W_PROTOCOL_NOT_IMPLEMENTED,
                    "policy group `G`: imported policies of type `snell` version 1 are not implemented in this version; they behave as REJECT".to_string()
```

`crates/rurge-policy/src/registry.rs`——把

```rust
Stream = ss, 1.2.3.4, 8388, encrypt-method=rc4-md5, password=x\n[Rule]\nFINAL,DIRECT\n";
```

换成

```rust
Stream = ss, 1.2.3.4, 8388, encrypt-method=rc4-md5, password=x\n\
Snell1 = snell, 1.2.3.4, 443, psk=x\n[Rule]\nFINAL,DIRECT\n";
```

`crates/rurge-policy/src/registry.rs`——把

```rust
            "policy protocol not implemented: ss (rc4-md5)"
```

换成

```rust
            "policy protocol not implemented: ss (rc4-md5)"
        );
        // a `snell` line without `version` is version 1
        let snell = registry.resolve(&PolicyRef::parse("Snell1"));
        assert_eq!(snell.terminal, TerminalKind::Reject);
        assert_eq!(chain(&snell), ["Snell1", "!unsupported:snell", "REJECT"]);
        assert_eq!(
            snell.note.clone().unwrap().to_string(),
            "policy protocol not implemented: snell v1"
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-config --lib`
Expected: FAIL——`SnellSpec`、`ProtoSpec::Snell` 与 `NotImplemented::SnellVersion` 由 Step 3 引入，编译不过（节选）：

```text
error[E0422]: cannot find struct, variant or union type `SnellSpec` in this scope
   --> crates\rurge-config\src\spec\mod.rs:922:30
error[E0599]: no variant or associated item named `SnellVersion` found for enum `spec::NotImplemented` in the current scope
    --> crates\rurge-config\src\config.rs:1727:35
error[E0599]: no variant or associated item named `SnellVersion` found for enum `spec::NotImplemented` in the current scope
   --> crates\rurge-config\src\spec\mod.rs:879:41
error[E0599]: no variant or associated item named `Snell` found for enum `spec::ProtoSpec` in the current scope
   --> crates\rurge-config\src\spec\mod.rs:922:24
error[E0433]: failed to resolve: use of undeclared type `SnellVersion`
   --> crates\rurge-config\src\spec\mod.rs:923:26
Some errors have detailed explanations: E0422, E0433, E0599.
For more information about an error, try `rustc --explain E0422`.
error: could not compile `rurge-config` (lib test) due to 5 previous errors
exit 101
```

- [ ] **Step 3: 实现**

新模块（自带用例）：

新建 `crates/rurge-config/src/spec/snell.rs`：

```rust
//! `snell` policy parameters (manual: Policies › Snell; phase 2 M6 design
//! 4.2).

use super::obfs::{ObfsMode, ObfsOpts, read_obfs};
use super::reader::ParamReader;
use super::secret::Secret;
use crate::diagnostic::codes;

/// The versions this version implements: v5 speaks the v4 wire format over
/// TCP (M6-D2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnellVersion {
    V4,
    V5,
}

impl SnellVersion {
    pub fn number(self) -> u8 {
        match self {
            SnellVersion::V4 => 4,
            SnellVersion::V5 => 5,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnellSpec {
    pub version: SnellVersion,
    pub psk: Secret<String>,
    /// `reuse`: a finished stream hands its connection back for the next
    /// request (ConnectV2).
    pub reuse: bool,
    /// `udp-port`: where UDP goes; `None`: the policy's port.
    pub udp_port: Option<u16>,
    /// `obfs`: only `http` on versions 4 and 5.
    pub obfs: Option<ObfsOpts>,
}

/// What a `snell` line says. With a version that is valid but not
/// implemented (1–3, 6) `not_implemented_version` holds it and `spec` is
/// meaningless: the caller makes no spec of it (M6-D2).
pub struct SnellRead {
    pub spec: SnellSpec,
    pub not_implemented_version: Option<u8>,
}

/// Surge's default when `version` is not written.
const DEFAULT_VERSION: u8 = 1;

/// `mode` on version 6.
const MODES: [(&str, ()); 3] = [("default", ()), ("unshaped", ()), ("unsafe-raw", ())];

const OBFS_KEYS: [&str; 3] = ["obfs", "obfs-host", "obfs-uri"];

/// Everything `snell`-specific on the line. After an error was reported the
/// returned value is meaningless: the caller checks `r.has_errors()`.
///
/// The PSK is named-only (`psk=`), as the manual writes it, and is never
/// quoted in a diagnostic; the version may be, it is no secret.
pub fn read_snell(r: &mut ParamReader<'_>) -> SnellRead {
    let psk = r.str("psk").unwrap_or_default();
    if psk.is_empty() {
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            "`psk` is required".to_string(),
        );
    }
    let number = match r.str("version").map(str::trim) {
        None => Some(DEFAULT_VERSION),
        Some(v) => match v.parse::<u8>() {
            Ok(n @ 1..=6) => Some(n),
            _ => {
                r.invalid("version", v, "an integer from 1 to 6");
                None
            }
        },
    };
    let version = match number {
        Some(5) => SnellVersion::V5,
        _ => SnellVersion::V4,
    };
    if number == Some(6) {
        r.choice("mode", &MODES);
    } else if r.has("mode") {
        r.touch("mode");
        r.warn(
            codes::W_PARAM_NOT_APPLICABLE,
            "`mode` only applies to `snell` version 6; ignored".to_string(),
        );
    }
    let reuse = r.bool("reuse").unwrap_or(false);
    let udp_port = match r.number::<u16>("udp-port", "a port from 1 to 65535") {
        Some(0) => {
            r.invalid("udp-port", "0", "a port from 1 to 65535");
            None
        }
        port => port,
    };
    let obfs = match number {
        Some(6) => {
            for key in OBFS_KEYS {
                if r.has(key) {
                    r.touch(key);
                    r.warn(
                        codes::W_PARAM_NOT_APPLICABLE,
                        format!("`{key}` does not apply to `snell` version 6; ignored"),
                    );
                }
            }
            None
        }
        Some(4 | 5) => read_obfs(r, &[ObfsMode::Http]),
        // versions 1 to 3 had `tls` too (and a bad version is an error
        // already: no second word on the mode)
        _ => read_obfs(r, &[ObfsMode::Http, ObfsMode::Tls]),
    };
    SnellRead {
        spec: SnellSpec {
            version,
            psk: psk.into(),
            reuse,
            udp_port,
            obfs,
        },
        not_implemented_version: number.filter(|n| !matches!(n, 4 | 5)),
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

    fn read(def: &str) -> (SnellRead, bool, Vec<Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        let got = read_snell(&mut r);
        let failed = r.has_errors();
        (got, failed, r.finish())
    }

    fn messages(diags: &[Diagnostic]) -> Vec<(&str, &str)> {
        diags.iter().map(|d| (d.code, d.message.as_str())).collect()
    }

    #[test]
    fn the_manuals_example() {
        let (got, failed, diags) = read("snell, 1.2.3.4, 8000, psk=xxx, version=4, obfs=http");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(got.not_implemented_version, None);
        let spec = got.spec;
        assert_eq!(spec.version, SnellVersion::V4);
        assert_eq!(spec.psk.expose(), "xxx");
        assert!(!spec.reuse);
        assert_eq!(spec.udp_port, None);
        let obfs = spec.obfs.expect("obfs");
        assert_eq!(
            (obfs.mode, obfs.host, obfs.uri.as_str()),
            (ObfsMode::Http, None, "/")
        );
    }

    #[test]
    fn version_5_with_every_parameter() {
        let (got, failed, diags) = read(
            "snell, h.test, 443, psk=pw, version=5, reuse=true, udp-port=8443, obfs=HTTP, obfs-host=cdn.test, obfs-uri=/x",
        );
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(got.not_implemented_version, None);
        let spec = got.spec;
        assert_eq!((spec.version, spec.version.number()), (SnellVersion::V5, 5));
        assert!(spec.reuse);
        assert_eq!(spec.udp_port, Some(8443));
        let obfs = spec.obfs.expect("obfs");
        assert_eq!(
            (obfs.mode, obfs.host.as_deref(), obfs.uri.as_str()),
            (ObfsMode::Http, Some("cdn.test"), "/x")
        );
    }

    /// Surge's default is 1, so a line without `version` is valid but not
    /// implemented; so are 2, 3 and 6 (M6-D2).
    #[test]
    fn versions_1_to_3_and_6_are_valid_but_not_implemented() {
        let (got, failed, diags) = read("snell, h.test, 443, psk=pw");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(got.not_implemented_version, Some(1));
        for n in [1, 2, 3, 6] {
            let (got, failed, diags) = read(&format!("snell, h.test, 443, psk=pw, version={n}"));
            assert!(!failed && diags.is_empty(), "{n}: {diags:?}");
            assert_eq!(got.not_implemented_version, Some(n));
        }
    }

    #[test]
    fn a_version_out_of_range_is_an_error_that_quotes_it() {
        for bad in ["0", "07", "300", "4.0", "v4", "-1"] {
            let (got, failed, diags) = read(&format!("snell, h.test, 443, psk=pw, version={bad}"));
            assert!(failed, "{bad}");
            assert_eq!(
                messages(&diags),
                [(
                    codes::E_INVALID_POLICY_PARAM,
                    format!(
                        "policy `P`: invalid value `{bad}` for `version` (expected an integer from 1 to 6)"
                    )
                    .as_str()
                )]
            );
            assert_eq!(got.not_implemented_version, None, "{bad}");
        }
    }

    #[test]
    fn the_psk_is_required_and_never_quoted() {
        let (_, failed, diags) = read("snell, h.test, 443, version=4");
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: `psk` is required"
            )]
        );
        let (_, failed, diags) = read("snell, h.test, 443, psk=, version=4, hunter2");
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: `psk` is required"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `P`: unexpected positional value #1 ignored"
                )
            ]
        );
        assert!(diags.iter().all(|d| !d.message.contains("hunter2")));
    }

    #[test]
    fn obfs_tls_is_an_error_on_versions_4_and_5() {
        for version in [4, 5] {
            let (_, failed, diags) = read(&format!(
                "snell, h.test, 443, psk=pw, version={version}, obfs=tls, obfs-host=cdn.test"
            ));
            assert!(failed, "{version}");
            assert_eq!(
                messages(&diags),
                [(
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: invalid value `tls` for `obfs` (expected http)"
                )]
            );
        }
        // versions 1 to 3 had it: the line is valid, just not implemented
        let (got, failed, diags) = read("snell, h.test, 443, psk=pw, version=3, obfs=tls");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(got.not_implemented_version, Some(3));
    }

    #[test]
    fn obfs_on_version_6_and_mode_elsewhere_are_not_applicable() {
        let (_, failed, diags) = read(
            "snell, h.test, 443, psk=pw, version=6, mode=unshaped, obfs=http, obfs-host=cdn.test",
        );
        assert!(!failed);
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `obfs` does not apply to `snell` version 6; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `obfs-host` does not apply to `snell` version 6; ignored"
                ),
            ]
        );
        for version in ["version=4", "version=5", "version=2"] {
            let (got, failed, diags) = read(&format!(
                "snell, h.test, 443, psk=pw, {version}, mode=default"
            ));
            assert!(!failed, "{version}");
            assert_eq!(
                messages(&diags),
                [(
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `mode` only applies to `snell` version 6; ignored"
                )]
            );
            if version != "version=2" {
                assert_eq!(got.not_implemented_version, None);
            }
        }
        // a mode version 6 does not know is wrong on it
        let (_, failed, diags) = read("snell, h.test, 443, psk=pw, version=6, mode=fast");
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: invalid value `fast` for `mode` (expected default / unshaped / unsafe-raw)"
            )]
        );
    }

    #[test]
    fn udp_port_and_reuse() {
        for bad in ["0", "65536", "port"] {
            let (_, failed, diags) = read(&format!(
                "snell, h.test, 443, psk=pw, version=4, udp-port={bad}"
            ));
            assert!(failed, "{bad}");
            assert_eq!(
                messages(&diags),
                [(
                    codes::E_INVALID_POLICY_PARAM,
                    format!(
                        "policy `P`: invalid value `{bad}` for `udp-port` (expected a port from 1 to 65535)"
                    )
                    .as_str()
                )]
            );
        }
        let (_, failed, _) = read("snell, h.test, 443, psk=pw, version=4, reuse=maybe");
        assert!(failed);
    }

    #[test]
    fn the_spec_does_not_print_the_psk() {
        let (got, _, _) = read("snell, h.test, 443, psk=s3cretPsk, version=5");
        let printed = format!("{:?}", got.spec);
        assert!(printed.contains("Secret(***)"), "{printed}");
        assert!(!printed.contains("s3cretPsk"), "{printed}");
    }
}
```

`spec/mod.rs`：模块、`ProtoSpec::Snell`、`NotImplemented::SnellVersion`、`to_spec` 的 `snell` 分支（暂不产出 spec）：

`crates/rurge-config/src/spec/mod.rs`——把

```rust
pub mod shadow_tls;
```

换成

```rust
pub mod shadow_tls;
pub mod snell;
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
pub use shadow_tls::{ShadowTlsOpts, ShadowTlsVersion};
```

换成

```rust
pub use shadow_tls::{ShadowTlsOpts, ShadowTlsVersion};
pub use snell::{SnellSpec, SnellVersion};
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    Ss(SsSpec),
```

换成

```rust
    Ss(SsSpec),
    Snell(SnellSpec),
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            | ProtoSpec::Ss(_) => None,
```

换成

```rust
            | ProtoSpec::Ss(_)
            | ProtoSpec::Snell(_) => None,
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    SsStreamCipher(&'static str),
```

换成

```rust
    SsStreamCipher(&'static str),
    /// A `snell` version other than 4 and 5 (phase 2 M6 design 4.2).
    SnellVersion(u8),
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            ),
        }
```

换成

```rust
            ),
            NotImplemented::SnellVersion(n) => {
                // Surge's default: many lines never say which version they are
                let default = if *n == 1 {
                    " (the default when `version` is not written)"
                } else {
                    ""
                };
                format!(
                    "`snell` version {n}{default} is not implemented; rurge supports versions 4 and 5 (`version` must match the server); such policies behave as REJECT"
                )
            }
        }
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            NotImplemented::SsStreamCipher(method) => format!("ss ({method})"),
```

换成

```rust
            NotImplemented::SsStreamCipher(method) => format!("ss ({method})"),
            NotImplemented::SnellVersion(n) => format!("snell v{n}"),
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
                format!("`ss` with the stream cipher `{method}`")
            }
        }
```

换成

```rust
                format!("`ss` with the stream cipher `{method}`")
            }
            NotImplemented::SnellVersion(n) => format!("`snell` version {n}"),
        }
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            (common, ProtoSpec::Ss(read.spec))
        }
        PolicyKind::AnyTls => {
```

换成

```rust
            (common, ProtoSpec::Ss(read.spec))
        }
        PolicyKind::Snell => {
            let common = read_common(&mut r, Applies::Proxy, &mut notes);
            tls::refuse_tls(&mut r);
            let read = snell::read_snell(&mut r);
            not_implemented = read
                .not_implemented_version
                .map(NotImplemented::SnellVersion);
            (common, ProtoSpec::Snell(read.spec))
        }
        PolicyKind::AnyTls => {
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    let spec = (!failed && not_implemented.is_none()).then(|| PolicySpec {
```

换成

```rust
    // the engine builds `snell` from M6b task 5 on: until then a valid line
    // is checked in full but has no spec
    let built = policy.kind != PolicyKind::Snell;
    let spec = (!failed && not_implemented.is_none() && built).then(|| PolicySpec {
```

引擎工厂的占位分支（Task 5 换成真正的出站）：

`crates/rurge-engine/src/outbounds.rs`——把

```rust
            )?),
            // nothing starts here: the program starts on the first dial
```

换成

```rust
            )?),
            // the loader makes no spec of a `snell` line before M6b task 5
            ProtoSpec::Snell(_) => {
                return Err(BuildError::new(format!(
                    "policy `{}`: `snell` is not implemented yet",
                    spec.name
                )));
            }
            // nothing starts here: the program starts on the first dial
```

要点：
- `psk` 只接受命名写法、必填，错误文字不引用它；`version` 不对时可以引用原文（不是秘密）。
- 版本的 `W0007` 每次加载每种版本一条；在 `snell` 进能力表（Task 5）之前，通用的 "policy type `snell` is not implemented" 也会出现。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-config` → 通过（`spec::snell` 的用例、`snell_versions_are_reported_once_per_load_and_version`、脱敏用例；语料库快照不变）。
Run: `cargo test -p rurge-policy` → 通过（订阅导入与注册表的 `snell v<n>` 说明）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-config crates/rurge-policy crates/rurge-engine/src/outbounds.rs
git commit -m "feat(config): snell 的配置（SnellSpec）；只实现 v4 / v5，其余版本只解析并说明"
```

### Task 2: Argon2id 与记录流

`rurge_proto::snell` 的 `kdf`（P3）与 `record`：按记录收发的 `SnellStream`（每方向的 salt、头与负载分别封、首个数据记录的交错填充、空记录即本方向结束、复用时计数继续、服务端密钥在阻塞线程上派生——P3、P4、P5）。本任务还没有出站，`pub mod snell` 暂时带 `#[allow(dead_code)]`（Task 3 去掉）。

本任务的用例都在新模块里（已知答案、任意切片的往返、两段隧道共用一条流、篡改与沉默），与实现一同写出，没有单独的失败步骤。

**Files:**
- Create: `crates/rurge-proto/src/snell/mod.rs`、`kdf.rs`、`record.rs`（均自带用例）
- Modify: `Cargo.toml`（工作区依赖）、`crates/rurge-proto/Cargo.toml`、`src/lib.rs`

**Interfaces:**
- Consumes: M6a 的 `shadowsocks::cipher::{AeadCipher, AeadKind, CountingAead}`。
- Produces:
  - `kdf.rs`：`derive_key(psk: &[u8], salt: &[u8; 16]) -> [u8; 16]`、`Psk`（`key(salt)`、`spawn_key(salt)`）
  - `record.rs`：`pub(crate) const MAX_PAYLOAD: usize = 0x3FFF`、`new_salt()`、`first_padding()`、`pub(crate) struct SnellStream`（`new` / `open`、`AsyncRead` / `AsyncWrite`、`poll_end`（发空记录并 flush，不关连接）、`read_ended()`、`is_reusable()`、`next_tunnel()`（同一 salt、密钥与计数，不再发 salt 与填充）；`poll_shutdown` = `poll_end` 再关底层连接）

- [ ] **Step 1: 依赖与新模块**

`argon2` 已经经 `ssh-key` 在锁文件里（同版本同特性），这里只给 `rurge-proto` 加一条依赖边：

`Cargo.toml`——把

```toml
crc32fast = "1"
```

换成

```toml
crc32fast = "1"
# Snell v4 / v5: the per-direction key is Argon2id of the PSK (already in the
# tree through ssh-key, with the same features)
argon2 = { version = "0.6", default-features = false, features = ["alloc"] }
```

`crates/rurge-proto/Cargo.toml`——把

```toml
crc32fast.workspace = true
```

换成

```toml
crc32fast.workspace = true
argon2.workspace = true
```

`crates/rurge-proto/src/lib.rs`——把

```rust
pub mod shadowsocks;
```

换成

```rust
pub mod shadowsocks;
// the outbound (M6b task 3) is its first user
#[allow(dead_code)]
pub mod snell;
```

新建 `crates/rurge-proto/src/snell/mod.rs`：

```rust
//! `snell` outbound, versions 4 and 5 (manual: Policies › Snell; phase 2 M6
//! design 4): the record stream (`record`) keyed per direction by Argon2id
//! of the PSK (`kdf`).

pub(crate) mod kdf;
pub(crate) mod record;
```

新建 `crates/rurge-proto/src/snell/kdf.rs`：

```rust
//! Snell v4 / v5 key derivation (phase 2 M6 design 4.3): each direction of a
//! connection starts with its own 16-byte salt, and its AES-128-GCM key is
//! the first half of Argon2id(PSK, salt, t = 3, m = 8 KiB, p = 1) — the PSK
//! as the UTF-8 bytes it is written with, 32 bytes of output.
//!
//! One derivation per direction per connection: `spawn_key` runs it on a
//! blocking thread, off the runtime.

use argon2::{Algorithm, Argon2, Params, Version};
use std::io;
use std::sync::Arc;
use tokio::task::JoinHandle;

pub(crate) const SALT_LEN: usize = 16;
pub(crate) const KEY_LEN: usize = 16;

/// Argon2id's output; only its first `KEY_LEN` bytes are used.
const OUTPUT_LEN: usize = 32;
/// Memory in KiB, iterations, lanes.
const M_COST: u32 = 8;
const T_COST: u32 = 3;
const P_COST: u32 = 1;

/// The AES-128-GCM key of the direction that starts with `salt`.
pub(crate) fn derive_key(psk: &[u8], salt: &[u8; SALT_LEN]) -> [u8; KEY_LEN] {
    let params = Params::new(M_COST, T_COST, P_COST, Some(OUTPUT_LEN))
        .expect("Snell's parameters are valid");
    let mut out = [0u8; OUTPUT_LEN];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(psk, salt, &mut out)
        // fails only for a salt shorter than 8 bytes or a password of 4 GiB
        .expect("a 16-byte salt");
    let mut key = [0u8; KEY_LEN];
    key.copy_from_slice(&out[..KEY_LEN]);
    key
}

/// A policy's PSK, shared by its connections. No `Debug`.
#[derive(Clone)]
pub(crate) struct Psk(Arc<[u8]>);

impl Psk {
    pub(crate) fn new(psk: &str) -> Psk {
        Psk(Arc::from(psk.as_bytes()))
    }

    /// Derives the key of `salt` on a blocking thread. Needs a Tokio runtime.
    pub(crate) fn spawn_key(&self, salt: [u8; SALT_LEN]) -> JoinHandle<[u8; KEY_LEN]> {
        let psk = self.0.clone();
        tokio::task::spawn_blocking(move || derive_key(&psk, &salt))
    }

    /// `spawn_key`, awaited.
    pub(crate) async fn key(&self, salt: [u8; SALT_LEN]) -> io::Result<[u8; KEY_LEN]> {
        self.spawn_key(salt).await.map_err(key_failed)
    }
}

/// The blocking task panicked or the runtime is shutting down.
pub(crate) fn key_failed(_: tokio::task::JoinError) -> io::Error {
    io::Error::other("snell: deriving the key failed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vmess::vectors::hex;

    /// Computed with Python `cryptography`'s `Argon2id(length=32,
    /// iterations=3, lanes=1, memory_cost=8)` (scratch script of the M6b
    /// plan); the first matches the vector the byte-level notes cross-check
    /// against Go's `argon2.IDKey`.
    #[test]
    fn the_key_is_the_first_half_of_argon2id() {
        let salt: [u8; 16] = std::array::from_fn(|i| i as u8);
        assert_eq!(
            derive_key(b"password", &salt).to_vec(),
            hex("1ba4bb719f2afd88ee1ab71d82195eff")
        );
        // the PSK as its UTF-8 bytes
        let salt: [u8; 16] = std::array::from_fn(|i| 16 + i as u8);
        assert_eq!(
            derive_key("pässwörd".as_bytes(), &salt).to_vec(),
            hex("d1f4f38f2e7c23b800c99ca3f915ba27")
        );
    }

    #[tokio::test]
    async fn the_psk_derives_off_the_runtime_to_the_same_key() {
        let salt = [9u8; 16];
        let psk = Psk::new("password");
        assert_eq!(psk.key(salt).await.unwrap(), derive_key(b"password", &salt));
    }
}
```

新建 `crates/rurge-proto/src/snell/record.rs`：

````rust
//! The Snell v4 / v5 record stream (phase 2 M6 design 4.3). Each direction
//! starts with its own salt, then records:
//!
//! ```text
//! sealed(04 00 00 ‖ padding length, u16 BE ‖ payload length, u16 BE)
//! padding
//! sealed(payload)                 (absent when the payload is empty)
//! ```
//!
//! both sealed with the direction's AES-128-GCM key and the next values of
//! its counting nonce (the header one, the payload the next). Only the first
//! record of a direction is padded — 256 to 511 random bytes — and its
//! padding and payload ciphertext are mixed: the bytes at the even indices
//! below the shorter of the two trade places. A record with an empty payload
//! ends the direction (a half-close); reads then return end-of-file.
//!
//! One stream carries one request at a time (`reuse`, design 4.3): once
//! both directions have ended, `next_tunnel` opens the next request on the
//! same salts, keys and nonce counters, without padding.
//!
//! Every record we write is at most `MAX_PAYLOAD` bytes; Surge's growing
//! record sizes (and v5's dynamic record sizing on the server) only shape
//! traffic, and receivers take any length (M6-D7). A write reports success
//! only after its record has been handed to the layer below, so the stream
//! never depends on anyone calling `flush`.
//!
//! The server's key is derived when its salt arrives, inside a read: the
//! Argon2id runs on a blocking thread whose task the read polls.
//!
//! Two contracts on the caller, as for `AeadStream`: a write that returned
//! `Pending` must be retried with the same bytes, and an error is final.

use super::kdf::{KEY_LEN, Psk, SALT_LEN, key_failed};
use crate::shadowsocks::cipher::{AeadKind, CountingAead, TAG};
use rurge_net::connector::BoxedStream;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::task::JoinHandle;

/// The largest payload we put in a record.
pub(crate) const MAX_PAYLOAD: usize = 0x3FFF;
/// The version byte of v4's records, which v5 keeps.
const VERSION: u8 = 0x04;
const HEADER: usize = 7;
/// The first record's padding: `PADDING_MIN + (0..256)` bytes.
const PADDING_MIN: usize = 0x100;

const NO_ANSWER: &str = "snell: the server closed the connection without answering";
const UNDECRYPTABLE: &str = "snell: the server's data failed to decrypt (wrong psk or version?)";
const CUT_SHORT: &str = "snell: the connection ended in the middle of a record";
const UNKNOWN_VERSION: &str = "snell: the server sent a record of an unknown version";
const ENDED: &str = "snell: the request's sending side has already ended";

fn invalid(text: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, text)
}

fn cut_short() -> io::Error {
    io::Error::new(io::ErrorKind::UnexpectedEof, CUT_SHORT)
}

fn no_randomness(_: getrandom::Error) -> io::Error {
    io::Error::other("snell: no randomness available")
}

/// A fresh salt.
pub(crate) fn new_salt() -> io::Result<[u8; SALT_LEN]> {
    let mut salt = [0u8; SALT_LEN];
    getrandom::fill(&mut salt).map_err(no_randomness)?;
    Ok(salt)
}

/// The first record's padding: 256 to 511 random bytes.
pub(crate) fn first_padding() -> io::Result<Vec<u8>> {
    let mut len = [0u8; 1];
    getrandom::fill(&mut len).map_err(no_randomness)?;
    let mut padding = vec![0u8; PADDING_MIN + usize::from(len[0])];
    getrandom::fill(&mut padding).map_err(no_randomness)?;
    Ok(padding)
}

/// Trades the bytes at the even indices of `padding` and `sealed` below the
/// shorter length; its own inverse.
fn mix(padding: &mut [u8], sealed: &mut [u8]) {
    for i in (0..padding.len().min(sealed.len())).step_by(2) {
        std::mem::swap(&mut padding[i], &mut sealed[i]);
    }
}

/// Appends one record: `payload` (at most `MAX_PAYLOAD` bytes; empty ends
/// the direction), after `padding`.
fn seal_record(up: &mut CountingAead, padding: &mut [u8], payload: &[u8], out: &mut Vec<u8>) {
    let padding_len = u16::try_from(padding.len()).expect("the padding is below 512 bytes");
    let payload_len = u16::try_from(payload.len()).expect("a record's payload fits two bytes");
    let mut header = [0u8; HEADER];
    header[0] = VERSION;
    header[3..5].copy_from_slice(&padding_len.to_be_bytes());
    header[5..7].copy_from_slice(&payload_len.to_be_bytes());
    up.seal(&header, out);
    let start = out.len();
    out.extend_from_slice(padding);
    if !payload.is_empty() {
        up.seal(payload, out);
        let (padding, sealed) = out[start..].split_at_mut(padding.len());
        mix(padding, sealed);
    }
}

enum Reading {
    Salt {
        buf: [u8; SALT_LEN],
        filled: usize,
    },
    /// The server's key, on a blocking thread.
    Key(JoinHandle<[u8; KEY_LEN]>),
    Header {
        buf: [u8; HEADER + TAG],
        filled: usize,
    },
    /// The padding and the sealed payload (none when it is empty).
    Body {
        buf: Vec<u8>,
        filled: usize,
        padding: usize,
    },
    Payload {
        buf: Vec<u8>,
        pos: usize,
        end: usize,
    },
    /// The server's empty record: its side of this request is over.
    Ended,
    /// The connection closed between two records.
    Closed,
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

/// No `Debug`: it holds the connection's keys.
pub(crate) struct SnellStream {
    inner: BoxedStream,
    psk: Psk,
    /// Ours, sent in front of the first record; kept to tell a reflected
    /// stream apart.
    salt: [u8; SALT_LEN],
    salt_sent: bool,
    /// The first record's, until it has gone.
    padding: Option<Vec<u8>>,
    up: CountingAead,
    /// Known once the server's salt has arrived and its key is derived.
    down: Option<CountingAead>,
    /// The record being written, how much of it is out, and how many payload
    /// bytes it carries.
    out: Vec<u8>,
    out_pos: usize,
    accepted: usize,
    /// Our empty record of this request is sealed (maybe still in `out`).
    write_ended: bool,
    reading: Reading,
    /// An error was returned: the stream is not reusable.
    failed: bool,
}

impl SnellStream {
    /// `up_key` is `salt`'s key under `psk`; `padding` goes into the first
    /// record that carries a payload (`first_padding`).
    pub(crate) fn new(
        inner: BoxedStream,
        psk: Psk,
        salt: [u8; SALT_LEN],
        up_key: [u8; KEY_LEN],
        padding: Vec<u8>,
    ) -> SnellStream {
        SnellStream {
            inner,
            psk,
            salt,
            salt_sent: false,
            padding: Some(padding),
            up: CountingAead::new(AeadKind::Aes128Gcm, &up_key),
            down: None,
            out: Vec::new(),
            out_pos: 0,
            accepted: 0,
            write_ended: false,
            reading: Reading::Salt {
                buf: [0; SALT_LEN],
                filled: 0,
            },
            failed: false,
        }
    }

    /// A stream over `inner` with a fresh salt and padding, its key derived
    /// on a blocking thread. Nothing is written yet.
    pub(crate) async fn open(inner: BoxedStream, psk: Psk) -> io::Result<SnellStream> {
        let salt = new_salt()?;
        let padding = first_padding()?;
        let key = psk.key(salt).await?;
        Ok(SnellStream::new(inner, psk, salt, key, padding))
    }

    /// The server has ended its side of the current request.
    pub(crate) fn read_ended(&self) -> bool {
        matches!(self.reading, Reading::Ended)
    }

    /// Both sides of the current request have ended, cleanly, and our end
    /// has gone out: the next request may follow (`next_tunnel`).
    pub(crate) fn is_reusable(&self) -> bool {
        !self.failed && self.write_ended && self.out_pos == self.out.len() && self.read_ended()
    }

    /// Starts the next request on the same salts, keys and counters. Only
    /// when `is_reusable`.
    pub(crate) fn next_tunnel(&mut self) {
        debug_assert!(self.is_reusable(), "a request still in progress");
        self.write_ended = false;
        self.reading = Reading::Header {
            buf: [0; HEADER + TAG],
            filled: 0,
        };
    }

    /// Sends our empty record, which ends our side of the current request,
    /// and flushes; the connection stays open. Once per request.
    pub(crate) fn poll_end(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // a parked record first: it already consumed its nonces
        ready!(self.poll_out(cx))?;
        if !self.write_ended {
            self.out.clear();
            self.out_pos = 0;
            self.start_record();
            // an empty record is never padded
            seal_record(&mut self.up, &mut [], &[], &mut self.out);
            self.write_ended = true;
        }
        ready!(self.poll_out(cx))?;
        let flushed = ready!(Pin::new(&mut self.inner).poll_flush(cx));
        Poll::Ready(self.check(flushed))
    }

    /// The salt goes in front of the first record.
    fn start_record(&mut self) {
        if !self.salt_sent {
            self.out.extend_from_slice(&self.salt);
            self.salt_sent = true;
        }
    }

    fn check<T>(&mut self, result: io::Result<T>) -> io::Result<T> {
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn poll_out(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.out_pos < self.out.len() {
            let written =
                ready!(Pin::new(&mut self.inner).poll_write(cx, &self.out[self.out_pos..]));
            match self.check(written) {
                Ok(0) => {
                    self.failed = true;
                    return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
                }
                Ok(n) => self.out_pos += n,
                Err(e) => return Poll::Ready(Err(e)),
            }
        }
        Poll::Ready(Ok(()))
    }

    fn poll_read_records(
        &mut self,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            match &mut self.reading {
                Reading::Salt { buf, filled } => {
                    if !ready!(poll_fill(&mut self.inner, cx, buf, filled))? {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            NO_ANSWER,
                        )));
                    }
                    // our own salt back: a reflected stream, whose records
                    // would open under our own key
                    if *buf == self.salt {
                        return Poll::Ready(Err(invalid(UNDECRYPTABLE)));
                    }
                    self.reading = Reading::Key(self.psk.spawn_key(*buf));
                }
                Reading::Key(task) => {
                    let key = ready!(Pin::new(task).poll(cx)).map_err(key_failed)?;
                    self.down = Some(CountingAead::new(AeadKind::Aes128Gcm, &key));
                    self.reading = Reading::Header {
                        buf: [0; HEADER + TAG],
                        filled: 0,
                    };
                }
                Reading::Header { buf, filled } => {
                    if !ready!(poll_fill(&mut self.inner, cx, buf, filled))? {
                        self.reading = Reading::Closed;
                        continue;
                    }
                    let down = self.down.as_mut().expect("the key came first");
                    if down.open(buf).is_none() {
                        return Poll::Ready(Err(invalid(UNDECRYPTABLE)));
                    }
                    if buf[0] != VERSION {
                        return Poll::Ready(Err(invalid(UNKNOWN_VERSION)));
                    }
                    // the reserved bytes are not checked
                    let padding = usize::from(u16::from_be_bytes([buf[3], buf[4]]));
                    let len = usize::from(u16::from_be_bytes([buf[5], buf[6]]));
                    let sealed = if len == 0 { 0 } else { len + TAG };
                    self.reading = Reading::Body {
                        buf: vec![0; padding + sealed],
                        filled: 0,
                        padding,
                    };
                }
                Reading::Body {
                    buf,
                    filled,
                    padding,
                } => {
                    if !buf.is_empty() && !ready!(poll_fill(&mut self.inner, cx, buf, filled))? {
                        return Poll::Ready(Err(cut_short()));
                    }
                    let padding = *padding;
                    if buf.len() == padding {
                        // an empty payload: the server's side has ended (a
                        // padding on it is tolerated)
                        self.reading = Reading::Ended;
                        continue;
                    }
                    let (pad, sealed) = buf.split_at_mut(padding);
                    mix(pad, sealed);
                    let down = self.down.as_mut().expect("the key came first");
                    let Some(n) = down.open(sealed) else {
                        return Poll::Ready(Err(invalid(UNDECRYPTABLE)));
                    };
                    self.reading = Reading::Payload {
                        buf: std::mem::take(buf),
                        pos: padding,
                        end: padding + n,
                    };
                }
                Reading::Payload { buf, pos, end } => {
                    let n = out.remaining().min(*end - *pos);
                    out.put_slice(&buf[*pos..*pos + n]);
                    *pos += n;
                    if pos == end {
                        self.reading = Reading::Header {
                            buf: [0; HEADER + TAG],
                            filled: 0,
                        };
                    }
                    return Poll::Ready(Ok(()));
                }
                Reading::Ended | Reading::Closed => return Poll::Ready(Ok(())),
            }
        }
    }
}

impl AsyncRead for SnellStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let result = ready!(this.poll_read_records(cx, out));
        Poll::Ready(this.check(result))
    }
}

impl AsyncWrite for SnellStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        // (a parked record is then our empty one)
        if this.write_ended {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, ENDED)));
        }
        if this.out_pos == this.out.len() {
            this.out.clear();
            this.out_pos = 0;
            this.start_record();
            let n = data.len().min(MAX_PAYLOAD);
            let mut padding = this.padding.take().unwrap_or_default();
            seal_record(&mut this.up, &mut padding, &data[..n], &mut this.out);
            this.accepted = n;
        }
        ready!(this.poll_out(cx))?;
        Poll::Ready(Ok(this.accepted.min(data.len())))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_out(cx))?;
        let flushed = ready!(Pin::new(&mut this.inner).poll_flush(cx));
        Poll::Ready(this.check(flushed))
    }

    /// Our empty record, then the connection's own shutdown: the stream is
    /// not reused. A request on a reused connection ends with `poll_end`.
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_end(cx))?;
        let shut = ready!(Pin::new(&mut this.inner).poll_shutdown(cx));
        Poll::Ready(this.check(shut))
    }
}

#[cfg(test)]
mod tests {
    use super::super::kdf::derive_key;
    use super::*;
    use crate::vmess::vectors::hex;
    use std::future::poll_fn;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

    /// Computed with Python `cryptography` (AES-GCM, Argon2id) from the
    /// byte-level notes alone: PSK "password", salt `00 01 … 0f`; "hello"
    /// padded with `a0 … a4`, then "world", then the empty record.
    const HELLO_WORLD: &str = "000102030405060708090a0b0c0d0e0f46904cd0db456dc6896911d75cdc876786ec38a822d80fe4a1f1a32ba015a219a48bbed0cfa9e98020abbedb61bb3dcd479ea6faaca185f1d36f60e6c6c7556418fb24871281897bfbb1b0829388a5abdda8dfae70038f19dca653d0e3b7c8c8e32fe5379ba3db0f4a59b307403b583420b32840";
    /// The same key: "hi" padded with `c0 … d3`, a padding longer than the
    /// payload's ciphertext (18 bytes), so only its first 18 bytes mix.
    const PADDED_HI: &str = "000102030405060708090a0b0c0d0e0f46904cd0ca456a55dac908474d47bd258c8e797e7e67ece4c1e9c324c576c735c94fcb24cd43cf42d1d2d3c019c212c483c69fc89dcab1cc04ce9dd03d";

    fn salt(first: u8) -> [u8; SALT_LEN] {
        std::array::from_fn(|i| first + i as u8)
    }

    /// A stream keyed with `psk` and `salt`, its key derived here.
    fn keyed(inner: BoxedStream, psk: &str, salt: [u8; SALT_LEN], padding: Vec<u8>) -> SnellStream {
        let key = derive_key(psk.as_bytes(), &salt);
        SnellStream::new(inner, Psk::new(psk), salt, key, padding)
    }

    fn boxed(stream: DuplexStream) -> BoxedStream {
        Box::new(stream)
    }

    /// What a stream keyed with `psk` reads from a peer that sends `wire`
    /// through a pipe of `capacity` bytes and then closes.
    async fn read_from(psk: &str, wire: Vec<u8>, capacity: usize) -> io::Result<Vec<u8>> {
        let (near, mut far) = tokio::io::duplex(capacity);
        tokio::spawn(async move {
            let _ = far.write_all(&wire).await;
            // dropping `far` is the close
        });
        let mut stream = keyed(boxed(near), psk, [0xee; SALT_LEN], Vec::new());
        let mut got = Vec::new();
        stream.read_to_end(&mut got).await.map(|_| got)
    }

    /// Two streams on the two ends of a pipe of `capacity` bytes, each
    /// with its own salt and a padding of 300 bytes.
    fn pair(capacity: usize) -> (SnellStream, SnellStream) {
        let (near, far) = tokio::io::duplex(capacity);
        (
            keyed(boxed(near), "psk", salt(0), vec![0x11; 300]),
            keyed(boxed(far), "psk", salt(0x40), vec![0x22; 300]),
        )
    }

    #[tokio::test]
    async fn the_first_record_is_padded_and_mixed_as_the_known_answer() {
        let (near, mut far) = tokio::io::duplex(64 * 1024);
        let mut stream = keyed(boxed(near), "password", salt(0), (0xa0..=0xa4).collect());
        // an empty write seals nothing: an empty record would end the direction
        assert_eq!(stream.write(b"").await.unwrap(), 0);
        stream.write_all(b"hello").await.unwrap();
        stream.write_all(b"world").await.unwrap();
        stream.shutdown().await.unwrap();
        let mut wire = Vec::new();
        far.read_to_end(&mut wire).await.unwrap();
        assert_eq!(wire, hex(HELLO_WORLD));

        let (near, mut far) = tokio::io::duplex(64 * 1024);
        let mut stream = keyed(boxed(near), "password", salt(0), (0xc0..=0xd3).collect());
        stream.write_all(b"hi").await.unwrap();
        drop(stream);
        let mut wire = Vec::new();
        far.read_to_end(&mut wire).await.unwrap();
        assert_eq!(wire, hex(PADDED_HI));
    }

    #[tokio::test]
    async fn the_known_answers_read_back_whatever_the_slicing() {
        for capacity in [1, 7, 4096] {
            // the empty record is the end
            let got = read_from("password", hex(HELLO_WORLD), capacity)
                .await
                .unwrap();
            assert_eq!(got, b"helloworld", "through {capacity}");
            // a close between two records is the end too
            let got = read_from("password", hex(PADDED_HI), capacity)
                .await
                .unwrap();
            assert_eq!(got, b"hi", "through {capacity}");
        }
    }

    #[tokio::test]
    async fn two_streams_carry_a_large_payload_both_ways_whatever_the_slicing() {
        let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        for capacity in [1, 7, 4096] {
            let (mut client, mut server) = pair(capacity);
            let sent = data.clone();
            let (to_server, from_server) = tokio::join!(
                async {
                    client.write_all(&sent).await.unwrap();
                    client.shutdown().await.unwrap();
                    let mut got = Vec::new();
                    client.read_to_end(&mut got).await.unwrap();
                    got
                },
                async {
                    let mut got = Vec::new();
                    server.read_to_end(&mut got).await.unwrap();
                    server.write_all(&got).await.unwrap();
                    server.shutdown().await.unwrap();
                    got
                },
            );
            assert_eq!(to_server, data, "through {capacity}");
            assert_eq!(from_server, data, "back through {capacity}");
        }
    }

    #[tokio::test]
    async fn a_large_write_is_cut_into_records_of_at_most_0x3fff() {
        let (near, mut far) = tokio::io::duplex(1 << 20);
        let mut stream = keyed(boxed(near), "psk", salt(0), vec![0; 300]);
        let data = vec![0x55u8; MAX_PAYLOAD + 1];
        assert_eq!(
            stream.write(&data).await.unwrap(),
            MAX_PAYLOAD,
            "one record"
        );
        stream.write_all(&data[MAX_PAYLOAD..]).await.unwrap();
        stream.shutdown().await.unwrap();
        let mut wire = Vec::new();
        far.read_to_end(&mut wire).await.unwrap();
        // only the first record is padded; the empty record is a header alone
        let header = HEADER + TAG;
        assert_eq!(
            wire.len(),
            SALT_LEN + (header + 300 + MAX_PAYLOAD + TAG) + (header + 1 + TAG) + header
        );
        let (near, mut far) = tokio::io::duplex(1 << 20);
        tokio::spawn(async move { far.write_all(&wire).await });
        let mut peer = keyed(boxed(near), "psk", salt(0x40), Vec::new());
        let mut got = Vec::new();
        peer.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, data);
    }

    #[tokio::test]
    async fn the_empty_record_ends_one_direction_and_the_other_goes_on() {
        let (mut client, mut server) = pair(4096);
        client.write_all(b"request").await.unwrap();
        poll_fn(|cx| client.poll_end(cx)).await.unwrap();
        let mut got = Vec::new();
        server.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, b"request");
        assert!(server.read_ended());
        // the end reads as end-of-file again, without touching the connection
        assert_eq!(server.read(&mut [0u8; 8]).await.unwrap(), 0);
        let err = client.write(b"more").await.unwrap_err();
        assert_eq!(
            (err.kind(), err.to_string().as_str()),
            (io::ErrorKind::BrokenPipe, ENDED)
        );
        // the other direction is still open
        server.write_all(b"answer").await.unwrap();
        let mut got = [0u8; 6];
        client.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"answer");
        assert!(!client.is_reusable() && !server.is_reusable());
    }

    /// Moves exactly `len` bytes from one wire to the other: the records
    /// between the two streams, counted.
    async fn pass(from: &mut DuplexStream, to: &mut DuplexStream, len: usize) {
        let mut buf = vec![0u8; len];
        from.read_exact(&mut buf).await.unwrap();
        to.write_all(&buf).await.unwrap();
    }

    /// One request each way: `ask` from the client, `answer` back, each
    /// side ending its direction. `first`: the salts and paddings go too.
    async fn exchange(
        client: &mut SnellStream,
        server: &mut SnellStream,
        wires: (&mut DuplexStream, &mut DuplexStream),
        (ask, answer): (&[u8], &[u8]),
        first: bool,
    ) {
        let header = HEADER + TAG;
        let opening = if first { SALT_LEN + 300 } else { 0 };
        client.write_all(ask).await.unwrap();
        poll_fn(|cx| client.poll_end(cx)).await.unwrap();
        pass(
            wires.0,
            wires.1,
            opening + header + ask.len() + TAG + header,
        )
        .await;
        let mut got = Vec::new();
        server.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, ask);
        server.write_all(answer).await.unwrap();
        poll_fn(|cx| server.poll_end(cx)).await.unwrap();
        pass(
            wires.1,
            wires.0,
            opening + header + answer.len() + TAG + header,
        )
        .await;
        let mut got = Vec::new();
        client.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, answer);
    }

    #[tokio::test]
    async fn two_requests_follow_each_other_on_one_stream() {
        // the two streams on separate pipes, the test carrying the records
        // across: it counts them
        let (near, mut client_wire) = tokio::io::duplex(64 * 1024);
        let (far, mut server_wire) = tokio::io::duplex(64 * 1024);
        let mut client = keyed(boxed(near), "psk", salt(0), vec![0x11; 300]);
        let mut server = keyed(boxed(far), "psk", salt(0x40), vec![0x22; 300]);
        exchange(
            &mut client,
            &mut server,
            (&mut client_wire, &mut server_wire),
            (b"one", b"1"),
            true,
        )
        .await;
        assert!(client.is_reusable() && server.is_reusable());
        client.next_tunnel();
        server.next_tunnel();
        assert!(!client.read_ended());
        // no salt and no padding the second time; the records open only
        // because both counters went on from where the first request left them
        exchange(
            &mut client,
            &mut server,
            (&mut client_wire, &mut server_wire),
            (b"two", b"22"),
            false,
        )
        .await;
        assert!(client.is_reusable() && server.is_reusable());
    }

    #[tokio::test]
    async fn what_the_server_gets_wrong_is_an_error_that_quotes_nothing() {
        let good = hex(HELLO_WORLD);
        // salt, the header, the padding and "hello"'s ciphertext
        let first_record = SALT_LEN + (HEADER + TAG) + 5 + (5 + TAG);
        let mut flipped = good.clone();
        flipped[first_record - 1] ^= 1;
        // a correctly sealed header of version 3
        let mut version_3 = salt(0).to_vec();
        let mut up = CountingAead::new(AeadKind::Aes128Gcm, &derive_key(b"password", &salt(0)));
        up.seal(&[3, 0, 0, 0, 0, 0, 1], &mut version_3);
        let cases: [(&str, &str, Vec<u8>, io::ErrorKind, &str); 7] = [
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
                good[..8].to_vec(),
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
                "another psk",
                "other",
                good.clone(),
                io::ErrorKind::InvalidData,
                UNDECRYPTABLE,
            ),
            (
                "another version",
                "password",
                version_3,
                io::ErrorKind::InvalidData,
                UNKNOWN_VERSION,
            ),
            (
                "the middle of a header",
                "password",
                good[..SALT_LEN + 10].to_vec(),
                io::ErrorKind::UnexpectedEof,
                CUT_SHORT,
            ),
            (
                "the middle of a payload",
                "password",
                good[..first_record - 1].to_vec(),
                io::ErrorKind::UnexpectedEof,
                CUT_SHORT,
            ),
        ];
        for (case, psk, wire, kind, text) in cases {
            let err = read_from(psk, wire, 4096).await.unwrap_err();
            assert_eq!(
                (err.kind(), err.to_string().as_str()),
                (kind, text),
                "{case}"
            );
        }
    }

    #[tokio::test]
    async fn a_reflected_stream_does_not_decrypt() {
        let (near, mut far) = tokio::io::duplex(4096);
        // a "server" that sends the client's bytes back
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            while let Ok(n @ 1..) = far.read(&mut buf).await {
                if far.write_all(&buf[..n]).await.is_err() {
                    break;
                }
            }
        });
        let mut stream = keyed(boxed(near), "psk", salt(0), vec![0; 300]);
        stream.write_all(b"hello").await.unwrap();
        let err = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut [0u8; 5]))
            .await
            .expect("an answer in time")
            .unwrap_err();
        assert_eq!(
            (err.kind(), err.to_string().as_str()),
            (io::ErrorKind::InvalidData, UNDECRYPTABLE)
        );
        assert!(!stream.is_reusable());
    }

    /// With the only blocking thread busy, the server's key cannot be
    /// derived: the read waits instead of deriving on the runtime's thread,
    /// and goes on once the thread is free.
    #[test]
    fn the_servers_key_is_derived_on_a_blocking_thread() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (release, wait) = std::sync::mpsc::channel::<()>();
            let busy = tokio::task::spawn_blocking(move || wait.recv());
            let (mut stream, _server) = stream_from(hex(HELLO_WORLD)).await;
            let mut got = [0u8; 10];
            let pending =
                tokio::time::timeout(Duration::from_millis(200), stream.read_exact(&mut got)).await;
            assert!(pending.is_err(), "no key without a blocking thread");
            release.send(()).unwrap();
            busy.await.unwrap().unwrap();
            tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut got))
                .await
                .expect("the key in time")
                .unwrap();
            assert_eq!(&got, b"helloworld");
        });

        /// The stream, and the server's end, which stays open.
        async fn stream_from(wire: Vec<u8>) -> (SnellStream, DuplexStream) {
            let (near, mut far) = tokio::io::duplex(4096);
            far.write_all(&wire).await.unwrap();
            (
                keyed(boxed(near), "password", [0xee; SALT_LEN], Vec::new()),
                far,
            )
        }
    }
}
````

- [ ] **Step 2: 运行**

Run: `cargo test -p rurge-proto --lib snell`
Expected: 通过（Argon2id 与整条记录的已知答案、任意切片（1 / 7 / 4096）的往返、跨多条记录的大负载、空记录即 EOF、两段隧道共用一条流且计数继续、篡改一个字节即解密失败、沉默即"没有应答"、反射的 salt 被拒、服务端密钥不在运行时线程上派生）。

要点：
- 错误文字：服务端 salt 一个字节没到就 EOF → `snell: the server closed the connection without answering`；salt、头或负载中途 EOF → `snell: the connection ended in the middle of a record`；标签不对 → `snell: the server's data failed to decrypt (wrong psk or version?)`；版本字节不是 4 → `snell: the server sent a record of an unknown version`。都不带密钥或 salt。
- 任何读、写、flush 出错后流都标记为不可复用。
- `Psk` 与 `SnellStream` 不实现 `Debug`。

- [ ] **Step 3: 门禁与提交**

跑门禁。

```bash
git add Cargo.toml Cargo.lock crates/rurge-proto
git commit -m "feat(proto): Snell 的 Argon2id 与记录流——每方向 salt、交错填充、空记录结束、复用时计数继续"
```

### Task 3: TCP、复用池与 `FakeSnell`

`SnellOutbound` 的 TCP：请求头与应答（P6）、复用池（P7）、陈旧连接的一次重试（P8）、`obfs=http`（P9）。`LazyHead` 泛化成 `LazyHead<S = BoxedStream>`（已有的调用处不变）。`FakeSnell` 独立实现（只共用经向量测试过的 KDF 与 AEAD 原语）。

**Files:**
- Create: `crates/rurge-proto/src/snell/pool.rs`、`tunnel.rs`、`crates/rurge-proto/src/testing/snell.rs`
- Modify: `crates/rurge-proto/src/snell/mod.rs`（出站与用例）、`src/lib.rs`、`src/transport/lazy_head.rs`、`src/testing/mod.rs`

**Interfaces:**
- Consumes: Task 1 的 `SnellSpec`；Task 2 的 `SnellStream` / `Psk`；M6a 的 `ObfsClient` / `accept_obfs`；既有的 `Stack`、`hostname::to_ascii`。
- Produces:
  - `pub struct SnellOutbound`：`new(name: &str, server: Target, spec: &SnellSpec, shadow_tls: Option<&ShadowTlsOpts>, roots: Arc<RootCertStore>, connector: Arc<dyn Connector>) -> Result<SnellOutbound, BuildError>`
  - `LazyHead<S = BoxedStream>`、`LazyHead::get_mut` / `into_inner`
  - 测试设施：`SnellScript`、`FakeSnell::spawn` / `addr` / `requests` / `connections` / `rejected` / `unanswered` / `largest_record` 等（见新文件）

- [ ] **Step 1: 先写假服务端与出站（连同用例）**

假服务端（自己的记录、nonce、填充交错与请求解析）：

新建 `crates/rurge-proto/src/testing/snell.rs`：

```rust
//! A scriptable Snell v4 / v5 server: optionally simple-obfs `http` in
//! front, then the record stream, requests one after another on a
//! connection (ConnectV2), and a relay. Its records, nonce counting,
//! padding and request parsing are written apart from the client's
//! (`crate::snell`), so each checks the other; only the primitives (the
//! Argon2id derivation and AES-GCM, which have vectors of their own) are
//! shared. It never resolves a name.
//!
//! As the reference server (per the byte-level notes of the M6b plan) it
//! answers a request only when the target first sends: `00` and the data
//! in one record, the direction's first record padded. A target that ends
//! before sending anything gets `02 65 "Remote EOF"` (ConnectV2) or
//! `02 ff "end of file"` (Connect) and the close. When the target ends, a
//! ConnectV2 tunnel sends its empty record, forwards (or, once the target
//! is gone, discards) the client's records until the client's empty record,
//! then reads the next request; a Connect tunnel closes instead.
//!
//! A wrong PSK gets no word back, only the close.

use super::AbortOnDrop;
use super::obfs::{ObfsHello, accept_obfs};
use crate::shadowsocks::cipher::{AeadCipher, AeadKind, TAG};
use crate::snell::kdf;
use rurge_config::spec::ObfsMode;
use rurge_net::connector::BoxedStream;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};

const SALT: usize = 16;
const HEADER: usize = 7 + TAG;
const VERSION: u8 = 0x04;
const MAX_PAYLOAD: usize = 0x3FFF;
const CONNECT: u8 = 0x01;
const CONNECT_V2: u8 = 0x05;

#[derive(Clone, Debug)]
pub struct SnellScript {
    pub psk: String,
    /// Expect simple-obfs `http` in front of the protocol.
    pub obfs_http: bool,
    /// Relay here whatever the client asked for (needed for a domain target).
    pub connect_to: Option<SocketAddr>,
    /// A request that arrives after this many tunnels on its connection is
    /// not answered: the connection closes (a server that retires reused
    /// connections).
    pub tunnels_per_connection: Option<usize>,
    /// Answer every request with this error code and message, then close.
    pub refuse: Option<(u8, Vec<u8>)>,
}

impl SnellScript {
    pub fn new(psk: &str) -> SnellScript {
        SnellScript {
            psk: psk.to_string(),
            obfs_http: false,
            connect_to: None,
            tunnels_per_connection: None,
            refuse: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedSnell {
    /// `01` Connect, `05` ConnectV2.
    pub command: u8,
    pub client_id: Vec<u8>,
    /// Exactly as it was on the wire (an IP literal is text too).
    pub host: String,
    pub port: u16,
    /// Payload after the request, in the same record.
    pub early: Vec<u8>,
    /// The connection's number, from 0 in the order they were accepted.
    pub connection: usize,
    /// The request's number on its connection, from 0.
    pub tunnel: usize,
    /// The padding of the record that carried the request.
    pub padding: usize,
}

pub struct FakeSnell {
    addr: SocketAddr,
    shared: Arc<Shared>,
    _task: AbortOnDrop,
}

#[derive(Default)]
struct Seen {
    requests: Mutex<Vec<RecordedSnell>>,
    obfs: Mutex<Vec<ObfsHello>>,
    connections: AtomicUsize,
    rejected: AtomicUsize,
    unanswered: AtomicUsize,
    largest_record: AtomicUsize,
}

struct Shared {
    script: SnellScript,
    seen: Seen,
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("fake snell: {what}"))
}

/// One direction's cipher, its nonce a little-endian counter from zero.
struct Direction {
    cipher: AeadCipher,
    count: u64,
}

impl Direction {
    fn new(psk: &str, salt: &[u8; SALT]) -> Direction {
        Direction {
            cipher: AeadCipher::new(AeadKind::Aes128Gcm, &kdf::derive_key(psk.as_bytes(), salt)),
            count: 0,
        }
    }

    fn nonce(&mut self) -> [u8; 12] {
        let mut nonce = [0u8; 12];
        nonce[..8].copy_from_slice(&self.count.to_le_bytes());
        self.count += 1;
        nonce
    }

    fn seal(&mut self, plain: &[u8]) -> Vec<u8> {
        let nonce = self.nonce();
        let mut sealed = plain.to_vec();
        let tag = self.cipher.seal_in_place(&nonce, &mut sealed);
        sealed.extend_from_slice(&tag);
        sealed
    }

    fn open(&mut self, sealed: &[u8]) -> Option<Vec<u8>> {
        let nonce = self.nonce();
        let (data, tag) = sealed.split_at(sealed.len().checked_sub(TAG)?);
        let mut data = data.to_vec();
        let tag: [u8; TAG] = tag.try_into().ok()?;
        self.cipher
            .open_in_place(&nonce, &mut data, &tag)
            .then_some(data)
    }
}

/// Swaps `a[i]` and `b[i]` for the even `i` below the shorter length.
fn unmix(a: &mut [u8], b: &mut [u8]) {
    let mut i = 0;
    while i < a.len() && i < b.len() {
        std::mem::swap(&mut a[i], &mut b[i]);
        i += 2;
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

enum Record {
    Data { payload: Vec<u8>, padding: usize },
    End,
}

/// The client's next record; `Ok(None)` when it closed between records.
async fn read_record<R: AsyncRead + Unpin>(
    r: &mut R,
    up: &mut Direction,
    seen: &Seen,
) -> io::Result<Option<Record>> {
    let mut sealed = [0u8; HEADER];
    if !read_full(r, &mut sealed).await? {
        return Ok(None);
    }
    let header = up
        .open(&sealed)
        .ok_or_else(|| bad("a header that does not authenticate"))?;
    if header[0] != VERSION {
        return Err(bad("a record of another version"));
    }
    let padding = usize::from(u16::from_be_bytes([header[3], header[4]]));
    let len = usize::from(u16::from_be_bytes([header[5], header[6]]));
    if len > MAX_PAYLOAD {
        return Err(bad("a record over 0x3fff"));
    }
    seen.largest_record.fetch_max(len, Ordering::SeqCst);
    let mut body = vec![0u8; padding + if len == 0 { 0 } else { len + TAG }];
    r.read_exact(&mut body).await?;
    if len == 0 {
        return Ok(Some(Record::End));
    }
    let (pad, payload) = body.split_at_mut(padding);
    unmix(pad, payload);
    let payload = up
        .open(payload)
        .ok_or_else(|| bad("a payload that does not authenticate"))?;
    Ok(Some(Record::Data { payload, padding }))
}

/// The server's direction: its salt goes with the first record, and the
/// first record with a payload is padded.
struct Answers {
    down: Direction,
    salt: Option<[u8; SALT]>,
    padded: bool,
}

impl Answers {
    fn new(psk: &str) -> Answers {
        let mut salt = [0u8; SALT];
        getrandom::fill(&mut salt).expect("randomness");
        Answers {
            down: Direction::new(psk, &salt),
            salt: Some(salt),
            padded: false,
        }
    }

    /// One record of `payload`; empty: the end of this tunnel.
    fn record(&mut self, payload: &[u8]) -> Vec<u8> {
        let mut out = self.salt.take().map(Vec::from).unwrap_or_default();
        let mut padding = Vec::new();
        if !payload.is_empty() && !self.padded {
            self.padded = true;
            let mut len = [0u8; 1];
            getrandom::fill(&mut len).expect("randomness");
            padding = vec![0u8; 256 + usize::from(len[0])];
            getrandom::fill(&mut padding).expect("randomness");
        }
        let mut header = [VERSION, 0, 0, 0, 0, 0, 0];
        header[3..5].copy_from_slice(&(padding.len() as u16).to_be_bytes());
        header[5..7].copy_from_slice(&(payload.len() as u16).to_be_bytes());
        out.extend_from_slice(&self.down.seal(&header));
        if !payload.is_empty() {
            let mut sealed = self.down.seal(payload);
            unmix(&mut padding, &mut sealed);
            out.extend_from_slice(&padding);
            out.extend_from_slice(&sealed);
        }
        out
    }

    /// `02 code length message`.
    fn error(&mut self, code: u8, message: &[u8]) -> Vec<u8> {
        let mut payload = vec![0x02, code, message.len() as u8];
        payload.extend_from_slice(message);
        self.record(&payload)
    }
}

/// `01 command id-length id host-length host port early`.
fn parse_request(p: &[u8]) -> Option<RecordedSnell> {
    if *p.first()? != 0x01 {
        return None;
    }
    let command = *p.get(1)?;
    if command != CONNECT && command != CONNECT_V2 {
        return None;
    }
    let id_len = usize::from(*p.get(2)?);
    let client_id = p.get(3..3 + id_len)?.to_vec();
    let at = 3 + id_len;
    let host_len = usize::from(*p.get(at)?);
    let host = p.get(at + 1..at + 1 + host_len)?;
    let at = at + 1 + host_len;
    let port = u16::from_be_bytes(p.get(at..at + 2)?.try_into().ok()?);
    Some(RecordedSnell {
        command,
        client_id,
        host: String::from_utf8_lossy(host).into_owned(),
        port,
        early: p[at + 2..].to_vec(),
        connection: 0,
        tunnel: 0,
        padding: 0,
    })
}

/// Shuts our side and reads until the client goes: closing with unread
/// data would reset.
async fn close<R, W>(r: &mut R, w: &mut W)
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let _ = w.shutdown().await;
    let mut sink = [0u8; 4096];
    while let Ok(n) = r.read(&mut sink).await {
        if n == 0 {
            break;
        }
    }
}

type Reader = ReadHalf<BoxedStream>;
type Writer = WriteHalf<BoxedStream>;

impl Shared {
    fn upstream(&self, request: &RecordedSnell) -> Option<SocketAddr> {
        self.script.connect_to.or_else(|| {
            let ip = request.host.parse::<IpAddr>().ok()?;
            Some(SocketAddr::new(ip, request.port))
        })
    }

    /// One tunnel: `true` when both sides ended with their empty records
    /// (ConnectV2), so the next request may follow.
    async fn tunnel(
        &self,
        request: &RecordedSnell,
        reader: &mut Reader,
        up: &mut Direction,
        writer: &mut Writer,
        answers: &mut Answers,
    ) -> bool {
        let reuse = request.command == CONNECT_V2;
        let upstream = match self.upstream(request) {
            Some(to) => TcpStream::connect(to).await.ok(),
            None => None,
        };
        let Some(mut upstream) = upstream else {
            let _ = writer
                .write_all(&answers.error(0x01, b"fake snell: cannot connect"))
                .await;
            close(reader, writer).await;
            return false;
        };
        // a target already gone shows in its answer
        let _ = upstream.write_all(&request.early).await;
        let (mut target_read, mut target_write) = upstream.split();
        let requests = async {
            loop {
                match read_record(reader, up, &self.seen).await {
                    // once the target is gone, the client's data is discarded
                    Ok(Some(Record::Data { payload, .. })) => {
                        let _ = target_write.write_all(&payload).await;
                    }
                    Ok(Some(Record::End)) => {
                        let _ = target_write.shutdown().await;
                        return true;
                    }
                    _ => return false,
                }
            }
        };
        let replies = async {
            let mut buf = vec![0u8; MAX_PAYLOAD];
            let n = target_read.read(&mut buf[1..]).await.unwrap_or(0);
            if n == 0 {
                let error = if reuse {
                    answers.error(0x65, b"Remote EOF")
                } else {
                    answers.error(0xff, b"end of file")
                };
                let _ = writer.write_all(&error).await;
                let _ = writer.shutdown().await;
                return false;
            }
            // the answer rides with the target's first data
            buf[0] = 0x00;
            if writer
                .write_all(&answers.record(&buf[..1 + n]))
                .await
                .is_err()
            {
                return false;
            }
            loop {
                match target_read.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if writer.write_all(&answers.record(&buf[..n])).await.is_err() {
                            return false;
                        }
                    }
                }
            }
            if reuse {
                writer.write_all(&answers.record(&[])).await.is_ok()
            } else {
                // Connect: no empty record, the close
                let _ = writer.shutdown().await;
                false
            }
        };
        let (client_ended, server_ended) = tokio::join!(requests, replies);
        client_ended && server_ended
    }
}

async fn serve(tcp: TcpStream, shared: Arc<Shared>, connection: usize) -> io::Result<()> {
    let script = &shared.script;
    let mut stream: BoxedStream = Box::new(tcp);
    if script.obfs_http {
        let (inner, hello) = accept_obfs(stream, ObfsMode::Http).await?;
        shared.seen.obfs.lock().expect("obfs").push(hello);
        stream = inner;
    }
    let mut salt = [0u8; SALT];
    if !read_full(&mut stream, &mut salt).await? {
        return Ok(());
    }
    let mut up = Direction::new(&script.psk, &salt);
    let mut answers = Answers::new(&script.psk);
    let (mut reader, mut writer) = tokio::io::split(stream);
    for tunnel in 0.. {
        let (payload, padding) = match read_record(&mut reader, &mut up, &shared.seen).await {
            Ok(Some(Record::Data { payload, padding })) => (payload, padding),
            Ok(None) => return Ok(()),
            Err(_) if tunnel == 0 => {
                // a wrong PSK: no word back
                shared.seen.rejected.fetch_add(1, Ordering::SeqCst);
                close(&mut reader, &mut writer).await;
                return Ok(());
            }
            _ => return Ok(()),
        };
        let Some(mut request) = parse_request(&payload) else {
            return Ok(());
        };
        if script.tunnels_per_connection.is_some_and(|n| tunnel >= n) {
            shared.seen.unanswered.fetch_add(1, Ordering::SeqCst);
            return Ok(());
        }
        request.connection = connection;
        request.tunnel = tunnel;
        request.padding = padding;
        shared
            .seen
            .requests
            .lock()
            .expect("requests")
            .push(request.clone());
        if let Some((code, message)) = &script.refuse {
            writer.write_all(&answers.error(*code, message)).await?;
            close(&mut reader, &mut writer).await;
            return Ok(());
        }
        let next = shared
            .tunnel(&request, &mut reader, &mut up, &mut writer, &mut answers)
            .await;
        if !next {
            close(&mut reader, &mut writer).await;
            return Ok(());
        }
    }
    Ok(())
}

impl FakeSnell {
    pub async fn spawn(script: SnellScript) -> FakeSnell {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let shared = Arc::new(Shared {
            script,
            seen: Seen::default(),
        });
        let serving = shared.clone();
        let task = tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let connection = serving.seen.connections.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(serve(tcp, serving.clone(), connection));
            }
        });
        FakeSnell {
            addr,
            shared,
            _task: AbortOnDrop(task),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn requests(&self) -> Vec<RecordedSnell> {
        self.shared.seen.requests.lock().expect("requests").clone()
    }

    /// What each connection's camouflage said, when the script expects one.
    pub fn obfs_seen(&self) -> Vec<ObfsHello> {
        self.shared.seen.obfs.lock().expect("obfs").clone()
    }

    /// TCP connections accepted so far.
    pub fn connections(&self) -> usize {
        self.shared.seen.connections.load(Ordering::SeqCst)
    }

    /// Connections given no answer because they did not decrypt.
    pub fn rejected(&self) -> usize {
        self.shared.seen.rejected.load(Ordering::SeqCst)
    }

    /// Requests closed without an answer (`tunnels_per_connection`).
    pub fn unanswered(&self) -> usize {
        self.shared.seen.unanswered.load(Ordering::SeqCst)
    }

    /// The longest payload of any client record so far.
    pub fn largest_record(&self) -> usize {
        self.shared.seen.largest_record.load(Ordering::SeqCst)
    }
}
```

`crates/rurge-proto/src/testing/mod.rs`——把

```rust
mod shadowsocks;
```

换成

```rust
mod shadowsocks;
mod snell;
```

`crates/rurge-proto/src/testing/mod.rs`——把

```rust
pub use shadowsocks::{FakeShadowsocks, RecordedDatagram, RecordedShadowsocks, ShadowsocksScript};
```

换成

```rust
pub use shadowsocks::{FakeShadowsocks, RecordedDatagram, RecordedShadowsocks, ShadowsocksScript};
pub use snell::{FakeSnell, RecordedSnell, SnellScript};
```

出站与它的用例（引用的 `pool` / `tunnel` 两个模块在 Step 3 写）：

`crates/rurge-proto/src/snell/mod.rs`——把

```rust
//! design 4): the record stream (`record`) keyed per direction by Argon2id
//! of the PSK (`kdf`).

pub(crate) mod kdf;
pub(crate) mod record;

```

换成

```rust
//! design 4): optionally behind Shadow TLS and / or simple-obfs `http`, the
//! record stream (`record`) keyed per direction by Argon2id of the PSK
//! (`kdf`). v5 speaks v4's wire format over TCP.
//!
//! A request is `01 command 00 host-length host port`, the host as text
//! (an IP literal too, an IDN as its A-labels) and no client id; it waits
//! for the first payload and goes out in the same record (`tunnel`). The
//! command is Connect (`01`), or ConnectV2 (`05`) with `reuse=true`: a
//! request whose two sides both ended hands its connection back to the
//! outbound's pool (`pool`), and the next request goes out on it.

pub(crate) mod kdf;
mod pool;
pub(crate) mod record;
mod tunnel;

use crate::build::shadow_tls_client;
use crate::task::AbortOnDrop;
use crate::transport::Stack;
use crate::transport::obfs::ObfsClient;
use crate::{BuildError, Outbound, OutboundError};
use kdf::Psk;
use pool::Pool;
use record::SnellStream;
use rurge_config::HostName;
use rurge_config::spec::{ObfsMode, ShadowTlsOpts, SnellSpec};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rustls::RootCertStore;
use std::sync::{Arc, OnceLock};
use tunnel::SnellTunnel;

/// The request's version byte.
const REQUEST_VERSION: u8 = 0x01;
const CONNECT: u8 = 0x01;
/// Connect on a connection that may carry further requests.
const CONNECT_V2: u8 = 0x05;

/// Opens fresh connections: the transport, then a record stream with a salt
/// of its own. No `Debug`: it holds the PSK.
pub(crate) struct Dialer {
    stack: Stack,
    psk: Psk,
}

impl Dialer {
    pub(crate) async fn fresh(&self, opts: &ConnectOpts) -> Result<SnellStream, OutboundError> {
        let transport = self.stack.open(opts).await?;
        Ok(SnellStream::open(transport, self.psk.clone()).await?)
    }
}

/// No `Debug`: the dialer holds the PSK.
pub struct SnellOutbound {
    name: String,
    dialer: Arc<Dialer>,
    /// `CONNECT`, or `CONNECT_V2` with `reuse=true`.
    command: u8,
    /// `Some` with `reuse=true`.
    pool: Option<Arc<Pool>>,
    /// Started by the first connection: building an outbound (a dry build
    /// included) leaves no task behind.
    reaper: OnceLock<AbortOnDrop>,
}

impl SnellOutbound {
    pub fn new(
        name: &str,
        server: Target,
        spec: &SnellSpec,
        shadow_tls: Option<&ShadowTlsOpts>,
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> Result<SnellOutbound, BuildError> {
        // error texts carry no policy name: the registry's `build_one` and the
        // dry build both prefix it; the configuration checked both, but a
        // spec made by hand may not be
        if spec.psk.expose().is_empty() {
            return Err(BuildError::new("`psk` is empty"));
        }
        if spec.obfs.as_ref().is_some_and(|o| o.mode != ObfsMode::Http) {
            return Err(BuildError::new(
                "`snell` versions 4 and 5 take only `obfs=http`",
            ));
        }
        // no TLS of its own: the camouflage certificate is checked against
        // the server's name
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
        Ok(SnellOutbound {
            name: name.to_string(),
            dialer: Arc::new(Dialer {
                stack,
                psk: Psk::new(spec.psk.expose()),
            }),
            command: if spec.reuse { CONNECT_V2 } else { CONNECT },
            pool: spec.reuse.then(Arc::<Pool>::default),
            reaper: OnceLock::new(),
        })
    }

    async fn open(&self, head: Vec<u8>, opts: &ConnectOpts) -> Result<BoxedStream, OutboundError> {
        let Some(pool) = &self.pool else {
            let stream = self.dialer.fresh(opts).await?;
            return Ok(Box::new(SnellTunnel::new(stream, head, None)));
        };
        self.reaper.get_or_init(|| pool::spawn_reaper(pool));
        let back = Arc::downgrade(pool);
        if let Some(stream) = pool.take() {
            return Ok(Box::new(SnellTunnel::reused(
                stream,
                head,
                back,
                self.dialer.clone(),
                opts.clone(),
            )));
        }
        let stream = self.dialer.fresh(opts).await?;
        Ok(Box::new(SnellTunnel::new(stream, head, Some(back))))
    }
}

/// `01 command 00 host-length host port`: no client id, the host as text.
fn request_head(command: u8, target: &Target) -> Result<Vec<u8>, OutboundError> {
    let host = match &target.host {
        // an IPv6 literal without brackets
        HostName::Ip(ip) => ip.to_string(),
        HostName::Domain(name) => crate::hostname::to_ascii(name).ok_or_else(|| {
            OutboundError::Proxy("snell: the host name cannot be sent to the server".to_string())
        })?,
    };
    let len = u8::try_from(host.len()).map_err(|_| {
        OutboundError::Proxy("snell: the host name is longer than 255 bytes".to_string())
    })?;
    let mut head = vec![REQUEST_VERSION, command, 0, len];
    head.extend_from_slice(host.as_bytes());
    head.extend_from_slice(&target.port.to_be_bytes());
    Ok(head)
}

impl Outbound for SnellOutbound {
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
            let head = request_head(self.command, target)?;
            // one budget for the connection, Shadow TLS, obfs and the key;
            // the server answers with the target's first data, in the relay
            match tokio::time::timeout(opts.timeout, self.open(head, opts)).await {
                Ok(result) => result,
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeSnell, SnellScript, echo_server};
    use rurge_config::Span;
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::shadow_tls::read_shadow_tls;
    use rurge_config::spec::snell::read_snell;
    use rurge_config::spec::{ObfsOpts, ParamReader, Secret, SnellVersion};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// The outbound for `definition` (a `snell, host, port, ...` line).
    fn outbound(definition: &str) -> SnellOutbound {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let policy = parse_policy("N", definition, &span).unwrap();
        let mut r = ParamReader::new(&policy);
        let read = read_snell(&mut r);
        let shadow_tls = read_shadow_tls(&mut r);
        assert!(!r.has_errors(), "{:?}", r.finish());
        assert_eq!(read.not_implemented_version, None);
        SnellOutbound::new(
            "N",
            Target::new(policy.server.clone().unwrap(), policy.port.unwrap()),
            &read.spec,
            shadow_tls.as_ref(),
            Arc::new(RootCertStore::empty()),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .unwrap()
    }

    /// A version 4 outbound to `fake`, with `extra` parameters.
    fn outbound_to(fake: &FakeSnell, extra: &str) -> SnellOutbound {
        outbound(&format!(
            "snell, 127.0.0.1, {}, psk=secret, version=4{extra}",
            fake.addr().port()
        ))
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

    /// One whole request to the echo server: `text` there and back, then
    /// both sides end.
    async fn request(out: &SnellOutbound, echo: SocketAddr, text: &[u8]) {
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        roundtrip(&mut stream, text).await;
        stream.shutdown().await.unwrap();
        let mut rest = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut rest))
            .await
            .expect("the server's end arrives")
            .unwrap();
        assert!(rest.is_empty());
    }

    /// (connection, tunnel) of every request the fake saw.
    fn places(fake: &FakeSnell) -> Vec<(usize, usize)> {
        fake.requests()
            .iter()
            .map(|r| (r.connection, r.tunnel))
            .collect()
    }

    #[test]
    fn the_request_head_names_the_host_as_text() {
        let head = |host: &str| request_head(CONNECT_V2, &Target::new(HostName::parse(host), 443));
        // the payload of the byte-level notes' worked example
        assert_eq!(
            head("example.com").unwrap(),
            crate::vmess::vectors::hex("0105000b6578616d706c652e636f6d01bb")
        );
        let v6 = [&[1, 5, 0, 3][..], b"::1", &[1, 0xbb]].concat();
        assert_eq!(head("::1").unwrap(), v6, "no brackets");
        let v4 = request_head(CONNECT, &Target::new(HostName::parse("10.0.0.1"), 80)).unwrap();
        assert_eq!(v4, [&[1, 1, 0, 8][..], b"10.0.0.1", &[0, 80]].concat());
        assert_eq!(
            &head("bücher.example").unwrap()[4..25],
            b"xn--bcher-kva.example"
        );
    }

    #[tokio::test]
    async fn v4_and_v5_carry_the_head_with_the_first_payload() {
        let echo = echo_server().await;
        for version in [4, 5] {
            let fake = FakeSnell::spawn(SnellScript::new("secret")).await;
            let out = outbound(&format!(
                "snell, 127.0.0.1, {}, psk=secret, version={version}",
                fake.addr().port()
            ));
            assert_eq!(out.name(), "N");
            assert_eq!(out.udp(), crate::UdpSupport::Unsupported);
            let mut stream = out
                .connect_tcp(&target(echo), &ConnectOpts::default())
                .await
                .unwrap();
            roundtrip(&mut stream, b"hello through snell").await;
            roundtrip(&mut stream, b"and again").await;
            let seen = fake.requests();
            assert_eq!(seen.len(), 1, "v{version}");
            assert_eq!(
                (seen[0].command, seen[0].host.as_str(), seen[0].port),
                (CONNECT, "127.0.0.1", echo.port())
            );
            assert!(seen[0].client_id.is_empty());
            assert_eq!(seen[0].early, b"hello through snell", "one record");
            assert!((256..512).contains(&seen[0].padding), "{}", seen[0].padding);
        }
    }

    #[tokio::test]
    async fn without_reuse_every_request_has_a_connection_of_its_own() {
        let echo = echo_server().await;
        let fake = FakeSnell::spawn(SnellScript::new("secret")).await;
        let out = outbound_to(&fake, "");
        assert!(out.pool.is_none());
        for text in [&b"one"[..], b"two"] {
            request(&out, echo, text).await;
        }
        assert_eq!(fake.connections(), 2);
        assert_eq!(places(&fake), [(0, 0), (1, 0)]);
        assert!(fake.requests().iter().all(|r| r.command == CONNECT));
    }

    #[tokio::test]
    async fn with_reuse_one_connection_carries_the_requests_one_after_another() {
        let echo = echo_server().await;
        for version in [4, 5] {
            let fake = FakeSnell::spawn(SnellScript::new("secret")).await;
            let out = outbound(&format!(
                "snell, 127.0.0.1, {}, psk=secret, version={version}, reuse=true",
                fake.addr().port()
            ));
            for text in [&b"one"[..], b"two", b"three"] {
                request(&out, echo, text).await;
            }
            assert_eq!(fake.connections(), 1, "v{version}");
            assert_eq!(places(&fake), [(0, 0), (0, 1), (0, 2)]);
            let seen = fake.requests();
            assert!(seen.iter().all(|r| r.command == CONNECT_V2));
            assert_eq!(seen[1].early, b"two");
            // only a direction's first record is padded
            assert!((256..512).contains(&seen[0].padding));
            assert_eq!((seen[1].padding, seen[2].padding), (0, 0));
        }
    }

    #[tokio::test]
    async fn a_request_dropped_before_its_end_finishes_in_the_background_and_is_reused() {
        let echo = echo_server().await;
        let fake = FakeSnell::spawn(SnellScript::new("secret")).await;
        let out = outbound_to(&fake, ", reuse=true");
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        roundtrip(&mut stream, b"left open").await;
        drop(stream);
        let pool = out.pool.clone().unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while pool.len() == 0 {
            assert!(tokio::time::Instant::now() < deadline, "pooled again");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        request(&out, echo, b"next").await;
        assert_eq!(places(&fake), [(0, 0), (0, 1)]);
    }

    #[tokio::test]
    async fn a_request_without_its_answer_does_not_go_back_to_the_pool() {
        // accepts and says nothing: the fake answers only once it sends
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let silent = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((tcp, _)) = listener.accept().await {
                held.push(tcp);
            }
        });
        let fake = FakeSnell::spawn(SnellScript::new("secret")).await;
        let out = outbound_to(&fake, ", reuse=true");
        let mut stream = out
            .connect_tcp(&target(silent), &ConnectOpts::default())
            .await
            .unwrap();
        stream.write_all(b"hello").await.unwrap();
        drop(stream);
        let echo = echo_server().await;
        request(&out, echo, b"next").await;
        assert_eq!(fake.connections(), 2);
    }

    #[tokio::test]
    async fn a_pooled_connection_the_server_retired_is_retried_on_a_fresh_one() {
        let echo = echo_server().await;
        let fake = FakeSnell::spawn(SnellScript {
            tunnels_per_connection: Some(1),
            ..SnellScript::new("secret")
        })
        .await;
        let out = outbound_to(&fake, ", reuse=true");
        request(&out, echo, b"first").await;
        // taken from the pool; the server closes on the request
        request(&out, echo, b"second").await;
        assert_eq!(fake.unanswered(), 1);
        assert_eq!(fake.connections(), 2);
        assert_eq!(places(&fake), [(0, 0), (1, 0)]);
        // the fresh connection carried the head and the payload again
        let seen = fake.requests();
        assert_eq!(seen[1].early, b"second");
        assert!((256..512).contains(&seen[1].padding));
        // and was pooled in turn
        request(&out, echo, b"third").await;
        assert_eq!(places(&fake), [(0, 0), (1, 0), (2, 0)]);
        assert_eq!(fake.unanswered(), 2);
    }

    #[tokio::test]
    async fn the_servers_refusal_is_the_error_and_the_connection_is_not_reused() {
        let echo = echo_server().await;
        let fake = FakeSnell::spawn(SnellScript {
            refuse: Some((0x05, b"no such host\r\n".to_vec())),
            ..SnellScript::new("secret")
        })
        .await;
        let out = outbound_to(&fake, ", reuse=true");
        for _ in 0..2 {
            let mut stream = out
                .connect_tcp(&target(echo), &ConnectOpts::default())
                .await
                .unwrap();
            stream.write_all(b"hello").await.unwrap();
            let mut buf = [0u8; 16];
            let err = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
                .await
                .expect("the answer arrives")
                .unwrap_err();
            assert_eq!(err.to_string(), "snell: the server refused: no such host");
            assert_eq!(err.kind(), std::io::ErrorKind::ConnectionRefused);
        }
        assert_eq!(fake.connections(), 2);
    }

    #[tokio::test]
    async fn a_target_that_ends_before_sending_is_the_servers_remote_eof() {
        // accepts and closes at once
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let closer = listener.local_addr().unwrap();
        tokio::spawn(async move { while listener.accept().await.is_ok() {} });
        for (reuse, expected) in [
            ("true", "snell: the server refused: Remote EOF"),
            ("false", "snell: the server refused: end of file"),
        ] {
            let fake = FakeSnell::spawn(SnellScript::new("secret")).await;
            let out = outbound_to(&fake, &format!(", reuse={reuse}"));
            let mut stream = out
                .connect_tcp(&target(closer), &ConnectOpts::default())
                .await
                .unwrap();
            stream.write_all(b"hello").await.unwrap();
            let mut buf = Vec::new();
            let err = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut buf))
                .await
                .expect("the answer arrives")
                .unwrap_err();
            assert_eq!(err.to_string(), expected);
        }
    }

    #[tokio::test]
    async fn a_wrong_psk_is_a_connection_closed_without_an_answer() {
        let echo = echo_server().await;
        let fake = FakeSnell::spawn(SnellScript::new("right")).await;
        for reuse in ["false", "true"] {
            let out = outbound(&format!(
                "snell, 127.0.0.1, {}, psk=wrong, version=5, reuse={reuse}",
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
                "snell: the server closed the connection without answering"
            );
        }
        assert_eq!((fake.rejected(), fake.requests().len()), (2, 0));
    }

    #[tokio::test]
    async fn through_obfs_http_once_per_connection() {
        let echo = echo_server().await;
        let fake = FakeSnell::spawn(SnellScript {
            obfs_http: true,
            ..SnellScript::new("secret")
        })
        .await;
        let port = fake.addr().port();
        let out = outbound_to(&fake, ", reuse=true, obfs=http, obfs-host=cdn.example");
        request(&out, echo, b"behind the camouflage").await;
        request(&out, echo, b"on the same connection").await;
        let data = vec![0x5au8; 100_000];
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        roundtrip(&mut stream, &data).await;
        let hellos = fake.obfs_seen();
        assert_eq!(hellos.len(), 1);
        assert_eq!(hellos[0].host, format!("cdn.example:{port}"));
        assert_eq!(hellos[0].uri.as_deref(), Some("/"));
        assert_eq!(places(&fake), [(0, 0), (0, 1), (0, 2)]);
        assert_eq!(fake.requests()[0].early, b"behind the camouflage");
    }

    #[tokio::test]
    async fn names_go_out_as_a_labels_and_an_unsendable_name_never_dials() {
        let echo = echo_server().await;
        let fake = FakeSnell::spawn(SnellScript {
            connect_to: Some(echo),
            ..SnellScript::new("secret")
        })
        .await;
        let out = outbound_to(&fake, "");
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
            (seen[0].host.as_str(), seen[0].port),
            ("xn--bcher-kva.example", 443)
        );
        let before = fake.connections();
        for (name, expected) in [
            (
                "a@b.test".to_string(),
                "snell: the host name cannot be sent to the server",
            ),
            (
                "a".repeat(256),
                "snell: the host name is longer than 255 bytes",
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
    async fn a_large_payload_crosses_many_records_both_ways() {
        let echo = echo_server().await;
        let fake = FakeSnell::spawn(SnellScript::new("secret")).await;
        let out = outbound_to(&fake, "");
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
        assert_eq!(fake.largest_record(), record::MAX_PAYLOAD, "full records");
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
        for (reuse, connections) in [("false", 2), ("true", 1)] {
            let fake = FakeSnell::spawn(SnellScript::new("secret")).await;
            let out = outbound_to(&fake, &format!(", reuse={reuse}"));
            for _ in 0..2 {
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
                assert_eq!(answer, b"got 5", "reuse={reuse}");
            }
            assert_eq!(fake.connections(), connections, "reuse={reuse}");
        }
    }

    #[tokio::test]
    async fn a_silent_client_sends_its_head_alone_and_hears_the_target_first() {
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
        let fake = FakeSnell::spawn(SnellScript::new("secret")).await;
        let out = outbound_to(&fake, ", reuse=true");
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
        assert!(fake.requests()[0].early.is_empty());
    }

    #[test]
    fn what_cannot_be_built_is_a_build_error() {
        let build = |psk: &str, obfs: Option<ObfsOpts>| {
            SnellOutbound::new(
                "N",
                Target::new(HostName::parse("127.0.0.1"), 8388),
                &SnellSpec {
                    version: SnellVersion::V5,
                    psk: Secret::from(psk),
                    reuse: false,
                    udp_port: None,
                    obfs,
                },
                None,
                Arc::new(RootCertStore::empty()),
                Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
            )
            .map(|_| ())
            .map_err(|e| e.message)
        };
        assert_eq!(build("", None), Err("`psk` is empty".to_string()));
        let tls = ObfsOpts {
            mode: ObfsMode::Tls,
            host: None,
            uri: "/".into(),
        };
        assert_eq!(
            build("secret", Some(tls)),
            Err("`snell` versions 4 and 5 take only `obfs=http`".to_string())
        );
        assert_eq!(build("secret", None), Ok(()));
    }
}

```

`crates/rurge-proto/src/lib.rs`——把

```rust
pub mod shadowsocks;
// the outbound (M6b task 3) is its first user
#[allow(dead_code)]
```

换成

```rust
pub mod shadowsocks;
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-proto --lib snell`
Expected: FAIL——`pool` 与 `tunnel` 两个模块由 Step 3 写出，编译不过：

```text
error[E0583]: file not found for module `pool`
  --> crates\rurge-proto\src\snell\mod.rs:14:1
error[E0583]: file not found for module `tunnel`
  --> crates\rurge-proto\src\snell\mod.rs:16:1
error[E0282]: type annotations needed for `std::sync::Weak<_, _>`
   --> crates\rurge-proto\src\snell\mod.rs:117:13
Some errors have detailed explanations: E0282, E0583.
For more information about an error, try `rustc --explain E0282`.
error: could not compile `rurge-proto` (lib test) due to 3 previous errors
exit 101
```

- [ ] **Step 3: 实现**

`LazyHead` 泛化（已有调用处不变）：

`crates/rurge-proto/src/transport/lazy_head.rs`——把

```rust
pub struct LazyHead {
    inner: BoxedStream,
```

换成

```rust
/// Over a `BoxedStream` unless a protocol needs its own stream back
/// (`into_inner`: Snell's reused connections).
pub struct LazyHead<S = BoxedStream> {
    inner: S,
```

`crates/rurge-proto/src/transport/lazy_head.rs`——把

```rust
impl LazyHead {
    pub fn new(inner: BoxedStream, head: Vec<u8>) -> LazyHead {
```

换成

```rust
impl<S: AsyncRead + AsyncWrite + Unpin> LazyHead<S> {
    pub fn new(inner: S, head: Vec<u8>) -> LazyHead<S> {
```

`crates/rurge-proto/src/transport/lazy_head.rs`——把

```rust
    pub fn with_grace(inner: BoxedStream, head: Vec<u8>, grace: Duration) -> LazyHead {
```

换成

```rust
    pub fn with_grace(inner: S, head: Vec<u8>, grace: Duration) -> LazyHead<S> {
```

`crates/rurge-proto/src/transport/lazy_head.rs`——把

```rust
            reader: None,
        }
    }
```

换成

```rust
            reader: None,
        }
    }

    pub fn get_mut(&mut self) -> &mut S {
        &mut self.inner
    }

    /// The stream below; a head not yet sent is dropped with its payload.
    pub fn into_inner(self) -> S {
        self.inner
    }
```

`crates/rurge-proto/src/transport/lazy_head.rs`——把

```rust
impl AsyncRead for LazyHead {
```

换成

```rust
impl<S: AsyncRead + AsyncWrite + Unpin> AsyncRead for LazyHead<S> {
```

`crates/rurge-proto/src/transport/lazy_head.rs`——把

```rust
impl AsyncWrite for LazyHead {
```

换成

```rust
impl<S: AsyncRead + AsyncWrite + Unpin> AsyncWrite for LazyHead<S> {
```

新建 `crates/rurge-proto/src/snell/pool.rs`：

```rust
//! Idle Snell connections of one outbound (`reuse=true`, phase 2 M6 design
//! 4.3): the newest is reused first, at most `MAX_IDLE` are kept, and one
//! that has idled for a minute is closed (as anytls's pool).
//!
//! The server may close an idle connection at any moment; `take` skips one
//! whose close has already arrived, and a request on a connection that dies
//! before its answer is sent again on a fresh one (`tunnel`).

use super::record::SnellStream;
use crate::task::AbortOnDrop;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Waker};
use std::time::Duration;
use tokio::io::{AsyncRead, ReadBuf};
use tokio::time::Instant;

pub(crate) const REAP_EVERY: Duration = Duration::from_secs(30);
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// Idle connections kept per outbound; the oldest goes first.
pub(crate) const MAX_IDLE: usize = 8;

#[derive(Default)]
pub(crate) struct Pool {
    idle: Mutex<Vec<(SnellStream, Instant)>>,
}

impl Pool {
    /// Only a stream that `is_reusable`; it is taken back ready for the next
    /// request.
    pub(crate) fn put(&self, mut stream: SnellStream) {
        stream.next_tunnel();
        let mut idle = self.idle.lock().expect("pool");
        if idle.len() == MAX_IDLE {
            idle.remove(0);
        }
        idle.push((stream, Instant::now()));
    }

    /// The newest connection that has not idled too long and has heard
    /// nothing from the server since it was pooled; the others found on the
    /// way are dropped (the look happens outside the lock).
    pub(crate) fn take(&self) -> Option<SnellStream> {
        let now = Instant::now();
        loop {
            let (mut stream, since) = self.idle.lock().expect("pool").pop()?;
            if now.duration_since(since) < IDLE_TIMEOUT && is_quiet(&mut stream) {
                return Some(stream);
            }
        }
    }

    pub(crate) fn reap(&self, now: Instant) {
        self.idle
            .lock()
            .expect("pool")
            .retain(|(_, since)| now.duration_since(*since) < IDLE_TIMEOUT);
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.idle.lock().expect("pool").len()
    }
}

/// Between two requests the server says nothing: a read that is not
/// pending is its close, an error, or bytes out of turn. The read never
/// waits: a pending one leaves a waker that does nothing.
fn is_quiet(stream: &mut SnellStream) -> bool {
    let mut byte = [0u8; 1];
    let mut buf = ReadBuf::new(&mut byte);
    let mut cx = Context::from_waker(Waker::noop());
    Pin::new(stream).poll_read(&mut cx, &mut buf).is_pending()
}

/// Reaps `pool` until it is gone. Holds it weakly: the pool dies with its outbound.
pub(crate) fn spawn_reaper(pool: &Arc<Pool>) -> AbortOnDrop {
    let pool = Arc::downgrade(pool);
    AbortOnDrop(tokio::spawn(async move {
        let mut tick = tokio::time::interval(REAP_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let Some(pool) = pool.upgrade() else {
                return;
            };
            pool.reap(Instant::now());
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::super::kdf::{Psk, derive_key};
    use super::*;
    use tokio::io::AsyncReadExt;

    fn keyed(pipe: tokio::io::DuplexStream, salt: [u8; 16]) -> SnellStream {
        let key = derive_key(b"psk", &salt);
        SnellStream::new(Box::new(pipe), Psk::new("psk"), salt, key, Vec::new())
    }

    /// A client stream whose request has ended both ways, and the server's
    /// stream on the other end of the pipe (dropping it is the close).
    async fn ended() -> (SnellStream, SnellStream) {
        let (near, far) = tokio::io::duplex(4096);
        let (mut client, mut server) = (keyed(near, [7; 16]), keyed(far, [9; 16]));
        for side in [&mut client, &mut server] {
            std::future::poll_fn(|cx| side.poll_end(cx)).await.unwrap();
        }
        let mut buf = [0u8; 1];
        for side in [&mut client, &mut server] {
            assert_eq!(side.read(&mut buf).await.unwrap(), 0);
        }
        assert!(client.is_reusable());
        (client, server)
    }

    #[tokio::test(start_paused = true)]
    async fn an_idle_connection_expires_after_a_minute() {
        let pool = Pool::default();
        let (stream, _server) = ended().await;
        pool.put(stream);
        tokio::time::advance(IDLE_TIMEOUT - Duration::from_secs(1)).await;
        assert!(pool.take().is_some(), "still fresh");
        let (stream, _server) = ended().await;
        pool.put(stream);
        tokio::time::advance(IDLE_TIMEOUT).await;
        assert!(pool.take().is_none(), "expired");
        // the reaper's pass drops it too
        let (stream, _server) = ended().await;
        pool.put(stream);
        tokio::time::advance(IDLE_TIMEOUT).await;
        pool.reap(Instant::now());
        assert_eq!(pool.len(), 0);
    }

    #[tokio::test]
    async fn a_connection_the_server_closed_is_not_taken() {
        let pool = Pool::default();
        let (open, _server) = ended().await;
        let (closed, server) = ended().await;
        pool.put(open);
        pool.put(closed);
        drop(server);
        // the newest is closed: skipped and dropped
        assert!(pool.take().is_some());
        assert_eq!(pool.len(), 0);
    }

    #[tokio::test]
    async fn at_most_eight_idle_the_oldest_going_first() {
        let pool = Pool::default();
        let mut servers = Vec::new();
        for _ in 0..MAX_IDLE + 2 {
            let (stream, server) = ended().await;
            pool.put(stream);
            servers.push(server);
        }
        assert_eq!(pool.len(), MAX_IDLE);
        let mut buf = [0u8; 1];
        // the oldest was closed: its server reads the close
        let oldest = &mut servers[0];
        oldest.next_tunnel();
        assert_eq!(oldest.read(&mut buf).await.unwrap(), 0);
        // the newest is still open: nothing to read
        let newest = servers.last_mut().unwrap();
        newest.next_tunnel();
        let read = tokio::time::timeout(Duration::from_millis(50), newest.read(&mut buf)).await;
        assert!(read.is_err(), "still open");
    }
}
```

新建 `crates/rurge-proto/src/snell/tunnel.rs`：

```rust
//! One request on a Snell connection (phase 2 M6 design 4.3): the request
//! head waits in a `LazyHead` for the first payload, and the server's
//! answer — the first byte of its first payload of this request — is read
//! in front of the data:
//!
//! - `00` (tunnel): the rest is the target's data;
//! - `02 code length message` (error): `snell: the server refused: …`;
//! - anything else is an error.
//!
//! The server answers only once the target has sent something, so the
//! answer surfaces at the first read, never before a write.
//!
//! With `reuse=true` the application's shutdown sends only our empty
//! record, and a request whose two sides both ended cleanly hands its
//! connection back to the pool when dropped. Dropped earlier, the
//! connection finishes in the background: our end, then the server's data
//! discarded (at most `MAX_DISCARD` bytes, as Surge) up to its end.
//!
//! A pooled connection may have been closed by the server while it idled.
//! A request on one that fails before any answer arrived — a write error, or
//! a read that ends or fails — goes again, once, on a fresh connection,
//! with the head and every byte written so far (at most `MAX_REPLAY`).

use super::Dialer;
use super::pool::Pool;
use super::record::SnellStream;
use crate::OutboundError;
use crate::transport::lazy_head::LazyHead;
use rurge_net::BoxFuture;
use rurge_net::connector::ConnectOpts;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Weak};
use std::task::{Context, Poll, ready};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};

const TUNNEL: u8 = 0x00;
const ERROR: u8 = 0x02;

/// The longest part of an error message that is quoted.
const MAX_MESSAGE: usize = 200;
/// What a request may have written before its answer and still go again
/// on a fresh connection.
const MAX_REPLAY: usize = 64 * 1024;
/// Surge's limit on the server's data discarded while waiting for its end.
const MAX_DISCARD: usize = 0x80001;
/// How long a dropped request's connection may take to finish cleanly.
const FINISH_TIMEOUT: Duration = Duration::from_secs(10);

const NO_ANSWER: &str = "snell: the server closed the connection without answering";
const UNKNOWN_REPLY: &str = "snell: the server answered with an unknown reply";

fn no_answer() -> io::Error {
    io::Error::new(io::ErrorKind::UnexpectedEof, NO_ANSWER)
}

/// `code length message`, complete: the error for the application. The
/// message comes from the far end: printable ASCII only, and bounded.
fn refused(answer: &[u8]) -> io::Error {
    let message: String = answer[2..]
        .iter()
        .filter(|b| b.is_ascii_graphic() || **b == b' ')
        .take(MAX_MESSAGE)
        .map(|b| char::from(*b))
        .collect();
    let message = message.trim();
    let text = if message.is_empty() {
        format!("snell: the server refused: error {}", answer[0])
    } else {
        format!("snell: the server refused: {message}")
    };
    io::Error::new(io::ErrorKind::ConnectionRefused, text)
}

enum Conn {
    /// Boxed: the stream's buffers are large and a redial is rare.
    Open(Box<LazyHead<SnellStream>>),
    /// The request going again on a fresh connection: the dial, and what
    /// the old one carried (the head and the payload written since).
    Redial(BoxFuture<'static, io::Result<SnellStream>>, Vec<u8>),
    /// The redial failed.
    Gone,
}

enum Reply {
    Waiting,
    /// An error answer: its code, message length and message so far.
    Refused(Vec<u8>),
    Tunnel,
}

/// A pooled connection's request that may still go again.
struct Retry {
    /// The head and every payload byte written since.
    sent: Vec<u8>,
    dialer: Arc<Dialer>,
    opts: ConnectOpts,
}

/// No `Debug`: it holds the connection's keys.
pub(crate) struct SnellTunnel {
    conn: Conn,
    reply: Reply,
    /// `Some` while the request may go again on a fresh connection.
    retry: Option<Retry>,
    /// The application shut its side down: a fresh connection owes our end too.
    shut: bool,
    /// Our end still has to go out on the current connection.
    end_owed: bool,
    /// The connection is not in a state to be reused.
    broken: bool,
    /// `reuse=true`: where the connection goes after a clean end.
    pool: Option<Weak<Pool>>,
}

impl SnellTunnel {
    /// A request on a connection of its own, pooled afterwards with `pool`.
    pub(crate) fn new(stream: SnellStream, head: Vec<u8>, pool: Option<Weak<Pool>>) -> SnellTunnel {
        SnellTunnel {
            conn: Conn::Open(Box::new(LazyHead::new(stream, head))),
            reply: Reply::Waiting,
            retry: None,
            shut: false,
            end_owed: false,
            broken: false,
            pool,
        }
    }

    /// A request on a connection from `pool`: it may go again once, on a
    /// connection from `dialer`.
    pub(crate) fn reused(
        stream: SnellStream,
        head: Vec<u8>,
        pool: Weak<Pool>,
        dialer: Arc<Dialer>,
        opts: ConnectOpts,
    ) -> SnellTunnel {
        let retry = Retry {
            sent: head.clone(),
            dialer,
            opts,
        };
        let mut tunnel = SnellTunnel::new(stream, head, Some(pool));
        tunnel.retry = Some(retry);
        tunnel
    }

    /// `error` ended the current connection's request: `Ok` when it goes
    /// again on a fresh connection, else the error, final.
    fn fail(&mut self, error: io::Error) -> io::Result<()> {
        let Some(retry) = self.retry.take() else {
            self.broken = true;
            return Err(error);
        };
        let Retry { sent, dialer, opts } = retry;
        let dial = Box::pin(async move {
            match tokio::time::timeout(opts.timeout, dialer.fresh(&opts)).await {
                Ok(Ok(stream)) => Ok(stream),
                Ok(Err(OutboundError::Io(e))) => Err(e),
                Ok(Err(e)) => Err(io::Error::other(e.to_string())),
                Err(_) => Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "snell: connecting again timed out",
                )),
            }
        });
        self.conn = Conn::Redial(dial, sent);
        Ok(())
    }

    /// The current connection, once a redial has finished.
    fn poll_open(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.conn {
            Conn::Open(_) => Poll::Ready(Ok(())),
            Conn::Redial(dial, sent) => match ready!(dial.as_mut().poll(cx)) {
                Ok(stream) => {
                    // no grace: the application's bytes are already here
                    let sent = std::mem::take(sent);
                    self.conn =
                        Conn::Open(Box::new(LazyHead::with_grace(stream, sent, Duration::ZERO)));
                    self.end_owed = self.shut;
                    Poll::Ready(Ok(()))
                }
                Err(e) => {
                    self.conn = Conn::Gone;
                    self.broken = true;
                    Poll::Ready(Err(e))
                }
            },
            Conn::Gone => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "snell: the connection is gone",
            ))),
        }
    }

    fn lazy(&mut self) -> &mut LazyHead<SnellStream> {
        match &mut self.conn {
            Conn::Open(lazy) => lazy,
            _ => unreachable!("after poll_open"),
        }
    }

    /// Sends our end: the empty record alone when the connection may be
    /// reused, else the connection's shutdown too.
    fn poll_end(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let reuse = self.pool.is_some();
        let lazy = self.lazy();
        if !reuse {
            return Pin::new(lazy).poll_shutdown(cx);
        }
        ready!(Pin::new(&mut *lazy).poll_flush(cx))?;
        lazy.get_mut().poll_end(cx)
    }

    /// The answer, read ahead of the data. `Ready(Ok)` once it said tunnel.
    fn poll_reply(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        loop {
            let need = match &self.reply {
                Reply::Tunnel => return Poll::Ready(Ok(())),
                Reply::Waiting => 1,
                Reply::Refused(got) if got.len() < 2 => 2 - got.len(),
                Reply::Refused(got) => 2 + usize::from(got[1]) - got.len(),
            };
            let mut space = [0u8; 256];
            let mut buf = ReadBuf::new(&mut space[..need]);
            let read = ready!(Pin::new(self.lazy()).poll_read(cx, &mut buf));
            let got = buf.filled();
            match (read, &mut self.reply) {
                (Err(e), _) => return Poll::Ready(Err(e)),
                (Ok(()), _) if got.is_empty() => return Poll::Ready(Err(no_answer())),
                (Ok(()), Reply::Waiting) => match got[0] {
                    TUNNEL => {
                        self.reply = Reply::Tunnel;
                        self.retry = None;
                    }
                    ERROR => {
                        self.reply = Reply::Refused(Vec::new());
                        self.retry = None;
                    }
                    _ => {
                        self.retry = None;
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            UNKNOWN_REPLY,
                        )));
                    }
                },
                (Ok(()), Reply::Refused(answer)) => {
                    answer.extend_from_slice(got);
                    if answer.len() >= 2 && answer.len() == 2 + usize::from(answer[1]) {
                        return Poll::Ready(Err(refused(answer)));
                    }
                }
                (Ok(()), Reply::Tunnel) => unreachable!("returned above"),
            }
        }
    }

    /// `f` on the current connection; a failure that lets the request go
    /// again starts the redial and tries again.
    fn drive<T>(
        &mut self,
        cx: &mut Context<'_>,
        mut f: impl FnMut(&mut SnellTunnel, &mut Context<'_>) -> Poll<io::Result<T>>,
    ) -> Poll<io::Result<T>> {
        loop {
            ready!(self.poll_open(cx))?;
            if self.end_owed {
                match ready!(self.poll_end(cx)) {
                    Ok(()) => self.end_owed = false,
                    Err(e) => {
                        self.fail(e)?;
                        continue;
                    }
                }
            }
            match ready!(f(self, cx)) {
                Ok(value) => return Poll::Ready(Ok(value)),
                Err(e) => self.fail(e)?,
            }
        }
    }
}

impl AsyncRead for SnellTunnel {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.drive(cx, |this, cx| this.poll_reply(cx)))?;
        let read = ready!(Pin::new(this.lazy()).poll_read(cx, buf));
        if read.is_err() {
            this.broken = true;
        }
        Poll::Ready(read)
    }
}

impl AsyncWrite for SnellTunnel {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let n = ready!(this.drive(cx, |this, cx| Pin::new(this.lazy()).poll_write(cx, data)))?;
        if let Some(retry) = &mut this.retry {
            if retry.sent.len() + n > MAX_REPLAY {
                this.retry = None;
            } else {
                retry.sent.extend_from_slice(&data[..n]);
            }
        }
        Poll::Ready(Ok(n))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.get_mut()
            .drive(cx, |this, cx| Pin::new(this.lazy()).poll_flush(cx))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if !this.shut {
            this.shut = true;
            this.end_owed = true;
        }
        // `drive` sends the end owed
        this.drive(cx, |_, _| Poll::Ready(Ok(())))
    }
}

impl Drop for SnellTunnel {
    fn drop(&mut self) {
        let Some(pool) = self.pool.as_ref().and_then(Weak::upgrade) else {
            return;
        };
        // without the tunnel answer, or after an error, where the
        // connection stands is unknown
        if self.broken || !matches!(self.reply, Reply::Tunnel) {
            return;
        }
        let Conn::Open(lazy) = std::mem::replace(&mut self.conn, Conn::Gone) else {
            return;
        };
        let stream = (*lazy).into_inner();
        if stream.is_reusable() {
            pool.put(stream);
        } else if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(finish(stream, Arc::downgrade(&pool)));
        }
    }
}

/// Ends our side (once more is harmless), then reads the server's side to
/// its end; a connection that gets there cleanly goes back to `pool`.
async fn finish(mut stream: SnellStream, pool: Weak<Pool>) {
    let ended = tokio::time::timeout(FINISH_TIMEOUT, async {
        std::future::poll_fn(|cx| stream.poll_end(cx)).await?;
        let mut sink = vec![0u8; 16 * 1024];
        let mut discarded = 0;
        loop {
            // 0: the server's end, or its close (`is_reusable` tells them apart)
            let n = stream.read(&mut sink).await?;
            if n == 0 {
                return Ok(());
            }
            discarded += n;
            if discarded > MAX_DISCARD {
                return Err(io::Error::other("snell: too much data after our end"));
            }
        }
    })
    .await;
    if matches!(ended, Ok(Ok(())))
        && stream.is_reusable()
        && let Some(pool) = pool.upgrade()
    {
        pool.put(stream);
    }
}

#[cfg(test)]
mod tests {
    use super::super::kdf::Psk;
    use super::*;
    use tokio::io::AsyncWriteExt;

    /// A request on one end of a pipe and the server's stream on the other
    /// (the record format is symmetric).
    async fn pair(pool: Option<Weak<Pool>>) -> (SnellTunnel, SnellStream) {
        let (near, far) = tokio::io::duplex(64 * 1024);
        let client = SnellStream::open(Box::new(near), Psk::new("psk"))
            .await
            .unwrap();
        let server = SnellStream::open(Box::new(far), Psk::new("psk"))
            .await
            .unwrap();
        (SnellTunnel::new(client, b"HEAD".to_vec(), pool), server)
    }

    /// What the tunnel reads after the server sent `records`, one write each.
    async fn answered(records: &[&[u8]]) -> io::Result<Vec<u8>> {
        let (mut tunnel, mut server) = pair(None).await;
        tunnel.write_all(b"ping").await.unwrap();
        let mut request = [0u8; 8];
        server.read_exact(&mut request).await.unwrap();
        assert_eq!(&request, b"HEADping", "one record");
        for record in records {
            server.write_all(record).await.unwrap();
        }
        server.shutdown().await.unwrap();
        let mut got = Vec::new();
        tunnel.read_to_end(&mut got).await.map(|_| got)
    }

    #[tokio::test]
    async fn the_tunnel_answer_is_taken_off_the_data() {
        assert_eq!(answered(&[b"\x00pong"]).await.unwrap(), b"pong");
        assert_eq!(answered(&[b"\x00", b"po", b"ng"]).await.unwrap(), b"pong");
        assert_eq!(answered(&[b"\x00"]).await.unwrap(), b"");
    }

    #[tokio::test]
    async fn an_error_answer_quotes_the_servers_message_made_safe() {
        let text = |result: io::Result<Vec<u8>>| result.unwrap_err().to_string();
        assert_eq!(
            text(answered(&[b"\x02\x65\x0aRemote EOF"]).await),
            "snell: the server refused: Remote EOF"
        );
        // cut over several records
        assert_eq!(
            text(answered(&[b"\x02", b"\x01\x03", b"a", b"bc"]).await),
            "snell: the server refused: abc"
        );
        assert_eq!(
            text(answered(&[b"\x02\x07\x00"]).await),
            "snell: the server refused: error 7"
        );
        let mut long = vec![0x02, 0x01, 255, 0x1b];
        long.extend_from_slice(b"[31m");
        long.extend(std::iter::repeat_n(b'x', 250));
        assert_eq!(
            text(answered(&[&long]).await),
            format!("snell: the server refused: [31m{}", "x".repeat(196))
        );
    }

    #[tokio::test]
    async fn anything_else_is_an_unknown_answer_or_none() {
        let err = answered(&[b"\x07data"]).await.unwrap_err();
        assert_eq!(err.to_string(), UNKNOWN_REPLY);
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        // the server's end without an answer
        let err = answered(&[]).await.unwrap_err();
        assert_eq!(err.to_string(), NO_ANSWER);
    }

    #[tokio::test]
    async fn only_a_request_that_ended_cleanly_both_ways_is_pooled() {
        let pool = Arc::new(Pool::default());
        // both ends: pooled at once
        let (mut tunnel, mut server) = pair(Some(Arc::downgrade(&pool))).await;
        tunnel.write_all(b"ping").await.unwrap();
        tunnel.shutdown().await.unwrap();
        let mut request = Vec::new();
        server.read_to_end(&mut request).await.unwrap();
        assert_eq!(request, b"HEADping");
        server.write_all(b"\x00pong").await.unwrap();
        std::future::poll_fn(|cx| server.poll_end(cx))
            .await
            .unwrap();
        let mut got = Vec::new();
        tunnel.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, b"pong");
        drop(tunnel);
        assert_eq!(pool.len(), 1);
        // no answer yet: dropped
        let (mut tunnel, _server) = pair(Some(Arc::downgrade(&pool))).await;
        tunnel.write_all(b"ping").await.unwrap();
        drop(tunnel);
        // a refusal: dropped
        let (mut tunnel, mut server) = pair(Some(Arc::downgrade(&pool))).await;
        tunnel.write_all(b"ping").await.unwrap();
        server.write_all(b"\x02\x01\x00").await.unwrap();
        let mut buf = [0u8; 4];
        assert!(tunnel.read(&mut buf).await.is_err());
        drop(tunnel);
        assert_eq!(pool.len(), 1);
    }
}
```

要点：
- 池的锁里只做 `Vec` 的进出；取出时的探测在锁外。
- `SnellStream` 约 2.5 KB，`Conn::Open` 装箱（clippy 的 `large_enum_variant`）。
- 建出站时的兜底检查（配置层已经检查过）：`psk` 为空 → `` `psk` is empty ``；obfs 不是 `http` → `` `snell` versions 4 and 5 take only `obfs=http` ``。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-proto --lib snell` → 通过（新增 23 条：v4 / v5 的往返、不复用时每个请求一条新连接且命令字 0x01、复用时命令字 0x05 且顺序的请求共用一条连接、陈旧的池连接在新连接上重试、池的空闲回收、拒绝的说明、未知应答、psk 错、obfs http、IDN 与发不出去的名字、大负载、半关闭等）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-proto
git commit -m "feat(proto): Snell 的 TCP——请求与应答、复用池、陈旧连接的一次重试；FakeSnell"
```

### Task 4: UDP over TCP

`SnellOutbound::udp()` 为 `Native`，`open_udp` 开一条新连接发 `01 06 00`、等 `00`，载体 `SnellUdp` 在记录之上按包收发（P10），`udp-port` 是这条连接的端口（P11）。`FakeSnell` 加 UDP（独立的编解码，全锥，可注入非数据报的记录）。

**Files:**
- Create: `crates/rurge-proto/src/snell/udp.rs`
- Modify: `crates/rurge-proto/src/snell/mod.rs`（与用例）、`record.rs`（`poll_record`，与用例）、`tunnel.rs`、`src/testing/snell.rs`

**Interfaces:**
- Consumes: Task 2 / 3 的 `SnellStream`、`Dialer`、应答解析；M5a 的 `PacketSocket`。
- Produces: `SnellStream::poll_record`（一次读一整条记录）；`pub(crate) struct SnellUdp`（实现 `PacketSocket`）；`FakeSnell` 的 `udp_junk`、`datagrams()`、`udp_outside()`

- [ ] **Step 1: 先写用例（连同假服务端的 UDP）**

`crates/rurge-proto/src/testing/snell.rs`——把

```rust
//!
//! A wrong PSK gets no word back, only the close.
```

换成

```rust
//!
//! UDP (`06`) is answered `00` at once; then every record of the client's
//! is one datagram, sent from a loopback socket of the connection's own,
//! and whatever reaches that socket goes back as `04 IPv4 port payload`
//! (full cone). Names are never resolved: a datagram for one goes to
//! `connect_to`, or nowhere.
//!
//! A wrong PSK gets no word back, only the close.
```

`crates/rurge-proto/src/testing/snell.rs`——把

```rust
use std::net::{IpAddr, SocketAddr};
```

换成

```rust
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
```

`crates/rurge-proto/src/testing/snell.rs`——把

```rust
use tokio::net::{TcpListener, TcpStream};
```

换成

```rust
use tokio::net::{TcpListener, TcpStream, UdpSocket};
```

`crates/rurge-proto/src/testing/snell.rs`——把

```rust
const CONNECT_V2: u8 = 0x05;
```

换成

```rust
const CONNECT_V2: u8 = 0x05;
const UDP: u8 = 0x06;
```

`crates/rurge-proto/src/testing/snell.rs`——把

```rust
    pub refuse: Option<(u8, Vec<u8>)>,
```

换成

```rust
    pub refuse: Option<(u8, Vec<u8>)>,
    /// In front of every UDP answer, two records that are no datagram: an
    /// unknown address family and an address cut short.
    pub udp_junk: bool,
```

`crates/rurge-proto/src/testing/snell.rs`——把

```rust
            refuse: None,
```

换成

```rust
            refuse: None,
            udp_junk: false,
```

`crates/rurge-proto/src/testing/snell.rs`——把

```rust
    /// `01` Connect, `05` ConnectV2.
    pub command: u8,
    pub client_id: Vec<u8>,
    /// Exactly as it was on the wire (an IP literal is text too).
    pub host: String,
    pub port: u16,
    /// Payload after the request, in the same record.
```

换成

```rust
    /// `01` Connect, `05` ConnectV2, `06` UDP.
    pub command: u8,
    pub client_id: Vec<u8>,
    /// Exactly as it was on the wire (an IP literal is text too); UDP has
    /// no target (empty, port 0).
    pub host: String,
    pub port: u16,
    /// Payload after the request, in the same record (UDP: after the id).
```

`crates/rurge-proto/src/testing/snell.rs`——把

```rust
    largest_record: AtomicUsize,
```

换成

```rust
    largest_record: AtomicUsize,
    /// Every UDP datagram's target, `host:port`, the name as on the wire.
    datagrams: Mutex<Vec<String>>,
    /// Each UDP connection's own socket, in the order they opened.
    udp_outside: Mutex<Vec<SocketAddr>>,
```

`crates/rurge-proto/src/testing/snell.rs`——把

```rust
/// `01 command id-length id host-length host port early`.
```

换成

```rust
/// `01 command id-length id host-length host port early`, UDP's
/// `01 06 id-length id`.
```

`crates/rurge-proto/src/testing/snell.rs`——把

```rust
    let command = *p.get(1)?;
```

换成

```rust
    let command = *p.get(1)?;
    let id_len = usize::from(*p.get(2)?);
    let client_id = p.get(3..3 + id_len)?.to_vec();
    let at = 3 + id_len;
    if command == UDP {
        return Some(RecordedSnell {
            command,
            client_id,
            host: String::new(),
            port: 0,
            early: p[at..].to_vec(),
            connection: 0,
            tunnel: 0,
            padding: 0,
        });
    }
```

`crates/rurge-proto/src/testing/snell.rs`——把

```rust
        return None;
    }
    let id_len = usize::from(*p.get(2)?);
    let client_id = p.get(3..3 + id_len)?.to_vec();
    let at = 3 + id_len;
```

换成

```rust
        return None;
    }
```

`crates/rurge-proto/src/testing/snell.rs`——把

```rust
    })
}
```

换成

```rust
    })
}

/// A client datagram, `01 host-length host port payload` or
/// `01 00 04|06 address port payload`: `(host, port, payload)`.
fn parse_datagram(p: &[u8]) -> Option<(String, u16, &[u8])> {
    if *p.first()? != 0x01 {
        return None;
    }
    let (host, at) = match *p.get(1)? {
        0 => match *p.get(2)? {
            4 => {
                let b: [u8; 4] = p.get(3..7)?.try_into().ok()?;
                (Ipv4Addr::from(b).to_string(), 7)
            }
            6 => {
                let b: [u8; 16] = p.get(3..19)?.try_into().ok()?;
                (Ipv6Addr::from(b).to_string(), 19)
            }
            _ => return None,
        },
        len => {
            let len = usize::from(len);
            (
                String::from_utf8_lossy(p.get(2..2 + len)?).into_owned(),
                2 + len,
            )
        }
    };
    let port = u16::from_be_bytes(p.get(at..at + 2)?.try_into().ok()?);
    Some((host, port, &p[at + 2..]))
}
```

`crates/rurge-proto/src/testing/snell.rs`——把

```rust
        client_ended && server_ended
    }
}

async fn serve(tcp: TcpStream, shared: Arc<Shared>, connection: usize) -> io::Result<()> {
```

换成

```rust
        client_ended && server_ended
    }
}

impl Shared {
    /// UDP on this connection until either side ends.
    async fn udp(
        &self,
        reader: &mut Reader,
        up: &mut Direction,
        writer: &mut Writer,
        answers: &mut Answers,
    ) -> io::Result<()> {
        let socket = UdpSocket::bind("127.0.0.1:0").await?;
        self.seen
            .udp_outside
            .lock()
            .expect("outside")
            .push(socket.local_addr()?);
        // the answer at once, in the direction's first (padded) record
        writer.write_all(&answers.record(&[0x00])).await?;
        let datagrams = async {
            while let Some(Record::Data { payload, .. }) =
                read_record(reader, up, &self.seen).await?
            {
                let (host, port, data) =
                    parse_datagram(&payload).ok_or_else(|| bad("a record that is no datagram"))?;
                self.seen
                    .datagrams
                    .lock()
                    .expect("datagrams")
                    .push(format!("{host}:{port}"));
                let to = match (host.parse::<IpAddr>(), self.script.connect_to) {
                    (Ok(ip), _) => SocketAddr::new(ip, port),
                    (Err(_), Some(addr)) => addr,
                    // never resolves: a name without `connect_to` is a dead end
                    (Err(_), None) => continue,
                };
                socket.send_to(data, to).await?;
            }
            Ok::<(), io::Error>(())
        };
        let replies = async {
            let mut buf = vec![0u8; 65536];
            loop {
                let (n, from) = match socket.recv_from(&mut buf).await {
                    Ok(got) => got,
                    // an ICMP "unreachable" for an earlier datagram (Windows)
                    Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
                    Err(e) => return Err(e),
                };
                if self.script.udp_junk {
                    writer
                        .write_all(&answers.record(&[5, 1, 2, 3, 4, 0, 53, b'x']))
                        .await?;
                    writer.write_all(&answers.record(&[4, 1, 2])).await?;
                }
                let mut datagram = match from.ip() {
                    IpAddr::V4(ip) => [&[4][..], &ip.octets()].concat(),
                    IpAddr::V6(ip) => [&[6][..], &ip.octets()].concat(),
                };
                datagram.extend_from_slice(&from.port().to_be_bytes());
                let room = MAX_PAYLOAD - datagram.len();
                datagram.extend_from_slice(&buf[..n.min(room)]);
                writer.write_all(&answers.record(&datagram)).await?;
            }
        };
        tokio::select! {
            done = datagrams => done,
            done = replies => done,
        }
    }
}

async fn serve(tcp: TcpStream, shared: Arc<Shared>, connection: usize) -> io::Result<()> {
```

`crates/rurge-proto/src/testing/snell.rs`——把

```rust
            writer.write_all(&answers.error(*code, message)).await?;
```

换成

```rust
            writer.write_all(&answers.error(*code, message)).await?;
            close(&mut reader, &mut writer).await;
            return Ok(());
        }
        if request.command == UDP {
            let _ = shared
                .udp(&mut reader, &mut up, &mut writer, &mut answers)
                .await;
```

`crates/rurge-proto/src/testing/snell.rs`——把

```rust
        self.shared.seen.unanswered.load(Ordering::SeqCst)
    }

    /// The longest payload of any client record so far.
```

换成

```rust
        self.shared.seen.unanswered.load(Ordering::SeqCst)
    }

    /// Every UDP datagram's target, `host:port`, in arrival order.
    pub fn datagrams(&self) -> Vec<String> {
        self.shared
            .seen
            .datagrams
            .lock()
            .expect("datagrams")
            .clone()
    }

    /// Where each UDP connection sends from: a datagram to one of these goes
    /// back to its client.
    pub fn udp_outside(&self) -> Vec<SocketAddr> {
        self.shared
            .seen
            .udp_outside
            .lock()
            .expect("outside")
            .clone()
    }

    /// The longest payload of any client record so far.
```

`crates/rurge-proto/src/snell/record.rs`——把

```rust
        assert!(!client.is_reusable() && !server.is_reusable());
    }

    /// Moves exactly `len` bytes from one wire to the other: the records
```

换成

```rust
        assert!(!client.is_reusable() && !server.is_reusable());
    }

    #[tokio::test]
    async fn a_record_is_read_whole_and_the_end_is_none() {
        let (mut client, mut server) = pair(64 * 1024);
        client.write_all(b"one").await.unwrap();
        client.write_all(b"two, longer").await.unwrap();
        client.write_all(b"three").await.unwrap();
        poll_fn(|cx| client.poll_end(cx)).await.unwrap();
        for expected in [&b"one"[..], b"two, longer"] {
            let record = poll_fn(|cx| server.poll_record(cx)).await.unwrap();
            assert_eq!(record.unwrap(), expected);
        }
        // what a byte read left of a record comes whole
        let mut first = [0u8; 2];
        server.read_exact(&mut first).await.unwrap();
        assert_eq!(&first, b"th");
        assert_eq!(
            poll_fn(|cx| server.poll_record(cx)).await.unwrap().unwrap(),
            b"ree"
        );
        assert_eq!(poll_fn(|cx| server.poll_record(cx)).await.unwrap(), None);
        assert!(server.read_ended());
    }

    /// Moves exactly `len` bytes from one wire to the other: the records
```

`crates/rurge-proto/src/snell/mod.rs`——把

```rust
    use crate::testing::{FakeSnell, SnellScript, echo_server};
```

换成

```rust
    use crate::testing::{FakeSnell, SnellScript, echo_server, udp_echo_server};
```

`crates/rurge-proto/src/snell/mod.rs`——把

```rust
    use rurge_net::connector::{DirectConnector, SystemResolve};
```

换成

```rust
    use rurge_net::connector::{DirectConnector, PacketSocket, SystemResolve};
```

`crates/rurge-proto/src/snell/mod.rs`——把

```rust
            assert_eq!(out.udp(), crate::UdpSupport::Unsupported);
```

换成

```rust
            assert_eq!(out.udp(), UdpSupport::Native);
```

`crates/rurge-proto/src/snell/mod.rs`——把

```rust
        assert_eq!(build("secret", None), Ok(()));
    }
}
```

换成

```rust
        assert_eq!(build("secret", None), Ok(()));
    }

    async fn udp_answer(carrier: &dyn PacketSocket) -> (Vec<u8>, Target) {
        let mut buf = vec![0u8; 65536];
        let (n, from) = tokio::time::timeout(Duration::from_secs(10), carrier.recv_from(&mut buf))
            .await
            .expect("an answer within the bound")
            .unwrap();
        (buf[..n].to_vec(), from)
    }

    async fn udp_roundtrip(carrier: &dyn PacketSocket, to: SocketAddr, payload: &[u8]) {
        carrier.send_to(payload, &target(to)).await.unwrap();
        assert_eq!(udp_answer(carrier).await, (payload.to_vec(), target(to)));
    }

    /// v4 and v5 alike, without `udp-relay`: one connection asks for UDP,
    /// and each datagram names its target.
    #[tokio::test]
    async fn udp_goes_through_one_connection_to_any_target() {
        let (one, two) = (udp_echo_server().await, udp_echo_server().await);
        for version in [4, 5] {
            let fake = FakeSnell::spawn(SnellScript {
                connect_to: Some(two),
                ..SnellScript::new("secret")
            })
            .await;
            let out = outbound(&format!(
                "snell, 127.0.0.1, {}, psk=secret, version={version}",
                fake.addr().port()
            ));
            assert_eq!(out.udp(), UdpSupport::Native);
            let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
            udp_roundtrip(carrier.as_ref(), one, b"to one").await;
            // a name goes to the server as its A-labels; the answer names
            // its source by address
            let name = Target::new(HostName::Domain("bücher.example".into()), 53);
            carrier.send_to(b"by name", &name).await.unwrap();
            assert_eq!(
                udp_answer(carrier.as_ref()).await,
                (b"by name".to_vec(), target(two))
            );
            let seen = fake.requests();
            assert_eq!(seen.len(), 1, "v{version}");
            assert_eq!(seen[0].command, udp::UDP);
            assert!(seen[0].client_id.is_empty() && seen[0].early.is_empty());
            assert!((256..512).contains(&seen[0].padding), "{}", seen[0].padding);
            assert_eq!(
                fake.datagrams(),
                [one.to_string(), "xn--bcher-kva.example:53".to_string()]
            );
            assert_eq!(fake.connections(), 1);
        }
    }

    /// Full cone: whoever reaches the server's socket is heard, under its
    /// own address.
    #[tokio::test]
    async fn anyone_may_answer_through_snell() {
        let echo = udp_echo_server().await;
        let fake = FakeSnell::spawn(SnellScript::new("secret")).await;
        let out = outbound_to(&fake, "");
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        udp_roundtrip(carrier.as_ref(), echo, b"hello").await;
        let stranger = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        stranger
            .send_to(b"unasked", fake.udp_outside()[0])
            .await
            .unwrap();
        assert_eq!(
            udp_answer(carrier.as_ref()).await,
            (b"unasked".to_vec(), target(stranger.local_addr().unwrap()))
        );
    }

    #[tokio::test]
    async fn a_servers_record_that_is_no_datagram_is_dropped() {
        let echo = udp_echo_server().await;
        let fake = FakeSnell::spawn(SnellScript {
            udp_junk: true,
            ..SnellScript::new("secret")
        })
        .await;
        let out = outbound_to(&fake, "");
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        udp_roundtrip(carrier.as_ref(), echo, b"first").await;
        udp_roundtrip(carrier.as_ref(), echo, b"second").await;
    }

    #[tokio::test]
    async fn a_datagram_longer_than_a_record_is_refused_and_nothing_is_sent() {
        let echo = udp_echo_server().await;
        let fake = FakeSnell::spawn(SnellScript::new("secret")).await;
        let out = outbound_to(&fake, "");
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        let err = carrier
            .send_to(&vec![0u8; 16_375], &target(echo))
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "snell: a datagram longer than 16374 bytes");
        // the largest one fits a record exactly
        let largest = vec![7u8; 16_374];
        udp_roundtrip(carrier.as_ref(), echo, &largest).await;
        assert_eq!(fake.datagrams(), [echo.to_string()]);
        assert_eq!(fake.largest_record(), record::MAX_PAYLOAD);
    }

    /// `udp-port` is where the UDP session's connection goes, through the
    /// same layers; TCP keeps the policy's port.
    #[tokio::test]
    async fn udp_goes_to_the_udp_port() {
        // the policy's port accepts and says nothing
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let silent = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((tcp, _)) = listener.accept().await {
                held.push(tcp);
            }
        });
        let echo = udp_echo_server().await;
        let fake = FakeSnell::spawn(SnellScript {
            obfs_http: true,
            ..SnellScript::new("secret")
        })
        .await;
        let out = outbound(&format!(
            "snell, 127.0.0.1, {}, psk=secret, version=5, obfs=http, udp-port={}",
            silent.port(),
            fake.addr().port()
        ));
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        udp_roundtrip(carrier.as_ref(), echo, b"via udp-port").await;
        assert_eq!(fake.connections(), 1);
        // the camouflage names the port the connection went to
        let host = format!("127.0.0.1:{}", fake.addr().port());
        assert_eq!(fake.obfs_seen()[0].host, host);
    }

    #[tokio::test]
    async fn udp_has_a_connection_of_its_own_never_pooled() {
        let (tcp_echo, echo) = (echo_server().await, udp_echo_server().await);
        let fake = FakeSnell::spawn(SnellScript::new("secret")).await;
        let out = outbound_to(&fake, ", reuse=true");
        request(&out, tcp_echo, b"pooled").await;
        let pool = out.pool.clone().unwrap();
        assert_eq!(pool.len(), 1);
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        udp_roundtrip(carrier.as_ref(), echo, b"fresh").await;
        drop(carrier);
        assert_eq!(fake.connections(), 2);
        assert_eq!(
            pool.len(),
            1,
            "UDP took nothing from the pool, gave nothing back"
        );
        assert_eq!(places(&fake), [(0, 0), (1, 0)]);
    }

    #[tokio::test]
    async fn the_servers_refusal_of_udp_fails_the_open() {
        let fake = FakeSnell::spawn(SnellScript {
            refuse: Some((0x07, b"udp disabled".to_vec())),
            ..SnellScript::new("secret")
        })
        .await;
        let out = outbound_to(&fake, "");
        let err = out
            .open_udp(&ConnectOpts::default())
            .await
            .err()
            .expect("refused");
        assert_eq!(err.to_string(), "snell: the server refused: udp disabled");
        // a server that never answers is the open's time-out
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let silent = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((tcp, _)) = listener.accept().await {
                held.push(tcp);
            }
        });
        let out = outbound(&format!(
            "snell, 127.0.0.1, {}, psk=secret, version=4",
            silent.port()
        ));
        let opts = ConnectOpts {
            timeout: Duration::from_millis(300),
        };
        let err = out.open_udp(&opts).await.err().expect("times out");
        assert!(matches!(err, OutboundError::Timeout), "{err}");
    }
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-proto --lib snell`
Expected: FAIL——`poll_record` 与 `udp()` 由 Step 3 引入，编译不过（节选）：

```text
error[E0433]: failed to resolve: use of undeclared type `UdpSupport`
   --> crates\rurge-proto\src\snell\mod.rs:285:35
error[E0433]: failed to resolve: use of undeclared type `UdpSupport`
   --> crates\rurge-proto\src\snell\mod.rs:719:35
error[E0599]: no method named `poll_record` found for struct `snell::record::SnellStream` in the current scope
   --> crates\rurge-proto\src\snell\record.rs:628:46
   --> crates\rurge-proto\src\snell\record.rs:297:5
error[E0599]: no method named `poll_record` found for struct `snell::record::SnellStream` in the current scope
   --> crates\rurge-proto\src\snell\record.rs:636:33
   --> crates\rurge-proto\src\snell\record.rs:297:5
error[E0599]: no method named `poll_record` found for struct `snell::record::SnellStream` in the current scope
   --> crates\rurge-proto\src\snell\record.rs:639:40
   --> crates\rurge-proto\src\snell\record.rs:297:5
error[E0433]: failed to resolve: use of unresolved module or unlinked crate `udp`
   --> crates\rurge-proto\src\snell\mod.rs:732:41
Some errors have detailed explanations: E0433, E0599.
For more information about an error, try `rustc --explain E0433`.
error: could not compile `rurge-proto` (lib test) due to 6 previous errors
exit 101
```

- [ ] **Step 3: 实现**

`crates/rurge-proto/src/snell/record.rs`——把

```rust
        Poll::Ready(self.check(flushed))
```

换成

```rust
        Poll::Ready(self.check(flushed))
    }

    /// The rest of the current record's payload, whole, or the next
    /// record's when nothing of it is left: UDP carries one datagram per
    /// record (`udp`). `None` once the server's side ended or the connection
    /// closed between records.
    pub(crate) fn poll_record(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<Option<Vec<u8>>>> {
        // no room: the reader stops at a payload without taking any of it
        let read = ready!(self.poll_read_records(cx, &mut ReadBuf::new(&mut [])));
        self.check(read)?;
        let reading = std::mem::replace(
            &mut self.reading,
            Reading::Header {
                buf: [0; HEADER + TAG],
                filled: 0,
            },
        );
        Poll::Ready(Ok(match reading {
            Reading::Payload { mut buf, pos, end } => {
                buf.truncate(end);
                buf.drain(..pos);
                Some(buf)
            }
            ended => {
                self.reading = ended;
                None
            }
        }))
```

新建 `crates/rurge-proto/src/snell/udp.rs`：

````rust
//! UDP over a Snell connection (phase 2 M6 design 4.3): a fresh connection
//! of its own asks `01 06 00` (UDP, no client id) and the server answers at
//! once — `00`, or an error as for TCP. From then on every record carries
//! one datagram, each way:
//!
//! ```text
//! ours:   01 ‖ host-length host | 00 04 IPv4 | 00 06 IPv6 ‖ port ‖ payload
//! theirs: 04 IPv4 | 06 IPv6 ‖ port ‖ payload
//! ```
//!
//! Every target goes through the one connection and whoever answers the
//! server's socket is heard (full cone). The record boundary is the
//! datagram's, which is why this is not a `stream_udp` framing: that one
//! reads lengths off a byte stream, and Snell's datagrams carry none. A
//! datagram that does not fit one record (`MAX_PAYLOAD` with its address)
//! is refused; a record of the server's that is no datagram is dropped, as
//! Surge does. The connection's end is the carrier's.

use super::REQUEST_VERSION;
use super::record::{MAX_PAYLOAD, SnellStream};
use super::tunnel::{ERROR, TUNNEL, UNKNOWN_REPLY, no_answer, refused};
use rurge_config::HostName;
use rurge_net::BoxFuture;
use rurge_net::connector::{PacketSocket, Target};
use std::future::poll_fn;
use std::io;
use std::net::IpAddr;
use std::pin::Pin;
use tokio::io::{AsyncWrite, AsyncWriteExt};

/// The request's command.
pub(super) const UDP: u8 = 0x06;
/// A datagram's command, in front of each of ours.
const FORWARD: u8 = 0x01;
const IPV4: u8 = 0x04;
const IPV6: u8 = 0x06;

fn unsendable(text: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, format!("snell: {text}"))
}

/// Our record for `payload` to `to`: a name as its A-labels, an IP after
/// the `00` marker.
fn datagram(to: &Target, payload: &[u8]) -> io::Result<Vec<u8>> {
    let mut out = vec![FORWARD];
    match &to.host {
        HostName::Ip(IpAddr::V4(v4)) => {
            out.extend_from_slice(&[0, IPV4]);
            out.extend_from_slice(&v4.octets());
        }
        HostName::Ip(IpAddr::V6(v6)) => {
            out.extend_from_slice(&[0, IPV6]);
            out.extend_from_slice(&v6.octets());
        }
        HostName::Domain(name) => {
            let name = crate::hostname::to_ascii(name)
                .ok_or_else(|| unsendable("the host name cannot be sent to the server"))?;
            let len = u8::try_from(name.len())
                .map_err(|_| unsendable("the host name is longer than 255 bytes"))?;
            out.push(len);
            out.extend_from_slice(name.as_bytes());
        }
    }
    out.extend_from_slice(&to.port.to_be_bytes());
    let room = MAX_PAYLOAD - out.len();
    if payload.len() > room {
        return Err(unsendable(&format!("a datagram longer than {room} bytes")));
    }
    out.extend_from_slice(payload);
    Ok(out)
}

/// The source at the start of the server's record, and where its payload
/// starts; `Err` says why the record is no datagram.
fn source(record: &[u8]) -> Result<(Target, usize), &'static str> {
    const SHORT: &str = "a datagram cut short";
    let (ip, at) = match record.first() {
        Some(&IPV4) => {
            let b: [u8; 4] = record.get(1..5).ok_or(SHORT)?.try_into().expect("4 bytes");
            (IpAddr::from(b), 5)
        }
        Some(&IPV6) => {
            let b: [u8; 16] = record
                .get(1..17)
                .ok_or(SHORT)?
                .try_into()
                .expect("16 bytes");
            (IpAddr::from(b), 17)
        }
        _ => return Err("an unknown address family"),
    };
    let port = record.get(at..at + 2).ok_or(SHORT)?;
    let port = u16::from_be_bytes([port[0], port[1]]);
    Ok((Target::new(HostName::Ip(ip), port), at + 2))
}

/// The server's answer to the request: `Ok` with the rest of its record,
/// if any — the first datagram.
async fn answer(stream: &mut SnellStream) -> io::Result<Option<Vec<u8>>> {
    let mut got = Vec::new();
    loop {
        let Some(record) = poll_fn(|cx| stream.poll_record(cx)).await? else {
            return Err(no_answer());
        };
        got.extend_from_slice(&record);
        match got[0] {
            TUNNEL => return Ok((got.len() > 1).then(|| got.split_off(1))),
            // `code length message`, maybe over several records
            ERROR => {
                if let Some(&len) = got.get(2)
                    && got.len() >= 3 + usize::from(len)
                {
                    return Err(refused(&got[1..3 + usize::from(len)]));
                }
            }
            _ => {
                return Err(io::Error::new(io::ErrorKind::InvalidData, UNKNOWN_REPLY));
            }
        }
    }
}

/// A carrier on one Snell connection. No `Debug`: the stream holds the
/// connection's keys.
pub(crate) struct SnellUdp {
    /// Locked only while a send or a receive polls it, as `tokio::io::split`
    /// does: the two directions share the connection.
    stream: std::sync::Mutex<SnellStream>,
    /// One datagram at a time, so that each is one record.
    sending: tokio::sync::Mutex<()>,
    /// One receiver at a time, and the datagram that came with the answer.
    receiving: tokio::sync::Mutex<Option<Vec<u8>>>,
}

impl SnellUdp {
    /// Asks `stream` (a fresh connection) for UDP and waits for the answer.
    pub(super) async fn open(mut stream: SnellStream) -> io::Result<SnellUdp> {
        stream.write_all(&[REQUEST_VERSION, UDP, 0]).await?;
        stream.flush().await?;
        let first = answer(&mut stream).await?;
        Ok(SnellUdp {
            stream: std::sync::Mutex::new(stream),
            sending: tokio::sync::Mutex::new(()),
            receiving: tokio::sync::Mutex::new(first),
        })
    }

    fn with_stream<T>(&self, f: impl FnOnce(&mut SnellStream) -> T) -> T {
        f(&mut self.stream.lock().expect("stream"))
    }
}

fn closed() -> io::Error {
    io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "snell: the server closed the UDP connection",
    )
}

impl PacketSocket for SnellUdp {
    fn send_to<'a>(&'a self, buf: &'a [u8], to: &'a Target) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            let record = datagram(to, buf)?;
            let _turn = self.sending.lock().await;
            // a record an abandoned send left parked goes out first: the
            // stream wants a write retried with the same bytes
            poll_fn(|cx| self.with_stream(|s| Pin::new(s).poll_flush(cx))).await?;
            // at most `MAX_PAYLOAD` bytes: one write, one record
            let written =
                poll_fn(|cx| self.with_stream(|s| Pin::new(s).poll_write(cx, &record))).await?;
            debug_assert_eq!(written, record.len(), "one record");
            poll_fn(|cx| self.with_stream(|s| Pin::new(s).poll_flush(cx))).await
        })
    }

    /// A datagram longer than `buf` is dropped: give it 64 KiB.
    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
        Box::pin(async move {
            let mut first = self.receiving.lock().await;
            loop {
                let record = match first.take() {
                    Some(record) => record,
                    None => poll_fn(|cx| self.with_stream(|s| s.poll_record(cx)))
                        .await?
                        .ok_or_else(closed)?,
                };
                match source(&record) {
                    Ok((from, at)) => {
                        let payload = &record[at..];
                        if let Some(space) = buf.get_mut(..payload.len()) {
                            space.copy_from_slice(payload);
                            return Ok((payload.len(), from));
                        }
                    }
                    Err(why) => {
                        tracing::debug!("snell: a UDP record from the server was dropped: {why}")
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::kdf::Psk;
    use super::*;
    use rurge_net::connector::BoxedStream;
    use std::time::Duration;

    fn to(host: &str, port: u16) -> Target {
        Target::new(HostName::parse(host), port)
    }

    #[test]
    fn our_datagrams_name_an_ip_after_a_marker_and_a_name_by_its_length() {
        assert_eq!(
            datagram(&to("1.2.3.4", 53), b"q").unwrap(),
            [1, 0, 4, 1, 2, 3, 4, 0, 53, b'q']
        );
        let mut v6 = vec![1, 0, 6];
        v6.extend_from_slice(&[0; 15]);
        v6.extend_from_slice(&[1, 0x01, 0xbb, b'x', b'y']);
        assert_eq!(datagram(&to("::1", 443), b"xy").unwrap(), v6);
        let name = Target::new(HostName::Domain("bücher.example".into()), 53);
        let mut expected = vec![1, 21];
        expected.extend_from_slice(b"xn--bcher-kva.example");
        expected.extend_from_slice(&[0, 53]);
        assert_eq!(datagram(&name, b"").unwrap(), expected, "an empty datagram");
        let err =
            datagram(&Target::new(HostName::Domain("a@b.test".into()), 53), b"x").unwrap_err();
        assert_eq!(
            err.to_string(),
            "snell: the host name cannot be sent to the server"
        );
    }

    #[test]
    fn a_datagram_fills_at_most_one_record() {
        // 9 bytes of command and IPv4 address
        let largest = vec![0u8; MAX_PAYLOAD - 9];
        assert_eq!(
            datagram(&to("1.2.3.4", 53), &largest).unwrap().len(),
            MAX_PAYLOAD
        );
        let err = datagram(&to("1.2.3.4", 53), &[0u8; MAX_PAYLOAD - 8]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(err.to_string(), "snell: a datagram longer than 16374 bytes");
    }

    #[test]
    fn the_servers_datagrams_start_with_their_source() {
        let record = [4, 9, 8, 7, 6, 0, 53, b'a', b'b'];
        assert_eq!(source(&record), Ok((to("9.8.7.6", 53), 7)));
        let mut v6 = vec![6];
        v6.extend_from_slice(&[0; 15]);
        v6.extend_from_slice(&[1, 0, 7]);
        assert_eq!(source(&v6), Ok((to("::1", 7), 19)), "an empty payload");
        assert_eq!(source(&[5, 1, 2]), Err("an unknown address family"));
        assert_eq!(
            source(&[0, 4, 1, 2, 3, 4, 0, 53]),
            Err("an unknown address family")
        );
        assert_eq!(source(&[4, 1, 2, 3, 4, 0]), Err("a datagram cut short"));
        assert_eq!(source(&v6[..18]), Err("a datagram cut short"));
    }

    /// Our stream and the server's, on the two ends of a pipe.
    async fn pair() -> (SnellStream, SnellStream) {
        let (near, far) = tokio::io::duplex(64 * 1024);
        let open = |end: tokio::io::DuplexStream| {
            SnellStream::open(Box::new(end) as BoxedStream, Psk::new("psk"))
        };
        (open(near).await.unwrap(), open(far).await.unwrap())
    }

    async fn next_record(server: &mut SnellStream) -> Option<Vec<u8>> {
        tokio::time::timeout(Duration::from_secs(5), poll_fn(|cx| server.poll_record(cx)))
            .await
            .expect("a record within the bound")
            .unwrap()
    }

    /// The carrier over `client`, the server answering `answers` (a record
    /// each) to the request.
    async fn opened(
        client: SnellStream,
        server: &mut SnellStream,
        answers: &[&[u8]],
    ) -> io::Result<SnellUdp> {
        let open = tokio::spawn(SnellUdp::open(client));
        assert_eq!(next_record(server).await.unwrap(), [1, 6, 0]);
        for answer in answers {
            server.write_all(answer).await.unwrap();
        }
        tokio::time::timeout(Duration::from_secs(5), open)
            .await
            .expect("opened within the bound")
            .unwrap()
    }

    async fn received(udp: &SnellUdp) -> io::Result<(Vec<u8>, Target)> {
        let mut buf = vec![0u8; 65536];
        let (n, from) = tokio::time::timeout(Duration::from_secs(5), udp.recv_from(&mut buf))
            .await
            .expect("received within the bound")?;
        Ok((buf[..n].to_vec(), from))
    }

    #[tokio::test]
    async fn a_datagram_in_the_answers_record_is_the_first_one() {
        let (client, mut server) = pair().await;
        let udp = opened(client, &mut server, &[&[0, 4, 1, 2, 3, 4, 0, 53, b'a']])
            .await
            .unwrap();
        assert_eq!(
            received(&udp).await.unwrap(),
            (b"a".to_vec(), to("1.2.3.4", 53))
        );
        // one record per datagram
        udp.send_to(b"one", &to("1.2.3.4", 53)).await.unwrap();
        udp.send_to(b"two", &to("5.6.7.8", 53)).await.unwrap();
        assert_eq!(
            next_record(&mut server).await.unwrap(),
            [1, 0, 4, 1, 2, 3, 4, 0, 53, b'o', b'n', b'e']
        );
        assert_eq!(
            next_record(&mut server).await.unwrap(),
            [1, 0, 4, 5, 6, 7, 8, 0, 53, b't', b'w', b'o']
        );
    }

    #[tokio::test]
    async fn what_is_no_datagram_is_dropped_and_the_next_arrives() {
        let (client, mut server) = pair().await;
        let udp = opened(client, &mut server, &[&[0]]).await.unwrap();
        // an unknown family, a cut-short address, one too long for `buf`
        server
            .write_all(&[5, 1, 2, 3, 4, 0, 53, b'x'])
            .await
            .unwrap();
        server.write_all(&[4, 1, 2, 3]).await.unwrap();
        let mut long = vec![4, 1, 2, 3, 4, 0, 53];
        long.extend_from_slice(&[0u8; 2000]);
        server.write_all(&long).await.unwrap();
        server
            .write_all(&[4, 1, 2, 3, 4, 0, 53, b'z'])
            .await
            .unwrap();
        let mut buf = vec![0u8; 1500];
        let (n, from) = tokio::time::timeout(Duration::from_secs(5), udp.recv_from(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!((&buf[..n], from), (&b"z"[..], to("1.2.3.4", 53)));
    }

    #[tokio::test]
    async fn the_servers_refusal_or_another_answer_fails_the_open() {
        let (client, mut server) = pair().await;
        // the error's message in a record of its own
        let err = opened(client, &mut server, &[&[2, 9, 7], b"no udp\x07"])
            .await
            .err()
            .unwrap();
        assert_eq!(err.kind(), io::ErrorKind::ConnectionRefused);
        assert_eq!(err.to_string(), "snell: the server refused: no udp");
        let (client, mut server) = pair().await;
        let err = opened(client, &mut server, &[&[1]]).await.err().unwrap();
        assert_eq!(err.to_string(), UNKNOWN_REPLY);
        let (client, mut server) = pair().await;
        let open = tokio::spawn(SnellUdp::open(client));
        next_record(&mut server).await.unwrap();
        server.shutdown().await.unwrap();
        let err = open.await.unwrap().err().unwrap();
        assert_eq!(
            err.to_string(),
            "snell: the server closed the connection without answering"
        );
    }

    #[tokio::test]
    async fn the_connections_end_is_the_carriers_end() {
        let (client, mut server) = pair().await;
        let udp = opened(client, &mut server, &[&[0]]).await.unwrap();
        // the server's empty record
        poll_fn(|cx| server.poll_end(cx)).await.unwrap();
        let err = received(&udp).await.unwrap_err();
        assert_eq!(
            (err.kind(), err.to_string().as_str()),
            (
                io::ErrorKind::UnexpectedEof,
                "snell: the server closed the UDP connection"
            )
        );
        // the close between records too
        let (client, mut server) = pair().await;
        let udp = opened(client, &mut server, &[&[0]]).await.unwrap();
        drop(server);
        let err = received(&udp).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "snell: the server closed the UDP connection"
        );
    }

    #[tokio::test]
    async fn a_datagram_too_long_for_a_record_is_refused_unsent() {
        let (client, mut server) = pair().await;
        let udp = opened(client, &mut server, &[&[0]]).await.unwrap();
        let err = udp
            .send_to(&[0u8; MAX_PAYLOAD], &to("1.2.3.4", 53))
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "snell: a datagram longer than 16374 bytes");
        udp.send_to(b"fits", &to("1.2.3.4", 53)).await.unwrap();
        assert_eq!(
            next_record(&mut server).await.unwrap(),
            [1, 0, 4, 1, 2, 3, 4, 0, 53, b'f', b'i', b't', b's'],
            "nothing went out before it"
        );
    }
}
````

`crates/rurge-proto/src/snell/tunnel.rs`——把

```rust
const TUNNEL: u8 = 0x00;
const ERROR: u8 = 0x02;
```

换成

```rust
pub(super) const TUNNEL: u8 = 0x00;
pub(super) const ERROR: u8 = 0x02;
```

`crates/rurge-proto/src/snell/tunnel.rs`——把

```rust
const UNKNOWN_REPLY: &str = "snell: the server answered with an unknown reply";

fn no_answer() -> io::Error {
```

换成

```rust
pub(super) const UNKNOWN_REPLY: &str = "snell: the server answered with an unknown reply";

pub(super) fn no_answer() -> io::Error {
```

`crates/rurge-proto/src/snell/tunnel.rs`——把

```rust
fn refused(answer: &[u8]) -> io::Error {
```

换成

```rust
pub(super) fn refused(answer: &[u8]) -> io::Error {
```

`crates/rurge-proto/src/snell/mod.rs`——把

```rust
//! outbound's pool (`pool`), and the next request goes out on it.
```

换成

```rust
//! outbound's pool (`pool`), and the next request goes out on it.
//!
//! UDP needs no parameter on v4 / v5: a connection of its own, never
//! pooled, to `udp-port` (else the policy's port) through the same layers,
//! carries every datagram (`udp`).
```

`crates/rurge-proto/src/snell/mod.rs`——把

```rust
mod tunnel;
```

换成

```rust
mod tunnel;
mod udp;
```

`crates/rurge-proto/src/snell/mod.rs`——把

```rust
use crate::{BuildError, Outbound, OutboundError};
```

换成

```rust
use crate::{BuildError, Outbound, OutboundError, UdpSupport};
```

`crates/rurge-proto/src/snell/mod.rs`——把

```rust
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
```

换成

```rust
use rurge_net::connector::{BoxedPacketSocket, BoxedStream, ConnectOpts, Connector, Target};
```

`crates/rurge-proto/src/snell/mod.rs`——把

```rust
use tunnel::SnellTunnel;
```

换成

```rust
use tunnel::SnellTunnel;
use udp::SnellUdp;
```

`crates/rurge-proto/src/snell/mod.rs`——把

```rust
    dialer: Arc<Dialer>,
```

换成

```rust
    dialer: Arc<Dialer>,
    /// To `udp-port`; the same as `dialer` without one.
    udp_dialer: Arc<Dialer>,
```

`crates/rurge-proto/src/snell/mod.rs`——把

```rust
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
        Ok(SnellOutbound {
            name: name.to_string(),
            dialer: Arc::new(Dialer {
                stack,
                psk: Psk::new(spec.psk.expose()),
            }),
```

换成

```rust
        let stack = |to: &Target| -> Result<Stack, BuildError> {
            let shadow_tls = shadow_tls_client(shadow_tls, None, &to.host, roots.clone())?;
            let mut stack = Stack::new(connector.clone(), to.clone(), shadow_tls, None, None);
            if let Some(obfs) = &spec.obfs {
                stack = stack.with_obfs(ObfsClient::new(obfs, to)?);
            }
            Ok(stack)
        };
        let psk = Psk::new(spec.psk.expose());
        let dialer = Arc::new(Dialer {
            stack: stack(&server)?,
            psk: psk.clone(),
        });
        // UDP rides a TCP connection: `udp-port` is where that one goes
        let udp_dialer = match spec.udp_port {
            Some(port) if port != server.port => Arc::new(Dialer {
                stack: stack(&Target::new(server.host.clone(), port))?,
                psk,
            }),
            _ => dialer.clone(),
        };
        Ok(SnellOutbound {
            name: name.to_string(),
            dialer,
            udp_dialer,
```

`crates/rurge-proto/src/snell/mod.rs`——把

```rust
        })
    }
}
```

换成

```rust
        })
    }

    /// v4 and v5 carry UDP whatever the policy says.
    fn udp(&self) -> UdpSupport {
        UdpSupport::Native
    }

    fn open_udp<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedPacketSocket, OutboundError>> {
        Box::pin(async move {
            // one budget for the connection, its layers, the key and the
            // server's answer, which comes at once
            let open = async {
                let stream = self.udp_dialer.fresh(opts).await?;
                let udp = SnellUdp::open(stream).await?;
                Ok(Box::new(udp) as BoxedPacketSocket)
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
- `SnellUdp` 把流放在 `std::sync::Mutex` 里，只在一次 `poll_*` 期间持有（`tokio::io::split` 的做法，从不跨 await）；两个 tokio 互斥锁分别让发送与接收各自排队；发送前先 flush，被取消的发送留下的记录先发出去。
- 丢掉的记录只记 `debug!("snell: a UDP record from the server was dropped: <why>")`，原因是固定文字，不带载荷。
- IPv4 映射的 IPv6 目标按 `06` 发，不做转换。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-proto --lib snell` → 通过（新增 16 条：数据报编解码的已知答案、经 `FakeSnell` 到回环 UDP 回显的往返（IP 与名字）、全锥、非数据报的记录被丢且下一个照常到达、太长的数据报报错且什么也不发、`udp-port`、连接结束即载体结束、服务端拒绝 UDP）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-proto
git commit -m "feat(proto): Snell 的 UDP over TCP——每条记录一个数据报、全锥、udp-port 为该连接的端口"
```

### Task 5: 引擎装配与能力表

去掉 Task 1 的门，引擎工厂装上 `SnellOutbound`（经直连连接器或 `underlying-proxy` 的链，同 `ss`），能力表加入 `snell`。重载按指纹复用不需要改代码（`SnellSpec` 按值比较），用例证明参数没变时连复用池也沿用。M6a 已把"未实现协议"的用例例子换成 `hysteria2`，本任务不需要再换。

**Files:**
- Create: `crates/rurge-engine/tests/outbounds_snell.rs`
- Modify: `crates/rurge-config/src/spec/mod.rs`（与用例）、`crates/rurge-engine/src/outbounds.rs`（与用例）、`crates/rurge/src/capabilities.rs`、`crates/rurge-engine/tests/common/mod.rs`、`crates/rurge/tests/cli.rs`

**Interfaces:**
- Consumes: Task 1 ～ 4 的全部；既有的 `EngineFactory::build`、`server_of`、测试夹具。
- Produces: 工厂的 `ProtoSpec::Snell` 分支；`PolicyKind::Snell` 进能力表；`tests/common` 再导出 `FakeSnell` / `SnellScript`。

- [ ] **Step 1: 先写用例**

新建 `crates/rurge-engine/tests/outbounds_snell.rs`：

```rust
//! Sessions that leave through `snell` (phase 2 M6 design 4.3): TCP and
//! UDP through the engine to the loopback `FakeSnell` — versions 4 and 5,
//! `reuse`, obfs `http`, the server's refusal, a wrong PSK,
//! `underlying-proxy`, reloads, and the versions that are not implemented.

mod common;
use common::*;
use rurge_config::session::Transport;
use rurge_engine::RequestRecord;

fn udp_records(h: &Harness) -> Vec<RequestRecord> {
    h.engine
        .request_log()
        .recent(4096)
        .into_iter()
        .filter(|r| r.transport == Transport::Udp)
        .collect()
}

/// `S = snell, …` to `fake`, with `params` after the port.
fn snell_line(fake: &FakeSnell, params: &str) -> String {
    format!("S = snell, 127.0.0.1, {}, {params}", fake.addr().port())
}

async fn through(proxies: &str) -> Harness {
    harness(Profile {
        proxies,
        rules: "DOMAIN,target.test,S\nIP-CIDR,127.0.0.1/32,S,no-resolve",
        ..Profile::default()
    })
    .await
}

/// The first `n` sessions' records, oldest first, once they have finished.
async fn the_records(h: &Harness, n: usize) -> Vec<RequestRecord> {
    let log = h.engine.request_log();
    wait_until("the sessions to finish", || log.recent(10).len() >= n).await;
    let mut records = log.recent(10);
    records.reverse();
    records
}

/// One session to the echo through `h`: a round trip, then the client
/// goes and the session finishes.
async fn one_session(h: &Harness, n: usize, payload: &[u8]) -> RequestRecord {
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, payload).await;
    drop(tunnel);
    the_records(h, n).await[n - 1].clone()
}

/// Opens a session whose server never answers with the tunnel: the
/// client's bytes get nothing back, the tunnel closes within the bound,
/// and the record says why.
async fn a_failed_session(h: &Harness) -> RequestRecord {
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    tunnel.write_all(b"hello").await.unwrap();
    let mut rest = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), tunnel.read_to_end(&mut rest))
        .await
        .expect("the tunnel closes within the bound");
    assert!(rest.is_empty(), "nothing is passed on");
    let record = the_records(h, 1).await[0].clone();
    assert_eq!(record.status, RecordStatus::Failed, "{record:?}");
    record
}

/// Versions 4 and 5 with and without `reuse`: each carries a tunnel to the
/// echo, and the server is asked for the name.
#[tokio::test]
async fn a_connect_leaves_through_snell_v4_and_v5() {
    let echo = rurge_proto::testing::echo_server().await;
    for params in [
        "psk=s3same, version=4",
        "psk=s3same, version=5",
        "psk=s3same, version=5, reuse=true",
    ] {
        let fake = FakeSnell::spawn(SnellScript {
            connect_to: Some(echo),
            ..SnellScript::new("s3same")
        })
        .await;
        let h = through(&snell_line(&fake, params)).await;
        let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
        echo_through(&mut tunnel, b"through snell").await;
        echo_through(&mut tunnel, &vec![0x5a; 100_000]).await;
        let seen = fake.requests();
        let first = seen.first().expect("the server never saw a request");
        assert_eq!(
            (first.host.as_str(), first.port),
            ("target.test", 7),
            "{params}: the server resolves the name"
        );
        assert!(h.dns.queries().is_empty(), "rurge never looked the name up");
        drop(tunnel);
        let record = the_records(&h, 1).await[0].clone();
        assert_eq!(record.policy, ["S"], "{params}");
        assert!(record.error.is_none(), "{params}: {:?}", record.error);
    }
}

/// With `reuse=true` two sessions one after the other share one Snell
/// connection; without it each has its own.
#[tokio::test]
async fn reuse_carries_sessions_one_after_another_on_one_connection() {
    let echo = rurge_proto::testing::echo_server().await;
    for (reuse, connections) in [("true", 1), ("false", 2)] {
        let fake = FakeSnell::spawn(SnellScript {
            connect_to: Some(echo),
            ..SnellScript::new("s3same")
        })
        .await;
        let h = through(&snell_line(
            &fake,
            &format!("psk=s3same, version=4, reuse={reuse}"),
        ))
        .await;
        for (n, payload) in [(1, &b"first"[..]), (2, b"second")] {
            let record = one_session(&h, n, payload).await;
            assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
        }
        assert_eq!(fake.connections(), connections, "reuse={reuse}");
        let places: Vec<(usize, usize)> = fake
            .requests()
            .iter()
            .map(|r| (r.connection, r.tunnel))
            .collect();
        let expected: &[(usize, usize)] = if reuse == "true" {
            &[(0, 0), (0, 1)]
        } else {
            &[(0, 0), (1, 0)]
        };
        assert_eq!(places, expected, "reuse={reuse}");
    }
}

#[tokio::test]
async fn obfs_http_carries_the_session() {
    let echo = rurge_proto::testing::echo_server().await;
    let fake = FakeSnell::spawn(SnellScript {
        obfs_http: true,
        connect_to: Some(echo),
        ..SnellScript::new("s3same")
    })
    .await;
    let h = through(&snell_line(
        &fake,
        "psk=s3same, version=5, obfs=http, obfs-host=cdn.test, obfs-uri=/a",
    ))
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, b"behind the camouflage").await;
    let hello = &fake.obfs_seen()[0];
    assert_eq!(hello.host, format!("cdn.test:{}", fake.addr().port()));
    assert_eq!(hello.uri.as_deref(), Some("/a"));
}

/// The server's error answer fails the session, and the record quotes it.
#[tokio::test]
async fn the_servers_refusal_is_the_sessions_failure() {
    let fake = FakeSnell::spawn(SnellScript {
        refuse: Some((0x05, b"no such host".to_vec())),
        ..SnellScript::new("s3same")
    })
    .await;
    let h = through(&snell_line(&fake, "psk=s3same, version=4")).await;
    let record = a_failed_session(&h).await;
    assert_eq!(
        record.error.as_deref(),
        Some("snell: the server refused: no such host")
    );
}

/// A server that cannot decrypt the request closes without a word: the
/// session fails with that, and the PSK is nowhere in the record.
#[tokio::test]
async fn a_wrong_psk_fails_the_session_closed_without_an_answer() {
    let fake = FakeSnell::spawn(SnellScript::new("right")).await;
    let h = through(&snell_line(&fake, "psk=wr0ngPsk, version=5")).await;
    let record = a_failed_session(&h).await;
    assert_eq!(
        record.error.as_deref(),
        Some("snell: the server closed the connection without answering")
    );
    assert!(!format!("{record:?}").contains("wr0ngPsk"));
    assert_eq!((fake.rejected(), fake.requests().len()), (1, 0));
}

/// A version other than 4 and 5 is parsed and rejects at run time, and the
/// record names the version (M6-D2).
#[tokio::test]
async fn snell_v1_rejects_and_says_which() {
    let h = harness(Profile {
        proxies: "V1 = snell, 127.0.0.1, 9, psk=s3same, version=1",
        rules: "DOMAIN,target.test,V1",
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
    let record = the_records(&h, 1).await[0].clone();
    assert_eq!(record.status, RecordStatus::Rejected("REJECT".into()));
    assert_eq!(
        record.error.as_deref(),
        Some("policy protocol not implemented: snell v1")
    );
}

/// `snell` over a SOCKS5 `underlying-proxy`: the entry is asked for the
/// `snell` server, and the session goes through its tunnel.
#[tokio::test]
async fn snell_goes_through_an_underlying_socks5_proxy() {
    let echo = rurge_proto::testing::echo_server().await;
    let fake = FakeSnell::spawn(SnellScript {
        connect_to: Some(echo),
        ..SnellScript::new("s3same")
    })
    .await;
    let entry = FakeSocks5::spawn(Socks5Script::default()).await;
    let h = through(&format!(
        "Entry = socks5, 127.0.0.1, {}\n{}",
        entry.addr().port(),
        snell_line(&fake, "psk=s3same, version=4, underlying-proxy=Entry")
    ))
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, b"socks5 then snell").await;
    let first = entry.requests()[0].clone();
    assert_eq!(
        (first.command, first.host.as_str(), first.port),
        (1, "127.0.0.1", fake.addr().port())
    );
    assert_eq!(fake.requests()[0].host, "target.test");
}

/// Three datagrams to two echoes through `S` (v5) over one association:
/// every flow leaves through `S`, over one Snell connection, and ends with
/// the association.
#[tokio::test]
async fn udp_goes_through_snell() {
    let fake = FakeSnell::spawn(SnellScript::new("s3same")).await;
    let h = through(&snell_line(&fake, "psk=s3same, version=5, reuse=true")).await;
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
    assert_eq!(
        fake.datagrams(),
        [one.to_string(), two.to_string(), one.to_string()]
    );
    let commands: Vec<u8> = fake.requests().iter().map(|r| r.command).collect();
    assert_eq!(commands, [6], "one UDP session");
    assert_eq!(fake.connections(), 1);
}

/// Full cone: whoever reaches the server's socket for this client reaches
/// the client, under its own address.
#[tokio::test]
async fn anyone_may_answer_through_snell() {
    let fake = FakeSnell::spawn(SnellScript::new("s3same")).await;
    let h = through(&snell_line(&fake, "psk=s3same, version=5")).await;
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

/// The outbound — and with it the idle connection `reuse` keeps — is kept
/// across a reload that leaves the line alone, and rebuilt when its PSK,
/// version, `reuse`, obfs or `udp-port` changes (design 6).
#[tokio::test]
async fn a_reload_keeps_an_unchanged_snell_policy_and_rebuilds_a_changed_one() {
    let echo = rurge_proto::testing::echo_server().await;
    let fake = FakeSnell::spawn(SnellScript {
        connect_to: Some(echo),
        ..SnellScript::new("s3same")
    })
    .await;
    let base = "psk=s3same, version=4, reuse=true";
    let proxies = |params: &str, extra: &str| format!("{}\n{extra}", snell_line(&fake, params));
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
    one_session(&h, 1, b"before the reload").await;
    let before = outbound_now(&h, "S");
    h.engine
        .swap_runtime(reload(base, "Other = http, other.example, 8080").await);
    assert!(
        Arc::ptr_eq(&before, &outbound_now(&h, "S")),
        "an unrelated reload rebuilt S"
    );
    one_session(&h, 2, b"after the reload").await;
    assert_eq!(fake.connections(), 1, "the pooled connection was kept");

    let mut previous = before;
    for changed in [
        "psk=0ther, version=4, reuse=true",
        "psk=0ther, version=5, reuse=true",
        "psk=0ther, version=5, reuse=false",
        "psk=0ther, version=5, reuse=false, obfs=http",
        "psk=0ther, version=5, reuse=false, obfs=http, udp-port=9999",
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
    AnyTlsScript, FakeAnyTls, FakeHttpProxy, FakeShadowsocks, FakeSocks5, FakeTrojan, FakeVmess,
    HttpProxyScript, ShadowsocksScript, Socks5Script, TlsFixture, TrojanScript, VmessScript,
```

换成

```rust
    AnyTlsScript, FakeAnyTls, FakeHttpProxy, FakeShadowsocks, FakeSnell, FakeSocks5, FakeTrojan,
    FakeVmess, HttpProxyScript, ShadowsocksScript, SnellScript, Socks5Script, TlsFixture,
    TrojanScript, VmessScript,
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
SN = ss, proxy.test, 8388, encrypt-method=none\n\
```

换成

```rust
SN = ss, proxy.test, 8388, encrypt-method=none\n\
N4 = snell, proxy.test, 443, psk=pw, version=4, reuse=true, obfs=http\n\
N5 = snell, proxy.test, 443, psk=pw, version=5, udp-port=8443, shadow-tls-password=st\n\
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
            ("SN", "SN"),
```

换成

```rust
            ("SN", "SN"),
            ("N4", "N4"),
            ("N5", "N5"),
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
        assert!(loaded.config.spec("S").is_none());
```

换成

```rust
        assert!(loaded.config.spec("S").is_none());
    }

    /// Sound `snell` lines pass the dry build, and building one starts
    /// nothing: there is no tokio runtime here (phase 2 M6 design 4.3).
    #[test]
    fn a_snell_policy_passes_the_dry_build() {
        let cfg = config(
            "[Proxy]\nN4 = snell, proxy.test, 443, psk=pw, version=4, reuse=true, obfs=http, obfs-host=cdn.test\n\
N5 = snell, proxy.test, 443, psk=pw, version=5, udp-port=8443, shadow-tls-password=st, shadow-tls-version=3, shadow-tls-sni=site.test\n\
[Rule]\nFINAL,DIRECT\n",
        );
        assert!(dry_build(&cfg).is_empty());
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    /// A `snell` line is read and checked in full; it has no spec until the
    /// engine builds `snell` (M6b task 5), and a version other than 4 and 5
    /// says why (M6-D2).
```

换成

```rust
    /// A `snell` line is read and checked in full; a version other than 4
    /// and 5 has no spec and says why (M6-D2).
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
        assert_eq!(o.not_implemented, None);
        assert!(o.spec.is_none());
```

换成

```rust
        assert_eq!(o.not_implemented, None);
        let spec = o.spec.expect("a version 5 line has a spec");
        let ProtoSpec::Snell(snell) = &spec.proto else {
            panic!("{:?}", spec.proto)
        };
        assert_eq!(
            (snell.version, snell.reuse, snell.udp_port),
            (SnellVersion::V5, true, Some(8443))
        );
        assert_eq!(snell.obfs.as_ref().map(|o| o.mode), Some(ObfsMode::Http));
        assert!(spec.shadow_tls.is_some());
```

`crates/rurge/tests/cli.rs`——把

```rust
            "bad.conf:3: policy `K`: key #1 of `password` is not a Base64 key of 16 bytes, as `2022-blake3-aes-128-gcm` requires",
        ))
        .stdout(predicate::str::contains("c2VjcmV0").not());
}

const SUBSCRIBED: &str = "[General]\n[Proxy Group]\nLocal = select, DIRECT, policy-path=nodes.txt\n\
```

换成

```rust
            "bad.conf:3: policy `K`: key #1 of `password` is not a Base64 key of 16 bytes, as `2022-blake3-aes-128-gcm` requires",
        ))
        .stdout(predicate::str::contains("c2VjcmV0").not());
}

const SNELL: &str = "[General]\n[Proxy]\n\
N = snell, proxy.test, 443, psk=s3cretPsk, version=4, reuse=true, obfs=http\n\
Old1 = snell, proxy.test, 443, psk=s3cretPsk\n\
Old2 = snell, proxy.test, 443, psk=s3cretPsk, version=1\n[Rule]\nFINAL,DIRECT\n";
const SNELL_BAD_VERSION: &str = "[General]\n[Proxy]\n\
N = snell, proxy.test, 443, psk=s3cretPsk, version=7\n[Rule]\nFINAL,DIRECT\n";

/// `rurge check` knows `snell` versions 4 and 5 (phase 2 M6 design 4.2):
/// version 1 — also the default — is still "not implemented", once however
/// many lines use it; a version out of range is an error; the PSK is never
/// printed.
#[test]
fn check_knows_snell() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "snell.conf", SNELL))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("snell.conf:4")
            && out.contains("`snell` version 1 (the default when `version` is not written)"),
        "{out}"
    );
    assert!(!out.contains("policy type `snell`"), "{out}");
    assert!(!out.contains("s3cretPsk"), "{out}");

    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "bad.conf", SNELL_BAD_VERSION))
        .assert()
        .code(2)
        .stdout(predicate::str::contains("E0018"))
        .stdout(predicate::str::contains(
            "bad.conf:3: policy `N`: invalid value `7` for `version` (expected an integer from 1 to 6)",
        ))
        .stdout(predicate::str::contains("s3cretPsk").not());
}

const SUBSCRIBED: &str = "[General]\n[Proxy Group]\nLocal = select, DIRECT, policy-path=nodes.txt\n\
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-engine --test outbounds_snell`
Expected: FAIL——`snell` 行还没有 spec、工厂还不会构建，经它的会话都被拒绝（`common/mod.rs:176` 是 HTTP 入站的 CONNECT 等不到应答）：

```text
test reuse_carries_sessions_one_after_another_on_one_connection ... FAILED
test obfs_http_carries_the_session ... FAILED
test snell_goes_through_an_underlying_socks5_proxy ... FAILED
test a_reload_keeps_an_unchanged_snell_policy_and_rebuilds_a_changed_one ... FAILED
test a_connect_leaves_through_snell_v4_and_v5 ... FAILED
test a_wrong_psk_fails_the_session_closed_without_an_answer ... FAILED
test the_servers_refusal_is_the_sessions_failure ... FAILED
test anyone_may_answer_through_snell ... FAILED
test udp_goes_through_snell ... FAILED
thread 'reuse_carries_sessions_one_after_another_on_one_connection' panicked at crates\rurge-engine\tests\common\mod.rs:176:9:
thread 'obfs_http_carries_the_session' panicked at crates\rurge-engine\tests\common\mod.rs:176:9:
thread 'snell_goes_through_an_underlying_socks5_proxy' panicked at crates\rurge-engine\tests\common\mod.rs:176:9:
thread 'a_reload_keeps_an_unchanged_snell_policy_and_rebuilds_a_changed_one' panicked at crates\rurge-engine\tests\common\mod.rs:176:9:
thread 'a_connect_leaves_through_snell_v4_and_v5' panicked at crates\rurge-engine\tests\common\mod.rs:176:9:
thread 'a_wrong_psk_fails_the_session_closed_without_an_answer' panicked at crates\rurge-engine\tests\common\mod.rs:176:9:
thread 'the_servers_refusal_is_the_sessions_failure' panicked at crates\rurge-engine\tests\common\mod.rs:176:9:
thread 'anyone_may_answer_through_snell' panicked at crates\rurge-engine\tests\common\mod.rs:278:18:
thread 'udp_goes_through_snell' panicked at crates\rurge-engine\tests\common\mod.rs:278:18:
test result: FAILED. 1 passed; 9 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.06s
error: test failed, to rerun pass `-p rurge-engine --test outbounds_snell`
exit 101
```

- [ ] **Step 3: 实现**

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    // the engine builds `snell` from M6b task 5 on: until then a valid line
    // is checked in full but has no spec
    let built = policy.kind != PolicyKind::Snell;
    let spec = (!failed && not_implemented.is_none() && built).then(|| PolicySpec {
```

换成

```rust
    let spec = (!failed && not_implemented.is_none()).then(|| PolicySpec {
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
use rurge_proto::shadowsocks::ShadowsocksOutbound;
```

换成

```rust
use rurge_proto::shadowsocks::ShadowsocksOutbound;
use rurge_proto::snell::SnellOutbound;
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
            // the loader makes no spec of a `snell` line before M6b task 5
            ProtoSpec::Snell(_) => {
                return Err(BuildError::new(format!(
                    "policy `{}`: `snell` is not implemented yet",
                    spec.name
                )));
            }
```

换成

```rust
            ProtoSpec::Snell(snell) => Arc::new(SnellOutbound::new(
                &spec.name,
                server_of(spec)?,
                snell,
                spec.shadow_tls.as_ref(),
                self.roots.clone(),
                connector,
            )?),
```

`crates/rurge/src/capabilities.rs`——把

```rust
//! M6a), `select` groups, `url-test` / `fallback` /
//! `load-balance` groups (phase 2 M3b), and `smart` groups (phase 2 M3c).
```

换成

```rust
//! M6a), `snell` versions 4 and 5 (phase 2 M6b), `select` groups,
//! `url-test` / `fallback` / `load-balance` groups (phase 2 M3b), and
//! `smart` groups (phase 2 M3c).
```

`crates/rurge/src/capabilities.rs`——把

```rust
            PolicyKind::Shadowsocks,
```

换成

```rust
            PolicyKind::Shadowsocks,
            PolicyKind::Snell,
```

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-engine --test outbounds_snell` → 通过（10 条：v4 / v5 与复用的连接、复用时两个顺序会话共用一条 Snell 连接而不复用时是两条、obfs http、服务端拒绝即会话的失败原因、psk 错、`version=1` 被拒并说明、经 `underlying-proxy`（socks5）的 TCP、经 SOCKS5 UDP ASSOCIATE 的 UDP 与全锥、重载时沿用与重建）。
Run: `cargo test -p rurge --test cli check_knows_snell` → 通过。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates
git commit -m "feat(engine): 装配 snell 出站、能力表翻转 snell（v4 / v5）"
```

### Task 6: 互操作与文档

sing-box 升到 1.14.2 并加 `snell` 入站、官方 snell-server v5.0.1 的夹具（只在 Linux CI 安装，P12）；兼容性清单、手工验收、两份 README（含停在 M4b 的路线图一行）、`CLAUDE.md` 与总设计（C2）。

**Files:**
- Create: `tests/interop/src/snell_server.rs`（自带用例）、`tests/interop/tests/snell.rs`
- Modify: `tests/interop/src/lib.rs`、`tests/interop/README.md`、`.github/workflows/ci.yml`、`docs/surge-compatibility-matrix.md`、`docs/acceptance/phase2-manual.md`、`README.md`、`README_en.md`、`CLAUDE.md`、`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`

**Interfaces:**
- Consumes: 互操作夹具既有的 `Reference`、`roundtrip` / `roundtrip_big` / `udp_roundtrip`、`outbound(profile, name, fixture)`。
- Produces: sing-box 夹具的 `InboundKind::Snell { obfs_http }`；`rurge_interop::snell_server`（`RURGE_TEST_SNELL_SERVER`、配置只监听 `127.0.0.1`）。

- [ ] **Step 1: 互操作夹具与用例**

`tests/interop/src/lib.rs`——把

```rust
pub mod shadowsocks_rust;
```

换成

```rust
pub mod shadowsocks_rust;
pub mod snell_server;
```

`tests/interop/src/lib.rs`——把

```rust
        method: &'static str,
    },
}
```

换成

```rust
        method: &'static str,
    },
    /// Snell `version: 5`, which also accepts v4 clients (the wire format
    /// is the same); `users[0]` holds the PSK (the name is ignored). With
    /// `obfs_http`, simple-obfs `http` in front (`obfs_mode: http`).
    Snell {
        obfs_http: bool,
    },
}
```

`tests/interop/src/lib.rs`——把

```rust
                    InboundKind::Shadowsocks { .. } => "shadowsocks",
```

换成

```rust
                    InboundKind::Shadowsocks { .. } => "shadowsocks",
                    InboundKind::Snell { .. } => "snell",
```

`tests/interop/src/lib.rs`——把

```rust
                        .collect();
```

换成

```rust
                        .collect();
                }
            } else if let InboundKind::Snell { obfs_http } = inbound.kind {
                let (_, psk) = inbound.users.first().expect("a Snell PSK");
                v["version"] = json!(5);
                v["psk"] = json!(psk);
                if obfs_http {
                    v["obfs_mode"] = json!("http");
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
                    kind: InboundKind::Snell { obfs_http: false },
                    users: vec![("ignored".into(), "sn3ll".into())],
                    tls: None,
                    ws_path: None,
                },
                1011,
            ),
            (
                Inbound {
                    kind: InboundKind::Snell { obfs_http: true },
                    users: vec![("ignored".into(), "sn3ll".into())],
                    tls: None,
                    ws_path: None,
                },
                1012,
            ),
        ]
```

`tests/interop/src/lib.rs`——把

```rust
            json!([{ "name": "u", "password": "ZmVkY2JhOTg3NjU0MzIxMA==" }])
        );
    }
```

换成

```rust
            json!([{ "name": "u", "password": "ZmVkY2JhOTg3NjU0MzIxMA==" }])
        );
        // version 5 (it takes v4 clients too), one PSK and no users
        let snell = &config["inbounds"][10];
        assert_eq!(
            (&snell["type"], &snell["version"], &snell["psk"]),
            (&json!("snell"), &json!(5), &json!("sn3ll"))
        );
        assert!(snell.get("users").is_none() && snell.get("obfs_mode").is_none());
        assert_eq!(config["inbounds"][11]["obfs_mode"], "http");
    }
```

新建 `tests/interop/src/snell_server.rs`：

```rust
//! Surge's official `snell-server` v5.0.1 as a child process on the loopback,
//! the reference for the `snell` outbound next to sing-box (phase 2 M6
//! design 4.4, M6-D5). It is built for Linux only: elsewhere a missing
//! binary is always a skip, even with `RURGE_INTEROP_REQUIRED=1`. The same
//! rules as for sing-box apply: nothing is downloaded or installed here, and
//! the rendered configuration listens on 127.0.0.1 only.

use crate::{REQUIRED_ENV, Reference, free_port};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const BINARY_ENV: &str = "RURGE_TEST_SNELL_SERVER";

/// `RURGE_TEST_SNELL_SERVER`, else the first `snell-server` on `PATH`.
pub fn locate() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(BINARY_ENV).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join("snell-server"))
        .find(|candidate| candidate.is_file())
}

/// The binary — or `None` after saying why `test` is skipped. With
/// `RURGE_INTEROP_REQUIRED=1` (CI) a missing binary is a failure on Linux,
/// the only platform snell-server is built for.
pub fn snell_server_or_skip(test: &str) -> Option<PathBuf> {
    if let Some(path) = locate() {
        return Some(path);
    }
    if cfg!(target_os = "linux") && std::env::var(REQUIRED_ENV).as_deref() == Ok("1") {
        panic!("{REQUIRED_ENV}=1 but no snell-server was found ({BINARY_ENV} or PATH)");
    }
    eprintln!(
        "skipping {test}: no snell-server ({BINARY_ENV} or PATH; Linux only); see tests/interop/README.md"
    );
    None
}

/// The whole configuration: one server on `127.0.0.1:port`, IPv4 only, with
/// simple-obfs `http` in front when `obfs_http`.
pub fn render(port: u16, psk: &str, obfs_http: bool) -> String {
    let mut text =
        format!("[snell-server]\nlisten = 127.0.0.1:{port}\npsk = {psk}\nipv6 = false\n");
    if obfs_http {
        text.push_str("obfs = http\n");
    }
    text
}

/// A running snell-server; killed and reaped on drop.
pub struct SnellServer(Reference);

impl SnellServer {
    /// Writes the configuration into `dir`, starts `binary` there and waits
    /// until it accepts connections.
    pub fn spawn(binary: &Path, dir: &Path, psk: &str, obfs_http: bool) -> SnellServer {
        let port = free_port();
        let config = dir.join("snell-server.conf");
        std::fs::write(&config, render(port, psk, obfs_http)).expect("write the config");
        let mut command = Command::new(binary);
        command.arg("-c").arg(&config).current_dir(dir);
        SnellServer(Reference::start(
            "snell-server",
            command,
            vec![port],
            dir.join("snell-server.log"),
        ))
    }

    /// The loopback port.
    pub fn port(&self) -> u16 {
        self.0.port(0)
    }

    pub fn log_text(&self) -> String {
        self.0.log_text()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_configuration_stays_on_the_loopback() {
        for obfs_http in [false, true] {
            let text = render(4001, "sn3ll", obfs_http);
            for forbidden in ["0.0.0.0", "::", "egress-interface", "dns"] {
                assert!(!text.contains(forbidden), "`{forbidden}` in {text}");
            }
            let listen: Vec<&str> = text.lines().filter(|l| l.starts_with("listen")).collect();
            assert_eq!(listen, ["listen = 127.0.0.1:4001"]);
        }
    }

    #[test]
    fn the_configuration_is_written_as_snell_server_reads_it() {
        assert_eq!(
            render(4001, "sn3ll", false),
            "[snell-server]\nlisten = 127.0.0.1:4001\npsk = sn3ll\nipv6 = false\n"
        );
        assert_eq!(
            render(4002, "sn3ll", true),
            "[snell-server]\nlisten = 127.0.0.1:4002\npsk = sn3ll\nipv6 = false\nobfs = http\n"
        );
    }
}
```

新建 `tests/interop/tests/snell.rs`：

```rust
//! rurge's `snell` outbound against sing-box's `snell` inbound (`version: 5`,
//! which takes v4 clients too) and Surge's official snell-server v5.0.1
//! (Linux only), phase 2 M6 design 4.4: v4 and v5 over TCP with one record
//! and with many, `reuse=true` with requests one after another, `obfs=http`,
//! and UDP over TCP. Every target is a loopback IP literal.

mod common;

use common::*;
use rurge_interop::snell_server::{SnellServer, snell_server_or_skip};

const PSK: &str = "sn3ll-interop";

/// `name = snell, 127.0.0.1, <port>, psk=…, <params>`.
fn line(name: &str, port: u16, params: &str) -> String {
    format!("{name} = snell, 127.0.0.1, {port}, psk={PSK}, {params}\n")
}

fn profile(lines: &[String]) -> String {
    format!("[Proxy]\n{}[Rule]\nFINAL,DIRECT\n", lines.concat())
}

fn snell_inbound(obfs_http: bool) -> Inbound {
    plain(InboundKind::Snell { obfs_http }, &[("ignored", PSK)])
}

/// `count` requests one after another, each ended cleanly both ways, so
/// that with `reuse=true` each one hands its connection to the next.
/// Bounded like `roundtrip`.
async fn requests_one_after_another(out: &OutboundRef, echo: SocketAddr, count: usize) {
    let bound = std::time::Duration::from_secs(10);
    for i in 0..count {
        let mut stream = tokio::time::timeout(
            bound,
            out.connect_tcp(&target(echo), &ConnectOpts::default()),
        )
        .await
        .expect("the tunnel is established within the bound")
        .expect("the tunnel is established");
        let payload = format!("request {i}");
        let mut back = vec![0u8; payload.len()];
        let exchange = async {
            stream.write_all(payload.as_bytes()).await.unwrap();
            stream.read_exact(&mut back).await.unwrap();
            stream.shutdown().await.unwrap();
            // the target's end comes back as the server's end of the request
            let mut rest = Vec::new();
            stream.read_to_end(&mut rest).await.unwrap();
            rest
        };
        let rest = tokio::time::timeout(bound, exchange)
            .await
            .expect("the request completes within the bound");
        assert_eq!((back.as_slice(), rest.len()), (payload.as_bytes(), 0));
    }
}

/// Each policy: one small and one large TCP round trip.
async fn tcp(profile: &str, names: &[&str]) {
    let echo = echo_server().await;
    for name in names {
        let out = outbound(profile, name, None);
        roundtrip(&out, echo).await;
        roundtrip_big(&out, echo).await;
    }
}

/// Each policy: a UDP carrier, two datagrams there and back.
async fn udp(profile: &str, names: &[&str]) {
    let echo = udp_echo_server().await;
    for name in names {
        udp_roundtrip(&outbound(profile, name, None), echo).await;
    }
}

#[tokio::test]
async fn v5_and_v4_clients_against_sing_box() {
    let Some(bin) = sing_box_or_skip("v5_and_v4_clients_against_sing_box") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let sb = SingBox::spawn(&bin, dir.path(), vec![snell_inbound(false)]);
    let profile = profile(&[
        line("V5", sb.port(0), "version=5"),
        line("V4", sb.port(0), "version=4"),
    ]);
    tcp(&profile, &["V5", "V4"]).await;
}

#[tokio::test]
async fn reuse_against_sing_box() {
    let Some(bin) = sing_box_or_skip("reuse_against_sing_box") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let sb = SingBox::spawn(&bin, dir.path(), vec![snell_inbound(false)]);
    let profile = profile(&[line("Reuse", sb.port(0), "version=5, reuse=true")]);
    let (out, echo) = (outbound(&profile, "Reuse", None), echo_server().await);
    requests_one_after_another(&out, echo, 4).await;
    roundtrip_big(&out, echo).await;
}

#[tokio::test]
async fn obfs_http_against_sing_box() {
    let Some(bin) = sing_box_or_skip("obfs_http_against_sing_box") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let sb = SingBox::spawn(&bin, dir.path(), vec![snell_inbound(true)]);
    let profile = profile(&[
        line("Obfs", sb.port(0), "version=5, obfs=http"),
        line("ObfsReuse", sb.port(0), "version=5, obfs=http, reuse=true"),
    ]);
    tcp(&profile, &["Obfs"]).await;
    let out = outbound(&profile, "ObfsReuse", None);
    requests_one_after_another(&out, echo_server().await, 3).await;
    udp(&profile, &["Obfs"]).await;
}

#[tokio::test]
async fn udp_against_sing_box() {
    let Some(bin) = sing_box_or_skip("udp_against_sing_box") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let sb = SingBox::spawn(&bin, dir.path(), vec![snell_inbound(false)]);
    let profile = profile(&[
        line("V5", sb.port(0), "version=5"),
        line("V4", sb.port(0), "version=4"),
    ]);
    udp(&profile, &["V5", "V4"]).await;
}

#[tokio::test]
async fn v5_and_v4_clients_against_snell_server() {
    let Some(bin) = snell_server_or_skip("v5_and_v4_clients_against_snell_server") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let server = SnellServer::spawn(&bin, dir.path(), PSK, false);
    let profile = profile(&[
        line("V5", server.port(), "version=5"),
        line("V4", server.port(), "version=4"),
    ]);
    tcp(&profile, &["V5", "V4"]).await;
}

#[tokio::test]
async fn reuse_against_snell_server() {
    let Some(bin) = snell_server_or_skip("reuse_against_snell_server") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let server = SnellServer::spawn(&bin, dir.path(), PSK, false);
    let profile = profile(&[line("Reuse", server.port(), "version=5, reuse=true")]);
    let (out, echo) = (outbound(&profile, "Reuse", None), echo_server().await);
    requests_one_after_another(&out, echo, 4).await;
    roundtrip_big(&out, echo).await;
}

#[tokio::test]
async fn obfs_http_against_snell_server() {
    let Some(bin) = snell_server_or_skip("obfs_http_against_snell_server") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let server = SnellServer::spawn(&bin, dir.path(), PSK, true);
    let profile = profile(&[
        line("Obfs", server.port(), "version=5, obfs=http"),
        line(
            "ObfsReuse",
            server.port(),
            "version=5, obfs=http, reuse=true",
        ),
    ]);
    tcp(&profile, &["Obfs"]).await;
    let out = outbound(&profile, "ObfsReuse", None);
    requests_one_after_another(&out, echo_server().await, 3).await;
    udp(&profile, &["Obfs"]).await;
}

#[tokio::test]
async fn udp_against_snell_server() {
    let Some(bin) = snell_server_or_skip("udp_against_snell_server") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let server = SnellServer::spawn(&bin, dir.path(), PSK, false);
    let profile = profile(&[
        line("V5", server.port(), "version=5"),
        line("V4", server.port(), "version=4"),
    ]);
    udp(&profile, &["V5", "V4"]).await;
}
```

- [ ] **Step 2: 运行**

Run: `cargo test -p rurge-interop`
Expected: 本机没有 sing-box 与 snell-server 时，8 条 Snell 用例（sing-box 4 条、snell-server 4 条）各打印一行 `skipping …` 后通过，夹具的单元用例照常断言并通过。互操作由首次推送后的 CI 证明。

- [ ] **Step 3: CI 与文档**

CI：sing-box 升到 1.14.2（三个 SHA-256 取自 GitHub 发布资产的 `digest` 字段）；snell-server 只在 Linux 作业里按 zip 的 SHA-256 安装：

`.github/workflows/ci.yml`——把

```yaml
          version=1.14.1
          case "$RUNNER_OS" in
            Linux)   asset="sing-box-$version-linux-amd64.tar.gz";  sha=12cb2816b52febb356f6a885b740cc8758c3f30b8ae0ca8edba80f0d2d35343f ;;
            Windows) asset="sing-box-$version-windows-amd64.zip";   sha=5197f16d492d93202dc623622149a6ed040f8eca263128f91d603f2b901baa89 ;;
            macOS)   asset="sing-box-$version-darwin-arm64.tar.gz"; sha=b9024642ef7b4848252df5469b7f60ef3c18bb5e217a16a0934f0174f8ad11b4 ;;
```

换成

```yaml
          version=1.14.2
          case "$RUNNER_OS" in
            Linux)   asset="sing-box-$version-linux-amd64.tar.gz";  sha=a684484d7477d1437282ee411f4d131d0340aaad60a7868841ebd5d87dd8a0c6 ;;
            Windows) asset="sing-box-$version-windows-amd64.zip";   sha=c2d8bfff918755808781dfdeeb8581b6c91eb3a243d9a7b55483cfc0c0684d32 ;;
            macOS)   asset="sing-box-$version-darwin-arm64.tar.gz"; sha=925c5382eca8492b0150f868a6db20b18290a38700e621724b3703fd453e032d ;;
```

`.github/workflows/ci.yml`——把

```yaml
          echo "RURGE_TEST_SSSERVER=$bin" >> "$GITHUB_ENV"
```

换成

```yaml
          echo "RURGE_TEST_SSSERVER=$bin" >> "$GITHUB_ENV"
      - name: Install snell-server for the Snell interoperability tests
        if: runner.os == 'Linux'
        shell: bash
        run: |
          set -euo pipefail
          version=5.0.1
          asset="snell-server-v$version-linux-amd64.zip"
          sha=9bea1c2b9e35b73b31634856c04d18c393072b9e5dcde6a32781d8b8f908c539
          cd "$RUNNER_TEMP"
          curl -fsSL --retry 3 --retry-all-errors -o "$asset" "https://dl.nssurge.com/snell/$asset"
          actual=$(sha256sum "$asset" | cut -d' ' -f1)
          if [ "$actual" != "$sha" ]; then
            echo "snell-server checksum mismatch: expected $sha, got $actual"
            exit 1
          fi
          mkdir -p snell-server
          unzip -q "$asset" -d snell-server
          bin=$(find "$PWD/snell-server" -type f -name snell-server | head -n 1)
          [ -n "$bin" ] || { echo "no snell-server binary in $asset"; exit 1; }
          chmod +x "$bin"
          echo "RURGE_TEST_SNELL_SERVER=$bin" >> "$GITHUB_ENV"
```

`tests/interop/README.md`——把

```markdown
`rurge-interop` 是仅供测试使用的工作区成员（`publish = false`），不对外发布，也不是任何其它 crate 的依赖。它把 [sing-box](https://sing-box.sagernet.org/)、[xray](https://github.com/XTLS/Xray-core) 与 [shadowsocks-rust](https://github.com/shadowsocks/shadowsocks-rust) 的 `ssserver` 作为参照实现，以回环子进程的方式拉起来，驱动 rurge 的 `http` / `https` / `socks5` / `trojan` / `vmess` / `anytls` / `wireguard` / `ss` 出站，以及包在 Shadow TLS 里的 `trojan`，去连它们，验证 rurge 与真实的第三方实现互通。xray 只用来跑 `vmess`：VMess 协议由 xray 所在的这一脉实现定义，sing-box 的实现是重写，手写的编解码需要两个独立参照互相印证（M2 设计 M2-D5）。
```

换成

```markdown
`rurge-interop` 是仅供测试使用的工作区成员（`publish = false`），不对外发布，也不是任何其它 crate 的依赖。它把 [sing-box](https://sing-box.sagernet.org/)、[xray](https://github.com/XTLS/Xray-core)、[shadowsocks-rust](https://github.com/shadowsocks/shadowsocks-rust) 的 `ssserver` 与 Surge 官方的 `snell-server` 作为参照实现，以回环子进程的方式拉起来，驱动 rurge 的 `http` / `https` / `socks5` / `trojan` / `vmess` / `anytls` / `wireguard` / `ss` / `snell` 出站，以及包在 Shadow TLS 里的 `trojan`，去连它们，验证 rurge 与真实的第三方实现互通。xray 只用来跑 `vmess`：VMess 协议由 xray 所在的这一脉实现定义，sing-box 的实现是重写，手写的编解码需要两个独立参照互相印证（M2 设计 M2-D5）。
```

`tests/interop/README.md`——把

```markdown
互操作测试固定 sing-box **1.14.1**（2026-09-15 发布的稳定版）。CI 下载并校验以下三个发布包：
```

换成

```markdown
互操作测试固定 sing-box **1.14.2**（2026-09-24 发布的稳定版；阶段 2 / M6b 从 1.14.1 升上来：`snell` 入站 1.14.0 起才有，1.14.2 是 1.14.x 的最新版）。CI 下载并校验以下三个发布包（SHA-256 取自 GitHub 发布 API 每个资产的 `digest`）：
```

`tests/interop/README.md`——把

```markdown
| Linux (amd64) | `sing-box-1.14.1-linux-amd64.tar.gz` | `12cb2816b52febb356f6a885b740cc8758c3f30b8ae0ca8edba80f0d2d35343f` |
| Windows (amd64) | `sing-box-1.14.1-windows-amd64.zip` | `5197f16d492d93202dc623622149a6ed040f8eca263128f91d603f2b901baa89` |
| macOS (arm64) | `sing-box-1.14.1-darwin-arm64.tar.gz` | `b9024642ef7b4848252df5469b7f60ef3c18bb5e217a16a0934f0174f8ad11b4` |
```

换成

```markdown
| Linux (amd64) | `sing-box-1.14.2-linux-amd64.tar.gz` | `a684484d7477d1437282ee411f4d131d0340aaad60a7868841ebd5d87dd8a0c6` |
| Windows (amd64) | `sing-box-1.14.2-windows-amd64.zip` | `c2d8bfff918755808781dfdeeb8581b6c91eb3a243d9a7b55483cfc0c0684d32` |
| macOS (arm64) | `sing-box-1.14.2-darwin-arm64.tar.gz` | `925c5382eca8492b0150f868a6db20b18290a38700e621724b3703fd453e032d` |
```

`tests/interop/README.md`——把

```markdown
本地默认不安装 sing-box、xray 与 shadowsocks-rust：`cargo test -p rurge-interop` 会正常通过，sing-box 的十五个互操作用例、xray 的两个与 shadowsocks-rust 的两个互操作用例各打印一行 `skipping …` 后直接返回（各夹具自身的单元测试——配置渲染、"配置绝不碰本机"的安全守卫、取空闲端口——照常运行并断言）。要在本机真正跑 sing-box 的用例，二选一：

- 自行安装 sing-box 1.14.1，让 `sing-box` / `sing-box.exe` 出现在 `PATH` 上；或
- 不安装到 `PATH`，改用 `RURGE_TEST_SING_BOX=<sing-box 可执行文件路径> cargo test -p rurge-interop`。

xray 与 shadowsocks-rust 的本机运行方式同理，见下面「xray」「shadowsocks-rust」两节。
```

换成

```markdown
本地默认不安装 sing-box、xray、shadowsocks-rust 与 snell-server：`cargo test -p rurge-interop` 会正常通过，sing-box 的十九个互操作用例、xray 的两个、shadowsocks-rust 的两个与 snell-server 的四个互操作用例各打印一行 `skipping …` 后直接返回（各夹具自身的单元测试——配置渲染、"配置绝不碰本机"的安全守卫、取空闲端口——照常运行并断言）。要在本机真正跑 sing-box 的用例，二选一：

- 自行安装 sing-box 1.14.2，让 `sing-box` / `sing-box.exe` 出现在 `PATH` 上；或
- 不安装到 `PATH`，改用 `RURGE_TEST_SING_BOX=<sing-box 可执行文件路径> cargo test -p rurge-interop`。

xray、shadowsocks-rust 与 snell-server 的本机运行方式同理，见下面「xray」「shadowsocks-rust」「snell-server」三节。
```

`tests/interop/README.md`——把

```markdown
- `RURGE_INTEROP_REQUIRED=1`：找不到二进制时让用例直接失败，而不是打印 `skipping …` 后跳过；对 sing-box、xray、shadowsocks-rust 与 sshd 各夹具都生效。CI 会设置它，本机一般不需要。
```

换成

```markdown
- `RURGE_TEST_SNELL_SERVER`：Surge 官方 `snell-server` 可执行文件的路径，优先于 `PATH` 查找。
- `RURGE_INTEROP_REQUIRED=1`：找不到二进制时让用例直接失败，而不是打印 `skipping …` 后跳过；对 sing-box、xray、shadowsocks-rust 与 sshd 各夹具都生效，对 snell-server 只在 Linux 上生效（它只有 Linux 版）。CI 会设置它，本机一般不需要。
```

`tests/interop/README.md`——把

```markdown
- Shadowsocks（`tests/shadowsocks.rs` 里的 `ss_against_sing_box_aead_2022_and_a_user`，阶段 2 / M6a）：sing-box 的 `shadowsocks` 入站，`aes-192-gcm` 与 `xchacha20-ietf-poly1305` 两种 AEAD 方法（shadowsocks-rust 的发布包不带这两种，见下面「shadowsocks-rust」一节）、单密钥的 `2022-blake3-aes-256-gcm`，以及 `2022-blake3-aes-128-gcm` 两个用户里的第二个（`password=服务端密钥:用户密钥`，一层身份头）；每种都做单块与跨多块（100 000 字节）的 TCP 往返，以及经 `udp-relay=true` 往返一个回环 UDP 回显两次。
```

换成

```markdown
- Shadowsocks（`tests/shadowsocks.rs` 里的 `ss_against_sing_box_aead_2022_and_a_user`，阶段 2 / M6a）：sing-box 的 `shadowsocks` 入站，`aes-192-gcm` 与 `xchacha20-ietf-poly1305` 两种 AEAD 方法（shadowsocks-rust 的发布包不带这两种，见下面「shadowsocks-rust」一节）、单密钥的 `2022-blake3-aes-256-gcm`，以及 `2022-blake3-aes-128-gcm` 两个用户里的第二个（`password=服务端密钥:用户密钥`，一层身份头）；每种都做单块与跨多块（100 000 字节）的 TCP 往返，以及经 `udp-relay=true` 往返一个回环 UDP 回显两次。

- Snell（`tests/snell.rs` 里 `…_against_sing_box` 的四个用例，阶段 2 / M6b）：sing-box 的 `snell` 入站（1.14.0 起），`version: 5`、一个 `psk`、不配 `users`（服务端不看 client id）。sing-box 没有"version 4"的入站；`version: 5` 走 v4 / v5 共用的线上格式，所以同时接受 v4 客户端。覆盖 `version=5` 与 `version=4` 两种客户端的单块与跨多块（100 000 字节）TCP 往返；`reuse=true` 时一连四个请求（每个都两个方向干净结束，连接回池给下一个请求用）再加一个大负载；`obfs=http`（入站的 `obfs_mode: http`；含 `reuse=true` 与 UDP）；以及 UDP over TCP（命令 `0x06`）经同一个载体往返一个回环 UDP 回显两次。
```

`tests/interop/README.md`——把

```markdown
渲染出的配置（`rurge_interop::shadowsocks_rust::render`）只有 `servers` 一个顶层键：每个服务端只监听 `127.0.0.1`，`mode` 为 `tcp_and_udp`，没有 `manager`、`locals`、插件、ACL 与 `outbound_*` 这些键（夹具的单元用例 `the_configuration_stays_on_the_loopback` 断言这一点）。

## sshd
```

换成

```markdown
渲染出的配置（`rurge_interop::shadowsocks_rust::render`）只有 `servers` 一个顶层键：每个服务端只监听 `127.0.0.1`，`mode` 为 `tcp_and_udp`，没有 `manager`、`locals`、插件、ACL 与 `outbound_*` 这些键（夹具的单元用例 `the_configuration_stays_on_the_loopback` 断言这一点）。

## snell-server

Surge 官方的 `snell-server`（发布在 Surge 知识库）是 `snell` 的第二个参照：sing-box 的实现是重写，线上格式需要两个独立参照互相印证（阶段 2 / M6 设计 M6-D5）。互操作测试固定 **v5.0.1**，只有 Linux (amd64) 版：

| 平台 | 资产 | SHA-256 |
| ---- | ---- | ------- |
| Linux (amd64) | `snell-server-v5.0.1-linux-amd64.zip`（`https://dl.nssurge.com/snell/`） | `9bea1c2b9e35b73b31634856c04d18c393072b9e5dcde6a32781d8b8f908c539` |

知识库不公布校验和；上面的值是计划期对该 zip 自行计算的（解压出的 `snell-server` 的 SHA-256 为 `5b2e221f2c6e29b1db8e47053e1221be29d5627da807cb932b089f514a3609f0`，与第三方项目钉住的值一致）。二进制经 UPX 压缩。

`tests/snell.rs` 里 `…_against_snell_server` 的四个用例覆盖：`version=5` 与 `version=4` 两种客户端的单块与跨多块 TCP 往返；`reuse=true` 时一连四个请求加一个大负载；`obfs = http`（含 `reuse=true` 与 UDP）；UDP over TCP 往返一个回环 UDP 回显两次。

CI 只在 Linux 上下载、校验并设置 `RURGE_TEST_SNELL_SERVER`；Windows 与 macOS 上这四个用例照常编译，运行时打印 `skipping …` 后返回（`RURGE_INTEROP_REQUIRED=1` 对 snell-server 只在 Linux 上生效）。本机不安装它：`RURGE_TEST_SNELL_SERVER`（优先于 `PATH` 查找）没有指向可执行文件、`PATH` 上也找不到 `snell-server` 时同样跳过；这个 crate 不会下载或安装它。

渲染出的配置（`rurge_interop::snell_server::render`）是一个 `[snell-server]` 节：`listen = 127.0.0.1:<端口>`、`psk`、`ipv6 = false`，按需加 `obfs = http`；没有 `dns`、`egress-interface` 这些键（夹具的单元用例 `the_configuration_stays_on_the_loopback` 断言只监听回环）。启动命令是 `snell-server -c <配置文件>`，就绪与否看 TCP 端口能否连上。

## sshd
```

`tests/interop/README.md`——把

```markdown
- 夹具渲染出的 sing-box 配置只有 `log` / `inbounds` / `outbounds` 三个顶层键（WireGuard 的配置另有 `endpoints`）；每个入站只监听 `127.0.0.1`；唯一的出站是 `direct`。WireGuard 端点在用户态运行（`system: false`），它的 UDP 端口开在所有地址上（端点没有监听地址这一项；`rurge_interop::render_wireguard` 的单元测试 `the_wireguard_configuration_never_touches_the_machine` 断言其余各项）。xray 配置同样只有这三个顶层键，唯一的出站是 `freedom`。`ssserver` 的配置只有 `servers`，每个服务端只听 `127.0.0.1`（TCP 与同号的 UDP）。任何地方都不出现 `set_system_proxy`、`tun`、`auto_route` 这些键（`rurge_interop::render` 与 `rurge_interop::xray::render` 的单元测试 `the_configuration_never_touches_the_machine` 各自断言这一点）；`shadowtls` 入站的 `handshake.server` 恒为 `127.0.0.1`（夹具的单元用例断言）。
- 每个用例的连接目标都是回环 IP 字面量（`127.0.0.1` 上的 echo / 测试服务器；WireGuard 用例在隧道里连的是 sing-box 自己的隧道地址 `10.9.0.1`，由 sing-box 映射到它的 `127.0.0.1`），sing-box、xray 与 `ssserver` 因此既不解析域名也不会访问公网。
- 这个 crate 本身不下载、不安装任何东西；本机是否装有 sing-box、xray 或 shadowsocks-rust 由项目所有者决定，没装就跳过。
```

换成

```markdown
- 夹具渲染出的 sing-box 配置只有 `log` / `inbounds` / `outbounds` 三个顶层键（WireGuard 的配置另有 `endpoints`）；每个入站只监听 `127.0.0.1`；唯一的出站是 `direct`。WireGuard 端点在用户态运行（`system: false`），它的 UDP 端口开在所有地址上（端点没有监听地址这一项；`rurge_interop::render_wireguard` 的单元测试 `the_wireguard_configuration_never_touches_the_machine` 断言其余各项）。xray 配置同样只有这三个顶层键，唯一的出站是 `freedom`。`ssserver` 的配置只有 `servers`，每个服务端只听 `127.0.0.1`（TCP 与同号的 UDP）。`snell-server` 的配置只有 `[snell-server]` 一节，只听 `127.0.0.1`。任何地方都不出现 `set_system_proxy`、`tun`、`auto_route` 这些键（`rurge_interop::render` 与 `rurge_interop::xray::render` 的单元测试 `the_configuration_never_touches_the_machine` 各自断言这一点）；`shadowtls` 入站的 `handshake.server` 恒为 `127.0.0.1`（夹具的单元用例断言）。
- 每个用例的连接目标都是回环 IP 字面量（`127.0.0.1` 上的 echo / 测试服务器；WireGuard 用例在隧道里连的是 sing-box 自己的隧道地址 `10.9.0.1`，由 sing-box 映射到它的 `127.0.0.1`），sing-box、xray、`ssserver` 与 `snell-server` 因此既不解析域名也不会访问公网。
- 这个 crate 本身不下载、不安装任何东西；本机是否装有 sing-box、xray、shadowsocks-rust 或 snell-server 由项目所有者决定，没装就跳过。
```

兼容性清单（4.2 的 `snell` 行与 `W0007` 行、4.5、4.6；P4、P6、P9、P11 登记为差异）：

`docs/surge-compatibility-matrix.md`——把

```markdown
| `snell` | Snell v1–v6 | v6 需 iOS 5.20 / Mac 6.7+ | 🟡 | 2 | v1–v4 计划支持；v5（QUIC Proxy Mode）与 v6（PSK 派生协议画像、流量整形，beta）协议细节未公开，❓ 待评估 |
```

换成

```markdown
| `snell` | Snell v1–v6 | v6 需 iOS 5.20 / Mac 6.7+ | 🟡 | 2 | M6b（阶段 2）已实现 v4 / v5 的 TCP 与 UDP（两个版本在 TCP 上的线上格式相同，v5 服务端接受 v4 客户端；`version` 须与服务端相符）。**v1–v3 与 v6 只解析**：加载时 `W0007`（`` `snell` version <n> is not implemented; rurge supports versions 4 and 5 (`version` must match the server); such policies behave as REJECT ``，每个版本每次加载一条；Surge 里 `version` 缺省为 1，所以**没写 `version` 的行按 v1 拒绝**，文本写作 `version 1 (the default when `version` is not written)`），运行时 `REJECT`，会话日志 `policy protocol not implemented: snell v<n>`；订阅导入的这类行跳过并告警。v5 的 QUIC Proxy Mode 不实现（UDP 一律经 TCP）；v5 的动态帧大小不实现：rurge 发送的每帧负载都以 0x3FFF 字节为上限、只有本方向第一帧带 256–511 字节的随机填充（Surge 首帧约 1 KiB、之后逐步增大、空闲 31 秒后复原），接收照常接受任何长度。密钥：每个方向一个 16 字节随机 salt，Argon2id（t=3、m=8 KiB、p=1）取前 16 字节作 AES-128-GCM 密钥，每次握手算一次、放在阻塞线程上。请求 `01 命令 00 主机名长度 主机名 端口`：主机名按文本写（IP 字面量也是，IPv6 不带方括号，IDN 转成 A-label；写不出的名字与超过 255 字节的名字不拨号即失败，`snell: the host name cannot be sent to the server` / `snell: the host name is longer than 255 bytes`），不带 client id；请求头与首段负载合并成一帧写出，客户端 100 ms 内不发数据时（服务端先说话的协议）请求头单独发出、这类协议的首字节因此晚 100 ms。命令：`reuse=false` 时 Connect（`0x01`），`reuse=true` 时 ConnectV2（`0x05`）（据报 Surge 总是发 `0x05`，未核对）。**`reuse=true`**：一个请求两个方向都以空帧干净结束后，连接回到该策略自己的池（至多 8 条空闲连接，最旧的先丢；空闲 60 秒回收，每 30 秒检查一次），下一个请求在它上面发出（不再有 salt 与填充）；没结束就被丢下的请求在后台补发结束帧、读完服务端剩余的数据（至多 0x80001 字节、10 秒）后再回池；被拒、出错或还没收到应答的连接不回池；从池里取出的连接若在第一个应答字节之前失效（服务端已关闭），换一条新连接重发一次（已写出的负载至多 64 KiB 可重放，超过则不重试）。服务端的应答：`00` 为通、`02` 为拒绝——`snell: the server refused: <服务端文本>`（只留可打印 ASCII、至多 200 字节），其它首字节 `snell: the server answered with an unknown reply`；psk 错或版本不符在连接期无法可靠识别，服务端不应答就关闭时是 `snell: the server closed the connection without answering`，解不开的数据是 `snell: the server's data failed to decrypt (wrong psk or version?)`；错误文本与日志都不含 psk。`obfs`：v4 / v5 只有 `http`（simple-obfs 的 `http`，每条 TCP 连接一次，复用的连接不再重复），伪装层在 Shadow TLS 之上、协议之下；**`obfs-host` 缺省为服务器主机名，Surge 的缺省据报是 `bing.com`（登记的差异；官方服务端不看请求头，只影响伪装）**；`Host` 在服务器端口不是 80 时带 `:端口`（同 `ss`）。**UDP**（v4 / v5 自动，不需要参数）：UDP over TCP——每个 UDP 载体一条自己的连接（命令 `0x06`，从不进池也不取自池），经与 TCP 相同的各层（Shadow TLS、obfs），每个数据报一帧，发出的数据报带目标（主机名或 IP 字面量），回包带来源 IP 与端口，全锥；**`udp-port` 写了时是这条 UDP 会话的 TCP 连接所连的端口**（缺省主端口；Surge 里它是否另指 v5 QUIC Proxy Mode 的 UDP 端口未核对，登记的差异）；一帧放不下的数据报（1 字节标记 + 地址 + 负载超过 0x3FFF 字节，IPv4 目标的负载至多 16374 字节）不发送、以 `snell: a datagram longer than <n> bytes` 失败；服务端发来的不是数据报的帧丢弃（`debug` 日志只写原因）；连接结束即载体结束（`snell: the server closed the UDP connection`），下一个包由引擎重开载体。可叠 Shadow TLS 与 `underlying-proxy`；TLS 参数不适用。 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| 策略指向 rurge 尚未实现的协议类型 | Surge 原生支持全部协议 | 全部 | 🟡 | 1 / 2 | 加载时告警 `W0007`；运行时该策略按 `REJECT` 处理，会话日志 `error = policy protocol not implemented: <type>`；随阶段 2 各里程碑逐协议移除：M1 已移除 `http` `https` `socks5` `socks5-tls`；M2a 已移除 `trojan`；M2b 已移除 `vmess`（写了 `vmess-aead=true` 的行）与 `anytls`；M4a 已移除 `ssh`；M4b 已移除 `wireguard`；M4c 已移除 `external`；M6a 已移除 `ss`（流式旧方法除外）。例外有两个：没写 `vmess-aead=true` 的 `vmess` 行仍按 `W0007` 处理，但走专门的诊断文本 `` `vmess` without `vmess-aead=true` uses the legacy handshake, which is not implemented yet ``（每次加载一条，不是每行一条）与专门的会话日志文本 `policy protocol not implemented: vmess (legacy handshake)`；流式旧方法的 `ss` 行同样如此，诊断文本 `` `ss` stream cipher `<method>` is not implemented yet; such policies behave as REJECT ``（每种方法每次加载一条），会话日志 `policy protocol not implemented: ss (<method>)`；两者都不是这里的通用 `<type>` 模板 |
```

换成

```markdown
| 策略指向 rurge 尚未实现的协议类型 | Surge 原生支持全部协议 | 全部 | 🟡 | 1 / 2 | 加载时告警 `W0007`；运行时该策略按 `REJECT` 处理，会话日志 `error = policy protocol not implemented: <type>`；随阶段 2 各里程碑逐协议移除：M1 已移除 `http` `https` `socks5` `socks5-tls`；M2a 已移除 `trojan`；M2b 已移除 `vmess`（写了 `vmess-aead=true` 的行）与 `anytls`；M4a 已移除 `ssh`；M4b 已移除 `wireguard`；M4c 已移除 `external`；M6a 已移除 `ss`（流式旧方法除外）；M6b 已移除 `snell`（v4 / v5）。例外有三个：没写 `vmess-aead=true` 的 `vmess` 行仍按 `W0007` 处理，但走专门的诊断文本 `` `vmess` without `vmess-aead=true` uses the legacy handshake, which is not implemented yet ``（每次加载一条，不是每行一条）与专门的会话日志文本 `policy protocol not implemented: vmess (legacy handshake)`；流式旧方法的 `ss` 行同样如此，诊断文本 `` `ss` stream cipher `<method>` is not implemented yet; such policies behave as REJECT ``（每种方法每次加载一条），会话日志 `policy protocol not implemented: ss (<method>)`；`version` 为 1–3 或 6（含没写 `version` 的，Surge 的缺省是 1）的 `snell` 行也是，诊断文本 `` `snell` version <n> is not implemented; rurge supports versions 4 and 5 (`version` must match the server); such policies behave as REJECT ``（每个版本每次加载一条），会话日志 `policy protocol not implemented: snell v<n>`；三者都不是这里的通用 `<type>` 模板 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `udp-port`（端口；默认主端口） | 适用 Shadowsocks / Snell | ✅ M6a：Shadowsocks 已生效（1–65535，其它取值 `E0018`；没写 `udp-relay=true` 时不用）；Snell 随 M6b | 2 |
| 自动支持 UDP 的协议：Snell v3+、VMess、Trojan、TUIC、Hysteria 2、MASQUE、AnyTLS（UDP over TCP）、WireGuard、Tailscale | | ✅ M5b：VMess（对称型）、Trojan、AnyTLS 已生效；M5c：WireGuard 已生效；其余随各自的协议 | 2 |
```

换成

```markdown
| `udp-port`（端口；默认主端口） | 适用 Shadowsocks / Snell | ✅ M6a：Shadowsocks 已生效（1–65535，其它取值 `E0018`；没写 `udp-relay=true` 时不用）；M6b：Snell 已生效——Snell 的 UDP 经 TCP，`udp-port` 是 UDP 会话那条 TCP 连接所连的端口（与 Surge 可能不同，见 4.2 `snell` 一行） | 2 |
| 自动支持 UDP 的协议：Snell v3+、VMess、Trojan、TUIC、Hysteria 2、MASQUE、AnyTLS（UDP over TCP）、WireGuard、Tailscale | | ✅ M5b：VMess（对称型）、Trojan、AnyTLS 已生效；M5c：WireGuard 已生效；M6b：Snell v4 / v5 已生效（UDP over TCP，全锥；v1–v3 在 M8 之前不实现）；其余随各自的协议 | 2 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `snell` | `psk` `version`（1–6，默认 1）`reuse`（v4+）`obfs`（v1–3 `http`/`tls`；v4–5 `http`；v6 无）`obfs-host` `obfs-uri` `udp-port` `mode`（v6：`default` `unshaped` `unsafe-raw`） | 🟡 | 2 | 版本支持范围见 4.2；v3+ 自动 UDP |
```

换成

```markdown
| `snell` | `psk` `version`（1–6，默认 1）`reuse`（v4+）`obfs`（v1–3 `http`/`tls`；v4–5 `http`；v6 无）`obfs-host` `obfs-uri` `udp-port` `mode`（v6：`default` `unshaped` `unsafe-raw`） | 🟡 | 2 | M6b 已实现 v4 / v5（版本支持范围与行为见 4.2）。`psk` 只读命名写法、必填（`E0018` `` `psk` is required ``，不引用取值）；`version` 须是 1–6 的整数（其它写法 `E0018`，引用所写的取值）；`reuse` 缺省 false；`obfs` 在 v4 / v5 只接受 `http`（`tls` 为 `E0018`），v1–v3 接受 `http` / `tls`，v6 写了 `obfs` / `obfs-host` / `obfs-uri` 为 `W0028` 并忽略；`obfs-host` / `obfs-uri` 的规则与缺省同 `ss`（`obfs-host` 缺省为服务器主机名，Surge 据报是 `bing.com`）；`udp-port` 1–65535（其它取值 `E0018`），含义见 4.5；`mode` 只对 v6 校验（其它取值 `E0018`），别的版本写了 `W0028` 并忽略；Snell 没有 `udp-relay` 参数（v3+ 自动 UDP），写了按未知参数 `W0001`；TLS 参数（`sni` 等）不适用于 `snell`（`W0028`）；可叠 Shadow TLS 与 `underlying-proxy` |
```

`docs/acceptance/phase2-manual.md`——把

```markdown
- [ ] 脱敏：`GET /v1/policies/detail?policy_name=<策略名>` 与 `GET /v1/profiles/current` 里 `password` 与 `obfs-host` 都是 `***`。

```

换成

```markdown
- [ ] 脱敏：`GET /v1/policies/detail?policy_name=<策略名>` 与 `GET /v1/profiles/current` 里 `password` 与 `obfs-host` 都是 `***`。

## M6b　Snell

前置：同 M5a 一节的 SOCKS5 UDP 客户端；自己的 Snell v5 节点（记下服务端实现与版本，如官方 snell-server v5.0.1 或 sing-box 的 `snell` 入站），另开一个同样的节点（或同一节点的第二个端口）配 `obfs = http`。每份配置 `[Rule]` 里 `FINAL,<策略名>`，`rurge check -c <配置>` 零错误（没有 `W0007`）。

- [ ] TCP：`snell` 策略写 `psk=<psk>, version=5`，`curl -x http://127.0.0.1:<http-listen 端口> https://example.com/ -I` 与 `curl --socks5-hostname 127.0.0.1:<socks5-listen 端口> https://example.com/ -I` 都返回 200，请求记录里策略链是该策略、`error` 为空；服务端先说话的协议（经代理连一个 SMTP / SSH 主机）能看到对端的欢迎行。把 `version` 改成 4，重载后同样可用（v5 服务端接受 v4 客户端）。
- [ ] `reuse`：`reuse=true` 时连续访问几个网站，抓包（或服务端日志）看到后面的请求走同一条 TCP 连接、没有新的握手；空闲 60 秒后这条连接被关掉。去掉 `reuse`（或 `reuse=false`）后每个请求各开一条连接。`reuse=true` 下服务端重启一次，之后的第一个请求照常成功（池里的旧连接失效时换新连接重试一次）。
- [ ] obfs：`obfs=http, obfs-host=<伪装域名>` 连配了 `obfs = http` 的节点，TCP 与 UDP 都能往返；抓包看到首个包是带 `Host: <伪装域名>:<端口>`（端口为 80 时不带端口）与 `Upgrade: websocket` 的 `GET` 请求；不写 `obfs-host` 时伪装域名是服务器主机名（Surge 用 `bing.com`，已知差异，见兼容性清单 `snell` 一行）。
- [ ] UDP：不需要参数（v4 / v5 自动支持 UDP），经 SOCKS5 UDP 发 DNS 查询（如 Proxifier 代理 `nslookup example.com 8.8.8.8`）得到回答；经它进行一次语音通话或联机游戏；用 NAT 类型检测工具（STUN）检测，结果是 Full Cone（节点的出口须是全锥）；抓包确认 UDP 是经一条 TCP 连接送到节点的（UDP over TCP），不写 `udp-port` 时连的是主端口。
- [ ] psk 错误：把 `psk` 改错一位，重载后访问 `https://example.com/`：请求失败，会话记录的 `error` 是 `snell: the server closed the connection without answering`（或 `snell: the server's data failed to decrypt (wrong psk or version?)`；记下实际是哪一个）；**错误文本与日志都不含 psk**。
- [ ] 版本不符：删掉 `version`（Surge 的缺省是 1），`rurge check` 报一条 `W0007`：`` `snell` version 1 (the default when `version` is not written) is not implemented; rurge supports versions 4 and 5 (`version` must match the server); such policies behave as REJECT ``；运行时这条策略 REJECT，会话记录写 `policy protocol not implemented: snell v1`。`version=6` 同样是 `W0007`，会话记录写 `snell v6`。
- [ ] 脱敏：`GET /v1/policies/detail?policy_name=<策略名>` 与 `GET /v1/profiles/current` 里 `psk` 与 `obfs-host` 都是 `***`。

```

`README.md`——把

```markdown
> **阶段 1 功能齐备：M1 ～ M4b**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）进行中：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；M3b（测速与自动组）已完成——`url-test` / `fallback` / `load-balance` 组按连通性测试（经各节点在同一条连接上两次 `HEAD`，HTTPS 测试 URL 在已建立的 TLS 连接上测第二次）自动选择，`interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` 与策略的 `test-url` / `test-timeout` 生效，自动组可经 `POST /v1/policy_groups/select` 临时覆盖，新增 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 三个端点；M3c（`smart`）已完成——`smart` 组按真实会话的首字节耗时与测速结果打分（失败罚分、`policy-priority` 因子、约 1 小时的站点记忆），选中的成员连不上时自动换下一个（最多再试 2 个），请求记录增加 `connectMs` / `firstByteMs`；M4a（SSH）已完成——`ssh` 策略（TCP；每个策略一条 SSH 会话、每个连接一个通道；口令或 `[Keystore]` 里的 OpenSSH 私钥登录；`server-fingerprint` 校验主机密钥；按 `idle-timeout` 断开空闲会话、30 秒一次保活）已可用；M4b（WireGuard）已完成——`wireguard` 策略（TCP；用户态隧道，不建虚拟网卡；多个 peer 按 `allowed-ips` 选路；`client-id`（WARP 的保留字节）；配了 `dns-server` 时目标域名经隧道查询，否则在本机解析；没有 `dns-server` 也没写 `test-url` 时以握手测速）已可用；M4c（external）已完成——`external` 策略（TCP；第一次用到时拉起外部程序、经它在本机端口上的 SOCKS5 转发；程序退出后下次用到时再拉起；输出写进数据目录；rurge 退出时连同它启动的进程一起停掉——Unix 用进程组，Windows 用 Job Object；三平台都支持，Surge 只在 Mac 上有）已可用，订阅导入的 `external` 一律跳过；M5a（UDP 地基）已完成——SOCKS5 监听支持 UDP ASSOCIATE，UDP 按规则分流到 DIRECT、REJECT 或 `socks5` / `socks5-tls` / `external`（`udp-relay=true`，含 `underlying-proxy` 链），全锥 NAT，每条 UDP 流一条请求记录，`block-quic` 与 `udp-policy-not-supported-behaviour` 生效；M5b（TLS 族的 UDP）已完成——`trojan`（UDP ASSOCIATE）、`anytls`（UDP over TCP v2）全锥，`vmess`（命令 2，每个目标一条连接）对称型；M5c（WireGuard 的 UDP 与其余）已完成——`wireguard` 的 UDP（全锥）与经 `underlying-proxy` 的隧道、`test-udp` / `proxy-test-udp`、`smart` 组计入 UDP、`dns-follow-interface`；M6（Shadowsocks / Snell / HTTP/2 族）进行中：M6a（Shadowsocks）已完成——`ss` 的 AEAD、`none` 与 SS 2022（含多用户身份头）方法、simple-obfs（`http` / `tls`），TCP 与 UDP（`udp-relay=true`、`udp-port`，全锥），流式旧方法在 M8 之前按 REJECT 处理；其余出站协议仍在阶段 2 后续里程碑，`subnet` 策略组在阶段 3。
```

换成

```markdown
> **阶段 1 功能齐备：M1 ～ M4b**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）进行中：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；M3b（测速与自动组）已完成——`url-test` / `fallback` / `load-balance` 组按连通性测试（经各节点在同一条连接上两次 `HEAD`，HTTPS 测试 URL 在已建立的 TLS 连接上测第二次）自动选择，`interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` 与策略的 `test-url` / `test-timeout` 生效，自动组可经 `POST /v1/policy_groups/select` 临时覆盖，新增 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 三个端点；M3c（`smart`）已完成——`smart` 组按真实会话的首字节耗时与测速结果打分（失败罚分、`policy-priority` 因子、约 1 小时的站点记忆），选中的成员连不上时自动换下一个（最多再试 2 个），请求记录增加 `connectMs` / `firstByteMs`；M4a（SSH）已完成——`ssh` 策略（TCP；每个策略一条 SSH 会话、每个连接一个通道；口令或 `[Keystore]` 里的 OpenSSH 私钥登录；`server-fingerprint` 校验主机密钥；按 `idle-timeout` 断开空闲会话、30 秒一次保活）已可用；M4b（WireGuard）已完成——`wireguard` 策略（TCP；用户态隧道，不建虚拟网卡；多个 peer 按 `allowed-ips` 选路；`client-id`（WARP 的保留字节）；配了 `dns-server` 时目标域名经隧道查询，否则在本机解析；没有 `dns-server` 也没写 `test-url` 时以握手测速）已可用；M4c（external）已完成——`external` 策略（TCP；第一次用到时拉起外部程序、经它在本机端口上的 SOCKS5 转发；程序退出后下次用到时再拉起；输出写进数据目录；rurge 退出时连同它启动的进程一起停掉——Unix 用进程组，Windows 用 Job Object；三平台都支持，Surge 只在 Mac 上有）已可用，订阅导入的 `external` 一律跳过；M5a（UDP 地基）已完成——SOCKS5 监听支持 UDP ASSOCIATE，UDP 按规则分流到 DIRECT、REJECT 或 `socks5` / `socks5-tls` / `external`（`udp-relay=true`，含 `underlying-proxy` 链），全锥 NAT，每条 UDP 流一条请求记录，`block-quic` 与 `udp-policy-not-supported-behaviour` 生效；M5b（TLS 族的 UDP）已完成——`trojan`（UDP ASSOCIATE）、`anytls`（UDP over TCP v2）全锥，`vmess`（命令 2，每个目标一条连接）对称型；M5c（WireGuard 的 UDP 与其余）已完成——`wireguard` 的 UDP（全锥）与经 `underlying-proxy` 的隧道、`test-udp` / `proxy-test-udp`、`smart` 组计入 UDP、`dns-follow-interface`；M6（Shadowsocks / Snell / HTTP/2 族）进行中：M6a（Shadowsocks）已完成——`ss` 的 AEAD、`none` 与 SS 2022（含多用户身份头）方法、simple-obfs（`http` / `tls`），TCP 与 UDP（`udp-relay=true`、`udp-port`，全锥），流式旧方法在 M8 之前按 REJECT 处理；M6b（Snell）已完成——`snell` v4 / v5 的 TCP 与 UDP（UDP over TCP，全锥）、`reuse`（连接回池给下一个请求用）、simple-obfs `http`，v1–v3 与 v6 按 REJECT 处理（没写 `version` 即 v1）；其余出站协议仍在阶段 2 后续里程碑，`subnet` 策略组在阶段 3。
```

`README.md`——把

```markdown
| 出站协议       | DIRECT / REJECT 系列 / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / 外部程序（DIRECT 与 REJECT 系四种已实现，M3a；HTTP / SOCKS5（含 `socks5-tls`）TCP 上游与 `underlying-proxy` 链已实现，阶段 2 / M1；Trojan（TCP；WebSocket 传输）已实现，阶段 2 / M2a；VMess（AEAD 握手，TCP；可叠加 TLS / WebSocket）与 AnyTLS（TCP，会话复用）已实现，阶段 2 / M2b；SSH（TCP，会话复用）已实现，阶段 2 / M4a；WireGuard（TCP，用户态隧道）已实现，阶段 2 / M4b；外部程序（TCP，三平台）已实现，阶段 2 / M4c；Shadowsocks（AEAD / 2022、obfs，TCP 与 UDP）已实现，阶段 2 / M6a） | 2     |
```

换成

```markdown
| 出站协议       | DIRECT / REJECT 系列 / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / 外部程序（DIRECT 与 REJECT 系四种已实现，M3a；HTTP / SOCKS5（含 `socks5-tls`）TCP 上游与 `underlying-proxy` 链已实现，阶段 2 / M1；Trojan（TCP；WebSocket 传输）已实现，阶段 2 / M2a；VMess（AEAD 握手，TCP；可叠加 TLS / WebSocket）与 AnyTLS（TCP，会话复用）已实现，阶段 2 / M2b；SSH（TCP，会话复用）已实现，阶段 2 / M4a；WireGuard（TCP，用户态隧道）已实现，阶段 2 / M4b；外部程序（TCP，三平台）已实现，阶段 2 / M4c；Shadowsocks（AEAD / 2022、obfs，TCP 与 UDP）已实现，阶段 2 / M6a；Snell v4 / v5（TCP 与 UDP、`reuse`、obfs `http`）已实现，阶段 2 / M6b） | 2     |
```

`README.md`——把

```markdown
3. **阶段 2** 出站协议全集、策略组、策略订阅（进行中：M1 出站地基与 HTTP / SOCKS5 上游已完成；M2a（Trojan，TCP + WebSocket）已完成；M2b（VMess / AnyTLS）已完成；M2c（Shadow TLS）已完成；M3a（成员装配与订阅）已完成；M3b（测速与自动组）已完成；M3c（`smart`）已完成；M4a（SSH）已完成；M4b（WireGuard）已完成）
```

换成

```markdown
3. **阶段 2** 出站协议全集、策略组、策略订阅（进行中：M1 出站地基与 HTTP / SOCKS5 上游已完成；M2a（Trojan，TCP + WebSocket）已完成；M2b（VMess / AnyTLS）已完成；M2c（Shadow TLS）已完成；M3a（成员装配与订阅）已完成；M3b（测速与自动组）已完成；M3c（`smart`）已完成；M4a（SSH）已完成；M4b（WireGuard）已完成；M4c（external）已完成；M5a（UDP 地基）已完成；M5b（TLS 族的 UDP）已完成；M5c（WireGuard 的 UDP 与其余）已完成；M6a（Shadowsocks）已完成；M6b（Snell）已完成）
```

`README_en.md`——把

```markdown
> **Phase 1 feature-complete: M1 through M4b** (profile parsing, rule engine, rule sets, GeoIP, external resource management, DNS client, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`; M3b added the request log and traffic stats, SNI recording, idle timeout, REJECT auto-escalation, a 502 for failed CONNECT dials, graceful shutdown, hot reload (SIGHUP / `--watch`), `--log-file`, and `encrypted-dns-follow-outbound-mode`; M4a added a Surge-compatible HTTP API (phase-1 endpoints, `X-Key` auth with banning), outbound mode / global policy persisted to `state.json`, and `rurge reload` / `stop` / `status`; M4b added the system proxy (Windows registry + a WinINet notification, macOS `networksetup`, Linux GNOME / KDE, `--system-proxy` and `POST /v1/features/system_proxy`, automatic recovery on exit or crash, and following listener changes across a reload) and `rurge service install / uninstall [--dry-run]` (systemd / launchd / a Windows scheduled task)) — the phase-1 acceptance criteria that need a real desktop environment (the three-platform system proxy, service install) have not been through manual acceptance yet; see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md) for the checklist. The dashboard is phase 6. Phase 2 (outbound protocols and policy groups) is in progress: M1 (outbound foundation and HTTP / SOCKS5 upstreams) is done — `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections by rule through DIRECT / REJECT or `http` / `https` / `socks5` / `socks5-tls` upstreams (TCP, including two-or-more-hop `underlying-proxy` chains), and a `select` group's choice can be read and switched over the HTTP API (see [docs/api/phase2.md](docs/api/phase2.md)); M2a (the TLS family, Trojan first) is done — the `trojan` policy (TCP, TLS always on, optionally over WebSocket) is usable; M2b (VMess / AnyTLS) is done — the `vmess` (AEAD handshake, TCP, optionally over TLS / WebSocket) and `anytls` (TCP, session reuse) policies are usable, and a reload reuses outbounds by fingerprint without disturbing connection pools still in use; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); M3a (member assembly and subscriptions) is done — `policy-path` subscriptions (a local file or a URL, in Surge format; an update rebuilds only the policy table, without disturbing unrelated connections), `include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`, the group-level `underlying-proxy` (derived `Name (via Relay)` members), group cycle warnings (`W0030`) and the empty-group fallback (DIRECT by default, REJECT with `--empty-group-reject`) are usable; M3b (connectivity tests and automatic groups) is done — `url-test` / `fallback` / `load-balance` groups choose by connectivity tests (two `HEAD`s over one connection through each member, the second one on the established TLS connection for an HTTPS test URL), `interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` and the policies' `test-url` / `test-timeout` take effect, an automatic group takes a temporary override through `POST /v1/policy_groups/select`, and `POST /v1/policies/test`, `GET /v1/policy_groups/test_results` and `POST /v1/policy_groups/test` are available; M3c (`smart`) is done — a `smart` group scores its members by the first-byte time of real sessions and by the tests (failure penalties, `policy-priority` factors, per-site memory for about an hour) and moves on to the next member when the one it picked does not connect (up to two more), and the request log gains `connectMs` / `firstByteMs`; M4a (SSH) is done — the `ssh` policy (TCP; one SSH session per policy and a channel per connection; login with a password or an OpenSSH private key from `[Keystore]`; host keys checked against `server-fingerprint`; idle sessions closed after `idle-timeout`, a keepalive every 30 seconds) is usable; M4b (WireGuard) is done — the `wireguard` policy (TCP; a user-space tunnel, no virtual interface; several peers chosen by `allowed-ips`; `client-id` (WARP's reserved bytes); destination names looked up through the tunnel when the section has a `dns-server`, on this machine otherwise; tested by a handshake when there is neither a `dns-server` nor a `test-url`) is usable; M4c (external) is done — the `external` policy (TCP; the program is started on first use and reached as a SOCKS5 proxy on its local port; a program that exited is started again on the next use; its output goes to the data directory; when rurge exits, the program is stopped together with whatever it started — a process group on Unix, a Job Object on Windows; all three platforms, where Surge has it on the Mac only) is usable, and subscriptions never bring `external` policies in; M5a (the UDP foundation) is done — the SOCKS5 listener takes UDP ASSOCIATE, and UDP is routed by rule to DIRECT, REJECT or `socks5` / `socks5-tls` / `external` (`udp-relay=true`, `underlying-proxy` chains included), full-cone NAT, one request record per UDP flow, and `block-quic` and `udp-policy-not-supported-behaviour` take effect; M5b (UDP over the TLS family) is done — `trojan` (UDP ASSOCIATE) and `anytls` (UDP over TCP v2) with full-cone NAT, `vmess` (command 2, one connection per target) symmetric; M5c (UDP over WireGuard and the rest) is done — UDP over `wireguard` (full cone) and tunnels over an `underlying-proxy`, `test-udp` / `proxy-test-udp`, UDP in `smart` groups, and `dns-follow-interface`; M6 (Shadowsocks / Snell / the HTTP/2 family) is in progress: M6a (Shadowsocks) is done — `ss` with the AEAD, `none` and SS 2022 (multi-user identity headers included) methods, simple-obfs (`http` / `tls`), TCP and UDP (`udp-relay=true`, `udp-port`, full cone), with the legacy stream ciphers behaving as REJECT until M8; the remaining outbound protocols are later phase-2 milestones, and the `subnet` group comes in phase 3.
```

换成

```markdown
> **Phase 1 feature-complete: M1 through M4b** (profile parsing, rule engine, rule sets, GeoIP, external resource management, DNS client, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`; M3b added the request log and traffic stats, SNI recording, idle timeout, REJECT auto-escalation, a 502 for failed CONNECT dials, graceful shutdown, hot reload (SIGHUP / `--watch`), `--log-file`, and `encrypted-dns-follow-outbound-mode`; M4a added a Surge-compatible HTTP API (phase-1 endpoints, `X-Key` auth with banning), outbound mode / global policy persisted to `state.json`, and `rurge reload` / `stop` / `status`; M4b added the system proxy (Windows registry + a WinINet notification, macOS `networksetup`, Linux GNOME / KDE, `--system-proxy` and `POST /v1/features/system_proxy`, automatic recovery on exit or crash, and following listener changes across a reload) and `rurge service install / uninstall [--dry-run]` (systemd / launchd / a Windows scheduled task)) — the phase-1 acceptance criteria that need a real desktop environment (the three-platform system proxy, service install) have not been through manual acceptance yet; see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md) for the checklist. The dashboard is phase 6. Phase 2 (outbound protocols and policy groups) is in progress: M1 (outbound foundation and HTTP / SOCKS5 upstreams) is done — `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections by rule through DIRECT / REJECT or `http` / `https` / `socks5` / `socks5-tls` upstreams (TCP, including two-or-more-hop `underlying-proxy` chains), and a `select` group's choice can be read and switched over the HTTP API (see [docs/api/phase2.md](docs/api/phase2.md)); M2a (the TLS family, Trojan first) is done — the `trojan` policy (TCP, TLS always on, optionally over WebSocket) is usable; M2b (VMess / AnyTLS) is done — the `vmess` (AEAD handshake, TCP, optionally over TLS / WebSocket) and `anytls` (TCP, session reuse) policies are usable, and a reload reuses outbounds by fingerprint without disturbing connection pools still in use; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); M3a (member assembly and subscriptions) is done — `policy-path` subscriptions (a local file or a URL, in Surge format; an update rebuilds only the policy table, without disturbing unrelated connections), `include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`, the group-level `underlying-proxy` (derived `Name (via Relay)` members), group cycle warnings (`W0030`) and the empty-group fallback (DIRECT by default, REJECT with `--empty-group-reject`) are usable; M3b (connectivity tests and automatic groups) is done — `url-test` / `fallback` / `load-balance` groups choose by connectivity tests (two `HEAD`s over one connection through each member, the second one on the established TLS connection for an HTTPS test URL), `interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` and the policies' `test-url` / `test-timeout` take effect, an automatic group takes a temporary override through `POST /v1/policy_groups/select`, and `POST /v1/policies/test`, `GET /v1/policy_groups/test_results` and `POST /v1/policy_groups/test` are available; M3c (`smart`) is done — a `smart` group scores its members by the first-byte time of real sessions and by the tests (failure penalties, `policy-priority` factors, per-site memory for about an hour) and moves on to the next member when the one it picked does not connect (up to two more), and the request log gains `connectMs` / `firstByteMs`; M4a (SSH) is done — the `ssh` policy (TCP; one SSH session per policy and a channel per connection; login with a password or an OpenSSH private key from `[Keystore]`; host keys checked against `server-fingerprint`; idle sessions closed after `idle-timeout`, a keepalive every 30 seconds) is usable; M4b (WireGuard) is done — the `wireguard` policy (TCP; a user-space tunnel, no virtual interface; several peers chosen by `allowed-ips`; `client-id` (WARP's reserved bytes); destination names looked up through the tunnel when the section has a `dns-server`, on this machine otherwise; tested by a handshake when there is neither a `dns-server` nor a `test-url`) is usable; M4c (external) is done — the `external` policy (TCP; the program is started on first use and reached as a SOCKS5 proxy on its local port; a program that exited is started again on the next use; its output goes to the data directory; when rurge exits, the program is stopped together with whatever it started — a process group on Unix, a Job Object on Windows; all three platforms, where Surge has it on the Mac only) is usable, and subscriptions never bring `external` policies in; M5a (the UDP foundation) is done — the SOCKS5 listener takes UDP ASSOCIATE, and UDP is routed by rule to DIRECT, REJECT or `socks5` / `socks5-tls` / `external` (`udp-relay=true`, `underlying-proxy` chains included), full-cone NAT, one request record per UDP flow, and `block-quic` and `udp-policy-not-supported-behaviour` take effect; M5b (UDP over the TLS family) is done — `trojan` (UDP ASSOCIATE) and `anytls` (UDP over TCP v2) with full-cone NAT, `vmess` (command 2, one connection per target) symmetric; M5c (UDP over WireGuard and the rest) is done — UDP over `wireguard` (full cone) and tunnels over an `underlying-proxy`, `test-udp` / `proxy-test-udp`, UDP in `smart` groups, and `dns-follow-interface`; M6 (Shadowsocks / Snell / the HTTP/2 family) is in progress: M6a (Shadowsocks) is done — `ss` with the AEAD, `none` and SS 2022 (multi-user identity headers included) methods, simple-obfs (`http` / `tls`), TCP and UDP (`udp-relay=true`, `udp-port`, full cone), with the legacy stream ciphers behaving as REJECT until M8; M6b (Snell) is done — `snell` v4 / v5 over TCP and UDP (UDP over TCP, full cone), `reuse` (a connection goes back to a pool for the next request) and simple-obfs `http`, with v1–v3 and v6 behaving as REJECT (a line without `version` is v1); the remaining outbound protocols are later phase-2 milestones, and the `subnet` group comes in phase 3.
```

`README_en.md`——把

```markdown
| Outbound            | DIRECT / REJECT family / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / external program (DIRECT and the four REJECT flavours implemented, M3a; HTTP / SOCKS5 (including `socks5-tls`) TCP upstreams and `underlying-proxy` chains implemented, phase 2 / M1; Trojan (TCP; WebSocket transport) implemented, phase 2 / M2a; VMess (AEAD handshake, TCP; optionally TLS / WebSocket) and AnyTLS (TCP, session reuse) implemented, phase 2 / M2b; SSH (TCP, session reuse) implemented, phase 2 / M4a; WireGuard (TCP, user-space tunnel) implemented, phase 2 / M4b; external program (TCP, all three platforms) implemented, phase 2 / M4c; Shadowsocks (AEAD / 2022, obfs, TCP and UDP) implemented, phase 2 / M6a) | 2     |
```

换成

```markdown
| Outbound            | DIRECT / REJECT family / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / external program (DIRECT and the four REJECT flavours implemented, M3a; HTTP / SOCKS5 (including `socks5-tls`) TCP upstreams and `underlying-proxy` chains implemented, phase 2 / M1; Trojan (TCP; WebSocket transport) implemented, phase 2 / M2a; VMess (AEAD handshake, TCP; optionally TLS / WebSocket) and AnyTLS (TCP, session reuse) implemented, phase 2 / M2b; SSH (TCP, session reuse) implemented, phase 2 / M4a; WireGuard (TCP, user-space tunnel) implemented, phase 2 / M4b; external program (TCP, all three platforms) implemented, phase 2 / M4c; Shadowsocks (AEAD / 2022, obfs, TCP and UDP) implemented, phase 2 / M6a; Snell v4 / v5 (TCP and UDP, `reuse`, obfs `http`) implemented, phase 2 / M6b) | 2     |
```

`README_en.md`——把

```markdown
3. **Phase 2** All outbound protocols, policy groups, subscriptions (in progress: M1, the outbound foundation and HTTP / SOCKS5 upstreams, is done; M2a, Trojan (TCP + WebSocket), is done; M2b, VMess / AnyTLS, is done; M2c, Shadow TLS, is done; M3a, member assembly and subscriptions, is done; M3b, connectivity tests and automatic groups, is done; M3c, `smart` groups, is done; M4a, SSH, is done; M4b, WireGuard, is done)
```

换成

```markdown
3. **Phase 2** All outbound protocols, policy groups, subscriptions (in progress: M1, the outbound foundation and HTTP / SOCKS5 upstreams, is done; M2a, Trojan (TCP + WebSocket), is done; M2b, VMess / AnyTLS, is done; M2c, Shadow TLS, is done; M3a, member assembly and subscriptions, is done; M3b, connectivity tests and automatic groups, is done; M3c, `smart` groups, is done; M4a, SSH, is done; M4b, WireGuard, is done; M4c, external, is done; M5a, the UDP foundation, is done; M5b, UDP over the TLS family, is done; M5c, UDP over WireGuard and the rest, is done; M6a, Shadowsocks, is done; M6b, Snell, is done)
```

`CLAUDE.md`——把

```markdown
阶段 1 功能已齐备（M1 ～ M4b）；阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`。M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。M3b（测速与自动组）已完成——`[General] test-timeout` 区分未设置（`General::test_target`；策略的 `test-url` / `test-timeout` 生效，`W0029` 对它们退役）；`rurge-policy` 的 `probe`（经策略自己的出站在一条连接上两次 HEAD，HTTPS 在已建立的 TLS 连接上测第二次，根证书取 `OutboundFactory::roots`）、`testbook`（`TestBook`：结果按策略与定义保存、同一策略同时只测一次、最多 8 个并发、`TestObserver`、一次性的 `test_once`）与 `auto`（`url-test` / `fallback` / `load-balance` 三种选法、`SelectCtx`、`AutoGroups`：临时覆盖、`url-test` 保持的成员、每组上一轮测试的时间与测试请求）；注册表按测试结果选成员（只有拨号触发测试、`evaluate-before-use` 的 `Resolution.pending`、嵌套组的分数、`round_timeout`）、`resolve_relay`（空组当中继一律拒绝）、订阅行不能动用主配置的 `client-cert` 与策略（`W0023`）；`rurge-engine` 的测试调度任务、测试会话进请求记录（规则 `policy test`）、`evaluate-before-use` 的有界等待（`policy group evaluation failed`）、重载时保留未变组的覆盖、`Engine::snapshot`（运行时与注册表在代际锁下成对读取）与 DNS 会话在链底为 REJECT 时的旁路；`rurge-api` 的 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 与 `POST /v1/policy_groups/select` 对自动组的临时覆盖；能力表翻转 `url-test` / `fallback` / `load-balance`（`W0008` 只剩 `smart` 与 `subnet`）。M3c（`smart`）已完成——`rurge-policy::smart`（`SmartBook`：按策略的时间加权首字节分数与失败罚分、健康 / 未知 / 失败三种状态、站点记忆、使用计数；`rank` 给出选中的成员与重试列表；`sample` 给大组的常规轮抽样）、注册表的 `smart` 成员过滤与 `policy-priority` 因子、`Resolution.smart`（`SmartPick`）、`resolve_member`、`test_round`，以及每代预存的测试查表（`TestBook::outcome`，M3b 延后事项 #14）；`rurge-inbound` 的 `SessionHandle` 两个时刻（出站就绪、首字节）、可挂多个的结束钩子与首字节钩子、`mark_upstream_failed`；`rurge-engine` 的 `smart` 模块（拨号时按重试列表换成员，单次时限"剩余时间 ÷ 剩余尝试次数"；质量回报：首字节、3 秒无响应、上游先断）、请求记录的 `connectMs` / `firstByteMs`、`automatic()` 纳入 `smart`、每次发布注册表时 `SmartBook::retain`；能力表翻转 `smart`（`W0008` 只剩 `subnet`）。M4（WireGuard / SSH / external）按三份计划推进（M4a SSH → M4b WireGuard → M4c external）：M4a（SSH）已完成——`rurge-config::spec::SshSpec`（`server-fingerprint` 解析为 `HostKeyPin`、`idle-timeout`、`private-key` 的 `E0020`）与 `ProtoSpec::keystore_item`（重载的指纹含 SSH 私钥）；新 crate `rurge-proto-ssh`（russh 0.63.3，`ring` 后端）：Keystore 私钥解码（Ed25519 / ECDSA / RSA；带口令与 DSA 为 `E0022`）、主机密钥比对、`SshOutbound`（每个策略一条会话、单飞握手、认证失败等持久失败后按 60 秒起翻倍至 10 分钟退避、钉住的主机密钥算法优先协商、`direct-tcpip` 通道、会话断了重建一次、按通道计的空闲断开、30 秒 × 3 的保活、没配指纹的一次性告警、RSA 只用 SHA-2 签名）与 `testing::FakeSsh`；`rurge-engine` 的工厂分支；订阅行自己的 `private-key=` 进订阅安全门（`W0023`）；`tests/interop` 的临时 `sshd`（`RURGE_TEST_SSHD`，只在 Unix）；能力表翻转 `ssh`。M4b（WireGuard）已完成——`rurge-config` 的 `[WireGuard <name>]` 类型化（`WireGuardSection`，`E0023`，重名的节 `W0020`）与 `spec::WireGuardSpec`（spec 带着节的内容；`underlying-proxy` 为 `W0029` 且策略 REJECT；订阅行自己的 `section-name=` 进订阅安全门）；`rurge-net` 的 `Datagram`、`Connector::connect_udp` 与 `DirectConnector::connect_udp`（按 `ip-version`、`set_tos`、7 MiB 收发缓冲）；新 crate `rurge-proto-wireguard`（boringtun 0.7.1 的 sans-IO `Tunn` 加 smoltcp 0.12：`routes` 最长前缀选 peer、`wire` 的 `client-id`、`stack`（锁内只做内存操作，每次推进都 poll 到发完，Reno 拥塞控制）、`device`（每条隧道一个任务、批量收、握手 TOS 0x88、5 分钟重拨与 endpoint 跟随、发送时本机地址或路由失效或握手无回应时换新载体（网络变化后自愈）、网络变化入口、同一私钥与 peer 只留一条隧道，节与载体设置都相同的策略共用）、`stream`、`dns`（隧道内 DNS 与缓存；每个问题 2 秒内重发、先等隧道第一次握手）、`outbound`（`WireGuardOutbound`：没有 `dns-server` 时在本机解析、含 `[Host]`；原生握手测速）与 `testing::FakeWgPeer`）；`rurge-proto` 的 `Outbound::native_test`；`rurge-policy` 的 `TestMode`（URL 与原生两种测速，`wireguard` 另加 10 秒）；`rurge-engine` 的工厂分支、拨号时 `Unsupported` 带说明、DNS 会话防环扩展到 endpoint 写成域名的 `wireguard`；bin 不输出 boringtun 自己的日志；`tests/interop` 对 sing-box WireGuard 端点（带保留字节）的互操作用例；能力表翻转 `wireguard`。M4c（external）已完成——`rurge-config::spec::ExternalSpec`（`args` 是 `Secret`；两个 `external` 策略写了同一个 `local-port` 是 `E0018`；`args` 进内联参数的脱敏名单）、订阅导入的 `external` 行一律跳过（`W0023`）；`rurge-platform::process`（Unix 进程组与 `killpg`，Windows Job Object）；`rurge_proto::external`（`ExternalOutbound`：第一次用到时拉起、输出写 `<数据目录>/external/<策略名>.log` 且超过 1 MiB 时在拉起前轮转、子进程环境去掉代理变量并设 `NO_PROXY=*`、程序退出后下次用到时再拉起且两次拉起至少隔 2 秒、每 500 ms 连一次本机端口最多 6 次；`ProcessHook` / `ProcessGroup` 与 `NoProcessGroups`；程序自己退出时连同它启动的进程一起结束）；`rurge-engine` 的工厂分支、`EngineShared.processes` / `externals` 与 `Engine::stop_external_programs`；bin 的 `PlatformProcesses` 与退出流程最后停掉全部外部程序；只用于测试的工作区成员 `tests/external`（`rurge-external-tests`：极小的 SOCKS5 辅助程序 `socks-helper` 与真实拉起它的用例）；能力表翻转 `external`（`W0007` 不再因 `external` 出现）。M4 至此完成。M5（UDP 路径）按三份计划推进（M5a 地基 → M5b TLS 族 → M5c WireGuard 与其余）：M5a 已完成——`rurge_net::connector::PacketSocket`（按包收发、带地址的 UDP 载体）与 `Connector::open_udp`（`DirectConnector`：每个地址族一个未连接的 socket，忽略 Windows 的 ICMP 不可达报错）；`Outbound::udp()` / `open_udp()` 与 `UdpSupport`；DIRECT 与 `socks5` / `socks5-tls` / `external` 的 UDP（`udp-relay`，`W0029` 退役）；`ChainConnector::open_udp`（链式 UDP 载体）；`rurge-inbound` 的 SOCKS5 UDP ASSOCIATE（`UdpClient`、`UdpAdmission`、`Dialer::admit_udp` / `associate`）；`rurge-engine` 的 UDP 流水线（`udp` 模块：一条流 = 关联 + 目标、按"关联 × 出站"共用载体的全锥、60 秒 / DNS 10 秒回收、1024 流 / 4096 关联的上限）、请求记录与 API 的 `transport`、`block-quic`（`W0029` 退役）与 `udp-policy-not-supported-behaviour`、QUIC Initial 识别、`PROTOCOL` 规则按传输层匹配 `TCP` / `UDP`；`FakeSocks5` 与 `tests/external` 辅助程序的 UDP ASSOCIATE；对 sing-box `socks` 入站的 UDP 互操作用例。M5b（TLS 族的 UDP）已完成——`rurge-proto` 的 `stream_udp`（一条字节流上按包收发：请求头随第一个包发出、写那个包的目标；`trojan` 的 UDP ASSOCIATE 与 `anytls` 的 UDP over TCP v2 两种封装）、`trojan` / `anytls` 的 `open_udp`（全锥）、`vmess` 的命令 2（`vmess::udp`：每个目标一条 VMess 连接、随它的第一个包建立、每个数据报一个分块，对称型）；`FakeTrojan` / `FakeAnyTls` / `FakeVmess` 的 UDP 与 `rurge_proto::testing::udp_echo_server`；经引擎的端到端用例（`tests/udp_tls_family.rs`）；对 sing-box（三种）与 xray（vmess）的 UDP 互操作用例。M5c（WireGuard 的 UDP 与其余）已完成——`rurge-proto-wireguard` 的 `TunnelUdp`（隧道里每个地址族一个 UDP socket、第一次发往该族时绑定、全锥，目标名经隧道 DNS 或本机解析）与 `Stack::udp_bind` / `check`；`rurge_net::packet_datagram`（把 `PacketSocket` 变成一条到固定目标的 `Datagram`）与 `ChainConnector::connect_udp`，`wireguard` 的载体经 `underlying-proxy`（底层策略不载 UDP 时拨号失败，`W0029` 退役）；启动时 peer 连不上的告警每个策略的每个 peer 5 分钟至多一次（M4b 延后事项 #15）；`rurge_policy::udp_probe`（经策略的 UDP 向 `hostname@ipv4` 问一次 A 记录）、`Engine::test_udp` 与 `POST /v1/policies/test` 结果里的 `udp` 键（`test-udp` / `proxy-test-udp`，不保存、不参与组的选择）；`smart` 计入 UDP（载体打不开算失败、第一个回包算首字节、3 秒无回包只在 53 / 443 端口算失败、UDP 不换成员）；`dns-follow-interface`（`rurge_net::connector::Via` / `ResolveVia`、`Resolver::lookup_via`：策略自己的解析经它的 `interface` 问普通 DNS 服务器、答案另存，配了加密 DNS 时不跟随；没有 `interface` 时 `W0028`）；对 sing-box WireGuard 端点的 UDP 互操作。M5 至此完成。M6（Shadowsocks / Snell / HTTP/2 族）按三份计划推进（M6a Shadowsocks → M6b Snell → M6c HTTP/2 族）：M6a（Shadowsocks）已完成——`rurge-config::spec` 的 `SsSpec`（`SsMethod`：AEAD 五种、`none`、SS 2022 两种；2022 的 `password` 按冒号拆成逐层的 Base64 密钥、最后一段是用户密钥，不合法是 `E0018` 且不引用取值；`udp-relay`、`udp-port`）与 `ObfsOpts`（与 Snell 共用；`obfs-host` 缺省服务器主机名、`obfs-uri` 缺省 `/`）、`NotImplemented`（取代 `legacy_vmess` 标记：vmess 旧握手与 `ss` 流式旧方法都是 `W0007` + REJECT，流式方法每种每次加载一条，会话日志 `policy protocol not implemented: ss (<method>)`）、`obfs-host` 进内联参数的脱敏名单；`rurge_proto::transport::obfs`（simple-obfs 的 `http` / `tls`，自写模板，`Stack` 的一层：connect → shadow-tls → obfs → tls → ws）；`rurge_proto::shadowsocks`（`ShadowsocksOutbound`：AEAD 分块流与 `none`、SS 2022（BLAKE3 子密钥、请求头块与填充、应答的类型 / 时间戳 / 回显 salt 校验、SIP023 多用户身份头）、UDP（`udp-relay=true` 发往 `udp-port`，全锥；2022 的分离头、按服务端 session 的防重放窗口））；新依赖只有 `blake3`；`rurge_proto::testing` 的 `FakeShadowsocks`（AEAD、2022 含身份头、UDP、两种 obfs）与 `accept_obfs`；`rurge-engine` 的工厂分支与经引擎的端到端用例（`tests/outbounds_shadowsocks.rs`）；能力表翻转 `ss`（流式旧方法除外）；测试里"未实现的协议"的例子从 `ss` 换成 `hysteria2`；`tests/interop` 对 shadowsocks-rust `ssserver`（固定版本 v1.25.0，`RURGE_TEST_SSSERVER`）与 sing-box `shadowsocks` 入站的互操作用例。
```

换成

```markdown
阶段 1 功能已齐备（M1 ～ M4b）；阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`。M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。M3b（测速与自动组）已完成——`[General] test-timeout` 区分未设置（`General::test_target`；策略的 `test-url` / `test-timeout` 生效，`W0029` 对它们退役）；`rurge-policy` 的 `probe`（经策略自己的出站在一条连接上两次 HEAD，HTTPS 在已建立的 TLS 连接上测第二次，根证书取 `OutboundFactory::roots`）、`testbook`（`TestBook`：结果按策略与定义保存、同一策略同时只测一次、最多 8 个并发、`TestObserver`、一次性的 `test_once`）与 `auto`（`url-test` / `fallback` / `load-balance` 三种选法、`SelectCtx`、`AutoGroups`：临时覆盖、`url-test` 保持的成员、每组上一轮测试的时间与测试请求）；注册表按测试结果选成员（只有拨号触发测试、`evaluate-before-use` 的 `Resolution.pending`、嵌套组的分数、`round_timeout`）、`resolve_relay`（空组当中继一律拒绝）、订阅行不能动用主配置的 `client-cert` 与策略（`W0023`）；`rurge-engine` 的测试调度任务、测试会话进请求记录（规则 `policy test`）、`evaluate-before-use` 的有界等待（`policy group evaluation failed`）、重载时保留未变组的覆盖、`Engine::snapshot`（运行时与注册表在代际锁下成对读取）与 DNS 会话在链底为 REJECT 时的旁路；`rurge-api` 的 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 与 `POST /v1/policy_groups/select` 对自动组的临时覆盖；能力表翻转 `url-test` / `fallback` / `load-balance`（`W0008` 只剩 `smart` 与 `subnet`）。M3c（`smart`）已完成——`rurge-policy::smart`（`SmartBook`：按策略的时间加权首字节分数与失败罚分、健康 / 未知 / 失败三种状态、站点记忆、使用计数；`rank` 给出选中的成员与重试列表；`sample` 给大组的常规轮抽样）、注册表的 `smart` 成员过滤与 `policy-priority` 因子、`Resolution.smart`（`SmartPick`）、`resolve_member`、`test_round`，以及每代预存的测试查表（`TestBook::outcome`，M3b 延后事项 #14）；`rurge-inbound` 的 `SessionHandle` 两个时刻（出站就绪、首字节）、可挂多个的结束钩子与首字节钩子、`mark_upstream_failed`；`rurge-engine` 的 `smart` 模块（拨号时按重试列表换成员，单次时限"剩余时间 ÷ 剩余尝试次数"；质量回报：首字节、3 秒无响应、上游先断）、请求记录的 `connectMs` / `firstByteMs`、`automatic()` 纳入 `smart`、每次发布注册表时 `SmartBook::retain`；能力表翻转 `smart`（`W0008` 只剩 `subnet`）。M4（WireGuard / SSH / external）按三份计划推进（M4a SSH → M4b WireGuard → M4c external）：M4a（SSH）已完成——`rurge-config::spec::SshSpec`（`server-fingerprint` 解析为 `HostKeyPin`、`idle-timeout`、`private-key` 的 `E0020`）与 `ProtoSpec::keystore_item`（重载的指纹含 SSH 私钥）；新 crate `rurge-proto-ssh`（russh 0.63.3，`ring` 后端）：Keystore 私钥解码（Ed25519 / ECDSA / RSA；带口令与 DSA 为 `E0022`）、主机密钥比对、`SshOutbound`（每个策略一条会话、单飞握手、认证失败等持久失败后按 60 秒起翻倍至 10 分钟退避、钉住的主机密钥算法优先协商、`direct-tcpip` 通道、会话断了重建一次、按通道计的空闲断开、30 秒 × 3 的保活、没配指纹的一次性告警、RSA 只用 SHA-2 签名）与 `testing::FakeSsh`；`rurge-engine` 的工厂分支；订阅行自己的 `private-key=` 进订阅安全门（`W0023`）；`tests/interop` 的临时 `sshd`（`RURGE_TEST_SSHD`，只在 Unix）；能力表翻转 `ssh`。M4b（WireGuard）已完成——`rurge-config` 的 `[WireGuard <name>]` 类型化（`WireGuardSection`，`E0023`，重名的节 `W0020`）与 `spec::WireGuardSpec`（spec 带着节的内容；`underlying-proxy` 为 `W0029` 且策略 REJECT；订阅行自己的 `section-name=` 进订阅安全门）；`rurge-net` 的 `Datagram`、`Connector::connect_udp` 与 `DirectConnector::connect_udp`（按 `ip-version`、`set_tos`、7 MiB 收发缓冲）；新 crate `rurge-proto-wireguard`（boringtun 0.7.1 的 sans-IO `Tunn` 加 smoltcp 0.12：`routes` 最长前缀选 peer、`wire` 的 `client-id`、`stack`（锁内只做内存操作，每次推进都 poll 到发完，Reno 拥塞控制）、`device`（每条隧道一个任务、批量收、握手 TOS 0x88、5 分钟重拨与 endpoint 跟随、发送时本机地址或路由失效或握手无回应时换新载体（网络变化后自愈）、网络变化入口、同一私钥与 peer 只留一条隧道，节与载体设置都相同的策略共用）、`stream`、`dns`（隧道内 DNS 与缓存；每个问题 2 秒内重发、先等隧道第一次握手）、`outbound`（`WireGuardOutbound`：没有 `dns-server` 时在本机解析、含 `[Host]`；原生握手测速）与 `testing::FakeWgPeer`）；`rurge-proto` 的 `Outbound::native_test`；`rurge-policy` 的 `TestMode`（URL 与原生两种测速，`wireguard` 另加 10 秒）；`rurge-engine` 的工厂分支、拨号时 `Unsupported` 带说明、DNS 会话防环扩展到 endpoint 写成域名的 `wireguard`；bin 不输出 boringtun 自己的日志；`tests/interop` 对 sing-box WireGuard 端点（带保留字节）的互操作用例；能力表翻转 `wireguard`。M4c（external）已完成——`rurge-config::spec::ExternalSpec`（`args` 是 `Secret`；两个 `external` 策略写了同一个 `local-port` 是 `E0018`；`args` 进内联参数的脱敏名单）、订阅导入的 `external` 行一律跳过（`W0023`）；`rurge-platform::process`（Unix 进程组与 `killpg`，Windows Job Object）；`rurge_proto::external`（`ExternalOutbound`：第一次用到时拉起、输出写 `<数据目录>/external/<策略名>.log` 且超过 1 MiB 时在拉起前轮转、子进程环境去掉代理变量并设 `NO_PROXY=*`、程序退出后下次用到时再拉起且两次拉起至少隔 2 秒、每 500 ms 连一次本机端口最多 6 次；`ProcessHook` / `ProcessGroup` 与 `NoProcessGroups`；程序自己退出时连同它启动的进程一起结束）；`rurge-engine` 的工厂分支、`EngineShared.processes` / `externals` 与 `Engine::stop_external_programs`；bin 的 `PlatformProcesses` 与退出流程最后停掉全部外部程序；只用于测试的工作区成员 `tests/external`（`rurge-external-tests`：极小的 SOCKS5 辅助程序 `socks-helper` 与真实拉起它的用例）；能力表翻转 `external`（`W0007` 不再因 `external` 出现）。M4 至此完成。M5（UDP 路径）按三份计划推进（M5a 地基 → M5b TLS 族 → M5c WireGuard 与其余）：M5a 已完成——`rurge_net::connector::PacketSocket`（按包收发、带地址的 UDP 载体）与 `Connector::open_udp`（`DirectConnector`：每个地址族一个未连接的 socket，忽略 Windows 的 ICMP 不可达报错）；`Outbound::udp()` / `open_udp()` 与 `UdpSupport`；DIRECT 与 `socks5` / `socks5-tls` / `external` 的 UDP（`udp-relay`，`W0029` 退役）；`ChainConnector::open_udp`（链式 UDP 载体）；`rurge-inbound` 的 SOCKS5 UDP ASSOCIATE（`UdpClient`、`UdpAdmission`、`Dialer::admit_udp` / `associate`）；`rurge-engine` 的 UDP 流水线（`udp` 模块：一条流 = 关联 + 目标、按"关联 × 出站"共用载体的全锥、60 秒 / DNS 10 秒回收、1024 流 / 4096 关联的上限）、请求记录与 API 的 `transport`、`block-quic`（`W0029` 退役）与 `udp-policy-not-supported-behaviour`、QUIC Initial 识别、`PROTOCOL` 规则按传输层匹配 `TCP` / `UDP`；`FakeSocks5` 与 `tests/external` 辅助程序的 UDP ASSOCIATE；对 sing-box `socks` 入站的 UDP 互操作用例。M5b（TLS 族的 UDP）已完成——`rurge-proto` 的 `stream_udp`（一条字节流上按包收发：请求头随第一个包发出、写那个包的目标；`trojan` 的 UDP ASSOCIATE 与 `anytls` 的 UDP over TCP v2 两种封装）、`trojan` / `anytls` 的 `open_udp`（全锥）、`vmess` 的命令 2（`vmess::udp`：每个目标一条 VMess 连接、随它的第一个包建立、每个数据报一个分块，对称型）；`FakeTrojan` / `FakeAnyTls` / `FakeVmess` 的 UDP 与 `rurge_proto::testing::udp_echo_server`；经引擎的端到端用例（`tests/udp_tls_family.rs`）；对 sing-box（三种）与 xray（vmess）的 UDP 互操作用例。M5c（WireGuard 的 UDP 与其余）已完成——`rurge-proto-wireguard` 的 `TunnelUdp`（隧道里每个地址族一个 UDP socket、第一次发往该族时绑定、全锥，目标名经隧道 DNS 或本机解析）与 `Stack::udp_bind` / `check`；`rurge_net::packet_datagram`（把 `PacketSocket` 变成一条到固定目标的 `Datagram`）与 `ChainConnector::connect_udp`，`wireguard` 的载体经 `underlying-proxy`（底层策略不载 UDP 时拨号失败，`W0029` 退役）；启动时 peer 连不上的告警每个策略的每个 peer 5 分钟至多一次（M4b 延后事项 #15）；`rurge_policy::udp_probe`（经策略的 UDP 向 `hostname@ipv4` 问一次 A 记录）、`Engine::test_udp` 与 `POST /v1/policies/test` 结果里的 `udp` 键（`test-udp` / `proxy-test-udp`，不保存、不参与组的选择）；`smart` 计入 UDP（载体打不开算失败、第一个回包算首字节、3 秒无回包只在 53 / 443 端口算失败、UDP 不换成员）；`dns-follow-interface`（`rurge_net::connector::Via` / `ResolveVia`、`Resolver::lookup_via`：策略自己的解析经它的 `interface` 问普通 DNS 服务器、答案另存，配了加密 DNS 时不跟随；没有 `interface` 时 `W0028`）；对 sing-box WireGuard 端点的 UDP 互操作。M5 至此完成。M6（Shadowsocks / Snell / HTTP/2 族）按三份计划推进（M6a Shadowsocks → M6b Snell → M6c HTTP/2 族）：M6a（Shadowsocks）已完成——`rurge-config::spec` 的 `SsSpec`（`SsMethod`：AEAD 五种、`none`、SS 2022 两种；2022 的 `password` 按冒号拆成逐层的 Base64 密钥、最后一段是用户密钥，不合法是 `E0018` 且不引用取值；`udp-relay`、`udp-port`）与 `ObfsOpts`（与 Snell 共用；`obfs-host` 缺省服务器主机名、`obfs-uri` 缺省 `/`）、`NotImplemented`（取代 `legacy_vmess` 标记：vmess 旧握手与 `ss` 流式旧方法都是 `W0007` + REJECT，流式方法每种每次加载一条，会话日志 `policy protocol not implemented: ss (<method>)`）、`obfs-host` 进内联参数的脱敏名单；`rurge_proto::transport::obfs`（simple-obfs 的 `http` / `tls`，自写模板，`Stack` 的一层：connect → shadow-tls → obfs → tls → ws）；`rurge_proto::shadowsocks`（`ShadowsocksOutbound`：AEAD 分块流与 `none`、SS 2022（BLAKE3 子密钥、请求头块与填充、应答的类型 / 时间戳 / 回显 salt 校验、SIP023 多用户身份头）、UDP（`udp-relay=true` 发往 `udp-port`，全锥；2022 的分离头、按服务端 session 的防重放窗口））；新依赖只有 `blake3`；`rurge_proto::testing` 的 `FakeShadowsocks`（AEAD、2022 含身份头、UDP、两种 obfs）与 `accept_obfs`；`rurge-engine` 的工厂分支与经引擎的端到端用例（`tests/outbounds_shadowsocks.rs`）；能力表翻转 `ss`（流式旧方法除外）；测试里"未实现的协议"的例子从 `ss` 换成 `hysteria2`；`tests/interop` 对 shadowsocks-rust `ssserver`（固定版本 v1.25.0，`RURGE_TEST_SSSERVER`）与 sing-box `shadowsocks` 入站的互操作用例。M6b（Snell）已完成——`rurge-config::spec` 的 `SnellSpec`（`SnellVersion` 只有 V4 / V5；`version` 1–6、缺省 1，1–3 与 6 是 `NotImplemented::SnellVersion`：`W0007` + REJECT，每个版本每次加载一条，会话日志 `policy protocol not implemented: snell v<n>`；`psk` 是 `Secret`；v4 / v5 的 `obfs` 只有 `http`；`mode` 只对 v6 校验）；`rurge_proto::snell`（`SnellOutbound`：`kdf`（Argon2id 取前 16 字节，在阻塞线程上算；工作区依赖 `argon2` 0.6，已在依赖树里）、`record`（`SnellStream`：每方向一个 salt、7 字节加密头、首帧 256–511 字节填充与负载密文交错、每帧至多 0x3FFF、空帧结束一个方向、复用时计数器延续）、`tunnel`（请求头随首段负载写出，`reuse=false` 发 Connect `0x01`、`reuse=true` 发 ConnectV2 `0x05`；应答 `00` / `02`；干净结束的连接在 `Drop` 时回池，没结束的在后台补完再回池；池里取出的失效连接换新连接重发一次）、`pool`（每个出站至多 8 条空闲连接、空闲 60 秒回收）、`udp`（命令 `0x06`：每个载体一条自己的连接、从不进池，每个数据报一帧，全锥；`udp-port` 是这条连接所连的端口））；`LazyHead` 改为泛型（`LazyHead<S = BoxedStream>`，加 `get_mut` / `into_inner`）；`rurge_proto::testing` 的 `FakeSnell`（`SnellScript`：TCP、ConnectV2 复用、UDP、obfs http、拒绝、每条连接的请求数上限）；`rurge-engine` 的工厂分支与经引擎的端到端用例（`tests/outbounds_snell.rs`）；能力表翻转 `snell`（v4 / v5）；`tests/interop` 对 sing-box `snell` 入站（`version: 5`，同时接受 v4 客户端；sing-box 固定版本从 1.14.1 升到 1.14.2）与 Surge 官方 snell-server v5.0.1（只有 Linux 版，`RURGE_TEST_SNELL_SERVER`，只在 Linux CI 上装）的互操作用例。
```

`CLAUDE.md`——把

```markdown
- `docs/superpowers/specs/2026-09-30-phase2-m6-ss-snell-h2-design.md`：阶段 2 / M6 细化设计（Shadowsocks / Snell / HTTP/2 族），细化总设计的 M6 里程碑、不一致处以它为准，并关闭总设计的开放问题 Q7。三份计划的拆分（M6a Shadowsocks → M6b Snell → M6c `h2-connect` / `trust-tunnel`）；已决事项 M6-D1 ～ D8（四种协议都做、Snell 只实现 v4 / v5 而 `version` 缺省为 1 的行按 v1 拒绝、GPL 参考实现只取协议事实、新依赖只有 `blake3`、各协议的互操作参考、sing-box 升到有 `snell` 入站的 1.14.x、v5 的动态帧大小只影响发送方、`h2-connect` 的 UDP 每个目标一条流）；各协议的配置、线上格式、错误与三层测试；第 7 节是需登记的差异，第 9 节 V1 ～ V10 是写各份计划时必须核对的事项，第 10 节是三份计划的任务草图，第 12 节是 M6a 计划期的订正，第 13 节是 M6a 实施期的订正。
- `docs/superpowers/plans/2026-09-30-phase2-m6a-shadowsocks-plan.md`：阶段 2 / M6a（Shadowsocks）实施计划（7 个任务）。开头「计划期决定」表记录核对参考实现、SIP022 / SIP023 原文与本仓库得出的结论和与设计文字不同的决定（加密只用 RustCrypto 且 `chacha20poly1305` 沿用依赖树里的 0.10、`LazyHead` 在加密流之上、SS 2022 与 AEAD 共用一种流且在应答头块校验、填充内容为零只有长度随机、UDP 包号从 0 开始并校验回显的客户端 session id、每个载体至多记 8 个服务端 session、obfs 的 `Host` 总带非 80 的端口与首包 16 KiB 上限、不接受 `2022-blake3-chacha20-poly1305`、测试里"未实现的协议"的例子换成 `hysteria2`、sing-box 覆盖 ssserver 发布包没有的 `aes-192-gcm` / `xchacha20-ietf-poly1305` 等）与「承接事项」；末尾「执行期修正记录」与「延后事项」两张表。
```

换成

```markdown
- `docs/superpowers/specs/2026-09-30-phase2-m6-ss-snell-h2-design.md`：阶段 2 / M6 细化设计（Shadowsocks / Snell / HTTP/2 族），细化总设计的 M6 里程碑、不一致处以它为准，并关闭总设计的开放问题 Q7。三份计划的拆分（M6a Shadowsocks → M6b Snell → M6c `h2-connect` / `trust-tunnel`）；已决事项 M6-D1 ～ D8（四种协议都做、Snell 只实现 v4 / v5 而 `version` 缺省为 1 的行按 v1 拒绝、GPL 参考实现只取协议事实、新依赖只有 `blake3`、各协议的互操作参考、sing-box 升到有 `snell` 入站的 1.14.x、v5 的动态帧大小只影响发送方、`h2-connect` 的 UDP 每个目标一条流）；各协议的配置、线上格式、错误与三层测试；第 7 节是需登记的差异，第 9 节 V1 ～ V10 是写各份计划时必须核对的事项，第 10 节是三份计划的任务草图，第 12 节是 M6a 计划期的订正，第 13 节是 M6a 实施期的订正，第 14 节是 M6b 计划期的订正。
- `docs/superpowers/plans/2026-09-30-phase2-m6a-shadowsocks-plan.md`：阶段 2 / M6a（Shadowsocks）实施计划（7 个任务）。开头「计划期决定」表记录核对参考实现、SIP022 / SIP023 原文与本仓库得出的结论和与设计文字不同的决定（加密只用 RustCrypto 且 `chacha20poly1305` 沿用依赖树里的 0.10、`LazyHead` 在加密流之上、SS 2022 与 AEAD 共用一种流且在应答头块校验、填充内容为零只有长度随机、UDP 包号从 0 开始并校验回显的客户端 session id、每个载体至多记 8 个服务端 session、obfs 的 `Host` 总带非 80 的端口与首包 16 KiB 上限、不接受 `2022-blake3-chacha20-poly1305`、测试里"未实现的协议"的例子换成 `hysteria2`、sing-box 覆盖 ssserver 发布包没有的 `aes-192-gcm` / `xchacha20-ietf-poly1305` 等）与「承接事项」；末尾「执行期修正记录」与「延后事项」两张表。
- `docs/superpowers/plans/2026-09-30-phase2-m6b-snell-plan.md`：阶段 2 / M6b（Snell v4 / v5）实施计划（6 个任务）。开头「计划期决定」表记录核对公开协议描述、参考实现与本仓库得出的结论和与设计文字不同的决定（`reuse=false` 发 Connect、`reuse=true` 发 ConnectV2、主机名一律按文本写、发送固定以 0x3FFF 为帧上限、复用池至多 8 条并对失效的池连接重试一次、UDP 不走 `stream_udp` 而按帧收发、`udp-port` 是 UDP 会话那条 TCP 连接的端口、`obfs-host` 缺省仍是服务器主机名、sing-box 升到 1.14.2 与官方 snell-server 只在 Linux CI 上跑等）；末尾「执行期修正记录」与「延后事项」两张表。
```

`CLAUDE.md`——把

```markdown
cargo test -p rurge-external-tests              # external：真实拉起测试辅助程序（socks-helper）——按需拉起、参数顺序与代理变量、再拉起与 2 秒间隔、连接被拒的重试、日志、整棵进程树的停止、经引擎的端到端、检查不拉起、重载沿用与替换
cargo test -p rurge-interop                     # 对 sing-box（全部协议）、xray（只测 vmess）、shadowsocks-rust ssserver（只测 ss）与 OpenSSH sshd（只测 ssh，只在 Unix）的互操作测试；没装就跳过（RURGE_TEST_SING_BOX / RURGE_TEST_XRAY / RURGE_TEST_SSSERVER / RURGE_TEST_SSHD / RURGE_INTEROP_REQUIRED=1）
```

换成

```markdown
cargo test -p rurge-proto snell                 # snell：Argon2id 与帧的已知答案、请求头、复用池与失效重试、UDP 数据报，出站对回环假服务端（FakeSnell，含 obfs http）
cargo test -p rurge-engine --test outbounds_snell   # 经 snell 出站的端到端用例：v4 / v5、reuse、obfs http、拒绝与 psk 错、v1 的 REJECT、经 underlying-proxy、UDP、全锥、重载沿用与替换
cargo test -p rurge-external-tests              # external：真实拉起测试辅助程序（socks-helper）——按需拉起、参数顺序与代理变量、再拉起与 2 秒间隔、连接被拒的重试、日志、整棵进程树的停止、经引擎的端到端、检查不拉起、重载沿用与替换
cargo test -p rurge-interop                     # 对 sing-box 1.14.2（全部协议）、xray（只测 vmess）、shadowsocks-rust ssserver（只测 ss）、官方 snell-server v5.0.1（只测 snell，只在 Linux）与 OpenSSH sshd（只测 ssh，只在 Unix）的互操作测试；没装就跳过（RURGE_TEST_SING_BOX / RURGE_TEST_XRAY / RURGE_TEST_SSSERVER / RURGE_TEST_SNELL_SERVER / RURGE_TEST_SSHD / RURGE_INTEROP_REQUIRED=1）
```

总设计（第 1.4 节 M6 与 M8 行、第 2 节 Snell 行、第 13 节 M6 行、风险表 D 行：v5 已实现，"只出可行性报告"改为 v6 与 v5 的 QUIC Proxy Mode）：

`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`——把

```markdown
| **M6 Shadowsocks / Snell / HTTP/2 族** | `ss`（AEAD、2022、obfs，含 UDP）、`snell` v1 ～ v4（obfs、reuse，v3+ UDP）、`h2-connect`（`max-streams`、CONNECT-UDP）、`trust-tunnel`（h2） | FR-OUT-05 / 07 | 四种协议对参考实现转发通过 |
| **M7 QUIC 族** | `rurge-net::quic` 公共件、`rurge-proto-quic`（`tuic` `tuic-v5` `hysteria2` `masque`、`trust-tunnel` 的 h3 模式）、`port-hopping`、`ecn`、DoH3 / DoQ 上游 | FR-OUT-03（`ecn`）/ 05、FR-DNS-04 | QUIC 族 TCP 与 UDP 转发通过；`h3://` `quic://` 上游解析正常 |
| **M8 收尾与验收** | P2 项（Shadowsocks 流式旧方法、VMess 旧握手）、Snell v5 / v6 · Gecko · Tailscale 可行性报告、阶段验收清单、文档同步 | FR-OUT-06 / 15 | 第 14 节验收标准全部通过 |
```

换成

```markdown
| **M6 Shadowsocks / Snell / HTTP/2 族** | `ss`（AEAD、2022、obfs，含 UDP）、`snell` v4 / v5（obfs `http`、reuse，UDP over TCP；v1 ～ v3 与 v6 只解析、按 REJECT 处理，见 M6 细化设计 M6-D2）、`h2-connect`（`max-streams`、CONNECT-UDP）、`trust-tunnel`（h2） | FR-OUT-05 / 07 | 四种协议对参考实现转发通过 |
| **M7 QUIC 族** | `rurge-net::quic` 公共件、`rurge-proto-quic`（`tuic` `tuic-v5` `hysteria2` `masque`、`trust-tunnel` 的 h3 模式）、`port-hopping`、`ecn`、DoH3 / DoQ 上游 | FR-OUT-03（`ecn`）/ 05、FR-DNS-04 | QUIC 族 TCP 与 UDP 转发通过；`h3://` `quic://` 上游解析正常 |
| **M8 收尾与验收** | P2 项（Shadowsocks 流式旧方法、VMess 旧握手）、Snell v1 ～ v3（需要时）、Snell v6 与 v5 QUIC Proxy Mode · Gecko · Tailscale 可行性报告、阶段验收清单、文档同步 | FR-OUT-06 / 15 | 第 14 节验收标准全部通过 |
```

`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`——把

```markdown
| Snell | 协议非公开（PRD R1 / R7）：只依据公开的第三方资料实现 v1 ～ v4 | M6 | 各版本可得的公开资料范围 |
```

换成

```markdown
| Snell | 协议非公开（PRD R1 / R7）：只依据公开的第三方资料实现 v4 / v5 的 TCP 线上格式（v5 在 TCP 上与 v4 相同；M6 细化设计第 4.1 节、M6-D2） | M6 | 各版本可得的公开资料范围 |
```

`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`——把

```markdown
| M6 | 按 M6 细化设计 M6-D5：shadowsocks-rust v1.25.0（含 2022 与多用户，三平台）与 sing-box 的 `shadowsocks` 入站：`ss`；sing-box 的 `snell` 入站（1.14.0 起，只支持 v5 / v6）：`snell`，三平台；官方 snell-server v5.0.1 只有 Linux 版，只在 Linux CI 跑；sing-box 的 `http` 入站（HTTP/2 over TLS）：`h2-connect` 的普通 CONNECT；TrustTunnel endpoint v1.1.0 只有 Linux / macOS 版，只在这两个平台的 CI 跑；CONNECT-UDP over HTTP/2 与 obfs 没有可用的预编译参考服务端，只有回环假服务端与手工验收 |
```

换成

```markdown
| M6 | 按 M6 细化设计 M6-D5：shadowsocks-rust v1.25.0（含 2022 与多用户，三平台）与 sing-box 的 `shadowsocks` 入站：`ss`；sing-box 的 `snell` 入站（1.14.0 起，只支持 v5 / v6；`version: 5` 同时接受 v4 客户端）：`snell`，三平台，CI 固定的 sing-box 为此从 1.14.1 升到 1.14.2（M6-D6）；官方 snell-server v5.0.1 只有 Linux 版，只在 Linux CI 跑；sing-box 的 `http` 入站（HTTP/2 over TLS）：`h2-connect` 的普通 CONNECT；TrustTunnel endpoint v1.1.0 只有 Linux / macOS 版，只在这两个平台的 CI 跑；CONNECT-UDP over HTTP/2 与 obfs 没有可用的预编译参考服务端，只有回环假服务端与手工验收 |
```

`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`——把

```markdown
| D | Snell 与 `smart` 的细节非公开（PRD R1） | 行为无法完全一致 | 近似实现，差异登记在清单；Snell v5 / v6 只出可行性报告 |
```

换成

```markdown
| D | Snell 与 `smart` 的细节非公开（PRD R1） | 行为无法完全一致 | 近似实现，差异登记在清单；Snell v6 只出可行性报告 |
```

- [ ] **Step 4: 核对**

- `README.md` 与 `README_en.md` 的状态一段、特性表与路线图内容一致。
- `grep -n "v1 ～ v4\|v1 - v4\|v1–v4" docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md` 没有输出。

- [ ] **Step 5: 门禁与提交**

跑门禁。副本上最后一次全工作区门禁：fmt、clippy 通过，`cargo test --workspace --no-fail-fast` 1452 通过、0 失败、2 忽略。

```bash
git add tests/interop .github docs README.md README_en.md CLAUDE.md
git commit -m "docs: M6b Snell——兼容性清单、手工验收、README 与 CLAUDE.md；sing-box 1.14.2 与 snell-server 的互操作"
```

## 验收对照（设计第 1 节与第 4 节，M6b 部分）

| # | 验收项 | 由谁保证 |
| - | ------ | -------- |
| 1 | `snell` 对参考实现的 TCP 转发通过 | Task 6 的 sing-box 与 snell-server 用例（CI）；Task 3 的回环往返 |
| 2 | UDP 转发通过 | Task 6 同上（CI）；Task 4 的往返；Task 5 `udp_goes_through_snell` |
| 3 | `reuse` | Task 3 的复用与陈旧连接重试；Task 5 `reuse_carries_sessions_one_after_another_on_one_connection`；Task 6 的互操作 |
| 4 | `obfs=http` | Task 3 的回环用例；Task 5 `obfs_http_carries_the_session`；Task 6 的 sing-box `obfs_mode: http` |
| 5 | 未实现的版本 `W0007` + REJECT | Task 1 的加载告警；Task 5 `check_knows_snell`、`snell_v1_rejects_and_says_which` |
| 6 | 能力表不再为 `snell` 出 `W0007` | Task 5 `check_knows_snell` |
| 7 | 门禁全绿 | 各任务的门禁 |
| 8 | 需要真实节点的项目进手工验收清单 | Task 6：`docs/acceptance/phase2-manual.md` 的 M6b 一节 |

## 执行期修正记录

| # | 任务 | 与计划的出入 | 原因 |
| - | ---- | ------------ | ---- |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | `version` 1–3 与 6、v5 的 QUIC Proxy Mode | M8（需要时） |
| 2 | v5 的动态记录大小（P4） | 接受（只影响发送方） |
| 3 | `obfs-host` 缺省、命令字、`udp-port` 的含义与 Surge 可能不同（P6、P9、P11） | 有真实 Surge 抓包或用户报告时对齐 |
| 4 | 互操作数不了复用的连接数；snell-server 是否默认开 UDP 未确认（P12） | CI 首跑；手工验收 |
| 5 | 陈旧连接的重试最多重发 64 KiB，超出就不重试（P8） | 接受 |
