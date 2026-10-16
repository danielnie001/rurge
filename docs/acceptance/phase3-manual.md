# 阶段 3 手工验收清单

需要真实网卡、真实的 Wi-Fi 与有线网络、真实的系统代理设置和真实的位置权限的项目。自动化测试只用回环地址，用文件后端（`RURGE_NETWORK_BACKEND`、`RURGE_SYSTEM_PROXY_BACKEND`）模拟网络和系统代理，覆盖不了这些：读网络的那几条命令与文件在 Linux 与 macOS 上只对着假的机器测过（`docs/superpowers/plans/2026-10-16-phase3-m1-network-subnet-plan.md` 末尾「延后事项」第 1、3 行），非英文系统、含非 ASCII 字母的真实网络名、位置权限和 VPN 这些情形没有在任何一台真机上量过（解析器对着按公开格式写的文本夹具测过，其中有含非 ASCII 字母的名字）。由项目所有者在自己的机器上验收；每一项记下日期、系统版本与结果。**一项也没有做过。**

清单随里程碑逐节增加：下面是 M1（网络环境与子网），M2 起各加一节。设计见 `docs/superpowers/specs/2026-10-16-phase3-m1-network-subnet-design.md`，API 与日志行见 `docs/api/phase3.md`，兼容性清单里的差异见 `docs/surge-compatibility-matrix.md` 的 2.3、3.1、3.4、5.1、5.2、9.4、10.4 节。

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
| W4 | **以 SYSTEM 身份运行。** 先停掉别的 rurge；`schtasks /create /tn rurge-sys /sc once /st 00:00 /ru SYSTEM /tr "<rurge.exe 全路径> run -c <conf> --log-level debug --log-file <日志路径>"`，`schtasks /run /tn rurge-sys`；看完用 `schtasks /delete /tn rurge-sys /f` 删掉任务并结束进程 | 设计预期以 SYSTEM 运行的进程读不到 SSID（连权限提示都没有）：同 W3 的那一句 DEBUG，选择是 `REJECT-TINYGIF`；`TYPE:` 与 `ROUTER:` 条件照常。**若读得到，把它记下来**——说明这条限制比设计写的窄 | 日志；当前选择 | | | |
| W5 | **非英文系统与含非 ASCII 字母的名字。** 在系统语言不是英文的 Windows 上（开发机是中文，量过键，但那时 Wi-Fi 断开），连一个名字含中文（或别的非 ASCII 字母）的 Wi-Fi，`<conf>` 里的 `<A>` 就写这个名字，运行 rurge；另开 `cmd` 执行 `chcp 65001 >nul & netsh wlan show interfaces` 看那两个键 | `SSID:<含中文的名字>` 命中（选择 `DIRECT`）；DEBUG `where the machine is now` 里 `ssid` 完整；日志里没有 `cannot ask this program about the network`。`chcp 65001` 之后 `netsh` 的键是英文的 `SSID`，BSSID 一行是 `BSSID` 或 Windows 11 的 `AP BSSID`；若不是，把实际的键抄下来 | 日志；当前选择；`netsh` 的输出 | | | |
| W6 | **`[SSID Setting]` 的 DNS 覆盖。** 连着 `<A>` 运行；读 `upstreams`；插上网线（第一行 `TYPE:WIRED suspend=true` 命中，它没有 DNS 项）；再读；拔线；再读。另外在 `<A>` 上执行 `rurge.exe dns lookup -c <conf> example.com --trace` | `<A>` 上 `upstreams` 只有 `8.8.8.8`（形如 `udp://8.8.8.8:53`），有线时回到全局的 `1.1.1.1`，拔线后又是 `8.8.8.8`；每次变化之前有一行 INFO `this network sets other DNS servers than the generation in use was built with; building it again`；经代理的请求（`curl.exe -x http://127.0.0.1:6152 http://example.com/ -I`）照常能解析；`dns lookup` 在 `<A>` 上问的是 `8.8.8.8`。这期间改一下 `<conf>` 里的一条规则但不重载：换网络之后 `GET /v1/rules` 的条数不变（不重读磁盘上的 Profile） | `GET /v1/dns`；日志；`--trace` 的输出；`GET /v1/rules` | | | |
| W7 | **接管暂停：对真实的系统代理。** 先用 PowerShell 记下 `Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings' \| Select ProxyEnable, ProxyServer, ProxyOverride`；连着 `<A>` 运行 `rurge.exe run -c <conf> --system-proxy`，再记一次。(a) 插上网线（`TYPE:WIRED suspend=true` 命中）；(b) 暂停期间 `curl.exe -X POST -H "X-Key: testkey" -d "{\"enabled\":true}" http://127.0.0.1:6171/v1/features/system_proxy`，再 `GET`；(c) 拔线。(d) 再插线，暂停期间 POST `{"enabled":false}`，拔线。(e) 插着网线冷启动 `rurge.exe run -c <conf> --system-proxy`，再拔线。(f) 暂停中用任务管理器强杀 rurge，看注册表，再启动一次 | (a) INFO `takeover paused on this network`；注册表三个值回到启动前的原值；`GET` 是 `{"enabled":false}`。(b) POST 返回 `{}`，注册表不变，日志一行 INFO `the takeover is paused on this network; the system proxy is left as it is, and set as asked when the pause ends`。(c) INFO `takeover resumed`，注册表再指向 `127.0.0.1:6152`，`GET` 是 `true`。(d) 拔线后注册表保持原值（日志仍有 `takeover resumed`，但没有设置）。(e) 启动输出里没有 `system proxy enabled:` 那一行，注册表不动，拔线后才指向 rurge。(f) 强杀时注册表本来就是原值，`<数据目录>\state.json` 的 `system_proxy_backup` 是 `null`，下一次启动没有 `a previous run left the system proxy pointing at rurge` 那条 WARN | 注册表；`GET /v1/features/system_proxy`；日志；`state.json` | | | |
| W8 | **睡眠与唤醒。** 连着 `<A>` 运行，让机器睡眠 15 秒以上，换到另一个网络（`<B>` 或有线）唤醒；另做一遍唤醒到原来的网络 | 换了网络：联网后数秒内一行 INFO `network changed`，选择与 DNS 上游随之变（`<B>` 或有线上按 `<conf>` 是拒绝的，这是配置，不是故障）。回到 `<A>`：断开重连的几次变化被去抖合并，`network changed` 至多出现一两行，最终选择是 `DIRECT`，经代理的请求（`curl.exe -x http://127.0.0.1:6152 http://example.com/ -I`）立刻返回 200，不卡在旧网络的 DNS 上 | 日志；当前选择；`GET /v1/dns` | | | |
| W9 | **空闲开销（NFR-02）。** rurge 空闲（不发请求）运行 10 分钟；任务管理器看 CPU；用 Process Monitor 看 `rurge.exe` 起的子进程（Operation 为 Process Create） | CPU 接近 0。Wi-Fi 连着时约每 30 秒出现一次 `cmd.exe`（带着 `chcp.com` 与 `netsh.exe`）；没有变化时再没有别的子进程；没有无线网卡开着时一个子进程也没有 | 任务管理器；Process Monitor | | | |

