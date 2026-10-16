# 阶段 3 / M1「网络环境与子网」细化设计

| 项       | 内容 |
| -------- | ---- |
| 日期     | 2026-10-16 |
| 状态     | 草案。已决事项 M1-D1 ～ D16 是在项目所有者"自己决定、继续做"的常设授权下定的，所有者可以改 |
| 上位设计 | [阶段 3 总设计](2026-10-16-phase3-enhanced-mode-design.md) 第 1.4、5、6、9.7 节；不一致处以本文为准 |
| 需求     | FR-IN-09（网络环境探测与网络变化事件）、FR-IN-10 的非 TUN 部分（`[SSID Setting]` 的 `suspend` 只做系统代理的一半）、FR-GRP-02（`subnet` 组）、FR-DNS-09（`[SSID Setting]` 的 DNS 覆盖）、FR-RULE-01 的 `SUBNET` |
| 清单     | 2.1 节 `subnet-exp-wifi-always-match`；3.1 节 `SUBNET`；3.4 节子网表达式；5.1 节 `subnet` 与临时覆盖；5.2 节 `default` / `cellular`；9.4 节 `[SSID Setting]`；10.4 节 `POST /v1/policy_groups/select` |

M1 不需要虚拟网卡，开发机上能完整测试（除了三个平台读网络环境的那几条命令与文件，它们用文本夹具测解析，真实的读取在 CI 与手工验收里核对）。它交付的东西对今天经系统代理与代理端口进来的流量就有用：`subnet` 组与 `SUBNET` 规则按当前网络选、换网络时清 DNS 缓存与作废测速、在指定的网络上换 DNS 服务器或暂停接管系统代理。

## 1. 范围

| 做 | 不做（去向） |
| -- | ------------ |
| `rurge_platform::netenv`：三个平台读网络快照（主网卡、网关、类型、DNS 服务器、Wi-Fi 的 SSID / BSSID）与变化的提示；只供测试的文件后端 | 排除 rurge 自己的虚拟网卡（M4：M1 时还没有虚拟网卡，留好入口） |
| 可执行文件里的网络任务：读快照、去抖、交给引擎、处理 `[SSID Setting]` 的 DNS 覆盖与接管暂停 | 接管暂停的 TUN 一半（M4） |
| 引擎对网络变化的下游：当前网络、DNS（清缓存、AAAA 复位、`system` 重新展开）、自动组的测速作废、`subnet` 组的临时覆盖清除、WireGuard 重建、日志 | `network-changed` 事件脚本（阶段 5）、Logbook（阶段 6） |
| `subnet` 组：按条件选、临时覆盖、没写 `default` 是加载错误 | |
| `SUBNET` 规则：按当前网络匹配，含预匹配 | DNS 层与 SYN 层的预匹配入口的其余扩展（M3） |
| `[SSID Setting]`：类型化、DNS 覆盖、接管暂停（系统代理）、`tfo-behaviour` 只解析 | `cellular-mode`（远期） |
| `rurge rule match` / `dns lookup` 读当前网络；`rurge check` 的诊断 | |
| 文档：兼容性清单、API 参考 `docs/api/phase3.md`（新建）、手工清单 `docs/acceptance/phase3-manual.md`（新建）、CLAUDE.md | |

## 2. 数据模型（`rurge_config::network`，新模块）

子网表达式在 `rurge-config` 里（`SubnetExpr`），规则（`rurge-rules`）与组（`rurge-policy`）都要用它求值，两者之间没有依赖，所以"当前网络"这个数据类型也放在 `rurge-config`：

```rust
/// What the subnet expressions look at (phase 3 design §5, §6.1).
pub struct NetworkFacts {
    /// The physical interface of the IPv4 default route — of the IPv6 one
    /// when there is no IPv4 default route; `None` when there is neither.
    pub primary: Option<Primary>,
    /// The Wi-Fi link, when an adapter is associated with an access point.
    pub wifi: Option<WifiLink>,
}

pub struct Primary {
    pub interface: String,
    pub kind: NetKind,          // Wifi | Wired | Other
    pub routers: Vec<IpAddr>,   // IPv4 default gateway first, then IPv6
}

pub struct WifiLink {
    pub interface: String,
    /// `None`: the platform would not say (no location permission, redacted).
    pub ssid: Option<String>,
    pub bssid: Option<String>,
}
```

