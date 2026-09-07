# 阶段 1 / M4「控制面与平台」设计

> 状态：已与项目所有者逐节确认（2026-09-07）。本文件是 M4a / M4b 两份实施计划的依据；与阶段 1 骨架设计（`2026-09-03-phase1-core-skeleton-design.md` 第 9 ～ 11 节）不一致处以本文件为准，并在第 10 节登记。

## 1. 目标与范围

### 1.1 阶段目标

- 给运行中的 `rurge run` 一个 Surge 兼容的控制面：`http-api` 指定的地址上提供阶段 1 的端点子集，`rurge reload | stop | status` 作为它的客户端；运行期可改的状态（出站模式、全局策略、日志级别、功能开关）落到 `state.json`，重启后保持。
- 让 rurge 能成为桌面的系统代理：三平台开关、退出与崩溃后恢复、`skip-proxy` 语义；并提供基础的开机自启安装。
- 建立阶段 2 / 6 的插入点：策略组 `select`、Dashboard、`/v1/metrics`、多 Profile、`security ban` 都只在这一层扩展，不再动引擎边界。

### 1.2 范围内（PRD 编号）

| 编号 | 内容 | 子里程碑 |
| --- | --- | --- |
| FR-OBS-02（骨架） | `http-api` 监听、`X-Key` / `?x-key=` 鉴权、错误鉴权计数与封禁、未配置不监听、统一错误体 | M4a |
| FR-OBS-04 | `rurge reload` / `stop`（`status` 为 rurge 专有），经 HTTP API 操作实例 | M4a |
| FR-OBS-05（基础） | `GET /v1/traffic`（总计、按策略、按监听器、实时速率） | M4a |
| FR-OBS-10 | `/v1/requests/*` 暴露会话 id，与日志字段一致 | M4a |
| FR-HTTP-10（基础） | `GET /v1/requests/recent` / `active`、`POST /v1/requests/kill` | M4a |
| FR-CFG-16 | API / CLI 触发热重载（SIGHUP / `--watch` 已在 M3b） | M4a |
| FR-DNS-11（API 部分） | `GET /v1/dns`、`POST /v1/dns/flush`、`POST /v1/test/dns_delay` | M4a |
| 状态持久化（PRD 3.5） | `state.json` 写入：出站模式、全局策略、功能开关、系统代理备份、当前 Profile | M4a（备份：M4b） |
| FR-IN-04 | 系统代理：`--system-proxy` / API / CLI 开关；三平台设置；`skip-proxy` `exclude-simple-hostnames` `set-system-socks-proxy`；退出或崩溃后恢复 | M4b |
| FR-OBS-09（基础） | `rurge service install / uninstall` | M4b |

### 1.3 范围外

多 Profile 目录、`GET /v1/profiles` 列表与 `POST /v1/profiles/switch`（阶段 6）；`/v1/policies/detail`、`/v1/policies/test`、`/v1/policy_groups*`（阶段 2，随代理协议与组算法）；`/v1/metrics`、Web Dashboard、`security ban` 命令、`rurge shell`、`--json` 之外的其余 surge-cli 同名命令（阶段 6）；`http-api-tls`（阶段 4，需 MITM CA）；`/v1/features/{mitm,capture,rewrite,scripting,enhanced_mode}` 只返回关闭、POST 回 501；真正的 Windows 服务（阶段 6）。

### 1.4 子里程碑

| 子里程碑 | 内容 | 验收产出 |
| --- | --- | --- |
| M4a 控制面 | `rurge-api`（axum、鉴权、封禁、阶段 1 端点）、引擎运行期覆盖与 `StateStore`、`Control` trait 与命令通道、`POST /v1/log/level`、`rurge reload / stop / status`、`docs/api/phase1.md` | API 集成测试逐端点通过；CLI 端到端：`status` / `reload` / `stop`；重启后出站模式保持 |
| M4b 平台 | `rurge_platform::sysproxy` 三平台实现、`--system-proxy`、`/v1/features/system_proxy`、备份 / 恢复 / 崩溃恢复、`rurge service install / uninstall` | mock 生命周期测试；手工验收：三平台开系统代理后浏览器流量进入请求记录并按规则分流 |

每个子里程碑一份实施计划，独立分支、独立验收。

## 2. 已确认的技术决策

