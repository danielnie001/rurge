# 阶段 3 / M1「网络环境与子网」实施计划

> **执行方式**：子代理驱动（superpowers:subagent-driven-development）——每个任务一个新的实现者，做完由新的评审者评审，全部做完后整分支评审一次。本计划写到"改哪里、接口是什么、要哪些用例、判据是什么"为止：实现者先写失败的用例、看到红，再实现到绿；与计划的出入记进末尾的「执行期修正记录」。

**目标**：`subnet` 组、`SUBNET` 规则与 `[SSID Setting]` 按当前网络生效；三个平台读网络快照、换网络时引擎清 DNS 缓存、作废测速、重建 WireGuard；`[SSID Setting]` 的 DNS 覆盖与接管暂停（系统代理的一半）。

**依据**：设计 `docs/superpowers/specs/2026-10-16-phase3-m1-network-subnet-design.md`（下面写"设计"指它；第 12 节 M1-D1 ～ D16）；总设计 `docs/superpowers/specs/2026-10-16-phase3-enhanced-mode-design.md`。

## 全局约束

- 不新增包，`Cargo.lock` 的包数不变（517 个）。可以给已有的包开**不带新依赖**的特性；`nix` 的 `socket` / `net` 特性要 `memoffset`（锁文件里没有），**不开**。新的依赖边只限工作区已有的包（如 `rurge-config` 不加依赖；`rurge-platform` 可以加 `libc`，它已在锁文件里）。
- `unsafe_code`：全工作区 `forbid`，`rurge-platform` 的两个例外不增加——M1 用不到 FFI（macOS 的路由套接字经 `socket2`，Windows 经 `ipconfig` 包与 `netsh`，Linux 读 `/proc` 与 `/sys`）。MSRV 1.89，edition 2024。
- 凭据不进日志、错误文本、诊断、`Debug` 与 API 输出；诊断不引用配置里的取值（子网表达式写错时照 `subnet` 组今天的写法）。`[SSID Setting]` 里的 DNS URL 可能带口令：写进标记用例 `crates/rurge-config/tests/secrets.rs`（任务 1）。SSID、BSSID、网关与本机地址只进 DEBUG 日志（M1-D16）。
- 测试只用回环地址与端口 0，等待都有时限，不用固定的 sleep 当同步；不联网。**测试从不执行会改系统设置的命令**；读网络的只读命令（`netsh wlan show interfaces`、`nmcli`、`iw`、`route -n get`、`networksetup -listallhardwareports`、`ipconfig getsummary`）也不在缺省的用例里跑——解析器用文本夹具测；只有一条"在本平台上真读一次快照不 panic"的用例真去读（只读）。起 `rurge` 二进制的用例经 `rurge_command()` / `rurge_process()`，给 `--data-dir`，并且（任务 6 起）缺省用网络的文件后端。
- 系统代理只经文件后端（`RURGE_SYSTEM_PROXY_BACKEND=file:<路径>`）测；不运行 `rurge run --system-proxy` 于真实后端。
- "改一行代码看用例变红"的验证之前先写下这一改让哪些会话、查找、下载与文件去哪里；可能离开回环或写到用例的临时目录之外的不做。
- 计时的用例走暂停的时钟（`tokio::time::pause`），或在用例自己的线程上按系统时钟等；会让工作线程停住的不靠运行时的定时器；新写的计时用例提交前单独循环跑二十次以上。
- 只运行本仓库构建出来的二进制与系统自带的只读工具；不下载、不安装任何东西。
- Surge 语法就是 rurge 语法；与 Surge 的每一处行为差异都进兼容性清单（任务 7）。
- 注释、日志与错误文本用英文，照所在文件的写法；提交说明的标题用中文。
- 先写失败的用例、看到红，再实现到绿。现有用例变红时：计划点了名的照计划改并在提交说明里说；没点名的停下来报告，不顺手改期望值。
- 门禁（每个任务末尾一次）：`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test -j 8 --workspace --no-fail-fast`（本机 `RUSTFMT` 指向 gnu 工具链的 rustfmt）。聚焦地跑用例也经整个工作区（`cargo test -j 8 --workspace --lib -- <过滤词>`，或 `--test <目标> -- <过滤词>`），不用 `-p`。测试二进制以 `STATUS_HEAP_CORRUPTION` 退出时单独重跑那一个（开发机的管控软件注入的钩子 DLL，不是 rurge 的缺陷）。跑全量之前 `df -h /d`，剩余不足约 15 GiB 就停下。
- 新增的诊断用法在 `codes::hint` 的表里各做一个决定（`every_code_is_decided_once`）。

## 评审要点

计划的用例之外最可能咬到使用者的五种情形：

1. **SSID 拿不到**（Windows 没有位置权限、macOS 的守护进程）：`SSID:` / `BSSID:` 条件一律不命中、不报错，`ROUTER:` 与 `TYPE:` 照常；日志在 DEBUG 里说一次原因，不在每次变化时都说（任务 2、6）。
2. **换网络的那一刻**：解析器的缓存清掉之前、新一代建好之前，正在进行的查找与会话照旧完成；DNS 覆盖导致的重建不重读磁盘上的 Profile（使用者没让重载的改动不能顺带生效）（任务 5、6）。
3. **`[SSID Setting]` 一行里的 URL 带 `=` 与逗号**（`encrypted-dns-server=https://a.example/dns-query?x=1,https://b.example/`）：两个都是列表项，不被当成参数；URL 里的口令不进诊断（任务 1）。
4. **接管暂停时使用者经 API 打开系统代理**：返回成功、不立刻设，离开暂停的网络时按最后的想要的状态办；暂停不进 `state.json`，崩溃之后的恢复照旧只看备份（任务 6）。
5. **`subnet` 组没写 `default`**：原来按空组兜底能加载的 Profile 现在是加载错误——`rurge check` 与重载都说清楚是哪个组、要写什么（任务 1；兼容性清单登记行为变化，任务 7）。

---

## 文件结构

