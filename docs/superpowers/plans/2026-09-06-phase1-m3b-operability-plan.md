# M3b「可运维」实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 M3a 的连接流水线上补齐可运维能力：请求记录、流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 连接失败错误页、优雅退出与会话生命周期统一、热重载（SIGHUP / `--watch`）、`--log-file` 滚动、`encrypted-dns-follow-outbound-mode`。

**Architecture:** 会话生命周期改为「每会话一个 `CancellationToken`（由引擎根令牌派生）+ 引擎级 `TaskTracker`」，relay 改写为可中断、带空闲超时、边传边计数的手写双向泵；观测数据（`RequestLog` 环形缓冲 + 活动索引、`TrafficStats` 计数与速率）挂在 `Engine` 上，由会话结束钩子填充；热重载用 `ArcSwap<Runtime>` 原子切换并按监听地址集合差异重建监听器；`encrypted-dns-follow-outbound-mode` 通过一个「延迟绑定引擎」的 `Connector` 让 DNS 上游连接走 dial 流水线；上游主机名由解析器自带的 `BootstrapConnector` 先解析，流水线只见 IP 目标，故无环。

**Tech Stack:** Rust stable（edition 2024，MSRV 1.88）、tokio、tokio-util（`CancellationToken` / `TaskTracker`）、hyper 1、tracing / tracing-subscriber / tracing-appender、notify、arc-swap。

**Spec:** `docs/superpowers/specs/2026-09-05-phase1-m3-pipeline-design.md`（M3b：§6.4 生命周期、§7.3 relay 与日志、§7.4 引擎扩展、§8 CONNECT 502、§9.2 `--log-file`、§9.3 `rurge run` 的 M3b 参数、§10 测试、§11 差异）。M3a 计划末尾「延后事项」中标记本阶段处理的项也并入本计划。

## Global Constraints

- Rust stable，edition 2024，workspace `rust-version = 1.88`（let-chains 允许，clippy 对嵌套 `if let` 要求用 let-chains）；`unsafe_code = "forbid"`。
- 质量门（每个任务提交前）：`RUSTFMT="C:\Users\SZV01065\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe" cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace`。时序敏感的二进制（`rurge-inbound --lib`、`rurge-engine --test pipeline`、`rurge --test cli`）各跑 3 次确认稳定。
- 测试绝不访问公网：集成测试用 `rurge_net::testing::TestServer` 与 `rurge_dns::testing::MockDns`，监听 `127.0.0.1:0`，用超时 + 轮询而非裸 `sleep` 等待条件；子进程 `rurge run` 一律带 `--no-network` 且 `http-listen = 127.0.0.1:0`。
- CLI 输出与日志文本用英文；文档用中文，README 中英双语两半内容一致。
- rurge 专有运行时选项只经命令行参数与环境变量提供（FR-CFG-17），绝不扩展 Surge 配置格式。新增参数：`--idle-timeout`（`RURGE_IDLE_TIMEOUT`）、`--request-log-size`（`RURGE_REQUEST_LOG_SIZE`）、`--watch`（`RURGE_WATCH`）、`--log-file`（`RURGE_LOG_FILE`）。
- 与 Surge 的任何行为差异登记进 `docs/surge-compatibility-matrix.md`（列数不变）。
- 配置对象不可变，重载时 `ArcSwap` 原子切换（AR-04）；每个连接一个 tokio 任务（AR-03）。
- 提交：中文主题行，`git commit -F -` + heredoc，末尾两行 trailer：
  `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`
  `Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW`
- 依赖方向不变：`rurge → rurge-engine → { rurge-inbound → rurge-proto, rurge-policy → rurge-proto, rurge-dns } → rurge-rules → rurge-net → rurge-config`；`rurge-engine` 不依赖 `rurge-platform`。

## 现有接口（M3a 结束时，供各任务参考）

- `rurge_inbound::session::SessionHandle`：`new(id, SessionInfo) -> Arc<Self>`、`id()`、`session()->&SessionInfo`、`elapsed()`、`set_rule/rule`、`set_policy_chain/policy_chain`、`set_error/error`、`add_up/add_down/bytes()->(u64,u64)`、`on_finish(FnOnce(&SessionHandle,&SessionOutcome))`、`finish(SessionOutcome)`（幂等）、`is_finished`、`outcome`。`SessionOutcome::{Completed, Rejected(RejectKind), Failed(String)}`。`Counting<S>`（读计 down、写计 up）。`FailKind::{Dns,Connect,Timeout,Other}`。`DialError::{Reject{kind,rule,handle}, Failed{kind,message,rule,handle}}`。`trait Dialer { dial(SessionInfo)->Result<Dialed,DialError>; relay(client,upstream,handle) }`。`Dialed{stream,handle}`。
- `rurge_inbound::listener`：`ListenerOpts { kind, restrict_to_lan, auth, show_error_page, show_error_page_for_reject, drop_hold, handshake_timeout }`（`Default`）、`Running { local_addr }`（`Drop` abort accept 任务）、`serve(listener, name, restrict_to_lan, handler) -> Running`、`bind(addr)`。`HttpAuth::{Password, UserPass}`。
- `rurge_inbound::http`：`HttpListener::bind(addr, dialer, opts)`；私有 `serve_connection`、`handle`、`connect`、`forward`、`origin_form`、`strip_hop_by_hop`、`host_port`、`failure_response`、`ct_eq`、`authorized`。`HandlerError::Close`。
- `rurge_inbound::socks5`：`Socks5Listener::bind(addr, dialer, opts)`；私有 `handshake`、`read_request`、`handle`、`reply(code)`；`REP_*` 常量。
- `rurge_inbound::responses`：`error_page(StatusCode, &ErrorPage)`、`ErrorPage{title,session_id,dst,rule,chain,message}`、`tiny_gif`、`connect_established`、`bad_request`、`proxy_auth_required`、`ResponseBody`。
- `rurge_engine::engine::Engine`：`new(Runtime)->Arc<Engine>`、`runtime()->Arc<Runtime>`、`listener_specs(&General)->Vec<ListenerSpec>`、`bind_listeners(&Arc<Self>)->io::Result<Vec<(ListenerSpec,Running)>>`、`impl Dialer`；`log_session`、`fail`、`reject`（自由函数）；常量 `CONNECT_TIMEOUT=10s`、`DROP_HOLD=30s`、`HANDSHAKE_TIMEOUT=30s`、`DEFAULT_HTTP_PORT/SOCKS5_PORT`。`Engine { runtime: ArcSwap<Runtime>, next_session: AtomicU64 }`。
- `rurge_engine::runtime`：`Runtime { config: Arc<Config>, stack: Stack, rules: RuleEngine, policies: PolicyRegistry, outbound_mode: OutboundMode }`、`RuntimeOptions { stack: StackOptions, outbound_mode, selections }`、`Runtime::build(Config, RuntimeOptions)->anyhow::Result<Runtime>`、`diagnostics()->&Diagnostics`。
- `rurge_engine::stack`：`Stack { resources, registry, geo, geo_updater, resolver: Arc<Resolver>, diagnostics }`、`StackOptions { data_dir, no_network, geo_urls, dns_cache_size, system: Arc<dyn SystemDns>, wait }`、`build_stack(&Config,&StackOptions)`、`build_stack_with(&Config,&StackOptions, impl FnOnce(&mut ResolverConfig))`。
- `rurge_engine::state`：`State`、`STATE_FILE`、`profile_key(&Path)->String`、`State::load(&Path)->State`、`selections_for(&str)->GroupSelections`。
- `rurge_policy`：`PolicyRegistry::{build(&Config,&GroupSelections,OutboundRef), names()->Vec<String>, resolve(&PolicyRef)->Resolution}`、`Resolution { chain: Vec<String>, outbound: OutboundRef, unsupported: Option<String> }`、`GroupSelections::{new, from_map, get}`。
- `rurge_proto`：`RejectKind::{Reject,Drop,NoDrop,TinyGif}`（`name()`、`escalates()`、`from_builtin`）、`OutboundError::{Reject(RejectKind),Unsupported(String),Dns(String),Io(io::Error),Timeout}`、`Direct::with_resolver(Arc<Resolver>)`、`OutboundRef = Arc<dyn Outbound>`。
- `rurge_rules`：`RuleEngine::{evaluate(&SessionInfo, OutboundMode, &dyn LazyResolver)->Decision, rules()->&[CompiledRule], build_with_registry(&Config, Arc<SetRegistry>, Arc<GeoDb>)}`、`Decision { outcome: Outcome, matched: Option<usize>, .. }`、`Outcome::{Policy(PolicyRef),DnsFailed}`、`CompiledRule { index, raw, .. }`、`OutboundMode::{Direct,Proxy(PolicyRef),Rule}`。`SessionInfo.protocol` 参与 `PROTOCOL` 规则匹配，`sni`/`http_host` 参与 extended-matching。
- `rurge_net::connector`：`trait Connector { connect(&Target,&ConnectOpts)->io::Result<BoxedStream> }`、`Target::new(HostName,u16)`、`ConnectOpts { timeout, prefer_v6 }`、`DirectConnector::new(Arc<dyn Resolve>)`、`SystemResolve`、`BoxedStream`、`interleave`。
- `rurge_dns`：`Resolver::{new(ResolverConfig, ResolverDeps)->(Arc<Resolver>,Diagnostics), flush()}`、`ResolverConfig::from_config(&Config)`（字段含 `cache_capacity`）、`ResolverDeps { connector: Arc<dyn Connector>, sets, system, resources }`、`Bootstrap`、`BootstrapConnector::new(inner, bootstrap)`。`rurge_config::session::SessionInfo { src, in_port, listener: ListenerKind, dst_host, dst_port, transport, protocol, sni, http_host, user_agent, url, process, device }`，`ListenerKind::{Http,Socks5,Tun,Forward,Internal}`。

## 文件结构

| 文件 | 职责 | 任务 |
| --- | --- | --- |
| `Cargo.toml`（workspace） | 新增 `tokio-util`（`rt`）、`tracing-appender` 依赖 | 1、11 |
| `crates/rurge-inbound/src/session.rs` | `SessionHandle` 增：`token`（`CancellationToken`）、`killed`、`sni`/`set_sni`、`protocol`/`set_protocol`；`new_with_token` | 1、6 |
| `crates/rurge-engine/src/relay.rs`（新建） | 可中断、带空闲超时、边传边计数的手写双向泵 | 1 |
| `crates/rurge-engine/src/engine.rs` | 引擎持有根令牌 / `TaskTracker` / 观测；`new_handle` 派生子令牌 + 注册活动会话；relay 委托 `relay.rs`；REJECT 升级；accessors | 1、2、5、7 |
| `crates/rurge-inbound/src/listener.rs` | `serve` 接受停机令牌并在停机时优雅排空 JoinSet；`Running` 增 `stop()` | 2 |
| `crates/rurge-inbound/src/http.rs` | CONNECT 隧道任务交给 `TaskTracker`；CONNECT 连接失败回 502；SNI 记录 | 2、6、8 |
| `crates/rurge-inbound/src/socks5.rs` | SOCKS5 relay 前 SNI 记录；IPv6 / 未知 ATYP 应答码补全 | 6、13 |
| `crates/rurge-engine/src/observe.rs`（新建） | `RequestLog`（环形 + 活动索引 + kill）与 `TrafficStats`（计数 + 速率采样） | 3、4 |
| `crates/rurge-engine/src/sniff.rs`（新建） | TLS ClientHello SNI 解析（纯函数） | 6 |
| `crates/rurge-engine/src/reload.rs`（新建） | `Engine::reload`、监听地址差异计算 | 9 |
| `crates/rurge-engine/src/dns_pipeline.rs`（新建） | `PipelineConnector`（延迟绑定引擎，DNS 走流水线，防环用 Bootstrap） | 12 |
| `crates/rurge-engine/src/runtime.rs` | `RuntimeOptions` / `Runtime` 增 `idle_timeout`、`request_log_size`；`build` 按 `encrypted-dns-follow-outbound-mode` 选择 DNS 连接器 | 1、5、12 |
| `crates/rurge/src/cli/run.rs` | `RunArgs` 增 `--idle-timeout` / `--request-log-size` / `--watch` / `--log-file`（run 专属，不进 `RuntimeArgs`）；优雅退出；SIGHUP / SIGTERM；主循环；attach DNS 流水线 | 1、2、5、10、11、12 |
| `crates/rurge/src/cli/runtime.rs` | `stack_options` 补 `dns_connector: None`（`RuntimeArgs` 被 `rule` / `dns` / `run` 共用，不放 run 专属参数） | 12 |
| 各 `tests/` 与 `#[cfg(test)]` | 单元 + 集成 + CLI 测试 | 全部 |
| `README.md` / `CLAUDE.md` / `docs/surge-compatibility-matrix.md` / 设计文档 / 本计划末尾 | 文档同步 | 14 |

---

### Task 1: 可中断、带空闲超时的 relay 与每会话取消令牌

引擎的 relay 从 M3a 的 `copy_bidirectional` 换成手写双向泵，支持空闲超时与取消（`kill` 与优雅退出共用）。会话取消令牌挂在 `SessionHandle` 上，由引擎根令牌派生。

**Files:**
- Modify: `Cargo.toml`（workspace）—— 新增 `tokio-util`
- Modify: `crates/rurge-inbound/Cargo.toml`、`crates/rurge-engine/Cargo.toml` —— 加 `tokio-util`
- Modify: `crates/rurge-inbound/src/session.rs` —— `SessionHandle` 增取消令牌与 `kill`
- Create: `crates/rurge-engine/src/relay.rs` —— `pump`
- Modify: `crates/rurge-engine/src/lib.rs` —— `pub mod relay;`
- Modify: `crates/rurge-engine/src/runtime.rs` —— `RuntimeOptions`/`Runtime` 增 `idle_timeout`
- Modify: `crates/rurge-engine/src/engine.rs` —— 引擎持根令牌，`new_handle` 派生子令牌，`relay` 委托 `pump`
- Modify: `crates/rurge/src/cli/runtime.rs`、`crates/rurge/src/cli/run.rs` —— `--idle-timeout`
- Modify: `crates/rurge-engine/tests/pipeline.rs` —— 补 `RuntimeOptions.idle_timeout`；加空闲超时用例

**Interfaces:**
- Consumes: `SessionHandle`、`BoxedStream`、`SessionOutcome`。
- Produces: `SessionHandle::{new_with_token(id, SessionInfo, CancellationToken)->Arc<Self>, token()->&CancellationToken, kill(), was_killed()->bool}`；`rurge_engine::relay::pump(client: BoxedStream, upstream: BoxedStream, handle: Arc<SessionHandle>, idle: Duration)`；`RuntimeOptions.idle_timeout: Duration`、`Runtime.idle_timeout: Duration`。

- [ ] **Step 1: 加依赖**

`Cargo.toml`（workspace `[workspace.dependencies]`）加一行：

```toml
tokio-util = { version = "0.7", features = ["rt"] }
```

`crates/rurge-inbound/Cargo.toml` 与 `crates/rurge-engine/Cargo.toml` 的 `[dependencies]` 各加 `tokio-util.workspace = true`。

- [ ] **Step 2: `SessionHandle` 增取消令牌（先写失败测试）**

在 `crates/rurge-inbound/src/session.rs` 的 `#[cfg(test)] mod tests` 加：

```rust
    #[test]
    fn kill_cancels_the_token_and_marks_the_handle() {
        let tok = tokio_util::sync::CancellationToken::new();
        let h = SessionHandle::new_with_token(
            9,
            SessionInfo::tcp(HostName::parse("a.test"), 443),
            tok.child_token(),
        );
        assert!(!h.token().is_cancelled());
        assert!(!h.was_killed());
        h.kill();
        assert!(h.token().is_cancelled());
        assert!(h.was_killed());
        // cancelling the parent also cancels a plain handle's token
        let h2 = SessionHandle::new(1, SessionInfo::tcp(HostName::parse("b.test"), 80));
        assert!(!h2.token().is_cancelled());
    }
```

- [ ] **Step 3: 运行确认失败**

Run: `cargo test -p rurge-inbound session::tests::kill_cancels`
Expected: 编译失败（`new_with_token` / `token` / `kill` / `was_killed` 未定义）。

- [ ] **Step 4: 实现取消令牌**

在 `session.rs`：顶部 `use tokio_util::sync::CancellationToken;`。在 `SessionHandle` 的现有字段末尾追加两个字段（其余字段与类型保持不变）：

```rust
    // 追加到 struct SessionHandle 的字段末尾：
    token: CancellationToken,
    killed: AtomicBool,
```

（`AtomicBool` 已由现有 `use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};` 导入。）保留现有 `new`，改为委托新构造器并自造根令牌：

```rust
impl SessionHandle {
    pub fn new(id: u64, session: SessionInfo) -> Arc<SessionHandle> {
        SessionHandle::new_with_token(id, session, CancellationToken::new())
    }

    pub fn new_with_token(
        id: u64,
        session: SessionInfo,
        token: CancellationToken,
    ) -> Arc<SessionHandle> {
        Arc::new(SessionHandle {
            id,
            started: Instant::now(),
            session,
            rule: Mutex::new(None),
            policy_chain: Mutex::new(Vec::new()),
            error: Mutex::new(None),
            up: AtomicU64::new(0),
            down: AtomicU64::new(0),
            finished: AtomicBool::new(false),
            outcome: Mutex::new(None),
            on_finish: Mutex::new(None),
            token,
            killed: AtomicBool::new(false),
        })
    }

    /// The session's cancellation token; `relay` stops when it fires.
    pub fn token(&self) -> &CancellationToken {
        &self.token
    }

    /// Cancels the session (a `kill` request); `relay` will close and finish.
    pub fn kill(&self) {
        self.killed.store(true, Ordering::Relaxed);
        self.token.cancel();
    }

    pub fn was_killed(&self) -> bool {
        self.killed.load(Ordering::Relaxed)
    }
}
```

（`policy_chain` 字段类型不变，仍是 `Mutex<Vec<String>>`；上面的 struct 片段只标注新增字段的位置。）

- [ ] **Step 5: 运行确认通过**

Run: `cargo test -p rurge-inbound session::`
Expected: PASS。

- [ ] **Step 6: `relay::pump`（先写失败测试）**

新建 `crates/rurge-engine/src/relay.rs`，先只放测试确认接口签名（实现随后补）。测试放在文件底部：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::HostName;
    use rurge_config::session::SessionInfo;
    use rurge_inbound::SessionHandle;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn handle() -> Arc<SessionHandle> {
        SessionHandle::new(1, SessionInfo::tcp(HostName::parse("a.test"), 80))
    }

    #[tokio::test]
    async fn relays_both_directions_and_counts_bytes() {
        let (client_a, client_b) = tokio::io::duplex(1024);
        let (upstream_a, upstream_b) = tokio::io::duplex(1024);
        let h = handle();
        let task = tokio::spawn(pump(
            Box::new(client_b),
            Box::new(upstream_b),
            h.clone(),
            Duration::from_secs(30),
        ));
        // client → upstream
        let mut ca = client_a;
        let mut ua = upstream_a;
        ca.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        ua.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");
        // upstream → client
        ua.write_all(b"po").await.unwrap();
        let mut buf2 = [0u8; 2];
        ca.read_exact(&mut buf2).await.unwrap();
        assert_eq!(&buf2, b"po");
        drop(ca);
        drop(ua);
        task.await.unwrap();
        assert_eq!(h.bytes(), (4, 2));
        assert_eq!(h.outcome(), Some(rurge_inbound::SessionOutcome::Completed));
    }

    #[tokio::test(start_paused = true)]
    async fn idle_timeout_closes_a_quiet_session() {
        let (_client_a, client_b) = tokio::io::duplex(1024);
        let (_upstream_a, upstream_b) = tokio::io::duplex(1024);
        let h = handle();
        let task = tokio::spawn(pump(
            Box::new(client_b),
            Box::new(upstream_b),
            h.clone(),
            Duration::from_secs(600),
        ));
        tokio::time::advance(Duration::from_secs(601)).await;
        task.await.unwrap();
        assert!(h.is_finished());
    }

    #[tokio::test]
    async fn kill_stops_the_relay() {
        let (_client_a, client_b) = tokio::io::duplex(1024);
        let (_upstream_a, upstream_b) = tokio::io::duplex(1024);
        let h = handle();
        let task = tokio::spawn(pump(
            Box::new(client_b),
            Box::new(upstream_b),
            h.clone(),
            Duration::from_secs(600),
        ));
        h.kill();
        task.await.unwrap();
        assert_eq!(
            h.outcome(),
            Some(rurge_inbound::SessionOutcome::Failed("killed".into()))
        );
    }
}
```

- [ ] **Step 7: 运行确认失败**

Run: `cargo test -p rurge-engine relay::`
Expected: 编译失败（`pump` 未定义）。

- [ ] **Step 8: 实现 `pump`**

`relay.rs` 顶部实现：

```rust
//! The relay: a cancellable, idle-timed, byte-counting bidirectional copy
//! (M3 design §7.3). Replaces M3a's `copy_bidirectional` so a session can be
//! killed, drained on shutdown, and bounded by an idle timeout.