---

### macOS

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| M1 | **真实的换网络：Wi-Fi 与有线。** 只连 Wi-Fi `<A>` 运行 `./rurge run -c <conf> --log-level debug`；读当前选择；接上有线（USB 或雷电以太网适配器）；再读；拔掉；再读。再切一次 Wi-Fi（`<A>` → `<B>`） | 同 W1、W2，只是靠路由套接字，反应比 Windows 与 Linux 的 2 秒轮询更快；有线适配器的硬件端口名（`./networksetup -listallhardwareports` 里的 `Hardware Port:`）被认成有线，`kind=wired` | 日志；当前选择；`networksetup -listallhardwareports` | | | |
| M2 | **非英文系统上的 `TYPE:WIRED`。** 在系统语言不是英文的 macOS 上执行 `networksetup -listallhardwareports`，把有线适配器的 `Hardware Port:` 抄下来；接上它，看 `network changed` 的 `kind` 与 `TYPE:WIRED` | rurge 按英文单词 `Ethernet` 或 `LAN` 判有线（`Wi-Fi`、`AirPort` 判 Wi-Fi）。端口名里有这两个词之一时 `kind=wired`、`REJECT` 被选中；**端口名被本地化成别的文字时判不出，`kind=other`**，那就是已知限制成了事实，把端口名记下来 | `networksetup` 的输出；日志的 `kind` | | | |
| M3 | **SSID 要位置授权（macOS 14 起）。** 在 Terminal 里执行 `ipconfig getsummary en0 \| grep -i ssid`（`en0` 换成你的 Wi-Fi 设备），看是不是 `<redacted>`；运行 rurge，连着 `<A>`。再分别在这几种情形下试：给 Terminal 开位置服务；用 `rurge service install --user` 装的启动项；`sudo` 运行 | 读不到名字时：日志里**一次** DEBUG `a Wi-Fi device is on a network but macOS does not tell this program its name (it wants the permission to see where the machine is); SSID and BSSID conditions will not match`；选择是 `REJECT-TINYGIF`（`TYPE:WIFI` 命中），`ROUTER:` 条件照常。把每种情形下读不读得到记下来，哪种能读到就说明守护进程怎样才拿得到名字（总设计留给项目所有者的问题 Q6：要不要另做一个带位置授权的 App 包） | `ipconfig getsummary` 的输出；日志；当前选择 | | | |
| M4 | **IPv6 网关。** 在有 IPv6 路由器通告的网络上执行 `route -n get -inet6 default`，抄下 `gateway:`（形如 `fe80::1%en0`）；把条件 `ROUTER:fe80::1 = REJECT-NO-DROP` 加在 `<conf>` 的 `Net` 组最前面（换成抄下的地址，不带 `%en0`） | 当前选择是 `REJECT-NO-DROP`（路由器是主网卡上的默认网关，条件命中），证明 rurge 读 `gateway:` 时去掉了 `%网卡`。写一个不是主网卡上的 IPv6 网关的地址则不命中 | 当前选择 | | | |
| M5 | **`[SSID Setting]` 的 DNS 覆盖与接管暂停：对真实的系统代理。** 照 W6、W7 做，系统代理用 `networksetup -getwebproxy "Wi-Fi"`、`networksetup -getsecurewebproxy "Wi-Fi"`、`networksetup -getsocksfirewallproxy "Wi-Fi"` 看（有线时换成相应的网络服务名）。`networksetup` 要不要管理员账户由阶段 1 的清单（`docs/acceptance/phase1-manual.md`）验 | 同 W6、W7 的期望：暂停时三项都回到原来的设置，离开时指向 `127.0.0.1:6152`；`GET /v1/features/system_proxy` 暂停时是 `false` | `networksetup` 的输出；API；日志；`state.json` | | | |
| M6 | **睡眠与唤醒。** 照 W8 做 | 同 W8；路由套接字在唤醒后继续用，`network changed` 出现 | 日志；当前选择 | | | |
| M7 | **空闲开销（NFR-02）。** rurge 空闲运行 10 分钟；活动监视器看 CPU；用 `sudo fs_usage -w -f exec` 看有没有 `route`、`networksetup`、`ipconfig` 子进程 | CPU 接近 0；没有路由消息时只有每 30 秒一次兜底读取（几个子进程）；一阵路由消息（比如连上 VPN）合成一次提示（静下来 300 毫秒才发，连续不停也至多拖 1 秒），不是每条消息读一次 | 活动监视器；`fs_usage` | | | |

