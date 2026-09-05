# 阶段 1 / M3a「连接流水线：跑起来」实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 `rurge run -c <conf>` 作为前台守护进程按 Surge 配置提供 HTTP / SOCKS5 代理，并把每个连接经 M2 的规则引擎与 DNS 解析器分流到 `DIRECT` 或 `REJECT` 系策略；新增 `rurge-proto`、`rurge-policy`、`rurge-inbound`、`rurge-engine` 四个 crate。

**Architecture:** 入站（hyper 1 的 HTTP/1.1 代理、手写 SOCKS5）只负责协议语义，通过 `Dialer` trait 调用引擎：`dial(SessionInfo)` 走「出站模式 → 规则评估 → 策略注册表解析 → `Outbound::connect_tcp`」得到出站流与会话句柄，`relay` 做带计数的双向转发；`rurge-proto` 定义 `Outbound` 并实现 `Direct`（包 `rurge-net` 的 happy-eyeballs 连接器 + `Resolver`）与 `Reject`；`rurge-policy` 把 `PolicyRef` 解析为 `Outbound`（select 组读 `state.json`，其他组取首成员并告警，未实现协议按 REJECT）；`rurge-engine` 持有一代 `Runtime`（配置 + 规则引擎 + 解析器 + 注册表 + M2 的资源栈）并实现 `Dialer`；bin 的 `run` 子命令初始化日志、构建 `Runtime`、绑定监听、等 Ctrl-C。

**Tech Stack:** Rust stable / edition 2024（MSRV 1.88）、tokio（net / io-util / signal / JoinSet）、hyper 1（`server::conn::http1` + upgrade、`client::conn::http1`）、hyper-util `TokioIo`、http-body-util、base64 0.22、arc-swap、serde_json（`state.json`）、tracing + tracing-subscriber；测试用 `rurge_net::testing::TestServer` 与 `rurge_dns::testing::MockDns`。

**Spec:** `docs/superpowers/specs/2026-09-05-phase1-m3-pipeline-design.md`（M3a 部分：§1 ～ §6、§7.1 ～ 7.3、§8、§9、§10、§11、§13）；上位文档 `docs/superpowers/specs/2026-09-03-phase1-core-skeleton-design.md` §8、§11 ～ 13；需求 FR-IN-01 ～ 03、05；FR-OUT-01、02；FR-HTTP-12；FR-OBS-01、04（`run`）、10。

## Global Constraints

- 工具链：`rust-toolchain.toml` 固定 `channel = "stable"`；workspace `edition = "2024"`，`rust-version = "1.88"`；lints `unsafe_code = "forbid"`，clippy `all = warn`。嵌套 `if let` 写成 let-chains（clippy 会要求）。
- 质量门：每个任务结束时 `cargo fmt --all --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --workspace` 三者必须通过。本机 msvc 工具链缺 rustfmt 时用 `RUSTFMT="C:\Users\SZV01065\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe" cargo fmt --all --check`。
- 依赖方向（设计 §3）：`rurge (bin) → rurge-engine → { rurge-inbound → rurge-proto, rurge-policy → rurge-proto, rurge-dns } → rurge-rules → rurge-net → rurge-config`；`rurge-inbound` 不依赖 `rurge-policy` / `rurge-dns`；`rurge-engine` 不依赖 `rurge-platform`（平台对象由 bin 注入）；平台特定代码只在 `rurge-platform`（AR-02）。
- 设计的 binding 语义（§4 ～ §9）：每连接一个 tokio 任务；配置不可变，会话取 `Runtime` 快照用到结束；`Dialer::dial` 的顺序为出站模式 → 规则评估 → 策略解析 → `connect_tcp`；REJECT 四种行为与错误码见设计 §8 表；`proxy-restricted-to-lan`（默认 true）只对非回环监听生效，来源须为回环 / 私有 / 链路本地 / ULA；Basic 认证只比较密码；SOCKS5 只支持无认证 + CONNECT；明文 HTTP 每请求一次 dial、一条出站连接；连接超时 10 s，REJECT-DROP 保持 30 s；日志级别映射 verbose→TRACE、info→DEBUG、notify→INFO、warning→WARN；凭据不进日志。
- 未实现的策略协议按 REJECT 处理，iOS 专属内置策略按 DIRECT，`DEVICE:` 按 REJECT，非 select 组取首成员：对应的加载告警 `W0007` / `W0009` / `W0010` / `W0008` 已由 M1 的加载器发出（`crates/rurge-config/src/config.rs`），`PolicyRegistry` 不再重复告警，因此 `PolicyRegistry::build` 不返回诊断（与设计 §5 的签名差异，Task 10 登记）。诊断码一经定义不得改号。
- rurge 专有运行时选项只走命令行参数与环境变量（FR-CFG-17）：本计划新增 `--outbound-mode` / `RURGE_OUTBOUND_MODE`、`--log-level` / `RURGE_LOG_LEVEL`。
- 测试不访问公网：目标站是 `rurge_net::testing::TestServer`，DNS 是 `rurge_dns::testing::MockDns`，全部 127.0.0.1；CLI 测试用 `http-listen = 127.0.0.1:0` 并从子进程 stdout 的 `listening on` 行取端口。
- 公共接口签名以各任务的 **Interfaces** 块为准；设计 §11 列出的差异在 Task 10 登记到 `docs/surge-compatibility-matrix.md`。
- 语言：文档与提交信息中文；代码标识符、注释、日志、CLI 输出英文。
- 提交：每个任务一次提交，在分支 `m3a-pipeline` 上进行，不推送、不合并；提交信息末尾带两行尾注 `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>` 与 `Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW`。
- 手册基线：Surge 官方手册 2026-09 版（`profile/general.html`、`policies/reject.html` 已于 2026-09-05 核对）。

## 文件结构

```
Cargo.toml                                    workspace：四个新 crate 路径；base64；tokio 增加 signal feature
crates/rurge-config/src/general.rs            Listener 的 Debug 脱敏密码
crates/rurge-proto/
  Cargo.toml
  src/lib.rs                                  模块声明与再导出
  src/outbound.rs                             Outbound trait、OutboundError、RejectKind
  src/direct.rs                               Direct（经 Connector）
  src/reject.rs                               Reject
crates/rurge-policy/
  Cargo.toml
  src/lib.rs
  src/selections.rs                           GroupSelections
  src/registry.rs                             PolicyRegistry、Resolution
crates/rurge-inbound/
  Cargo.toml
  src/lib.rs
  src/session.rs                              Dialer、Dialed、DialError、SessionHandle、SessionOutcome
  src/restrict.rs                             来源限制谓词
  src/listener.rs                             accept 循环、Running、ListenerOpts
  src/responses.rs                            错误页、1px GIF、407 / 400 / 403 / 502 响应
  src/socks5.rs                               Socks5Listener 与编解码
  src/http.rs                                 HttpListener（CONNECT、明文转发、认证）
crates/rurge-engine/
  Cargo.toml
  src/lib.rs
  src/stack.rs                                Stack / StackOptions / build_stack（自 bin 迁入）
  src/state.rs                                state.json 只读解析
  src/runtime.rs                              Runtime::build
  src/engine.rs                               Engine：Dialer 实现、relay、bind_listeners、会话日志
  tests/pipeline.rs                           端到端集成测试
crates/rurge/
  Cargo.toml                                  依赖 rurge-engine、tracing、tracing-subscriber
  src/main.rs                                 Run 子命令
  src/cli/run.rs                              rurge run
  src/cli/runtime.rs                          RuntimeArgs / PlatformSystemDns 保留，Stack 构建委托 rurge-engine
  src/cli/rule.rs                             parse_mode 改为 pub(crate)
  tests/cli.rs                                run 的子进程测试
README.md / CLAUDE.md / docs/surge-compatibility-matrix.md / 本计划末尾两节
```

模块内依赖：`engine` → `runtime` → `stack` + `state`；`http` / `socks5` → `listener` + `session` + `responses` + `restrict`。

---

### Task 1: 分支、依赖、四个 crate 骨架、`Listener` 脱敏

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/rurge-config/src/general.rs`（`Listener` 的 `Debug`）
- Create: `crates/rurge-proto/{Cargo.toml,src/lib.rs}`、`crates/rurge-policy/{Cargo.toml,src/lib.rs}`、`crates/rurge-inbound/{Cargo.toml,src/lib.rs}`、`crates/rurge-engine/{Cargo.toml,src/lib.rs}`

**Interfaces:**
- Consumes: 无。
- Produces: 四个空 crate；workspace 依赖 `base64 = "0.22"`、tokio 的 `signal` feature；`Listener` 的 `Debug` 输出不含密码。

- [ ] **Step 1: 建分支并提交计划**

```bash
git checkout -b m3a-pipeline
git add docs/superpowers/plans/2026-09-05-phase1-m3a-pipeline-plan.md
git commit -F - <<'EOF'
docs: M3a 连接流水线实施计划

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

- [ ] **Step 2: workspace 依赖**

`Cargo.toml` 的 `[workspace.dependencies]` 追加：

```toml
rurge-proto = { path = "crates/rurge-proto" }
rurge-policy = { path = "crates/rurge-policy" }
rurge-inbound = { path = "crates/rurge-inbound" }
rurge-engine = { path = "crates/rurge-engine" }
base64 = "0.22"
```

并把 tokio 一行改为：

```toml
tokio = { version = "1", features = ["rt-multi-thread", "net", "time", "fs", "sync", "macros", "io-util", "signal"] }
```

- [ ] **Step 3: `Listener` 的 `Debug` 脱敏**

`crates/rurge-config/src/general.rs`：把 `Listener` 的派生从 `#[derive(Clone, Debug, PartialEq, Eq)]` 改为 `#[derive(Clone, PartialEq, Eq)]`，并在其后追加（文件顶部若无 `use std::fmt;` 则添加）：

```rust
impl fmt::Debug for Listener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Listener")
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("addr", &self.addr)
            .finish()
    }
}
```

在 `general.rs` 的测试模块追加：

```rust
    #[test]
    fn listener_debug_redacts_the_password() {
        let l = Listener {
            password: Some("s3cret".to_string()),
            addr: "127.0.0.1:6152".parse().unwrap(),
        };
        let shown = format!("{l:?}");
        assert!(!shown.contains("s3cret"), "{shown}");
        assert!(shown.contains("<redacted>") && shown.contains("6152"), "{shown}");
    }
```

若 `cargo test -p rurge-config` 报告 insta 快照变化，用 `cargo insta test -p rurge-config --review` 审阅：只允许密码字段变为 `<redacted>` 的差异，其余差异视为回归。

- [ ] **Step 4: 四个 crate 骨架**

`crates/rurge-proto/Cargo.toml`：

```toml
[package]
name = "rurge-proto"
description = "Outbound abstraction and the built-in DIRECT / REJECT outbounds for rurge"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
rurge-config.workspace = true
rurge-net.workspace = true
tokio.workspace = true
tracing.workspace = true

[lints]
workspace = true
```

`crates/rurge-proto/src/lib.rs`：

```rust
//! Outbound abstraction (M3 design §4): the `Outbound` trait every policy
//! implements, plus the phase 1 built-ins `Direct` and `Reject`.
```

`crates/rurge-policy/Cargo.toml`：

```toml
[package]
name = "rurge-policy"
description = "Policy registry: names, aliases and groups resolved to outbounds"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
rurge-config.workspace = true
rurge-proto.workspace = true
tracing.workspace = true

[lints]
workspace = true
```

`crates/rurge-policy/src/lib.rs`：

```rust
//! Policy registry (M3 design §5): resolves a `PolicyRef` through aliases and
//! groups to a concrete `Outbound`, recording the chain it took.
```

`crates/rurge-inbound/Cargo.toml`：

```toml
[package]
name = "rurge-inbound"
description = "HTTP and SOCKS5 proxy listeners for rurge"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
rurge-config.workspace = true
rurge-net.workspace = true
rurge-proto.workspace = true
tokio.workspace = true
hyper.workspace = true
hyper-util.workspace = true
http.workspace = true
http-body-util.workspace = true
bytes.workspace = true
base64.workspace = true
tracing.workspace = true

[dev-dependencies]
rurge-net = { workspace = true, features = ["testing"] }

[lints]
workspace = true
```

`crates/rurge-inbound/src/lib.rs`：

```rust
//! Inbound listeners (M3 design §6): HTTP/1.1 proxy (CONNECT and plain
//! forwarding) on hyper, SOCKS5 by hand. Listeners only speak the protocol;
//! rules, policies, outbound connections and relaying belong to the engine,
//! reached through the `Dialer` trait.
```

`crates/rurge-engine/Cargo.toml`：

```toml
[package]
name = "rurge-engine"
description = "Session pipeline: runtime, dialer, relay and session log for rurge"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
rurge-config.workspace = true
rurge-net.workspace = true
rurge-rules.workspace = true
rurge-dns.workspace = true
rurge-proto.workspace = true
rurge-policy.workspace = true
rurge-inbound.workspace = true
tokio.workspace = true
arc-swap.workspace = true
serde.workspace = true
serde_json.workspace = true
url.workspace = true
tracing.workspace = true

[dev-dependencies]
rurge-net = { workspace = true, features = ["testing"] }
rurge-dns = { workspace = true, features = ["testing"] }
rustls.workspace = true
tokio-rustls.workspace = true
tempfile.workspace = true

[lints]
workspace = true
```

`crates/rurge-engine/src/lib.rs`：

```rust
//! The session pipeline (M3 design §7): one immutable `Runtime` per config
//! generation, the `Engine` that dials and relays sessions for the inbound
//! listeners, and the session log.
```

- [ ] **Step 5: 质量门**

```bash
cargo build --workspace
cargo test -p rurge-config listener_debug
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
```

预期：四个空 crate 编译；`Cargo.lock` 更新；全部测试通过（280 + 1）。

- [ ] **Step 6: 提交**

```bash
git add Cargo.toml Cargo.lock crates/rurge-config crates/rurge-proto crates/rurge-policy crates/rurge-inbound crates/rurge-engine
git commit -F - <<'EOF'
chore: M3a 骨架：rurge-proto / rurge-policy / rurge-inbound / rurge-engine、依赖、Listener 脱敏

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 2: `rurge-proto`：`Outbound`、`Direct`、`Reject`

**Files:**
- Modify: `crates/rurge-proto/src/lib.rs`
- Create: `crates/rurge-proto/src/outbound.rs`、`crates/rurge-proto/src/direct.rs`、`crates/rurge-proto/src/reject.rs`

**Interfaces:**
- Consumes: `rurge_net::connector::{BoxedStream, ConnectOpts, Connector, DirectConnector, Resolve, Target}`、`rurge_net::BoxFuture`、`rurge_config::policy::Builtin`。
- Produces（crate 根再导出 `Direct, Outbound, OutboundError, OutboundRef, Reject, RejectKind`）：
  - `RejectKind::{Reject, Drop, NoDrop, TinyGif}`（`Copy, Hash`）；`RejectKind::from_builtin(Builtin) -> Option<RejectKind>`；`name(self) -> &'static str`（`REJECT` / `REJECT-DROP` / `REJECT-NO-DROP` / `REJECT-TINYGIF`）；`escalates(self) -> bool`（Reject 与 TinyGif 为 true）。
  - `OutboundError::{Reject(RejectKind), Unsupported(String), Dns(String), Io(io::Error), Timeout}`（`Debug, Display, std::error::Error, From<io::Error>`）。
  - `trait Outbound: Send + Sync { fn name(&self) -> &str; fn connect_tcp<'a>(&'a self, target: &'a Target, opts: &'a ConnectOpts) -> BoxFuture<'a, Result<BoxedStream, OutboundError>>; }`；`OutboundRef = Arc<dyn Outbound>`。`ConnectOpts` 直接复用 `rurge_net::connector::ConnectOpts { timeout, prefer_v6 }`（设计 §4 单独定义了同形结构，实施时复用，Task 10 登记）。
  - `Direct::new(connector: Arc<dyn Connector>) -> Direct`；`Direct::with_resolver(resolver: Arc<dyn Resolve>) -> Direct`（包 `DirectConnector`）；`name() == "DIRECT"`；`connect_tcp` 在 `opts.timeout` 内完成，否则 `Timeout`；`io::ErrorKind::TimedOut` → `Timeout`，其余 → `Io`。
  - `Reject::new(kind) -> Reject`；`Reject::kind(&self)`；`name()` = `kind.name()`；`connect_tcp` 立即返回 `Err(OutboundError::Reject(kind))`。

- [ ] **Step 1: 写 `outbound.rs`**

```rust
//! The `Outbound` trait and its error type.

use rurge_config::policy::Builtin;
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Target};
use std::fmt;
use std::io;
use std::sync::Arc;

/// The four REJECT flavours (M3 design §8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RejectKind {
    Reject,
    Drop,
    NoDrop,
    TinyGif,
}

impl RejectKind {
    pub fn from_builtin(b: Builtin) -> Option<RejectKind> {
        match b {
            Builtin::Reject => Some(RejectKind::Reject),
            Builtin::RejectDrop => Some(RejectKind::Drop),
            Builtin::RejectNoDrop => Some(RejectKind::NoDrop),
            Builtin::RejectTinyGif => Some(RejectKind::TinyGif),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            RejectKind::Reject => "REJECT",
            RejectKind::Drop => "REJECT-DROP",
            RejectKind::NoDrop => "REJECT-NO-DROP",
            RejectKind::TinyGif => "REJECT-TINYGIF",
        }
    }

    /// Kinds that count towards the automatic escalation to DROP (M3b).
    pub fn escalates(self) -> bool {
        matches!(self, RejectKind::Reject | RejectKind::TinyGif)
    }
}

#[derive(Debug)]
pub enum OutboundError {
    Reject(RejectKind),
    /// The policy's protocol is not implemented in this version.
    Unsupported(String),
    Dns(String),
    Io(io::Error),
    Timeout,
}

impl fmt::Display for OutboundError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OutboundError::Reject(k) => write!(f, "rejected by {}", k.name()),
            OutboundError::Unsupported(t) => write!(f, "policy protocol not implemented: {t}"),
            OutboundError::Dns(m) => write!(f, "dns: {m}"),
            OutboundError::Io(e) => write!(f, "{e}"),
            OutboundError::Timeout => f.write_str("connect timed out"),
        }
    }
}

impl std::error::Error for OutboundError {}

impl From<io::Error> for OutboundError {
    fn from(e: io::Error) -> OutboundError {
        if e.kind() == io::ErrorKind::TimedOut {
            OutboundError::Timeout
        } else {
            OutboundError::Io(e)
        }
    }
}

/// A way to reach a destination. Phase 1 ships `Direct` and `Reject`; every
/// proxy protocol of phase 2 implements this trait too.
pub trait Outbound: Send + Sync {
    /// Display name: `DIRECT`, `REJECT-TINYGIF`, or the policy name.
    fn name(&self) -> &str;
    fn connect_tcp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>>;
}

pub type OutboundRef = Arc<dyn Outbound>;
```

- [ ] **Step 2: 写 `direct.rs`**

```rust
//! `DIRECT`: connect straight to the destination through a `Connector`.

use crate::outbound::{Outbound, OutboundError};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, DirectConnector, Resolve, Target};
use std::sync::Arc;

pub struct Direct {
    connector: Arc<dyn Connector>,
}

impl Direct {
    pub fn new(connector: Arc<dyn Connector>) -> Direct {
        Direct { connector }
    }

    /// Plain TCP with happy-eyeballs address ordering, resolving through `resolver`.
    pub fn with_resolver(resolver: Arc<dyn Resolve>) -> Direct {
        Direct::new(Arc::new(DirectConnector::new(resolver)))
    }
}

impl Outbound for Direct {
    fn name(&self) -> &str {
        "DIRECT"
    }

    fn connect_tcp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            match tokio::time::timeout(opts.timeout, self.connector.connect(target, opts)).await {
                Ok(Ok(stream)) => Ok(stream),
                Ok(Err(e)) => Err(OutboundError::from(e)),
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }
}
```

- [ ] **Step 3: 写 `reject.rs`**

```rust
//! The REJECT family: never connects; the listener turns the error into the
//! protocol-appropriate response (M3 design §8).

use crate::outbound::{Outbound, OutboundError, RejectKind};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Target};

pub struct Reject {
    kind: RejectKind,
}

impl Reject {
    pub fn new(kind: RejectKind) -> Reject {
        Reject { kind }
    }

    pub fn kind(&self) -> RejectKind {
        self.kind
    }
}

impl Outbound for Reject {
    fn name(&self) -> &str {
        self.kind.name()
    }

    fn connect_tcp<'a>(
        &'a self,
        _target: &'a Target,
        _opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(std::future::ready(Err(OutboundError::Reject(self.kind))))
    }
}
```

`lib.rs`：

```rust
pub mod direct;
pub mod outbound;
pub mod reject;

pub use direct::Direct;
pub use outbound::{Outbound, OutboundError, OutboundRef, RejectKind};
pub use reject::Reject;
```

- [ ] **Step 4: 测试**