use rurge_inbound::{SessionHandle, SessionOutcome};
use rurge_net::connector::BoxedStream;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const BUF: usize = 8 * 1024;

/// Copies bytes both ways until either side closes, the idle timer fires, or
/// the handle's token is cancelled; then finishes the handle. Counts client→
/// upstream as `up` and upstream→client as `down` as the bytes move.
pub async fn pump(
    mut client: BoxedStream,
    mut upstream: BoxedStream,
    handle: Arc<SessionHandle>,
    idle: Duration,
) {
    let mut cbuf = vec![0u8; BUF];
    let mut ubuf = vec![0u8; BUF];
    let mut client_open = true;
    let mut upstream_open = true;
    let token = handle.token().clone();
    let idle_timer = tokio::time::sleep(idle);
    tokio::pin!(idle_timer);

    let outcome = loop {
        if !client_open && !upstream_open {
            break SessionOutcome::Completed;
        }
        tokio::select! {
            biased;
            _ = token.cancelled() => {
                break if handle.was_killed() {
                    SessionOutcome::Failed("killed".into())
                } else {
                    SessionOutcome::Completed
                };
            }
            _ = &mut idle_timer => {
                break SessionOutcome::Completed;
            }
            r = client.read(&mut cbuf), if client_open => match r {
                Ok(0) => {
                    let _ = upstream.shutdown().await;
                    client_open = false;
                }
                Ok(n) => {
                    if let Err(e) = upstream.write_all(&cbuf[..n]).await {
                        break SessionOutcome::Failed(e.to_string());
                    }
                    handle.add_up(n as u64);
                    idle_timer.as_mut().reset(tokio::time::Instant::now() + idle);
                }
                Err(e) => break SessionOutcome::Failed(e.to_string()),
            },
            r = upstream.read(&mut ubuf), if upstream_open => match r {
                Ok(0) => {
                    let _ = client.shutdown().await;
                    upstream_open = false;
                }
                Ok(n) => {
                    if let Err(e) = client.write_all(&ubuf[..n]).await {
                        break SessionOutcome::Failed(e.to_string());
                    }
                    handle.add_down(n as u64);
                    idle_timer.as_mut().reset(tokio::time::Instant::now() + idle);
                }
                Err(e) => break SessionOutcome::Failed(e.to_string()),
            },
        }
    };
    handle.finish(outcome);
}
```

在 `crates/rurge-engine/src/lib.rs` 加 `pub mod relay;`（在 `pub mod engine;` 附近）。

- [ ] **Step 9: 运行确认通过**

Run: `cargo test -p rurge-engine relay::`
Expected: 三个用例 PASS。

- [ ] **Step 10: `Runtime` 增 `idle_timeout`**

`crates/rurge-engine/src/runtime.rs`：`RuntimeOptions` 加 `pub idle_timeout: Duration,`（需 `use std::time::Duration;`）；`Runtime` 加 `pub idle_timeout: Duration,`；`build` 结尾构造 `Runtime` 时加 `idle_timeout: opts.idle_timeout,`。

- [ ] **Step 11: 引擎持根令牌并派生子令牌，`relay` 委托 `pump`**

`crates/rurge-engine/src/engine.rs`：
- 顶部 `use tokio_util::sync::CancellationToken;`。
- `Engine` 加字段 `sessions_root: CancellationToken,`；`new` 里 `sessions_root: CancellationToken::new(),`。
- `new_handle` 改为派生子令牌：

```rust
    fn new_handle(&self, session: SessionInfo) -> Arc<SessionHandle> {
        let id = self.next_session.fetch_add(1, Ordering::Relaxed) + 1;
        let handle = SessionHandle::new_with_token(id, session, self.sessions_root.child_token());
        handle.on_finish(log_session);
        handle
    }
```

- `relay` 改为：

```rust
    fn relay<'a>(
        &'a self,
        client: BoxedStream,
        upstream: BoxedStream,
        handle: Arc<SessionHandle>,
    ) -> BoxFuture<'a, ()> {
        let idle = self.runtime().idle_timeout;
        Box::pin(crate::relay::pump(client, upstream, handle, idle))
    }
```

删除 engine.rs 里不再用到的 `Counting`、`copy_bidirectional` 相关导入（`use rurge_inbound::{... Counting ...}` 去掉 `Counting`）。

- [ ] **Step 12: 打通 CLI `--idle-timeout`（放在 `RunArgs`，不放 `RuntimeArgs`）**

`RuntimeArgs` 同时被 `rule match` / `dns lookup` / `run` flatten，run 专属参数一律放 `crates/rurge/src/cli/run.rs` 的 `RunArgs`：

```rust
    /// Close a session after this many seconds with no traffic either way (default 600)
    #[arg(long, env = "RURGE_IDLE_TIMEOUT", value_name = "SECS")]
    pub idle_timeout: Option<u64>,
```

`run()` 里在构造 `RuntimeOptions` 前算 `let idle_timeout = Duration::from_secs(args.idle_timeout.unwrap_or(600).max(1));`，构造时 `idle_timeout,`。（Task 10 会把这些 run 专属值收进一个 `RunOptions { idle_timeout, request_log_size }` 供重载复用。）

- [ ] **Step 13: 集成测试：`build_runtime` 助手与真实空闲关闭用例**

`crates/rurge-engine/tests/pipeline.rs`：两处现有 `RuntimeOptions { ... }` 各加 `idle_timeout: Duration::from_secs(600),`。再加一个可指定空闲超时的 `Runtime` 构建助手（后续任务复用），以及一个真实的空闲关闭用例（SOCKS5 隧道建立后双方都不发数据，relay 必须在空闲超时后关闭客户端连接）：

```rust
/// Builds a Runtime for a profile whose listeners are 127.0.0.1:0; `general_extra`
/// lands in [General], `rules` before FINAL,DIRECT. Reused by later tests.
async fn build_runtime(
    dir: &std::path::Path,
    dns: &MockDns,
    general_extra: &str,
    rules: &str,
    mode: OutboundMode,
    idle_timeout: Duration,
) -> Runtime {
    let profile = format!(
        "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\ndns-server = {}\nipv6 = false\n{general_extra}\n\
[Proxy]\nBlock = reject-tinygif\n[Proxy Group]\n[Rule]\n{rules}\nFINAL,DIRECT\n",
        dns.addr()
    );
    let loaded = from_text(&profile, &dir.join("t.conf"), &LoadOptions::for_tests());
    assert!(!loaded.diagnostics.has_errors());
    Runtime::build(
        loaded.config,
        RuntimeOptions {
            stack: StackOptions {
                data_dir: dir.to_path_buf(),
                no_network: true,
                geo_urls: GeoUrls::default(),
                dns_cache_size: 2000,
                system: Arc::new(StaticSystemDns::default()),
                wait: Duration::ZERO,
            },
            outbound_mode: mode,
            idle_timeout,
            selections: GroupSelections::new(),
        },
    )
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_sessions_are_closed() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let target = TestServer::spawn().await; // an HTTP server: it never speaks first
    let dir = tempfile::tempdir().unwrap();
    let runtime = build_runtime(
        dir.path(),
        &dns,
        "",
        "", // the template already ends with FINAL,DIRECT
        OutboundMode::Rule,
        Duration::from_millis(300),
    )
    .await;
    let engine = Engine::new(runtime);
    let listeners = engine.bind_listeners().await.unwrap();
    let socks = listeners
        .iter()
        .find(|(s, _)| s.kind == ListenerKind::Socks5)
        .unwrap()
        .1
        .local_addr;
    let mut s = TcpStream::connect(socks).await.unwrap();
    s.write_all(&[5, 1, 0]).await.unwrap();
    let mut r = [0u8; 2];
    s.read_exact(&mut r).await.unwrap();
    let mut req = vec![5, 1, 0, 3, 11];
    req.extend_from_slice(b"target.test");
    req.extend_from_slice(&target.url("/").port().unwrap().to_be_bytes());
    s.write_all(&req).await.unwrap();
    let mut reply = [0u8; 10];
    s.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply[1], 0, "tunnel established");
    // no traffic either way → the relay closes the tunnel after the idle timeout
    let mut buf = [0u8; 1];
    let n = tokio::time::timeout(Duration::from_secs(3), s.read(&mut buf))
        .await
        .expect("closed well before 3 s")
        .unwrap_or(0);
    assert_eq!(n, 0, "client side closed by the idle timeout");
}
```

（`build_runtime` 此时的 `RuntimeOptions` 只有到 Task 1 为止的字段；后续任务新增字段时按各自步骤补齐。）

既有用例 `relay_counts_bytes_as_they_move_and_keeps_them_on_failure` 的注释提到 `copy_bidirectional`，改成描述 `pump`；其 (b) 段（上游消失后客户端再写）期望 `Failed`——pump 把写入失败记为 `Failed(e)`，断言无需改。

- [ ] **Step 13b: relay 吞吐基准（设计 §10，对应 NFR-01；CI 只编译）**

`crates/rurge-engine/Cargo.toml`：`[dev-dependencies]` 加 `criterion.workspace = true`，并加

```toml
[[bench]]
name = "relay"
harness = false
```

新建 `crates/rurge-engine/benches/relay.rs`（与 `rurge-rules` / `rurge-dns` 的基准同一写法）：

```rust
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use rurge_config::HostName;
use rurge_config::session::SessionInfo;
use rurge_engine::relay::pump;
use rurge_inbound::SessionHandle;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const BYTES: usize = 16 * 1024 * 1024;
const CHUNK: usize = 64 * 1024;

fn relay_throughput(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let mut g = c.benchmark_group("relay");
    g.throughput(Throughput::Bytes(BYTES as u64));
    g.bench_function("pump_16MiB_over_duplex", |b| {
        b.iter(|| {
            rt.block_on(async {
                let (mut client_a, client_b) = tokio::io::duplex(CHUNK);
                let (mut upstream_a, upstream_b) = tokio::io::duplex(CHUNK);
                let h = SessionHandle::new(1, SessionInfo::tcp(HostName::parse("bench.test"), 80));
                let relay = tokio::spawn(pump(
                    Box::new(client_b),
                    Box::new(upstream_b),
                    h,
                    Duration::from_secs(60),
                ));
                let writer = tokio::spawn(async move {
                    let chunk = vec![0u8; CHUNK];
                    let mut sent = 0;
                    while sent < BYTES {
                        client_a.write_all(&chunk).await.expect("write");
                        sent += chunk.len();
                    }
                    client_a.shutdown().await.expect("shutdown");
                });
                let mut buf = vec![0u8; CHUNK];
                let mut got = 0;
                while got < BYTES {
                    let n = upstream_a.read(&mut buf).await.expect("read");
                    if n == 0 {
                        break;
                    }
                    got += n;
                }
                drop(upstream_a);
                writer.await.expect("writer");
                relay.await.expect("relay");
            })
        });
    });
    g.finish();
}

criterion_group!(benches, relay_throughput);
criterion_main!(benches);
```

Run: `cargo bench -p rurge-engine --no-run`（CI 同样只编译）。本机可 `cargo bench -p rurge-engine` 看吞吐数字，记入本计划末尾「执行期修正记录」的基准一行。

- [ ] **Step 14: 质量门与提交**

```bash
cargo test -p rurge-inbound -p rurge-engine
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add Cargo.toml Cargo.lock crates/rurge-inbound crates/rurge-engine crates/rurge
git commit -F - <<'EOF'
feat(engine): 可中断、带空闲超时的 relay；每会话取消令牌

M3a 的 copy_bidirectional 换成手写双向泵：支持 kill / 优雅退出的取消，
两个方向都无数据超过 idle-timeout 则关闭，边传边计数。会话取消令牌挂在
SessionHandle 上由引擎根令牌派生。新增 --idle-timeout。

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

预期：`relay` 3 个单元测试、`session` 取消测试、`pipeline` 全绿。

---

### Task 2: 优雅退出与会话生命周期统一

引擎持一个「停止接受」令牌与一个 `TaskTracker`；监听器在收到停止令牌后停止 accept 并排空在飞会话；CONNECT 隧道与明文转发的上游连接任务交给 `TaskTracker`；`rurge run` 收到 Ctrl-C 后停止接受、等活动会话最多 5 s、再强制取消，第二次 Ctrl-C 立即退出。

**Files:**
- Modify: `crates/rurge-inbound/src/listener.rs` —— `serve` 接受停止令牌并排空；`Running::join`
- Modify: `crates/rurge-inbound/src/http.rs` —— `bind` 接受 `TaskTracker`；CONNECT / 上游连接任务交给它
- Modify: `crates/rurge-inbound/src/socks5.rs` —— `bind` 接受停止令牌（转交 `serve`）
- Modify: `crates/rurge-engine/src/engine.rs` —— 引擎持 `accept` 令牌与 `tracker`；`bind_listeners` 传入；`shutdown` 辅助
- Modify: `crates/rurge/src/cli/run.rs` —— 优雅退出编排；第二次 Ctrl-C
- Modify: 各 `#[cfg(test)]` 与 `pipeline.rs` —— 更新 `bind` 调用

**Interfaces:**
- Consumes: `CancellationToken`、`TaskTracker`。
- Produces:
  - `serve(listener, name, restrict_to_lan, stop: CancellationToken, handler) -> Running`
  - `Running::join(self) -> impl Future<Output=()>`（等 accept 循环退出，包含其 JoinSet 排空）
  - `HttpListener::bind(addr, dialer, opts, stop: CancellationToken, tracker: TaskTracker)`
  - `Socks5Listener::bind(addr, dialer, opts, stop: CancellationToken)`
  - `Engine`：`stop_accepting(&self)`、`cancel_sessions(&self)`、`tracker(&self)->&TaskTracker`、`close_tracker(&self)`

- [ ] **Step 1: `serve` 接受停止令牌并排空（改实现）**

`crates/rurge-inbound/src/listener.rs`：顶部 `use tokio_util::sync::CancellationToken;`。`serve` 增参数 `stop: CancellationToken`，accept 循环增一个 `stop` 分支，跳出后排空 JoinSet：

```rust
pub(crate) fn serve<F, Fut>(
    listener: TcpListener,
    name: &'static str,
    restrict_to_lan: bool,
    stop: CancellationToken,
    handler: F,
) -> Running
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
                _ = stop.cancelled() => break,
                accepted = listener.accept() => match accepted {
                    Ok((stream, peer)) => {
                        if restrict_to_lan && !source_allowed(local_addr, peer.ip()) {
                            let mut map = warned.lock().expect("warned sources");
                            if warn_due(&mut map, peer.ip(), Instant::now()) {
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
                    if let Err(e) = joined && e.is_panic() {
                        tracing::error!(listener = name, "session task panicked: {e}");
                    }
                }
            }
        }
        // Stopped accepting: close the socket so nothing queues in the backlog,
        // then drain the in-flight sessions. A relay only returns once its own
        // token is cancelled (graceful) or its peers close; the daemon cancels
        // the session root after the grace period.
        drop(listener);
        closed_signal.cancel();
        while let Some(joined) = sessions.join_next().await {
            if let Err(e) = joined && e.is_panic() {
                tracing::error!(listener = name, "session task panicked: {e}");
            }
        }
    });
    Running { local_addr, task: Some(task), stop: stop_for_running, closed }
}
```

（`serve` 开头先 `let stop_for_running = stop.clone();` 与 `let closed = CancellationToken::new(); let closed_signal = closed.clone();`——一份留给 `Running`，一份移进任务。）

`Running` 增 `stop`（只停这一个监听器的 accept，让它的会话自然排空）与 `join`（`Drop` 不变，仍在被直接丢弃时 abort accept 任务）：

```rust
pub struct Running {
    pub local_addr: SocketAddr,
    task: Option<JoinHandle<()>>,
    stop: CancellationToken,
    /// Latched once the accept loop has closed its socket.
    closed: CancellationToken,
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(t) = &self.task {
            t.abort();
        }
    }
}

impl Running {
    /// Stop accepting on this listener only; its in-flight sessions finish on
    /// their own (awaited by `join`).
    pub fn stop(&self) {
        self.stop.cancel();
    }

    /// Resolves once the accept loop has closed its socket, i.e. the address
    /// can be bound again (Windows sets no `SO_REUSEADDR`). Sessions may still
    /// be draining; see `join`.
    pub async fn wait_closed(&self) {
        self.closed.cancelled().await
    }

    /// Waits for the accept loop to stop and its in-flight sessions to drain.
    /// Consumes `self` so the `Drop` abort only fires on an already-finished task.
    pub async fn join(mut self) {
        if let Some(t) = self.task.take() {
            let _ = t.await;
        }
    }
}
```

`serve` 开头再建 `let closed = CancellationToken::new(); let closed_signal = closed.clone();`（`closed_signal` 移进任务，`drop(listener);` 之后紧跟 `closed_signal.cancel();`）。`serve` 末尾构造改为 `Running { local_addr, task: Some(task), stop: stop_for_running, closed }`。

- [ ] **Step 2: 两个监听器 `bind` 转交停止令牌 / tracker**

`crates/rurge-inbound/src/socks5.rs`：`Socks5Listener::bind` 增参数 `stop: CancellationToken`（`use tokio_util::sync::CancellationToken;`），`serve(listener, "socks5", opts.restrict_to_lan, stop, move |...|)`。

`crates/rurge-inbound/src/http.rs`：`HttpListener::bind` 增参数 `stop: CancellationToken, tracker: tokio_util::task::TaskTracker`。把 `tracker` 放进 `Ctx`（`Ctx { dialer, opts, local, peer, tracker }`）：`bind` 捕获 `tracker`，`serve` 的 handler 闭包是 `Fn`（每个连接调用一次），所以在闭包里 `tracker: tracker.clone()` 放进每个连接的 `Ctx`，不能 move。`serve(listener, "http", opts.restrict_to_lan, stop, move |...|)`。`connect` 与 `forward` 里原先的 `tokio::spawn(...)` 改成 `ctx.tracker.spawn(...)`（隧道任务与上游连接驱动任务都纳入 tracker，便于优雅退出等待）。

- [ ] **Step 3: 引擎持令牌与 tracker，`bind_listeners` 传入**

`crates/rurge-engine/src/engine.rs`：
- `use tokio_util::sync::CancellationToken;` 与 `use tokio_util::task::TaskTracker;`。
- `Engine` 加字段 `accept: CancellationToken,`（停止接受）与 `tracker: TaskTracker,`；`sessions_root` 来自 Task 1。`new` 里初始化 `accept: CancellationToken::new(), tracker: TaskTracker::new(),`。
- `bind_listeners` 里按类型传参。每个监听器拿 `accept` 的**子令牌**：全局 `stop_accepting` 取消父令牌时所有监听器停止；重载时可对单个旧监听器 `Running::stop()` 而不影响新监听器：

