# M4a「控制面」实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给 `rurge run` 一个 Surge 兼容的控制面：`http-api` 上的阶段 1 端点（鉴权 + 封禁）、`state.json` 的写入与出站模式 / 全局策略持久化、经命令通道触发的重载 / 停止 / 日志级别，以及 `rurge reload | stop | status` 客户端命令。

**Architecture:** 引擎持运行期可变状态（`Mode` / 全局策略的 `ArcSwap`，改动即经 `StateStore` 原子落盘）；两条 dial 路径共用一个 `choose_policy`；`rurge-api`（axum）只依赖 `rurge-engine` 的只读视图与一个 `Control` trait 对象；需要主循环配合的动作（reload / stop）经 `mpsc` 命令通道交给 `run.rs` 的 `select!` 循环，日志级别经 `tracing_subscriber::reload::Handle` 直接改；CLI 客户端用 `hyper-util` 的 legacy client 访问同一 API。

**Tech Stack:** Rust stable（edition 2024，MSRV 1.88）、tokio、axum 0.8、hyper 1 / hyper-util（客户端）、serde / serde_json、arc-swap、tokio-util、tracing-subscriber `reload`。

**Spec:** `docs/superpowers/specs/2026-09-07-phase1-m4-control-plane-design.md`（M4a：§1–§7、§9、§10、§12、§13；M4b 的 §8 不在本计划）。

## Global Constraints

- Rust stable，edition 2024，workspace `rust-version = 1.88`（let-chains 允许，clippy 对嵌套 `if let` 要求用 let-chains）；`unsafe_code = "forbid"`。
- 质量门（每个任务提交前）：`RUSTFMT="C:\Users\SZV01065\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe" cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace`。时序敏感的二进制（`rurge-engine --test pipeline`、`rurge-api --test api`、`rurge --test cli`）各跑 3 次。全工作区偶发单个测试二进制异常退出（已知、未定位）——遇到时用 `--no-fail-fast` 重跑一次再判断。
- 测试绝不访问公网：`TestServer` / `MockDns` / API 服务全部 `127.0.0.1:0`；等待条件用有界轮询而非固定 sleep。
- CLI 输出、日志、API 错误文案用英文；文档中文，README 中英两半内容一致。
- 依赖方向：`rurge → { rurge-api → rurge-engine, rurge-engine, rurge-platform }`；`rurge-api` 不依赖 `rurge-platform` 也不认识 bin；`rurge-engine` 不依赖 `rurge-platform`。
- API 约定（设计 §5）：鉴权 `X-Key` 头或 `?x-key=`，常量时间比较；失败 `401 {"error":"unauthorized"}`；同一来源 IP 10 分钟内 5 次失败 → 后续 10 分钟 `403 {"error":"banned"}`；统一错误体 `{"error":"<英文>"}`（400 / 401 / 403 / 404 / 409 / 500 / 501）；成功无内容返回 `{}`；JSON 字段 camelCase，手册已定义字段照抄。
- 出站模式优先级（设计 D5）：显式 `--outbound-mode` > `state.json` > `rule`；显式值写回 `state.json`。`proxy` 模式而全局策略缺失 / 未知 → 按规则模式处理并 WARN 一次（Q3）。
- rurge 专有运行时选项只经 CLI 参数 / 环境变量提供（FR-CFG-17）：`--remote`、`--key`（`RURGE_API_KEY`）。
- 与 Surge 的行为差异一律登记进 `docs/surge-compatibility-matrix.md`（表列数不变）。
- 提交：中文主题行，`git commit -F -` + heredoc，末尾两行 trailer：
  `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`
  `Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW`

## 现有接口（M3b 结束时，供各任务参考）

- `rurge_engine::Engine`：`new(Runtime)->Arc<Engine>`、`runtime()->Arc<Runtime>`、`bind_listeners`、`rebind_listeners(old)`、`swap_runtime(&Arc<Self>, Runtime)->bool`、`stop_accepting`、`cancel_sessions`、`tracker()->&TaskTracker`、`request_log()->&RequestLog`、`traffic()->&TrafficStats`、`kill(id)->bool`、`start_sampler(&Arc<Self>)`、`dial_internal(..)`；`impl Dialer`（`dial` / `relay`）。私有：`new_handle`、`rule_raw(&RuleEngine, usize)->Option<String>`、`fail`、`reject`、`Escalation`。
- `rurge_engine::Runtime { config: Arc<Config>, stack: Stack, rules: RuleEngine, policies: PolicyRegistry, outbound_mode: OutboundMode, idle_timeout, request_log_size, dns_pipeline }`，`RuntimeOptions { stack, outbound_mode, selections, idle_timeout, request_log_size }`。
- `rurge_engine::observe`：`RequestRecord { id, listener: ListenerKind, src: SocketAddr, dst: String, rule: Option<String>, policy: Vec<String>, sni, protocol: Option<ProtocolKind>, up, down, started_ms, elapsed_ms, status: RecordStatus, error }`，`RecordStatus::{Active, Completed, Rejected(String), Failed}`，`RequestLog::{recent(n), active(), active_bytes(), kill(id), mark_active, record_finished(&SessionHandle,&SessionOutcome)}`，`TrafficStats::{record(&SessionHandle), totals()->TrafficTotals{up,down}, by_listener()->[(ListenerKind,u64,u64);2], by_policy()->Vec<(String,u64,u64)>, sample((u64,u64)), rate()->(u64,u64)}`。引擎的结束钩子顺序：`traffic.record(h)` 再 `log.record_finished(h, outcome)`；`start_sampler` 每秒 `traffic.sample(log.active_bytes())`。
- `rurge_engine::state`：`State { version, outbound_mode: Option<String>, global_policy: Option<String>, features: Features, group_selections, system_proxy_backup, current_profile }`、`State::load(&Path)`（同步）、`selections_for`、`profile_key`、`STATE_FILE`。
- `rurge_rules::OutboundMode::{Direct, Proxy(PolicyRef), Rule}`；`RuleEngine::{evaluate(&SessionInfo, OutboundMode, &dyn LazyResolver)->Decision, rules()->&[CompiledRule { index, raw, hits() }]}`；`Decision { outcome: Outcome::{Policy(PolicyRef), DnsFailed}, matched: Option<usize>, .. }`。
- `rurge_config`：`PolicyRef::{Builtin(Builtin), Named(String), Device(String)}`、`PolicyRef::parse(&str)`、`PolicyRef::name()->String`、`Builtin::{Direct, Reject, RejectDrop, RejectNoDrop, RejectTinyGif, Cellular, CellularOnly, Hybrid, NoHybrid}` + `Builtin::name()`；`Config { general, policies: Vec<ProxyPolicy{name,..}>, groups: Vec<PolicyGroup{name,..}>, source: SourceInfo{main,includes}, .. }`；`General.http_api: Option<ControllerAccess { key: String, addr: SocketAddr }>`、`General.internet_test_url: String`；`Diagnostic { severity: Severity, code: &'static str, message, span: Option<Span>, hint }`（`Serialize`）、`Diagnostics::{has_errors, sorted, iter}`；`config::{load(&Path,&LoadOptions)->Result<Loaded{config,diagnostics}, LoadError>, from_text, LoadOptions, Platform}`。
- `rurge_policy::PolicyRegistry::{names()->Vec<String>（配置策略 + 组，按定义顺序）, resolve(&PolicyRef)->Resolution}`。
- `rurge_dns::Resolver::{cache_snapshot()->Vec<CacheEntry{name,v4,v6,expires_in: Option<Duration>,stale,negative,source}>, flush(), measure_delay(&str)->Vec<UpstreamDelay{upstream,result: Result<Duration,String>}>, primary_upstreams()->Vec<String>, bootstrap_upstreams()->Vec<String>}`。
- bin：`cli::run::{RunArgs, run, init_logging(level, log_file)->anyhow::Result<Option<WorkerGuard>>, parse_log_level, level_for, RunOptions, build_engine_runtime(cfg,&rt,&run_opts,mode), reload(engine,config,load_opts,rt,run_opts,mode,&mut listeners), spawn_watcher, print_listening}`；主循环 `select!{ shutdown_signal, reload_signal }`；`cli::rule::{parse_mode, print_diagnostics}`；`cli::check::{parse_platform, Report}`；`cli::environment(platform, core_version)`；`capabilities::{CORE_VERSION, current()}`；`main.rs` 的 `Command` 枚举。
- `rurge_net::http` 里 `hyper_util::client::legacy::Client::builder(TokioExecutor::new()).build::<_, Full<Bytes>>(connector)` 的用法可参照。

## 文件结构

| 文件 | 职责 | 任务 |
| --- | --- | --- |
| `crates/rurge-engine/src/state.rs` | `StateStore`（异步、原子写、损坏改名） | 1 |
| `crates/rurge-config/src/redact.rs`（新建） | `redact_profile(text)`：配置文本脱敏（纯函数） | 2 |
| `crates/rurge-engine/src/control.rs`（新建） | `Mode`、`ReloadReport`、`LogLevel`、`Control` trait | 2、3 |
| `crates/rurge-engine/src/engine.rs` | `mode` / `global_policy` 覆盖与持久化、`choose_policy`、只读视图（`policies_view` / `rules_view` / `config_text`） | 2 |
| `crates/rurge-engine/src/observe.rs` | 采样一致性：`record_finished` 携带 `TrafficStats`，`snapshot_bytes` 在同一把锁下读 | 3 |
| `crates/rurge-api/`（新 crate） | `lib.rs`（`ApiContext`、`serve`）、`auth.rs`（鉴权 + 封禁）、`error.rs`（`ApiError`）、`routes/{outbound,features,misc,policies,requests,traffic,dns,profiles,log}.rs`、`tests/api.rs` | 4、5、6 |
| `crates/rurge/src/cli/run.rs` | `--outbound-mode` 可选 + 优先级、`StateStore`、`LoopControl` 与命令通道、API 启动、`api on` 行、日志级别句柄 | 7 |
| `crates/rurge/src/cli/api_client.rs`（新建） | hyper-util legacy client：GET / POST JSON、`X-Key` | 8 |
| `crates/rurge/src/cli/control.rs`（新建） | `rurge reload / stop / status` | 8 |
| `crates/rurge/src/main.rs` | 三个子命令 | 8 |
| `docs/api/phase1.md`（新建）、README、CLAUDE.md、清单、设计文档、本计划末尾 | 文档 | 9 |

---

### Task 1: `StateStore` —— 异步、原子写入的 `state.json`

**Files:**
- Modify: `crates/rurge-engine/src/state.rs`
- Modify: `crates/rurge-engine/Cargo.toml`（`tempfile` 已是 dev-dep；无新依赖）

**Interfaces:**
- Consumes: `State`（不变）、tokio `fs` / `spawn_blocking`。
- Produces: `StateStore::{open(PathBuf)->(Arc<StateStore>, State), snapshot()->State, update(impl FnOnce(&mut State))->State, path()->&Path}`；`State::load` 保留到 Task 7 再删。

- [ ] **Step 1: 写失败测试**

在 `crates/rurge-engine/src/state.rs` 的 `mod tests` 追加：

```rust
    #[tokio::test]
    async fn store_opens_updates_atomically_and_renames_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATE_FILE);
        // missing → defaults, nothing written yet
        let (store, initial) = StateStore::open(path.clone()).await;
        assert_eq!(initial, State::default());
        assert!(!path.exists());
        // update writes the file atomically (no .tmp left behind)
        let after = store
            .update(|s| {
                s.outbound_mode = Some("direct".into());
                s.global_policy = Some("HK".into());
            })
            .await;
        assert_eq!(after.outbound_mode.as_deref(), Some("direct"));
        assert!(path.exists());
        assert!(!path.with_extension("json.tmp").exists());
        let on_disk: State = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(on_disk, after);
        assert_eq!(store.snapshot().await, after);
        // reopen reads it back
        let (_store2, reread) = StateStore::open(path.clone()).await;
        assert_eq!(reread.global_policy.as_deref(), Some("HK"));
        // garbage → defaults, and the broken file is kept aside as evidence
        std::fs::write(&path, "{ not json").unwrap();
        let (_store3, from_garbage) = StateStore::open(path.clone()).await;
        assert_eq!(from_garbage, State::default());
        assert!(path.with_extension("json.broken").exists());
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn concurrent_updates_are_serialized() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = StateStore::open(dir.path().join(STATE_FILE)).await;
        let mut tasks = Vec::new();
        for i in 0..20u32 {
            let store = store.clone();
            tasks.push(tokio::spawn(async move {
                store
                    .update(move |s| {
                        s.group_selections
                            .entry("p.conf".into())
                            .or_default()
                            .insert(format!("g{i}"), "m".into());
                    })
                    .await;
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }
        let s = store.snapshot().await;
        assert_eq!(s.group_selections["p.conf"].len(), 20);
        let on_disk: State =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join(STATE_FILE)).unwrap())
                .unwrap();
        assert_eq!(on_disk, s);
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p rurge-engine state::`
Expected: 编译失败（`StateStore` 未定义）。

- [ ] **Step 3: 实现 `StateStore`**

`state.rs`（模块文档改为「M3 只读；M4a 起由 `StateStore` 写入」）。在 `State::load` 之后加：

```rust
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

/// The single writer of `state.json`: keeps an in-memory copy and rewrites
/// the file atomically (temp file + rename) on every `update`, serialized
/// by one async mutex.
pub struct StateStore {
    path: PathBuf,
    state: tokio::sync::Mutex<State>,
}

impl StateStore {
    /// Reads the file (missing → defaults; unparsable → defaults, and the bad
    /// file is renamed to `state.json.broken` so it is not silently lost).
    pub async fn open(path: PathBuf) -> (Arc<StateStore>, State) {
        let p = path.clone();
        let state = tokio::task::spawn_blocking(move || read_state(&p))
            .await
            .unwrap_or_default();
        let store = Arc::new(StateStore {
            path,
            state: tokio::sync::Mutex::new(state.clone()),
        });
        (store, state)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn snapshot(&self) -> State {
        self.state.lock().await.clone()
    }

    /// Applies `f` to the in-memory state and writes the result to disk before
    /// returning. A write failure is logged and does not fail the caller: the
    /// running state is still updated.
    pub async fn update(&self, f: impl FnOnce(&mut State)) -> State {
        let mut guard = self.state.lock().await;
        f(&mut guard);
        let snapshot = guard.clone();
        let path = self.path.clone();
        let text = serde_json::to_string_pretty(&snapshot).expect("State serializes");
        let written = tokio::task::spawn_blocking(move || write_atomic(&path, &text)).await;
        match written {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::error!(path = %self.path.display(), error = %e, "cannot write state.json")
            }
            Err(e) => tracing::error!(error = %e, "state.json writer task failed"),
        }
        snapshot
    }
}

fn read_state(path: &Path) -> State {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<State>(&text) {
            Ok(state) => state,
            Err(e) => {
                let broken = path.with_extension("json.broken");
                tracing::warn!(path = %path.display(), error = %e, kept = %broken.display(), "state.json is malformed; using defaults");
                let _ = std::fs::rename(path, &broken);
                State::default()
            }
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => State::default(),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "cannot read state.json; using defaults");
            State::default()
        }
    }
}

fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}
```

（`std::fs::rename` 在 Windows 上也会覆盖已存在的目标文件。`path.with_extension("json.tmp")` 对 `state.json` 得到 `state.json.tmp`。）

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p rurge-engine state::`
Expected: 全部 PASS（含既有 `loads_defaults_selections_and_tolerates_garbage`）。

- [ ] **Step 5: 质量门与提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-engine
git commit -F - <<'EOF'
feat(engine): StateStore：state.json 的异步原子写入，损坏文件改名保留

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 2: 引擎运行期覆盖（`Mode` / 全局策略）、`choose_policy`、只读视图与脱敏

`Engine` 持运行期可变的出站模式与全局策略（`ArcSwap`），改动即持久化；两条 dial 路径共用 `choose_policy`；给 API 提供 `policies_view` / `rules_view` / `config_text`；配置文本脱敏是 `rurge-config` 里的纯函数。

**Files:**
- Create: `crates/rurge-engine/src/control.rs`（本任务只放 `Mode`）
- Create: `crates/rurge-config/src/redact.rs`
- Modify: `crates/rurge-config/src/lib.rs` —— `pub mod redact;`
- Modify: `crates/rurge-engine/src/engine.rs`、`lib.rs`
- Modify: `crates/rurge-engine/tests/pipeline.rs`

**Interfaces:**
- Produces:
  - `rurge_engine::control::Mode::{Direct, Proxy, Rule}`（`Copy`），`Mode::as_str()->&'static str`、`Mode::parse(&str)->Option<Mode>`、`Mode::from_outbound(&OutboundMode)->(Mode, Option<String>)`
  - `Engine::{mode()->Mode, set_mode(Mode)->impl Future, global_policy()->Option<String>, set_global_policy(&str)->impl Future<Output=Result<(),UnknownPolicy>>, policy_exists(&str)->bool, attach_state(&self, Arc<StateStore>), policies_view()->PoliciesView{proxies,groups}, rules_view()->Vec<RuleView{index,rule,hits}>, config_text(sensitive: bool)->io::Result<String>}`；`pub struct UnknownPolicy(pub String)`
  - `rurge_config::redact::redact_profile(&str)->String`

- [ ] **Step 1: `Mode` + `redact_profile` 的失败测试**

新建 `crates/rurge-engine/src/control.rs`，先放测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::rule::PolicyRef;
    use rurge_rules::OutboundMode;

    #[test]
    fn mode_round_trips_and_maps_from_outbound_mode() {
        for m in [Mode::Direct, Mode::Proxy, Mode::Rule] {
            assert_eq!(Mode::parse(m.as_str()), Some(m));
        }
        assert_eq!(Mode::parse("PROXY"), Some(Mode::Proxy));
        assert_eq!(Mode::parse("global"), None);
        assert_eq!(Mode::from_outbound(&OutboundMode::Direct), (Mode::Direct, None));
        assert_eq!(Mode::from_outbound(&OutboundMode::Rule), (Mode::Rule, None));
        assert_eq!(
            Mode::from_outbound(&OutboundMode::Proxy(PolicyRef::parse("HK"))),
            (Mode::Proxy, Some("HK".to_string()))
        );
    }
}
```

新建 `crates/rurge-config/src/redact.rs`，先放测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_secrets_but_keeps_structure() {
        let text = "[General]\nhttp-api = s3cret@127.0.0.1:6171\nhttp-listen = pw@127.0.0.1:6152, 127.0.0.1:6153\nwifi-access-http-auth = alice:hunter2\n[Proxy]\nHK = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x1, udp-relay=true\nTJ = trojan, h, 443, password = y2\n[MITM]\nca-passphrase = abc\nca-p12 = MIIK...\n[Rule]\nFINAL,DIRECT\n";
        let out = redact_profile(text);
        assert!(out.contains("http-api = ***@127.0.0.1:6171"), "{out}");
        assert!(out.contains("http-listen = ***@127.0.0.1:6152, 127.0.0.1:6153"), "{out}");
        assert!(out.contains("wifi-access-http-auth = alice:***"), "{out}");
        assert!(out.contains("password=***, udp-relay=true"), "{out}");
        assert!(out.contains("password = ***"), "{out}");
        assert!(out.contains("ca-passphrase = ***") && out.contains("ca-p12 = ***"), "{out}");
        assert!(!out.contains("s3cret") && !out.contains("hunter2") && !out.contains("x1") && !out.contains("y2") && !out.contains("MIIK"));
        assert_eq!(out.lines().count(), text.lines().count());
        assert!(out.contains("FINAL,DIRECT"));
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p rurge-engine control:: ; cargo test -p rurge-config redact::`
Expected: 编译失败。