| 文件 | 任务 | 职责 |
| --- | --- | --- |
| `crates/rurge-config/src/network.rs`（新） | 1 | `NetworkFacts`、`Primary`、`WifiLink`、`NetKind`、`NetworkCell` |
| `crates/rurge-config/src/policy.rs` | 1 | `SubnetExpr::matches` |
| `crates/rurge-config/src/spec/group.rs` | 1 | `SubnetSpec`、`subnet` 组的成员表、没写 `default` |
| `crates/rurge-config/src/ssid_setting.rs`（新）、`config.rs`、`deferred.rs` | 1 | `[SSID Setting]` 的类型化与诊断 |
| `crates/rurge-config/tests/secrets.rs` | 1 | `[SSID Setting]` 的 DNS URL |
| `crates/rurge-platform/src/netenv/`（新目录） | 2 | 快照、三个平台的读取、解析器、提示、文件后端 |
| `crates/rurge-platform/src/command.rs` | 2 | 带时限的执行 |
| `crates/rurge-rules/src/{matcher.rs,engine.rs}` | 3 | `SUBNET` 的求值、`EvalCtx` 带网络、规则引擎持有 `NetworkCell` |
| `crates/rurge-proto/src/outbound.rs`、`crates/rurge-proto-wireguard/src/outbound.rs` | 4 | `Outbound::network_changed` |
| `crates/rurge-policy/src/{registry.rs,auto.rs}` | 4 | `subnet` 组的选择与覆盖、网络变化时作废轮次与清覆盖、通知出站 |
| `crates/rurge-engine/src/{shared.rs,engine.rs,runtime.rs,stack.rs,views.rs,auto.rs}` | 4、5 | `EngineShared.network`、`Engine::network_changed`、建一代时的 DNS 覆盖、`subnet` 组可覆盖 |
| `crates/rurge/src/cli/network.rs`（新）、`run.rs`、`runtime.rs`、`rule.rs`、`dns.rs`、`capabilities.rs` | 3、4、6 | 网络任务、重建一代、接管暂停、离线命令读快照、能力表 |
| `crates/rurge/tests/cli.rs` | 6 | 辅助函数缺省设网络的文件后端；端到端用例 |
| 文档 | 7 | 兼容性清单、`docs/api/phase3.md`（新）、`docs/acceptance/phase3-manual.md`（新）、CLAUDE.md |

---

## Task 1：配置层——当前网络、子网表达式求值、`subnet` 组、`[SSID Setting]`

**改哪里**

1. `rurge-config/src/network.rs`（新，`lib.rs` 里 `pub mod network`）：设计第 2 节的 `NetworkFacts`（`Default` 是什么都不知道）、`Primary`、`WifiLink`、`NetKind { Wifi, Wired, Other }`，都 `Clone + Debug + PartialEq + Eq`；`Debug` 照常（这几个字段不是凭据；SSID 进日志的限制在调用方）。`NetworkCell(std::sync::RwLock<Arc<NetworkFacts>>)`：`new(NetworkFacts)`、`load() -> Arc<NetworkFacts>`、`store(NetworkFacts)`；锁中毒时取里面的值继续（`into_inner` 的写法），不 panic。另加 `serde` 的 `Serialize` / `Deserialize`（文件后端要，任务 2；`rurge-config` 已依赖 `serde`）。
2. `policy.rs`：`impl SubnetExpr { pub fn matches(&self, facts: &NetworkFacts, wifi_always_match: bool) -> bool }`，照设计第 2 节的表。`Glob` 的大小写选项沿用今天解析时给的（`SSID:` 区分、`BSSID:` 不区分）；裸值的 BSSID 比较不区分大小写。
3. `spec/group.rs`：`GroupSpec` 加 `pub subnet: Option<SubnetSpec>`（`SubnetSpec { conditions: Vec<(SubnetExpr, String)>, default: String }`）；`subnet` 组的 `members` 改为"条件里的策略按书写顺序、再 `default`，去重"；没写 `default` → `E0018`，文本 `subnet group \`<名字>\` needs \`default\``（名字是组名，照 `named(..)` 的写法），组不建（`spec` 为 `None`）。`GroupSpec` 的手写 `Debug` 带上 `subnet`（条件的表达式与策略名不是凭据）。
4. `[SSID Setting]`：`deferred.rs` 的 `DEFERRED` 去掉 `SSID Setting`；新模块 `ssid_setting.rs`：`SsidSetting`（设计第 7.1 节的字段；`encrypted_dns_server: Option<Vec<EncryptedDns>>`，`off` 是 `Some(vec![])`）与 `parse(entries, diags) -> Vec<SsidSetting>`；`Config` 加 `pub ssid_settings: Vec<SsidSetting>`，在 `Config` 的加载里调用。一行的读法照设计第 7.1 节：
   - 分界：找第一个"前面是空白、写成 `<认识的键>=`"的位置（键不区分大小写，等号两边可以有空白）；之前去掉首尾空白是表达式，之后是参数。
   - 参数：用 `value::split_fields`（一行怎样拆成各项的扫描）按逗号分；一段的键是认识的键 → 新参数；否则跟在 `dns-server` / `encrypted-dns-server` 之后 → 那个列表的下一项；都不是 → `W0028`（文本不引用这一段：`unknown parameter in [SSID Setting]`，位置是这一行）。
   - 值：`dns-server` 与 `encrypted-dns-server` 交给 `[General]` 同名键的读取函数（找到 `general.rs` 里它们的解析，抽成能单独调用的函数，诊断与 `W0026` 照旧，不引用凭据）；`suspend` 用 `[General]` 的布尔读法；`tfo-behaviour` 认三个值，写错 `W0028`，写对了 `W0029`（`tfo-behaviour has no effect: TCP Fast Open is not implemented`）；`cellular-fallback` 同 `[General]` 里 iOS 专属的键的诊断；`cellular-mode` 是 `W0029`。
   - 没有参数的行：`W0029`（`a [SSID Setting] line without parameters has no effect`），照收。
   - 表达式写错：`E0011`，文本照 `SubnetExpr::parse` 今天的，这一行不进 `ssid_settings`。
   - `SsidSetting` 手写 `Debug`：DNS 服务器的列表经它们自己的 `Debug`（`EncryptedDns` 的已经不带凭据）。
5. `Config` 加 `pub fn effective_ssid_setting(&self, facts: &NetworkFacts) -> Option<&SsidSetting>`：第一条 `expr.matches(facts, self.general.subnet_exp_wifi_always_match)` 的行。

**用例**（`rurge-config`）

- `subnet_expressions_match_the_network`：设计第 2 节表里每一行至少一个命中、一个不命中；`wifi_always_match` 为假时 Wi-Fi 不是主网卡就不命中；SSID 为 `None` 时 `SSID:*` 也不命中；`TYPE:CELLULAR` 与 `MCCMNC:` 在任何快照下都不命中。
- `the_network_cell_hands_out_what_was_stored_last`。
- `a_subnet_group_lists_its_conditions_then_its_default`（去重、顺序）；`a_subnet_group_without_default_is_an_error`（`E0018`，文本含组名，组不建）——**现有的 `a_subnet_group_stands_for_its_default` 照新语义改写**（它断言 `members` 只有 `default`），在提交说明里说。
- `ssid_setting_lines_are_read`：表达式带空格（`SSID:My Home suspend=true`）；`dns-server=1.1.1.1,8.8.8.8,suspend=true` 是两项列表加一个参数；`encrypted-dns-server=https://a.example/dns-query?x=1,https://b.example/` 是两项；`encrypted-dns-server=off`；不认识的键 `W0028` 且诊断文本里没有这一段；`tfo-behaviour` 与 `cellular-mode` 的 `W0029`；表达式写错是 `E0011` 且这一行不用；没有参数的行。
- `the_first_matching_ssid_setting_line_is_in_effect`。
- `secrets.rs`：标记用例的配置加一行 `[SSID Setting]`，`encrypted-dns-server` 是一个用户信息带标记的 `https://` URL；加载后的 `Debug`、诊断与脱敏输出里都找不到标记。
- 语料库快照：`[SSID Setting]` 原来是保留的节，现在类型化——快照若有变化，用 `INSTA_UPDATE=always cargo test -j 8 --workspace --test corpus` 写出、`git diff` 逐行看，只留下能用这一任务解释的，否则停下来报告。