```rust
                ListenerKind::Http => {
                    HttpListener::bind(spec.addr, dialer, opts, self.accept.child_token(), self.tracker.clone()).await?
                }
                ListenerKind::Socks5 => {
                    Socks5Listener::bind(spec.addr, dialer, opts, self.accept.child_token()).await?
                }
```

- `crates/rurge-engine/src/lib.rs` 加 `pub use rurge_inbound::Running;`（bin 不依赖 `rurge-inbound`，Task 10 的 `run.rs` 要按 `rurge_engine::Running` 命名它）。
- 加辅助：

```rust
impl Engine {
    /// Stop accepting new connections (in-flight sessions keep running).
    pub fn stop_accepting(&self) {
        self.accept.cancel();
    }
    /// Force every in-flight relay to end now.
    pub fn cancel_sessions(&self) {
        self.sessions_root.cancel();
    }
    pub fn tracker(&self) -> &TaskTracker {
        &self.tracker
    }
}
```

- [ ] **Step 4: 更新既有测试对 `bind` 的调用**

`http.rs` / `socks5.rs` 的 `#[cfg(test)]`：给每个 `HttpListener::bind(...)` 补 `CancellationToken::new(), TaskTracker::new()`，给 `Socks5Listener::bind(...)` 补 `CancellationToken::new()`。测试模块顶部 `use tokio_util::sync::CancellationToken;`（http 再加 `use tokio_util::task::TaskTracker;`）。`pipeline.rs` 用的是 `engine.bind_listeners()`，无需改。

- [ ] **Step 5: `rurge run` 优雅退出（改实现）**

`crates/rurge/src/cli/run.rs`：把等待 Ctrl-C 与退出那段改为：

```rust
        let engine = Engine::new(engine_rt);
        let listeners = match engine.bind_listeners().await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("error: cannot bind listener: {e}");
                return Ok(ExitCode::from(1));
            }
        };
        for (spec, running) in &listeners {
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
        println!("shutting down (Ctrl-C again to exit now)");
        engine.stop_accepting();
        engine.tracker().close();
        let running: Vec<_> = listeners.into_iter().map(|(_, r)| r).collect();
        let drain = async {
            for r in running {
                r.join().await;
            }
            engine.tracker().wait().await;
        };
        tokio::select! {
            _ = drain => {}
            _ = tokio::signal::ctrl_c() => {
                println!("forced shutdown");
                return Ok(ExitCode::SUCCESS);
            }
            _ = tokio::time::sleep(GRACE) => {
                println!("grace period elapsed; closing active sessions");
                engine.cancel_sessions();
                // brief wait for the relays to unwind, then exit regardless
                let _ = tokio::time::timeout(Duration::from_secs(1), async {
                    engine.tracker().wait().await;
                }).await;
            }
        }
        Ok(ExitCode::SUCCESS)
```

在文件常量区加 `const GRACE: Duration = Duration::from_secs(5);`。注意：`drain` 借用了 `engine` 与消费了 `listeners`，`select!` 的 `sleep` 分支后仍要能访问 `engine`——把 `engine` 放进 `Arc` 已由 `Engine::new` 保证（返回 `Arc<Engine>`），`drain` 闭包按引用捕获即可；`running` 在 `drain` 里被消费。若借用冲突，改为先 `let engine = engine;`（`Arc`）并在两个分支各 `engine.clone()`。

- [ ] **Step 6: CLI 优雅退出测试（Unix：真实 SIGINT；Windows 无法向子进程发 Ctrl-C，靠 Step 7 的引擎级用例覆盖）**

`crates/rurge/tests/cli.rs` 的 `mod run` 加（`Daemon` 的 `child` / `lines` 字段与本模块同级，可直接访问）：

```rust
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_shuts_down_gracefully_on_sigint() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(
            dir.path(),
            "http-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nloglevel = warning",
        );
        let daemon = tokio::task::spawn_blocking({
            let conf = conf.clone();
            let data = dir.path().join("data");
            move || spawn_daemon(&conf, &data)
        })
        .await
        .unwrap();
        let pid = daemon.child.id().to_string();
        let sent = std::process::Command::new("kill").args(["-INT", &pid]).status().unwrap();
        assert!(sent.success(), "kill -INT");
        // exit 0 within 10 s, and the shutdown line was printed
        let finished = tokio::task::spawn_blocking(move || {
            let mut daemon = daemon;
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            loop {
                if let Ok(Some(status)) = daemon.child.try_wait() {
                    let lines: Vec<String> = daemon.lines.try_iter().collect();
                    return Some((status, lines));
                }
                if std::time::Instant::now() > deadline {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        })
        .await
        .unwrap();
        let (status, lines) = finished.expect("daemon exited within 10 s of SIGINT");
        assert!(status.success(), "exit code 0: {status:?}");
        assert!(lines.iter().any(|l| l.contains("shutting down")), "{lines:?}");
    }
```

（`Daemon` 的 `Drop` 在进程已退出后再 `kill`/`wait` 是无害的。）

- [ ] **Step 7: 引擎级优雅退出用例**

`crates/rurge-engine/tests/pipeline.rs` 末尾：

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_accepting_then_cancel_sessions_drains() {
    let h = harness("", "", OutboundMode::Rule).await; // template already ends with FINAL,DIRECT
    // open a CONNECT tunnel that stays idle (no data), then shut down
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    s.write_all(format!("CONNECT target.test:{} HTTP/1.1\r\n\r\n", h.target_port()).as_bytes())
        .await
        .unwrap();
    let mut buf = [0u8; 12];
    let n = s.read(&mut buf).await.unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 200"));
    // stop accepting; a new connection must be refused (listener closed)
    h.engine.stop_accepting();
    h.engine.tracker().close();
    // force the idle tunnel to end and wait for the tracker to drain
    h.engine.cancel_sessions();
    tokio::time::timeout(Duration::from_secs(5), h.engine.tracker().wait())
        .await
        .expect("tracker drains after cancel");
}
```

（`Harness.engine` 已是 `Arc<Engine>` 公有字段。）

- [ ] **Step 8: 质量门与提交**

```bash
cargo test -p rurge-inbound -p rurge-engine -p rurge
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add Cargo.lock crates/rurge-inbound crates/rurge-engine crates/rurge
git commit -F - <<'EOF'
feat(engine): 优雅退出与会话生命周期统一

监听器收到停止令牌后停止 accept 并排空在飞会话；CONNECT 隧道与上游连接
任务纳入 TaskTracker；rurge run 收到 Ctrl-C 后停止接受、等活动会话最多 5 s
再强制取消，第二次 Ctrl-C 立即退出。

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

预期：全工作区绿；`serve` 的排空、tracker 等待稳定。

---

### Task 3: `RequestLog` —— 请求记录环形缓冲与活动索引

`observe.rs` 的第一半：定长环形缓冲存已结束的请求记录，活动索引存在飞会话（供 `active` 视图与 `kill`）。纯数据结构，单元测试独立。

**Files:**
- Modify: `crates/rurge-inbound/src/session.rs` —— `SessionHandle` 增 `sni`/`protocol` 覆盖字段与访问器
- Create: `crates/rurge-engine/src/observe.rs`
- Modify: `crates/rurge-engine/src/lib.rs` —— `pub mod observe;`

**Interfaces:**
- Consumes: `SessionHandle`（`id/session/rule/policy_chain/error/bytes/elapsed`）、`SessionOutcome`、`rurge_config::session::{ListenerKind, SessionInfo}`、`rurge_config::rule::ProtocolKind`。
- Produces（本任务补在 `SessionHandle` 上，Task 6 的嗅探填充它们）：`set_sni(String)`、`sni()->Option<String>`、`set_protocol(ProtocolKind)`、`protocol()->Option<ProtocolKind>`。
- Produces:
  - `RecordStatus::{Active, Completed, Rejected(String), Failed}`
  - `RequestRecord { id, listener, src, dst, rule, policy, sni, protocol, up, down, started_ms, elapsed_ms, status, error }`
  - `RequestLog::{new(capacity), mark_active(&Arc<SessionHandle>), record_finished(&SessionHandle, &SessionOutcome), recent(usize)->Vec<RequestRecord>, active()->Vec<RequestRecord>, active_bytes()->(u64,u64), kill(u64)->bool, len()->usize}`

- [ ] **Step 0: `SessionHandle` 增 `sni`/`protocol` 覆盖（供记录读取，Task 6 填充）**

`crates/rurge-inbound/src/session.rs`：顶部 `use rurge_config::rule::ProtocolKind;`。给 `SessionHandle` 加字段 `sni: Mutex<Option<String>>,` 与 `protocol: Mutex<Option<ProtocolKind>>,`，在 `new_with_token` 里初始化为 `Mutex::new(None)`。加访问器：

```rust
    /// The sniffed TLS SNI, filled by the engine's relay path (M3b).
    pub fn set_sni(&self, sni: String) {
        *self.sni.lock().expect("session sni") = Some(sni);
    }
    pub fn sni(&self) -> Option<String> {
        self.sni.lock().expect("session sni").clone()
    }
    /// The sniffed application protocol; overrides the (immutable) session's.
    pub fn set_protocol(&self, protocol: ProtocolKind) {
        *self.protocol.lock().expect("session protocol") = Some(protocol);
    }
    pub fn protocol(&self) -> Option<ProtocolKind> {
        *self.protocol.lock().expect("session protocol")
    }
```

加一个小单元测试到 `session.rs` 的 `mod tests`：

```rust
    #[test]
    fn sni_and_protocol_overrides_default_to_none() {
        let h = SessionHandle::new(1, SessionInfo::tcp(HostName::parse("a.test"), 443));
        assert_eq!(h.sni(), None);
        assert_eq!(h.protocol(), None);
        h.set_sni("api.test".into());
        h.set_protocol(rurge_config::rule::ProtocolKind::Https);
        assert_eq!(h.sni().as_deref(), Some("api.test"));
        assert_eq!(h.protocol(), Some(rurge_config::rule::ProtocolKind::Https));
    }
```

- [ ] **Step 1: 写失败测试**