- [ ] **Step 3: 实现 `Mode` 与 `redact_profile`**

`crates/rurge-engine/src/control.rs`：

```rust
//! Runtime control surface (M4 design §4, §6): the outbound mode as the API
//! and CLI see it, and (Task 3) the `Control` trait the daemon implements.

use rurge_rules::OutboundMode;

/// The outbound mode as exposed by `GET/POST /v1/outbound`. `Proxy` routes
/// everything through the global policy (`Engine::global_policy`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Direct,
    Proxy,
    Rule,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Direct => "direct",
            Mode::Proxy => "proxy",
            Mode::Rule => "rule",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "direct" => Some(Mode::Direct),
            "proxy" => Some(Mode::Proxy),
            "rule" => Some(Mode::Rule),
            _ => None,
        }
    }

    /// The CLI's `--outbound-mode` (`proxy=<name>` carries the global policy).
    pub fn from_outbound(mode: &OutboundMode) -> (Mode, Option<String>) {
        match mode {
            OutboundMode::Direct => (Mode::Direct, None),
            OutboundMode::Rule => (Mode::Rule, None),
            OutboundMode::Proxy(p) => (Mode::Proxy, Some(p.name())),
        }
    }
}
```

`crates/rurge-config/src/redact.rs`：

```rust
//! Profile text redaction for `GET /v1/profiles/current?sensitive=0` (M4
//! design §4.3): secrets become `***`, everything else (including line count)
//! is preserved so line numbers in diagnostics still line up.

const SECRET_KEYS: [&str; 3] = ["password", "ca-passphrase", "ca-p12"];
const KEY_AT_KEYS: [&str; 4] = ["http-api", "external-controller-access", "http-listen", "socks5-listen"];

pub fn redact_profile(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&redact_line(line));
    }
    out
}

fn redact_line(line: &str) -> String {
    let Some((key, value)) = line.split_once('=') else {
        return line.to_string();
    };
    let k = key.trim().to_ascii_lowercase();
    if SECRET_KEYS.contains(&k.as_str()) {
        return format!("{key}= ***");
    }
    if k == "wifi-access-http-auth" {
        let v = value.trim_start();
        let lead = &value[..value.len() - v.len()];
        return match v.split_once(':') {
            Some((user, _)) => format!("{key}={lead}{user}:***"),
            None => format!("{key}={lead}***"),
        };
    }
    if KEY_AT_KEYS.contains(&k.as_str()) {
        let items: Vec<String> = value
            .split(',')
            .map(|item| {
                let t = item.trim_start();
                let lead = &item[..item.len() - t.len()];
                match t.split_once('@') {
                    Some((_, rest)) => format!("{lead}***@{rest}"),
                    None => item.to_string(),
                }
            })
            .collect();
        return format!("{key}={}", items.join(","));
    }
    // policy lines: `name = type, host, port, password=..., psk=...`
    let mut redacted = value.to_string();
    for param in ["password", "psk", "private-key"] {
        redacted = redact_param(&redacted, param);
    }
    format!("{key}={redacted}")
}

/// Replaces the value of every `<param> = <value>` occurrence (up to the next
/// comma or end of line) with `***`, keeping the surrounding spacing.
fn redact_param(value: &str, param: &str) -> String {
    let lower = value.to_ascii_lowercase();
    let mut out = String::with_capacity(value.len());
    let mut rest = 0usize;
    let mut search = 0usize;
    while let Some(pos) = lower[search..].find(param) {
        let start = search + pos;
        // must be at a token boundary followed by optional spaces and '='
        let before_ok = start == 0 || matches!(lower.as_bytes()[start - 1], b',' | b' ' | b'\t');
        let after = &value[start + param.len()..];
        let eq = after.trim_start().strip_prefix('=');
        match (before_ok, eq) {
            (true, Some(tail)) => {
                let tail_start = value.len() - tail.len();
                let tail_lead = &tail[..tail.len() - tail.trim_start().len()];
                let val_len = tail.trim_start().find(',').unwrap_or(tail.trim_start().len());
                out.push_str(&value[rest..tail_start]);
                out.push_str(tail_lead);
                out.push_str("***");
                rest = tail_start + tail_lead.len() + val_len;
                search = rest;
            }
            _ => search = start + param.len(),
        }
    }
    out.push_str(&value[rest..]);
    out
}
```

`crates/rurge-config/src/lib.rs` 加 `pub mod redact;`。（若 `lib.rs` 用显式 `pub use` 列表，只需加模块声明。）

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p rurge-engine control:: ; cargo test -p rurge-config redact::`
Expected: PASS。若 `redact_param` 对 `password = y2`（等号两侧有空格）没匹配上，检查 `after.trim_start().strip_prefix('=')` 的分支——测试要求两种写法都脱敏。

- [ ] **Step 5: 引擎覆盖与视图（先写集成失败测试）**

`crates/rurge-engine/tests/pipeline.rs`：`harness()` 目前只用 `from_text` 解析内存里的配置，`Engine::config_text` 要读磁盘上的 `source.main`，所以在 `harness()` 里 `let loaded = from_text(..)` 之前加一行 `std::fs::write(dir.path().join("t.conf"), &profile).unwrap();`。然后在文件末尾追加：

```rust
#[tokio::test]
async fn runtime_mode_and_global_policy_override_routing() {
    use rurge_engine::control::Mode;
    // rules reject the target; the runtime mode can bypass them
    let h = harness("", "DOMAIN,target.test,REJECT", OutboundMode::Rule).await;
    let url = format!("http://target.test:{}/hello", h.target_port());
    let (head, _) = get_via_proxy(h.http(), &url).await;
    assert!(head.is_empty(), "rules reject: {head}");
    assert_eq!(h.engine.mode(), Mode::Rule);
    h.engine.set_mode(Mode::Direct).await;
    let (head, body) = get_via_proxy(h.http(), &url).await;
    assert!(head.starts_with("HTTP/1.1 200"), "direct mode bypasses rules: {head}");
    assert_eq!(body, b"hi there");
    // proxy mode routes through the global policy (Block = reject-tinygif)
    h.engine.set_global_policy("Block").await.unwrap();
    h.engine.set_mode(Mode::Proxy).await;
    let (head, body) = get_via_proxy(h.http(), &url).await;
    assert!(head.starts_with("HTTP/1.1 200") && body.len() == 43, "{head}");
    // unknown policy is refused; a builtin is accepted
    assert!(h.engine.set_global_policy("nope").await.is_err());
    h.engine.set_global_policy("DIRECT").await.unwrap();
    let (head, body) = get_via_proxy(h.http(), &url).await;
    assert!(head.starts_with("HTTP/1.1 200") && body == b"hi there", "{head}");
    // proxy mode with the global policy cleared falls back to the rules
    h.engine.set_global_policy("").await.unwrap();
    assert_eq!(h.engine.global_policy(), None);
    let (head, _) = get_via_proxy(h.http(), &url).await;
    assert!(head.is_empty(), "no global policy → rules → reject: {head}");
    // views
    let pv = h.engine.policies_view();
    assert!(pv.proxies.iter().any(|p| p == "DIRECT") && pv.proxies.iter().any(|p| p == "HK"));
    assert!(pv.groups.iter().any(|g| g == "Pick"));
    let rules = h.engine.rules_view();
    assert!(rules.iter().any(|r| r.rule == "DOMAIN,target.test,REJECT" && r.hits >= 1));
    assert!(rules.last().unwrap().rule.starts_with("FINAL"));
    let text = h.engine.config_text(false).await.unwrap();
    assert!(text.contains("password=***") && !text.contains("password=x"), "{text}");
    let full = h.engine.config_text(true).await.unwrap();
    assert!(full.contains("password=x"));
}
```

- [ ] **Step 6: 运行确认失败**

Run: `cargo test -p rurge-engine --test pipeline runtime_mode`
Expected: 编译失败。

- [ ] **Step 7: 实现引擎覆盖、`choose_policy` 与视图**

`crates/rurge-engine/src/engine.rs`：
- 顶部 `use crate::control::Mode; use crate::state::StateStore; use std::sync::OnceLock; use std::sync::atomic::AtomicBool;`。
- `Engine` 加字段：

```rust
    /// Runtime outbound mode and global policy (M4 design §4.1). Seeded from
    /// the first `Runtime`, changed by the API / CLI, never touched by reload.
    mode: ArcSwap<Mode>,
    global_policy: ArcSwap<Option<String>>,
    /// "proxy mode but no usable global policy" is warned once per change.
    global_warned: AtomicBool,
    state: OnceLock<Arc<StateStore>>,
```

- `Engine::new`：从 `runtime.outbound_mode` 播种：

```rust
        let (mode, global) = Mode::from_outbound(&runtime.outbound_mode);
        // ... 构造时
            mode: ArcSwap::from_pointee(mode),
            global_policy: ArcSwap::from_pointee(global),
            global_warned: AtomicBool::new(false),
            state: OnceLock::new(),
```

- 新的 `impl Engine` 块：

```rust
#[derive(Debug)]
pub struct UnknownPolicy(pub String);

impl std::fmt::Display for UnknownPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown policy `{}`", self.0)
    }
}
impl std::error::Error for UnknownPolicy {}

pub struct PoliciesView {
    pub proxies: Vec<String>,
    pub groups: Vec<String>,
}

pub struct RuleView {
    pub index: usize,
    pub rule: String,
    pub hits: u64,
}

impl Engine {
    /// Attaches the state store so mode / global-policy changes persist.
    pub fn attach_state(&self, store: Arc<StateStore>) {
        let _ = self.state.set(store);
    }

    pub fn mode(&self) -> Mode {
        **self.mode.load()
    }

    pub async fn set_mode(&self, mode: Mode) {
        self.mode.store(Arc::new(mode));
        self.global_warned.store(false, Ordering::Relaxed);
        if let Some(store) = self.state.get() {
            store
                .update(|s| s.outbound_mode = Some(mode.as_str().to_string()))
                .await;
        }
    }

    pub fn global_policy(&self) -> Option<String> {
        (**self.global_policy.load()).clone()
    }

    /// `true` for the built-in policies and every configured policy / group.
    pub fn policy_exists(&self, name: &str) -> bool {
        matches!(PolicyRef::parse(name), PolicyRef::Builtin(_))
            || self.runtime().policies.names().iter().any(|n| n == name)
    }

    /// Empty name clears the global policy.
    pub async fn set_global_policy(&self, name: &str) -> Result<(), UnknownPolicy> {
        let name = name.trim();
        let value = if name.is_empty() {
            None
        } else if self.policy_exists(name) {
            Some(name.to_string())
        } else {
            return Err(UnknownPolicy(name.to_string()));
        };
        self.global_policy.store(Arc::new(value.clone()));
        self.global_warned.store(false, Ordering::Relaxed);
        if let Some(store) = self.state.get() {
            store.update(|s| s.global_policy = value).await;
        }
        Ok(())
    }

    pub fn policies_view(&self) -> PoliciesView {
        let rt = self.runtime();
        let mut proxies: Vec<String> = ["DIRECT", "REJECT", "REJECT-DROP", "REJECT-NO-DROP", "REJECT-TINYGIF"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        proxies.extend(rt.config.policies.iter().map(|p| p.name.clone()));
        let groups = rt.config.groups.iter().map(|g| g.name.clone()).collect();
        PoliciesView { proxies, groups }
    }

    pub fn rules_view(&self) -> Vec<RuleView> {
        self.runtime()
            .rules
            .rules()
            .iter()
            .map(|r| RuleView {
                index: r.index,
                rule: r.raw.clone(),
                hits: r.hits(),
            })
            .collect()
    }

    /// The current main profile text; secrets redacted unless `sensitive`.
    pub async fn config_text(&self, sensitive: bool) -> io::Result<String> {
        let path = self.runtime().config.source.main.clone();
        let text = tokio::fs::read_to_string(&path).await?;
        Ok(if sensitive {
            text
        } else {
            rurge_config::redact::redact_profile(&text)
        })
    }

    /// Mode / rule → policy, shared by `dial` and `dial_internal` (M4 §4.1).
    async fn choose_policy(&self, rt: &Runtime, handle: &SessionHandle) -> Chosen {
        match self.mode() {
            Mode::Direct => return Chosen::Policy(PolicyRef::Builtin(Builtin::Direct)),
            Mode::Proxy => match self.global_policy() {
                Some(name) if self.policy_exists(&name) => {
                    return Chosen::Policy(PolicyRef::parse(&name));
                }
                other => {
                    if !self.global_warned.swap(true, Ordering::Relaxed) {
                        tracing::warn!(
                            policy = ?other,
                            "outbound mode is proxy but the global policy is unset or unknown; routing by rules"
                        );
                    }
                }
            },
            Mode::Rule => {}
        }
        let decision = rt
            .rules
            .evaluate(handle.session(), OutboundMode::Rule, rt.stack.resolver.as_ref())
            .await;
        if let Some(i) = decision.matched {
            handle.set_rule(rule_raw(&rt.rules, i));
        }
        match decision.outcome {
            Outcome::Policy(p) => Chosen::Policy(p),
            Outcome::DnsFailed => Chosen::DnsFailed,
        }
    }
}

enum Chosen {
    Policy(PolicyRef),
    DnsFailed,
}
```

- `dial` 里把 `let policy = match &rt.outbound_mode { ... };` 整段替换为：

```rust
            let policy = match self.choose_policy(&rt, &handle).await {
                Chosen::Policy(p) => p,
                Chosen::DnsFailed => return fail(handle, FailKind::Dns, "dns lookup failed"),
            };
```

- `dial_internal` 里同样替换为：

```rust
        let policy = match self.choose_policy(&rt, &handle).await {
            Chosen::Policy(p) => p,
            // An IP-literal DNS session never needs resolution; a DnsFailed here
            // would only come from a misconfigured rule → direct.
            Chosen::DnsFailed => PolicyRef::Builtin(Builtin::Direct),
        };
```

- `crates/rurge-engine/src/lib.rs` 加 `pub mod control;` 与 `pub use engine::{PoliciesView, RuleView, UnknownPolicy};`。`ArcSwap<Mode>` 的 `load()` 返回 `Guard<Arc<Mode>>`，`**` 取值；`ArcSwap<Option<String>>::load().as_ref().clone()` 得 `Option<String>`。

- [ ] **Step 8: 运行确认通过**

Run: `cargo test -p rurge-engine`（`--test pipeline` 3×）
Expected: PASS；既有 `outbound_modes_bypass_the_rules` 仍通过（`RuntimeOptions.outbound_mode` 只作播种，语义不变）。

- [ ] **Step 9: 质量门与提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-config crates/rurge-engine
git commit -F - <<'EOF'
feat(engine): 运行期出站模式 / 全局策略覆盖、choose_policy、API 只读视图与配置脱敏

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 3: `Control` trait、`ReloadReport`，以及速率采样的一致性快照

`rurge-api` 只认识 `Control` trait；bin 在 Task 7 实现它。顺带修 M3b 延后的「速率采样在会话结束瞬间重复计一次」：把累计计数的更新与活动索引的移除放到同一把锁下，采样从同一把锁下读一致的 `(累计 + 活动)`。

**Files:**
- Modify: `crates/rurge-engine/src/control.rs`
- Modify: `crates/rurge-engine/src/observe.rs`、`engine.rs`（钩子与采样）、`lib.rs`

**Interfaces:**
- Produces:
  - `rurge_engine::control::{ReloadReport { ok, errors, warnings, listeners_rebound }, LogLevel::{Verbose, Debug, Info, Notify, Warning, Error} (parse/as_str), Control}`：
    ```rust
    pub trait Control: Send + Sync {
        fn reload(&self) -> BoxFuture<'_, ReloadReport>;
        fn stop(&self) -> BoxFuture<'_, ()>;
        fn set_log_level(&self, level: LogLevel) -> Result<(), String>;
        fn set_system_proxy(&self, enabled: bool) -> BoxFuture<'_, Result<(), String>>;
    }
    ```
  - `RequestLog::record_finished(&self, handle, outcome, traffic: &TrafficStats)`（签名变化）、`RequestLog::snapshot_bytes(&self, traffic: &TrafficStats) -> (u64, u64)`（累计 + 活动，一致快照）；`TrafficStats::sample(total: (u64,u64))`（语义改为「一致的总量」）。

- [ ] **Step 1: 写失败测试**

`control.rs` 的 `mod tests` 追加：

```rust
    #[test]
    fn log_level_names_round_trip() {
        for (name, level) in [
            ("verbose", LogLevel::Verbose),
            ("debug", LogLevel::Debug),
            ("info", LogLevel::Info),
            ("notify", LogLevel::Notify),
            ("warning", LogLevel::Warning),
            ("error", LogLevel::Error),
        ] {
            assert_eq!(LogLevel::parse(name), Some(level));
            assert_eq!(level.as_str(), name);
        }
        assert_eq!(LogLevel::parse("loud"), None);
    }
```

`observe.rs` 的 `mod tests` 追加（替换原 `rate_is_the_delta_between_samples` 的 `sample((..))` 语义仍成立；新增一致性用例）：

```rust
    #[test]
    fn snapshot_bytes_never_double_counts_a_finishing_session() {
        let log = RequestLog::new(8);
        let t = TrafficStats::new();
        let a = handle(1, "a.test");
        log.mark_active(&a);
        a.add_up(10);
        a.add_down(20);
        assert_eq!(log.snapshot_bytes(&t), (10, 20), "active bytes only");
        a.finish(SessionOutcome::Completed);
        log.record_finished(&a, &SessionOutcome::Completed, &t);
        assert_eq!(log.snapshot_bytes(&t), (10, 20), "moved to cumulative exactly once");
        assert_eq!((t.totals().up, t.totals().down), (10, 20));
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p rurge-engine control:: observe::`（两条命令分开跑）
Expected: 编译失败。

- [ ] **Step 3: 实现 `Control` / `ReloadReport` / `LogLevel`**

`control.rs` 追加：

```rust
use rurge_net::BoxFuture;

/// Outcome of `POST /v1/profiles/reload` / `rurge reload` (M4 design §6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReloadReport {
    pub ok: bool,
    pub errors: usize,
    pub warnings: usize,
    pub listeners_rebound: bool,
}

/// `POST /v1/log/level` values (phase 1 design §12 mapping is the daemon's job).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Verbose,
    Debug,
    Info,
    Notify,
    Warning,
    Error,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Verbose => "verbose",
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Notify => "notify",
            LogLevel::Warning => "warning",
            LogLevel::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Option<LogLevel> {
        match s.trim().to_ascii_lowercase().as_str() {
            "verbose" => Some(LogLevel::Verbose),
            "debug" => Some(LogLevel::Debug),
            "info" => Some(LogLevel::Info),
            "notify" => Some(LogLevel::Notify),
            "warning" => Some(LogLevel::Warning),
            "error" => Some(LogLevel::Error),
            _ => None,
        }
    }
}

