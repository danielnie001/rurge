# 阶段 3 手工验收清单

需要真实网卡、真实的 Wi-Fi 与有线网络、真实的系统代理设置和真实的位置权限的项目。自动化测试只用回环地址，用文件后端（`RURGE_NETWORK_BACKEND`、`RURGE_SYSTEM_PROXY_BACKEND`）模拟网络和系统代理，覆盖不了这些：读网络的那几条命令与文件在 Linux 与 macOS 上只对着假的机器测过（`docs/superpowers/plans/2026-10-16-phase3-m1-network-subnet-plan.md` 末尾「延后事项」第 1、3 行），非英文系统上真实的 `netsh wlan show interfaces` 输出（开发机的系统语言是中文，量的时候 Wi-Fi 是断开的，输出里没有 `SSID` 与 `BSSID` 这两行，所以这两个键本身没有见过，见过的只是断开时就有的键）、含非 ASCII 字母的真实网络名、位置权限和 VPN 这些情形没有量过（解析器对着按公开格式写的文本夹具测过，其中有含非 ASCII 字母的名字）。由项目所有者在自己的机器上验收；每一项记下日期、系统版本与结果。**一项也没有做过。**

清单随里程碑逐节增加：下面是 M1（网络环境与子网）与 M2（进程识别），M3 起各加一节。M1 的设计见 `docs/superpowers/specs/2026-10-16-phase3-m1-network-subnet-design.md`，M2 的设计见 `docs/superpowers/specs/2026-10-17-phase3-m2-process-design.md`；API 与日志行见 `docs/api/phase3.md`，兼容性清单里的差异见 `docs/surge-compatibility-matrix.md` 的 2.3、3.1、3.4、3.5、5.1、5.2、9.4、10.4 节。

## M1　网络环境与子网

### 前置条件

- `cargo build --release`，用得到的二进制（`target/release/rurge`，Windows 下是 `target\release\rurge.exe`）。下文按平台写作 `rurge.exe`（Windows）或 `./rurge`（macOS / Linux）。
- 一份测试用 Profile（下文的 `<conf>`）。`<A>` 是你自己的 Wi-Fi 的名字（名字里不要有 `=` 和 `,`），`1.1.1.1` 与 `8.8.8.8` 换成你的网络都够得到的两个 DNS 服务器：

  ```ini
  [General]
  http-listen = 127.0.0.1:6152
  http-api = testkey@127.0.0.1:6171
  dns-server = 1.1.1.1

  [Proxy Group]
  Net = subnet, TYPE:WIRED = REJECT, SSID:<A> = DIRECT, TYPE:WIFI = REJECT-TINYGIF, default = REJECT-DROP

  [Rule]
  SUBNET,TYPE:WIRED,REJECT
  FINAL,Net

  [SSID Setting]
  TYPE:WIRED suspend=true
  SSID:<A> dns-server=8.8.8.8
  ```

  `Net` 组只用内置策略，当前选的是谁一眼就看得出：有线网络上是 `REJECT`；连着 `<A>` 且读得到名字时是 `DIRECT`；别的 Wi-Fi，或 `<A>` 但读不到名字时是 `REJECT-TINYGIF`（`TYPE:WIFI` 命中）；既不是有线也不是 Wi-Fi，或网络不知道时是 `REJECT-DROP`。有线条件写在最前面，是因为插着网线又连着 `<A>` 时两个条件都命中，取第一个（`subnet-exp-wifi-always-match` 缺省为 true）。`[SSID Setting]` 里同样第一行命中的生效，`TYPE:WIRED` 排在前面也是这个原因。
- 日志：`rurge run -c <conf> --log-level debug --log-file <日志路径>`（或看标准错误）。下文「日志」都指这里；`network changed` 是 INFO，`where the machine is now` 是 DEBUG。
- API（Windows 用 `curl.exe`）：
  - 当前选择：`curl -s -H "X-Key: testkey" "http://127.0.0.1:6171/v1/policy_groups/select?group_name=Net"`
  - DNS 上游：`curl -s -H "X-Key: testkey" http://127.0.0.1:6171/v1/dns`（看 `upstreams`）
  - 系统代理：`curl -s -H "X-Key: testkey" http://127.0.0.1:6171/v1/features/system_proxy`
- 每一步执行后，在对应行填写「结果」（通过 / 不通过，附简要说明）、「日期」与「系统版本」三栏。

---

