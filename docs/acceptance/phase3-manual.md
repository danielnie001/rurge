# 阶段 3 手工验收清单

需要真实网卡、真实的 Wi-Fi 与有线网络、真实的系统代理设置和真实的位置权限的项目。自动化测试只用回环地址，用文件后端（`RURGE_NETWORK_BACKEND`、`RURGE_SYSTEM_PROXY_BACKEND`）模拟网络和系统代理，覆盖不了这些：读网络的那几条命令与文件在 Linux 与 macOS 上只对着假的机器测过（`docs/superpowers/plans/2026-10-16-phase3-m1-network-subnet-plan.md` 末尾「延后事项」第 1、3 行），非英文系统上真实的 `netsh wlan show interfaces` 输出（开发机的系统语言是中文，量的时候 Wi-Fi 是断开的，输出里没有 `SSID` 与 `BSSID` 这两行，所以这两个键本身没有见过，见过的只是断开时就有的键）、含非 ASCII 字母的真实网络名、位置权限和 VPN 这些情形没有量过（解析器对着按公开格式写的文本夹具测过，其中有含非 ASCII 字母的名字）。由项目所有者在自己的机器上验收；每一项记下日期、系统版本与结果。**一项也没有做过。**

清单随里程碑逐节增加：下面是 M1（网络环境与子网）、M2（进程识别）、M3（内存设备上的 TUN 内核，M3a、M3b、M3c 合成一节）、M4（增强模式：三个平台的虚拟网卡、地址与路由、系统 DNS、防环、权限）与 M5（识别、阻断与 `[IP Rewrite]`），M6 一节补齐验收文档里“真机上”写着“缺”的项。M1 的设计见 `docs/superpowers/specs/2026-10-16-phase3-m1-network-subnet-design.md`，M2 的设计见 `docs/superpowers/specs/2026-10-17-phase3-m2-process-design.md`，M3 的设计见 `docs/superpowers/specs/2026-10-18-phase3-m3-tun-core-design.md`，M4 的设计见 `docs/superpowers/specs/2026-10-26-phase3-m4-platform-design.md`（先读它第 21 节与末尾「执行期的订正」一节），M5 的设计见 `docs/superpowers/specs/2026-10-28-phase3-m5-recognition-blocking-ip-rewrite-design.md`（先读它第 20、21 节的订正）；API 与日志行见 `docs/api/phase3.md`，兼容性清单里的差异见 `docs/surge-compatibility-matrix.md` 的 2.3、3.1、3.4、3.5、4.1、5.1、5.2、6.4、9.1、9.4、10.4 节。**M1 与 M2 的各节在 `rurge run` 上就能做；M4 一节要真机（每个平台的增强模式都要 root / 管理员）；M3、M5 与 M6 一节要用 M4 的增强模式（`--enhanced-mode`）。**

## 怎么用

阶段 3 / M6 修正第 1 轮改了「第 14 节八条各由哪几项证明」的索引表（第 1、6 条）与「改系统设置的项与恢复」里的 Linux 系统代理、第 1 条 (b) 的 DNS 恢复命令。

**先做哪一节。** 建议的次序是 M1 → M2 → M4 → M3 → M5 → M6：M1（网络环境与子网）与 M2（进程识别）不需要虚拟网卡，可以先在普通网络上看真实系统；M4 把三个平台的虚拟网卡、地址、路由与系统 DNS 接起来；M3（TUN 内核）、M5（识别、阻断与 `[IP Rewrite]`）与 M6（补齐）的项都要在增强模式开着时做。每一节里先做不改系统设置的项，再做开增强模式、改系统代理或改 `rp_filter` 的项；做完一项就让 rurge 恢复一次，别把几项叠在一起做。

**共用的前置条件。**
- 一次性的 Windows、macOS、Linux 机器或能还原的虚拟机；Windows 要管理员，macOS 与 Linux 要 root。这些项会建虚拟网卡、改路由、改系统 DNS、改系统代理与 `rp_filter`，还会装计划任务、停 NetworkManager、改 `hidepid`。
- Windows 要一个 WireGuard LLC 签名的 `wintun.dll` 0.14 或更新，放在 `rurge.exe` 旁边或用 `--wintun-dll` 指过去（W19、W35、W36）。
- `cargo build --release` 得到的二进制；一份测试用 Profile（各节的前置条件里写）；一个局域网里的另一台机器（M3 的几个脚本要用）。
- 真实的 Wi-Fi、有线网络、一个全隧道 VPN、一个载 UDP 的代理与一个支持 HTTP/3 的服务（M1、M4、M5 的项按需要）。
- 每一项记下「结果」「日期」「系统版本」三栏；连原始输出一起记下的项，照那一项自己写的做。

### 改系统设置的项与恢复

开增强模式、改路由与系统 DNS、改系统代理、改 `rp_filter`、装开机自启、建计划任务、停 NetworkManager、改 `hidepid`、加防火墙规则这些项，都只在一次性的机器或能还原的虚拟机上做；Windows 要管理员，macOS 与 Linux 要 root。不以管理员 / root 运行的那几项（W37、M38、L40）不改任何设置。

做完一项先让 rurge 自己恢复：Ctrl-C 或 `rurge stop`（增强模式先关、系统代理后关），再核对下面几样回到原值。rurge 在正常退出、`POST {"enabled":false}`、接管暂停与下一次启动时都会恢复（强杀之后靠下一次启动；Windows 的网卡随进程消失，macOS 的各服务 DNS 与 Linux 的 `rp_filter` 会留到下一次启动）。备份在数据目录的 `state.json`（`enhanced_backup`、`system_proxy_backup`）；备份是 `null` 时不用手工恢复。

手工恢复的命令（照代码里 rurge 自己恢复时用的写法）：
- Windows 的系统代理：按 W7 开头用 `Get-ItemProperty` 抄下的三个原值写回 `HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings` 的 `ProxyEnable`、`ProxyServer`、`ProxyOverride`（`Set-ItemProperty` 三条），再注销重新登录让 WinINet 读到；rurge 自己恢复时另用 `InternetSetOptionW` 通知 WinINet。
- macOS 的各网络服务 DNS：`sudo networksetup -setdnsservers "<服务>" empty`（原来一条也没有时），或按 M20 / M29 抄下的地址列表写回；系统代理用 `networksetup -setwebproxy` / `-setsecurewebproxy` / `-setsocksfirewallproxy <服务> <主机> <端口>` 与对应的 `-set…state <服务> on|off`。
- Linux 的系统 DNS：链路随增强模式消失，`resolvectl revert rurge` 是 rurge 关闭时用的那一条；`rp_filter` 用 `echo 1 | sudo tee /proc/sys/net/ipv4/conf/all/rp_filter` 写回（原值照 L22 读到的）。
- Linux 的系统代理（L6、L7）：GNOME 用 `gsettings set` 把 L6 开头用 `gsettings get` 抄下的原值写回——`org.gnome.system.proxy` 的 `mode` 与 `ignore-hosts`，`org.gnome.system.proxy.http` 的 `host` 与 `port`，`org.gnome.system.proxy.https` 的 `host` 与 `port`，`org.gnome.system.proxy.socks` 的 `host` 与 `port`；rurge 自己恢复时先写别的键，最后写 `mode`。KDE 用 `kwriteconfig6 --file kioslaverc --group 'Proxy Settings' --key <键> <原值>`（Plasma 5 是 `kwriteconfig5`；原值是空的用 `--delete` 代替值）——键是 `ProxyType`、`httpProxy`、`httpsProxy`、`socksProxy`、`NoProxyFor`，原值用 L6 开头的 `kreadconfig6` 抄下；写完再 `dbus-send --type=signal /KIO/Scheduler org.kde.KIO.Scheduler.reparseSlaveConfiguration string:`，让 KIO 重读。
- 第 1 条 (b) 改过的系统 DNS（W49、L41、M30）：Windows `Set-DnsClientServerAddress -InterfaceAlias <网卡> -ResetServerAddresses`（回到 DHCP），或用 `-ServerAddresses` 写回记下的地址；Linux `sudo resolvectl revert <物理链路>`，再让管网络的程序重新下发（NetworkManager 是 `sudo nmcli device reapply <链路>`，systemd-networkd 是 `sudo networkctl reconfigure <链路>`）；macOS `sudo networksetup -setdhcp "<服务>"`，再 `sudo networksetup -setdnsservers "<服务>" <记下的地址>`（原来一条也没有就写 `empty`）。
- Windows 的防火墙规则（W42 的 ICMP 入站）：`Remove-NetFirewallRule -DisplayName rurge-icmp-in`。
- 计划任务（W4）：`schtasks /delete /tn rurge-sys /f`；SMB 共享与目录联接（W13）：`Remove-SmbShare -Name tools`，再删掉联接；`hidepid`（L12）：`sudo mount -o remount,hidepid=0 /proc`；NetworkManager（L2）：`sudo systemctl start NetworkManager`。

### 第 14 节八条各由哪几项证明

| 第 14 节 | 要看到什么 | Windows | macOS | Linux | 与平台无关 |
| --- | --- | --- | --- | --- | --- |
| 1 | 浏览器、命令行工具与不支持代理的应用的流量出现在请求记录里，并按规则分流 | W48、W18、W27 | M37、M16、M25 | L39、L17、L27 | P25、P31 |
| 2 | `ping` 正常 | W26、W41 | M24 | L26 | P31 |
| 3 | fake-IP 映射与真实连接一致 | W18 | M16 | L17 | P33、P34、P35、P26 |
| 4 | 网络切换后 30 秒内恢复 | W24 | M22 | L24 | P31 |
| 5 | `PROCESS-NAME` 在三个平台命中 | W11、W12、W15 | M9、M11、M13 | L10、L11、L13 | P17 |
| 6 | 真实设备上的三项基准：fake 应答延迟、单流吞吐、一万条连接的内存 | P19 ～ P21 在 Windows 上的那一次 | P19 ～ P21 在 macOS 上的那一次 | P19 ～ P21 在 Linux 上的那一次 | P19、P20、P21 |
| 7 | 需求逐句对照过代码、三个数照实写 | — | — | — | `docs/acceptance/phase3-acceptance.md` 的第一部分（手工的项证不了它） |
| 8 | 三平台 CI 绿 | — | — | — | 一次推送之后的 CI 运行（M5、M6 的提交没有推送之前写“CI 没有见过”） |

## M1　网络环境与子网

阶段 3 / M6 订正了 M1、L2、L6（M1 的 `networksetup` 前面多余的 `./` 去掉了；L2 补了停掉 NetworkManager 之后的恢复命令；修正第 1 轮让 L6 先把系统代理各键的原值都抄下来）。会改系统设置的项（W4、W7、W8、M5、M6、L2、L6、L7、L12）按上面「改系统设置的项与恢复」做。

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
| M1 | **真实的换网络：Wi-Fi 与有线。** 只连 Wi-Fi `<A>` 运行 `./rurge run -c <conf> --log-level debug`；读当前选择；接上有线（USB 或雷电以太网适配器）；再读；拔掉；再读。再切一次 Wi-Fi（`<A>` → `<B>`） | 同 W1、W2，只是靠路由套接字，反应比 Windows 与 Linux 的 2 秒轮询更快；有线适配器的硬件端口名（`networksetup -listallhardwareports` 里的 `Hardware Port:`）被认成有线，`kind=wired` | 日志；当前选择；`networksetup -listallhardwareports` | | | |
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
| L2 | **有无 NetworkManager，有无 `nmcli` / `iw`。** (a) NetworkManager 在用：照 L1。(b) 在一台可以随便折腾的机器上停掉 NetworkManager，用 `wpa_supplicant` 连上 `<A>`，重启 rurge。(c) 再把 `nmcli` 与 `iw` 都从 `PATH` 里拿掉（比如 `env PATH=/usr/bin/nothing ./rurge …` 并给 rurge 的二进制写全路径），重启；做完用原来的方式启动 NetworkManager（systemd 的发行版是 `sudo systemctl start NetworkManager`） | (a) 经 `nmcli` 读到名字，选择 `DIRECT`。(b) 退到 `iw dev <网卡> link`，仍读到名字，选择 `DIRECT`。(c) 日志里**一次** DEBUG `a Wi-Fi interface is up but neither NetworkManager nor iw gives a name for the network; SSID and BSSID conditions will not match`，选择是 `REJECT-TINYGIF`，`TYPE:` 与 `ROUTER:` 照常，不报错 | 日志；当前选择 | | | |
| L3 | **rurge 不引发 Wi-Fi 扫描。** NetworkManager 的笔记本上，连着 Wi-Fi，先不开 rurge，执行 `iw event -t \| grep -i scan` 15 分钟，数一数 `scan started`；再开 rurge（空闲）同样 15 分钟 | 有 rurge 时的扫描次数与没有时相当，**没有每 30 秒一次**的规律（rurge 问 `nmcli` 用的是 `--rescan no`，只读 NetworkManager 已有的列表）；NetworkManager 自己的后台扫描两次都会有 | `iw event -t` 的输出 | | | |
| L4 | **含非 ASCII 字母的名字，与 `LANGUAGE`。** (a) 连一个名字含中文的 Wi-Fi，`<A>` 写这个名字，在 `LANG=zh_CN.UTF-8` 的环境里运行 rurge。(b) glibc 早于 2.39 的发行版上设 `LANGUAGE=zh_CN` 再运行 | (a) 名字完整读到，选择 `DIRECT`（rurge 用 `LC_ALL=C.UTF-8` 问 `nmcli`）。(b) 设了 `LANGUAGE` 也读得到名字，选择 `DIRECT`：rurge 给 `nmcli` 的子进程把 `LANGUAGE` 设成空（glibc 2.39 之前 `LANGUAGE` 对 `C.UTF-8` 仍然生效，会让 `nmcli` 把 `yes` 译成中文，`ACTIVE` 一列读不出）；读不到的话把 `env LC_ALL=C.UTF-8 LANGUAGE= nmcli -t -f ACTIVE,SSID,BSSID,DEVICE dev wifi list --rescan no` 的输出抄下来 | 日志；当前选择；`nmcli` 的输出 | | | |
| L5 | **只换 DNS 服务器。** 在 NetworkManager 里只改当前连接的 DNS 服务器（不换网卡、不换地址）；如果系统用 systemd-resolved，先看 `resolvectl status` 与 `/run/systemd/resolve/resolv.conf` | 数秒内一行 INFO `network changed`（DNS 服务器变了也算变化；DEBUG `the network is not what it was` 里 `dns_servers=true`）；rurge 读的是 systemd-resolved 记下的上游，不是 `127.0.0.53` 的本机桩 | 日志 | | | |
| L6 | **`[SSID Setting]` 的 DNS 覆盖与接管暂停：对真实的系统代理。** 照 W6、W7 做；**先把「怎么用」的恢复命令里列的那些键的原值都抄下来**（GNOME 八项、KDE 五项）：GNOME 用 `gsettings get org.gnome.system.proxy mode` 与 `gsettings get org.gnome.system.proxy.http host`，KDE 用 `kreadconfig6 --file kioslaverc --group 'Proxy Settings' --key ProxyType` | 同 W6、W7：暂停时恢复成原来的值，离开时指向 `127.0.0.1:6152`；`GET /v1/features/system_proxy` 暂停时是 `false`。不支持的桌面（见阶段 1 清单）上 `--system-proxy` 照旧失败 | `gsettings` / `kreadconfig` 的输出；API；日志；`state.json` | | | |
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

自动化里真的读系统的表的，主要是每个平台一条读自己回环连接的用例（`this_process_owns_its_loopback_sockets`）和一条起真实二进制的端到端用例（`a_process_rule_matches_the_test_process`）；这几条用例在 CI 上编译、运行并通过（运行 40283632986 起，各在它所属的平台上；任务日志要登录才能读，逐条的结果没有读到），其中 Linux 与 macOS 的真读用例在这台 Windows 开发机上编译不到；Linux 与 macOS 的真实读取（`/proc` 的内容与权限、`pcblist_n` 的真实布局、`proc_pidpath`）在开发机上只对着按公开格式写的夹具测过。下面是 CI 与回环验不了的：真实的浏览器与命令行工具会显示成哪个进程、别的用户的进程、虚拟机与容器的客户端、macOS 上代建的连接取哪个 PID、几百条并发连接时查找在不在 200 毫秒里结束。**一项也没有做过。**编号接着 M1 往下排（W11、M9、L10、P11），整份清单里的编号不重复。

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
- 日志：`rurge run -c <conf> --log-level debug --log-file <日志路径>`。查找的行是 DEBUG 的 `process lookup found the process`（字段 `src`、`remote`、`elapsed_ms`、`pid`、`name`、`path`；`remote` 是客户端那条连接在 rurge 一侧的端点，HTTP 与 SOCKS5 的会话是被接受的那条连接的本地地址，如 `127.0.0.1:6152`，阶段 3 / M3b 之前这个字段叫 `listen_port`）与 `process lookup found no process`。
- 要被拒的会话不需要外网：命中 `REJECT` 的会话 `status` 是 `rejected`；放行的会话要能连上下文里的网站（写作 `http://example.com/`）。
- 对进程号：Windows 用任务管理器的"详细信息"页（加上"命令行"一列），macOS 与 Linux 用 `ps -p <pid> -o pid,comm,args`。
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
| W11 | **命令行工具与浏览器，经 HTTP 代理，记录里是哪个进程。** `rurge.exe run -c <conf> --log-level debug`。(a) PowerShell：`$p = Start-Process curl.exe -ArgumentList '-x','http://127.0.0.1:6152','--limit-rate','1k','http://example.com/','-o','NUL' -PassThru; $p.Id`（限速是为了让它多活一会儿），读 `recent` 后 `Stop-Process $p`；(b) Edge、Chrome、Firefox 各一个，在设置里手动指定 HTTP 代理 `127.0.0.1:6152`，打开 `http://example.com/`；(c) `<conf>` 的 `[Rule]` 里加 `DOMAIN,ads.test,REJECT`，`curl.exe -x http://127.0.0.1:6152 http://ads.test/` | (a) 记录的 `pid` 等于 `$p.Id`，`processPath` 是 `curl.exe` 的完整路径（形如 `C:\Windows\System32\curl.exe`，JSON 里反斜杠写作 `\\`）；DEBUG 有一行 `process lookup found the process`，`pid` 与 `path` 相同，`src` 是 `127.0.0.1:<端口>`，`remote` 是 `127.0.0.1:6152`，`elapsed_ms` 在个位数到几十毫秒。(b) 每个浏览器的记录都有 `pid` 与 `processPath`；浏览器常把网络请求放在单独的网络服务进程里，路径不一定是主程序的——把每个显示成什么记下来，并用任务管理器对 PID。(c) 被拒的会话 `status` 是 `rejected`，`pid` 与 `processPath` 同样有：查找可能在会话结束之后才结束，记录读的时候再看 | `recent`；DEBUG 日志；任务管理器 | | | |
| W12 | **`PROCESS-NAME` 的三种写法与大小写。** 一次一条规则，写在 `FINAL,DIRECT` 前面，重载后每次 `curl.exe -x http://127.0.0.1:6152 http://example.com/ -I`：① `PROCESS-NAME,curl.exe,REJECT`；② `PROCESS-NAME,CURL.EXE,REJECT`；③ `PROCESS-NAME,cur*,REJECT`；④ `PROCESS-NAME,curl,REJECT`；⑤ `PROCESS-NAME,C:\Windows\System32\curl.exe,REJECT`；⑥ `PROCESS-NAME,c:/windows/system32/CURL.exe,REJECT`；⑦ `PROCESS-NAME,C:\Windows\System32\,REJECT`（前缀）；⑧ `PROCESS-NAME,c:/windows/,REJECT`。每条之后也跑一次 `rurge.exe check -c <conf>` | ①②③⑤⑥⑦⑧ 被拒（`status` 为 `rejected`，`rule` 是这一行）；**④ 不命中**——Windows 上不去掉 `.exe`，写 `curl` 匹配不到 `curl.exe`——，会话走到 `FINAL,DIRECT`。文件名与路径都不分大小写、路径里 `/` 与 `\` 通用（这是 rurge 在 Windows 上的读法，手册只讲 Mac）。`check` 对每一条都没有错误、没有 `W0013`（不再说 `PROCESS-NAME` 永不匹配） | `recent` 的 `rule` 与 `status`；`check` 的输出 | | | |
| W13 | **UNC 路径与目录联接上的程序。** (a) 在本机共享一个目录（资源管理器里共享，或管理员 PowerShell 里先建好 `C:\tools`，再 `New-SmbShare -Name tools -Path C:\tools`），把 `curl.exe` 拷进去，经 UNC 路径运行：`\\localhost\tools\curl.exe -x http://127.0.0.1:6152 http://example.com/ -I`；(b) 建目录联接 `cmd /c mklink /J C:\toolslink C:\Windows\System32`，运行 `C:\toolslink\curl.exe …`。规则分别写 `PROCESS-NAME,\\localhost\tools\curl.exe,REJECT`、`PROCESS-NAME,\\localhost\tools\,REJECT` 与 `PROCESS-NAME,C:\toolslink\curl.exe,REJECT` | (a) 记下 `processPath` 实际的写法（预期是 `\\localhost\tools\curl.exe`），以 `\\` 开头的两条规则读作路径与前缀，应当命中它；写法不同于预期时抄下来。(b) 记下 `processPath` 是 `C:\toolslink\curl.exe` 还是系统解开联接之后的 `C:\Windows\System32\curl.exe`，和哪条规则命中——路径是文字比较，rurge 不替使用者展开联接。用完 `Remove-SmbShare` 并删掉联接 | `recent`；规则的命中 | | | |
| W14 | **权限：普通用户、管理员与 SYSTEM 运行的 rurge 看得到哪些进程。** (a) 以普通用户运行 rurge，另开一个管理员 PowerShell 运行 `curl.exe -x http://127.0.0.1:6152 http://example.com/ -I`，再用普通用户的 PowerShell 同样运行一次；(b) 以管理员运行 rurge，重复；(c) 以 SYSTEM 运行 rurge（W4 的 `schtasks` 做法），重复 | 读得到的有 `pid` 与 `processPath`；**读不到的两个都是 `null`**（打不开进程就是"不知道"，没有"只有 PID"的结果），会话照常完成，没有 WARN，也没有会话因此失败。把 (a) 里管理员的 curl 读不读得到、(b)(c) 里普通用户的 curl 读不读得到写下来——这是普通用户运行时哪些进程看不见的实际范围 | `recent`；日志 | | | |
| W15 | **真实流量里读不到进程的占多少。** `rurge.exe run -c <conf> --system-proxy --log-level debug`，正常使用电脑十分钟（浏览器、聊天与会议软件、Windows 更新、商店应用），然后统计 `recent`：`(curl.exe -s -H "X-Key: testkey" "http://127.0.0.1:6171/v1/requests/recent?limit=1000" \| ConvertFrom-Json).requests \| Group-Object processPath \| Sort-Object Count -Descending \| Format-Table Count,Name`，另数 `pid` 为 `null` 的条数 | 大多数会话有 `pid` 与 `processPath`；`null` 的是已经退出的短命客户端、读不了的进程、局域网与虚拟机里的设备。把 `null` 的占比与其中最常见的目标记下来；同一个 `svchost.exe` 承载多个服务，`PROCESS-NAME` 分不开它们。日志里没有 `process lookup took longer than 200 ms`，也没有 `process lookup: the tcp table could not be read` | `recent`；日志 | | | |