`NetworkFacts::default()` 是"什么都不知道"（没有主网卡、没有 Wi-Fi），所有条件都不命中。

**求值**（`SubnetExpr::matches(&self, facts, wifi_always_match) -> bool`）照清单 3.4 节：

| 表达式 | 命中 |
| ------ | ---- |
| `SSID:<通配>` | `wifi.ssid` 存在且通配匹配（区分大小写），并且 `wifi_always_match` 或 `wifi.interface == primary.interface` |
| `BSSID:<通配>` | `wifi.bssid` 存在且通配匹配（不区分大小写），条件同上 |
| `ROUTER:<ip>` | `primary.routers` 里有这个地址（IPv4 或 IPv6） |
| `TYPE:WIFI` / `TYPE:WIRED` | `primary.kind` 是 `Wifi` / `Wired` |
| `TYPE:CELLULAR`、`MCCMNC:<…>` | 从不 |
| 裸值 | 含 `*` 或 `?` 时同 `SSID:`；否则依次比 SSID（精确、区分大小写）、BSSID（精确、不区分大小写）、网关地址，任一相等即命中；`SSID` / `BSSID` 那两项同样受 `wifi_always_match` 约束 |

`wifi_always_match` 是 `[General] subnet-exp-wifi-always-match`（缺省 `true`，已解析）。`TYPE:` 与 `ROUTER:` 只看主网卡（手册："Only the network of the outgoing interface is considered"）。

**当前网络放在哪里**：`rurge_config::network::NetworkCell`——一把 `std::sync::RwLock<Arc<NetworkFacts>>`，`load()` 取一份快照（克隆 `Arc`）、`store()` 换掉；`rurge-config` 不加依赖。引擎的 `EngineShared` 持有一个 `Arc<NetworkCell>`，跨代存续（同 `selections`、`auto`），建规则引擎与注册表时交给它们。

## 3. 读网络快照（`rurge_platform::netenv`）

### 3.1 快照

```rust
pub struct Snapshot {
    pub facts: NetworkFacts,      // what the subnet expressions look at
    pub dns: Vec<IpAddr>,          // the system's DNS servers, for change detection
    pub addresses: Vec<IpAddr>,    // the primary interface's addresses, likewise
}
pub fn read(opts: &ReadOpts) -> Snapshot      // blocking; may run commands
pub fn read_quick(opts: &ReadOpts) -> Quick   // no command on Windows and Linux
```

`read_quick` 只读便宜的来源（Windows 的 `ipconfig`、Linux 的 `/proc` 与 `/sys`），给出"要不要做一次完整读取"的指纹；完整读取才去问 Wi-Fi（要起子进程）。`ReadOpts` 带要排除的网卡名（M4 用来排除虚拟网卡；M1 是空的）。

### 3.2 各平台的来源

每一种文本都有一个纯函数解析器（平台作为参数或与平台无关，任何平台上都能用夹具测，同 `dns::hosts_path_for` 的写法）；只有把命令跑起来、把文件读进来的那一层按平台编译。