| 编号 | 决策 | 理由 |
| --- | --- | --- |
| D1 | API 用 axum（`json` `query` `tokio` `http1` features），不手写 hyper 路由 | 20 个端点的路由 / 抽取 / 中间件现成，阶段 6 的 Dashboard 静态文件与更多端点直接复用；与已有 hyper 1 / tower-service 同生态 |
| D2 | `rurge reload / stop / status` 依赖 `http-api`；未配置时退出 2 并提示 SIGHUP / `--watch` / Ctrl-C | 与 Surge 的 surge-cli 一致，守 PRD「未配置 `http-api` 时不监听」 |
| D3 | Profile 端点只做 `current` / `reload` / `check` | 配置目录与切换属阶段 6；`rurge run` 仍是 `-c <单个文件>` |
| D4 | 架构 A：引擎持运行期可变状态（`ArcSwap`），`StateStore` 是 `state.json` 唯一写入者，需要主循环配合的动作经命令通道；`rurge-api` 只依赖 `rurge-engine` | 改动面最小、边界清楚；将来要把 reload / stop 下沉进引擎（方案 C）可在此基础上逐步做 |
| D5 | 出站模式启动优先级：显式 `--outbound-mode` > `state.json` > `rule`；显式值写回 `state.json` | 模式跨重启保持（Surge 行为），命令行仍能一次性覆盖并成为新的持久值 |
| D6 | 手册未定义的 JSON 结构采用暂定结构并写进 `docs/api/phase1.md`，阶段 6 用真实 Surge 校准（沿用骨架 D7） | |
| D7 | 鉴权失败封禁：同一来源 IP 10 分钟内 5 次失败 → 后续 10 分钟一律 403；`security ban` 的查看 / 解除接口留阶段 6 | 骨架 §10.1 的约定；有界表沿用 M3a `warn_due` 模式 |
| D8 | Windows 开机自启用计划任务（`schtasks`），真正的服务控制握手留阶段 6 | 服务主循环（`StartServiceCtrlDispatcher`）超出基础版 |

## 3. crate 边界与依赖方向

```
rurge (bin: run / check / rule / dns / reload / stop / status / service)
  ├─ rurge-api ──▶ rurge-engine（只读访问器 + Control trait 对象）、rurge-config（类型）
  ├─ rurge-engine（新增：outbound_mode / global_policy 运行期覆盖、StateStore、Control trait、只读视图）
  └─ rurge-platform（M4b 新增 sysproxy / service 模块；只被 bin 使用）
```

- `rurge-api` 不依赖 `rurge-platform`、不认识 bin；系统代理开关经同一个 `Control` trait 的方法暴露，实现在 bin（它持有平台适配与 `StateStore`）。
- `rurge-engine` 不依赖 `rurge-platform`（AR-02 不变）。
- 新依赖：`axum`（M4a）、`winreg` + `windows-sys`（M4b，仅 `cfg(windows)`）。CLI 客户端复用 `hyper-util` 的 legacy client（不引入 reqwest；测试里也用它）。

## 4. 引擎的运行期可变状态与 `StateStore`

### 4.1 运行期覆盖

```rust
pub struct Engine {
    // 既有字段 …
    outbound_mode: ArcSwap<OutboundMode>,
    global_policy: ArcSwap<Option<String>>,
}
impl Engine {
    pub fn outbound_mode(&self) -> OutboundMode;
    pub fn set_outbound_mode(&self, mode: OutboundMode);
    pub fn global_policy(&self) -> Option<String>;
    /// 校验策略存在（含内置与别名）；不存在 → Err(unknown policy)
    pub fn set_global_policy(&self, name: &str) -> Result<(), UnknownPolicy>;
}
```

- `dial` / `dial_internal` 改读 `self.outbound_mode()`：`Direct` → DIRECT；`Proxy` → `global_policy` 指向的策略（`Proxy` 模式不再携带策略名，`OutboundMode::Proxy(p)` 的 `p` 只在 CLI 解析 `proxy=<name>` 时用来同时设置 `global_policy`）；`Rule` → 规则引擎。`Proxy` 模式而 `global_policy` 为空或指向已不存在的策略（重载后可能发生）→ 按 `Rule` 处理并 WARN 一次（Q3）。
- `Runtime.outbound_mode` 退化为启动初值；`swap_runtime` 不触碰这两个覆盖——用户经 API 改过的模式在重载后保持（AR-04 约束的是配置对象，不是运行态）。
- 两处 dial 共享的「模式 / 规则 → 策略」判定抽成一个私有 `resolve_policy(&self, rt, handle) -> Result<PolicyRef, DialFailure>`，顺带消掉 M3b 延后的 `dial` / `dial_internal` 重复。

### 4.2 `StateStore`