---

### macOS

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| M9 | **命令行工具与浏览器，经 HTTP 代理，记录里是哪个进程。** `./rurge run -c <conf> --log-level debug`。(a) `curl -x http://127.0.0.1:6152 --limit-rate 1k http://example.com/ -o /dev/null & echo $!`，读 `recent` 后 `kill %1`；(b) Safari、Chrome、Firefox 各一个，在系统设置的"网络 → 代理"里指定网页代理 `127.0.0.1:6152`（或 `./rurge run … --system-proxy`），打开 `http://example.com/`；(c) 加 `DOMAIN,ads.test,REJECT`，`curl -x http://127.0.0.1:6152 http://ads.test/` | (a) 记录的 `pid` 等于 `$!`，`processPath` 是 `/usr/bin/curl`；DEBUG 里 `process lookup found the process` 的 `pid`、`path` 相同，`elapsed_ms` 在个位数到几十毫秒。(b) 记下每个浏览器显示成哪个进程（Safari 的网络请求多半在 `com.apple.WebKit.Networking` 里，Chrome 在网络服务的 Helper 进程里，以实际为准）并用 `ps -p <pid> -o pid,comm,args` 对。(c) 被拒的会话同样有 `pid` 与 `processPath` | `recent`；DEBUG 日志；`ps` | | | |
| M10 | **代建的连接：`so_last_pid` 还是 `so_e_pid`。** 系统服务替 App 建连接时（`nsurlsessiond` 替用后台 `URLSession` 的 App 下载之类），套接字的"最后使用者"和"委托的实际进程"可能不是同一个。`./rurge run -c <conf> --system-proxy --log-level debug`，触发这样的流量（在 App Store 里更新或下载一个 App、在"音乐"或"播客"里下载一首歌，或你知道的别的经 `nsurlsessiond` 的 App），读 `recent`，再对照 `nettop -n -m tcp -L 1`（按进程分组列出连接）里同一条连接挂在谁名下。另试两条规则：`PROCESS-NAME,nsurlsessiond,REJECT` 与 `PROCESS-NAME,<那个 App 的文件名>,REJECT` | 观察项，不预设答案：记下这条连接在 `recent` 里显示成谁（rurge 取 `so_last_pid`）、在 `nettop` 里归谁、哪一条规则命中。若 rurge 显示的是 `nsurlsessiond` 而使用者想写进规则的是那个 App，那就是 `so_last_pid` 与 `so_e_pid` 的取舍落到了使用者头上：把 App、系统版本与两种显示写下来，交项目所有者决定要不要改取 `so_e_pid`（设计 M2-D10；计划「延后事项」第 2 行） | `recent`；`nettop`；规则的命中 | | | |
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
| P14 | **经 SOCKS5 UDP ASSOCIATE 的客户端。** `[Rule]` 里 `PROCESS-NAME,<python 的文件名>,DIRECT`（Windows 写 `python.exe`，别的系统写真实路径的文件名，见 L11），`FINAL,REJECT`；运行前置条件里的 `socks5_udp.py`（或任何走 SOCKS5 UDP 的客户端，如支持 UDP 的游戏或语音软件）；读 `recent` 与 DEBUG 日志 | 两个目标都收到应答（`1.1.1.1` 与 `8.8.8.8` 能连上时）——说明 `PROCESS-NAME` 在 UDP 流上命中了控制连接的进程（认不出进程的流会落到 `FINAL,REJECT`，没有应答）。`recent` 里 `transport` 为 `udp` 的两条记录有 `pid` 与 `processPath`（python 的），是同一个进程；DEBUG 里这个关联只有**一行** `process lookup found the process`（`remote` 是 `127.0.0.1:6153`，`src` 是控制连接的来源），不是每条流一行 | `recent`；DEBUG 日志；脚本的输出 | | | |
| P15 | **IPv6 回环与双栈的客户端。** `http-listen = 127.0.0.1:6152, [::1]:6152`（重启）。(a) `curl -x 'http://[::1]:6152' http://example.com/ -I`（Windows 用 `curl.exe`）；(b) 一个双栈套接字去连 IPv4 的代理地址：`python3 -c "import socket,time; s=socket.create_connection(('::ffff:127.0.0.1',6152)); s.sendall(b'CONNECT example.com:80 HTTP/1.1\r\nHost: example.com:80\r\n\r\n'); time.sleep(3)"`（Windows 用 `python`） | (a) 来源是 `[::1]:<端口>`，记录有 `pid` 与 `processPath`（curl 的）。(b) 来源按 IPv4 显示成 `127.0.0.1:<端口>`（IPv4 映射的地址按 IPv4 处理），记录有 `pid` 与 `processPath`（python 的）——客户端的行在 Linux 的 `tcp6` 表里以 `::ffff:127.0.0.1` 出现，在 macOS 上是带 `INP_IPV4` 标志的双栈记录（计划「执行期修正记录」第 11 行），Windows 上记在哪张表里要看实际；rurge 先找 IPv4 的行、再找 IPv6 表里的映射形式。哪个平台上 (b) 查不到，就是这条路径在真实的系统上不通，抄下平台与版本 | `recent`；DEBUG 日志 | | | |
| P16 | **进程不进 INFO。** 缺省日志级别（`loglevel = notify`，INFO）下，用 W11 / M9 / L10 的办法产生几条被拒与放行的会话（含 `ads.test` 那种被拒的），然后在日志文件里搜进程的名字（`curl`）、路径里的目录名和 PID。再用 `--log-level debug` 做同样的搜索 | 缺省级别下**搜不到**查到的进程：被拒与失败的会话的 INFO 行带来源、目标、规则文字和策略；规则文字里的 `PROCESS-NAME,curl,…` 是使用者自己写的，不是查到的。`--log-level debug` 时进程的名字、路径与 PID 只出现在 `process lookup found the process` 那一行（和请求记录、API 里）。两句 WARN 不带会话、地址与进程 | 日志文件 | | | |
| P17 | **`check` 与 `rule match`。** `<conf>` 里写 `PROCESS-NAME,curl,DIRECT`、`PROCESS-NAME,C:\Program Files\App\app.exe,DIRECT`、`PROCESS-NAME,/Applications/Foo.app/Contents/MacOS/*,DIRECT`、`DEVICE-NAME,Kids-iPad,DIRECT`，在三个平台上各跑 `rurge check -c <conf>`；再 `rurge rule match -c <conf> example.com --process curl` | 三个平台上输出一样：**没有** `PROCESS-NAME` 的 `W0013`，有 `` `DEVICE-NAME` rules never match `` 一条警告（`0 error(s), 1 warning(s)`）——以盘符开头的路径在 Linux 与 macOS 上同样读作路径，为 Windows 写的 Profile 能在别的系统上被检查。`rule match … --process curl` 命中第一条（`rule match` 不查真实的进程，进程是命令行给的） | 各命令的输出 | | | |
| P18 | **日常使用一天。** `rurge run -c <conf> --system-proxy --log-file <日志路径>`（缺省日志级别），正常使用一天，含睡眠与唤醒、换网络 | 日志里没有 `process lookup took longer than 200 ms`（出现过的话把前后的情形、平台和机器写下来：谁慢了，是表大、进程多，还是机器刚睡醒），没有 `process lookup: the tcp table could not be read`；空闲时 CPU 接近 0（查找只随会话发生，空闲时不读表） | 日志；任务管理器 / `top` | | | |

---

## M3　内存设备上的 TUN 内核

阶段 3 / M6 订正了 W18、M16、L17、P27（AAAA 的期望照「地址族一致」改成空应答；M4 之后的写法）；修正第 1 轮又订正了前置条件（`<服务器>` 的地址要写进 `tun-included-routes`）与 P19、P20、P21（M5 重跑的读数）。会改系统设置的步骤（开增强模式）按「怎么用」的「改系统设置的项与恢复」做。

设计 `docs/superpowers/specs/2026-10-18-phase3-m3-tun-core-design.md`，计划 `docs/superpowers/plans/2026-10-18-phase3-m3a-tun-tcp-plan.md`、`docs/superpowers/plans/2026-10-22-phase3-m3b-udp-icmp-pool-engine-plan.md` 与 `docs/superpowers/plans/2026-10-23-phase3-m3c-dns-responder-prematch-plan.md`（M3a 延后第 2、11、13 行，M3b 延后第 6 行，M3c 延后第 5、12、14 行是这一节的来由），API 与日志行见 `docs/api/phase3.md` 的「M3b」与「M3c」两节，兼容性清单里的差异见 `docs/surge-compatibility-matrix.md` 的 4.1、6.4、9.1 节。

**这一节的每一项都要真实设备上的 TUN（P27 的监听器那一半除外）。**设备层、地址、路由与系统 DNS 是 M4 的内容：阶段 3 / M4 之后三个平台都建得起虚拟网卡（Windows 要 WireGuard LLC 签名的 `wintun.dll` 0.14 或更新、以管理员运行；macOS 与 Linux 要 root），但**没有在真机上验证过**——这正是这一节要做的。M3 的内核只在内存里的设备上测过——“应用”是测试里的另一个 smoltcp 栈，上游是回环上的测试服务器，什么也没有改系统。下面的“操作”按 M4 之后的 `--enhanced-mode` 写。自动化覆盖不了的是：真实的应用（浏览器、`dig`、`nslookup`、带 0x20 的解析器）问应答器会怎样、真实设备上的数字（系统调用的开销不在内存基准里）、真实的发送方在长停顿里怎样探测零窗口、停止时最后一轮的复位会不会被设备丢掉。**一项也没有做过。**编号接着 M2 往下排（W16、M14、L15、P19），整份清单里的编号不重复。

### 前置条件

- M4 的构建：`cargo build --release`，用得到的二进制，以管理员（Windows）或 root（macOS、Linux）运行，开增强模式（`--enhanced-mode`，M4 起）。下文把虚拟网卡写作 `198.18.0.1`，应答器写作 `198.18.0.2`（`198.18.0.2` ～ `198.18.0.9` 与 `fd00:6152::2` 都是应答器的地址，只有 53 端口去应答器）；M4 的实际地址不同时照实改。
- 一份测试用 Profile（下文的 `<conf>`）。每一项要加的规则与键写在对应的节里，改完重载（`curl -X POST -H "X-Key: testkey" http://127.0.0.1:6171/v1/profiles/reload`，Windows 用 `curl.exe`）或重启：

  ```ini
  [General]
  http-listen = 127.0.0.1:6152
  socks5-listen = 127.0.0.1:6153
  http-api = testkey@127.0.0.1:6171

  [Rule]
  FINAL,DIRECT
  ```

- 看记录：`curl -s -H "X-Key: testkey" "http://127.0.0.1:6171/v1/requests/recent?limit=1000"`（M2 一节有 PowerShell 与 `jq` 的写法）；每条看 `listener`（虚拟网卡的会话是 `tun`）、`dst`、`rule`、`status`、`rejectKind`、`error`、`notes`、`pid`、`processPath`。
- 看计数：`GET /v1/features/enhanced_mode`（M4 起）的 `dropped`（十个原因）与 `stackPanics`，形状见设计第 14 节。
- 日志：`rurge run -c <conf> --log-level debug --log-file <日志路径>`；DEBUG 的 `tun: DNS query` 行说应答器对每个查询做了什么（`decision`），`docs/api/phase3.md` 的「M3c」一节有全部的行。
- 内存设备上量过的数字（对照用；一台 Windows 11 x86-64 的开发机，release 构建，不是真实设备）：**B1**（单流吞吐，MTU 1500，`tx_max` 256 KiB，远端在内存里；M3a 时量的，M3c 没有复跑）中位数上行约 7.2、下行约 5.5 Gbit/s，门槛是每个方向不低于 0.7 Gbit/s；**B2**（一万条空闲连接加一百条活跃连接，缺省设置）门槛读数（扣掉测试一侧自己的内存）402.5 MB，对着 429,000,000 字节的线，保守读数 444.1 MB（超线）；**b3**（`Responder::answer` 加造应答包，不含设备、写任务与引擎的规则评估）中位数 0.9 微秒、p99 3.2 微秒、最大 5.8 毫秒，NFR-01 要小于 0.2 毫秒。阶段 3 / M5 重跑的读数（同一台机器、release、`TUN_BENCH_FAR=memory`；一台机器上的一次运行，不是 CI，细节见 `docs/testing.md` 的「阶段 3 / M5 重跑的基准读数」）：**B1** 八个量出来的行都过 0.7 Gbit/s 的线，最低 4.065，中位数上行 8.03、下行 4.84（两个 MTU 混在一起，只作量级）；**B2** 门槛读数 403.7 MB、保守读数 445.3 MB（超 429,000,000 字节的线）；**b3** 中位数 0.9 微秒、p99 2.9 微秒、最大 4542.2 微秒（第 28,673 个查询）。
- 一台局域网里的另一台机器（下文写作 `<服务器>`，下面几个脚本里的服务器在它上面跑，被测机器经 TUN 连它；规则 `FINAL,DIRECT`）。**`<服务器>` 的地址要写进 Profile 的 `[General]` 的 `tun-included-routes = <服务器的地址>/32`**：rurge 装的默认路由比内核给直连网段的路由短，到直连网段的流量本来不进虚拟网卡（M4 设计「执行期的订正」第 31 条，`crates/rurge-platform/src/route/set.rs::RouteSet::compute`）；rurge 自己到 `<服务器>` 的连接照旧走局域网的网卡——出口判定的第 3 条（直连子网）在第 4 条之前（`docs/api/phase3.md` 的「出口判定」）。七个脚本（Python 3；Windows 上写 `python`）：
  - `stall_server.py <端口> <秒数>`：接受一条连接，睡 `<秒数>` 秒不读，再读完并打印收到的字节数。零窗口探测用：

    ```python
    import socket, sys, time

    port = int(sys.argv[1]) if len(sys.argv) > 1 else 9000
    stall = int(sys.argv[2]) if len(sys.argv) > 2 else 300
    server = socket.socket()
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server.bind(("0.0.0.0", port))
    server.listen()
    conn, peer = server.accept()
    print("accepted", peer, "- not reading for", stall, "seconds", flush=True)
    time.sleep(stall)
    total = 0
    while True:
        data = conn.recv(1 << 16)
        if not data:
            break
        total += len(data)
    print("received", total, "bytes", flush=True)
    ```

  - `hold_server.py <端口>`：接受连接、一直拿着不读不写。停止与一万条连接用（每条连接占服务器一个文件描述符：一万条要在服务器上先放宽限制，Linux 与 macOS 用 `ulimit -n 20000`；不够时 `accept` 报 `Too many open files`，脚本退出）：

    ```python
    import socket, sys

    server = socket.socket()
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server.bind(("0.0.0.0", int(sys.argv[1])))
    server.listen(1024)
    held = []
    while True:
        conn, _ = server.accept()
        held.append(conn)
    ```

  - `many_connections.py <服务器> <端口> <条数> [hold]`：开 `<条数>` 条连接；有 `hold` 时拿着 15 分钟，否则等它们结束（你停掉 rurge），打印多少条在多久之内以复位或 EOF 结束（Windows 上 `select` 最多 512 条，条数多时用 `hold`）：

    ```python
    import select, socket, sys, time

    host, port, count = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
    hold = len(sys.argv) > 4 and sys.argv[4] == "hold"
    conns = [socket.create_connection((host, port)) for _ in range(count)]
    print("opened", len(conns), "connections", flush=True)
    if hold:
        print("holding them for 15 minutes", flush=True)
        time.sleep(900)
        sys.exit(0)
    print("stop rurge now", flush=True)
    alive = set(conns)
    ended = []
    deadline = None
    while alive and (deadline is None or time.time() < deadline):
        readable, _, _ = select.select(list(alive), [], [], 1.0)
        for conn in readable:
            try:
                how = "eof" if not conn.recv(4096) else None
            except OSError as error:
                how = type(error).__name__
            if how:
                ended.append((time.time(), how))
                alive.discard(conn)
                if deadline is None:
                    deadline = time.time() + 30
    if ended:
        first = ended[0][0]
        kinds = {}
        for _, how in ended:
            kinds[how] = kinds.get(how, 0) + 1
        print("ended", len(ended), "of", count, "- the last", "%.2f" % (ended[-1][0] - first), "seconds after the first", kinds)
    print("still open", len(alive))
    ```

  - `dns_latency.py <应答器>[:端口] <类型> <次数>`：向应答器发 `<次数>` 个查询、各等应答（2 秒没有应答的算丢了，接着发下一个；迟到的应答按 ID 认出来，不算下一个的），打印收到几个、丢了几个，和收到的往返的中位数、p99 与最大值；类型 1 是 A（每次一个不同的名字），16 是 TXT（`example.com`）：

    ```python
    import socket, struct, sys, time

    server, qtype, count = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])  # qtype: 1 = A, 16 = TXT
    host, _, port = server.partition(":")
    address = (host, int(port or 53))


    def query(ident, name):
        labels = b"".join(bytes([len(part)]) + part.encode() for part in name.split("."))
        return struct.pack(">HHHHHH", ident & 0xFFFF, 0x0100, 1, 0, 0, 0) + labels + b"\x00" + struct.pack(">HH", qtype, 1)


    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.settimeout(2)
    times = []
    lost = 0
    for i in range(count):
        name = "n%d.test" % i if qtype == 1 else "example.com"
        ident = struct.pack(">H", i & 0xFFFF)
        started = time.perf_counter()
        sock.sendto(query(i, name), address)
        while True:
            try:
                answer = sock.recvfrom(4096)[0]
            except socket.timeout:
                lost += 1  # no answer in 2 seconds: lost, and the next query goes out
                break
            if answer[:2] == ident:  # an answer that came late to an earlier query is skipped
                times.append((time.perf_counter() - started) * 1000)
                break
    print("answered %d of %d, lost %d" % (len(times), count, lost))
    if times:
        times.sort()
        print("median %.3f ms, p99 %.3f ms, max %.3f ms" % (times[len(times) // 2], times[int(len(times) * 0.99)], times[-1]))
    ```

  - `burst_queries.py <应答器>[:端口] <条数>`：一个 UDP 套接字不等应答、连发 `<条数>` 个不同 ID 与名字的 A 查询，再收 3 秒应答，打印收到几个；没收到的补问一遍再收 3 秒：

    ```python
    import socket, struct, sys, time

    server, count = sys.argv[1], int(sys.argv[2])
    host, _, port = server.partition(":")
    address = (host, int(port or 53))


    def query(ident, name):
        labels = b"".join(bytes([len(part)]) + part.encode() for part in name.split("."))
        return struct.pack(">HHHHHH", ident, 0x0100, 1, 0, 0, 0) + labels + b"\x00" + struct.pack(">HH", 1, 1)


    def listen(sock, seconds):
        seen = set()
        end = time.time() + seconds
        while time.time() < end:
            try:
                data, _ = sock.recvfrom(4096)
                seen.add(struct.unpack(">H", data[:2])[0])
            except socket.timeout:
                pass
        return seen


    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.settimeout(0.2)
    for i in range(count):
        sock.sendto(query(i, "n%d.test" % i), address)
    seen = listen(sock, 3)
    print("answered at once:", len(seen), "of", count)
    missing = [i for i in range(count) if i not in seen]
    for i in missing:
        sock.sendto(query(i, "n%d.test" % i), address)
    seen |= listen(sock, 3)
    print("answered after asking the rest again:", len(seen), "of", count)
    ```

  - `tcp_pipeline.py <应答器>[:端口] <条数>`：向应答器开一条 TCP 连接，一次写完 `<条数>` 个带两字节长度的 A 查询，再读应答，打印收到几个、ID 是不是按发的次序：

    ```python
    import socket, struct, sys

    server, count = sys.argv[1], int(sys.argv[2])
    host, _, port = server.partition(":")


    def query(ident, name):
        labels = b"".join(bytes([len(part)]) + part.encode() for part in name.split("."))
        message = struct.pack(">HHHHHH", ident, 0x0100, 1, 0, 0, 0) + labels + b"\x00" + struct.pack(">HH", 1, 1)
        return struct.pack(">H", len(message)) + message


    def read_exactly(sock, n):
        data = b""
        while len(data) < n:
            chunk = sock.recv(n - len(data))
            if not chunk:
                raise SystemExit("the connection was closed after %d bytes" % len(data))
            data += chunk
        return data


    sock = socket.create_connection((host, int(port or 53)), timeout=10)
    sock.sendall(b"".join(query(i, "t%d.test" % i) for i in range(count)))
    ids = []
    for _ in range(count):
        length = struct.unpack(">H", read_exactly(sock, 2))[0]
        ids.append(struct.unpack(">H", read_exactly(sock, length)[:2])[0])
    print("answers:", len(ids), "of", count, "- in order:", ids == list(range(count)))
    ```

  - `show_requests.py <端口>`：在 `<服务器>` 上收明文的 HTTP 请求，打印收到的头（看 `Host:`）并答 200：

    ```python
    import socket, sys

    server = socket.socket()
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server.bind(("0.0.0.0", int(sys.argv[1])))
    server.listen()
    while True:
        conn, peer = server.accept()
        print("---", peer, flush=True)
        print(conn.recv(4096).decode("latin-1"), flush=True)
        conn.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        conn.close()
    ```