### Windows

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| W1 | **真实的换网络：Wi-Fi 与有线。** 只连 Wi-Fi `<A>`（拔掉网线）运行 `rurge.exe run -c <conf> --log-level debug`；读当前选择；插上网线；再读；拔掉网线；再读。另在有线时与 Wi-Fi 时各执行一次 `rurge.exe rule match -c <conf> example.com` | 连着 `<A>`：`{"policy":"DIRECT"}`。插上网线后数秒内（Windows 每 2 秒比一次网卡表，再加至多 1 秒的去抖）选择变成 `REJECT`，一行 INFO `network changed`，`kind=wired`；拔掉后回到 `DIRECT`，`kind=wifi`。`rule match` 在有线时命中 `SUBNET,TYPE:WIRED,REJECT`，在 Wi-Fi 时落到 `FINAL`。（有线与 Wi-Fi 同时连着时 Windows 通常让有线的度量更低，成为主网卡；若不是，记下 `Get-NetIPInterface` 的度量） | 日志的 `network changed` 行（字段 `interface`、`kind`、`wifi`）；当前选择；`rule match` 输出里的命中规则 | | | |
| W2 | **换 Wi-Fi：`<A>` → 另一个 Wi-Fi `<B>`。** 连着 `<A>` 运行，再切到 `<B>` | 选择从 `DIRECT` 变成 `REJECT-TINYGIF`；一行 INFO `network changed`；DEBUG 的 `where the machine is now` 里 `ssid` 是 `<B>`；`network changed` 那一行（INFO）里**没有** SSID（缺省日志级别下整个日志文件里都搜不到它，P8 专门验） | 日志 | | | |
| W3 | **位置权限关着时 SSID 读不到。** 在「设置 → 隐私和安全性 → 位置」里把位置服务（以及与桌面应用有关的那个开关）关掉，重启 rurge，连着 `<A>`。再打开权限、重启 rurge | 关着时：日志里**一次**（以后换网络不再重复）DEBUG：`a Wi-Fi adapter is up but Windows gives no name for the network (it wants the permission to see where the machine is); SSID and BSSID conditions will not match`；选择是 `REJECT-TINYGIF`（`SSID:<A>` 不命中而 `TYPE:WIFI` 命中），不报错。打开权限后：读到名字，选择是 `DIRECT`。按系统版本的实际界面记下要打开哪几个开关 | 日志；当前选择 | | | |
| W4 | **以 SYSTEM 身份运行。** 用管理员权限的命令行；先停掉别的 rurge；`schtasks /create /tn rurge-sys /sc once /st 00:00 /ru SYSTEM /tr "<rurge.exe 全路径> run -c <conf> --log-level debug --log-file <日志路径>"`，`schtasks /run /tn rurge-sys`；看完用 `schtasks /delete /tn rurge-sys /f` 删掉任务并结束进程 | 设计预期以 SYSTEM 运行的进程读不到 SSID（连权限提示都没有）：同 W3 的那一句 DEBUG，选择是 `REJECT-TINYGIF`；`TYPE:` 与 `ROUTER:` 条件照常。**若读得到，把它记下来**——说明这条限制比设计写的窄 | 日志；当前选择 | | | |
| W5 | **非英文系统与含非 ASCII 字母的名字。** 在系统语言不是英文的 Windows 上（开发机是中文，但量的时候 Wi-Fi 断开，输出里没有 `SSID` 与 `BSSID` 两行：这两个键还没有见过），连一个名字含中文（或别的非 ASCII 字母）的 Wi-Fi，`<conf>` 里的 `<A>` 就写这个名字，运行 rurge；另开 `cmd` 执行 `chcp 65001 >nul & netsh wlan show interfaces` 看那两个键 | `SSID:<含中文的名字>` 命中（选择 `DIRECT`）；DEBUG `where the machine is now` 里 `ssid` 完整；日志里没有 `cannot ask this program about the network`。`chcp 65001` 之后 `netsh` 的键是英文的 `SSID`，BSSID 一行是 `BSSID` 或 Windows 11 的 `AP BSSID`；若不是，把实际的键抄下来 | 日志；当前选择；`netsh` 的输出 | | | |
| W6 | **`[SSID Setting]` 的 DNS 覆盖。** 连着 `<A>` 运行；读 `upstreams`；插上网线（第一行 `TYPE:WIRED suspend=true` 命中，它没有 DNS 项）；再读；拔线；再读。另外在 `<A>` 上执行 `rurge.exe dns lookup -c <conf> example.com --trace` | `<A>` 上 `upstreams` 只有 `8.8.8.8`（形如 `udp://8.8.8.8:53`），有线时回到全局的 `1.1.1.1`，拔线后又是 `8.8.8.8`；每次变化之前有一行 INFO `this network sets other DNS servers than the generation in use was built with; building it again`；经代理的请求（`curl.exe -x http://127.0.0.1:6152 http://example.com/ -I`）照常能解析；`dns lookup` 在 `<A>` 上问的是 `8.8.8.8`。这期间改一下 `<conf>` 里的一条规则但不重载：换网络之后 `GET /v1/rules` 的条数不变（不重读磁盘上的 Profile） | `GET /v1/dns`；日志；`--trace` 的输出；`GET /v1/rules` | | | |
| W7 | **接管暂停：对真实的系统代理。** 先用 PowerShell 记下 `Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings' \| Select ProxyEnable, ProxyServer, ProxyOverride`；连着 `<A>` 运行 `rurge.exe run -c <conf> --system-proxy`，再记一次。(a) 插上网线（`TYPE:WIRED suspend=true` 命中）；(b) 暂停期间 `curl.exe -X POST -H "X-Key: testkey" -d "{\"enabled\":true}" http://127.0.0.1:6171/v1/features/system_proxy`，再 `GET`；(c) 拔线。(d) 再插线，暂停期间 POST `{"enabled":false}`，拔线。(e) 插着网线冷启动 `rurge.exe run -c <conf> --system-proxy`，再拔线。(f) 暂停中用任务管理器强杀 rurge，看注册表，再启动一次 | (a) INFO `takeover paused on this network`；注册表三个值回到启动前的原值；`GET` 是 `{"enabled":false}`。(b) POST 返回 `{}`，注册表不变，日志一行 INFO `the takeover is paused on this network; the system proxy is left as it is, and set as asked when the pause ends`。(c) INFO `takeover resumed`，注册表再指向 `127.0.0.1:6152`，`GET` 是 `true`。(d) 拔线后注册表保持原值（日志仍有 `takeover resumed`，但没有设置）。(e) 启动输出里没有 `system proxy enabled:` 那一行，注册表不动，拔线后才指向 rurge。(f) 强杀时注册表本来就是原值，`<数据目录>\state.json` 的 `system_proxy_backup` 是 `null`，下一次启动没有 `a previous run left the system proxy pointing at rurge` 那条 WARN | 注册表；`GET /v1/features/system_proxy`；日志；`state.json` | | | |
| W8 | **睡眠与唤醒。** (a) 连着 `<A>` 运行，让机器睡眠 15 秒以上，换到另一个网络（`<B>` 或有线）唤醒；(b) 另做一遍唤醒到原来的网络；(c) 接上有线网络（`<conf>` 的第一行 `TYPE:WIRED suspend=true` 命中，接管暂停），带 `--system-proxy` 运行（W7 (e) 的状态：系统代理是原值），睡眠 15 秒以上再唤醒，网络掉线又连回有线 | (a) 换了网络：联网后数秒内一行 INFO `network changed`，选择与 DNS 上游随之变（`<B>` 或有线上按 `<conf>` 是拒绝的，这是配置，不是故障）。(b) 回到 `<A>`：睡眠时读不到任何网络（没有网卡、没有路由）不算变化，不告知，日志里至多一行 DEBUG `network unknown; the last known network stands`；读回的网络与睡前一样（SSID、网关、地址、DNS 服务器）时没有 `network changed`，最终选择是 `DIRECT`，经代理的请求（`curl.exe -x http://127.0.0.1:6152 http://example.com/ -I`）立刻返回 200，不卡在旧网络的 DNS 上；睡眠中地址或 DNS 服务器换了（租约换了）时才有一行 `network changed`。(c) 整个过程里系统代理**一次也不被指向** rurge：注册表（W7 的命令）始终是原值，`GET /v1/features/system_proxy` 始终是 `{"enabled":false}`，日志里没有 `takeover resumed`，也没有第二行 `takeover paused on this network` | 日志；当前选择；`GET /v1/dns`；注册表 | | | |
| W9 | **空闲开销（NFR-02）与位置的隐私指示。** rurge 空闲（不发请求）运行 10 分钟；任务管理器看 CPU；用 Process Monitor 看 `rurge.exe` 起的子进程（Operation 为 Process Create）。同时在 Windows 11 上看位置的指示：任务栏通知区域的位置图标（“位置正在使用”），以及设置 → 隐私和安全性 → 位置 → 最近的活动 | CPU 接近 0。Wi-Fi 连着时约每 30 秒出现一次 `cmd.exe`（带着 `chcp.com` 与 `netsh.exe`）；没有变化时再没有别的子进程；没有无线网卡开着时一个子进程也没有。每 30 秒一次的 `netsh wlan show interfaces` **不点亮**位置图标，“最近的活动”里也**没有** `netsh.exe`、`cmd.exe` 或 `rurge.exe` 的条目；若点亮或有条目，记下是哪个进程、间隔多久——一个每 30 秒闪一次的隐私指示是使用者看得见的，补救（没有 `SSID:` / `BSSID:` 条件时不问 Wi-Fi）由项目所有者定 | 任务管理器；Process Monitor；通知区域与位置的“最近的活动” | | | |
| W10 | **冷启动到可服务的时间（NFR-02：不到 1 秒），用真实的后端。** 连着 Wi-Fi `<A>`，不设 `RURGE_NETWORK_BACKEND`，`rurge.exe run -c <conf> --log-level debug`；记下从回车到标准输出出现第一行 `listening on` 的时间（比如另开窗口用秒表，或比较进程的启动时间与日志行的时间戳），做五次，第一次（`netsh` 等没有进过缓存）与后几次分开记；再拔掉 Wi-Fi 做一次 | 空闲的机器上不到 1 秒。读网络最多等 1 秒：读得更慢时日志里有一行 DEBUG `the network is not read yet; the start goes on without it, and it is told when it is read`，之后一行 INFO `network changed`（读完的网络当作一次变化，DNS 覆盖因此可能多一次重建，接管暂停也在那时才按这个网络决定）。记下实际用了多久、有没有走到这一支 | 日志；秒表 | | | |

---