**判据**：上面的用例绿；`rurge check` 对一份带 `[SSID Setting]` 的配置不再报"这一节没有作用"的 `W0016`（找到今天发它的地方，用例在任务 6 的 CLI 里覆盖）；门禁绿。

## Task 2：`rurge_platform::netenv`——读快照、提示、文件后端

**改哪里**（`crates/rurge-platform/src/netenv/`，`lib.rs` 里 `pub mod netenv`；依赖只加锁文件里已有的 `libc`，只在 macOS 上用）

1. `mod.rs`：`Snapshot { facts: NetworkFacts, dns: Vec<IpAddr>, addresses: Vec<IpAddr> }`（`Serialize` / `Deserialize`、`PartialEq`）；`Quick`（快速读取的指纹，`PartialEq`）；`ReadOpts { exclude: Vec<String> }`（要排除的网卡名，M1 是空的）；`pub fn read(&ReadOpts) -> Snapshot`、`pub fn read_quick(&ReadOpts) -> Quick`；`pub enum Backend { Platform, File(PathBuf) }` 与 `pub fn backend_from_env() -> Result<Backend, String>`（读 `RURGE_NETWORK_BACKEND`：没设是 `Platform`，`file:<路径>` 是 `File`，别的值是错误，文本说只认 `file:<path>`）。
2. 解析器（`parse.rs`，与平台无关、全部 `pub(crate)` 并有夹具用例）：`netsh_wlan(text) -> Option<WifiLink>`（键去掉空白恰为 `SSID` 的一行与以 `BSSID` 结尾的一行；值去掉空白；空值是 `None`；没有 `SSID` 行 → 没连上）、`proc_net_route(text) -> Option<(String, Ipv4Addr)>`（度量最小的默认路由）、`proc_ipv6_route(text) -> Option<(String, Ipv6Addr)>`、`nmcli_wifi(text) -> Option<WifiLink>`（`ACTIVE` 为 `yes` 的一行，`\:` 还原成 `:`）、`iw_link(text, iface) -> Option<WifiLink>`、`route_get_default(text) -> Option<(String, IpAddr)>`、`hardware_ports(text) -> Vec<(String /*device*/, NetKind)>`、`ipconfig_summary(text, iface) -> Option<WifiLink>`（`<redacted>` 当 `None`）。
3. 三个平台的读取（`windows.rs`、`linux.rs`、`macos.rs`，各自 `#[cfg]`）：照设计第 3.2 节的表。Windows 的主网卡选择写成纯函数 `pick_primary(adapters: &[AdapterView]) -> Option<…>`（`AdapterView` 是从 `ipconfig::Adapter` 取出的几个字段，用例可以手写），跳过回环与隧道类型、不开着的网卡、`exclude` 里的名字；类型照计划期决定 P1。命令一律经第 5 项的带时限执行，读不到的项当作没有（DEBUG 里一行，不带 SSID）。
4. 提示（`watch.rs`）：`pub fn hints(backend: &Backend) -> std::sync::mpsc::Receiver<()>`——起一个专用线程：`File` 每 100 毫秒看文件的内容是否变了；Windows、Linux 每 2 秒 `read_quick` 比指纹；macOS 一个 `PF_ROUTE` 套接字（`socket2::Socket::new(Domain::from(libc::AF_ROUTE), Type::RAW, None)`，阻塞地收；消息头的 `rtm_type`（第 4 字节）是 `RTM_NEWADDR`、`RTM_DELADDR`、`RTM_IFINFO`，或是 `RTM_ADD` / `RTM_DELETE` / `RTM_CHANGE` 而 `rtm_flags`（偏移 8 的 `i32`，本机字节序）不带 `RTF_LLINFO` / `RTF_WASCLONED` 时提示）加每 30 秒一次；另外 Windows、Linux、macOS 在有无线网卡连着时每 30 秒提示一次（Wi-Fi 的完整读取）。消息头的过滤写成纯函数 `route_message_matters(&[u8]) -> bool`（任何平台都编译与测试；`libc` 的常数只在 macOS 上有，纯函数里用本地常数并注明出处，或按平台条件编译常数——实现者选一种并在报告里说）。接收端被丢掉时线程结束。
5. `command.rs`：`pub fn run_with_timeout(cmd: &[String], timeout: Duration) -> io::Result<String>`——`spawn` 子进程（标准输出与错误接管道），在另一个线程上读完输出并 `wait`，主线程 `recv_timeout`；超时 `kill` 子进程、返回 `TimedOut` 错误（文本经 `failure` 的写法，不带凭据）。`SystemRunner` 不变。
6. 文件后端：`read` / `read_quick` 在 `Backend::File` 时读文件反序列化（读不了或解析不了 → `Snapshot::default()`，DEBUG 一行）。`read` 与 `read_quick` 取 `&Backend` 参数（或把后端放进 `ReadOpts`——实现者选一种，在报告里说）。

**用例**（`rurge-platform`）

- 每个解析器对夹具：`netsh` 的 Windows 10 写法（`BSSID`）与 Windows 11 写法（`AP BSSID`）、没连上、没有位置权限时只有提示文字（`SSID` 行不在 → `None`）；`/proc/net/route` 两条默认路由取度量小的、没有默认路由；`/proc/net/ipv6_route`；`nmcli` 的 `\:` 与不活动的行；`iw dev link` 连上与没连上；`route -n get default` 的 IPv4 与 IPv6；`networksetup -listallhardwareports`；`ipconfig getsummary` 的 `<redacted>`。夹具写在用例里（几行文本），注明是按公开的输出格式写的、不是从哪台机器拷来的。
- `pick_primary`：度量最小的、跳过回环 / 隧道 / 没开 / 被排除的、没有网关的不算。
- `route_message_matters`：地址增删与网卡状态是、带 `RTF_LLINFO` 的增删不是、截断的消息头不是。
- `run_with_timeout`：正常结束返回输出；不会自己结束的子进程在时限到时被结束、返回 `TimedOut`（计划期决定 P6：再起当前测试二进制自己，跑一个只在环境变量 `RURGE_TEST_HOLD=1` 时才阻塞的忽略用例；不起本仓库之外的程序）。
- `the_file_backend_reads_what_the_test_wrote`、`a_missing_file_is_an_unknown_network`、`an_unknown_backend_value_is_refused`。
- `reading_this_machine_does_not_panic`（只读；不断言内容）。
- `hints` 对文件后端：改写文件之后有界时间里收到提示。

**判据**：上面的用例在本机（Windows）全绿；Linux 与 macOS 的读取代码在本机至少经过 `cargo clippy` 不了的部分——**本机编译不到 `#[cfg(target_os = "linux")]` / `"macos"` 的代码**：实现者把这两个文件里能做成纯函数的都做成纯函数（已在第 2 项），剩下的薄层照抄已有模块（`dns.rs`、`sysproxy`）的写法，评审者逐行读；第一次 CI 运行时核对（写进延后事项）。门禁绿。