在 `direct.rs` 末尾追加：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::HostName;
    use std::io;
    use std::net::IpAddr;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    struct Loopback;

    impl Resolve for Loopback {
        fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
            Box::pin(async move {
                if host == "echo.test" {
                    Ok(vec!["127.0.0.1".parse().unwrap()])
                } else {
                    Err(io::Error::new(io::ErrorKind::NotFound, format!("no addresses for {host}")))
                }
            })
        }
    }

    async fn echo_server() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 64];
                    if let Ok(n) = s.read(&mut buf).await {
                        let _ = s.write_all(&buf[..n]).await;
                    }
                });
            }
        });
        port
    }

    #[tokio::test]
    async fn connects_by_ip_literal_and_by_resolved_domain() {
        let port = echo_server().await;
        let direct = Direct::with_resolver(Arc::new(Loopback));
        assert_eq!(direct.name(), "DIRECT");
        for host in ["127.0.0.1", "echo.test"] {
            let mut stream = direct
                .connect_tcp(&Target::new(HostName::parse(host), port), &ConnectOpts::default())
                .await
                .unwrap();
            stream.write_all(b"ping").await.unwrap();
            let mut buf = [0u8; 4];
            stream.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"ping");
        }
    }

    #[tokio::test]
    async fn resolution_and_connect_failures_are_io_errors() {
        let direct = Direct::with_resolver(Arc::new(Loopback));
        let err = direct
            .connect_tcp(&Target::new(HostName::parse("nx.test"), 80), &ConnectOpts::default())
            .await
            .unwrap_err();
        assert!(matches!(err, OutboundError::Io(_)), "{err}");
        // a port nobody listens on
        let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = closed.local_addr().unwrap().port();
        drop(closed);
        let err = direct
            .connect_tcp(
                &Target::new(HostName::parse("127.0.0.1"), port),
                &ConnectOpts {
                    timeout: Duration::from_secs(3),
                    prefer_v6: false,
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, OutboundError::Io(_) | OutboundError::Timeout), "{err}");
    }
}
```

在 `reject.rs` 末尾追加：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::HostName;
    use rurge_config::policy::Builtin;

    #[tokio::test]
    async fn every_kind_rejects_immediately_with_its_name() {
        for (builtin, kind, name) in [
            (Builtin::Reject, RejectKind::Reject, "REJECT"),
            (Builtin::RejectDrop, RejectKind::Drop, "REJECT-DROP"),
            (Builtin::RejectNoDrop, RejectKind::NoDrop, "REJECT-NO-DROP"),
            (Builtin::RejectTinyGif, RejectKind::TinyGif, "REJECT-TINYGIF"),
        ] {
            assert_eq!(RejectKind::from_builtin(builtin), Some(kind));
            let r = Reject::new(kind);
            assert_eq!(r.name(), name);
            assert_eq!(r.kind(), kind);
            let err = r
                .connect_tcp(&Target::new(HostName::parse("a.test"), 443), &ConnectOpts::default())
                .await
                .unwrap_err();
            assert!(matches!(err, OutboundError::Reject(k) if k == kind));
        }
        assert_eq!(RejectKind::from_builtin(Builtin::Direct), None);
        assert!(RejectKind::Reject.escalates() && RejectKind::TinyGif.escalates());
        assert!(!RejectKind::Drop.escalates() && !RejectKind::NoDrop.escalates());
        assert_eq!(OutboundError::Unsupported("ss".into()).to_string(), "policy protocol not implemented: ss");
    }
}
```

> 本机上连接已关闭的回环端口约需 2 s 才返回拒绝（M2b 执行期观察），所以第二个用例的超时给了 3 s，并同时接受 `Io` 与 `Timeout`。

- [ ] **Step 5: 运行与提交**

```bash
cargo test -p rurge-proto
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-proto
git commit -F - <<'EOF'
feat(proto): Outbound 抽象与内置 DIRECT / REJECT 出站

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

预期：3 个测试通过。

---

### Task 3: `rurge-policy`：`GroupSelections` 与 `PolicyRegistry`

**Files:**
- Modify: `crates/rurge-policy/src/lib.rs`
- Create: `crates/rurge-policy/src/selections.rs`、`crates/rurge-policy/src/registry.rs`

**Interfaces:**
- Consumes: `rurge_config::{Config, PolicyKind, GroupKind, Builtin}`、`rurge_config::config::PolicyTarget`（`Config::resolve_policy(name) -> Option<PolicyTarget<'_>>`，变体 `Builtin(Builtin)` / `Proxy(&ProxyPolicy)` / `Group(&PolicyGroup)`）、`rurge_config::rule::PolicyRef::{Builtin, Named, Device}` 与 `PolicyRef::parse`、`rurge_proto::{Outbound, OutboundRef, Reject, RejectKind}`。
- Produces（crate 根再导出 `GroupSelections, PolicyRegistry, Resolution`）：
  - `GroupSelections`（`Clone, Debug, Default, PartialEq, Eq`）：`new()`、`from_map(HashMap<String, String>)`、`get(&self, group: &str) -> Option<&str>`、`set(&mut self, group: &str, member: &str)`、`is_empty()`。
  - `Resolution { chain: Vec<String>, outbound: OutboundRef, unsupported: Option<String> }`：`chain` 是解析路径（首项为请求的策略名，末项为实际出站名；未实现协议时末项前一项为 `!unsupported:<type>`），`unsupported` 为未实现协议的类型关键字（`DEVICE:` 策略为 `"DEVICE"`）。
  - `PolicyRegistry::build(cfg: &Config, selections: &GroupSelections, direct: OutboundRef) -> PolicyRegistry`；`resolve(&self, policy: &PolicyRef) -> Resolution`；`names(&self) -> Vec<String>`（`[Proxy]` 与 `[Proxy Group]` 的名字，按配置顺序）；`direct(&self) -> OutboundRef`；`reject(&self, kind: RejectKind) -> OutboundRef`（四个共享实例）。
  - 解析规则见设计 §5 表：内置 → 对应出站（iOS 专属 → DIRECT）；别名类型 `direct` / `reject*` → 对应出站；未实现协议的代理策略 → `Reject(Reject)` + `unsupported`；`select` 组 → 持久选择（须仍是成员）否则首成员；其他组 → 首成员；`DEVICE:` → `Reject(Reject)` + `unsupported = "DEVICE"`；递归深度上限 16，超限或名字未定义 → `Reject(Reject)` 并 `tracing::error!`（M1 校验后不可达）。

- [ ] **Step 1: 写 `selections.rs`**

```rust
//! Persisted `select` group choices for the current profile (read from
//! `state.json` by the engine, written by the M4 API).

use std::collections::HashMap;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GroupSelections {
    map: HashMap<String, String>,
}

impl GroupSelections {
    pub fn new() -> GroupSelections {
        GroupSelections::default()
    }

    pub fn from_map(map: HashMap<String, String>) -> GroupSelections {
        GroupSelections { map }
    }

    pub fn get(&self, group: &str) -> Option<&str> {
        self.map.get(group).map(String::as_str)
    }

    pub fn set(&mut self, group: &str, member: &str) {
        self.map.insert(group.to_string(), member.to_string());
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}
```

- [ ] **Step 2: 写 `registry.rs`**

```rust
//! Name → outbound resolution (M3 design §5). Built once per config
//! generation; `resolve` is a table walk with no allocation beyond the chain.

use crate::selections::GroupSelections;
use rurge_config::config::PolicyTarget;
use rurge_config::rule::PolicyRef;
use rurge_config::{Builtin, Config, GroupKind, PolicyKind};
use rurge_proto::{OutboundRef, Reject, RejectKind};
use std::collections::HashMap;
use std::sync::Arc;

/// Deeper chains than this are treated as a defect (M1 rejects group cycles).
pub const MAX_DEPTH: usize = 16;

#[derive(Clone)]
pub struct Resolution {
    pub chain: Vec<String>,
    pub outbound: OutboundRef,
    /// Protocol keyword (or `DEVICE`) when the terminal policy is not implemented.
    pub unsupported: Option<String>,
}

enum Terminal {
    Direct,
    Reject(RejectKind),
}

enum Entry {
    Alias(Terminal),
    Proxy { kind: PolicyKind },
    Group { kind: GroupKind, members: Vec<String>, selected: Option<String> },
}

pub struct PolicyRegistry {
    entries: HashMap<String, Entry>,
    order: Vec<String>,
    direct: OutboundRef,
    rejects: [OutboundRef; 4],
}

fn reject_slot(kind: RejectKind) -> usize {
    match kind {
        RejectKind::Reject => 0,
        RejectKind::Drop => 1,
        RejectKind::NoDrop => 2,
        RejectKind::TinyGif => 3,
    }
}

fn alias_terminal(kind: PolicyKind) -> Option<Terminal> {
    match kind {
        PolicyKind::Direct => Some(Terminal::Direct),
        PolicyKind::Reject => Some(Terminal::Reject(RejectKind::Reject)),
        PolicyKind::RejectDrop => Some(Terminal::Reject(RejectKind::Drop)),
        PolicyKind::RejectNoDrop => Some(Terminal::Reject(RejectKind::NoDrop)),
        PolicyKind::RejectTinyGif => Some(Terminal::Reject(RejectKind::TinyGif)),
        _ => None,
    }
}

impl PolicyRegistry {
    pub fn build(cfg: &Config, selections: &GroupSelections, direct: OutboundRef) -> PolicyRegistry {
        let mut entries = HashMap::new();
        let mut order = Vec::new();
        for p in &cfg.policies {
            let entry = match alias_terminal(p.kind) {
                Some(t) => Entry::Alias(t),
                None => Entry::Proxy { kind: p.kind },
            };
            entries.insert(p.name.clone(), entry);
            order.push(p.name.clone());
        }
        for g in &cfg.groups {
            let selected = match g.kind {
                GroupKind::Select => selections.get(&g.name).map(str::to_string),
                _ => None,
            };
            entries.insert(
                g.name.clone(),
                Entry::Group {
                    kind: g.kind,
                    members: g.members.clone(),
                    selected,
                },
            );
            order.push(g.name.clone());
        }
        let rejects = [
            Arc::new(Reject::new(RejectKind::Reject)) as OutboundRef,
            Arc::new(Reject::new(RejectKind::Drop)) as OutboundRef,
            Arc::new(Reject::new(RejectKind::NoDrop)) as OutboundRef,
            Arc::new(Reject::new(RejectKind::TinyGif)) as OutboundRef,
        ];
        PolicyRegistry {
            entries,
            order,
            direct,
            rejects,
        }
    }

    pub fn direct(&self) -> OutboundRef {
        self.direct.clone()
    }

    pub fn reject(&self, kind: RejectKind) -> OutboundRef {
        self.rejects[reject_slot(kind)].clone()
    }

    pub fn names(&self) -> Vec<String> {
        self.order.clone()
    }

    pub fn resolve(&self, policy: &PolicyRef) -> Resolution {
        let mut chain = Vec::new();
        match policy {
            PolicyRef::Builtin(b) => self.builtin(*b, &mut chain),
            PolicyRef::Device(name) => {
                chain.push(format!("DEVICE:{name}"));
                chain.push(RejectKind::Reject.name().to_string());
                Resolution {
                    chain,
                    outbound: self.reject(RejectKind::Reject),
                    unsupported: Some("DEVICE".to_string()),
                }
            }
            PolicyRef::Named(name) => self.named(name, &mut chain, 0),
        }
    }

    fn builtin(&self, b: Builtin, chain: &mut Vec<String>) -> Resolution {
        chain.push(b.name().to_string());
        if b == Builtin::Direct {
            return self.done(chain, self.direct(), None);
        }
        if let Some(kind) = RejectKind::from_builtin(b) {
            return self.done(chain, self.reject(kind), None);
        }
        // CELLULAR / CELLULAR-ONLY / HYBRID / NO-HYBRID: iOS-only, DIRECT on desktop (W0009 at load).
        chain.push("DIRECT".to_string());
        self.done(chain, self.direct(), None)
    }

    fn named(&self, name: &str, chain: &mut Vec<String>, depth: usize) -> Resolution {
        chain.push(name.to_string());
        if depth > MAX_DEPTH {
            tracing::error!(policy = name, "policy chain deeper than {MAX_DEPTH}; treating as REJECT");
            chain.push(RejectKind::Reject.name().to_string());
            return self.done(chain, self.reject(RejectKind::Reject), None);
        }
        match self.entries.get(name) {
            None => {
                tracing::error!(policy = name, "policy not found in registry; treating as REJECT");
                chain.push(RejectKind::Reject.name().to_string());
                self.done(chain, self.reject(RejectKind::Reject), None)
            }
            Some(Entry::Alias(Terminal::Direct)) => {
                chain.push("DIRECT".to_string());
                self.done(chain, self.direct(), None)
            }
            Some(Entry::Alias(Terminal::Reject(kind))) => {
                chain.push(kind.name().to_string());
                self.done(chain, self.reject(*kind), None)
            }
            Some(Entry::Proxy { kind }) => {
                chain.push(format!("!unsupported:{}", kind.keyword()));
                chain.push(RejectKind::Reject.name().to_string());
                self.done(chain, self.reject(RejectKind::Reject), Some(kind.keyword().to_string()))
            }
            Some(Entry::Group {
                members, selected, ..
            }) => {
                let next = selected
                    .as_deref()
                    .filter(|m| members.iter().any(|x| x == m))
                    .or_else(|| members.first().map(String::as_str));
                match next {
                    Some(member) => match PolicyRef::parse(member) {
                        PolicyRef::Builtin(b) => self.builtin(b, chain),
                        PolicyRef::Device(d) => {
                            chain.push(format!("DEVICE:{d}"));
                            chain.push(RejectKind::Reject.name().to_string());
                            self.done(chain, self.reject(RejectKind::Reject), Some("DEVICE".to_string()))
                        }
                        PolicyRef::Named(n) => self.named(&n, chain, depth + 1),
                    },
                    None => {
                        tracing::error!(group = name, "policy group has no members; treating as REJECT");
                        chain.push(RejectKind::Reject.name().to_string());
                        self.done(chain, self.reject(RejectKind::Reject), None)
                    }
                }
            }
        }
    }

    fn done(&self, chain: &mut Vec<String>, outbound: OutboundRef, unsupported: Option<String>) -> Resolution {
        Resolution {
            chain: std::mem::take(chain),
            outbound,
            unsupported,
        }
    }
}
```

`lib.rs`：

```rust
pub mod registry;
pub mod selections;

pub use registry::{PolicyRegistry, Resolution};
pub use selections::GroupSelections;
```

> `Config::resolve_policy` 在本任务不需要：注册表用 `cfg.policies` / `cfg.groups` 自建索引。若 `PolicyGroup.members` 的成员字符串带空格，以 M1 解析结果为准（M1 已 trim）。

- [ ] **Step 3: 测试**

在 `registry.rs` 末尾追加：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::config::{LoadOptions, from_text};
    use rurge_net::BoxFuture;
    use rurge_net::connector::{BoxedStream, ConnectOpts, Target};
    use rurge_proto::{Outbound, OutboundError};
    use std::path::Path;

    struct FakeDirect;

    impl Outbound for FakeDirect {
        fn name(&self) -> &str {
            "DIRECT"
        }
        fn connect_tcp<'a>(&'a self, _t: &'a Target, _o: &'a ConnectOpts) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
            Box::pin(std::future::ready(Err(OutboundError::Timeout)))
        }
    }

    const PROFILE: &str = "[General]\nloglevel = notify\n[Proxy]\nHK = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\nD = direct\nBlock = reject-tinygif\n[Proxy Group]\nAuto = url-test, HK, D\nPick = select, HK, D, DIRECT\nOuter = select, Pick, Auto\nEmptyish = select, Block\n[Rule]\nFINAL,Pick\n";

    fn registry(selections: GroupSelections) -> PolicyRegistry {
        let loaded = from_text(PROFILE, Path::new("t.conf"), &LoadOptions::for_tests());
        assert!(!loaded.diagnostics.has_errors(), "{:?}", loaded.diagnostics.iter().map(|d| d.code).collect::<Vec<_>>());
        PolicyRegistry::build(&loaded.config, &selections, Arc::new(FakeDirect))
    }

    fn chain(r: &Resolution) -> Vec<&str> {
        r.chain.iter().map(String::as_str).collect()
    }

    #[test]
    fn builtins_and_aliases() {
        let reg = registry(GroupSelections::new());
        let d = reg.resolve(&PolicyRef::Builtin(Builtin::Direct));
        assert_eq!((chain(&d), d.outbound.name(), d.unsupported.as_deref()), (vec!["DIRECT"], "DIRECT", None));
        let r = reg.resolve(&PolicyRef::Builtin(Builtin::RejectTinyGif));
        assert_eq!((chain(&r), r.outbound.name()), (vec!["REJECT-TINYGIF"], "REJECT-TINYGIF"));
        let cell = reg.resolve(&PolicyRef::Builtin(Builtin::Cellular));
        assert_eq!((chain(&cell), cell.outbound.name()), (vec!["CELLULAR", "DIRECT"], "DIRECT"));
        let alias = reg.resolve(&PolicyRef::parse("D"));
        assert_eq!((chain(&alias), alias.outbound.name()), (vec!["D", "DIRECT"], "DIRECT"));
        let block = reg.resolve(&PolicyRef::parse("Block"));
        assert_eq!((chain(&block), block.outbound.name()), (vec!["Block", "REJECT-TINYGIF"], "REJECT-TINYGIF"));
        assert_eq!(reg.names(), vec!["HK", "D", "Block", "Auto", "Pick", "Outer", "Emptyish"]);
    }

    #[test]
    fn unsupported_protocols_and_devices_reject() {
        let reg = registry(GroupSelections::new());
        let hk = reg.resolve(&PolicyRef::parse("HK"));
        assert_eq!(chain(&hk), vec!["HK", "!unsupported:ss", "REJECT"]);
        assert_eq!(hk.outbound.name(), "REJECT");
        assert_eq!(hk.unsupported.as_deref(), Some("ss"));
        let dev = reg.resolve(&PolicyRef::parse("DEVICE:Living Room"));
        assert_eq!(chain(&dev), vec!["DEVICE:Living Room", "REJECT"]);
        assert_eq!(dev.unsupported.as_deref(), Some("DEVICE"));
        let missing = reg.resolve(&PolicyRef::Named("Nope".to_string()));
        assert_eq!(chain(&missing), vec!["Nope", "REJECT"]);
    }

    #[test]
    fn groups_follow_selection_or_first_member() {
        // no persisted selection: select → first member (HK, unsupported); url-test → first member
        let reg = registry(GroupSelections::new());
        let pick = reg.resolve(&PolicyRef::parse("Pick"));
        assert_eq!(chain(&pick), vec!["Pick", "HK", "!unsupported:ss", "REJECT"]);
        let auto = reg.resolve(&PolicyRef::parse("Auto"));
        assert_eq!(chain(&auto), vec!["Auto", "HK", "!unsupported:ss", "REJECT"]);
        // persisted selections are honoured through nesting; a stale selection falls back to the first member
        let mut sel = GroupSelections::new();
        sel.set("Pick", "DIRECT");
        sel.set("Outer", "Pick");
        sel.set("Auto", "D"); // ignored: not a select group
        sel.set("Emptyish", "Gone"); // not a member any more
        let reg = registry(sel);
        let pick = reg.resolve(&PolicyRef::parse("Pick"));
        assert_eq!((chain(&pick), pick.outbound.name()), (vec!["Pick", "DIRECT"], "DIRECT"));
        let outer = reg.resolve(&PolicyRef::parse("Outer"));
        assert_eq!(chain(&outer), vec!["Outer", "Pick", "DIRECT"]);
        let auto = reg.resolve(&PolicyRef::parse("Auto"));
        assert_eq!(chain(&auto)[1], "HK");
        let e = reg.resolve(&PolicyRef::parse("Emptyish"));
        assert_eq!(chain(&e), vec!["Emptyish", "Block", "REJECT-TINYGIF"]);
        assert!(e.unsupported.is_none());
    }

    #[test]
    fn selections_api() {
        let mut s = GroupSelections::new();
        assert!(s.is_empty());
        s.set("G", "A");
        assert_eq!(s.get("G"), Some("A"));
        assert_eq!(s.get("X"), None);
        let s2 = GroupSelections::from_map(HashMap::from([("G".to_string(), "B".to_string())]));
        assert_eq!(s2.get("G"), Some("B"));
    }
}
```

`crates/rurge-policy/Cargo.toml` 的 `[dev-dependencies]` 追加 `rurge-net.workspace = true`（测试里的 `FakeDirect` 需要 `BoxFuture` / `Target`）。

> 若 M1 对 `ss` 策略要求更多参数而报错，按 `cargo test` 的诊断补齐参数（保持 `ss` 类型）。

- [ ] **Step 4: 运行与提交**

```bash
cargo test -p rurge-policy
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-policy
git commit -F - <<'EOF'
feat(policy): 策略注册表：内置 / 别名 / select 持久选择 / 首成员回退 / 未实现协议按 REJECT

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

预期：4 个测试通过。

---

### Task 4: `rurge-inbound` 核心：`Dialer` / `SessionHandle`、来源限制、accept 循环、响应构造

**Files:**
- Modify: `crates/rurge-inbound/src/lib.rs`
- Create: `crates/rurge-inbound/src/session.rs`、`crates/rurge-inbound/src/restrict.rs`、`crates/rurge-inbound/src/listener.rs`、`crates/rurge-inbound/src/responses.rs`、`crates/rurge-inbound/src/testing.rs`（`cfg(test)`）

