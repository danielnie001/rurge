# rurge HTTP API（阶段 3）

阶段 3 / M1（网络环境与子网，设计 `docs/superpowers/specs/2026-10-16-phase3-m1-network-subnet-design.md`，计划 `docs/superpowers/plans/2026-10-16-phase3-m1-network-subnet-plan.md`）**没有新增端点，也没有改任何响应的形状**。变的是三个已有端点的行为——`POST` / `GET /v1/policy_groups/select` 对 `subnet` 组、`GET` / `POST /v1/features/system_proxy` 在接管暂停时、`GET /v1/dns` 的 `upstreams` 随网络的 DNS 覆盖——，和一组日志行。没有任何端点给出本机当前所在的网络：SSID、BSSID、网关和本机地址说明使用者在哪里，只在 DEBUG 日志里出现（设计 M1-D16）。鉴权（`X-Key`）、错误体（`{"error":"<message>"}`）与无内容的成功响应（`{}`）沿用阶段 1 的约定，见 `docs/api/phase1.md`；本页只记录变了的部分。

## 端点

| 方法 | 路径 | 阶段 3 / M1 的变化 |
| --- | --- | --- |
| GET | `/v1/policy_groups` | 形状不变。`subnet` 组的成员表是它的条件里的策略（按书写顺序）再加 `default`，每个策略一次；**不带当前选择**，当前选择用下一行读 |
| GET | `/v1/policy_groups/select?group_name=<subnet 组>` | `{"policy": "<当前选择>"}`：临时覆盖优先，其次是第一个命中当前网络的条件的策略，都不命中是 `default` |
| POST | `/v1/policy_groups/select` | 对 `subnet` 组设置或清除临时覆盖，同自动组；**换网络时清除**。此前对 `subnet` 组答 400 的那一行取消了 |
| GET | `/v1/features/system_proxy` | 接管暂停期间（`[SSID Setting]` 的 `suspend=true`）给出真实状态，即 `{"enabled": false}` |
| POST | `/v1/features/system_proxy` | 接管暂停期间返回成功 `{}`，但什么也不设，只记下想要的状态，暂停结束时照最后一次办 |
| GET | `/v1/dns` | 形状不变；`upstreams` 是正在用的那一代的上游，所在网络的 `[SSID Setting]` 写了 `dns-server` / `encrypted-dns-server` 时是覆盖后的 |
| POST | `/v1/profiles/check`、`/v1/profiles/reload` | 没写 `default` 的 `subnet` 组是加载错误 `E0018`，`[SSID Setting]` 有了自己的诊断（见下面「Profile 检查里的诊断」） |

## `subnet` 组

### 选择

`subnet` 组按当前网络选成员（`PolicyRegistry` 的 `GroupKind::Subnet`，`crates/rurge-policy/src/registry.rs`），每次选择都读当前网络，组自己不存状态：

1. 有临时覆盖时用它；
2. 否则是**第一个**命中当前网络的条件的策略（按书写顺序，条件的写法与求值见 `docs/surge-compatibility-matrix.md` 3.4 节）；
3. 都不命中，或者当前网络是"什么都不知道"（还没读到、读不到，测试里文件后端指着的文件不在）时用 `default`。

`cellular` 在桌面上从不生效（读了，不起作用）。条件里的策略在注册表里找不到（加载时已经说过了）时，跳过那一条。`default` 必填：没写（或写成空）是加载错误 `E0018`（见下面）。

### `GET /v1/policy_groups`

形状同别的组（`docs/api/phase2.md` 的「`GET /v1/policy_groups`」）。例：

```
Net = subnet, SSID:Home = HK, TYPE:WIRED = DIRECT, default = Proxy
```

```json
{
  "Net": [
    {"name": "HK", "typeDescription": "http", "isGroup": false, "enabled": true, "lineHash": "3f2a9c1d4b5e6a7f"},
    {"name": "DIRECT", "typeDescription": "DIRECT", "isGroup": false, "enabled": true, "lineHash": "9b7e1c4a2f6d8035"},
    {"name": "Proxy", "typeDescription": "select", "isGroup": true, "enabled": true, "lineHash": "5c0e7a1b9d3f2468"}
  ]
}
```

成员表是"条件里的策略，再 `default`，去重"：同一个策略写在两个条件里、或与 `default` 相同，只列一次。成员表用在 `GET /v1/policy_groups`、临时覆盖的成员检查和组环的检测上。**当前选择不在这个响应里**——响应的形状对所有类型的组都一样，要看当前选了谁用 `GET /v1/policy_groups/select`。

### `GET /v1/policy_groups/select`

参数 `group_name`（必填）。对 `subnet` 组：

```json
{"policy": "HK"}
```