## Task 3：`SUBNET` 规则

**改哪里**

1. `rurge-rules/src/matcher.rs`：`EvalCtx` 加 `network: Option<(&'a NetworkFacts, bool)>`（事实与 `wifi_always_match`）；`EvalCtx::new(geo)` 不变（没有网络）；加 `EvalCtx::with_network(geo, facts, wifi_always_match)`。`RuleKind::Subnet(e)` 编译成 `Matcher::Subnet(SubnetExpr)`，求值 `ctx.network.is_some_and(|(f, w)| e.matches(f, w))`。
2. `engine.rs`：`RuleEngine` 加 `network: Arc<NetworkCell>` 与 `wifi_always_match: bool`；`build` / `build_with_registry` 加参数（找到全部调用方：`rurge-engine` 建一代的地方、`rurge rule match`、`rurge dns lookup`、测试与基准——没有网络的调用方传 `Arc::new(NetworkCell::new(NetworkFacts::default()))`）；`run` 与 `pre_match` 开头 `let facts = self.network.load();` 并用 `with_network`。
3. 可执行文件 `capabilities.rs`：`INACTIVE_RULE_TYPES` 去掉 `SUBNET`；文件头的说明加一句。

**用例**

- `subnet_rules_follow_the_network`（`rurge-rules`）：同一个规则引擎，`NetworkCell` 换了快照之后 `SUBNET,TYPE:WIFI` 的结果跟着变；`SUBNET,ROUTER:192.168.1.1`；`SUBNET,SSID:Home` 在 SSID 拿不到时不命中。
- `a_pre_matching_subnet_rule_sees_the_network`：带 `pre-matching` 的 `SUBNET` 规则在预匹配里命中。
- `rurge check` 不再对 `SUBNET` 报"永不匹配"：找到今天断言它的 CLI 用例（若有），照新语义改并在提交说明里说。

**判据**：用例绿；门禁绿。

## Task 4：`subnet` 组的选择与覆盖、网络变化时的组与出站

**改哪里**

1. `rurge-proto/src/outbound.rs`：`trait Outbound` 加 `fn network_changed(&self) {}`（文档注释：网络变了，出站可以丢掉绑在旧网络上的东西；缺省什么也不做）。`rurge-proto-wireguard/src/outbound.rs` 的 `impl Outbound for WireGuardOutbound` 转调它已有的 `network_changed()`。
2. `rurge-policy/src/registry.rs`：注册表持有 `Arc<NetworkCell>` 与 `wifi_always_match`（`build` 的参数，`rurge-engine` 传入）；`GroupKind::Subnet` 的选择：覆盖（`AutoGroups` 里的）→ 第一个命中的条件 → `default`；条件引用的策略不在注册表里时跳过那一个条件（加载期已报过）。加 `pub fn network_changed(&self)`：对每个 `Entry::Outbound` 调 `outbound.network_changed()`；`self.auto().network_changed(&group_specs)`。
3. `auto.rs`（`AutoGroups`）：`pub fn network_changed(&self, groups: &[GroupSpec])`——清掉 `url-test` / `fallback` / `load-balance` 组在 `rounds` 里的时间（不动 `smart`）；清掉 `subnet` 组的覆盖。`set_override` 接受 `subnet` 组。
4. `rurge-engine`：`auto.rs` 的 `automatic()` 不变；加 `overridable(kind)` = `automatic(kind) || kind == Subnet`，`views.rs` 的 `select_group` 用它判断能不能覆盖（`subnet` 组走覆盖的那一支）；建注册表的地方传 `NetworkCell` 与 `wifi_always_match`（`EngineShared.network` 在任务 5 加；这一任务先在 `EngineShared` 加上字段 `network: Arc<NetworkCell>`，缺省"什么都不知道"）。
5. 可执行文件 `capabilities.rs`：`group_kinds` 加 `GroupKind::Subnet`，文件头的说明加一句；`W0008` 不再因 `subnet` 出现——找到断言它的用例（`rurge-config` 的 `config.rs` 里有两处测 `W_GROUP_NOT_IMPLEMENTED`，它们用的是测试自己的能力表，照旧），CLI 一侧若有断言照新语义改。

**用例**

- `rurge-policy`：`a_subnet_group_picks_by_the_network`（第一个命中的条件；都不命中用 `default`；换快照之后跟着变）；`a_subnet_group_can_be_overridden_until_the_network_changes`；`a_network_change_restarts_the_tests_but_keeps_the_results`（`url-test` 组：网络变化之后 `last_round` 是 `None`，下一次使用唤起一轮；测完之前选择不变；`smart` 组的轮次不动）；`evaluate_before_use_waits_again_after_a_network_change`；`a_network_change_reaches_every_outbound`（假出站数 `network_changed` 的调用次数）。
- `rurge-engine`：`POST /v1/policy_groups/select` 对 `subnet` 组设覆盖与清除（`views` 的单元用例或 `rurge-api` 的 `--test api` 用例，实现者选现有用例更近的一处）。
- `rurge-proto-wireguard`：`network_changed` 经 `Outbound` 转到设备（若已有的 `a_network_change_gives_every_peer_a_new_carrier` 是直接调 inherent 方法，加一条经 `&dyn Outbound` 调的）。

**判据**：用例绿；门禁绿。

## Task 5：引擎——当前网络、`network_changed`、建一代时的 DNS 覆盖

**改哪里**

1. `shared.rs`：`EngineShared.network: Arc<NetworkCell>`（任务 4 已加字段；这里接到规则引擎的 `build`）。
2. `engine.rs`：`pub fn network_changed(&self, facts: NetworkFacts)`——`shared.network.store(facts)`；当前这一代的解析器 `on_network_change()`；当前注册表 `network_changed()`。`pub fn network(&self) -> Arc<NetworkFacts>`。
3. `runtime.rs` / `stack.rs`：建一代时，`ResolverConfig::from_config(cfg)` 之后取 `cfg.effective_ssid_setting(&shared.network.load())`，写了 `dns_server` 就替换 `servers`，写了 `encrypted_dns_server` 就替换 `encrypted`（设计第 7.3 节）；`Runtime` 记下它建时用的覆盖（`pub fn dns_override(&self) -> Option<(…)>`，比较用，`PartialEq`），供可执行文件判断要不要重建。`StackOptions` 不需要新字段（网络在 `EngineShared` 里）——若 `build_stack` 拿不到 `EngineShared`，实现者把生效的覆盖作为 `StackOptions` 的一个字段传入，在报告里说。

**用例**（`rurge-engine`）

- `a_network_change_flushes_the_resolver`：先查一个名字进缓存（回环上的假 DNS 上游），`network_changed` 之后缓存是空的、AAAA 抑制复位。
- `a_network_change_moves_the_subnet_group`：经引擎的拨号，换网络之前与之后选到不同的成员（回环上的两个假上游）。
- `the_ssid_setting_of_the_network_sets_the_dns_servers`：建一代时网络命中一行 `dns-server=` 的覆盖，解析器的上游名单（`primary_upstreams()`）是覆盖的服务器；不命中时是全局的；`encrypted-dns-server=off` 去掉全局的加密 DNS。
- `the_dns_override_of_a_generation_is_known`：`Runtime::dns_override()` 在两种网络下不同、在同一种网络下相同。