- 每一步执行后，在对应行填写「结果」（通过 / 不通过，附简要说明）、「日期」与「系统版本」三栏。

---

### Windows

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| W16 | **零窗口探测：Windows 作发送方。**（M3a 延后第 11 行）开增强模式。在 `<服务器>` 上 `python3 stall_server.py 9000 300`；在被测机器上 `python -c "import socket; s=socket.create_connection(('<服务器>',9000)); s.sendall(b'x'*(200<<20)); print('sent')"`，同时在虚拟网卡上抓包（Wireshark）。服务器睡着的 5 分钟里不要动，之后等服务器打印收到的字节数 | 上传在服务器睡着时停住（rurge 的接收缓冲满了，窗口为 0），**连接不被重置**：服务器醒来读完之后，上传继续并完成，服务器打印的字节数是 209,715,200，客户端打印 `sent`。记录里这条连接一直是 `active`。抓包里记下 Windows 发零窗口探测的间隔（重传计时器的指数退避）和 rurge 是不是每次都回了 ACK（smoltcp 对越过窗口边缘的段每个套接字每秒至多回一个 ACK）。`dropped` 里 `beyond_window` 很小，`rst_limited` 与 `stackPanics` 为 0。没有观察过：真实的 Windows 发送方会不会因为得不到足够的回答而重置连接——重置了就把间隔、持续多久与报错写下来 | `stall_server.py` 的输出；Wireshark；`/v1/requests/active`；计数 | | | |
| W17 | **停止时最后一轮的复位与 `STOP_GRACE`。**（M3a 延后第 13 行）开增强模式。在 `<服务器>` 上 `python3 hold_server.py 7000`；在被测机器上 `python many_connections.py <服务器> 7000 500`，打印 `stop rurge now` 之后在另一个窗口 `rurge.exe stop -c <conf>`（或在 rurge 的窗口按 Ctrl-C），同时抓虚拟网卡 | 脚本打印 `ended 500 of 500`，最后一条在第一条之后约 1 秒之内（`STOP_GRACE` 是 1 秒），种类是 `ConnectionResetError`；**没有一条挂到应用自己的 TCP 超时才断**（`still open 0`）。少了几条、或最后一条晚了多久，写下来——最后一轮的复位在真实设备满了时会被丢，应用的 TCP 就要等自己的超时。抓包里看这些复位是不是都发出去了 | 脚本的输出；Wireshark | | | |
| W18 | **Windows 的应用问应答器。**开增强模式，系统 DNS 指向应答器（M4）。(a) `nslookup example.com 198.18.0.2`、`nslookup -type=AAAA example.com 198.18.0.2`、`nslookup -type=TXT example.com 198.18.0.2`、`nslookup -type=HTTPS example.com 198.18.0.2`、`nslookup use-application-dns.net 198.18.0.2`；(b) `nslookup -vc example.com 198.18.0.2`（经 TCP）和 `Resolve-DnsName example.com -Server 198.18.0.2 -DnsOnly -TcpOnly`；(c) Edge、Chrome、Firefox 各打开 `http://example.com/`，读 `recent` | (a) A 得 `198.18.x.y`（TTL 30）；AAAA 往 `198.18.0.2` 这个 IPv4 的应答器问是 NOERROR、没有记录（地址族一致；要看 `fd00:6152::` 开头的地址，往 `fd00:6152::2` 问 AAAA，见 P33）；TXT 得真实的 TXT（转发），HTTPS 得 NOTIMP（`allow-dns-svcb` 缺省关着），canary 得 NXDOMAIN；(b) 经 TCP 的结果相同；(c) 页面能打开，`recent` 里 `dst` 是名字（`example.com:80`）、`listener` 是 `tun`、`pid` 是浏览器的进程。把每个浏览器的 DNS 请求是不是真的发给了应答器写下来（Chrome 的"安全 DNS"、Edge 的 DNS over HTTPS 会绕开它，应当在 DEBUG 日志里看不到它们的 `tun: DNS query`） | 命令的输出；`recent`；DEBUG 日志 | | | |

---

### macOS

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| M14 | **零窗口探测：macOS 作发送方。**（M3a 延后第 11 行）同 W16：在 `<服务器>` 上 `python3 stall_server.py 9000 300`；在被测机器上 `python3 -c "import socket; s=socket.create_connection(('<服务器>',9000)); s.sendall(b'x'*(200<<20)); print('sent')"`，同时在 utun 上抓包（`sudo tcpdump -i utunN -n`，`N` 看 M4 建的是哪一个） | 同 W16：上传停住、连接不被重置、服务器醒来之后收到 209,715,200 字节；记下 macOS 的探测间隔与 rurge 的 ACK；`dropped` 里 `beyond_window` 很小，`rst_limited` 与 `stackPanics` 为 0 | `stall_server.py` 的输出；`tcpdump`；计数 | | | |
| M15 | **停止时最后一轮的复位与 `STOP_GRACE`。**（M3a 延后第 13 行）同 W17：`python3 hold_server.py 7000` 在 `<服务器>`，被测机器 `ulimit -n 4096; python3 many_connections.py <服务器> 7000 500`（macOS 缺省只许开 256 个文件），打印 `stop rurge now` 之后 `./rurge stop -c <conf>` | 同 W17：500 条约 1 秒之内都以复位结束，没有挂到应用自己的 TCP 超时的；macOS 上 utun 收不下时最后一轮的复位会不会被丢（设计 4.1 节：内存设备会等，真实设备收不下就丢）——少了几条写下来 | 脚本的输出；`tcpdump` | | | |
| M16 | **macOS 的应用问应答器。**开增强模式，系统 DNS 指向应答器（M4）。(a) `dig @198.18.0.2 example.com`、`dig @198.18.0.2 example.com AAAA`、`dig @198.18.0.2 example.com TXT`、`dig @198.18.0.2 example.com HTTPS`、`dig @198.18.0.2 use-application-dns.net`；(b) `dig +tcp @198.18.0.2 example.com`；(c) `dscacheutil -q host -a name example.com` 和 `ping -c 3 example.com`（系统解析库经 mDNSResponder 问应答器）；(d) Safari、Chrome、Firefox 各打开 `http://example.com/`，读 `recent`；`scutil --dns` 看系统的解析器 | (a) 同 W18(a)：A 得 `198.18.x.y`（TTL 30，`aa` 与 `ra` 置位）；AAAA 往 `198.18.0.2` 问是 NOERROR、没有记录（要看 `fd00:6152::` 开头的地址，往 `fd00:6152::2` 问 AAAA，见 P33）；TXT 得真实的 TXT，HTTPS 得 NOTIMP，canary 得 NXDOMAIN；(b) 经 TCP 的结果相同；(c) 解析出 fake 地址，`ping` 的回显发往名字背后的真实地址并拿到应答（`icmp-forwarding` 缺省开着；真实的 ICMP 套接字是 M4 的）；(d) 页面能打开，`recent` 里 `dst` 是名字、`listener` 是 `tun`。`.local` 名字（mDNS）的行为单独记下：A / AAAA 拿 fake 地址还是被系统直接用 mDNS 解析 | `dig` 的输出；`recent`；`scutil --dns` | | | |

---

### Linux

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| L15 | **零窗口探测：Linux 作发送方。**（M3a 延后第 11 行）同 W16：在 `<服务器>` 上 `python3 stall_server.py 9000 300`；在被测机器上 `python3 -c "import socket; s=socket.create_connection(('<服务器>',9000)); s.sendall(b'x'*(200<<20)); print('sent')"`，同时在 TUN 设备上抓包（`sudo tcpdump -i <设备名> -n`） | 同 W16：上传停住、连接不被重置、服务器醒来之后收到 209,715,200 字节；Linux 的持续计时器（persist timer）的探测间隔与 rurge 的 ACK 写下来；`dropped` 里 `beyond_window` 很小，`rst_limited` 与 `stackPanics` 为 0 | `stall_server.py` 的输出；`tcpdump`；计数 | | | |
| L16 | **停止时最后一轮的复位与 `STOP_GRACE`。**（M3a 延后第 13 行）同 W17：`python3 hold_server.py 7000` 在 `<服务器>`，被测机器 `ulimit -n 4096; python3 many_connections.py <服务器> 7000 500`，打印 `stop rurge now` 之后 `./rurge stop -c <conf>` | 同 W17：500 条约 1 秒之内都以复位结束；少了几条、最后一条晚了多久写下来（TUN 设备的发送队列满时最后一轮的复位会不会被丢） | 脚本的输出；`tcpdump` | | | |
| L17 | **Linux 的应用问应答器。**开增强模式，系统 DNS 指向应答器（M4）。(a) `dig @198.18.0.2 example.com`、`dig @198.18.0.2 example.com AAAA`、`dig @198.18.0.2 example.com TXT`、`dig @198.18.0.2 example.com HTTPS`、`dig @198.18.0.2 use-application-dns.net`；(b) `dig +tcp @198.18.0.2 example.com`；(c) `getent hosts example.com`、`resolvectl query example.com`（systemd-resolved 的服务器指向应答器时）和 `host -t TXT example.com 198.18.0.2`；(d) Firefox、Chromium 各打开 `http://example.com/`，读 `recent` | (a)(b) 同 M16（AAAA 往 `198.18.0.2` 问是 NOERROR、没有记录；要看 `fd00:6152::` 的地址往 `fd00:6152::2` 问，见 P33）；(c) 解析出 fake 地址；systemd-resolved 对 SVCB / HTTPS 的 NOTIMP 的反应（会不会因此降级到别的服务器）写下来；(d) 页面能打开，`recent` 里 `dst` 是名字、`listener` 是 `tun`、`pid` 是浏览器的进程（Snap 或 Flatpak 的见 M2 的 L14） | `dig` 的输出；`recent` | | | |

---

### 与平台无关

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| P19 | **B1：真实设备上的单流吞吐。**（M3a 延后第 2 行）开增强模式，MTU 取 M4 给的（M3 的缺省 1500）。在 `<服务器>` 上 `iperf3 -s`；在被测机器上 `iperf3 -c <服务器> -t 30`（上行）和 `iperf3 -c <服务器> -t 30 -R`（下行），三个平台各做一次；同时看 rurge 的 CPU（任务管理器 / `top`） | 记下每个平台每个方向的 Gbit/s、rurge 占几个核、`dropped` 里 `beyond_window`、`queue_full`、`rst_limited` 与 `stackPanics`（都应当是 0）。对照：内存设备上 B1（M3a 时量的）的中位数上行约 7.2、下行约 5.5 Gbit/s，门槛是每个方向不低于 0.7 Gbit/s（rurge 一侧单线程，M3-D9）。真实设备上低多少、是系统调用、转发循环的 8 KiB 缓冲还是别的拖的，写下来 | `iperf3` 的输出；任务管理器 / `top`；计数 | | | |
| P20 | **B2：真实设备上的内存。**（M3a 延后第 2 行）开增强模式。在 `<服务器>` 上（Linux 与 macOS 先 `ulimit -n 20000`，一万条连接要一万多个文件描述符）`python3 hold_server.py 7000`；在被测机器上（Linux 与 macOS 同样先 `ulimit -n 20000`）`python3 many_connections.py <服务器> 7000 10000 hold`，再在 `<服务器>` 上 `iperf3 -s`、被测机器上 `iperf3 -c <服务器> -P 100 -t 60`（一百条在传的连接）；读 rurge 的峰值常驻内存（Windows：`(Get-Process rurge).PeakWorkingSet64`；Linux：`grep VmHWM /proc/<pid>/status`；macOS：`/usr/bin/time -l ./rurge …` 的 `maximum resident set size`，或活动监视器） | 峰值不超过 429,000,000 字节（NFR-01 的 300 MB 除以 0.7）。内存设备上缺省设置的门槛读数是 402.5 MB、保守读数 444.1 MB（超线）；真实设备上 rurge 一侧没有测试的东西，读到的峰值就是 rurge 自己的，记下它。再记下：预算（64 MiB）用完之后建的连接一辈子只有 4 KiB 的缓冲，这样的一条高吞吐连接多快（M3a 延后第 6 行：跑 `iperf3 -c <服务器> -t 30` 看，此时一万条已开着） | 内存读数；`iperf3` 的输出 | | | |
| P21 | **fake 应答的延迟与转发的往返。**（M3c 延后第 5、12 行）开增强模式。在被测机器上 `python3 dns_latency.py 198.18.0.2 1 100000`（十万个不同的名字，A 查询，fake 应答）和 `python3 dns_latency.py 198.18.0.2 16 1000`（TXT，转发到上游），三个平台各做一次 | A 查询的往返：记下丢了几个（脚本打印 `answered … lost …`）、中位数、p99、最大。NFR-01 要小于 0.2 毫秒；内存里 `Responder::answer` 加造应答包是中位数 0.9 微秒、p99 3.2 微秒（不含设备、写任务与引擎的规则评估），真实设备上多出来的是设备的往返。最大值落在池的 `HashMap` 翻倍处附近（M3c 时是第 57,345 个名字，阶段 3 / M5 重跑时是第 28,673 个；读代码得出的解释，内存里量到 5.8 毫秒，M3c 延后第 12 行）；TXT 的往返是上游的往返加一点。怎么量（进程常驻内存）、由谁、在哪台机器上量，是总设计第 16 节 Q7，归项目所有者 | 脚本的输出 | | | |
| P22 | **一阵上千个查询。**（M3c 延后第 14 行）开增强模式。(a) `python3 burst_queries.py 198.18.0.2 1000`；(b) 同时开五个，各 `python3 burst_queries.py 198.18.0.2 2000`；(c) 一条 TCP 连接上流水线 200 个查询：`python3 tcp_pipeline.py 198.18.0.2 200` | (a)(b) 第一次收到几个、补问之后几个，`dropped.queue_full` 涨了多少：应答器的队列（1,024）与写任务的队列（缺省 256）放不下的应答被丢，应用约 1 秒后重问，所以补问之后应当全部收到；记下第一次丢了多少，丢得多就是真实设备比内存设备慢；(c) 脚本打印 `answers: 200 of 200 - in order: True`（应答器在本地按次序一个接一个答，不并发）；之后 rurge 的 CPU 回到接近 0，(c) 没有新的 WARN；(a)(b) 要是丢了应答，日志里有且只有一行 WARN `tun: a packet was dropped (reason: queue_full); …`（每种丢包原因只在第一次说一次 WARN，其余在 DEBUG） | 脚本的输出；计数；日志 | | | |
| P23 | **带 0x20 的解析器与 canary。**开增强模式。(a) 把 Unbound 的上游指到应答器（`forward-zone: name: "." forward-addr: 198.18.0.2`，`use-caps-for-id: yes`），解析 `ExAmPle.test`、`example.com` 几次；(b) 缺省打开 DNS over HTTPS 的 Firefox，系统 DNS 指向应答器（M4），打开 `http://example.com/`，看 `about:networking#dns` 或 `about:config` 的 `network.trr.mode` | (a) Unbound 不报 0x20 失败，解析成功：应答的问题段逐字节回显它发的大小写；同一个名字不论大小写拿同一个 fake 地址。(b) 应答器对 `use-application-dns.net` 答 NXDOMAIN，Firefox 不用 DoH，它的 DNS 查询出现在 DEBUG 的 `tun: DNS query` 行里 | Unbound 的日志；DEBUG 日志；Firefox 的页面 | | | |
| P24 | **`hijack-dns`。**开增强模式，`[General]` 里 `hijack-dns = 8.8.8.8:53, 1.1.1.1:53`（重载）。(a) `dig @8.8.8.8 example.com`；(b) `dig +tcp @1.1.1.1 example.com`；(c) 向 `8.8.8.8:53` 发一个不是 DNS 查询的 UDP 包（`printf 'garbage' \| nc -u -w1 8.8.8.8 53`）；(d) `dig @2001:4860:4860::8888 example.com`；(e) 把 `hijack-dns` 换成 `*:53` 再试 (a)，然后去掉这个键、重载，再试 (a) | (a) 得到 fake 地址，`dig` 不抱怨"reply from unexpected source"（应答的来源是 `8.8.8.8:53`）；(b) 经 TCP 的不被劫持：真的连 `1.1.1.1:53`（经规则与策略，`recent` 里 `dst` 是 `1.1.1.1:53`）；(c) 照常作 UDP 会话（`recent` 里有 `transport` 为 `udp` 的会话）；(d) IPv6 的从不劫持；(e) `*:53` 劫持任何 IPv4 的 DNS 查询；去掉之后，下一个包就不再被劫持（发往 `8.8.8.8` 的真的出去了） | `dig` 的输出；`recent` | | | |
| P25 | **预匹配在真实应用上。**开增强模式，`[Rule]` 里加（写在 `FINAL,DIRECT` 之前）：`DOMAIN-SUFFIX,ads.test,REJECT,pre-matching`、`DOMAIN-SUFFIX,drop.test,REJECT-DROP,pre-matching`、`DOMAIN-SUFFIX,sink.test,REJECT-NO-DROP,pre-matching`、`AND,((DOMAIN-SUFFIX,port.test),(DEST-PORT,443)),REJECT,pre-matching`；`[Host]` 里加 `x.port.test = <服务器的地址>`（(d) 里 8080 的那一条要靠它解析出真实地址，`.test` 不会自己解析）；重载。(a) 对 `x.ads.test`、`x.drop.test`、`x.sink.test`、`x.port.test` 各 `dig @198.18.0.2` 一次（A 与 AAAA 都问）；(b) 浏览器打开 `http://x.ads.test/`；(c) `curl http://198.18.0.244/`（`dig` 到的 `x.sink.test` 的地址）；(d) 对 `x.port.test` 的 fake 地址 `curl https://x.port.test/ -k` 和 `curl http://x.port.test:8080/`（`<服务器>` 上起一个 8080 的服务，`[Host]` 把 `x.port.test` 指到它，规则让它走 DIRECT）；(e) 在 30 秒内对 `x.ads.test` 连问 60 次 | (a) `ads.test`：空应答（NOERROR、没有记录）；`drop.test`：没有应答（`dig` 超时重试）；`sink.test`：A 得 `198.18.0.244`，AAAA 得空应答；`port.test`：DNS 层不拒绝，得 fake 地址；(b) 浏览器得不到地址；(c) 连接被拒（RST）；(d) 443 的连接被 RST，8080 的连得上；(e) 第 50 次起不再有应答（自动升级，第 50 次本身就静默）。`recent` 里：每条规则 5 分钟只有一条记录，`notes` 含 `pre-matching at DNS` 或 `pre-matching at SYN`，DNS 层记录的 `dst` 是名字、没有 `:0`；INFO 日志里 `session rejected by REJECT` 一行带名字（设计要名字只在 DEBUG，这一处不照做，M3c 延后第 6 行） | `dig` 的输出；`recent`；日志 | | | |
| P26 | **经监听器来的 fake 地址。**开增强模式，`http-listen = 127.0.0.1:6152`、`socks5-listen = 127.0.0.1:6153`，系统代理不指向 rurge；`<conf>` 的 `[Host]` 里加 `origin.test = <服务器的地址>`，`<服务器>` 上 `python3 show_requests.py 8080`。先拿一个 fake 地址：`A=$(dig +short @198.18.0.2 origin.test)`（PowerShell：`$A=(Resolve-DnsName origin.test -Server 198.18.0.2 -DnsOnly -Type A).IPAddress`），再：(a) `curl -p -x http://127.0.0.1:6152 http://$A:8080/ -I`（`-p`：用 CONNECT）；(b) `curl --socks5 127.0.0.1:6153 http://$A:8080/ -I`；(c) `curl -x http://127.0.0.1:6152 http://$A:8080/ -I`（明文，绝对形式）；(d) 对 `198.18.0.1`、`198.18.0.244` 和池不认识的 `198.19.200.1` 重复 (a)；(e) 不开增强模式（没有池）重复 (a) | (a)(b) 连得上，`show_requests.py` 收到请求，`recent` 里 `dst` 是名字（`origin.test:8080`），不是 fake 地址；(c) 连得上，但**已知的缺口**（阶段 4）：请求行与 `Host` 仍是 fake 地址，`show_requests.py` 打印的 `Host:` 是 `$A:8080`，把实际看到的写下来；(d) HTTP 答 502，SOCKS5 答 `0x05`（把 `-p -x` 换成 `--socks5` 试一遍），`recent` 里 `error` 是 `fake address not known`，没有拨号；(e) 照普通地址处理（连不上），没有这条错误 | `curl` 的输出；`show_requests.py` 的输出；`recent` | | | |
| P27 | **`local` 与进程查找：监听器与虚拟网卡各一种。**（M3b 延后第 6 行）`[General]` 里 `http-listen = 0.0.0.0:6152`（同时 `[::]:6152`，在支持处），`--log-level debug`。(a) 不需要 TUN：本机的客户端经本机的局域网地址连：`curl -x http://<本机的局域网地址>:6152 http://example.com/ -I`；(b) 开增强模式（要 TUN），同一个 `curl` 不经代理端口、直接连：`curl http://example.com/ -I`（经虚拟网卡）；读 DEBUG 的 `process lookup found the process` 行 | (a) `remote` 是 `<本机的局域网地址>:6152`——被接受的那条连接自己的本地地址，不是绑定的 `0.0.0.0:6152`——，找得到进程，`pid` 是 `curl`；(b) `remote` 是应用连的端点（fake 地址与 80），找得到进程，`pid` 是 `curl`。这一项的监听器那一半不需要 TUN，可以先做；虚拟网卡那一半要 M4 之后的增强模式 | DEBUG 日志；`recent` | | | |
| P28 | **计数与日志行。**（设计第 16 节）做完上面的项之后：(a) 读 `GET /v1/features/enhanced_mode`；(b) 缺省日志级别（INFO）下搜日志里的名字（`example.com`、`ads.test`）、上游的地址（`8.8.8.8`）和 fake 地址；(c) `--log-level debug` 看 `tun: DNS query` 行的 `decision` | (a) `dropped` 的十个原因里不为 0 的，各自能对上做过的事（`unknown_fake`：重启之后应用还在用的旧 fake 地址；`martian`：真实网卡上的组播与广播，从不写日志；`queue_full`：P22 里丢的应答；`rst_limited`：一阵被拒的 SYN）；`stackPanics` 是 0。(b) INFO 里带名字或地址的只有引擎写的两种会话行：`session rejected by …`（被拒的会话：P25 的预匹配拒绝，和你自己试出的别的拒绝）与 `session failed`（失败的会话：P26(d) 的 `fake address not known`，W18 / M16 / L17 里连不上的连接），都带 `src` 与 `dst`（阶段 1 起的写法，`docs/api/phase3.md` 的「M3b」与「M3c」两节都说了）；除了这两种行，TUN 与应答器方面的 INFO 与 WARN 里没有名字与地址（别的方面的 INFO 行可以带地址：被忽略的 IPv6 DNS 服务器那一行 `IPv6 DNS server ignored because ipv6 is off` 的 `server`，`http-api listening` 的监听地址；这里要搜的名字、`8.8.8.8` 与 fake 地址在这些行里找不到）；WARN 只有每种丢包原因第一次的一行（不带地址，`martian` 除外）、池耗尽、设备出错这几句。(c) `decision` 的取值见 `docs/api/phase3.md` 的「M3c」一节 | `/v1/features/enhanced_mode`；日志文件 | | | |

