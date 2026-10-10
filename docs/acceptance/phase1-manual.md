# 阶段 1 系统代理与服务安装：手工验收清单

对应 `docs/requirements.md` 第 7 节「阶段 1」验收标准里「系统代理开关后浏览器流量经 rurge」一项，以及第 8 节「验收与测试策略」里的「手工验收」行：这类项目需要真实桌面环境（注册表 / `networksetup` / GNOME `gsettings` / KDE `kwriteconfig` / 系统服务管理器），M4b 的实施计划明确规定自动化测试不能触碰真实的系统代理设置或服务注册，所以只能靠人工在三个平台上各跑一遍。

设计文档 `docs/superpowers/specs/2026-09-07-phase1-m4-control-plane-design.md` 的开放问题 Q2（macOS 的 `networksetup` 是否需要管理员权限）在本分支的自动化测试里完全没有验证——全部走 `FakeRunner` / 文件后端——所以 macOS 一节多一条专项（第 8 条）。

阶段 2 / M8c 加了两项，同样只有真机验得了。三个平台各一条「配置加载不了时的恢复」：`rurge run` 在配置读不了、配置有错误或日志文件打不开时，退出之前也把上一次崩溃留下的系统代理恢复原值（`docs/api/phase1.md`「系统代理」一节）——自动化用例只经测试用的文件后端，真实的后端从没在这条路径上跑过。Windows 另有一条「睡眠唤醒后的解析器」：它与系统代理无关，列在这里是因为解析器是阶段 1 的组件，而真实的睡眠只能手工验（自动化用例用拨钟代替）。

## 前置条件

- `cargo build --release`，用得到的二进制（`target/release/rurge`，Windows 下是 `target\release\rurge.exe`）。下文按平台写作 `rurge.exe`（Windows）或 `./rurge`（macOS / Linux，按实际路径替换，或把二进制放进 `PATH`）。
- 一份测试用 Profile（下文的 `<conf>`），至少包含：

  ```ini
  [General]
  http-listen = 127.0.0.1:6152
  socks5-listen = 127.0.0.1:6153
  http-api = testkey@127.0.0.1:6171
  skip-proxy = 127.0.0.1, localhost

  [Rule]
  DOMAIN-SUFFIX,example.com,REJECT
  FINAL,DIRECT
  ```

  （`DOMAIN-SUFFIX,example.com,REJECT` 只是用来在第 2 条制造一个会被拒绝的请求，可以换成任何本机能访问、确定会被拒绝的域名。）
- macOS / Linux 需要一个真正登录的图形桌面会话（`rurge` 要能看到 `XDG_CURRENT_DESKTOP` 与桌面自己的 `gsettings` / D-Bus 后端；纯 SSH 远程登录可能看不到这些）。
- 每一步执行后，在对应行填写「结果」（通过 / 不通过，附简要说明）、「日期」与「系统版本」三栏。所有命令里的 `<conf>` 替换成实际的 Profile 路径。

---

## Windows