**Interfaces:**
- Consumes: `rurge_config::session::{ListenerKind, SessionInfo}`、`rurge_net::{BoxFuture, connector::BoxedStream}`、`rurge_proto::RejectKind`、hyper / http / http-body-util / bytes、base64。
- Produces（crate 根再导出 `Dialed, DialError, Dialer, FailKind, HttpAuth, ListenerOpts, Running, SessionHandle, SessionOutcome`）：
  - `SessionOutcome::{Completed, Rejected(RejectKind), Failed(String)}`（`Clone, Debug, PartialEq, Eq`）。
  - `SessionHandle`：`new(id: u64, session: SessionInfo) -> Arc<SessionHandle>`；`id()`；`session() -> &SessionInfo`；`elapsed() -> Duration`；`set_rule(Option<String>)` / `rule() -> Option<String>`；`set_policy_chain(Vec<String>)` / `policy_chain() -> Vec<String>`；`add_up(u64)` / `add_down(u64)` / `bytes() -> (u64, u64)`；`on_finish(f: impl FnOnce(&SessionHandle, &SessionOutcome) + Send + 'static)`（引擎安装日志 / 记录回调）；`finish(SessionOutcome)`（只生效一次，执行回调）；`is_finished()`；`outcome() -> Option<SessionOutcome>`。
  - `Counting<S>`：`Counting::new(inner: S, handle: Arc<SessionHandle>)`，实现 `AsyncRead`（读到的字节计入 `down`）与 `AsyncWrite`（写出的字节计入 `up`）。
  - `FailKind::{Dns, Connect, Timeout, Other}`；`DialError::{Reject { kind: RejectKind, rule: Option<String>, handle: Arc<SessionHandle> }, Failed { kind: FailKind, message: String, rule: Option<String>, handle: Arc<SessionHandle> }}`；`Dialed { stream: BoxedStream, handle: Arc<SessionHandle> }`。
  - `trait Dialer: Send + Sync { fn dial<'a>(&'a self, session: SessionInfo) -> BoxFuture<'a, Result<Dialed, DialError>>; fn relay<'a>(&'a self, client: BoxedStream, upstream: BoxedStream, handle: Arc<SessionHandle>) -> BoxFuture<'a, ()>; }`（`relay` 结束时由实现方 `finish` 句柄）。
  - `restrict::is_local(ip: IpAddr) -> bool`（回环 / RFC 1918 / v4 链路本地 / v6 ULA / v6 链路本地 / 映射到 v4 的按 v4 判）；`restrict::source_allowed(listen: SocketAddr, source: IpAddr) -> bool`（监听回环地址时恒真）。
  - `HttpAuth::{Password(String), UserPass { user: String, password: String }}`；`ListenerOpts { kind: ListenerKind, restrict_to_lan: bool, auth: Option<HttpAuth>, show_error_page: bool, show_error_page_for_reject: bool, drop_hold: Duration }`（`Default`：`Http`、true、None、true、false、30 s）。
  - `Running { local_addr: SocketAddr }` + `Drop` 终止 accept 任务；`listener::serve(listener: TcpListener, name: &'static str, restrict_to_lan: bool, handler: F) -> Running`，`F: Fn(TcpStream, SocketAddr) -> Fut + Send + Sync + 'static`，`Fut: Future<Output = ()> + Send + 'static`：accept 循环 + `JoinSet`（会话 panic 记 ERROR）、accept 错误记 WARN 并退避 50 ms、来源限制（按来源 IP 每 60 s 最多一条 WARN）。
  - `responses`：`ResponseBody = BoxBody<Bytes, hyper::Error>`；`full(Bytes) -> ResponseBody`；`empty() -> ResponseBody`；`TINY_GIF: [u8; 43]`；`tiny_gif() -> Response<ResponseBody>`；`proxy_auth_required() -> Response<ResponseBody>`（407 + `Proxy-Authenticate: Basic realm="rurge"`）；`bad_request(msg: &str) -> Response<ResponseBody>`；`ErrorPage { title: &str, session_id: u64, dst: String, rule: Option<String>, chain: Vec<String>, message: String }` 与 `error_page(status: StatusCode, page: &ErrorPage) -> Response<ResponseBody>`（`text/html; charset=utf-8`，转义 `<>&"`）；`connect_established() -> Response<ResponseBody>`（200 空体）。

- [ ] **Step 1: 写 `session.rs`**

```rust
//! The seam between listeners and the engine (M3 design §6.1).

use rurge_config::session::SessionInfo;
use rurge_net::BoxFuture;
use rurge_net::connector::BoxedStream;
use rurge_proto::RejectKind;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionOutcome {
    Completed,
    Rejected(RejectKind),
    Failed(String),
}

type FinishHook = Box<dyn FnOnce(&SessionHandle, &SessionOutcome) + Send>;

/// Per-session bookkeeping shared by the listener (bytes) and the engine
/// (rule, policy chain, outcome). Finishing is idempotent.
pub struct SessionHandle {
    id: u64,
    started: Instant,
    session: SessionInfo,
    rule: Mutex<Option<String>>,
    policy_chain: Mutex<Vec<String>>,
    up: AtomicU64,
    down: AtomicU64,
    finished: AtomicBool,
    outcome: Mutex<Option<SessionOutcome>>,
    on_finish: Mutex<Option<FinishHook>>,
}

impl SessionHandle {
    pub fn new(id: u64, session: SessionInfo) -> Arc<SessionHandle> {
        Arc::new(SessionHandle {
            id,
            started: Instant::now(),
            session,
            rule: Mutex::new(None),
            policy_chain: Mutex::new(Vec::new()),
            up: AtomicU64::new(0),
            down: AtomicU64::new(0),
            finished: AtomicBool::new(false),
            outcome: Mutex::new(None),
            on_finish: Mutex::new(None),
        })
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn session(&self) -> &SessionInfo {
        &self.session
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    pub fn set_rule(&self, rule: Option<String>) {
        *self.rule.lock().expect("session rule") = rule;
    }

    pub fn rule(&self) -> Option<String> {
        self.rule.lock().expect("session rule").clone()
    }

    pub fn set_policy_chain(&self, chain: Vec<String>) {
        *self.policy_chain.lock().expect("policy chain") = chain;
    }

    pub fn policy_chain(&self) -> Vec<String> {
        self.policy_chain.lock().expect("policy chain").clone()
    }

    pub fn add_up(&self, n: u64) {
        self.up.fetch_add(n, Ordering::Relaxed);
    }

    pub fn add_down(&self, n: u64) {
        self.down.fetch_add(n, Ordering::Relaxed);
    }

    /// (client → upstream, upstream → client) bytes so far.
    pub fn bytes(&self) -> (u64, u64) {
        (self.up.load(Ordering::Relaxed), self.down.load(Ordering::Relaxed))
    }

    /// Installs the hook `finish` runs once (the engine's session log).
    pub fn on_finish(&self, f: impl FnOnce(&SessionHandle, &SessionOutcome) + Send + 'static) {
        *self.on_finish.lock().expect("finish hook") = Some(Box::new(f));
    }

    pub fn finish(&self, outcome: SessionOutcome) {
        if self.finished.swap(true, Ordering::AcqRel) {
            return;
        }
        *self.outcome.lock().expect("outcome") = Some(outcome.clone());
        let hook = self.on_finish.lock().expect("finish hook").take();
        if let Some(hook) = hook {
            hook(self, &outcome);
        }
    }

    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    pub fn outcome(&self) -> Option<SessionOutcome> {
        self.outcome.lock().expect("outcome").clone()
    }
}

/// Counts bytes flowing through a stream into a session handle: reads are
/// `down` (upstream → client), writes are `up` (client → upstream).
pub struct Counting<S> {
    inner: S,
    handle: Arc<SessionHandle>,
}

impl<S> Counting<S> {
    pub fn new(inner: S, handle: Arc<SessionHandle>) -> Counting<S> {
        Counting { inner, handle }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Counting<S> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let res = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &res {
            let n = buf.filled().len() - before;
            self.handle.add_down(n as u64);
        }
        res
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Counting<S> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, data: &[u8]) -> Poll<std::io::Result<usize>> {
        let res = Pin::new(&mut self.inner).poll_write(cx, data);
        if let Poll::Ready(Ok(n)) = &res {
            self.handle.add_up(*n as u64);
        }
        res
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Why a dial failed (drives the protocol-level error code, design §8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailKind {
    Dns,
    Connect,
    Timeout,
    Other,
}

pub struct Dialed {
    pub stream: BoxedStream,
    pub handle: Arc<SessionHandle>,
}

pub enum DialError {
    /// The policy rejected the session; the handle is already finished.
    Reject {
        kind: RejectKind,
        rule: Option<String>,
        handle: Arc<SessionHandle>,
    },
    /// Resolution or connection failed; the handle is already finished.
    Failed {
        kind: FailKind,
        message: String,
        rule: Option<String>,
        handle: Arc<SessionHandle>,
    },
}

/// Implemented by the engine: rules → policy → outbound (`dial`), then the
/// counted bidirectional copy (`relay`, which finishes the handle).
pub trait Dialer: Send + Sync {
    fn dial<'a>(&'a self, session: SessionInfo) -> BoxFuture<'a, Result<Dialed, DialError>>;
    fn relay<'a>(&'a self, client: BoxedStream, upstream: BoxedStream, handle: Arc<SessionHandle>) -> BoxFuture<'a, ()>;
}
```

- [ ] **Step 2: 写 `restrict.rs`**

```rust
//! `proxy-restricted-to-lan` (M3 design §6.4): only local sources may use a
//! listener bound to a non-loopback address.

use std::net::{IpAddr, SocketAddr};

/// Loopback, RFC 1918, IPv4 link-local, IPv6 unique-local / link-local; an
/// IPv4-mapped IPv6 address is judged as its IPv4 form.
pub fn is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => a.is_loopback() || a.is_private() || a.is_link_local(),
        IpAddr::V6(a) => {
            if let Some(v4) = a.to_ipv4_mapped() {
                return is_local(IpAddr::V4(v4));
            }
            a.is_loopback() || a.is_unique_local() || a.is_unicast_link_local()
        }
    }
}

/// A loopback listener trusts every source (only local processes reach it).
pub fn source_allowed(listen: SocketAddr, source: IpAddr) -> bool {
    listen.ip().is_loopback() || is_local(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_sources() {
        let ok = |s: &str| is_local(s.parse().unwrap());
        assert!(ok("127.0.0.1") && ok("10.1.2.3") && ok("172.16.5.5") && ok("192.168.1.9") && ok("169.254.1.1"));
        assert!(ok("::1") && ok("fd00::1") && ok("fe80::1") && ok("::ffff:192.168.0.1"));
        assert!(!ok("8.8.8.8") && !ok("2001:db8::1") && !ok("::ffff:1.1.1.1") && !ok("100.64.0.1"));
        let lan: SocketAddr = "0.0.0.0:6152".parse().unwrap();
        let lo: SocketAddr = "127.0.0.1:6152".parse().unwrap();
        assert!(source_allowed(lan, "192.168.1.2".parse().unwrap()));
        assert!(!source_allowed(lan, "8.8.8.8".parse().unwrap()));
        assert!(source_allowed(lo, "8.8.8.8".parse().unwrap()));
    }
}
```

- [ ] **Step 3: 写 `listener.rs`**

```rust
//! Shared accept loop (M3 design §6.4): one task per connection, panics
//! isolated by a `JoinSet`, source restriction, accept-error backoff.

use crate::restrict::source_allowed;
use rurge_config::session::ListenerKind;
use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{JoinHandle, JoinSet};

/// Basic credentials accepted by an HTTP listener.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HttpAuth {
    /// `password@` from `http-listen`: any user name, this password.
    Password(String),
    /// `wifi-access-http-auth`: both must match.
    UserPass { user: String, password: String },
}

#[derive(Clone, Debug)]
pub struct ListenerOpts {
    pub kind: ListenerKind,
    pub restrict_to_lan: bool,
    pub auth: Option<HttpAuth>,
    pub show_error_page: bool,
    pub show_error_page_for_reject: bool,
    /// How long REJECT-DROP keeps a connection open without answering.
    pub drop_hold: Duration,
}

impl Default for ListenerOpts {
    fn default() -> Self {
        ListenerOpts {
            kind: ListenerKind::Http,
            restrict_to_lan: true,
            auth: None,
            show_error_page: true,
            show_error_page_for_reject: false,
            drop_hold: Duration::from_secs(30),
        }
    }
}

/// A bound listener; dropping it stops accepting (live sessions finish on their own).
pub struct Running {
    pub local_addr: SocketAddr,
    task: JoinHandle<()>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
    }
}

const REJECTED_SOURCE_LOG_INTERVAL: Duration = Duration::from_secs(60);
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(50);

pub(crate) fn serve<F, Fut>(listener: TcpListener, name: &'static str, restrict_to_lan: bool, handler: F) -> Running
where
    F: Fn(TcpStream, SocketAddr) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let local_addr = listener.local_addr().expect("bound listener has an address");
    let handler = Arc::new(handler);
    let task = tokio::spawn(async move {
        let mut sessions: JoinSet<()> = JoinSet::new();
        let warned: Mutex<HashMap<IpAddr, Instant>> = Mutex::new(HashMap::new());
        loop {
            tokio::select! {
                accepted = listener.accept() => match accepted {
                    Ok((stream, peer)) => {
                        if restrict_to_lan && !source_allowed(local_addr, peer.ip()) {
                            let mut map = warned.lock().expect("warned sources");
                            let now = Instant::now();
                            let due = map.get(&peer.ip()).is_none_or(|t| now.duration_since(*t) >= REJECTED_SOURCE_LOG_INTERVAL);
                            if due {
                                map.insert(peer.ip(), now);
                                tracing::warn!(listener = name, source = %peer.ip(), "connection refused: source is not on the LAN (proxy-restricted-to-lan)");
                            }
                            drop(stream);
                            continue;
                        }
                        let _ = stream.set_nodelay(true);
                        let h = handler.clone();
                        sessions.spawn(async move { h(stream, peer).await });
                    }
                    Err(e) => {
                        tracing::warn!(listener = name, error = %e, "accept failed");
                        tokio::time::sleep(ACCEPT_ERROR_BACKOFF).await;
                    }
                },
                Some(joined) = sessions.join_next(), if !sessions.is_empty() => {
                    if let Err(e) = joined
                        && e.is_panic()
                    {
                        tracing::error!(listener = name, "session task panicked: {e}");
                    }
                }
            }
        }
    });
    Running { local_addr, task }
}

pub(crate) async fn bind(addr: SocketAddr) -> std::io::Result<TcpListener> {
    TcpListener::bind(addr).await
}
```

> `Option::is_none_or` 需要 Rust ≥ 1.82（满足）。`sessions.join_next()` 只在集合非空时被 `select!` 轮询，避免空集合时立即返回 `None` 的忙循环。

- [ ] **Step 4: 写 `responses.rs`**

```rust
//! Responses the HTTP listener produces itself (M3 design §8): error pages,
//! the 1×1 GIF, 407 / 400 and the CONNECT success line.

use bytes::Bytes;
use http::{Response, StatusCode, header};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty, Full};

pub type ResponseBody = BoxBody<Bytes, hyper::Error>;

/// A 1×1 transparent GIF (43 bytes).
pub const TINY_GIF: [u8; 43] = [
    0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff,
    0x21, 0xf9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02,
    0x02, 0x44, 0x01, 0x00, 0x3b,
];

pub fn full(bytes: Bytes) -> ResponseBody {
    Full::new(bytes).map_err(|never| match never {}).boxed()
}

pub fn empty() -> ResponseBody {
    Empty::<Bytes>::new().map_err(|never| match never {}).boxed()
}

pub fn connect_established() -> Response<ResponseBody> {
    Response::builder().status(StatusCode::OK).body(empty()).expect("static response")
}

pub fn tiny_gif() -> Response<ResponseBody> {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/gif")
        .header(header::CACHE_CONTROL, "no-store")
        .body(full(Bytes::from_static(&TINY_GIF)))
        .expect("static response")
}

pub fn proxy_auth_required() -> Response<ResponseBody> {
    Response::builder()
        .status(StatusCode::PROXY_AUTHENTICATION_REQUIRED)
        .header(header::PROXY_AUTHENTICATE, "Basic realm=\"rurge\"")
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(full(Bytes::from_static(b"proxy authentication required")))
        .expect("static response")
}

pub fn bad_request(msg: &str) -> Response<ResponseBody> {
    Response::builder()
        .status(StatusCode::BAD_REQUEST)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(full(Bytes::from(msg.to_string())))
        .expect("static response")
}

pub struct ErrorPage<'a> {
    pub title: &'a str,
    pub session_id: u64,
    pub dst: String,
    pub rule: Option<String>,
    pub chain: Vec<String>,
    pub message: String,
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// The rurge error page: self-contained HTML, no external resources.
pub fn error_page(status: StatusCode, page: &ErrorPage<'_>) -> Response<ResponseBody> {
    let rule = page.rule.as_deref().unwrap_or("(none)");
    let chain = if page.chain.is_empty() { "(none)".to_string() } else { page.chain.join(" > ") };
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>rurge: {title}</title>\
<style>body{{font-family:system-ui,sans-serif;margin:3em auto;max-width:40em;color:#222}}code{{background:#f3f3f3;padding:.1em .3em}}</style></head>\
<body><h1>{title}</h1><p>{message}</p><table>\
<tr><td>Destination</td><td><code>{dst}</code></td></tr>\
<tr><td>Rule</td><td><code>{rule}</code></td></tr>\
<tr><td>Policy</td><td><code>{chain}</code></td></tr>\
<tr><td>Session</td><td><code>{id}</code></td></tr></table>\
<p style=\"color:#888\">Generated by rurge</p></body></html>",
        title = escape(page.title),
        message = escape(&page.message),
        dst = escape(&page.dst),
        rule = escape(rule),
        chain = escape(&chain),
        id = page.session_id,
    );
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(full(Bytes::from(html)))
        .expect("static response")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn responses_have_the_expected_shape() {
        let gif = tiny_gif();
        assert_eq!(gif.status(), 200);
        assert_eq!(gif.headers()[header::CONTENT_TYPE], "image/gif");
        let body = gif.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body.len(), 43);
        assert_eq!(&body[..6], b"GIF89a");
        let auth = proxy_auth_required();
        assert_eq!(auth.status(), 407);
        assert_eq!(auth.headers()[header::PROXY_AUTHENTICATE], "Basic realm=\"rurge\"");
        let page = error_page(
            StatusCode::FORBIDDEN,
            &ErrorPage {
                title: "Rejected",
                session_id: 7,
                dst: "ads.example:443".to_string(),
                rule: Some("DOMAIN-SUFFIX,<ads>.example,REJECT".to_string()),
                chain: vec!["Block".to_string(), "REJECT".to_string()],
                message: "rejected by policy".to_string(),
            },
        );
        assert_eq!(page.status(), 403);
        let html = String::from_utf8(page.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
        assert!(html.contains("&lt;ads&gt;.example") && html.contains("Block &gt; REJECT") && html.contains("<code>7</code>"), "{html}");
        assert_eq!(connect_established().status(), 200);
        assert_eq!(bad_request("nope").status(), 400);
    }
}
```

- [ ] **Step 5: 写 `testing.rs`（`cfg(test)`，Task 5 ～ 7 的假引擎）**

```rust
//! A scripted `Dialer` for listener tests: the destination host decides the
//! outcome, `relay` is a plain bidirectional copy.

use crate::session::{DialError, Dialed, Dialer, FailKind, SessionHandle, SessionOutcome};
use rurge_config::HostName;
use rurge_config::session::SessionInfo;
use rurge_net::BoxFuture;
use rurge_net::connector::BoxedStream;
use rurge_proto::RejectKind;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::net::TcpStream;

/// Hosts: `echo.test` → `echo`, `target.test` → `target`, `reject.test`
/// (port 443 REJECT, 444 DROP, 445 NO-DROP, 446 TINYGIF), `fail.test`
/// (connect failure), `dns.test` (dns failure), `slow.test` (timeout).
pub(crate) struct FakeDialer {
    pub echo: SocketAddr,
    pub target: Option<SocketAddr>,
    next_id: AtomicU64,
    pub handles: Mutex<Vec<Arc<SessionHandle>>>,
}

impl FakeDialer {
    pub(crate) fn new(echo: SocketAddr, target: Option<SocketAddr>) -> Arc<FakeDialer> {
        Arc::new(FakeDialer {
            echo,
            target,
            next_id: AtomicU64::new(1),
            handles: Mutex::new(Vec::new()),
        })
    }

    pub(crate) fn sessions(&self) -> Vec<Arc<SessionHandle>> {
        self.handles.lock().unwrap().clone()
    }

    fn handle(&self, session: SessionInfo) -> Arc<SessionHandle> {
        let h = SessionHandle::new(self.next_id.fetch_add(1, Ordering::Relaxed), session);
        h.set_rule(Some("FAKE,rule".to_string()));
        self.handles.lock().unwrap().push(h.clone());
        h
    }
}

impl Dialer for FakeDialer {
    fn dial<'a>(&'a self, session: SessionInfo) -> BoxFuture<'a, Result<Dialed, DialError>> {
        Box::pin(async move {
            let host = match &session.dst_host {
                HostName::Domain(d) => d.clone(),
                HostName::Ip(ip) => ip.to_string(),
            };
            let port = session.dst_port;
            let handle = self.handle(session);
            let connect_to = match host.as_str() {
                "echo.test" | "127.0.0.1" => Some(self.echo),
                "target.test" => self.target,
                _ => None,
            };
            if let Some(addr) = connect_to {
                handle.set_policy_chain(vec!["DIRECT".to_string()]);
                let stream = TcpStream::connect(addr).await.map_err(|e| DialError::Failed {
                    kind: FailKind::Connect,
                    message: e.to_string(),
                    rule: handle.rule(),
                    handle: handle.clone(),
                })?;
                return Ok(Dialed {
                    stream: Box::new(stream),
                    handle,
                });
            }
            let rule = handle.rule();
            match host.as_str() {
                "reject.test" => {
                    let kind = match port {
                        444 => RejectKind::Drop,
                        445 => RejectKind::NoDrop,
                        446 => RejectKind::TinyGif,
                        _ => RejectKind::Reject,
                    };
                    handle.set_policy_chain(vec![kind.name().to_string()]);
                    handle.finish(SessionOutcome::Rejected(kind));
                    Err(DialError::Reject { kind, rule, handle })
                }
                "dns.test" => {
                    handle.finish(SessionOutcome::Failed("dns lookup failed".into()));
                    Err(DialError::Failed {
                        kind: FailKind::Dns,
                        message: "dns lookup failed".into(),
                        rule,
                        handle,
                    })
                }
                "slow.test" => {
                    handle.finish(SessionOutcome::Failed("connect timed out".into()));
                    Err(DialError::Failed {
                        kind: FailKind::Timeout,
                        message: "connect timed out".into(),
                        rule,
                        handle,
                    })
                }
                _ => {
                    handle.finish(SessionOutcome::Failed("connection refused".into()));
                    Err(DialError::Failed {
                        kind: FailKind::Connect,
                        message: "connection refused".into(),
                        rule,
                        handle,
                    })
                }
            }
        })
    }

    fn relay<'a>(&'a self, mut client: BoxedStream, mut upstream: BoxedStream, handle: Arc<SessionHandle>) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            match tokio::io::copy_bidirectional(&mut client, &mut upstream).await {
                Ok((up, down)) => {
                    handle.add_up(up);
                    handle.add_down(down);
                    handle.finish(SessionOutcome::Completed);
                }
                Err(e) => handle.finish(SessionOutcome::Failed(e.to_string())),
            }
        })
    }
}

/// A TCP echo server on the loopback.
pub(crate) async fn echo_server() -> SocketAddr {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                loop {
                    match s.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if s.write_all(&buf[..n]).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            });
        }
    });
    addr
}
```