新建 `crates/rurge-engine/src/observe.rs`，先放测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::HostName;
    use rurge_config::session::{ListenerKind, SessionInfo};
    use rurge_inbound::{SessionHandle, SessionOutcome};
    use std::sync::Arc;

    fn handle(id: u64, host: &str) -> Arc<SessionHandle> {
        let mut s = SessionInfo::tcp(HostName::parse(host), 443);
        s.listener = ListenerKind::Http;
        let h = SessionHandle::new(id, s);
        h.set_rule(Some(format!("DOMAIN,{host},DIRECT")));
        h.set_policy_chain(vec!["DIRECT".into()]);
        h
    }

    #[test]
    fn active_then_finished_moves_into_the_ring() {
        let log = RequestLog::new(2);
        let a = handle(1, "a.test");
        log.mark_active(&a);
        assert_eq!(log.active().len(), 1);
        assert_eq!(log.active()[0].status, RecordStatus::Active);
        a.add_up(10);
        a.add_down(20);
        a.finish(SessionOutcome::Completed);
        log.record_finished(&a, &SessionOutcome::Completed);
        assert_eq!(log.active().len(), 0);
        let recent = log.recent(10);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].id, 1);
        assert_eq!((recent[0].up, recent[0].down), (10, 20));
        assert_eq!(recent[0].status, RecordStatus::Completed);
        assert_eq!(recent[0].rule.as_deref(), Some("DOMAIN,a.test,DIRECT"));
    }

    #[test]
    fn ring_evicts_oldest_beyond_capacity() {
        let log = RequestLog::new(2);
        for id in 1..=3 {
            let h = handle(id, "x.test");
            log.mark_active(&h);
            log.record_finished(&h, &SessionOutcome::Completed);
        }
        let recent = log.recent(10);
        assert_eq!(recent.len(), 2);
        // newest first
        assert_eq!(recent[0].id, 3);
        assert_eq!(recent[1].id, 2);
    }

    #[test]
    fn kill_cancels_an_active_session_only() {
        let log = RequestLog::new(4);
        let a = handle(7, "k.test");
        log.mark_active(&a);
        assert!(log.kill(7));
        assert!(a.token().is_cancelled());
        assert!(!log.kill(999));
    }

    #[test]
    fn rejected_and_failed_carry_their_status() {
        let log = RequestLog::new(4);
        let r = handle(1, "r.test");
        r.set_error("policy protocol not implemented: ss");
        log.mark_active(&r);
        log.record_finished(&r, &SessionOutcome::Rejected(rurge_proto::RejectKind::Reject));
        let f = handle(2, "f.test");
        log.mark_active(&f);
        log.record_finished(&f, &SessionOutcome::Failed("dns lookup failed".into()));
        let recent = log.recent(10);
        assert_eq!(recent[0].status, RecordStatus::Failed);
        assert_eq!(recent[0].error.as_deref(), Some("dns lookup failed"));
        assert_eq!(recent[1].status, RecordStatus::Rejected("REJECT".into()));
        assert_eq!(recent[1].error.as_deref(), Some("policy protocol not implemented: ss"));
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p rurge-engine observe::`
Expected: 编译失败。

- [ ] **Step 3: 实现 `RequestLog`**

`observe.rs` 顶部：

```rust
//! Runtime observation (M3 design §7.3): the request log (a bounded ring of
//! finished requests plus an index of the in-flight ones) and traffic stats.
//! The M4 API reads these; nothing here reaches outside the process.

use rurge_config::rule::ProtocolKind;
use rurge_config::session::ListenerKind;
use rurge_inbound::{SessionHandle, SessionOutcome};
use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordStatus {
    Active,
    Completed,
    Rejected(String),
    Failed,
}

#[derive(Clone, Debug)]
pub struct RequestRecord {
    pub id: u64,
    pub listener: ListenerKind,
    pub src: SocketAddr,
    pub dst: String,
    pub rule: Option<String>,
    pub policy: Vec<String>,
    pub sni: Option<String>,
    pub protocol: Option<ProtocolKind>,
    pub up: u64,
    pub down: u64,
    pub started_ms: u64,
    pub elapsed_ms: u64,
    pub status: RecordStatus,
    pub error: Option<String>,
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn record_of(h: &SessionHandle, outcome: Option<&SessionOutcome>) -> RequestRecord {
    let s = h.session();
    let (up, down) = h.bytes();
    let elapsed_ms = h.elapsed().as_millis() as u64;
    let status = match outcome {
        None => RecordStatus::Active,
        Some(SessionOutcome::Completed) => RecordStatus::Completed,
        Some(SessionOutcome::Rejected(k)) => RecordStatus::Rejected(k.name().to_string()),
        Some(SessionOutcome::Failed(_)) => RecordStatus::Failed,
    };
    // `set_error` explains rejects (and anything else the engine notes); a
    // failure's own message is the error when nothing more specific was set.
    let error = h.error().or_else(|| match outcome {
        Some(SessionOutcome::Failed(m)) => Some(m.clone()),
        _ => None,
    });
    RequestRecord {
        id: h.id(),
        listener: s.listener,
        src: s.src,
        dst: format!("{}:{}", s.dst_host, s.dst_port),
        rule: h.rule(),
        policy: h.policy_chain(),
        sni: h.sni(),
        protocol: h.protocol().or(s.protocol),
        up,
        down,
        started_ms: unix_millis().saturating_sub(elapsed_ms),
        elapsed_ms,
        status,
        error,
    }
}

pub struct RequestLog {
    capacity: usize,
    active: Mutex<BTreeMap<u64, Arc<SessionHandle>>>,
    finished: Mutex<VecDeque<RequestRecord>>,
}

impl RequestLog {
    pub fn new(capacity: usize) -> RequestLog {
        RequestLog {
            capacity: capacity.max(1),
            active: Mutex::new(BTreeMap::new()),
            finished: Mutex::new(VecDeque::with_capacity(capacity.max(1))),
        }
    }

    pub fn mark_active(&self, handle: &Arc<SessionHandle>) {
        self.active
            .lock()
            .expect("active index")
            .insert(handle.id(), handle.clone());
    }

    pub fn record_finished(&self, handle: &SessionHandle, outcome: &SessionOutcome) {
        self.active.lock().expect("active index").remove(&handle.id());
        let rec = record_of(handle, Some(outcome));
        let mut ring = self.finished.lock().expect("finished ring");
        if ring.len() == self.capacity {
            ring.pop_front();
        }
        ring.push_back(rec);
    }

    /// Finished requests, newest first.
    pub fn recent(&self, n: usize) -> Vec<RequestRecord> {
        self.finished
            .lock()
            .expect("finished ring")
            .iter()
            .rev()
            .take(n)
            .cloned()
            .collect()
    }

    /// In-flight requests (bytes are live), newest first.
    pub fn active(&self) -> Vec<RequestRecord> {
        self.active
            .lock()
            .expect("active index")
            .values()
            .rev()
            .map(|h| record_of(h, None))
            .collect()
    }

    /// Total bytes moved by the in-flight requests so far.
    pub fn active_bytes(&self) -> (u64, u64) {
        self.active
            .lock()
            .expect("active index")
            .values()
            .fold((0, 0), |(u, d), h| {
                let (hu, hd) = h.bytes();
                (u + hu, d + hd)
            })
    }

    /// Kills an in-flight session by id; false if it is not active.
    pub fn kill(&self, id: u64) -> bool {
        match self.active.lock().expect("active index").get(&id) {
            Some(h) => {
                h.kill();
                true
            }
            None => false,
        }
    }

    pub fn len(&self) -> usize {
        self.finished.lock().expect("finished ring").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
```

`SessionHandle::{sni, protocol}` 已在本任务 Step 0 加好；Task 6 的嗅探负责调用 `set_sni` / `set_protocol` 填充它们。

`crates/rurge-engine/src/lib.rs` 加 `pub mod observe;`。`rurge-engine` 已依赖 `rurge-proto`（`RejectKind::name`）。

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p rurge-engine observe::`
Expected: PASS（四个用例）。

- [ ] **Step 5: 提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-inbound crates/rurge-engine
git commit -F - <<'EOF'
feat(engine): RequestLog：请求记录环形缓冲与活动索引

已结束请求进定长环形缓冲，在飞会话进活动索引（供 active 视图与 kill）。

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 4: `TrafficStats` —— 流量计数与实时速率

`observe.rs` 的第二半：全局 / 按监听器 / 按策略的累计上下行字节，加每秒采样速率。

**Files:**
- Modify: `crates/rurge-engine/src/observe.rs`

**Interfaces:**
- Produces:
  - `TrafficTotals { up, down }`
  - `TrafficStats::{new(), record(&SessionHandle), totals()->TrafficTotals, by_listener()->[(ListenerKind,u64,u64);2], by_policy()->Vec<(String,u64,u64)>, sample(active:(u64,u64)), rate()->(u64,u64)}`

- [ ] **Step 1: 写失败测试**

在 `observe.rs` 的 `mod tests` 追加：

```rust
    #[test]
    fn traffic_accumulates_globally_and_per_dimension() {
        let t = TrafficStats::new();
        let a = handle(1, "a.test"); // Http listener, policy DIRECT
        a.add_up(100);
        a.add_down(200);
        t.record(&a);
        assert_eq!(t.totals().up, 100);
        assert_eq!(t.totals().down, 200);
        let by_l = t.by_listener();
        let http = by_l.iter().find(|(k, _, _)| *k == ListenerKind::Http).unwrap();
        assert_eq!((http.1, http.2), (100, 200));
        let by_p = t.by_policy();
        assert_eq!(by_p, vec![("DIRECT".to_string(), 100, 200)]);
    }

    #[test]
    fn rate_is_the_delta_between_samples() {
        let t = TrafficStats::new();
        // cumulative finished = 0; first sample sees 1000 active bytes up
        t.sample((1000, 0));
        assert_eq!(t.rate(), (1000, 0));
        t.sample((1500, 300));
        assert_eq!(t.rate(), (500, 300));
        // a sample that goes backwards (a session ended, active dropped) clamps to 0
        t.sample((1400, 300));
        assert_eq!(t.rate(), (0, 0));
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p rurge-engine observe::traffic observe::rate`
Expected: 编译失败。

- [ ] **Step 3: 实现 `TrafficStats`**

在 `observe.rs`（`RequestLog` 之后）加：

```rust
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrafficTotals {
    pub up: u64,
    pub down: u64,
}

pub struct TrafficStats {
    up: AtomicU64,
    down: AtomicU64,
    http_up: AtomicU64,
    http_down: AtomicU64,
    socks_up: AtomicU64,
    socks_down: AtomicU64,
    per_policy: Mutex<HashMap<String, (u64, u64)>>,
    // rate sampling: last cumulative-plus-active reading and the last delta
    last_sample: Mutex<(u64, u64)>,
    rate_up: AtomicU64,
    rate_down: AtomicU64,
}

impl Default for TrafficStats {
    fn default() -> Self {
        TrafficStats::new()
    }
}

impl TrafficStats {
    pub fn new() -> TrafficStats {
        TrafficStats {
            up: AtomicU64::new(0),
            down: AtomicU64::new(0),
            http_up: AtomicU64::new(0),
            http_down: AtomicU64::new(0),
            socks_up: AtomicU64::new(0),
            socks_down: AtomicU64::new(0),
            per_policy: Mutex::new(HashMap::new()),
            last_sample: Mutex::new((0, 0)),
            rate_up: AtomicU64::new(0),
            rate_down: AtomicU64::new(0),
        }
    }

    /// Adds a finished session's bytes to the cumulative counters.
    pub fn record(&self, h: &SessionHandle) {
        let (up, down) = h.bytes();
        self.up.fetch_add(up, Ordering::Relaxed);
        self.down.fetch_add(down, Ordering::Relaxed);
        match h.session().listener {
            ListenerKind::Socks5 => {
                self.socks_up.fetch_add(up, Ordering::Relaxed);
                self.socks_down.fetch_add(down, Ordering::Relaxed);
            }
            _ => {
                self.http_up.fetch_add(up, Ordering::Relaxed);
                self.http_down.fetch_add(down, Ordering::Relaxed);
            }
        }
        if let Some(policy) = h.policy_chain().into_iter().rev().find(|p| !p.starts_with('!')) {
            let mut m = self.per_policy.lock().expect("per-policy traffic");
            let e = m.entry(policy).or_insert((0, 0));
            e.0 += up;
            e.1 += down;
        }
    }

    pub fn totals(&self) -> TrafficTotals {
        TrafficTotals {
            up: self.up.load(Ordering::Relaxed),
            down: self.down.load(Ordering::Relaxed),
        }
    }

    pub fn by_listener(&self) -> [(ListenerKind, u64, u64); 2] {
        [
            (
                ListenerKind::Http,
                self.http_up.load(Ordering::Relaxed),
                self.http_down.load(Ordering::Relaxed),
            ),
            (
                ListenerKind::Socks5,
                self.socks_up.load(Ordering::Relaxed),
                self.socks_down.load(Ordering::Relaxed),
            ),
        ]
    }

    pub fn by_policy(&self) -> Vec<(String, u64, u64)> {
        let mut v: Vec<_> = self
            .per_policy
            .lock()
            .expect("per-policy traffic")
            .iter()
            .map(|(k, (u, d))| (k.clone(), *u, *d))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    /// Records one rate sample: `active` is the live byte total of in-flight
    /// sessions; the total tracked is finished-cumulative + active. Rate is the
    /// non-negative delta from the previous sample (call once per second).
    pub fn sample(&self, active: (u64, u64)) {
        let total_up = self.up.load(Ordering::Relaxed) + active.0;
        let total_down = self.down.load(Ordering::Relaxed) + active.1;
        let mut last = self.last_sample.lock().expect("rate sample");
        self.rate_up
            .store(total_up.saturating_sub(last.0), Ordering::Relaxed);
        self.rate_down
            .store(total_down.saturating_sub(last.1), Ordering::Relaxed);
        *last = (total_up, total_down);
    }

    /// Bytes per second from the last two samples.
    pub fn rate(&self) -> (u64, u64) {
        (
            self.rate_up.load(Ordering::Relaxed),
            self.rate_down.load(Ordering::Relaxed),
        )
    }
}
```

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p rurge-engine observe::`
Expected: 全部 PASS。

- [ ] **Step 5: 提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-engine
git commit -F - <<'EOF'
feat(engine): TrafficStats：全局 / 按监听器 / 按策略流量计数与每秒速率

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 5: 把观测挂进引擎（活动注册、结束钩子、accessors、速率采样任务）

`Engine` 持一份 `Observe { log, traffic }`，`new_handle` 注册活动会话，会话结束钩子同时记录到请求记录与流量统计；暴露 `request_log()` / `traffic()` / `kill(id)`；`--request-log-size` 与 1 Hz 速率采样任务。

**Files:**
- Modify: `crates/rurge-engine/src/runtime.rs` —— `RuntimeOptions`/`Runtime` 增 `request_log_size`
- Modify: `crates/rurge-engine/src/engine.rs` —— `Observe`、钩子、accessors、采样任务
- Modify: `crates/rurge-engine/src/lib.rs` —— 重导出 `observe::{RequestLog, RequestRecord, RecordStatus, TrafficStats, TrafficTotals}`
- Modify: `crates/rurge/src/cli/runtime.rs`、`run.rs` —— `--request-log-size`、启动采样
- Modify: `crates/rurge-engine/tests/pipeline.rs` —— 断言请求记录与流量

**Interfaces:**
- Produces: `Engine::{request_log(&self)->&RequestLog, traffic(&self)->&TrafficStats, kill(&self,u64)->bool, start_sampler(self:&Arc<Self>)}`；`Runtime.request_log_size`、`RuntimeOptions.request_log_size`。

- [ ] **Step 1: `Runtime` 增 `request_log_size`**

`runtime.rs`：`RuntimeOptions` 加 `pub request_log_size: usize,`；`Runtime` 加 `pub request_log_size: usize,`；`build` 结尾加 `request_log_size: opts.request_log_size.max(1),`。

- [ ] **Step 2: `Engine` 持 `Observe`（先写失败测试）**

请求记录在会话**结束**时才写入，而明文转发的会话要等上游连接任务结束才 finish——响应读完立刻断言会有竞态。先在 `crates/rurge-engine/tests/pipeline.rs` 加一个轮询助手（后续任务复用）：

```rust
/// Polls the request log until a finished record matches `pred` (≤ `timeout`).
async fn wait_for_record(
    engine: &Engine,
    timeout: Duration,
    pred: impl Fn(&rurge_engine::RequestRecord) -> bool,
) -> Option<rurge_engine::RequestRecord> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Some(r) = engine.request_log().recent(50).into_iter().find(|r| pred(r)) {
            return Some(r);
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
```

然后给「明文经 DIRECT」用例补断言（已有 `plain_http_is_forwarded_through_direct`，在其末尾追加）：

```rust
    // recorded once its upstream connection ended; bytes counted
    let rec = wait_for_record(&h.engine, Duration::from_secs(3), |r| {
        r.dst.starts_with("target.test:") && matches!(r.status, rurge_engine::RecordStatus::Completed)
    })
    .await
    .expect("completed record for target.test");
    assert!(rec.up > 0 && rec.down > 0, "{rec:?}");
    assert!(h.engine.traffic().totals().down > 0);
```

所有 `RuntimeOptions { ... }` 构造点（`harness`、`http_listener_password_from_the_profile`、`build_runtime`）补 `request_log_size: 1000,`。

- [ ] **Step 3: 运行确认失败**

Run: `cargo test -p rurge-engine --test pipeline plain_http`
Expected: 编译失败（`request_log` / `traffic` 未定义）。

- [ ] **Step 4: 实现 `Observe` 与钩子**

`engine.rs`：
- 顶部 `use crate::observe::{RequestLog, TrafficStats};` 与 `use std::sync::Arc;`（已在）。
- 加内部结构：

```rust
struct Observe {
    log: RequestLog,
    traffic: TrafficStats,
}
```

- `Engine` 加字段 `observe: Arc<Observe>,`。`new` 改为读 `runtime.request_log_size`：

```rust
    pub fn new(runtime: Runtime) -> Arc<Engine> {
        let observe = Arc::new(Observe {
            log: RequestLog::new(runtime.request_log_size),
            traffic: TrafficStats::new(),
        });
        Arc::new(Engine {
            runtime: ArcSwap::from_pointee(runtime),
            next_session: AtomicU64::new(0),
            sessions_root: CancellationToken::new(),
            accept: CancellationToken::new(),
            tracker: TaskTracker::new(),
            observe,
        })
    }
```

- `new_handle` 注册活动并在结束钩子里同时记录日志与观测：

```rust
    fn new_handle(&self, session: SessionInfo) -> Arc<SessionHandle> {
        let id = self.next_session.fetch_add(1, Ordering::Relaxed) + 1;
        let handle = SessionHandle::new_with_token(id, session, self.sessions_root.child_token());
        // Weak: the active index holds the handle and the handle holds this
        // hook, so a strong `Arc<Observe>` here would be a reference cycle.
        let observe = Arc::downgrade(&self.observe);
        handle.on_finish(move |h, outcome| {
            log_session(h, outcome);
            if let Some(o) = observe.upgrade() {
                o.log.record_finished(h, outcome);
                o.traffic.record(h);
            }
        });
        self.observe.log.mark_active(&handle);
        handle
    }
```

- accessors 与采样任务：

```rust
impl Engine {
    pub fn request_log(&self) -> &RequestLog {
        &self.observe.log
    }
    pub fn traffic(&self) -> &TrafficStats {
        &self.observe.traffic
    }
    /// Kills an in-flight session by id (M4 `POST /v1/requests/{id}/kill`).
    pub fn kill(&self, id: u64) -> bool {
        self.observe.log.kill(id)
    }
    /// Starts the 1 Hz traffic-rate sampler on the engine's tracker. Call once
    /// after `new`, inside a tokio runtime.
    pub fn start_sampler(self: &Arc<Self>) {
        let engine = self.clone();
        self.tracker.spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {
                    _ = engine.accept.cancelled() => break,
                    _ = tick.tick() => {
                        let active = engine.observe.log.active_bytes();
                        engine.observe.traffic.sample(active);
                    }
                }
            }
        });
    }
}
```

（采样任务在 `accept` 令牌取消后退出，随优雅退出自然结束。）

- `crates/rurge-engine/src/lib.rs` 加：

```rust
pub use observe::{RecordStatus, RequestLog, RequestRecord, TrafficStats, TrafficTotals};
```

- [ ] **Step 5: 运行确认通过**

Run: `cargo test -p rurge-engine --test pipeline`
Expected: PASS。

- [ ] **Step 6: 打通 `--request-log-size` 与采样启动**

`crates/rurge/src/cli/run.rs` 的 `RunArgs` 加（run 专属，不放 `RuntimeArgs`）：

```rust
    /// Keep this many finished requests in the in-memory log (default 1000)
    #[arg(long, env = "RURGE_REQUEST_LOG_SIZE", value_name = "N")]
    pub request_log_size: Option<usize>,
```

`run()` 里 `let request_log_size = args.request_log_size.unwrap_or(1000).max(1);`，构造 `RuntimeOptions` 时 `request_log_size,`；`Engine::new` 之后加 `engine.start_sampler();`。

- [ ] **Step 7: 质量门与提交**

```bash
cargo test -p rurge-engine -p rurge
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-engine crates/rurge
git commit -F - <<'EOF'
feat(engine): 请求记录与流量统计挂进引擎；--request-log-size 与速率采样

会话结束钩子同时写会话日志、请求记录环形缓冲与流量统计；引擎暴露
request_log() / traffic() / kill(id)；start_sampler 每秒采样速率。

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 6: SNI 嗅探（观测用）

解析 TLS ClientHello 的 SNI，在 relay 的第一段客户端数据上就地嗅探，回填 handle 的 `sni` 与 `protocol = Https`。**范围说明（计划级偏差，需登记）**：设计 §7.4 让 SNI 在 dial 前回填以参与路由；但 TLS 客户端要等 CONNECT 200 / SOCKS 成功应答后才发 ClientHello，dial 前嗅探会迫使先应答再路由，破坏 §8 的 REJECT 应答码。M3b 因此只做「观测用」嗅探：填 `sni`/`protocol` 供请求记录与日志，路由仍用 CONNECT / SOCKS 目标主机（HTTPS 场景下二者通常同名）。基于 SNI 的路由与 `PROTOCOL,HTTPS` 的 dial 前匹配随阶段 4 的 HTTP 引擎落地，登记进兼容性清单。

**Files:**
- Create: `crates/rurge-engine/src/sniff.rs` —— `parse_sni`
- Modify: `crates/rurge-engine/src/lib.rs` —— `pub mod sniff;`
- Modify: `crates/rurge-engine/src/relay.rs` —— pump 首段客户端数据嗅探
- Modify: `crates/rurge-engine/tests/pipeline.rs` —— CONNECT 隧道内 TLS 的 SNI 记录断言

**Interfaces:**
- Produces: `rurge_engine::sniff::parse_sni(record: &[u8]) -> Option<String>`。

- [ ] **Step 1: 写 `parse_sni` 失败测试**

新建 `crates/rurge-engine/src/sniff.rs`，先放测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal TLS ClientHello record carrying one SNI host_name.
    fn client_hello(host: &str) -> Vec<u8> {
        let sni = host.as_bytes();
        // server_name extension body: list_len(2) + type(1) + name_len(2) + name
        let mut server_name = Vec::new();
        server_name.extend_from_slice(&((sni.len() + 3) as u16).to_be_bytes());
        server_name.push(0); // host_name
        server_name.extend_from_slice(&(sni.len() as u16).to_be_bytes());
        server_name.extend_from_slice(sni);
        // extension: type 0x0000 + len + body
        let mut ext = Vec::new();
        ext.extend_from_slice(&0u16.to_be_bytes());
        ext.extend_from_slice(&(server_name.len() as u16).to_be_bytes());
        ext.extend_from_slice(&server_name);
        // handshake body
        let mut body = Vec::new();
        body.extend_from_slice(&[0x03, 0x03]); // client_version TLS1.2
        body.extend_from_slice(&[0u8; 32]); // random
        body.push(0); // session_id len
        body.extend_from_slice(&2u16.to_be_bytes()); // cipher suites len
        body.extend_from_slice(&[0x13, 0x01]);
        body.push(1); // compression len
        body.push(0);
        body.extend_from_slice(&(ext.len() as u16).to_be_bytes()); // extensions len
        body.extend_from_slice(&ext);
        // handshake header: type 0x01 + 3-byte len
        let mut hs = vec![0x01];
        let bl = body.len();
        hs.extend_from_slice(&[(bl >> 16) as u8, (bl >> 8) as u8, bl as u8]);
        hs.extend_from_slice(&body);
        // TLS record: type 0x16, version, len
        let mut rec = vec![0x16, 0x03, 0x01];
        rec.extend_from_slice(&(hs.len() as u16).to_be_bytes());
        rec.extend_from_slice(&hs);
        rec
    }

    #[test]
    fn parses_the_sni_host() {
        assert_eq!(parse_sni(&client_hello("api.example.com")).as_deref(), Some("api.example.com"));
    }

    #[test]
    fn rejects_non_tls_and_truncated_input() {
        assert_eq!(parse_sni(b"GET / HTTP/1.1\r\n"), None);
        let full = client_hello("x.test");
        assert_eq!(parse_sni(&full[..20]), None); // truncated
        assert_eq!(parse_sni(&[]), None);
        assert_eq!(parse_sni(&[0x16, 0x03, 0x01]), None);
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p rurge-engine sniff::`
Expected: 编译失败。

- [ ] **Step 3: 实现 `parse_sni`**

`sniff.rs` 顶部（纯字节解析，任何越界 / 非预期都返回 `None`）：

```rust
//! TLS ClientHello SNI parsing (M3b, observability only). Best-effort: any
//! malformed, truncated, or non-TLS input returns `None`.

/// Extracts the SNI host_name from a single TLS ClientHello record, if present.
pub fn parse_sni(record: &[u8]) -> Option<String> {
    // TLS record header: content_type(0x16 handshake) + version(2) + length(2)
    let rec = record.get(..5)?;
    if rec[0] != 0x16 {
        return None;
    }
    let rec_len = u16::from_be_bytes([rec[3], rec[4]]) as usize;
    let body = record.get(5..5 + rec_len)?;
    // Handshake header: type(0x01 ClientHello) + length(3, big-endian)
    if *body.first()? != 0x01 {
        return None;
    }
    let hs_len = ((*body.get(1)? as usize) << 16)
        | ((*body.get(2)? as usize) << 8)
        | (*body.get(3)? as usize);
    let hs = body.get(4..4 + hs_len)?;
    let mut p = 0usize;
    p += 2; // client_version
    p += 32; // random
    let sid_len = *hs.get(p)? as usize;
    p += 1 + sid_len;
    let cs_len = u16::from_be_bytes([*hs.get(p)?, *hs.get(p + 1)?]) as usize;
    p += 2 + cs_len;
    let comp_len = *hs.get(p)? as usize;
    p += 1 + comp_len;
    let ext_total = u16::from_be_bytes([*hs.get(p)?, *hs.get(p + 1)?]) as usize;
    p += 2;
    let exts = hs.get(p..p + ext_total)?;
    let mut q = 0usize;
    while q + 4 <= exts.len() {
        let etype = u16::from_be_bytes([exts[q], exts[q + 1]]);
        let elen = u16::from_be_bytes([exts[q + 2], exts[q + 3]]) as usize;
        let ebody = exts.get(q + 4..q + 4 + elen)?;
        if etype == 0x0000 {
            // server_name list: list_len(2) then entries of type(1)+len(2)+name
            let list_len = u16::from_be_bytes([*ebody.first()?, *ebody.get(1)?]) as usize;
            let list = ebody.get(2..2 + list_len)?;
            let mut r = 0usize;
            while r + 3 <= list.len() {
                let ntype = list[r];
                let nlen = u16::from_be_bytes([list[r + 1], list[r + 2]]) as usize;
                let name = list.get(r + 3..r + 3 + nlen)?;
                if ntype == 0 {
                    return std::str::from_utf8(name).ok().map(str::to_string);
                }
                r += 3 + nlen;
            }
            return None;
        }
        q += 4 + elen;
    }
    None
}
```

`crates/rurge-engine/src/lib.rs` 加 `pub mod sniff;`。

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p rurge-engine sniff::`
Expected: PASS。

- [ ] **Step 5: pump 首段客户端数据嗅探**

`crates/rurge-engine/src/relay.rs`：顶部 `use rurge_config::rule::ProtocolKind;`。在 `pump` 里加一个「首段」标志，首次客户端读到数据时嗅探：

```rust
    let mut sniffed = false;
    // ... 在循环内 client 读取的 Ok(n) 分支，写 upstream 之前：
                Ok(n) => {
                    if !sniffed {
                        sniffed = true;
                        if let Some(sni) = crate::sniff::parse_sni(&cbuf[..n]) {
                            handle.set_sni(sni);
                            handle.set_protocol(ProtocolKind::Https);
                        }
                    }
                    if let Err(e) = upstream.write_all(&cbuf[..n]).await {
                        break SessionOutcome::Failed(e.to_string());
                    }
                    handle.add_up(n as u64);
                    idle_timer.as_mut().reset(tokio::time::Instant::now() + idle);
                }
```

（明文 HTTP 转发不走 pump，因此不嗅探，符合设计。）

- [ ] **Step 6: CONNECT 隧道 SNI 记录集成测试**

`crates/rurge-engine/tests/pipeline.rs`：已有 `connect_tunnel_carries_tls_to_the_target` 用 `TestServer::spawn_tls()`。在其末尾（隧道内 TLS 握手完成、拿到响应后、连接关闭前）追加：

```rust
    // the SNI the client sent through the tunnel was recorded (poll: the record
    // may be active or just finished)
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let sni_seen = loop {
        let recent = h.engine.request_log().recent(20);
        let active = h.engine.request_log().active();
        if recent.iter().chain(active.iter()).any(|r| {
            r.sni.as_deref() == Some("tls.test")
                && r.protocol == Some(rurge_config::rule::ProtocolKind::Https)
        }) {
            break true;
        }
        if tokio::time::Instant::now() >= deadline {
            break false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert!(sni_seen, "SNI recorded");
```

（该用例里 TLS 客户端用 `ServerName` `tls.test` 发起握手；若既有用例未设置 SNI 为 `tls.test`，实施者据实调整断言的主机名，与 `spawn_tls` / 客户端 `ServerName` 一致。）

- [ ] **Step 7: 质量门与提交**

```bash
cargo test -p rurge-engine
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-inbound crates/rurge-engine
git commit -F - <<'EOF'
feat(engine): SNI 嗅探（观测用）：relay 首段解析 ClientHello 回填 sni / protocol

路由仍用 CONNECT / SOCKS 目标主机；基于 SNI 的路由随阶段 4 落地。

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 7: REJECT 自动升级（30 s / 50 次 → DROP）

同一目标主机 30 秒内触发 50 次可升级的 REJECT（`REJECT` / `REJECT-TINYGIF`）后，后续对该主机的这类拒绝按 `REJECT-DROP` 处理。`REJECT-NO-DROP` 不计入也不升级；本就是 `REJECT-DROP` 的照常。

**Files:**
- Modify: `crates/rurge-engine/src/engine.rs` —— `Escalation` 与 dial 中的应用
- Modify: `crates/rurge-engine/tests/pipeline.rs` —— 升级用例

**Interfaces:**
- Produces（引擎内部）：`Escalation::{new(), record(&str, Instant), should_drop(&str, Instant)->bool}`；引擎常量 `ESCALATE_WINDOW=30s`、`ESCALATE_COUNT=50`。

- [ ] **Step 1: `Escalation` 失败测试**

`engine.rs` 的 `#[cfg(test)] mod tests` 加：

```rust
    #[test]
    fn escalation_after_threshold_within_window() {
        let esc = Escalation::new();
        let t0 = std::time::Instant::now();
        for i in 0..(ESCALATE_COUNT - 1) {
            esc.record("ads.test", t0 + Duration::from_millis(i as u64));
        }
        assert!(!esc.should_drop("ads.test", t0 + Duration::from_secs(1)));
        esc.record("ads.test", t0 + Duration::from_secs(1));
        assert!(esc.should_drop("ads.test", t0 + Duration::from_secs(1)));
        // a different host is unaffected
        assert!(!esc.should_drop("other.test", t0 + Duration::from_secs(1)));
        // once the window slides past, the count decays
        assert!(!esc.should_drop("ads.test", t0 + ESCALATE_WINDOW + Duration::from_secs(1)));
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p rurge-engine escalation`
Expected: 编译失败。

- [ ] **Step 3: 实现 `Escalation`**

`engine.rs`：顶部补 `use std::collections::HashMap; use std::collections::VecDeque; use std::time::Instant;`。常量区加 `pub const ESCALATE_WINDOW: Duration = Duration::from_secs(30); pub const ESCALATE_COUNT: usize = 50; const ESCALATE_MAX_HOSTS: usize = 4096;`。

```rust
/// Per-destination sliding-window counter for REJECT auto-escalation (§7.4).
struct Escalation {
    hosts: std::sync::Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl Escalation {
    fn new() -> Escalation {
        Escalation {
            hosts: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Records one escalating reject for `host` at `now`.
    fn record(&self, host: &str, now: Instant) {
        let mut map = self.hosts.lock().expect("escalation");
        if map.len() >= ESCALATE_MAX_HOSTS && !map.contains_key(host) {
            map.retain(|_, times| {
                prune(times, now);
                !times.is_empty()
            });
        }
        let times = map.entry(host.to_string()).or_default();
        times.push_back(now);
        prune(times, now);
    }

    /// Whether `host` has reached the threshold within the window.
    fn should_drop(&self, host: &str, now: Instant) -> bool {
        let mut map = self.hosts.lock().expect("escalation");
        match map.get_mut(host) {
            Some(times) => {
                prune(times, now);
                times.len() >= ESCALATE_COUNT
            }
            None => false,
        }
    }
}

fn prune(times: &mut VecDeque<Instant>, now: Instant) {
    while let Some(front) = times.front() {
        if now.duration_since(*front) >= ESCALATE_WINDOW {
            times.pop_front();
        } else {
            break;
        }
    }
}
```

`Engine` 加字段 `escalation: Escalation,`，`new` 里 `escalation: Escalation::new(),`。

- [ ] **Step 4: 在 dial 中应用升级**

`engine.rs` 的 `dial`，把 `Err(OutboundError::Reject(kind))` 分支改为：

```rust
                Err(OutboundError::Reject(kind)) => {
                    let effective = if kind.escalates() {
                        let host = handle.session().dst_host.to_string();
                        let now = Instant::now();
                        self.escalation.record(&host, now);
                        if self.escalation.should_drop(&host, now) {
                            rurge_proto::RejectKind::Drop
                        } else {
                            kind
                        }
                    } else {
                        kind
                    };
                    reject(handle, effective)
                }
```

- [ ] **Step 5: 升级集成用例**

`pipeline.rs` 末尾（用 `Engine::dial` 直接驱动，避免起 50 条真实连接）：

```rust
#[tokio::test]
async fn repeated_rejects_escalate_to_drop() {
    let h = harness("", "DOMAIN,ads.test,REJECT", OutboundMode::Rule).await;
    let host = rurge_config::HostName::parse("ads.test");
    // the first ESCALATE_COUNT-1 rejects stay REJECT
    for _ in 0..(rurge_engine::engine::ESCALATE_COUNT - 1) {
        match h.engine.dial(SessionInfo::tcp(host.clone(), 80)).await {
            Err(DialError::Reject { kind, .. }) => {
                assert_eq!(kind, rurge_proto::RejectKind::Reject)
            }
            _ => panic!("expected a reject before the threshold"),
        }
    }
    // the next one crosses the threshold → DROP
    match h.engine.dial(SessionInfo::tcp(host.clone(), 80)).await {
        Err(DialError::Reject { kind, .. }) => assert_eq!(kind, rurge_proto::RejectKind::Drop),
        _ => panic!("expected a drop after escalation"),
    }
}
```

（若 `Engine::engine::ESCALATE_COUNT` 路径不可达，实施者把常量 `pub` 导出或在测试里用字面 50 并加注释。为可读性，`engine.rs` 已把两个常量设为 `pub const`；测试用 `rurge_engine::engine::ESCALATE_COUNT`。`mod engine` 需在 `lib.rs` 为 `pub mod engine;`——现状即是。）

- [ ] **Step 6: 质量门与提交**

```bash
cargo test -p rurge-engine
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-engine
git commit -F - <<'EOF'
feat(engine): REJECT 自动升级：同主机 30 s 内 50 次可升级拒绝后转 DROP

REJECT-NO-DROP 不计入也不升级。

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 8: CONNECT 连接失败回 502

M3a 时 CONNECT 的连接失败直接关闭；M3b 在 `show-error-page = true` 时回 502 + rurge 错误页（与明文转发一致）。REJECT 仍关闭，REJECT-DROP 仍保持。

**Files:**
- Modify: `crates/rurge-inbound/src/http.rs` —— `connect` 的失败分支
- Modify: `crates/rurge-engine/tests/pipeline.rs` —— CONNECT 502 用例

- [ ] **Step 1: 集成失败测试**

`pipeline.rs` 的 `dns_failure_and_ip_rules` 末尾追加（`nx.test` 无记录 → DNS 失败）：

```rust
    // CONNECT to a name that fails DNS → 502 error page (M3b)
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    let (head, body) = http_exchange(&mut s, "CONNECT nx.test:443 HTTP/1.1\r\n\r\n").await;
    assert!(head.starts_with("HTTP/1.1 502"), "{head}");
    assert!(String::from_utf8_lossy(&body).contains("DNS lookup failed"));
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p rurge-engine --test pipeline dns_failure`
Expected: 断言失败（当前 CONNECT 失败是关闭，`head` 为空）。

- [ ] **Step 3: 实现 CONNECT 502**

`crates/rurge-inbound/src/http.rs` 的 `connect`：把末尾

```rust
        Err(DialError::Reject { .. }) | Err(DialError::Failed { .. }) => Err(HandlerError::Close),
```

拆成：

```rust
        Err(DialError::Reject { .. }) => Err(HandlerError::Close),
        Err(DialError::Failed {
            kind,
            message,
            handle,
            ..
        }) => {
            let what = match kind {
                FailKind::Dns => "DNS lookup failed",
                FailKind::Timeout => "Connection timed out",
                FailKind::Connect | FailKind::Other => "Connection failed",
            };
            failure_response(&ctx, &handle, &format!("{what}: {message}"))
        }
```

（`failure_response`、`FailKind` 已在本文件作用域内；`failure_response` 在 `show_error_page = false` 时返回 `Err(HandlerError::Close)`，与 §8 一致。DROP 分支在上面已单独处理。）

同文件既有用例 `rejected_connect_closes_without_a_response` 里 `CONNECT fail.test:443` 原断言 `head.is_empty()`；该监听器用 `ListenerOpts::default()`（`show_error_page = true`），现在应改为 `assert!(head.starts_with("HTTP/1.1 502"), "{head}")`。REJECT / DROP 各分支的断言不变。

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p rurge-engine --test pipeline dns_failure`
Expected: PASS。

- [ ] **Step 5: 质量门与提交**

```bash
cargo test -p rurge-inbound -p rurge-engine
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-inbound crates/rurge-engine
git commit -F - <<'EOF'
feat(inbound): CONNECT 连接失败在 show-error-page 时回 502 错误页

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 9: 热重载核心（`swap_runtime` + 监听地址差异）

引擎提供原子换代：传入一个新构建的 `Runtime`，`ArcSwap::store` 换入，返回监听地址集合是否变化（调用方据此决定是否重建监听器）。**M3b 简化**：每次重载重建整个 `Stack`（含解析器），DNS 缓存清空、上游重连；「配置未变时复用解析器」的优化留待后续（登记进兼容性清单）。

**Files:**
- Create: `crates/rurge-engine/src/reload.rs` —— `listen_addrs`、`Engine::swap_runtime`
- Modify: `crates/rurge-engine/src/lib.rs` —— `mod reload;`
- Modify: `crates/rurge-engine/tests/pipeline.rs` —— 换代用例

**Interfaces:**
- Produces: `Engine::swap_runtime(&self, next: Runtime) -> bool`（返回监听地址集合是否变化）。

- [ ] **Step 1: 写换代失败测试**

`pipeline.rs` 末尾（复用 Task 1 的 `build_runtime`，空闲超时传 600 s）：

```rust
#[tokio::test]
async fn reload_swaps_rules_without_changing_listeners() {
    let h = harness("", "DOMAIN,ads.test,REJECT", OutboundMode::Rule).await;
    // ads.test currently rejects (closes)
    let (head, _) = get_via_proxy(h.http(), "http://ads.test/").await;
    assert!(head.is_empty());
    // reload with a config that instead serves a tinygif for ads.test, same listen addrs
    let next = build_runtime(h._dir.path(), &h.dns, "", "DOMAIN,ads.test,Block", OutboundMode::Rule, Duration::from_secs(600)).await;
    let changed = h.engine.swap_runtime(next);
    assert!(!changed, "listen addrs unchanged");
    let (head, body) = get_via_proxy(h.http(), "http://ads.test/ad.gif").await;
    assert!(head.starts_with("HTTP/1.1 200") && body.len() == 43, "{head}");
    // reload with a different http-listen address set → changed = true
    let next = build_runtime(h._dir.path(), &h.dns, "http-listen = 127.0.0.1:1\nsocks5-listen = 127.0.0.1:0", "", OutboundMode::Rule, Duration::from_secs(600)).await;
    assert!(h.engine.swap_runtime(next), "listen addr set changed");
}
```

（`Harness` 与测试同在一个模块，`h._dir` / `h.dns` 可直接访问，无需改可见性。）

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p rurge-engine --test pipeline reload_swaps`
Expected: 编译失败（`swap_runtime` 未定义）。

- [ ] **Step 3: 实现 `swap_runtime` 与 `listen_addrs`**

新建 `crates/rurge-engine/src/reload.rs`：

```rust
//! Hot reload (M3 design §7.4): atomically swap in a new config generation.
//! Building the new `Runtime` (validating config, rebuilding the stack) is the
//! caller's job; the engine only swaps and reports whether listeners must be
//! rebound.

use crate::engine::Engine;
use crate::runtime::Runtime;
use rurge_config::general::General;
use rurge_config::session::ListenerKind;
use std::collections::BTreeSet;
use std::net::SocketAddr;

/// The set of (kind, address) a config asks listeners to bind, in the same
/// derivation as `Engine::listener_specs`.
pub(crate) fn listen_addrs(general: &General) -> BTreeSet<(u8, SocketAddr)> {
    Engine::listener_specs(general)
        .into_iter()
        .map(|s| {
            let tag = match s.kind {
                ListenerKind::Http => 0u8,
                ListenerKind::Socks5 => 1,
                ListenerKind::Tun => 2,
                ListenerKind::Forward => 3,
                ListenerKind::Internal => 4,
            };
            (tag, s.addr)
        })
        .collect()
}

impl Engine {
    /// Atomically swaps in the next config generation. In-flight sessions keep
    /// their snapshot; new sessions use the new one. Returns whether the set of
    /// listen addresses changed, so the caller can rebind listeners.
    pub fn swap_runtime(&self, next: Runtime) -> bool {
        let before = listen_addrs(&self.runtime().config.general);
        let after = listen_addrs(&next.config.general);
        self.store_runtime(next);
        before != after
    }
}
```

`Engine` 的 `runtime: ArcSwap<Runtime>` 是私有字段，`reload.rs` 与 `engine.rs` 同 crate。给 `engine.rs` 的 `impl Engine` 加一个 `pub(crate) fn store_runtime(&self, next: Runtime) { self.runtime.store(std::sync::Arc::new(next)); }`。`crates/rurge-engine/src/lib.rs` 加 `mod reload;`（`listen_addrs` 是 `pub(crate)`，`swap_runtime` 经 `impl Engine` 公开）。

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p rurge-engine --test pipeline reload_swaps`
Expected: PASS。

- [ ] **Step 5: 提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-engine
git commit -F - <<'EOF'
feat(engine): 热重载核心：swap_runtime 原子换代并报告监听地址是否变化

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 10: 重载触发（SIGHUP、`--watch`）与 `rurge run` 主循环

`rurge run` 的主循环从「等 Ctrl-C」改为 select：Ctrl-C → 优雅退出；SIGHUP（Unix）或 `--watch` 监视到主配置 / include 变化（去抖 500 ms）→ 从磁盘重载。重载失败保留旧配置并 WARN；成功则 `swap_runtime`，监听地址变化时重建监听器；记一条 INFO。

**Files:**
- Modify: `crates/rurge/src/cli/run.rs` —— 重构主循环、`build_engine_runtime` 辅助、`reload` 辅助、`--watch`
- Modify: `crates/rurge/src/cli/runtime.rs` —— `--watch`
- Modify: `crates/rurge/Cargo.toml` —— 加 `notify`（`--watch`）
- Modify: `crates/rurge/tests/cli.rs` —— `--watch` 重载用例

**Interfaces:**
- Consumes: `Engine::{swap_runtime, bind_listeners, stop_accepting, tracker, cancel_sessions}`、`Runtime::build`、`load`。

- [ ] **Step 1: 加 `--watch` 参数与 `notify` 依赖**

`crates/rurge/Cargo.toml` 的 `[dependencies]` 加 `notify.workspace = true` 与 `tracing.workspace = true`（`run.rs` 的重载 / 关停路径要直接打 `tracing::info!`，bin 此前只依赖 `tracing-subscriber`）。`crates/rurge/src/cli/run.rs` 的 `RunArgs` 加（run 专属）：

```rust
    /// Reload the profile when it or its included files change on disk
    #[arg(long, env = "RURGE_WATCH")]
    pub watch: bool,
```

- [ ] **Step 2: 把「构建引擎 Runtime」抽成可复用辅助**

`crates/rurge/src/cli/run.rs`：把启动时构建 `RuntimeOptions` + `Runtime::build` 的逻辑抽成一个函数，重载时复用：

```rust
/// The run-only knobs (from `RunArgs`) a reload has to carry over unchanged.
struct RunOptions {
    idle_timeout: Duration,
    request_log_size: usize,
}

async fn build_engine_runtime(
    cfg: rurge_config::Config,
    rt: &super::runtime::Runtime,
    run_opts: &RunOptions,
    outbound_mode: OutboundMode,
) -> anyhow::Result<Runtime> {
    let state = State::load(&rt.data_dir.join(STATE_FILE));
    let selections = state.selections_for(&profile_key(&cfg.source.main));
    Runtime::build(
        cfg,
        RuntimeOptions {
            stack: rt.stack_options(Duration::ZERO),
            outbound_mode,
            selections,
            idle_timeout: run_opts.idle_timeout,
            request_log_size: run_opts.request_log_size,
        },
    )
    .await
    .context("cannot build the runtime")
}
```

- [ ] **Step 3: 重载辅助（从磁盘加载 → 校验 → 换代 → 按需重建监听器）**

`run.rs` 加：

```rust
async fn reload(
    engine: &std::sync::Arc<Engine>,
    args_config: &std::path::Path,
    load_opts: &LoadOptions,
    rt: &super::runtime::Runtime,
    run_opts: &RunOptions,
    outbound_mode: &OutboundMode,
    listeners: &mut Vec<(rurge_engine::ListenerSpec, rurge_engine::Running)>,
) {
    let loaded = match load(args_config, load_opts) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: reload failed, keeping current config: {e}");
            return;
        }
    };
    if loaded.diagnostics.has_errors() {
        eprintln!("reload failed, keeping current config:");
        print_diagnostics(&loaded.diagnostics.sorted());
        return;
    }
    print_diagnostics(&loaded.diagnostics.sorted());
    let next = match build_engine_runtime(loaded.config, rt, run_opts, outbound_mode.clone()).await {
        Ok(n) => n,
        Err(e) => {
            eprintln!("error: reload failed, keeping current config: {e}");
            return;
        }
    };
    print_diagnostics(next.diagnostics());
    let changed = engine.swap_runtime(next);
    if changed {
        // Stop the old accept loops, wait until their sockets are closed (so the
        // addresses can be re-bound; Windows has no SO_REUSEADDR by default),
        // then let their in-flight sessions drain in the background. Dropping
        // the old `Running`s instead would abort those sessions.
        let olds: Vec<rurge_engine::Running> = listeners.drain(..).map(|(_, r)| r).collect();
        for old in &olds {
            old.stop();
        }
        for old in &olds {
            let _ = tokio::time::timeout(Duration::from_secs(2), old.wait_closed()).await;
        }
        for old in olds {
            tokio::spawn(old.join());
        }
        match engine.bind_listeners().await {
            Ok(new_listeners) => {
                *listeners = new_listeners;
                for (spec, running) in listeners.iter() {
                    let scheme = if spec.kind == ListenerKind::Socks5 { "socks5" } else { "http" };
                    println!("listening on {scheme}://{}", running.local_addr);
                }
            }
            Err(e) => eprintln!("error: reload rebound listeners failed: {e}"),
        }
    }
    tracing::info!("profile reloaded");
}
```

- [ ] **Step 4: 主循环重构**

`run.rs` 把 `Engine::new` 之后到函数结尾整段替换为主循环 + 优雅退出。要点：`listeners` 可变；`load_opts` 与 `outbound_mode` 提前克隆；Unix 下建 SIGHUP 流，非 Unix 为 `None`；`--watch` 时用 `notify` 起监视线程把事件送进 `tokio::sync::mpsc`（去抖 500 ms）。

```rust
        let engine = Engine::new(engine_rt);
        engine.start_sampler();
        let mut listeners = match engine.bind_listeners().await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("error: cannot bind listener: {e}");
                return Ok(ExitCode::from(1));
            }
        };
        for (spec, running) in &listeners {
            let scheme = if spec.kind == ListenerKind::Socks5 { "socks5" } else { "http" };
            println!("listening on {scheme}://{}", running.local_addr);
        }
        println!(
            "rurge {} running: {policies} policies, {rules} rules, outbound mode {}",
            env!("CARGO_PKG_VERSION"),
            mode_name(&outbound_mode)
        );

        // reload triggers. `reload_tx` stays alive here on purpose: if every
        // sender were dropped, `recv()` would return `None` immediately and the
        // loop below would spin reloading.
        let (reload_tx, mut reload_rx) = tokio::sync::mpsc::channel::<()>(1);
        let _watcher = if args.watch {
            Some(spawn_watcher(&cfg_paths, reload_tx.clone())?)
        } else {
            None
        };
        #[cfg(unix)]
        let mut sighup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
            .context("cannot listen for SIGHUP")?;
        #[cfg(unix)]
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .context("cannot listen for SIGTERM")?;

        loop {
            // Ctrl-C everywhere; SIGTERM as well on Unix (design §7.4).
            let shutdown_signal = async {
                #[cfg(unix)]
                {
                    tokio::select! {
                        _ = tokio::signal::ctrl_c() => {}
                        _ = sigterm.recv() => {}
                    }
                }
                #[cfg(not(unix))]
                {
                    let _ = tokio::signal::ctrl_c().await;
                }
            };
            let reload_signal = async {
                #[cfg(unix)]
                {
                    tokio::select! {
                        _ = sighup.recv() => {}
                        _ = reload_rx.recv() => {}
                    }
                }
                #[cfg(not(unix))]
                {
                    reload_rx.recv().await;
                }
            };
            tokio::select! {
                _ = shutdown_signal => break,
                _ = reload_signal => {
                    reload(&engine, &args.config, &opts, &rt, &run_opts, &outbound_mode, &mut listeners).await;
                }
            }
        }

        // graceful shutdown (Task 2)
        println!("shutting down (Ctrl-C again to exit now)");
        engine.stop_accepting();
        engine.tracker().close();
        let running: Vec<_> = std::mem::take(&mut listeners).into_iter().map(|(_, r)| r).collect();
        let drain = async {
            for r in running {
                r.join().await;
            }
            engine.tracker().wait().await;
        };
        tokio::select! {
            _ = drain => {}
            _ = tokio::signal::ctrl_c() => { println!("forced shutdown"); }
            _ = tokio::time::sleep(GRACE) => {
                println!("grace period elapsed; closing active sessions");
                engine.cancel_sessions();
                let _ = tokio::time::timeout(Duration::from_secs(1), engine.tracker().wait()).await;
            }
        }
        Ok(ExitCode::SUCCESS)
