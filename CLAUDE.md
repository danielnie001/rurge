# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

这里只放规则与概况，每次会话都整份载入。每个阶段、里程碑的进度与交接写在 `docs/status.md`，不写进这里。

## 项目是什么

rurge 是用 Rust 复刻 Surge（macOS / iOS 网络代理工具）全部功能的跨平台核心，原生兼容 Surge 的 `.conf` / `.sgmodule` 配置格式、JavaScript 脚本 API 与 HTTP API。目标平台 Windows / Linux / macOS；产品形态是单二进制守护进程 `rurge` + CLI + Surge 兼容 HTTP API，Web Dashboard 与桌面 GUI 放在后期阶段。

## 现在在哪里

- 阶段 1（核心骨架）与阶段 2（出站协议与策略组）的功能做完了，验收没有完成：三份手工验收清单（`docs/acceptance/phase*-manual.md`）一项也没有做过。
- 阶段 1 的需求逐句对过代码（`docs/acceptance/phase1-acceptance.md`），结果是三个数，要一起说：2 句没有做完、37 句阶段栏与登记不一致、10 句有意不同。阶段 2 逐条的证据在 `docs/acceptance/phase2-acceptance.md`。
- 阶段 3（增强模式 / TUN）的 M1（网络环境与子网）、M2（进程识别）、M3（内存设备上的 TUN 内核）、M4（平台接入：M4a 平台无关的骨架、M4b 防环与接管期间的 DNS、M4c Linux 与 macOS、M4d Windows 与收尾）与 M5（识别、阻断与 `[IP Rewrite]`：所有端口上的 QUIC 与 STUN、`block-quic` 的 `adaptive` 与 6.11 的 `auto`、`[IP Rewrite]` 生效、`udp-priority` 的两条写回队列、内部 DNS 会话带主机名、手册 6.11 的 WireGuard 服务端写法能加载、三个基准重跑）都做完了（M5 做完于 2026-10-28）。三个平台都有了平台后端的代码（Windows 不再答“不可用”，它建 Wintun 的虚拟网卡）：照代码会建虚拟网卡、配地址与 MTU 1500、装路由、接管系统 DNS，Linux 上按需改 `rp_filter`，关闭或崩溃之后的下一次启动把它们还回去。**三个平台的代码都在 CI 上编译、缺省用例通过（运行 40611702392，提交 `3a76b80`，2026-10-28；Linux 与 macOS 专有的代码——`tun/linux.rs`、`tun/macos.rs` 与挑平台的 `cfg` 等——是第一次被编译），但缺省的用例不建网卡、不改系统设置：三个平台的设备与系统改动都没有真的运行过，没有在真机上验证**；Windows 上要一个 WireGuard LLC 签名的 `wintun.dll` 0.14 或更新、以管理员运行；L2 特权用例写好了没有运行过。M5 的提交（`e6666c1` 之后）都在本地、**CI 没有见过**（`origin/main` 仍是 `3a76b80`）；`[IP Rewrite]` 只在内存设备上证明过，`udp-priority` 的效果没有观察过。**下一步是 M6：阶段 3 的需求逐句对照代码、验收文档与手工验收（阶段 1、2、3 三份清单一项也没有做过，M5 一节这一轮才写）——需求没对完，所以阶段 3 不称齐备（「称齐备」）。**照项目所有者 2026-10-26 的指示，M4d 与 M5 由协调的会话逐个任务调 Pi 做、逐个任务评审；协调者代项目所有者作的裁定在 `docs/status.md` 的 M4d 与 M5 两节列着，所有者可以改。细节与交接在 `docs/status.md` 的 M4a ～ M4d 与 M5 各节；设计是 `docs/superpowers/specs/2026-10-26-phase3-m4-platform-design.md`（先读它第 21 节与末尾「执行期的订正」一节）与 `docs/superpowers/specs/2026-10-28-phase3-m5-recognition-blocking-ip-rewrite-design.md`（先读它第 20、21 节的订正）。
- CI 最近一次运行在 `3a76b80`（运行号 40611702392，2026-10-28）：三个平台的 `check` 与 `bench` 都通过（第二次全绿；第一次是 `ac0a98a` 上的 40283632986）；注解只有平台的弃用警告与通知。此后的提交 CI 都没有见过。
- 推送由项目所有者做。先 `git rev-parse origin/main` 看推送到了哪里；推送之后先读那次运行的注解与任务摘要（公开可读；任务日志要登录才能读），修掉它列出的失败。