---

## M4　增强模式（平台接入）

阶段 3 / M6 订正了 W21、W23、M21、L23、M30（`rurge dns lookup` 是另一个进程，不在增强模式里；W21 的 `--trace` 期望照代码改，W23 / M21 / L23 的 (b) 改成让运行中的 rurge 向上游问一次，`server:system` 的三项改看运行中的 `rurge run`）。会改系统设置的步骤按「怎么用」的「改系统设置的项与恢复」做。

设计 `docs/superpowers/specs/2026-10-26-phase3-m4-platform-design.md`（先读它第 21 节与末尾「执行期的订正」一节），计划 `docs/superpowers/plans/2026-10-26-phase3-m4a-skeleton-plan.md`、`…-m4b-loop-dns-plan.md`、`…-m4c-linux-macos-plan.md` 与 `…-m4d-windows-wrapup-plan.md`（末尾两张表是这一节的来由：M4b 延后第 9、10、15 行，M4c 延后第 1、2、5 ～ 10 行，M4d 计划的任务 9）；API 与日志行见 `docs/api/phase3.md` 的「M4」、「M4b」、「M4c」与「M4d」四节，兼容性清单里的差异见 `docs/surge-compatibility-matrix.md` 的 2.1、6.3、9.1、9.4、10.4 节。

自动化验不了的是真机上的这一整套：真的虚拟网卡、真的地址与路由、真的系统 DNS、真的权限、真的 ICMP、真的睡眠与换网络。三个平台的设备与系统改动在一台 Windows 11 x86-64 的开发机上都没有跑过（Windows 的设备一段也没有运行过；Linux 与 macOS 的设备与挑平台的代码那台机器编不到。CI 的运行 40611702392 在三个平台上编译了 M4 的代码、缺省用例通过，但缺省的用例不建网卡、不改系统设置）。**一项也没有做过。**

**会改系统设置的步骤（开增强模式、改路由与系统 DNS、改 `rp_filter`、强杀进程）在一次性的 Windows / macOS / Linux 机器或虚拟机上做。**每一项记下日期、系统版本与结果；连原始输出（`netsh` / `ip` / `networksetup` / `scutil` / `resolvectl`）一起记下的那几项，是为核对代码里的读回与解析。做完怎样恢复见「怎么用」的「改系统设置的项与恢复」。

编号接着 M3 往下排（W19、M17、L18、P29），整份清单里的编号不重复。另外：M3 一节里标着“开增强模式（M4）”的那些行（W16 ～ W18、M14 ～ M16、L15 ～ L17、P19 ～ P28）现在可以做；W18、M16、L17、P27 由阶段 3 / M6 就地订正过（AAAA 的期望与 M4 之后的写法），其余不改。

### 前置条件

- `cargo build --release`，用得到的二进制（Windows 下是 `target\release\rurge.exe`，其余是 `target/release/rurge`）。下文按平台写作 `rurge.exe`（Windows）或 `./rurge`。
- 要 root / 管理员：Windows 上以管理员打开命令行（否则见 W37），Linux 与 macOS 上 `sudo`。
- 一份测试用 Profile（下文的 `<conf>`）。`1.1.1.1` 换成你的网络够得到的 DNS 服务器；Wi-Fi 的名字用 `<A>`：

  ```ini
  [General]
  http-listen = 127.0.0.1:6152
  http-api = testkey@127.0.0.1:6171
  dns-server = 1.1.1.1
  ipv6-vif = auto

  [Rule]
  FINAL,DIRECT
  ```

  每一项要加的行写在它自己的操作里，改完重启（增强模式的打开只看启动参数与 `POST`，不跨重启保留）。
- 日志：`rurge run -c <conf> --enhanced-mode --log-level debug --log-file <日志路径>`。下文「日志」都指这里；`enhanced mode on` / `enhanced mode off`、`takeover paused on this network` 是 INFO，网卡名、地址、前缀、DNS 服务器与命令行的全文是 DEBUG。
- API（Windows 用 `curl.exe`）：
  - 状态与计数：`curl -s -H "X-Key: testkey" http://127.0.0.1:6171/v1/features/enhanced_mode`
  - 开：`curl -s -X POST -H "X-Key: testkey" -d "{\"enabled\":true}" http://127.0.0.1:6171/v1/features/enhanced_mode`；关把 `true` 换成 `false`
- 看虚拟网卡：
  - Windows：`Get-NetAdapter -Name rurge | Format-List Name,InterfaceIndex,Status,LinkSpeed`；序号也可以从 `netsh interface ipv4 show interfaces` 的 `Idx` 一列读。
  - macOS：`ifconfig -l`（找一个 `utunN`），`ifconfig utunN`（`N` 从日志的 DEBUG 行 `the enhanced mode's adapter` 里读，字段 `adapter`）。
  - Linux：`ip addr show rurge`。
- 每一步执行后，在对应行填写「结果」「日期」「系统版本」三栏。

---

### Windows

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| W19 | **开与关：命令行与 API。**（设计第 16 节）管理员下 `rurge.exe run -c <conf> --enhanced-mode --log-level debug`；记下标准输出；`GET /v1/features/enhanced_mode`；`POST {"enabled":false}`；再 `GET`；再 `POST {"enabled":true}`；最后 Ctrl-C 退出。做之前先把 `wintun.dll` 放到 `rurge.exe` 旁边（W35、W36） | 打开成功：标准输出里有 `enhanced mode on`，之后是 `rurge … running: …`；`GET` 是 `{"enabled":true,…}`，`dropped` 的十个数大多是 0、`stackPanics` 是 0；名字叫 `rurge` 的网卡在，地址 `198.18.0.1`、MTU 1500。`POST false` 回 `{}`，日志 INFO `enhanced mode off` 与 `enhanced mode switched via http-api`，网卡从系统里消失；`GET` 是 `{"enabled":false,…}`，十个数都是 0。`POST true` 再开一次，日志里同样两行 INFO。Ctrl-C 退出时先关增强模式、再关系统代理（没开就跳过），退出码 0 | 标准输出；`GET` 的 JSON；`Get-NetAdapter -Name rurge`；日志 | | | |
| W20 | **路由表与网卡的样子。** 增强模式开着时读：`netsh interface ipv4 show route`、`netsh interface ipv6 show route`、`netsh interface ipv4 show addresses name=<序号>`、`netsh interface ipv6 show addresses interface=<序号>`、`netsh interface ipv4 show subinterfaces interface=<序号>`、`netsh interface ipv4 show interfaces` | IPv4 的路由表里 `Idx` 是 `rurge` 网卡的序号、`Type` 是 `Manual`、`Met` 是 1 的行**正好是** 27 条：`1.0.0.0/8`、`2.0.0.0/7`、`4.0.0.0/6`、`8.0.0.0/5`、`16.0.0.0/4`、`32.0.0.0/3`、`64.0.0.0/3`、`96.0.0.0/4`、`112.0.0.0/5`、`120.0.0.0/6`、`124.0.0.0/7`、`126.0.0.0/8`、`128.0.0.0/3`、`160.0.0.0/5`、`168.0.0.0/8`、`169.0.0.0/9`、`169.128.0.0/10`、`169.192.0.0/11`、`169.224.0.0/12`、`169.240.0.0/13`、`169.248.0.0/14`、`169.252.0.0/15`、`169.255.0.0/16`、`170.0.0.0/7`、`172.0.0.0/6`、`176.0.0.0/4`、`192.0.0.0/3`；没有一条落在 `240.0.0.0/4`（Windows 不往这一段发包，rurge 也不装它）。系统随网卡与地址自己加的那几条（`Type` 是 `System`，例如 `198.18.0.0/15`、`198.18.0.1/32`、`198.19.255.255/32`、`224.0.0.0/4`、`255.255.255.255/32`）不是 rurge 装的、不算。`ipv6-vif = auto` 而这个网络有 IPv6 时，`Manual` 的是 16 条：`64:ff9b::/96`、`100::/8`、`200::/7`、`400::/6`、`800::/5`、`1000::/4`、`2000::/3`、`4000::/2`、`8000::/2`、`c000::/3`、`e000::/4`、`f000::/5`、`f800::/6`、`fc00::/7`、`fe00::/9`、`fec0::/10`；没有 IPv6 的网络里一条也没有（W28）。`show addresses` 有 `198.18.0.1`，IPv6 开着时它的 IPv6 那一份有 `fd00:6152::1`，`show subinterfaces` 的第一个数是 1500，`show interfaces` 里 `rurge` 的 `Met` 是 1（W31）、`State` 是 connected。记下缺省的 MTU——设之前是多少（要是看得见的话）。**记下 `Get-NetAdapter` 里网卡的名字**——是 `rurge`；是 `rurge 1` 一类时照实记下（R25） | 四条 `netsh` 的输出（抄进验收记录） | | | |
| W21 | **系统 DNS 的接管与交还。** 开着增强模式：`netsh interface ipv4 show dnsservers name=<序号>`、`netsh interface ipv6 show dnsservers name=<序号>`、`nslookup example.com`、`nslookup example.com 198.18.0.2`；然后关掉增强模式，再执行一次 `nslookup example.com`、看物理网卡的 `show dnsservers` | 开着时：`rurge` 网卡的 DNS 列表只有 `198.18.0.2`（IPv6 有的话还有 `fd00:6152::2`），`nslookup` 拿到 `198.18.x.y` 这样的 fake 地址；`ipconfig /flushdns` 在打开时已经跑过一次（日志 DEBUG 里有它的命令行）。关掉之后：网卡没了，物理网卡的 DNS 列表与打开前一模一样（rurge 不改它），`nslookup` 重新拿到真地址。注：Windows 上只改虚拟网卡那一份 DNS 设置，物理网卡上手动设过的服务器可能仍被直接问到（兼容性清单 9.1 节登记的限制，也要在这里核对）。`rurge dns lookup` 是另一个进程，不知道增强模式开着：它的系统服务器表是各网卡的 DNS 去掉 rurge 的地址空间（`crates/rurge-platform/src/dns.rs::servers` → `without_rurge_space`），`198.18.0.2` 不在里面，所以 `--trace` 里问的是物理网卡的服务器，不是应答器；这一项不看它 | `netsh` 的输出；`nslookup`；DEBUG 日志 | | | |
| W22 | **强行结束后再起的恢复（`taskkill /F`）。** 开着增强模式，另开一个窗口 `taskkill /F /IM rurge.exe`；`Get-NetAdapter -Name rurge`；再启动一次（不带 `--enhanced-mode`），看日志与标准错误；再带 `--enhanced-mode` 启一次 | 强杀之后：网卡随进程消失（`Get-NetAdapter` 找不到），路由随之消失，系统 DNS 回到物理网卡；`<数据目录>\state.json` 的 `enhanced_backup` 本来就是 `null`（Windows 没有会留下的改动），`features.enhanced_mode` 可能还是 `true`。不带 `--enhanced-mode` 的下一次启动把 `features.enhanced_mode` 写回 `false`、**不记日志**（`EnhancedMode::recover` 回 `None`，`run.rs` 对 `None` 什么也不说；M4a 记录第 99 行），`restored what the enhanced mode left last time` 那一行不出现（它只在备份里有东西时出现）；带 `--enhanced-mode` 的那一次**不去报“正在被使用”**，它重新建网卡、再打开（P8 的前提：Wintun 的网卡随建它的进程消失；这是读源码与邮件列表得出的**推论**，这一项就是它的证据） | `Get-NetAdapter`；`state.json`；标准错误；日志。把两次启动的间隔与结果记下来 | | | |
| W23 | **防环：rurge 自己的连接不进虚拟网卡。** 开着增强模式、`--log-level debug`。让 rurge 自己做三类连接：(a) 经一个以域名配置的代理策略拨号（`curl -x http://127.0.0.1:6152 https://example.com/ -I`，Profile 里让它走你的代理，`server` 写域名）；(b) 让运行中的 rurge 向上游问一次：`nslookup -type=TXT example.com 198.18.0.2`（应答器把 TXT 转发给上游，`crates/rurge-tun/src/dns/mod.rs` 的 `forward`）；(c) 一次资源下载（让一个规则集或订阅更新一次，或 `POST /v1/external_resources/update`）。每做一项之前之后读 `GET /v1/features/enhanced_mode` 的计数，并用 Wireshark 在虚拟网卡上抓包 | 三类都通；**虚拟网卡的计数不因 rurge 自己的流量涨**（`dropped` 十个数与 `stackPanics` 都不变，网卡上抓不到 rurge 自己的包）；DEBUG 里有 `rurge's own connection leaves through the egress interface`（字段 `dst` 与 `interface`，`interface` 是物理网卡），没有 `cannot bind to the network outside the enhanced mode`。代理的域名解析走的是物理网卡；(b) 拿到 TXT，DEBUG 里 target 为 `rurge_dns::fanout` 的 `forward send` 与 `forward answer` 两行（原文在 `crates/rurge-dns/src/fanout.rs`），虚拟网卡上只看得到应用发给 `198.18.0.2` 的查询与回答，看不到 rurge 发给上游的包，DEBUG 里 `rurge's own connection leaves through the egress interface` 那一行的 `dst` 是上游的地址（这一行在建上游套接字时出现，套接字留着复用，可能在更早的地方出现过一次） | 计数（前后各一份）；Wireshark 在 `rurge` 网卡上；DEBUG 日志 | | | |
| W24 | **睡眠唤醒与换网络。** 开着增强模式：(a) 让机器睡眠 15 秒以上、换到另一个网络唤醒（Wi-Fi 换 `<B>`，或插拔网线）；(b) 唤醒到原来的网络；(c) 换网络的中途（拔线再插、Wi-Fi 重连） | (a) 联网后 30 秒内路由与系统 DNS 与新的网络一致：`netsh interface ipv4 show route` 里 W20 那 27 条仍在，`show dnsservers` 仍是应答器，应用（浏览器、`ping`、`curl`）经虚拟网卡照常工作；`network changed` 之后没有 `cannot follow the change; the enhanced mode goes on with what it has` 的 ERROR。(b) 同一个网络时没有多余的接管与交还。(c) 读不到网络的瞬间不算变化（M1 一节 P6 的期望），网卡与路由不动。记下恢复用了多久（阶段 3 的验收标准：30 秒内） | 日志；`netsh` 的输出；应用的成败 | | | |
| W25 | **接管暂停：增强模式的那一半。** 在 `<conf>` 的 `[SSID Setting]` 里给当前网络写 `suspend=true`（Wi-Fi 写 `SSID:<A> suspend=true`，有线写 `TYPE:WIRED suspend=true`）再重启到那个网络上；运行中 `POST {"enabled":true}` 一次；再把那一行改成别的网络的（或去掉）并重载；(a) 暂停期间看网卡与系统 DNS；(b) 暂停结束后看 | 启动时就在这样的网络上：标准输出里**没有** `enhanced mode on`，`GET` 的 `enabled` 是 `false`；日志 INFO `takeover paused on this network`。运行中进入暂停：INFO `enhanced mode off` 与 `takeover paused on this network`，网卡消失、系统 DNS 回到物理网卡；暂停期间的 `POST true` 回 `{}`、日志 INFO `the takeover is paused on this network; the enhanced mode is left as it is, and set as asked when the pause ends`（字段 `enabled`），网卡不回来。离开这个网络：INFO `takeover resumed` 与 `enhanced mode on`，网卡与路由回来。暂停期间强杀 rurge，`state.json` 里通常没有要恢复的东西（`enhanced_backup` 是 `null`）；`enabled` 是 `false` 是真的（不是“想要的”） | 标准输出；`GET`；日志；`Get-NetAdapter`；`state.json` | | | |
| W26 | **`ping` 经虚拟网卡。** 开着增强模式：`ping example.com`、`ping -4 example.com`、`ping -6 example.com`（只在有 IPv6 的网络上）、`ping 1.1.1.1`；`tracert -d example.com`；读 `GET /v1/features/enhanced_mode` 的计数 | 回显都收到应答；`icmpLimited` 不涨、`unsupported` 不涨；`GET /v1/requests/recent` 里没有回显的会话（ICMP 不经规则与策略，设计第 10 节）。DEBUG 里有 `icmp: echo requests leave through an interface`（字段 `interface` 是物理网卡），没有 `icmp: echo requests are unavailable`。记下 `ping` 的往返与直连时（不开增强模式）相比差多少 | 命令的输出；计数；DEBUG 日志 | | | |
| W27 | **外部程序一律 DIRECT。** 加一份 `external` 策略（例如 `Helper = external, exec = <一个会自己发起连接的程序的路径>, local-port = <端口>`），`[Rule]` 里 `FINAL,Helper`；开增强模式；让那个程序自己发一个连接（它经自己的方式连出去），同时从本机 `curl` 一个站点让它经 Helper | 本机的会话走 `Helper`；程序自己发起的连接在 `GET /v1/requests/recent` 里 `policy` 是 `DIRECT`、`notes` 里有 `Requested by an external program: always DIRECT`；DEBUG 日志里有带 `session` 与 `pid` 的那一行；程序拉起的子进程（不是它自己）的连接也一样（Windows 经 Job Object，`ProcessTree::contains` 认得出）。把子进程那一条单独做一次（让程序起一个子程序去连） | `recent`；DEBUG 日志；程序的输出 | | | |
| W28 | **`ipv6-vif` 的三种取值。** 分别用 `disabled`、`auto`、`always`（改 `<conf>`、重启）：(a) 在一个有 IPv6 的网络（路由器通告了地址与网关）上；(b) 在一个没有 IPv6 的网络上；(c) `always` 在没有 IPv6 的网络上（看 `W0035`） | `disabled`：网卡上没有 `fd00:6152::1`，IPv6 的路由表里没有 rurge 的条目，`rurge` 网卡的 IPv6 DNS 被清空（`show dnsservers` 是空的），系统 DNS 只接管 IPv4，`enhanced mode on` 的 `ipv6` 字段是 `false`。`auto`：有 IPv6 的网络与 `always` 一样（地址、路由、`fd00:6152::2`）；没有 IPv6 的网络与 `disabled` 一样，且没有 WARN。`always`：总是配；配不上时一行 WARN `IPv6 could not be configured; the enhanced mode goes on without it` 或 `the IPv6 routes could not be added to the virtual adapter; they are left out`，增强模式照开、`GET` 是 `true`；`rurge check` 对 `always` 有一句 `W0035` | `netsh` 的输出；日志；`rurge.exe check -c <conf>` | | | |
| W29 | **`tun-excluded-routes` 与 `tun-included-routes`。** `<conf>` 里加 `tun-excluded-routes = 203.0.113.0/24` 与 `tun-included-routes = 203.0.113.128/25`，重载；开着增强模式，看路由表，并分别访问 `203.0.113.1` 与 `203.0.113.129`（用 `tracert -d` 或一个监听在那两处的测试服务器） | 路由表里**没有** `203.0.113.0/24`，**有** `203.0.113.128/25`（包含项压过排除项，是更具体的前缀）；发往 `203.0.113.1` 的流量照系统原来的路由走（不进虚拟网卡），发往 `203.0.113.129` 的进虚拟网卡。把 `tun-excluded-routes` 写一段与 rurge 自己的 `198.18.0.0/15` 相交的，`rurge check` 有一句 `W0037`；一段整个包含 RFC 1918 私网的 `tun-included-routes` 给一句 `W0036`。再试 `tun-included-routes` 写一段**不在本机直连网段内**的地址（见 P30 的限制） | `netsh interface ipv4 show route`；`tracert` 或测试服务器；`rurge check` 的输出 | | | |
| W30 | **与全隧道 VPN 共存。** 先连上一个全隧道的 VPN（OpenVPN 的 `redirect-gateway def1` 一类会装 `0.0.0.0/1` 与 `128.0.0.0/1`），再开增强模式；记下错误；关掉 VPN，再开一次 | 打开**失败**，自己倒着撤销（网卡消失、系统 DNS 不动），标准错误 `error: cannot turn the enhanced mode on: <原因>`；原因里是 `netsh` 的原文（`The object already exists.` 一类）或 `<n> of the routes are missing after netsh added them`（`netsh` 的退出码不看，成败看读回）；现在网卡、路由与 DNS 都回到打开之前。关掉 VPN 再开：成功。`POST true` 时是 500 与同样的一句；`GET` 仍是 `false` | 标准错误；日志；`netsh` 的输出；`GET` | | | |
| W31 | **`set interface … metric=1` 被接受**（P9）。开着增强模式：`netsh interface ipv4 show interfaces`、`netsh interface ipv6 show interfaces`、`Get-NetIPInterface -InterfaceAlias rurge \| Format-List InterfaceMetric` | `rurge` 那一行的 `Met` 是 1（IPv4 与 IPv6 都是）；`Get-NetIPInterface` 的 `InterfaceMetric` 是 1。**现行文档的 `set interface` 没有 `metric` 参数**：它不被接受时日志 DEBUG 里那一行 `a step that may fail failed; going on` 会带着 `netsh` 的话，度量仍是系统给的值——记下是哪种。路由的 `Met` 也是 1（`show route`） | 三条命令的输出；DEBUG 日志 | | | |
| W32 | **读回在中文系统上照样认得出**（P10、P17）。在两台机器上各开一次增强模式：一台英文 Windows、一台中文 Windows；开之前先各跑一次 `netsh interface ipv4 show route`、`netsh interface ipv6 show route`、`netsh interface ipv4 show subinterfaces interface=<序号>`、`netsh interface ipv4 show addresses name=<序号>`、`netsh interface ipv4 show dnsservers name=<序号>` 并抄下原文；再开着增强模式读一次 | 两种语言的机器上打开都成功（地址、MTU、路由、DNS 都读回得到，没有 `the virtual adapter did not get its address` / `the virtual adapter's MTU is not 1500` / `the system's DNS could not be pointed at the enhanced mode`）。中文机器上的原文里汉字变成替换字符也无妨（输出按 `from_utf8_lossy` 读，数字与地址是 ASCII）；记下每张表的列与行格式，与代码里 `routes_in` / `addresses_in` / `mtu_in` 的读法对一遍（只要前缀后面跟着序号、`show subinterfaces` 的数据行以第一个数是 MTU）。解析不出来时把原文抄进记录 | 两种语言下的 `netsh` 原文；日志 | | | |
| W33 | **建网卡到拿到序号、以及关闭用了多久。** 开着增强模式与 `--log-level debug`，记下从启动到标准输出出现 `enhanced mode on` 的时间；关闭（Ctrl-C 或 `POST false`）时记下从发出到进程退出（或回 `{}`）的时间；两项各做五次 | 序号在 5 秒之内拿到（代码每 100 毫秒问一次，至多 5 秒；超过就是 `cannot open the virtual adapter: the virtual adapter has no interface index 5 seconds after it was made: <系统的话>`（`frame::explained` 的缺省一支加上了那个前缀），那一次打开失败）；关闭不挂住，通常远小于 3 秒；超过 3 秒时日志有一行 WARN `the TUN did not end within 3 seconds; going on`，关掉的只是等 TUN 这一步。关掉之后 `Get-NetAdapter -Name rurge` 找不到网卡。把两项的五次读数记下来 | 秒表；标准输出；日志 | | | |
| W34 | **两个 rurge 先后打开增强模式。** 用两份不同的数据目录与不同的监听端口（`--data-dir`、`--listen-http` / `--listen-socks5`），第一份开着增强模式；第二份也带 `--enhanced-mode` 启动 | 第二份打开**失败**：标准错误 `error: cannot turn the enhanced mode on: the virtual adapter is in use: is another rurge running with the enhanced mode on?`，以 1 退出（它不建第二块网卡，也不复用第一块）；第一份照常运行。`POST true` 时是 500 与同一句。DEBUG 里第二份的日志有 `wintun.dll` 那一行、**没有** `no adapter of that name is open`（探名字探不到时写的那一行只在第一份打开时出现）。先关掉第一份，第二份再开就成功 | 两份的标准输出与标准错误；日志；`Get-NetAdapter` | | | |
| W35 | **`wintun.dll` 的四种坏文件。** (a) 换一个 0.13 的 `wintun.dll`；(b) 把好的 `wintun.dll` 改掉一个字节（签名就坏了）再用；(c) `--wintun-dll` 指向一个不存在的文件；(d) 用压缩包里别的架构的那一份（`bin/x86` 或 `bin/arm64`）。每次都带 `--enhanced-mode` 启动，看标准错误 | (a) `error: cannot turn the enhanced mode on: wintun.dll has no WintunCloseAdapter: is it wintun.dll?`（0.13 没有这个入口，那时是 `WintunFreeAdapter` / `WintunDeleteAdapter`，没有找到 0.13 的文件时照实记下）。(b) `wintun.dll is not signed` 或 `wintun.dll is not signed by WireGuard LLC (it is signed by <名字>)` 或 `wintun.dll's signature cannot be trusted (0x<状态码>)`；不是“要管理员”。(c) `cannot find wintun.dll (next to rurge.exe, or where --wintun-dll points): get it from https://www.wintun.net/`。(d) `error: cannot turn the enhanced mode on: cannot load wintun.dll: <系统的话>`（x86-64 的 Windows 上通常是 `%1 is not a valid Win32 application` 一类，照实抄）。四种都不带路径；路径只在 DEBUG 的 `wintun.dll` 一行里（带账户名也不算错，因为它只在 DEBUG）。四种都不留任何改动（没有网卡、没有路由） | 标准错误；DEBUG 日志 | | | |
| W36 | **真的 `wintun.dll` 的签名者是 `WireGuard LLC`**（P5）。在一台有真的 `wintun.dll` 的机器上：不设 `RURGE_TEST_WINTUN_DLL` 先跑一次 `cargo test -j 8 --workspace --lib -- a_real_wintun_dll_passes`（它应当打印原因并返回）；再设上 `RURGE_TEST_WINTUN_DLL=<那个文件的路径>` 跑同一条 | 设了变量时它通过：签名者是 `WireGuard LLC`，模块加载得起来（`wintun.dll is loaded` 那一行 DEBUG 里的模块句柄非 0）；没设时它不失败、只打印原因（这是“没有这个文件”的正常路径）。把签名者的名字与文件的版本（`wintun.h` 的 `WINTUN_VERSION` 或 Wintun 的发布说明）记下来 | 用例的输出；`wintun.dll` 的属性页 / 版本 | | | |
| W37 | **不以管理员运行。** 以普通用户开一个命令行：(a) 运行 `rurge.exe run -c <conf> --enhanced-mode`，看标准错误与退出码；(b) 再以普通用户起一个**不带** `--enhanced-mode` 的实例（监听器与 API 起来），经 API `POST true` 一次 | (a) 标准错误 `error: cannot turn the enhanced mode on: the enhanced mode needs to run as Administrator`，以 1 退出；(b) `POST true` 是 500 `{"error":"the enhanced mode needs to run as Administrator"}`，`GET` 仍是 `false`。两者都不留改动。把机器上 UAC 的设置也记下来（管理员批准模式下双击是不是已经提权） | 标准错误；`GET` / `POST` 的应答；日志 | | | |
| W38 | **开着 Hyper-V / WSL / ICS 的机器**（设计第 7.4 节）。在一台装了 Hyper-V 或 WSL2（有 `vEthernet` 一类虚拟交换机）、或开了 Internet 连接共享（ICS）的机器上开增强模式；看路由表与 DNS；`ping` 与浏览器 | 能开：网卡、地址、路由、DNS 都起来，应用经虚拟网卡工作；**rurge 不动别人的网卡**（虚拟交换机的地址与 DNS 不变）；`Get-NetIPInterface` 里 rurge 的 `InterfaceMetric` 是 1、虚拟交换机的仍是原值；虚拟网卡自己的 `Forwarding` / `WeakHostSend` 是 disabled（`Get-NetIPInterface` 的 `Forwarding`，`Get-NetAdapterBinding`）。开不了时记下错误与原因（这些虚拟网卡的路由可能与 rurge 的前缀相撞） | `netsh` 与 `Get-NetIPInterface` 的输出；日志；应用的成败 | | | |
| W39 | **增强模式开着时大流量下载一次。** 开着增强模式，下载一个大的文件（几百 MiB；浏览器或 `curl`），同时看日志与计数 | 下载完成、速率与不开增强模式时接近；日志里 `the system had no room for a packet; the enhanced mode drops such packets and counts them` 出现几次（这一次运行的第一条是 WARN，之后每条 DEBUG），记下次数与下载速率；**`GET /v1/features/enhanced_mode` 的 `dropped` 不因这种丢包涨**（设备自己的计数与那一行 WARN 走的是设备，不是 `rurge-tun` 的原因表）。丢包多到影响 TCP 时把现象写下来 | 日志；计数；下载速率 | | | |
| W40 | **增强模式开着时，网络快照里没有虚拟网卡**（M4b 延后第 9 行）。开着增强模式，换一次网络（W24 的换法），看 DEBUG 的 `where the machine is now` 一行，以及 `subnet` 组的当前选择 | `where the machine is now` 的 `interface` 是物理网卡（不是 `rurge`），`wifi` / `ssid` 照旧读得到；`subnet` 组按物理网卡与 Wi-Fi 判（M1 清单 W1 的读法），不会因为多了一块虚拟网卡而变成 `default` 或另一个成员 | DEBUG 日志；`GET /v1/policy_groups/select?group_name=Net` | | | |
| W41 | **物理网卡上 ping 外部的 IPv4 与 IPv6**（M4b 延后第 15 行）。开着增强模式：(a) `ping -4 <一个外部的 IPv4 地址>`；(b) `ping -6 <一个外部的 IPv6 地址>`（没有 IPv6 的网络跳过，记下原因）；(c) 把 Windows 防火墙打开、关各做一次 (a) 与 (b)；(d) 在被 ping 的那台机器上看收到的包（或看它的防火墙日志） | 回显都收到应答，`icmpLimited` 不涨；防火墙开着、关着都通。**记下包真的出得去**（对端的抓包或日志）：Windows 上的 ICMP 套接字是原始套接字，负载的校验和由内核算——这一点没有出处，回环上的往返证明不了对外的那一条路；不通时把防火墙的规则、套接字创建失败的 WARN（`icmp: echo requests are unavailable, so pings go unanswered`）与系统的错误抄下来 | `ping` 的输出；对端的抓包；日志；计数 | | | |