`lib.rs`：

```rust
pub mod listener;
pub mod responses;
pub mod restrict;
pub mod session;
#[cfg(test)]
pub(crate) mod testing;

pub use listener::{HttpAuth, ListenerOpts, Running};
pub use session::{Counting, DialError, Dialed, Dialer, FailKind, SessionHandle, SessionOutcome};
```

- [ ] **Step 6: `session.rs` 的测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::HostName;
    use std::sync::atomic::AtomicUsize;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn finish_runs_the_hook_once_and_keeps_the_outcome() {
        let h = SessionHandle::new(7, SessionInfo::tcp(HostName::parse("a.test"), 443));
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        h.on_finish(move |handle, outcome| {
            assert_eq!(handle.id(), 7);
            assert_eq!(*outcome, SessionOutcome::Completed);
            c.fetch_add(1, Ordering::SeqCst);
        });
        assert!(!h.is_finished());
        h.set_rule(Some("DOMAIN,a.test,DIRECT".into()));
        h.set_policy_chain(vec!["DIRECT".into()]);
        h.finish(SessionOutcome::Completed);
        h.finish(SessionOutcome::Failed("late".into()));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(h.outcome(), Some(SessionOutcome::Completed));
        assert_eq!(h.rule().as_deref(), Some("DOMAIN,a.test,DIRECT"));
        assert_eq!(h.policy_chain(), vec!["DIRECT".to_string()]);
    }

    #[tokio::test]
    async fn counting_stream_attributes_bytes() {
        let (mut a, b) = tokio::io::duplex(64);
        let h = SessionHandle::new(1, SessionInfo::tcp(HostName::parse("a.test"), 80));
        let mut counted = Counting::new(b, h.clone());
        counted.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 5];
        a.read_exact(&mut buf).await.unwrap();
        a.write_all(b"ok").await.unwrap();
        let mut buf2 = [0u8; 2];
        counted.read_exact(&mut buf2).await.unwrap();
        assert_eq!(h.bytes(), (5, 2));
    }
}
```

- [ ] **Step 7: 运行与提交**

```bash
cargo test -p rurge-inbound
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-inbound
git commit -F - <<'EOF'
feat(inbound): Dialer / SessionHandle 接口、来源限制、accept 循环与自产响应

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

预期：4 个测试通过（`restrict` 1、`responses` 1、`session` 2）。`testing.rs` 与 `listener::serve` 在本任务尚无调用方，`cfg(test)` 与 `pub(crate)` 项若触发 dead-code 告警，给 `serve` / `bind` 加 `#[allow(dead_code)]` 并在 Task 5 移除。

---

### Task 5: `rurge-inbound` SOCKS5 监听

**Files:**
- Modify: `crates/rurge-inbound/src/lib.rs`
- Create: `crates/rurge-inbound/src/socks5.rs`

**Interfaces:**
- Consumes: Task 4 的 `Dialer` / `DialError` / `FailKind` / `ListenerOpts` / `listener::{serve, bind}`、`rurge_config::{HostName, session::{ListenerKind, SessionInfo, Transport}}`、`rurge_proto::RejectKind`。
- Produces（crate 根再导出 `Socks5Listener`）：`Socks5Listener::bind(addr: SocketAddr, dialer: Arc<dyn Dialer>, opts: ListenerOpts) -> io::Result<Running>`；协议：方法协商只接受 `0x00`（否则回 `05 FF` 关闭）；`CONNECT` 支持 ATYP `0x01` / `0x03` / `0x04`；`BIND` / `UDP ASSOCIATE` 回 `0x07`；dial 成功回 `05 00 00 01 0.0.0.0 0` 再 `relay`；`Reject { Drop }` → 等 `drop_hold` 后关闭；其他 `Reject` → `0x02`；`Failed { Dns }` → `0x04`；`Failed { Connect | Timeout | Other }` → `0x05`。`SessionInfo { listener: Socks5, in_port: 监听端口, src: 对端, transport: Tcp }`。

- [ ] **Step 1: 写 `socks5.rs`**

```rust
//! SOCKS5 (RFC 1928) listener: no authentication, CONNECT only (M3 design §6.3).

use crate::listener::{ListenerOpts, Running, bind, serve};
use crate::session::{DialError, Dialer, FailKind};
use rurge_config::HostName;
use rurge_config::session::{ListenerKind, SessionInfo, Transport};
use rurge_proto::RejectKind;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const VERSION: u8 = 0x05;
const METHOD_NONE: u8 = 0x00;
const METHOD_UNACCEPTABLE: u8 = 0xff;
const CMD_CONNECT: u8 = 0x01;
const ATYP_V4: u8 = 0x01;
const ATYP_DOMAIN: u8 = 0x03;
const ATYP_V6: u8 = 0x04;

pub const REP_SUCCESS: u8 = 0x00;
pub const REP_NOT_ALLOWED: u8 = 0x02;
pub const REP_HOST_UNREACHABLE: u8 = 0x04;
pub const REP_CONNECTION_REFUSED: u8 = 0x05;
pub const REP_COMMAND_NOT_SUPPORTED: u8 = 0x07;

pub struct Socks5Listener;

impl Socks5Listener {
    pub async fn bind(addr: SocketAddr, dialer: Arc<dyn Dialer>, opts: ListenerOpts) -> io::Result<Running> {
        let listener = bind(addr).await?;
        let local = listener.local_addr()?;
        let opts = Arc::new(opts);
        Ok(serve(listener, "socks5", opts.restrict_to_lan, move |stream, peer| {
            let dialer = dialer.clone();
            let opts = opts.clone();
            async move {
                if let Err(e) = handle(stream, peer, local, dialer, opts).await {
                    tracing::debug!(listener = "socks5", source = %peer, error = %e, "socks5 session ended with an error");
                }
            }
        }))
    }
}

fn reply(code: u8) -> [u8; 10] {
    [VERSION, code, 0x00, ATYP_V4, 0, 0, 0, 0, 0, 0]
}

async fn read_request(stream: &mut TcpStream) -> io::Result<Result<(HostName, u16), u8>> {
    let mut head = [0u8; 4];
    stream.read_exact(&mut head).await?;
    if head[0] != VERSION {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad socks version in request"));
    }
    let host = match head[3] {
        ATYP_V4 => {
            let mut b = [0u8; 4];
            stream.read_exact(&mut b).await?;
            HostName::Ip(IpAddr::V4(Ipv4Addr::from(b)))
        }
        ATYP_DOMAIN => {
            let len = stream.read_u8().await? as usize;
            let mut b = vec![0u8; len];
            stream.read_exact(&mut b).await?;
            let s = String::from_utf8(b).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "non-utf8 domain"))?;
            HostName::parse(&s)
        }
        ATYP_V6 => {
            let mut b = [0u8; 16];
            stream.read_exact(&mut b).await?;
            HostName::Ip(IpAddr::V6(Ipv6Addr::from(b)))
        }
        _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "unknown address type")),
    };
    let port = stream.read_u16().await?;
    if head[1] != CMD_CONNECT {
        return Ok(Err(REP_COMMAND_NOT_SUPPORTED));
    }
    Ok(Ok((host, port)))
}

async fn handle(mut stream: TcpStream, peer: SocketAddr, local: SocketAddr, dialer: Arc<dyn Dialer>, opts: Arc<ListenerOpts>) -> io::Result<()> {
    // method negotiation
    let mut hello = [0u8; 2];
    stream.read_exact(&mut hello).await?;
    if hello[0] != VERSION {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad socks version"));
    }
    let mut methods = vec![0u8; hello[1] as usize];
    stream.read_exact(&mut methods).await?;
    if !methods.contains(&METHOD_NONE) {
        stream.write_all(&[VERSION, METHOD_UNACCEPTABLE]).await?;
        return Ok(());
    }
    stream.write_all(&[VERSION, METHOD_NONE]).await?;

    let (host, port) = match read_request(&mut stream).await? {
        Ok(target) => target,
        Err(code) => {
            stream.write_all(&reply(code)).await?;
            return Ok(());
        }
    };
    let mut session = SessionInfo::tcp(host, port);
    session.src = peer;
    session.in_port = local.port();
    session.listener = ListenerKind::Socks5;
    session.transport = Transport::Tcp;

    match dialer.dial(session).await {
        Ok(dialed) => {
            stream.write_all(&reply(REP_SUCCESS)).await?;
            dialer.relay(Box::new(stream), dialed.stream, dialed.handle).await;
            Ok(())
        }
        Err(DialError::Reject { kind: RejectKind::Drop, .. }) => {
            tokio::time::sleep(opts.drop_hold).await;
            Ok(())
        }
        Err(DialError::Reject { .. }) => {
            stream.write_all(&reply(REP_NOT_ALLOWED)).await?;
            Ok(())
        }
        Err(DialError::Failed { kind, .. }) => {
            let code = match kind {
                FailKind::Dns => REP_HOST_UNREACHABLE,
                FailKind::Connect | FailKind::Timeout | FailKind::Other => REP_CONNECTION_REFUSED,
            };
            stream.write_all(&reply(code)).await?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeDialer, echo_server};
    use std::time::Duration;

    async fn listener(drop_hold: Duration) -> (Running, Arc<FakeDialer>) {
        let echo = echo_server().await;
        let dialer = FakeDialer::new(echo, None);
        let opts = ListenerOpts {
            kind: ListenerKind::Socks5,
            drop_hold,
            ..ListenerOpts::default()
        };
        let running = Socks5Listener::bind("127.0.0.1:0".parse().unwrap(), dialer.clone(), opts).await.unwrap();
        (running, dialer)
    }

    async fn negotiate(addr: SocketAddr) -> TcpStream {
        let mut s = TcpStream::connect(addr).await.unwrap();
        s.write_all(&[VERSION, 1, METHOD_NONE]).await.unwrap();
        let mut r = [0u8; 2];
        s.read_exact(&mut r).await.unwrap();
        assert_eq!(r, [VERSION, METHOD_NONE]);
        s
    }

    fn domain_request(host: &str, port: u16) -> Vec<u8> {
        let mut v = vec![VERSION, CMD_CONNECT, 0, ATYP_DOMAIN, host.len() as u8];
        v.extend_from_slice(host.as_bytes());
        v.extend_from_slice(&port.to_be_bytes());
        v
    }

    async fn read_reply(s: &mut TcpStream) -> [u8; 10] {
        let mut r = [0u8; 10];
        s.read_exact(&mut r).await.unwrap();
        r
    }

    #[tokio::test]
    async fn connect_by_domain_and_by_ipv4_relays_bytes() {
        let (running, dialer) = listener(Duration::from_secs(30)).await;
        let mut s = negotiate(running.local_addr).await;
        s.write_all(&domain_request("echo.test", 7)).await.unwrap();
        let r = read_reply(&mut s).await;
        assert_eq!((r[0], r[1]), (VERSION, REP_SUCCESS));
        s.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        s.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");
        drop(s);
        // IPv4 address type
        let mut s = negotiate(running.local_addr).await;
        let mut req = vec![VERSION, CMD_CONNECT, 0, ATYP_V4, 127, 0, 0, 1];
        req.extend_from_slice(&7u16.to_be_bytes());
        s.write_all(&req).await.unwrap();
        assert_eq!(read_reply(&mut s).await[1], REP_SUCCESS);
        s.write_all(b"x").await.unwrap();
        let mut one = [0u8; 1];
        s.read_exact(&mut one).await.unwrap();
        drop(s);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let sessions = dialer.sessions();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].session().listener, ListenerKind::Socks5);
        assert_eq!(sessions[0].session().dst_host, HostName::parse("echo.test"));
        assert_eq!(sessions[0].session().in_port, running.local_addr.port());
        assert!(sessions[0].is_finished());
        assert_eq!(sessions[0].bytes(), (4, 4));
    }

    #[tokio::test]
    async fn rejects_failures_and_unsupported_commands_map_to_reply_codes() {
        let (running, _dialer) = listener(Duration::from_millis(200)).await;
        for (host, port, expected) in [
            ("reject.test", 443, REP_NOT_ALLOWED),
            ("reject.test", 445, REP_NOT_ALLOWED),
            ("reject.test", 446, REP_NOT_ALLOWED),
            ("dns.test", 80, REP_HOST_UNREACHABLE),
            ("fail.test", 80, REP_CONNECTION_REFUSED),
            ("slow.test", 80, REP_CONNECTION_REFUSED),
        ] {
            let mut s = negotiate(running.local_addr).await;
            s.write_all(&domain_request(host, port)).await.unwrap();
            let r = read_reply(&mut s).await;
            assert_eq!(r[1], expected, "{host}:{port}");
            let mut eof = [0u8; 1];
            assert_eq!(s.read(&mut eof).await.unwrap(), 0, "connection closed after {host}");
        }
        // REJECT-DROP: no reply until the hold expires, then closed
        let mut s = negotiate(running.local_addr).await;
        s.write_all(&domain_request("reject.test", 444)).await.unwrap();
        let mut buf = [0u8; 10];
        let started = std::time::Instant::now();
        let n = tokio::time::timeout(Duration::from_secs(2), s.read(&mut buf)).await.unwrap().unwrap();
        assert_eq!(n, 0, "drop must not answer");
        assert!(started.elapsed() >= Duration::from_millis(180), "{:?}", started.elapsed());
        // BIND is not supported
        let mut s = negotiate(running.local_addr).await;
        let mut req = vec![VERSION, 0x02, 0, ATYP_DOMAIN, 9];
        req.extend_from_slice(b"echo.test");
        req.extend_from_slice(&7u16.to_be_bytes());
        s.write_all(&req).await.unwrap();
        assert_eq!(read_reply(&mut s).await[1], REP_COMMAND_NOT_SUPPORTED);
    }

    #[tokio::test]
    async fn refuses_clients_that_require_authentication() {
        let (running, _dialer) = listener(Duration::from_secs(30)).await;
        let mut s = TcpStream::connect(running.local_addr).await.unwrap();
        s.write_all(&[VERSION, 1, 0x02]).await.unwrap(); // username/password only
        let mut r = [0u8; 2];
        s.read_exact(&mut r).await.unwrap();
        assert_eq!(r, [VERSION, METHOD_UNACCEPTABLE]);
        let mut eof = [0u8; 1];
        assert_eq!(s.read(&mut eof).await.unwrap(), 0);
    }
}
```

`lib.rs`：追加 `pub mod socks5;` 与 `pub use socks5::Socks5Listener;`，并移除 Task 4 临时加的 `#[allow(dead_code)]`。

> `tokio::io::AsyncReadExt::read_u8` / `read_u16`（大端）随 `io-util` feature 提供。字节计数断言 `(4, 4)` 依赖 `FakeDialer::relay` 用 `copy_bidirectional` 的返回值。

- [ ] **Step 2: 运行与提交**

```bash
cargo test -p rurge-inbound socks5
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-inbound
git commit -F - <<'EOF'
feat(inbound): SOCKS5 监听（无认证、CONNECT、错误应答码）

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

预期：3 个新测试通过。

---

### Task 6: `rurge-inbound` HTTP 监听：CONNECT、Basic 认证、非法请求

**Files:**
- Modify: `crates/rurge-inbound/src/lib.rs`
- Create: `crates/rurge-inbound/src/http.rs`

**Interfaces:**
- Consumes: Task 4 的 `Dialer` / `DialError` / `Dialed` / `SessionHandle` / `Counting` / `ListenerOpts` / `HttpAuth` / `listener::{serve, bind}` / `responses::*`；hyper 1：`hyper::server::conn::http1::Builder`、`hyper::service::service_fn`、`hyper::upgrade::on(&mut req) -> OnUpgrade`、`hyper::body::Incoming`；`hyper_util::rt::TokioIo`；`base64::engine::general_purpose::STANDARD` + `base64::Engine`。
- Produces（crate 根再导出 `HttpListener`）：`HttpListener::bind(addr: SocketAddr, dialer: Arc<dyn Dialer>, opts: ListenerOpts) -> io::Result<Running>`。行为：每个连接 `serve_connection(..).with_upgrades()`；`opts.auth` 为 `Some` 时先校验 `Proxy-Authorization: Basic`（`Password(p)` 只比较密码，`UserPass` 都比较），失败 → 407；`CONNECT host[:port]`（默认 443）→ `SessionInfo { listener: Http, protocol: None }` → dial：成功 → 200 后 upgrade → `dialer.relay`；`Reject { Drop }` → 等 `drop_hold` 后关闭；其他 `Reject` / `Failed` → 直接关闭（M3a）；既非 CONNECT 又非绝对 URI → 400；绝对 URI 的明文请求交 Task 7 的 `forward`（本任务先返回 `501 Not Implemented` 占位，Task 7 替换）。
- 内部：`pub(crate) fn authorized(auth: &HttpAuth, header: Option<&HeaderValue>) -> bool`；`enum HandlerError { Close }`（实现 `std::error::Error`，hyper 收到它就关闭连接）。

- [ ] **Step 1: 写 `http.rs`**

```rust
//! HTTP/1.1 proxy listener on hyper (M3 design §6.2): CONNECT tunnels and
//! absolute-URI plain requests, Basic authentication, per-request dialing.

use crate::listener::{HttpAuth, ListenerOpts, Running, bind, serve};
use crate::responses::{self, ResponseBody};
use crate::session::{DialError, Dialer, SessionOutcome};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use http::{HeaderValue, Method, Request, Response, StatusCode, header};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use rurge_config::HostName;
use rurge_config::session::{ListenerKind, SessionInfo, Transport};
use rurge_net::connector::BoxedStream;
use rurge_proto::RejectKind;
use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpStream;

pub struct HttpListener;

struct Ctx {
    dialer: Arc<dyn Dialer>,
    opts: Arc<ListenerOpts>,
    local: SocketAddr,
    peer: SocketAddr,
}

/// Returned from the service to make hyper close the connection without a response.
#[derive(Debug)]
pub(crate) enum HandlerError {
    Close,
}

impl fmt::Display for HandlerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("connection closed by policy")
    }
}

impl std::error::Error for HandlerError {}

impl HttpListener {
    pub async fn bind(addr: SocketAddr, dialer: Arc<dyn Dialer>, opts: ListenerOpts) -> io::Result<Running> {
        let listener = bind(addr).await?;
        let local = listener.local_addr()?;
        let opts = Arc::new(opts);
        Ok(serve(listener, "http", opts.restrict_to_lan, move |stream, peer| {
            let ctx = Arc::new(Ctx {
                dialer: dialer.clone(),
                opts: opts.clone(),
                local,
                peer,
            });
            async move { serve_connection(stream, ctx).await }
        }))
    }
}

async fn serve_connection(stream: TcpStream, ctx: Arc<Ctx>) {
    let service = service_fn(move |req: Request<Incoming>| {
        let ctx = ctx.clone();
        async move { handle(req, ctx).await }
    });
    let conn = hyper::server::conn::http1::Builder::new()
        .preserve_header_case(true)
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades();
    if let Err(e) = conn.await {
        tracing::debug!(listener = "http", error = %e, "http connection ended");
    }
}

pub(crate) fn authorized(auth: &HttpAuth, header: Option<&HeaderValue>) -> bool {
    let Some(value) = header.and_then(|h| h.to_str().ok()) else {
        return false;
    };
    let Some(encoded) = value
        .strip_prefix("Basic ")
        .or_else(|| value.strip_prefix("basic "))
    else {
        return false;
    };
    let Ok(decoded) = BASE64.decode(encoded.trim()) else {
        return false;
    };
    let Ok(text) = std::str::from_utf8(&decoded) else {
        return false;
    };
    let (user, password) = text.split_once(':').unwrap_or(("", text));
    match auth {
        HttpAuth::Password(p) => password == p,
        HttpAuth::UserPass { user: u, password: p } => user == u && password == p,
    }
}

async fn handle(req: Request<Incoming>, ctx: Arc<Ctx>) -> Result<Response<ResponseBody>, HandlerError> {
    if let Some(auth) = &ctx.opts.auth
        && !authorized(auth, req.headers().get(header::PROXY_AUTHORIZATION))
    {
        return Ok(responses::proxy_auth_required());
    }
    if req.method() == Method::CONNECT {
        return connect(req, ctx).await;
    }
    if req.uri().scheme().is_some() && req.uri().authority().is_some() {
        return forward(req, ctx).await;
    }
    Ok(responses::bad_request("rurge is an HTTP proxy: send CONNECT or an absolute-URI request"))
}

fn session_for(ctx: &Ctx, host: HostName, port: u16) -> SessionInfo {
    let mut s = SessionInfo::tcp(host, port);
    s.src = ctx.peer;
    s.in_port = ctx.local.port();
    s.listener = ListenerKind::Http;
    s.transport = Transport::Tcp;
    s
}

async fn connect(mut req: Request<Incoming>, ctx: Arc<Ctx>) -> Result<Response<ResponseBody>, HandlerError> {
    let Some(authority) = req.uri().authority().cloned() else {
        return Ok(responses::bad_request("CONNECT needs host:port"));
    };
    let host = HostName::parse(authority.host());
    let port = authority.port_u16().unwrap_or(443);
    let session = session_for(&ctx, host, port);
    match ctx.dialer.dial(session).await {
        Ok(dialed) => {
            let upgrade = hyper::upgrade::on(&mut req);
            let dialer = ctx.dialer.clone();
            tokio::spawn(async move {
                match upgrade.await {
                    Ok(upgraded) => {
                        let client: BoxedStream = Box::new(TokioIo::new(upgraded));
                        dialer.relay(client, dialed.stream, dialed.handle).await;
                    }
                    Err(e) => dialed.handle.finish(SessionOutcome::Failed(format!("upgrade failed: {e}"))),
                }
            });
            Ok(responses::connect_established())
        }
        Err(DialError::Reject { kind: RejectKind::Drop, .. }) => {
            tokio::time::sleep(ctx.opts.drop_hold).await;
            Err(HandlerError::Close)
        }
        Err(DialError::Reject { .. }) | Err(DialError::Failed { .. }) => Err(HandlerError::Close),
    }
}