| 项 | Windows | Linux | macOS |
| -- | ------- | ----- | ----- |
| 网卡、地址、类型 | `ipconfig::get_adapters()`（已有依赖）：状态为开、类型 IEEE 802.11 是 `Wifi`、以太网类是 `Wired`、回环与隧道跳过 | `if-addrs`；`/sys/class/net/<网卡>/wireless`（或 `phy80211`）存在是 `Wifi`，`/sys/class/net/<网卡>/type` 为 1 是 `Wired` | `if-addrs`；`networksetup -listallhardwareports` 把设备名对到硬件端口（`Wi-Fi` 是 `Wifi`，名字里有 Ethernet / LAN 的是 `Wired`）；这张对照表只在网卡集合变了时重读 |
| 默认路由（主网卡与网关） | 有 IPv4 网关、状态为开的网卡里 `ipv4_metric` 最小的；IPv6 同样用 `ipv6_metric` | `/proc/net/route` 里目的与掩码都是 0、带网关标志、度量最小的一行（网关是小端十六进制）；`/proc/net/ipv6_route` 里目的长度为 0、下一跳不为 0 的一行 | `route -n get default` 的 `interface:` 与 `gateway:`；IPv6 用 `route -n get -inet6 default` |
| DNS 服务器 | 主网卡的 `dns_servers()` | `/run/systemd/resolve/resolv.conf`（systemd-resolved 记下的上游）存在时读它，否则 `/etc/resolv.conf` | `/etc/resolv.conf`（系统维护，反映主解析器） |
| Wi-Fi | `netsh wlan show interfaces`：取键（去掉空白）恰为 `SSID` 的一行与以 `BSSID` 结尾的一行（Windows 11 写 `AP BSSID`）；没有这两行时 SSID / BSSID 是 `None` | `nmcli -t -f ACTIVE,SSID,BSSID,DEVICE dev wifi`（简洁格式里值中的 `:` 写作 `\:`），没有 `nmcli` 时 `iw dev <网卡> link`（`Connected to <bssid>`、`SSID: <名字>`） | `ipconfig getsummary <Wi-Fi 设备>` 的 `SSID :` 与 `BSSID :` 两行；值是 `<redacted>` 或没有时是 `None` |

说明：

- Windows 上 `netsh` 的输出随系统语言翻译，`SSID` 与 `BSSID` 两个词据了解不翻译，但没有在非英文系统上核对过，进手工清单；Windows 11 在没有授予精确位置权限时不给 SSID / BSSID（调研报告 6.3 节），此时那两行没有或是提示文字，按 `None` 处理。
- macOS 14 起读 SSID 要位置授权，命令行守护进程多半拿不到（调研报告 6.3 节），`None` 是常态；`ROUTER:` 与 `TYPE:` 不受影响。
- 读不到的项（命令不存在、文件不在、解析不了）一律当作没有，DEBUG 里说一次原因，不报错。
- 子进程一律带时限（2 秒），时限到了结束它、当作读不到——`rurge_platform::command` 的执行器今天没有时限，M1 给它加一个带时限的入口（标准库的 `Child::kill` 与一个等待线程，零 unsafe）。

### 3.3 变化的提示与读取的频率

| 平台 | 提示 | 完整读取（问 Wi-Fi） |
| ---- | ---- | -------------------- |
| Windows | 每 2 秒 `read_quick`，指纹变了即提示 | 指纹变了时；另外有无线网卡连着时每 30 秒一次（同一个 SSID 下换 AP、地址不变） |
| Linux | 每 2 秒 `read_quick`（只读 `/proc` 与 `/sys` 下的文件） | 同上 |
| macOS | 一个 `PF_ROUTE` 套接字（经 `socket2` 建，`Domain::from(libc::AF_ROUTE)`，在一个专用线程上阻塞地收）：只看消息头里的类型——地址增删（`RTM_NEWADDR` / `RTM_DELADDR`）、网卡状态（`RTM_IFINFO`）、路由增删改里不带 `RTF_LLINFO` / `RTF_WASCLONED` 的（滤掉 ARP 一类的噪声）；另外每 30 秒一次兜底 | 每次提示与兜底都做（要起 `route` 子进程，所以提示经过上面的过滤；1 秒的去抖之内至多一次） |

NFR-02（空闲时 CPU 接近 0）：Windows 与 Linux 每 2 秒一次的快速读取不起子进程（`ipconfig` 是一次 `GetAdaptersAddresses` 调用，在 `ipconfig` 包里；Linux 读几个小文件）；起子进程的完整读取只在变化时与每 30 秒一次。

### 3.4 只供测试的文件后端