```rust
pub struct StateStore { path: PathBuf, state: tokio::sync::Mutex<State> }
impl StateStore {
    /// 缺失 = 默认；损坏 = WARN + 默认，并把坏文件改名 `state.json.broken`
    pub async fn open(path: PathBuf) -> (Arc<StateStore>, State);
    pub async fn snapshot(&self) -> State;
    /// 改内存副本后原子写盘（`spawn_blocking`：写 `state.json.tmp` → rename）；写失败只记 ERROR
    pub async fn update(&self, f: impl FnOnce(&mut State)) -> State;
}
```

- schema 保持 v1（`version` `outbound_mode` `global_policy` `features` `group_selections` `system_proxy_backup` `current_profile`）。`outbound_mode` 存 `"direct" | "proxy" | "rule"`；`global_policy` 存策略名；`features.system_proxy` 由 M4b 写；`group_selections` 本阶段只读（阶段 2 的 `select` 端点写）；`current_profile` 启动时写为 `profile_key`。
- 启动合并（`rurge run`）：`--outbound-mode` 改为 `Option<OutboundMode>`（`proxy=<name>` 同时给出全局策略）；显式给出 → 采用并 `update` 写回；否则取 `state.json`；再无 → `rule`。`global_policy` 只有 `state.json` 一个来源（或 `proxy=<name>`）。
- `State::load`（M3a 的同步读取）由 `StateStore::open` 取代；`selections_for` / `profile_key` 保留。

### 4.3 引擎暴露给 API 的只读视图

`policy_names()`（内置 + 配置策略 + 组名）、`rules()`（原文、index、命中数）、`request_log()`、`traffic()`、`resolver()`（缓存快照 / flush / `measure_delay`）、`config_text(sensitive: bool)`（读当前主配置文件；`sensitive = false` 时脱敏：`password=` 与 `password @` 的值、`http-api` / `external-controller-access` 的 `key@` 前缀、`ca-passphrase`、`ca-p12` 的值一律替换为 `***`）。

## 5. `rurge-api`

### 5.1 服务

```rust
pub struct ApiContext { pub engine: Arc<Engine>, pub control: Arc<dyn Control> }
pub async fn serve(addr: SocketAddr, key: String, ctx: ApiContext, shutdown: CancellationToken) -> io::Result<JoinHandle<()>>;
```

- 只在 `[General] http-api` 存在时调用；绑定失败与监听器同等对待（`rurge run` 退出 1，`cannot bind http-api`）。任务挂在引擎的 `TaskTracker` 上，axum `with_graceful_shutdown(shutdown.cancelled())`，随优雅退出一起结束。绑定非回环地址时启动打印一条 WARN（Q4）。
- 鉴权中间件：`X-Key` 头或 `?x-key=`；常量时间比较；失败 `401 {"error":"unauthorized"}`；封禁表按来源 IP：10 分钟窗口内第 5 次失败起封 10 分钟，被封期间一律 `403 {"error":"banned"}`；表上限 1024 条、满时先清过期（M3a `warn_due` 模式）。
- 统一错误体 `{"error":"<英文>"}`：400 参数错误（含 JSON 解析失败）、401 / 403 鉴权、404 未知路径或对象不存在、501 未实现、500 内部错误；成功且无内容时返回 `{}`。所有响应 `Content-Type: application/json`。
- JSON 字段用 camelCase；手册已定义的字段（`mode` `policy` `enabled` `level`）照抄。

### 5.2 端点（阶段 1）