---

### Linux

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| L1 | **真实的换网络：Wi-Fi 与有线**（NetworkManager 的桌面）。照 W1、W2 做，运行 `./rurge run -c <conf> --log-level debug` | 同 W1、W2（Linux 每 2 秒比一次 `/proc/net/route` 等文件的指纹）；有线与 Wi-Fi 同时连着时主网卡是度量最小的默认路由所在的网卡，记下 `ip route` 的度量；`kind` 对应 `/sys/class/net/<网卡>/wireless`（Wi-Fi）与 `type` 为 1（有线） | 日志；当前选择；`ip route` | | | |
| L2 | **有无 NetworkManager，有无 `nmcli` / `iw`。** (a) NetworkManager 在用：照 L1。(b) 在一台可以随便折腾的机器上停掉 NetworkManager，用 `wpa_supplicant` 连上 `<A>`，重启 rurge。(c) 再把 `nmcli` 与 `iw` 都从 `PATH` 里拿掉（比如 `env PATH=/usr/bin/nothing ./rurge …` 并给 rurge 的二进制写全路径），重启 | (a) 经 `nmcli` 读到名字，选择 `DIRECT`。(b) 退到 `iw dev <网卡> link`，仍读到名字，选择 `DIRECT`。(c) 日志里**一次** DEBUG `a Wi-Fi interface is up but neither NetworkManager nor iw gives a name for the network; SSID and BSSID conditions will not match`，选择是 `REJECT-TINYGIF`，`TYPE:` 与 `ROUTER:` 照常，不报错 | 日志；当前选择 | | | |
| L3 | **rurge 不引发 Wi-Fi 扫描。** NetworkManager 的笔记本上，连着 Wi-Fi，先不开 rurge，执行 `iw event -t \| grep -i scan` 15 分钟，数一数 `scan started`；再开 rurge（空闲）同样 15 分钟 | 有 rurge 时的扫描次数与没有时相当，**没有每 30 秒一次**的规律（rurge 问 `nmcli` 用的是 `--rescan no`，只读 NetworkManager 已有的列表）；NetworkManager 自己的后台扫描两次都会有 | `iw event -t` 的输出 | | | |
| L4 | **含非 ASCII 字母的名字，与 `LANGUAGE`。** (a) 连一个名字含中文的 Wi-Fi，`<A>` 写这个名字，在 `LANG=zh_CN.UTF-8` 的环境里运行 rurge。(b) glibc 早于 2.39 的发行版上设 `LANGUAGE=zh_CN` 再运行 | (a) 名字完整读到，选择 `DIRECT`（rurge 用 `LC_ALL=C.UTF-8` 问 `nmcli`）。(b) 只设 `LANGUAGE` 时 `nmcli` 可能仍把 `yes` 译成中文，`ACTIVE` 一列读不出，rurge 退到 `iw`，名字照读；读不到的话把 `nmcli -t -f ACTIVE,SSID,BSSID,DEVICE dev wifi list --rescan no` 的输出抄下来（补救是给子进程清空 `LANGUAGE`，一行） | 日志；当前选择；`nmcli` 的输出 | | | |
| L5 | **只换 DNS 服务器。** 在 NetworkManager 里只改当前连接的 DNS 服务器（不换网卡、不换地址）；如果系统用 systemd-resolved，先看 `resolvectl status` 与 `/run/systemd/resolve/resolv.conf` | 数秒内一行 INFO `network changed`（DNS 服务器变了也算变化；DEBUG `the network is not what it was` 里 `dns_servers=true`）；rurge 读的是 systemd-resolved 记下的上游，不是 `127.0.0.53` 的本机桩 | 日志 | | | |
| L6 | **`[SSID Setting]` 的 DNS 覆盖与接管暂停：对真实的系统代理。** 照 W6、W7 做；GNOME 用 `gsettings get org.gnome.system.proxy mode` 与 `gsettings get org.gnome.system.proxy.http host`，KDE 用 `kreadconfig6 --file kioslaverc --group 'Proxy Settings' --key ProxyType` | 同 W6、W7：暂停时恢复成原来的值，离开时指向 `127.0.0.1:6152`；`GET /v1/features/system_proxy` 暂停时是 `false`。不支持的桌面（见阶段 1 清单）上 `--system-proxy` 照旧失败 | `gsettings` / `kreadconfig` 的输出；API；日志；`state.json` | | | |
| L7 | **睡眠与唤醒。** `systemctl suspend`，换网络唤醒，再唤醒到原来的网络 | 同 W8 | 日志；当前选择 | | | |
| L8 | **空闲开销（NFR-02）。** rurge 空闲运行 10 分钟，`top` 看 CPU；`sudo strace -f -e trace=execve -p <rurge 的 pid> 2>&1 \| grep -v ENOENT`（或 `execsnoop`）看子进程 | CPU 接近 0；有无线网卡开着时每 30 秒一次 `nmcli`（或 `iw`），没有变化时再没有别的子进程；没有无线网卡开着时一个子进程也没有 | `top`；`strace` | | | |