```

其中：
- `opts`（`LoadOptions`）在 `load` 之后仍需保留给重载用；把启动处的 `let opts = LoadOptions {...}` 保留其所有权，不要在 `load(&args.config, &opts)?` 之后被移动（`load` 借用 `&opts`，OK）。
- `outbound_mode` 已在启动处 `let outbound_mode = args.outbound_mode.clone();`。`run_opts` 在启动处从 `RunArgs` 算一次：`let run_opts = RunOptions { idle_timeout: Duration::from_secs(args.idle_timeout.unwrap_or(600).max(1)), request_log_size: args.request_log_size.unwrap_or(1000).max(1) };`，启动构建与重载都经 `build_engine_runtime(cfg, &rt, &run_opts, mode)`（Task 1 / Task 5 里直接构造 `RuntimeOptions` 的代码在此收口）。
- `cfg_paths`：主配置 + include 列表，在构建 `Runtime` 之前从 `cfg.source` 取（`cfg` 在构建 `Runtime` 时被移动，所以先 `let cfg_paths: Vec<PathBuf> = std::iter::once(cfg.source.main.clone()).chain(cfg.source.includes.iter().cloned()).collect();`）。
- `rt`（bin `Runtime`）需在闭包外仍可借用——它是 `resolve` 的结果，`Clone`，按引用传给 `reload`。

`spawn_watcher`（`notify` + 去抖）：

```rust
fn spawn_watcher(
    paths: &[std::path::PathBuf],
    tx: tokio::sync::mpsc::Sender<()>,
) -> anyhow::Result<notify::RecommendedWatcher> {
    use notify::{RecursiveMode, Watcher};
    let (raw_tx, raw_rx) = std::sync::mpsc::channel::<()>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            let _ = raw_tx.send(());
        }
    })?;
    for p in paths {
        // Watch the parent directory so an editor's rename/replace is caught.
        // Canonicalize first: a relative `-c t.conf` has an empty parent.
        let full = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
        let dir = full
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        if let Err(e) = watcher.watch(&dir, RecursiveMode::NonRecursive) {
            eprintln!("warning: cannot watch {}: {e}", dir.display());
        }
    }
    // debounce: collapse a burst of events into one reload every 500 ms
    std::thread::spawn(move || {
        while raw_rx.recv().is_ok() {
            // drain the burst
            while raw_rx.recv_timeout(std::time::Duration::from_millis(500)).is_ok() {}
            if tx.blocking_send(()).is_err() {
                break;
            }
        }
    });
    Ok(watcher)
}
```

（返回 `RecommendedWatcher` 由 `_watcher` 持有以保活；丢弃即停止监视。）

- [ ] **Step 5: `--watch` 重载 CLI 测试（确定性）**

关键设计：两份配置的 `http-listen` / `socks5-listen` **都保持 `127.0.0.1:0`**，于是 `listen_addrs` 前后相等 → 不重建监听器 → 启动时读到的端口在重载后仍然有效。请求用 **IP 字面量** 目标（`http://127.0.0.1:<targetport>/`，无需 DNS），初始规则 `IP-CIDR,127.0.0.0/8,REJECT`（拒绝、空响应），改写为 `FINAL,DIRECT`（直连到 `TestServer`）。轮询启动端口直到放行。实施者据 `spawn_daemon` 扩展 `spawn_daemon_watching(conf, data)`（追加 `--watch`）。