async fn forward(_req: Request<Incoming>, _ctx: Arc<Ctx>) -> Result<Response<ResponseBody>, HandlerError> {
    // Task 7 replaces this with per-request forwarding.
    Ok(Response::builder()
        .status(StatusCode::NOT_IMPLEMENTED)
        .body(responses::empty())
        .expect("static response"))
}
```

`lib.rs`：追加 `pub mod http;` 与 `pub use http::HttpListener;`。

> hyper 1 的 CONNECT 语义：service 返回 2xx 后 `OnUpgrade` 才会解析出 `Upgraded`；`TokioIo::new(upgraded)` 把它变回 tokio 的 `AsyncRead + AsyncWrite`。`hyper::upgrade::on` 必须在返回响应之前从请求上取得。

- [ ] **Step 2: 测试（追加到 `http.rs`）**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeDialer, echo_server};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    pub(crate) async fn listener(opts: ListenerOpts) -> (Running, Arc<FakeDialer>) {
        let echo = echo_server().await;
        let dialer = FakeDialer::new(echo, None);
        let running = HttpListener::bind("127.0.0.1:0".parse().unwrap(), dialer.clone(), opts).await.unwrap();
        (running, dialer)
    }

    /// Sends raw bytes and reads until the connection closes or `until` matches.
    pub(crate) async fn raw(addr: SocketAddr, request: &str) -> (TcpStream, String) {
        let mut s = TcpStream::connect(addr).await.unwrap();
        s.write_all(request.as_bytes()).await.unwrap();
        let mut buf = vec![0u8; 8192];
        let mut out = String::new();
        loop {
            match tokio::time::timeout(Duration::from_millis(500), s.read(&mut buf)).await {
                Ok(Ok(0)) | Err(_) => break,
                Ok(Ok(n)) => {
                    out.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if out.contains("\r\n\r\n") {
                        break;
                    }
                }
                Ok(Err(_)) => break,
            }
        }
        (s, out)
    }

    #[tokio::test]
    async fn connect_tunnels_to_the_dialed_stream() {
        let (running, dialer) = listener(ListenerOpts::default()).await;
        let (mut s, head) = raw(running.local_addr, "CONNECT echo.test:443 HTTP/1.1\r\nHost: echo.test:443\r\n\r\n").await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        s.write_all(b"tunnelled").await.unwrap();
        let mut buf = [0u8; 9];
        s.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"tunnelled");
        drop(s);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let sessions = dialer.sessions();
        assert_eq!(sessions.len(), 1);
        let info = sessions[0].session();
        assert_eq!((info.dst_host.clone(), info.dst_port, info.listener), (HostName::parse("echo.test"), 443, ListenerKind::Http));
        assert_eq!(info.in_port, running.local_addr.port());
        assert!(sessions[0].is_finished());
    }

    #[tokio::test]
    async fn rejected_connect_closes_without_a_response() {
        let (running, _dialer) = listener(ListenerOpts {
            drop_hold: Duration::from_millis(200),
            ..ListenerOpts::default()
        })
        .await;
        for port in [443, 445, 446] {
            let (_s, head) = raw(running.local_addr, &format!("CONNECT reject.test:{port} HTTP/1.1\r\n\r\n")).await;
            assert!(head.is_empty(), "port {port}: {head}");
        }
        let (_s, head) = raw(running.local_addr, "CONNECT fail.test:443 HTTP/1.1\r\n\r\n").await;
        assert!(head.is_empty(), "{head}");
        // DROP holds the connection for drop_hold before closing
        let started = std::time::Instant::now();
        let mut s = TcpStream::connect(running.local_addr).await.unwrap();
        s.write_all(b"CONNECT reject.test:444 HTTP/1.1\r\n\r\n").await.unwrap();
        let mut buf = [0u8; 16];
        let n = tokio::time::timeout(Duration::from_secs(2), s.read(&mut buf)).await.unwrap().unwrap();
        assert_eq!(n, 0);
        assert!(started.elapsed() >= Duration::from_millis(180));
    }

    #[tokio::test]
    async fn basic_auth_compares_the_password_only() {
        let (running, _dialer) = listener(ListenerOpts {
            auth: Some(HttpAuth::Password("s3cret".to_string())),
            ..ListenerOpts::default()
        })
        .await;
        let (_s, head) = raw(running.local_addr, "CONNECT echo.test:443 HTTP/1.1\r\n\r\n").await;
        assert!(head.starts_with("HTTP/1.1 407"), "{head}");
        assert!(head.contains("Proxy-Authenticate: Basic realm=\"rurge\"") || head.contains("proxy-authenticate: Basic realm=\"rurge\""), "{head}");
        let wrong = BASE64.encode("alice:nope");
        let (_s, head) = raw(running.local_addr, &format!("CONNECT echo.test:443 HTTP/1.1\r\nProxy-Authorization: Basic {wrong}\r\n\r\n")).await;
        assert!(head.starts_with("HTTP/1.1 407"), "{head}");
        let right = BASE64.encode("anyone:s3cret");
        let (_s, head) = raw(running.local_addr, &format!("CONNECT echo.test:443 HTTP/1.1\r\nProxy-Authorization: Basic {right}\r\n\r\n")).await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        let auth = HttpAuth::UserPass { user: "u".into(), password: "p".into() };
        assert!(authorized(&auth, Some(&HeaderValue::from_str(&format!("Basic {}", BASE64.encode("u:p"))).unwrap())));
        assert!(!authorized(&auth, Some(&HeaderValue::from_str(&format!("Basic {}", BASE64.encode("x:p"))).unwrap())));
        assert!(!authorized(&auth, None));
    }

    #[tokio::test]
    async fn non_proxy_requests_get_400() {
        let (running, _dialer) = listener(ListenerOpts::default()).await;
        let (_s, head) = raw(running.local_addr, "GET /index.html HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
        assert!(head.starts_with("HTTP/1.1 400"), "{head}");
    }
}
```

> hyper 以 `preserve_header_case(true)` 保留头名大小写，但响应头由 hyper 序列化为小写，所以 407 用例同时接受两种写法。`raw` 只读到响应头结束（`\r\n\r\n`）；CONNECT 200 的响应没有正文。

- [ ] **Step 3: 运行与提交**

```bash
cargo test -p rurge-inbound http
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-inbound
git commit -F - <<'EOF'
feat(inbound): HTTP 代理监听：CONNECT 隧道、Basic 认证、非法请求 400

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

预期：4 个新测试通过。

---

### Task 7: `rurge-inbound` HTTP 明文转发与 REJECT 响应

**Files:**
- Modify: `crates/rurge-inbound/src/http.rs`

**Interfaces:**
- Consumes: Task 6 的 `http.rs`、`Counting`、`responses::{error_page, tiny_gif, ErrorPage}`、hyper 1 `hyper::client::conn::http1::handshake`。
- Produces：`forward` 的完整实现（替换 Task 6 的 501 占位）：绝对 URI 请求 → `SessionInfo { protocol: Some(Http), url, http_host, user_agent, dst 来自 URI（端口默认 80）}` → 每请求 dial → 出站流套 `Counting` → `hyper::client::conn::http1::handshake` → 请求改写（URI 改 origin-form、删除 `Proxy-Authorization` / `Proxy-Connection`、缺 `Host` 时补上）→ `send_request` → 响应体原样回写（`Incoming` → `BoxBody`）→ 连接任务结束时 `handle.finish(Completed)`；dial 结果映射：`Reject { TinyGif }` → 200 GIF；`Reject { Reject | NoDrop }` → `show_error_page_for_reject` 时 403 错误页否则关闭；`Reject { Drop }` → 等 `drop_hold` 后关闭；`Failed` → `show_error_page` 时 502 错误页否则关闭；出站握手 / 发送失败 → 同 `Failed`（502 或关闭）并 `handle.finish(Failed)`。
- 内部：`pub(crate) fn origin_form(req: &mut Request<Incoming>) -> Result<(), http::Error>`（改写 URI，补 `Host`，删代理头）。

- [ ] **Step 1: 替换 `forward` 并补辅助函数**

在 `http.rs` 中删除 Task 6 的 `forward` 占位，追加：

```rust
use crate::session::{Counting, FailKind, SessionHandle};
use http::Uri;
use http_body_util::BodyExt;
use rurge_config::rule::ProtocolKind;

/// Rewrites a proxy request into the form the origin expects: origin-form
/// URI, a `Host` header, and no proxy-only headers.
pub(crate) fn origin_form(req: &mut Request<Incoming>) -> Result<(), http::Error> {
    let authority = req.uri().authority().map(|a| a.to_string());
    let path = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());
    *req.uri_mut() = path.parse::<Uri>()?;
    if !req.headers().contains_key(header::HOST)
        && let Some(a) = authority
        && let Ok(v) = HeaderValue::from_str(&a)
    {
        req.headers_mut().insert(header::HOST, v);
    }
    req.headers_mut().remove(header::PROXY_AUTHORIZATION);
    req.headers_mut().remove("proxy-connection");
    Ok(())
}

fn failure_response(ctx: &Ctx, handle: &SessionHandle, message: &str) -> Result<Response<ResponseBody>, HandlerError> {
    if !ctx.opts.show_error_page {
        return Err(HandlerError::Close);
    }
    let s = handle.session();
    Ok(responses::error_page(
        StatusCode::BAD_GATEWAY,
        &responses::ErrorPage {
            title: "Connection failed",
            session_id: handle.id(),
            dst: format!("{}:{}", s.dst_host, s.dst_port),
            rule: handle.rule(),
            chain: handle.policy_chain(),
            message: message.to_string(),
        },
    ))
}

async fn forward(mut req: Request<Incoming>, ctx: Arc<Ctx>) -> Result<Response<ResponseBody>, HandlerError> {
    let Some(authority) = req.uri().authority().cloned() else {
        return Ok(responses::bad_request("absolute URI without a host"));
    };
    let host = HostName::parse(authority.host());
    let port = authority.port_u16().unwrap_or(80);
    let mut session = session_for(&ctx, host, port);
    session.protocol = Some(ProtocolKind::Http);
    session.url = Some(req.uri().to_string());
    session.http_host = Some(authority.host().to_ascii_lowercase());
    session.user_agent = req
        .headers()
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let dialed = match ctx.dialer.dial(session).await {
        Ok(d) => d,
        Err(DialError::Reject { kind: RejectKind::TinyGif, .. }) => return Ok(responses::tiny_gif()),
        Err(DialError::Reject { kind: RejectKind::Drop, .. }) => {
            tokio::time::sleep(ctx.opts.drop_hold).await;
            return Err(HandlerError::Close);
        }
        Err(DialError::Reject { kind, rule, handle }) => {
            if !ctx.opts.show_error_page_for_reject {
                return Err(HandlerError::Close);
            }
            let s = handle.session();
            return Ok(responses::error_page(
                StatusCode::FORBIDDEN,
                &responses::ErrorPage {
                    title: "Request rejected",
                    session_id: handle.id(),
                    dst: format!("{}:{}", s.dst_host, s.dst_port),
                    rule,
                    chain: handle.policy_chain(),
                    message: format!("The request was rejected by the {} policy.", kind.name()),
                },
            ));
        }
        Err(DialError::Failed { kind, message, handle, .. }) => {
            let what = match kind {
                FailKind::Dns => "DNS lookup failed",
                FailKind::Timeout => "Connection timed out",
                FailKind::Connect | FailKind::Other => "Connection failed",
            };
            return failure_response(&ctx, &handle, &format!("{what}: {message}"));
        }
    };

    let handle = dialed.handle.clone();
    let io = TokioIo::new(Counting::new(dialed.stream, handle.clone()));
    let (mut sender, conn) = match hyper::client::conn::http1::handshake(io).await {
        Ok(pair) => pair,
        Err(e) => {
            handle.finish(SessionOutcome::Failed(format!("upstream handshake failed: {e}")));
            return failure_response(&ctx, &handle, &format!("Upstream handshake failed: {e}"));
        }
    };
    let conn_handle = handle.clone();
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            conn_handle.finish(SessionOutcome::Failed(format!("upstream connection error: {e}")));
        } else {
            conn_handle.finish(SessionOutcome::Completed);
        }
    });
    if let Err(e) = origin_form(&mut req) {
        handle.finish(SessionOutcome::Failed(format!("bad request uri: {e}")));
        return Ok(responses::bad_request("malformed request URI"));
    }
    match sender.send_request(req).await {
        Ok(resp) => Ok(resp.map(|body| body.boxed())),
        Err(e) => {
            handle.finish(SessionOutcome::Failed(format!("upstream request failed: {e}")));
            failure_response(&ctx, &handle, &format!("Upstream request failed: {e}"))
        }
    }
}
```

> `sender` 在 `send_request` 之后随函数返回而丢弃，hyper 在响应体读完后关闭这条出站连接，`conn` 任务随之结束并 `finish(Completed)`——这就是「每请求一条出站连接」（设计 D7）。`Incoming` 的错误类型是 `hyper::Error`，`.boxed()` 后与 `ResponseBody` 一致。

- [ ] **Step 2: 测试（追加到 `http.rs` 的 `tests` 模块）**

```rust
    use rurge_net::testing::TestServer;

    async fn listener_with_target(opts: ListenerOpts) -> (Running, Arc<FakeDialer>, TestServer) {
        let target = TestServer::spawn().await;
        let target_addr: SocketAddr = format!("127.0.0.1:{}", target.url("/").port().unwrap()).parse().unwrap();
        let echo = echo_server().await;
        let dialer = FakeDialer::new(echo, Some(target_addr));
        let running = HttpListener::bind("127.0.0.1:0".parse().unwrap(), dialer.clone(), opts).await.unwrap();
        (running, dialer, target)
    }

    /// Reads one full HTTP/1.1 response (headers + Content-Length body) from `s`.
    async fn read_response(s: &mut TcpStream) -> (String, Vec<u8>) {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let n = tokio::time::timeout(Duration::from_secs(2), s.read(&mut chunk)).await.unwrap().unwrap();
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..pos]).to_string();
                let len = head
                    .lines()
                    .find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("content-length")).map(|(_, v)| v.trim().parse::<usize>().unwrap()))
                    .unwrap_or(0);
                while buf.len() < pos + 4 + len {
                    let n = s.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                return (head, buf[pos + 4..].to_vec());
            }
        }
        (String::from_utf8_lossy(&buf).to_string(), Vec::new())
    }

    #[tokio::test]
    async fn plain_requests_are_forwarded_per_request_with_rewritten_headers() {
        let (running, dialer, target) = listener_with_target(ListenerOpts::default()).await;
        target.set("/hello", "hi there");
        let port = target.url("/").port().unwrap();
        let mut s = TcpStream::connect(running.local_addr).await.unwrap();
        s.write_all(
            format!("GET http://target.test:{port}/hello HTTP/1.1\r\nHost: target.test:{port}\r\nUser-Agent: t/1\r\nProxy-Authorization: Basic eDp5\r\nProxy-Connection: keep-alive\r\n\r\n").as_bytes(),
        )
        .await
        .unwrap();
        let (head, body) = read_response(&mut s).await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert_eq!(body, b"hi there");
        let reqs = target.requests();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].path, "/hello");
        assert!(reqs[0].header("proxy-authorization").is_none() && reqs[0].header("proxy-connection").is_none());
        assert_eq!(reqs[0].header("user-agent"), Some("t/1"));
        // second request on the same client connection hits a different rule (tinygif)
        s.write_all(b"GET http://reject.test:446/ad.gif HTTP/1.1\r\nHost: reject.test\r\n\r\n").await.unwrap();
        let (head, body) = read_response(&mut s).await;
        assert!(head.starts_with("HTTP/1.1 200") && head.to_ascii_lowercase().contains("content-type: image/gif"), "{head}");
        assert_eq!(body.len(), 43);
        drop(s);
        tokio::time::sleep(Duration::from_millis(150)).await;
        let sessions = dialer.sessions();
        assert_eq!(sessions.len(), 2);
        let first = sessions[0].session();
        assert_eq!((first.protocol, first.dst_port), (Some(ProtocolKind::Http), port));
        assert_eq!(first.url.as_deref(), Some(format!("http://target.test:{port}/hello").as_str()));
        assert_eq!(first.http_host.as_deref(), Some("target.test"));
        assert_eq!(first.user_agent.as_deref(), Some("t/1"));
        assert!(sessions[0].is_finished(), "completed when the upstream connection closed");
        let (up, down) = sessions[0].bytes();
        assert!(up > 0 && down > 0, "{up} {down}");
        assert_eq!(sessions[1].outcome(), Some(SessionOutcome::Rejected(RejectKind::TinyGif)));
    }

    #[tokio::test]
    async fn rejects_and_failures_render_pages_or_close() {
        // defaults: reject closes, failures show a 502 page
        let (running, _d, _t) = listener_with_target(ListenerOpts {
            drop_hold: Duration::from_millis(200),
            ..ListenerOpts::default()
        })
        .await;
        let (_s, head) = raw(running.local_addr, "GET http://reject.test/ HTTP/1.1\r\nHost: reject.test\r\n\r\n").await;
        assert!(head.is_empty(), "reject must close: {head}");
        let mut s = TcpStream::connect(running.local_addr).await.unwrap();
        s.write_all(b"GET http://dns.test/ HTTP/1.1\r\nHost: dns.test\r\n\r\n").await.unwrap();
        let (head, body) = read_response(&mut s).await;
        assert!(head.starts_with("HTTP/1.1 502"), "{head}");
        let html = String::from_utf8_lossy(&body);
        assert!(html.contains("DNS lookup failed") && html.contains("dns.test:80"), "{html}");
        let started = std::time::Instant::now();
        let (_s, head) = raw(running.local_addr, "GET http://reject.test:444/ HTTP/1.1\r\nHost: reject.test\r\n\r\n").await;
        assert!(head.is_empty() && started.elapsed() >= Duration::from_millis(180), "{head}");
        // error page for rejects when enabled; no page for failures when disabled
        let (running, _d, _t) = listener_with_target(ListenerOpts {
            show_error_page: false,
            show_error_page_for_reject: true,
            ..ListenerOpts::default()
        })
        .await;
        let mut s = TcpStream::connect(running.local_addr).await.unwrap();
        s.write_all(b"GET http://reject.test:445/x HTTP/1.1\r\nHost: reject.test\r\n\r\n").await.unwrap();
        let (head, body) = read_response(&mut s).await;
        assert!(head.starts_with("HTTP/1.1 403"), "{head}");
        let html = String::from_utf8_lossy(&body);
        assert!(html.contains("REJECT-NO-DROP") && html.contains("FAKE,rule"), "{html}");
        let (_s, head) = raw(running.local_addr, "GET http://fail.test/ HTTP/1.1\r\nHost: fail.test\r\n\r\n").await;
        assert!(head.is_empty(), "failure must close when pages are off: {head}");
    }
```

> `TestServer::url(path)` 返回 `Url`，`.port()` 给出端口；`RecordedRequest { path, .. }` 与 `header(name)` 由 M2a 的测试服务器提供（`crates/rurge-net/src/testing.rs`），若字段名不同按源码调整断言。

- [ ] **Step 3: 运行与提交**

```bash
cargo test -p rurge-inbound http
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-inbound
git commit -F - <<'EOF'
feat(inbound): HTTP 明文请求逐请求转发；REJECT 错误页 / 1px GIF / DROP；连接失败错误页

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

预期：`http` 模块 6 个测试通过。

---

### Task 8: `rurge-engine` 资源栈迁入与 `state.json`

**Files:**
- Modify: `crates/rurge-engine/Cargo.toml`（追加 `anyhow.workspace = true`）、`crates/rurge-engine/src/lib.rs`
- Create: `crates/rurge-engine/src/stack.rs`、`crates/rurge-engine/src/state.rs`
- Modify: `crates/rurge/Cargo.toml`（追加 `rurge-engine.workspace = true`）、`crates/rurge/src/cli/runtime.rs`（`Stack` 构建委托给 `rurge-engine`）

**Interfaces:**
- Consumes: bin 现有的 `Stack` / `build_stack_with` / `settle` 代码（`crates/rurge/src/cli/runtime.rs` 第 79 ～ 180 行，原样迁入）、`rurge_dns::system::SystemDns`、`rurge_policy::GroupSelections`、serde / serde_json。
- Produces：
  - `rurge_engine::stack::{Stack, StackOptions, build_stack, build_stack_with}`：`StackOptions { data_dir: PathBuf, no_network: bool, geo_urls: GeoUrls, dns_cache_size: usize, system: Arc<dyn SystemDns>, wait: Duration }`；`Stack { resources, registry, geo, geo_updater, resolver, diagnostics }` 字段与 bin 现有版本相同；`build_stack(cfg: &Config, opts: &StackOptions) -> anyhow::Result<Stack>`；`build_stack_with(cfg, opts, customize: impl FnOnce(&mut ResolverConfig)) -> anyhow::Result<Stack>`。
  - `rurge_engine::state::{State, Features, STATE_FILE = "state.json", STATE_VERSION = 1, profile_key}`：`State { version, outbound_mode: Option<String>, global_policy: Option<String>, features: Features, group_selections: HashMap<String, HashMap<String, String>>, system_proxy_backup: Option<serde_json::Value>, current_profile: Option<String> }`（serde `default`，`Default` 的 `version = 1`）；`State::load(path: &Path) -> State`（缺文件 → 默认；读取 / 解析失败 → 默认 + `tracing::warn!`）；`State::selections_for(&self, profile: &str) -> GroupSelections`；`profile_key(main: &Path) -> String`（主配置文件名）。
  - bin：`cli/runtime.rs` 保留 `RuntimeArgs` / `Runtime` / `PlatformSystemDns`，新增 `Runtime::stack_options(&self, wait: Duration) -> StackOptions`，`build_stack` / `build_stack_with` 变为委托；`pub use rurge_engine::stack::Stack;` 让 `rule.rs` / `dns.rs` 的 `super::runtime::Stack` 路径不变。

- [ ] **Step 1: 写 `stack.rs`（迁移）**

把 bin `runtime.rs` 里的 `Stack`、`build_stack`、`build_stack_with`、`settle` 移到 `crates/rurge-engine/src/stack.rs`，做以下替换后其余原样：