---

### macOS

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| M17 | **开与关：命令行与 API。** `sudo ./rurge run -c <conf> --enhanced-mode --log-level debug`；看网卡；`GET /v1/features/enhanced_mode`；`POST {"enabled":false}`；再 `GET`；再 `POST {"enabled":true}` | 同 W19：`enhanced mode on`、名字 `utunN` 的网卡（`ifconfig utunN` 有 `198.18.0.1 netmask 255.254.0.0 mtu 1500`，IPv6 有 `fd00:6152::1 prefixlen 128`）、`GET` 的 `enabled` 为真、关掉之后网卡消失且 `GET` 全 0 | 标准输出；`ifconfig`；`GET` 的 JSON；日志 | | | |
| M18 | **路由表与网卡的样子。** 开着增强模式：`netstat -rn -f inet`、`netstat -rn -f inet6`、`ifconfig utunN`、`networksetup -getinfo "Wi-Fi"`（有线换成相应的服务） | rurge 装的、经 `utunN` 的路由，IPv4 是 55 条：W20 那 27 条，加 `240.0.0.0/4` 减去 `255.255.255.255/32` 的 28 条（`240.0.0.0/5`、`248.0.0.0/6`、……、`255.255.255.254/32`；回环、链路本地与组播都不在其中）；IPv6 开着时是那 16 条（`64:ff9b::/96` 与 `100::/8` … `fec0::/10`）。**系统随网卡地址自己加的那一两条不算**，例如网卡地址的主机路由，有的话还有 `fe80::%utunN/64`；看到别的多出来的行，照实记下来。缺省的 Profile 下 `198.18.0.0/15` 没有自己的一条：显式装的那一条被 `192.0.0.0/3` 盖住、聚合掉了（有排除项把 `192.0.0.0/3` 切开时才单独出现）；`fd00:6152::/64` 同理被 `fc00::/7` 盖住。`ifconfig utunN` 的 MTU 是 1500、地址是 `/128` 加 `alias`（不是 `prefixlen 64`）。记下接口的 `utun` 序号与名字（内核挑的） | `netstat` 与 `ifconfig` 的输出 | | | |
| M19 | **系统 DNS 的接管与交还。** 开着增强模式：`networksetup -getdnsservers "Wi-Fi"`、`scutil --dns`；`dig example.com`、`dscacheutil -q host -a name example.com`；关掉之后再做一次 | 开着时：每个**启用的**网络服务的 DNS 都是 `198.18.0.2`（有 IPv6 时还有 `fd00:6152::2`），`scutil --dns` 的 `resolver #1` 是它，`dig` 拿到 fake 地址，`ping example.com` 发往真地址并收到应答。关掉之后：服务回到原来的服务器（原来一条也没有的是 `empty`，`-getdnsservers` 打印 `There aren't any DNS Servers set on <服务>.`），`scutil --dns` 回到 `192.168.x.x` 一类；`state.json` 的 `enhanced_backup` 清空。**原来手动设过 DNS 的服务**单独记一条：交还之后是原来那几条 | `networksetup` 与 `scutil` 的输出；`dig`；`state.json` | | | |
| M20 | **强行结束后再起的恢复（`kill -9`）。** 开着增强模式，`sudo kill -9 <rurge 的 pid>`；看 `ifconfig -l` 与服务 DNS；再启动一次（不带 `--enhanced-mode`） | 强杀之后：`utunN` 从 `ifconfig -l` 里消失（设备随进程走），但**各网络服务的 DNS 还指着 `198.18.0.2`**（那一条会留在系统里）；`state.json` 的 `enhanced_backup` 里有 `{"kind":"mac_dns","service":…}` 的几项、`features.enhanced_mode` 可能还是 `true`。下一次启动（哪怕不带 `--enhanced-mode`）先把它们撤销：日志 INFO `restored what the enhanced mode left last time`，各服务的 DNS 回到原值，`enhanced_backup` 变成 `null`、`features.enhanced_mode` 写成 `false`。手工恢复的办法见 `docs/api/phase3.md` 的「手工恢复 macOS 的 DNS」（`networksetup -setdnsservers <服务> empty` 是小写的 `empty`），也试一次 | `ifconfig -l`；`networksetup -getdnsservers`；`state.json`；日志；手工恢复的那几条命令 | | | |
| M21 | **防环。** 同 W23：代理以域名配置、(b) 让运行中的 rurge 向上游问一次（`dig @198.18.0.2 example.com TXT`）、一次资源下载；前后读计数；`sudo tcpdump -i utunN -n` 看网卡上有没有 rurge 自己的包 | 同 W23：三类都通，计数不涨，网卡上抓不到 rurge 自己的流量；DEBUG 里有 `rurge's own connection leaves through the egress interface`（`interface` 是 `en0` 一类，`dst` 是上游的地址） | 计数；`tcpdump`；DEBUG 日志 | | | |
| M22 | **睡眠唤醒与换网络。** 同 W24：睡眠后换网络唤醒、唤醒到原来的网络、换网络的中途 | 同 W24：联网后 30 秒内路由与 DNS 与新的网络一致，应用照常；没有 `cannot follow the change` 的 ERROR。macOS 上换网络之后接管的是**新的**服务集合：新出现的服务也被接管（拔插扩展坞、VPN 连上又断开时能看到），不在了的服务在撤销时算做完（INFO `a network service the enhanced mode changed is gone; there is nothing to put back for it`）。记下恢复用了多久 | 日志；`networksetup` 的输出；应用成败 | | | |
| M23 | **接管暂停。** 照 W25 做，`[SSID Setting]` 写成 `SSID:<A> suspend=true`（macOS 读得到 SSID 时） | 同 W25：启动时就不开（没有 `enhanced mode on`）、进入与离开暂停的日志与网卡状态、暂停期间 `POST` 只记下想要的。另外记下：暂停期间服务 DNS 已交还（`networksetup -getdnsservers` 是原值） | 标准输出；`GET`；日志；`networksetup` | | | |
| M24 | **`ping`。** 同 W26：`ping example.com`、`ping -6 example.com`、`ping 1.1.1.1`；`sudo tcpdump -i utunN -n icmp` | 同 W26：回显有应答、计数不涨、没有回显的会话；`tcpdump` 里看得到回显从 `utunN` 进去、从 `en0` 出去（rurge 的套接字按出口绑在物理网卡上）。记下 macOS 数据报套接字的标识符/序号对不对得上（按标识符与序号对号，这一点没有在 macOS 上观察过） | 命令的输出；`tcpdump`；计数 | | | |
| M25 | **外部程序一律 DIRECT。** 同 W27 | 同 W27：本机客户的会话走 Helper，程序（与它起的子进程，同一个进程组）的会话 `policy` 是 `DIRECT`、`notes` 有那一句；DEBUG 有带 `session` 与 `pid` 的行。**`setsid` 出去的子进程不算**（登记的限制）：让程序自己 `setsid` 一个孩子再连，它的会话照规则走（走 Helper），把这一条也记下来 | `recent`；DEBUG 日志；程序的输出 | | | |
| M26 | **`ipv6-vif` 的三种取值。** 同 W28 | 同 W28；macOS 上另记：IPv6 地址是 `fd00:6152::1 prefixlen 128 alias`，`netstat -rn -f inet6` 里对应的显式路由在；`disabled` 之后地址与路由都没了（`ifconfig` 上看不到 `fd00:6152::1`） | `ifconfig`；`netstat`；日志；`rurge check` | | | |
| M27 | **`tun-excluded-routes` 与 `tun-included-routes`。** 同 W29 | 同 W29，用 `netstat -rn` 看；macOS 上 `198.18.0.0/15` 是显式装的，排除项里写它时 `W0037` 照报 | `netstat`；`tracert` 的等价物（`traceroute -n`）或测试服务器；`rurge check` | | | |
| M28 | **与全隧道 VPN 共存。** 同 W30（用一个装 `0.0.0.0/1` 与 `128.0.0.0/1` 的 VPN） | 同 W30：打开失败、倒着撤销、错误里是 `route` 的原文（`File exists` 一类）；关掉 VPN 再开成功 | 标准错误；日志；`netstat` | | | |
| M29 | **多个网络服务与手动设过 DNS 的服务**（M4c 延后第 2、6、7、8 行）。在一台有多个启用服务（Wi-Fi、以太网、Thunderbolt 网桥、一个 VPN 服务）的机器上：(a) 抄下 `networksetup -listallhardwareports`、`networksetup -listallnetworkservices`、`networksetup -listnetworkserviceorder`、`ipconfig getpacket <主网卡>` 的原文，开增强模式，对照日志里被改的服务名；(b) `sudo networksetup -setdnsservers "<不存在的服务>" 1.1.1.1; echo $?`；(c) 让一个有 VPN（NetworkExtension 装的服务）的机器开增强模式；(d) 开着增强模式，给一个服务改名（系统设置里改名字），再关掉增强模式 | (a) rurge 改的是**启用的**服务，主网卡所在的那个在其中；日志 DEBUG 的 `cannot list the network services' devices` 没有出现；`-listnetworkserviceorder` 的配对与代码一致（两行一个服务，`(Hardware Port: …, Device: …)`，VPN 服务的 `Device:` 可以是空的，服务名里可以有括号）。(b) 记下退出码：不是 0 的话 `take_dns` / `undo` 的判断靠得住；**是 0 的话**记下来（那时改了不存在的服务也被当成改成了，见 M4c 延后第 6 行）。(c) 一个服务读得了原值、改不了时**整个打开失败**（P8 的那一句 `no network service's DNS could be pointed at the enhanced mode` 或管理器回的错误），倒着撤销；记下是哪一个服务、为什么（VPN 的服务可能改不了）。(d) 改名的那个服务在撤销时被当成“不在了”（INFO `a network service the enhanced mode changed is gone; there is nothing to put back for it`），**它的 DNS 留在 `198.18.0.2`**——已知限制，手工把它改回去，并记下来 | 四条 `networksetup` / `ipconfig` 的原文；日志；`state.json`；`echo $?` | | | |
| M30 | **接管期间 `server:system` 问的是物理网卡的服务器**（M4c 延后第 5 行）。看运行中的 `rurge run`，不用 `rurge dns lookup`——它是另一个进程，不在增强模式里。照 W49 做，平台不同的地方：`[General]` 的 `dns-server = 9.9.9.9`；`[Host]` 的 `example.com = server:system`；开增强模式之前记下 `networksetup -getdnsservers "<服务>"` 与 `ipconfig getpacket <主网卡>` 里的 `domain_name_server`。(b) 先停下 rurge；把主网卡所在服务改成手动地址、一条 DNS 也不设——`sudo networksetup -setmanual "<服务>" <原来的地址> <掩码> <路由器>` 与 `sudo networksetup -setdnsservers "<服务>" empty`，这样手设的与 DHCP 给的都没有；再起 rurge，再做一次经 HTTP 监听端口那一条。恢复：`sudo networksetup -setdhcp "<服务>"`，再 `sudo networksetup -setdnsservers "<服务>" <记下的地址>`（原来一条也没有就写 `empty`） | 同 W49。(a) 的物理上游是主网卡所在服务手设的服务器，没有手设的就是 DHCP 给的（`crates/rurge-platform/src/enhanced/macos.rs::physical_dns`）；(b) 的 `curl` 打印 `502`，记录 `status` 是 `failed`，`error` 里有 `all upstreams failed (system DNS servers: no DNS server outside the enhanced mode)`，`remoteAddr` 是 `null`。`run.rs` 把 `physical_dns` 交给管理器的那一行没有用例走到，这一项是它的端到端证据 | `recent` 的 JSON；DEBUG 日志；`curl` 的输出 | | | |