```rust
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watch_reloads_rules_on_change() {
        let target = TestServer::spawn().await;
        target.set("/hello", "reloaded");
        let tport = target.url("/").port().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join("t.conf");
        // initial: reject loopback targets (listen addrs are :0, unchanged across reload)
        std::fs::write(&conf,
            "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nloglevel = warning\n[Rule]\nIP-CIDR,127.0.0.0/8,REJECT\nFINAL,DIRECT\n"
        ).unwrap();
        let daemon = tokio::task::spawn_blocking({
            let conf = conf.clone();
            let data = dir.path().join("data");
            move || spawn_daemon_watching(&conf, &data)
        }).await.unwrap();
        let http = daemon.http;
        // before reload: rejected -> empty response
        let before = tokio::task::spawn_blocking(move || http_get(http, &format!("http://127.0.0.1:{tport}/hello")))
            .await.unwrap();
        assert!(!before.contains("reloaded"), "rejected before reload: {before}");
        // rewrite: allow everything (same listen addrs -> no rebind -> same port)
        std::fs::write(&conf,
            "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nloglevel = warning\n[Rule]\nFINAL,DIRECT\n"
        ).unwrap();
        let ok = tokio::task::spawn_blocking(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(8);
            loop {
                if http_get(http, &format!("http://127.0.0.1:{tport}/hello")).contains("reloaded") {
                    return true;
                }
                if std::time::Instant::now() > deadline { return false; }
                std::thread::sleep(Duration::from_millis(200));
            }
        }).await.unwrap();
        drop(daemon);
        assert!(ok, "reload did not take effect");
    }
```

（本用例直接 `std::fs::write` 覆盖 `conf` 触发重载；`spawn_daemon_watching` 只负责起进程（追加 `--watch`）并读端口。`http_get` 已在 `mod run` 中。SIGHUP 触发在 Unix 上等价，难以三平台稳定测，靠本 `--watch` 用例代表热重载路径。）

- [ ] **Step 6: 质量门与提交**

```bash
cargo test -p rurge
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add Cargo.lock crates/rurge
git commit -F - <<'EOF'
feat(cli): 热重载触发（SIGHUP / --watch）与 rurge run 主循环

主循环 select：Ctrl-C 优雅退出；SIGHUP（Unix）或 --watch 监视到配置变化
（去抖 500 ms）则从磁盘重载，失败保留旧配置，监听地址变化时重建监听器。

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 11: `--log-file` 按天滚动

`--log-file <path>` 时在 stdout 之外再写一个按天滚动、保留 7 个的日志文件。

**Files:**
- Modify: `Cargo.toml`（workspace）—— 加 `tracing-appender`
- Modify: `crates/rurge/Cargo.toml` —— 加 `tracing-appender`
- Modify: `crates/rurge/src/cli/runtime.rs` —— `--log-file`
- Modify: `crates/rurge/src/cli/run.rs` —— `init_logging` 加文件层
- Modify: `crates/rurge/tests/cli.rs` —— 日志文件生成用例

**Interfaces:**
- Produces: `--log-file` / `RURGE_LOG_FILE`；`init_logging(level, Option<&Path>) -> Option<tracing_appender::non_blocking::WorkerGuard>`。

- [ ] **Step 1: 加依赖与参数**

`Cargo.toml`（workspace `[workspace.dependencies]`）加 `tracing-appender = "0.2"`。`crates/rurge/Cargo.toml` 的 `[dependencies]` 加 `tracing-appender.workspace = true`。`run.rs` 的 `RunArgs` 加（run 专属）：

```rust
    /// Also write logs to this file, rotated daily (7 kept)
    #[arg(long, env = "RURGE_LOG_FILE", value_name = "PATH")]
    pub log_file: Option<PathBuf>,
```

- [ ] **Step 2: `init_logging` 加文件层**

`run.rs`：改 `init_logging` 签名与实现（用分层 registry；返回非阻塞写入的 `WorkerGuard`，`run()` 全程持有它，否则缓冲不刷新）：

```rust
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

fn init_logging(
    level: LevelFilter,
    log_file: Option<&std::path::Path>,
) -> anyhow::Result<Option<tracing_appender::non_blocking::WorkerGuard>> {
    let stdout_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_ansi(std::io::stdout().is_terminal());
    let registry = tracing_subscriber::registry().with(level).with(stdout_layer);
    match log_file {
        Some(path) => {
            let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| std::path::Path::new("."));
            let prefix = path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "rurge.log".to_string());
            let appender = tracing_appender::rolling::Builder::new()
                .rotation(tracing_appender::rolling::Rotation::DAILY)
                .filename_prefix(prefix)
                .max_log_files(7)
                .build(dir)
                .with_context(|| format!("cannot open log file {}", path.display()))?;
            let (nb, guard) = tracing_appender::non_blocking(appender);
            let file_layer = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(nb);
            let _ = registry.with(file_layer).try_init();
            Ok(Some(guard))
        }
        None => {
            let _ = registry.try_init();
            Ok(None)
        }
    }
}
```

在 `run()` 里改调用点：`let _log_guard = init_logging(args.log_level.unwrap_or_else(|| level_for(&cfg.general.loglevel)), args.log_file.as_deref())?;`（`anyhow::Context` 已导入），并让 `_log_guard` 活到 `run()` 结束（放在 `runtime.block_on` 之前的作用域，保证异步块跑完前不被丢弃）。

- [ ] **Step 3: CLI 用例**

`crates/rurge/tests/cli.rs` 的 `mod run`：起带 `--log-file <dir>/rurge.log` 与 `loglevel = notify`（INFO 可见）的 daemon；启动时 `bind_listeners` 的 `tracing::info!("listening")` 就会写进文件。轮询断言日志目录里出现以 `rurge.log` 为前缀且长度 > 0 的文件（按天滚动的文件名形如 `rurge.log.2026-09-06`）。

```rust
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn log_file_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), "http-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nloglevel = notify");
        let logdir = dir.path().join("logs");
        std::fs::create_dir_all(&logdir).unwrap();
        let log = logdir.join("rurge.log");
        let daemon = tokio::task::spawn_blocking({
            let conf = conf.clone();
            let data = dir.path().join("data");
            let log = log.clone();
            move || spawn_daemon_with_log(&conf, &data, &log)
        }).await.unwrap();
        // give the non-blocking appender a moment, then check the dir has a file
        let ok = tokio::task::spawn_blocking(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                if let Ok(rd) = std::fs::read_dir(&logdir) {
                    if rd.filter_map(|e| e.ok()).any(|e| {
                        e.file_name().to_string_lossy().starts_with("rurge.log")
                            && e.metadata().map(|m| m.len() > 0).unwrap_or(false)
                    }) {
                        return true;
                    }
                }
                if std::time::Instant::now() > deadline { return false; }
                std::thread::sleep(Duration::from_millis(150));
            }
        }).await.unwrap();
        drop(daemon);
        assert!(ok, "no non-empty rurge.log* file was written");
    }
```

（实施者加 `spawn_daemon_with_log(conf, data, log)`：在 `spawn_daemon` 基础上追加 `--log-file <log>`。）

- [ ] **Step 4: 质量门与提交**

```bash
cargo test -p rurge
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add Cargo.toml Cargo.lock crates/rurge
git commit -F - <<'EOF'
feat(cli): --log-file 按天滚动（保留 7 个），与 stdout 并行

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 12: `encrypted-dns-follow-outbound-mode`

`encrypted-dns-follow-outbound-mode = true` 时，DNS 上游的 TCP / DoT / DoH 连接经 dial 流水线出站（成为 `Internal` 会话，`PROTOCOL,DOH/DOT/DNS` 可匹配、进请求记录）。**防环机制（与设计 §7.4 一致，已按代码核实）**：`Resolver::new` 把 `deps.connector` 包在 `BootstrapConnector` 里（`resolver.rs` ~301 行）；上游若以域名配置，由 Bootstrap（纯 UDP，不进规则引擎）先解析成 IP，再以 **IP 目标** 调内层连接器（`bootstrap.rs` 的 `connect`）。因此 `PipelineConnector` 收到的永远是 IP 字面量目标——规则匹配对 IP 会话不触发解析、`DIRECT` 对 IP 目标不解析——流水线里任何环节都无需再解析，天然无环；域名配置的 DoH/DoT 服务器同样走流水线。`PipelineConnector` 里的 `Domain` 分支只是保险丝，正常路径到不了。**保底**：DNS 会话若被规则路由到 REJECT / 未实现策略，则告警并直连以保证 DNS 不被打断（设计 §7.4「回退 DIRECT」）。UDP DNS 上游不经连接器，故不受影响（与手册一致，登记）。**局限（登记）**：协议标签按端口启发（853→DoT、443→DoH、其余→DNS）；DNS 会话按 IP 匹配规则，域名规则不会匹配到上游主机名。

**Files:**
- Create: `crates/rurge-engine/src/dns_pipeline.rs` —— `PipelineConnector`、`FinishOnDrop`、`Engine::dial_internal`
- Modify: `crates/rurge-engine/src/lib.rs` —— `pub mod dns_pipeline;`
- Modify: `crates/rurge-engine/src/stack.rs` —— `StackOptions.dns_connector`
- Modify: `crates/rurge-engine/src/runtime.rs` —— `Runtime::build` 按开关建 `PipelineConnector`，`Runtime` 持有它
- Modify: `crates/rurge-engine/src/engine.rs` —— `Engine::new`/`swap_runtime` attach；`dial_internal`
- Modify: `crates/rurge/src/cli/runtime.rs`、`pipeline.rs` —— 各 `StackOptions` 补 `dns_connector: None`
- Modify: `crates/rurge-engine/tests/pipeline.rs` —— TCP MockDns 走流水线的集成用例

**Interfaces:**
- Produces:
  - `PipelineConnector::new(fallback: Arc<dyn Connector>) -> Arc<PipelineConnector>`、`attach(&self, engine: Weak<Engine>)`
  - `StackOptions.dns_connector: Option<Arc<dyn Connector>>`
  - `Runtime::dns_pipeline(&self) -> Option<&Arc<PipelineConnector>>`
  - `Engine::dial_internal(&self, session: SessionInfo, fallback: &Arc<dyn Connector>) -> io::Result<BoxedStream>`

- [ ] **Step 1: `StackOptions.dns_connector` + build_stack 用它**

`crates/rurge-engine/src/stack.rs`：`StackOptions` 加 `pub dns_connector: Option<Arc<dyn Connector>>,`（`use rurge_net::connector::Connector;`）。`build_stack_with` 里，资源管理器仍用本地 `connector`（DIRECT），但 `ResolverDeps.connector` 改为：

```rust
    // `connector` is `Arc<DirectConnector>`; coerce explicitly so both arms unify.
    let resolver_connector: Arc<dyn Connector> = match &opts.dns_connector {
        Some(c) => c.clone(),
        None => connector.clone() as Arc<dyn Connector>,
    };
    // ... 构建 resolver 时：
    let (resolver, dns_diags) = Resolver::new(
        resolver_cfg,
        ResolverDeps {
            connector: resolver_connector,
            sets: registry.clone(),
            system: opts.system.clone(),
            resources: resources.clone(),
        },
    );
```

更新所有 `StackOptions { ... }` 构造点补 `dns_connector: None,`：`crates/rurge/src/cli/runtime.rs::stack_options`、`crates/rurge-engine/tests/pipeline.rs`（`harness` 与 `build_runtime` 两处，及 Step 5 新增处）。

- [ ] **Step 2: `PipelineConnector` 与 `FinishOnDrop`（先写失败测试）**

新建 `crates/rurge-engine/src/dns_pipeline.rs`，先放测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rurge_net::connector::{ConnectOpts, DirectConnector, SystemResolve, Target};
    use rurge_config::HostName;
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn without_an_engine_it_delegates_to_the_fallback() {
        let fallback: Arc<dyn Connector> = Arc::new(DirectConnector::new(Arc::new(SystemResolve)));
        let pc = PipelineConnector::new(fallback);
        // no engine attached yet → even an IP target goes to the fallback; port 9 is
        // closed on loopback so the result is an io error, never a panic or a hang
        let target = Target::new(HostName::parse("127.0.0.1"), 9);
        let res = tokio::time::timeout(Duration::from_secs(5), pc.connect(&target, &ConnectOpts::default())).await;
        assert!(matches!(res, Ok(Err(_))), "fallback direct connect fails cleanly: {res:?}");
    }
}
```

- [ ] **Step 3: 运行确认失败**

Run: `cargo test -p rurge-engine dns_pipeline::`
Expected: 编译失败。

- [ ] **Step 4: 实现 `dns_pipeline.rs`**

```rust
//! `encrypted-dns-follow-outbound-mode` (M3 design §7.4): a connector that
//! routes DNS upstream connections through the dial pipeline. Only IP-literal
//! DNS servers take the pipeline — a domain DNS server would need resolving,
//! which is exactly the loop we must not create, so those fall back to the
//! bootstrap-direct connector. UDP upstreams never use a connector.