**判据**：用例绿；门禁绿。

## Task 6：可执行文件——网络任务、重建一代、接管暂停、离线命令、端到端

**改哪里**

1. `crates/rurge/src/cli/network.rs`（新）：网络任务（设计第 4 节）。启动：`netenv::backend_from_env()`（错误时 `rurge run` 以 2 退出，同 `RURGE_SYSTEM_PROXY_BACKEND`）→ 建第一代之前在阻塞线程上 `read` 一次、`EngineShared.network.store`。之后：`hints` → 阻塞线程上 `read` → 与上一次比（设计第 4 节第 2 步）→ 1 秒首发 + 尾发去抖（用 tokio 的时钟，用例可以暂停）→ 经 `run.rs` 的命令通道交给主循环应用（与重载、API 的命令共用同一个循环，一次一个）。
2. `run.rs`：主循环收到"网络变了"：`engine.network_changed(facts)` → 生效的 `[SSID Setting]` 行 → DNS 覆盖与当前这一代的不同时**用内存里的配置重建一代**（`build_engine_runtime((*engine.runtime().config).clone(), …)` + `swap_runtime`；`sources.track` 与系统代理的 `refresh` 照重载的后半段；失败时保留当前这一代并记 ERROR）→ 接管暂停（设计第 7.4 节）：进入时若系统代理开着就 `disable()` 并记下想要的状态，离开时按想要的状态 `enable()`；`Command::SystemProxy` 在暂停期间只改想要的状态并回成功；`--system-proxy` 启动时就在暂停的网络上同样只记下。INFO 日志照设计第 4、7.4 节；SSID 等只在 DEBUG。
3. `rule.rs`、`dns.rs`（离线命令）：读一次快照（认文件后端），交给规则引擎与解析器（`dns lookup` 也按生效的 `[SSID Setting]` 覆盖 DNS）。
4. `crates/rurge/tests/cli.rs`：`rurge_command()` / `rurge_process()` 缺省设 `RURGE_NETWORK_BACKEND=file:<草稿目录>/network.json`（每个调用一个独立的路径；文件不存在），要模拟网络的用例在构造之后自己设一个写好的文件。CLAUDE.md 测试约定那一段在任务 7 记下这一条。

**用例**（`--test cli`，都用网络与系统代理的文件后端，给 `--data-dir`，只用回环）

- `a_network_change_moves_the_subnet_group_of_a_running_daemon`：起守护进程（网络 A），`GET /v1/policy_groups` 里 `subnet` 组的当前成员是 A 的；改写网络文件为 B，有界时间（去抖加轮询，至多几秒，按事件等、不固定 sleep）里变成 B 的。
- `the_ssid_setting_dns_servers_follow_the_network`：`[SSID Setting]` 一行给网络 B `dns-server=127.0.0.1:<端口>`（回环上不会被问到的端口；只看名单），换到 B 之后 `GET /v1/dns` 的 `upstreams` 是它，换回 A 之后是全局的。
- `the_system_proxy_pauses_on_a_suspend_network`：`--system-proxy`（文件后端）起在网络 A；换到写了 `suspend=true` 的 B → 系统代理的文件回到原设置、`GET /v1/features/system_proxy` 是 `false`；暂停期间 `POST` 打开 → 回成功、文件不变；换回 A → 文件里又是 rurge 的设置。
- `rule_match_reads_the_network`：`rurge rule match` 按网络文件匹配 `SUBNET` 规则。
- `an_unknown_network_backend_is_refused`：`RURGE_NETWORK_BACKEND=bogus` 时 `rurge run` 以 2 退出、说只认 `file:<path>`。
- 网络任务的去抖（单元用例，暂停的时钟）：1 秒内三次变化 → 应用两次（首发与尾发）、第二次用最新的快照。

**判据**：用例绿（计时的单独循环跑二十次以上）；门禁绿。

## Task 7：文档

- 兼容性清单：设计第 9 节列的各行（状态、备注、行为变化：`subnet` 组没写 `default` 是加载错误、临时覆盖、`[SSID Setting]` 的读法与桌面上 `suspend` 的读法、SSID 拿不到的平台限制、网络变化的下游），四处写得过于肯定的措辞（L266、L334、L712、L741，行号以清单当时为准，按内容找）；附录的统计按清单的规矩重数。
- `docs/api/phase3.md`（新建）：`subnet` 组的选择与临时覆盖；接管暂停时 `GET` / `POST /v1/features/system_proxy` 的行为；网络任务的日志行。
- `docs/acceptance/phase3-manual.md`（新建）：设计第 9 节的手工项，三个平台各一节加"与平台无关"一节，每项写操作、期望与怎么看（日志行、API）。
- CLAUDE.md：状态一段（M1 完成的样子与没做的）、文档列表加本计划与 M1 设计、常用命令加 M1 的过滤词、测试约定加"起二进制的辅助函数缺省设网络的文件后端"、`RURGE_NETWORK_BACKEND` 一句（同 `RURGE_SYSTEM_PROXY_BACKEND`）。
- 本计划末尾的两张表填齐。

**判据**：`git grep` 核对文档里引用的用例名与路径都存在；门禁绿（文档不影响，但跑一次确认没有别的变化）。

---

## 计划期决定