/// What the daemon lets the API drive (M4 design §6). Implemented by the
/// `rurge run` main loop; tests use a fake.
pub trait Control: Send + Sync {
    fn reload(&self) -> BoxFuture<'_, ReloadReport>;
    fn stop(&self) -> BoxFuture<'_, ()>;
    fn set_log_level(&self, level: LogLevel) -> Result<(), String>;
    /// M4b wires this to the platform; M4a implementations return `Err("not implemented")`.
    fn set_system_proxy(&self, enabled: bool) -> BoxFuture<'_, Result<(), String>>;
}
```

`lib.rs`：`pub use control::{Control, LogLevel, Mode, ReloadReport};`。

- [ ] **Step 4: 一致性快照**

`observe.rs`：

```rust
impl RequestLog {
    /// Moves the session out of the active index and into the ring, adding its
    /// bytes to `traffic` under the same lock so `snapshot_bytes` never sees a
    /// session in both places (M3b deferred item m2).
    pub fn record_finished(&self, handle: &SessionHandle, outcome: &SessionOutcome, traffic: &TrafficStats) {
        {
            let mut active = self.active.lock().expect("active index");
            traffic.record(handle);
            active.remove(&handle.id());
        }
        let rec = record_of(handle, Some(outcome));
        let mut ring = self.finished.lock().expect("finished ring");
        if ring.len() == self.capacity {
            ring.pop_front();
        }
        ring.push_back(rec);
    }

    /// Cumulative (finished) plus in-flight bytes, read under the active lock.
    pub fn snapshot_bytes(&self, traffic: &TrafficStats) -> (u64, u64) {
        let active = self.prune_and_lock();
        let totals = traffic.totals();
        let (au, ad) = active
            .values()
            .filter_map(Weak::upgrade)
            .fold((0, 0), |(u, d), h| {
                let (hu, hd) = h.bytes();
                (u + hu, d + hd)
            });
        (totals.up + au, totals.down + ad)
    }
}
```

（`prune_and_lock` 是 Task 5 引入的现有私有助手，返回已清理死条目的 `MutexGuard`；若其签名不同，按其实际用法改。）`active_bytes()` 保留（其它调用方不变）。`TrafficStats::sample` 改为接受一致总量：

```rust
    /// Records one rate sample from a consistent total (finished + in-flight);
    /// rate is the non-negative delta since the previous sample.
    pub fn sample(&self, total: (u64, u64)) {
        let mut last = self.last_sample.lock().expect("rate sample");
        self.rate_up.store(total.0.saturating_sub(last.0), Ordering::Relaxed);
        self.rate_down.store(total.1.saturating_sub(last.1), Ordering::Relaxed);
        *last = total;
    }
```

`engine.rs`：结束钩子改为 `o.log.record_finished(h, outcome, &o.traffic);`（不再单独 `o.traffic.record(h)`）；`start_sampler` 改为 `let total = engine.observe.log.snapshot_bytes(&engine.observe.traffic); engine.observe.traffic.sample(total);`。更新 `rate_is_the_delta_between_samples` 用例：它原本传的是「活动字节」，现在语义是总量——把三次调用改成 `sample((1000, 0))`、`sample((1500, 300))`、`sample((1400, 300))` 的断言保持不变即可（数值不变，仅注释改为 total）。既有 `record_finished` 的调用点（`observe.rs` 测试、`engine.rs`）补上 `&TrafficStats` 参数（测试里新建一个 `TrafficStats::new()`）。

- [ ] **Step 5: 运行确认通过**

Run: `cargo test -p rurge-engine`
Expected: PASS（含 `--test pipeline` 3×）。

- [ ] **Step 6: 质量门与提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-engine
git commit -F - <<'EOF'
feat(engine): Control trait 与 ReloadReport；速率采样改为一致快照

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 4: `rurge-api` 骨架——服务、鉴权与封禁、错误体、outbound / features / 杂项 / stop

新 crate。`serve` 绑定端口并返回「已绑定地址 + 服务 future」（bin 把 future 放到引擎的 `TaskTracker` 上，测试用 `tokio::spawn`）；鉴权中间件先查封禁再验 key；所有错误都是 `{"error": "..."}`。本任务同时建立 API 集成测试的 harness（真实引擎 + MockDns + TestServer + hyper-util 客户端），Task 5 / 6 复用。

**Files:**
- Create: `crates/rurge-api/Cargo.toml`、`src/lib.rs`、`src/auth.rs`、`src/error.rs`、`src/routes/mod.rs`、`src/routes/outbound.rs`、`src/routes/features.rs`、`src/routes/misc.rs`、`tests/api.rs`
- Modify: `Cargo.toml`（workspace deps：`axum`、`rurge-api`）

**Interfaces:**
- Consumes: Task 2 的 `Engine::{mode, set_mode, global_policy, set_global_policy, attach_state}`、`Mode`；Task 3 的 `Control`、`LogLevel`、`ReloadReport`；Task 1 的 `StateStore`。
- Produces:
  - `rurge_api::ApiContext { engine: Arc<Engine>, control: Arc<dyn Control>, load_options: LoadOptions }`（`load_options` 供 Task 6 的 `profiles/check` 用同一套加载选项重新校验；设计 §5.1 的 `ApiContext` 在此基础上多一个字段）
  - `rurge_api::ServerFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>`
  - `rurge_api::serve(addr: SocketAddr, key: String, ctx: ApiContext, shutdown: CancellationToken) -> io::Result<(SocketAddr, ServerFuture)>`
  - `rurge_api::router(key, ctx) -> axum::Router`（测试与 `serve` 共用）
  - crate 内：`App = Arc<Shared { engine, control, load_options, auth: AuthState, started_secs: f64 }>`；`error::{ApiError, ApiResult<T>, json_body}`；`auth::{AuthState, require_key, BAN_FAILURES, BAN_WINDOW, BAN_DURATION}`。

- [ ] **Step 1: workspace 与 crate 清单**

`Cargo.toml`（workspace）`[workspace.dependencies]` 追加：

```toml
axum = { version = "0.8", default-features = false, features = ["http1", "json", "query", "tokio"] }
rurge-api = { path = "crates/rurge-api" }
```

`crates/rurge-api/Cargo.toml`：

```toml
[package]
name = "rurge-api"
description = "Surge-compatible HTTP API for rurge (phase 1 endpoints)"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
rurge-config.workspace = true
rurge-engine.workspace = true
rurge-dns.workspace = true
axum.workspace = true
serde.workspace = true
serde_json.workspace = true
tokio.workspace = true
tokio-util.workspace = true
tracing.workspace = true

[dev-dependencies]
rurge-dns = { workspace = true, features = ["testing"] }
rurge-net = { workspace = true, features = ["testing"] }
rurge-rules.workspace = true
rurge-policy.workspace = true
hyper.workspace = true
hyper-util.workspace = true
http-body-util.workspace = true
bytes.workspace = true
tempfile.workspace = true

[lints]
workspace = true
```

- [ ] **Step 2: 鉴权表的单元测试（先失败）**

`crates/rurge-api/src/auth.rs` 底部：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([10, 0, 0, last])
    }

    #[test]
    fn five_failures_in_the_window_ban_for_ten_minutes() {
        let auth = AuthState::new("k".into());
        let t0 = Instant::now();
        for i in 0..4 {
            assert!(!auth.record_failure(ip(1), t0 + Duration::from_secs(i)));
            assert!(!auth.is_banned(ip(1), t0 + Duration::from_secs(i)));
        }
        assert!(auth.record_failure(ip(1), t0 + Duration::from_secs(4)), "fifth failure bans");
        assert!(auth.is_banned(ip(1), t0 + Duration::from_secs(5)));
        assert!(auth.is_banned(ip(1), t0 + BAN_DURATION + Duration::from_secs(4)));
        assert!(!auth.is_banned(ip(1), t0 + BAN_DURATION + Duration::from_secs(5)), "ban expires");
        assert!(!auth.is_banned(ip(2), t0), "other sources are unaffected");
    }

    #[test]
    fn failures_outside_the_window_do_not_count() {
        let auth = AuthState::new("k".into());
        let t0 = Instant::now();
        for i in 0..4 {
            auth.record_failure(ip(1), t0 + Duration::from_secs(i));
        }
        // the fifth comes after the window slid past the first four
        assert!(!auth.record_failure(ip(1), t0 + BAN_WINDOW + Duration::from_secs(10)));
        assert!(!auth.is_banned(ip(1), t0 + BAN_WINDOW + Duration::from_secs(10)));
    }

    #[test]
    fn table_is_bounded() {
        let auth = AuthState::new("k".into());
        let t0 = Instant::now();
        for i in 0..(MAX_TRACKED as u32 + 50) {
            let addr = IpAddr::from(std::net::Ipv4Addr::from(0x0a00_0000 + i));
            auth.record_failure(addr, t0 + Duration::from_millis(i as u64));
        }
        assert!(auth.table.lock().unwrap().len() <= MAX_TRACKED);
    }

    #[test]
    fn key_compare_is_exact() {
        let auth = AuthState::new("s3cret".into());
        assert!(auth.key_matches("s3cret"));
        assert!(!auth.key_matches("s3cre"));
        assert!(!auth.key_matches("s3cret "));
        assert!(!auth.key_matches("S3CRET"));
    }
}
```

- [ ] **Step 3: 运行确认失败**

Run: `cargo test -p rurge-api`
Expected: 编译失败（crate 尚无实现）。

- [ ] **Step 4: 实现 `error.rs`、`auth.rs`、`lib.rs`**

`crates/rurge-api/src/error.rs`：

```rust
//! The unified error body (M4 design §5.3): every failure is
//! `{"error": "<message>"}` with the matching status code.

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> ApiError {
        ApiError {
            status,
            message: message.into(),
        }
    }
    pub fn bad_request(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::BAD_REQUEST, message)
    }
    pub fn unauthorized() -> ApiError {
        ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized")
    }
    pub fn banned() -> ApiError {
        ApiError::new(StatusCode::FORBIDDEN, "banned")
    }
    pub fn not_found(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::NOT_FOUND, message)
    }
    pub fn conflict(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::CONFLICT, message)
    }
    pub fn internal(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }
    pub fn not_implemented(message: impl Into<String>) -> ApiError {
        ApiError::new(StatusCode::NOT_IMPLEMENTED, message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

/// Unwraps a JSON body, turning axum's rejection (bad JSON, wrong
/// content-type, missing field) into our 400 body.
pub fn json_body<T>(body: Result<Json<T>, JsonRejection>) -> ApiResult<T> {
    match body {
        Ok(Json(v)) => Ok(v),
        Err(e) => Err(ApiError::bad_request(e.body_text())),
    }
}
```

`crates/rurge-api/src/auth.rs`：

```rust
//! `X-Key` / `?x-key=` authentication with a bounded ban table (M4 design
//! D7): five failures from one source within ten minutes ban it for ten
//! minutes; the table never grows past `MAX_TRACKED` sources.

use crate::App;
use crate::error::ApiError;
use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const BAN_FAILURES: usize = 5;
pub const BAN_WINDOW: Duration = Duration::from_secs(600);
pub const BAN_DURATION: Duration = Duration::from_secs(600);
const MAX_TRACKED: usize = 1024;

struct Entry {
    failures: VecDeque<Instant>,
    banned_until: Option<Instant>,
    touched: Instant,
}

pub struct AuthState {
    key: String,
    table: Mutex<HashMap<IpAddr, Entry>>,
}

impl AuthState {
    pub fn new(key: String) -> AuthState {
        AuthState {
            key,
            table: Mutex::new(HashMap::new()),
        }
    }

    pub fn key_matches(&self, presented: &str) -> bool {
        ct_eq(presented.as_bytes(), self.key.as_bytes())
    }

    pub fn is_banned(&self, ip: IpAddr, now: Instant) -> bool {
        let mut table = self.table.lock().expect("ban table");
        let Some(entry) = table.get_mut(&ip) else {
            return false;
        };
        match entry.banned_until {
            Some(until) if now < until => true,
            Some(_) => {
                entry.banned_until = None;
                entry.failures.clear();
                false
            }
            None => false,
        }
    }

    /// Records one failed attempt; `true` when this failure starts a ban.
    pub fn record_failure(&self, ip: IpAddr, now: Instant) -> bool {
        let mut table = self.table.lock().expect("ban table");
        if !table.contains_key(&ip) && table.len() >= MAX_TRACKED {
            // evict the least recently touched source
            if let Some(oldest) = table
                .iter()
                .min_by_key(|(_, e)| e.touched)
                .map(|(ip, _)| *ip)
            {
                table.remove(&oldest);
            }
        }
        let entry = table.entry(ip).or_insert_with(|| Entry {
            failures: VecDeque::new(),
            banned_until: None,
            touched: now,
        });
        entry.touched = now;
        while entry
            .failures
            .front()
            .is_some_and(|&f| now.duration_since(f) > BAN_WINDOW)
        {
            entry.failures.pop_front();
        }
        entry.failures.push_back(now);
        if entry.failures.len() >= BAN_FAILURES {
            entry.banned_until = Some(now + BAN_DURATION);
            entry.failures.clear();
            true
        } else {
            false
        }
    }
}

/// Equal-length inputs are compared without short-circuiting; the length
/// itself is not a secret.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn presented_key(req: &Request) -> Option<String> {
    if let Some(v) = req.headers().get("x-key")
        && let Ok(s) = v.to_str()
    {
        return Some(s.to_string());
    }
    req.uri().query().and_then(|q| {
        q.split('&')
            .find_map(|kv| kv.strip_prefix("x-key="))
            .map(str::to_string)
    })
}

pub async fn require_key(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> Response {
    let now = Instant::now();
    let ip = peer.ip();
    if app.auth.is_banned(ip, now) {
        return ApiError::banned().into_response();
    }
    match presented_key(&req) {
        Some(k) if app.auth.key_matches(&k) => next.run(req).await,
        _ => {
            if app.auth.record_failure(ip, now) {
                tracing::warn!(%ip, "http-api: too many failed attempts; source banned for 10 minutes");
            }
            ApiError::unauthorized().into_response()
        }
    }
}
```

`crates/rurge-api/src/lib.rs`：

```rust
//! Surge-compatible HTTP API (M4 design §5): the phase-1 endpoints, served
//! with axum over the engine's read-only views and the daemon's `Control`.

mod auth;
mod error;
mod routes;

use axum::Router;
use axum::middleware;
use axum::routing::{get, post};
use rurge_config::config::LoadOptions;
use rurge_engine::{Control, Engine};
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio_util::sync::CancellationToken;

pub use auth::{BAN_DURATION, BAN_FAILURES, BAN_WINDOW};

/// What the API needs from the daemon.
pub struct ApiContext {
    pub engine: Arc<Engine>,
    pub control: Arc<dyn Control>,
    /// The daemon's load options, so `POST /v1/profiles/check` validates the
    /// profile exactly as a reload would.
    pub load_options: LoadOptions,
}

pub(crate) struct Shared {
    pub(crate) engine: Arc<Engine>,
    pub(crate) control: Arc<dyn Control>,
    pub(crate) load_options: LoadOptions,
    pub(crate) auth: auth::AuthState,
    /// Unix seconds when the API came up (`/v1/traffic` `startTime`).
    pub(crate) started_secs: f64,
}

pub(crate) type App = Arc<Shared>;

pub type ServerFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

pub fn router(key: String, ctx: ApiContext) -> Router {
    let app: App = Arc::new(Shared {
        engine: ctx.engine,
        control: ctx.control,
        load_options: ctx.load_options,
        auth: auth::AuthState::new(key),
        started_secs: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0),
    });
    Router::new()
        .route(
            "/v1/outbound",
            get(routes::outbound::get_mode).post(routes::outbound::set_mode),
        )
        .route(
            "/v1/outbound/global",
            get(routes::outbound::get_global).post(routes::outbound::set_global),
        )
        .route(
            "/v1/features/{name}",
            get(routes::features::get_feature).post(routes::features::set_feature),
        )
        .route("/v1/modules", get(routes::misc::modules))
        .route("/v1/scripting", get(routes::misc::scripting))
        .route("/v1/events", get(routes::misc::events))
        .route("/v1/stop", post(routes::misc::stop))
        .fallback(routes::misc::not_found)
        .layer(middleware::from_fn_with_state(app.clone(), auth::require_key))
        .with_state(app)
}

/// Binds `addr` and returns the bound address plus the server future. The
/// caller spawns the future (the daemon puts it on the engine's task tracker);
/// it completes after `shutdown` is cancelled and in-flight responses finish.
pub async fn serve(
    addr: SocketAddr,
    key: String,
    ctx: ApiContext,
    shutdown: CancellationToken,
) -> io::Result<(SocketAddr, ServerFuture)> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let local = listener.local_addr()?;
    if !local.ip().is_loopback() {
        tracing::warn!(
            %local,
            "http-api listens on a non-loopback address: anyone who reaches it with the key controls this rurge"
        );
    }
    let app = router(key, ctx);
    let fut = async move {
        let result = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await;
        if let Err(e) = result {
            tracing::error!(error = %e, "http-api server stopped with an error");
        }
    };
    Ok((local, Box::pin(fut)))
}
```

- [ ] **Step 5: 路由：outbound、features、misc**

`crates/rurge-api/src/routes/mod.rs`：

```rust
pub mod features;
pub mod misc;
pub mod outbound;
```

`crates/rurge-api/src/routes/outbound.rs`：

```rust
//! `GET/POST /v1/outbound` and `/v1/outbound/global` (M4 design §5.2).

use crate::App;
use crate::error::{ApiError, ApiResult, json_body};
use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use rurge_engine::Mode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Serialize)]
pub struct ModeJson {
    pub mode: &'static str,
}

#[derive(Deserialize)]
pub struct SetMode {
    pub mode: String,
}

pub async fn get_mode(State(app): State<App>) -> Json<ModeJson> {
    Json(ModeJson {
        mode: app.engine.mode().as_str(),
    })
}

pub async fn set_mode(
    State(app): State<App>,
    body: Result<Json<SetMode>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let body = json_body(body)?;
    let mode = Mode::parse(&body.mode).ok_or_else(|| {
        ApiError::bad_request(format!(
            "unknown mode `{}`: expected direct, proxy or rule",
            body.mode
        ))
    })?;
    if mode == Mode::Proxy && app.engine.global_policy().is_none() {
        return Err(ApiError::bad_request(
            "proxy mode needs a global policy; set it with POST /v1/outbound/global first",
        ));
    }
    app.engine.set_mode(mode).await;
    tracing::info!(mode = mode.as_str(), "outbound mode changed via http-api");
    Ok(Json(json!({})))
}

#[derive(Serialize)]
pub struct GlobalJson {
    pub policy: Option<String>,
}

#[derive(Deserialize)]
pub struct SetGlobal {
    pub policy: String,
}

pub async fn get_global(State(app): State<App>) -> Json<GlobalJson> {
    Json(GlobalJson {
        policy: app.engine.global_policy(),
    })
}

pub async fn set_global(
    State(app): State<App>,
    body: Result<Json<SetGlobal>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let body = json_body(body)?;
    app.engine
        .set_global_policy(&body.policy)
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    tracing::info!(policy = %body.policy, "global policy changed via http-api");
    Ok(Json(json!({})))
}
```