### macOS

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| M1 | **真实的换网络：Wi-Fi 与有线。** 只连 Wi-Fi `<A>` 运行 `./rurge run -c <conf> --log-level debug`；读当前选择；接上有线（USB 或雷电以太网适配器）；再读；拔掉；再读。再切一次 Wi-Fi（`<A>` → `<B>`） | 同 W1、W2，只是靠路由套接字，反应比 Windows 与 Linux 的 2 秒轮询更快；有线适配器的硬件端口名（`./networksetup -listallhardwareports` 里的 `Hardware Port:`）被认成有线，`kind=wired` | 日志；当前选择；`networksetup -listallhardwareports` | | | |
| M2 | **非英文系统上的 `TYPE:WIRED`。** 在系统语言不是英文的 macOS 上执行 `networksetup -listallhardwareports`，把有线适配器的 `Hardware Port:` 抄下来；接上它，看 `network changed` 的 `kind` 与 `TYPE:WIRED` | rurge 按英文单词 `Ethernet` 或 `LAN` 判有线（`Wi-Fi`、`AirPort` 判 Wi-Fi）。端口名里有这两个词之一时 `kind=wired`、`REJECT` 被选中；**端口名被本地化成别的文字时判不出，`kind=other`**，那就是已知限制成了事实，把端口名记下来 | `networksetup` 的输出；日志的 `kind` | | | |
| M3 | **SSID 要位置授权（macOS 14 起）。** 在 Terminal 里执行 `ipconfig getsummary en0 \| grep -i ssid`（`en0` 换成你的 Wi-Fi 设备），看是不是 `<redacted>`；运行 rurge，连着 `<A>`。再分别在这几种情形下试：给 Terminal 开位置服务；用 `rurge service install --user` 装的启动项；`sudo` 运行 | 读不到名字时：日志里**一次** DEBUG `a Wi-Fi device is on a network but macOS does not tell this program its name (it wants the permission to see where the machine is); SSID and BSSID conditions will not match`；选择是 `REJECT-TINYGIF`（`TYPE:WIFI` 命中），`ROUTER:` 条件照常。把每种情形下读不读得到记下来，哪种能读到就说明守护进程怎样才拿得到名字（总设计留给项目所有者的问题 Q6：要不要另做一个带位置授权的 App 包） | `ipconfig getsummary` 的输出；日志；当前选择 | | | |
| M4 | **IPv6 网关。** 在有 IPv6 路由器通告的网络上执行 `route -n get -inet6 default`，抄下 `gateway:`（形如 `fe80::1%en0`）；把条件 `ROUTER:fe80::1 = REJECT-NO-DROP` 加在 `<conf>` 的 `Net` 组最前面（换成抄下的地址，不带 `%en0`） | 当前选择是 `REJECT-NO-DROP`（路由器是主网卡上的默认网关，条件命中），证明 rurge 读 `gateway:` 时去掉了 `%网卡`。写一个不是主网卡上的 IPv6 网关的地址则不命中 | 当前选择 | | | |
| M5 | **`[SSID Setting]` 的 DNS 覆盖与接管暂停：对真实的系统代理。** 照 W6、W7 做，系统代理用 `networksetup -getwebproxy "Wi-Fi"`、`networksetup -getsecurewebproxy "Wi-Fi"`、`networksetup -getsocksfirewallproxy "Wi-Fi"` 看（有线时换成相应的网络服务名）。`networksetup` 要不要管理员账户由阶段 1 的清单（`docs/acceptance/phase1-manual.md`）验 | 同 W6、W7 的期望：暂停时三项都回到原来的设置，离开时指向 `127.0.0.1:6152`；`GET /v1/features/system_proxy` 暂停时是 `false` | `networksetup` 的输出；API；日志；`state.json` | | | |
| M6 | **睡眠与唤醒。** 照 W8 做，含 (c)：在 `suspend` 的网络上睡眠唤醒，系统代理用 M5 的 `networksetup -get…` 命令看 | 同 W8（含 (c)：系统代理一次也不被指向 rurge）；路由套接字在唤醒后继续用，换了网络时 `network changed` 出现 | 日志；当前选择；`networksetup` 的输出 | | | |
| M7 | **空闲开销（NFR-02）。** rurge 空闲运行 10 分钟；活动监视器看 CPU；用 `sudo fs_usage -w -f exec` 看有没有 `route`、`networksetup`、`ipconfig` 子进程。再在开着隔空投送（AirDrop）与接力（Handoff）时同样空闲 10 分钟数一遍（`awdl0`、`llw0` 一类本身会起落的网卡也会发路由消息，频率没有人在真机上看过） | CPU 接近 0；没有路由消息时只有每 30 秒一次兜底读取（几个子进程）；一阵路由消息（比如连上 VPN）合成一次提示（静下来 300 毫秒才发，连续不停也至多拖 1 秒），不是每条消息读一次；完整读取（那几个子进程）两次之间至少隔 1 秒，一阵提示至多花两次读取，所以任何情形下都不会超过每秒一次；开着隔空投送时的子进程数记下来 | 活动监视器；`fs_usage` | | | |
| M8 | **冷启动到可服务的时间（NFR-02：不到 1 秒），用真实的后端。** 连着 Wi-Fi `<A>`，不设 `RURGE_NETWORK_BACKEND`，`./rurge run -c <conf> --log-level debug`；记下从回车到标准输出出现第一行 `listening on` 的时间，做五次，第一次（`networksetup` 的硬件端口表还是空的）与后几次分开记；再拔掉 Wi-Fi 做一次 | 空闲的机器上不到 1 秒。读网络最多等 1 秒：读得更慢时（`route` 两次、`networksetup`、`ipconfig getsummary`，各至多 2 秒）日志里有一行 DEBUG `the network is not read yet; the start goes on without it, and it is told when it is read`，之后一行 INFO `network changed`。记下实际用了多久、有没有走到这一支 | 日志；秒表 | | | |

---

### Linux

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| L1 | **真实的换网络：Wi-Fi 与有线**（NetworkManager 的桌面）。照 W1、W2 做，运行 `./rurge run -c <conf> --log-level debug` | 同 W1、W2（Linux 每 2 秒比一次 `/proc/net/route` 等文件的指纹）；有线与 Wi-Fi 同时连着时主网卡是度量最小的默认路由所在的网卡，记下 `ip route` 的度量；`kind` 对应 `/sys/class/net/<网卡>/wireless`（Wi-Fi）与 `type` 为 1（有线） | 日志；当前选择；`ip route` | | | |
| L2 | **有无 NetworkManager，有无 `nmcli` / `iw`。** (a) NetworkManager 在用：照 L1。(b) 在一台可以随便折腾的机器上停掉 NetworkManager，用 `wpa_supplicant` 连上 `<A>`，重启 rurge。(c) 再把 `nmcli` 与 `iw` 都从 `PATH` 里拿掉（比如 `env PATH=/usr/bin/nothing ./rurge …` 并给 rurge 的二进制写全路径），重启 | (a) 经 `nmcli` 读到名字，选择 `DIRECT`。(b) 退到 `iw dev <网卡> link`，仍读到名字，选择 `DIRECT`。(c) 日志里**一次** DEBUG `a Wi-Fi interface is up but neither NetworkManager nor iw gives a name for the network; SSID and BSSID conditions will not match`，选择是 `REJECT-TINYGIF`，`TYPE:` 与 `ROUTER:` 照常，不报错 | 日志；当前选择 | | | |
| L3 | **rurge 不引发 Wi-Fi 扫描。** NetworkManager 的笔记本上，连着 Wi-Fi，先不开 rurge，执行 `iw event -t \| grep -i scan` 15 分钟，数一数 `scan started`；再开 rurge（空闲）同样 15 分钟 | 有 rurge 时的扫描次数与没有时相当，**没有每 30 秒一次**的规律（rurge 问 `nmcli` 用的是 `--rescan no`，只读 NetworkManager 已有的列表）；NetworkManager 自己的后台扫描两次都会有 | `iw event -t` 的输出 | | | |
| L4 | **含非 ASCII 字母的名字，与 `LANGUAGE`。** (a) 连一个名字含中文的 Wi-Fi，`<A>` 写这个名字，在 `LANG=zh_CN.UTF-8` 的环境里运行 rurge。(b) glibc 早于 2.39 的发行版上设 `LANGUAGE=zh_CN` 再运行 | (a) 名字完整读到，选择 `DIRECT`（rurge 用 `LC_ALL=C.UTF-8` 问 `nmcli`）。(b) 设了 `LANGUAGE` 也读得到名字，选择 `DIRECT`：rurge 给 `nmcli` 的子进程把 `LANGUAGE` 设成空（glibc 2.39 之前 `LANGUAGE` 对 `C.UTF-8` 仍然生效，会让 `nmcli` 把 `yes` 译成中文，`ACTIVE` 一列读不出）；读不到的话把 `env LC_ALL=C.UTF-8 LANGUAGE= nmcli -t -f ACTIVE,SSID,BSSID,DEVICE dev wifi list --rescan no` 的输出抄下来 | 日志；当前选择；`nmcli` 的输出 | | | |
| L5 | **只换 DNS 服务器。** 在 NetworkManager 里只改当前连接的 DNS 服务器（不换网卡、不换地址）；如果系统用 systemd-resolved，先看 `resolvectl status` 与 `/run/systemd/resolve/resolv.conf` | 数秒内一行 INFO `network changed`（DNS 服务器变了也算变化；DEBUG `the network is not what it was` 里 `dns_servers=true`）；rurge 读的是 systemd-resolved 记下的上游，不是 `127.0.0.53` 的本机桩 | 日志 | | | |
| L6 | **`[SSID Setting]` 的 DNS 覆盖与接管暂停：对真实的系统代理。** 照 W6、W7 做；GNOME 用 `gsettings get org.gnome.system.proxy mode` 与 `gsettings get org.gnome.system.proxy.http host`，KDE 用 `kreadconfig6 --file kioslaverc --group 'Proxy Settings' --key ProxyType` | 同 W6、W7：暂停时恢复成原来的值，离开时指向 `127.0.0.1:6152`；`GET /v1/features/system_proxy` 暂停时是 `false`。不支持的桌面（见阶段 1 清单）上 `--system-proxy` 照旧失败 | `gsettings` / `kreadconfig` 的输出；API；日志；`state.json` | | | |
| L7 | **睡眠与唤醒。** `systemctl suspend`，换网络唤醒，再唤醒到原来的网络；再做 W8 的 (c)：在 `suspend` 的网络上睡眠唤醒，系统代理用 L6 的 `gsettings` / `kreadconfig` 命令看 | 同 W8（含 (c)：系统代理一次也不被指向 rurge） | 日志；当前选择；`gsettings` / `kreadconfig` 的输出 | | | |
| L8 | **空闲开销（NFR-02）。** rurge 空闲运行 10 分钟，`top` 看 CPU；`sudo strace -f -e trace=execve -p <rurge 的 pid> 2>&1 \| grep -v ENOENT`（或 `execsnoop`）看子进程 | CPU 接近 0；有无线网卡开着时每 30 秒一次 `nmcli`（或 `iw`），没有变化时再没有别的子进程；没有无线网卡开着时一个子进程也没有 | `top`；`strace` | | | |
| L9 | **冷启动到可服务的时间（NFR-02：不到 1 秒），用真实的后端。** 连着 Wi-Fi `<A>`，不设 `RURGE_NETWORK_BACKEND`，`./rurge run -c <conf> --log-level debug`；记下从回车到标准输出出现第一行 `listening on` 的时间，做五次，第一次与后几次分开记；再拔掉 Wi-Fi 做一次 | 空闲的机器上不到 1 秒。读网络最多等 1 秒：读得更慢时（`nmcli` 或 `iw` 各至多 2 秒）日志里有一行 DEBUG `the network is not read yet; the start goes on without it, and it is told when it is read`，之后一行 INFO `network changed`。记下实际用了多久、有没有走到这一支 | 日志；秒表 | | | |