---

### 与平台无关

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| P1 | **`subnet` 组的临时覆盖，换网络时清除。** 运行 rurge；`curl -s -X POST -H "X-Key: testkey" -d '{"group_name":"Net","policy":"DIRECT"}' http://127.0.0.1:6171/v1/policy_groups/select`（Windows 上 `curl.exe` 要把引号转义，写法见 W7）；读选择；换一个网络（W1 的换法）；再读 | 覆盖设置后选择是 `DIRECT`，不论网络；换网络之后回到按新网络选的那个（覆盖没了）；不在组里的名字是 400 `` `Somewhere` is not a member of `Net` ``；空的 `policy` 清除覆盖。重启 rurge 后覆盖也不在 | 当前选择；`state.json` 里没有这个覆盖 | | | |
| P2 | **换网络之后 `url-test` 重测**（用你自己的真实节点）。组 `Auto = url-test, <节点 1>, <节点 2>`（可加 `evaluate-before-use = true`），`FINAL,Auto`。先发几个请求让它测过；`GET /v1/policy_groups/test_results`；换网络；发一个请求；`GET /v1/requests/recent`。另放一个 `smart` 组对照 | 换网络之后的第一次使用触发一轮新的测试：请求记录里出现 `listener` 为 `internal`、`rule` 为 `policy test` 的会话，时间在换网络之后；写了 `evaluate-before-use` 时这第一个请求等这一轮测完（至多最长的测试超时）才走；`smart` 组不因换网络而重测。新一轮选出的节点与新网络上的延迟相符 | `GET /v1/requests/recent`；`test_results` | | | |
| P3 | **WireGuard 在换网络后的恢复**（用你自己的 WireGuard 或 WARP 节点）。经该策略持续请求（`curl` 循环）；换网络 | 预期换网络后数秒内经隧道的请求恢复（此前靠载体发送失败或握手无回应才换载体，最坏约两分钟）；日志里没有一直重复的 `wireguard: the peer did not answer the handshake`。没有专门的日志行说隧道换了载体，只能从请求的恢复看；记下实际用了多久 | 请求的成败与耗时；日志 | | | |
| P4 | **真实的 DoH 地址写进 `[SSID Setting]`。** `SSID:<A> encrypted-dns-server=https://dns.google/dns-query?x=1,https://cloudflare-dns.com/dns-query`（含 `?` 与 `=`，两个 URL 用逗号分开）；另试一行带用户名口令的 `https://user:pw@…`（口令是编的，只看会不会出现在输出里）；再试全局配了 `encrypted-dns-server`、这一行写 `encrypted-dns-server=off` | `rurge check -c <conf>` 没有 `W0028`，也没有 `E`；两个 URL 都是列表项（不被当成参数），连着 `<A>` 时 `upstreams` 里是这两个 DoH 上游，经代理的请求能解析；带口令的那行，口令不出现在 `check` 的输出、日志、`GET /v1/dns` 与默认的（`sensitive=0`）`GET /v1/profiles/current` 里；`off` 的网络上 `upstreams` 是传统的 | `rurge check`；`GET /v1/dns`；日志 | | | |
| P5 | **VPN 接管默认路由。** 连上一个全隧道的 VPN 客户端（试两种：基于 WireGuard / utun 的，和基于 TAP 或 PPP 的），看 rurge | 连上与断开时各有一行 INFO `network changed`；主网卡变成 VPN 的网卡（`kind` 一般是 `other`；TAP 一类的虚拟以太网卡在 Linux 上是 `wired`，在 Windows 上多半也是），`TYPE:WIFI` 不再命中、`ROUTER:` 比的是 VPN 的网关；`subnet-exp-wifi-always-match`（缺省 true）让 `SSID:` 仍然看 Wi-Fi 连着的网络。把用到的 VPN 客户端和观察到的 `kind` 记下来（已知限制：rurge 不区分物理与虚拟网卡，排除别的 VPN 没有计划） | 日志的 `kind` 与 `interface` | | | |
| P6 | **去抖。** 在一秒内把 Wi-Fi（或网线）反复关开三五次，最后停在连着的状态 | `network changed` 至多每秒一行：第一次变化立刻一行，其余合并成窗口结束时的一行（用最新的网络）；一个在窗口里又回到原样的网络不另出一行；最终选择与最终的网络一致 | 日志里 `network changed` 的行数与时间 | | | |
| P7 | **同一个 SSID 下换接入点。** 两个接入点同一个 SSID，在它们之间漫游（或把设备搬到另一个接入点）；地址不变，BSSID 变了。`Net` 组加一个 `BSSID:` 条件 | 约 30 秒内（有无线网卡开着时每 30 秒重读一次 Wi-Fi）一行 INFO `network changed`；DEBUG `where the machine is now` 里 `bssid` 是新的；`BSSID:` 条件随之变 | 日志 | | | |
| P8 | **个人信息不进 INFO。** 在默认日志级别下换几次网络，对日志文件搜索 SSID、BSSID 的前几段、网关地址、本机地址和 DNS 服务器地址 | 一个也搜不到；`--log-level debug` 时 SSID、BSSID 与网关只出现在 `where the machine is now` 一行里，本机地址与 DNS 服务器的取值任何级别都不出现 | 日志文件 | | | |
| P9 | **IPv6 隐私地址不引发变化。** 在 IPv6 临时地址会轮换的网络上让 rurge 空闲运行 24 小时以上 | 没有只因为临时地址轮换而出现的 `network changed`（rurge 只比较 IPv6 的 /64 前缀）。有其它原因的变化（换网络、DHCP 租约换了地址）照常出现 | 日志 | | | |
| P10 | **离线命令读真实的网络。** 在有线与 Wi-Fi 上各执行 `rurge rule match -c <conf> example.com`、`rurge dns lookup -c <conf> example.com --trace`、`rurge check -c <conf>` | `rule match` 在有线上命中 `SUBNET,TYPE:WIRED,REJECT`，在 Wi-Fi 上落到 `FINAL`；`dns lookup` 在 `<A>` 上问的是 `8.8.8.8`（`--server` 仍然最后说了算）；`check` 不读网络，在没有网络的机器上也零错误，不再说 `SUBNET` 永不匹配 | 各命令的输出 | | | |