---

### Linux

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| L18 | **开与关：命令行与 API。** `sudo ./rurge run -c <conf> --enhanced-mode --log-level debug`；`ip addr show rurge`；`GET /v1/features/enhanced_mode`；`POST {"enabled":false}`；再 `GET`；再 `POST {"enabled":true}` | 同 W19：`enhanced mode on`；`rurge` 网卡在（`IFF_TUN`、MTU 1500、`198.18.0.1/15` 与 `fd00:6152::1/64`）；`GET` 的 `enabled` 为真；关掉之后网卡消失（Linux 的 TUN 不持久，描述符一关就连同地址与路由一起没有）、`GET` 全 0；`POST true` 再开一次 | 标准输出；`ip addr`；`GET` 的 JSON；日志 | | | |
| L19 | **路由表与网卡的样子。** 开着增强模式：`ip route show`、`ip -6 route show`、`ip addr show rurge`、`ip link show rurge` | rurge 装的、**不带 `proto kernel`** 的 `dev rurge` 行，IPv4 是 55 条：W20 那 27 条，加 `240.0.0.0/4` 减去 `255.255.255.255/32` 的 28 条（`240.0.0.0/5`、`248.0.0.0/6`、……、`255.255.255.254/32`）；IPv6 开着时是那 16 条（`64:ff9b::/96` 与 `100::/8` … `fec0::/10`）。**`proto kernel` 的那几行不算**，例如 `198.18.0.0/15`（`ip route` 里是 `198.18.0.0/15 dev rurge proto kernel scope link src 198.18.0.1`）与 `fd00:6152::/64`，有的话还有 `fe80::/64`；回环、链路本地与组播不在 rurge 装的那些里。`ip link` 的 `mtu 1500`、`qdisc` 与 `state UNKNOWN` 记下 | `ip` 的输出 | | | |
| L20 | **系统 DNS 的接管与交还。** 开着增强模式：`resolvectl status rurge`、`resolvectl dns rurge`、`resolvectl domain rurge`、`cat /etc/resolv.conf`、`dig example.com`；关掉之后再做一次（含 `resolvectl status rurge` 不存在了） | 开着时：`rurge` 链路的 `DNS Servers: 198.18.0.2`（有 IPv6 时还有 `fd00:6152::2`）、`DNS Domain: ~.`、`Default Route: yes`；`/etc/resolv.conf` 仍是 resolved 的存根（`nameserver 127.0.0.53`），rurge 不改它；`dig` 拿 fake 地址。关掉之后：链路没了，`dig` 拿真地址，`resolvectl status` 里没有 `rurge`；`resolvectl revert rurge` 在关闭时做过（不报错） | `resolvectl` 与 `dig` 的输出；`/etc/resolv.conf` | | | |
| L21 | **没有 systemd-resolved 的机器。** 在一台不用 resolved 的机器上（`/etc/resolv.conf` 不是存根，或没有 `resolvectl`）开增强模式 | 增强模式照开：网卡、地址、路由都在；系统 DNS **不接管**，WARN 一行 `the system's DNS is not systemd-resolved's stub; it is left as it is: write hijack-dns = *:53, or point the system's DNS at 198.18.0.2`（一次打开至多一行）；按那句话里的两个办法之一做，DNS 就经应答器（`dig` 拿 fake 地址） | 日志；`ip` 的输出；`dig` | | | |
| L22 | **强行结束后再起的恢复（`kill -9`）。** 把 `/proc/sys/net/ipv4/conf/all/rp_filter` 设成 1（需要的话），开增强模式，读它；`sudo kill -9 <pid>`；看网卡与 `rp_filter`；再启动一次（不带 `--enhanced-mode`） | 强杀之后：`rurge` 网卡消失（TUN 不持久），`/proc/sys/net/ipv4/conf/all/rp_filter` **可能仍是 2**（那是会留在系统里的改动）；`state.json` 的 `enhanced_backup` 里有 `{"kind":"rp_filter","value":1}`。下一次启动先撤销：日志 INFO `put net.ipv4.conf.all.rp_filter back to 1`（或 `restored what the enhanced mode left last time`），值回到 1，`enhanced_backup` 变成 `null`。没有主网卡时不改 `rp_filter`（不需要放宽），那时备份里没有它 | `cat /proc/sys/...`；`ip addr`；`state.json`；日志 | | | |
| L23 | **防环。** 同 W23：代理以域名配置、(b) 让运行中的 rurge 向上游问一次（`dig @198.18.0.2 example.com TXT`）、一次资源下载；`sudo tcpdump -i rurge -n` 看虚拟网卡上有没有 rurge 自己的包 | 同 W23：三类都通，计数不涨，网卡上抓不到 rurge 自己的流量；DEBUG 有 `rurge's own connection leaves through the egress interface`（`interface` 是物理网卡，`dst` 是上游的地址） | 计数；`tcpdump`；DEBUG 日志 | | | |
| L24 | **睡眠唤醒与换网络。** 同 W24：`systemctl suspend` 后换网络唤醒、唤醒到原来的网络、换网络的中途 | 同 W24：联网后 30 秒内路由与 DNS 与新的网络一致；没有 `cannot follow the change` 的 ERROR；网络变化时 DNS 与 `rp_filter` 再做一次（`loosened net.ipv4.conf.all.rp_filter to 2: …` 可能再出现一行）。记下恢复用了多久 | 日志；`ip route` 与 `resolvectl` 的输出 | | | |
| L25 | **接管暂停。** 照 W25 做（`[SSID Setting]` 写 `SSID:<A> suspend=true` 或 `TYPE:WIRED suspend=true`） | 同 W25 | 标准输出；`GET`；日志；`resolvectl status rurge` | | | |
| L26 | **`ping`。** 同 W26：`ping example.com`、`ping -6 example.com`、`ping 1.1.1.1`；`sudo tcpdump -i rurge -n icmp` | 同 W26：回显有应答、计数不涨；`tcpdump` 里回显从 `rurge` 进去、从物理网卡出去。Linux 上 ICMP 套接字先数据报（不要特权，受 `net.ipv4.ping_group_range` 管）、不行退原始：两种都试一次（把 `ping_group_range` 调窄、再放开），记下走的是哪一种（日志 DEBUG 的 `icmp: echo requests leave through an interface` 与读回时用的标识符规则） | 命令的输出；`tcpdump`；`/proc/sys/net/ipv4/ping_group_range`；日志 | | | |
| L27 | **外部程序一律 DIRECT。** 同 W27 | 同 W27（Unix 认的是同一个进程组，程序起的子进程在里面）；`setsid` 出去的子进程照规则走，登录下来 | `recent`；DEBUG 日志；程序的输出 | | | |
| L28 | **`ipv6-vif` 的三种取值。** 同 W28 | 同 W28；Linux 上 IPv6 地址是 `fd00:6152::1/64`（不是 `/128`），`ip -6 addr show rurge` 上看得见；`disabled` 之后它被显式撤掉（`addr del`） | `ip addr` / `ip -6 addr`；`ip -6 route`；日志；`rurge check` | | | |
| L29 | **`tun-excluded-routes` 与 `tun-included-routes`。** 同 W29 | 同 W29，用 `ip route show` 看；排除项写 `198.18.0.0/15` 里的地址时 `W0037` 照报 | `ip route`；`traceroute -n` 或测试服务器；`rurge check` | | | |
| L30 | **与全隧道 VPN 共存。** 同 W30（用一个装 `0.0.0.0/1` 与 `128.0.0.0/1` 的 VPN，例如 `wg-quick` 的全隧道配置或 OpenVPN） | 同 W30：打开失败、倒着撤销，错误里是 `ip` 的原文（`RTNETLINK answers: File exists` 一类，脚本的路径换成 `…`）；关掉 VPN 再开成功。**rurge 用 `route add` 不用 `replace`**：别人的路由不被换掉，这也顺带核对了这一点 | 标准错误；日志；`ip route` | | | |
| L31 | **严格 `rp_filter` 的发行版与逐链路设置**（M4c 延后第 9 行与 R25）。在 RHEL 一族（`net.ipv4.conf.all.rp_filter = 1`）上开增强模式：读 `/proc/sys/net/ipv4/conf/all/rp_filter` 与主网卡的；`resolvectl status rurge`；关掉之后再读；另在 NetworkManager 的机器上开 30 分钟，看 `resolvectl status rurge` 有没有被盖掉 | 打开时：主网卡的有效值是 1 时 `all` 被写成 2，日志 INFO `loosened net.ipv4.conf.all.rp_filter to 2: replies to rurge's own connections come in on another interface than the routes to their senders`；回程不被丢，经虚拟网卡的连接与 `ping` 都正常（不设 2 时严格 `rp_filter` 会丢掉 rurge 自己连接的回包）。`resolvectl status rurge` 的 LLMNR、mDNS、DNSSEC、DNSOverTLS 都是 `no`，DNS Servers 是 `198.18.0.2`，Domains 是 `~.`；**关掉之后** `resolvectl revert rurge` 把它撤掉（链路没了，全局的设置回到原值），`rp_filter` 写回 1（INFO `put net.ipv4.conf.all.rp_filter back to 1`）。NetworkManager 的机器上开了 30 分钟，逐链路的设置没有被盖掉（仍是 `198.18.0.2` 与 `~.`）；被盖掉时记下多久、被谁盖的 | `cat /proc/sys/...`；`resolvectl status`；日志 | | | |
| L32 | **L2 特权用例在一次性的虚拟机里跑一遍**（M4c 延后第 1 行、总设计 Q4）。照 `tests/privileged/README.md`：一次性的 Linux 虚拟机（root、`iproute2`、iputils 的 `ping`、`/dev/net/tun`，不需要 systemd-resolved），`cargo build -p rurge`，`RURGE_TEST_PRIVILEGED=1 cargo test -p rurge-privileged-tests` | 三条用例都通过：一次 TCP 往返与一次 `ping` 经虚拟网卡、以 `DIRECT` 出去；rurge 自己的 DNS 查询不进虚拟网卡（b 端看到的来源是 veth 地址）；`rp_filter` 设成 1、打开之后是 2、`kill -9` 之后下一次启动把它写回 1 且 `enhanced_backup` 是 `null`。失败的用例把输出与虚拟机的发行版记下来。跑完 `ip netns list` 里没有留下的命名空间 | 用例的输出；`ip netns list`；`/etc/netns/` | | | |

---

### 与平台无关

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| P29 | **`dns_tcp_max`（缺省 128）在真实设备上**（M3c 延后第 27 行、M4-D25）。开增强模式。同时开 200 条短的 TCP DNS 连接：`for i in $(seq 200); do python3 tcp_pipeline.py 198.18.0.2 1 & done; wait`（`tcp_pipeline.py` 见 M3 一节的前置条件）；地址与端口不变，并行 200 条，真到 200 条同时活着时连接超出的那些会立刻失败；之后再问一次 UDP 的查询 | 超出的 SYN 回 RST（应用立刻失败，而不是排队等），计 `busy`；已建立的连接照常应答；UDP 的查询不受影响；内存占用不因 128 条连接出问题（缺省下约 24 MiB）。**没有选项能改这个数**（`TunOptions` 的缺省 128，只能经由代码改）：要验的就是它在真实设备上够不够。把 128 条够不够、应用的反应、`busy` 的增长记下来——这个数取的是同类的做法（systemd-resolved 对进来的 TCP 流至多 128 条），**没有在真实设备上试过** | 应用的反应；`GET /v1/features/enhanced_mode` 的 `busy`；内存 | | | |
| P30 | **非直连的 VPN 路由的限制与它的办法**（M4b 延后第 14 行、兼容性清单 9.1 节）。装一个拆分隧道的 VPN，它推下来的路由**不是直连的**（`10.0.0.0/8 via 10.8.0.1 dev tun0` 一类）；开着增强模式，让 rurge 自己去访问那一段里的一个地址（例如一个在那段里的订阅源）：(a) 不加任何设置；(b) 把那一段写进 `tun-excluded-routes` 再来 | (a) 应用的流量照系统的路由进 VPN（那条路由比基础前缀更具体），**rurge 自己**到那里的连接却绑在主网卡上、到不了（失败的原因只进 DEBUG 与请求记录）；这就是登记的限制。(b) 写进排除项之后，rurge 自己的连接落在出口判定的“不绑”那一条，照系统的路由走、进 VPN，下载与 DNS 因此也通。把两种情形下的失败文本与耗时记下来 | 请求记录；日志；VPN 的路由表 | | | |
| P31 | **换网络之后 30 秒内恢复**（阶段 3 的验收标准）。三个平台各做一次：开着增强模式，换网络（Wi-Fi 互换、插拔网线各一次），立刻用浏览器、`curl` 与 `ping` 试着访问；记下从换网络完成到第一次成功的时间 | 30 秒内全部恢复：DNS（应答器仍在被问）、路由（八条基础前缀仍在）、应用与 `ping` 都通；`GET /v1/features/enhanced_mode` 的 `enabled` 一直是 `true`（换网络不关增强模式），十个数不因换网络涨（`unknown_fake` 可能因为换网络那一刻的旧 fake 地址涨一点，记下原因）。超过 30 秒的把日志（`network changed`、`cannot follow the change`）与耗时间记下来 | 秒表；应用成败；日志；计数 | | | |

---

## M5　识别、阻断与 `[IP Rewrite]`

阶段 3 / M6 订正了 W25、W42、W43、W47、M31、M32、L33、L34（`GET` 的 `enabled` 写法；W42 一族的防火墙恢复命令与新增的 `[IP Rewrite]` `drop` + 经代理监听端口一步，`dropped.ipRewrite` 按被丢的包数涨；W43 一族新增的 IPv6 一步、`ipv6-vif` 写 `always`；W47 的 A 查询改成 TXT；自适应两行日志照代码写全）。会改系统设置的步骤按「怎么用」的「改系统设置的项与恢复」做。

设计 `docs/superpowers/specs/2026-10-28-phase3-m5-recognition-blocking-ip-rewrite-design.md`（先读它第 20、21 节的订正），计划 `docs/superpowers/plans/2026-10-28-phase3-m5-recognition-blocking-ip-rewrite-plan.md`（这一节的来由：设计第 16 节与任务原文列的六样，计划「延后事项」第 1 行的手工验收，和 `docs/api/phase3.md`「M5」一节「没有覆盖的」里“没有真机”一条列的事（只在内存设备与单元用例里验证过的））；API 与日志行见 `docs/api/phase3.md` 的「M5」一节，兼容性清单里的差异见 `docs/surge-compatibility-matrix.md` 的 1.2、2.1、3.1、4.3、4.6、6.1、6.2、7.2 与 9.10 节。

自动化验不了的是真实系统与真实应用对这些包的反应：`reflect` 送回的包由操作系统的协议栈接住、它的应答又被送回；被 `reject` 与 `drop` 处理的连接由真实的应用看到；Windows 的防火墙、真实的浏览器与 `curl` 的 QUIC / STUN、真实的 DoH 服务器都在内存设备之外。**M5 的这一切只在一台 Windows 11 x86-64 开发机的内存设备里测过**（一台机器上的一次运行，不是 CI；M5 的提交 CI 一个也没有见过），`reflect` 送回的包在真机上的去向没有观察过，`udp-priority` 的效果没有观察过；**一项也没有做过**。

**会改系统设置的步骤（开增强模式、写路由、改防火墙）在一次性的 Windows / macOS / Linux 机器或虚拟机上做。**每一项记下日期、系统版本与结果。做完怎样恢复见「怎么用」的「改系统设置的项与恢复」。

编号接着 M4 往下排（W42、M31、L33、P32），整份清单里的编号不重复。

**`udp-priority` 不设手工项**：它的效果是“设备写得慢时 UDP 回包不排在 TCP 后面”，真实应用上量不出来（差别在一次设备写与排队的量级，被网络与系统调度淹没），兼容性清单 2.1 节照实写着“尽力而为”；两条队列由 `tests::writer` 的单元用例与 M5 的二十次循环钉住（M5 记录第 40、45 行）。

### 前置条件

- `cargo build --release`，用得到的二进制（Windows 下是 `target\release\rurge.exe`，其余是 `target/release/rurge`）。下文按平台写作 `rurge.exe`（Windows）或 `./rurge`。
- 要 root / 管理员；开增强模式（M4 一节的前置条件同样适用，Windows 要 `wintun.dll`）。
- 一份测试用 Profile（下文的 `<conf>`），每项要加的行写在它自己的操作里，改完重启（增强模式的打开只看启动参数与 `POST`）：

  ```ini
  [General]
  http-listen = 127.0.0.1:6152
  http-api = testkey@127.0.0.1:6171
  dns-server = 1.1.1.1
  ipv6-vif = auto
  # 每一项按需要加：tun-included-routes 把文档地址送进虚拟网卡

  [Rule]
  FINAL,DIRECT
  ```

- **文档地址要先被路由进虚拟网卡**（手册：被 `tun-excluded-routes` 或本地网络的路由盖住的，写进 `tun-included-routes`）：`[IP Rewrite]` 用的 `192.0.2.0/24`、`198.51.100.0/24`、`203.0.113.0/24` 写进 `tun-included-routes`（Windows 上是 `tun-included-routes = 192.0.2.0/24,198.51.100.0/24,203.0.113.0/24`），否则包根本不会进虚拟网卡，看起来像“没生效”。
- 一份能用的 `[IP Rewrite]`（每一行先只写那一项要用的，做完删掉，免得互相影响）：

  ```ini
  [IP Rewrite]
  192.0.2.10 = reflect
  198.51.100.0/24 = reject
  203.0.113.0/24 = drop
  ```

- `curl --version` 要看得到 `HTTP3` 特性（没有的话需要换一个带 HTTP/3 的 `curl`，这一项要记下来；不支持 HTTP/3 的 `curl` 做不了 W44 与 W46）；`--http3-only` 是 `curl` 的选项（Windows 上的 `curl.exe` 同样）。
- 一个 443 以外的 HTTP/3 服务（W44；公开服务很少，可以在局域网的另一台机器上起一个，并把它的地址写进 `tun-included-routes`）；一个支持 HTTP/3 的域名（W46）；一个 DoH 服务器的主机名（W47）；一个载 UDP 的代理策略（`hysteria2` / `masque` / `tuic` / `socks5` + `udp-relay=true` 一类）。
- 日志：`rurge run -c <conf> --enhanced-mode --log-level debug --log-file <日志路径>`；下文「日志」都指这里。`reflected` 不写日志；`[IP Rewrite]` 的丢弃是 DEBUG 的 `tun: packet dropped`（`reason` 是 `ip_rewrite`）；自适应的两行是 DEBUG 的 `quic: <域名>: no TCP within 1s; QUIC allowed` 与 `quic: <域名> falls back to TCP; QUIC stays blocked`。
- 计数：`curl -s -H "X-Key: testkey" http://127.0.0.1:6171/v1/features/enhanced_mode`（Windows 用 `curl.exe`）。M5 之后 `dropped` 有十一个键（多 `ipRewrite`），顶层有 `reflected`。
- 抓包：Windows 用 Wireshark 在 `rurge` 网卡上；macOS `sudo tcpdump -i utunN -n`；Linux `sudo tcpdump -i rurge -n`。

---