| 端点 | 行为 |
| --- | --- |
| `GET /v1/outbound` | `{"mode":"direct"\|"proxy"\|"rule"}` |
| `POST /v1/outbound` | 同上；校验后 `set_outbound_mode` 并持久化；`proxy` 且无全局策略 → 400 `global policy not set` |
| `GET /v1/outbound/global` | `{"policy":"<name>"}`（未设 → `{"policy":""}`） |
| `POST /v1/outbound/global` | `{"policy":"<name>"}`；策略不存在 → 400；成功持久化 |
| `GET /v1/policies` | 暂定 `{"proxies":["DIRECT","REJECT",…配置策略],"policy-groups":[…组名]}` |
| `GET /v1/rules` | 暂定 `{"rules":[{"index":n,"rule":"<原文>","hits":n}]}`（含 FINAL） |
| `GET /v1/requests/recent` `GET /v1/requests/active` | 暂定 `{"requests":[{"id","listener","src","dst","rule","policy":[…链],"sni","protocol","up","down","startedMs","elapsedMs","status":"active"\|"completed"\|"rejected"\|"failed","rejectKind","error"}]}`；`recent` 最多返回 `?limit=`（默认 100，上限为环形缓冲容量） |
| `POST /v1/requests/kill` | `{"id":n}` → `Engine::kill`；不是活动会话 → 404 |
| `GET /v1/traffic` | 暂定 `{"startTime":ms,"total":{"in","out","inCurrentSpeed","outCurrentSpeed"},"connector":{"<policy>":{"in","out"}},"listener":{"http":{"in","out"},"socks5":{"in","out"}}}`；`in` = 下行（`down`）、`out` = 上行（`up`），客户端视角；`startTime` 为进程启动的 Unix 毫秒 |
| `GET /v1/dns` | 暂定 `{"dnsCache":[{"domain","data":[ip…],"expiresTime":ms,"server"}],"upstreams":[…名字],"bootstrap":[…]}` |
| `POST /v1/dns/flush` | `Resolver::flush` → `{}` |
| `POST /v1/test/dns_delay` | body 可选 `{"name":"…"}`，默认 `internet-test-url` 的主机名；`measure_delay` → `{"delays":[{"upstream","ms"}]}`（失败的上游 `ms` 为 null） |
| `GET /v1/profiles/current?sensitive=0\|1` | `text/plain` 的配置文本；默认 0 = 脱敏 |
| `POST /v1/profiles/reload` | `Control::reload` → `{"ok":bool,"errors":n,"warnings":n,"listenersRebound":bool}`；失败为 200 + `ok:false`（运行配置未变） |
| `POST /v1/profiles/check` | 重新加载当前文件但不换代 → `{"ok":bool,"errors":n,"warnings":n,"diagnostics":[与 rurge check --json 相同的对象]}` |
| `POST /v1/log/level` | `{"level":"verbose"\|"debug"\|"info"\|"notify"\|"warning"\|"error"}` → `Control::set_log_level`（映射见骨架 §12） |
| `GET /v1/features/{system_proxy,mitm,capture,rewrite,scripting,enhanced_mode}` | `{"enabled":bool}`（M4a 全 false；M4b 的 `system_proxy` 读 `state.features`） |
| `POST /v1/features/system_proxy` | `{"enabled":bool}` → `Control::set_system_proxy`（M4a 回 501；M4b 接入） |
| `POST /v1/features/{mitm,capture,rewrite,scripting,enhanced_mode}` | 501 `{"error":"not implemented in this version"}` |
| `GET /v1/modules` `GET /v1/scripting` `GET /v1/events` | `{"modules":[]}` / `{"scripts":[]}` / `{"events":[]}` |
| `POST /v1/stop` | 回 `{}` 后 `Control::stop`（优雅退出） |

`docs/api/phase1.md` 记录每个暂定结构的示例响应，并标注「阶段 6 校准」。

## 6. `Control` trait 与 `rurge run` 主循环

```rust
pub struct ReloadReport { pub ok: bool, pub errors: usize, pub warnings: usize, pub listeners_rebound: bool }
pub trait Control: Send + Sync {
    fn reload(&self) -> BoxFuture<'_, ReloadReport>;
    fn stop(&self) -> BoxFuture<'_, ()>;
    fn set_log_level(&self, level: LevelFilter) -> Result<(), String>;
    fn set_system_proxy(&self, enabled: bool) -> BoxFuture<'_, Result<(), String>>;   // M4a 实现返回 Err("not implemented")
}
```

- bin 的 `LoopControl` 持 `mpsc::Sender<Command>`（`Command::Reload(oneshot::Sender<ReloadReport>)`、`Command::Stop`、M4b `Command::SystemProxy(bool, oneshot)`）与日志级别的 `reload::Handle`。主循环的 `select!` 多一个 `rx.recv()` 分支：`Reload` 与 SIGHUP / `--watch` 共用 M3b 的 `reload` 函数（改为返回 `ReloadReport`）；`Stop` 走现有 `break` → 优雅退出（API 任务在 tracker 上随之停）。
- `set_log_level` 不经主循环：`init_logging` 用 `tracing_subscriber::reload::Layer` 包住 `LevelFilter`，`Handle::modify` 直接改。
- 启动顺序：加载配置 → `StateStore::open` → 合并出站模式 / 全局策略 → `Engine::new` + 覆盖 → 绑定监听器 → （M4b：若 `features.system_proxy` 或 `--system-proxy`，先恢复残留备份再应用）→ 配置了 `http-api` 则启动 API → 打印 `listening on …`、`api on http://<addr>`（不打印密钥）→ 主循环。
- 优雅退出顺序：停止接受 → 关 API → （M4b：恢复系统代理）→ 排空会话 → 写 `state.json`（最后一次 `update` 已落盘，此处无额外写）→ 退出。