---

### 与平台无关

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| P1 | **`subnet` 组的临时覆盖，换网络时清除。** 运行 rurge；`curl -s -X POST -H "X-Key: testkey" -d '{"group_name":"Net","policy":"DIRECT"}' http://127.0.0.1:6171/v1/policy_groups/select`（Windows 上 `curl.exe` 要把引号转义，写法见 W7）；读选择；换一个网络（W1 的换法）；再读 | 覆盖设置后选择是 `DIRECT`，不论网络；换网络之后回到按新网络选的那个（覆盖没了）；不在组里的名字是 400 `` `Somewhere` is not a member of `Net` ``；空的 `policy` 清除覆盖。重启 rurge 后覆盖也不在 | 当前选择；`state.json` 里没有这个覆盖 | | | |
| P2 | **换网络之后 `url-test` 重测**（用你自己的真实节点）。组 `Auto = url-test, <节点 1>, <节点 2>`（可加 `evaluate-before-use = true`），`FINAL,Auto`。先发几个请求让它测过；`GET /v1/policy_groups/test_results`；换网络；发一个请求；`GET /v1/requests/recent`。另放一个 `smart` 组对照 | 换网络之后的第一次使用触发一轮新的测试：请求记录里出现 `listener` 为 `internal`、`rule` 为 `policy test` 的会话，时间在换网络之后；写了 `evaluate-before-use` 时这第一个请求等这一轮测完（至多最长的测试超时）才走；`smart` 组不因换网络而重测。新一轮选出的节点与新网络上的延迟相符 | `GET /v1/requests/recent`；`test_results` | | | |
| P3 | **WireGuard 在换网络后的恢复**（用你自己的 WireGuard 或 WARP 节点）。经该策略持续请求（`curl` 循环）；换网络 | 预期换网络后数秒内经隧道的请求恢复（此前靠载体发送失败或握手无回应才换载体，最坏约两分钟）；日志里没有一直重复的 `wireguard: the peer did not answer the handshake`。没有专门的日志行说隧道换了载体，只能从请求的恢复看；记下实际用了多久 | 请求的成败与耗时；日志 | | | |
| P4 | **真实的 DoH 地址写进 `[SSID Setting]`。** `SSID:<A> encrypted-dns-server=https://dns.google/dns-query?x=1,https://cloudflare-dns.com/dns-query`（含 `?` 与 `=`，两个 URL 用逗号分开）；另试一行带用户名口令的 `https://user:pw@…`（口令是编的，只看会不会出现在输出里）；再试全局配了 `encrypted-dns-server`、这一行写 `encrypted-dns-server=off` | `rurge check -c <conf>` 没有 `W0028`，也没有 `E`；两个 URL 都是列表项（不被当成参数），连着 `<A>` 时 `upstreams` 里是这两个 DoH 上游，经代理的请求能解析；带口令的那行，口令不出现在 `check` 的输出、日志、`GET /v1/dns` 与默认的（`sensitive=0`）`GET /v1/profiles/current` 里；`off` 的网络上 `upstreams` 是传统的 | `rurge check`；`GET /v1/dns`；日志 | | | |
| P5 | **VPN 接管默认路由。** 连上一个全隧道的 VPN 客户端（试两种：基于 WireGuard / utun 的，和基于 TAP 或 PPP 的），看 rurge | 连上与断开时各有一行 INFO `network changed`；主网卡变成 VPN 的网卡（`kind` 一般是 `other`；TAP 一类的虚拟以太网卡在 Linux 上是 `wired`，在 Windows 上多半也是），`TYPE:WIFI` 不再命中、`ROUTER:` 比的是 VPN 的网关；`subnet-exp-wifi-always-match`（缺省 true）让 `SSID:` 仍然看 Wi-Fi 连着的网络。把用到的 VPN 客户端和观察到的 `kind` 记下来（已知限制：rurge 不区分物理与虚拟网卡，排除别的 VPN 没有计划） | 日志的 `kind` 与 `interface` | | | |
| P6 | **去抖，与读不到网络的瞬间。** (a) 在一秒内把 Wi-Fi（或网线）反复关开三五次，最后停在连着的状态；(b) 在 `<A>` 与 `<B>` 两个网络之间尽快来回切三五次（比如在设置里点着换 Wi-Fi），最后停在 `<B>` | (a) 读不到任何网络的瞬间不算变化：至多一行 DEBUG `network unknown; the last known network stands`，连回同一个网络时没有 `network changed`（网络与告知过的一样）；连回来时地址或 DNS 服务器换了才有一行。(b) `network changed` 至多每秒一行：第一次变化立刻一行，其余合并成窗口结束时的一行（用最新的网络）；一个在窗口里又回到原样的网络不另出一行；最终选择与最终的网络一致。两种情形下完整读取（看子进程）都至多每秒一次 | 日志里 `network changed` 与 `network unknown` 的行数与时间；子进程 | | | |
| P7 | **同一个 SSID 下换接入点。** 两个接入点同一个 SSID，在它们之间漫游（或把设备搬到另一个接入点）；地址不变，BSSID 变了。`Net` 组加一个 `BSSID:` 条件 | 约 30 秒内（有无线网卡开着时每 30 秒重读一次 Wi-Fi）一行 INFO `network changed`；DEBUG `where the machine is now` 里 `bssid` 是新的；`BSSID:` 条件随之变 | 日志 | | | |
| P8 | **个人信息不进 INFO。** 在默认日志级别下换几次网络，对日志文件搜索 SSID、BSSID 的前几段、网关地址、本机地址和 DNS 服务器地址 | 一个也搜不到（例外：Profile 里写了 IPv6 的 DNS 服务器而 `ipv6` 关着时，INFO 有一行 `IPv6 DNS server ignored because ipv6 is off` 带着那个服务器的地址，那是 Profile 里写的，不是读到的网络；被拒与失败的会话的 INFO 行带客户端的来源地址 `src`，换网络的步骤里不应有这样的会话）；`--log-level debug` 时 SSID、BSSID 与网关只出现在 `where the machine is now` 一行里，读网络的行里没有本机网卡的地址和系统 DNS 服务器的取值——DNS 服务器的地址会出现在解析器自己的并发查询行里（`upstream=udp://<地址>:53`），这是预期的，不是泄漏 | 日志文件 | | | |
| P9 | **IPv6 隐私地址不引发变化。** 在 IPv6 临时地址会轮换的网络上让 rurge 空闲运行 24 小时以上 | 没有只因为临时地址轮换而出现的 `network changed`（rurge 只比较 IPv6 的 /64 前缀）。有其它原因的变化（换网络、DHCP 租约换了地址）照常出现 | 日志 | | | |
| P10 | **离线命令读真实的网络。** 在有线与 Wi-Fi 上各执行 `rurge rule match -c <conf> example.com`、`rurge dns lookup -c <conf> example.com --trace`、`rurge check -c <conf>` | `rule match` 在有线上命中 `SUBNET,TYPE:WIRED,REJECT`，在 Wi-Fi 上落到 `FINAL`；`dns lookup` 在 `<A>` 上问的是 `8.8.8.8`（`--server` 仍然最后说了算）；`check` 不读网络，在没有网络的机器上也零错误，不再说 `SUBNET` 永不匹配 | 各命令的输出 | | | |