### Windows

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| W42 | **`[IP Rewrite]` 的 `reflect`。** `[IP Rewrite]` 只写 `192.0.2.10 = reflect`，`tun-included-routes` 加 `192.0.2.0/24`，重启并开增强模式；先 `curl.exe --connect-timeout 3 http://192.0.2.10/`；再 `ping 192.0.2.10`；Windows 防火墙缺省挡入站的回显请求，放行 ICMPv4 入站（`New-NetFirewallRule -DisplayName rurge-icmp-in -Protocol ICMPv4 -IcmpType 8 -Direction Inbound -Action Allow`）后再 `ping` 一次；再做一步（验收文档 #106）：`[IP Rewrite]` 加一行 `1.1.1.1 = drop`（换成你能直接连上的公网地址），`curl.exe --connect-timeout 3 http://1.1.1.1/`（经虚拟网卡）与 `curl.exe -x http://127.0.0.1:6152 http://1.1.1.1/`（经 HTTP 监听端口），做完删掉这一行；前后读 `GET /v1/features/enhanced_mode`；Wireshark 在 `rurge` 网卡上看，日志 `--log-level debug` | `reflected` 每个送回的包涨一、`ipRewrite` 不涨；没有 WARN 行；Wireshark 里包的两个地址对调、**端口与其余字节不变**、TTL 与原来相同（记下 TTL）。`curl` 的连接立刻以“拒绝”结束（操作系统的 RST 又被送回，RST 不再引发回应，所以不会来回不停）；`GET /v1/requests/recent` 里没有这两个地址的会话。`ping`：防火墙放行时收到应答（操作系统的 echo reply 再被送回一次，两次都对调了地址）；没放行时超时（Windows 挡入站回显）——两种都记下来。`1.1.1.1 = drop` 那一步：经虚拟网卡的不通、`GET` 的 `dropped.ipRewrite` 按被丢的包数涨（SYN 与它的重发，Wireshark 里数得出来）；经监听端口的通、`/v1/requests/recent` 里有这一条请求（`listener` 是 `http`）；DEBUG 里有 `rurge's own connection leaves through the egress interface`（`interface` 是物理网卡），rurge 自己发往这个地址的出站不回到虚拟网卡 | `GET` 前后各一份；Wireshark 的两三个包；日志；防火墙规则 | | | |
| W43 | **`reject` 与 `drop`。** `[IP Rewrite]` 写 `198.51.100.0/24 = reject` 与 `203.0.113.0/24 = drop`；`curl.exe --connect-timeout 3 http://198.51.100.10/`；`curl.exe --connect-timeout 3 http://203.0.113.10/`；再各发一个 UDP 数据报（`nslookup example.com 198.51.100.10` 与 `nslookup example.com 203.0.113.10`）；再加一步（验收文档 #96）：`[IP Rewrite]` 写 `2001:db8::10 = reject`，`tun-included-routes` 加 `2001:db8::/32`（`[General]` 写 `ipv6-vif = always`；网络有 IPv6 时 `auto` 也行），`curl.exe -6 --connect-timeout 3 http://[2001:db8::10]/` 与 `ping -6 2001:db8::10`，做完删掉；前后读计数；Wireshark | `reject`：TCP 立刻被拒（RST，`curl` 毫秒级结束而不是等到 3 秒）；UDP 收到 ICMP“管理上禁止”（`nslookup` 立刻失败；Wireshark 看到 ICMP 差错，IPv4 type 3、IPv6 type 1）。`drop`：两者都等到自己的超时（`curl` 3 秒），**没有任何回包**。两者都计 `ipRewrite`（drop 也计），都不进请求记录；日志里每个包一行 DEBUG `tun: packet dropped`（`reason=ip_rewrite`、带两个地址）、没有 WARN。连发到限频再读：3 秒内各发一百多个（一个循环）——`rstLimited` 与 `icmpLimited` 是**两份**限频，各是每 3 秒 100 个（`reject` 的 RST 与虚拟网卡自己的 RST 共用一份，`reject` 的 ICMP 与被拒 UDP 流的 ICMP 共用另一份），超出之后回包停下而 `ipRewrite` 照涨（M5 记录第 28 行）。IPv6 那一步：连接与 `ping` 立刻失败（RST / ICMPv6 的“管理上禁止”），`dropped.ipRewrite` 每次拒绝加一；Wireshark 里 IPv6 的 ICMP 是 type 1 | `curl` 的耗时与错误；`nslookup`；Wireshark；计数前后；日志 | | | |
| W44 | **443 以外的 QUIC 被挡。** `[Proxy]` 加 `D = direct, block-quic=on`，`[Rule]` 在 `FINAL` 之前加 `DEST-PORT,8443,D`；开着增强模式，`curl.exe --http3-only -I https://<443 以外的 HTTP/3 服务>:8443/`；再把那条规则暂时去掉重试（对照） | 写规则时：`curl` 很快失败（QUIC 被挡，`--http3-only` 不会回落到 TCP），`GET /v1/requests/recent` 里那条 UDP 流的 `protocol` 是 `quic`、`policy` 是 `REJECT`、`error` 是 `QUIC blocked`，端口是 8443。去掉规则：同一条请求能到服务（至少握手），记录里的 `protocol` 仍是 `quic`、这次走 `DIRECT`（或规则选中的策略）。**M5 之前只有 443 会被识别，8443 不会被挡**——这一项要看的就是这个差别；QUIC 少认的版本（gQUIC、草案 28 以前）不在这一项里 | 两次 `curl` 的输出；`recent` 的记录；计数 | | | |
| W45 | **`PROTOCOL,STUN` 挡掉 WebRTC 的 STUN。** `[Rule]` 在 `FINAL` 之前加 `PROTOCOL,STUN,REJECT`；开着增强模式，在浏览器里打开一个 WebRTC 测试页（或 `chrome://webrtc-internals`）发起一次连接；读 `recent` 与计数；把规则换成 `PROTOCOL,UDP,REJECT` 再对照一次；去掉规则再试一次 | 写 `STUN` 规则时：浏览器收集不到服务器反射候选（`srflx`）或明显变慢/失败；`recent` 里出现 `protocol` 是 `stun` 的 UDP 流（`rule` 是那条 `PROTOCOL,STUN` 规则、`policy` 是 `REJECT`——这里**不**是 `QUIC blocked`，那句话只在 QUIC 被 `block-quic` 挡下时出现），端口不是 443（3478 / 19302 一类，证明所有端口都识别）。换成 `UDP` 规则：STUN 也被挡，但记录里的 `protocol` **仍是 `stun`**（标签来自嗅探、不来自规则；`UDP` 覆盖 STUN），变的是 `rule`。去掉规则：STUN 通、`srflx` 有。记下浏览器的版本与它用的 STUN 服务器 | `recent` 的记录；浏览器的 WebRTC 统计；计数；日志 | | | |
| W46 | **`block-quic=adaptive` 与 6.11 的 `auto`。** `[Proxy]` 加一个载 UDP 的代理 `Udp = hysteria2, <server>, 443, password=<口令>, sni=<名字>`；规则 `DOMAIN,<支持 HTTP/3 的域名>,Udp`；`Udp` 的策略参数写 `block-quic=adaptive`（不写时用缺省的 `auto`，对这个代理同样落到自适应——两种都试一次）；开着增强模式，`curl.exe --http3-only -I https://<域名>/` 一次，1 秒以上之后再跑一次；再用浏览器访问同一个域名一次（浏览器会从 QUIC 回落到 TCP）；读计数与日志 | `curl --http3-only`：第一次失败；1 秒之后第二次成功。日志 DEBUG 一行 `quic: <域名>: no TCP within 1s; QUIC allowed`（没有回落），之后这个名字的 QUIC 不再被挡；`recent` 里第一次是 `REJECT` + `QUIC blocked`，第二次经 `Udp` 成功。浏览器：先被挡的 QUIC 之后来一条到同一个名字的 TCP，日志是 `quic: <域名> falls back to TCP; QUIC stays blocked`，之后的 QUIC 仍被挡；**阻断之前已经在的 TCP 不算回落**（M5-D6）。写 `block-quic=off` 的对照：第一次就成功；写 `on` 的对照：一直失败 | 两次 `curl` 的耗时与结果；浏览器的表现；两行 DEBUG；`recent` | | | |
| W47 | **按 DNS 服务器的主机名写的规则。** `<conf>` 写 `encrypted-dns-server = https://<自己的 DoH 服务器>/dns-query` 与 `encrypted-dns-follow-outbound-mode = true`；`[Rule]` 在 `FINAL` 之前写 `DOMAIN,<DoH 服务器的主机名>,DIRECT`；让运行中的 rurge 解析一个缓存里没有的名字（开着增强模式时 `nslookup -type=TXT <新名字>` 经虚拟网卡的应答器——TXT 由应答器转发给上游，也就是这个 DoH 服务器；A / AAAA 查询由应答器发 fake 地址，不问上游；或 `curl.exe -x http://127.0.0.1:6152 http://<新名字>/` 经 HTTP 代理）；读 `GET /v1/requests/recent`；再把那条规则交给一个载 UDP 的代理、看代理那一侧的访问日志 | 记录里 DNS 会话的 `dst` 是**名字与端口**（`<主机名>:443`），`rule` 是那条 `DOMAIN` 规则、`policy` 是 `DIRECT`；`dnsMs` 是 `null`；`remoteAddr` 是拨到的地址（TCP）或 `null`（QUIC 载体）。把规则写成 `IP-CIDR,<解析出的地址>/32,DIRECT` 也命中（用地址匹配，不查名字）。**交给代理时，代理那一侧看到的是地址，不是名字**（M5-D16）——用代理的访问日志核对。注：`rurge dns lookup` 是另一个进程，它的 DNS 会话**不会**出现在运行中的 `rurge run` 的 `recent` 里，只用来看它引导解析出的地址 | `recent` 的 JSON；`nslookup` 或 `curl` 的输出；代理的访问日志；日志 | | | |

---

### macOS

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| M31 | **`reflect`。** 照 W42 做（`sudo ./rurge run …`；`curl --connect-timeout 3 http://192.0.2.10/`；`ping 192.0.2.10`；W42 里补的那一步 `1.1.1.1 = drop` 与经代理监听端口也做一次） | 同 W42；macOS 的应用防火墙缺省允许 ICMP，应当直接收到应答（记下有没有弹窗）；`tcpdump -i utunN -n icmp` 里看得到两次对调与 TTL；`1.1.1.1 = drop` 那一步同 W42 | 同 W42，用 `tcpdump` 代替 Wireshark | | | |
| M32 | **`reject` 与 `drop`。** 照 W43 做；W43 里补的 IPv6 那一步也做一次 | 同 W43；macOS 上 UDP 收到的 ICMP“管理上禁止”由系统转成下一次读写的错误，记下 `nslookup` 的反应；IPv6 那一步同 W43 | 同 W43 | | | |
| M33 | **443 以外的 QUIC 被挡。** 照 W44 做 | 同 W44 | 同 W44 | | | |
| M34 | **`PROTOCOL,STUN`。** 照 W45 做（Safari 或 Chrome） | 同 W45；记下浏览器与它用的 STUN 服务器 | 同 W45 | | | |
| M35 | **`adaptive` 与 `auto`。** 照 W46 做 | 同 W46；macOS 上 `curl` 的 HTTP/3 特性要先 `curl --version` 核对 | 同 W46 | | | |
| M36 | **按 DNS 服务器主机名的规则。** 照 W47 做 | 同 W47；`remoteAddr` 与 `recent` 对一遍（`dns lookup` 是另一个进程，只看它引导解析出的地址） | 同 W47 | | | |

---

### Linux

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| L33 | **`reflect`。** 照 W42 做（`sudo ./rurge run …`；`ping 192.0.2.10`；W42 里补的那一步 `1.1.1.1 = drop` 与经代理监听端口也做一次） | 同 W42；Linux 上没有默认的防火墙时直接收到应答；`tcpdump -i rurge -n icmp` 里看得到两次对调与 TTL；`rp_filter` 不影响这两种包（记下它开着时的行为）；`1.1.1.1 = drop` 那一步同 W42 | 同 W42，用 `tcpdump` | | | |
| L34 | **`reject` 与 `drop`。** 照 W43 做；W43 里补的 IPv6 那一步也做一次 | 同 W43；Linux 上 UDP 收到的 ICMP“管理上禁止”在下一次发送时以 `ECONNREFUSED` 一类报到，记下 `nslookup` 与 `curl` 的原话；IPv6 那一步同 W43 | 同 W43 | | | |
| L35 | **443 以外的 QUIC 被挡。** 照 W44 做 | 同 W44 | 同 W44 | | | |
| L36 | **`PROTOCOL,STUN`。** 照 W45 做（Firefox 或 Chromium） | 同 W45 | 同 W45 | | | |
| L37 | **`adaptive` 与 `auto`。** 照 W46 做 | 同 W46 | 同 W46 | | | |
| L38 | **按 DNS 服务器主机名的规则。** 照 W47 做 | 同 W47 | 同 W47 | | | |

---

### 与平台无关

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| P32 | **`reflect` 送回的包在真实系统上的去向与终止。** 三个平台各做一次：`[IP Rewrite]` 写 `192.0.2.10 = reflect` 与 `192.0.2.11 = reflect`；开着增强模式：(a) `ping 192.0.2.10`（防火墙按 W42 处理）；(b) `curl --connect-timeout 3 http://192.0.2.11/`；(c) 发一个 UDP 数据报到 `192.0.2.11`（`nslookup example.com 192.0.2.11`）；(d) 用 `ping -i <TTL>` 一类办法把原包的 TTL 调到很小，看回包的 TTL 与循环何时停（`reflect` 自己不改 TTL）；(e) 让一台机器空闲一分钟，看 CPU 与日志 | (a) 操作系统的 echo reply 又被送回、作为对原 echo request 的应答；`reflected` 涨了两个包；记下回包的 TTL 与直连时是否一样（`reflect` 不改 TTL）。(b) 操作系统回的 RST 又被送回，应用的连接以“拒绝”结束；RST 不再引发回应，所以停了。(c) 操作系统回的 ICMP 端口不可达又被送回；**不对差错回差错**，所以停了。(d) 记下回包在 Wireshark / `tcpdump` 里的 TTL 与直连时相比少了几次（`reflect` 只对调地址，TTL 应当不变）；TTL 小到 1 时照实记下循环怎样停。(e) 空闲一分钟里没有持续的包、CPU 不涨、日志没有新的 `reflected`/丢弃行。三个平台各记一份；与内存设备上的用例（设计 8.4、8.5）对一遍 | 三个平台的抓包；`reflected` 与 `ipRewrite`；日志；`top` / 任务管理器 | | | |

---

## M6　补齐

阶段 3 / M6 对着验收文档与第 14 节补齐的项：验收文档「真机上」一格原来写着“缺”的 27 行（3a 的 9 行、3b 的 9 行、任务 4 的 3 行、任务 5 的 6 行）与第 14 节第 1 条的三个平台各一项（W48、M37、L39）。每一项的开头写着它对着验收文档的哪几行（`#` 是那 27 行的序号）。这一节的新项盖 3a、3b 与任务 4 的 21 行（其中几行另有 M3 ～ M5 的旧项一起盖）；任务 5 的 6 行里，#96、#101、#106 由 M5 一节 W42、W43 一族加的步盖，#97 改成 `—`，#132、#133 由 M3 一节的 P19 ～ P21 盖。**一项也没有做过。**编号接着 M5 往下排（W48、M37、L39、P33），整份清单里的编号不重复。

会改系统设置的步骤（开增强模式、改路由与系统 DNS、改 `rp_filter`）按「怎么用」的「改系统设置的项与恢复」做：一次性的机器或能还原的虚拟机，Windows 要管理员、macOS 与 Linux 要 root。M4 一节的前置条件（二进制、`wintun.dll`、Profile）同样适用；下面只写每一项自己的前提。

### Windows

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| W48 | **开着增强模式，不经代理的浏览器、命令行工具与不支持代理的程序（第 14 节第 1 条，Windows）。** 系统代理不指向 rurge：不带 `--system-proxy`，Windows 的代理设置里手动代理关着，`$env:HTTP_PROXY`、`$env:HTTPS_PROXY`、`$env:ALL_PROXY` 是空的；浏览器的“安全 DNS”与 Firefox 的“基于 HTTPS 的 DNS”关掉（W18 说过它们绕开应答器）。`<conf>` 的 `[Rule]` 在 `FINAL,DIRECT` 之前写 `DOMAIN,example.com,DIRECT` 与 `DOMAIN,example.org,REJECT`；开增强模式，`rurge run -c <conf> --enhanced-mode --log-level debug`。(a) Edge、Chrome、Firefox 各打开 `http://example.com/` 与 `http://example.org/`；(b) `curl.exe http://example.com/ -o NUL -s -w "%{http_code}\n"` 与同样的 `http://example.org/`，不带 `-x`；(c) 不支持代理的程序：`Test-NetConnection example.com -Port 80`（TCP），以及 `nslookup -type=TXT example.com 9.9.9.9`（UDP，直接问一个公网的 DNS 服务器）；(d) 有可用的代理时（M5 前置条件里那一个），`[Proxy]` 写上它，`[Rule]` 加 `DOMAIN,example.net,<代理名>`，浏览器与 `curl.exe` 打开 `http://example.net/`；读 `GET /v1/requests/recent` | (a)(b)：`example.com` 打得开，`curl` 打印 `200`；`example.org` 打不开（浏览器的错误页，`curl` 以错误结束，把原话记下来）。每个连接在 `recent` 里一条：`listener` 是 `tun`；`dst` 是名字与端口（`example.com:80`、`example.org:80`）——应用拿到的是应答器给的 fake 地址，rurge 反查成名字；`rule` 是命中的那一行规则的原文（`DOMAIN,example.com,DIRECT`、`DOMAIN,example.org,REJECT`；原文来自 `crates/rurge-engine/src/engine.rs::rule_raw`）；`policy` 是 `DIRECT` 或 `REJECT`（JSON 里是数组）；`example.org` 那条的 `status` 是 `rejected`；`pid` 与 `processPath` 是发起连接的程序（浏览器的网络进程、`curl.exe`）。(c)：`Test-NetConnection` 的 `TcpTestSucceeded` 是 `True`，`recent` 里有一条 `dst` 为 `example.com:80`、`rule` 为 `DOMAIN,example.com,DIRECT` 的记录，`processPath` 是 PowerShell；`nslookup` 拿到 TXT，`recent` 里有一条 `transport` 为 `udp`、`dst` 为 `9.9.9.9:53`、`rule` 为 `FINAL,DIRECT` 的记录。(d)：`policy` 是那个代理的名字，代理那一侧的访问日志里有这次请求。这一项的记录里没有一条的 `listener` 是 `http` 或 `socks5` | `recent` 的 JSON；浏览器与命令的输出；代理的访问日志 | | | |
| W49 | **接管期间 `server:system` 问的是物理网卡的服务器（Windows；验收文档 #55 的 Windows 那一半）。看运行中的 `rurge run`，不用 `rurge dns lookup`——它是另一个进程，不在增强模式里。** `<conf>` 的 `[General]` 写 `dns-server = 9.9.9.9`（别的名字都问 `9.9.9.9`，日志里问物理网卡服务器的行就只来自下面这一条 `[Host]`）；`[Host]` 写 `example.com = server:system`（要一个真能解析的名字；`sys.test` 解析不出来）；`[Rule]` 是 `FINAL,DIRECT`；`rurge.exe run -c <conf> --enhanced-mode --log-level debug --log-file <日志>`。开增强模式之前记下物理网卡的 DNS 服务器（`Get-DnsClientServerAddress`）。(a) `curl.exe -x http://127.0.0.1:6152 http://example.com/ -o NUL -s -w "%{http_code}\n"`（经 HTTP 监听端口），再 `curl.exe http://example.com/ -o NUL -s -w "%{http_code}\n"`（不经代理，经虚拟网卡）；读 `GET /v1/requests/recent` 与日志。(b) 先停下 rurge，把物理网卡之外能问的服务器都拿掉：每块带服务器的网卡都 `Set-DnsClientServerAddress -InterfaceAlias <网卡> -ServerAddresses 198.18.0.2`，直到 `Get-DnsClientServerAddress` 里除了 `198.18.0.0/15` 没有别的（IPv4 与 IPv6 两族都看）；再起 rurge（开增强模式），再做一次 (a) 里经 HTTP 监听端口那一条。恢复：`Set-DnsClientServerAddress -InterfaceAlias <网卡> -ResetServerAddresses`（回到 DHCP），或用 `-ServerAddresses` 写回记下的地址 | (a) 两次都打印 `200`；两条记录（`listener` 分别是 `http` 与 `tun`）的 `dst` 是 `example.com:80`，`status` 是 `completed`，`remoteAddr` 是真地址（不在 `198.18.0.0/15` 里）；DEBUG 日志里 target 为 `rurge_dns::fanout` 的 `send` 与 `answer` 两种行，`upstream` 是 `udp://<物理网卡的服务器>:53`（`answer` 的 `rcode` 是 `NOERROR`、`records` 不是 0）；没有 `upstream` 是 `udp://198.18.0.2:53` 的行。物理网卡的服务器是各网卡的 DNS 去掉 rurge 的地址空间（`crates/rurge-platform/src/dns.rs::servers`；`crates/rurge-platform/src/enhanced/windows.rs::physical_dns` 回 `None`，rurge 只改虚拟网卡的 DNS）。(b) `curl` 打印 `502`（HTTP 监听端口在拨号失败时缺省回 502，`crates/rurge-inbound/src/http.rs`）；记录的 `status` 是 `failed`，`error` 里有 `all upstreams failed (system DNS servers: no DNS server outside the enhanced mode)`（`crates/rurge-dns/src/resolver.rs::no_server_outside`）；日志里没有为它发出的 `send` 行；没有退回系统解析库（`remoteAddr` 是 `null`） | `recent` 的 JSON；DEBUG 日志；`curl` 的输出 | | | |
| W50 | **`external` 的 `addresses` 是路由里的洞（Windows；验收文档 #130）。** `[Proxy]` 写 `Helper = external, exec = <一个会自己发起连接的程序>, local-port = 23456, addresses = 1.1.1.1, addresses = 9.9.9.9`（多个地址写成重复的 `addresses` 键；写成逗号分隔的一项会把后面的地址读成位置参数）；`rurge.exe check -c <conf>` 应当没有 `W0029`；开着增强模式，`Find-NetRoute -RemoteIPAddress 1.1.1.1` 与 `Find-NetRoute -RemoteIPAddress 9.9.9.9`，再对一个不在 `addresses` 里的公网地址（例如 `8.8.4.4`）做一次对照 | `1.1.1.1` 与 `9.9.9.9` 的 `InterfaceAlias` 是物理网卡（不是 `rurge`）、`NextHop` 是物理网络的网关；对照的 `8.8.4.4` 的 `InterfaceAlias` 是 `rurge`。这就是 `RouteSet::compute` 给每个 `addresses` 留的 `/32` 的洞（`crates/rurge-platform/src/route/set.rs::RouteSet::compute` 与 `crates/rurge/src/cli/enhanced/wanted.rs::wanted`）；应用与 rurge 自己的连接都照这条路由走。把每条命令的输出抄下来 | `Find-NetRoute` 的输出；`rurge check` 的输出 | | | |