照 `RURGE_SYSTEM_PROXY_BACKEND=file:<路径>` 的先例：环境变量 `RURGE_NETWORK_BACKEND=file:<路径>` 把网络快照改从一个 JSON 文件读（形状同 `Snapshot`，序列化在 `rurge-platform`），每 100 毫秒看一次文件是否变了，从不读真实的网络；文件不在时是"什么都不知道"；取值不认识时可执行文件报错退出。`crates/rurge/tests/cli.rs` 的 `rurge_command()` / `rurge_process()` 缺省就把它指到构建自己的草稿目录下的一个文件（不存在），所以起守护进程的用例从不读真实的网络，真实的网络变化也不会在用例中途清 DNS 缓存、作废测速；要模拟网络的用例自己写那个文件。正常使用中不要设置它（同系统代理的文件后端）。

## 4. 网络任务（可执行文件）与去抖

`rurge run` 起一个网络任务（`crates/rurge/src/cli/network.rs`，新文件）：

1. 启动时（建第一代之前）同步读一次完整快照，存进 `EngineShared` 的 `NetworkCell`——第一代的 `subnet` 组、`SUBNET` 规则与 `[SSID Setting]` 的 DNS 覆盖从一开始就按真实网络；
2. 之后在后台：收提示（平台的或文件后端的）→ 在阻塞线程上读快照 → 与上一次比较 `NetworkFacts`、DNS 服务器与主网卡地址，任何一项不同即是一次变化；
3. **去抖**：1 秒的"首发 + 尾发"——第一个变化立刻应用并开一个 1 秒的窗口，窗口里再来的变化合并成窗口结束时的一次（用最新的快照）；
4. **应用一次变化**（按这个顺序）：`Engine::network_changed(facts)`（第 5 节）→ 重算生效的 `[SSID Setting]` 行（第 7 节）→ DNS 覆盖变了就按当前的配置重建一代（第 7.3 节）→ 接管暂停的状态变了就暂停或恢复系统代理（第 7.4 节）；
5. 一行 INFO：`network changed`，带主网卡名与类型、Wi-Fi 是否连着；SSID、BSSID、网关与地址只在 DEBUG 里（它们说明使用者在哪里）。

`rurge rule match` 与 `rurge dns lookup`（离线命令）各读一次完整快照（同样认文件后端），交给它们建的规则引擎与解析器；`rurge check` 不读。

## 5. 引擎对网络变化的下游

`Engine::network_changed(&self, facts: NetworkFacts)`：

1. `EngineShared.network.store(facts)`——之后的规则求值与组的选择看新的网络；
2. 当前这一代的解析器 `on_network_change()`（阶段 1 已有：清缓存、AAAA 抑制复位、`system` 重新展开、清 `[Host]` 上游的缓存）；
3. 自动组：`url-test`、`fallback`、`load-balance` 上一轮测试的时间清掉（`AutoGroups` 的 `rounds`），下一次使用时测一轮；结果本身留着，测完之前照旧用它们选——写了 `evaluate-before-use` 的组下一次使用时等这一轮（同第一次使用）。手册："A network change also invalidates the result."、"Surge retests when a group is used and the previous result has expired or the network has changed."。`smart` 不动（手册没有写，总设计 P3-D22）；
4. `subnet` 组的临时覆盖清除（第 6.2 节）；
5. 当前这一代的注册表里每个出站 `network_changed()`：`rurge_proto::Outbound` 加一个缺省什么也不做的方法，`wireguard` 的出站转调它已有的 `network_changed()`（阶段 2 留的入口：给每个 peer 换新载体）；别的协议不动（手册只写了 WireGuard 与 Tailscale）。

`Engine::network()` 给出当前的 `NetworkFacts`（API 与测试用）。

## 6. `subnet` 组

### 6.1 配置

`GroupSpec` 加 `subnet: Option<SubnetSpec>`：

```rust
pub struct SubnetSpec {
    pub conditions: Vec<(SubnetExpr, String)>, // in the order written
    pub default: String,
}
```