家里的 Wi-Fi 上是 `HK`，插上网线之后（`TYPE:WIRED` 命中）是 `DIRECT`，哪个条件都不命中时是 `Proxy`。读取不触发任何东西。

### `POST /v1/policy_groups/select`

请求体同别的组：`{"group_name": "Net", "policy": "DIRECT"}`，成功 `{}`。对 `subnet` 组这是**临时覆盖**：

| 情形 | 结果 |
| --- | --- |
| `policy` 是该组成员表里的名字 | 200 `{}`；从**下一条**连接起用它，正在进行的连接不受影响 |
| `policy` 是空字符串 | 200 `{}`；清除覆盖，恢复按网络选 |
| `policy` 不是该组的成员 | 400 `` `Somewhere` is not a member of `Net` `` |
| `group_name` 不是任何已知策略组 | 400 `` unknown policy group `Net-typo` `` |

覆盖的寿命：

- 不写 `state.json`，进程重启后不在（不像 `select` 组的选择）。
- 组定义（不计行位置）不变的重载保留它；组消失或定义变了就清除。
- **换网络时清除。** `subnet` 组按网络选，覆盖说的是"在这个网络上先用它"，所以网络一变（见下面「网络变化」）它就让位给按新网络选。这是 rurge 的读法：手册只说自动类型的组可以临时覆盖，没有写换网络会怎样。
- `subnet` 组从不测速：能覆盖和会不会测速是两个判断。`GET /v1/policy_groups/test_results` 不列它。

## 网络变化

`rurge run` 起一个网络任务：启动时（建第一代之前）读一次网络，之后在后台盯着，网络**变了**（主网卡、默认网关、网卡类型、Wi-Fi 的名字或接入点、系统 DNS 服务器或主网卡的地址任何一项不同）就依次做三件事，由主循环一次一个地做，与重载、API 的命令不交叉：

1. **引擎处理**（`Engine::network_changed`，同步）：
   - 存下新的网络——之后的规则求值与 `subnet` 组的选择看它；
   - 正在用的这一代的解析器清 DNS 缓存、解除 AAAA 抑制、重读系统的 DNS 服务器；
   - 注册表里每个出站收到 `network_changed()`，只有 `wireguard` 做事：给每个 peer 换新的载体、重新握手；
   - `url-test`、`fallback`、`load-balance` 组上一轮测试的时间作废，下一次使用时重测（结果本身留着，新一轮测完之前照旧用；`evaluate-before-use` 的组下一次使用时等这一轮；换网络那一刻正在跑的一轮不算新网络上的一轮）；`smart` 组不动；
   - `subnet` 组的临时覆盖清除；
2. **DNS 覆盖变了就重建一代**：新的网络让生效的 `[SSID Setting]` 行的 DNS 覆盖与这一代建的时候不同时，用**内存里的配置**重建一代并换上（不重读磁盘上的 Profile——网络变化不该顺带让使用者没让重载的改动生效；`select` 组的选择照旧）；
3. **接管暂停的状态变了就暂停或恢复系统代理**（见下一节）。

正在进行的会话与查找照旧用它们开始时的东西完成，不被打断（读代码得出；没有用例持着一个会话跨过一次换网络）。

怎样发现变化：Windows 与 Linux 每 2 秒比一次便宜的指纹（Windows 是 `ipconfig` 包的网卡表，Linux 是 `/proc/net/route`、`/proc/net/ipv6_route`、`/sys/class/net`、网卡地址与 `resolv.conf`），不起子进程；问 Wi-Fi 的名字要起子进程（`netsh`、`nmcli` / `iw`），只在指纹变了时问，以及有无线网卡开着时每 30 秒问一次（同一个 SSID 下换接入点、地址不变）。macOS 听一个 `PF_ROUTE` 套接字（只看地址增删、网卡状态与路由的增删改，滤掉 ARP 一类的噪声，一阵消息合成一次），另外每 30 秒兜底读一次。变化经 1 秒的"首发加尾发"去抖：第一个变化立刻处理并开一个 1 秒的窗口，窗口里再来的变化合并成窗口结束时的一次，用最新的网络；一个回到原样的网络不算变化。每个读网络的子进程限时 2 秒，读不到的当作没有，不报错。

SSID 与 BSSID 可能读不到（Windows 没有位置权限，据调研以 SYSTEM 运行时也一样；macOS 14 起的守护进程；Linux 上 NetworkManager 与 `iw` 都说不出）：这时 `SSID:` / `BSSID:` 的条件从不命中、不报错，`TYPE:` 与 `ROUTER:` 照常，原因在 DEBUG 日志里说一次（不是每次变化都说）。

## 接管暂停时的系统代理端点