`crates/rurge-api/src/routes/features.rs`：

```rust
//! `GET/POST /v1/features/{name}` (M4 design §5.2): every feature reads as
//! off in phase 1; only `system_proxy` can be switched, and only once M4b
//! implements `Control::set_system_proxy`.

use crate::App;
use crate::error::{ApiError, ApiResult, json_body};
use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use serde::Deserialize;
use serde_json::{Value, json};

const FEATURES: [&str; 6] = [
    "system_proxy",
    "enhanced_mode",
    "mitm",
    "capture",
    "rewrite",
    "scripting",
];

fn known(name: &str) -> ApiResult<()> {
    if FEATURES.contains(&name) {
        Ok(())
    } else {
        Err(ApiError::not_found(format!("unknown feature `{name}`")))
    }
}

pub async fn get_feature(Path(name): Path<String>) -> ApiResult<Json<Value>> {
    known(&name)?;
    Ok(Json(json!({ "enabled": false })))
}

#[derive(Deserialize)]
pub struct SetFeature {
    pub enabled: bool,
}

pub async fn set_feature(
    State(app): State<App>,
    Path(name): Path<String>,
    body: Result<Json<SetFeature>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    known(&name)?;
    let body = json_body(body)?;
    if name != "system_proxy" {
        return Err(ApiError::not_implemented(format!(
            "feature `{name}` is not available in this phase"
        )));
    }
    match app.control.set_system_proxy(body.enabled).await {
        Ok(()) => Ok(Json(json!({}))),
        Err(e) if e.contains("not implemented") => Err(ApiError::not_implemented(e)),
        Err(e) => Err(ApiError::internal(e)),
    }
}
```

`crates/rurge-api/src/routes/misc.rs`：

```rust
//! Empty phase-1 collections, `POST /v1/stop`, and the 404 fallback.

use crate::App;
use crate::error::ApiError;
use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};

pub async fn modules() -> Json<Value> {
    Json(json!({ "enabled": [], "available": [] }))
}

pub async fn scripting() -> Json<Value> {
    Json(json!({ "scripts": [] }))
}

pub async fn events() -> Json<Value> {
    Json(json!({ "events": [] }))
}

/// Answers `{}` first, then asks the daemon to stop: the stop cancels the
/// API's shutdown token, and a response still being written would race it.
pub async fn stop(State(app): State<App>) -> Json<Value> {
    tracing::info!("stop requested via http-api");
    let control = app.control.clone();
    tokio::spawn(async move { control.stop().await });
    Json(json!({}))
}

pub async fn not_found() -> ApiError {
    ApiError::not_found("no such endpoint")
}
```

- [ ] **Step 6: 单元测试通过**

Run: `cargo test -p rurge-api --lib`
Expected: 4 个 auth 测试 PASS。

- [ ] **Step 7: 集成测试 harness 与端点测试（先失败）**

`crates/rurge-api/tests/api.rs`：

```rust
//! Real engine (MockDns + TestServer + loopback listeners) behind the API,
//! driven with a hyper-util client (M4 design §9).

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use rurge_api::{ApiContext, serve};
use rurge_config::config::{LoadOptions, from_text};
use rurge_config::session::ListenerKind;
use rurge_dns::system::StaticSystemDns;
use rurge_dns::testing::MockDns;
use rurge_engine::control::{Control, LogLevel, ReloadReport};
use rurge_engine::stack::StackOptions;
use rurge_engine::state::{STATE_FILE, State, StateStore};
use rurge_engine::{Engine, ListenerSpec, Runtime, RuntimeOptions, Running};
use rurge_net::BoxFuture;
use rurge_net::testing::TestServer;
use rurge_policy::GroupSelections;
use rurge_rules::{GeoUrls, OutboundMode};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

const KEY: &str = "s3cret";

#[derive(Default)]
struct FakeControl {
    reloads: AtomicUsize,
    stops: AtomicUsize,
    levels: Mutex<Vec<LogLevel>>,
}

impl Control for FakeControl {
    fn reload(&self) -> BoxFuture<'_, ReloadReport> {
        Box::pin(async move {
            self.reloads.fetch_add(1, Ordering::SeqCst);
            ReloadReport {
                ok: true,
                errors: 0,
                warnings: 1,
                listeners_rebound: false,
            }
        })
    }
    fn stop(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.stops.fetch_add(1, Ordering::SeqCst);
        })
    }
    fn set_log_level(&self, level: LogLevel) -> Result<(), String> {
        self.levels.lock().unwrap().push(level);
        Ok(())
    }
    fn set_system_proxy(&self, _enabled: bool) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async { Err("not implemented".to_string()) })
    }
}

struct Api {
    _dir: tempfile::TempDir,
    conf: PathBuf,
    state_path: PathBuf,
    engine: Arc<Engine>,
    listeners: Vec<(ListenerSpec, Running)>,
    target: TestServer,
    dns: MockDns,
    control: Arc<FakeControl>,
    store: Arc<StateStore>,
    addr: SocketAddr,
    _token: CancellationToken,
}

impl Api {
    fn http(&self) -> SocketAddr {
        self.listeners
            .iter()
            .find(|(spec, _)| spec.kind == ListenerKind::Http)
            .map(|(_, r)| r.local_addr)
            .expect("http listener")
    }
    fn target_port(&self) -> u16 {
        self.target.url("/").port().unwrap()
    }
    fn target_url(&self) -> String {
        format!("http://target.test:{}/hello", self.target_port())
    }
}

/// The binary declares no proxy protocols, so drop shadowsocks from the test
/// capabilities to get the load-time W0007 warning the tests look for.
fn load_options() -> LoadOptions {
    let mut opts = LoadOptions::for_tests();
    opts.capabilities
        .policy_kinds
        .remove(&rurge_config::PolicyKind::Shadowsocks);
    opts
}

async fn api() -> Api {
    api_with("").await
}

/// `rules` are inserted before `DOMAIN,ads.test,REJECT` / `FINAL,DIRECT`.
async fn api_with(rules: &str) -> Api {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    dns.set("cached.test", &["10.0.0.1"], &[], 300);
    let target = TestServer::spawn().await;
    target.set("/hello", "hi there");
    let dir = tempfile::tempdir().unwrap();
    let conf = dir.path().join("t.conf");
    let profile = format!(
        "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\ndns-server = {}\nipv6 = false\n\
internet-test-url = http://target.test:{}/hello\n\
[Proxy]\nHK = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\nBlock = reject-tinygif\n\
[Proxy Group]\nPick = select, HK, DIRECT\n\
[Rule]\n{rules}\nDOMAIN,ads.test,REJECT\nFINAL,DIRECT\n",
        dns.addr(),
        target.url("/").port().unwrap()
    );
    std::fs::write(&conf, &profile).unwrap();
    let loaded = from_text(&profile, &conf, &load_options());
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
            selections: GroupSelections::new(),
            request_log_size: 1000,
        },
    )
    .await
    .unwrap();
    let engine = Engine::new(runtime);
    let state_path = dir.path().join(STATE_FILE);
    let (store, _) = StateStore::open(state_path.clone()).await;
    engine.attach_state(store.clone());
    engine.start_sampler();
    let listeners = engine.bind_listeners().await.unwrap();
    let control = Arc::new(FakeControl::default());
    let token = CancellationToken::new();
    let (addr, fut) = serve(
        "127.0.0.1:0".parse().unwrap(),
        KEY.to_string(),
        ApiContext {
            engine: engine.clone(),
            control: control.clone(),
            load_options: load_options(),
        },
        token.clone(),
    )
    .await
    .unwrap();
    tokio::spawn(fut);
    Api {
        _dir: dir,
        conf,
        state_path,
        engine,
        listeners,
        target,
        dns,
        control,
        store,
        addr,
        _token: token,
    }
}

async fn call_raw(
    addr: SocketAddr,
    method: &str,
    path: &str,
    key: Option<&str>,
    body: Option<Value>,
) -> (u16, String, Bytes) {
    let client = Client::builder(TokioExecutor::new()).build_http::<Full<Bytes>>();
    let mut req = Request::builder()
        .method(method)
        .uri(format!("http://{addr}{path}"));
    if let Some(k) = key {
        req = req.header("x-key", k);
    }
    let body = match body {
        Some(v) => {
            req = req.header("content-type", "application/json");
            Full::new(Bytes::from(v.to_string()))
        }
        None => Full::new(Bytes::new()),
    };
    let resp = client.request(req.body(body).unwrap()).await.unwrap();
    let status = resp.status().as_u16();
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, content_type, bytes)
}

async fn call(addr: SocketAddr, method: &str, path: &str, key: Option<&str>, body: Option<Value>) -> (u16, Value) {
    let (status, _, bytes) = call_raw(addr, method, path, key, body).await;
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    (status, value)
}

async fn get(api: &Api, path: &str) -> (u16, Value) {
    call(api.addr, "GET", path, Some(KEY), None).await
}

async fn post(api: &Api, path: &str, body: Value) -> (u16, Value) {
    call(api.addr, "POST", path, Some(KEY), Some(body)).await
}

/// Plain HTTP GET through the proxy listener; returns (status line, body).
async fn get_via_proxy(proxy: SocketAddr, url: &str) -> (String, Vec<u8>) {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    let host = url
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap()
        .to_string();
    s.write_all(format!("GET {url} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut buf)).await;
    let text = String::from_utf8_lossy(&buf).into_owned();
    let (head, body) = match text.find("\r\n\r\n") {
        Some(pos) => (text[..pos].to_string(), buf[pos + 4..].to_vec()),
        None => (text, Vec::new()),
    };
    (head.lines().next().unwrap_or("").to_string(), body)
}

/// Polls `f` every 50 ms for up to 5 s.
async fn wait_until(mut f: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !f().await {
        assert!(tokio::time::Instant::now() < deadline, "condition not met in time");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn rejects_missing_or_wrong_key_and_bans_after_five_failures() {
    let api = api().await;
    let (status, body) = call(api.addr, "GET", "/v1/outbound", None, None).await;
    assert_eq!((status, body), (401, json!({ "error": "unauthorized" })));
    // the query form works too
    let (status, body) = call(api.addr, "GET", &format!("/v1/outbound?x-key={KEY}"), None, None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["mode"], "rule");
    for _ in 0..4 {
        let (status, _) = call(api.addr, "GET", "/v1/outbound", Some("wrong"), None).await;
        assert_eq!(status, 401);
    }
    let (status, _) = call(api.addr, "GET", "/v1/outbound", Some("wrong"), None).await;
    assert_eq!(status, 401, "the fifth failure still answers 401");
    let (status, body) = get(&api, "/v1/outbound").await;
    assert_eq!((status, body), (403, json!({ "error": "banned" })), "even the right key is banned now");
}

#[tokio::test]
async fn outbound_mode_and_global_policy_round_trip_and_persist() {
    let api = api().await;
    let (status, body) = get(&api, "/v1/outbound").await;
    assert_eq!((status, body), (200, json!({ "mode": "rule" })));
    let (status, body) = post(&api, "/v1/outbound", json!({ "mode": "proxy" })).await;
    assert_eq!(status, 400, "{body}");
    assert!(body["error"].as_str().unwrap().contains("global policy"));
    let (status, body) = post(&api, "/v1/outbound/global", json!({ "policy": "nope" })).await;
    assert_eq!(status, 400, "{body}");
    let (status, body) = post(&api, "/v1/outbound/global", json!({ "policy": "Pick" })).await;
    assert_eq!((status, body), (200, json!({})));
    let (status, body) = post(&api, "/v1/outbound", json!({ "mode": "proxy" })).await;
    assert_eq!((status, body), (200, json!({})));
    assert_eq!(get(&api, "/v1/outbound").await.1, json!({ "mode": "proxy" }));
    assert_eq!(get(&api, "/v1/outbound/global").await.1, json!({ "policy": "Pick" }));
    let on_disk: State = serde_json::from_str(&std::fs::read_to_string(&api.state_path).unwrap()).unwrap();
    assert_eq!(on_disk.outbound_mode.as_deref(), Some("proxy"));
    assert_eq!(on_disk.global_policy.as_deref(), Some("Pick"));
    assert_eq!(api.store.snapshot().await, on_disk);
    // bad inputs
    let (status, body) = post(&api, "/v1/outbound", json!({ "mode": "loud" })).await;
    assert_eq!(status, 400, "{body}");
    let (status, body) = post(&api, "/v1/outbound", json!({ "nope": 1 })).await;
    assert_eq!(status, 400, "{body}");
    assert!(body["error"].is_string());
    let (status, _, bytes) = call_raw(api.addr, "POST", "/v1/outbound", Some(KEY), None).await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&bytes));
}

#[tokio::test]
async fn features_collections_stop_and_unknown_paths() {
    let api = api().await;
    assert_eq!(get(&api, "/v1/features/mitm").await, (200, json!({ "enabled": false })));
    let (status, _) = post(&api, "/v1/features/mitm", json!({ "enabled": true })).await;
    assert_eq!(status, 501);
    let (status, _) = post(&api, "/v1/features/system_proxy", json!({ "enabled": true })).await;
    assert_eq!(status, 501, "M4a: not implemented");
    assert_eq!(get(&api, "/v1/features/teleport").await.0, 404);
    assert_eq!(get(&api, "/v1/modules").await.1, json!({ "enabled": [], "available": [] }));
    assert_eq!(get(&api, "/v1/scripting").await.1, json!({ "scripts": [] }));
    assert_eq!(get(&api, "/v1/events").await.1, json!({ "events": [] }));
    let (status, body) = get(&api, "/v1/nope").await;
    assert_eq!((status, body), (404, json!({ "error": "no such endpoint" })));
    assert_eq!(post(&api, "/v1/stop", json!({})).await, (200, json!({})));
    wait_until(async || api.control.stops.load(Ordering::SeqCst) == 1).await;
}
```

（`AsyncFnMut` 是 Rust 1.85 起的稳定语法，MSRV 1.88 可用。`Api` 的 `conf`、`engine`、`dns`、`target` 等字段在 Task 5 / 6 使用；本任务允许 `dead_code` 警告——给 struct 加 `#[allow(dead_code)]`，Task 6 结束时移除。）

- [ ] **Step 8: 运行确认失败，再通过**

Run: `cargo test -p rurge-api --test api`
Expected: 先编译失败（缺少 `serve` 等）→ 实现 Step 4/5 后 3 个测试 PASS。注意 `into_make_service_with_connect_info::<SocketAddr>()` 缺失时 `ConnectInfo` 提取会 500——所有测试会一起失败，先查这个。

- [ ] **Step 9: 质量门与提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add Cargo.toml Cargo.lock crates/rurge-api
git commit -F - <<'EOF'
feat(api): rurge-api 骨架：axum 服务、X-Key 鉴权与封禁、统一错误体、outbound / features / stop

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 5: 观测端点——policies / rules / requests（recent、active、kill）/ traffic

**Files:**
- Create: `crates/rurge-api/src/routes/policies.rs`、`routes/requests.rs`、`routes/traffic.rs`
- Modify: `crates/rurge-api/src/routes/mod.rs`、`src/lib.rs`（路由）、`tests/api.rs`

**Interfaces:**
- Consumes: Task 2 的 `Engine::{policies_view, rules_view}`；`Engine::{request_log, traffic, kill}`；`RequestLog::{recent, active}`；`TrafficStats::{totals, by_listener, by_policy, rate}`；`Shared.started_secs`。
- Produces: JSON 形状（写进 Task 9 的 `docs/api/phase1.md`）：
  - `GET /v1/policies` → `{"proxies":[..],"policy-groups":[..]}`
  - `GET /v1/rules` → `{"rules":[{"index":0,"rule":"DOMAIN,…","hits":3}]}`
  - `GET /v1/requests/recent?limit=N`（默认 100）/ `GET /v1/requests/active` → `{"requests":[{"id","listener","src","dst","rule","policy":[..],"sni","protocol","up","down","startedMs","elapsedMs","status","rejectKind","error"}]}`；`listener` ∈ `http|socks5|tun|forward|internal`；`status` ∈ `active|completed|rejected|failed`；`protocol` 小写变体名（`mtproto`、`doh3`…）或 `null`
  - `POST /v1/requests/kill {"id":N}` → `{}` / 404 `no active request with id N` / 409 `not killable: internal session`
  - `GET /v1/traffic` → `{"startTime":<unix 秒>,"total":{"in","out","inCurrentSpeed","outCurrentSpeed"},"connector":{"<policy>":{"in","out"}},"listener":{"http":{"in","out"},"socks5":{"in","out"}}}`（`in` = 下行 down，`out` = 上行 up）
  - crate 内纯函数 `requests::kill_check(active: &[RequestRecord], id: u64) -> ApiResult<()>`

- [ ] **Step 1: 失败测试**

`crates/rurge-api/src/routes/requests.rs` 先建文件放单元测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rurge_engine::RecordStatus;

    fn record(id: u64, listener: ListenerKind) -> RequestRecord {
        RequestRecord {
            id,
            listener,
            src: "127.0.0.1:1".parse().unwrap(),
            dst: "1.1.1.1:443".into(),
            rule: None,
            policy: vec![],
            sni: None,
            protocol: None,
            up: 0,
            down: 0,
            started_ms: 0,
            elapsed_ms: 0,
            status: RecordStatus::Active,
            error: None,
        }
    }

    #[test]
    fn kill_check_distinguishes_missing_internal_and_killable() {
        let active = [record(1, ListenerKind::Http), record(2, ListenerKind::Internal)];
        assert!(kill_check(&active, 1).is_ok());
        let e = kill_check(&active, 2).unwrap_err();
        assert_eq!(e.status.as_u16(), 409);
        assert_eq!(e.message, "not killable: internal session");
        let e = kill_check(&active, 3).unwrap_err();
        assert_eq!(e.status.as_u16(), 404);
    }

    #[test]
    fn json_names_are_lowercase_and_reject_kind_is_split_out() {
        let mut r = record(1, ListenerKind::Socks5);
        r.status = RecordStatus::Rejected("REJECT-TINYGIF".into());
        r.protocol = Some(rurge_config::rule::ProtocolKind::MtProto);
        let j = RequestJson::from(&r);
        assert_eq!(j.listener, "socks5");
        assert_eq!(j.status, "rejected");
        assert_eq!(j.reject_kind.as_deref(), Some("REJECT-TINYGIF"));
        assert_eq!(j.protocol, Some("mtproto"));
    }
}
```

`tests/api.rs` 追加：

```rust
#[tokio::test]
async fn policies_and_rules_are_listed_with_hits() {
    let api = api().await;
    let (status, body) = get(&api, "/v1/policies").await;
    assert_eq!(status, 200);
    let proxies: Vec<&str> = body["proxies"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert!(proxies.contains(&"DIRECT") && proxies.contains(&"REJECT-TINYGIF") && proxies.contains(&"HK") && proxies.contains(&"Block"), "{proxies:?}");
    assert_eq!(body["policy-groups"], json!(["Pick"]));
    let (head, _) = get_via_proxy(api.http(), &api.target_url()).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let (_, body) = get(&api, "/v1/rules").await;
    let rules = body["rules"].as_array().unwrap();
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0]["index"], 0);
    assert_eq!(rules[0]["rule"], "DOMAIN,ads.test,REJECT");
    assert_eq!(rules[0]["hits"], 0);
    assert_eq!(rules[1]["rule"], "FINAL,DIRECT");
    assert!(rules[1]["hits"].as_u64().unwrap() >= 1);
}