## 7. CLI 客户端命令（M4a）

```
rurge reload [-c <conf>] [--remote <host:port>] [--key <key>] [--platform <p>]
rurge stop   [同上]
rurge status [同上] [--json]
```

- 地址 / 密钥解析：`--remote` + `--key`（`--key` 亦可来自 `RURGE_API_KEY`）→ 否则 `-c <conf>` 的 `[General] http-api`（只解析配置，不建引擎；`--platform` 与 `check` 相同）→ 都没有 → 退出 2：`http-api is not configured; add "http-api = <key>@127.0.0.1:6171" to [General] or pass --remote/--key (reload can also be triggered by SIGHUP or --watch)`。`http-api` 绑定 `0.0.0.0` 时客户端连 `127.0.0.1`。
- 传输：`hyper-util` legacy client、纯 HTTP、5 s 超时；`stop` 允许「已回 `{}` 后连接被对端关闭」视为成功。退出码：2 = 参数 / 鉴权错误（打印服务端错误体）；1 = 连不上（`cannot reach rurge at http://…`）或非 2xx；0 = 成功。
- `status` 组合 `GET /v1/outbound`、`/v1/outbound/global`、`/v1/policies`、`/v1/rules`、`/v1/requests/active`、`/v1/traffic`：

```
rurge at http://127.0.0.1:6171
mode: rule (global policy: none)
policies: 5   rules: 120   active requests: 3
traffic: in 12.3 MiB, out 1.1 MiB (in 8.2 KiB/s, out 0.6 KiB/s)
```

`--json` 输出 `{"outbound":…,"global":…,"policies":…,"rules":…,"requests":…,"traffic":…}`。

- `reload` 打印 `reloaded: 0 error(s), 2 warning(s)`；失败 `reload failed: N error(s); the running configuration is unchanged`，退出 1。

## 8. M4b 平台：系统代理与服务安装

### 8.1 `rurge_platform::sysproxy`

```rust
pub struct ProxySettings { pub http: Option<SocketAddr>, pub https: Option<SocketAddr>, pub socks: Option<SocketAddr>, pub bypass: Vec<String>, pub exclude_simple: bool }
#[derive(Serialize, Deserialize)] pub struct Backup(serde_json::Value);   // 平台各自的原值
pub trait SystemProxy: Send + Sync {
    fn snapshot(&self) -> io::Result<Backup>;
    fn apply(&self, settings: &ProxySettings) -> io::Result<()>;
    fn restore(&self, backup: &Backup) -> io::Result<()>;
}
pub fn platform() -> Box<dyn SystemProxy>;
```

| 平台 | 设置 | 备注 |
| --- | --- | --- |
| Windows | `HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings`：`ProxyEnable=1`、`ProxyServer=http=h:p;https=h:p[;socks=h:p]`、`ProxyOverride=<skip-proxy 条目;>[<local>]`；随后 `InternetSetOption(INTERNET_OPTION_SETTINGS_CHANGED)` + `(INTERNET_OPTION_REFRESH)` | `winreg` + `windows-sys`；备份三个值 |
| macOS | `networksetup -listallnetworkservices` 取启用的服务，逐个 `-setwebproxy` / `-setsecurewebproxy` / `-setsocksfirewallproxy`（关闭用 `-set…state off`）与 `-setproxybypassdomains` | 可能需管理员权限（Q2）；备份每个服务的 `-getwebproxy` 等输出解析结果 |
| Linux | GNOME（`gsettings` 可用且 `XDG_CURRENT_DESKTOP` 含 GNOME）：`org.gnome.system.proxy mode manual` + `http/https/socks host/port` + `ignore-hosts`；KDE（`kwriteconfig6` 可用）：`kioslaverc` 的 `ProxyType=1` 与 `httpProxy` 等；其他：不改任何东西，打印 `export http_proxy=… https_proxy=… no_proxy=…` 提示并返回 `Err(Unsupported)` | 备份对应键的原值 |

- 代理地址：第一个 `http-listen`（`0.0.0.0` / `::` 时用 `127.0.0.1`）作 http / https；`set-system-socks-proxy = true` 时用第一个 `socks5-listen`；`bypass` = `skip-proxy` 各项（macOS 语义，已登记）；`exclude_simple = exclude-simple-hostnames`（Windows 追加 `<local>`）。