| 编号 | 事项 | 决定 |
| ---- | ---- | ---- |
| P1 | `ipconfig` 0.3.4 的网卡类型（V1） | `IfType::Ieee80211`（71）是 `Wifi`；`EthernetCsmacd`（6）是 `Wired`；`SoftwareLoopback`（24）与 `Tunnel`（131）跳过；其余（含 `Unsupported`）是 `Other`。`gateways()` 同时含 IPv4 与 IPv6 的网关（取自 `FirstGatewayAddress`），`ipv4_metric()` / `ipv6_metric()` 是 `u32`。注意 `ipconfig` 遇到不认识的 `OperStatus` 会 panic（它自己的代码），读取放在 `catch_unwind` 之外的专用线程里，线程 panic 当作读不到 |
| P2 | 用内存里的配置重建一代（V2） | `Config` 是 `Clone`；`build_engine_runtime(cfg, …)` 收值；重建照重载的后半段：`swap_runtime`、`sources.track(&source)`（同一份来源，等于不变）、系统代理 `refresh`（设置不变，等于不变）、监听器的表面没变不重绑 |
| P3 | 自动组的轮次（V3） | `AutoGroups.rounds: HashMap<String, Instant>`；`evaluate-before-use` 的等待条件是 `last.is_none()`（`registry.rs` 里 `pending: spec.test.evaluate_before_use && last.is_none()`），所以清掉轮次就让下一次使用等一轮；`smart` 的分支同样看 `last`，所以只清 `url-test` / `fallback` / `load-balance` 的 |
| P4 | 注册表里的出站（V4） | `enum Entry` 的 `Outbound { outbound, .. }`；导入与派生的条目也是这一种（实现者核对，不是的话在报告里说） |
| P5 | `EvalCtx` 的构造点（V5） | `rurge-rules` 内外共 18 处（`matcher.rs`、`set.rs`、`engine.rs`、基准、`tests/memory.rs`、`rurge-dns/src/hosts.rs`）；只有 `engine.rs` 的两处改用带网络的构造器 |
| P6 | 带时限执行的用例（V6） | 再起当前测试二进制（`std::env::current_exe()`）跑一个忽略的用例，那个用例只在 `RURGE_TEST_HOLD=1` 时阻塞（等一个永远不来的通道），否则立刻返回；不起系统自带的 `sleep` / `ping` 之类（三个平台不一样） |
| P7 | macOS 的路由套接字（V7） | `socket2` 0.6.5 有 `impl From<c_int> for Domain` 与 `for Type`；`libc` 0.2.189 有 `AF_ROUTE`、`RTM_NEWADDR`（0xc）、`RTM_IFINFO`（0xe）、`RTF_LLINFO`（0x400）、`RTF_WASCLONED`（0x20000）、`RTM_ADD`（0x1）与 `rt_msghdr`（`rtm_type` 在第 4 字节、`rtm_flags` 是偏移 8 的 `c_int`）；本机编译不到，第一次 CI 运行时核对 |
| P8 | `netsh` 在没有无线服务时（V8） | 退出码非 0 或输出里没有 `SSID` 行：一律"没有 Wi-Fi"；不解析提示文字 |
| P9 | `GET /v1/dns`（V9） | 已给出 `upstreams`（`Resolver::primary_upstreams()`），任务 6 的用例读它 |
| P10 | 暂停与恢复系统代理（V10） | `SystemProxyManager::disable()` 恢复原设置并清掉备份；`enable(settings)` 重新快照并备份；`flag()` 给 API 读真实状态。暂停就是 `disable()`，恢复就是 `enable(current_settings)`；"想要的状态"放在主循环里 |

## 执行期修正记录

第 1 ～ 14 行是主持这一轮的会话在执行中代项目所有者作的裁定（所有者可以改），每行写着被它改掉的计划或设计的哪一句、代价是什么；第 15 行起是实现者做的、与计划或设计的文字不同的地方。