---

## M2　进程识别

设计 `docs/superpowers/specs/2026-10-17-phase3-m2-process-design.md`，计划 `docs/superpowers/plans/2026-10-17-phase3-m2-process-plan.md`（末尾「延后事项」第 1、2、4、5 行是这一节的来由），API 与日志行见 `docs/api/phase3.md` 的「M2　进程识别」，兼容性清单里的差异见 `docs/surge-compatibility-matrix.md` 的 3.1 节 `PROCESS-NAME`、3.5 节内部集 `SYSTEM` 与 10.4 节请求记录。

自动化里真的读系统的表的，主要是每个平台一条读自己回环连接的用例（`this_process_owns_its_loopback_sockets`）和一条起真实二进制的端到端用例（`a_process_rule_matches_the_test_process`），都只在一台 Windows 11 x86-64 的开发机上跑过；Linux 与 macOS 的真实读取（`/proc` 的内容与权限、`pcblist_n` 的真实布局、`proc_pidpath`）只对着按公开格式写的夹具测过，第一次 CI 运行时核对。下面是 CI 与回环验不了的：真实的浏览器与命令行工具会显示成哪个进程、别的用户的进程、虚拟机与容器的客户端、macOS 上代建的连接取哪个 PID、几百条并发连接时查找在不在 200 毫秒里结束。**一项也没有做过。**编号接着 M1 往下排（W11、M9、L10、P11），整份清单里的编号不重复。

### 前置条件

- 同 M1：`cargo build --release`，用得到的二进制，下文按平台写作 `rurge.exe`（Windows）或 `./rurge`（macOS / Linux）。
- 一份测试用 Profile（下文的 `<conf>`）。每一项要加的规则写在 `[Rule]` 里 `FINAL,DIRECT` 的前面，改完重载（`curl -X POST -H "X-Key: testkey" http://127.0.0.1:6171/v1/profiles/reload`，Windows 用 `curl.exe`）或重启：

  ```ini
  [General]
  http-listen = 127.0.0.1:6152
  socks5-listen = 127.0.0.1:6153
  http-api = testkey@127.0.0.1:6171

  [Rule]
  FINAL,DIRECT
  ```

- 看记录：`curl -s -H "X-Key: testkey" "http://127.0.0.1:6171/v1/requests/recent?limit=1000"`（缺省只给最近 100 条；请求记录环缺省存 1000 条，`--request-log-size` 可改）；每条看 `pid`、`processPath`、`rule`、`status`、`rejectKind`、`notes`。PowerShell：`(curl.exe -s -H "X-Key: testkey" "http://127.0.0.1:6171/v1/requests/recent?limit=1000" | ConvertFrom-Json).requests | Format-Table id,pid,processPath,rule,status`；有 `jq` 时：`curl -s -H "X-Key: testkey" "http://127.0.0.1:6171/v1/requests/recent?limit=1000" | jq '.requests[] | {id,pid,processPath,rule,status,notes}'`。正在进行的会话是 `/v1/requests/active`。
- 日志：`rurge run -c <conf> --log-level debug --log-file <日志路径>`。查找的行是 DEBUG 的 `process lookup found the process`（字段 `src`、`listen_port`、`elapsed_ms`、`pid`、`name`、`path`）与 `process lookup found no process`。
- 要被拒的会话不需要外网：命中 `REJECT` 的会话 `status` 是 `rejected`；放行的会话要能连上下文里的网站（写作 `http://example.com/`）。
- 对进程号：Windows 用任务管理器的“详细信息”页（加上“命令行”一列），macOS 与 Linux 用 `ps -p <pid> -o pid,comm,args`。
- P14 要用的一段 SOCKS5 UDP 客户端（Python 3；存成 `socks5_udp.py`，`python3 socks5_udp.py`，Windows 用 `python`）：向 `127.0.0.1:6153` 建 UDP ASSOCIATE，经它向两个地址各发一个 DNS 查询，等应答，再让控制连接多开几秒：

  ```python
  import socket, struct, time
  tcp = socket.create_connection(("127.0.0.1", 6153))
  tcp.sendall(b"\x05\x01\x00")                      # 问候：不认证
  assert tcp.recv(2) == b"\x05\x00"
  tcp.sendall(b"\x05\x03\x00\x01\x00\x00\x00\x00\x00\x00")   # UDP ASSOCIATE，0.0.0.0:0
  reply = tcp.recv(10)                              # 05 00 00 01 <IPv4> <端口>
  relay = (socket.inet_ntoa(reply[4:8]), struct.unpack(">H", reply[8:10])[0])
  udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
  udp.settimeout(5)
  query = b"\x12\x34\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00\x07example\x03com\x00\x00\x01\x00\x01"
  for target in ("1.1.1.1", "8.8.8.8"):
      udp.sendto(b"\x00\x00\x00\x01" + socket.inet_aton(target) + struct.pack(">H", 53) + query, relay)
      try:
          print(target, udp.recvfrom(512)[0][10:12])   # 应答里的 DNS ID：b"\x124"
      except socket.timeout:
          print(target, "no answer")
  time.sleep(5)
  ```

- 每一步执行后，在对应行填写「结果」（通过 / 不通过，附简要说明）、「日期」与「系统版本」三栏。

---