## 文档在哪里

- `docs/requirements.md`：需求文档（PRD）。功能需求 `FR-<模块>-<序号>` 在第 4 节（带优先级与所属阶段），8 个阶段的路线图与验收标准在第 7 节。
- `docs/surge-compatibility-matrix.md`：逐项兼容清单。实现任何 Surge 特性之前先查这里；与 Surge 的任何行为差异必须登记在这里。
- `docs/superpowers/specs/`：设计，每个阶段一份总设计、每个里程碑一份细化设计。设计末尾的"订正"各节记着实施期对正文的改动，读正文之前先看它们。
- `docs/superpowers/plans/`：实施计划，末尾有「执行期修正记录」与「延后事项」两张表；别处写"记录第 N 行""延后第 N 行"，指的是这两张表的行。
- `docs/acceptance/`：需求逐句对照代码的结果与手工验收清单；`docs/api/`：按阶段的 HTTP API 参考；`docs/reports/`：手册重对、调研与可行性报告。
- `docs/testing.md`：各模块的测试命令、只供测试的后端与测试约定的全文。
- `docs/README.md`：上面每一份文档的说明。
- `README.md`（中文）与 `README_en.md`（英文）：对外的状态、特性表与路线图，两份内容保持一致，并与 PRD、`docs/acceptance/phase2-acceptance.md` 保持一致。

## 工作流约定

- 每个阶段：先写设计文档（`docs/superpowers/specs/YYYY-MM-DD-<topic>-design.md`），再写实施计划（`docs/superpowers/plans/`），再实现。新工作必须能对应到 PRD 第 7 节的某个阶段和第 4 节的 FR 编号。
- 兼容性原则：Surge 语法就是 rurge 语法。不认识或平台不适用的配置项要"解析并忽略 + 记录日志"，不能报错；rurge 不改写用户的配置文件；rurge 专有的运行时选项只通过命令行参数和环境变量提供，不扩展 Surge 配置格式（FR-CFG-17）。
- 事实来源：Surge 官方手册 <https://manual.nssurge.com/>（本项目对照的是 2026-10-28 复查时取的手册文本，对应 Surge Mac 6.11.0 / iOS 5.24.0：目录里照旧是 87 页，其中 83 页与 2026-10-14 的基线逐字节相同、4 页改过；87 页的文本怎样取、每页的摘要见 `docs/reports/2026-10-14-manual-baseline-recheck.md`，这次复查见 `docs/reports/2026-10-28-manual-6.11-recheck.md`）。手册改版频繁，有疑问查手册，不凭记忆；页面路径也换过，策略页在 `policies/<协议>.html`（如 <https://manual.nssurge.com/policies/snell.html>）。已知的常见误区：`doh-server` 已改名 `encrypted-dns-server`；URL Rewrite 只有 `header` / `302` / `307` / `reject` 四种模式；`[MITM]` 没有 `tcp-connection` 键；url-test 组的测试 URL来自策略的 `test-url` 或全局 `proxy-test-url`。
- 语言：文档与对话用中文；README 分中文（`README.md`）与英文（`README_en.md`）两份；日志与 CLI 输出以英文为主（NFR-10）。
- 提交：用户自行决定何时 commit；未被要求时不要提交。
- 记录：计划末尾两张表里已写下的行不改写，推翻它的行写明它覆盖哪一行；历史的段落也不逐处改，由后写的段落订正。新写的设计、计划、报告在 `docs/README.md` 加一项。
- 进度：收尾一个里程碑时，做了什么、没做的与不要当成已经有的、以后的会话必须知道的事、去向写着项目所有者的事，写进 `docs/status.md` 它的阶段下新的一节，细节指到计划的两张表。`CLAUDE.md` 只改「现在在哪里」；新的规矩才进「必须遵守的规矩」。