| # | 任务 | 修正 | 提交 |
| - | ---- | ---- | ---- |
| 1 | 1、2 | `rurge-platform` 不依赖任何别的 crate（需求文档 3.2 节"无内部依赖"，其它平台类型也都经可执行文件里的适配器到达引擎）：`netenv` 有自己的 `Snapshot`、`Primary`、`Wifi`、`NetKind`（带 serde，文件后端用），由可执行文件映射成 `NetworkFacts`（`cli::network::facts_of`）；设计里的 `Snapshot { facts: NetworkFacts }` 与计划要求的"`NetworkFacts` 带 serde"撤回，`rurge-config` 的 `NetworkFacts` 没有 serde；代价：两套并行的类型和一个映射函数 | `a6a9ff1`、`0ea4938` |
| 2 | 3 | `EngineShared.network` 在任务 3 加，不在任务 4：任务 3 是第一个要在引擎里建 `RuleEngine` 的任务，按计划的次序它得先接一个用完就扔的空格子 | `8279b7a` |
| 3 | 5 | `[SSID Setting]` 的 DNS 覆盖是 `StackOptions` 的一个字段（`dns_override`），在 `build_stack_with` 里 `ResolverConfig::from_config` 之后、`customize` 钩子之前套用——`Runtime::build` 与 `rurge dns lookup` 共用这一处，`--server` 仍然最后说了算；`Runtime::dns_override()` 返回这一代建的时候用的那份（`&DnsOverride`）。计划写的是在 `Runtime::build` 里套用、返回 `Option<(…)>`，而 `dns lookup` 不经 `Runtime::build`；代价：`StackOptions` 多一个字段 | `b775bd1` |
| 4 | 2 | 计划里"在本平台上真读一次快照不 panic"的用例不断言内容；`reading_this_machine_does_not_panic` 照样读真实的机器（只读），但断言在任何机器上都成立的不变量：被排除的网卡不出现；有主网卡时名字非空、不是回环；每个网关都不是未指定地址。什么也不断言的用例是评审标准里的缺陷 | `0ea4938` |
| 5 | 3、4 | `RuleEngine::build`、`build_with_registry` 与 `PolicyRegistry::build` 的签名不变（合计约四十处调用点，多在测试里）：加建造者 `with_network(Arc<NetworkCell>)`，`subnet-exp-wifi-always-match` 从它们本来就收的 `Config` 里读；没调 `with_network` 的引擎或注册表看到"什么都不知道"的网络（`SUBNET` 永不命中，`subnet` 组用 `default`）；代价：漏调的调用点不声不响地看不到网络（任务 5、6 的端到端用例覆盖引擎与 `rurge rule match` 两条路径） | `8279b7a`、`297c11e` |
| 6 | 1 | 子网表达式写错时，`E0011` 与 `SsidSetting` 的 `Debug` 只在表达式能被引用时才引用它（`spec::reader::quotable`：没有空白、`=`、全角的 `＝` 与 `，`，至多 64 个字符）。表达式是第一个认识的键之前的全部文字，键拼错时整个参数——可能是带口令的 DNS URL——并进了它；"凭据不进诊断"的规矩压过计划写的"`E0011` 的文本同 `subnet` 组今天的写法"。代价：不能被引用的表达式得到一句不带任何文字的错误 | `afb3ecb` |
| 7 | 2 | `Snapshot.addresses` 不是原样的地址表：IPv4 地址原样，IPv6 只留 /64 前缀（链路本地的不要），排序去重。IPv6 的隐私地址每隔几小时自己变，原样比较会让"网络变化"每天来几次（清 DNS 缓存、作废测速、清 `subnet` 组的覆盖、让 WireGuard 换载体）；设计 3.1 节写的是"主网卡的地址"；代价：同一个 /64 里只有接口标识变了的变化看不到 | `0ea4938` |
| 8 | 2 | Linux 问 NetworkManager 的命令是 `nmcli -t -f ACTIVE,SSID,BSSID,DEVICE dev wifi list --rescan no`，不是设计写的 `dev wifi`（列表比 30 秒旧时 `nmcli` 默认会发起一次 Wi-Fi 扫描，笔记本上就是每 30 秒一次对着无线电要一份用不着的答案）；子进程的环境变量是 `LC_ALL=C.UTF-8`：`nmcli` 把 `yes` 译成使用者的语言，C 区域没有翻译，但 C 区域的字符集是 ASCII，含非 ASCII 字母的网络名很可能读成 `?`（据对 glib 的了解推断，没有在 Linux 上量过）；没有 `C.UTF-8` 的系统退回 C | `1f434b5`、`12217f4` |
| 9 | 2 | Windows 上 `cmd`、`chcp`、`netsh` 一律按 `<SystemRoot>\System32` 下的绝对路径起，并设 `NoDefaultCurrentDirectoryInExePath=1`（评审时从次要升为重要：增强模式下 rurge 以管理员身份运行，`cmd` 不能先去当前目录里找 `netsh`）；`SystemRoot` 不是朴素的路径（盘符、冒号、反斜杠，再是由字母、数字、`_`、`-`、`.` 组成的名字）时按没有设处理，退回 `C:\Windows`——这个值要进 `cmd` 读的命令行，放着不查，一个被篡改的 `SystemRoot` 就是命令注入 | `1f434b5` |
| 10 | 3 | 三个拿 `SUBNET` 当"运行期不支持的规则类型"的现有用例改了：`unsupported_and_never_kinds` 与 `unsupported_kinds_note_once_and_count_hits` 改用仍然不支持的 `DEVICE-NAME`；`a_note_of_the_pre_pass_is_surfaced_once_and_reaches_the_decision` 保留用途，在测试模块里把第一条规则的匹配器换成 `Matcher::Unsupported`（现在没有"允许 `pre-matching`、运行期又不受支持"的规则类型了，加载器会拒绝带 `pre-matching` 的 `DEVICE-NAME`） | `8279b7a` |
| 11 | 4 | `only_a_member_can_be_selected`（`crates/rurge-engine/tests/outbounds.rs`）对 `subnet` 组的断言从 `NotSelectable` 改为非成员的 `NotAMember`：`subnet` 组现在能临时覆盖，旧期望与设计矛盾；`NotSelectable` 的文字与别的断言不变（现有的组类型已没有一个能返回它，变体留着当护栏） | `67fb3c0` |
| 12 | 4 | `AutoGroups` 记一个网络世代（`epoch`）：换网络那一刻正在跑的一轮测速，结束时不算新网络上的一轮——否则它给 `rounds` 盖章，换网络要求的重测被跳过，`evaluate-before-use` 的拨号也等不到新的一轮。按组判而不是一个全局的比较，免得拖住不该动的 `smart` 组。另外 `resolve_ready` 在旧的一轮结束后再请求一轮（`AutoGroups::wake` 改成 `pub`），否则等第一轮的拨号会把整个 `round_timeout` 等满（测到 5.07 秒）再带着旧结果继续 | `2890cf2` |
| 13 | 6 | 因 DNS 覆盖变了而用内存里的配置重建一代时，不调 `ProfileSources::track`，也不刷新系统代理：计划的 P2 假定这两步等于不变，其实 `track(&同一份来源)` 不是空操作——它重置 `outdated_for`，并放掉在用的一代没有点名的来源（一次失败的重载之后临时跟上、等着下载的那些），换网络于是会取消使用者需要的下载；这一次重建配置没变，来源也没变 | `1f44b33` |
| 14 | 6 | 隐藏的 `--network-backend <file:path>`（`RuntimeArgs` 里，`env = RURGE_NETWORK_BACKEND`，`hide`；`rurge run`、`rule match`、`dns lookup`、`dns cache` 共用）：`external` 策略拉起的程序拿不到名字以 `RURGE_` 开头的变量（`is_own_setting`），嵌套的 `rurge run`（用例 `run_starts_an_external_program_and_stops_it_on_exit`）于是没有文件后端、会去读本机的网络。选项在命令行上取代变量，未知取值与变量一样报同一句并以 2 退出；`is_own_setting` 不动。此前"起守护进程的用例从不读本机的网络"说得过宽，这个嵌套的进程是例外；任务 6 的评审发现了它 | `ea9353c` |
| 15 | 2 | 三个平台的读取器没有各自按 `#[cfg(target_os)]` 编译，而是都在每个宿主上编译、运行时按 `dirs::Os` 选，读取器收一个注入的"机器"（`Host`），只有 `RealHost` 碰系统——同 `sysproxy`、`sysinfo` 的做法。Linux 与 macOS 的读取逻辑因此全在开发机上被假机器（`FakeHost`）跑过；本机编译不到的只剩 `windows.rs` 里 `#[cfg(not(windows))] system_adapters` 三行与 `command.rs` 里 `Tree` 的非 Windows 分支约八行 | `0ea4938` |
| 16 | 2 | Windows 的 `netsh` 经 `cmd /d /c "chcp 65001 >nul & netsh …"` 问：核对开发机（系统语言是中文）上的真实输出发现，管道里是 GBK、键被翻译，含非 ASCII 字母的网络名会读坏；经 UTF-8 的控制台问，键是英文、值按 UTF-8 写。`run_with_timeout` 在 Windows 上给子进程 `CREATE_NO_WINDOW`，并把它放进 Job Object，超时时连它启动的进程一起结束 | `388d98c` |
| 17 | 2 | macOS 的路由消息一阵合成一次提示（静下来 300 毫秒才发，连续不停也至多拖 1 秒），不是每条相关的消息一次提示：一次网络切换在 macOS 上是几十条消息，每个提示都要起三四个子进程 | `0ea4938` |
| 18 | 2 | BSSID 的八位组补齐前导 0（`0:0:5e:0:53:2` 读成 `00:00:5e:00:53:02`，只对六段十六进制的写法）：macOS 的工具少写前导 0，而求值一侧按字符串比，同一个 BSSID 要在各平台读成一个样子 | `0ea4938` |
| 19 | 6 | "网络变化之后比较 `Runtime::dns_override()` 与网络给的 DNS"这一问放在两处：网络变化之后（起作用的一处），和一次成功的重载之后——后一处其实触发不了：一代总是建在引擎的网络格子里的网络上，而格子只由主循环写。留着它，是为了对任何新一代问同一个便宜的问题 | `1f44b33` |
| 20 | 6 | `GET /v1/policy_groups` 不带当前选择：设计 6.3 节写的是它"给出 `subnet` 组的成员表与当前选择"，其实这个端点对所有的组都只给成员表，当前选择是 `GET /v1/policy_groups/select`；任务 6 的用例读后者，API 文档 `docs/api/phase3.md` 照实写 | `1f44b33` |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | Linux 与 macOS 的 `netenv` 读取与路由套接字没有碰过真实的系统（读取器在开发机上编译、对着假的机器测过；细节见第 3 行） | 第一次 CI 运行时核对 |
| 2 | `netsh`、`nmcli`、`iw`、`route`、`networksetup`、`ipconfig getsummary` 的输出夹具是按公开格式写的，非英文系统与各版本的真实输出没有核对 | 手工验收（`docs/acceptance/phase3-manual.md`） |
| 3 | Linux 与 macOS 的读取只对着假的机器测过。读取器三个平台都编译、运行时按 `Os` 选，所以它们的逻辑（路由表、`/sys`、`nmcli` / `iw` / `route` / `networksetup` / `ipconfig getsummary` 的调用顺序与解析、缓存、排除）在开发机上被 `FakeHost` 跑过；没有跑过的是它们与真实系统的接触：真实的 `/proc` 与 `/sys` 的内容，`nmcli` 真实的列顺序，`iw` 的输出，`route -n get -inet6 default` 里 `gateway: fe80::1%en0` 的写法，`networksetup` 的端口名，`ipconfig getsummary` 里 `SSID :` 一行的空白与 BSSID 是否少写前导 0，以及 `PF_ROUTE` 套接字的建立（`socket(17, SOCK_RAW, 0)`）与消息头的偏移（类型在第 4 字节、标志在偏移 8）。夹具是按公开的输出格式写的，没有从任何一台真机上抄 | 第一次 CI 运行时核对（冒烟用例 `reading_this_machine_does_not_panic` 在 Linux 与 macOS 的 runner 上读真实的机器） |
| 4 | Windows：非英文系统上 `netsh wlan show interfaces` 的两个键（`SSID`，`BSSID` 或 Windows 11 的 `AP BSSID`）。开发机（系统语言是中文，Wi-Fi 当时断开）上量过：管道里是 GBK、键被翻译，经 `chcp 65001` 之后键是英文、没有替换字符；含非 ASCII 字母的真实 SSID 没有量过 | 手工验收（`docs/acceptance/phase3-manual.md` 的 Windows 一节） |
| 5 | Linux：glibc 早于 2.39 的系统上，`LANGUAGE` 设了时 `nmcli` 可能仍把 `yes` 译成使用者的语言（`LC_ALL=C.UTF-8` 挡不住 `LANGUAGE`），`ACTIVE` 一列读不出，退到 `iw`；补救是给 `nmcli` 的子进程把 `LANGUAGE` 清空，一行 | 手工验收（Linux 一节）；真有人撞上再改 |
| 6 | Linux：NetworkManager 的笔记本上，rurge 没有因为问 `nmcli` 而发起 Wi-Fi 扫描（`--rescan no`）——用 `iw event` 对比有无 rurge 时的扫描事件——没有量过 | 手工验收（Linux 一节） |
| 7 | macOS：本地化的硬件端口名可能让 `TYPE:WIRED` 判不出来（按英文单词 `Ethernet` / `LAN` 判）；没有位置授权的守护进程读不到 SSID（`ipconfig getsummary` 写 `<redacted>`），要拿到只能另做一个带位置授权的 App 包 | 手工验收（macOS 一节）；App 包是留给项目所有者的问题（总设计 Q6） |
| 8 | 平台层的次要项（任务评审留下的，整分支终审决定改不改）：超时杀孙进程没有永久的用例，`ProcessTree::contain` 的错误被丢掉；几处读不到的项没有 DEBUG 行；两块无线网卡都连着时各平台的取法不同（Windows 可能把一块网卡的 SSID 安到另一块上）；macOS 上 `networksetup` 失败一次就没有旧的端口表可用（那一次读到的类型是 `other`）；去抖窗口里的每个提示都触发一次完整读取（macOS 上每次要起三四个子进程，设计 3.3 节要的是窗口内至多一次） | 整分支终审决定 |
| 9 | 配置层的次要项：`SSID:Home, suspend=true`（第一个参数前是逗号而不是空白）里逗号成了表达式的一部分，这一行永不命中，没有诊断；`quotable` 仍会引用含 `://` 的短表达式（比如把键拼成 `dns-sever:https://…`），`is_name` 对 `://` 的判据能挡住它；`SubnetSpec` 与 `PolicyGroup` 的 `Debug` 把 `subnet` 组条件的表达式原样打出来；裸值的通配每次求值都重建一个 `Glob`；`is_parameter_name` 与 `is_plain_name` 几乎重复 | 整分支终审决定 |
| 10 | 引擎与可执行文件的次要项：`DnsOverride` 比较的是原始列表，一行恰好列了 Profile 自己的服务器时，进出这样的网络会多重建一次（无害）；为 DNS 覆盖重建的一代不打印新一代的诊断（启动与重载打）；离开暂停时只在"想要开着"时重新打开，从不为"想要关着"去关（只有进入暂停时 `disable()` 失败才碰得到）；暂停结束时重新指向 rurge 失败只记一条 ERROR，不重试，直到下一次进出暂停或一次 `POST`；换网络那一刻已经在进行的查找，旧网络上的答案可能落进刚清空的缓存（此前就有）；一代在换网络的过程中建好时，它的解析器在构造时读的系统 DNS 服务器与 IPv6 状态是旧的，要等下一次变化；`Engine::network_changed` 的文档没写"总是清缓存、只该在真有差别时调"与"读系统 DNS 服务器会短暂阻塞" | 整分支终审决定 |
| 11 | 测试的次要项：没有用例钉住集合文件里的 `SUBNET` 行随引擎求值（读代码得出），也没有跨两代的用例；没有用例持着一个会话跨过 `network_changed`；`network()` / `at_home()` 一类的夹具与 `on_wifi` / `on_cable` 在几个测试二进制里各抄了一份；跨两次换网络的一轮测速没有用例；`TestBook` 对同一个策略一次只测一个，换网络后新开的一轮会与旧网络上还没结束的那一个测试共用结果；`rurge dns cache` 也收 `--network-backend`，没有用例驱动它，非 UTF-8 的 `RURGE_NETWORK_BACKEND` 由 clap 自己的话拒绝 | 整分支终审决定 |
| 12 | 一次全量运行里 `rurge-api` 的测试二进制以 `STATUS_ACCESS_VIOLATION`（0xc0000005）退出（任务 6 修正轮的第一次全量，跑到 56 个里的第 40 个），单独重跑五次、下一次全量都通过；原因没有查明。开发机上已查明的是另一个代码 `STATUS_HEAP_CORRUPTION`（管控软件注入的钩子 DLL） | 再出现时先单独重跑；重复就查 |
| 13 | `[SSID Setting]` 的 DNS 覆盖今天只管解析器的 `dns-server` 与 `encrypted-dns-server` 两张表；DNS 应答器的转发（总设计 Q-24）还没有，应答器本身在 M3 | M3 |
| 14 | 接管暂停的增强模式一半；排除 rurge 自己的虚拟网卡（`ReadOpts.exclude` 已留好入口，今天传空表）；`network-changed` 事件脚本；Logbook 里记网络变化 | 前两项 M4，事件脚本阶段 5，Logbook 阶段 6 |
| 15 | `[Host]` 键里的 `RULE-SET` / `DOMAIN-SET`（`rurge-dns` 用 `EvalCtx::new` 建上下文）看不到网络，那里的 `SUBNET` 行永不命中，也不再有"not supported"的提示 | 有人写了这样的 Profile 再说 |
| 16 | 主网卡是默认路由所在的网卡，不分物理与虚拟：全隧道的 VPN 接管默认路由后，主网卡就是 VPN 的网卡，`TYPE:` 与 `ROUTER:` 看的是它；Windows 只跳过类型 131 的隧道网卡，别的类型的 VPN 网卡（PPP、TAP）有网关又度量低时同样成了主网卡；Linux 上 `type` 为 1 的虚拟网卡（如 `tap`）算 `WIRED` | 手工验收（与平台无关一节）；排除别的 VPN 没有计划，排除 rurge 自己的虚拟网卡是 M4 |