#[tokio::test]
async fn recent_and_active_requests_and_kill() {
    let api = api().await;
    let (head, _) = get_via_proxy(api.http(), &api.target_url()).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let mut recent = Vec::new();
    wait_until(async || {
        recent = get(&api, "/v1/requests/recent").await.1["requests"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        recent.iter().any(|r| r["status"] == "completed")
    })
    .await;
    let r = recent.iter().find(|r| r["status"] == "completed").unwrap();
    assert_eq!(r["listener"], "http");
    assert_eq!(r["dst"], format!("target.test:{}", api.target_port()));
    assert_eq!(r["policy"], json!(["DIRECT"]));
    assert_eq!(r["rule"], "FINAL,DIRECT");
    assert!(r["up"].as_u64().unwrap() > 0 && r["down"].as_u64().unwrap() > 0);
    assert!(r["startedMs"].as_u64().unwrap() > 0);
    assert!(r["rejectKind"].is_null() && r["error"].is_null());
    assert_eq!(get(&api, "/v1/requests/recent?limit=1").await.1["requests"].as_array().unwrap().len(), 1);
    let (status, body) = post(&api, "/v1/requests/kill", json!({ "id": 999_999 })).await;
    assert_eq!(status, 404, "{body}");
    // a CONNECT tunnel stays active until killed
    let mut tunnel = TcpStream::connect(api.http()).await.unwrap();
    let dst = format!("target.test:{}", api.target_port());
    tunnel
        .write_all(format!("CONNECT {dst} HTTP/1.1\r\nHost: {dst}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut buf = [0u8; 256];
    let n = tunnel.read(&mut buf).await.unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 200"));
    let mut active_id = 0;
    wait_until(async || {
        let body = get(&api, "/v1/requests/active").await.1;
        let found = body["requests"].as_array().unwrap().iter().find(|r| r["dst"] == dst && r["status"] == "active").cloned();
        if let Some(r) = found {
            active_id = r["id"].as_u64().unwrap();
        }
        active_id != 0
    })
    .await;
    assert_eq!(post(&api, "/v1/requests/kill", json!({ "id": active_id })).await, (200, json!({})));
    let closed = tokio::time::timeout(Duration::from_secs(3), tunnel.read(&mut buf)).await;
    assert!(matches!(closed, Ok(Ok(0)) | Ok(Err(_))), "killed tunnel closes: {closed:?}");
    wait_until(async || get(&api, "/v1/requests/active").await.1["requests"].as_array().unwrap().iter().all(|r| r["id"] != active_id)).await;
}

#[tokio::test]
async fn traffic_reports_totals_by_policy_and_listener() {
    let api = api().await;
    let (head, _) = get_via_proxy(api.http(), &api.target_url()).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let mut body = Value::Null;
    wait_until(async || {
        body = get(&api, "/v1/traffic").await.1;
        body["total"]["in"].as_u64().unwrap_or(0) > 0
    })
    .await;
    assert!(body["startTime"].as_f64().unwrap() > 1.0e9);
    assert!(body["total"]["out"].as_u64().unwrap() > 0);
    assert!(body["total"]["inCurrentSpeed"].is_u64() && body["total"]["outCurrentSpeed"].is_u64());
    assert!(body["connector"]["DIRECT"]["in"].as_u64().unwrap() > 0);
    assert!(body["listener"]["http"]["out"].as_u64().unwrap() > 0);
    assert_eq!(body["listener"]["socks5"]["in"], 0);
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p rurge-api`
Expected: 编译失败（`RequestJson`、`kill_check` 不存在；路由不存在时集成测试 404）。

- [ ] **Step 3: 实现三个路由文件**

`routes/policies.rs`：

```rust
//! `GET /v1/policies` and `GET /v1/rules`.

use crate::App;
use axum::Json;
use axum::extract::State;
use serde::Serialize;

#[derive(Serialize)]
pub struct PoliciesJson {
    pub proxies: Vec<String>,
    #[serde(rename = "policy-groups")]
    pub policy_groups: Vec<String>,
}

pub async fn policies(State(app): State<App>) -> Json<PoliciesJson> {
    let view = app.engine.policies_view();
    Json(PoliciesJson {
        proxies: view.proxies,
        policy_groups: view.groups,
    })
}

#[derive(Serialize)]
pub struct RuleJson {
    pub index: usize,
    pub rule: String,
    pub hits: u64,
}

#[derive(Serialize)]
pub struct RulesJson {
    pub rules: Vec<RuleJson>,
}

pub async fn rules(State(app): State<App>) -> Json<RulesJson> {
    let rules = app
        .engine
        .rules_view()
        .into_iter()
        .map(|r| RuleJson {
            index: r.index,
            rule: r.rule,
            hits: r.hits,
        })
        .collect();
    Json(RulesJson { rules })
}
```

`routes/requests.rs`（测试之前）：

```rust
//! `GET /v1/requests/recent|active` and `POST /v1/requests/kill`.

use crate::App;
use crate::error::{ApiError, ApiResult, json_body};
use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Query, State};
use rurge_config::rule::ProtocolKind;
use rurge_config::session::ListenerKind;
use rurge_engine::{RecordStatus, RequestRecord};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const DEFAULT_LIMIT: usize = 100;

pub(crate) fn listener_name(kind: ListenerKind) -> &'static str {
    match kind {
        ListenerKind::Http => "http",
        ListenerKind::Socks5 => "socks5",
        ListenerKind::Tun => "tun",
        ListenerKind::Forward => "forward",
        ListenerKind::Internal => "internal",
    }
}

fn protocol_name(p: ProtocolKind) -> &'static str {
    match p {
        ProtocolKind::Http => "http",
        ProtocolKind::Https => "https",
        ProtocolKind::Tcp => "tcp",
        ProtocolKind::Udp => "udp",
        ProtocolKind::Quic => "quic",
        ProtocolKind::Stun => "stun",
        ProtocolKind::MtProto => "mtproto",
        ProtocolKind::Doh => "doh",
        ProtocolKind::Doh3 => "doh3",
        ProtocolKind::Doq => "doq",
        ProtocolKind::Dot => "dot",
        ProtocolKind::Dns => "dns",
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestJson {
    pub id: u64,
    pub listener: &'static str,
    pub src: String,
    pub dst: String,
    pub rule: Option<String>,
    pub policy: Vec<String>,
    pub sni: Option<String>,
    pub protocol: Option<&'static str>,
    pub up: u64,
    pub down: u64,
    pub started_ms: u64,
    pub elapsed_ms: u64,
    pub status: &'static str,
    pub reject_kind: Option<String>,
    pub error: Option<String>,
}

impl From<&RequestRecord> for RequestJson {
    fn from(r: &RequestRecord) -> RequestJson {
        let (status, reject_kind) = match &r.status {
            RecordStatus::Active => ("active", None),
            RecordStatus::Completed => ("completed", None),
            RecordStatus::Rejected(kind) => ("rejected", Some(kind.clone())),
            RecordStatus::Failed => ("failed", None),
        };
        RequestJson {
            id: r.id,
            listener: listener_name(r.listener),
            src: r.src.to_string(),
            dst: r.dst.clone(),
            rule: r.rule.clone(),
            policy: r.policy.clone(),
            sni: r.sni.clone(),
            protocol: r.protocol.map(protocol_name),
            up: r.up,
            down: r.down,
            started_ms: r.started_ms,
            elapsed_ms: r.elapsed_ms,
            status,
            reject_kind,
            error: r.error.clone(),
        }
    }
}

#[derive(Serialize)]
pub struct RequestsJson {
    pub requests: Vec<RequestJson>,
}

#[derive(Deserialize)]
pub struct RecentQuery {
    pub limit: Option<usize>,
}

pub async fn recent(State(app): State<App>, Query(q): Query<RecentQuery>) -> Json<RequestsJson> {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).max(1);
    let requests = app
        .engine
        .request_log()
        .recent(limit)
        .iter()
        .map(RequestJson::from)
        .collect();
    Json(RequestsJson { requests })
}

pub async fn active(State(app): State<App>) -> Json<RequestsJson> {
    let requests = app
        .engine
        .request_log()
        .active()
        .iter()
        .map(RequestJson::from)
        .collect();
    Json(RequestsJson { requests })
}

#[derive(Deserialize)]
pub struct KillBody {
    pub id: u64,
}

/// 404 when the id is not in flight, 409 for rurge's own sessions (M4 §12).
pub(crate) fn kill_check(active: &[RequestRecord], id: u64) -> ApiResult<()> {
    match active.iter().find(|r| r.id == id) {
        None => Err(ApiError::not_found(format!("no active request with id {id}"))),
        Some(r) if r.listener == ListenerKind::Internal => {
            Err(ApiError::conflict("not killable: internal session"))
        }
        Some(_) => Ok(()),
    }
}

pub async fn kill(
    State(app): State<App>,
    body: Result<Json<KillBody>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let body = json_body(body)?;
    kill_check(&app.engine.request_log().active(), body.id)?;
    if !app.engine.kill(body.id) {
        return Err(ApiError::not_found(format!("no active request with id {}", body.id)));
    }
    tracing::info!(id = body.id, "request killed via http-api");
    Ok(Json(json!({})))
}
```

`routes/traffic.rs`：

```rust
//! `GET /v1/traffic` (Surge's shape: `in` is bytes downloaded, `out` uploaded).

use crate::App;
use crate::routes::requests::listener_name;
use axum::Json;
use axum::extract::State;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Serialize)]
pub struct Bytes {
    #[serde(rename = "in")]
    pub down: u64,
    pub out: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Total {
    #[serde(rename = "in")]
    pub down: u64,
    pub out: u64,
    pub in_current_speed: u64,
    pub out_current_speed: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficJson {
    pub start_time: f64,
    pub total: Total,
    pub connector: BTreeMap<String, Bytes>,
    pub listener: BTreeMap<&'static str, Bytes>,
}

pub async fn traffic(State(app): State<App>) -> Json<TrafficJson> {
    let stats = app.engine.traffic();
    let totals = stats.totals();
    let (rate_up, rate_down) = stats.rate();
    let connector = stats
        .by_policy()
        .into_iter()
        .map(|(name, up, down)| (name, Bytes { down, out: up }))
        .collect();
    let listener = stats
        .by_listener()
        .into_iter()
        .map(|(kind, up, down)| (listener_name(kind), Bytes { down, out: up }))
        .collect::<BTreeMap<_, _>>();
    Json(TrafficJson {
        start_time: app.started_secs,
        total: Total {
            down: totals.down,
            out: totals.up,
            in_current_speed: rate_down,
            out_current_speed: rate_up,
        },
        connector,
        listener,
    })
}
```

`routes/mod.rs` 加 `pub mod policies; pub mod requests; pub mod traffic;`；`lib.rs` 的 `Router` 加：

```rust
        .route("/v1/policies", get(routes::policies::policies))
        .route("/v1/rules", get(routes::policies::rules))
        .route("/v1/requests/recent", get(routes::requests::recent))
        .route("/v1/requests/active", get(routes::requests::active))
        .route("/v1/requests/kill", post(routes::requests::kill))
        .route("/v1/traffic", get(routes::traffic::traffic))
```

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p rurge-api`（`--test api` 3×）
Expected: PASS。若 `recent` 里 `policy` 断言失败，看 `RequestRecord.policy` 是否含链式前缀——M3b 里 DIRECT 会话的链是 `["DIRECT"]`。

- [ ] **Step 5: 质量门与提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-api
git commit -F - <<'EOF'
feat(api): policies / rules / requests / kill / traffic 端点

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 6: DNS、profiles（current / reload / check）与日志级别端点

**Files:**
- Create: `crates/rurge-api/src/routes/dns.rs`、`routes/profiles.rs`、`routes/log.rs`
- Modify: `crates/rurge-api/src/routes/mod.rs`、`src/lib.rs`、`tests/api.rs`

**Interfaces:**
- Consumes: `Resolver::{cache_snapshot, flush, measure_delay, primary_upstreams, bootstrap_upstreams}`（经 `engine.runtime().stack.resolver`）；`General.internet_test_url`；Task 2 的 `Engine::config_text`；Task 3 的 `Control::{reload, set_log_level}`、`LogLevel`；`Shared.load_options`；`rurge_config::config::load`。
- Produces（JSON 形状进 `docs/api/phase1.md`）：
  - `GET /v1/dns` → `{"dnsCache":[{"domain","data":[..],"expiresTime":<unix 秒|null>,"server","stale","negative"}],"upstreams":[..],"bootstrap":[..]}`
  - `POST /v1/dns/flush` → `{}`
  - `POST /v1/test/dns_delay {"name"?}` → `{"delays":[{"upstream","ms":<数|null>,"error":<串|null>}]}`；缺省 `name` 为 `internet-test-url` 的主机名
  - `GET /v1/profiles/current?sensitive=0|1` → `text/plain; charset=utf-8`
  - `POST /v1/profiles/reload` → `{"ok","errors","warnings","listenersRebound"}`
  - `POST /v1/profiles/check` → `{"ok","errors","warnings","diagnostics":[Diagnostic…]}`（`Diagnostic` 的 serde 输出，与 `rurge check --json` 一致）
  - `POST /v1/log/level {"level"}` → `{}`；未知级别 400
  - crate 内纯函数 `dns::host_of(url: &str) -> Option<String>`

- [ ] **Step 1: 失败测试**

`routes/dns.rs` 先放单元测试：

```rust
#[cfg(test)]
mod tests {
    use super::host_of;

    #[test]
    fn host_of_extracts_the_authority_host() {
        assert_eq!(host_of("http://bing.com/").as_deref(), Some("bing.com"));
        assert_eq!(host_of("http://target.test:8080/hello?x=1").as_deref(), Some("target.test"));
        assert_eq!(host_of("https://user:pw@example.com").as_deref(), Some("example.com"));
        assert_eq!(host_of("http://[::1]:53/").as_deref(), Some("::1"));
        assert_eq!(host_of(""), None);
    }
}
```

`tests/api.rs` 追加：

```rust
#[tokio::test]
async fn dns_cache_flush_and_delay() {
    let api = api().await;
    let resolver = api.engine.runtime().stack.resolver.clone();
    resolver
        .lookup("cached.test", rurge_dns::LookupOpts::default())
        .await
        .unwrap();
    let (status, body) = get(&api, "/v1/dns").await;
    assert_eq!(status, 200);
    let entry = body["dnsCache"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["domain"] == "cached.test")
        .cloned()
        .expect("cached entry");
    assert_eq!(entry["data"], json!(["10.0.0.1"]));
    assert!(entry["expiresTime"].as_f64().unwrap() > 1.0e9);
    assert!(entry["server"].as_str().unwrap().contains(&api.dns.addr().to_string()), "{entry}");
    assert_eq!(entry["stale"], false);
    let upstreams = body["upstreams"].as_array().unwrap();
    assert!(upstreams.iter().any(|u| u.as_str().unwrap().contains(&api.dns.addr().to_string())), "{upstreams:?}");
    assert!(body["bootstrap"].is_array());
    assert_eq!(post(&api, "/v1/dns/flush", json!({})).await, (200, json!({})));
    assert!(get(&api, "/v1/dns").await.1["dnsCache"].as_array().unwrap().is_empty());
    let (status, body) = post(&api, "/v1/test/dns_delay", json!({ "name": "target.test" })).await;
    assert_eq!(status, 200, "{body}");
    let delays = body["delays"].as_array().unwrap();
    assert_eq!(delays.len(), 1, "{delays:?}");
    assert!(delays[0]["upstream"].as_str().unwrap().contains(&api.dns.addr().to_string()));
    assert!(delays[0]["ms"].is_number() && delays[0]["error"].is_null(), "{delays:?}");
    // no name → the host of internet-test-url (target.test)
    let (status, body) = post(&api, "/v1/test/dns_delay", json!({})).await;
    assert_eq!(status, 200, "{body}");
    assert!(body["delays"][0]["ms"].is_number());
    assert!(api.dns.query_count("target.test", rurge_dns::message::Qtype::A) >= 2);
}

#[tokio::test]
async fn profiles_current_check_and_reload() {
    let api = api().await;
    let (status, content_type, bytes) = call_raw(api.addr, "GET", "/v1/profiles/current", Some(KEY), None).await;
    assert_eq!(status, 200);
    assert!(content_type.starts_with("text/plain"), "{content_type}");
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.contains("password=***") && !text.contains("password=x"), "{text}");
    let (_, _, bytes) = call_raw(api.addr, "GET", "/v1/profiles/current?sensitive=1", Some(KEY), None).await;
    assert!(String::from_utf8_lossy(&bytes).contains("password=x"));
    let (status, body) = post(&api, "/v1/profiles/check", json!({})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["ok"], true);
    assert_eq!(body["errors"], 0);
    assert!(body["warnings"].as_u64().unwrap() >= 1, "W0007 for the unsupported ss policy: {body}");
    assert!(body["diagnostics"].as_array().unwrap().iter().any(|d| d["code"] == "W0007"), "{body}");
    // break the file on disk: check reports it, the running config is untouched
    let broken = std::fs::read_to_string(&api.conf).unwrap().replace("FINAL,DIRECT", "FINAL,NoSuchPolicy");
    std::fs::write(&api.conf, broken).unwrap();
    let (status, body) = post(&api, "/v1/profiles/check", json!({})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["ok"], false);
    assert!(body["errors"].as_u64().unwrap() >= 1);
    assert!(body["diagnostics"].as_array().unwrap().iter().any(|d| d["code"] == "E0007"), "{body}");
    let (head, _) = get_via_proxy(api.http(), &api.target_url()).await;
    assert!(head.starts_with("HTTP/1.1 200"), "still serving: {head}");
    let (status, body) = post(&api, "/v1/profiles/reload", json!({})).await;
    assert_eq!(status, 200);
    assert_eq!(body, json!({ "ok": true, "errors": 0, "warnings": 1, "listenersRebound": false }));
    assert_eq!(api.control.reloads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn log_level_is_forwarded_to_control() {
    let api = api().await;
    assert_eq!(post(&api, "/v1/log/level", json!({ "level": "debug" })).await, (200, json!({})));
    assert_eq!(*api.control.levels.lock().unwrap(), vec![LogLevel::Debug]);
    let (status, body) = post(&api, "/v1/log/level", json!({ "level": "loud" })).await;
    assert_eq!(status, 400, "{body}");
}
```

（`rurge_dns::message::Qtype` 是 MockDns 计数用的类型；若路径不同以 `crates/rurge/tests/cli.rs` 里 `dns lookup` 测试的 import 为准。）

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p rurge-api`
Expected: 编译失败 / 404。

- [ ] **Step 3: 实现**

`routes/dns.rs`：

```rust
//! `GET /v1/dns`, `POST /v1/dns/flush`, `POST /v1/test/dns_delay`.

use crate::App;
use crate::error::{ApiError, ApiResult, json_body};
use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheJson {
    pub domain: String,
    pub data: Vec<String>,
    pub expires_time: Option<f64>,
    pub server: String,
    pub stale: bool,
    pub negative: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DnsJson {
    pub dns_cache: Vec<CacheJson>,
    pub upstreams: Vec<String>,
    pub bootstrap: Vec<String>,
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub async fn dns(State(app): State<App>) -> Json<DnsJson> {
    let resolver = app.engine.runtime().stack.resolver.clone();
    let now = now_secs();
    let dns_cache = resolver
        .cache_snapshot()
        .into_iter()
        .map(|e| CacheJson {
            domain: e.name,
            data: e
                .v4
                .iter()
                .map(|a| a.to_string())
                .chain(e.v6.iter().map(|a| a.to_string()))
                .collect(),
            expires_time: e.expires_in.map(|d| now + d.as_secs_f64()),
            server: e.source,
            stale: e.stale,
            negative: e.negative,
        })
        .collect();
    Json(DnsJson {
        dns_cache,
        upstreams: resolver.primary_upstreams(),
        bootstrap: resolver.bootstrap_upstreams(),
    })
}

pub async fn flush(State(app): State<App>) -> Json<Value> {
    app.engine.runtime().stack.resolver.flush();
    tracing::info!("dns cache flushed via http-api");
    Json(json!({}))
}

#[derive(Deserialize, Default)]
pub struct DelayBody {
    pub name: Option<String>,
}

#[derive(Serialize)]
pub struct DelayJson {
    pub upstream: String,
    pub ms: Option<u64>,
    pub error: Option<String>,
}

/// The host part of a URL without pulling in a URL parser: strips the scheme,
/// userinfo, port, path and query.
pub(crate) fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next()?,
        None => authority.split(':').next()?,
    };
    (!host.is_empty()).then(|| host.to_string())
}

pub async fn dns_delay(
    State(app): State<App>,
    body: Result<Json<DelayBody>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let body = json_body(body)?;
    let rt = app.engine.runtime();
    let name = match body.name.filter(|n| !n.trim().is_empty()) {
        Some(n) => n,
        None => host_of(&rt.config.general.internet_test_url).ok_or_else(|| {
            ApiError::bad_request("no name given and internet-test-url has no host")
        })?,
    };
    let delays: Vec<DelayJson> = rt
        .stack
        .resolver
        .measure_delay(&name)
        .await
        .into_iter()
        .map(|d| match d.result {
            Ok(elapsed) => DelayJson {
                upstream: d.upstream,
                ms: Some(elapsed.as_millis() as u64),
                error: None,
            },
            Err(e) => DelayJson {
                upstream: d.upstream,
                ms: None,
                error: Some(e),
            },
        })
        .collect();
    Ok(Json(json!({ "delays": delays })))
}
```

`routes/profiles.rs`：

```rust
//! `GET /v1/profiles/current`, `POST /v1/profiles/reload|check` (M4 D3).

use crate::App;
use crate::error::{ApiError, ApiResult};
use axum::Json;
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::IntoResponse;
use rurge_config::Severity;
use rurge_config::config::load;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Deserialize)]
pub struct CurrentQuery {
    pub sensitive: Option<u8>,
}