生效的 `[SSID Setting]` 行写了 `suspend=true` 时，接管暂停（代码里叫 takeover pause；与托管配置 `strict` 到期时引擎的 `suspend`——拒绝新会话——是两回事）：

- 开着的系统代理恢复成使用者原来的设置（同关闭系统代理，`state.json` 里的备份随之清掉），并记下"暂停之前是开着的"；
- 监听器与 API 照常，经代理端口来的请求照常处理——它们是使用者明确指过来的；
- 离开这个网络（生效的那一行不再写 `suspend=true`）时，想要开着的重新指向 rurge；
- 增强模式的那一半（撤掉虚拟网卡）随阶段 3 / M4，今天没有。

暂停期间这两个端点这样答：

| 请求 | 暂停期间 |
| --- | --- |
| `GET /v1/features/system_proxy` | `{"enabled": false}`——真实状态，系统代理此刻没有指向 rurge。**应答里没有任何字段说"暂停中"**，要知道用下面的日志行 |
| `POST /v1/features/system_proxy` `{"enabled": true}` | 返回成功 `{}`，但什么也不设；只记下"想要开着"，暂停结束时照办。日志里一行 INFO |
| `POST /v1/features/system_proxy` `{"enabled": false}` | 返回成功 `{}`；记下"不想要"，暂停结束时就不再打开 |

要点：

- "想要的状态"由三处决定，最后一次生效：启动参数 `--system-proxy`、暂停开始前系统代理是不是开着、暂停期间每一次得到成功应答的 `POST`。
- 启动时就在这样的网络上，`--system-proxy` 只记下想要的，不设系统代理，标准输出里也没有 `system proxy enabled:` 那一行；离开这个网络时才设。
- 暂停只在内存里，不进 `state.json`。进入暂停时备份已经清掉，所以暂停期间 rurge 崩溃，下一次启动没有东西要恢复——系统代理早已是原来的样子。
- 一次成功的重载之后会再问一遍"现在暂停吗"：重载可能加上或拿掉了 `suspend=true` 那一行。
- 暂停结束时重新指向 rurge 失败（比如那时没有可用的 http / socks5 监听器）只记一条 ERROR，不重试，直到下一次进出暂停或下一次 `POST`。

## `GET /v1/dns` 与 `[SSID Setting]` 的 DNS 覆盖

所在网络的 `[SSID Setting]` 行（几行同时命中时取第一行）写了 `dns-server` 或 `encrypted-dns-server` 时，它们**替换**（不是合并）Profile 全局的同名列表：没写的那一项保持全局的；`encrypted-dns-server=off` 去掉加密 DNS（全局配了加密 DNS、这个网络要用传统 DNS 时必须写它）；`dns-server` 下写的加密 URL 照 `[General]` 的读法算加密服务器；`[Host]` 里 `server:` 写明的服务器不受影响；`rurge dns lookup --server` 仍然最后说了算。

`GET /v1/dns` 的 `upstreams` 因此是随网络变的：启动时的第一代就按启动时的网络建；网络变化之后覆盖变了，就在 `network changed` 之后用内存里的配置重建一代（日志里有一行，见下面），重建完成后 `upstreams` 才是新的——不是 `network changed` 那一刻。离开这个网络时同样重建回去。

## Profile 检查里的诊断

`POST /v1/profiles/check`、`rurge check`、启动与重载共用的诊断里，阶段 3 / M1 的新用法：

| 码 | 情形 |
| --- | --- |
| `E0018` | ``subnet group `<名字>` needs `default` ``：没写（或写成空）`default` 的 `subnet` 组；组不建。**行为变化**：此前这样的 Profile 加载得了，运行时按空组兜底 |
| `E0011` | `[SSID Setting]` 一行的表达式写错（如 `TYPE:SATELLITE`），这一行不用；表达式只在能被引用时才出现在文本里 |
| `W0028` | `[SSID Setting]` 里不认识的参数（不引用它）；`tfo-behaviour` 的值写错 |
| `W0029` | `[SSID Setting]` 的一行没有参数；`tfo-behaviour`（TCP Fast Open 没有实现）与 `cellular-mode` 写对了也不生效 |
| `W0004` | `cellular-fallback`（iOS 专属） |
| `W0012` | `suspend` 的值不是布尔值，不引用取值 |

`SUBNET` 规则不再引出"永不匹配"的 `W0013`；`[SSID Setting]` 不再列在"解析了、这一版不生效"的 `W0016` 里；`subnet` 组不再引出 `W0008`。

## 网络任务的日志行

日志文本是英文。缺省的日志级别（`loglevel = notify`，对应 tracing 的 INFO）看得到 INFO 与 ERROR 的行，DEBUG 的行要 `--log-level debug`。