### 8.2 生命周期

- 开启（`rurge run --system-proxy` 或 `POST /v1/features/system_proxy {"enabled":true}`）：`snapshot()` → `StateStore.update(system_proxy_backup = Some(b), features.system_proxy = true)` → `apply`；`apply` 失败 → 回滚状态、返回错误（API 500 / CLI 退出 1）。
- 关闭（`…false`、优雅退出）：`restore(backup)` → `update(system_proxy_backup = None, features.system_proxy = false)`。
- 崩溃恢复：启动时若 `system_proxy_backup` 非空 → 先 `restore` 并 WARN，再按本次是否开启决定是否重新 `apply`。
- 重载后监听地址变化且 `features.system_proxy = true` → 重新 `apply`（`Control::reload` 结束时由 bin 处理）。
- `Command::SystemProxy(bool, oneshot)` 由主循环执行（平台调用可能阻塞数百毫秒，用 `spawn_blocking`）。

### 8.3 `rurge service install | uninstall [--user]`（P1 基础版）

| 平台 | install | uninstall |
| --- | --- | --- |
| Linux | 生成 systemd unit（`--user`：`~/.config/systemd/user/rurge.service`，否则 `/etc/systemd/system/rurge.service`，需 root），`ExecStart=<rurge 绝对路径> run -c <conf 绝对路径> [--system-proxy]`，`systemctl [--user] enable --now rurge` | `disable --now` + 删除文件 |
| macOS | 生成 launchd plist（`--user`：`~/Library/LaunchAgents/io.rurge.daemon.plist`，否则 `/Library/LaunchDaemons`），`launchctl bootstrap` | `launchctl bootout` + 删除 |
| Windows | `schtasks /create /tn rurge /sc onlogon /tr "<rurge> run -c <conf>"`（D8） | `schtasks /delete /tn rurge /f` |

- 只做文件生成与调用系统命令；失败原样打印命令的 stderr。测试只断言生成的文件内容。

## 9. 测试策略

| 层 | 内容 |
| --- | --- |
| 单元 | `StateStore`（缺失 / 损坏改名 / 原子写 / 并发 `update` 串行）；鉴权中间件（头 / 查询参数 / 常量时间 / 5 次封禁 / 有界表）；脱敏函数；`ProxySettings` 生成；各平台后端只测「生成的注册表值 / 命令行 / 设置键」不真正执行 |
| 集成（`crates/rurge-api/tests`，127.0.0.1） | 按 M3 `pipeline.rs` 方式起 `Engine` + `serve()`，用 hyper 客户端逐端点断言：401 / 403 与封禁；`outbound` 读写并落盘（重开 `StateStore` 读回）；`global` 校验；`requests/*` 与 `kill`（明文转发会话可 kill，M3b 修复波）；`traffic` 与一次代理请求的字节一致；`dns` 快照 / flush；`profiles/current` 脱敏；`profiles/check` 诊断；`log/level`；features 501；`stop` 触发测试替身 `Control::stop` |
| CLI | 子进程 `rurge run`（`http-api = k@127.0.0.1:0`，解析 `api on` 行取端口）→ `rurge status` / `reload` / `stop --remote` 端到端；未配置 `http-api` 时 `reload` 退出 2；`--outbound-mode` 显式 / `state.json` 优先级 |
| M4b | `SystemProxy` mock 验证生命周期（开 / 关 / 崩溃残留恢复 / 重载重应用 / apply 失败回滚）；`service` 只测生成内容 |
| 手工验收（阶段 1 第 5、6 条） | 三平台开系统代理后浏览器流量出现在 `/v1/requests/recent` 并按规则分流；`rurge status` 显示模式 / 策略数 / 规则数 / 活动请求数 |

测试不访问公网。

## 10. 兼容性清单需登记的差异（实施时更新 `docs/surge-compatibility-matrix.md`）