pub async fn current(
    State(app): State<App>,
    Query(q): Query<CurrentQuery>,
) -> ApiResult<impl IntoResponse> {
    let sensitive = q.sensitive.unwrap_or(0) != 0;
    let text = app
        .engine
        .config_text(sensitive)
        .await
        .map_err(|e| ApiError::internal(format!("cannot read the profile: {e}")))?;
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReloadJson {
    pub ok: bool,
    pub errors: usize,
    pub warnings: usize,
    pub listeners_rebound: bool,
}

pub async fn reload(State(app): State<App>) -> Json<ReloadJson> {
    let report = app.control.reload().await;
    Json(ReloadJson {
        ok: report.ok,
        errors: report.errors,
        warnings: report.warnings,
        listeners_rebound: report.listeners_rebound,
    })
}

#[derive(Serialize)]
pub struct CheckJson {
    pub ok: bool,
    pub errors: usize,
    pub warnings: usize,
    pub diagnostics: Vec<Value>,
}

/// Re-validates the profile on disk with the daemon's load options; never
/// touches the running config.
pub async fn check(State(app): State<App>) -> ApiResult<Json<CheckJson>> {
    let path = app.engine.runtime().config.source.main.clone();
    let opts = app.load_options.clone();
    let loaded = tokio::task::spawn_blocking(move || load(&path, &opts))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .map_err(|e| ApiError::internal(format!("cannot load the profile: {e}")))?;
    let diagnostics = loaded.diagnostics.sorted();
    let errors = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    let warnings = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .count();
    Ok(Json(CheckJson {
        ok: errors == 0,
        errors,
        warnings,
        diagnostics: diagnostics
            .iter()
            .map(|d| serde_json::to_value(d).unwrap_or(Value::Null))
            .collect(),
    }))
}
```

`routes/log.rs`：

```rust
//! `POST /v1/log/level`.

use crate::App;
use crate::error::{ApiError, ApiResult, json_body};
use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use rurge_engine::LogLevel;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
pub struct LevelBody {
    pub level: String,
}

pub async fn set_level(
    State(app): State<App>,
    body: Result<Json<LevelBody>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let body = json_body(body)?;
    let level = LogLevel::parse(&body.level).ok_or_else(|| {
        ApiError::bad_request(format!(
            "unknown log level `{}`: expected verbose, debug, info, notify, warning or error",
            body.level
        ))
    })?;
    app.control
        .set_log_level(level)
        .map_err(ApiError::internal)?;
    tracing::info!(level = level.as_str(), "log level changed via http-api");
    Ok(Json(json!({})))
}
```

`routes/mod.rs` 加 `pub mod dns; pub mod log; pub mod profiles;`；`lib.rs` 路由加：

```rust
        .route("/v1/dns", get(routes::dns::dns))
        .route("/v1/dns/flush", post(routes::dns::flush))
        .route("/v1/test/dns_delay", post(routes::dns::dns_delay))
        .route("/v1/profiles/current", get(routes::profiles::current))
        .route("/v1/profiles/reload", post(routes::profiles::reload))
        .route("/v1/profiles/check", post(routes::profiles::check))
        .route("/v1/log/level", post(routes::log::set_level))
```

`Diagnostics::sorted()` 返回 `Diagnostics`；`Severity` 派生 `PartialEq` + `Serialize`。移除 Task 4 留在 `tests/api.rs` `Api` 上的 `#[allow(dead_code)]`（所有字段现已使用；仍未使用的字段直接删除）。

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p rurge-api`（`--test api` 3×）
Expected: PASS。`dns_delay` 默认名依赖 harness 里 `internet-test-url = http://target.test:<port>/hello`。

- [ ] **Step 5: 质量门与提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-api
git commit -F - <<'EOF'
feat(api): dns / profiles / log-level 端点

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 7: `rurge run` 集成——模式优先级、`StateStore`、命令通道、API 启动、日志级别句柄

**Files:**
- Modify: `crates/rurge/Cargo.toml`（deps：`rurge-api`、`rurge-net`（已有）、`tokio-util`）
- Modify: `crates/rurge/src/cli/run.rs`
- Modify: `crates/rurge-engine/src/state.rs`（删除同步 `State::load` 及其测试——被 `StateStore::open` 取代）
- Modify: `crates/rurge/tests/cli.rs`（`mod run`）

**Interfaces:**
- Consumes: Task 1 `StateStore`；Task 2 `Mode`、`Engine::{attach_state, set_global_policy, policy_exists, mode, global_policy}`；Task 3 `Control`、`ReloadReport`、`LogLevel`；Task 4 `rurge_api::{serve, ApiContext}`；`rurge_net::BoxFuture`。
- Produces（bin 内部）：
  - `RunArgs.outbound_mode: Option<OutboundMode>`（无默认值）
  - `enum Command { Reload(oneshot::Sender<ReloadReport>), Stop }`；`struct LoopControl { tx: mpsc::Sender<Command>, log_level: reload::Handle<LevelFilter, Registry> }` 实现 `Control`
  - `init_logging(level, log_file) -> anyhow::Result<(Option<WorkerGuard>, reload::Handle<LevelFilter, Registry>)>`
  - `struct Daemon<'a> { engine, config, load_opts, rt, run_opts, outbound_mode, store, http_api }`；`reload(&Daemon<'_>, &mut Vec<(ListenerSpec, Running)>) -> ReloadReport`
  - `build_engine_runtime(cfg, rt, run_opts, outbound_mode, state: &State)`
  - 启动输出：监听行之后一行 `api on http://<addr>`；`http-api` 绑定失败 → stderr `error: cannot bind http-api on <addr>: <e>`，退出 1
  - 关闭顺序：`api_token.cancel()` → `stop_accepting` → `tracker.close()` → drain（API 服务在 tracker 上，随 drain 结束）

- [ ] **Step 1: CLI 端到端测试（先失败）**

`crates/rurge/tests/cli.rs` 的 `mod run` 里：把 `spawn_daemon_with` 改名为 `spawn_daemon_full(conf, data, watch, log_file, extra: &[&str])`（在 `--data-dir` 之后追加 `extra` 参数），保留 `spawn_daemon_with(conf, data, watch, log_file)` 委托 `spawn_daemon_full(.., &[])`。新增助手：

```rust
    /// Next stdout line starting with `prefix`, without the prefix.
    fn wait_for_line(daemon: &Daemon, prefix: &str) -> String {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let line = daemon
                .lines
                .recv_timeout(remaining)
                .unwrap_or_else(|_| panic!("rurge run never printed a `{prefix}` line"));
            if let Some(rest) = line.strip_prefix(prefix) {
                return rest.to_string();
            }
        }
    }

    fn api_port(daemon: &Daemon) -> u16 {
        wait_for_line(daemon, "api on http://")
            .rsplit(':')
            .next()
            .and_then(|p| p.parse().ok())
            .expect("api port")
    }

    /// Minimal HTTP/1.1 call against the API: (status, body).
    fn api_call(port: u16, method: &str, path: &str, key: &str, body: Option<&str>) -> (u16, String) {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let body = body.unwrap_or("");
        write!(
            s,
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Key: {key}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        let status = out
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let body = out.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
        (status, body)
    }

    fn wait_for_exit(daemon: &mut Daemon, secs: u64) -> Option<i32> {
        let deadline = std::time::Instant::now() + Duration::from_secs(secs);
        loop {
            if let Ok(Some(status)) = daemon.child.try_wait() {
                return status.code();
            }
            if std::time::Instant::now() > deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
```

测试（同一 `mod run`，都是同步 `#[test]`，不需要 tokio）：

```rust
    const API_GENERAL: &str = "http-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nhttp-api = k@127.0.0.1:0\nloglevel = warning";

    #[test]
    fn run_serves_the_api_and_persists_the_outbound_mode() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        let data = dir.path().join("data");
        // 1. fresh start: rule mode; change it through the API
        let d1 = spawn_daemon(&conf, &data);
        let port = api_port(&d1);
        let (status, body) = api_call(port, "GET", "/v1/outbound", "k", None);
        assert_eq!((status, body.as_str()), (200, r#"{"mode":"rule"}"#));
        assert_eq!(api_call(port, "GET", "/v1/outbound", "wrong", None).0, 401);
        assert_eq!(api_call(port, "POST", "/v1/outbound/global", "k", Some(r#"{"policy":"DIRECT"}"#)).0, 200);
        assert_eq!(api_call(port, "POST", "/v1/outbound", "k", Some(r#"{"mode":"direct"}"#)).0, 200);
        drop(d1);
        let state = std::fs::read_to_string(data.join("state.json")).unwrap();
        assert!(state.contains("\"outbound_mode\": \"direct\"") && state.contains("\"global_policy\": \"DIRECT\""), "{state}");
        // 2. restart without flags: state.json wins over the default
        let d2 = spawn_daemon(&conf, &data);
        let port = api_port(&d2);
        assert_eq!(api_call(port, "GET", "/v1/outbound", "k", None).1, r#"{"mode":"direct"}"#);
        assert_eq!(api_call(port, "GET", "/v1/outbound/global", "k", None).1, r#"{"policy":"DIRECT"}"#);
        let summary = wait_for_line(&d2, "rurge ");
        assert!(summary.contains("outbound mode direct"), "{summary}");
        drop(d2);
        // 3. an explicit flag wins over state.json and is written back
        let d3 = spawn_daemon_full(&conf, &data, false, None, &["--outbound-mode", "rule"]);
        let port = api_port(&d3);
        assert_eq!(api_call(port, "GET", "/v1/outbound", "k", None).1, r#"{"mode":"rule"}"#);
        drop(d3);
        let state = std::fs::read_to_string(data.join("state.json")).unwrap();
        assert!(state.contains("\"outbound_mode\": \"rule\""), "{state}");
    }

    #[test]
    fn run_reloads_and_stops_via_the_api() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        let mut daemon = spawn_daemon(&conf, &dir.path().join("data"));
        let port = api_port(&daemon);
        let (status, body) = api_call(port, "POST", "/v1/profiles/reload", "k", Some("{}"));
        assert_eq!(status, 200, "{body}");
        assert!(body.contains("\"ok\":true") && body.contains("\"listenersRebound\":false"), "{body}");
        assert_eq!(api_call(port, "POST", "/v1/log/level", "k", Some(r#"{"level":"verbose"}"#)).0, 200);
        assert_eq!(api_call(port, "POST", "/v1/stop", "k", Some("{}")), (200, "{}".to_string()));
        assert_eq!(wait_for_exit(&mut daemon, 10), Some(0), "stop exits 0");
    }

    #[test]
    fn run_exits_1_when_the_api_port_is_taken() {
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(
            dir.path(),
            &format!("http-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nhttp-api = k@127.0.0.1:{port}\nloglevel = warning"),
        );
        let mut daemon = spawn_daemon(&conf, &dir.path().join("data"));
        assert_eq!(wait_for_exit(&mut daemon, 10), Some(1));
    }
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p rurge --test cli run_serves`
Expected: FAIL（无 `api on` 行 → `wait_for_line` 超时 panic）。

- [ ] **Step 3: 依赖与 `state.rs` 清理**

`crates/rurge/Cargo.toml` `[dependencies]` 追加 `rurge-api.workspace = true`、`tokio-util.workspace = true`。`crates/rurge-engine/src/state.rs`：删除 `State::load`（同步版）与测试 `loads_defaults_selections_and_tolerates_garbage` 中依赖它的部分（`selections_for` / `profile_key` 的断言保留，改用 `StateStore::open` 读取）。

- [ ] **Step 4: `run.rs` 改造**

按顺序改：

1. imports：

```rust
use rurge_api::ApiContext;
use rurge_config::general::ControllerAccess;
use rurge_config::rule::PolicyRef;
use rurge_engine::control::{Control, LogLevel as ApiLogLevel, Mode, ReloadReport};
use rurge_engine::state::{STATE_FILE, State, StateStore, profile_key};
use rurge_net::BoxFuture;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::Registry;
use tracing_subscriber::reload;
```

2. `RunArgs.outbound_mode`：

```rust
    /// Outbound mode: direct, proxy=<policy>, rule (default: the last mode
    /// saved in state.json, else rule)
    #[arg(long, env = "RURGE_OUTBOUND_MODE", value_parser = parse_mode)]
    pub outbound_mode: Option<OutboundMode>,
```

3. `init_logging` 返回句柄：

```rust
type LevelHandle = reload::Handle<LevelFilter, Registry>;

fn init_logging(
    level: LevelFilter,
    log_file: Option<&std::path::Path>,
) -> anyhow::Result<(Option<tracing_appender::non_blocking::WorkerGuard>, LevelHandle)> {
    let (level_layer, handle): (reload::Layer<LevelFilter, Registry>, LevelHandle) =
        reload::Layer::new(level);
    let stdout_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_ansi(std::io::stdout().is_terminal());
    let registry = tracing_subscriber::registry()
        .with(level_layer)
        .with(stdout_layer);
    // ... 其余不变；两个分支分别返回 Ok((Some(guard), handle)) / Ok((None, handle))
}
```

4. 命令通道与 `LoopControl`：

```rust
/// What the API can ask the main loop to do (M4 design §6).
enum Command {
    Reload(oneshot::Sender<ReloadReport>),
    Stop,
}

struct LoopControl {
    tx: mpsc::Sender<Command>,
    log_level: LevelHandle,
}

fn failed_report() -> ReloadReport {
    ReloadReport {
        ok: false,
        errors: 1,
        warnings: 0,
        listeners_rebound: false,
    }
}

impl Control for LoopControl {
    fn reload(&self) -> BoxFuture<'_, ReloadReport> {
        Box::pin(async move {
            let (reply, rx) = oneshot::channel();
            if self.tx.send(Command::Reload(reply)).await.is_err() {
                return failed_report();
            }
            rx.await.unwrap_or_else(|_| failed_report())
        })
    }

    fn stop(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let _ = self.tx.send(Command::Stop).await;
        })
    }

    fn set_log_level(&self, level: ApiLogLevel) -> Result<(), String> {
        // the same mapping as `parse_log_level`
        let filter = match level {
            ApiLogLevel::Verbose => LevelFilter::TRACE,
            ApiLogLevel::Debug | ApiLogLevel::Info => LevelFilter::DEBUG,
            ApiLogLevel::Notify => LevelFilter::INFO,
            ApiLogLevel::Warning => LevelFilter::WARN,
            ApiLogLevel::Error => LevelFilter::ERROR,
        };
        self.log_level
            .modify(|f| *f = filter)
            .map_err(|e| e.to_string())?;
        tracing::info!(level = level.as_str(), "log level changed");
        Ok(())
    }

    fn set_system_proxy(&self, _enabled: bool) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async { Err("not implemented".to_string()) })
    }
}
```

5. `build_engine_runtime` 改签名，选择来自传入的快照：

```rust
async fn build_engine_runtime(
    cfg: rurge_config::Config,
    rt: &super::runtime::Runtime,
    run_opts: &RunOptions,
    outbound_mode: OutboundMode,
    state: &State,
) -> anyhow::Result<Runtime> {
    let selections = state.selections_for(&profile_key(&cfg.source.main));
    // ... Runtime::build 不变
}
```

6. 初始模式（D5）：

```rust
/// Explicit flag > state.json > rule. An explicit value is written back.
async fn initial_mode(explicit: Option<OutboundMode>, store: &StateStore, state: &State) -> OutboundMode {
    if let Some(mode) = explicit {
        let (m, global) = Mode::from_outbound(&mode);
        store
            .update(|s| {
                s.outbound_mode = Some(m.as_str().to_string());
                if global.is_some() {
                    s.global_policy = global;
                }
            })
            .await;
        return mode;
    }
    match state.outbound_mode.as_deref().and_then(Mode::parse) {
        Some(Mode::Direct) => OutboundMode::Direct,
        Some(Mode::Proxy) => match &state.global_policy {
            Some(p) => OutboundMode::Proxy(PolicyRef::parse(p)),
            None => {
                eprintln!("warning: state.json says proxy mode but names no global policy; using rule mode");
                OutboundMode::Rule
            }
        },
        Some(Mode::Rule) | None => OutboundMode::Rule,
    }
}
```

7. `Daemon` 与 `reload`（返回报告；`http-api` 变化只警告）：

```rust
/// Everything a reload needs besides the listeners it swaps.
struct Daemon<'a> {
    engine: &'a Arc<Engine>,
    config: &'a Path,
    load_opts: &'a LoadOptions,
    rt: &'a super::runtime::Runtime,
    run_opts: &'a RunOptions,
    outbound_mode: &'a OutboundMode,
    store: &'a StateStore,
    http_api: &'a Option<ControllerAccess>,
}

fn count(diags: &rurge_config::Diagnostics, severity: rurge_config::Severity) -> usize {
    diags.iter().filter(|d| d.severity == severity).count()
}

async fn reload(d: &Daemon<'_>, listeners: &mut Vec<(ListenerSpec, Running)>) -> ReloadReport {
    use rurge_config::Severity;
    let loaded = match load(d.config, d.load_opts) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: reload failed, keeping current config: {e}");
            return failed_report();
        }
    };
    let errors = count(&loaded.diagnostics, Severity::Error);
    let warnings = count(&loaded.diagnostics, Severity::Warning);
    if errors > 0 {
        eprintln!("reload failed, keeping current config:");
        print_diagnostics(&loaded.diagnostics.sorted());
        return ReloadReport { ok: false, errors, warnings, listeners_rebound: false };
    }
    print_diagnostics(&loaded.diagnostics.sorted());
    if &loaded.config.general.http_api != d.http_api {
        tracing::warn!("http-api changed in the profile; the API keeps its current address and key until rurge restarts");
    }
    let state = d.store.snapshot().await;
    let next = match build_engine_runtime(loaded.config, d.rt, d.run_opts, d.outbound_mode.clone(), &state).await {
        Ok(n) => n,
        Err(e) => {
            eprintln!("error: reload failed, keeping current config: {e}");
            return ReloadReport { ok: false, errors: 1, warnings, listeners_rebound: false };
        }
    };
    print_diagnostics(next.diagnostics());
    let surface_changed = d.engine.swap_runtime(next);
    let mut rebound = false;
    if surface_changed || listeners.is_empty() {
        match d.engine.rebind_listeners(std::mem::take(listeners)).await {
            Ok(next_listeners) => {
                *listeners = next_listeners;
                print_listening(listeners);
                rebound = true;
            }
            Err(e) => {
                tracing::error!(error = %e, "reload could not rebind listeners; the daemon has no listeners until the next successful reload");
                eprintln!("error: reload could not rebind listeners: {e}; the daemon has no listeners until the next successful reload");
                return ReloadReport { ok: false, errors: 1, warnings, listeners_rebound: false };
            }
        }
    }
    tracing::info!("profile reloaded");
    ReloadReport { ok: true, errors: 0, warnings, listeners_rebound: rebound }
}
```

（原来的注释「An empty list is the degraded state…」保留在 `if` 上方。`ControllerAccess` 需要 `PartialEq`——若没有，给它加 `#[derive(PartialEq, Eq)]`。）

8. `run()` 主体：

```rust
    let (_log_guard, level_handle) = init_logging(/* 同前 */)?;
    let rt = args.runtime.resolve(&cfg)?;
    let run_opts = /* 同前 */;
    let http_api = cfg.general.http_api.clone();
    let cfg_paths = /* 同前 */;
    let explicit_mode = args.outbound_mode.clone();
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(async move {
        let (store, state) = StateStore::open(rt.data_dir.join(STATE_FILE)).await;
        let outbound_mode = initial_mode(explicit_mode, &store, &state).await;
        let engine_rt = build_engine_runtime(cfg, &rt, &run_opts, outbound_mode.clone(), &state).await?;
        print_diagnostics(engine_rt.diagnostics());
        let (policies, rules) = (/* 同前 */);
        let engine = Engine::new(engine_rt);
        engine.attach_state(store.clone());
        // a global policy saved by an earlier run (mode may be rule today)
        if engine.global_policy().is_none()
            && let Some(saved) = state.global_policy.clone()
            && let Err(e) = engine.set_global_policy(&saved).await
        {
            tracing::warn!(error = %e, "state.json names a global policy that is not in the profile; ignoring it");
        }
        engine.start_sampler();
        let mut listeners = /* 同前：bind_listeners，失败退出 1 */;
        print_listening(&listeners);

        // Command channel from the API (M4 design §6). `cmd_tx` stays alive
        // here for the same reason as `reload_tx` below.
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<Command>(4);
        let control: Arc<dyn Control> = Arc::new(LoopControl {
            tx: cmd_tx.clone(),
            log_level: level_handle,
        });
        // The API comes up right after the listeners and before the summary
        // line, so `api on` is the third startup line (the CLI tests and
        // `rurge status` users read it there).
        let api_token = CancellationToken::new();
        if let Some(api) = http_api.clone() {
            let ctx = ApiContext {
                engine: engine.clone(),
                control: control.clone(),
                load_options: opts.clone(),
            };
            match rurge_api::serve(api.addr, api.key.clone(), ctx, api_token.clone()).await {
                Ok((addr, server)) => {
                    engine.tracker().spawn(server);
                    println!("api on http://{addr}");
                    tracing::info!(%addr, "http-api listening");
                }
                Err(e) => {
                    eprintln!("error: cannot bind http-api on {}: {e}", api.addr);
                    return Ok(ExitCode::from(1));
                }
            }
        }
        tracing::info!(policies, rules, mode = %mode_name(&outbound_mode), "rurge running");
        println!(/* 同前：`rurge <version> running: …` */);
        let daemon = Daemon { engine: &engine, config: &args.config, load_opts: &opts, rt: &rt, run_opts: &run_opts, outbound_mode: &outbound_mode, store: &store, http_api: &http_api };
        // ... reload_tx / watcher / signal streams 同前 ...
        loop {
            // shutdown_signal / reload_signal 同前
            tokio::select! {
                _ = shutdown_signal => break,
                _ = reload_signal => {
                    reload(&daemon, &mut listeners).await;
                }
                cmd = cmd_rx.recv() => match cmd {
                    Some(Command::Reload(reply)) => {
                        let report = reload(&daemon, &mut listeners).await;
                        let _ = reply.send(report);
                    }
                    Some(Command::Stop) => {
                        println!("stop requested via http-api");
                        break;
                    }
                    None => {}
                },
            }
        }
        // shutdown: the API first, so its graceful stop runs inside the drain
        api_token.cancel();
        engine.stop_accepting();
        engine.tracker().close();
        // ... 其余同前
```

`opts`（`LoadOptions`）要在 `block_on` 闭包里可用：它已在闭包外定义并被 `reload` 借用，闭包是 `async move`，因此 `opts` 会被移入——`ApiContext` 用 `opts.clone()`。`LoadOptions: Clone`。

- [ ] **Step 5: 运行确认通过**

Run: `cargo test -p rurge`（`--test cli` 3×；`run_*` 用例启动真实二进制，先 `cargo build -p rurge`）
Expected: 新增 3 个 PASS，既有 `run_*`（含 `--watch`、`--log-file`、SIGINT）不变。`cargo test -p rurge-engine`：`state::` 测试在删除 `State::load` 后仍 PASS。

- [ ] **Step 6: 质量门与提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge crates/rurge-engine Cargo.lock
git commit -F - <<'EOF'
feat(cli): rurge run 接入控制面：模式优先级与 state.json、命令通道、http-api 启动、日志级别热改

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 8: CLI 客户端：`rurge reload | stop | status`

**Files:**
- Create: `crates/rurge/src/cli/api_client.rs`、`crates/rurge/src/cli/control.rs`
- Modify: `crates/rurge/src/cli/mod.rs`、`src/main.rs`、`Cargo.toml`（deps：`hyper`、`hyper-util`、`http-body-util`、`bytes`、`http`）
- Modify: `crates/rurge/tests/cli.rs`（`mod run`）

**Interfaces:**
- Consumes: Task 7 的 `api on` 输出与所有端点；`cli::check::parse_platform`、`cli::environment`、`capabilities`。
- Produces:
  - `api_client::ApiClient::{new(addr: SocketAddr, key: String), get(&self, path) -> Result<(u16, Value), ClientError>, post(&self, path, body: Value) -> …}`；`enum ClientError { Unreachable(String), Timeout, BadBody(String) }`（`Display`）
  - `control::{ControlArgs, StatusArgs, reload(ControlArgs), stop(ControlArgs), status(StatusArgs)}`，均 `-> anyhow::Result<ExitCode>`
  - `control::resolve_endpoint(&ControlArgs) -> anyhow::Result<(SocketAddr, String)>`；常量 `NOT_CONFIGURED`（设计 §7 的整句提示）
  - `main.rs`：`Command::{Reload(cli::control::ControlArgs), Stop(cli::control::ControlArgs), Status(cli::control::StatusArgs)}`
  - 退出码：参数 / 未配置 / 鉴权（401 / 403）→ 2；连不上 / 超时 / 其它非 2xx → 1；成功 0

- [ ] **Step 1: 单元测试（先失败）**

`control.rs` 底部：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_binds_are_reached_on_loopback() {
        assert_eq!(connect_addr("0.0.0.0:6171".parse().unwrap()).to_string(), "127.0.0.1:6171");
        assert_eq!(connect_addr("[::]:6171".parse().unwrap()).to_string(), "[::1]:6171");
        assert_eq!(connect_addr("10.0.0.2:6171".parse().unwrap()).to_string(), "10.0.0.2:6171");
    }

    #[test]
    fn bytes_are_human_readable() {
        assert_eq!(fmt_bytes(0), "0 B");
        assert_eq!(fmt_bytes(1023), "1023 B");
        assert_eq!(fmt_bytes(1024), "1.0 KiB");
        assert_eq!(fmt_bytes(12_897_484), "12.3 MiB");
        assert_eq!(fmt_bytes(5 * 1024 * 1024 * 1024), "5.0 GiB");
    }

    #[test]
    fn status_text_matches_the_design() {
        let text = status_text(
            "http://127.0.0.1:6171",
            &serde_json::json!({
                "outbound": {"mode": "rule"},
                "global": {"policy": null},
                "policies": {"proxies": ["DIRECT", "HK"], "policy-groups": ["Pick"]},
                "rules": {"rules": [{}, {}, {}]},
                "requests": {"requests": [{}]},
                "traffic": {"total": {"in": 12_897_484, "out": 1_153_434, "inCurrentSpeed": 8397, "outCurrentSpeed": 614}}
            }),
        );
        assert_eq!(
            text,
            "rurge at http://127.0.0.1:6171\nmode: rule (global policy: none)\npolicies: 3   rules: 3   active requests: 1\ntraffic: in 12.3 MiB, out 1.1 MiB (in 8.2 KiB/s, out 614 B/s)\n"
        );
    }

    #[test]
    fn endpoint_resolution_prefers_flags_then_config() {
        let dir = tempfile::tempdir().unwrap();
        let with = dir.path().join("with.conf");
        std::fs::write(&with, "[General]\nhttp-api = abc@0.0.0.0:6171\n[Rule]\nFINAL,DIRECT\n").unwrap();
        let without = dir.path().join("without.conf");
        std::fs::write(&without, "[General]\n[Rule]\nFINAL,DIRECT\n").unwrap();
        let args = |config: Option<&std::path::Path>, remote: Option<&str>, key: Option<&str>| ControlArgs {
            config: config.map(|p| p.to_path_buf()),
            remote: remote.map(|r| r.parse().unwrap()),
            key: key.map(str::to_string),
            platform: None,
        };
        let (addr, key) = resolve_endpoint(&args(None, Some("10.0.0.2:1"), Some("k"))).unwrap();
        assert_eq!((addr.to_string().as_str(), key.as_str()), ("10.0.0.2:1", "k"));
        assert!(resolve_endpoint(&args(None, Some("10.0.0.2:1"), None)).unwrap_err().to_string().contains("--key"));
        let (addr, key) = resolve_endpoint(&args(Some(&with), None, None)).unwrap();
        assert_eq!((addr.to_string().as_str(), key.as_str()), ("127.0.0.1:6171", "abc"));
        let (_, key) = resolve_endpoint(&args(Some(&with), None, Some("override"))).unwrap();
        assert_eq!(key, "override");
        let e = resolve_endpoint(&args(Some(&without), None, None)).unwrap_err().to_string();
        assert_eq!(e, NOT_CONFIGURED);
        assert_eq!(resolve_endpoint(&args(None, None, None)).unwrap_err().to_string(), NOT_CONFIGURED);
    }
}
```

（`tempfile` 已是 bin 的 dev-dep。测试里 `ControlArgs` 直接构造，不经过 clap，所以环境变量 `RURGE_API_KEY` 不影响这些断言；字段见 Step 3。）

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p rurge control::`
Expected: 编译失败。