## 架构

- Rust stable，MSRV 1.89（CI 钉在 1.89.0），Cargo workspace，按职责拆 crate：`rurge-config`（解析 / 校验 / include / 模块叠加 / 托管配置）、`rurge-rules`、`rurge-dns`（客户端 / 加密 DNS / `[Host]` / fake-IP）、`rurge-policy`（策略组 / 测试 / 订阅）、`rurge-proto`（出站协议；按传输族另有 `rurge-proto-quic`、`rurge-proto-ssh`、`rurge-proto-wireguard`）、`rurge-net`（内部 HTTP 客户端 / 外部资源管理 / QUIC 与 HTTP/3 的共用部分）、`rurge-inbound`、`rurge-engine`（会话流水线 / 请求记录 / 运行时状态）、`rurge-tun`（虚拟网卡 / 协议栈 / 网关 / DHCP）、`rurge-api`、`rurge-platform`、`rurge`（bin）；`rurge-http`（HTTP 引擎 / MITM / 重写 / 抓包）与 `rurge-script` 到后面的阶段才建。`tests/interop` 与 `tests/external` 是只用于测试的工作区成员。职责与依赖见 PRD 3.2；平台特定代码只允许出现在 `rurge-platform` 与 `rurge-tun`（AR-02）。
- 生产代码的依赖方向（事实以各 `Cargo.toml` 为准；哪个里程碑加了哪条边记在 `docs/status.md` 的附录）：`rurge-config` 与 `rurge-platform` 不依赖任何工作区 crate；只有可执行文件依赖 `rurge-platform`，平台层用自己的类型，由可执行文件映射成配置与引擎的类型；`rurge-api` 只依赖 `rurge-engine` 与 `rurge-config`，不认识可执行文件；`rurge-tun` 不依赖 `rurge-engine`、`rurge-platform`、`rurge-rules`——引擎依赖它、实现它的 `Hooks`，预匹配的 `PreMatchLayer` 因此放在 `rurge-config`；`rurge-dns` 不依赖协议 crate，QUIC 的共用部分在 `rurge-net::quic`。
- 加载器（`rurge-config`）自己从不联网：下载到的文本经调用方给的 `RemoteSource` 取，被 include 的文本能做什么由 `Reach` 定（见「必须遵守的规矩」）。
- 连接处理流水线（PRD 3.3）：入站 → 协议嗅探（SNI / Host / QUIC / STUN）→ 预匹配 → 出站模式判断 → 规则匹配（域名规则不触发 DNS，IP 规则按需解析）→ 策略解析（组 / 链式 / 别名）→ 出站建立 → HTTP 引擎（MITM → Header Rewrite → URL Rewrite → Body Rewrite → 脚本 → Map Local 短路）→ 观测。
- 配置对象不可变，重载时原子切换（AR-04）；每个连接是独立 tokio 任务（AR-03）。
- `unsafe_code`：全工作区 `forbid`；只有 `rurge-platform` 用 crate 自己的 `[lints] unsafe_code = "deny"` 放宽，且只有二十三个函数标 `#[allow(unsafe_code)]`：`sysproxy::windows` 里调用 `InternetSetOptionW` 通知 WinINet 的 `notify_wininet`（阶段 1 / M4b），`process` 里创建 Job Object、设"关闭即结束全部进程"、把外部程序放进去的 `job_for`（阶段 2 / M4c，M4-D6），`procinfo::windows` 里的 `tcp_table`（只包 `GetExtendedTcpTable`）、`udp_table`（只包 `GetExtendedUdpTable`）与 `image_path`（包 `OpenProcess` 与 `QueryFullProcessImageNameW` 两次调用，接住句柄的 `from_raw_handle` 也在里面），`procinfo::macos` 里的 `pcblist`（`sysctlbyname`）与 `pid_path`（`proc_pidpath`）（阶段 3 / M2，M2-D15），`tun::linux` 里的 `set_iff`（只包 `ioctl(TUNSETIFF)`）、`tun::macos` 里的 `utun_connect`（包 `ioctl(CTLIOCGINFO)`、`connect` 与 `getsockopt(UTUN_OPT_IFNAME)`）（阶段 3 / M4c），`socket` 里的 `unicast_if`（只包一次 `setsockopt`，设 `IP_UNICAST_IF` / `IPV6_UNICAST_IF` 让套接字从指定的网卡出去）（阶段 3 / M4d，设计第 7.4 节、M4-D11），`process` 里的 `in_job`（包 `OpenProcess`、零等待的 `WaitForSingleObject` 与 `IsProcessInJob` 三次调用，进程句柄接进 `OwnedHandle` 离开函数就关）（阶段 3 / M4d，设计第 11 节、M4-D21），`tun::windows::wintun` 里的 `verify_signer`（包 `WinVerifyTrust` 两次——一次校验、一次关掉它开的状态——以及从状态数据取签名者的证书、`CertGetNameStringW` 取名字）与 `load`（包 `LoadLibraryExW`、`GetProcAddress` × 11 与把 `FARPROC` 转成各入口的函数指针）（阶段 3 / M4d，设计第 3.8、8.4 节、M4-D13），同一个文件里的 `create_adapter`（只包 `WintunCreateAdapter`）、`open_adapter`（只包 `WintunOpenAdapter`，探测网卡名）、`close_adapter`（只包 `WintunCloseAdapter`）、`adapter_index`（包 `WintunGetAdapterLUID` 与 `ConvertInterfaceLuidToIndex`）、`start_session`（只包 `WintunStartSession`）、`end_session`（只包 `WintunEndSession`）、`read_event`（只包 `WintunGetReadWaitEvent`）、`wait_readable`（只包 `WaitForSingleObject`）、`receive`（包 `WintunReceivePacket`、拷出与 `WintunReleaseReceivePacket`）、`send`（包 `WintunAllocateSendPacket`、拷入与 `WintunSendPacket`）（阶段 3 / M4d，任务 5，设计第 8.1、8.4 节）。这张名单由 `crates/rurge-platform/tests/unsafe_functions.rs` 的 `every_unsafe_exception_is_on_the_list` 钉住（常量 `LISTED`）：它读 `src/` 下的全部源码，找出每个 `#[allow(unsafe_code)]` 和紧跟它的 `fn` 的名字，与名单比对，多一个、少一个、标在 `fn` 之外的东西上都让它变红；任何在 `//` 注释之外提到 `unsafe_code`、而去掉首尾空白后不恰好是 `#[allow(unsafe_code)]` 的一行也让它变红（信息带文件和行号），所以被 rustfmt 拆成多行的属性逃不掉，要在源码里提这个词就写在 `//` 注释里；加一个放开的函数要改这张名单，并在提交说明与这一条里说。每个 `unsafe` 块前一行 `// SAFETY:` 写明前提。macOS 的三个函数（`procinfo::macos` 里的 `pcblist`、`pid_path` 与 `tun::macos` 里的 `utun_connect`）与 Linux 的 `set_iff` 在开发机上编译不到，只有 CI 编译（`pcblist` 与 `pid_path` 从运行 40283632986 起，`utun_connect` 与 `set_iff` 从运行 40611702392 起）。