| 项 | 状态 | 说明 |
| --- | --- | --- |
| `/v1/policies` `/v1/rules` `/v1/requests/*` `/v1/traffic` `/v1/dns` 的 JSON 结构 | 🟡 | 手册未定义，暂定结构见 `docs/api/phase1.md`，阶段 6 对齐真实 Surge |
| 鉴权封禁 | 🟡 | 10 分钟 5 次 → 封 10 分钟；`security ban` 查看 / 解除在阶段 6 |
| `POST /v1/stop` | 🟡 | 停引擎并退出进程，由服务管理器决定是否重启 |
| `POST /v1/test/dns_delay` | 🟡 | 返回按上游的时延列表而非单一数字 |
| `rurge reload` / `stop` / `status` | 🟡 | 依赖 `http-api`；未配置时提示 SIGHUP / `--watch` |
| 出站模式持久化 | ✅ | `state.json` 优先于默认值；显式 `--outbound-mode` 覆盖并写回（更新 M3 登记的「初值来自 CLI」） |
| `proxy` 模式下全局策略缺失或已不存在 | 🟡 | 按规则模式处理并 WARN 一次（Q3） |
| Linux 系统代理 | 🟡 | GNOME / KDE 写设置，其他桌面只打印环境变量提示 |
| macOS 系统代理 | 🟡 | `networksetup`，可能需管理员权限（Q2 实施时核实） |
| Windows 开机自启 | 🟡 | 计划任务；真正的 Windows 服务在阶段 6 |
| `http-api-tls` | ⛔（本阶段） | 阶段 4 随 MITM CA |

## 11. 开放问题

| 编号 | 问题 | 处理 |
| --- | --- | --- |
| Q1 | `/v1/traffic` 的 `startTime` 单位与 `connector` 键名是否与 Surge 一致 | 阶段 6 用真实实例校准；本阶段暂定毫秒 |
| Q2 | macOS `networksetup` 是否需 sudo（骨架 Q2） | M4b 实施时验证，结果写进清单与 README |
| Q3 | 重载后 `global_policy` 指向已不存在的策略 | 保持 `proxy` 模式但按 `Rule` 处理并 WARN；`GET /v1/outbound/global` 仍返回原名 |
| Q4 | API 是否限制来源为局域网 | 不限制（与 Surge 一致，靠密钥）；绑定非回环地址时启动 WARN 一条 |

## 12. 与 M3b 延后事项的关系

M4a 顺带处理：`State::load` 同步读取（→ `StateStore`）；`dial` / `dial_internal` 重复（→ `resolve_policy`）；速率采样在会话结束瞬间重复计一次（暴露 `/v1/traffic` 前先快照累计再读活动并扣掉期间结束的句柄）；`RequestLog::kill` 对内部 DNS 会话是空操作（API 对 `listener = internal` 的 kill 回 409 `not killable`）。其余延后项保持登记。

## 13. 与后续阶段的接缝

- 阶段 2：`POST /v1/policy_groups/select` 写 `StateStore.group_selections` 并调用 `PolicyRegistry` 的运行期 `select`；`/v1/policies/detail`、`/v1/policies/test`、`/v1/policy_groups*` 补齐。
- 阶段 4：`http-api-tls` 复用 MITM CA。
- 阶段 6：Dashboard 静态文件挂在同一 axum 路由；`/v1/metrics`（`surge_*` 指标）读 `TrafficStats` / `RequestLog` / 封禁表；多 Profile 目录与 `switch`；`security ban`；`rurge shell` 与其余同名命令复用 bin 的 API 客户端模块；真正的 Windows 服务。

## 14. M4a 实施备注

M4a 已实现（分支 `m4a-control-plane`）；下面记录实施期与 §4–§7 文本出入的地方。不回填修改 §1–§13，一律以本节与代码为准。