- [ ] **Step 3: 实现**

`crates/rurge/Cargo.toml` `[dependencies]` 追加 `hyper.workspace = true`、`hyper-util.workspace = true`、`http-body-util.workspace = true`、`bytes.workspace = true`、`http.workspace = true`。

`crates/rurge/src/cli/api_client.rs`：

```rust
//! A small HTTP client for the daemon's API (M4 design §7): plain HTTP, 5 s
//! per request, `X-Key` header, JSON in and out.

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use serde_json::Value;
use std::fmt;
use std::net::SocketAddr;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);

pub struct ApiClient {
    base: String,
    key: String,
    client: Client<HttpConnector, Full<Bytes>>,
}

#[derive(Debug)]
pub enum ClientError {
    Unreachable(String),
    Timeout,
    BadBody(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::Unreachable(e) => write!(f, "{e}"),
            ClientError::Timeout => write!(f, "timed out after {} s", TIMEOUT.as_secs()),
            ClientError::BadBody(e) => write!(f, "invalid response body: {e}"),
        }
    }
}

impl std::error::Error for ClientError {}

impl ApiClient {
    pub fn new(addr: SocketAddr, key: String) -> ApiClient {
        ApiClient {
            base: format!("http://{addr}"),
            key,
            client: Client::builder(TokioExecutor::new()).build_http(),
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub async fn get(&self, path: &str) -> Result<(u16, Value), ClientError> {
        self.call("GET", path, None).await
    }

    pub async fn post(&self, path: &str, body: Value) -> Result<(u16, Value), ClientError> {
        self.call("POST", path, Some(body)).await
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Result<(u16, Value), ClientError> {
        let mut req = Request::builder()
            .method(method)
            .uri(format!("{}{path}", self.base))
            .header("x-key", &self.key);
        let body = match body {
            Some(v) => {
                req = req.header("content-type", "application/json");
                Full::new(Bytes::from(v.to_string()))
            }
            None => Full::new(Bytes::new()),
        };
        let req = req.body(body).map_err(|e| ClientError::Unreachable(e.to_string()))?;
        let resp = tokio::time::timeout(TIMEOUT, self.client.request(req))
            .await
            .map_err(|_| ClientError::Timeout)?
            .map_err(|e| ClientError::Unreachable(e.to_string()))?;
        let status = resp.status().as_u16();
        let bytes = tokio::time::timeout(TIMEOUT, resp.into_body().collect())
            .await
            .map_err(|_| ClientError::Timeout)?
            .map_err(|e| ClientError::Unreachable(e.to_string()))?
            .to_bytes();
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .map_err(|e| ClientError::BadBody(format!("{e}: {}", String::from_utf8_lossy(&bytes))))?
        };
        Ok((status, value))
    }
}
```

`crates/rurge/src/cli/control.rs`：

```rust
//! `rurge reload | stop | status`: clients of the daemon's HTTP API (M4 §7).

use super::api_client::{ApiClient, ClientError};
use crate::capabilities;
use anyhow::{Context, anyhow};
use clap::Args;
use rurge_config::config::{LoadOptions, Platform, load};
use serde_json::{Value, json};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;

pub const NOT_CONFIGURED: &str = "http-api is not configured; add \"http-api = <key>@127.0.0.1:6171\" to [General] or pass --remote/--key (reload can also be triggered by SIGHUP or --watch)";

#[derive(Args, Clone, Debug)]
pub struct ControlArgs {
    /// Profile whose [General] http-api names the daemon
    #[arg(short = 'c', long = "config", value_name = "FILE")]
    pub config: Option<PathBuf>,
    /// API address, e.g. 127.0.0.1:6171 (overrides the profile)
    #[arg(long, value_name = "HOST:PORT")]
    pub remote: Option<SocketAddr>,
    /// API key (overrides the profile)
    #[arg(long, env = "RURGE_API_KEY", value_name = "KEY")]
    pub key: Option<String>,
    /// Evaluate the profile as if running on this platform
    #[arg(long, value_parser = super::check::parse_platform)]
    pub platform: Option<Platform>,
}

#[derive(Args)]
pub struct StatusArgs {
    #[command(flatten)]
    pub control: ControlArgs,
    /// Print the raw API responses as one JSON object
    #[arg(long)]
    pub json: bool,
}

/// A daemon bound to a wildcard address is reached on the loopback.
fn connect_addr(addr: SocketAddr) -> SocketAddr {
    match addr.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => SocketAddr::new(Ipv4Addr::LOCALHOST.into(), addr.port()),
        IpAddr::V6(ip) if ip.is_unspecified() => SocketAddr::new(Ipv6Addr::LOCALHOST.into(), addr.port()),
        _ => addr,
    }
}

/// `--remote` + `--key` → profile `http-api` (only parsed) → `NOT_CONFIGURED`.
pub fn resolve_endpoint(args: &ControlArgs) -> anyhow::Result<(SocketAddr, String)> {
    if let Some(remote) = args.remote {
        let key = args
            .key
            .clone()
            .ok_or_else(|| anyhow!("--remote needs --key (or RURGE_API_KEY)"))?;
        return Ok((connect_addr(remote), key));
    }
    let Some(config) = &args.config else {
        return Err(anyhow!("{NOT_CONFIGURED}"));
    };
    let platform = args.platform.unwrap_or_else(Platform::current);
    let opts = LoadOptions {
        environment: super::environment(platform, capabilities::CORE_VERSION),
        platform,
        capabilities: capabilities::current(),
    };
    let loaded = load(config, &opts).with_context(|| format!("cannot read {}", config.display()))?;
    match &loaded.config.general.http_api {
        Some(api) => Ok((
            connect_addr(api.addr),
            args.key.clone().unwrap_or_else(|| api.key.clone()),
        )),
        None => Err(anyhow!("{NOT_CONFIGURED}")),
    }
}

fn client(args: &ControlArgs) -> anyhow::Result<ApiClient> {
    let (addr, key) = resolve_endpoint(args)?;
    Ok(ApiClient::new(addr, key))
}

/// Maps transport failures and non-2xx answers to the exit codes of §7;
/// `Ok(body)` only for 2xx.
fn expect_2xx(client: &ApiClient, result: Result<(u16, Value), ClientError>) -> Result<Value, ExitCode> {
    match result {
        Ok((status, body)) if (200..300).contains(&status) => Ok(body),
        Ok((status, body)) => {
            let message = body["error"].as_str().unwrap_or("").to_string();
            eprintln!("error: rurge at {} answered {status}: {message}", client.base());
            Err(ExitCode::from(if status == 401 || status == 403 { 2 } else { 1 }))
        }
        Err(e) => {
            eprintln!("error: cannot reach rurge at {}: {e}", client.base());
            Err(ExitCode::from(1))
        }
    }
}

fn block_on<T>(fut: impl std::future::Future<Output = T>) -> anyhow::Result<T> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    Ok(runtime.block_on(fut))
}

pub fn reload(args: ControlArgs) -> anyhow::Result<ExitCode> {
    let client = client(&args)?;
    block_on(async {
        let body = match expect_2xx(&client, client.post("/v1/profiles/reload", json!({})).await) {
            Ok(b) => b,
            Err(code) => return code,
        };
        let errors = body["errors"].as_u64().unwrap_or(0);
        let warnings = body["warnings"].as_u64().unwrap_or(0);
        if body["ok"].as_bool().unwrap_or(false) {
            println!("reloaded: {errors} error(s), {warnings} warning(s)");
            ExitCode::SUCCESS
        } else {
            eprintln!("reload failed: {errors} error(s); the running configuration is unchanged");
            ExitCode::from(1)
        }
    })
}

pub fn stop(args: ControlArgs) -> anyhow::Result<ExitCode> {
    let client = client(&args)?;
    block_on(async {
        match expect_2xx(&client, client.post("/v1/stop", json!({})).await) {
            Ok(_) => {
                println!("stop requested");
                ExitCode::SUCCESS
            }
            Err(code) => code,
        }
    })
}

fn fmt_bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut value = n as f64 / 1024.0;
    let mut unit = UNITS[0];
    for u in &UNITS[1..] {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = u;
    }
    format!("{value:.1} {unit}")
}

/// The four-line status block of design §7.
fn status_text(base: &str, v: &Value) -> String {
    let mode = v["outbound"]["mode"].as_str().unwrap_or("?");
    let global = v["global"]["policy"].as_str().unwrap_or("none");
    let policies = v["policies"]["proxies"].as_array().map_or(0, Vec::len)
        + v["policies"]["policy-groups"].as_array().map_or(0, Vec::len);
    let rules = v["rules"]["rules"].as_array().map_or(0, Vec::len);
    let active = v["requests"]["requests"].as_array().map_or(0, Vec::len);
    let t = &v["traffic"]["total"];
    let n = |k: &str| t[k].as_u64().unwrap_or(0);
    format!(
        "rurge at {base}\nmode: {mode} (global policy: {global})\npolicies: {policies}   rules: {rules}   active requests: {active}\ntraffic: in {}, out {} (in {}/s, out {}/s)\n",
        fmt_bytes(n("in")),
        fmt_bytes(n("out")),
        fmt_bytes(n("inCurrentSpeed")),
        fmt_bytes(n("outCurrentSpeed")),
    )
}

pub fn status(args: StatusArgs) -> anyhow::Result<ExitCode> {
    let client = client(&args.control)?;
    block_on(async {
        let mut combined = serde_json::Map::new();
        for (name, path) in [
            ("outbound", "/v1/outbound"),
            ("global", "/v1/outbound/global"),
            ("policies", "/v1/policies"),
            ("rules", "/v1/rules"),
            ("requests", "/v1/requests/active"),
            ("traffic", "/v1/traffic"),
        ] {
            match expect_2xx(&client, client.get(path).await) {
                Ok(body) => {
                    combined.insert(name.to_string(), body);
                }
                Err(code) => return code,
            }
        }
        let combined = Value::Object(combined);
        if args.json {
            println!("{}", serde_json::to_string_pretty(&combined).expect("json"));
        } else {
            print!("{}", status_text(client.base(), &combined));
        }
        ExitCode::SUCCESS
    })
}
```