```rust
//! One config generation's shared objects (M2 design §5, M3 design §7.1):
//! resource manager → set registry → GeoIP → resolver. `rurge run`,
//! `rule match` and `dns lookup` all build it here.

use rurge_config::{Config, Diagnostics};
use rurge_dns::system::SystemDns;
use rurge_dns::{Resolver, ResolverConfig, ResolverDeps};
use rurge_net::connector::{DirectConnector, SystemResolve};
use rurge_net::http::{HttpClient, HttpClientConfig};
use rurge_net::resource::{ResourceManager, ResourceOptions};
use rurge_rules::{GeoDb, GeoUpdater, GeoUrls, SetRegistry};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub struct StackOptions {
    pub data_dir: PathBuf,
    pub no_network: bool,
    pub geo_urls: GeoUrls,
    pub dns_cache_size: usize,
    /// Platform DNS facts (the bin injects `rurge-platform`; tests use `StaticSystemDns`).
    pub system: Arc<dyn SystemDns>,
    /// Wait this long for the first fetch of every external resource (zero = don't wait).
    pub wait: Duration,
}

pub struct Stack {
    /// Kept alive so the manager's background refresh tasks keep running;
    /// callers reach individual resources through `registry` and `geo`.
    pub resources: Arc<ResourceManager>,
    pub registry: Arc<SetRegistry>,
    pub geo: Arc<GeoDb>,
    /// Kept alive so its install tasks are not aborted by `Drop`.
    pub geo_updater: Option<GeoUpdater>,
    pub resolver: Arc<Resolver>,
    pub diagnostics: Diagnostics,
}

pub async fn build_stack(cfg: &Config, opts: &StackOptions) -> anyhow::Result<Stack> {
    build_stack_with(cfg, opts, |_| {}).await
}

pub async fn build_stack_with(
    cfg: &Config,
    opts: &StackOptions,
    customize: impl FnOnce(&mut ResolverConfig),
) -> anyhow::Result<Stack> {
    let connector = Arc::new(DirectConnector::new(Arc::new(SystemResolve)));
    let client = Arc::new(HttpClient::new(connector.clone(), HttpClientConfig::default())?);
    let resources = ResourceManager::with_options(
        opts.data_dir.clone(),
        client,
        ResourceOptions {
            offline: opts.no_network,
            ..ResourceOptions::default()
        },
    );
    let base_dir = cfg
        .source
        .main
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let (registry, mut diagnostics) = SetRegistry::build(cfg, resources.clone(), &base_dir);
    let (geo, geo_diags) = GeoDb::open(&opts.data_dir.join("geoip"));
    for d in geo_diags {
        diagnostics.push(d);
    }
    let geo_updater = (!opts.no_network).then(|| {
        GeoUpdater::spawn(
            geo.clone(),
            resources.clone(),
            opts.geo_urls.clone(),
            !cfg.general.disable_geoip_db_auto_update,
        )
    });
    let mut resolver_cfg = ResolverConfig::from_config(cfg);
    resolver_cfg.cache_capacity = opts.dns_cache_size;
    customize(&mut resolver_cfg);
    let (resolver, dns_diags) = Resolver::new(
        resolver_cfg,
        ResolverDeps {
            connector,
            sets: registry.clone(),
            system: opts.system.clone(),
            resources: resources.clone(),
        },
    );
    diagnostics.extend(dns_diags);
    if !opts.no_network && !opts.wait.is_zero() {
        resources.wait_initial(opts.wait).await;
        settle(&registry, &geo, &resources).await;
    }
    Ok(Stack {
        resources,
        registry,
        geo,
        geo_updater,
        resolver,
        diagnostics,
    })
}

/// Gives the background reload / install tasks up to two seconds to apply
/// resources that just arrived.
async fn settle(registry: &SetRegistry, geo: &GeoDb, resources: &ResourceManager) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let sets_pending = registry
            .statuses()
            .iter()
            .any(|s| s.state == "missing" || s.state == "compiling");
        let geo_pending = {
            let info = geo.info();
            let available = |name: &str| {
                resources
                    .statuses()
                    .iter()
                    .any(|s| s.state == "available" && s.source.to_string().ends_with(name))
            };
            (info.country_epoch.is_none() && available("GeoLite2-Country.mmdb"))
                || (info.asn_epoch.is_none() && available("GeoLite2-ASN.mmdb"))
        };
        if (!sets_pending && !geo_pending) || tokio::time::Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
```

> 这就是 bin 现有的 `build_stack_with` / `settle`（`crates/rurge/src/cli/runtime.rs`），只把 `rt.*` 换成 `opts.*`、`Arc::new(PlatformSystemDns)` 换成 `opts.system.clone()`；两处 `#[allow(dead_code)]` 删除（引擎会读这些字段）。完成后 bin 的 `runtime.rs` 不再包含 `Stack` / `settle` 的定义。

- [ ] **Step 2: 写 `state.rs`**

```rust
//! `state.json` (M3 design §9.1): runtime state that outlives the config.
//! M3 only reads it; the M4 API writes it (temp file + rename).

use rurge_policy::GroupSelections;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

pub const STATE_FILE: &str = "state.json";
pub const STATE_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct Features {
    pub system_proxy: bool,
    pub enhanced_mode: bool,
    pub mitm: bool,
    pub capture: bool,
    pub rewrite: bool,
    pub scripting: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct State {
    pub version: u32,
    pub outbound_mode: Option<String>,
    pub global_policy: Option<String>,
    pub features: Features,
    /// profile file name → group name → selected member
    pub group_selections: HashMap<String, HashMap<String, String>>,
    pub system_proxy_backup: Option<serde_json::Value>,
    pub current_profile: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        State {
            version: STATE_VERSION,
            outbound_mode: None,
            global_policy: None,
            features: Features::default(),
            group_selections: HashMap::new(),
            system_proxy_backup: None,
            current_profile: None,
        }
    }
}

impl State {
    /// A missing file is the default state; a broken one is reported and ignored.
    pub fn load(path: &Path) -> State {
        match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<State>(&text) {
                Ok(state) => state,
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "state.json is malformed; using defaults");
                    State::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "cannot read state.json; using defaults");
                State::default()
            }
        }
    }

    pub fn selections_for(&self, profile: &str) -> GroupSelections {
        self.group_selections
            .get(profile)
            .map(|m| GroupSelections::from_map(m.clone()))
            .unwrap_or_default()
    }
}

/// Profiles are keyed by their file name (`surge.conf`), not their full path.
pub fn profile_key(main: &Path) -> String {
    main.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| main.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_defaults_selections_and_tolerates_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let missing = State::load(&dir.path().join("none.json"));
        assert_eq!(missing, State::default());
        assert_eq!(missing.version, 1);
        let path = dir.path().join(STATE_FILE);
        std::fs::write(
            &path,
            r#"{"version":1,"outbound_mode":"direct","group_selections":{"surge.conf":{"Pick":"HK"}},"features":{"mitm":true}}"#,
        )
        .unwrap();
        let state = State::load(&path);
        assert_eq!(state.outbound_mode.as_deref(), Some("direct"));
        assert!(state.features.mitm && !state.features.capture);
        assert_eq!(state.selections_for("surge.conf").get("Pick"), Some("HK"));
        assert!(state.selections_for("other.conf").is_empty());
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(State::load(&path), State::default());
        assert_eq!(profile_key(Path::new("/etc/rurge/surge.conf")), "surge.conf");
        assert_eq!(profile_key(Path::new("C:\\p\\my.conf")), "my.conf");
        let round = serde_json::to_string(&state).unwrap();
        assert_eq!(serde_json::from_str::<State>(&round).unwrap(), state);
    }
}
```

`crates/rurge-engine/src/lib.rs`：追加 `pub mod stack;` 与 `pub mod state;`。

- [ ] **Step 3: bin 委托**

`crates/rurge/Cargo.toml` `[dependencies]` 追加 `rurge-engine.workspace = true`。`crates/rurge/src/cli/runtime.rs`：删除 `Stack` / `build_stack` / `build_stack_with` / `settle` 及其专用导入，改为：

```rust
pub use rurge_engine::stack::Stack;
use rurge_engine::stack::StackOptions;

impl Runtime {
    /// Options for `rurge_engine::stack::build_stack` with the platform DNS adapter injected.
    pub fn stack_options(&self, wait: Duration) -> StackOptions {
        StackOptions {
            data_dir: self.data_dir.clone(),
            no_network: self.no_network,
            geo_urls: self.geo_urls.clone(),
            dns_cache_size: self.dns_cache_size,
            system: Arc::new(PlatformSystemDns),
            wait,
        }
    }
}

pub async fn build_stack(cfg: &Config, rt: &Runtime, wait: Duration) -> anyhow::Result<Stack> {
    rurge_engine::stack::build_stack(cfg, &rt.stack_options(wait)).await
}

pub async fn build_stack_with(
    cfg: &Config,
    rt: &Runtime,
    wait: Duration,
    customize: impl FnOnce(&mut rurge_dns::ResolverConfig),
) -> anyhow::Result<Stack> {
    rurge_engine::stack::build_stack_with(cfg, &rt.stack_options(wait), customize).await
}
```

`PlatformSystemDns` 保留在 bin。`rule.rs` / `dns.rs` 无需改动（路径 `super::runtime::{Stack, build_stack, build_stack_with}` 不变）。

- [ ] **Step 4: 运行与提交**

```bash
cargo test -p rurge-engine
cargo test -p rurge            # rule match / dns lookup 的 CLI 测试必须原样通过
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add Cargo.lock crates/rurge-engine crates/rurge
git commit -F - <<'EOF'
refactor(engine): 资源栈构建迁入 rurge-engine；state.json 只读解析

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

预期：`state` 1 个测试通过；bin 的既有 CLI 测试全部通过。

---

### Task 9: `rurge-engine`：`Runtime`、`Engine`（`Dialer` 实现、relay、监听绑定、会话日志）与端到端集成测试

**Files:**
- Modify: `crates/rurge-engine/src/lib.rs`
- Create: `crates/rurge-engine/src/runtime.rs`、`crates/rurge-engine/src/engine.rs`、`crates/rurge-engine/tests/pipeline.rs`

**Interfaces:**
- Consumes: Task 8 的 `stack` / `state`、`rurge_rules::{RuleEngine, OutboundMode, Outcome}`（`RuleEngine::build_with_registry(&Config, Arc<SetRegistry>, Arc<dyn GeoLookup>) -> Result<RuleEngine, BuildError>`；`evaluate(&SessionInfo, OutboundMode, &dyn LazyResolver) -> Decision { outcome, matched: Option<usize>, .. }`；`rules() -> &[CompiledRule { index, raw, .. }]`）、`rurge_policy::{PolicyRegistry, GroupSelections}`、`rurge_proto::{Direct, OutboundError, OutboundRef}`、`rurge_inbound::*`（Task 4 ～ 7）、`rurge_config::{Builtin, rule::PolicyRef, general::General}`、`rurge_net::connector::{ConnectOpts, Target}`、`tokio::io::copy_bidirectional`。
- Produces（crate 根再导出 `Engine, ListenerSpec, Runtime, RuntimeOptions`）：
  - `RuntimeOptions { stack: StackOptions, outbound_mode: OutboundMode, selections: GroupSelections }`；`Runtime { config: Arc<Config>, stack: Stack, rules: RuleEngine, policies: PolicyRegistry, outbound_mode: OutboundMode }`；`Runtime::build(config: Config, opts: RuntimeOptions) -> anyhow::Result<Runtime>`（async；须在 tokio 运行时内）；`Runtime::diagnostics(&self) -> &Diagnostics`（资源栈 + 解析器的诊断）。
  - `Engine::new(runtime: Runtime) -> Arc<Engine>`；`runtime(&self) -> Arc<Runtime>`；`Engine::listener_specs(general: &General) -> Vec<ListenerSpec>`（`http-listen` → `Http` + `HttpAuth::Password`，`socks5-listen` → `Socks5`；两者皆空时：`allow-wifi-access` → `0.0.0.0:wifi-access-http-port`（`wifi-access-http-auth` → `UserPass`）与 `0.0.0.0:wifi-access-socks5-port`，否则 `127.0.0.1:6152` / `6153`）；`ListenerSpec { kind: ListenerKind, addr: SocketAddr, auth: Option<HttpAuth> }`；`bind_listeners(self: &Arc<Self>) -> io::Result<Vec<Running>>`（与 `listener_specs` 同序；`ListenerOpts` 来自 `[General]`：`restrict_to_lan = proxy_restricted_to_lan`、`show_error_page`、`show_error_page_for_reject`、`drop_hold = 30 s`）。
  - `impl Dialer for Engine`：`dial` = 取 `Runtime` 快照 → 新 `SessionHandle`（递增 id，安装会话日志钩子）→ 出站模式（`Direct` → `PolicyRef::Builtin(Direct)`；`Proxy(p)` → `p`；`Rule` → `rules.evaluate`，`matched` 写入 `handle.set_rule`，`DnsFailed` → `Failed { Dns }`）→ `policies.resolve` → `handle.set_policy_chain` → `connect_tcp(Target(dst), ConnectOpts { timeout: 10 s, prefer_v6: general.ipv6 })` → 成功 `Dialed`；`Reject(kind)` → `finish(Rejected(kind))` + `DialError::Reject`；`Unsupported(t)` → 同 `Reject { Reject }`（链里已有 `!unsupported:<t>`）；`Dns` → `Failed { Dns }`；`Io` → `Failed { Connect }`；`Timeout` → `Failed { Timeout }`。`relay` = `copy_bidirectional` 后 `add_up` / `add_down` / `finish(Completed)`，出错 `finish(Failed)`。
  - 会话日志（`on_finish` 钩子）：`Completed` → `tracing::debug!`，`Rejected` / `Failed` → `tracing::info!`；字段 `session`、`listener`、`src`、`dst`、`rule`、`policy`（链用 ` > ` 连接）、`up`、`down`、`elapsed_ms`、`error`。
  - 常量：`CONNECT_TIMEOUT = 10 s`、`DROP_HOLD = 30 s`、`DEFAULT_HTTP_PORT = 6152`、`DEFAULT_SOCKS5_PORT = 6153`。

- [ ] **Step 1: 写 `runtime.rs`**

```rust
//! One immutable config generation (M3 design §7.1).

use crate::stack::{Stack, StackOptions, build_stack};
use rurge_config::{Config, Diagnostics};
use rurge_policy::{GroupSelections, PolicyRegistry};
use rurge_proto::{Direct, OutboundRef};
use rurge_rules::{OutboundMode, RuleEngine};
use std::sync::Arc;

pub struct RuntimeOptions {
    pub stack: StackOptions,
    pub outbound_mode: OutboundMode,
    pub selections: GroupSelections,
}

pub struct Runtime {
    pub config: Arc<Config>,
    pub stack: Stack,
    pub rules: RuleEngine,
    pub policies: PolicyRegistry,
    pub outbound_mode: OutboundMode,
}

impl Runtime {
    /// Resources → sets → GeoIP → resolver → rule engine → policy registry.
    /// Must run inside a tokio runtime (background tasks are spawned).
    pub async fn build(config: Config, opts: RuntimeOptions) -> anyhow::Result<Runtime> {
        let stack = build_stack(&config, &opts.stack).await?;
        let rules = RuleEngine::build_with_registry(&config, stack.registry.clone(), stack.geo.clone())?;
        let direct: OutboundRef = Arc::new(Direct::with_resolver(stack.resolver.clone()));
        let policies = PolicyRegistry::build(&config, &opts.selections, direct);
        Ok(Runtime {
            config: Arc::new(config),
            stack,
            rules,
            policies,
            outbound_mode: opts.outbound_mode,
        })
    }

    /// Diagnostics produced while building the stack (sets, GeoIP, resolver).
    pub fn diagnostics(&self) -> &Diagnostics {
        &self.stack.diagnostics
    }
}
```

- [ ] **Step 2: 写 `engine.rs`**

```rust
//! The engine (M3 design §7.2 – 7.3): implements `Dialer` for the listeners,
//! relays bytes, binds listeners from `[General]`, writes the session log.

use crate::runtime::Runtime;
use arc_swap::ArcSwap;
use rurge_config::Builtin;
use rurge_config::general::General;
use rurge_config::rule::PolicyRef;
use rurge_config::session::{ListenerKind, SessionInfo};
use rurge_inbound::{
    DialError, Dialed, Dialer, FailKind, HttpAuth, HttpListener, ListenerOpts, Running, SessionHandle, SessionOutcome,
    Socks5Listener,
};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Target};
use rurge_proto::OutboundError;
use rurge_rules::{Outcome, OutboundMode};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const DROP_HOLD: Duration = Duration::from_secs(30);
pub const DEFAULT_HTTP_PORT: u16 = 6152;
pub const DEFAULT_SOCKS5_PORT: u16 = 6153;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListenerSpec {
    pub kind: ListenerKind,
    pub addr: SocketAddr,
    pub auth: Option<HttpAuth>,
}

pub struct Engine {
    runtime: ArcSwap<Runtime>,
    next_session: AtomicU64,
}

impl Engine {
    pub fn new(runtime: Runtime) -> Arc<Engine> {
        Arc::new(Engine {
            runtime: ArcSwap::from_pointee(runtime),
            next_session: AtomicU64::new(0),
        })
    }

    /// The current config generation (sessions snapshot it once at dial time).
    pub fn runtime(&self) -> Arc<Runtime> {
        self.runtime.load_full()
    }

    /// Listeners `[General]` asks for (M3 design §1.2 / matrix `allow-wifi-access`).
    pub fn listener_specs(general: &General) -> Vec<ListenerSpec> {
        let mut specs = Vec::new();
        for l in &general.http_listen {
            specs.push(ListenerSpec {
                kind: ListenerKind::Http,
                addr: l.addr,
                auth: l.password.clone().map(HttpAuth::Password),
            });
        }
        for l in &general.socks5_listen {
            specs.push(ListenerSpec {
                kind: ListenerKind::Socks5,
                addr: l.addr,
                auth: None,
            });
        }
        if specs.is_empty() {
            let (ip, http_port, socks_port, auth) = if general.allow_wifi_access {
                (
                    IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                    general.wifi_access_http_port,
                    general.wifi_access_socks5_port,
                    general
                        .wifi_access_http_auth
                        .clone()
                        .map(|(user, password)| HttpAuth::UserPass { user, password }),
                )
            } else {
                (IpAddr::V4(Ipv4Addr::LOCALHOST), DEFAULT_HTTP_PORT, DEFAULT_SOCKS5_PORT, None)
            };
            specs.push(ListenerSpec {
                kind: ListenerKind::Http,
                addr: SocketAddr::new(ip, http_port),
                auth,
            });
            specs.push(ListenerSpec {
                kind: ListenerKind::Socks5,
                addr: SocketAddr::new(ip, socks_port),
                auth: None,
            });
        }
        specs
    }

    fn listener_opts(general: &General, spec: &ListenerSpec) -> ListenerOpts {
        ListenerOpts {
            kind: spec.kind,
            restrict_to_lan: general.proxy_restricted_to_lan,
            auth: spec.auth.clone(),
            show_error_page: general.show_error_page,
            show_error_page_for_reject: general.show_error_page_for_reject,
            drop_hold: DROP_HOLD,
        }
    }

    /// Binds every listener in `listener_specs` order; the first failure aborts.
    pub async fn bind_listeners(self: &Arc<Self>) -> io::Result<Vec<Running>> {
        let rt = self.runtime();
        let mut out = Vec::new();
        for spec in Self::listener_specs(&rt.config.general) {
            let opts = Self::listener_opts(&rt.config.general, &spec);
            let dialer: Arc<dyn Dialer> = self.clone();
            let running = match spec.kind {
                ListenerKind::Http => HttpListener::bind(spec.addr, dialer, opts).await?,
                ListenerKind::Socks5 => Socks5Listener::bind(spec.addr, dialer, opts).await?,
                _ => continue,
            };
            tracing::info!(kind = ?spec.kind, addr = %running.local_addr, "listening");
            out.push(running);
        }
        Ok(out)
    }

    fn new_handle(&self, session: SessionInfo) -> Arc<SessionHandle> {
        let id = self.next_session.fetch_add(1, Ordering::Relaxed) + 1;
        let handle = SessionHandle::new(id, session);
        handle.on_finish(log_session);
        handle
    }
}

fn log_session(h: &SessionHandle, outcome: &SessionOutcome) {
    let s = h.session();
    let (up, down) = h.bytes();
    let dst = format!("{}:{}", s.dst_host, s.dst_port);
    let rule = h.rule().unwrap_or_default();
    let policy = h.policy_chain().join(" > ");
    let elapsed_ms = h.elapsed().as_millis() as u64;
    match outcome {
        SessionOutcome::Completed => tracing::debug!(
            session = h.id(), listener = ?s.listener, src = %s.src, dst = %dst, rule = %rule, policy = %policy,
            up, down, elapsed_ms, "session completed"
        ),
        SessionOutcome::Rejected(kind) => tracing::info!(
            session = h.id(), listener = ?s.listener, src = %s.src, dst = %dst, rule = %rule, policy = %policy,
            elapsed_ms, "session rejected by {}", kind.name()
        ),
        SessionOutcome::Failed(error) => tracing::info!(
            session = h.id(), listener = ?s.listener, src = %s.src, dst = %dst, rule = %rule, policy = %policy,
            up, down, elapsed_ms, error = %error, "session failed"
        ),
    }
}

fn fail(handle: Arc<SessionHandle>, kind: FailKind, message: impl Into<String>) -> Result<Dialed, DialError> {
    let message = message.into();
    handle.finish(SessionOutcome::Failed(message.clone()));
    let rule = handle.rule();
    Err(DialError::Failed {
        kind,
        message,
        rule,
        handle,
    })
}

fn reject(handle: Arc<SessionHandle>, kind: rurge_proto::RejectKind) -> Result<Dialed, DialError> {
    handle.finish(SessionOutcome::Rejected(kind));
    let rule = handle.rule();
    Err(DialError::Reject { kind, rule, handle })
}