| # | 操作 | 期望结果 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- |
| 1 | 运行 `rurge.exe run -c <conf> --system-proxy` | 启动日志里出现一行 `system proxy enabled: http 127.0.0.1:6152, socks 127.0.0.1:6153`；「设置 → 网络和 Internet → 代理」的「使用代理服务器」显示 `127.0.0.1:6152`（或用 PowerShell `Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings'` 核对 `ProxyEnable` / `ProxyServer` / `ProxyOverride`） | | | |
| 2 | 浏览器分别访问一个走 DIRECT 的站点与一个命中 Profile 里 REJECT 规则的站点（如 `example.com`） | `rurge.exe status -c <conf>` 的活动请求数随访问变化；`GET http://127.0.0.1:6171/v1/requests/recent`（带 `X-Key: testkey`）里能看到这两条记录，`rule` / `policy` 分别对应 DIRECT 与 REJECT | | | |
| 3 | `curl.exe -X POST -H "X-Key: testkey" -d "{\"enabled\":false}" http://127.0.0.1:6171/v1/features/system_proxy`，确认后再用 `{\"enabled\":true}` 重新打开 | 关闭后注册表三个值回到原值；重新打开后再次指向 `127.0.0.1:6152` | | | |
| 4 | 先 Ctrl-C 终止一次；再启动一次，改成直接点右上角关掉终端窗口（不是 Ctrl-C） | 两种方式都能看到（或来不及看到，但结果一致）注册表恢复原值；关掉终端窗口那次不应留下一个仍然指向 rurge 的代理设置 | | | |
| 5 | 带 `--system-proxy` 启动后，用「任务管理器」结束 `rurge.exe` 进程（或 `taskkill /f /im rurge.exe`）；确认代理仍指向 rurge；再执行一次 `rurge.exe run -c <conf>`（这次不带 `--system-proxy`） | 强杀后注册表仍指向 rurge；下一次启动的日志里有一条 WARN（"a previous run left the system proxy pointing at rurge; restoring the saved settings"），随后注册表恢复原值；`<data-dir>\state.json` 的 `system_proxy_backup` 为 `null` | | | |
| 6 | 带 `--system-proxy` 运行中，编辑 `<conf>` 的 `skip-proxy`（加一条新域名），执行 `rurge.exe reload -c <conf>` | `ProxyOverride` 随之更新，包含新加域名对应的模式 | | | |
| 7 | `rurge.exe service install -c <conf> --user --dry-run` 看计划；确认无误后去掉 `--dry-run` 正式安装；重新登录 Windows；最后 `rurge.exe service uninstall --user` | dry-run 打印一条 `schtasks /create /tn rurge /sc onlogon …` 命令且不改变任何东西；正式安装后「任务计划程序」里出现名为 `rurge` 的任务；重新登录后该任务已启动 rurge（任务管理器里能看到 `rurge.exe`）；卸载后任务消失 | | | |
| 8 | **PAC 专项**：先在「设置 → 网络和 Internet → 代理」里打开「使用设置脚本」（即注册表的 `AutoConfigURL`，指向一个本机 `.pac` 文件即可），再执行 `rurge.exe run -c <conf> --system-proxy` | 记录浏览器实际走的是 PAC 还是 rurge。rurge 只读写 `ProxyEnable` / `ProxyServer` / `ProxyOverride`，既不清除也不备份 `AutoConfigURL`，因此预期是「看起来开着但不起作用」（PAC 优先）——确认这一行为并回填；退出后 `AutoConfigURL` 应原封未动 | | | |
| 9 | **仅 SOCKS 专项**：用一份没有 `http-listen`、只有 `socks5-listen = 127.0.0.1:6153` 的 Profile 执行 `rurge.exe run -c <conf> --system-proxy` | 记录 `ProxyServer` 的取值（应当只有 `socks=127.0.0.1:6153` 一段），以及浏览器 / `curl.exe` 是否真的能经这一段连上 rurge——基于 WinINet 的客户端历史上把 `socks=` 当作 SOCKS4 解释，而 rurge 的入站只讲 SOCKS5，这一条尚未在真机验证，结论回填后同步到 `docs/surge-compatibility-matrix.md` | | | |
| 10 | **实例锁**：带 `--system-proxy` 运行中，在另一个终端再执行一次 `rurge.exe run -c <conf>`（同一个数据目录——两次都不带 `--data-dir` 时就是默认数据目录）；随后 `taskkill /f /im rurge.exe` 强杀第一个实例，再执行一次 `rurge.exe run -c <conf>` | 第二个实例以退出码 1 结束，stderr 是 `error: another rurge instance is already running with the data directory <路径>`，且注册表仍指向 rurge（第一个实例完全不受影响，`GET /v1/features/system_proxy` 仍是 `true`）；强杀之后的那一次能正常启动，并先把注册表恢复原值（同第 5 条的 WARN） | | | |
| 11 | **配置加载不了时的恢复（阶段 2 / M8c）**：带 `--system-proxy` 启动后 `taskkill /f /im rurge.exe` 强杀，确认注册表仍指向 rurge；把 `<conf>` 改坏（删掉 `FINAL,DIRECT` 一行），执行 `rurge.exe run -c <conf>`，看过结果后原样再执行一次。**变体（日志文件打不开）**：把 `<conf>` 改回原样，重做「启动、强杀」，再执行 `rurge.exe run -c <conf> --log-file <conf>\rurge.log`（拿一个已有的普通文件当目录，日志文件必然打不开） | 改坏的第一次：stderr 先是诊断（`error[E0010]: missing FINAL rule`），随后一行 `warning: a previous run left the system proxy pointing at rurge; the saved settings were restored`，退出码 2；注册表三个值回到原值，`<data-dir>\state.json` 的 `system_proxy_backup` 为 `null`。第二次仍以 2 退出，stderr 里不再有 `system proxy` 的字样，注册表不变。变体：stderr 有同一行 `warning: … the saved settings were restored`，随后 `error: cannot open log file …`（日志库自己可能先往 stderr 打一行），退出码 2，注册表回到原值。阶段 2 / M8c 之前这几个出口不恢复，机器要一直指着死掉的端口到配置修好为止；这是真实的注册表后端第一次走这条路——注册表没有恢复，或出现的是 `warning: cannot restore the system proxy: <原因>; the saved settings stay in state.json and are restored on the next start` 时，记下原文（备份还在 `state.json`，配置修好后的下一次启动会再恢复） | | | |
| 12 | **睡眠唤醒后的解析器（阶段 2 / M8c 的修正；与系统代理无关）**：在 `<conf>` 的 `[General]` 里加一行 `dns-server = 8.8.8.8`（断网之后解析器仍有上游可问），执行 `rurge.exe run -c <conf> --log-level debug`（不带 `--system-proxy`）；断开网络（关掉 Wi-Fi 或拔网线）；另开一个 PowerShell 循环请求一个不存在的名字：``while ($true) { curl.exe -s -m 8 -o NUL -w "%{http_code} %{time_total}`n" -x http://127.0.0.1:6152 http://sleep-test.invalid/; Start-Sleep 1 }``；循环跑着的时候让机器睡眠，15 秒以上后唤醒，这样做三遍（要有一遍赶上一次还有重发没发出去的查找：每次查找在途约 5 秒，前 4 秒里都算）；接回网络，再看一分钟循环的输出后停掉它，经代理访问 `http://example.org/` | 唤醒后几秒内 rurge 的 CPU 占用回落到空闲水平（任务管理器「详细信息」页；不是一个核一直占满）。循环的每一行以 `502` 开头（例外见后）：网络断着的时候每个请求约 5 秒后结束（查找每秒重发一轮、共 5 轮，到 5 秒的时限才结束）；接回网络之后同一个名字的请求一秒之内就结束（`.invalid` 是保留的顶级域，上游答的是"没有这个名字"）。例外是睡眠那一刻正在途中的那一个请求：它可以是 `000`（curl 自己的 8 秒时限在睡眠里过完了）——要看的是它之后的几行，**不是**此后行行都是 `000`、`time_total` 都在 8 秒上下。`http://example.org/` 打得开。这一条是从代码与一条拨钟的单元用例（`rurge-dns` 的 `a_fanout_kept_from_running_until_past_its_deadline_ends_as_a_timeout`）推出来的，**没有在真机上睡眠验证过**：Windows 的单调时钟把睡眠的时间算进去，唤醒时在途的查找已经过了时限而还有重发没发出去——M8c 之前这样的查找占着一个核空转、永不返回，同一个名字之后的查找都挂在它上面，只有重启能了结。CPU 不回落，或唤醒之后这个名字的请求行行都是 `000` 而别的名字正常，就是这个缺陷还有别的入口：记下 Windows 版本与睡眠的方式（S3 / 现代待机），留下 `--log-level debug` 的日志 | | | |

---

## macOS

| # | 操作 | 期望结果 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- |
| 1 | `networksetup -listallnetworkservices` 找到当前启用的服务名（如 `Wi-Fi`），再运行 `./rurge run -c <conf> --system-proxy` | 启动日志里出现 `system proxy enabled: http 127.0.0.1:6152, socks 127.0.0.1:6153`；`networksetup -getwebproxy Wi-Fi`（换成实际服务名）显示 `Enabled: Yes` / `Server: 127.0.0.1` / `Port: 6152`。**可选**：把 `http-listen` 改成 `[::1]:6152` 再跑一次，记录 `networksetup -getwebproxy <服务>` 的 `Server` 是否是 `::1`——rurge 会把通配地址换成同族回环，但 `networksetup` 是否接受 IPv6 字面量没有在真机验证过 | | | |
| 2 | 浏览器分别访问一个走 DIRECT 的站点与一个命中 Profile 里 REJECT 规则的站点（如 `example.com`） | `./rurge status -c <conf>` 的活动请求数随访问变化；`GET http://127.0.0.1:6171/v1/requests/recent`（带 `X-Key: testkey`）里能看到这两条记录，`rule` / `policy` 分别对应 DIRECT 与 REJECT | | | |
| 3 | `curl -s -X POST -H 'X-Key: testkey' -d '{"enabled":false}' http://127.0.0.1:6171/v1/features/system_proxy`，确认后再用 `{"enabled":true}` 重新打开 | 关闭后 `networksetup -getwebproxy <服务>` 等回到原值；重新打开后再次指向 `127.0.0.1:6152` | | | |
| 4 | Ctrl-C 终止 `rurge run` | 终端打印 `system proxy restored`；`networksetup -getwebproxy <服务>` 回到原值 | | | |
| 5 | 带 `--system-proxy` 运行中，用「活动监视器」结束 `rurge` 进程（或 `pkill -9 -f "rurge run"`）；确认代理仍指向 rurge；再执行一次 `./rurge run -c <conf>`（不带 `--system-proxy`） | 强杀后 `networksetup -getwebproxy <服务>` 仍指向 rurge；下一次启动的日志里有一条 WARN（"a previous run left the system proxy pointing at rurge; restoring the saved settings"），随后设置回到原值；`<data-dir>/state.json` 的 `system_proxy_backup` 为 `null` | | | |
| 6 | 带 `--system-proxy` 运行中，编辑 `<conf>` 的 `skip-proxy`（加一条新域名），执行 `./rurge reload -c <conf>` | `networksetup -getproxybypassdomains <服务>` 随之更新，包含新加的域名 | | | |
| 7 | `./rurge service install -c <conf> --user --dry-run` 看计划；确认无误后去掉 `--dry-run` 正式安装；重新登录（或重启）；最后 `./rurge service uninstall --user` | dry-run 打印一份 `io.rurge.daemon.plist` 内容和一条 `launchctl bootstrap gui/<uid> …` 命令且不改变任何东西；正式安装后 `launchctl list \| grep rurge` 能看到该 Label；重新登录后 rurge 已在运行；卸载后 `launchctl list` 里不再出现 | | | |
| 8 | **专项（设计文档 Q2）**：分别用「管理员账户」与「标准（非管理员）账户」各执行一次第 1 步 | 记录两种账户下是否需要 `sudo` 才能成功、`networksetup` 的失败文案原文（若标准账户被拒绝，rurge 会把这段文案原样打到 `error: cannot enable the system proxy: …`） | | | |
| 9 | **实例锁**：带 `--system-proxy` 运行中，在另一个终端再执行一次 `./rurge run -c <conf>`（同一个数据目录——两次都不带 `--data-dir` 时就是默认数据目录）；随后 `pkill -9 -f "rurge run"` 强杀第一个实例，再执行一次 `./rurge run -c <conf>` | 第二个实例以退出码 1 结束，stderr 是 `error: another rurge instance is already running with the data directory <路径>`，且 `networksetup -getwebproxy <服务>` 仍指向 rurge（第一个实例完全不受影响，`GET /v1/features/system_proxy` 仍是 `true`）；强杀之后的那一次能正常启动，并先把设置恢复原值（同第 5 条的 WARN） | | | |
| 10 | **配置加载不了时的恢复（阶段 2 / M8c）**：带 `--system-proxy` 启动后 `pkill -9 -f "rurge run"` 强杀，确认 `networksetup -getwebproxy <服务>` 仍指向 rurge；把 `<conf>` 改坏（删掉 `FINAL,DIRECT` 一行），执行 `./rurge run -c <conf>`，看过结果后原样再执行一次。**变体（日志文件打不开）**：把 `<conf>` 改回原样，重做「启动、强杀」，再执行 `./rurge run -c <conf> --log-file <conf>/rurge.log`（拿一个已有的普通文件当目录，日志文件必然打不开） | 改坏的第一次：stderr 先是诊断（`error[E0010]: missing FINAL rule`），随后一行 `warning: a previous run left the system proxy pointing at rurge; the saved settings were restored`，退出码 2（`echo $?`）；`networksetup -getwebproxy <服务>` 等回到原值，`<data-dir>/state.json` 的 `system_proxy_backup` 为 `null`。第二次仍以 2 退出，stderr 里不再有 `system proxy` 的字样，设置不变。变体：stderr 有同一行 `warning: … the saved settings were restored`，随后 `error: cannot open log file …`，退出码 2，设置回到原值。这是真实的 `networksetup` 后端第一次走这条路：设置没有恢复，或出现的是 `warning: cannot restore the system proxy: <原因>; the saved settings stay in state.json and are restored on the next start`（`<原因>` 是失败的 `networksetup` 命令；标准账户下它若要授权，会落在这里，连同第 8 条一起记）时，记下原文 | | | |

---

## Linux

| # | 操作 | 期望结果 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- |
| 1 | `./rurge run -c <conf> --system-proxy` | 启动日志里出现 `system proxy enabled: http 127.0.0.1:6152, socks 127.0.0.1:6153`；GNOME 下 `gsettings get org.gnome.system.proxy mode` 返回 `'manual'`，`gsettings get org.gnome.system.proxy.http host`/`port` 是 `'127.0.0.1'`/`6152`；KDE 下 `kreadconfig6 --file kioslaverc --group "Proxy Settings" --key ProxyType` 返回 `1`，`httpProxy` 是 `http://127.0.0.1 6152`（工具是 Plasma 5 时把 `6` 换成 `5`） | | | |
| 2 | 浏览器分别访问一个走 DIRECT 的站点与一个命中 Profile 里 REJECT 规则的站点（如 `example.com`） | `./rurge status -c <conf>` 的活动请求数随访问变化；`GET http://127.0.0.1:6171/v1/requests/recent`（带 `X-Key: testkey`）里能看到这两条记录，`rule` / `policy` 分别对应 DIRECT 与 REJECT | | | |
| 3 | `curl -s -X POST -H 'X-Key: testkey' -d '{"enabled":false}' http://127.0.0.1:6171/v1/features/system_proxy`，确认后再用 `{"enabled":true}` 重新打开 | 关闭后 GNOME/KDE 的代理设置回到原值；重新打开后再次指向 `127.0.0.1:6152` | | | |
| 4 | Ctrl-C 终止 `rurge run` | 终端打印 `system proxy restored`；GNOME/KDE 的代理设置回到原值 | | | |
| 5 | 带 `--system-proxy` 运行中，`kill -9 $(pgrep -f "rurge run")`；确认代理仍指向 rurge；再执行一次 `./rurge run -c <conf>`（不带 `--system-proxy`） | 强杀后代理设置仍指向 rurge；下一次启动的日志里有一条 WARN（"a previous run left the system proxy pointing at rurge; restoring the saved settings"），随后设置回到原值；`<data-dir>/state.json` 的 `system_proxy_backup` 为 `null` | | | |
| 6 | 带 `--system-proxy` 运行中，编辑 `<conf>` 的 `skip-proxy`（加一条新域名），执行 `./rurge reload -c <conf>` | GNOME 的 `ignore-hosts` 或 KDE 的 `NoProxyFor` 随之更新，包含新加的域名 | | | |
| 7 | `./rurge service install -c <conf> --user --system-proxy --dry-run` 看计划；确认无误后去掉 `--dry-run` 正式安装；重新登录（或重启）；最后 `./rurge service uninstall --user`。另外再执行一次 `./rurge service install -c <conf> --system-proxy --dry-run`（**不带** `--user`，即 system 范围） | dry-run 打印一份 systemd unit 内容（含 `ExecStart=… --system-proxy`、`PartOf=graphical-session.target`、`After=graphical-session.target`、`WantedBy=graphical-session.target`、`StartLimitIntervalSec=60`、`StartLimitBurst=5`）和一条 `systemctl --user enable --now rurge` 命令且不改变任何东西；正式安装后 `systemctl --user status rurge` 是 running；重新登录（或重启）后 rurge 已在运行，**并且桌面的代理设置已经由这个服务指向 rurge**（GNOME 下 `gsettings get org.gnome.system.proxy mode` 为 `'manual'`、`.http host` 为 `'127.0.0.1'`；KDE 下 `ProxyType` 为 `1`）；卸载后该 unit 消失。不带 `--user` 的那一次必须以退出码 2 结束，stderr 是 `error: --system-proxy needs a desktop session: on Linux install with --user`，且不写任何文件 | | | |
| 8 | **额外检查**：在一个既不是 GNOME 也不是 KDE 的桌面（或没有安装 `gsettings`/`kwriteconfig6`/`5` 的环境）下执行 `./rurge run -c <conf> --system-proxy` | 命令以退出码 1 结束；stderr 有一行 `error: cannot enable the system proxy: no supported desktop proxy settings (GNOME or KDE) were found; set the proxy in your shell instead:` 并附一行 `export http_proxy=… https_proxy=… no_proxy=…` 提示；系统没有任何设置被改动 | | | |
| 9 | **实例锁**：带 `--system-proxy` 运行中，在另一个终端再执行一次 `./rurge run -c <conf>`（同一个数据目录——两次都不带 `--data-dir` 时就是默认数据目录）；随后 `kill -9 $(pgrep -f "rurge run")` 强杀第一个实例，再执行一次 `./rurge run -c <conf>` | 第二个实例以退出码 1 结束，stderr 是 `error: another rurge instance is already running with the data directory <路径>`，且 GNOME/KDE 的代理设置仍指向 rurge（第一个实例完全不受影响，`GET /v1/features/system_proxy` 仍是 `true`）；强杀之后的那一次能正常启动，并先把设置恢复原值（同第 5 条的 WARN） | | | |
| 10 | **配置加载不了时的恢复（阶段 2 / M8c）**：带 `--system-proxy` 启动后 `kill -9 $(pgrep -f "rurge run")` 强杀，确认 GNOME/KDE 的代理设置仍指向 rurge；把 `<conf>` 改坏（删掉 `FINAL,DIRECT` 一行），执行 `./rurge run -c <conf>`，看过结果后原样再执行一次。**变体（日志文件打不开）**：把 `<conf>` 改回原样，重做「启动、强杀」，再执行 `./rurge run -c <conf> --log-file <conf>/rurge.log`（拿一个已有的普通文件当目录，日志文件必然打不开） | 改坏的第一次：stderr 先是诊断（`error[E0010]: missing FINAL rule`），随后一行 `warning: a previous run left the system proxy pointing at rurge; the saved settings were restored`，退出码 2（`echo $?`）；GNOME/KDE 的代理设置回到原值（同第 1 条的命令），`<data-dir>/state.json` 的 `system_proxy_backup` 为 `null`。第二次仍以 2 退出，stderr 里不再有 `system proxy` 的字样，设置不变。变体：stderr 有同一行 `warning: … the saved settings were restored`，随后 `error: cannot open log file …`，退出码 2，设置回到原值。这是真实的 `gsettings` / `kwriteconfig` 后端第一次走这条路：设置没有恢复，或出现的是 `warning: cannot restore the system proxy: <原因>; the saved settings stay in state.json and are restored on the next start`（`<原因>` 是失败的命令）时，记下原文与桌面环境 | | | |

---

## 记录

完成以上三个平台的验收后，把每一步的结果、日期、系统版本回填到上面的表格里；有「不通过」的项目附简要说明，并视情况登记进 `docs/surge-compatibility-matrix.md` 或提交 issue。