`cli/mod.rs` 加 `pub mod api_client; pub mod control;`。`main.rs`：

```rust
    /// Reload the running daemon's profile (needs http-api)
    Reload(cli::control::ControlArgs),
    /// Stop the running daemon (needs http-api)
    Stop(cli::control::ControlArgs),
    /// Show the running daemon's mode, counts and traffic (needs http-api)
    Status(cli::control::StatusArgs),
    // ...
        Command::Reload(args) => cli::control::reload(args),
        Command::Stop(args) => cli::control::stop(args),
        Command::Status(args) => cli::control::status(args),
```

`main` 的 `Err(e)` 分支已打印 `error: {e}` 并退出 2——`NOT_CONFIGURED` 与 `--remote needs --key` 都走这里。

- [ ] **Step 4: 端到端测试**

`crates/rurge/tests/cli.rs` `mod run` 追加：

```rust
    fn rurge() -> assert_cmd::Command {
        let mut cmd = assert_cmd::Command::cargo_bin("rurge").unwrap();
        cmd.env_remove("RURGE_API_KEY");
        cmd
    }

    #[test]
    fn control_commands_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        let mut daemon = spawn_daemon(&conf, &dir.path().join("data"));
        let port = api_port(&daemon);
        let remote = format!("127.0.0.1:{port}");
        // status via flags
        let out = rurge().args(["status", "--remote", &remote, "--key", "k"]).assert().success();
        let text = String::from_utf8(out.get_output().stdout.clone()).unwrap();
        assert!(text.starts_with(&format!("rurge at http://{remote}\nmode: rule (global policy: none)\n")), "{text}");
        assert!(text.contains("policies: 5   rules: 2   active requests: 0"), "{text}");
        assert!(text.contains("traffic: in 0 B, out 0 B (in 0 B/s, out 0 B/s)"), "{text}");
        // --json and the env var
        let out = rurge().args(["status", "--remote", &remote, "--json"]).env("RURGE_API_KEY", "k").assert().success();
        let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
        assert_eq!(v["outbound"]["mode"], "rule");
        assert_eq!(v["policies"]["policy-groups"], serde_json::json!([]));
        // wrong key → 2, missing --key → 2, not configured → 2
        rurge().args(["status", "--remote", &remote, "--key", "nope"]).assert().code(2).stderr(predicate::str::contains("unauthorized"));
        rurge().args(["reload", "--remote", &remote]).assert().code(2).stderr(predicate::str::contains("--key"));
        let plain = write_conf(&dir.path().join("plain"), "http-listen = 127.0.0.1:0");
        rurge().args(["reload", "-c"]).arg(&plain).assert().code(2).stderr(predicate::str::contains("http-api is not configured"));
        // resolution through a profile that names the live port
        let pointing = dir.path().join("pointing.conf");
        std::fs::write(&pointing, format!("[General]\nhttp-api = k@0.0.0.0:{port}\n[Rule]\nFINAL,DIRECT\n")).unwrap();
        rurge().args(["reload", "-c"]).arg(&pointing).assert().success().stdout(predicate::str::starts_with("reloaded: 0 error(s), "));
        // stop: the daemon exits 0
        rurge().args(["stop", "--remote", &remote, "--key", "k"]).assert().success().stdout("stop requested\n");
        assert_eq!(wait_for_exit(&mut daemon, 10), Some(0));
        // gone → 1
        rurge().args(["status", "--remote", &remote, "--key", "k"]).assert().code(1).stderr(predicate::str::contains("cannot reach rurge"));
    }
```

（`write_conf(&dir.path().join("plain"), ..)` 需要该目录存在：先 `std::fs::create_dir_all`。`policies: 5` = 5 个内置策略，测试配置没有 `[Proxy]` 条目；`rules: 2` = `DOMAIN,ads.test,REJECT` + `FINAL,DIRECT`。`predicate` 来自文件顶部的 `use predicates::prelude::*`，在 `mod run` 里需要 `use predicates::prelude::*;`。）

- [ ] **Step 5: 运行确认通过**

Run: `cargo test -p rurge`（`--test cli` 3×）
Expected: PASS。

- [ ] **Step 6: 质量门与提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge Cargo.lock
git commit -F - <<'EOF'
feat(cli): rurge reload / stop / status 客户端命令

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 9: 文档：`docs/api/phase1.md`、README、CLAUDE.md、兼容性清单、设计文档备注、计划收尾表

**Files:**
- Create: `docs/api/phase1.md`
- Modify: `README.md`（中英两半）、`CLAUDE.md`、`docs/surge-compatibility-matrix.md`、`docs/superpowers/specs/2026-09-07-phase1-m4-control-plane-design.md`（末尾追加「M4a 实施备注」）、本计划末尾两张表

**Interfaces:**
- Consumes: Task 4–8 最终的 JSON 形状、CLI 文案与退出码（以代码与测试为准，文档照抄）。

- [ ] **Step 1: `docs/api/phase1.md`**

```markdown
# rurge HTTP API（阶段 1）

rurge 在 `[General] http-api = <key>@<ip>:<port>` 指定的地址上提供 Surge 兼容的 HTTP API。本页记录阶段 1（M4a）实现的端点与 rurge 暂定的 JSON 结构；手册未定义的结构在阶段 6 与真实 Surge 对齐时可能调整（见 `docs/surge-compatibility-matrix.md` 第 9 节）。

## 鉴权与错误

- 每个请求带 `X-Key: <key>` 头或 `?x-key=<key>` 查询参数；比较为常量时间。
- 失败：`401 {"error":"unauthorized"}`。同一来源 IP 10 分钟内 5 次失败 → 之后 10 分钟内一律 `403 {"error":"banned"}`；封禁表最多跟踪 1024 个来源。
- 所有错误都是 `{"error":"<message>"}`：400（参数 / JSON 错误）、401、403、404（未知端点 / 未知请求 id / 未知功能名）、409（不可终止的内部会话）、500（内部错误）、501（本阶段未实现）。
- 无内容的成功响应是 `{}`。字段名 camelCase；手册已定义的字段（如 `policy-groups`、`dnsCache`）照抄。
- `http-api` 绑定到非环回地址时启动时打 WARN；改动 `http-api` 需重启 rurge（重载只警告）。

## 端点

| 方法 | 路径 | 请求 | 响应 |
| --- | --- | --- | --- |
| GET | `/v1/outbound` | | `{"mode":"direct"\|"proxy"\|"rule"}` |
| POST | `/v1/outbound` | `{"mode":…}` | `{}`；`proxy` 且未设全局策略 → 400 |
| GET | `/v1/outbound/global` | | `{"policy":"<name>"\|null}` |
| POST | `/v1/outbound/global` | `{"policy":"<name>"}`（空串清除） | `{}`；未知策略 → 400 |
| GET | `/v1/policies` | | `{"proxies":[…内置 + 配置策略],"policy-groups":[…]}` |
| GET | `/v1/rules` | | `{"rules":[{"index":0,"rule":"DOMAIN,…","hits":3}]}` |
| GET | `/v1/requests/recent?limit=N` | 默认 100 | `{"requests":[Request…]}`（最新在前） |
| GET | `/v1/requests/active` | | 同上 |
| POST | `/v1/requests/kill` | `{"id":N}` | `{}`；404 不在进行中；409 `not killable: internal session` |
| GET | `/v1/traffic` | | 见下 |
| GET | `/v1/dns` | | `{"dnsCache":[Cache…],"upstreams":[…],"bootstrap":[…]}` |
| POST | `/v1/dns/flush` | | `{}` |
| POST | `/v1/test/dns_delay` | `{"name":"<host>"}`，缺省为 `internet-test-url` 的主机 | `{"delays":[{"upstream":"udp://…","ms":12,"error":null}]}` |
| GET | `/v1/profiles/current?sensitive=0\|1` | | `text/plain`；默认脱敏（`password` / `psk` / `private-key` 参数、`ca-passphrase`、`ca-p12`、`key@` 前缀、`wifi-access-http-auth` 的口令 → `***`） |
| POST | `/v1/profiles/reload` | | `{"ok":true,"errors":0,"warnings":1,"listenersRebound":false}` |
| POST | `/v1/profiles/check` | | `{"ok":…,"errors":N,"warnings":N,"diagnostics":[Diagnostic…]}`（校验磁盘上的当前配置，不影响运行；`Diagnostic` 与 `rurge check --json` 相同） |
| POST | `/v1/log/level` | `{"level":"verbose"\|"debug"\|"info"\|"notify"\|"warning"\|"error"}` | `{}` |
| GET | `/v1/features/{system_proxy\|enhanced_mode\|mitm\|capture\|rewrite\|scripting}` | | `{"enabled":false}` |
| POST | `/v1/features/{name}` | `{"enabled":bool}` | 501（`system_proxy` 在 M4b 生效） |
| GET | `/v1/modules` | | `{"enabled":[],"available":[]}` |
| GET | `/v1/scripting` | | `{"scripts":[]}` |
| GET | `/v1/events` | | `{"events":[]}` |
| POST | `/v1/stop` | | `{}`，随后进程退出（退出码 0） |

### Request

```json
{"id":12,"listener":"http","src":"127.0.0.1:51234","dst":"example.com:443","rule":"DOMAIN-SUFFIX,example.com,Proxy","policy":["Proxy","HK"],"sni":"example.com","protocol":"https","up":1234,"down":56789,"startedMs":1757200000000,"elapsedMs":812,"status":"completed","rejectKind":null,"error":null}
```

`listener` ∈ `http` `socks5` `tun` `forward` `internal`；`status` ∈ `active` `completed` `rejected` `failed`；`rejectKind` 在 `rejected` 时是 `REJECT` / `REJECT-DROP` / `REJECT-NO-DROP` / `REJECT-TINYGIF`；`protocol` 是嗅探到的协议小写名或 `null`。

### Traffic

```json
{"startTime":1757200000.5,"total":{"in":123,"out":45,"inCurrentSpeed":0,"outCurrentSpeed":0},"connector":{"DIRECT":{"in":123,"out":45}},"listener":{"http":{"in":123,"out":45},"socks5":{"in":0,"out":0}}}
```

`in` 是下行（远端 → 客户端）字节，`out` 是上行；速度是最近一秒的字节数。

### Cache

```json
{"domain":"example.com","data":["93.184.216.34"],"expiresTime":1757200060.2,"server":"udp://1.1.1.1:53","stale":false,"negative":false}
```

## CLI 客户端

`rurge reload | stop | status [-c <conf>] [--remote host:port] [--key <key>] [--platform <p>]`，`status` 另有 `--json`。地址 / 密钥：`--remote` + `--key`（或 `RURGE_API_KEY`）→ `-c` 配置的 `http-api`（只解析）→ 未配置时退出 2 并提示。退出码：2 参数 / 鉴权，1 连不上或非 2xx，0 成功。
```

- [ ] **Step 2: README（中英两半同步改）**

- 状态引言（中文第 18 行附近、英文第 143 行附近）：「M1 ～ M3b 完成」→「M1 ～ M4a 完成」，追加 M4a 一句：Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload / stop / status`；「M4（控制面 API、系统代理、Dashboard）未开始」改为「M4b（系统代理、服务安装）未开始；Dashboard 在阶段 6」。
- 第 56 / 180 行的提示：「系统代理与 API 在 M4」→「HTTP API 与 `rurge reload / stop / status` 已可用（见 `docs/api/phase1.md`），系统代理在 M4b」。
- 特性表 / 路线图中涉及「HTTP API」「控制命令」的行改为已完成（M4a），并保持中英内容一致。

- [ ] **Step 3: CLAUDE.md**

- 「当前状态」段：M4a 完成（`rurge-api`：axum 服务、鉴权封禁、阶段 1 端点；`StateStore`；`Control` 命令通道；`rurge reload / stop / status`）；「M4（控制面与平台）未开始」→「M4b（系统代理、服务安装）未开始」。
- 「先读这些文档」加两条：M4 设计文档 `docs/superpowers/specs/2026-09-07-phase1-m4-control-plane-design.md`、M4a 计划 `docs/superpowers/plans/2026-09-07-phase1-m4a-control-plane-plan.md`（末尾两表同前），以及 `docs/api/phase1.md`。
- 「计划中的架构」依赖方向补：`rurge (bin) → rurge-api → rurge-engine`；`rurge-api` 不依赖 `rurge-platform`。
- 「常用命令」加：`cargo run -p rurge -- status -c config.conf`（`reload` / `stop` 同）、`cargo test -p rurge-api`。

- [ ] **Step 4: 兼容性清单（列数不变）**

按设计 §10 逐行改 `docs/surge-compatibility-matrix.md`（行号以当时文件为准，用 `grep -n` 定位）：
- `http-api` 行（约 152）备注：M4a 已实现；鉴权失败 10 分钟 5 次封 10 分钟；绑定非环回地址 WARN；改动需重启。
- CLI 控制命令行（约 809，`reload` `switch-profile` `kill` `stop`…）：`rurge reload` / `stop` 已实现（M4a，依赖 `http-api`，未配置时提示 SIGHUP / `--watch`）；`kill` 走 `POST /v1/requests/kill`；`switch-profile` 阶段 6。另按第 871 行「rurge 扩展」行的列布局新增一行：`rurge status`（Surge 无对应命令）🟡 阶段 1。
- `/v1/outbound/global`（约 827）：状态 🟡，备注 `proxy` 模式下全局策略缺失 / 不存在 → 按规则并 WARN 一次；出站模式与全局策略持久化到 `state.json`，显式 `--outbound-mode` 覆盖并写回。
- `/v1/policies`（828）、`/v1/rules`（845）、`/v1/traffic`（846）、`/v1/dns` 行（839）：状态 🟡，备注「JSON 结构手册未定义，暂定结构见 `docs/api/phase1.md`，阶段 6 对齐」；`dns_delay` 返回按上游列表。
- `/v1/requests/*`（835）：备注追加「M4a 暂定结构见 `docs/api/phase1.md`；kill 内部会话 → 409」。
- `/v1/profiles/current`（836）、`/v1/profiles/reload`（837）、`/v1/profiles/check`（838 的 check 部分）、`/v1/log/level`（847）、`/v1/stop`（843）、`/v1/features/*`（823–825，阶段 1 部分）：备注「M4a 已实现」+ 各自差异（`current` 脱敏规则；`check` 校验磁盘上的当前配置；`log/level` 另接受 `notify`；`stop` 退出进程）。
- 第 3 行左右的「图例」不改；每行保持 6 列。

- [ ] **Step 5: 设计文档备注与计划收尾**

- 设计文档末尾追加「## 14. M4a 实施备注」：列出与 §4–§7 的偏差（`ApiContext.load_options`；`Mode` / `LogLevel` 枚举；`serve` 返回 `(addr, future)` 由 bin 放到 tracker；`ReloadReport` 在重绑失败时 `ok:false`；`/v1/modules` `/v1/scripting` `/v1/events` 的具体形状；`status` 的 `policies` 计数含内置策略；其它执行期裁定）。
- 本计划末尾「执行期修正记录」表与「延后事项」列表按 SDD 账本填写。

- [ ] **Step 6: 检查与提交**

Run: `cargo test --workspace`（文档改动不影响；确认无误后提交）。README 中英两半逐段对照一遍。

```bash
git add docs README.md CLAUDE.md
git commit -F - <<'EOF'
docs: M4a 控制面：API 参考、README / CLAUDE / 兼容性清单同步、设计备注与计划收尾

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

## 执行期修正记录

（执行时填写：任务号、与计划的偏差、原因。）

| 任务 | 偏差 | 原因 |
| --- | --- | --- |
| | | |

## 延后事项

（执行时填写：审查中发现但不在 M4a 范围内的问题，带去向——M4b / 阶段 2 / 阶段 6。）

- （空）