### Windows

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| W11 | **命令行工具与浏览器，经 HTTP 代理，记录里是哪个进程。** `rurge.exe run -c <conf> --log-level debug`。(a) PowerShell：`$p = Start-Process curl.exe -ArgumentList '-x','http://127.0.0.1:6152','--limit-rate','1k','http://example.com/','-o','NUL' -PassThru; $p.Id`（限速是为了让它多活一会儿），读 `recent` 后 `Stop-Process $p`；(b) Edge、Chrome、Firefox 各一个，在设置里手动指定 HTTP 代理 `127.0.0.1:6152`，打开 `http://example.com/`；(c) `<conf>` 的 `[Rule]` 里加 `DOMAIN,ads.test,REJECT`，`curl.exe -x http://127.0.0.1:6152 http://ads.test/` | (a) 记录的 `pid` 等于 `$p.Id`，`processPath` 是 `curl.exe` 的完整路径（形如 `C:\Windows\System32\curl.exe`，JSON 里反斜杠写作 `\\`）；DEBUG 有一行 `process lookup found the process`，`pid` 与 `path` 相同，`src` 是 `127.0.0.1:<端口>`，`listen_port` 是 6152，`elapsed_ms` 在个位数到几十毫秒。(b) 每个浏览器的记录都有 `pid` 与 `processPath`；浏览器常把网络请求放在单独的网络服务进程里，路径不一定是主程序的——把每个显示成什么记下来，并用任务管理器对 PID。(c) 被拒的会话 `status` 是 `rejected`，`pid` 与 `processPath` 同样有：查找可能在会话结束之后才结束，记录读的时候再看 | `recent`；DEBUG 日志；任务管理器 | | | |
| W12 | **`PROCESS-NAME` 的三种写法与大小写。** 一次一条规则，写在 `FINAL,DIRECT` 前面，重载后每次 `curl.exe -x http://127.0.0.1:6152 http://example.com/ -I`：① `PROCESS-NAME,curl.exe,REJECT`；② `PROCESS-NAME,CURL.EXE,REJECT`；③ `PROCESS-NAME,cur*,REJECT`；④ `PROCESS-NAME,curl,REJECT`；⑤ `PROCESS-NAME,C:\Windows\System32\curl.exe,REJECT`；⑥ `PROCESS-NAME,c:/windows/system32/CURL.exe,REJECT`；⑦ `PROCESS-NAME,C:\Windows\System32\,REJECT`（前缀）；⑧ `PROCESS-NAME,c:/windows/,REJECT`。每条之后也跑一次 `rurge.exe check -c <conf>` | ①②③⑤⑥⑦⑧ 被拒（`status` 为 `rejected`，`rule` 是这一行）；**④ 不命中**——Windows 上不去掉 `.exe`，写 `curl` 匹配不到 `curl.exe`——，会话走到 `FINAL,DIRECT`。文件名与路径都不分大小写、路径里 `/` 与 `\` 通用（这是 rurge 在 Windows 上的读法，手册只讲 Mac）。`check` 对每一条都没有错误、没有 `W0013`（不再说 `PROCESS-NAME` 永不匹配） | `recent` 的 `rule` 与 `status`；`check` 的输出 | | | |
| W13 | **UNC 路径与目录联接上的程序。** (a) 在本机共享一个目录（资源管理器里共享，或管理员 PowerShell 里先建好 `C:\tools`，再 `New-SmbShare -Name tools -Path C:\tools`），把 `curl.exe` 拷进去，经 UNC 路径运行：`\\localhost\tools\curl.exe -x http://127.0.0.1:6152 http://example.com/ -I`；(b) 建目录联接 `cmd /c mklink /J C:\toolslink C:\Windows\System32`，运行 `C:\toolslink\curl.exe …`。规则分别写 `PROCESS-NAME,\\localhost\tools\curl.exe,REJECT`、`PROCESS-NAME,\\localhost\tools\,REJECT` 与 `PROCESS-NAME,C:\toolslink\curl.exe,REJECT` | (a) 记下 `processPath` 实际的写法（预期是 `\\localhost\tools\curl.exe`），以 `\\` 开头的两条规则读作路径与前缀，应当命中它；写法不同于预期时抄下来。(b) 记下 `processPath` 是 `C:\toolslink\curl.exe` 还是系统解开联接之后的 `C:\Windows\System32\curl.exe`，和哪条规则命中——路径是文字比较，rurge 不替使用者展开联接。用完 `Remove-SmbShare` 并删掉联接 | `recent`；规则的命中 | | | |
| W14 | **权限：普通用户、管理员与 SYSTEM 运行的 rurge 看得到哪些进程。** (a) 以普通用户运行 rurge，另开一个管理员 PowerShell 运行 `curl.exe -x http://127.0.0.1:6152 http://example.com/ -I`，再用普通用户的 PowerShell 同样运行一次；(b) 以管理员运行 rurge，重复；(c) 以 SYSTEM 运行 rurge（W4 的 `schtasks` 做法），重复 | 读得到的有 `pid` 与 `processPath`；**读不到的两个都是 `null`**（打不开进程就是“不知道”，没有“只有 PID”的结果），会话照常完成，没有 WARN，也没有会话因此失败。把 (a) 里管理员的 curl 读不读得到、(b)(c) 里普通用户的 curl 读不读得到写下来——这是普通用户运行时哪些进程看不见的实际范围 | `recent`；日志 | | | |
| W15 | **真实流量里读不到进程的占多少。** `rurge.exe run -c <conf> --system-proxy --log-level debug`，正常使用电脑十分钟（浏览器、聊天与会议软件、Windows 更新、商店应用），然后统计 `recent`：`(curl.exe -s -H "X-Key: testkey" "http://127.0.0.1:6171/v1/requests/recent?limit=1000" \| ConvertFrom-Json).requests \| Group-Object processPath \| Sort-Object Count -Descending \| Format-Table Count,Name`，另数 `pid` 为 `null` 的条数 | 大多数会话有 `pid` 与 `processPath`；`null` 的是已经退出的短命客户端、读不了的进程、局域网与虚拟机里的设备。把 `null` 的占比与其中最常见的目标记下来；同一个 `svchost.exe` 承载多个服务，`PROCESS-NAME` 分不开它们。日志里没有 `process lookup took longer than 200 ms`，也没有 `process lookup: the tcp table could not be read` | `recent`；日志 | | | |

---

### macOS

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| M9 | **命令行工具与浏览器，经 HTTP 代理，记录里是哪个进程。** `./rurge run -c <conf> --log-level debug`。(a) `curl -x http://127.0.0.1:6152 --limit-rate 1k http://example.com/ -o /dev/null & echo $!`，读 `recent` 后 `kill %1`；(b) Safari、Chrome、Firefox 各一个，在系统设置的“网络 → 代理”里指定网页代理 `127.0.0.1:6152`（或 `./rurge run … --system-proxy`），打开 `http://example.com/`；(c) 加 `DOMAIN,ads.test,REJECT`，`curl -x http://127.0.0.1:6152 http://ads.test/` | (a) 记录的 `pid` 等于 `$!`，`processPath` 是 `/usr/bin/curl`；DEBUG 里 `process lookup found the process` 的 `pid`、`path` 相同，`elapsed_ms` 在个位数到几十毫秒。(b) 记下每个浏览器显示成哪个进程（Safari 的网络请求多半在 `com.apple.WebKit.Networking` 里，Chrome 在网络服务的 Helper 进程里，以实际为准）并用 `ps -p <pid> -o pid,comm,args` 对。(c) 被拒的会话同样有 `pid` 与 `processPath` | `recent`；DEBUG 日志；`ps` | | | |
| M10 | **代建的连接：`so_last_pid` 还是 `so_e_pid`。** 系统服务替 App 建连接时（`nsurlsessiond` 替用后台 `URLSession` 的 App 下载之类），套接字的“最后使用者”和“委托的实际进程”可能不是同一个。`./rurge run -c <conf> --system-proxy --log-level debug`，触发这样的流量（在 App Store 里更新或下载一个 App、在“音乐”或“播客”里下载一首歌，或你知道的别的经 `nsurlsessiond` 的 App），读 `recent`，再对照 `nettop -n -m tcp -L 1`（按进程分组列出连接）里同一条连接挂在谁名下。另试两条规则：`PROCESS-NAME,nsurlsessiond,REJECT` 与 `PROCESS-NAME,<那个 App 的文件名>,REJECT` | 观察项，不预设答案：记下这条连接在 `recent` 里显示成谁（rurge 取 `so_last_pid`）、在 `nettop` 里归谁、哪一条规则命中。若 rurge 显示的是 `nsurlsessiond` 而使用者想写进规则的是那个 App，那就是 `so_last_pid` 与 `so_e_pid` 的取舍落到了使用者头上：把 App、系统版本与两种显示写下来，交项目所有者决定要不要改取 `so_e_pid`（设计 M2-D10；计划「延后事项」第 2 行） | `recent`；`nettop`；规则的命中 | | | |
| M11 | **路径的写法：符号链接、`/private`、大小写。** 把 `curl` 拷到 `/tmp/curl-copy`，`/tmp/curl-copy -x http://127.0.0.1:6152 http://example.com/ -I`。规则一次一条：`PROCESS-NAME,/tmp/curl-copy,REJECT`、`PROCESS-NAME,/private/tmp/curl-copy,REJECT`、`PROCESS-NAME,/private/tmp/,REJECT`（前缀）、`PROCESS-NAME,curl-copy,REJECT`、`PROCESS-NAME,Curl-Copy,REJECT`；再用 `/Applications` 里的一个 App（比如 Safari 或 Firefox）：文件名、`/Applications/<App>.app/Contents/MacOS/*`（带通配符的路径）与 `/Applications/<App>.app/`（前缀）各一条 | `processPath` 应当是 `/private/tmp/curl-copy`（`proc_pidpath` 给的是解开符号链接之后的真实路径，以实际为准，抄下来）：**写 `/tmp/curl-copy` 的那条不命中**，`/private/tmp/curl-copy`、前缀与文件名的命中；`Curl-Copy` 不命中——macOS 上规则区分大小写，即使文件系统不分（同手册）。App 的三种写法按它们的真实路径命中 | `recent`；规则的命中 | | | |
| M12 | **权限：普通用户与 root 运行的 rurge。** (a) 以普通用户运行 rurge，`sudo curl -x http://127.0.0.1:6152 http://example.com/ -I`（root 的进程）和普通用户的 `curl` 各一次；(b) `sudo ./rurge run -c <conf> --log-level debug` 重复 | 读得到的有 `pid` 与 `processPath`；读不到的两个都是 `null`，会话照常完成。把 (a) 里 root 的 `curl` 读不读得到（`proc_pidpath` 对别的用户的进程）写下来；日志里没有 `process lookup: the tcp table could not be read`（`pcblist_n` 对普通用户可读与否，以实际为准） | `recent`；日志 | | | |
| M13 | **内部集 `SYSTEM` 的两条进程行只在 macOS 上生效。** `<conf>` 的 `[Rule]` 最前面加 `RULE-SET,SYSTEM,REJECT`（会拒掉 Apple 的系统域名，试完就拿掉），`./rurge run -c <conf> --system-proxy --log-level debug`，用 Safari、邮件、App Store 一阵；再 `./rurge check -c <conf>` 与 `./rurge rule match -c <conf> example.com --process trustd` | 尽力而为的观察：`recent` 里出现 `processPath` 是 `/usr/libexec/trustd` 的会话时，它被 `RULE-SET,SYSTEM` 接住，`notes` 里有 `Sub-rule matched: PROCESS-NAME,trustd (in …)` 一句（也可能被集合里的域名行先接住，那时 `notes` 里是那一行）；`netbiosd` 同理。`rule match … --process trustd` 在 macOS 上命中，`check` 没有 `W0013`。Windows 与 Linux 上同样的 `rule match --process trustd` **不**命中这一行（两条进程行没有编进集合，M2-D17） | `recent`；`rule match` 的输出 | | | |