`GroupSpec.members` 对 `subnet` 组是"条件里的策略按书写顺序、再是 `default`"，去掉重复——成员表用于构建、`GET /v1/policy_groups` 的成员列表、临时覆盖的成员检查与环的检测（`group_edges` 已经把条件算进去）。**没写 `default`**：`E0018`（`subnet group \`<名字>\` needs \`default\``），组不建——手册写 `default` 必填，清单 L453 把它留给阶段 3 定；此前按空组兜底。`cellular` 照旧只解析（桌面上从不生效，🔁）。可执行文件的能力表加上 `subnet`，`W0008` 不再因它出现。

### 6.2 选择与临时覆盖

注册表选 `subnet` 组的成员：有临时覆盖时用它；否则第一个命中当前网络的条件的策略；都不命中时用 `default`。每次选择都读 `NetworkCell`，不存状态。

临时覆盖：`POST /v1/policy_groups/select` 对 `subnet` 组同自动组（M3b）——成员必须在 6.1 节的成员表里，空字符串清除，不写 `state.json`，重载时组的定义不变就保留；另外**网络变化时清除**（第 5 节第 4 步）：`subnet` 组按网络选，覆盖说的是"在这个网络上先用它"（rurge 的读法，手册只说自动类型的组"can be temporarily overridden"）。代码里"能不能覆盖"与"会不会测速"分成两个判断：`subnet` 能覆盖、从不测速。

### 6.3 API

`GET /v1/policy_groups` 给出 `subnet` 组的成员表与当前选择（同别的组）；`GET /v1/policy_groups/select` 同样。形状不变。

## 7. `[SSID Setting]`

### 7.1 一行怎样读

`[SSID Setting]` 从"解析并保留、没有作用"的节（`deferred.rs`）里拿出来，类型化成 `Config.ssid_settings: Vec<SsidSetting>`：

```rust
pub struct SsidSetting {
    pub expr: SubnetExpr,
    pub suspend: bool,                                 // absent = false
    pub dns_server: Option<Vec<DnsServer>>,            // as [General] dns-server
    pub encrypted_dns_server: Option<Vec<EncryptedDns>>, // `off` = Some(empty)
    pub tfo_behaviour: Option<TfoBehaviour>,           // parsed, no effect
    pub span: Span,
}
```

- **表达式与参数的分界**：参数从第一个"前面是空白、写成 `<认识的键>=`"的地方开始，表达式是它之前的部分去掉首尾空白——所以 SSID 里可以有空格（`SSID:My Home suspend=true`）。一个认识的键也没有的行：整行是表达式，没有参数（照收，什么也不做，一条告警）。
- **参数**：以逗号分开；一段写成 `<认识的键>=…` 的开始一个新参数，否则（`dns-server` / `encrypted-dns-server` 之后）是那个列表的下一项（清单 9.4 节的读法；`https://…/dns-query?a=b` 这样含 `=` 的 URL 不会被当成参数，因为 `=` 前面不是认识的键）。逗号与引号的处理用一行怎样拆成各项的扫描（引号只在一个值的开头开启引用）。
- **认识的键**：`suspend`（布尔）、`dns-server`（同 `[General] dns-server` 的读法与诊断，`system` 也认）、`encrypted-dns-server`（同 `[General]`，`off` 关掉加密 DNS）、`tfo-behaviour`（`auto` / `force-enabled` / `force-disabled`；rurge 没有实现 TCP Fast Open，同策略的 `tfo`：`W0029` 没有效果）、`cellular-fallback`（iOS 专属，同 `[General]` 里 iOS 专属的键的诊断）、`cellular-mode`（`W0029`，计费网络模式是远期）。不认识的键：`W0028`，不引用它（未知的键名可能是使用者写错的别的东西）。
- **表达式写错**（如 `TYPE:SATELLITE`）：同 `subnet` 组的条件，`E0011`，这一行不用。
- `[SSID Setting]` 里的 DNS URL 可能带口令：诊断照 `[General]` 的写法不引用凭据，脱敏（`redact.rs`）已认得这一节的列表（阶段 2 / M8a）；新的写法进 `crates/rurge-config/tests/secrets.rs`。