## 必须遵守的规矩

- **项目所有者**：设计与计划里写着"代项目所有者作的裁定"的，是在所有者"自己决定、继续做"的常设授权下定的，所有者可以改。去向写着"项目所有者"的事，后来的会话不要自己定：列出来，交给所有者。
- **称齐备**：`docs/status.md` 里的一句状态不是某条需求已经做好的证据。一个阶段要在它的需求逐句对照过代码之后才称齐备，不是在它的里程碑都合并之后；收尾一个阶段的会话，先拿这个阶段的需求对一遍代码，再写"齐备"。
- **照实记录**：没有证据的照实写，不写成通过。本机的全量运行写明是一台机器上的一次运行、不是 CI；需要参照服务端的互操作用例在本机算在通过的数里（见「这台开发机」），要说出来；没有在某个平台上编译、运行过的代码，照实说。
- **凭据**（项目所有者的规矩）：凭据不进日志、错误文本、API 输出与 `Debug`。新的带凭据的字段或写法写进 `crates/rurge-config/tests/secrets.rs`。托管配置的链接整个当作凭据，最多显示到 `scheme://host[:port]/…`（`rurge_config::remote::origin`）；订阅的链接不进日志。脱敏后的视图（`crates/rurge-config/src/redact.rs`）是一张"藏什么"的清单，尽力而为，不是没有秘密会出现的保证：不要把它说成可靠的，也不要自己去追更多的拼法；已知没有覆盖的写法在 M8c 计划「延后事项」第 58 ～ 77 行，要不要换成只显示已知无害内容的视图归项目所有者（第 51 行）。
- **下载到的文本**：下载到的文本不因为被 include 而多出权力。以后任何把 Profile 文本变成本机路径或程序的东西，先问 `Reach`。托管配置的副本不 include 本地文件；后来的会话不要不问项目所有者就把这个例外加回来（去掉的代码在 `a5dd842` 的历史里）。
- **一行怎样拆成各项**：解析器（`crates/rurge-config/src/value.rs`）与脱敏（`redact.rs`）的读法在同一个提交里一起改。
- **诊断**：新的诊断码在 `codes::hint` 的表里做一个决定（缺省建议 / 每处自带 / 不设并写理由），否则 `every_code_is_decided_once` 变红。加新的"不适用 / 没有效果"告警之前先问 `ParamReader::has_unrejected`：已经报错的参数不再多一条告警。
- **日志的分级**：进程（名字、路径、PID）与网络（SSID、BSSID、网关）在日志里只进 DEBUG；虚拟网卡与应答器的行里的名字与地址也只在 DEBUG。各日志行在哪一级说什么见 `docs/api/phase3.md`。已知的两处例外归项目所有者：被预匹配拒绝的名字随会话日志行进 INFO（M3c 计划延后第 6 行），INFO 里的规则文字带着 `PROCESS-NAME` 的值（M2 计划延后第 20 行）。
- **网络与进程**：`Engine::network_changed` 总是清缓存、不比较前后的网络，只在网络真的变了时调（调用方是可执行文件的网络任务）。会话的进程在 `SessionHandle` 的进程槽里（`process()`），要看进程的东西读同一个槽，不另起查找。
- **`rurge-tun`**：停止取消正在转发的会话，不取消正在进行的拨号；结束一个会话靠取消它的令牌，不靠丢掉拨号或转发的 future（丢掉的话请求记录会一直开着），所以 M4 的可执行文件给等 `join` 的那一步一个自己的时限。起 TUN 之前先调 `TunOptions::check`（`Tun::start` 对不合格的选项 panic）。接收缓冲不超过 `RX_MAX`（65535）：窗口边缘的守卫靠窗口缩放为 0。TIME-WAIT 里没读的数据交给流（`Leftover`），不要只修结局——那会把看得见的丢失变成悄悄的截断。其余的（保活与超时、DNS over TCP 的关法、`udp_dns_idle` 不大于 `udp_idle`、一个 TUN 一个应答器等）写在 `docs/status.md` M3a、M3b、M3c 各节"必须知道的事"里，改那一块代码之前读对应的一节。
- **`rurge-proto-wireguard` 的协议栈**：`stack.rs` 的 `TCP_BUFFER` 有一个读源码得出、没有观察过的 panic 风险（M3a 计划延后第 1 行），修法归项目所有者选，后来的会话不要自己动。
- **出站套接字只有一处建法**：rurge 自建的出站套接字都由 `rurge-net` 的 `new_socket` 建，建好之后向平台问出口、按回答绑网卡（`bind_egress`）；`open_socket` 是这两步连在一起的那一个，`DirectPacket` 每个数据报自己调这两步。ICMP 的套接字由 `rurge_platform::icmp` 建、可执行文件的 `PlatformPinger` 问出口。活得比一次会话长的套接字（WireGuard 的载体、QUIC 的连接）在出口表变了之后靠连接器的 `route()` 重建：新加这样的套接字要么同样跟着 `route()`、要么在表变时自己重建。新加建套接字、问系统解析库或建不带钩子的连接器（`NoopSocketHook)`）的地方，进 `crates/rurge/tests/sockets.rs` 的名单并写理由；那一条扫描用例逐处比对，同一个文件里同一种写法的第二处也要一行，多了少了都变红（M4b 任务 9 与 M4b 计划「执行期修正记录」第 68、69、73 行）。