---

### Linux

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| L10 | **命令行工具与浏览器，经 HTTP 代理，记录里是哪个进程。** `./rurge run -c <conf> --log-level debug`。(a) `curl -x http://127.0.0.1:6152 --limit-rate 1k http://example.com/ -o /dev/null & echo $!`，读 `recent` 后 `kill %1`；(b) `wget -e use_proxy=yes -e http_proxy=http://127.0.0.1:6152 -O /dev/null http://example.com/`，和 `http_proxy=http://127.0.0.1:6152 python3 -c "import urllib.request as u; u.urlopen('http://example.com/', timeout=10).read()"`；(c) Firefox、Chromium 各一个，手动指定 HTTP 代理 `127.0.0.1:6152`；(d) 加 `DOMAIN,ads.test,REJECT`，`curl -x http://127.0.0.1:6152 http://ads.test/` | (a) `pid` 等于 `$!`，`processPath` 是 `curl` 的真实路径（如 `/usr/bin/curl`）；DEBUG `process lookup found the process`，`name` 是 `curl`。(b) wget 与 python 的 `processPath` 是它们的真实路径（`python3` 常是符号链接，见 L11）。(c) 记下每个浏览器显示成哪个进程与路径（Snap / Flatpak 见 L14），用 `ps -p <pid> -o pid,comm,args` 对。(d) 被拒的会话同样有进程 | `recent`；DEBUG 日志；`ps` | | | |
| L11 | **符号链接与已删除的可执行文件。** (a) `readlink -f $(which python3)` 抄下真实路径；`http_proxy=http://127.0.0.1:6152 python3 -c "import urllib.request as u; u.urlopen('http://example.com/', timeout=10).read()"` 一次；规则一次一条：`PROCESS-NAME,python3,REJECT`、`PROCESS-NAME,<真实路径的文件名>,REJECT`、`PROCESS-NAME,/usr/bin/python3,REJECT`、`PROCESS-NAME,<真实路径>,REJECT`。(b) `cp $(readlink -f $(which python3)) /tmp/py-copy`，然后 `/tmp/py-copy -c "import os,socket,time; os.remove('/tmp/py-copy'); s=socket.create_connection(('127.0.0.1',6152)); s.sendall(b'CONNECT example.com:80 HTTP/1.1\r\nHost: example.com:80\r\n\r\n'); time.sleep(3)"` | (a) `processPath` 是**真实路径**（`/proc/<pid>/exe` 的链接目标，符号链接已解开，如 `/usr/bin/python3.11`）：`PROCESS-NAME,python3` 与 `PROCESS-NAME,/usr/bin/python3` **不命中**，真实路径的文件名与完整路径命中。(b) `processPath` 是 `/tmp/py-copy`（末尾的 ` (deleted)` 已去掉），`pid` 有 | `recent`；规则的命中 | | | |
| L12 | **权限：普通用户与 root 运行的 rurge，别的用户的进程。** (a) 以普通用户运行 rurge，`sudo -u nobody curl -x http://127.0.0.1:6152 http://example.com/ -I`（别的用户的进程）与自己的 `curl` 各一次；(b) `sudo ./rurge run -c <conf> --log-level debug`，重复；(c) 在一台可以折腾的机器上 `sudo mount -o remount,hidepid=2 /proc` 之后重复 (a)（做完 `sudo mount -o remount,hidepid=0 /proc` 恢复） | (a) 自己的进程有 `pid` 与 `processPath`；**别的用户的进程两个都是 `null`**（不以 root 运行时读不了别的用户的 `/proc/<pid>/fd`，按 inode 找不到持有者），会话照常完成，没有 WARN。(b) 以 root 运行，别的用户的进程读得到。(c) `hidepid=2` 下别的用户的进程同 (a)。日志里没有 `process lookup: the tcp table could not be read` | `recent`；日志 | | | |
| L13 | **进程很多的机器上，很多个短命的客户端。** 在一台有几千个进程的机器上（容器宿主机或开了大量容器的机器；`ls /proc \| grep -c '^[0-9]'` 数一下，记下），`./rurge run -c <conf> --log-level debug --log-file <日志路径>`，然后 `seq 300 \| xargs -P 50 -I{} curl -s -o /dev/null -x http://127.0.0.1:6152 'http://example.com/?{}'`（300 个 `curl` 进程，同时 50 个）；统计日志里 `process lookup found the process` 行的 `elapsed_ms` | 每个 `curl` 都是不同的进程，最近找到过的 16 个进程里常常没有它，要扫 `/proc` 下全部进程的 `fd`。看 `elapsed_ms` 的分布（最大值、超过 50 的有多少）。**期望全部在 200 毫秒以内**：`notes` 里没有 `Process not known within 200 ms`，日志里没有 `process lookup took longer than 200 ms` 的 WARN，`recent` 里 300 条都有 `pid`。超过 200 毫秒时把进程数、`elapsed_ms` 的分布与 CPU 型号写下来（计划「延后事项」第 4、14 行） | 日志；`recent` | | | |
| L14 | **Snap / Flatpak 包装的程序。** 用 Snap 或 Flatpak 装的浏览器（Ubuntu 的 Firefox 缺省是 Snap），手动指定 HTTP 代理 `127.0.0.1:6152`，打开 `http://example.com/`，读 `recent` | 记下 `processPath`（Snap 里形如 `/snap/firefox/<版本号>/usr/lib/firefox/firefox`，路径里带着版本号；Flatpak 里是沙箱里看到的路径，或读不到）。用文件名、和 `/snap/firefox/` 前缀各试一条规则，哪种写法稳定（升级之后版本号变了，完整路径就不再匹配）；读不到时 `pid` 与 `processPath` 都是 `null` | `recent`；规则的命中 | | | |

---

