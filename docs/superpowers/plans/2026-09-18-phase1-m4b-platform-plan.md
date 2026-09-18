# M4b「平台」实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 `rurge run` 能把自己设为操作系统的系统代理（三平台、可运行期开关、退出与崩溃后恢复原设置），并提供基础的开机自启安装 `rurge service install | uninstall`。

**Architecture:** `rurge-platform` 新增 `sysproxy`（`SystemProxy` trait + Windows 注册表 / macOS `networksetup` / Linux GNOME·KDE 三个后端——后端的逻辑在所有主机上编译与测试，只有真正触碰操作系统的薄层按 `cfg` 隔离或经 `CommandRunner` / `Registry` 抽象注入）与 `service`（按 `Os` 生成 systemd unit / launchd plist / 计划任务的「计划」，再由执行器落地）。bin 里的 `SystemProxyManager` 负责生命周期：快照 → 备份写进 `state.json` → 应用；关闭 / 退出时恢复；启动时发现残留备份先恢复；重载后跟随监听地址。它只由 `rurge run` 的主循环串行调用（M4a 的命令通道多一条 `Command::SystemProxy`），API 的 `/v1/features/system_proxy` 经 `Control` 接到同一条路径。

**Tech Stack:** Rust stable（edition 2024，MSRV 1.88）、tokio、serde / serde_json、`windows-registry` 0.6 + `windows-sys` 0.61（仅 Windows）、clap、axum（既有）。

**Spec:** `docs/superpowers/specs/2026-09-07-phase1-m4-control-plane-design.md`（M4b：§1.2 的 FR-IN-04 / FR-OBS-09、§8、§9 的 M4b 行、§10 的平台行、§11 Q2；§14 是 M4a 的实施备注）。

## 计划期决定（与设计文档文字有出入或设计未定的地方）

| 编号 | 决定 | 理由 |
| --- | --- | --- |
| P1 | `rurge-platform` 不再继承 workspace 的 `unsafe_code = "forbid"`，改用 crate 自己的 `[lints]`：`unsafe_code = "deny"`，只有 `sysproxy/windows.rs` 里调用 `InternetSetOptionW` 的那一个函数 `#[allow(unsafe_code)]` 并写 SAFETY 注释；其余 crate 仍是 forbid | FR-IN-04 要求「通知 WinINet」，只能经 FFI；用户于 2026-09-18 选定此方案 |
| P2 | 注册表读写用 `windows-registry` 0.6（安全 API，已经由 `ipconfig` 带进依赖图），不用设计文档写的 `winreg` | 不新增下载；微软维护；我们的代码里注册表部分不需要 `unsafe` |
| P3 | 监听地址是通配地址时换成**同族**回环：`0.0.0.0` → `127.0.0.1`，`::` → `::1`（设计写的是一律 `127.0.0.1`） | Windows 上 `[::]` 默认 v6-only，`127.0.0.1` 连不上；复用 `cli::control::connect_addr` |
| P4 | Linux 的 KDE 分支要求 `XDG_CURRENT_DESKTOP` 含 `KDE` **且** 有 `kwriteconfig6`（退而求其次 `kwriteconfig5`）+ 对应的 `kreadconfig` | 只看工具存在会在别的桌面上白写 `kioslaverc` 却报告成功；Plasma 5 仍常见，工具名只差一个数字 |
| P5 | 端到端测试用文件后端：环境变量 `RURGE_SYSTEM_PROXY_BACKEND=file:<path>` 让 `rurge run` 把「系统代理」读写到一个 JSON 文件；未知取值直接报错退出。CLI 测试 harness 给**每一个**它启动的守护进程都设这个变量 | 生命周期（尤其崩溃恢复）必须经真实二进制验证，而测试绝不能改开发机 / CI 的真实代理设置 |
| P6 | Windows 上除 Ctrl-C 外，控制台关闭、注销、关机事件也触发优雅退出（tokio 的 `ctrl_close` / `ctrl_logoff` / `ctrl_shutdown`） | 关掉终端窗口是最常见的「退出」；不处理的话系统代理会指向一个已死的端口（FR-IN-04「退出后恢复」） |
| P7 | `rurge service install / uninstall` 增加 `--dry-run`：只打印将写的文件与将执行的命令 | 让 CLI 可端到端测试；用户可在 `sudo` 前先看清要做什么 |
| P8 | Windows 的 `schtasks /create` 追加 `/f` | 任务已存在时 `schtasks` 会交互式询问是否覆盖，非交互环境会挂住 |
| P9 | macOS 经 `networksetup` 无法设置「Exclude simple hostnames」：`exclude-simple-hostnames = true` 时 WARN 一条并忽略，登记进兼容性清单 | `networksetup` 没有对应选项；SystemConfiguration 直写在阶段 6 评估（骨架设计 §9） |
| P10 | `POST /v1/features/system_proxy` 的失败一律 500（带原因）；M4a 里靠字符串 `"not implemented"` 判 501 的分支删除 | M4b 起这个功能已实现；不支持的桌面属于「应用失败」，设计 §8.2 规定为 API 500 |
| P11 | 顺带处理 M4a 计划「延后事项」里去向为 M4b 的两条：`POST /v1/outbound/global {"policy":""}` 在 proxy 模式下回 400（与另一侧守卫对称）；`Engine` 增加 `resolver()` / `internet_test_url()` / `profile_path()` 视图，`rurge-api` 不再直接读 `Runtime` 的字段 | 最终评审的分诊结论 |

## Global Constraints

- Rust stable，edition 2024，workspace `rust-version = 1.88`（let-chains 允许，clippy 对嵌套 `if let` 要求用 let-chains）。`unsafe_code`：除 `rurge-platform`（P1）外全工作区 `forbid`。
- 质量门（每个任务提交前）：`RUSTFMT="C:\Users\SZV01065\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe" cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace`。时序敏感的二进制（`rurge-api --test api`、`rurge --test cli`）改动后各跑 3 次。全工作区偶发单个测试二进制异常退出（已知、未定位）——遇到时用 `--no-fail-fast` 重跑一次再判断。
- **测试绝不修改真实的操作系统代理设置或服务注册**：平台后端只经 `FakeRegistry` / `FakeRunner` 测试；例外只有两个 Windows 测试——在 `HKCU\Software\rurge-test-<pid>` 这个临时键下的读写往返，以及对真实 Internet Settings 的**只读**快照。CLI 测试启动的每个守护进程都带 `RURGE_SYSTEM_PROXY_BACKEND=file:…`（P5）。测试不访问公网，全部 `127.0.0.1:0`，等待用有界轮询。
- 平台特定代码只出现在 `rurge-platform`（AR-02）。`rurge-engine` / `rurge-api` 不依赖 `rurge-platform`；`rurge-platform` 不依赖任何内部 crate。
- CLI 输出、日志、API 错误文案用英文；文档中文，README 中英两半内容一致。rurge 专有运行时选项只经 CLI 参数 / 环境变量提供（FR-CFG-17）：`--system-proxy`（`RURGE_SYSTEM_PROXY`）、`RURGE_SYSTEM_PROXY_BACKEND`。
- 与 Surge 的行为差异一律登记进 `docs/surge-compatibility-matrix.md`（表列数不变）。
- 系统代理语义（设计 §8）：http / https 用第一个 `http-listen`；`set-system-socks-proxy = true`（默认）时 socks 用第一个 `socks5-listen`；绕过列表 = `skip-proxy`（macOS 语义）；`exclude-simple-hostnames` → Windows 的 `<local>`。开启 = `snapshot` → 备份与 `features.system_proxy = true` 写进 `state.json` → `apply`；`apply` 失败 → 尽力 `restore`、清状态、返回错误（API 500 / CLI 退出 1）。关闭与优雅退出 = `restore` → 清状态。启动时 `system_proxy_backup` 非空 → 先 `restore` 并 WARN，再按本次是否带 `--system-proxy` 决定是否开启（`features.system_proxy` 不决定重启后的状态，PRD 3.5）。
- 提交：中文主题行，`git commit -F -` + heredoc，末尾两行 trailer：
  `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`
  `Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th`

## 现有接口（M4a 结束时，供各任务参考）

- `rurge_platform`：`dirs::{Os::{Windows, MacOs, Unix}, Os::current(), EnvLookup<'a> = &'a dyn Fn(&str) -> Option<OsString>, data_dir(), config_dir()}`（`home(env)` 目前是私有函数）、`dns::*`。`Cargo.toml` 目前 `[lints] workspace = true`，Windows 专属依赖 `ipconfig`。
- `rurge_config::general::General`（`impl Default`）：`http_listen: Vec<Listener { password, addr }>`、`socks5_listen`、`skip_proxy: HostList`、`exclude_simple_hostnames: bool`（默认 false）、`set_system_socks_proxy: bool`（默认 true）、`http_api: Option<ControllerAccess>`。`HostList { entries: Vec<HostListEntry { negate, pattern: HostPattern, port, raw }> }`，`HostList::parse(text, default_port)`；`HostPattern::{Glob(Glob), Cidr(IpNet), AnyIp, AnyV4, AnyV6, SimpleHostname}`；`Glob::source() -> &str`；`IpNet::{trunc(), prefix_len(), max_prefix_len(), addr()}`。
- `rurge_engine::state`：`State { features: Features { system_proxy, .. }, system_proxy_backup: Option<serde_json::Value>, .. }`、`StateStore::{open(PathBuf) -> (Arc<StateStore>, State), snapshot().await -> State, update(FnOnce(&mut State)).await -> State}`。
- `rurge_engine::control::Control`：`reload()`、`stop()`、`set_log_level(LogLevel)`、`set_system_proxy(bool) -> BoxFuture<'_, Result<(), String>>`（M4a 的实现都返回 `Err("not implemented")`）。
- `rurge_engine::Engine`：`runtime() -> Arc<Runtime>`（`Runtime { config: Arc<Config>, stack: Stack { resolver: Arc<Resolver>, .. }, .. }`）、`mode() -> Mode`、`global_policy()`、`set_global_policy(&str)`、`config_text(sensitive)`。
- `rurge-api`：`routes/features.rs`（`GET` 恒 `{"enabled":false}`；`POST system_proxy` 走 `Control::set_system_proxy`，`Err` 含 `"not implemented"` → 501，其它 → 500）、`routes/outbound.rs::set_global`、`routes/dns.rs` 与 `routes/profiles.rs` 直接读 `engine.runtime()` 的字段；`tests/api.rs` 的 `FakeControl { reloads, stops, levels }`、`api()`、`get` / `post` / `call`。
- bin `cli/run.rs`：`RunArgs`、`Command { Reload(oneshot), Stop }`、`LoopControl { tx, log_level }`、`Daemon<'a>`、`reload(&Daemon, &mut listeners) -> ReloadReport`、`print_listening`；`run()` 的顺序：绑定监听 → 命令通道与 API（`api on` 行）→ 汇总行（`rurge <ver> running: …`）→ `Daemon` → 重载通道 / watcher → 信号流（Unix：`interrupt` / `sighup` / `sigterm`；Windows：`ctrl_c`）→ `loop select!` → 关闭（`api_token.cancel()` → `stop_accepting` → `tracker().close()` → drain / `GRACE` / force-exit）。`cli/control.rs` 有私有的 `connect_addr(SocketAddr) -> SocketAddr`（通配地址 → 同族回环）。
- CLI 测试 harness（`crates/rurge/tests/cli.rs` 的 `mod run`）：`Daemon { child, http, socks, lines }`、`write_conf(dir, general)`、`spawn_daemon(conf, data)`、`spawn_daemon_full(conf, data, watch, log_file, extra: &[&str])`、`wait_for_line(&Daemon, prefix)`、`api_port`、`api_call(port, method, path, key, body) -> (u16, String)`、`wait_for_exit(&mut Daemon, secs)`、`API_GENERAL`、`rurge() -> assert_cmd::Command`。

## 文件结构

| 文件 | 职责 | 任务 |
| --- | --- | --- |
| `crates/rurge-platform/Cargo.toml` | crate 自己的 lints（P1）、serde / serde_json、Windows 专属 `windows-registry` / `windows-sys` | 1 |
| `crates/rurge-platform/src/sysproxy/mod.rs` | `ProxySettings`、`Backup`、`SystemProxy`、`platform()` | 1、3 |
| `crates/rurge-platform/src/sysproxy/windows.rs` | `Registry` 抽象、`WindowsProxy`、`ProxyServer` / `ProxyOverride` 取值、CIDR → 通配、`RealRegistry` + WinINet 通知（仅 Windows） | 1 |
| `crates/rurge-platform/src/command.rs` | `Cmd`、`CommandRunner`、`SystemRunner`、测试用 `FakeRunner` | 2 |
| `crates/rurge-platform/src/sysproxy/macos.rs` | `networksetup` 输出解析、命令生成、`MacosProxy` | 2 |
| `crates/rurge-platform/src/sysproxy/linux.rs` | 桌面探测、GNOME / KDE 命令生成、环境变量提示、`LinuxProxy` | 3 |
| `crates/rurge-platform/src/service.rs` | systemd / launchd / schtasks 的安装与卸载计划、执行器 | 4 |
| `crates/rurge/src/cli/sysproxy.rs` | `proxy_settings`、`SystemProxyManager`、文件后端（P5） | 5 |
| `crates/rurge-engine/src/control.rs`、`crates/rurge-api/src/routes/features.rs` | `Control::system_proxy_enabled`；features 端点接通 | 6 |
| `crates/rurge-engine/src/engine.rs`、`crates/rurge-api/src/routes/{outbound,dns,profiles}.rs` | M4a 延后的两条（P11） | 7 |
| `crates/rurge/src/cli/run.rs`、`crates/rurge/tests/cli.rs` | `--system-proxy`、`Command::SystemProxy`、启动恢复、重载跟随、退出恢复、Windows 控制台事件；端到端测试 | 8 |
| `crates/rurge/src/cli/service.rs`、`main.rs` | `rurge service install / uninstall` | 9 |
| README、CLAUDE.md、清单、`docs/api/phase1.md`、设计文档 §15、`docs/acceptance/phase1-manual.md`、本计划末尾 | 文档与手工验收清单 | 10 |

---

### Task 1: `rurge_platform::sysproxy` 核心类型与 Windows 后端

**Files:**
- Modify: `Cargo.toml`（workspace deps：`windows-registry`、`windows-sys`）
- Modify: `crates/rurge-platform/Cargo.toml`、`crates/rurge-platform/src/lib.rs`
- Create: `crates/rurge-platform/src/sysproxy/mod.rs`、`crates/rurge-platform/src/sysproxy/windows.rs`

**Interfaces:**
- Produces:
  - `rurge_platform::sysproxy::{ProxySettings { http, https, socks: Option<SocketAddr>, bypass: Vec<String>, exclude_simple: bool }, Backup(pub serde_json::Value), SystemProxy { snapshot() -> io::Result<Backup>, apply(&ProxySettings) -> io::Result<()>, restore(&Backup) -> io::Result<()> }}`；`ProxySettings: Clone + Debug + Default + PartialEq + Eq`；`Backup: Clone + Debug + PartialEq + Serialize + Deserialize`（`#[serde(transparent)]`）。
  - crate 内：`sysproxy::wrong_platform(expected: &str) -> io::Error`（`ErrorKind::InvalidData`）。
  - `sysproxy::windows::{KEY_PATH, Registry, WindowsProxy<R: Registry>, WindowsProxy::new(R), proxy_server_value(&ProxySettings) -> String, proxy_override_value(&ProxySettings) -> String}`；仅 Windows：`RealRegistry::{internet_settings(), at(path)}`。
  - 备份 JSON：`{"platform":"windows","ProxyEnable":<u32|null>,"ProxyServer":<str|null>,"ProxyOverride":<str|null>}`。

- [ ] **Step 1: 清单与 lints**

workspace `Cargo.toml` 的 `[workspace.dependencies]` 追加：

```toml
windows-registry = "0.6"
windows-sys = { version = "0.61", features = ["Win32_Networking_WinInet"] }
```

`crates/rurge-platform/Cargo.toml`：`description` 改为 `"Platform-specific helpers for rurge (directories, system DNS, system proxy, service install)"`；`[dependencies]` 追加 `serde.workspace = true`、`serde_json.workspace = true`；Windows 专属依赖块追加两行；把末尾的 `[lints]\nworkspace = true` 整段替换：

```toml
[target.'cfg(windows)'.dependencies]
ipconfig.workspace = true
windows-registry.workspace = true
windows-sys.workspace = true

# The workspace lints, except `unsafe_code`: notifying WinINet that the proxy
# settings changed (`sysproxy::windows`) is an FFI call. The lint stays `deny`
# here and only that one function opts out; every other crate keeps `forbid`.
[lints.rust]
unsafe_code = "deny"

[lints.clippy]
all = { level = "warn", priority = -1 }
```

`crates/rurge-platform/src/lib.rs` 加 `pub mod sysproxy;`（保持字母序：`dirs`、`dns`、`sysproxy`）。

- [ ] **Step 2: 写失败测试**