impl Dialer for Engine {
    fn dial<'a>(&'a self, session: SessionInfo) -> BoxFuture<'a, Result<Dialed, DialError>> {
        Box::pin(async move {
            let rt = self.runtime();
            let handle = self.new_handle(session);
            let policy = match &rt.outbound_mode {
                OutboundMode::Direct => PolicyRef::Builtin(Builtin::Direct),
                OutboundMode::Proxy(p) => p.clone(),
                OutboundMode::Rule => {
                    let decision = rt
                        .rules
                        .evaluate(handle.session(), OutboundMode::Rule, rt.stack.resolver.as_ref())
                        .await;
                    if let Some(i) = decision.matched {
                        handle.set_rule(rt.rules.rules().iter().find(|r| r.index == i).map(|r| r.raw.clone()));
                    }
                    match decision.outcome {
                        Outcome::Policy(p) => p,
                        Outcome::DnsFailed => return fail(handle, FailKind::Dns, "dns lookup failed"),
                    }
                }
            };
            let resolution = rt.policies.resolve(&policy);
            handle.set_policy_chain(resolution.chain.clone());
            let target = Target::new(handle.session().dst_host.clone(), handle.session().dst_port);
            let opts = ConnectOpts {
                timeout: CONNECT_TIMEOUT,
                prefer_v6: rt.config.general.ipv6,
            };
            match resolution.outbound.connect_tcp(&target, &opts).await {
                Ok(stream) => Ok(Dialed { stream, handle }),
                Err(OutboundError::Reject(kind)) => reject(handle, kind),
                Err(OutboundError::Unsupported(_)) => reject(handle, rurge_proto::RejectKind::Reject),
                Err(OutboundError::Dns(m)) => fail(handle, FailKind::Dns, m),
                Err(OutboundError::Io(e)) => fail(handle, FailKind::Connect, e.to_string()),
                Err(OutboundError::Timeout) => fail(handle, FailKind::Timeout, "connect timed out"),
            }
        })
    }

    fn relay<'a>(&'a self, mut client: BoxedStream, mut upstream: BoxedStream, handle: Arc<SessionHandle>) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            match tokio::io::copy_bidirectional(&mut client, &mut upstream).await {
                Ok((up, down)) => {
                    handle.add_up(up);
                    handle.add_down(down);
                    handle.finish(SessionOutcome::Completed);
                }
                Err(e) => handle.finish(SessionOutcome::Failed(e.to_string())),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::config::{LoadOptions, from_text};
    use std::path::Path;

    fn general(text: &str) -> General {
        let loaded = from_text(&format!("[General]\n{text}\n[Proxy]\n[Rule]\nFINAL,DIRECT\n"), Path::new("t.conf"), &LoadOptions::for_tests());
        assert!(!loaded.diagnostics.has_errors());
        loaded.config.general
    }

    #[test]
    fn listener_specs_follow_the_profile_and_defaults() {
        let specs = Engine::listener_specs(&general(""));
        assert_eq!(specs.len(), 2);
        assert_eq!((specs[0].kind, specs[0].addr.to_string()), (ListenerKind::Http, "127.0.0.1:6152".to_string()));
        assert_eq!((specs[1].kind, specs[1].addr.to_string()), (ListenerKind::Socks5, "127.0.0.1:6153".to_string()));
        let specs = Engine::listener_specs(&general("http-listen = s3cret@127.0.0.1:7000, [::1]:7001\nsocks5-listen = 127.0.0.1:7002"));
        assert_eq!(specs.len(), 3);
        assert_eq!(specs[0].auth, Some(HttpAuth::Password("s3cret".to_string())));
        assert_eq!(specs[1].addr.to_string(), "[::1]:7001");
        assert_eq!(specs[2].kind, ListenerKind::Socks5);
        let specs = Engine::listener_specs(&general("allow-wifi-access = true\nwifi-access-http-port = 8080\nwifi-access-socks5-port = 8081\nwifi-access-http-auth = alice:pw"));
        assert_eq!(specs[0].addr.to_string(), "0.0.0.0:8080");
        assert_eq!(specs[0].auth, Some(HttpAuth::UserPass { user: "alice".into(), password: "pw".into() }));
        assert_eq!(specs[1].addr.to_string(), "0.0.0.0:8081");
    }
}
```

`lib.rs`：追加 `pub mod engine;`、`pub mod runtime;` 与 `pub use engine::{Engine, ListenerSpec}; pub use runtime::{Runtime, RuntimeOptions};`。

> `wifi-access-http-auth` 的值格式以 M1 解析器为准（`Option<(String, String)>`）；若 M1 不接受 `alice:pw` 写法，按 `general.rs` 的解析改测试输入。`handle.session()` 的借用在每次调用后即结束，`handle` 之后按值移入 `fail` / `reject`。

- [ ] **Step 3: 端到端集成测试 `tests/pipeline.rs`**

```rust
//! Profile text → Runtime → Engine → real listeners on the loopback, talking to
//! `TestServer` targets through the HTTP and SOCKS5 proxies (M3 design §10).

use rurge_config::config::{LoadOptions, from_text};
use rurge_dns::system::StaticSystemDns;
use rurge_dns::testing::MockDns;
use rurge_engine::stack::StackOptions;
use rurge_engine::{Engine, Runtime, RuntimeOptions};
use rurge_inbound::Running;
use rurge_net::testing::TestServer;
use rurge_policy::GroupSelections;
use rurge_rules::{GeoUrls, OutboundMode};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

struct Harness {
    _dir: tempfile::TempDir,
    _engine: Arc<Engine>,
    listeners: Vec<Running>,
    target: TestServer,
    dns: MockDns,
    diagnostics: Vec<&'static str>,
}

impl Harness {
    fn http(&self) -> SocketAddr {
        self.listeners[0].local_addr
    }
    fn socks(&self) -> SocketAddr {
        self.listeners[1].local_addr
    }
    fn target_port(&self) -> u16 {
        self.target.url("/").port().unwrap()
    }
}

/// `general_extra` lands in [General]; `rules` are inserted before `FINAL,DIRECT`.
async fn harness(general_extra: &str, rules: &str, mode: OutboundMode) -> Harness {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    dns.set("tls.test", &["127.0.0.1"], &[], 60);
    dns.set("cidr.test", &["10.9.9.9"], &[], 60);
    dns.set_empty("nx.test");
    let target = TestServer::spawn().await;
    target.set("/hello", "hi there");
    let dir = tempfile::tempdir().unwrap();
    let profile = format!(
        "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\ndns-server = {}\nipv6 = false\n{general_extra}\n\
[Proxy]\nHK = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\nBlock = reject-tinygif\n\
[Proxy Group]\nPick = select, HK, DIRECT\n\
[Rule]\n{rules}\nFINAL,DIRECT\n",
        dns.addr()
    );
    let loaded = from_text(&profile, &dir.path().join("t.conf"), &LoadOptions::for_tests());
    assert!(!loaded.diagnostics.has_errors(), "{:?}", loaded.diagnostics.iter().map(|d| d.code).collect::<Vec<_>>());
    let diagnostics: Vec<&'static str> = loaded.diagnostics.iter().map(|d| d.code).collect();
    let runtime = Runtime::build(
        loaded.config,
        RuntimeOptions {
            stack: StackOptions {
                data_dir: dir.path().to_path_buf(),
                no_network: true,
                geo_urls: GeoUrls::default(),
                dns_cache_size: 2000,
                system: Arc::new(StaticSystemDns::default()),
                wait: Duration::ZERO,
            },
            outbound_mode: mode,
            selections: GroupSelections::new(),
        },
    )
    .await
    .unwrap();
    let engine = Engine::new(runtime);
    let listeners = engine.bind_listeners().await.unwrap();
    assert_eq!(listeners.len(), 2);
    Harness {
        _dir: dir,
        _engine: engine,
        listeners,
        target,
        dns,
        diagnostics,
    }
}

/// Writes `request`, reads headers + Content-Length body (or until close).
async fn http_exchange(stream: &mut TcpStream, request: &str) -> (String, Vec<u8>) {
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = match tokio::time::timeout(Duration::from_secs(3), stream.read(&mut chunk)).await {
            Ok(Ok(n)) => n,
            _ => 0,
        };
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..pos]).to_string();
            let len = head
                .lines()
                .find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("content-length")).map(|(_, v)| v.trim().parse::<usize>().unwrap()))
                .unwrap_or(0);
            while buf.len() < pos + 4 + len {
                let n = stream.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            return (head, buf[pos + 4..].to_vec());
        }
    }
    (String::from_utf8_lossy(&buf).to_string(), Vec::new())
}

async fn get_via_proxy(proxy: SocketAddr, url: &str) -> (String, Vec<u8>) {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    let host = url.trim_start_matches("http://").split('/').next().unwrap().to_string();
    http_exchange(&mut s, &format!("GET {url} HTTP/1.1\r\nHost: {host}\r\n\r\n")).await
}

#[tokio::test]
async fn plain_http_is_forwarded_through_direct() {
    let h = harness("", "", OutboundMode::Rule).await;
    let (head, body) = get_via_proxy(h.http(), &format!("http://target.test:{}/hello", h.target_port())).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, b"hi there");
    assert_eq!(h.target.requests()[0].path, "/hello");
    assert_eq!(h.dns.query_count("target.test", rurge_dns::message::Qtype::A), 1, "resolved once through the profile resolver");
}

#[tokio::test]
async fn connect_tunnel_carries_tls_to_the_target() {
    let h = harness("", "", OutboundMode::Rule).await;
    let tls_target = TestServer::spawn_tls().await;
    tls_target.set("/secure", "very secret");
    let port = tls_target.url("/").port().unwrap();
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    let (head, _) = http_exchange(&mut s, &format!("CONNECT tls.test:{port} HTTP/1.1\r\nHost: tls.test:{port}\r\n\r\n")).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let config = rurge_net::http::tls_client_config(true).unwrap();
    let name = rustls::pki_types::ServerName::try_from("tls.test".to_string()).unwrap();
    let mut tls = tokio_rustls::TlsConnector::from(config).connect(name, s).await.unwrap();
    tls.write_all(b"GET /secure HTTP/1.1\r\nHost: tls.test\r\nConnection: close\r\n\r\n").await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(3), tls.read_to_end(&mut out)).await;
    let text = String::from_utf8_lossy(&out);
    assert!(text.starts_with("HTTP/1.1 200") && text.ends_with("very secret"), "{text}");
}

#[tokio::test]
async fn socks5_connect_reaches_the_target() {
    let h = harness("", "", OutboundMode::Rule).await;
    let port = h.target_port();
    let mut s = TcpStream::connect(h.socks()).await.unwrap();
    s.write_all(&[5, 1, 0]).await.unwrap();
    let mut r = [0u8; 2];
    s.read_exact(&mut r).await.unwrap();
    assert_eq!(r, [5, 0]);
    let mut req = vec![5, 1, 0, 3, 11];
    req.extend_from_slice(b"target.test");
    req.extend_from_slice(&port.to_be_bytes());
    s.write_all(&req).await.unwrap();
    let mut reply = [0u8; 10];
    s.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply[1], 0);
    let (head, body) = http_exchange(&mut s, "GET /hello HTTP/1.1\r\nHost: target.test\r\nConnection: close\r\n\r\n").await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, b"hi there");
}

#[tokio::test]
async fn reject_rules_close_serve_gifs_or_render_pages() {
    let rules = "DOMAIN,ads.test,REJECT\nDOMAIN,gif.test,Block\nDOMAIN,hk.test,HK";
    // defaults: REJECT closes, TINYGIF answers, unsupported policy behaves as REJECT
    let h = harness("", rules, OutboundMode::Rule).await;
    assert!(h.diagnostics.contains(&"W0007"), "unsupported ss policy warned at load: {:?}", h.diagnostics);
    let (head, _) = get_via_proxy(h.http(), "http://ads.test/").await;
    assert!(head.is_empty(), "REJECT must close: {head}");
    let (head, body) = get_via_proxy(h.http(), "http://gif.test/ad.gif").await;
    assert!(head.starts_with("HTTP/1.1 200") && head.to_ascii_lowercase().contains("image/gif"), "{head}");
    assert_eq!(body.len(), 43);
    let (head, _) = get_via_proxy(h.http(), "http://hk.test/").await;
    assert!(head.is_empty(), "unsupported policy closes like REJECT: {head}");
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    let (head, _) = http_exchange(&mut s, "CONNECT ads.test:443 HTTP/1.1\r\n\r\n").await;
    assert!(head.is_empty(), "CONNECT reject closes: {head}");
    let mut s = TcpStream::connect(h.socks()).await.unwrap();
    s.write_all(&[5, 1, 0]).await.unwrap();
    let mut r = [0u8; 2];
    s.read_exact(&mut r).await.unwrap();
    let mut req = vec![5, 1, 0, 3, 8];
    req.extend_from_slice(b"ads.test");
    req.extend_from_slice(&443u16.to_be_bytes());
    s.write_all(&req).await.unwrap();
    let mut reply = [0u8; 10];
    s.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply[1], 0x02, "SOCKS5 reports 'not allowed by ruleset'");
    // error pages on
    let h = harness("show-error-page-for-reject = true", rules, OutboundMode::Rule).await;
    let (head, body) = get_via_proxy(h.http(), "http://ads.test/").await;
    assert!(head.starts_with("HTTP/1.1 403"), "{head}");
    let html = String::from_utf8_lossy(&body);
    assert!(html.contains("DOMAIN,ads.test,REJECT") && html.contains("REJECT"), "{html}");
    let (head, body) = get_via_proxy(h.http(), "http://hk.test/").await;
    assert!(head.starts_with("HTTP/1.1 403"), "{head}");
    assert!(String::from_utf8_lossy(&body).contains("!unsupported:ss"));
}

#[tokio::test]
async fn keep_alive_requests_are_dialed_one_by_one() {
    let h = harness("", "DOMAIN,gif.test,Block", OutboundMode::Rule).await;
    let port = h.target_port();
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    let (head, body) = http_exchange(&mut s, &format!("GET http://target.test:{port}/hello HTTP/1.1\r\nHost: target.test:{port}\r\n\r\n")).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, b"hi there");
    let (head, body) = http_exchange(&mut s, "GET http://gif.test/x.gif HTTP/1.1\r\nHost: gif.test\r\n\r\n").await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body.len(), 43);
}

#[tokio::test]
async fn outbound_modes_bypass_the_rules() {
    let h = harness_with_final_reject(OutboundMode::Direct).await;
    let (head, body) = get_via_proxy(h.http(), &format!("http://target.test:{}/hello", h.target_port())).await;
    assert!(head.starts_with("HTTP/1.1 200"), "direct mode ignores rules: {head}");
    assert_eq!(body, b"hi there");
    let h = harness_with_final_reject(OutboundMode::Proxy(rurge_config::rule::PolicyRef::parse("REJECT"))).await;
    let (head, _) = get_via_proxy(h.http(), &format!("http://target.test:{}/hello", h.target_port())).await;
    assert!(head.is_empty(), "proxy=REJECT mode rejects everything: {head}");
}

async fn harness_with_final_reject(mode: OutboundMode) -> Harness {
    // every rule rejects; only the outbound mode can let traffic through
    harness("", "DOMAIN-SUFFIX,test,REJECT", mode).await
}

#[tokio::test]
async fn dns_failure_and_ip_rules() {
    let h = harness("", "IP-CIDR,10.0.0.0/8,Block", OutboundMode::Rule).await;
    // cidr.test resolves to 10.9.9.9 → the IP rule matches → tinygif
    let (head, body) = get_via_proxy(h.http(), "http://cidr.test/").await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body.len(), 43);
    // nx.test has no records → dns failed → 502 error page (show-error-page defaults to true)
    let (head, body) = get_via_proxy(h.http(), "http://nx.test/").await;
    assert!(head.starts_with("HTTP/1.1 502"), "{head}");
    assert!(String::from_utf8_lossy(&body).contains("DNS lookup failed"));
}

#[tokio::test]
async fn http_listener_password_from_the_profile() {
    let dns = MockDns::spawn().await;
    let dir = tempfile::tempdir().unwrap();
    let profile = format!("[General]\nhttp-listen = s3cret@127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\ndns-server = {}\n[Proxy]\n[Rule]\nFINAL,DIRECT\n", dns.addr());
    let loaded = from_text(&profile, &dir.path().join("t.conf"), &LoadOptions::for_tests());
    let runtime = Runtime::build(
        loaded.config,
        RuntimeOptions {
            stack: StackOptions {
                data_dir: dir.path().to_path_buf(),
                no_network: true,
                geo_urls: GeoUrls::default(),
                dns_cache_size: 100,
                system: Arc::new(StaticSystemDns::default()),
                wait: Duration::ZERO,
            },
            outbound_mode: OutboundMode::Rule,
            selections: GroupSelections::new(),
        },
    )
    .await
    .unwrap();
    let engine = Engine::new(runtime);
    let listeners = engine.bind_listeners().await.unwrap();
    let (head, _) = get_via_proxy(listeners[0].local_addr, "http://127.0.0.1:1/").await;
    assert!(head.starts_with("HTTP/1.1 407"), "{head}");
}
```

> `MockDns::query_count` / `set_empty` / `Qtype` 来自 `rurge_dns::testing` 与 `rurge_dns::message`（M2b）。`tls_client_config(true)` 跳过证书校验，`TestServer::spawn_tls` 的自签证书因此可用；`ServerName::try_from(String)` 接受域名。`outbound_modes_bypass_the_rules` 里 `DOMAIN-SUFFIX,test,REJECT` 匹配全部 `*.test` 目标。集成测试全部用 127.0.0.1，不访问公网。

- [ ] **Step 4: 运行与提交**

```bash
cargo test -p rurge-engine
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-engine
git commit -F - <<'EOF'
feat(engine): Runtime / Engine：dial 流水线、relay、监听绑定、会话日志；端到端集成测试

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

预期：`engine` 单元测试 1 个 + `pipeline` 集成测试 8 个通过。

---

### Task 10: `rurge run`、CLI 测试与文档同步

**Files:**
- Modify: `crates/rurge/Cargo.toml`（追加 `tracing.workspace = true`）、`crates/rurge/src/main.rs`、`crates/rurge/src/cli/mod.rs`、`crates/rurge/src/cli/rule.rs`（`parse_mode` 改为 `pub(crate)`）
- Create: `crates/rurge/src/cli/run.rs`
- Modify: `crates/rurge/tests/cli.rs`（追加 `mod run`）
- Modify: `README.md`、`CLAUDE.md`、`docs/surge-compatibility-matrix.md`、`docs/superpowers/specs/2026-09-05-phase1-m3-pipeline-design.md`、`docs/superpowers/plans/2026-09-05-phase1-m3a-pipeline-plan.md`

**Interfaces:**
- Consumes: Task 8 / 9 的 `rurge_engine::{Engine, Runtime, RuntimeOptions, stack::StackOptions, state::{State, STATE_FILE, profile_key}}`、`Runtime::stack_options`（bin）、`rurge_config::general::LogLevel::{Verbose, Info, Notify, Warning}`、`rurge_rules::OutboundMode`、`tokio::signal::ctrl_c`、`tracing_subscriber::fmt` + `filter::LevelFilter`。
- Produces：`rurge run -c <conf> [--outbound-mode direct|proxy=<policy>|rule] [--log-level <l>] [--platform <p>] + RuntimeArgs`；退出码 0（Ctrl-C）/ 1（绑定失败）/ 2（配置错误）；stdout 每个监听一行 `listening on http://127.0.0.1:PORT` 或 `listening on socks5://127.0.0.1:PORT`，随后一行 `rurge <version> running: <n> policies, <m> rules, outbound mode <mode>`；日志经 tracing 到 stdout，级别 = `--log-level` / `RURGE_LOG_LEVEL` 否则 `loglevel` 映射（`verbose`→TRACE、`info`→DEBUG、`notify`→INFO、`warning`→WARN；覆盖值另接受 `debug`、`error`、`trace`、`warn`）。

- [ ] **Step 1: 写 `cli/run.rs`**