### macOS

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| M37 | **开着增强模式，不经代理的浏览器、命令行工具与不支持代理的程序（第 14 节第 1 条，macOS）。** 照 W48 做：系统设置的“网络 → 代理”全关，`curl` 不带 `-x`，`env \| grep -i proxy` 是空的；浏览器是 Safari、Chrome、Firefox；(c) 是 `nc -vz example.com 80` 与 `dig @9.9.9.9 example.com TXT`；(d) 同 W48 | (a)(b)(c)(d) 同 W48；`processPath` 是 `/usr/bin/curl`、`/usr/bin/nc` 一类；记录里没有一条的 `listener` 是 `http` 或 `socks5` | `recent` 的 JSON；浏览器与命令的输出；代理的访问日志 | | | |
| M38 | **不以 root 运行（macOS；验收文档 #13 的 macOS 那一半）。** 以普通用户（不是 root、也不 `sudo`）运行 `./rurge run -c <conf> --enhanced-mode`，看标准错误与退出码 | 标准错误 `error: cannot turn the enhanced mode on: the enhanced mode needs root`，以 1 退出；没有建网卡、没有改路由与系统 DNS（`ifconfig -l` 里没有新的 `utunN`、`networksetup -getdnsservers` 不变）。这一句是 `crates/rurge-platform/src/tun/frame.rs::explained` 给 macOS 的 `PermissionDenied` 的那一句；`crates/rurge-platform/src/enhanced/macos.rs::open` 先清掉上一次留下的临时文件，再开设备 | 标准错误；退出码；`ifconfig -l`；`networksetup` | | | |
| M39 | **`external` 的 `addresses` 是路由里的洞（macOS；验收文档 #130）。** `[Proxy]` 写 `Helper = external, exec = <一个会自己发起连接的程序>, local-port = 23456, addresses = 1.1.1.1, addresses = 9.9.9.9`（多个地址写成重复的 `addresses` 键）；`./rurge check -c <conf>` 应当没有 `W0029`；开着增强模式，`route -n get 1.1.1.1` 与 `route -n get 9.9.9.9`，再对一个不在 `addresses` 里的公网地址（例如 `8.8.4.4`）做一次对照 | `1.1.1.1` 与 `9.9.9.9` 的 `interface:` 是物理网卡（`en0` 一类，不是 `utunN`）、`gateway:` 是物理网络的网关；对照的 `8.8.4.4` 的 `interface:` 是 `utunN`。这就是 `RouteSet::compute` 给每个 `addresses` 留的 `/32` 的洞（`crates/rurge-platform/src/route/set.rs::RouteSet::compute` 与 `crates/rurge/src/cli/enhanced/wanted.rs::wanted`）；应用与 rurge 自己的连接都照这条路由走。把每条命令的输出抄下来 | `route` 的输出；`rurge check` 的输出 | | | |

### Linux

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| L39 | **开着增强模式，不经代理的浏览器、命令行工具与不支持代理的程序（第 14 节第 1 条，Linux）。** 照 W48 做：浏览器的代理设置关着，`curl` 不带 `-x`，`env \| grep -i proxy` 是空的；浏览器是 Firefox、Chromium；(c) 是 `nc -vz example.com 80`（没有 `nc` 的用 `bash -c 'exec 3<>/dev/tcp/example.com/80'`）与 `dig @9.9.9.9 example.com TXT`；(d) 同 W48 | (a)(b)(c)(d) 同 W48；`processPath` 是 `curl`、`nc` 一类的真实路径；记录里没有一条的 `listener` 是 `http` 或 `socks5` | `recent` 的 JSON；浏览器与命令的输出；代理的访问日志 | | | |
| L40 | **不以 root 运行（Linux；验收文档 #13 的 Linux 那一半）。** (a) 以普通用户运行 `./rurge run -c <conf> --enhanced-mode`；(b) 再在一台没有 `ip` 的环境里做一次——`crates/rurge-platform/src/route/linux.rs::LinuxTools::find` 只在 `/usr/sbin`、`/sbin`、`/usr/bin`、`/bin` 四个目录里找，不看 `PATH`，所以要用没有装 iproute2 的最小容器或虚拟机（`ls /usr/sbin/ip /sbin/ip /usr/bin/ip /bin/ip` 都不存在），不是改 `PATH` | (a) 标准错误 `error: cannot turn the enhanced mode on: the enhanced mode needs root (in a container, root with CAP_NET_ADMIN)`，以 1 退出。普通用户打得开 `/dev/net/tun`，失败在 `set_iff`（`ioctl(TUNSETIFF)`）没有 `CAP_NET_ADMIN`，`frame::explained` 把 `PermissionDenied` 写成那一句；没有建网卡（`ip link` 里没有 `rurge`）、没有改路由与系统 DNS。(b) 标准错误 `error: cannot turn the enhanced mode on: cannot find the ip program (iproute2) in /usr/sbin, /sbin, /usr/bin or /bin`，以 1 退出——`crates/rurge-platform/src/enhanced/linux.rs::open` 在碰设备之前先查 `ip`，所以这一句不是权限那一句。两种都不留改动 | 标准错误；退出码；`ip link` | | | |
| L41 | **接管期间 `server:system` 问的是物理网卡的服务器（Linux；验收文档 #55 的 Linux 那一半）。看运行中的 `rurge run`，不用 `rurge dns lookup`——它是另一个进程，不在增强模式里。** 对照 W49 做，平台不同的地方：`[General]` 的 `dns-server = 9.9.9.9`；`[Host]` 的 `example.com = server:system`；在有 systemd-resolved 的机器上，开增强模式之前记下 `resolvectl dns` 与 `/run/systemd/resolve/resolv.conf` 的服务器。(a) 两条 `curl` 同 W49（`-o /dev/null`）。(b) 先停下 rurge；`sudo resolvectl dns <物理链路> 198.18.0.2`，`resolved.conf` 里配了全局 `DNS=` 的也拿掉，直到 `/run/systemd/resolve/resolv.conf` 里只剩 `198.18.0.0/15` 里的；再起 rurge，再做一次经 HTTP 监听端口那一条。恢复：`sudo resolvectl revert <物理链路>`，再让管网络的程序重新下发——NetworkManager 是 `sudo nmcli device reapply <链路>`，systemd-networkd 是 `sudo networkctl reconfigure <链路>` | 同 W49。(a) 的物理上游是 `/run/systemd/resolve/resolv.conf` 去掉 rurge 的地址空间（`crates/rurge-platform/src/enhanced/linux.rs::physical_dns`）；(b) 的 `curl` 打印 `502`，记录 `status` 是 `failed`，`error` 里有 `all upstreams failed (system DNS servers: no DNS server outside the enhanced mode)`，`remoteAddr` 是 `null` | `recent` 的 JSON；DEBUG 日志；`curl` 的输出 | | | |
| L42 | **`external` 的 `addresses` 是路由里的洞（Linux；验收文档 #130）。** `[Proxy]` 写 `Helper = external, exec = <一个会自己发起连接的程序>, local-port = 23456, addresses = 1.1.1.1, addresses = 9.9.9.9`（多个地址写成重复的 `addresses` 键）；`./rurge check -c <conf>` 应当没有 `W0029`；开着增强模式，`ip route get 1.1.1.1` 与 `ip route get 9.9.9.9`，再对一个不在 `addresses` 里的公网地址（例如 `8.8.4.4`）做一次对照 | `1.1.1.1` 与 `9.9.9.9` 的行是 `dev <物理网卡>`（不是 `dev rurge`）；对照的 `8.8.4.4` 是 `dev rurge`。这就是 `RouteSet::compute` 给每个 `addresses` 留的 `/32` 的洞（`crates/rurge-platform/src/route/set.rs::RouteSet::compute` 与 `crates/rurge/src/cli/enhanced/wanted.rs::wanted`）；应用与 rurge 自己的连接都照这条路由走。把每条命令的输出抄下来 | `ip route get` 的输出；`rurge check` 的输出 | | | |

### 与平台无关

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| P33 | **应答器的地址与地址族（验收文档 #57、#58、#60、#63）。三个平台各做一次。** 开增强模式（`[General]` 写 `ipv6-vif = always`；网络有 IPv6 时 `auto` 也行）。用 `dig`（macOS / Linux）或 `nslookup` / `Resolve-DnsName`（Windows）对 `198.18.0.2` ～ `198.18.0.9` 各问一次 A 与 AAAA，再对 `fd00:6152::2` 各问一次 A 与 AAAA：同一个名字（`example.com`）先记下它的 A，再用同一个名字问 AAAA | `.2` ～ `.9` 每一个的 A 都得到 fake 地址（`198.18.x.y`，TTL 30），AAAA 都是 NOERROR、没有记录；`fd00:6152::2` 的 AAAA 得到 `fd00:6152::` 开头的地址，A 是 NOERROR、没有记录。`fd00:6152::2` 的 AAAA 与 `.2` 的 A 是同一个编号：IPv6 地址的高 64 位是 `fd00:6152::`，低 32 位就是那个 IPv4 地址的四个字节（例如 A 是 `198.18.1.1` 时 AAAA 是 `fd00:6152::c612:101`，`crates/rurge-dns/src/fakeip/mod.rs::v6_of`）。`dig` 的 `ANSWER: 0`、`status: NOERROR`；`nslookup` 打印没有记录而不是超时或 NXDOMAIN | `dig` / `nslookup` 的输出；`GET /v1/features/enhanced_mode` 的计数（不应因这些查询涨 `dropped`） | | | |
| P34 | **fake 地址跨重启（验收文档 #61）。三个平台各做一次。** 用 `--data-dir <一个空目录>` 开增强模式；`dig @198.18.0.2 persist.test`（Windows 用 `nslookup`）记下地址；正常停下 rurge（Ctrl-C 或 `rurge stop`）；看数据目录里的 `fake-ip.txt`；再起一次（同一个数据目录），再问一次同一个名字。再做一遍强杀的：同样拿到地址之后**等 10 秒以上**（池的文件写一次之后 10 秒之内不再写，`crates/rurge-engine/src/tun.rs` 的 `POOL_SAVE_EVERY` 与 `crates/rurge-dns/src/fakeip/mod.rs::FakeIpPool::keep_saved`），再 `kill -9` / `taskkill /F`；强杀之后、再起之前看一次文件，再起一次看日志与文件，再问一次同一个名字。`fake-ip.txt` 都在 rurge 停着时看 | 第二次的地址与第一次相同。`fake-ip.txt` 第一行是 `rurge-fake-ip 1`，第二行是 `cursor <n> wrapped <0|1> clean 1`（正常停下之后 `clean` 是 1），后面有一行 `<编号> persist.test`；文件名是 `crates/rurge-dns/src/fakeip/mod.rs` 的 `FILE_NAME`，格式是 `crates/rurge-dns/src/fakeip/file.rs` 的 `render`。强杀之后、再起之前第二行是 `clean 0`；再起时日志 INFO 一行 `fake-ip: rurge did not stop cleanly last time; <N> fake addresses are held back for <S> seconds`（`crates/rurge-engine/src/tun.rs`，`N` 是 `QUARANTINE_COUNT`、`S` 是 `QUARANTINE` 的秒数）；再起之后文件随即被重写，第二行仍是 `clean 0`，正常停下之后才是 `clean 1`。同一个名字仍应当拿回同一个地址——把两种停下方式都做一次 | 两次查询的输出；`fake-ip.txt` 的内容；日志 | | | |
| P35 | **`always-real-ip` 与 `[Host]` 的权威应答（验收文档 #66、#67、#114、#119）。三个平台各做一次。** `[Host]` 里写 `real.test = 203.0.113.7, 2001:db8::7`，`[General]` 里写 `always-real-ip = real.test` 与 `ipv6-vif = always`；开增强模式；`dig @198.18.0.2 real.test A`、`dig @fd00:6152::2 real.test AAAA`（Windows 用 `nslookup` / `Resolve-DnsName`） | 两个应答的 AA 位都置位（`dig` 的 `flags: qr aa rd ra`），A 只有 `203.0.113.7` 一条、AAAA 只有 `2001:db8::7` 一条，TTL 30；地址来自 `[Host]`，没有从上游转发（DEBUG 的 `tun: DNS query` 里 `decision` 是 `always-real-ip: the addresses [Host] maps`）。`[Host]` 写了两种地址，A 的问只回 IPv4、AAAA 的问只回 IPv6（`crates/rurge-dns/src/message.rs::answer`）；往 IPv4 的应答器问 AAAA 是空应答，那是「地址族一致」（P33） | `dig` / `nslookup` 的输出；DEBUG 日志 | | | |
| P36 | **UDP 流被规则拒绝（验收文档 #89、#90、#91）。三个平台各做一次。** `tun-included-routes = 198.51.100.0/24`；`[Rule]` 分别写 `IP-CIDR,198.51.100.0/24,REJECT`、`IP-CIDR,198.51.100.0/24,REJECT-NO-DROP`、`IP-CIDR,198.51.100.0/24,REJECT-DROP`（一次一条，写在 `FINAL` 之前）；开增强模式，每一条都跑一次 `nslookup example.com 198.51.100.10`（或 `nc -u` 发一个数据报），并抓虚拟网卡的包；前后读计数 | `REJECT` 与 `REJECT-NO-DROP`：应用立刻失败（Linux / macOS 上常见 `ECONNREFUSED` 一类，Windows 上 `nslookup` 可能只显示超时），抓包看到 ICMPv4 的 type 3 code 13（“管理上禁止”），引用的是刚发的那个 UDP 首部，来源是 `198.51.100.10`；`REJECT-DROP`：应用等到自己的超时，抓包里没有任何回包。三条都在 `GET /v1/requests/recent` 里留一条 `transport` 为 `udp` 的记录；`icmpLimited` 在一次数据报时不涨（`REJECT` 的限频是每 3 秒 100 个，`REJECT-NO-DROP` 不限频，`REJECT-DROP` 不回也不计数——`crates/rurge-tun/src/udp/client.rs::TunUdpClient::refused` 与 `crates/rurge-tun/src/icmp.rs::prohibited`） | 抓包；`nslookup` / `nc` 的输出；计数；`recent` | | | |
| P37 | **SYN 层被预匹配拒绝（验收文档 #86、#87）。三个平台各做一次。** `tun-included-routes = 198.51.100.0/24,203.0.113.0/24`；`[Rule]` 写 `IP-CIDR,198.51.100.0/24,REJECT-DROP,pre-matching` 与 `IP-CIDR,203.0.113.0/24,REJECT-NO-DROP,pre-matching`；开增强模式，`curl --connect-timeout 3 http://198.51.100.10/` 与 `curl --connect-timeout 3 http://203.0.113.10/`；读日志与 `recent`；抓包 | `REJECT-DROP`：`curl` 3 秒超时、连接挂住，抓包里没有 SYN-ACK、没有 RST，只有重发的 SYN；DEBUG 一行 `tun: a SYN is dropped, as a pre-matching rule says`（`src`、`dst`、`name`）。`REJECT-NO-DROP`：`curl` 立刻以“连接被复位”结束，抓包看到 RST。两条都在 `recent` 里留一条 `notes` 含 `pre-matching at SYN` 的记录（五分钟一条）；**规则用 IP 字面量、不用名字**，否则名字会在 DNS 层先被拒，看到的不是 SYN 层（`crates/rurge-tun/src/tcp/shard.rs::refused_by_rules`、`crates/rurge-engine/src/prematch.rs`） | `curl` 的输出；抓包；日志；`recent` | | | |
| P38 | **每会话流量统计（验收文档 #23）。三个平台各做一次。** 在 `<服务器>` 上放一个已知大小的文件（例如 `python3 -m http.server` 与一个 10 MiB 的文件）；`<服务器>` 见 M3 一节的前置条件，它的地址要写进 `tun-included-routes`（否则不进虚拟网卡）。开增强模式，先读一次 `GET /v1/traffic`；经虚拟网卡 `curl -o /dev/null http://<服务器>/<文件>`（Windows `NUL`）；等 `curl` 结束、记录里的 `status` 是 `completed` 之后，再读一次 `GET /v1/traffic` 与 `GET /v1/requests/recent?limit=20` | 那条会话的 `listener` 是 `tun`；`recent` 里的 `down` 至少是文件大小，比它多出响应头的字节数（`Content-Length` 与实际响应对一遍）；`up` 是请求头的字节数。`GET /v1/traffic` 的 `listener` 与 `connector` 两张表只数结束了的会话，`total` 连正在进行的也数（`crates/rurge-api/src/routes/traffic.rs` 的注释）；增强模式开着时整台机器的流量都经虚拟网卡，同一段时间里别的会话也在结束，所以 `listener.tun.in` / `listener.tun.out` 的增量**不小于**这条会话的 `down` / `up`，不是等于；要对得上就把两次读之间结束的会话从 `recent` 里加起来对，或先关掉别的联网程序。`total.in` / `total.out` 也增加。把这条会话的四个数与文件大小对一遍（`crates/rurge-engine/src/relay.rs::pump` 的 `add_up` / `add_down`） | `recent` 的 JSON；`GET /v1/traffic` 前后各一份；文件大小 | | | |
| P39 | **用规则集做预匹配（验收文档 #122）。三个平台各做一次。** 在 Profile 旁边写一个规则集文件 `ads.list`，内容两行：`DOMAIN-SUFFIX,x.ads.test` 与 `IP-CIDR,198.51.100.0/24,no-resolve`；`[Rule]` 在 `FINAL` 之前写 `RULE-SET,ads.list,REJECT,pre-matching`；`tun-included-routes = 198.51.100.0/24`；开增强模式，`dig @198.18.0.2 x.ads.test` 与 `curl --connect-timeout 3 http://198.51.100.10/`；读 DEBUG 日志与 `recent` | DNS 查询得到空应答（NOERROR、没有记录），连接立刻被 RST；`recent` 里两条记录的 `notes` 都有 `Sub-rule matched: … (in .\ads.list)`（Windows；macOS 与 Linux 是 `./ads.list`，集合名是路径的显示，`crates/rurge-rules/src/registry.rs` 里 `Key::File(p) => p.display().to_string()`；Profile 在别的目录时是那个目录加文件名），一条另有 `pre-matching at DNS`、另一条 `pre-matching at SYN`；DEBUG 日志有同一句 `Sub-rule matched: … (in .\ads.list)`（带 `session`）。集合里的行相对 Profile 的目录写成 `ads.list`（`crates/rurge-engine/src/engine.rs::note_sub_rule`、`crates/rurge-rules/src/engine.rs::RuleEngine::pre_match`） | DEBUG 日志；`recent`；`dig` 与 `curl` 的输出 | | | |
| P40 | **`icmp-forwarding = false`（验收文档 #10、#111 的关掉那一半）。三个平台各做一次。** `[General]` 写 `icmp-forwarding = false`，重启并开增强模式；`ping example.com` 与 `ping 1.1.1.1`；读计数；再把 `icmp-forwarding` 改回 `true`，`POST /v1/profiles/reload`（或重启），再 ping 两个 | 关着时两个 `ping` 都收不到应答（等到自己的超时），`GET /v1/features/enhanced_mode` 的 `dropped.unsupported` 按发出的回显请求数增加（`crates/rurge-tun/src/icmp.rs::receive` 在 `icmp_forwarding` 为假时丢并计 `Unsupported`）；改回 `true` 并重载之后的下一个请求就有应答。把两次 ping 的输出与 `unsupported` 的前后差记下来 | `ping` 的输出；`GET` 前后 | | | |
| P41 | **`subnet-exp-wifi-always-match = false`（验收文档 #116 的关掉那一半）。** 一台有线与 Wi-Fi 同时连着、主网卡是有线的机器；**三个平台各做一次（要有线能同时连上；做不到的平台照实记下）**；`<B>` 是从 DEBUG 的 `where the machine is now` 抄下的 BSSID。`[Proxy Group]` 里分别做四次（一次一个组，`FINAL` 指到它）：`Net1 = subnet, SSID:<A> = DIRECT, default = REJECT`；`Net2 = subnet, BSSID:<B> = DIRECT, default = REJECT`；`Net3 = subnet, <A> = DIRECT, default = REJECT`；`Net4 = subnet, TYPE:WIRED = REJECT, ROUTER:<有线网关> = REJECT-NO-DROP, default = REJECT-DROP`。前三次在 `[General]` 写 `subnet-exp-wifi-always-match = false`；把 `Net1` 的 `false` 改成 `true`（或删掉）再做一次作对照；`Net4` 再把 `ROUTER:` 放到 `TYPE:WIRED` 前面做一次。再拔掉网线，让 Wi-Fi 成为主网卡（日志 INFO `network changed` 的 `kind` 是 `wifi`，字段原文在 `crates/rurge/src/cli/run.rs`），`subnet-exp-wifi-always-match = false` 不变，`Net1` 再做一次 | 设置 `false` 时 `Net1`、`Net2`、`Net3` 都落到 `default`（`REJECT`），即 `SSID:`、`BSSID:` 与裸值不再算——Wi-Fi 还连着，但不是主网卡；`Net4` 的 `REJECT`（`TYPE:WIRED`）命中，把 `ROUTER:` 放在它前面时 `REJECT-NO-DROP`（`ROUTER:`）命中；设置 `true` 时 `Net1` 是 `DIRECT`（`SSID:<A>` 命中，即使主网卡是有线）；拔掉网线、Wi-Fi 成为主网卡时，`false` 下 `Net1` 也是 `DIRECT`（`primary.interface == wifi.interface` 这一支照样算）。这就是 `crates/rurge-config/src/policy.rs::SubnetExpr::matches`（`wifi_always_match || primary.interface == wifi.interface`）与 `crates/rurge-policy/src/registry.rs::a_subnet_group_reads_subnet_exp_wifi_always_match` 的两半 | 当前选择（`GET /v1/policy_groups/select?group_name=…`）；日志里的 `kind` 与 `interface` | | | |