## 常用命令

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --no-fail-fast   # 全量门禁：每个任务结束、说"通过"之前跑一遍
cargo test --workspace <过滤词>                 # 按用例名选；优先这样写，-p 会另编一份依赖（见「这台开发机」）
cargo test -p rurge-config <name>               # 单个 crate / 用例
cargo insta test -p rurge-config --review       # 语料库快照有变化时审阅
cargo clippy --all-targets -- -D warnings       # 零警告（CI 设 RUSTUP_TOOLCHAIN=1.89.0，与本机的 MSRV 工具链一致；更新的 clippy 会多出 lint）
cargo fmt --all
cargo run -p rurge -- check -c config.conf      # 校验 Surge 配置（--json / --strict / --platform；--data-dir 连同已缓存的订阅与已下载的 Profile 来源——托管配置的副本、远程 include——一起检查，不联网，也不创建或写数据目录）
cargo run -p rurge -- rule match -c config.conf example.com --explain   # 离线测试规则匹配
cargo run -p rurge -- dns lookup -c config.conf example.com --trace    # 离线 DNS 解析（--server 覆盖上游）
cargo run -p rurge -- run -c config.conf --log-level info   # 前台运行 HTTP / SOCKS5 代理（Ctrl-C 退出）
cargo run -p rurge -- run -c config.conf --watch --log-file rurge.log   # 加配置热重载（SIGHUP / --watch）与滚动日志文件
cargo run -p rurge -- run -c config.conf --system-proxy   # 前台运行并把系统代理指向 rurge，退出时恢复；崩溃后在下次启动时恢复
cargo run -p rurge -- status -c config.conf --data-dir ./data   # 查看运行中实例的状态（reload / stop 同样支持 -c 或 --remote/--key；托管配置生效时 http-api 写在数据目录里的副本中，三个命令用 --data-dir 找到它，缺省是平台的数据目录）
cargo run -p rurge -- service install -c config.conf --user --dry-run   # 打印开机自启的安装计划（去掉 --dry-run 才真正写入 / 执行）
```

各模块的测试命令（按 crate 与功能的过滤词、基准、互操作的参照服务端）在 `docs/testing.md`。只供测试的后端 `RURGE_SYSTEM_PROXY_BACKEND`、`RURGE_NETWORK_BACKEND`、`RURGE_PROCESS_BACKEND`、`RURGE_ENHANCED_BACKEND` 与隐藏的 `--network-backend` 不要在正常使用中设置，取值见同一份文件。

`Cargo.lock` 需要提交（`.gitignore` 已注明）。

## 测试约定

这里是通用的规矩。各模块（`rurge-tun`、经引擎的 TUN 与 DNS 用例、网络环境、进程识别）的细则与每条规矩的来由在 `docs/testing.md` 的「测试约定（全文）」，写那一块的用例之前读对应的一节；改一条规矩，两处一起改。

- 测试只用回环地址和端口 0，失败时也不离开回环、不离开自己的临时目录。Profile 里写了主机名的用例另给第二道防线（一条按地址的规则、一行把名字钉到 `127.0.0.1` 的 `[Host]`，或一个会拒绝的连接器）；回环之外的地址（文档地址也算）不写成可能被拨的目标。
- 缺省的 `cargo test` 不改系统设置：系统代理走文件后端，从不执行路由与 DNS 的命令、从不建网卡；要特权的用例只在 CI 虚拟机里、由环境变量放开（阶段 3 总设计第 13 节）。
- 要先于主人绑定就写进配置的端口、或必须一直关着的端口，取自 20000–29999 并现场试绑一次（`closed_tcp_port()` / `closed_udp_port()`、`free_port()`、`free_tcp_port()`）；"一个没有应答的地方"是测试自己持有的监听器，不是某个固定端口。
- 起 `rurge` 二进制的用例一律经 `crates/rurge/tests/cli.rs` 的 `rurge_command()` 或 `rurge_process()`（它们隔开缺省的数据与配置目录，把网络指到一个不存在的文件，设 `RURGE_PROCESS_BACKEND=off`，设 `RURGE_ENHANCED_BACKEND=off`），并给 `--data-dir`。`external` 策略拉起的嵌套 `rurge run` 拿不到以 `RURGE_` 开头的变量，要在 `args` 里带 `--network-backend file:<一个不存在的文件>`。
- 在线跑起来的引擎或守护进程把 GeoIP 的两个下载地址指到回环（引擎用例的 `geo_urls`，命令行的 `--geoip-url` / `--geoip-asn-url`）。建资源管理器的测试照 `crates/rurge-net/src/resource/mod.rs` 的 `manager()` 与 `NoNames` 来建；真的下载的用例用回环上的 `TestServer`。
- 调真正的系统解析库的用例只问 `localhost`（现有五条），新的用例不把别的名字交给它。
- 做"改一行代码看用例变红"的验证之前，先说清这一改会让哪些会话、查找、下载与文件去哪里；可能离开回环或写到用例的临时目录之外的，不做。
- 等待：对守护进程做事之前先等它说准备好了；"断言什么都没发生"的等待是仅有允许的固定等待，注释写它等的是什么、余量多大；会让一个工作线程停住的用例在用例自己的线程上按系统时钟等，不等运行时的定时器；新写的计时用例提交前单独循环跑二十次以上。
- 计数：数"服务器被问了几次"的用例用不重发的 `FanoutOpts`；经自动组拨号会在后台触发一轮测试，数成员失败次数的用例让成员不参加轮次（`UNTESTED`），新写的这类用例把 `NO_TEST` 指到关着的端口再跑一遍。
- 抓日志的用例用 `LogCapture`，跑在当前线程运行时（普通的 `#[tokio::test]`）。测试里手写的 HTTP 客户端把每个请求——请求头与正文——在一次写里发出。
- UDP：要发大于 9216 字节数据报的测试 socket 先调高发送缓冲（macOS 的缺省；例子是 `crates/rurge-engine/tests/common/mod.rs` 的 `udp_associate`）；从 `PacketSocket` 收包的缓冲给 64 KiB。