### 7.2 生效的一行

当前网络命中的第一行（按书写顺序；手册只对 `subnet` 组说了 "the first match wins"，`[SSID Setting]` 照它，rurge 的读法）。没有一行命中时是全局的设置。

### 7.3 DNS 覆盖

建一代时（`Runtime::build`），解析器的配置在 `ResolverConfig::from_config` 之后按生效的那一行替换：写了 `dns-server` 就整个替换全局的 `dns-server`，写了 `encrypted-dns-server` 就整个替换全局的（`off` 即没有加密 DNS）；没写的那一项保持全局的（手册："These replace the global values while connected to that network."；全局配了加密 DNS 而这个网络要用传统 DNS 时，必须写 `encrypted-dns-server=off`）。`[Host]` 里 `server:` 写明的服务器不受影响。

网络变化之后生效的一行变了、而且它的 DNS 覆盖与这一代建的时候不同：可执行文件**用内存里这一代的配置重建一代**并换上（同重载的后半段：`build_engine_runtime` + `swap_runtime`，不重读磁盘上的 Profile——网络变化不应顺带把使用者没让重载的改动生效）。新的一代建的时候读的就是新网络。重建与重载、托管配置的换代共用同一把顺序锁，一次一个。

`rurge dns lookup` 按读到的网络应用同样的覆盖。

### 7.4 接管暂停（系统代理的一半）

生效的一行写了 `suspend=true` 时**接管暂停**（总设计第 9.7 节；代码里叫 takeover pause，与托管配置 `strict` 的 `Engine::suspend`——拒绝新会话——分开）：

- 开着的系统代理恢复成使用者原来的设置（同关闭系统代理），记下"暂停之前开着"；
- 监听器与 API 照常，经代理端口来的请求照常处理（它们是使用者明确指过来的）；
- 离开这个网络（生效的一行不再写 `suspend=true`）时，暂停之前开着的重新打开；
- 暂停期间 `GET /v1/features/system_proxy` 给出真实状态（`false`）；`POST` 只改"想要的状态"、返回成功，暂停结束时按它办；启动时就在这样的网络上，`--system-proxy` 同样只记下想要；
- 暂停只在内存里，不进 `state.json`；进入与离开各一行 INFO（`takeover paused on this network` / `takeover resumed`）。

增强模式的一半在 M4。

## 8. `SUBNET` 规则

匹配器的 `SUBNET` 从 `Unsupported` 换成按当前网络求值（第 2 节的表）。规则引擎建的时候拿到 `Arc<NetworkCell>` 与 `subnet-exp-wifi-always-match`，求值时 `EvalCtx` 带上当前网络的一份快照（`EvalCtx` 的构造器加一个带网络的版本；不带网络的旧构造器是"什么都不知道"，测试与别的调用方照旧）。预匹配同样看当前网络（`SUBNET` 在手册支持预匹配的名单里）。可执行文件的 `INACTIVE_RULE_TYPES` 去掉 `SUBNET`，加载时那条"永不匹配"的告警不再出现。

## 9. 诊断、日志与文档