### 与平台无关

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| P11 | **本机的局域网地址，与局域网里的别的设备。** `[General]` 里 `http-listen = 0.0.0.0:6152`（重启），`[Rule]` 里加 `PROCESS-NAME,curl,REJECT`（Windows 写 `curl.exe`）。(a) 本机的客户端经本机的局域网地址连：`curl -x http://<本机的局域网地址>:6152 http://example.com/ -I`（Windows 用 `curl.exe`）；(b) 同一局域网里的另一台电脑或手机，把代理指到 `<本机的局域网地址>:6152` 打开网页 | (a) 来源是本机自己网卡上的地址，查得到：记录有 `pid` 与 `processPath`，`src` 是 `<本机的局域网地址>:<端口>`，`PROCESS-NAME` 规则命中。(b) 别的设备：`pid` 与 `processPath` 是 `null`，`PROCESS-NAME` 规则不命中，`notes` 里**没有** `Process not known within 200 ms`（不是等过了，是一开始就知道没有）；**DEBUG 里这些会话大多没有 `process lookup` 的行**（来源显然不是本机，不起查找），本机地址的列表过期（超过 2 秒）之后的第一条会话除外：它起一次查找，重取列表后得到 `process lookup found no process`，不读表。局域网地址会出现在会话的日志行与记录的 `src` 里，`src` 一向如此，不是 M2 才有的 | `recent`；DEBUG 日志 | | | |
| P12 | **虚拟机、WSL2 与容器里的客户端。** `http-listen = 0.0.0.0:6152`，`[Rule]` 里加 `PROCESS-NAME,curl,REJECT`（客户端在虚拟机、WSL2 或容器里，规则看的是本机的进程，写什么都不该命中）。(a) Windows：WSL2 里 `curl -x http://<Windows 在 vEthernet (WSL) 上的地址>:6152 http://example.com/ -I`；Hyper-V 或别的虚拟机里的客户端同理；(b) Linux：Docker 缺省网桥上的容器 `docker run --rm curlimages/curl -x http://172.17.0.1:6152 http://example.com/ -I`（地址换成 `docker0` 的），再来一个 `--network host` 的容器；(c) macOS：Docker Desktop 或别的虚拟机里的客户端，经 `host.docker.internal:6152` | 缺省网桥上的容器、WSL2 与虚拟机里的客户端：来源是虚拟网卡上的另一个地址，在本机没有对应的套接字，`pid` 与 `processPath` 是 `null`，`PROCESS-NAME` 不命中，（除了地址列表过期之后的第一条会话，见 P11）没有 `process lookup` 的 DEBUG 行，会话照常完成。`--network host` 的容器与本机共用网络，来源是回环，查得到：记下 `processPath` 是容器里看到的路径（如 `/usr/bin/curl`）还是读不到。(c) 记下：Docker Desktop 把容器的流量经本机的转发程序送来，记录里的进程可能是转发程序，不是容器里的 `curl` | `recent`；DEBUG 日志 | | | |
| P13 | **几百条并发连接。** `[Rule]` 里 `PROCESS-NAME,nonexistent-app,REJECT` 写在最前面，`FINAL,DIRECT` 换成 `FINAL,REJECT`（每条会话都走到 `PROCESS-NAME`、等进程，又都很快被拒，不需要外网）。`rurge run -c <conf> --log-level debug --log-file <日志路径> --request-log-size 2000`。(a) 一个客户端进程、很多连接：`curl -x http://127.0.0.1:6152 --parallel --parallel-max 300 'http://example.com/?[1-500]' -o /dev/null -s`（Windows 用 `curl.exe` 与 `-o NUL`）；(b) 很多个短命的客户端进程：macOS 与 Linux `seq 300 \| xargs -P 50 -I{} curl -s -o /dev/null -x http://127.0.0.1:6152 'http://example.com/?{}'`，Windows（PowerShell 7）`1..300 \| ForEach-Object -Parallel { curl.exe -s -o NUL -x http://127.0.0.1:6152 "http://example.com/?$_" } -ThrottleLimit 50`；每次之后读 `recent?limit=2000`，再 `rurge stop -c <conf>`（或 Ctrl-C） | 全部会话都完成，没有一条因为查找失败（不知道进程从不让会话失败）；`recent` 里每条记录都有 `pid`；日志里 `process lookup found the process` 的 `elapsed_ms` 绝大多数在几毫秒到几十毫秒，**没有一条会话的 `notes` 里有 `Process not known within 200 ms`**，没有 `process lookup took longer than 200 ms` 的 WARN；任务管理器 / 活动监视器 / `top` 里 rurge 的 CPU 在这一阵之后回到接近 0；`rurge stop` 立刻退出，不挂住。把三个平台上 (a)(b) 的 `elapsed_ms` 最大值与分布各记下（对照：Windows 上开着 500 条回环连接时一次新查找约 3.4 毫秒，debug 构建，一次测量） | 日志；`recent`；任务管理器 / `top` | | | |
| P14 | **经 SOCKS5 UDP ASSOCIATE 的客户端。** `[Rule]` 里 `PROCESS-NAME,<python 的文件名>,DIRECT`（Windows 写 `python.exe`，别的系统写真实路径的文件名，见 L11），`FINAL,REJECT`；运行前置条件里的 `socks5_udp.py`（或任何走 SOCKS5 UDP 的客户端，如支持 UDP 的游戏或语音软件）；读 `recent` 与 DEBUG 日志 | 两个目标都收到应答（`1.1.1.1` 与 `8.8.8.8` 能连上时）——说明 `PROCESS-NAME` 在 UDP 流上命中了控制连接的进程（认不出进程的流会落到 `FINAL,REJECT`，没有应答）。`recent` 里 `transport` 为 `udp` 的两条记录有 `pid` 与 `processPath`（python 的），是同一个进程；DEBUG 里这个关联只有**一行** `process lookup found the process`（`listen_port` 是 6153，`src` 是控制连接的来源），不是每条流一行 | `recent`；DEBUG 日志；脚本的输出 | | | |
| P15 | **IPv6 回环与双栈的客户端。** `http-listen = 127.0.0.1:6152, [::1]:6152`（重启）。(a) `curl -x 'http://[::1]:6152' http://example.com/ -I`（Windows 用 `curl.exe`）；(b) 一个双栈套接字去连 IPv4 的代理地址：`python3 -c "import socket,time; s=socket.create_connection(('::ffff:127.0.0.1',6152)); s.sendall(b'CONNECT example.com:80 HTTP/1.1\r\nHost: example.com:80\r\n\r\n'); time.sleep(3)"`（Windows 用 `python`） | (a) 来源是 `[::1]:<端口>`，记录有 `pid` 与 `processPath`（curl 的）。(b) 来源按 IPv4 显示成 `127.0.0.1:<端口>`（IPv4 映射的地址按 IPv4 处理），记录有 `pid` 与 `processPath`（python 的）——客户端的行在 Linux 的 `tcp6` 表里以 `::ffff:127.0.0.1` 出现，在 macOS 上是带 `INP_IPV4` 标志的双栈记录（计划「执行期修正记录」第 11 行），Windows 上记在哪张表里要看实际；rurge 先找 IPv4 的行、再找 IPv6 表里的映射形式。哪个平台上 (b) 查不到，就是这条路径在真实的系统上不通，抄下平台与版本 | `recent`；DEBUG 日志 | | | |
| P16 | **进程不进 INFO。** 缺省日志级别（`loglevel = notify`，INFO）下，用 W11 / M9 / L10 的办法产生几条被拒与放行的会话（含 `ads.test` 那种被拒的），然后在日志文件里搜进程的名字（`curl`）、路径里的目录名和 PID。再用 `--log-level debug` 做同样的搜索 | 缺省级别下**搜不到**查到的进程：被拒与失败的会话的 INFO 行带来源、目标、规则文字和策略；规则文字里的 `PROCESS-NAME,curl,…` 是使用者自己写的，不是查到的。`--log-level debug` 时进程的名字、路径与 PID 只出现在 `process lookup found the process` 那一行（和请求记录、API 里）。两句 WARN 不带会话、地址与进程 | 日志文件 | | | |
| P17 | **`check` 与 `rule match`。** `<conf>` 里写 `PROCESS-NAME,curl,DIRECT`、`PROCESS-NAME,C:\Program Files\App\app.exe,DIRECT`、`PROCESS-NAME,/Applications/Foo.app/Contents/MacOS/*,DIRECT`、`DEVICE-NAME,Kids-iPad,DIRECT`，在三个平台上各跑 `rurge check -c <conf>`；再 `rurge rule match -c <conf> example.com --process curl` | 三个平台上输出一样：**没有** `PROCESS-NAME` 的 `W0013`，有 `` `DEVICE-NAME` rules never match `` 一条警告（`0 error(s), 1 warning(s)`）——以盘符开头的路径在 Linux 与 macOS 上同样读作路径，为 Windows 写的 Profile 能在别的系统上被检查。`rule match … --process curl` 命中第一条（`rule match` 不查真实的进程，进程是命令行给的） | 各命令的输出 | | | |
| P18 | **日常使用一天。** `rurge run -c <conf> --system-proxy --log-file <日志路径>`（缺省日志级别），正常使用一天，含睡眠与唤醒、换网络 | 日志里没有 `process lookup took longer than 200 ms`（出现过的话把前后的情形、平台和机器写下来：谁慢了，是表大、进程多，还是机器刚睡醒），没有 `process lookup: the tcp table could not be read`；空闲时 CPU 接近 0（查找只随会话发生，空闲时不读表） | 日志；任务管理器 / `top` | | | |