## 这台开发机

所有用例只在一台 Windows 11 x86-64 的开发机上跑过。

- 测试二进制偶发以 `STATUS_HEAP_CORRUPTION` 退出：来自开发机的终端管控软件注入的钩子 DLL，不是 rurge 或其依赖的缺陷（M8 设计 3.5），单独重跑那个二进制。`STATUS_ACCESS_VIOLATION` 见过一次（`rurge-api` 的测试二进制），单独重跑通过，原因没有查明（阶段 3 / M1 计划延后第 12 行）。
- rustc 1.89 在增量编译里出过内部编译器错误：删掉出错的那一个编译目标在 `target/debug/incremental/` 下的目录（可再生的缓存）再编译。
- 回环的 TCP / UDP 7、9、13、17、19 号端口开着（Windows 的 Simple TCP/IP Services），CI 上关着，所以测试不能假定任何固定端口的状态；关着的回环端口在 Windows 上约 2 秒才拒绝连接，在 Linux / macOS 上立刻拒绝。
- 需要参照服务端二进制的互操作用例在这台机器上立刻返回（那些服务端本机一个也没有）。
- `target/` 长得很快（`-p` 会另编一份依赖，两天攒到过 145 GiB，写满过 D: 盘）：跑全量门禁前先 `df -h /d`，剩余不足约 15 GiB 就停下。
- Windows 记在进程名下的 CPU 时间在这台机器上不可信，别凭任务管理器判断是不是在空转。