- 诊断：`subnet` 组没写 `default`（`E0018`）；`[SSID Setting]` 的表达式写错（`E0011`）、不认识的键（`W0028`）、没有参数的行与 `tfo-behaviour` / `cellular-mode`（`W0029`）、`cellular-fallback`（iOS 专属）；`codes::hint` 里每个新用法照规矩做一个决定。
- 日志：第 4 节；读不到的来源在 DEBUG 里说一次。
- 兼容性清单：2.1 节 `subnet-exp-wifi-always-match`；3.1 节 `SUBNET`；3.4 节的六种表达式；5.1 节 `subnet` 与临时覆盖（L455、L870）；5.2 节 `default`（L453）、`cellular`；9.4 节 `[SSID Setting]` 的五行（拆法、`suspend` 的桌面读法、DNS 覆盖、`tfo-behaviour`）；L449（网络变化触发的重测）、L435（WireGuard 的网络变化探测器）、L497（AAAA 抑制在网络变化时解除）；总设计第 2.1 节说的四处写得过于肯定的措辞（L266、L334、L712、L741）。
- `docs/api/phase3.md`（新建）：`subnet` 组的选择与临时覆盖、接管暂停时系统代理端点的行为、网络任务的日志。
- `docs/acceptance/phase3-manual.md`（新建）：三个平台各一节——真实的换网络（Wi-Fi ↔ 有线、换 Wi-Fi）、`subnet` 组与 `SUBNET` 规则随之变、SSID / BSSID 能不能拿到（Windows 的位置权限、以 SYSTEM 运行时、非英文系统上 `netsh` 的两个键；macOS 的位置授权；Linux 有无 NetworkManager）、`[SSID Setting]` 的 DNS 覆盖与接管暂停对真实的系统代理、睡眠唤醒之后的网络变化。
- CLAUDE.md：文档列表、状态一段、常用命令里 M1 的过滤词、`RURGE_NETWORK_BACKEND` 一句（同 `RURGE_SYSTEM_PROXY_BACKEND`）。

## 10. 测试

| 层 | 用例 |
| -- | ---- |
| `rurge-config` | `SubnetExpr::matches` 的表（每种表达式 × `wifi_always_match` 两个取值 × Wi-Fi 是否主网卡 × SSID 拿不到）；`[SSID Setting]` 的拆法（SSID 带空格、列表与下一个参数、含 `=` 的 URL、`off`、没有参数的行、不认识的键不被引用、表达式写错）；`subnet` 组的成员表与没写 `default`；凭据不外泄的标记用例加 `[SSID Setting]` 的 DNS URL |
| `rurge-platform` | 每一种文本的解析器对夹具（`netsh wlan show interfaces` 的英文与 Windows 11 写法、没有位置权限时的输出、`/proc/net/route`、`/proc/net/ipv6_route`、`nmcli` 的转义、`iw dev link`、`route -n get default`、`networksetup -listallhardwareports`、`ipconfig getsummary` 的 `<redacted>`）；Windows 的主网卡选择（纯函数，给一组网卡）；路由消息头的过滤；带时限的命令执行（对一个不会自己结束的子进程——测试只起本仓库构建的或系统自带的无害程序，见第 11 节 V6）；文件后端；在本平台上真读一次快照不 panic（只读） |
| `rurge-rules` | `SUBNET` 按当前网络命中与不命中、换网络之后同一个规则引擎的结果跟着变、预匹配里的 `SUBNET` |
| `rurge-policy` | `subnet` 组按条件选、`default`、临时覆盖的设置与清除、网络变化清除覆盖；测速轮次作废之后下一次使用测一轮、`evaluate-before-use` 的组等这一轮 |
| `rurge-engine` | `network_changed` 的下游：解析器的缓存清空与 AAAA 复位、自动组重测、假出站的 `network_changed` 被调到、`subnet` 组跟着变；`[SSID Setting]` 的 DNS 覆盖在建一代时生效（解析器的上游名单） |
| `rurge`（bin，`--test cli`） | 文件后端下的守护进程：换网络之后 `subnet` 组的当前选择变了（经 API 读）、DNS 覆盖生效（经 `GET /v1/dns` 读上游）、接管暂停对系统代理的文件后端（进入暂停时恢复原设置、离开时重新设上）、`rurge rule match` 按文件里的网络匹配 `SUBNET` |

所有用例照阶段 1 补遗（M5b）起的规矩：只用回环、端口 0、有界等待；起二进制的用例经 `rurge_command()` / `rurge_process()` 并给 `--data-dir`；计时的用例走暂停的时钟或在用例自己的线程上等，提交前单独循环跑二十次以上。

## 11. 写计划时要核对的事项