use crate::engine::Engine;
use rurge_config::HostName;
use rurge_config::rule::ProtocolKind;
use rurge_config::session::{ListenerKind, SessionInfo, Transport};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rurge_inbound::{Counting, SessionHandle, SessionOutcome};
use std::io;
use std::pin::Pin;
use std::sync::{Arc, OnceLock, Weak};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub struct PipelineConnector {
    engine: OnceLock<Weak<Engine>>,
    /// Plain direct connector: the reject-bypass path (targets are already IPs)
    /// and the defensive non-IP path.
    fallback: Arc<dyn Connector>,
}

impl PipelineConnector {
    pub fn new(fallback: Arc<dyn Connector>) -> Arc<PipelineConnector> {
        Arc::new(PipelineConnector {
            engine: OnceLock::new(),
            fallback,
        })
    }

    /// Binds the engine after it is built (idempotent; first wins).
    pub fn attach(&self, engine: Weak<Engine>) {
        let _ = self.engine.set(engine);
    }

    pub fn fallback(&self) -> &Arc<dyn Connector> {
        &self.fallback
    }
}

/// DNS server port → the protocol tag its session carries (best-effort).
fn protocol_for(port: u16) -> ProtocolKind {
    match port {
        853 => ProtocolKind::Dot,
        443 => ProtocolKind::Doh,
        _ => ProtocolKind::Dns,
    }
}

impl Connector for PipelineConnector {
    fn connect<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedStream>> {
        Box::pin(async move {
            let ip = match &target.host {
                HostName::Ip(ip) => *ip,
                // Defensive only: the resolver's BootstrapConnector resolves
                // upstream names itself and hands us IP targets. Anything else
                // must not enter the pipeline (it could recurse), so go direct.
                HostName::Domain(_) => return self.fallback.connect(target, opts).await,
            };
            let engine = self.engine.get().and_then(Weak::upgrade);
            let Some(engine) = engine else {
                // Not attached yet (bootstrap phase) → direct.
                return self.fallback.connect(target, opts).await;
            };
            let mut session = SessionInfo::tcp(HostName::Ip(ip), target.port);
            session.listener = ListenerKind::Internal;
            session.transport = Transport::Tcp;
            session.protocol = Some(protocol_for(target.port));
            engine.dial_internal(session, &self.fallback).await
        })
    }
}

/// Finishes the session handle when the DNS connection is dropped, so the
/// internal session leaves the active index and lands in the request log.
pub(crate) struct FinishOnDrop<S> {
    inner: S,
    handle: Arc<SessionHandle>,
}

impl<S> FinishOnDrop<S> {
    pub(crate) fn new(inner: S, handle: Arc<SessionHandle>) -> FinishOnDrop<S> {
        FinishOnDrop { inner, handle }
    }
}

impl<S> Drop for FinishOnDrop<S> {
    fn drop(&mut self) {
        self.handle.finish(SessionOutcome::Completed);
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for FinishOnDrop<S> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for FinishOnDrop<S> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, data: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, data)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Wraps a dialed stream so bytes are counted (`Counting`) and the handle is
/// finished on drop (`FinishOnDrop`).
pub(crate) fn wrap_internal(stream: BoxedStream, handle: Arc<SessionHandle>) -> BoxedStream {
    Box::new(FinishOnDrop::new(Counting::new(stream, handle.clone()), handle))
}
```

`crates/rurge-engine/src/lib.rs` 加 `pub mod dns_pipeline;`。`rurge-engine` 的 `Cargo.toml` 已含 `rurge-net`、`rurge-inbound`、`rurge-config`。

- [ ] **Step 5: `Engine::dial_internal`**

`crates/rurge-engine/src/engine.rs` 的 `impl Dialer for Engine` 附近（或 `impl Engine`）加 `dial_internal`。它复用规则 / 模式判定，出站失败按类型上抛，被 REJECT 时告警并用 `fallback` 直连以保 DNS 不断：

```rust
impl Engine {
    /// Dials a DNS upstream connection through the pipeline (Internal session).
    /// Never lets a REJECT break DNS: on reject it warns and connects directly.
    pub async fn dial_internal(
        &self,
        session: SessionInfo,
        fallback: &Arc<dyn Connector>,
    ) -> io::Result<BoxedStream> {
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
                    // An IP-literal DNS session never needs resolution; a DnsFailed
                    // here would only come from a misconfigured rule → direct.
                    Outcome::DnsFailed => PolicyRef::Builtin(Builtin::Direct),
                }
            }
        };
        let resolution = rt.policies.resolve(&policy);
        handle.set_policy_chain(resolution.chain.clone());
        let target = Target::new(handle.session().dst_host.clone(), handle.session().dst_port);
        let opts = ConnectOpts { timeout: CONNECT_TIMEOUT, prefer_v6: rt.config.general.ipv6 };
        match resolution.outbound.connect_tcp(&target, &opts).await {
            Ok(stream) => Ok(crate::dns_pipeline::wrap_internal(stream, handle)),
            Err(OutboundError::Reject(_)) | Err(OutboundError::Unsupported(_)) => {
                tracing::warn!(dst = %target.host, "DNS session routed to a reject/unsupported policy; connecting directly to keep DNS working");
                handle.set_error("dns-follow: reject bypassed to keep DNS working");
                let stream = fallback.connect(&target, &opts).await?;
                Ok(crate::dns_pipeline::wrap_internal(stream, handle))
            }
            Err(OutboundError::Dns(m)) => {
                handle.finish(SessionOutcome::Failed(m.clone()));
                Err(io::Error::other(m))
            }
            Err(OutboundError::Io(e)) => {
                let msg = e.to_string();
                handle.finish(SessionOutcome::Failed(msg));
                Err(e)
            }
            Err(OutboundError::Timeout) => {
                handle.finish(SessionOutcome::Failed("connect timed out".into()));
                Err(io::Error::new(io::ErrorKind::TimedOut, "connect timed out"))
            }
        }
    }
}
```

`engine.rs` 顶部需要 `use rurge_net::connector::Connector;`（`connect_tcp` 是 `Outbound` 的方法，已在用）。`Builtin`、`PolicyRef`、`Outcome`、`OutboundMode`、`OutboundError`、`Target`、`ConnectOpts` 均已导入。

- [ ] **Step 6: `Runtime` 按开关建 `PipelineConnector` 并被引擎 attach**

`crates/rurge-engine/src/runtime.rs`：`Runtime` 加 `pub(crate) dns_pipeline: Option<Arc<crate::dns_pipeline::PipelineConnector>>,` 与 `pub fn dns_pipeline(&self) -> Option<&Arc<crate::dns_pipeline::PipelineConnector>> { self.dns_pipeline.as_ref() }`。`build` 改为在开关开时建连接器并塞进 stack opts：

```rust
    pub async fn build(config: Config, mut opts: RuntimeOptions) -> anyhow::Result<Runtime> {
        let dns_pipeline = if config.general.encrypted_dns_follow_outbound_mode {
            let fallback: Arc<dyn rurge_net::connector::Connector> = Arc::new(
                rurge_net::connector::DirectConnector::new(Arc::new(rurge_net::connector::SystemResolve)),
            );
            let pc = crate::dns_pipeline::PipelineConnector::new(fallback);
            opts.stack.dns_connector = Some(pc.clone());
            Some(pc)
        } else {
            None
        };
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
            idle_timeout: opts.idle_timeout,
            request_log_size: opts.request_log_size.max(1),
            dns_pipeline,
        })
    }
```

（`build` 现在需要 `mut opts`；调用点无需改。`Direct` 的 fallback 用系统解析器足够，因为流水线只处理 IP 字面量 DNS 服务器。）

`crates/rurge-engine/src/engine.rs`：`Engine::new` 结尾 attach（`Weak`）：

```rust
    pub fn new(runtime: Runtime) -> Arc<Engine> {
        let observe = Arc::new(Observe {
            log: RequestLog::new(runtime.request_log_size),
            traffic: TrafficStats::new(),
        });
        let engine = Arc::new(Engine {
            runtime: ArcSwap::from_pointee(runtime),
            next_session: AtomicU64::new(0),
            sessions_root: CancellationToken::new(),
            accept: CancellationToken::new(),
            tracker: TaskTracker::new(),
            observe,
            escalation: Escalation::new(),
        });
        if let Some(pc) = engine.runtime().dns_pipeline() {
            pc.attach(Arc::downgrade(&engine));
        }
        engine
    }
```

`swap_runtime` 改为 `self: &Arc<Self>` 并 attach 新代的连接器（`crates/rurge-engine/src/reload.rs`）：

```rust
    pub fn swap_runtime(self: &std::sync::Arc<Self>, next: Runtime) -> bool {
        let before = listen_addrs(&self.runtime().config.general);
        let after = listen_addrs(&next.config.general);
        if let Some(pc) = next.dns_pipeline() {
            pc.attach(std::sync::Arc::downgrade(self));
        }
        self.store_runtime(next);
        before != after
    }
```

- [ ] **Step 7: 集成用例（TCP DNS 走流水线）**

`crates/rurge-engine/tests/pipeline.rs` 末尾。用 `MockDns::spawn()`（TCP 与 UDP 同端口），`encrypted-dns-server = tcp://127.0.0.1:<port>`（加密 / URL 型上游拥有主上游集，确保查询走这条 TCP 上游而不是 UDP；`tcp://` 是 M2b 已支持的写法），规则 `PROTOCOL,DNS,DIRECT`，开 `encrypted-dns-follow-outbound-mode`：

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dns_upstream_follows_the_pipeline_as_an_internal_session() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let dir = tempfile::tempdir().unwrap();
    let profile = format!(
        "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\n\
encrypted-dns-follow-outbound-mode = true\nencrypted-dns-server = tcp://127.0.0.1:{}\nipv6 = false\n\
[Proxy]\n[Proxy Group]\n[Rule]\nPROTOCOL,DNS,DIRECT\nFINAL,DIRECT\n",
        dns.addr().port()
    );
    let loaded = from_text(&profile, &dir.path().join("t.conf"), &LoadOptions::for_tests());
    assert!(!loaded.diagnostics.has_errors());
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
                dns_connector: None,
            },
            outbound_mode: OutboundMode::Rule,
            idle_timeout: Duration::from_secs(600),
            request_log_size: 1000,
            selections: GroupSelections::new(),
        },
    )
    .await
    .unwrap();
    let engine = Engine::new(runtime);
    let _listeners = engine.bind_listeners().await.unwrap();
    // resolving a name forces a TCP DNS query, which must go through the pipeline
    let res = engine
        .runtime()
        .stack
        .resolver
        .lookup("target.test", rurge_dns::resolver::LookupOpts::default())
        .await;
    assert!(res.is_ok(), "resolution through the pipeline: {res:?}");
    // the DNS server connection was recorded as an Internal session with DNS protocol
    let seen = engine.request_log().recent(50);
    let active = engine.request_log().active();
    assert!(
        seen.iter().chain(active.iter()).any(|r| r.listener == ListenerKind::Internal
            && r.protocol == Some(rurge_config::rule::ProtocolKind::Dns)),
        "internal DNS session recorded: recent={seen:?}"
    );
    assert!(!dns.queries().is_empty(), "the mock DNS server was actually queried");
}
```

（`LookupOpts` 的默认构造 / 字段以 `rurge_dns::resolver::LookupOpts` 实际为准；实施者据签名填。`ListenerKind`、`Runtime`、`RuntimeOptions`、`StackOptions` 已在 pipeline.rs 导入。）

- [ ] **Step 8: 质量门与提交**

```bash
cargo test -p rurge-engine -p rurge
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-engine crates/rurge
git commit -F - <<'EOF'
feat(engine): encrypted-dns-follow-outbound-mode：IP 字面量 DNS 服务器走流水线

DNS 上游的 TCP / DoT / DoH 连接成为 Internal 会话，PROTOCOL 规则可匹配、进
请求记录；域名 DNS 服务器仍走 Bootstrap 直连以防环；被规则 REJECT 时告警
并直连以保 DNS 不断。UDP 上游不受影响。

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 13: M3a 延后事项收尾（规则查找二分、SOCKS5 ATYP、select 持久选择、diagnostics）

清掉 M3a 计划末尾几项延后事项：`Engine::dial` 的规则原文线性查找改二分；SOCKS5 未知 ATYP 回 `0x08` 且补 IPv6 ATYP 用例；`select` 持久选择端到端用例；`Runtime::diagnostics()` 冒烟。

**Files:**
- Modify: `crates/rurge-engine/src/engine.rs` —— `rule_raw` 二分辅助
- Modify: `crates/rurge-inbound/src/socks5.rs` —— 未知 ATYP 回 `0x08`；IPv6 用例
- Modify: `crates/rurge-engine/tests/pipeline.rs` —— select 持久选择、diagnostics 用例

- [ ] **Step 1: 规则原文改二分**

`engine.rs`：`rules()` 按 `index` 升序，把两处 `rt.rules.rules().iter().find(|r| r.index == i).map(|r| r.raw.clone())`（`dial` 与 `dial_internal`）抽成一个辅助并用二分：

```rust
fn rule_raw(rules: &rurge_rules::RuleEngine, index: usize) -> Option<String> {
    let all = rules.rules();
    all.binary_search_by_key(&index, |r| r.index)
        .ok()
        .map(|pos| all[pos].raw.clone())
}
```

`dial` 与 `dial_internal` 里改为 `handle.set_rule(rule_raw(&rt.rules, i));`。（若 `rules()` 不保证升序，二分不成立——M2a 的 `CompiledRule.index` 是 `Config.rules` 的下标，按构建顺序升序，成立；加一行注释说明。）

- [ ] **Step 2: SOCKS5 未知 ATYP 回 0x08 + IPv6 用例（先写测试）**

`crates/rurge-inbound/src/socks5.rs` 的 `#[cfg(test)] mod tests` 加两个用例（沿用既有 `listener` / `negotiate` / `read_reply` 助手）：

```rust
    #[tokio::test]
    async fn ipv6_atyp_is_parsed_and_dialed() {
        let (running, dialer) = listener(Duration::from_secs(30)).await;
        let mut s = negotiate(running.local_addr).await;
        // CONNECT ::1 :443 via ATYP=0x04
        let mut req = vec![5, 1, 0, 4];
        req.extend_from_slice(&std::net::Ipv6Addr::LOCALHOST.octets());
        req.extend_from_slice(&443u16.to_be_bytes());
        s.write_all(&req).await.unwrap();
        // ::1 is not one of FakeDialer's mapped hosts → it fails, but the request
        // must have been parsed and a session recorded with that dst (poll)
        let _ = read_reply(&mut s).await;
        let want = rurge_config::HostName::Ip(std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            if dialer.sessions().iter().any(|h| h.session().dst_host == want) {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline, "IPv6 session recorded");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn unknown_atyp_is_answered_with_0x08() {
        let (running, _dialer) = listener(Duration::from_secs(30)).await;
        let mut s = negotiate(running.local_addr).await;
        s.write_all(&[5, 1, 0, 0x09]).await.unwrap(); // 0x09 is not a valid ATYP
        assert_eq!(read_reply(&mut s).await[1], REP_ADDR_TYPE_NOT_SUPPORTED);
    }
```

（若 `FakeDialer` 未映射 `::1`，其分支会走连接失败并 `finish` 句柄，`sessions()` 仍会记录到该会话，断言成立；实施者据 `testing.rs` 的实际未知主机分支确认。）

- [ ] **Step 3: 实现 0x08 应答**

`socks5.rs`：加常量 `pub const REP_ADDR_TYPE_NOT_SUPPORTED: u8 = 0x08;`。`read_request` 的未知 ATYP 分支从 `return Err(io::Error::new(..., "unknown address type"))` 改为 `_ => return Ok(Err(REP_ADDR_TYPE_NOT_SUPPORTED)),`。（`handle` 会 `write_all(&reply(0x08))` 后 `Ok(())`。）

- [ ] **Step 4: select 持久选择与 diagnostics 用例**

`crates/rurge-engine/tests/pipeline.rs` 末尾：

```rust
#[tokio::test]
async fn persisted_group_selection_is_honored() {
    // Pick = select, HK, DIRECT; a state selecting DIRECT must resolve to DIRECT
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    let target = TestServer::spawn().await;
    target.set("/hello", "hi there");
    let dir = tempfile::tempdir().unwrap();
    let profile = format!(
        "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\ndns-server = {}\nipv6 = false\n\
[Proxy]\nHK = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\n[Proxy Group]\nPick = select, HK, DIRECT\n\
[Rule]\nDOMAIN,target.test,Pick\nFINAL,DIRECT\n",
        dns.addr()
    );
    let loaded = from_text(&profile, &dir.path().join("t.conf"), &load_options());
    assert!(!loaded.diagnostics.has_errors());
    let mut selections = std::collections::HashMap::new();
    selections.insert("Pick".to_string(), "DIRECT".to_string());
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
                dns_connector: None,
            },
            outbound_mode: OutboundMode::Rule,
            idle_timeout: Duration::from_secs(600),
            request_log_size: 1000,
            selections: GroupSelections::from_map(selections),
        },
    )
    .await
    .unwrap();
    // diagnostics() is reachable and clean here
    assert!(!runtime.diagnostics().has_errors());
    let engine = Engine::new(runtime);
    let listeners = engine.bind_listeners().await.unwrap();
    let http = listeners.iter().find(|(s, _)| s.kind == ListenerKind::Http).unwrap().1.local_addr;
    // Pick → DIRECT (persisted) → the target is reachable
    let (head, body) = get_via_proxy(http, &format!("http://target.test:{}/hello", target.url("/").port().unwrap())).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, b"hi there");
    let rec = wait_for_record(&engine, Duration::from_secs(3), |r| r.policy.iter().any(|p| p == "DIRECT")).await;
    assert!(rec.is_some(), "resolved through DIRECT (persisted selection)");
}
```

- [ ] **Step 5: 质量门与提交**