新建 `crates/rurge-platform/src/sysproxy/windows.rs`，先只放测试模块（实现见 Step 4）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Clone, Debug, PartialEq)]
    enum Val {
        U32(u32),
        Str(String),
    }

    /// An in-memory registry key that also records the order of writes.
    #[derive(Default)]
    struct FakeRegistry {
        values: Mutex<BTreeMap<String, Val>>,
        writes: Mutex<Vec<String>>,
        notified: AtomicUsize,
    }

    impl FakeRegistry {
        fn with(values: &[(&str, Val)]) -> FakeRegistry {
            let reg = FakeRegistry::default();
            for (name, value) in values {
                reg.values
                    .lock()
                    .unwrap()
                    .insert(name.to_string(), value.clone());
            }
            reg
        }
        fn dump(&self) -> BTreeMap<String, Val> {
            self.values.lock().unwrap().clone()
        }
    }

    impl Registry for FakeRegistry {
        fn get_u32(&self, name: &str) -> io::Result<Option<u32>> {
            Ok(match self.values.lock().unwrap().get(name) {
                Some(Val::U32(v)) => Some(*v),
                _ => None,
            })
        }
        fn get_string(&self, name: &str) -> io::Result<Option<String>> {
            Ok(match self.values.lock().unwrap().get(name) {
                Some(Val::Str(v)) => Some(v.clone()),
                _ => None,
            })
        }
        fn set_u32(&self, name: &str, value: u32) -> io::Result<()> {
            self.writes.lock().unwrap().push(name.to_string());
            self.values
                .lock()
                .unwrap()
                .insert(name.to_string(), Val::U32(value));
            Ok(())
        }
        fn set_string(&self, name: &str, value: &str) -> io::Result<()> {
            self.writes.lock().unwrap().push(name.to_string());
            self.values
                .lock()
                .unwrap()
                .insert(name.to_string(), Val::Str(value.to_string()));
            Ok(())
        }
        fn delete(&self, name: &str) -> io::Result<()> {
            self.writes.lock().unwrap().push(format!("-{name}"));
            self.values.lock().unwrap().remove(name);
            Ok(())
        }
        fn notify(&self) {
            self.notified.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn settings() -> ProxySettings {
        ProxySettings {
            http: Some("127.0.0.1:6152".parse().unwrap()),
            https: Some("127.0.0.1:6152".parse().unwrap()),
            socks: Some("127.0.0.1:6153".parse().unwrap()),
            bypass: vec![
                "localhost".into(),
                "*.local".into(),
                "10.0.0.0/8".into(),
                "192.168.1.0/24".into(),
            ],
            exclude_simple: true,
        }
    }

    #[test]
    fn apply_writes_the_three_values_enables_last_and_notifies() {
        let proxy = WindowsProxy::new(FakeRegistry::default());
        proxy.apply(&settings()).unwrap();
        let values = proxy.reg.dump();
        assert_eq!(
            values["ProxyServer"],
            Val::Str("http=127.0.0.1:6152;https=127.0.0.1:6152;socks=127.0.0.1:6153".into())
        );
        assert_eq!(
            values["ProxyOverride"],
            Val::Str("localhost;*.local;10.*;192.168.1.*;<local>".into())
        );
        assert_eq!(values["ProxyEnable"], Val::U32(1));
        assert_eq!(
            *proxy.reg.writes.lock().unwrap(),
            ["ProxyServer", "ProxyOverride", "ProxyEnable"],
            "the proxy is switched on only after its address is in place"
        );
        assert_eq!(proxy.reg.notified.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn socks_is_optional_and_an_empty_bypass_list_removes_the_override() {
        let reg = FakeRegistry::with(&[("ProxyOverride", Val::Str("old".into()))]);
        let proxy = WindowsProxy::new(reg);
        let s = ProxySettings {
            socks: None,
            bypass: Vec::new(),
            exclude_simple: false,
            ..settings()
        };
        proxy.apply(&s).unwrap();
        let values = proxy.reg.dump();
        assert_eq!(
            values["ProxyServer"],
            Val::Str("http=127.0.0.1:6152;https=127.0.0.1:6152".into())
        );
        assert!(!values.contains_key("ProxyOverride"));
    }

    #[test]
    fn apply_without_any_address_is_an_error_and_writes_nothing() {
        let proxy = WindowsProxy::new(FakeRegistry::default());
        let err = proxy.apply(&ProxySettings::default()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(proxy.reg.dump().is_empty());
        assert_eq!(proxy.reg.notified.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn snapshot_then_restore_round_trips_present_and_absent_values() {
        let original = [
            ("ProxyEnable", Val::U32(0)),
            ("ProxyServer", Val::Str("corp:8080".into())),
        ];
        let proxy = WindowsProxy::new(FakeRegistry::with(&original));
        let backup = proxy.snapshot().unwrap();
        assert_eq!(backup.0["platform"], "windows");
        assert_eq!(backup.0["ProxyEnable"], 0);
        assert_eq!(backup.0["ProxyServer"], "corp:8080");
        assert!(backup.0["ProxyOverride"].is_null());
        proxy.apply(&settings()).unwrap();
        assert!(proxy.reg.dump().contains_key("ProxyOverride"));
        proxy.restore(&backup).unwrap();
        let expected: BTreeMap<String, Val> = original
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        assert_eq!(proxy.reg.dump(), expected, "ProxyOverride is absent again");
        assert_eq!(proxy.reg.notified.load(Ordering::SeqCst), 2);
        // the backup survives a JSON round trip (it lives in state.json)
        let text = serde_json::to_string(&backup).unwrap();
        let reread: Backup = serde_json::from_str(&text).unwrap();
        assert_eq!(reread, backup);
    }

    #[test]
    fn restore_rejects_a_backup_from_another_backend() {
        let proxy = WindowsProxy::new(FakeRegistry::default());
        let foreign = Backup(serde_json::json!({ "platform": "macos", "services": {} }));
        let err = proxy.restore(&foreign).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(proxy.reg.notified.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn bypass_entries_become_proxy_override_patterns() {
        let value = |entries: &[&str]| {
            proxy_override_value(&ProxySettings {
                bypass: entries.iter().map(|s| s.to_string()).collect(),
                ..ProxySettings::default()
            })
        };
        assert_eq!(value(&["localhost", "127.0.0.1", "*.corp.example"]), "localhost;127.0.0.1;*.corp.example");
        assert_eq!(value(&["10.0.0.0/8"]), "10.*");
        assert_eq!(value(&["192.168.0.0/16"]), "192.168.*");
        assert_eq!(value(&["1.2.3.0/24"]), "1.2.3.*");
        assert_eq!(value(&["1.2.3.4/32"]), "1.2.3.4");
        assert_eq!(value(&["0.0.0.0/0"]), "*");
        let twelve = value(&["172.16.0.0/12"]);
        let parts: Vec<&str> = twelve.split(';').collect();
        assert_eq!(parts.len(), 16);
        assert_eq!((parts[0], parts[15]), ("172.16.*", "172.31.*"));
        assert_eq!(value(&["1.2.3.252/30"]), "1.2.3.252;1.2.3.253;1.2.3.254;1.2.3.255");
        assert_eq!(value(&["::1"]), "[::1]");
        assert_eq!(value(&["fd00::/8", "10.0.0.0/8", "10.0.0.0/8"]), "10.*", "IPv6 networks are dropped, duplicates collapse");
    }

    #[cfg(windows)]
    #[test]
    fn real_registry_round_trips_under_a_scratch_key() {
        let path = format!(r"Software\rurge-test-{}", std::process::id());
        let reg = RealRegistry::at(path.clone());
        assert_eq!(reg.get_u32("ProxyEnable").unwrap(), None, "a missing key reads as absent");
        reg.set_u32("ProxyEnable", 1).unwrap();
        reg.set_string("ProxyServer", "http=127.0.0.1:1").unwrap();
        assert_eq!(reg.get_u32("ProxyEnable").unwrap(), Some(1));
        assert_eq!(reg.get_string("ProxyServer").unwrap().as_deref(), Some("http=127.0.0.1:1"));
        assert_eq!(reg.get_string("ProxyOverride").unwrap(), None);
        reg.delete("ProxyServer").unwrap();
        reg.delete("ProxyServer").unwrap(); // deleting an absent value is fine
        assert_eq!(reg.get_string("ProxyServer").unwrap(), None);
        windows_registry::CURRENT_USER.remove_tree(&path).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn real_internet_settings_can_be_snapshotted() {
        // read-only: this never writes the machine's proxy settings
        let backup = WindowsProxy::new(RealRegistry::internet_settings())
            .snapshot()
            .unwrap();
        assert_eq!(backup.0["platform"], "windows");
    }
}
```

- [ ] **Step 3: 运行确认失败**

Run: `cargo test -p rurge-platform sysproxy::`
Expected: 编译失败（`sysproxy` 模块与其中的类型都不存在）。

- [ ] **Step 4: 实现**

`crates/rurge-platform/src/sysproxy/mod.rs`：

```rust
//! System proxy settings (M4 design §8.1). Every backend's logic is compiled
//! on every host — only the thin layer that touches the OS is `cfg`-gated or
//! injected — so each platform is unit-tested everywhere (the approach `dirs`
//! takes with `Os`).

pub mod windows;

use serde::{Deserialize, Serialize};
use std::io;
use std::net::SocketAddr;

/// What the operating system should be pointed at.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProxySettings {
    pub http: Option<SocketAddr>,
    pub https: Option<SocketAddr>,
    pub socks: Option<SocketAddr>,
    /// Surge `skip-proxy` semantics (macOS): host names / globs, IPs, CIDRs.
    pub bypass: Vec<String>,
    pub exclude_simple: bool,
}

/// The OS settings as they were before `apply`. Opaque to callers; it is
/// stored in `state.json` so the next run can undo a crashed one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Backup(pub serde_json::Value);

pub trait SystemProxy: Send + Sync {
    fn snapshot(&self) -> io::Result<Backup>;
    fn apply(&self, settings: &ProxySettings) -> io::Result<()>;
    fn restore(&self, backup: &Backup) -> io::Result<()>;
}

pub(crate) fn wrong_platform(expected: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("the system proxy backup was not taken by the {expected} backend"),
    )
}
```

`crates/rurge-platform/src/sysproxy/windows.rs`（测试模块之前）：

```rust
//! Windows: the per-user Internet Settings key plus a WinINet refresh.

use super::{Backup, ProxySettings, SystemProxy};
use serde::{Deserialize, Serialize};
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr};

pub const KEY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";
const ENABLE: &str = "ProxyEnable";
const SERVER: &str = "ProxyServer";
const OVERRIDE: &str = "ProxyOverride";

/// The registry operations the backend needs; faked in tests.
pub trait Registry: Send + Sync {
    fn get_u32(&self, name: &str) -> io::Result<Option<u32>>;
    fn get_string(&self, name: &str) -> io::Result<Option<String>>;
    fn set_u32(&self, name: &str, value: u32) -> io::Result<()>;
    fn set_string(&self, name: &str, value: &str) -> io::Result<()>;
    /// Removing a value that does not exist is not an error.
    fn delete(&self, name: &str) -> io::Result<()>;
    /// Tells WinINet the settings changed.
    fn notify(&self);
}

#[derive(Serialize, Deserialize)]
struct WindowsBackup {
    platform: String,
    #[serde(rename = "ProxyEnable")]
    enable: Option<u32>,
    #[serde(rename = "ProxyServer")]
    server: Option<String>,
    #[serde(rename = "ProxyOverride")]
    bypass: Option<String>,
}

pub struct WindowsProxy<R: Registry> {
    reg: R,
}

impl<R: Registry> WindowsProxy<R> {
    pub fn new(reg: R) -> Self {
        WindowsProxy { reg }
    }
}

impl<R: Registry> SystemProxy for WindowsProxy<R> {
    fn snapshot(&self) -> io::Result<Backup> {
        let backup = WindowsBackup {
            platform: "windows".to_string(),
            enable: self.reg.get_u32(ENABLE)?,
            server: self.reg.get_string(SERVER)?,
            bypass: self.reg.get_string(OVERRIDE)?,
        };
        Ok(Backup(
            serde_json::to_value(backup).expect("backup serializes"),
        ))
    }

    fn apply(&self, settings: &ProxySettings) -> io::Result<()> {
        let server = proxy_server_value(settings);
        if server.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "no proxy address to apply",
            ));
        }
        self.reg.set_string(SERVER, &server)?;
        let bypass = proxy_override_value(settings);
        if bypass.is_empty() {
            self.reg.delete(OVERRIDE)?;
        } else {
            self.reg.set_string(OVERRIDE, &bypass)?;
        }
        // switched on last, once the address it points at is in place
        self.reg.set_u32(ENABLE, 1)?;
        self.reg.notify();
        Ok(())
    }

    fn restore(&self, backup: &Backup) -> io::Result<()> {
        let saved: WindowsBackup = serde_json::from_value(backup.0.clone())
            .map_err(|_| super::wrong_platform("windows"))?;
        if saved.platform != "windows" {
            return Err(super::wrong_platform("windows"));
        }
        match saved.enable {
            Some(v) => self.reg.set_u32(ENABLE, v)?,
            None => self.reg.delete(ENABLE)?,
        }
        match &saved.server {
            Some(v) => self.reg.set_string(SERVER, v)?,
            None => self.reg.delete(SERVER)?,
        }
        match &saved.bypass {
            Some(v) => self.reg.set_string(OVERRIDE, v)?,
            None => self.reg.delete(OVERRIDE)?,
        }
        self.reg.notify();
        Ok(())
    }
}

/// `http=h:p;https=h:p;socks=h:p`, leaving out what is not set.
pub fn proxy_server_value(settings: &ProxySettings) -> String {
    let mut parts = Vec::new();
    if let Some(addr) = settings.http {
        parts.push(format!("http={addr}"));
    }
    if let Some(addr) = settings.https {
        parts.push(format!("https={addr}"));
    }
    if let Some(addr) = settings.socks {
        parts.push(format!("socks={addr}"));
    }
    parts.join(";")
}

/// `;`-separated patterns, `<local>` last when simple host names are excluded.
pub fn proxy_override_value(settings: &ProxySettings) -> String {
    let mut out: Vec<String> = Vec::new();
    for entry in &settings.bypass {
        for pattern in override_patterns(entry) {
            if !out.contains(&pattern) {
                out.push(pattern);
            }
        }
    }
    if settings.exclude_simple {
        out.push("<local>".to_string());
    }
    out.join(";")
}

/// `ProxyOverride` has no CIDR syntax: an IPv4 network becomes wildcard
/// patterns, an IPv6 literal gets brackets, anything else passes through. An
/// IPv6 network cannot be expressed and is dropped.
fn override_patterns(entry: &str) -> Vec<String> {
    let entry = entry.trim();
    if let Some((addr, prefix)) = entry.split_once('/') {
        return match (addr.parse::<Ipv4Addr>(), prefix.parse::<u8>()) {
            (Ok(ip), Ok(prefix)) if prefix <= 32 => v4_wildcards(ip, prefix),
            _ => {
                tracing::debug!(entry, "skip-proxy entry cannot be expressed in ProxyOverride; dropped");
                Vec::new()
            }
        };
    }
    if entry.parse::<Ipv6Addr>().is_ok() {
        return vec![format!("[{entry}]")];
    }
    if entry.is_empty() {
        Vec::new()
    } else {
        vec![entry.to_string()]
    }
}

fn v4_wildcards(ip: Ipv4Addr, prefix: u8) -> Vec<String> {
    if prefix == 32 {
        return vec![ip.to_string()];
    }
    let octets = ip.octets();
    let whole = usize::from(prefix / 8); // octets that are fully fixed
    let rest = prefix % 8;
    let fixed: Vec<String> = octets[..whole].iter().map(|o| o.to_string()).collect();
    if rest == 0 {
        return vec![if whole == 0 {
            "*".to_string()
        } else {
            format!("{}.*", fixed.join("."))
        }];
    }
    // the partially fixed octet enumerates 2^(8 - rest) values
    let span = 1u16 << (8 - rest);
    let base = u16::from(octets[whole]) & !(span - 1);
    (base..base + span)
        .map(|value| {
            let mut parts = fixed.clone();
            parts.push(value.to_string());
            if whole + 1 == 4 {
                parts.join(".")
            } else {
                format!("{}.*", parts.join("."))
            }
        })
        .collect()
}

/// A key under `HKEY_CURRENT_USER`, opened per call.
#[cfg(windows)]
pub struct RealRegistry {
    path: String,
}

#[cfg(windows)]
/// `HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND)`: the key or the value is absent.
const NOT_FOUND: i32 = 0x8007_0002_u32 as i32;

#[cfg(windows)]
impl RealRegistry {
    pub fn internet_settings() -> RealRegistry {
        RealRegistry::at(KEY_PATH)
    }

    /// Any other key under `HKEY_CURRENT_USER` (the tests use a scratch key).
    pub fn at(path: impl Into<String>) -> RealRegistry {
        RealRegistry { path: path.into() }
    }

    fn read<T>(
        &self,
        get: impl FnOnce(&windows_registry::Key) -> windows_registry::Result<T>,
    ) -> io::Result<Option<T>> {
        let key = match windows_registry::CURRENT_USER.open(&self.path) {
            Ok(key) => key,
            Err(e) if e.code().0 == NOT_FOUND => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        match get(&key) {
            Ok(value) => Ok(Some(value)),
            Err(e) if e.code().0 == NOT_FOUND => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn writable(&self) -> io::Result<windows_registry::Key> {
        Ok(windows_registry::CURRENT_USER.create(&self.path)?)
    }
}

#[cfg(windows)]
impl Registry for RealRegistry {
    fn get_u32(&self, name: &str) -> io::Result<Option<u32>> {
        self.read(|key| key.get_u32(name))
    }

    fn get_string(&self, name: &str) -> io::Result<Option<String>> {
        self.read(|key| key.get_string(name))
    }

    fn set_u32(&self, name: &str, value: u32) -> io::Result<()> {
        Ok(self.writable()?.set_u32(name, value)?)
    }

    fn set_string(&self, name: &str, value: &str) -> io::Result<()> {
        Ok(self.writable()?.set_string(name, value)?)
    }

    fn delete(&self, name: &str) -> io::Result<()> {
        match self.writable()?.remove_value(name) {
            Ok(()) => Ok(()),
            Err(e) if e.code().0 == NOT_FOUND => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn notify(&self) {
        notify_wininet();
    }
}

/// The one `unsafe` in the workspace (plan decision P1).
#[cfg(windows)]
#[allow(unsafe_code)]
fn notify_wininet() {
    use windows_sys::Win32::Networking::WinInet::{
        INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED, InternetSetOptionW,
    };
    // SAFETY: a null handle with a null buffer of length 0 is the documented
    // way to broadcast these two options; the calls read no memory of ours.
    let (changed, refreshed) = unsafe {
        (
            InternetSetOptionW(
                std::ptr::null(),
                INTERNET_OPTION_SETTINGS_CHANGED,
                std::ptr::null(),
                0,
            ),
            InternetSetOptionW(std::ptr::null(), INTERNET_OPTION_REFRESH, std::ptr::null(), 0),
        )
    };
    if changed == 0 || refreshed == 0 {
        tracing::debug!("InternetSetOptionW reported a failure; running WinINet applications may notice the change late");
    }
}
```

（`#[cfg(windows)]` 与文档注释的先后按 rustfmt / clippy 的要求调整；`NOT_FOUND` 的文档注释放在 `#[cfg]` 之前。`windows_result::Error` 实现了 `From<Error> for std::io::Error`，所以 `?` 与 `.into()` 直接可用；`e.code()` 返回 `HRESULT(pub i32)`。）

- [ ] **Step 5: 运行确认通过**

Run: `cargo test -p rurge-platform sysproxy::`
Expected: 6 个跨平台测试 + Windows 上另外 2 个全部 PASS。`cargo clippy -p rurge-platform --all-targets -- -D warnings` 无警告（`unsafe_code` 是 `deny`，只有 `notify_wininet` 放行）。

- [ ] **Step 6: 质量门与提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add Cargo.toml Cargo.lock crates/rurge-platform
git commit -F - <<'EOF'
feat(platform): sysproxy 核心类型与 Windows 后端（注册表 + WinINet 通知）

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th
EOF
```

---

### Task 2: 命令执行抽象与 macOS 后端

**Files:**
- Create: `crates/rurge-platform/src/command.rs`、`crates/rurge-platform/src/sysproxy/macos.rs`
- Modify: `crates/rurge-platform/src/lib.rs`（`pub mod command;`）、`crates/rurge-platform/src/sysproxy/mod.rs`（`pub mod macos;`）

**Interfaces:**
- Consumes: Task 1 的 `ProxySettings`、`Backup`、`SystemProxy`、`wrong_platform`。
- Produces:
  - `rurge_platform::command::{Cmd = Vec<String>（`cmd[0]` 是程序名）, CommandRunner { run(&self, cmd: &[String]) -> io::Result<String> }, SystemRunner, run_all(&dyn CommandRunner, &[Cmd]) -> io::Result<()>（遇错即停）, run_best_effort(&dyn CommandRunner, &[Cmd]) -> io::Result<()>（全部执行，返回第一个错误）}`；`SystemRunner::run` 成功返回 stdout，失败返回带 stderr（为空则 stdout——`networksetup` 把错误打到 stdout）的 `io::Error`。
  - 测试用 `command::testing::FakeRunner { reply(cmd, out), fail(cmd, err), calls() -> Vec<String> }`（`#[cfg(test)] pub(crate)`；按空格拼接的整条命令查表，缺省回空 stdout）。
  - `sysproxy::macos::{ProxyState { enabled, server, port }, ServiceBackup { web, secure, socks, bypass }, parse_services, parse_proxy, parse_bypass, apply_commands(&[String], &ProxySettings) -> Vec<Cmd>, restore_commands(&BTreeMap<String, ServiceBackup>) -> Vec<Cmd>, MacosProxy<R: CommandRunner>, MacosProxy::new(R)}`。
  - 备份 JSON：`{"platform":"macos","services":{"<服务名>":{"web":{"enabled","server","port"},"secure":{…},"socks":{…},"bypass":[…]}}}`。

- [ ] **Step 1: 写失败测试**

`crates/rurge-platform/src/sysproxy/macos.rs` 先放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testing::FakeRunner;

    const SERVICES: &str = "An asterisk (*) denotes that a network service is disabled.\nWi-Fi\n*Thunderbolt Bridge\nUSB 10/100/1000 LAN\n";

    fn settings() -> ProxySettings {
        ProxySettings {
            http: Some("127.0.0.1:6152".parse().unwrap()),
            https: Some("127.0.0.1:6152".parse().unwrap()),
            socks: None,
            bypass: vec!["localhost".into(), "192.168.0.0/16".into()],
            exclude_simple: false,
        }
    }

    #[test]
    fn parses_the_service_list_skipping_the_header_and_disabled_services() {
        assert_eq!(parse_services(SERVICES), ["Wi-Fi", "USB 10/100/1000 LAN"]);
        assert!(parse_services("").is_empty());
    }

    #[test]
    fn parses_proxy_state() {
        let on = parse_proxy("Enabled: Yes\nServer: 10.0.0.1\nPort: 8080\nAuthenticated Proxy Enabled: 0\n");
        assert_eq!(on, ProxyState { enabled: true, server: "10.0.0.1".into(), port: 8080 });
        let off = parse_proxy("Enabled: No\nServer: \nPort: 0\nAuthenticated Proxy Enabled: 0\n");
        assert_eq!(off, ProxyState::default());
        assert_eq!(parse_proxy("Enabled: Yes\nServer: ::1\nPort: 1\n").server, "::1");
    }

    #[test]
    fn parses_bypass_domains() {
        assert_eq!(parse_bypass("*.local\n169.254/16\n"), ["*.local", "169.254/16"]);
        assert!(parse_bypass("There aren't any bypass domains set on Wi-Fi.\n").is_empty());
    }

    #[test]
    fn apply_commands_cover_every_kind_for_every_service() {
        let cmds: Vec<String> = apply_commands(&["Wi-Fi".to_string()], &settings())
            .iter()
            .map(|c| c.join(" "))
            .collect();
        assert_eq!(
            cmds,
            [
                "networksetup -setwebproxy Wi-Fi 127.0.0.1 6152",
                "networksetup -setwebproxystate Wi-Fi on",
                "networksetup -setsecurewebproxy Wi-Fi 127.0.0.1 6152",
                "networksetup -setsecurewebproxystate Wi-Fi on",
                "networksetup -setsocksfirewallproxystate Wi-Fi off",
                "networksetup -setproxybypassdomains Wi-Fi localhost 192.168.0.0/16",
            ]
        );
        let no_bypass = ProxySettings { bypass: Vec::new(), ..settings() };
        let last = apply_commands(&["Wi-Fi".to_string()], &no_bypass).pop().unwrap();
        assert_eq!(last.join(" "), "networksetup -setproxybypassdomains Wi-Fi Empty");
        // a service name with spaces stays one argument
        let spaced = apply_commands(&["USB 10/100/1000 LAN".to_string()], &settings());
        assert_eq!(spaced[0][2], "USB 10/100/1000 LAN");
    }

    fn runner_with_one_service() -> FakeRunner {
        let runner = FakeRunner::default();
        runner.reply("networksetup -listallnetworkservices", "An asterisk (*) denotes that a network service is disabled.\nWi-Fi\n");
        runner.reply("networksetup -getwebproxy Wi-Fi", "Enabled: Yes\nServer: corp\nPort: 3128\n");
        runner.reply("networksetup -getsecurewebproxy Wi-Fi", "Enabled: No\nServer: corp\nPort: 3128\n");
        runner.reply("networksetup -getsocksfirewallproxy Wi-Fi", "Enabled: No\nServer: \nPort: 0\n");
        runner.reply("networksetup -getproxybypassdomains Wi-Fi", "*.local\n");
        runner
    }

    #[test]
    fn snapshot_then_restore_puts_every_service_back() {
        let proxy = MacosProxy::new(runner_with_one_service());
        let backup = proxy.snapshot().unwrap();
        assert_eq!(backup.0["platform"], "macos");
        assert_eq!(backup.0["services"]["Wi-Fi"]["web"]["server"], "corp");
        assert_eq!(backup.0["services"]["Wi-Fi"]["bypass"][0], "*.local");
        proxy.runner.calls.lock().unwrap().clear();
        proxy.restore(&backup).unwrap();
        assert_eq!(
            proxy.runner.calls(),
            [
                "networksetup -setwebproxy Wi-Fi corp 3128",
                "networksetup -setwebproxystate Wi-Fi on",
                "networksetup -setsecurewebproxy Wi-Fi corp 3128",
                "networksetup -setsecurewebproxystate Wi-Fi off",
                "networksetup -setsocksfirewallproxystate Wi-Fi off",
                "networksetup -setproxybypassdomains Wi-Fi *.local",
            ]
        );
    }

    #[test]
    fn apply_stops_at_the_first_failure_and_reports_it() {
        let runner = runner_with_one_service();
        runner.fail("networksetup -setwebproxy Wi-Fi 127.0.0.1 6152", "** Error: Command requires admin privileges.");
        let proxy = MacosProxy::new(runner);
        let err = proxy.apply(&settings()).unwrap_err();
        assert!(err.to_string().contains("requires admin privileges"), "{err}");
        let calls = proxy.runner.calls();
        assert_eq!(calls.last().unwrap(), "networksetup -setwebproxy Wi-Fi 127.0.0.1 6152");
    }

    #[test]
    fn restore_keeps_going_after_a_failure_and_returns_the_first_error() {
        let proxy = MacosProxy::new(runner_with_one_service());
        let backup = proxy.snapshot().unwrap();
        proxy.runner.fail("networksetup -setwebproxy Wi-Fi corp 3128", "boom");
        proxy.runner.calls.lock().unwrap().clear();
        let err = proxy.restore(&backup).unwrap_err();
        assert!(err.to_string().contains("boom"));
        assert_eq!(proxy.runner.calls().len(), 6, "the remaining commands still ran");
    }

    #[test]
    fn apply_needs_an_enabled_service_and_restore_rejects_foreign_backups() {
        let runner = FakeRunner::default();
        runner.reply("networksetup -listallnetworkservices", "An asterisk (*) denotes that a network service is disabled.\n*Wi-Fi\n");
        let proxy = MacosProxy::new(runner);
        assert_eq!(proxy.apply(&settings()).unwrap_err().kind(), io::ErrorKind::NotFound);
        let foreign = Backup(serde_json::json!({ "platform": "windows" }));
        assert_eq!(proxy.restore(&foreign).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p rurge-platform macos::`
Expected: 编译失败（`command` 模块、`MacosProxy` 等不存在）。

- [ ] **Step 3: 实现 `command.rs`**

```rust
//! Running external tools (`networksetup`, `gsettings`, `systemctl`, …) behind
//! a trait, so the code that decides *what* to run is tested without running it.

use std::io;

/// One external command: `cmd[0]` is the program.
pub type Cmd = Vec<String>;

pub trait CommandRunner: Send + Sync {
    /// Runs the command; stdout on success, otherwise an error that carries
    /// what the tool printed.
    fn run(&self, cmd: &[String]) -> io::Result<String>;
}

pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, cmd: &[String]) -> io::Result<String> {
        let (program, args) = cmd
            .split_first()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty command"))?;
        let output = std::process::Command::new(program)
            .args(args)
            .output()
            .map_err(|e| io::Error::new(e.kind(), format!("cannot run {program}: {e}")))?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        if output.status.success() {
            return Ok(stdout);
        }
        // `networksetup` reports its errors on stdout
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        Err(io::Error::other(format!(
            "`{}` failed ({}): {detail}",
            cmd.join(" "),
            output.status
        )))
    }
}

/// Runs every command, stopping at the first failure.
pub fn run_all(runner: &dyn CommandRunner, cmds: &[Cmd]) -> io::Result<()> {
    cmds.iter().try_for_each(|c| runner.run(c).map(drop))
}

/// Runs every command even when some fail (undoing things should get as far
/// as it can); the first error is returned.
pub fn run_best_effort(runner: &dyn CommandRunner, cmds: &[Cmd]) -> io::Result<()> {
    let mut first_error = None;
    for c in cmds {
        if let Err(e) = runner.run(c) {
            tracing::warn!(error = %e, "command failed; continuing");
            first_error.get_or_insert(e);
        }
    }
    first_error.map_or(Ok(()), Err)
}

#[cfg(test)]
pub(crate) mod testing {
    use super::CommandRunner;
    use std::collections::HashMap;
    use std::io;
    use std::sync::Mutex;

    /// Records every command. Replies are looked up by the space-joined
    /// command; anything not listed succeeds with empty output.
    #[derive(Default)]
    pub struct FakeRunner {
        pub calls: Mutex<Vec<String>>,
        replies: Mutex<HashMap<String, Result<String, String>>>,
    }

    impl FakeRunner {
        pub fn reply(&self, cmd: &str, stdout: &str) {
            self.replies
                .lock()
                .unwrap()
                .insert(cmd.to_string(), Ok(stdout.to_string()));
        }
        pub fn fail(&self, cmd: &str, message: &str) {
            self.replies
                .lock()
                .unwrap()
                .insert(cmd.to_string(), Err(message.to_string()));
        }
        pub fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, cmd: &[String]) -> io::Result<String> {
            let line = cmd.join(" ");
            self.calls.lock().unwrap().push(line.clone());
            match self.replies.lock().unwrap().get(&line) {
                Some(Ok(stdout)) => Ok(stdout.clone()),
                Some(Err(message)) => Err(io::Error::other(message.clone())),
                None => Ok(String::new()),
            }
        }
    }
}
```

- [ ] **Step 4: 实现 `macos.rs`（测试模块之前）**

```rust
//! macOS: `networksetup`, once per enabled network service.

use super::{Backup, ProxySettings, SystemProxy};
use crate::command::{Cmd, CommandRunner, run_all, run_best_effort};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;
use std::net::SocketAddr;

const TOOL: &str = "networksetup";

/// The `-get…`, `-set…` and `-set…state` flags of one proxy kind.
struct Kind {
    get: &'static str,
    set: &'static str,
    state: &'static str,
}

const WEB: Kind = Kind {
    get: "-getwebproxy",
    set: "-setwebproxy",
    state: "-setwebproxystate",
};
const SECURE: Kind = Kind {
    get: "-getsecurewebproxy",
    set: "-setsecurewebproxy",
    state: "-setsecurewebproxystate",
};
const SOCKS: Kind = Kind {
    get: "-getsocksfirewallproxy",
    set: "-setsocksfirewallproxy",
    state: "-setsocksfirewallproxystate",
};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyState {
    pub enabled: bool,
    pub server: String,
    pub port: u16,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceBackup {
    pub web: ProxyState,
    pub secure: ProxyState,
    pub socks: ProxyState,
    pub bypass: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct MacosBackup {
    platform: String,
    services: BTreeMap<String, ServiceBackup>,
}

/// `-listallnetworkservices`: a header line, then one service per line; a
/// leading `*` marks a disabled service.
pub fn parse_services(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("An asterisk") && !l.starts_with('*'))
        .map(str::to_string)
        .collect()
}

/// `Enabled: Yes|No`, `Server: <host>`, `Port: <n>`.
pub fn parse_proxy(output: &str) -> ProxyState {
    let mut state = ProxyState::default();
    for line in output.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "Enabled" => state.enabled = value.eq_ignore_ascii_case("yes"),
            "Server" => state.server = value.to_string(),
            "Port" => state.port = value.parse().unwrap_or(0),
            _ => {}
        }
    }
    state
}

/// One domain per line, or a sentence saying there are none.
pub fn parse_bypass(output: &str) -> Vec<String> {
    if output.trim_start().starts_with("There aren't any") {
        return Vec::new();
    }
    output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

fn cmd(args: &[&str]) -> Cmd {
    std::iter::once(TOOL)
        .chain(args.iter().copied())
        .map(str::to_string)
        .collect()
}

fn bypass_cmd(service: &str, domains: &[String]) -> Cmd {
    let mut c = cmd(&["-setproxybypassdomains", service]);
    if domains.is_empty() {
        c.push("Empty".to_string());
    } else {
        c.extend(domains.iter().cloned());
    }
    c
}

fn apply_kind(out: &mut Vec<Cmd>, service: &str, kind: &Kind, addr: Option<SocketAddr>) {
    match addr {
        Some(addr) => {
            out.push(cmd(&[
                kind.set,
                service,
                &addr.ip().to_string(),
                &addr.port().to_string(),
            ]));
            out.push(cmd(&[kind.state, service, "on"]));
        }
        None => out.push(cmd(&[kind.state, service, "off"])),
    }
}

pub fn apply_commands(services: &[String], settings: &ProxySettings) -> Vec<Cmd> {
    let mut out = Vec::new();
    for service in services {
        apply_kind(&mut out, service, &WEB, settings.http);
        apply_kind(&mut out, service, &SECURE, settings.https);
        apply_kind(&mut out, service, &SOCKS, settings.socks);
        out.push(bypass_cmd(service, &settings.bypass));
    }
    out
}

fn restore_kind(out: &mut Vec<Cmd>, service: &str, kind: &Kind, state: &ProxyState) {
    if state.server.is_empty() {
        // `-set…proxy` refuses an empty server; off is all there is to restore
        out.push(cmd(&[kind.state, service, "off"]));
    } else {
        out.push(cmd(&[
            kind.set,
            service,
            &state.server,
            &state.port.to_string(),
        ]));
        out.push(cmd(&[
            kind.state,
            service,
            if state.enabled { "on" } else { "off" },
        ]));
    }
}

pub fn restore_commands(services: &BTreeMap<String, ServiceBackup>) -> Vec<Cmd> {
    let mut out = Vec::new();
    for (service, saved) in services {
        restore_kind(&mut out, service, &WEB, &saved.web);
        restore_kind(&mut out, service, &SECURE, &saved.secure);
        restore_kind(&mut out, service, &SOCKS, &saved.socks);
        out.push(bypass_cmd(service, &saved.bypass));
    }
    out
}

pub struct MacosProxy<R: CommandRunner> {
    runner: R,
}

impl<R: CommandRunner> MacosProxy<R> {
    pub fn new(runner: R) -> Self {
        MacosProxy { runner }
    }

    fn services(&self) -> io::Result<Vec<String>> {
        Ok(parse_services(
            &self.runner.run(&cmd(&["-listallnetworkservices"]))?,
        ))
    }

    fn state(&self, kind: &Kind, service: &str) -> io::Result<ProxyState> {
        Ok(parse_proxy(&self.runner.run(&cmd(&[kind.get, service]))?))
    }
}

impl<R: CommandRunner> SystemProxy for MacosProxy<R> {
    fn snapshot(&self) -> io::Result<Backup> {
        let mut services = BTreeMap::new();
        for service in self.services()? {
            let saved = ServiceBackup {
                web: self.state(&WEB, &service)?,
                secure: self.state(&SECURE, &service)?,
                socks: self.state(&SOCKS, &service)?,
                bypass: parse_bypass(
                    &self
                        .runner
                        .run(&cmd(&["-getproxybypassdomains", &service]))?,
                ),
            };
            services.insert(service, saved);
        }
        let backup = MacosBackup {
            platform: "macos".to_string(),
            services,
        };
        Ok(Backup(
            serde_json::to_value(backup).expect("backup serializes"),
        ))
    }

    fn apply(&self, settings: &ProxySettings) -> io::Result<()> {
        let services = self.services()?;
        if services.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "networksetup lists no enabled network service",
            ));
        }
        if settings.exclude_simple {
            tracing::warn!("exclude-simple-hostnames cannot be set through networksetup; ignored");
        }
        run_all(&self.runner, &apply_commands(&services, settings))
    }

    /// Best effort across services: every command runs, the first error is
    /// returned.
    fn restore(&self, backup: &Backup) -> io::Result<()> {
        let saved: MacosBackup = serde_json::from_value(backup.0.clone())
            .map_err(|_| super::wrong_platform("macos"))?;
        if saved.platform != "macos" {
            return Err(super::wrong_platform("macos"));
        }
        run_best_effort(&self.runner, &restore_commands(&saved.services))
    }
}
```

`lib.rs` 加 `pub mod command;`；`sysproxy/mod.rs` 加 `pub mod macos;`。

- [ ] **Step 5: 运行确认通过**

Run: `cargo test -p rurge-platform`
Expected: `macos::` 的 8 个测试与 Task 1 的测试全部 PASS。

- [ ] **Step 6: 质量门与提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-platform
git commit -F - <<'EOF'
feat(platform): 命令执行抽象与 macOS 系统代理后端（networksetup）

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th
EOF
```

---

### Task 3: Linux 后端（GNOME / KDE / 环境变量提示）与 `platform()`

**Files:**
- Create: `crates/rurge-platform/src/sysproxy/linux.rs`
- Modify: `crates/rurge-platform/src/sysproxy/mod.rs`（`pub mod linux;`、`platform()`）

**Interfaces:**
- Consumes: Task 1 的核心类型；Task 2 的 `command::{Cmd, CommandRunner, SystemRunner, run_all, run_best_effort}`；`crate::dirs::EnvLookup`。
- Produces:
  - `sysproxy::linux::{Desktop::{Gnome, Kde, Other}, detect(EnvLookup<'_>, &dyn Fn(&str) -> bool) -> Desktop, kde_tools(&dyn Fn(&str) -> bool) -> Option<(String, String)>（写工具, 读工具）, tool_on_path(&str) -> bool, gnome_apply_commands(&ProxySettings) -> Vec<Cmd>, kde_apply_commands(write_tool, &ProxySettings) -> Vec<Cmd>, env_hint(&ProxySettings) -> String, LinuxProxy<R: CommandRunner>, LinuxProxy::{new(R, Desktop, Option<(String, String)>), detect(R)}}`。
  - 备份 JSON：`{"platform":"linux","desktop":"gnome"|"kde"|"none","values":{…}}`；GNOME 的键是 `"<schema> <key>"`、值是 `gsettings get` 的原始输出（GVariant 文本，恢复时原样 `set` 回去）；KDE 的键是 `kioslaverc` 的键名、值是 `kreadconfig` 的输出（空串 = 原本未设，恢复时 `--delete`）。
  - `sysproxy::platform() -> Box<dyn SystemProxy>`：Windows → `WindowsProxy<RealRegistry>`；macOS → `MacosProxy<SystemRunner>`；其它 Unix → `LinuxProxy::detect(SystemRunner)`。
  - `Desktop::Other` 上：`snapshot` 成功（`desktop: "none"`），`apply` 返回 `ErrorKind::Unsupported`，错误文本含 `export http_proxy=… https_proxy=… no_proxy=…` 提示，`restore` 是空操作。

- [ ] **Step 1: 写失败测试**

`crates/rurge-platform/src/sysproxy/linux.rs` 先放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testing::FakeRunner;
    use std::ffi::OsString;

    fn settings() -> ProxySettings {
        ProxySettings {
            http: Some("127.0.0.1:6152".parse().unwrap()),
            https: Some("127.0.0.1:6152".parse().unwrap()),
            socks: Some("127.0.0.1:6153".parse().unwrap()),
            bypass: vec!["localhost".into(), "192.168.0.0/16".into()],
            exclude_simple: false,
        }
    }

    fn desktop_env(value: &'static str) -> impl Fn(&str) -> Option<OsString> {
        move |key| (key == "XDG_CURRENT_DESKTOP").then(|| OsString::from(value))
    }

    fn lines(cmds: &[Cmd]) -> Vec<String> {
        cmds.iter().map(|c| c.join(" ")).collect()
    }

    #[test]
    fn detects_the_desktop_from_the_environment_and_the_tools() {
        let all = |_: &str| true;
        let none = |_: &str| false;
        let plasma5 = |name: &str| name.ends_with('5');
        assert_eq!(detect(&desktop_env("ubuntu:GNOME"), &all), Desktop::Gnome);
        assert_eq!(detect(&desktop_env("GNOME"), &none), Desktop::Other, "gsettings is required");
        assert_eq!(detect(&desktop_env("KDE"), &all), Desktop::Kde);
        assert_eq!(detect(&desktop_env("KDE"), &plasma5), Desktop::Kde, "Plasma 5 tools are accepted");
        assert_eq!(detect(&desktop_env("KDE"), &none), Desktop::Other);
        assert_eq!(detect(&desktop_env("XFCE"), &all), Desktop::Other);
        assert_eq!(detect(&|_| None, &all), Desktop::Other);
        assert_eq!(kde_tools(&all), Some(("kwriteconfig6".to_string(), "kreadconfig6".to_string())));
        assert_eq!(kde_tools(&plasma5), Some(("kwriteconfig5".to_string(), "kreadconfig5".to_string())));
        assert_eq!(kde_tools(&none), None);
    }

    #[test]
    fn gnome_apply_sets_hosts_and_bypass_then_switches_to_manual() {
        assert_eq!(
            lines(&gnome_apply_commands(&settings())),
            [
                "gsettings set org.gnome.system.proxy.http host '127.0.0.1'",
                "gsettings set org.gnome.system.proxy.http port 6152",
                "gsettings set org.gnome.system.proxy.https host '127.0.0.1'",
                "gsettings set org.gnome.system.proxy.https port 6152",
                "gsettings set org.gnome.system.proxy.socks host '127.0.0.1'",
                "gsettings set org.gnome.system.proxy.socks port 6153",
                "gsettings set org.gnome.system.proxy ignore-hosts ['localhost', '192.168.0.0/16']",
                "gsettings set org.gnome.system.proxy mode 'manual'",
            ]
        );
        let bare = ProxySettings {
            socks: None,
            bypass: Vec::new(),
            ..settings()
        };
        let cmds = lines(&gnome_apply_commands(&bare));
        assert!(cmds.contains(&"gsettings set org.gnome.system.proxy.socks host ''".to_string()), "{cmds:?}");
        assert!(cmds.contains(&"gsettings set org.gnome.system.proxy.socks port 0".to_string()));
        assert!(cmds.contains(&"gsettings set org.gnome.system.proxy ignore-hosts @as []".to_string()));
        // the GVariant text is one argument, quotes escaped
        let quoted = ProxySettings {
            bypass: vec!["it's".into()],
            ..settings()
        };
        let ignore = &gnome_apply_commands(&quoted)[6];
        assert_eq!(ignore[4], r"['it\'s']");
    }

    fn gnome_runner() -> FakeRunner {
        let runner = FakeRunner::default();
        for (key, value) in [
            ("org.gnome.system.proxy mode", "'none'"),
            ("org.gnome.system.proxy ignore-hosts", "['localhost', '127.0.0.0/8', '::1']"),
            ("org.gnome.system.proxy.http host", "''"),
            ("org.gnome.system.proxy.http port", "8080"),
            ("org.gnome.system.proxy.https host", "''"),
            ("org.gnome.system.proxy.https port", "0"),
            ("org.gnome.system.proxy.socks host", "''"),
            ("org.gnome.system.proxy.socks port", "0"),
        ] {
            runner.reply(&format!("gsettings get {key}"), &format!("{value}\n"));
        }
        runner
    }

    #[test]
    fn gnome_snapshot_then_restore_writes_the_raw_values_back_with_mode_last() {
        let proxy = LinuxProxy::new(gnome_runner(), Desktop::Gnome, None);
        let backup = proxy.snapshot().unwrap();
        assert_eq!(backup.0["platform"], "linux");
        assert_eq!(backup.0["desktop"], "gnome");
        assert_eq!(backup.0["values"]["org.gnome.system.proxy.http port"], "8080");
        proxy.apply(&settings()).unwrap();
        proxy.runner.calls.lock().unwrap().clear();
        proxy.restore(&backup).unwrap();
        let calls = proxy.runner.calls();
        assert_eq!(calls.len(), 8);
        assert_eq!(calls[0], "gsettings set org.gnome.system.proxy ignore-hosts ['localhost', '127.0.0.0/8', '::1']");
        assert_eq!(calls[2], "gsettings set org.gnome.system.proxy.http port 8080");
        assert_eq!(calls[7], "gsettings set org.gnome.system.proxy mode 'none'", "the mode goes back last");
    }

    #[test]
    fn kde_apply_writes_kioslaverc_and_tells_kio() {
        let cmds = kde_apply_commands("kwriteconfig6", &settings());
        assert_eq!(cmds[0][4], "Proxy Settings", "the group is one argument");
        assert_eq!(
            lines(&cmds),
            [
                "kwriteconfig6 --file kioslaverc --group Proxy Settings --key httpProxy http://127.0.0.1 6152",
                "kwriteconfig6 --file kioslaverc --group Proxy Settings --key httpsProxy http://127.0.0.1 6152",
                "kwriteconfig6 --file kioslaverc --group Proxy Settings --key socksProxy socks://127.0.0.1 6153",
                "kwriteconfig6 --file kioslaverc --group Proxy Settings --key NoProxyFor localhost,192.168.0.0/16",
                "kwriteconfig6 --file kioslaverc --group Proxy Settings --key ProxyType 1",
            ]
        );
        let bare = ProxySettings {
            socks: None,
            bypass: Vec::new(),
            ..settings()
        };
        let cmds = lines(&kde_apply_commands("kwriteconfig5", &bare));
        assert_eq!(cmds[2], "kwriteconfig5 --file kioslaverc --group Proxy Settings --key socksProxy --delete");
        assert_eq!(cmds[3], "kwriteconfig5 --file kioslaverc --group Proxy Settings --key NoProxyFor --delete");
        let v6 = ProxySettings {
            http: Some("[::1]:6152".parse().unwrap()),
            ..settings()
        };
        assert_eq!(kde_apply_commands("kwriteconfig6", &v6)[0][7], "http://[::1] 6152");
    }

    #[test]
    fn kde_snapshot_then_restore_deletes_what_was_unset() {
        let runner = FakeRunner::default();
        runner.reply("kreadconfig6 --file kioslaverc --group Proxy Settings --key ProxyType", "0\n");
        runner.reply("kreadconfig6 --file kioslaverc --group Proxy Settings --key NoProxyFor", "localhost\n");
        let tools = Some(("kwriteconfig6".to_string(), "kreadconfig6".to_string()));
        let proxy = LinuxProxy::new(runner, Desktop::Kde, tools);
        let backup = proxy.snapshot().unwrap();
        assert_eq!(backup.0["desktop"], "kde");
        assert_eq!(backup.0["values"]["ProxyType"], "0");
        assert_eq!(backup.0["values"]["httpProxy"], "");
        proxy.apply(&settings()).unwrap();
        assert!(proxy.runner.calls().last().unwrap().starts_with("dbus-send "), "KIO is told to re-read its configuration");
        proxy.runner.calls.lock().unwrap().clear();
        proxy.restore(&backup).unwrap();
        let calls = proxy.runner.calls();
        assert!(calls.contains(&"kwriteconfig6 --file kioslaverc --group Proxy Settings --key ProxyType 0".to_string()), "{calls:?}");
        assert!(calls.contains(&"kwriteconfig6 --file kioslaverc --group Proxy Settings --key httpProxy --delete".to_string()));
        assert!(calls.contains(&"kwriteconfig6 --file kioslaverc --group Proxy Settings --key NoProxyFor localhost".to_string()));
        assert!(calls.last().unwrap().starts_with("dbus-send "));
    }

    #[test]
    fn a_failing_dbus_notification_does_not_fail_the_apply() {
        let runner = FakeRunner::default();
        runner.fail(&kio_reparse_cmd().join(" "), "no session bus");
        let tools = Some(("kwriteconfig6".to_string(), "kreadconfig6".to_string()));
        let proxy = LinuxProxy::new(runner, Desktop::Kde, tools);
        proxy.apply(&settings()).unwrap();
    }

    #[test]
    fn other_desktops_get_an_environment_hint_and_change_nothing() {
        let proxy = LinuxProxy::new(FakeRunner::default(), Desktop::Other, None);
        let backup = proxy.snapshot().unwrap();
        assert_eq!(backup.0["desktop"], "none");
        let err = proxy.apply(&settings()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
        let text = err.to_string();
        assert!(
            text.contains("export http_proxy=http://127.0.0.1:6152 https_proxy=http://127.0.0.1:6152 no_proxy=localhost,192.168.0.0/16"),
            "{text}"
        );
        proxy.restore(&backup).unwrap();
        assert!(proxy.runner.calls().is_empty(), "nothing was run");
    }

    #[test]
    fn restore_follows_the_backup_not_the_current_desktop() {
        // the backup was taken under GNOME; the session is something else now
        let gnome = LinuxProxy::new(gnome_runner(), Desktop::Gnome, None);
        let backup = gnome.snapshot().unwrap();
        let other = LinuxProxy::new(FakeRunner::default(), Desktop::Other, None);
        other.restore(&backup).unwrap();
        assert_eq!(other.runner.calls().len(), 8);
        let foreign = Backup(serde_json::json!({ "platform": "windows" }));
        assert_eq!(other.restore(&foreign).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p rurge-platform linux::`
Expected: 编译失败。

- [ ] **Step 3: 实现 `linux.rs`（测试模块之前）**

```rust
//! Linux: GNOME through `gsettings`, KDE through `kwriteconfig`; any other
//! desktop gets an environment-variable hint and nothing is changed.

use super::{Backup, ProxySettings, SystemProxy};
use crate::command::{Cmd, CommandRunner, run_all, run_best_effort};
use crate::dirs::EnvLookup;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;
use std::net::{IpAddr, SocketAddr};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Desktop {
    Gnome,
    Kde,
    Other,
}

const GNOME_ROOT: &str = "org.gnome.system.proxy";
/// (schema, key) pairs rurge touches, in snapshot order.
const GNOME_KEYS: [(&str, &str); 8] = [
    ("org.gnome.system.proxy", "mode"),
    ("org.gnome.system.proxy", "ignore-hosts"),
    ("org.gnome.system.proxy.http", "host"),
    ("org.gnome.system.proxy.http", "port"),
    ("org.gnome.system.proxy.https", "host"),
    ("org.gnome.system.proxy.https", "port"),
    ("org.gnome.system.proxy.socks", "host"),
    ("org.gnome.system.proxy.socks", "port"),
];
const KDE_FILE: &str = "kioslaverc";
const KDE_GROUP: &str = "Proxy Settings";
const KDE_KEYS: [&str; 5] = [
    "ProxyType",
    "httpProxy",
    "httpsProxy",
    "socksProxy",
    "NoProxyFor",
];

/// GNOME needs `gsettings` and `GNOME` in `XDG_CURRENT_DESKTOP`; KDE needs
/// `KDE` there and a `kwriteconfig` / `kreadconfig` pair.
pub fn detect(env: EnvLookup<'_>, has_tool: &dyn Fn(&str) -> bool) -> Desktop {
    let desktop = env("XDG_CURRENT_DESKTOP")
        .map(|v| v.to_string_lossy().to_ascii_uppercase())
        .unwrap_or_default();
    if desktop.contains("GNOME") && has_tool("gsettings") {
        Desktop::Gnome
    } else if desktop.contains("KDE") && kde_tools(has_tool).is_some() {
        Desktop::Kde
    } else {
        Desktop::Other
    }
}

/// (write tool, read tool): Plasma 6 first, then Plasma 5.
pub fn kde_tools(has_tool: &dyn Fn(&str) -> bool) -> Option<(String, String)> {
    [
        ("kwriteconfig6", "kreadconfig6"),
        ("kwriteconfig5", "kreadconfig5"),
    ]
    .into_iter()
    .find(|(write, read)| has_tool(write) && has_tool(read))
    .map(|(write, read)| (write.to_string(), read.to_string()))
}

pub fn tool_on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
}

fn gsettings(args: &[&str]) -> Cmd {
    std::iter::once("gsettings")
        .chain(args.iter().copied())
        .map(str::to_string)
        .collect()
}

/// A GVariant string literal.
fn gv_str(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// A GVariant string array; the empty one needs its type spelled out.
fn gv_strv(items: &[String]) -> String {
    if items.is_empty() {
        return "@as []".to_string();
    }
    let quoted: Vec<String> = items.iter().map(|s| gv_str(s)).collect();
    format!("[{}]", quoted.join(", "))
}

pub fn gnome_apply_commands(settings: &ProxySettings) -> Vec<Cmd> {
    let mut out = Vec::new();
    for (kind, addr) in [
        ("http", settings.http),
        ("https", settings.https),
        ("socks", settings.socks),
    ] {
        let schema = format!("{GNOME_ROOT}.{kind}");
        let (host, port) = match addr {
            Some(addr) => (addr.ip().to_string(), addr.port()),
            None => (String::new(), 0),
        };
        out.push(gsettings(&["set", &schema, "host", &gv_str(&host)]));
        out.push(gsettings(&["set", &schema, "port", &port.to_string()]));
    }
    out.push(gsettings(&[
        "set",
        GNOME_ROOT,
        "ignore-hosts",
        &gv_strv(&settings.bypass),
    ]));
    // switched to manual last, once the addresses are in place
    out.push(gsettings(&["set", GNOME_ROOT, "mode", "'manual'"]));
    out
}

fn kwrite(tool: &str, key: &str, value: Option<&str>) -> Cmd {
    let mut c: Cmd = [tool, "--file", KDE_FILE, "--group", KDE_GROUP, "--key", key]
        .iter()
        .map(|s| s.to_string())
        .collect();
    c.push(value.unwrap_or("--delete").to_string());
    c
}

fn kread(tool: &str, key: &str) -> Cmd {
    [tool, "--file", KDE_FILE, "--group", KDE_GROUP, "--key", key]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// `kioslaverc` keeps a proxy as `<scheme>://<host> <port>`.
fn kde_url(scheme: &str, addr: SocketAddr) -> String {
    let host = match addr.ip() {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    };
    format!("{scheme}://{host} {}", addr.port())
}

pub fn kde_apply_commands(write_tool: &str, settings: &ProxySettings) -> Vec<Cmd> {
    let http = settings.http.map(|a| kde_url("http", a));
    let https = settings.https.map(|a| kde_url("http", a));
    let socks = settings.socks.map(|a| kde_url("socks", a));
    let bypass = (!settings.bypass.is_empty()).then(|| settings.bypass.join(","));
    vec![
        kwrite(write_tool, "httpProxy", http.as_deref()),
        kwrite(write_tool, "httpsProxy", https.as_deref()),
        kwrite(write_tool, "socksProxy", socks.as_deref()),
        kwrite(write_tool, "NoProxyFor", bypass.as_deref()),
        kwrite(write_tool, "ProxyType", Some("1")),
    ]
}

/// Asks running KIO clients to re-read `kioslaverc`.
fn kio_reparse_cmd() -> Cmd {
    [
        "dbus-send",
        "--type=signal",
        "/KIO/Scheduler",
        "org.kde.KIO.Scheduler.reparseSlaveConfiguration",
        "string:",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// What a shell user can export when no desktop setting exists.
pub fn env_hint(settings: &ProxySettings) -> String {
    let mut vars = Vec::new();
    if let Some(addr) = settings.http {
        vars.push(format!("http_proxy=http://{addr}"));
    }
    if let Some(addr) = settings.https {
        vars.push(format!("https_proxy=http://{addr}"));
    }
    if !settings.bypass.is_empty() {
        vars.push(format!("no_proxy={}", settings.bypass.join(",")));
    }
    format!("export {}", vars.join(" "))
}

#[derive(Serialize, Deserialize)]
struct LinuxBackup {
    platform: String,
    desktop: String,
    #[serde(default)]
    values: BTreeMap<String, String>,
}

pub struct LinuxProxy<R: CommandRunner> {
    runner: R,
    desktop: Desktop,
    kde: Option<(String, String)>,
}

impl<R: CommandRunner> LinuxProxy<R> {
    pub fn new(runner: R, desktop: Desktop, kde: Option<(String, String)>) -> Self {
        LinuxProxy {
            runner,
            desktop,
            kde,
        }
    }

    /// Looks at this process's environment and `PATH`.
    pub fn detect(runner: R) -> Self {
        let has_tool = |name: &str| tool_on_path(name);
        LinuxProxy::new(
            runner,
            detect(&|key| std::env::var_os(key), &has_tool),
            kde_tools(&has_tool),
        )
    }

    fn kde(&self) -> io::Result<&(String, String)> {
        self.kde.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "kwriteconfig6 / kwriteconfig5 not found",
            )
        })
    }

    fn notify_kio(&self) {
        if let Err(e) = self.runner.run(&kio_reparse_cmd()) {
            tracing::debug!(error = %e, "cannot tell KIO to re-read its proxy settings");
        }
    }
}

impl<R: CommandRunner> SystemProxy for LinuxProxy<R> {
    fn snapshot(&self) -> io::Result<Backup> {
        let mut values = BTreeMap::new();
        let desktop = match self.desktop {
            Desktop::Gnome => {
                for (schema, key) in GNOME_KEYS {
                    let out = self.runner.run(&gsettings(&["get", schema, key]))?;
                    values.insert(format!("{schema} {key}"), out.trim().to_string());
                }
                "gnome"
            }
            Desktop::Kde => {
                let (_, read) = self.kde()?;
                for key in KDE_KEYS {
                    let out = self.runner.run(&kread(read, key))?;
                    values.insert(key.to_string(), out.trim().to_string());
                }
                "kde"
            }
            Desktop::Other => "none",
        };
        let backup = LinuxBackup {
            platform: "linux".to_string(),
            desktop: desktop.to_string(),
            values,
        };
        Ok(Backup(
            serde_json::to_value(backup).expect("backup serializes"),
        ))
    }

    fn apply(&self, settings: &ProxySettings) -> io::Result<()> {
        match self.desktop {
            Desktop::Gnome => run_all(&self.runner, &gnome_apply_commands(settings)),
            Desktop::Kde => {
                let (write, _) = self.kde()?;
                run_all(&self.runner, &kde_apply_commands(write, settings))?;
                self.notify_kio();
                Ok(())
            }
            Desktop::Other => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "no supported desktop proxy settings (GNOME or KDE) were found; set the proxy in your shell instead:\n  {}",
                    env_hint(settings)
                ),
            )),
        }
    }

    /// Follows the desktop recorded in the backup, not the current session.
    fn restore(&self, backup: &Backup) -> io::Result<()> {
        let saved: LinuxBackup = serde_json::from_value(backup.0.clone())
            .map_err(|_| super::wrong_platform("linux"))?;
        if saved.platform != "linux" {
            return Err(super::wrong_platform("linux"));
        }
        match saved.desktop.as_str() {
            "gnome" => {
                // everything but the mode first, the mode last
                let ordered = GNOME_KEYS
                    .iter()
                    .filter(|(_, key)| *key != "mode")
                    .chain(GNOME_KEYS.iter().filter(|(_, key)| *key == "mode"));
                let cmds: Vec<Cmd> = ordered
                    .filter_map(|&(schema, key)| {
                        let value = saved.values.get(&format!("{schema} {key}"))?;
                        (!value.is_empty())
                            .then(|| gsettings(&["set", schema, key, value.as_str()]))
                    })
                    .collect();
                run_best_effort(&self.runner, &cmds)
            }
            "kde" => {
                let (write, _) = self.kde()?;
                let cmds: Vec<Cmd> = KDE_KEYS
                    .iter()
                    .filter_map(|key| {
                        let value = saved.values.get(*key)?;
                        Some(kwrite(write, key, (!value.is_empty()).then_some(value.as_str())))
                    })
                    .collect();
                let result = run_best_effort(&self.runner, &cmds);
                self.notify_kio();
                result
            }
            "none" => Ok(()),
            _ => Err(super::wrong_platform("linux")),
        }
    }
}
```

`sysproxy/mod.rs` 追加 `pub mod linux;` 与：

```rust
/// The backend for the operating system rurge was built for.
pub fn platform() -> Box<dyn SystemProxy> {
    #[cfg(windows)]
    {
        Box::new(windows::WindowsProxy::new(
            windows::RealRegistry::internet_settings(),
        ))
    }
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MacosProxy::new(crate::command::SystemRunner))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Box::new(linux::LinuxProxy::detect(crate::command::SystemRunner))
    }
}
```

（测试 `gnome_snapshot_then_restore…` 里 `calls[0]` 是 `ignore-hosts`：`GNOME_KEYS` 去掉 `mode` 后第一项就是它；`calls[2]` 是 `http port`；`mode` 最后。）

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p rurge-platform`
Expected: `linux::` 的 8 个测试与此前的全部 PASS。

- [ ] **Step 5: 质量门与提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-platform
git commit -F - <<'EOF'
feat(platform): Linux 系统代理后端（GNOME / KDE / 环境变量提示）与 platform()

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th
EOF
```

---

### Task 4: `rurge_platform::service`——安装 / 卸载计划与执行器

**Files:**
- Create: `crates/rurge-platform/src/service.rs`
- Modify: `crates/rurge-platform/src/lib.rs`（`pub mod service;`）、`crates/rurge-platform/src/dirs.rs`（`fn home` → `pub(crate) fn home`）、`crates/rurge-platform/Cargo.toml`（`[dev-dependencies] tempfile.workspace = true`）

**Interfaces:**
- Consumes: `dirs::{Os, EnvLookup, home}`；Task 2 的 `command::{Cmd, CommandRunner}`。
- Produces:
  - `service::{SYSTEMD_UNIT = "rurge.service", LAUNCHD_LABEL = "io.rurge.daemon", TASK_NAME = "rurge", Scope::{User, System}, ServiceSpec { exe: PathBuf, config: PathBuf, system_proxy: bool }, Plan { files: Vec<(PathBuf, String)>, commands: Vec<Cmd>, remove: Vec<PathBuf> }}`（`Plan: Clone + Debug + Default + PartialEq + Eq`）。
  - `install_plan(os: Os, scope: Scope, spec: &ServiceSpec, env: EnvLookup<'_>, uid: Option<u32>) -> io::Result<Plan>`、`uninstall_plan(os, scope, env, uid) -> io::Result<Plan>`。macOS 的用户级计划需要 `uid`（`launchctl bootstrap gui/<uid>`），缺失 → `ErrorKind::InvalidInput`。Windows 忽略 `scope`（登录任务总是当前用户的）。
  - `execute(plan: &Plan, runner: &dyn CommandRunner) -> io::Result<()>`：写文件（建父目录）→ 执行命令 → 删文件。安装计划（`remove` 为空）遇到命令失败立即返回；卸载计划继续把文件删掉，最后返回第一个错误。

- [ ] **Step 1: 写失败测试**

`crates/rurge-platform/src/service.rs` 先放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::testing::FakeRunner;
    use std::ffi::OsString;

    fn spec(system_proxy: bool) -> ServiceSpec {
        ServiceSpec {
            exe: PathBuf::from("/usr/local/bin/rurge"),
            config: PathBuf::from("/home/me/my profiles/rurge.conf"),
            system_proxy,
        }
    }

    fn home_env(key: &str) -> Option<OsString> {
        (key == "HOME").then(|| OsString::from("/home/me"))
    }

    fn lines(cmds: &[Cmd]) -> Vec<String> {
        cmds.iter().map(|c| c.join(" ")).collect()
    }

    #[test]
    fn systemd_user_unit() {
        let plan = install_plan(Os::Unix, Scope::User, &spec(true), &home_env, None).unwrap();
        let unit = PathBuf::from("/home/me")
            .join(".config")
            .join("systemd")
            .join("user")
            .join(SYSTEMD_UNIT);
        assert_eq!(plan.files.len(), 1);
        assert_eq!(plan.files[0].0, unit);
        let text = &plan.files[0].1;
        assert!(
            text.contains("ExecStart=\"/usr/local/bin/rurge\" run -c \"/home/me/my profiles/rurge.conf\" --system-proxy\n"),
            "{text}"
        );
        assert!(text.contains("Restart=on-failure\n") && text.contains("WantedBy=default.target\n"), "{text}");
        assert_eq!(lines(&plan.commands), ["systemctl --user enable --now rurge"]);
        assert!(plan.remove.is_empty());
        let removal = uninstall_plan(Os::Unix, Scope::User, &home_env, None).unwrap();
        assert_eq!(lines(&removal.commands), ["systemctl --user disable --now rurge"]);
        assert_eq!(removal.remove, [unit]);
        assert!(removal.files.is_empty());
    }

    #[test]
    fn systemd_system_unit() {
        let plan = install_plan(Os::Unix, Scope::System, &spec(false), &home_env, None).unwrap();
        assert_eq!(plan.files[0].0, PathBuf::from("/etc/systemd/system").join(SYSTEMD_UNIT));
        let text = &plan.files[0].1;
        assert!(text.contains("WantedBy=multi-user.target\n"), "{text}");
        assert!(!text.contains("--system-proxy"));
        assert_eq!(lines(&plan.commands), ["systemctl enable --now rurge"]);
    }

    #[test]
    fn systemd_quoting_escapes_specifiers_and_quotes() {
        assert_eq!(systemd_quote(Path::new("/opt/100%/ru\"rge$")), "\"/opt/100%%/ru\\\"rge$$\"");
    }

    #[test]
    fn launchd_agent_and_daemon() {
        let plan = install_plan(Os::MacOs, Scope::User, &spec(true), &home_env, Some(501)).unwrap();
        let plist = PathBuf::from("/home/me")
            .join("Library")
            .join("LaunchAgents")
            .join("io.rurge.daemon.plist");
        assert_eq!(plan.files[0].0, plist);
        let text = &plan.files[0].1;
        assert!(text.contains("<key>Label</key>\n    <string>io.rurge.daemon</string>"), "{text}");
        assert!(text.contains("<string>/home/me/my profiles/rurge.conf</string>"), "{text}");
        assert!(text.contains("<string>--system-proxy</string>"));
        assert!(text.contains("<key>RunAtLoad</key>\n    <true/>") && text.contains("<key>SuccessfulExit</key>"), "{text}");
        assert_eq!(plan.commands, [vec![
            "launchctl".to_string(),
            "bootstrap".to_string(),
            "gui/501".to_string(),
            plist.display().to_string(),
        ]]);
        let err = install_plan(Os::MacOs, Scope::User, &spec(false), &home_env, None).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        let daemon = install_plan(Os::MacOs, Scope::System, &spec(false), &home_env, None).unwrap();
        assert_eq!(daemon.files[0].0, PathBuf::from("/Library/LaunchDaemons").join("io.rurge.daemon.plist"));
        assert_eq!(daemon.commands[0][..3], ["launchctl", "bootstrap", "system"]);
        let removal = uninstall_plan(Os::MacOs, Scope::User, &home_env, Some(501)).unwrap();
        assert_eq!(removal.commands[0][..3], ["launchctl", "bootout", "gui/501"]);
        assert_eq!(removal.remove, [plist]);
    }

    #[test]
    fn plist_strings_are_xml_escaped() {
        let odd = ServiceSpec {
            config: PathBuf::from("/tmp/a&b<c>.conf"),
            ..spec(false)
        };
        let plan = install_plan(Os::MacOs, Scope::System, &odd, &home_env, None).unwrap();
        assert!(plan.files[0].1.contains("<string>/tmp/a&amp;b&lt;c&gt;.conf</string>"));
    }

    #[test]
    fn windows_logon_task() {
        let spec = ServiceSpec {
            exe: PathBuf::from(r"C:\Program Files\rurge\rurge.exe"),
            config: PathBuf::from(r"C:\Users\me\rurge.conf"),
            system_proxy: true,
        };
        let plan = install_plan(Os::Windows, Scope::User, &spec, &home_env, None).unwrap();
        assert!(plan.files.is_empty());
        assert_eq!(
            plan.commands,
            [vec![
                "schtasks", "/create", "/tn", "rurge", "/sc", "onlogon", "/tr",
                r#""C:\Program Files\rurge\rurge.exe" run -c "C:\Users\me\rurge.conf" --system-proxy"#,
                "/f",
            ]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()]
        );
        let same = install_plan(Os::Windows, Scope::System, &spec, &home_env, None).unwrap();
        assert_eq!(same, plan, "the scope makes no difference on Windows");
        let removal = uninstall_plan(Os::Windows, Scope::User, &home_env, None).unwrap();
        assert_eq!(lines(&removal.commands), ["schtasks /delete /tn rurge /f"]);
    }

    #[test]
    fn execute_writes_runs_and_removes() {
        let dir = tempfile::tempdir().unwrap();
        let unit = dir.path().join("nested").join("rurge.service");
        let install = Plan {
            files: vec![(unit.clone(), "unit text".to_string())],
            commands: vec![vec!["systemctl".to_string(), "enable".to_string()]],
            remove: Vec::new(),
        };
        let runner = FakeRunner::default();
        execute(&install, &runner).unwrap();
        assert_eq!(std::fs::read_to_string(&unit).unwrap(), "unit text");
        assert_eq!(runner.calls(), ["systemctl enable"]);
        // an install stops at a failing command
        let failing = FakeRunner::default();
        failing.fail("systemctl enable", "access denied");
        let err = execute(&install, &failing).unwrap_err();
        assert!(err.to_string().contains("access denied"));
        // an uninstall still removes its files when the command fails
        let uninstall = Plan {
            files: Vec::new(),
            commands: vec![vec!["systemctl".to_string(), "disable".to_string()]],
            remove: vec![unit.clone(), dir.path().join("never-existed")],
        };
        let failing = FakeRunner::default();
        failing.fail("systemctl disable", "not loaded");
        let err = execute(&uninstall, &failing).unwrap_err();
        assert!(err.to_string().contains("not loaded"));
        assert!(!unit.exists(), "the unit is gone although the command failed");
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p rurge-platform service::`
Expected: 编译失败。

- [ ] **Step 3: 实现（测试模块之前）**

`dirs.rs`：`fn home(` → `pub(crate) fn home(`。`Cargo.toml` 加：

```toml
[dev-dependencies]
tempfile.workspace = true
```

`service.rs`：

```rust
//! `rurge service install | uninstall` (M4 design §8.3): the unit / plist /
//! logon task that starts rurge, and the commands that register it. Plans
//! take an explicit `Os`, so every platform is tested on every host; only
//! `execute` touches the system.

use crate::command::{Cmd, CommandRunner};
use crate::dirs::{EnvLookup, Os, home};
use std::io;
use std::path::{Path, PathBuf};

pub const SYSTEMD_UNIT: &str = "rurge.service";
pub const LAUNCHD_LABEL: &str = "io.rurge.daemon";
pub const TASK_NAME: &str = "rurge";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    User,
    System,
}

#[derive(Clone, Debug)]
pub struct ServiceSpec {
    /// Absolute path of the rurge binary.
    pub exe: PathBuf,
    /// Absolute path of the profile.
    pub config: PathBuf,
    pub system_proxy: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub files: Vec<(PathBuf, String)>,
    pub commands: Vec<Cmd>,
    pub remove: Vec<PathBuf>,
}

fn cmd(args: &[&str]) -> Cmd {
    args.iter().map(|s| s.to_string()).collect()
}

fn systemd_unit_path(scope: Scope, env: EnvLookup<'_>) -> PathBuf {
    match scope {
        Scope::User => home(env)
            .join(".config")
            .join("systemd")
            .join("user")
            .join(SYSTEMD_UNIT),
        Scope::System => PathBuf::from("/etc/systemd/system").join(SYSTEMD_UNIT),
    }
}

fn systemctl(scope: Scope, verb: &str) -> Cmd {
    match scope {
        Scope::User => cmd(&["systemctl", "--user", verb, "--now", "rurge"]),
        Scope::System => cmd(&["systemctl", verb, "--now", "rurge"]),
    }
}

/// Double-quoted for `ExecStart=`: `\` and `"` escaped, `%` and `$` doubled
/// (systemd expands specifiers and variables there).
fn systemd_quote(path: &Path) -> String {
    let escaped = path
        .display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%")
        .replace('$', "$$");
    format!("\"{escaped}\"")
}

fn systemd_unit(spec: &ServiceSpec, scope: Scope) -> String {
    let mut exec = format!(
        "{} run -c {}",
        systemd_quote(&spec.exe),
        systemd_quote(&spec.config)
    );
    if spec.system_proxy {
        exec.push_str(" --system-proxy");
    }
    let target = match scope {
        Scope::User => "default.target",
        Scope::System => "multi-user.target",
    };
    format!(
        "[Unit]\nDescription=rurge proxy\nAfter=network-online.target\nWants=network-online.target\n\n\
[Service]\nExecStart={exec}\nRestart=on-failure\nRestartSec=3\n\n\
[Install]\nWantedBy={target}\n"
    )
}

fn launchd_plist_path(scope: Scope, env: EnvLookup<'_>) -> PathBuf {
    let file = format!("{LAUNCHD_LABEL}.plist");
    match scope {
        Scope::User => home(env).join("Library").join("LaunchAgents").join(file),
        Scope::System => PathBuf::from("/Library/LaunchDaemons").join(file),
    }
}

fn launchd_domain(scope: Scope, uid: Option<u32>) -> io::Result<String> {
    match (scope, uid) {
        (Scope::System, _) => Ok("system".to_string()),
        (Scope::User, Some(uid)) => Ok(format!("gui/{uid}")),
        (Scope::User, None) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot determine the user id for the launchd domain",
        )),
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn launchd_plist(spec: &ServiceSpec) -> String {
    let mut args = vec![
        spec.exe.display().to_string(),
        "run".to_string(),
        "-c".to_string(),
        spec.config.display().to_string(),
    ];
    if spec.system_proxy {
        args.push("--system-proxy".to_string());
    }
    let args: String = args
        .iter()
        .map(|a| format!("        <string>{}</string>\n", xml_escape(a)))
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n<dict>\n    <key>Label</key>\n    <string>{LAUNCHD_LABEL}</string>\n    <key>ProgramArguments</key>\n    <array>\n{args}    </array>\n    <key>RunAtLoad</key>\n    <true/>\n    <key>KeepAlive</key>\n    <dict>\n        <key>SuccessfulExit</key>\n        <false/>\n    </dict>\n</dict>\n</plist>\n"
    )
}

/// The command line the logon task runs.
fn task_command(spec: &ServiceSpec) -> String {
    let mut line = format!(
        "\"{}\" run -c \"{}\"",
        spec.exe.display(),
        spec.config.display()
    );
    if spec.system_proxy {
        line.push_str(" --system-proxy");
    }
    line
}

pub fn install_plan(
    os: Os,
    scope: Scope,
    spec: &ServiceSpec,
    env: EnvLookup<'_>,
    uid: Option<u32>,
) -> io::Result<Plan> {
    Ok(match os {
        Os::Unix => Plan {
            files: vec![(systemd_unit_path(scope, env), systemd_unit(spec, scope))],
            commands: vec![systemctl(scope, "enable")],
            remove: Vec::new(),
        },
        Os::MacOs => {
            let plist = launchd_plist_path(scope, env);
            let domain = launchd_domain(scope, uid)?;
            Plan {
                commands: vec![cmd(&[
                    "launchctl",
                    "bootstrap",
                    &domain,
                    &plist.display().to_string(),
                ])],
                files: vec![(plist, launchd_plist(spec))],
                remove: Vec::new(),
            }
        }
        // `/f`: without it schtasks asks before replacing an existing task
        Os::Windows => Plan {
            files: Vec::new(),
            commands: vec![cmd(&[
                "schtasks",
                "/create",
                "/tn",
                TASK_NAME,
                "/sc",
                "onlogon",
                "/tr",
                &task_command(spec),
                "/f",
            ])],
            remove: Vec::new(),
        },
    })
}

pub fn uninstall_plan(
    os: Os,
    scope: Scope,
    env: EnvLookup<'_>,
    uid: Option<u32>,
) -> io::Result<Plan> {
    Ok(match os {
        Os::Unix => Plan {
            files: Vec::new(),
            commands: vec![systemctl(scope, "disable")],
            remove: vec![systemd_unit_path(scope, env)],
        },
        Os::MacOs => {
            let plist = launchd_plist_path(scope, env);
            let domain = launchd_domain(scope, uid)?;
            Plan {
                files: Vec::new(),
                commands: vec![cmd(&[
                    "launchctl",
                    "bootout",
                    &domain,
                    &plist.display().to_string(),
                ])],
                remove: vec![plist],
            }
        }
        Os::Windows => Plan {
            files: Vec::new(),
            commands: vec![cmd(&["schtasks", "/delete", "/tn", TASK_NAME, "/f"])],
            remove: Vec::new(),
        },
    })
}

/// Writes the files, runs the commands, removes what is to be removed. An
/// install stops at the first failing command; an uninstall carries on so its
/// files still go away, and returns the first error.
pub fn execute(plan: &Plan, runner: &dyn CommandRunner) -> io::Result<()> {
    for (path, content) in &plan.files {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, content)
            .map_err(|e| io::Error::new(e.kind(), format!("cannot write {}: {e}", path.display())))?;
    }
    let mut first_error = None;
    for command in &plan.commands {
        if let Err(e) = runner.run(command) {
            if plan.remove.is_empty() {
                return Err(e);
            }
            first_error.get_or_insert(e);
        }
    }
    for path in &plan.remove {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                first_error.get_or_insert(io::Error::new(
                    e.kind(),
                    format!("cannot remove {}: {e}", path.display()),
                ));
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}
```

`lib.rs` 加 `pub mod service;`（字母序：`command`、`dirs`、`dns`、`service`、`sysproxy`）。

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p rurge-platform`
Expected: `service::` 的 7 个测试与此前的全部 PASS。（在 Windows 主机上跑 Unix / macOS 的计划时，`PathBuf::join` 用的是 `\`；断言比较的是 `PathBuf` 或同样经 `display()` 得到的字符串，所以与主机无关。若某条断言因分隔符失败，改成比较 `PathBuf` 而不是硬编码字符串。）

- [ ] **Step 5: 质量门与提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-platform Cargo.lock
git commit -F - <<'EOF'
feat(platform): service 安装 / 卸载计划（systemd / launchd / schtasks）与执行器

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th
EOF
```

---

### Task 5: bin 侧的 `SystemProxyManager`、`proxy_settings` 与文件后端

**Files:**
- Create: `crates/rurge/src/cli/sysproxy.rs`
- Modify: `crates/rurge/src/cli/mod.rs`（`pub mod sysproxy;`）、`crates/rurge/src/cli/control.rs`（`fn connect_addr` → `pub(crate) fn connect_addr`）

**Interfaces:**
- Consumes: Task 1 / 3 的 `rurge_platform::sysproxy::{Backup, ProxySettings, SystemProxy, platform}`；`rurge_engine::state::StateStore`；`rurge_config::{General, HostList, HostPattern, ListenerKind}`；`cli::control::connect_addr`。
- Produces（bin 内部，Task 8 使用）：
  - `cli::sysproxy::proxy_settings(&General, &[(ListenerKind, SocketAddr)]) -> Option<ProxySettings>`：第一个 HTTP 监听 → http 与 https；`set_system_socks_proxy` 为真时第一个 SOCKS5 监听 → socks；两者皆无 → `None`；通配地址换同族回环；`bypass` 来自 `skip_proxy`（丢弃取反项、`<…>` 记号，去掉端口，CIDR 规整为网络地址，单个 IP 不带 `/32`，去重）。
  - `describe(&ProxySettings) -> String`（`http 127.0.0.1:6152, socks 127.0.0.1:6153`）。
  - `BACKEND_ENV = "RURGE_SYSTEM_PROXY_BACKEND"`、`backend() -> anyhow::Result<Arc<dyn SystemProxy>>`（未设 → `platform()`；`file:<path>` → `FileBackend`；其它 → 错误）。
  - `FileBackend`：`apply` 写 `{"http","https","socks","bypass","exclude_simple"}`（地址是字符串或 `null`）；`snapshot` 记下文件原文（不存在 → `null`）；`restore` 写回原文或删除文件。
  - `SystemProxyManager::{new(Arc<dyn SystemProxy>, Arc<StateStore>), flag() -> Arc<AtomicBool>, enabled() -> bool, applied() -> Option<&ProxySettings>, recover().await, enable(ProxySettings).await -> Result<(), String>, disable().await -> Result<(), String>, refresh(Option<ProxySettings>).await -> Result<(), String>}`。

- [ ] **Step 1: 写失败测试**

新建 `crates/rurge/src/cli/sysproxy.rs`，先放测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::HostList;
    use serde_json::json;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MockProxy {
        calls: Mutex<Vec<String>>,
        restored: Mutex<Vec<Backup>>,
        fail_apply: AtomicBool,
        fail_restore: AtomicBool,
    }

    impl MockProxy {
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl SystemProxy for MockProxy {
        fn snapshot(&self) -> io::Result<Backup> {
            self.calls.lock().unwrap().push("snapshot".to_string());
            Ok(Backup(json!({ "platform": "mock", "original": true })))
        }
        fn apply(&self, settings: &ProxySettings) -> io::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("apply {}", describe(settings)));
            if self.fail_apply.load(Ordering::SeqCst) {
                return Err(io::Error::other("apply refused"));
            }
            Ok(())
        }
        fn restore(&self, backup: &Backup) -> io::Result<()> {
            self.calls.lock().unwrap().push("restore".to_string());
            if self.fail_restore.load(Ordering::SeqCst) {
                return Err(io::Error::other("restore refused"));
            }
            self.restored.lock().unwrap().push(backup.clone());
            Ok(())
        }
    }

    fn settings(http_port: u16) -> ProxySettings {
        ProxySettings {
            http: Some(SocketAddr::from(([127, 0, 0, 1], http_port))),
            https: Some(SocketAddr::from(([127, 0, 0, 1], http_port))),
            socks: Some(SocketAddr::from(([127, 0, 0, 1], 6153))),
            bypass: vec!["localhost".to_string()],
            exclude_simple: false,
        }
    }

    async fn manager() -> (
        tempfile::TempDir,
        Arc<MockProxy>,
        Arc<StateStore>,
        SystemProxyManager,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = StateStore::open(dir.path().join("state.json")).await;
        let mock = Arc::new(MockProxy::default());
        let manager = SystemProxyManager::new(mock.clone(), store.clone());
        (dir, mock, store, manager)
    }

    #[tokio::test]
    async fn enable_saves_the_original_before_applying_and_disable_puts_it_back() {
        let (_dir, mock, store, mut manager) = manager().await;
        let flag = manager.flag();
        manager.enable(settings(6152)).await.unwrap();
        assert_eq!(
            mock.calls(),
            ["snapshot", "apply http 127.0.0.1:6152, socks 127.0.0.1:6153"]
        );
        let state = store.snapshot().await;
        assert_eq!(
            state.system_proxy_backup,
            Some(json!({ "platform": "mock", "original": true }))
        );
        assert!(state.features.system_proxy && manager.enabled() && flag.load(Ordering::SeqCst));
        manager.disable().await.unwrap();
        assert_eq!(
            *mock.restored.lock().unwrap(),
            [Backup(json!({ "platform": "mock", "original": true }))]
        );
        let state = store.snapshot().await;
        assert_eq!(state.system_proxy_backup, None);
        assert!(!state.features.system_proxy && !manager.enabled() && !flag.load(Ordering::SeqCst));
        manager.disable().await.unwrap();
        assert_eq!(mock.calls().len(), 3, "disabling twice touches nothing");
    }

    #[tokio::test]
    async fn changed_settings_are_applied_without_a_second_snapshot() {
        let (_dir, mock, store, mut manager) = manager().await;
        manager.enable(settings(6152)).await.unwrap();
        manager.enable(settings(6152)).await.unwrap();
        assert_eq!(mock.calls().len(), 2, "the same settings are not applied twice");
        manager.enable(settings(7000)).await.unwrap();
        assert_eq!(
            mock.calls(),
            [
                "snapshot",
                "apply http 127.0.0.1:6152, socks 127.0.0.1:6153",
                "apply http 127.0.0.1:7000, socks 127.0.0.1:6153",
            ],
            "a second snapshot would save rurge's own settings as the original"
        );
        assert_eq!(
            store.snapshot().await.system_proxy_backup,
            Some(json!({ "platform": "mock", "original": true }))
        );
    }

    #[tokio::test]
    async fn a_failed_apply_is_rolled_back() {
        let (_dir, mock, store, mut manager) = manager().await;
        mock.fail_apply.store(true, Ordering::SeqCst);
        let err = manager.enable(settings(6152)).await.unwrap_err();
        assert!(err.contains("apply refused"), "{err}");
        assert_eq!(mock.calls(), ["snapshot", "apply http 127.0.0.1:6152, socks 127.0.0.1:6153", "restore"]);
        let state = store.snapshot().await;
        assert_eq!(state.system_proxy_backup, None);
        assert!(!state.features.system_proxy && !manager.enabled() && !manager.flag().load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn recover_restores_what_a_crashed_run_left_behind() {
        let (_dir, mock, store, mut manager) = manager().await;
        manager.recover().await;
        assert!(mock.calls().is_empty(), "nothing to recover on a clean state");
        let left_behind = json!({ "platform": "mock", "from": "crashed run" });
        let saved = left_behind.clone();
        store
            .update(move |s| {
                s.system_proxy_backup = Some(saved);
                s.features.system_proxy = true;
            })
            .await;
        manager.recover().await;
        assert_eq!(*mock.restored.lock().unwrap(), [Backup(left_behind)]);
        let state = store.snapshot().await;
        assert_eq!(state.system_proxy_backup, None);
        assert!(!state.features.system_proxy);
    }

    #[tokio::test]
    async fn a_failed_recovery_keeps_the_original_for_later() {
        let (_dir, mock, store, mut manager) = manager().await;
        let left_behind = json!({ "platform": "mock", "from": "crashed run" });
        let saved = left_behind.clone();
        store.update(move |s| s.system_proxy_backup = Some(saved)).await;
        mock.fail_restore.store(true, Ordering::SeqCst);
        manager.recover().await;
        assert_eq!(
            store.snapshot().await.system_proxy_backup,
            Some(left_behind.clone()),
            "the backup stays for the next attempt"
        );
        // enabling now must not snapshot rurge's stale settings over the original
        manager.enable(settings(6152)).await.unwrap();
        assert!(!mock.calls().contains(&"snapshot".to_string()), "{:?}", mock.calls());
        mock.fail_restore.store(false, Ordering::SeqCst);
        manager.disable().await.unwrap();
        assert_eq!(*mock.restored.lock().unwrap(), [Backup(left_behind)]);
    }

    #[tokio::test]
    async fn refresh_follows_changes_only_while_enabled() {
        let (_dir, mock, _store, mut manager) = manager().await;
        manager.refresh(Some(settings(6152))).await.unwrap();
        assert!(mock.calls().is_empty(), "a disabled proxy is not switched on by a reload");
        manager.enable(settings(6152)).await.unwrap();
        manager.refresh(Some(settings(6152))).await.unwrap();
        manager.refresh(None).await.unwrap();
        assert_eq!(mock.calls().len(), 2, "unchanged or missing settings change nothing");
        manager.refresh(Some(settings(7000))).await.unwrap();
        assert_eq!(mock.calls().last().unwrap(), "apply http 127.0.0.1:7000, socks 127.0.0.1:6153");
    }

    #[test]
    fn settings_come_from_the_first_listeners_and_skip_proxy() {
        let general = General {
            skip_proxy: HostList::parse(
                "localhost, *.local, 192.168.1.5/16, 127.0.0.1, -internal.example, <ip-address>, www.example.com:8080, localhost",
                None,
            ),
            exclude_simple_hostnames: true,
            ..General::default()
        };
        let listeners = [
            (ListenerKind::Socks5, "0.0.0.0:6153".parse().unwrap()),
            (ListenerKind::Http, "0.0.0.0:6152".parse().unwrap()),
            (ListenerKind::Http, "127.0.0.1:7000".parse().unwrap()),
        ];
        let s = proxy_settings(&general, &listeners).unwrap();
        assert_eq!(s.http, Some("127.0.0.1:6152".parse().unwrap()));
        assert_eq!(s.https, s.http);
        assert_eq!(s.socks, Some("127.0.0.1:6153".parse().unwrap()));
        assert_eq!(
            s.bypass,
            ["localhost", "*.local", "192.168.0.0/16", "127.0.0.1", "www.example.com"]
        );
        assert!(s.exclude_simple);
        assert_eq!(describe(&s), "http 127.0.0.1:6152, socks 127.0.0.1:6153");

        let v6 = [(ListenerKind::Http, "[::]:6152".parse().unwrap())];
        assert_eq!(
            proxy_settings(&general, &v6).unwrap().http,
            Some("[::1]:6152".parse().unwrap()),
            "a wildcard bind becomes the loopback of the same family"
        );
        let no_socks = General {
            set_system_socks_proxy: false,
            ..General::default()
        };
        assert_eq!(proxy_settings(&no_socks, &listeners).unwrap().socks, None);
        assert_eq!(proxy_settings(&general, &[]), None);
        let socks_only = [(ListenerKind::Socks5, "127.0.0.1:6153".parse().unwrap())];
        assert_eq!(proxy_settings(&no_socks, &socks_only), None);
    }

    #[test]
    fn the_file_backend_round_trips_and_unknown_backends_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sysproxy.json");
        let spec = format!("file:{}", path.display());
        let backend = backend_from(&spec).unwrap();
        // no file yet: restore removes what apply created
        let empty = backend.snapshot().unwrap();
        backend.apply(&settings(6152)).unwrap();
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["http"], "127.0.0.1:6152");
        assert_eq!(written["socks"], "127.0.0.1:6153");
        assert_eq!(written["bypass"], json!(["localhost"]));
        backend.restore(&empty).unwrap();
        assert!(!path.exists());
        // an existing file comes back byte for byte
        std::fs::write(&path, "the user's own settings").unwrap();
        let original = backend.snapshot().unwrap();
        backend.apply(&settings(6152)).unwrap();
        backend.restore(&original).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "the user's own settings");
        let foreign = Backup(json!({ "platform": "windows" }));
        assert!(backend.restore(&foreign).is_err());
        assert!(backend_from("registry").is_err());
        assert!(backend_from("file:").is_err());
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p rurge sysproxy::`
Expected: 编译失败（模块未声明 / 类型不存在）。先在 `cli/mod.rs` 加 `pub mod sysproxy;` 再跑，确认失败原因是缺实现而不是缺模块。

- [ ] **Step 3: 实现（测试模块之前）**

`cli/control.rs`：`fn connect_addr` 改为 `pub(crate) fn connect_addr`（文档注释改为 `/// A wildcard bind is reached on the loopback of the same family.`）。

`cli/sysproxy.rs`：

```rust
//! The system proxy as `rurge run` drives it (M4 design §8.2): what to point
//! the operating system at, and the enable / disable / crash-recovery /
//! follow-the-listeners state machine around `rurge_platform::sysproxy`.

use super::control::connect_addr;
use rurge_config::general::General;
use rurge_config::session::ListenerKind;
use rurge_config::{HostList, HostPattern};
use rurge_engine::state::StateStore;
use rurge_platform::sysproxy::{Backup, ProxySettings, SystemProxy};
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// `file:<path>` swaps the operating system for a JSON file (end-to-end tests).
pub const BACKEND_ENV: &str = "RURGE_SYSTEM_PROXY_BACKEND";

/// The first HTTP listener serves http and https; the first SOCKS5 listener
/// serves socks when `set-system-socks-proxy` allows it.
pub fn proxy_settings(
    general: &General,
    listeners: &[(ListenerKind, SocketAddr)],
) -> Option<ProxySettings> {
    let first = |kind: ListenerKind| {
        listeners
            .iter()
            .find(|(k, _)| *k == kind)
            .map(|(_, addr)| connect_addr(*addr))
    };
    let http = first(ListenerKind::Http);
    let socks = if general.set_system_socks_proxy {
        first(ListenerKind::Socks5)
    } else {
        None
    };
    if http.is_none() && socks.is_none() {
        return None;
    }
    Some(ProxySettings {
        http,
        https: http,
        socks,
        bypass: bypass_list(&general.skip_proxy),
        exclude_simple: general.exclude_simple_hostnames,
    })
}

/// The `skip-proxy` entries an operating system's bypass list can express: no
/// negation, no `<…>` tokens, no ports.
fn bypass_list(list: &HostList) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for entry in &list.entries {
        if entry.negate {
            continue;
        }
        let item = match &entry.pattern {
            HostPattern::Glob(glob) => glob.source().to_string(),
            HostPattern::Cidr(net) if net.prefix_len() == net.max_prefix_len() => {
                net.addr().to_string()
            }
            HostPattern::Cidr(net) => net.trunc().to_string(),
            _ => continue,
        };
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

pub fn describe(settings: &ProxySettings) -> String {
    let mut parts = Vec::new();
    if let Some(addr) = settings.http {
        parts.push(format!("http {addr}"));
    }
    if let Some(addr) = settings.socks {
        parts.push(format!("socks {addr}"));
    }
    parts.join(", ")
}

/// The operating system's backend, unless `RURGE_SYSTEM_PROXY_BACKEND` says
/// otherwise. An unknown value is an error: silently falling back would let a
/// typo in a test change the real settings.
pub fn backend() -> anyhow::Result<Arc<dyn SystemProxy>> {
    match std::env::var_os(BACKEND_ENV) {
        None => Ok(Arc::from(rurge_platform::sysproxy::platform())),
        Some(value) => backend_from(&value.to_string_lossy()),
    }
}

fn backend_from(value: &str) -> anyhow::Result<Arc<dyn SystemProxy>> {
    match value.strip_prefix("file:") {
        Some(path) if !path.is_empty() => Ok(Arc::new(FileBackend {
            path: PathBuf::from(path),
        })),
        _ => anyhow::bail!("unknown {BACKEND_ENV} value `{value}` (expected file:<path>)"),
    }
}

/// The "system proxy" is a JSON file.
struct FileBackend {
    path: PathBuf,
}

impl SystemProxy for FileBackend {
    fn snapshot(&self) -> io::Result<Backup> {
        let previous = match std::fs::read_to_string(&self.path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        Ok(Backup(
            serde_json::json!({ "platform": "file", "previous": previous }),
        ))
    }

    fn apply(&self, settings: &ProxySettings) -> io::Result<()> {
        let text = serde_json::json!({
            "http": settings.http.map(|a| a.to_string()),
            "https": settings.https.map(|a| a.to_string()),
            "socks": settings.socks.map(|a| a.to_string()),
            "bypass": settings.bypass,
            "exclude_simple": settings.exclude_simple,
        });
        std::fs::write(&self.path, text.to_string())
    }

    fn restore(&self, backup: &Backup) -> io::Result<()> {
        if backup.0["platform"] != "file" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the system proxy backup was not taken by the file backend",
            ));
        }
        match backup.0["previous"].as_str() {
            Some(text) => std::fs::write(&self.path, text),
            None => match std::fs::remove_file(&self.path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e),
            },
        }
    }
}

/// Registry writes and spawned tools block: keep them off the runtime.
async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> Result<T, String> {
    match tokio::task::spawn_blocking(job).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(e.to_string()),
        Err(e) => Err(format!("system proxy task failed: {e}")),
    }
}

/// Owned by the `rurge run` main loop, which calls it one request at a time.
pub struct SystemProxyManager {
    backend: Arc<dyn SystemProxy>,
    store: Arc<StateStore>,
    /// What the operating system points at right now (`None` = not ours).
    applied: Option<ProxySettings>,
    flag: Arc<AtomicBool>,
}

impl SystemProxyManager {
    pub fn new(backend: Arc<dyn SystemProxy>, store: Arc<StateStore>) -> Self {
        SystemProxyManager {
            backend,
            store,
            applied: None,
            flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Mirrors `enabled()`; shared with `Control::system_proxy_enabled`.
    pub fn flag(&self) -> Arc<AtomicBool> {
        self.flag.clone()
    }

    pub fn enabled(&self) -> bool {
        self.applied.is_some()
    }

    /// What the operating system points at right now.
    pub fn applied(&self) -> Option<&ProxySettings> {
        self.applied.as_ref()
    }

    /// Startup: a backup still in `state.json` means the last run died with
    /// the system proxy pointing at it.
    pub async fn recover(&mut self) {
        let Some(saved) = self.store.snapshot().await.system_proxy_backup else {
            return;
        };
        tracing::warn!(
            "a previous run left the system proxy pointing at rurge; restoring the saved settings"
        );
        let backend = self.backend.clone();
        match blocking(move || backend.restore(&Backup(saved))).await {
            Ok(()) => self.forget().await,
            Err(e) => tracing::error!(
                error = %e,
                "cannot restore the system proxy; the saved settings stay in state.json for the next attempt"
            ),
        }
    }

    pub async fn enable(&mut self, settings: ProxySettings) -> Result<(), String> {
        if self.applied.as_ref() == Some(&settings) {
            return Ok(());
        }
        if self.applied.is_none() {
            // A backup that is already saved is the real original (an earlier
            // restore failed): never snapshot rurge's own settings over it.
            let backup = match self.store.snapshot().await.system_proxy_backup {
                Some(_) => None,
                None => {
                    let backend = self.backend.clone();
                    Some(blocking(move || backend.snapshot()).await?)
                }
            };
            // saved before anything changes, so a crash mid-apply is recoverable
            self.store
                .update(move |s| {
                    if let Some(backup) = backup {
                        s.system_proxy_backup = Some(backup.0);
                    }
                    s.features.system_proxy = true;
                })
                .await;
        }
        let backend = self.backend.clone();
        let wanted = settings.clone();
        match blocking(move || backend.apply(&wanted)).await {
            Ok(()) => {
                self.applied = Some(settings);
                self.flag.store(true, Ordering::SeqCst);
                Ok(())
            }
            Err(e) => {
                // a partial apply may have changed something: put the original back
                if let Err(undo) = self.disable().await {
                    tracing::error!(
                        error = %undo,
                        "cannot undo the failed system proxy change; the saved settings stay in state.json"
                    );
                    self.applied = None;
                    self.flag.store(false, Ordering::SeqCst);
                }
                Err(e)
            }
        }
    }

    /// Restores the saved settings. When the restore fails the backup stays in
    /// `state.json`, so the next start tries again.
    pub async fn disable(&mut self) -> Result<(), String> {
        if let Some(saved) = self.store.snapshot().await.system_proxy_backup {
            let backend = self.backend.clone();
            blocking(move || backend.restore(&Backup(saved))).await?;
        }
        self.forget().await;
        Ok(())
    }

    /// After a reload: follow the listeners and `skip-proxy` when they changed.
    pub async fn refresh(&mut self, settings: Option<ProxySettings>) -> Result<(), String> {
        match settings {
            Some(settings) if self.enabled() => self.enable(settings).await,
            _ => Ok(()),
        }
    }

    async fn forget(&mut self) {
        self.store
            .update(|s| {
                s.system_proxy_backup = None;
                s.features.system_proxy = false;
            })
            .await;
        self.applied = None;
        self.flag.store(false, Ordering::SeqCst);
    }
}
```

本任务结束时 `run.rs` 还没用到这些条目，bin 是二进制 crate，未使用的 `pub` 条目会触发 `dead_code`。在 `cli/mod.rs` 的声明上加：

```rust
// wired into `rurge run` by the next tasks of the M4b plan
#[allow(dead_code)]
pub mod sysproxy;
```

Task 8 接入后删除这个 `allow`（那里的步骤里有）。

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p rurge sysproxy::`
Expected: 8 个测试 PASS。`disable` 在「第二次调用」时会多做一次 `forget()` 的状态写入（幂等）；测试只断言后端没有被再次调用。

- [ ] **Step 5: 质量门与提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge
git commit -F - <<'EOF'
feat(cli): SystemProxyManager 生命周期、proxy_settings 与文件后端

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th
EOF
```

---

### Task 6: `Control::system_proxy_enabled` 与 `/v1/features/system_proxy` 接通

**Files:**
- Modify: `crates/rurge-engine/src/control.rs`
- Modify: `crates/rurge-api/src/routes/features.rs`、`crates/rurge-api/tests/api.rs`
- Modify: `crates/rurge/src/cli/run.rs`（`LoopControl` 补上新方法；真正接线在 Task 8）

**Interfaces:**
- Produces:
  - `Control` 新增 `fn system_proxy_enabled(&self) -> bool;`；`set_system_proxy` 的文档改为「enable / disable the system proxy; the error text is shown to the API caller」。
  - `GET /v1/features/system_proxy` → `{"enabled": <Control::system_proxy_enabled()>}`；其余五个功能仍恒为 `false`。`POST /v1/features/system_proxy {"enabled":bool}` → `{}`；失败 → `500 {"error":"<原因>"}`（P10：不再有 501 分支）。其余功能的 `POST` 仍是 501。
  - `tests/api.rs` 的 `FakeControl` 增加 `system_proxy: AtomicBool`、`fail_system_proxy: AtomicBool`。

- [ ] **Step 1: 写失败测试**

`crates/rurge-api/tests/api.rs`：`FakeControl` 增加两个字段（`#[derive(Default)]` 仍适用），并把 `set_system_proxy` 与新方法实现为：

```rust
    fn set_system_proxy(&self, enabled: bool) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            if self.fail_system_proxy.load(Ordering::SeqCst) {
                return Err("no supported desktop proxy settings were found".to_string());
            }
            self.system_proxy.store(enabled, Ordering::SeqCst);
            Ok(())
        })
    }
    fn system_proxy_enabled(&self) -> bool {
        self.system_proxy.load(Ordering::SeqCst)
    }
```

（`use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};`。）在 `features_collections_stop_and_unknown_paths` 里，把原来断言 `POST /v1/features/system_proxy` → 501 的两行替换为：

```rust
    assert_eq!(get(&api, "/v1/features/system_proxy").await, (200, json!({ "enabled": false })));
    assert_eq!(
        post(&api, "/v1/features/system_proxy", json!({ "enabled": true })).await,
        (200, json!({}))
    );
    assert_eq!(get(&api, "/v1/features/system_proxy").await, (200, json!({ "enabled": true })));
    assert_eq!(get(&api, "/v1/features/mitm").await, (200, json!({ "enabled": false })), "only system_proxy is live");
    assert_eq!(
        post(&api, "/v1/features/system_proxy", json!({ "enabled": false })).await,
        (200, json!({}))
    );
    assert!(!api.control.system_proxy.load(Ordering::SeqCst));
    api.control.fail_system_proxy.store(true, Ordering::SeqCst);
    let (status, body) = post(&api, "/v1/features/system_proxy", json!({ "enabled": true })).await;
    assert_eq!(status, 500, "{body}");
    assert_eq!(body["error"], "no supported desktop proxy settings were found");
    let (status, _) = post(&api, "/v1/features/system_proxy", json!({ "enabled": "yes" })).await;
    assert_eq!(status, 400);
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p rurge-api --test api features_collections`
Expected: 编译失败（`Control` 没有 `system_proxy_enabled`）。

- [ ] **Step 3: 实现**

`crates/rurge-engine/src/control.rs` 的 trait：

```rust
pub trait Control: Send + Sync {
    fn reload(&self) -> BoxFuture<'_, ReloadReport>;
    fn stop(&self) -> BoxFuture<'_, ()>;
    fn set_log_level(&self, level: LogLevel) -> Result<(), String>;
    /// Points the operating system's proxy settings at rurge, or puts them
    /// back. The error text is shown to the API caller.
    fn set_system_proxy(&self, enabled: bool) -> BoxFuture<'_, Result<(), String>>;
    fn system_proxy_enabled(&self) -> bool;
}
```

`crates/rurge-api/src/routes/features.rs`：模块文档改为「only `system_proxy` is live in phase 1; the others read as off and cannot be switched」；`get_feature` 与 `set_feature` 改为：

```rust
pub async fn get_feature(
    State(app): State<App>,
    Path(name): Path<String>,
) -> ApiResult<Json<Value>> {
    known(&name)?;
    let enabled = name == "system_proxy" && app.control.system_proxy_enabled();
    Ok(Json(json!({ "enabled": enabled })))
}
```

```rust
    app.control
        .set_system_proxy(body.enabled)
        .await
        .map_err(ApiError::internal)?;
    tracing::info!(enabled = body.enabled, "system proxy switched via http-api");
    Ok(Json(json!({})))
```

（`set_feature` 里原来的 `match … Err(e) if e.contains("not implemented")` 整段被上面四行取代。）

`crates/rurge/src/cli/run.rs` 的 `impl Control for LoopControl` 补：

```rust
    fn system_proxy_enabled(&self) -> bool {
        false
    }
```

（`set_system_proxy` 暂时保持 M4a 的 `Err("not implemented")`；Task 8 替换这两个方法。）

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p rurge-api`（`--test api` 3×）、`cargo test -p rurge-engine control::`
Expected: PASS。

- [ ] **Step 5: 质量门与提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-engine crates/rurge-api crates/rurge
git commit -F - <<'EOF'
feat(api): /v1/features/system_proxy 接通 Control（读真实状态，失败 500）

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th
EOF
```

---

### Task 7: M4a 延后的两条——全局策略守卫对称、`rurge-api` 改走引擎视图

**Files:**
- Modify: `crates/rurge-engine/src/engine.rs`
- Modify: `crates/rurge-api/src/routes/outbound.rs`、`routes/dns.rs`、`routes/profiles.rs`、`tests/api.rs`

**Interfaces:**
- Produces:
  - `Engine::{resolver() -> Arc<rurge_dns::Resolver>, internet_test_url() -> String, profile_path() -> PathBuf}`；`Engine::config_text` 改用 `profile_path()`。
  - `POST /v1/outbound/global {"policy":""}` 在出站模式是 `proxy` 时 → `400 {"error":"cannot clear the global policy while the outbound mode is proxy; switch the mode first"}`；其它模式下仍然清空。
  - `crates/rurge-api/src` 里不再出现 `runtime()`。

- [ ] **Step 1: 写失败测试**

`crates/rurge-api/tests/api.rs` 追加：

```rust
#[tokio::test]
async fn the_global_policy_cannot_be_cleared_while_in_proxy_mode() {
    let api = api().await;
    assert_eq!(get(&api, "/v1/outbound/global").await, (200, json!({ "policy": null })));
    assert_eq!(post(&api, "/v1/outbound/global", json!({ "policy": "Pick" })).await.0, 200);
    assert_eq!(post(&api, "/v1/outbound", json!({ "mode": "proxy" })).await.0, 200);
    let (status, body) = post(&api, "/v1/outbound/global", json!({ "policy": " " })).await;
    assert_eq!(status, 400, "{body}");
    assert!(body["error"].as_str().unwrap().contains("while the outbound mode is proxy"), "{body}");
    assert_eq!(get(&api, "/v1/outbound/global").await.1, json!({ "policy": "Pick" }), "unchanged");
    // replacing it is fine, and so is clearing it once the mode moved on
    assert_eq!(post(&api, "/v1/outbound/global", json!({ "policy": "DIRECT" })).await.0, 200);
    assert_eq!(post(&api, "/v1/outbound", json!({ "mode": "rule" })).await.0, 200);
    assert_eq!(post(&api, "/v1/outbound/global", json!({ "policy": "" })).await, (200, json!({})));
    assert_eq!(get(&api, "/v1/outbound/global").await.1, json!({ "policy": null }));
}
```

并把 `dns_cache_flush_and_delay` 里的 `api.engine.runtime().stack.resolver.clone()` 改成 `api.engine.resolver()`。

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p rurge-api --test api`
Expected: 编译失败（`Engine::resolver` 不存在）；补上视图后，新测试在守卫实现前以 `200 != 400` 失败。

- [ ] **Step 3: 实现**

`crates/rurge-engine/src/engine.rs`，放在 `rules_view` 之后：

```rust
    /// The resolver of the current config generation.
    pub fn resolver(&self) -> Arc<rurge_dns::Resolver> {
        self.runtime().stack.resolver.clone()
    }

    /// `internet-test-url` of the current config generation.
    pub fn internet_test_url(&self) -> String {
        self.runtime().config.general.internet_test_url.clone()
    }

    /// The main profile file of the current config generation.
    pub fn profile_path(&self) -> std::path::PathBuf {
        self.runtime().config.source.main.clone()
    }
```

`config_text` 的第一行改为 `let path = self.profile_path();`。

`routes/dns.rs`：`dns` 里 `let resolver = app.engine.resolver();`；`flush` 里 `app.engine.resolver().flush();`；`dns_delay` 里去掉 `let rt = app.engine.runtime();`，缺省名取 `host_of(&app.engine.internet_test_url())`，测量用 `app.engine.resolver().measure_delay(&name).await`。`routes/profiles.rs` 的 `check`：`let path = app.engine.profile_path();`。

`routes/outbound.rs` 的 `set_global`，在 `json_body` 之后：

```rust
    if body.policy.trim().is_empty() && app.engine.mode() == Mode::Proxy {
        return Err(ApiError::bad_request(
            "cannot clear the global policy while the outbound mode is proxy; switch the mode first",
        ));
    }
```

完成后 `grep -rn "runtime()" crates/rurge-api/src` 应无输出。

- [ ] **Step 4: 运行确认通过**

Run: `cargo test -p rurge-api`（`--test api` 3×）、`cargo test -p rurge-engine`
Expected: PASS。

- [ ] **Step 5: 质量门与提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-engine crates/rurge-api
git commit -F - <<'EOF'
refactor(api): 全局策略守卫两侧对称；rurge-api 改走引擎只读视图

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th
EOF
```

---

### Task 8: `rurge run` 集成——`--system-proxy`、命令通道、启动恢复、重载跟随、退出恢复、Windows 控制台事件

**Files:**
- Modify: `crates/rurge/src/cli/run.rs`
- Modify: `crates/rurge/src/cli/mod.rs`（删除 Task 5 留下的 `#[allow(dead_code)]` 与其注释）
- Modify: `crates/rurge/tests/cli.rs`（`mod run`）

**Interfaces:**
- Consumes: Task 5 的 `cli::sysproxy::{SystemProxyManager, backend, describe, proxy_settings}`；Task 6 的 `Control::{set_system_proxy, system_proxy_enabled}`；`rurge_platform::sysproxy::ProxySettings`。
- Produces（bin 内部与可观察行为）：
  - `RunArgs.system_proxy: bool`（`--system-proxy`，env `RURGE_SYSTEM_PROXY`）。
  - `Command::SystemProxy(bool, oneshot::Sender<Result<(), String>>)`；`LoopControl { tx, log_level, system_proxy: Arc<AtomicBool> }`。
  - `ShutdownSignals::{new() -> anyhow::Result<Self>, recv(&mut self).await}`：Unix = SIGINT + SIGTERM；Windows = Ctrl-C + 控制台关闭 + 注销 + 关机（P6）。
  - 启动顺序：绑定监听（`listening on …`）→ 建 `SystemProxyManager` → 命令通道与 API（`api on …`）→ 重载通道 / watcher → 信号流 → `recover()` →（带 `--system-proxy`）开启，成功打印 `system proxy enabled: http <addr>[, socks <addr>]`，失败 stderr `error: cannot enable the system proxy: <原因>` 并退出 1 → 汇总行 `rurge <ver> running: …`。
  - 退出：循环结束后先打印 `shutting down …`，再恢复系统代理（成功打印 `system proxy restored`；失败 stderr `error: cannot restore the system proxy: …; the saved settings stay in state.json and are restored on the next start`），然后才是 `api_token.cancel()` 等既有步骤。
  - 每次成功的重载之后调用 `SystemProxyManager::refresh`。
  - 测试 harness：`spawn_daemon_full` 给每个守护进程设 `RURGE_SYSTEM_PROXY_BACKEND=file:<data>/sysproxy.json` 并清掉 `RURGE_SYSTEM_PROXY`。

- [ ] **Step 1: 端到端测试（先失败）**

`crates/rurge/tests/cli.rs` 的 `mod run`：

1. `spawn_daemon_full` 里，在 `cmd.args(extra);` 之前加（P5：任何测试都碰不到真实的系统代理）：

```rust
        cmd.env(
            "RURGE_SYSTEM_PROXY_BACKEND",
            format!("file:{}", sysproxy_file(data).display()),
        )
        .env_remove("RURGE_SYSTEM_PROXY");
```

2. 新助手：

```rust
    /// Where the test backend keeps the "system proxy" of a daemon.
    fn sysproxy_file(data: &Path) -> std::path::PathBuf {
        data.join("sysproxy.json")
    }

    fn read_json(path: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    /// Polls until `check` passes or 10 s elapse.
    fn wait_until(what: &str, mut check: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !check() {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for {what}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
```

3. 三个测试：

```rust
    const ORIGINAL_PROXY: &str = "the user's own proxy settings";

    #[test]
    fn run_system_proxy_is_applied_switched_and_restored() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let file = sysproxy_file(&data);
        std::fs::write(&file, ORIGINAL_PROXY).unwrap();
        let mut daemon = spawn_daemon_full(&conf, &data, false, None, &["--system-proxy"]);
        let port = api_port(&daemon);
        let enabled = wait_for_line(&daemon, "system proxy enabled: ");
        assert_eq!(
            enabled,
            format!("http 127.0.0.1:{}, socks 127.0.0.1:{}", daemon.http, daemon.socks)
        );
        let applied = read_json(&file);
        assert_eq!(applied["http"], format!("127.0.0.1:{}", daemon.http));
        assert_eq!(applied["https"], applied["http"]);
        assert_eq!(applied["socks"], format!("127.0.0.1:{}", daemon.socks));
        let state = read_json(&data.join("state.json"));
        assert_eq!(state["system_proxy_backup"]["previous"], ORIGINAL_PROXY);
        assert_eq!(state["features"]["system_proxy"], true);
        assert_eq!(
            api_call(port, "GET", "/v1/features/system_proxy", "k", None).1,
            r#"{"enabled":true}"#
        );
        // off and on again through the API
        assert_eq!(
            api_call(port, "POST", "/v1/features/system_proxy", "k", Some(r#"{"enabled":false}"#)),
            (200, "{}".to_string())
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), ORIGINAL_PROXY);
        assert!(read_json(&data.join("state.json"))["system_proxy_backup"].is_null());
        assert_eq!(
            api_call(port, "GET", "/v1/features/system_proxy", "k", None).1,
            r#"{"enabled":false}"#
        );
        assert_eq!(
            api_call(port, "POST", "/v1/features/system_proxy", "k", Some(r#"{"enabled":true}"#)).0,
            200
        );
        assert_eq!(read_json(&file)["http"], format!("127.0.0.1:{}", daemon.http));
        // a graceful stop puts the original back
        assert_eq!(api_call(port, "POST", "/v1/stop", "k", Some("{}")).0, 200);
        assert_eq!(wait_for_exit(&mut daemon, 5), Some(0));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), ORIGINAL_PROXY);
        let state = read_json(&data.join("state.json"));
        assert!(state["system_proxy_backup"].is_null());
        assert_eq!(state["features"]["system_proxy"], false);
        let lines: Vec<String> = daemon.lines.try_iter().collect();
        assert!(lines.iter().any(|l| l == "system proxy restored"), "{lines:?}");
    }

    #[test]
    fn run_restores_the_system_proxy_a_crashed_run_left_behind() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let file = sysproxy_file(&data);
        std::fs::write(&file, ORIGINAL_PROXY).unwrap();
        let crashed = spawn_daemon_full(&conf, &data, false, None, &["--system-proxy"]);
        wait_for_line(&crashed, "system proxy enabled: ");
        drop(crashed); // `Daemon::drop` kills the process: no cleanup runs
        assert_ne!(std::fs::read_to_string(&file).unwrap(), ORIGINAL_PROXY, "still pointing at the dead daemon");
        assert!(!read_json(&data.join("state.json"))["system_proxy_backup"].is_null());
        // the next start, without --system-proxy, cleans up
        let next = spawn_daemon(&conf, &data);
        wait_for_line(&next, "rurge ");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), ORIGINAL_PROXY);
        let state = read_json(&data.join("state.json"));
        assert!(state["system_proxy_backup"].is_null());
        assert_eq!(state["features"]["system_proxy"], false);
    }

    #[test]
    fn run_reapplies_the_system_proxy_after_a_reload() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        let data = dir.path().join("data");
        let daemon = spawn_daemon_full(&conf, &data, false, None, &["--system-proxy"]);
        let port = api_port(&daemon);
        wait_for_line(&daemon, "system proxy enabled: ");
        let file = sysproxy_file(&data);
        assert_eq!(read_json(&file)["bypass"], serde_json::json!([]));
        write_conf(
            dir.path(),
            &format!("{API_GENERAL}\nskip-proxy = localhost, example.internal"),
        );
        let (status, body) = api_call(port, "POST", "/v1/profiles/reload", "k", Some("{}"));
        assert_eq!(status, 200, "{body}");
        assert!(body.contains("\"ok\":true"), "{body}");
        // tolerant read: the daemon may be rewriting the file at this instant
        wait_until("the bypass list to follow the reload", || {
            std::fs::read_to_string(&file)
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .is_some_and(|v| {
                    v["bypass"] == serde_json::json!(["localhost", "example.internal"])
                })
        });
        assert_eq!(read_json(&file)["http"], format!("127.0.0.1:{}", daemon.http));
    }
```

（第二个测试里 `wait_for_line(&next, "rurge ")` 等的是汇总行：恢复发生在它之前，所以此后的断言不需要轮询。第一个测试先 `api_port` 再等 `system proxy enabled: `，与启动输出的顺序一致：`listening on …` → `api on …` → `system proxy enabled: …` → 汇总行。）

- [ ] **Step 2: 运行确认失败**

Run: `cargo build -p rurge && cargo test -p rurge --test cli run_system_proxy`
Expected: FAIL——`--system-proxy` 是未知参数，守护进程启动即退出，`spawn_daemon_full` 等不到监听行而 panic。

- [ ] **Step 3: `run.rs` 改造**

按顺序改：

1. imports 追加：

```rust
use super::sysproxy::{SystemProxyManager, describe, proxy_settings};
use rurge_platform::sysproxy::ProxySettings;
use std::sync::atomic::{AtomicBool, Ordering};
```

2. `RunArgs`，放在 `watch` 之后：

```rust
    /// Point the operating system's proxy settings at rurge while it runs
    #[arg(long, env = "RURGE_SYSTEM_PROXY")]
    pub system_proxy: bool,
```

3. `Command` 与 `LoopControl`：

```rust
enum Command {
    Reload(oneshot::Sender<ReloadReport>),
    Stop,
    SystemProxy(bool, oneshot::Sender<Result<(), String>>),
}

struct LoopControl {
    tx: mpsc::Sender<Command>,
    log_level: LevelHandle,
    /// Mirrors `SystemProxyManager::enabled`.
    system_proxy: Arc<AtomicBool>,
}
```

`impl Control for LoopControl` 里替换 Task 6 的两个占位方法：

```rust
    fn set_system_proxy(&self, enabled: bool) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            let (reply, rx) = oneshot::channel();
            self.tx
                .send(Command::SystemProxy(enabled, reply))
                .await
                .map_err(|_| "rurge is shutting down".to_string())?;
            rx.await
                .unwrap_or_else(|_| Err("rurge is shutting down".to_string()))
        })
    }

    fn system_proxy_enabled(&self) -> bool {
        self.system_proxy.load(Ordering::SeqCst)
    }
```

4. 关闭信号（放在 `spawn_watcher` 之后）：

```rust
/// Everything that means "shut down": Ctrl-C and SIGTERM on Unix; Ctrl-C,
/// the console closing, logoff and system shutdown on Windows (the last three
/// leave the process a few seconds — enough to put the system proxy back).
/// One long-lived stream per signal, built once: a stream buffers a signal
/// that arrives while nobody is awaiting it, whereas a fresh future per loop
/// iteration would drop every signal delivered during a reload.
struct ShutdownSignals {
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
    #[cfg(windows)]
    ctrl_c: tokio::signal::windows::CtrlC,
    #[cfg(windows)]
    close: tokio::signal::windows::CtrlClose,
    #[cfg(windows)]
    logoff: tokio::signal::windows::CtrlLogoff,
    #[cfg(windows)]
    shutdown: tokio::signal::windows::CtrlShutdown,
}

impl ShutdownSignals {
    fn new() -> anyhow::Result<ShutdownSignals> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            Ok(ShutdownSignals {
                interrupt: signal(SignalKind::interrupt()).context("cannot listen for SIGINT")?,
                terminate: signal(SignalKind::terminate())
                    .context("cannot listen for SIGTERM")?,
            })
        }
        #[cfg(windows)]
        {
            use tokio::signal::windows;
            Ok(ShutdownSignals {
                ctrl_c: windows::ctrl_c().context("cannot listen for Ctrl-C")?,
                close: windows::ctrl_close().context("cannot listen for the console closing")?,
                logoff: windows::ctrl_logoff().context("cannot listen for logoff")?,
                shutdown: windows::ctrl_shutdown()
                    .context("cannot listen for system shutdown")?,
            })
        }
    }

    async fn recv(&mut self) {
        #[cfg(unix)]
        tokio::select! {
            _ = self.interrupt.recv() => {}
            _ = self.terminate.recv() => {}
        }
        #[cfg(windows)]
        tokio::select! {
            _ = self.ctrl_c.recv() => {}
            _ = self.close.recv() => {}
            _ = self.logoff.recv() => {}
            _ = self.shutdown.recv() => {}
        }
    }
}
```

5. 系统代理的三个助手（放在 `reload` 之后）：

```rust
/// What the system proxy should point at, given what is bound right now.
fn current_settings(
    engine: &Engine,
    listeners: &[(ListenerSpec, Running)],
) -> Option<ProxySettings> {
    let bound: Vec<_> = listeners
        .iter()
        .map(|(spec, running)| (spec.kind, running.local_addr))
        .collect();
    proxy_settings(&engine.runtime().config.general, &bound)
}

async fn switch_system_proxy(
    manager: &mut SystemProxyManager,
    engine: &Engine,
    listeners: &[(ListenerSpec, Running)],
    enabled: bool,
) -> Result<(), String> {
    if !enabled {
        return manager.disable().await;
    }
    let settings = current_settings(engine, listeners).ok_or_else(|| {
        "there is no http or socks5 listener to point the system proxy at".to_string()
    })?;
    manager.enable(settings).await
}

/// A reload may move the listeners or change `skip-proxy`: keep the system
/// proxy in step.
async fn reload_and_refresh(
    d: &Daemon<'_>,
    listeners: &mut Vec<(ListenerSpec, Running)>,
    sysproxy: &mut SystemProxyManager,
) -> ReloadReport {
    let report = reload(d, listeners).await;
    if report.ok
        && let Err(e) = sysproxy
            .refresh(current_settings(d.engine, listeners))
            .await
    {
        tracing::error!(error = %e, "cannot re-apply the system proxy after the reload");
    }
    report
}
```

6. `run()` 主体。`print_listening(&listeners);` 之后的部分重排为（未列出的语句原样保留）：

```rust
        print_listening(&listeners);
        let mut sysproxy =
            SystemProxyManager::new(super::sysproxy::backend()?, store.clone());

        // Command channel from the API …（注释原样）
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<Command>(4);
        let control: Arc<dyn Control> = Arc::new(LoopControl {
            tx: cmd_tx.clone(),
            log_level: level_handle,
            system_proxy: sysproxy.flag(),
        });
        // …API 启动与 `api on` 行：原样…

        // Reload triggers …（`reload_tx` / `_watcher`：原样，从汇总行之后移到这里）
        let mut signals = ShutdownSignals::new()?;
        #[cfg(unix)]
        let mut sighup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
            .context("cannot listen for SIGHUP")?;

        // Everything fallible is behind us: nothing below returns early while
        // the operating system points at rurge, except the failure to enable.
        sysproxy.recover().await;
        if args.system_proxy {
            if let Err(e) = switch_system_proxy(&mut sysproxy, &engine, &listeners, true).await {
                eprintln!("error: cannot enable the system proxy: {e}");
                return Ok(ExitCode::from(1));
            }
            let applied = sysproxy.applied().map(describe).unwrap_or_default();
            println!("system proxy enabled: {applied}");
            tracing::info!(%applied, "system proxy enabled");
        }

        // The one startup line that also reaches `--log-file` …（汇总行：原样）
        let daemon = Daemon { /* 原样 */ };

        loop {
            let reload_signal = async { /* 原样 */ };
            tokio::select! {
                _ = signals.recv() => break,
                _ = reload_signal => {
                    reload_and_refresh(&daemon, &mut listeners, &mut sysproxy).await;
                }
                cmd = cmd_rx.recv() => match cmd {
                    Some(Command::Reload(reply)) => {
                        let report =
                            reload_and_refresh(&daemon, &mut listeners, &mut sysproxy).await;
                        let _ = reply.send(report);
                    }
                    Some(Command::Stop) => {
                        println!("stop requested via http-api");
                        break;
                    }
                    Some(Command::SystemProxy(enabled, reply)) => {
                        let result =
                            switch_system_proxy(&mut sysproxy, &engine, &listeners, enabled).await;
                        if let Err(e) = &result {
                            tracing::error!(error = %e, enabled, "cannot switch the system proxy");
                        }
                        let _ = reply.send(result);
                    }
                    None => {}
                },
            }
        }

        // …`shutting down` 两行 println：原样…
        // The system proxy first: rurge is about to stop serving, and the
        // forced exit below must not leave the OS pointing at a dead port.
        if sysproxy.enabled() {
            match sysproxy.disable().await {
                Ok(()) => println!("system proxy restored"),
                Err(e) => eprintln!(
                    "error: cannot restore the system proxy: {e}; the saved settings stay in state.json and are restored on the next start"
                ),
            }
        }
        api_token.cancel();
        // …其余原样；`force_exit` 整段换成：
        let force_exit = signals.recv();
```

原来的 `interrupt` / `sigterm` 两个流与循环里的 `shutdown_signal` 块、关闭阶段的 `force_exit` 块随之删除（`sighup` 保留）。`Daemon` 借用 `engine` / `store` 等不受影响；`sysproxy` 与 `listeners` 是循环里的可变借用，与 `daemon` 的共享借用不冲突。

7. `cli/mod.rs`：删除 `sysproxy` 声明上的 `#[allow(dead_code)]` 与那行注释。若 `SystemProxyManager::enabled` 等仍有未使用条目，删掉条目而不是保留 allow（`flag`、`enabled`、`applied`、`recover`、`enable`、`disable`、`refresh`、`describe`、`proxy_settings`、`backend` 此时都有调用方）。

- [ ] **Step 4: 运行确认通过**

Run: `cargo build -p rurge && cargo test -p rurge`（`--test cli` 3×）
Expected: 3 个新测试与既有的全部 PASS。Windows 上既有的 Ctrl-C 行为不变（`ShutdownSignals` 多监听三种控制台事件）；Unix 分支由 CI 编译与运行（`run_shuts_down_gracefully_on_sigint`）。

- [ ] **Step 5: 质量门与提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge
git commit -F - <<'EOF'
feat(cli): rurge run --system-proxy：启动恢复、API 开关、重载跟随、退出恢复与 Windows 控制台事件

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th
EOF
```

---

### Task 9: `rurge service install | uninstall`

**Files:**
- Create: `crates/rurge/src/cli/service.rs`
- Modify: `crates/rurge/src/cli/mod.rs`（`pub mod service;`）、`crates/rurge/src/main.rs`
- Modify: `crates/rurge/tests/cli.rs`（新 `mod service`）

**Interfaces:**
- Consumes: Task 4 的 `rurge_platform::service::{Scope, ServiceSpec, Plan, install_plan, uninstall_plan, execute}`；`rurge_platform::command::{CommandRunner, SystemRunner}`；`rurge_platform::dirs::Os`。
- Produces:
  - `rurge service install -c <conf> [--user] [--system-proxy] [--dry-run]`、`rurge service uninstall [--user] [--dry-run]`；`main.rs` 的 `Command::Service(cli::service::ServiceArgs)`。
  - `--dry-run` 的输出：每个文件 `write <path>:` 后跟内容；每条命令 `run: <空格拼接>`；每个待删文件 `remove <path>`；最后一行 `dry run: nothing was changed`。
  - 退出码：配置文件不存在 → 2（`error: cannot find the profile <path>`）；执行失败 → 1（`error: <命令的输出>`）；成功 0（`installed: rurge starts automatically (<scope>)` / `uninstalled`）。

- [ ] **Step 1: 端到端测试（先失败）**

`crates/rurge/tests/cli.rs` 末尾新增：

```rust
mod service {
    use assert_cmd::Command;
    use predicates::prelude::*;

    fn rurge() -> Command {
        Command::cargo_bin("rurge").unwrap()
    }

    #[test]
    fn install_dry_run_prints_the_plan_for_this_platform() {
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join("my rurge.conf");
        std::fs::write(&conf, "[General]\n[Rule]\nFINAL,DIRECT\n").unwrap();
        let out = rurge()
            .args(["service", "install", "--user", "--system-proxy", "--dry-run", "-c"])
            .arg(&conf)
            .assert()
            .success();
        let text = String::from_utf8(out.get_output().stdout.clone()).unwrap();
        assert!(text.contains("my rurge.conf"), "{text}");
        assert!(text.contains("--system-proxy"), "{text}");
        assert!(text.ends_with("dry run: nothing was changed\n"), "{text}");
        if cfg!(windows) {
            assert!(text.contains("run: schtasks /create /tn rurge /sc onlogon /tr "), "{text}");
        } else if cfg!(target_os = "macos") {
            assert!(text.contains("io.rurge.daemon.plist:") && text.contains("run: launchctl bootstrap gui/"), "{text}");
        } else {
            assert!(text.contains("rurge.service:") && text.contains("ExecStart="), "{text}");
            assert!(text.contains("run: systemctl --user enable --now rurge"), "{text}");
        }
    }

    #[test]
    fn uninstall_dry_run_and_a_missing_profile() {
        let out = rurge()
            .args(["service", "uninstall", "--user", "--dry-run"])
            .assert()
            .success();
        let text = String::from_utf8(out.get_output().stdout.clone()).unwrap();
        assert!(text.contains("run: "), "{text}");
        assert!(text.ends_with("dry run: nothing was changed\n"), "{text}");
        rurge()
            .args(["service", "install", "--dry-run", "-c", "no-such-profile.conf"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("cannot find the profile"));
    }
}
```

- [ ] **Step 2: 运行确认失败**

Run: `cargo test -p rurge --test cli service::`
Expected: FAIL（`service` 不是已知子命令，退出码 2，stdout 为空）。

- [ ] **Step 3: 实现**

`crates/rurge/src/cli/service.rs`：

```rust
//! `rurge service install | uninstall` (M4 design §8.3): register rurge with
//! systemd / launchd / the Windows task scheduler so it starts automatically.

use anyhow::Context;
use clap::{Args, Subcommand};
use rurge_platform::command::{CommandRunner, SystemRunner};
use rurge_platform::dirs::Os;
use rurge_platform::service::{Plan, Scope, ServiceSpec, execute, install_plan, uninstall_plan};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Args)]
pub struct ServiceArgs {
    #[command(subcommand)]
    pub command: ServiceCommand,
}

#[derive(Subcommand)]
pub enum ServiceCommand {
    /// Start rurge automatically (systemd unit, launchd plist or a Windows logon task)
    Install(InstallArgs),
    /// Remove what `service install` registered
    Uninstall(UninstallArgs),
}

#[derive(Args)]
pub struct InstallArgs {
    /// Profile the service runs with
    #[arg(short = 'c', long = "config", value_name = "FILE")]
    pub config: PathBuf,
    /// Install for the current user instead of system-wide (Windows: always per user)
    #[arg(long)]
    pub user: bool,
    /// Start the service with --system-proxy
    #[arg(long)]
    pub system_proxy: bool,
    /// Print the files and commands without changing anything
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct UninstallArgs {
    /// Remove the per-user installation instead of the system-wide one
    #[arg(long)]
    pub user: bool,
    /// Print the commands without changing anything
    #[arg(long)]
    pub dry_run: bool,
}

fn scope(user: bool) -> Scope {
    if user { Scope::User } else { Scope::System }
}

/// launchd's per-user domain is `gui/<uid>`; `id -u` avoids an FFI call.
fn uid(os: Os, scope: Scope) -> Option<u32> {
    if os != Os::MacOs || scope != Scope::User {
        return None;
    }
    let out = SystemRunner
        .run(&["id".to_string(), "-u".to_string()])
        .ok()?;
    out.trim().parse().ok()
}

fn print_plan(plan: &Plan) {
    for (path, content) in &plan.files {
        println!("write {}:", path.display());
        print!("{content}");
    }
    for command in &plan.commands {
        println!("run: {}", command.join(" "));
    }
    for path in &plan.remove {
        println!("remove {}", path.display());
    }
    println!("dry run: nothing was changed");
}

fn carry_out(plan: &Plan, dry_run: bool, done: &str) -> ExitCode {
    if dry_run {
        print_plan(plan);
        return ExitCode::SUCCESS;
    }
    match execute(plan, &SystemRunner) {
        Ok(()) => {
            println!("{done}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}

pub fn run(args: ServiceArgs) -> anyhow::Result<ExitCode> {
    let os = Os::current();
    let env = |key: &str| std::env::var_os(key);
    match args.command {
        ServiceCommand::Install(install) => {
            if !install.config.is_file() {
                anyhow::bail!("cannot find the profile {}", install.config.display());
            }
            let scope = scope(install.user);
            let spec = ServiceSpec {
                exe: std::env::current_exe().context("cannot locate the rurge binary")?,
                // `absolute`, not `canonicalize`: no `\\?\` prefix on Windows
                config: std::path::absolute(&install.config)
                    .with_context(|| format!("cannot resolve {}", install.config.display()))?,
                system_proxy: install.system_proxy,
            };
            let plan = install_plan(os, scope, &spec, &env, uid(os, scope))?;
            let done = match scope {
                Scope::User => "installed: rurge starts automatically (per user)",
                Scope::System => "installed: rurge starts automatically (system-wide)",
            };
            Ok(carry_out(&plan, install.dry_run, done))
        }
        ServiceCommand::Uninstall(uninstall) => {
            let scope = scope(uninstall.user);
            let plan = uninstall_plan(os, scope, &env, uid(os, scope))?;
            Ok(carry_out(&plan, uninstall.dry_run, "uninstalled"))
        }
    }
}
```

（`Os` 与 `Scope` 都派生了 `PartialEq`。`main.rs` 把 `Err` 映射为 `error: …` + 退出 2，`bail!` 的「找不到配置」走这条路。）

`cli/mod.rs` 加 `pub mod service;`；`main.rs`：

```rust
    /// Install or remove the automatic start of rurge
    Service(cli::service::ServiceArgs),
    // …
        Command::Service(args) => cli::service::run(args),
```

- [ ] **Step 4: 运行确认通过**

Run: `cargo build -p rurge && cargo test -p rurge --test cli service::`
Expected: 2 个测试 PASS。macOS 上 `--dry-run` 也会执行只读的 `id -u`。

- [ ] **Step 5: 质量门与提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge
git commit -F - <<'EOF'
feat(cli): rurge service install / uninstall（含 --dry-run）

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th
EOF
```

---

### Task 10: 文档、兼容性清单、手工验收清单与计划收尾

**Files:**
- Create: `docs/acceptance/phase1-manual.md`
- Modify: `README.md`（中英两半）、`CLAUDE.md`、`docs/surge-compatibility-matrix.md`、`docs/api/phase1.md`、`docs/superpowers/specs/2026-09-07-phase1-m4-control-plane-design.md`（追加 `## 15. M4b 实施备注`）、本计划末尾两张表

**Interfaces:**
- Consumes: Task 1–9 最终的行为、输出文案与退出码——**以代码与测试为准**，写之前逐条对照 `crates/rurge-platform/src/{sysproxy,service}`、`crates/rurge/src/cli/{run,sysproxy,service}.rs`、`crates/rurge-api/src/routes/{features,outbound}.rs`。

- [ ] **Step 1: `docs/api/phase1.md`**

- 端点表：`GET /v1/features/{…}` 行改为「`system_proxy` 返回真实状态，其余恒 `{"enabled":false}`」；`POST /v1/features/{name}` 行改为「`system_proxy`：`{"enabled":bool}` → `{}`，失败 500（`{"error":"<原因>"}`，例如不支持的 Linux 桌面会带 `export http_proxy=…` 提示）；其余功能 501」。
- `POST /v1/outbound/global` 行补：proxy 模式下清空 → 400。
- 「启动输出」相关描述补上 `system proxy enabled: …` 一行的位置（`api on` 之后、汇总行之前）。
- 新增一节「系统代理」：地址取值规则（第一个 `http-listen` / `socks5-listen`，通配地址换同族回环）、`skip-proxy` 的转换（丢弃取反项与 `<…>` 记号、去端口；Windows 把 IPv4 CIDR 展开成通配模式、丢弃 IPv6 网段）、`state.json` 里的备份与崩溃恢复、重载后的跟随。

- [ ] **Step 2: README（中英同步）与 CLAUDE.md**

- README 状态引言：M1 ～ M4b 完成；阶段 1 功能齐备，三平台系统代理的手工验收见 `docs/acceptance/phase1-manual.md`；特性表 / 路线图里「系统代理」「服务安装（基础版）」标为已完成；「今天能用什么」的提示补 `rurge run --system-proxy` 与 `rurge service install | uninstall [--user] [--dry-run]`。macOS 一句话注明 `networksetup` 需要管理员账户（标准账户会被拒绝，rurge 原样报出）。
- CLAUDE.md：「当前状态」写 M4b 完成（`rurge_platform::sysproxy` 三后端、`service`、`SystemProxyManager`、`--system-proxy`、`rurge service`）；「先读这些文档」加 M4b 计划与手工验收清单；「工作流约定」或「计划中的架构」补一句：**`unsafe` 只允许出现在 `rurge-platform`（crate 级 `deny` + 单函数 `allow`），其余 crate 仍是 `forbid`**；「常用命令」加 `cargo run -p rurge -- run -c config.conf --system-proxy` 与 `cargo run -p rurge -- service install -c config.conf --user --dry-run`，并注明 `RURGE_SYSTEM_PROXY_BACKEND=file:<path>` 是测试用后端。

- [ ] **Step 3: 兼容性清单（列数不变，用 `grep -n` 定位）**

- `skip-proxy` 行：M4b 已实现；写入规则（同 Step 1）；Windows 的 CIDR 展开与 IPv6 网段丢弃；KDE 的 `NoProxyFor` 原样逗号拼接。
- `exclude-simple-hostnames` 行：状态改 🟡——Windows `<local>` ✅；macOS 经 `networksetup` 无法设置（WARN 并忽略，阶段 6 评估 SystemConfiguration）；GNOME / KDE 无对应项。
- `set-system-socks-proxy` 行：M4b 已实现（为 false 时各平台显式关闭 / 清空 socks 项）。
- `/v1/features/system_proxy` 行：M4b 已生效（读真实状态；失败 500）。
- 「rurge 专有 | 守护进程」行：`--system-proxy`（`RURGE_SYSTEM_PROXY`）与 `rurge service install | uninstall [--user] [--system-proxy] [--dry-run]` 已实现；Windows 用登录时触发的计划任务（🟡，真正的服务在阶段 6），`--user` 在 Windows 上无区别。
- 平台差异（设计 §10）：Linux 系统代理 🟡（GNOME / KDE，其它桌面只给环境变量提示并报错）；macOS 系统代理 🟡（`networksetup`，管理员账户；Q2 未在真机验证，列入手工验收）；Windows 控制台关闭 / 注销 / 关机时也会恢复系统代理（rurge 行为说明）。
- `/v1/outbound/global` 行：补「proxy 模式下清空 → 400」。
- CLI 的 `module` `feature` … 行（阶段 6）：备注补一句——系统代理的运行期 CLI 开关随 `feature` 命令在阶段 6 提供；阶段 1 用启动参数 `--system-proxy` 或 `POST /v1/features/system_proxy`（FR-IN-04 的「CLI」一项由此覆盖到启动参数为止）。

- [ ] **Step 4: `docs/acceptance/phase1-manual.md`（新建）**

阶段 1 验收第 5、6 条的手工清单，按平台分节，每节同一组步骤与「期望」：

1. `rurge run -c <conf> --system-proxy` → 系统设置里能看到 rurge 的地址（Windows：设置 → 网络 → 代理；macOS：`networksetup -getwebproxy <服务>`；GNOME：`gsettings get org.gnome.system.proxy mode`；KDE：`kreadconfig6 --file kioslaverc --group "Proxy Settings" --key ProxyType`）。
2. 浏览器访问一个走 DIRECT 与一个被 REJECT 的站点 → `rurge status` 的活动请求数变化、`GET /v1/requests/recent` 里出现对应记录且规则 / 策略正确。
3. `POST /v1/features/system_proxy {"enabled":false}` → 系统设置回到原值；再开 → 再次指向 rurge。
4. Ctrl-C（Windows 另测：直接关掉终端窗口）→ 系统设置回到原值。
5. 开着系统代理时强杀进程（`taskkill /f` / `kill -9`）→ 系统设置仍指向 rurge；再 `rurge run -c <conf>`（不带 `--system-proxy`）→ 启动日志有一条 WARN，系统设置回到原值，`state.json` 的 `system_proxy_backup` 为 `null`。
6. 改 `skip-proxy` 后 `rurge reload` → 系统的绕过列表随之更新。
7. `rurge service install -c <conf> --user --dry-run` 看计划 → 去掉 `--dry-run` 安装 → 重新登录 / 重启后 rurge 在运行 → `rurge service uninstall --user`。
8. macOS 专项（Q2）：分别用管理员账户与标准账户执行第 1 步，记录是否需要 `sudo`、报错原文。

每条留「结果 / 日期 / 系统版本」三个空栏。

- [ ] **Step 5: 设计文档 `## 15. M4b 实施备注` 与计划收尾**

- §15 列出与 §8 的出入：本计划的 P1–P11（逐条一句话），加上执行期的其它裁定；Q2 的状态（未真机验证，见手工验收清单）。
- 本计划末尾「执行期修正记录」与「延后事项」按 SDD 账本填写。

- [ ] **Step 6: 检查与提交**

逐段对照 README 中英两半；清单被改动的每一行数一遍 `|`；`docs/api/phase1.md` 的每个说法回到代码核对一次。

```bash
git add docs README.md CLAUDE.md
git commit -F - <<'EOF'
docs: M4b 平台：系统代理与服务安装的文档、兼容性清单、手工验收清单与计划收尾

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th
EOF
```

---

## 执行期修正记录

（执行时填写：任务号、与计划的偏差、原因。）

| 任务 | 偏差 | 原因 |
| --- | --- | --- |
| Task 1 | Windows 注册表临时键测试改为自清理：使用前先 `remove_tree`（容忍不存在）并加一个 `Drop` 守卫，不是计划原文那个只在断言全部通过时才清理的版本 | 评审（opus）标为 Important：原测试体在断言失败时会把 `HKCU\Software\rurge-test-<pid>` 留下来，PID 被系统回收复用后下一次跑会在第一条断言就失败，形成除非手工清理否则不会自愈的死循环；裁定改为自清理（test-only，无生产代码风险） |
| Task 4 | 实现者子代理的提交按自己会话的 `Co-Authored-By` 具名（`Claude Sonnet 5`），未使用计划文本里的 `Claude Fable 5.1`；`Claude-Session` 行仍是本次 M4b 会话的 URL | 裁定：subagent 的归属信息以其自身会话的系统提醒为准，优先于计划文本里复制的归属行；不改写历史（本地提交，`Co-Authored-By` 具名不一致但无实质影响） |
| Task 4 | `systemd_quote` 的测试补一条真实反斜杠与转义顺序的断言（`a\"b` → `"a\\\"b"`；Windows 风格路径的每个 `\` 各自翻倍）；`execute()` 在 `create_dir_all` 失败时也像同一函数里 write / remove 的错误那样把目录路径带进错误信息 | 评审（sonnet）标为 2 个 Important、判定为计划已隐含要求：计划原文给的 `systemd_quote` 样例没有反斜杠，顺序敏感的转义替换因此没有测试守住；`create_dir_all` 的错误缺少路径上下文，与同函数里其它错误路径不一致 |
| 附带（非计划任务，Task 4 质量门期间发现） | M4a 遗留的测试 `rurge-engine` 的 `observe::tests::snapshot_bytes_is_consistent_while_sessions_finish_concurrently` 偶发失败（exit 101），提交 `ae9e2e2` 作为分支内的附带修复：读者线程首次读完成后再放行 4 个 finisher 线程开始结束会话，30/30 稳定 | 分析认定锁本身没问题，唯一对调度敏感的断言是 `assert!(reads > 0)`——机器负载高时 4 个 finisher 线程可能在读者线程跑第一次迭代前就把 64 次琐碎操作全部做完并让 `stop` 提前置位；这个 flaky 门禁会拖累后续每个任务的质量门，裁定在本分支内顺手修掉（test-only，仅 1 个文件，随分支一起接受最终评审） |
| Task 5 → Task 8 | 生命周期新增一条设计文档 §8.2 未覆盖的规则：重载后如果没有任何可用监听器而系统代理仍是开启状态，rurge 不会自动关闭系统代理，只打一条 WARN；Task 8 实现与评审阶段把触发条件从「任意重载失败」收紧为「系统代理开着且当前监听器算不出任何可用地址」 | Task 5 裁定：静默关闭代理等于替用户做了一次路由决定，保留旧设置直到下一次成功重载或退出更安全。Task 8 评审（opus）指出原措辞在「重载失败但仍保留旧监听器」的三种情形（配置解析失败、构建 `Runtime` 失败，均发生在换代之前）里会误报——那些情形下地址并未失效；收紧条件更准确 |
| Task 8 | P5（每个由 CLI 测试启动的 `rurge run` 都必须带 `RURGE_SYSTEM_PROXY_BACKEND` 文件后端）收进一个共用的 `guard_system_proxy` 辅助函数，并补齐到 `cli.rs` 里两处原先绕过它的直接调用 | 评审（opus）指出这两处调用虽然当时无害（在触达 `backend()` 之前就已退出），但仍是 Global Constraints 明确禁止的测试安全漏洞；一并补一个系统代理开启失败的端到端测试（不可写的 `file:` 路径 → 退出 1、stderr 有消息、状态回滚），并把等待逻辑改成先等 `system proxy restored` 这一行再等进程退出，避免读取线程滞后于 `try_wait` |
| Task 7 | `/v1/test/dns_delay` 分两次快照分别读 `internet_test_url()` 与 `resolver()`（P11 落地时新增的两个引擎视图），评审指出重载恰好夹在两次读之间时可能用旧的默认域名去配一个新的 resolver | 裁定维持现状：两代之间不存在「域名必须与 resolver 同代」这种不变量（只是一个诊断端点，两次读之间没有 `await`，任何一代的默认域名对任何一代的 resolver 都是合法输入）；把两次读合并成一个视图会让 `rurge-api` 重新依赖 `Runtime` 的内部形状，这正是 P11 要解除的耦合 |
| 最终评审修复波 A | `SystemProxyManager` 的创建与 `recover()` 从「绑定监听器与 `http-api` 之后」移到「`StateStore::open` 之后、任何绑定之前」，并补一个端到端用例：崩溃后端口被占，`rurge run` 仍退出 1，但备份已还原、`state.json` 的 `system_proxy_backup` 为 `null` | 全分支评审（opus）标为 Important：上一次崩溃留下的端口往往正是本次绑不上的原因，原顺序下备份永远得不到恢复，机器一直指着一个死端口，且每次重试都卡在同一处。裁定：把崩溃前的设置放回去与本次能否启动无关，因此恢复必须排在一切会提前退出的步骤之前 |
| 最终评审修复波 B | `install_plan(Os::Unix, Scope::System, …, system_proxy = true)` 直接拒绝；`--user` + `--system-proxy` 的 unit 改挂 `graphical-session.target`（`PartOf=` / `After=` / `WantedBy=`）；所有 systemd unit 加 `StartLimitIntervalSec=60` / `StartLimitBurst=5` | 全分支评审（opus）标为 Important：system unit 以 root 运行、没有桌面会话，Linux 后端每次都会 `Unsupported`，配合 `Restart=on-failure` 就是每 3 秒一次的无限重启；`--user` 单元原先挂 `default.target`，可能在图形会话导出 `XDG_CURRENT_DESKTOP` / D-Bus 地址之前就启动。全部判断留在 `rurge-platform` 内（AR-02） |
| 最终评审修复波 C | `LinuxBackup` 增加 `#[serde(default)] kde_write`（只在 KDE 快照时填写）；恢复 `"kde"` 备份按「当前探测到的 → 备份记录的 → `kwriteconfig6`」取写工具，不再提前返回 `NotFound` | 全分支评审（opus）标为 Important：从 TTY、裁剪过的 `PATH` 或换过桌面的会话里恢复时 `self.kde()?` 会失败，备份就永远留在 `state.json` 里，每次启动只多打一条 ERROR。命令仍走 `run_best_effort`，工具真的缺失时报的是逐条命令的错误并带上工具名 |
| 最终评审修复波 D | CLI 测试新增唯一的 `rurge_run(conf, data)` 构造函数（P5 护栏内置），`guard_system_proxy` 并入其中；`spawn_daemon_full` 与三个直接调用点全部改走它，`"run"` 字面量只剩构造函数一处 | 全分支评审（opus）标为 Important：P5 原先靠「每个调用点都记得调用 `guard_system_proxy`」，将来新增一个带 `--system-proxy` 却忘了护栏的用例会直接写开发机的真实注册表。裁定：改成构造即安全，不留第二种构造 `run` 命令的方式 |
| 最终评审修复波 E | `carry_out` 改收 `&dyn CommandRunner`（两个调用点传 `&SystemRunner`），补 dry-run / 真实执行 / 命令失败三个单元测试；`rurge service … --user` 在 `sudo` 下（uid 0）由纯函数 `check_user_scope` 拒绝 | 全分支评审（opus）标为 Important 并在分诊里判为「合并前唯一必须修的延后项」：此前唯一挡在测试与真实 `schtasks /create` / `systemctl enable` 之间的只有一处 `if dry_run`，没有任何断言守住它（本波已用变异验证：把守卫改成恒假，dry-run 用例立即失败）。`--user` 的 sudo 拒绝是同一次评审的 minor（launchd 的 `gui/0` 是 root 的会话） |
| 最终评审修复波 F | `proxy_override_value`：含字面 `;` 的条目丢弃并打 debug 日志；`<local>` 追加前先去重 | 全分支评审（opus）把 Task 1 延后的这条 minor 判为「现在顺手修掉」：一个含 `;` 的 `skip-proxy` 条目会被 Windows 拆成两条模式，语义与用户写的不一致 |

## 延后事项

（执行时填写：审查中发现但不在 M4b 范围内的问题，带去向——阶段 2 / 阶段 6。）

去向的四个桶：**本分支最终评审**（留给合并前的全分支评审判断是否要修）、**阶段 2**、**阶段 6**、**手工验收**（`docs/acceptance/phase1-manual.md`）。已经在 Task 10 里直接处理完（写进设计文档 §15 或兼容性清单）的项目单独标注，不再是待办。

「本分支最终评审」这个桶已经关闭：全分支评审已经完成并对每一条做了分诊，结论与改写后的去向见本节末尾的「全分支最终评审（2026-09-18）的分诊结论」；仍写着「去向：本分支最终评审」而没有被单独标注的条目，一律按那一段改读为**阶段 6**。

**Task 1（`rurge_platform::sysproxy` 核心类型与 Windows 后端）**

- `FakeRegistry`（测试替身）把值类型不对的情形映射成"不存在"，而 `RealRegistry` 会返回一个错误（`ERROR_INVALID_DATA`）——例如 `ProxyEnable` 实际是 `REG_SZ` 时，`snapshot()` 会报错退出而不是当成缺失处理。去向：已在设计文档 §15「已知限制」与兼容性清单 `/v1/features/system_proxy` 行登记，不再是待办。
- `restore()` 把任何反序列化失败都统一归类成"wrong platform"，掩盖了真正的错误原因（消息质量）。去向：本分支最终评审。
- `delete()` 在键不存在时会先用 `create()` 打开（进而创建该键）再删值。去向：本分支最终评审。
- 注册表相关的 `io::Error` 一律是 `ErrorKind::Uncategorized`（HRESULT 被当成原始 os error），调用方无法按 `kind()` 分支处理。去向：本分支最终评审。
- CIDR 前缀展开表只测试了部分前缀长度（缺 `/1`–`/7`、`/17`–`/23` 与主机位非零的情形；人工验算结果正确，但没有落成用例）。去向：本分支最终评审。
- 单条 `/1`、`/9`、`/17` 或 `/25` 的 `skip-proxy` 网段会展开成 128 条 `ProxyOverride` 通配模式，没有上限。去向：已在兼容性清单 `skip-proxy` 行与 `docs/api/phase1.md` 登记，不再是待办。
- `<local>` 追加时未去重；条目内如果出现字面 `;` 会把 `ProxyOverride` 意外拆成多段。去向：**已在最终评审修复波 F 修复**，不再是待办。
- `NOT_FOUND` 常量的 `#[cfg(windows)]` 写在文档注释之前，与 `RealRegistry` 的顺序不一致（纯风格问题）。去向：本分支最终评审。
- `apply()` 不清除也不备份 `AutoConfigURL`（PAC）。去向：已在兼容性清单与设计文档 §15 登记为已知限制，不再是待办。

**Task 2（命令执行抽象与 macOS 后端）**

- `restore()` 里显式的 `platform != "macos"` 检查只有在 `services` 字段存在时才可达，现有测试走的是反序列化失败（`map_err`）那条路径。去向：本分支最终评审。
- `networksetup` 输出解析的测试缺少"内部/尾部空行"的用例。去向：本分支最终评审。
- `apply_commands` 没有 IPv6 地址的用例。去向：本分支最终评审。
- 没有测试验证 `exclude_simple = true` 时其余设置仍会正常应用（只是 WARN 并跳过这一项）。去向：本分支最终评审。
- `SystemRunner` 在 stderr 为空、回退用 stdout 拼错误信息的分支未测（测试不跑真实工具）。去向：本分支最终评审。
- macOS 真机上 `networksetup` 的真实输出与权限要求无法离线验证。去向：手工验收（`docs/acceptance/phase1-manual.md` 第 1、8 条）。

**Task 3（Linux 系统代理后端与 `platform()`）**

- GNOME 空数组 `@as []` 的断言只按整行拼接字符串比较，没有按参数下标断言（拆分 argv 的话测试仍会通过）。去向：本分支最终评审。
- `tool_on_path` 只看 `PATH` 上是否有同名文件，不检查可执行位。去向：本分支最终评审。
- `env_hint` 在 http/https/bypass 都为空时返回 `"export "`（尾部带空格但没有变量）。去向：本分支最终评审。
- 损坏的 Linux 备份统一报"wrong platform"，其中显式的桌面平台检查分支未测。去向：本分支最终评审。
- 测试 `kde_apply_writes_kioslaverc_and_tells_kio` 里对 dbus 调用的断言写在了函数末尾而不是紧邻处，命名与断言位置不完全对应（测试卫生）。去向：本分支最终评审。
- 测试覆盖缺口：GNOME 下 IPv6 主机、KDE 在 restore 时找不到工具、`XDG_CURRENT_DESKTOP` 大小写变体。去向：其中「KDE 在 restore 时找不到工具」**已在最终评审修复波 C 修复并补上用例**（连同行为本身）；另外两条去向阶段 6。
- `platform()` 对非 windows / 非 unix 目标没有 `compile_error!`，会在不支持的目标上给出较难懂的编译错误而不是明确提示。去向：本分支最终评审。

**Task 4（`rurge service` 安装 / 卸载计划与执行器）**

- `--system-proxy` 被省略时的行为只在 systemd 分支断言，没有对 launchd / schtasks 分别断言。去向：本分支最终评审。
- `execute()` 卸载测试里"文件本来就不存在"的分支没有单独区分（该测试的 `first_error` 已经被前面失败的命令占了），需要一个独立的、只测"文件缺失也算成功"的用例。去向：本分支最终评审。
- `launchd_domain` 在 `Scope::System` 且传了 `Some(uid)` 的组合没有测试（虽然此时实现会忽略 `uid`）。去向：本分支最终评审。

**Task 5（`SystemProxyManager` 生命周期、`proxy_settings` 与文件后端）**

- `apply` 失败且回滚 `restore` 也失败的双重失败路径：内存态 `applied`/`flag` 变回 false，但 `state.json` 的 `features.system_proxy` 仍是 true（下次 `disable`/`recover` 会自愈）；这条分支没有测试，代码里也没写注释说明。去向：本分支最终评审。
- `StateStore::update` 只记录写失败、不做 `fsync`，"开启前先把备份落盘"只是尽力而为，不保证掉电场景。去向：已写入设计文档 §15「已知限制」与 `docs/api/phase1.md` 系统代理一节，不再是待办。
- 被丢弃的取反 / `<…>` 形式的 `skip-proxy` 条目没有留日志（哪怕是 debug 级别）。去向：本分支最终评审。
- 从未开启过的 manager 调用一次 `disable()` 仍会写一次 `state.json`（无害但可以省掉）。去向：本分支最终评审。
- `FileBackend` 的测试没有断言 `https` / `exclude_simple` 字段；`bypass_list` 没有 IPv6 CIDR 的用例；`describe()` 单一 kind（只 http 或只 socks）的形式没有测试。去向：本分支最终评审。

**Task 6（`/v1/features/system_proxy` 接通 `Control`）**

- `LoopControl` 里临时的 `system_proxy_enabled` / `set_system_proxy` 桩实现没有留"等 Task 8 替换"的注释。去向：已随 Task 8 替换为真实实现而失效，不再是待办。

**Task 8（`rurge run --system-proxy` 集成）**

- `applied().map(describe).unwrap_or_default()` 有一个实际不可达的空字符串分支。去向：本分支最终评审。
- `cfg` 风格不统一：`ShutdownSignals` 用 `#[cfg(windows)]`，别处的打印用的是 `#[cfg(not(unix))]`。去向：本分支最终评审。
- 恢复失败时打印到 stderr 的那条消息没有测试覆盖。去向：本分支最终评审。
- 系统代理开启失败的端到端测试只在子进程退出之后才读取它的 stdout/stderr 管道（有 20 秒强杀兜底，输出量本来就只有几行，风险不大）。去向：本分支最终评审。

**Task 9（`rurge service install / uninstall` CLI）**

- dry-run 的安全性只靠一处 `if dry_run` 判断把关，没有"注入假 runner 后断言真实命令未被调用"的测试；如果这处判断回归，会在测试的 trailer 断言失败之前先真的执行一次系统命令。去向：**已在最终评审修复波 E 修复**（`carry_out` 收 `&dyn CommandRunner`，三个单元测试），不再是待办。
- 非 dry-run 的成功 / 执行失败提示文本（`installed: …`、`uninstalled`、失败时的 `error: …`）没有自动化测试，因为执行真实的 `systemctl` / `launchctl` / `schtasks` 违反测试规则。去向：修复波 E 之后，这三条文本已能用注入的假 runner 覆盖成功 / 失败两条路径；真实工具的行为仍归手工验收（`docs/acceptance/phase1-manual.md` 第 7 条）。
- 相对路径的 `-c` 没有自动化测试。去向：阶段 6。

**全分支最终评审（2026-09-18）的分诊结论**

最终评审对上面所有去向为「本分支最终评审」的条目做了一次分诊：**只有 Task 9 的 dry-run 守卫必须在合并前修**（连同评审自己发现的 5 个 Important 与 3 个「顺手修掉」的 minor，见「执行期修正记录」表里的修复波 A–F）；其余一律保持延后，去向按下表改写为**阶段 6**（代码质量与测试覆盖的集中整理）或**手工验收**。评审自己发现、修复波未处理的剩余 minor：

- `restore()` 把任何反序列化失败都归类成"wrong platform"，掩盖真正的原因（Windows / macOS / Linux 三个后端都是）。去向：阶段 6。
- Windows 的 `delete()` 在键不存在时会先 `create()` 打开（进而创建该键）再删值。去向：阶段 6。
- GNOME 的 `restore` 跳过值为空串的键（`filter_map` + `!value.is_empty()`）：原本就是空的键不会被显式写回，依赖 `mode` 最后一条把整体拨回原状态。去向：阶段 6。
- `env_hint` 在 http / https / bypass 全空时返回 `"export "`（尾部带空格、没有变量）。去向：阶段 6。
- `tool_on_path` 只看 `PATH` 上是否有同名文件，不检查可执行位。去向：阶段 6。
- `platform()` 对非 windows / 非 unix 目标没有 `compile_error!`；`cfg` 风格不统一（`ShutdownSignals` 用 `#[cfg(windows)]`，打印用 `#[cfg(not(unix))]`）。去向：阶段 6。
- `service uninstall` 之后没有 `systemctl daemon-reload`，systemd 会留下一条"unit 文件已消失"的告警直到下次 reload。去向：阶段 6。
- `execute()` 用 `plan.remove` 是否为空来区分"安装（首个命令失败就停）"与"卸载（继续做完再报第一个错）"，是隐式约定而不是显式参数。去向：阶段 6。
- `StateStore` 读到损坏的 `state.json` 时会改名成 `state.json.broken`——这个文件里可能正好存着用户的原始系统代理设置，而新的 `state.json` 是空的，恢复就丢了线索（日志提到了该文件，但没有说明它可能含有待恢复的备份）。去向：阶段 6。
- 双重失败（`apply` 失败且回滚的 `restore` 也失败）时 `state.json` 的 `features.system_proxy` 仍是 `true`，而内存里的 `applied` / `flag` 已是 `false`。补充事实：`features.system_proxy` 这个字段目前**没有任何读取方**（API 的 `GET /v1/features/system_proxy` 读的是 `Control::system_proxy_enabled`，即内存里的 flag；崩溃恢复看的是 `system_proxy_backup`），所以这条不一致目前不可观测，下一次 `disable()` / `recover()` 也会自愈。去向：阶段 6。
- Windows 的 `ProxyServer` 里 `socks=<addr>` 一段是否会被基于 WinINet 的客户端当成 SOCKS4（rurge 的入站只讲 SOCKS5）。去向：手工验收（`docs/acceptance/phase1-manual.md` Windows 第 9 条）。
- macOS 把 IPv6 字面量（如 `::1`）交给 `networksetup -setwebproxy` 是否被接受，没有真机验证过。去向：手工验收（`docs/acceptance/phase1-manual.md` macOS 第 1 条的可选检查）。