- V1：`ipconfig` 0.3.4 的 `IfType` 里无线与以太网的取值名，`gateways()` 是否含 IPv6 网关，`ipv4_metric()` 在没有网关的网卡上是什么。
- V2：`rurge_config` 的 `Config` 能否 `clone`（第 7.3 节用内存里的配置重建一代）；重载的后半段（`build_engine_runtime`、`swap_runtime`、`sources.track`）哪些步骤对"同一份配置重建"也要做。
- V3：`AutoGroups` 的 `rounds` 与覆盖表的接口，`evaluate-before-use` 看的是"没有上一轮"还是别的条件。
- V4：注册表里拿到"这一代的全部出站"的入口；导入与派生的条目是否也是 `Entry::Outbound`。
- V5：`EvalCtx` 的构造点（`rurge-rules` 内外），加带网络的构造器之后要改的调用方。
- V6：测带时限的命令执行要起一个"不会自己结束"的子进程——用当前测试二进制自己（`std::env::current_exe()` 加一个只在测试里认的参数）还是平台自带的程序；不得起本仓库之外的第三方程序。
- V7：`socket2` 0.6.5 能否以 `Domain::from(libc::AF_ROUTE)` 与 `Type::RAW` 建套接字并阻塞地收（macOS；本机编译不到，CI 核对）；`libc` 里 `RTM_*` 与 `RTF_*` 常数的名字。
- V8：`netsh wlan show interfaces` 在没有无线网卡、无线服务没开（Windows Server）时的退出码与输出；`ipconfig getsummary` 在没有连上 Wi-Fi 时的输出。
- V9：`GET /v1/dns` 是否给出解析器的上游名单（第 10 节的用例要读它）；没有的话用别的入口。
- V10：系统代理管理器（`SystemProxyManager`）暂停与恢复复用"关闭 / 打开"的哪一条路径，备份文件在暂停期间是否保留。

## 12. 已决事项

| 编号 | 事项 | 决定 |
| ---- | ---- | ---- |
| M1-D1 | 当前网络的数据类型放哪里 | `rurge_config::network`（`NetworkFacts`、`NetworkCell`）；规则与组都依赖 `rurge-config`，两者之间不依赖 |
| M1-D2 | 主网卡 | IPv4 默认路由的物理网卡（没有时取 IPv6 的）；Windows 按网卡度量选，Linux 按路由度量选，macOS 取 `route -n get default` |
| M1-D3 | 变化的提示 | Windows、Linux 每 2 秒快速读取比指纹（不起子进程）；macOS 路由套接字加过滤，30 秒兜底；Wi-Fi 只在变化时与每 30 秒读 |
| M1-D4 | 去抖 | 1 秒首发 + 尾发 |
| M1-D5 | 什么算一次变化 | `NetworkFacts`、DNS 服务器、主网卡地址任何一项不同 |
| M1-D6 | 测试后端 | `RURGE_NETWORK_BACKEND=file:<路径>`；起二进制的辅助函数缺省设它 |
| M1-D7 | 自动组 | 网络变化作废 `url-test` / `fallback` / `load-balance` 的轮次，结果留到新一轮测完；`smart` 不动 |
| M1-D8 | WireGuard | `Outbound` 加缺省空的 `network_changed()`，只有 `wireguard` 实现 |
| M1-D9 | `subnet` 组的成员表 | 条件的策略按顺序、再 `default`，去重 |
| M1-D10 | 没写 `default` | `E0018`，组不建 |
| M1-D11 | `subnet` 组的临时覆盖 | 同自动组；网络变化时清除 |
| M1-D12 | `[SSID Setting]` 的分界与列表 | 参数从第一个前面有空白的 `<认识的键>=` 开始；列表与参数照清单 9.4 节；不认识的键 `W0028` 不引用 |
| M1-D13 | 几行同时命中 | 第一行 |
| M1-D14 | DNS 覆盖怎样生效 | 建一代时替换；网络变化使覆盖变了就用内存里的配置重建一代，不重读磁盘 |
| M1-D15 | 接管暂停 | 系统代理恢复原设置、监听器与 API 照常；`POST` 只改想要的状态；不进 `state.json` |
| M1-D16 | 个人信息 | SSID、BSSID、网关与地址只在 DEBUG 日志里；API 不另开端点 |