- **`Mode` 与 `OutboundMode` 解耦**（§4.1）：`Engine` 没有采用 `ArcSwap<OutboundMode>`，而是新引入一个无载荷的三态枚举 `rurge_engine::control::Mode { Direct, Proxy, Rule }`；`Engine::mode() -> Mode` / `set_mode(Mode)` 取代了 `outbound_mode()` / `set_outbound_mode`。全局策略仍是独立的 `global_policy: ArcSwap<Option<String>>`；`Mode::from_outbound(&OutboundMode) -> (Mode, Option<String>)` 负责从 CLI 的 `--outbound-mode proxy=<name>` 里拆出两者。`Mode`、`LogLevel`、`ReloadReport`、`Control` 都定义在 `rurge_engine::control` 模块，并在 crate 根重导出。
- **策略校验改为按会话快照**：`Engine::policy_exists` 与内部的 `policy_known(rt, name)` 都针对*调用方持有的那个 `Runtime` 生成*校验，而不是当前最新的一份，避免重载与拨号竞态时用一代的注册表批准、另一代的注册表解析；新增非分配的 `PolicyRegistry::contains(&str)`（`rurge-policy`），取代逐次 `names().iter().any(..)` 的分配。
- **`config_text` 的脱敏面比 §4.3 描述的更宽**：除 `password=`、`http-api` / `external-controller-access` 的 `key@` 前缀、`ca-passphrase`、`ca-p12` 外，`crates/rurge-config/src/redact.rs` 还处理 `http-listen` / `socks5-listen` 的 `key@` 前缀、`wifi-access-http-auth` 的口令、策略行的 `psk=` / `private-key=` / `base64=` 参数，以及 `http` / `https` / `socks5` / `socks5-tls` 策略行第 4 个起、不含 `=` 的位置型凭据（这四种类型把 `username, password` 按位置传递，不是 `password=`）。
- **`ApiContext` 多一个字段**（§5.1）：`ApiContext { engine, control, load_options: LoadOptions }`；`load_options` 是守护进程自己的加载选项，供 `POST /v1/profiles/check` 用同一套环境 / 平台 / capabilities 重新校验磁盘上的配置。
- **`serve` 的签名**（§5.1）：不是 `io::Result<JoinHandle<()>>`，而是 `io::Result<(SocketAddr, ServerFuture)>`（`ServerFuture = Pin<Box<dyn Future<Output = ()> + Send>>`）——`serve` 只绑定端口、构造 `Router` 并返回服务 future，由 `rurge run` 自己把 future 挂到引擎的 `TaskTracker` 上；返回绑定地址是因为测试与 `http-api = key@127.0.0.1:0` 都需要拿到操作系统实际分配的端口。
- **`GET/POST /v1/outbound/global` 未设时是 `null` 不是空串**（§5.2）：`{"policy": Option<String>}` 走 serde 默认序列化，未设策略时是 `{"policy":null}`，不是文档草稿写的 `{"policy":""}`。
- **`/v1/traffic` 的 `startTime`、`/v1/dns` 的 `expiresTime` 都是 Unix 秒（`f64`）**，不是 §5.2 与开放问题 Q1 写的毫秒；`docs/api/phase1.md` 以此为准，Q1 视为已回答（秒，而非"待阶段 6 校准"）。
- **`/v1/modules`、`/v1/scripting`、`/v1/events` 的实际形状**（§5.2）：分别是 `{"enabled":[],"available":[]}`、`{"scripts":[]}`、`{"events":[]}`；`/v1/modules` 不是 §5.2 写的 `{"modules":[]}`。
- **`Control::set_log_level` 不接 `tracing_subscriber::filter::LevelFilter`**（§6）：trait 方法签名是 `set_log_level(&self, level: LogLevel) -> Result<(), String>`，`LogLevel` 是 `rurge_engine::control` 自己的枚举（`Verbose` / `Debug` / `Info` / `Notify` / `Warning` / `Error`）；从 `LogLevel` 到 `LevelFilter` 的映射留在 `rurge run` 的 `LoopControl` 里（与 `parse_log_level` 用同一张表），这样 `rurge-engine` 定义这个 trait 不必依赖 `tracing-subscriber` 的 `reload` feature。
- **`ReloadReport.ok` 在重绑监听器失败时也是 `false`**：解析失败、构建 `Runtime` 失败、重绑监听器失败三条路径都返回 `ok:false`（`errors` 至少为 1），运行中的配置保持不变；三者都成功才是 `ok:true`。
- **`rurge status` 的 `policies` 计数含 5 个内置策略**：`GET /v1/policies` 的 `proxies` 数组固定以 `DIRECT`、`REJECT`、`REJECT-DROP`、`REJECT-NO-DROP`、`REJECT-TINYGIF` 开头，再接配置的策略；CLI 的 `policies: N` 直接取 `proxies.len() + policy-groups.len()`，哪怕配置文件一个策略都没写，`N` 也从 5 起。
- **启动信息的固定顺序**（§6）：`listening on …`（每个监听器一行）→（配置了 `http-api` 才有）`api on http://<addr>` → `rurge <version> running: …`（汇总行）；CLI 集成测试与 `rurge status` 都从这个固定顺序里解析端口，顺序本身是约定的一部分。
- **显式 `--outbound-mode proxy=<name>` 写回 `state.json` 时不校验策略是否存在**（D5 既有行为，明确记录）：`initial_mode` 直接把解析出的 `(mode, global)` 写回 `StateStore`；策略是否存在只在运行期的 `choose_policy` 里校验（缺失或未知 → 按规则模式处理并 WARN 一次）。也就是说 `--outbound-mode proxy=Typo` 会把 `Typo` 落盘，但实际路由走规则，直到有人把全局策略改成一个真实存在的名字。