| 级别 | 文本 | 字段 | 什么时候 |
| --- | --- | --- | --- |
| INFO | `network changed` | `interface`（主网卡名，没有时 `none`）、`kind`（`wifi` / `wired` / `other` / `none`）、`wifi`（布尔：有没有连着的 Wi-Fi） | 去抖之后每处理一次变化一行。**不含** SSID、BSSID、网关与本机地址 |
| DEBUG | `where the machine is now` | `ssid`、`bssid`、`routers` | 紧跟上一行；说明使用者在哪里的东西只在这里 |
| DEBUG | `the network is not what it was` | `facts`、`dns_servers`、`addresses`（布尔：哪一项不同） | 网络任务判定有变化时；只说哪一项不同，不说值 |
| INFO | `this network sets other DNS servers than the generation in use was built with; building it again` | | 为 DNS 覆盖用内存里的配置重建一代之前 |
| ERROR | `cannot build the generation again for the DNS of this network; the one in use goes on` | `error` | 重建失败；在用的一代照旧 |
| INFO | `policy group selection changed via http-api` | `group`、`policy` | 每一次成功的 `POST /v1/policy_groups/select`，`subnet` 组的临时覆盖与清除也算（这一行阶段 2 就有；清除时 `policy` 是空的） |
| INFO | `takeover paused on this network` | | 进入接管暂停（启动时就在这样的网络上也记） |
| INFO | `takeover resumed` | | 离开接管暂停 |
| INFO | `the takeover is paused on this network; the system proxy is left as it is, and set as asked when the pause ends` | `enabled` | 暂停期间一次得到成功应答的 `POST /v1/features/system_proxy` |
| ERROR | `cannot give the system proxy back for the pause; it stays pointed at rurge` | `error` | 进入暂停时恢复系统代理失败 |
| ERROR | `cannot point the system proxy at rurge again` | `error` | 暂停结束时重新指向 rurge 失败（不重试） |
| ERROR | `cannot tell the engine that the network changed` | `panicked` | 引擎处理换网络时 panic |
| DEBUG | `cannot read the network` | `panicked` | 平台的读取器 panic；这次读取当作没发生，不是"网络不知道" |
| DEBUG | 平台层的几句（每个原因每个进程只说一次，只带程序名与错误种类，不带任何网络的名字）：`cannot ask this program about the network; what it would say is taken as unknown`（`program`、`reason`）；Windows、Linux、macOS 各一句"有无线网卡开着，但系统不给网络的名字"，结尾都是 `SSID and BSSID conditions will not match`（Windows 与 macOS 的写着"it wants the permission to see where the machine is"）；`cannot list the network interfaces`；`cannot open a routing socket; the network is looked at every 30 seconds`（macOS） | | 读不到时 |

SSID、BSSID、网关、本机地址与 DNS 服务器的取值从不出现在 INFO 及以上的行里；DNS 覆盖的列表也不进日志。

## 只供测试的网络后端

`RURGE_NETWORK_BACKEND=file:<路径>` 把"读本机的网络"换成读一个 JSON 文件（形状同 `rurge_platform::netenv::Snapshot`：`primary`、`wifi`、`dns`、`addresses`，每个字段都可以不写；文件不在或读不懂时是"什么都不知道"），每 100 毫秒看一次文件有没有变，从不碰真实的网络。这是**测试钩子，不是给用户用的**（同阶段 1 的 `RURGE_SYSTEM_PROXY_BACKEND`，见 `docs/api/phase1.md`「系统代理」一节）：取值只能是 `file:<路径>`，别的取值直接报错并以 2 退出，不会静默退回真实的网络。

`external` 策略拉起的程序拿不到名字以 `RURGE_` 开头的变量，这个变量也到不了嵌套的 `rurge`；`rurge run`、`rule match`、`dns lookup`、`dns cache` 因此另有一个隐藏的选项 `--network-backend <file:路径>`（同一个变量，不在 `--help` 里；在命令行上取代变量，未知取值报同一句话）。

## 没有覆盖的

- Linux 与 macOS 的读取（`/proc`、`/sys`、`nmcli`、`iw`、`route`、`networksetup`、`ipconfig getsummary`、`PF_ROUTE` 套接字）只在 Windows 开发机上对着假的机器测过，没有碰过真实的系统；手工验收清单 `docs/acceptance/phase3-manual.md` 一项也没有做过。
- 「网络变化」里引擎处理的几件事在真实网络上的效果（`url-test` 一类的组重测、WireGuard 换载体之后恢复得多快）只在回环上验过。
- 主网卡是默认路由所在的网卡，不分物理与虚拟：全隧道的 VPN 接管默认路由之后，`TYPE:` 与 `ROUTER:` 看的是 VPN 的网卡。