```bash
cargo test -p rurge-inbound -p rurge-engine
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-inbound crates/rurge-engine
git commit -F - <<'EOF'
refactor(engine,inbound): 规则原文二分查找；SOCKS5 未知 ATYP 回 0x08；补 select / diagnostics 用例

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 14: 文档同步

把 M3b 的能力与行为差异同步到 README、CLAUDE.md、兼容性清单、设计文档实施订正与本计划末尾。

**Files:**
- Modify: `README.md`（中英两半）
- Modify: `CLAUDE.md`
- Modify: `docs/surge-compatibility-matrix.md`
- Modify: `docs/superpowers/specs/2026-09-05-phase1-m3-pipeline-design.md`
- Modify: `docs/superpowers/plans/2026-09-06-phase1-m3b-operability-plan.md`（本文件末尾两表）

（Task 12 的范围说明若与本任务的清单措辞不一致，以 Task 12 开头「防环机制」一段为准。）

- [ ] **Step 1: README（中英一致）**

中文「当前状态」段（第 18 行附近）把「M1、M2a、M2b、M3a 完成」「M3b（请求记录、热重载）、M4 未开始」更新为「M1 ～ M3b 完成」，列出 M3b 新增：请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4（控制面 API、系统代理、Dashboard）未开始。英文「Status」段（第 143 行附近）同义更新。快速开始段补 `rurge run --watch` / `--log-file` / `--idle-timeout` 的一句说明（专有运行时选项）。

- [ ] **Step 2: CLAUDE.md**

「当前状态（2026-09）」段把 M3b 从「未开始」改为「已完成」，简述新增能力；「先读这些文档」列表加一行指向本计划 `docs/superpowers/plans/2026-09-06-phase1-m3b-operability-plan.md`。「常用命令」段补 `rurge run -c config.conf --watch --log-file rurge.log`（一行）。

- [ ] **Step 3: 兼容性清单（保持各表列数不变）**

在 `docs/surge-compatibility-matrix.md` 逐条落实/更新（按内容定位）：
1. `REJECT`（4.1 表 304 行）：备注补「同主机 30 s 内 50 次可升级拒绝后自动升级 REJECT-DROP（M3b）」。
2. `PROTOCOL`（3.1 表 231 行）：备注确认「`DOH*` `DOQ` `DOT` `DNS` 只匹配 rurge 自身 DNS 且需 `encrypted-dns-follow-outbound-mode=true`」，追加「M3b：DoT/DoH/DNS 标签按上游端口启发（853/443/其余）；基于 SNI 的路由与 `PROTOCOL,HTTPS` 的 dial 前匹配随阶段 4」。
3. `encrypted-dns-follow-outbound-mode`（2.1 表 134 行）：状态由 ✅ 改 🟡，备注补「M3b：TCP/DoT/DoH 上游连接走流水线（成 Internal 会话，`PROTOCOL,DOH/DOT/DNS` 可匹配）；上游主机名由 Bootstrap 解析，流水线只见 IP 目标，故域名规则不匹配上游主机名；协议标签按端口启发；被规则 REJECT 时告警并直连以保 DNS；UDP 上游不经连接器」。同步 9.x 表第 494 行同义。
4. `show-error-page`（2.1 表 165 行）：备注把「CONNECT 的 502 在 M3b」改为「CONNECT 连接失败的 502 已实现（M3b）」。
5. 扩展匹配 `extended-matching`（247 行）：备注补「SNI 于 M3b 记录进请求记录用于观测；基于 SNI 的匹配随阶段 4 的 HTTP 引擎生效」。
6. HTTP API `GET /v1/traffic`（843 行）、`POST /v1/profiles/reload`（834 行）：状态维持规划态，备注补「底层流量统计 / 热重载能力已在 M3b 就位，API 在 M4 暴露」。
7. 新增行（在 2.1 表内，列数与该表一致）：`空闲超时（idle-timeout）` —— Surge 未公开默认值 —— 🟡 —— 阶段 1 —— 「rurge 默认 600 s，`--idle-timeout` 覆盖（专有运行时选项）」。
8. 新增行（2.1 或 iOS/mac 专属之外的通用位置）：`请求记录环形缓冲` 与 `按策略 / 监听器流量统计` —— ✅ —— 阶段 1 基础 —— 「内存环形缓冲（`--request-log-size`，默认 1000）+ 活动索引 + kill；完整 API 在 M4/阶段 6」。
9. 10.3 CLI 表 `rurge run` 行（812 行）备注补 `--watch` / `--log-file` / `--idle-timeout` / `--request-log-size` 为专有运行时选项；`reload` / `stop` 仍依赖 M4 控制通道，但 SIGHUP / `--watch` 的热重载已可用。
10. `--log-file`：日志相关处（`loglevel` 附近或 10.x）补一行/一句「`--log-file` 按天滚动保留 7 个（专有运行时选项）」。
11. 热重载：`FR-CFG-16` 对应处补「SIGHUP / `--watch` 已实现（M3b）；`rurge reload` 命令与 API 触发在 M4；重载会重建 DNS 解析器（缓存清空），配置未变时复用解析器的优化待后续」。

- [ ] **Step 4: 设计文档实施订正**

在 `2026-09-05-phase1-m3-pipeline-design.md` 对应小节末尾各加一句「实施订正（M3b）：…」：
- §7.3：relay 换成手写可中断双向泵（`relay.rs::pump`），空闲超时默认 600 s，取消令牌驱动 kill / 优雅退出。
- §7.4：SNI 嗅探在 M3b 为观测用（填 `sni`/`protocol` 供请求记录与日志，路由仍用目标主机；基于 SNI 的路由随阶段 4）；REJECT 升级 30 s/50 次按目标主机；热重载每代重建 Stack（含解析器），解析器复用待后续；`encrypted-dns-follow-outbound-mode` 仅 IP 字面量 DNS 服务器走流水线，域名服务器走 Bootstrap，被 REJECT 时直连保底。
- §9.2：`--log-file` 按天滚动保留 7（`tracing-appender`）。
- §9.3：新增 `--idle-timeout` / `--request-log-size` / `--watch` / `--log-file`；主循环 select（Ctrl-C 优雅退出、SIGHUP/`--watch` 重载）。

- [ ] **Step 5: 本计划末尾两表**

填「执行期修正记录」（实施期与计划的偏差）与「延后事项」（见文末骨架；实施者据实补充）。

- [ ] **Step 6: 质量门与提交**

```bash
cargo test --workspace   # 确认文档改动未影响构建
git add README.md CLAUDE.md docs
git commit -F - <<'EOF'
docs: 同步 M3b 能力与行为差异到 README / CLAUDE.md / 兼容性清单 / 设计文档

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

## 执行期修正记录

| 任务 | 计划内容 | 实际处理 | 原因 |
| --- | --- | --- | --- |
| 1 | `pump` 为单循环 `select!`，写入在分支内 | 改为 `tokio::io::split` 双半 + `copy_half`（每次读写都与停止令牌竞速）+ 空闲看门狗；写入失败记 `Failed(e)` | 分支内的 `write_all` 不可中断，kill / 空闲超时救不了被对端堵住的会话；单循环两个方向互相队头阻塞 |
| 1 | 无 | `rurge-engine` dev-dependencies 的 `tokio` 加 `test-util` | `start_paused` 空闲测试需要（沿用 rurge-dns 先例） |
| 2 | 简报 Interfaces 列了 `Engine::close_tracker` | 未实现（各处直接 `tracker().close()`） | 简报摘要的陈旧条目，无任何步骤定义或调用它 |
| 2 | `Running { task, stop }` | 增 `closed` 门闩与 `wait_closed()`；`serve` 在 `drop(listener)` 后触发 | 重载重绑前必须确认旧 socket 已关（Windows 无 `SO_REUSEADDR`） |
| 2 | 宽限期分支直接 `cancel_sessions()` | `drain` 先 `tokio::pin!` 再按引用轮询，宽限到期先取消再继续等同一个 drain（≤1 s） | `select!` 会先丢弃分支 future，原写法在取消前就 abort 了监听器及其 JoinSet 里的会话 |
| 2 | 排空测试只等 `tracker.wait()` | 加 200 ms 反证（取消前必须超时）；CONNECT 应答读取加 3 s 上界；新增 SOCKS5 `stop → wait_closed → 在飞会话仍转发 → join` 用例 | 原用例在 tracker 已关且为空时立即通过，无法失败 |
| 2 | — | Unix 专属 `run_shuts_down_gracefully_on_sigint` 未在本机（Windows）编译运行，只做了独立编译核对 | 本机无法向子进程发 SIGINT |
| 4 | 两条测试只用 HTTP 会话 + `["DIRECT"]` 链 | 追加 SOCKS5 会话（链 `["Pick","HK","!unsupported:ss","REJECT"]`）与 `["Block","!unsupported:vmess"]`，变异验证通过 | 原测试无法区分 Socks5 分支与「最后一个非 `!` 项」取键规则 |
| 5 | 活动索引持 `Arc<SessionHandle>` | 改持 `Weak`，快照 / `kill` 时清理死条目；SOCKS5 成功应答写失败的早退路径补 `finish` | 早退未 finish 的句柄会在强引用索引里永久滞留 |
| 5 | 钩子先写记录再记流量 | 先 `traffic.record` 再 `record_finished` | 读者不会看到有记录无字节的瞬间 |
| 6 | `pump` 单循环客户端分支内嗅探 | 作为 `copy_half` 的 `first: Option<FirstChunkHook>` 首段钩子，只挂客户端→上游半边 | relay 已在 Task 1 修复轮改为分半结构 |
| 10 | 循环内每轮 `tokio::signal::ctrl_c()` | 循环外建一次长驻中断流（Unix `SignalKind::interrupt()` / Windows `signal::windows::ctrl_c()`）并 `recv()` | 重载期间（可达数秒）到达的 Ctrl-C 会被新注册的监听丢掉 |
| 10 | 重绑失败仍记 `profile reloaded` | 重绑失败 `eprintln!` 后直接返回 | 避免误报成功 |
| 10 | — | 实施者为交叉核对 Unix 分支安装了 rustup 目标 `x86_64-unknown-linux-gnu` | 本机环境副作用，可 `rustup target remove x86_64-unknown-linux-gnu` |
| 11 | 测试用 `DirEntry::metadata()` 判断文件非空 | 改用 `std::fs::metadata(path)` | Windows 下 `DirEntry::metadata()` 复用目录枚举缓存，写入进程持有句柄时大小不更新 |
| 12 | 单测连 `127.0.0.1:9` 期望失败 | 改为绑定回环监听并断言 fallback 连接成功；`BoxedStream` 无 `Debug` | 本机 9 端口开放；`{res:?}` 不编译 |
| 12 | REJECT 保底分支 `fallback.connect(..)?` | 保底连接也失败时先 `finish(Failed)` 再上抛 | 否则句柄不 finish、不进记录 |
| 12 | 模块文档「只有 IP 字面量 DNS 服务器走流水线」 | 订正为「Bootstrap 先解析域名再把 IP 目标交给流水线，域名配置的上游同样走流水线」 | 计划早期措辞错误（提交 5fbfa59 的说明无法修改，修复提交 28d20d6 的说明已订正） |
| 12 | — | `MockDns::start` 改为 TCP+UDP 端口对最多重试 16 次 | 先绑 TCP 临时端口再硬要同号 UDP 端口且不重试，是本阶段测试抖动（`keep_alive_requests_are_dialed_one_by_one` 等）的根因 |
| 12 | `PipelineConnector::fallback()` 访问器 | 删除 | 无调用者 |
| 14 | 兼容性清单 3.2 表补备注 | 该表无备注列，`extended-matching` 的 M3b 说明写进「效果」列 | 表结构固定，加列会波及整表 |
| 修复波 A1 | `Escalation` 表满时清理空闲条目后插入 | 清理后仍满则直接返回、不记录该主机（因而不会升级）；已有主机走 `get_mut` 不再分配 `String`；补 `escalation_map_is_bounded` | 原写法在窗口内全是新主机时无界增长，且每次插入都在锁内付一次 O(n) 清扫（与 `listener::warn_due` 已有的正确模式不一致） |
| 修复波 A2 | `listen_addrs` 比较 `(kind, addr)` 集合 | 改为 `listener_surface`：`Vec<ListenerSpec>`（含 `auth`）+ `proxy-restricted-to-lan` + 两个错误页开关；设计文档 §7.4 与兼容性清单同步订正 | `ListenerOpts` 在 `bind` 时定型、之后不刷新，只比地址会让密码轮换与来源限制在重载后静默不生效而日志仍报成功 |
| 修复波 A3 | `run.rs::reload` 手写 `stop` / `wait_closed` / `join` / `bind` 序列 | 收进 `Engine::rebind_listeners`（`REBIND_WAIT` 一并迁入 engine.rs）；`listeners.is_empty()` 时无条件重试绑定；重绑失败改记 `tracing::error!` 并说明「下次成功重载前没有任何监听器」 | 重绑失败留下「零监听器」退化态，而地址集合未变时后续重载不会再尝试绑定，守护进程只能重启 |
| 修复波 A4 | 明文 HTTP 转发不监听会话令牌 | 上游驱动任务与 `sender.send_request` 都与令牌 `biased` 竞速（令牌优先），令牌先到则关闭客户端连接并 `Failed("killed")` / `Completed`；空闲超时对该路径不适用，登记进兼容性清单与设计文档 §7.3 | `RequestLog::kill` 对这类会话返回 `true` 却什么也没做（M4 的 `POST /v1/requests/{id}/kill` 会照抄这个谎报） |
| 修复波 A5 | `TrafficStats::record` 的 `_ =>` 归入 HTTP 桶 | `Http` / `Socks5` 各自成桶，`Internal` / `Tun` / `Forward` 只计全局与按策略 | `Internal`（DNS 流水线会话）不是监听器，`encrypted-dns-follow-outbound-mode` 打开时会虚增 `by_listener()[Http]` |
| 修复波 A6 | 关停 `select!` 的强制退出分支新建 `tokio::signal::ctrl_c()` | 复用长驻 `interrupt`（Unix 并上 `sigterm`），提示文案按平台区分 | 与主循环在提交 37d3fd5 修掉的是同一个 per-iteration 注册缺陷；且 systemd 下 SIGTERM 触发优雅退出后无法再强制退出 |
| 修复波 A7 | CONNECT 隧道与上游驱动任务直接跑在 `TaskTracker` 上 | 外层任务 `await` 一个内层 `tokio::spawn`，`JoinError::is_panic` 记 ERROR | 从监听器 `JoinSet` 迁到 tracker 也迁出了它的 panic 处理（设计 §6.4 要求记 ERROR） |
| 修复波 A8 | `bind_listeners` 记 INFO `listening` | 降为 DEBUG；`rurge run` 启动成功后补一条 INFO `rurge running`（与 stdout 摘要同源）；`--log-file` 用例改为断言文件含该行 | 每个监听器在 `loglevel = notify` 下被宣告两次；降级后文件层需要保留一条 INFO 落点，用例也从「文件非空」加强为「含启动 INFO 行」 |

## 延后事项

- `restrict_to_lan` 的监听器级用例：回环测试无法产生「非本地来源」，需非回环绑定，留待有网络命名空间的 CI；`source_allowed` 谓词已有单测。
- 会话日志字段的 `tracing` 抓取用例：M3b 用请求记录断言等价覆盖了字段（`rule` / `policy` / `up` / `down` / `error`），独立的 tracing 抓取用例待引入测试用 subscriber。
- 热重载「配置未变时复用解析器」的优化（现每代重建 Stack，DNS 缓存清空）。
- 基于 SNI 的路由与 `PROTOCOL,HTTPS` 的 dial 前匹配（M3b 只做观测用 SNI），随阶段 4 HTTP 引擎。
- `encrypted-dns-follow-outbound-mode`：「命中的策略若是域名配置的代理则告警并回退 DIRECT」只在阶段 2 有代理后才可触发；DNS 会话按 IP 匹配规则，域名规则不匹配上游主机名（上游名由 Bootstrap 先解析）。
- DNS 上游协议标签按端口启发（853→DoT、443→DoH、其余→DNS），非标准端口可能误标；若要精确，需给 `ConnectOpts` 加协议意图字段（波及 rurge-net / rurge-dns 的全部构造点）。
- `FailKind::Other` 仍无专用用例；`Runtime::diagnostics()` 仅冒烟；`http_exchange` 仍把超时与关闭混为一谈；wifi-access 的 `listener_specs` 用例断言仍偏少。
- SOCKS5 成功应答的 `BND.ADDR` 恒为 `0.0.0.0:0`（`BoxedStream` 抹掉了本地地址；阶段 2 给 `Outbound` 加 `local_addr` 钩子）。
- `State::load` 为同步读取（M4 引入写入与 HTTP API 时改异步）。
- SNI 嗅探只看 relay 的第一段客户端数据；ClientHello 若跨多个 TCP 段（大 ClientHello / 分片），本段解析不到 SNI 就放弃，不做拼接。
- `rurge reload` / `stop` 命令与 HTTP API 触发（M4）；`state.json` 写入与 `outbound_mode` 持久化（M4）。
- `copy_half` 里 `writer.shutdown()` 是唯一不与停止令牌竞速的 await（TCP FIN 不阻塞；阶段 2 TLS 出站的 close_notify 会成为潜在卡点）。
- 被取消时正在进行的 `write_all` 已写出的部分字节不计数（≤ 8 KiB）。
- `Engine::stop_accepting` 不可逆（之后绑定的监听器拿到的是已取消的子令牌）；`serve` 的 `stop` 分支未 `biased`（停止后可能再多接受 1–2 个连接）；`tracker()` 缺文档注释；Unix CLI 关停测试在进程退出后才读 stdout 行。
- 排空测试里 `TcpStream::connect(addr)` 的「拒绝」检查没有超时包裹。
- `RequestLog::active_bytes()` 无专用用例；`kill` 的假路径只用未注册 id 测过。
- `TrafficStats::sample` 分两个原子写 `rate_up` / `rate_down`（仅多采样器并发时可能读到撕裂值）。
- 速率采样任务在 `bind_listeners` 失败的早退路径上只在运行时销毁时回收；`Engine::kill` 无引擎级用例。
- SNI：解析器测试只覆盖单扩展 ClientHello；SNI 字符串未校验 / 未小写化（仅进记录与日志）；pipeline 的轮询与 `wait_for_record` 重复（后者只看 `recent`）。
- `FailKind` → 文案映射在 `connect` 与 `forward` 各一份。
- `store_runtime` 里全限定 `std::sync::Arc::new`（外观）；旧会话排空期间新旧两代 `GeoUpdater` 可能同时指向 `data_dir/geoip`。
- `--watch` 的监视列表在启动时冻结（重载新增的 `#!include` 直到重启才被监视）；同目录多个 include 会重复 `watch` 并重复告警；去抖没有最长等待上限；CLI 用例的重载前检查只证明「未放行」而非「被规则拒绝」。
- `--log-file` 的文件层未 `with_target(false)`，与 stdout 格式略有不同。
- `FinishOnDrop` 一律记 `Completed`（DNS 连接中途死亡或 REJECT 保底路径也如此）；`dial_internal` 忽略调用方的 `ConnectOpts`（上游自身仍有截止时间）；`dial` 与 `dial_internal` 约 45 行近似重复。
- 内部 DNS 会话沿用 `SessionInfo::tcp` 默认 `src = 127.0.0.1:0`、`in_port = 0`，`SRC-IP,127.0.0.1/32` / `IN-PORT,0` 可能匹配到它们；`RequestLog::kill` 对 DNS 会话是空操作（DNS 路径不监听令牌）。已登记进兼容性清单（`encrypted-dns-follow-outbound-mode` 行）。
- 测试基础设施：全工作区并行跑测试时 `rurge-inbound` 测试二进制两次出现瞬时 `STATUS_HEAP_CORRUPTION` 退出（单独重跑均通过，无法复现）；怀疑 `TestServer` TLS 路径的原生依赖在并行压力下出问题，需要复现与排查。

以下四项由 M3b 最终整支审查分诊，控制器裁决为「只登记、本波不改」：

- **m2 速率采样重复计一次**：`start_sampler` 先读 `RequestLog::active_bytes()` 再调 `TrafficStats::sample`（内部读累计 `up` / `down`），两次读之间结束的会话会被同时计入活动与累计，产生一秒的速率尖峰、随后一秒被 `saturating_sub` 夹到 0。`rate_is_the_delta_between_samples` 已断言夹取行为。修法是先快照累计再读活动并扣掉期间结束的句柄，M4 暴露 `GET /v1/traffic` 时一并处理。
- **m6 `relay` 的空闲阈值取自当前代**：`Engine::relay` 读 `self.runtime().idle_timeout` 而不是会话 dial 时的快照，与 AR-04「会话只看自己那一代」有出入。今天无害，因为该值来自 CLI 参数、重载时原样带过；要么把 `idle` 挂到 `SessionHandle` 上，要么在文档注释里写明意图。
- **m10 / m11 重复代码（裁决：合并前不必改）**：`FailKind` → 文案映射在 `connect` / `forward` 各一份（5 行、两份完全相同，阶段 4 会整体替换 `forward`）；`dial` / `dial_internal` 约 45 行近似重复（两者在每个错误分支与 `DnsFailed` 结局上都不同，现在抽公共函数只会得到分支比重复更多的辅助函数）。后者在 M4 的 `set_outbound_mode` 落地时重新评估。
- **I5 Unix 信号代码只经阅读审查**：`run.rs` 的 `#[cfg(unix)]` 块（含本波 A6 改动）与 `tests/cli.rs` 的 SIGINT 用例在本机（Windows）无法编译验证——`cargo check --target x86_64-unknown-linux-gnu` 因 aws-lc / ring 的构建脚本需要 `x86_64-linux-gnu-gcc` 而失败，本机没有交叉 C 工具链。唯一有效覆盖是计划 §10 验收标准里的三平台 CI，需在合并前后尽快落地；本机那个 rustup 目标只会造成「已验证」的错觉，应当移除。