```rust
//! `rurge run`: the foreground proxy daemon (M3 design §9.3).

use super::rule::{parse_mode, print_diagnostics};
use super::runtime::RuntimeArgs;
use crate::capabilities;
use anyhow::Context;
use clap::Args;
use rurge_config::config::{LoadOptions, Platform, load};
use rurge_config::general::LogLevel;
use rurge_config::session::ListenerKind;
use rurge_engine::state::{STATE_FILE, State, profile_key};
use rurge_engine::{Engine, Runtime, RuntimeOptions};
use rurge_rules::OutboundMode;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;
use tracing_subscriber::filter::LevelFilter;

#[derive(Args)]
pub struct RunArgs {
    /// Profile to load
    #[arg(short = 'c', long = "config", value_name = "FILE")]
    pub config: PathBuf,
    /// Outbound mode: direct, proxy=<policy>, rule
    #[arg(long, env = "RURGE_OUTBOUND_MODE", value_parser = parse_mode, default_value = "rule")]
    pub outbound_mode: OutboundMode,
    /// Log level override: verbose|info|notify|warning (also debug|error)
    #[arg(long, env = "RURGE_LOG_LEVEL", value_parser = parse_log_level, value_name = "LEVEL")]
    pub log_level: Option<LevelFilter>,
    /// Evaluate the profile as if running on this platform
    #[arg(long, value_parser = super::check::parse_platform)]
    pub platform: Option<Platform>,
    #[command(flatten)]
    pub runtime: RuntimeArgs,
}

pub(crate) fn parse_log_level(s: &str) -> Result<LevelFilter, String> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "verbose" | "trace" => LevelFilter::TRACE,
        "info" | "debug" => LevelFilter::DEBUG,
        "notify" => LevelFilter::INFO,
        "warning" | "warn" => LevelFilter::WARN,
        "error" => LevelFilter::ERROR,
        other => return Err(format!("unknown log level `{other}` (expected verbose, info, notify, warning, debug or error)")),
    })
}

/// Surge `loglevel` → tracing level (phase 1 design §12).
pub(crate) fn level_for(level: &LogLevel) -> LevelFilter {
    match level {
        LogLevel::Verbose => LevelFilter::TRACE,
        LogLevel::Info => LevelFilter::DEBUG,
        LogLevel::Notify => LevelFilter::INFO,
        LogLevel::Warning => LevelFilter::WARN,
    }
}

fn init_logging(level: LevelFilter) {
    let _ = tracing_subscriber::fmt()
        .with_max_level(level)
        .with_target(false)
        .with_ansi(std::io::stdout().is_terminal())
        .try_init();
}

fn mode_name(mode: &OutboundMode) -> String {
    match mode {
        OutboundMode::Direct => "direct".to_string(),
        OutboundMode::Proxy(p) => format!("proxy={p}"),
        OutboundMode::Rule => "rule".to_string(),
    }
}

pub fn run(args: RunArgs) -> anyhow::Result<ExitCode> {
    let platform = args.platform.unwrap_or_else(Platform::current);
    let opts = LoadOptions {
        environment: super::environment(platform, capabilities::CORE_VERSION),
        platform,
        capabilities: capabilities::current(),
    };
    let loaded = load(&args.config, &opts)?;
    if loaded.diagnostics.has_errors() {
        print_diagnostics(&loaded.diagnostics.sorted());
        return Ok(ExitCode::from(2));
    }
    print_diagnostics(&loaded.diagnostics.sorted());
    let cfg = loaded.config;
    init_logging(args.log_level.unwrap_or_else(|| level_for(&cfg.general.loglevel)));
    let rt = args.runtime.resolve(&cfg)?;
    let state = State::load(&rt.data_dir.join(STATE_FILE));
    let selections = state.selections_for(&profile_key(&cfg.source.main));
    let outbound_mode = args.outbound_mode.clone();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let general = cfg.general.clone();
        let engine_rt = Runtime::build(
            cfg,
            RuntimeOptions {
                stack: rt.stack_options(Duration::ZERO),
                outbound_mode: outbound_mode.clone(),
                selections,
            },
        )
        .await
        .context("cannot build the runtime")?;
        print_diagnostics(engine_rt.diagnostics());
        let (policies, rules) = (engine_rt.policies.names().len(), engine_rt.rules.rules().len());
        let engine = Engine::new(engine_rt);
        let listeners = match engine.bind_listeners().await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("error: cannot bind listener: {e}");
                return Ok(ExitCode::from(1));
            }
        };
        for (spec, running) in Engine::listener_specs(&general).iter().zip(&listeners) {
            let scheme = match spec.kind {
                ListenerKind::Socks5 => "socks5",
                _ => "http",
            };
            println!("listening on {scheme}://{}", running.local_addr);
        }
        println!(
            "rurge {} running: {policies} policies, {rules} rules, outbound mode {}",
            env!("CARGO_PKG_VERSION"),
            mode_name(&outbound_mode)
        );
        tokio::signal::ctrl_c().await.context("cannot listen for Ctrl-C")?;
        println!("shutting down");
        drop(listeners);
        Ok(ExitCode::SUCCESS)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_levels_map_like_surge() {
        assert_eq!(level_for(&LogLevel::Verbose), LevelFilter::TRACE);
        assert_eq!(level_for(&LogLevel::Info), LevelFilter::DEBUG);
        assert_eq!(level_for(&LogLevel::Notify), LevelFilter::INFO);
        assert_eq!(level_for(&LogLevel::Warning), LevelFilter::WARN);
        assert_eq!(parse_log_level("DEBUG").unwrap(), LevelFilter::DEBUG);
        assert_eq!(parse_log_level("error").unwrap(), LevelFilter::ERROR);
        assert!(parse_log_level("loud").is_err());
        assert_eq!(mode_name(&OutboundMode::Rule), "rule");
    }
}
```

`crates/rurge/src/cli/rule.rs`：`fn parse_mode` 改为 `pub(crate) fn parse_mode`。`crates/rurge/src/cli/mod.rs`：追加 `pub mod run;`。`crates/rurge/src/main.rs` 的 `Command` 追加：

```rust
    /// Run the proxy in the foreground
    Run(Box<cli::run::RunArgs>),
```

与 `Command::Run(args) => cli::run::run(*args),`。`crates/rurge/Cargo.toml` `[dependencies]` 追加 `tracing.workspace = true`（`run.rs` 若最终不直接用 `tracing` 宏则不加）。

> `General` 需要 `Clone`（M1 已派生；若无，改为在 `Runtime::build` 前先算好 `Engine::listener_specs(&cfg.general)`）。`LogLevel` 若已派生 `Copy`，`level_for` 可按值接收，签名以能编译为准。

- [ ] **Step 2: CLI 测试（追加到 `crates/rurge/tests/cli.rs`）**

```rust
mod run {
    use rurge_net::testing::TestServer;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpStream;
    use std::path::Path;
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    struct Daemon {
        child: Child,
        http: u16,
        socks: u16,
        lines: mpsc::Receiver<String>,
    }

    impl Drop for Daemon {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn write_conf(dir: &Path, general: &str) -> std::path::PathBuf {
        let conf = dir.join("t.conf");
        std::fs::write(
            &conf,
            format!("[General]\n{general}\n[Proxy]\n[Rule]\nDOMAIN,ads.test,REJECT\nFINAL,DIRECT\n"),
        )
        .unwrap();
        conf
    }

    /// Spawns `rurge run` and waits for both `listening on` lines.
    fn spawn_daemon(conf: &Path, data: &Path) -> Daemon {
        let mut child = Command::new(assert_cmd::cargo::cargo_bin("rurge"))
            .arg("run")
            .arg("-c")
            .arg(conf)
            .arg("--no-network")
            .arg("--data-dir")
            .arg(data)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let (mut http, mut socks) = (None, None);
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while http.is_none() || socks.is_none() {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let line = rx.recv_timeout(remaining).expect("rurge run printed its listening lines");
            if let Some(rest) = line.strip_prefix("listening on http://") {
                http = rest.rsplit(':').next().and_then(|p| p.parse().ok());
            } else if let Some(rest) = line.strip_prefix("listening on socks5://") {
                socks = rest.rsplit(':').next().and_then(|p| p.parse().ok());
            }
        }
        Daemon {
            child,
            http: http.unwrap(),
            socks: socks.unwrap(),
            lines: rx,
        }
    }

    fn http_get(port: u16, url: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let host = url.trim_start_matches("http://").split('/').next().unwrap();
        write!(s, "GET {url} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        out
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_proxies_http_and_rejects_by_rule() {
        let target = TestServer::spawn().await;
        target.set("/hello", "hi from target");
        let port = target.url("/").port().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), "http-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nloglevel = warning");
        let daemon = tokio::task::spawn_blocking({
            let conf = conf.clone();
            let data = dir.path().join("data");
            move || spawn_daemon(&conf, &data)
        })
        .await
        .unwrap();
        let http_port = daemon.http;
        assert!(daemon.socks > 0);
        let ok = tokio::task::spawn_blocking(move || http_get(http_port, &format!("http://127.0.0.1:{port}/hello"))).await.unwrap();
        assert!(ok.starts_with("HTTP/1.1 200") && ok.ends_with("hi from target"), "{ok}");
        let rejected = tokio::task::spawn_blocking(move || http_get(http_port, "http://ads.test/")).await.unwrap();
        assert!(rejected.is_empty(), "REJECT closes the connection: {rejected}");
        assert_eq!(target.requests().len(), 1);
        let summary = daemon.lines.try_iter().find(|l| l.contains("running:"));
        assert!(summary.is_some_and(|l| l.contains("outbound mode rule")), "{summary:?}");
    }

    #[test]
    fn run_exits_2_on_a_broken_profile() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("t.conf"), "[General]\n[Proxy]\n[Rule]\nDOMAIN,a.test,DIRECT\n").unwrap(); // no FINAL
        let status = Command::new(assert_cmd::cargo::cargo_bin("rurge"))
            .args(["run", "-c"])
            .arg(dir.path().join("t.conf"))
            .arg("--no-network")
            .arg("--data-dir")
            .arg(dir.path().join("data"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(2));
    }

    #[test]
    fn run_exits_1_when_the_port_is_taken() {
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), &format!("http-listen = 127.0.0.1:{port}\nsocks5-listen = 127.0.0.1:0"));
        let output = Command::new(assert_cmd::cargo::cargo_bin("rurge"))
            .args(["run", "-c"])
            .arg(&conf)
            .arg("--no-network")
            .arg("--data-dir")
            .arg(dir.path().join("data"))
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&output.stderr).contains("cannot bind listener"));
        drop(taken);
    }
}
```

> `assert_cmd::cargo::cargo_bin("rurge")` 返回 `cargo test` 刚构建的二进制路径。子进程用 `--no-network` 与临时数据目录，不触碰真实网络；`loglevel = warning` 让 stdout 只剩 `listening on` / `running:` 两类行。`Drop` 里 `kill` 保证失败时不留孤儿进程。

- [ ] **Step 3: 文档**

`README.md`：
- 中文第 18 行改为：`> **阶段 1 进行中：M1、M2a、M2b、M3a 完成**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`）；M3b（请求记录、热重载）、M4 未开始。`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接送到 DIRECT 或 REJECT；代理协议与策略组算法在阶段 2。`
- 第 56 行改为：`> `rurge check`、`rurge rule match`、`rurge dns lookup` 与 `rurge run`（HTTP / SOCKS5 代理，DIRECT / REJECT）已可用；代理协议在阶段 2，系统代理与 API 在 M4。`
- 特性表：「入站」行第二列末尾追加 `（HTTP / SOCKS5 监听、Basic 认证、`proxy-restricted-to-lan` 已实现，M3a）`；「出站策略」行追加 `（DIRECT 与 REJECT 系四种已实现，M3a）`。
- 英文半部分第 143、180 行与两行表格做同样修改（"M1, M2a, M2b and M3a are done (…, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`); M3b (request log, reload) and M4 have not started. `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections to DIRECT or REJECT by rule; proxy protocols and group algorithms come in phase 2."；"… and `rurge run` (HTTP / SOCKS5 proxy, DIRECT / REJECT) work today; proxy protocols arrive in phase 2, system proxy and the API in M4."；"(HTTP / SOCKS5 listeners, Basic auth, `proxy-restricted-to-lan` implemented, M3a)"；"(DIRECT and the four REJECT flavours implemented, M3a)"）。
- 第 72 / 196 行的 `rurge run -c config.conf` 示例已存在，保留。

`CLAUDE.md`：
- 「当前状态」段改为：`阶段 1 进行中。M1、M2a、M2b、M3a 已完成：…（原有内容）…、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）。M3b（请求记录、流量统计、热重载、SNI 嗅探）、M4（控制面与平台）未开始。`
- 「先读这些文档」追加两条：`docs/superpowers/specs/2026-09-05-phase1-m3-pipeline-design.md`（M3 设计：四个 crate 的接口、dial 流水线、REJECT 语义、`state.json`、`rurge run`）与 `docs/superpowers/plans/2026-09-05-phase1-m3a-pipeline-plan.md`（M3a 实施计划）。
- 「常用命令」追加：`cargo run -p rurge -- run -c config.conf --log-level info   # 前台运行 HTTP / SOCKS5 代理（Ctrl-C 退出）`。
- 「计划中的架构」的依赖方向一句补上 `rurge (bin) → rurge-engine → { rurge-inbound → rurge-proto, rurge-policy → rurge-proto, rurge-dns }`。

`docs/surge-compatibility-matrix.md`：
- 第 149 行 `proxy-restricted-to-lan`：状态改 🟡，备注「rurge 按回环 / 私有 / 链路本地 / ULA 判定来源；手册为『当前子网』」。
- 第 165 / 166 行 `show-error-page` / `show-error-page-for-reject`：备注「错误页为 rurge 自己的 HTML（写明规则、策略链、会话 id）」；`show-error-page` 对连接失败的 502 页在 M3a 只覆盖明文请求，CONNECT 的 502 在 M3b。
- 第 186 行 `http-listen`：备注追加「Basic 认证只比较密码，用户名任意（手册只给出 `[password@]address[:port]`）」，状态改 🟡。
- 第 187 行 `socks5-listen`：备注追加「REJECT 时回 `0x02`（手册未说明）」。
- 第 306 行 `REJECT-DROP`：备注「rurge 最多保持 30 s（M3b 可调）；Surge 直到客户端超时」，状态改 🟡。
- §7 表末尾追加一行：`| 明文 HTTP 代理转发 | 每个请求独立分流；出站连接每请求一条（阶段 4 引入连接池） | | 🟡 | 1 | |`（列数与所在表一致）。
- §10 第 809 行之后追加一行：`| rurge 专有命令 | 前台运行 HTTP / SOCKS5 代理；出站模式初值来自 `--outbound-mode`（M4 起 `state.json` 优先）；`--log-level` 覆盖 `loglevel` | `rurge run -c <conf> [--outbound-mode direct\|proxy=<p>\|rule] [--log-level <l>]` | 1 | 见 M3 设计文档 §9.3；`reload` / `stop` 依赖 M4 的控制通道 |`。

`docs/superpowers/specs/2026-09-05-phase1-m3-pipeline-design.md`：§4 `ConnectOpts` 处加注「实现：直接复用 `rurge_net::connector::ConnectOpts`」；§5 `PolicyRegistry::build` 签名改为 `-> PolicyRegistry` 并加注「W0007 / W0008 / W0009 / W0010 已由 M1 加载器发出，注册表不再返回诊断」；§6.2 `bind` 签名改为 `bind(addr: SocketAddr, dialer: Arc<dyn Dialer>, opts: ListenerOpts)`；§7.1 加注「`Stack` 及其构建自 bin 迁入 `rurge_engine::stack`，平台 DNS 由 `StackOptions.system` 注入」。

本计划末尾追加两节（执行期间按实际情况补充）：

```markdown
## 执行期修正记录

| 任务 | 计划内容 | 实际处理 | 原因 |
| --- | --- | --- | --- |
| 2 | 设计 §4 单独定义 `ConnectOpts` | 复用 `rurge_net::connector::ConnectOpts` | 同形结构，避免重复 |
| 3 | 设计 §5 `PolicyRegistry::build -> (Registry, Diagnostics)` | 返回 `PolicyRegistry`，不产生诊断 | W0007 / W0008 / W0009 / W0010 已由 M1 加载器发出 |
| 6 | 设计 §6.2 `bind(&Listener, ..)` | `bind(addr, dialer, opts)`，认证经 `ListenerOpts.auth`（支持 `wifi-access-http-auth` 的用户名 + 密码） | 监听地址与认证来源不止 `http-listen` |
| 8 | 设计 §7.1 `Runtime` 直接持有资源栈 | `Stack` 构建自 bin 迁入 `rurge_engine::stack`，`rule match` / `dns lookup` 委托 | 避免三处重复的构建逻辑 |
| 9 | 设计 §8 CONNECT 连接失败回 502 | M3a 直接关闭，M3b 补 502 | 设计已注明 |

## 延后事项（M3b）

- 请求记录环形缓冲与活动索引、按策略 / 监听器的流量统计与实时速率（`RequestLog` / `TrafficStats`）。
- CONNECT / SOCKS5 首包 SNI 嗅探；空闲超时；REJECT 自动升级（30 s / 50 次）；CONNECT 连接失败的 502。
- 优雅退出（等会话 ≤ 5 s）；热重载（SIGHUP、`--watch`）；`--log-file` 滚动；`encrypted-dns-follow-outbound-mode`。
- `state.json` 写入与 `outbound_mode` 持久化（M4）；`rurge reload` / `stop`（M4）。
```

- [ ] **Step 4: 运行与提交**

```bash
cargo test -p rurge
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add Cargo.lock crates/rurge README.md CLAUDE.md docs
git commit -F - <<'EOF'
feat(cli): rurge run：前台 HTTP / SOCKS5 代理；文档与兼容性清单同步 M3a

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

预期：`run` 模块 3 个 CLI 测试 + 1 个单元测试通过；既有测试全部通过。

---

## 执行期修正记录

| 任务 | 计划内容 | 实际处理 | 原因 |
| --- | --- | --- | --- |
| 2 | 设计 §4 单独定义 `ConnectOpts` | 复用 `rurge_net::connector::ConnectOpts` | 同形结构，避免重复 |
| 3 | 设计 §5 `PolicyRegistry::build -> (Registry, Diagnostics)` | 返回 `PolicyRegistry`，不产生诊断 | W0007 / W0008 / W0009 / W0010 已由 M1 加载器发出 |
| 6 | 设计 §6.2 `bind(&Listener, ..)` | `bind(addr, dialer, opts)`，认证经 `ListenerOpts.auth`（支持 `wifi-access-http-auth` 的用户名 + 密码） | 监听地址与认证来源不止 `http-listen` |
| 8 | 设计 §7.1 `Runtime` 直接持有资源栈 | `Stack` 构建自 bin 迁入 `rurge_engine::stack`，`rule match` / `dns lookup` 委托 | 避免三处重复的构建逻辑 |
| 9 | 设计 §8 CONNECT 连接失败回 502 | M3a 直接关闭，M3b 补 502 | 设计已注明 |
| 2 | 测试直接对 `Result<BoxedStream, _>` 调 `unwrap_err()` | 先 `.map(|_| ())` 再 `unwrap_err()` | `BoxedStream` 未实现 `Debug` |
| 3 | 样例代码导入 `PolicyTarget`、`Entry::Group` 带 `kind` 字段 | 删除未用导入与从未读取的字段 | clippy `-D warnings` |
| 4 | 测试用 `FakeDialer` 连接失败时直接返回 `DialError::Failed` | 先 `handle.finish(Failed)` 再返回，并补用例 | `DialError::Failed` 的契约是「句柄已结束」 |
| 8 | `profile_key` 测试用 `C:\p\my.conf` 断言 | 改用 `Path::new("p").join("my.conf")` | 反斜杠在 Unix 不是分隔符，Linux / macOS CI 会失败 |
| 8 | 迁移的代码块丢了文档注释 | 补回 `build_stack` / `build_stack_with` 及 bin 包装函数的注释 | 公开 API 需要说明 |
| 9 | 集成测试用 `LoadOptions::for_tests()` 并断言 W0007 | 测试用去掉 `Shadowsocks` 能力的 `LoadOptions` | `for_tests()` 含全部能力，加载器不会对 `ss` 发 W0007；运行时「未实现 → REJECT」由注册表决定，不受能力集影响 |
| 最终审查 | 分支整体审查（1 Critical / 9 Important / 13 Minor）后的修复波 | A1 监听器新增 `handshake_timeout`（30 s），HTTP 侧显式装 `TokioTimer` 才能启用 `header_read_timeout`，SOCKS5 侧整段握手加超时；A2 明文转发按 RFC 7230 覆盖 `Host`、双向剥离逐跳头、只接受 `http://` 绝对 URI、CONNECT 缺端口回 400、`session.url` 去 userinfo；A3 `relay` 把 `Counting` 包在 upstream 侧边传边计数，失败时计数保留；A4 `SessionHandle` 新增 `error`，未实现协议写入 `policy protocol not implemented: <type>` 并进 `Rejected` 日志；A5 SOCKS5 空域名回 `0x01`、Basic scheme 大小写不敏感 + 常量时间比密码、告警表加 1024 条上限；A6 `bind_listeners` 返回 `Vec<(ListenerSpec, Running)>` 且显式列出跳过的 `ListenerKind`；A7 CLI 测试先建 kill-on-Drop 守卫再等 `listening on`；A8 删 `rurge-engine` 的 `url` 与 `rurge-proto` 的 `tracing`；A9 `Running` 文档注释改为事实（drop 会中止已派生的会话，CONNECT 隧道不受管）；A10 出站模式测试的 `proxy=` 半段改用 `Block` 以区分模式与规则 | 控制器裁决的最终修复清单（`final-fix-brief.md`）；I6 判定为与 Surge 一致不改代码，只登记 |

## 延后事项（M3b）

- 请求记录环形缓冲与活动索引、按策略 / 监听器的流量统计与实时速率（`RequestLog` / `TrafficStats`）。
- CONNECT / SOCKS5 首包 SNI 嗅探；空闲超时；REJECT 自动升级（30 s / 50 次）；CONNECT 连接失败的 502。
- 优雅退出（等会话 ≤ 5 s）；热重载（SIGHUP、`--watch`）；`--log-file` 滚动；`encrypted-dns-follow-outbound-mode`。
- `state.json` 写入与 `outbound_mode` 持久化（M4）；`rurge reload` / `stop`（M4）。
- `Engine::dial` 按 `index` 线性查找规则原文（`rules()` 已按 index 升序，可改二分）。
- 测试加强：`http_exchange` 把超时与关闭混为一谈；wifi-access 的 `listener_specs` 用例断言不足；`Runtime::diagnostics()` 无测试；`FailKind::Other` 无用例。
- 会话生命周期统一：`Running::drop` 会中止 accept 循环派生的全部会话，而 CONNECT 隧道由 hyper 的 upgrade 路径 `tokio::spawn`、不在 `JoinSet` 里，两者语义不一；M3b 做优雅退出时用 `CancellationToken` + `TaskTracker` 重做。
- HTTP 侧的 REJECT-DROP 不随客户端关闭提前结束（hyper 服务内观察不到），只能固定保持到超时；阶段 4 自有 HTTP 引擎后统一。
- `restrict_to_lan` 缺监听器级用例（现有测试只覆盖 `source_allowed` 本身）。
- 会话日志的字段（`rule` / `policy` / `up` / `down` / `error`）没有断言，需要一个抓 `tracing` 输出的用例。
- 策略组 `select` 的持久选择缺端到端用例（`state.json` → `GroupSelections` → `resolve`）。
- SOCKS5 缺 IPv6 ATYP 用例；未知 ATYP 目前直接断开，应答码 `0x07` / `0x08` 未实现；`BND.ADDR` 恒为 `0.0.0.0:0`，未回出站流的本地地址。
- `OutboundError::{Dns, Unsupported}` 目前没有任何构造点（阶段 2 出站协议落地后才会出现）。
- `State::load` 为同步读取（在 tokio 运行时之外调用，M4 引入写入与 HTTP API 时改异步）。
