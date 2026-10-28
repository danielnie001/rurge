# rurge HTTP API（阶段 3）

阶段 3 / M1（网络环境与子网，设计 `docs/superpowers/specs/2026-10-16-phase3-m1-network-subnet-design.md`，计划 `docs/superpowers/plans/2026-10-16-phase3-m1-network-subnet-plan.md`）**没有新增端点，也没有改任何响应的形状**。变的是三个已有端点的行为——`POST` / `GET /v1/policy_groups/select` 对 `subnet` 组、`GET` / `POST /v1/features/system_proxy` 在接管暂停时、`GET /v1/dns` 的 `upstreams` 随网络的 DNS 覆盖——，和一组日志行。没有任何端点给出本机当前所在的网络：SSID、BSSID 和网关说明使用者在哪里，只在 DEBUG 日志里出现；读网络的日志行（网络任务与平台层）不带本机网卡的地址，也不带系统 DNS 服务器的取值（设计 M1-D16）。这不等于这些地址在任何级别的日志里都没有——DNS 服务器的地址出现在解析器自己的行里，会话的日志行带来源地址——，见「网络任务的日志行」末尾（M1 收尾时写的"本机地址与 DNS 服务器的取值在任何级别的日志里都不出现"说得过了头，阶段 3 / M2 订正）。鉴权（`X-Key`）、错误体（`{"error":"<message>"}`）与无内容的成功响应（`{}`）沿用阶段 1 的约定，见 `docs/api/phase1.md`；本页只记录变了的部分。

阶段 3 / M2（进程识别，设计 `docs/superpowers/specs/2026-10-17-phase3-m2-process-design.md`，计划 `docs/superpowers/plans/2026-10-17-phase3-m2-process-plan.md`）**没有新增端点**，两个已有端点 `GET /v1/requests/recent` 与 `GET /v1/requests/active` 的每条记录多了两个键 `pid` 与 `processPath`；规则等不到进程时，会话的 `notes` 里多一句说明。下面到「没有覆盖的」为止说的是 M1，M2 在「M2　进程识别」一节。

阶段 3 / M3b（UDP、ICMP、fake-IP 池与引擎接入，设计 `docs/superpowers/specs/2026-10-18-phase3-m3-tun-core-design.md`，计划 `docs/superpowers/plans/2026-10-22-phase3-m3b-udp-icmp-pool-engine-plan.md`）**没有新增端点**：`GET /v1/traffic` 的 `listener` 多了一个键 `tun`，进程查找的 DEBUG 行的字段 `listen_port` 换成了 `remote`，数据目录里多了池的文件 `fake-ip.txt` 与它的三行日志。引擎一侧的 `Engine::start_tun` 只有测试调用，`rurge run` 要到 M4 才起虚拟网卡，所以前两处在今天的 `rurge run` 里是这样的：`tun` 恒为 0，查找行只有监听器的会话；池的文件与它的日志使用者要到 M4 才看得到。见最后一节「M3b　虚拟网卡的 UDP、ICMP 与引擎」。

阶段 3 / M3c（DNS 应答器、预匹配与经监听器来的 fake 地址，设计 `docs/superpowers/specs/2026-10-18-phase3-m3-tun-core-design.md` 第 11 ～ 13 节，计划 `docs/superpowers/plans/2026-10-23-phase3-m3c-dns-responder-prematch-plan.md`）**没有新增端点**，也没有改任何响应的形状：经虚拟网卡来的会话在请求记录里多了预匹配的两种 `notes` 和没有端口的 `dst`，经监听器来的 fake 地址有了一条失败原因 `fake address not known`，应答器与预匹配有了几行日志，计数的 `queue_full` 多了几处来源。**`rurge run` 要到 M4 才起虚拟网卡，使用者今天看不到这些**（连经监听器来的那一半也要有 TUN 的池）；见最后一节「M3c　虚拟网卡的 DNS 应答器、预匹配与经监听器来的 fake 地址」。

阶段 3 / M4a（增强模式的生命周期与平台接入的骨架，设计 `docs/superpowers/specs/2026-10-26-phase3-m4-platform-design.md`，计划 `docs/superpowers/plans/2026-10-26-phase3-m4a-skeleton-plan.md`）**没有新增端点**：`GET` / `POST /v1/features/enhanced_mode` 从恒假、不可切换变成活的（形状是 M3 设计定的——M3-D22，它的第 14 节——M4 设计第 3 节照用；阶段 1 到 M3 这个端点 `GET` 恒为 `{"enabled":false}`、`POST` 回 501，M4a 起才有真值），`rurge run` 多了 `--enhanced-mode` / `RURGE_ENHANCED_MODE`，`rurge check` 多了 `W0035` ～ `W0038`。**这一版的平台后端对每个动作都答 "not available"**，所以使用者仍然开不了增强模式；下面说的其余行为今天只有经只供测试的文件后端才看得到。见最后一节「M4　增强模式（平台接入）」。

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

`rurge run` 起一个网络任务：启动时（建第一代之前）读一次网络，最多等 1 秒——读得更慢就先当作不知道网络（同读失败），第一代建在那上面，读完之后由网络任务当作一次变化告知（DNS 覆盖因此可能多一次重建，接管暂停也在那时才按这个网络决定）；之后在后台盯着，网络**变了**（主网卡、默认网关、网卡类型、Wi-Fi 的名字或接入点、系统 DNS 服务器或主网卡的地址任何一项不同）就依次做三件事，由主循环一次一个地做，与重载、API 的命令不交叉：

1. **引擎处理**（`Engine::network_changed`，同步）：
   - 存下新的网络——之后的规则求值与 `subnet` 组的选择看它；
   - 正在用的这一代的解析器清 DNS 缓存、解除 AAAA 抑制、重读系统的 DNS 服务器；
   - 注册表里每个出站收到 `network_changed()`，只有 `wireguard` 做事：给每个 peer 换新的载体、重新握手；
   - `url-test`、`fallback`、`load-balance` 组上一轮测试的时间作废，下一次使用时重测（结果本身留着，新一轮测完之前照旧用；`evaluate-before-use` 的组下一次使用时等这一轮；换网络那一刻正在跑的一轮不算新网络上的一轮）；`smart` 组不动；
   - `subnet` 组的临时覆盖清除；
2. **DNS 覆盖变了就重建一代**：新的网络让生效的 `[SSID Setting]` 行的 DNS 覆盖与这一代建的时候不同时，用**内存里的配置**重建一代并换上（不重读磁盘上的 Profile——网络变化不该顺带让使用者没让重载的改动生效；`select` 组的选择照旧）；
3. **接管暂停的状态变了就暂停或恢复系统代理**（见下一节）。

正在进行的会话与查找照旧用它们开始时的东西完成，不被打断（读代码得出；没有用例持着一个会话跨过一次换网络）。

怎样发现变化：Windows 与 Linux 每 2 秒比一次便宜的指纹（Windows 是 `ipconfig` 包的网卡表，Linux 是 `/proc/net/route`、`/proc/net/ipv6_route`、`/sys/class/net`、网卡地址与 `resolv.conf`），不起子进程；问 Wi-Fi 的名字要起子进程（`netsh`、`nmcli` / `iw`），只在指纹变了时问，以及有无线网卡开着时每 30 秒问一次（同一个 SSID 下换接入点、地址不变）。macOS 听一个 `PF_ROUTE` 套接字（只看地址增删、网卡状态与路由的增删改，滤掉 ARP 一类的噪声，一阵消息合成一次），另外每 30 秒兜底读一次。变化经 1 秒的"首发加尾发"去抖：第一个变化立刻处理并开一个 1 秒的窗口，窗口里再来的变化合并成窗口结束时的一次，用最新的网络；一个回到原样的网络不算变化。读网络本身也限速：两次读取之间至少隔 1 秒，一个提示若在上一次读取之后 1 秒之内来，就等这一秒结束再读，窗口里来的所有提示（包括读取进行中来的）合成这一次读取，所以一阵提示至多花两次读取（子进程因此至多每秒起一批）。每个读网络的子进程限时 2 秒，读不到的当作没有，不报错。

**读不到任何网络不算变化。**一次读取什么都说不出来（没有主网卡，也没有 Wi-Fi 连接）时，网络任务认为机器丢了网络——睡眠唤醒、Wi-Fi 重连、拔插扩展坞时都会有这一刻——，而不是换到了另一个网络：它不告知主循环，所以引擎不收到 `network_changed`（不清缓存、不作废测速、不清 `subnet` 组的临时覆盖），`[SSID Setting]` 不重算（接管暂停不结束，DNS 覆盖不重建）。上一个告知过的网络继续有效；日志里一行 DEBUG `network unknown; the last known network stands`，丢失持续期间只说一次，读到有网络的快照后再丢失时再说一次。读回来的网络与最后一个告知过的网络比较：同一个网络什么也不发生，另一个网络照常是一次变化。启动时第一次读什么都不知道，就是初始状态，之后读到的网络是一次普通的变化。代价：真的换到一个什么都读不出来的网络（没有默认路由，也没有 Wi-Fi 连接）时，旧网络的设置一直生效，直到读到一个有网络的快照。

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
| `POST /v1/features/system_proxy` `{"enabled": false}` | 返回成功 `{}`；记下"不想要"，暂停结束时就不再打开；进入暂停时恢复系统代理失败、它仍指向 rurge 的话，暂停结束时把它恢复成使用者原来的设置 |

要点：

- "想要的状态"由三处决定，最后一次生效：启动参数 `--system-proxy`、暂停开始前系统代理是不是开着、暂停期间每一次得到成功应答的 `POST`。
- 启动时就在这样的网络上，`--system-proxy` 只记下想要的，不设系统代理，标准输出里也没有 `system proxy enabled:` 那一行；离开这个网络时才设。
- **例外：启动时第一次读网络慢于 1 秒。**第一代按"不知道网络"建（见「网络变化」），`--system-proxy` 这时看不出这是一个 `suspend` 的网络：照常把系统代理指向 rurge，标准输出里有 `system proxy enabled:` 那一行。读完之后网络任务把读到的网络当作一次变化告知，它落在 `suspend=true` 的网络上，接管暂停——系统代理恢复成原来的设置，一行 INFO `takeover paused on this network`；所以这样的启动里系统代理翻转一次：先设上，读完再恢复，离开这个网络时再指向 rurge。代价：慢的首读期间系统代理指向了 rurge，而使用者其实在一个该让出接管的网络上。
- 暂停只在内存里，不进 `state.json`。进入暂停时备份已经清掉，所以暂停期间 rurge 崩溃，下一次启动没有东西要恢复——系统代理早已是原来的样子。
- 一次成功的重载之后会再问一遍"现在暂停吗"：重载可能加上或拿掉了 `suspend=true` 那一行。
- 暂停结束时把系统代理的实际状态对到想要的状态，两个方向都对：想要开着而实际关着的，重新指向 rurge；想要关着而实际开着的（进入暂停时恢复失败才会有），恢复成使用者原来的设置。每个只试一次，失败只记一条 ERROR（比如那时没有可用的 http / socks5 监听器），不重试，直到下一次进出暂停或下一次 `POST`。

## `GET /v1/dns` 与 `[SSID Setting]` 的 DNS 覆盖

所在网络的 `[SSID Setting]` 行（几行同时命中时取第一行）写了 `dns-server` 或 `encrypted-dns-server` 时，它们**替换**（不是合并）Profile 全局的同名列表：没写的那一项保持全局的；`encrypted-dns-server=off` 去掉加密 DNS（全局配了加密 DNS、这个网络要用传统 DNS 时必须写它）；`dns-server` 下写的加密 URL 照 `[General]` 的读法算加密服务器；`[Host]` 里 `server:` 写明的服务器不受影响；`rurge dns lookup --server` 仍然最后说了算。

`GET /v1/dns` 的 `upstreams` 因此是随网络变的：启动时的第一代就按启动时的网络建；网络变化之后覆盖变了，就在 `network changed` 之后用内存里的配置重建一代（日志里有一行，见下面），重建完成后 `upstreams` 才是新的——不是 `network changed` 那一刻。离开这个网络时同样重建回去。重建出的一代像启动与重载建出的一代一样，把自己的诊断打印到标准错误：解析器用不了的服务器（比如带了路径的 `tcp://`）的 `W0026` 只会出现在这里，因为一行 `[SSID Setting]` 的服务器表只在那个网络成为当前网络时才交给解析器。

## Profile 检查里的诊断

`POST /v1/profiles/check`、`rurge check`、启动与重载共用的诊断里，阶段 3 / M1 的新用法：

| 码 | 情形 |
| --- | --- |
| `E0018` | ``subnet group `<名字>` needs `default` ``：没写（或写成空）`default` 的 `subnet` 组；组不建。**行为变化**：此前这样的 Profile 加载得了，运行时按空组兜底 |
| `E0011` | `[SSID Setting]` 一行的表达式写错（如 `TYPE:SATELLITE`），这一行不用；表达式只在能被引用时才出现在文本里（不长、没有空白与 `=`，也不含 `://` 与 `@`——那是一个 URL，可能带着口令） |
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
| DEBUG | `network unknown; the last known network stands` | | 读到什么都不知道的网络（没有主网卡，也没有 Wi-Fi 连接）时；丢失持续期间只说一次。不告知主循环，见「网络变化」 |
| DEBUG | `the network is not read yet; the start goes on without it, and it is told when it is read` | | 启动时第一次读网络超过 1 秒；读完之后是一行 INFO `network changed` |
| INFO | `this network sets other DNS servers than the generation in use was built with; building it again` | | 为 DNS 覆盖用内存里的配置重建一代之前 |
| ERROR | `cannot build the generation again for the DNS of this network; the one in use goes on` | `error` | 重建失败；在用的一代照旧 |
| INFO | `policy group selection changed via http-api` | `group`、`policy` | 每一次成功的 `POST /v1/policy_groups/select`，`subnet` 组的临时覆盖与清除也算（这一行阶段 2 就有；清除时 `policy` 是空的） |
| INFO | `takeover paused on this network` | | 进入接管暂停（启动时就在这样的网络上也记） |
| INFO | `takeover resumed` | | 离开接管暂停 |
| INFO | `the takeover is paused on this network; the system proxy is left as it is, and set as asked when the pause ends` | `enabled` | 暂停期间一次得到成功应答的 `POST /v1/features/system_proxy` |
| ERROR | `cannot give the system proxy back for the pause; it stays pointed at rurge` | `error` | 进入暂停时恢复系统代理失败 |
| ERROR | `cannot point the system proxy at rurge again` | `error` | 暂停结束时想要开着，重新指向 rurge 失败（不重试） |
| ERROR | `cannot give the system proxy back when the pause ends; it stays pointed at rurge` | `error` | 暂停结束时想要关着而它仍指向 rurge（进入暂停时恢复失败），这次恢复也失败（不重试） |
| ERROR | `cannot tell the engine that the network changed` | `panicked` | 引擎处理换网络时 panic |
| DEBUG | `cannot read the network` | `panicked` | 平台的读取器 panic；这次读取当作没发生，不是"网络不知道" |
| DEBUG | 平台层的几句（每个原因每个进程只说一次，只带程序名与错误种类，不带任何网络的名字）：`cannot ask this program about the network; what it would say is taken as unknown`（`program`、`reason`）；Windows、Linux、macOS 各一句"有无线网卡开着，但系统不给网络的名字"，结尾都是 `SSID and BSSID conditions will not match`（Windows 与 macOS 的写着"it wants the permission to see where the machine is"）；`cannot list the network interfaces`；`cannot open a routing socket; the network is looked at every 30 seconds`（macOS） | | 读不到时 |

SSID、BSSID 与网关从不出现在 INFO 及以上的行里；上表（网络任务与平台层读网络）的行不带本机网卡的地址，也不带系统 DNS 服务器的取值，DNS 覆盖的列表也不进这些行。**但这些地址不是在任何级别的日志里都没有**（阶段 3 / M2 订正了 M1 写下的那句"本机地址与 DNS 服务器的取值在任何级别的日志里都不出现"）：

- DNS 服务器的地址出现在解析器自己的行里。DEBUG 的 `send`、`answer`、`failed` 三种行（`rurge_dns::fanout`）带字段 `upstream`，是上游的名字，形如 `udp://<地址>:53`，系统的 DNS 服务器和 `[SSID Setting]` 覆盖的服务器用到时都在内；INFO 的 `IPv6 DNS server ignored because ipv6 is off`（`rurge_dns::resolver`）带字段 `server`，是 Profile 里（或所在网络的 `[SSID Setting]` 里）写的那个 IPv6 服务器。
- 会话自己的日志行带来源地址 `src`：完成的会话是 DEBUG 行，被拒与失败的会话是 INFO 行；客户端经本机的局域网地址连进来时，那就是本机网卡的地址。阶段 3 / M2 起进程查找的 DEBUG 行（见「M2　进程识别」）也带 `src`。
- 读网络读到的本机网卡的地址本身，任何级别的日志里都没有：`the network is not what it was` 只说 `addresses` 这一项不同。

## 只供测试的网络后端

`RURGE_NETWORK_BACKEND=file:<路径>` 把"读本机的网络"换成读一个 JSON 文件（形状同 `rurge_platform::netenv::Snapshot`：`primary`、`wifi`、`dns`、`addresses`，每个字段都可以不写；文件不在或读不懂时是"什么都不知道"），每 100 毫秒看一次文件有没有变，从不碰真实的网络。这是**测试钩子，不是给用户用的**（同阶段 1 的 `RURGE_SYSTEM_PROXY_BACKEND`，见 `docs/api/phase1.md`「系统代理」一节）：取值只能是 `file:<路径>`，别的取值直接报错并以 2 退出，不会静默退回真实的网络。

`external` 策略拉起的程序拿不到名字以 `RURGE_` 开头的变量，这个变量也到不了嵌套的 `rurge`；`rurge run`、`rule match`、`dns lookup`、`dns cache` 因此另有一个隐藏的选项 `--network-backend <file:路径>`（同一个变量，不在 `--help` 里；在命令行上取代变量，未知取值报同一句话）。

## 没有覆盖的

- Linux 与 macOS 的读取（`/proc`、`/sys`、`nmcli`、`iw`、`route`、`networksetup`、`ipconfig getsummary`、`PF_ROUTE` 套接字）只在 Windows 开发机上对着假的机器测过，没有碰过真实的系统；手工验收清单 `docs/acceptance/phase3-manual.md` 一项也没有做过。
- 「网络变化」里引擎处理的几件事在真实网络上的效果（`url-test` 一类的组重测、WireGuard 换载体之后恢复得多快）只在回环上验过。
- 主网卡是默认路由所在的网卡，不分物理与虚拟：全隧道的 VPN 接管默认路由之后，`TYPE:` 与 `ROUTER:` 看的是 VPN 的网卡。

## M2　进程识别

### 请求记录的两个键

`GET /v1/requests/recent` 与 `GET /v1/requests/active` 的每条记录多两个键，紧跟 `listenPort`：

| 键 | 类型 | 内容 |
| --- | --- | --- |
| `pid` | 数字或 `null` | 发起这条会话的进程的进程号 |
| `processPath` | 字符串或 `null` | 它的可执行文件的完整路径；Windows 上形如 `C:\Windows\System32\curl.exe`，在 JSON 里反斜杠要写成 `\\` |

```json
{"id":12,"listener":"http","transport":"tcp","src":"127.0.0.1:51234","listenPort":6152,"pid":4321,"processPath":"C:\\Windows\\System32\\curl.exe","dst":"example.com:80", …}
```

进程的名字不在记录里（路径的最后一段就是它），只在 DEBUG 日志里。键名同 Surge 的面板读的字段（这一事实取自第三方的面板，Surge 的手册没有写请求记录的形状）；记录的其余形状仍是 rurge 自己的，阶段 6 对齐（兼容性清单 10.4 节）。

读记录时才去看进程：会话结束之后才查到的进程照样出现在记录里（被 `REJECT` 的会话往往在查找结束之前就结束了），活动会话的列表同样。

### 什么时候是 `null`

- 会话不是从本机来的：来源地址既不是回环、也不是本机网卡上的地址——局域网里把 rurge 当代理用的设备；虚拟机、WSL2、容器里的程序经网络连进来（它们在本机没有对应的套接字）。经本机的转发程序来的连接，查到的是转发程序，不是原来的客户端。
- rurge 自己发起的会话（`listener` 为 `internal`：内部 DNS 会话、连通性测试），和 `forward`（今天没有监听器产生它）。`tun`（虚拟网卡上的会话）阶段 3 / M3b 起引擎照会话的形状查，见最后一节「M3b」；`rurge run` 今天不起虚拟网卡。
- 查找还没有结束；结束之后再读同一条记录就有了。规则因此没按进程判的会话，`notes` 里有下面那一句。
- 本机看不到发起的进程，或没有权限读它：不以 root 运行的 Linux 上别的用户的进程，Windows 上以普通用户运行时某些系统进程；客户端在查找之前就走了（短命的命令行工具，连接已关、进程已退出）也一样。
- `RURGE_PROCESS_BACKEND=off`（测试钩子，见下）。

只知道名字、不知道路径的进程（Linux 上 `/proc/<pid>/exe` 读不了、名字取自 `comm` 的；Windows 上的 `System`）：`pid` 有，`processPath` 是 `null`——名字写法的 `PROCESS-NAME` 照常命中，路径与前缀写法不命中。读不出进程的描述（打不开）就是"不知道"，没有"只有 PID"的结果：这时两个键都是 `null`。

### 哪些会话查，什么时候查

- 经 HTTP 与 SOCKS5 监听器进来、来源是回环或本机网卡地址的会话（`listener` 为 `http`、`socks5`）。会话开始时在后台查一次，**不论规则用不用得到进程**：请求记录总有进程可看，使用者才知道 `PROCESS-NAME` 该写什么（M2-D3；没有给使用者的开关）。明文 HTTP 的绝对 URI 转发是一个请求一个会话，每个请求各查一次；同一条客户端连接上 2 秒内的请求命中同一份快照，不重读。
- SOCKS5 的 UDP 关联查它的控制连接（TCP）一次，关联里的每条流共用这一次的结果；关联开始 200 毫秒之后才开的流，规则不再等（见下）。
- 来源显然不是本机的（见上）不查：不起任务、不占线程、不读表。"显然"指引擎手里的本机地址列表不超过 2 秒旧、来源不在其中；列表过期时引擎答不准，这一条会话照常起一次查找，由阻塞的那一半重取列表再判断——来源不在新列表里就不读表，结果是 `process lookup found no process`。
- 查找一次占一个阻塞线程，同时至多两个（`owner::LOOKUPS`），排在后面的等。平台层每种协议留一份系统表的快照，一次读取服务所有在它开始之前开始的会话，一阵新连接共用一两次读取；同一种表的两次读取至少隔 20 毫秒。
- 出站模式是直连、或是全局代理而全局策略有效时，不评估规则，也就不等进程；记录照样带进程。

### 规则怎样等

规则走到 `PROCESS-NAME`（顶层的、逻辑规则里的、集合里逐条求值的行）而这个会话的进程还没有查到时，等查找结束，**至多等到查找发起之后 200 毫秒**（`owner::PROCESS_WAIT`；UDP 流的期限从关联的查找发起时算）；一次评估至多等一次。规则用不到进程的会话从不等。

等不到时这次评估按"不知道"判——`PROCESS-NAME` 不命中——，会话继续；查找照常做完，记录随后带上进程。这时：

- 会话的 `notes` 里加一句 ``Process not known within 200 ms: PROCESS-NAME did not match``（记录里有进程、规则却没按它判的会话由这一句说明）；
- DEBUG 日志里有同文的一行，带会话号；
- 进程的一生里**第一次**有规则等不到进程时，WARN 一次（见下表）；之后同样的情形只在 DEBUG 与 `notes` 里。

不设断路器，也不让规则在等的查找插队：代价有界（每条走到 `PROCESS-NAME` 的会话至多多等 200 毫秒），规则的结果不随此前的超时而变。组合的规矩同"要先解析 IP 的规则"：`AND` 遇到确定的不命中就不命中，`OR` 遇到命中就命中，定不下来时**要进程优先于要解析**（进程只在本机查、至多 200 毫秒，先拿到它可能让 DNS 查询变得不必要）；预匹配那一遍不等进程，`PROCESS-NAME` 本身不许带 `pre-matching`。

### 日志

日志文本是英文；DEBUG 的行要 `--log-level debug`。

| 级别 | 文本 | 字段 | 什么时候 |
| --- | --- | --- | --- |
| DEBUG | `process lookup found the process` | `src`、`remote`、`elapsed_ms`、`pid`、`name`、`path` | 一次查找结束，找到了。`elapsed_ms` 从查找发起算起，含等阻塞线程的时间；UDP 关联的一次查找服务它的全部流，`src` 与 `remote` 是控制连接的。**不带会话号**：查找发起时会话号还没分配；同一会话自己的日志行也带 `src`，两行按地址对得上 |
| DEBUG | `process lookup found no process` | `src`、`remote`、`elapsed_ms` | 查找结束，没找到（来源不在表里、进程已经退出、读不了） |
| DEBUG | `Process not known within 200 ms: PROCESS-NAME did not match` | `session`（会话号） | 规则等不到进程、按"不知道"判的那一刻；同文的一句也在会话的 `notes` 里 |
| WARN | `process lookup took longer than 200 ms; PROCESS-NAME rules decided without the process` | | 进程的一生里第一次有规则等不到进程时一次。不带会话、地址与进程 |
| WARN | `process lookup: the tcp table could not be read: <原因>`（`udp` 同）；`process lookup: the tcp table has a layout this build does not know`（macOS 上表的布局认不得；阶段 3 / M3b 起 Linux 上一张有内容而一行也解析不出的表同样报它，此前读作空表、不写日志） | | 平台读系统的表失败：每一种在进程的一生里 WARN 一次，之后同样的失败只在 DEBUG 里。`<原因>` 是系统自己的话，不带地址、进程与路径。这次查询的结果是"不知道"，会话照常 |
| DEBUG | `process lookup: cannot list the network interfaces` | `reason` | 取本机网卡的地址列表失败，当作没有地址：只有回环上的来源查得到 |

前两行 DEBUG 的字段 `remote` 是阶段 3 / M3b 起的写法（M2 时是 `listen_port`）：它是什么、什么时候省略，见最后一节「M3b」。

**进程的名字、路径与 PID 只出现在 DEBUG 的查找行、请求记录与 API 里，不出现在 INFO 及以上的行里**（Windows 的路径常带用户名）。被拒与失败的会话的 INFO 行照旧带来源、目标、规则文字（`rule=`）和策略；规则文字是使用者自己写的规则，里面若写了进程名或路径，那是使用者写的，不是查到的。不知道进程从不让会话失败，也不进会话的 `error`。

### 只供测试的 `RURGE_PROCESS_BACKEND`

`RURGE_PROCESS_BACKEND=off` 让 `rurge run` 不去查会话来自的进程：记录里没有进程（`pid` 与 `processPath` 都是 `null`），`PROCESS-NAME` 一律按不知道判，不起查找任务、不读系统的表。没设时查本机。这是**测试钩子，不是给用户用的**，也不是"关掉查找"的开关（查找总开着，M2-D3）：取值只能恰好是 `off`（`OFF` 不行），别的取值直接报错——``error: unknown RURGE_PROCESS_BACKEND value `<取值>` (expected off)``，以 2 退出，退出之前先恢复上一次崩溃留下的系统代理，在绑定任何端口之前——，不会静默退回真实的查找（同 `RURGE_NETWORK_BACKEND`）。它不是命令行选项，不在 `--help` 里；`rurge check`、`rule match`、`dns lookup` 不读它（它们不查进程）；`external` 策略拉起的程序拿不到名字以 `RURGE_` 开头的变量，嵌套的 `rurge run` 照常查本机。

### 没有覆盖的

- Linux 与 macOS 的真实读取（`/proc/net/*` 与 `/proc/<pid>/fd` 的内容和权限、`pcblist_n` 的真实布局、`proc_pidpath`）只对着夹具测过；两个只在 macOS 上编译的 unsafe 函数与 Linux、macOS 的真读用例 `this_process_owns_its_loopback_sockets` 从没有在任何地方编译过（它们在只为本平台编译的模块里）；Windows 的真读用例和端到端用例 `a_process_rule_matches_the_test_process` 只在一台 Windows 11 x86-64 的开发机上编译、跑过。第一次 CI 运行时核对；手工验收清单 `docs/acceptance/phase3-manual.md` 的 M2 一节一项也没有做过。
- macOS 取 `so_last_pid`（最后用这个套接字的进程），不取 `so_e_pid`（委托时的"实际"进程）：经 `nsurlsessiond` 等系统服务代建的连接，记录里是哪一个，没有看过。
- 经真实虚拟网卡来的会话（引擎一侧的查找 M3b 做了，见「M3b」一节；真实的设备随 M4）；`$request.processPath`（脚本，阶段 5，读同一个进程槽）；外部程序的进程树判断（M4，防环用）。离线的 `rurge rule match` 不查真实的进程（没有连接；`--process` 照旧由命令行给）。
- 查找的代价只在 Windows 上量过一次（debug 构建，开着 500 条回环连接时一次新的查找 3.39 毫秒）；Linux 的 `fd` 扫描在进程很多的机器上、几百条并发连接时是否在 200 毫秒里结束，没有量过。
- 一个永不返回的平台查找会占住两个许可，此后每条会话排一个小任务、不设上限，`rurge run` 退出时 tokio 不设时限地等阻塞线程；三个平台的读取都是有界的系统调用，不知道哪一个会挂住。
- Surge 的实际行为没有观察过：`PROCESS-NAME` 在 Windows 上不分大小写、不去掉 `.exe`、盘符与 `\\` 开头的值读作路径，都是 rurge 的读法（兼容性清单 3.1 节）。

## M3b　虚拟网卡的 UDP、ICMP 与引擎

阶段 3 / M3b 把虚拟网卡的内核（`crates/rurge-tun`）接进引擎（`Engine::start_tun`，`crates/rurge-engine/src/tun.rs`）：经虚拟网卡来的 TCP 连接与 UDP 数据报照规则与策略走，发往 fake 地址的照它背后的名字走，被拒的 UDP 流以 ICMP"管理上禁止"回答应用；回显请求不经规则与策略：rurge 自己的地址本地应答，fake 地址先解析它背后的名字、向解析出的地址经 `Pinger` 发出，其余的照原地址经 `Pinger` 发出，`icmp-forwarding = false` 时一律丢弃。**`Engine::start_tun` 只有测试调用，`rurge run` 要到 M4 才起虚拟网卡**：经虚拟网卡来的会话和下面的池的文件与它的日志，使用者要到 M4 才看得到；前两小节说的两处今天的 `rurge run` 里就有，只是 `tun` 恒为 0、查找行只有监听器的会话。没有新增端点、诊断码或命令行选项，配置格式不变。

### `GET /v1/traffic` 的 `listener.tun`

`listener` 从两个键（`http`、`socks5`）变成三个，多了 `tun`，形状相同（`in` 是下行字节，`out` 是上行）：

```json
"listener":{"http":{"in":123,"out":45},"socks5":{"in":0,"out":0},"tun":{"in":0,"out":0}}
```

`tun` **一直在**：虚拟网卡的会话结束之前是 `{"in":0,"out":0}`，所以今天的 `rurge run` 里恒为 0。经虚拟网卡来的会话（一条 TCP 连接，或 UDP 会话里的一条流）结束时，它的字节计进 `tun`；HTTP 与 SOCKS5 的会话不计入它，它的会话也不计入 `http` 或 `socks5`。`total` 与 `connector` 照旧统计所有会话，`tun` 的会话也在其中；`listener` 仍只统计已结束的会话（`docs/api/phase1.md` 的「Traffic」一节）。

### 进程查找的 DEBUG 行：`remote`

M2 一节「日志」里两行 DEBUG（`process lookup found the process`、`process lookup found no process`）的字段 `listen_port` 换成了 `remote`：客户端那条连接在 rurge 一侧的端点（`地址:端口`）。进程的查找要这个地址和端口都对上，不只是端口（同一个本地端点同时连着两台主机上同号的端口时，只比端口会认错行）：

| 会话 | `remote` |
| --- | --- |
| HTTP 与 SOCKS5 监听器的会话 | 被接受的那条连接自己的本地地址：客户端连的是 `127.0.0.1:6152` 就是它，哪怕监听器绑的是 `0.0.0.0:6152` |
| SOCKS5 的 UDP 关联 | 它的控制连接的本地地址（关联按控制连接查） |
| 经虚拟网卡来的 TCP 会话 | 应用连的端点（fake 地址或真实地址） |
| 经虚拟网卡来的 UDP 会话 | 没有这个字段（UDP 的查找只按应用自己的端点） |

没有这个端点时字段省略，不写 `null`。这些行仍是 DEBUG：进程的名字、路径与 PID 不出现在 INFO 及以上的行里。

### 池的文件 `fake-ip.txt` 与它的三行日志

fake 地址与名字的对应记在数据目录里：`<数据目录>/fake-ip.txt`（UTF-8 文本）。第一行 `rurge-fake-ip 1`；第二行 `cursor <n> wrapped <0|1> clean <0|1>`；其余每行 `<编号> <名字>`（编号 *i* 同时是 `198.18.1.1 + i` 与 `fd00:6152::` 加同一个 IPv4 地址，名字是小写的域名）。**里面是应用查过、拿到 fake 地址的名字**，文件的权限就是数据目录给的，rurge 没有另设（M3b 延后第 23 行）。

rurge 在对应有变化时重写它，至多每 10 秒一次，TUN 停下时再写一次（`clean` 变 1）；先写 `fake-ip.txt.tmp` 再改名，不 fsync。打开时，`clean 1` 的文件立刻改写成 `clean 0`，没有文件时立刻写一份空表的 `clean 0`：所以崩溃或被强杀的那一次，下次启动看到的是 `clean 0`，接下来会交出的 2048 个 fake 地址（连同文件里没有名字的编号）隔离 120 秒，应用手里的旧地址不会指到新的名字。读不了或读不懂的文件（第一行不对、第二行不是三对、编号越界、名字重复或不是规范形式、行不以单个换行结束……）读作空表，文件留着不动，到第一次保存时才被覆盖，同样隔离 2048 个。

| 级别 | 文本 | 字段 | 什么时候 |
| --- | --- | --- | --- |
| WARN | `fake-ip: the saved addresses could not be read; starting with none` | `error` | 打开时文件读不了或读不懂。`error` 是池自己的原因（固定的话加行号，或系统的错误文本），不引用文件的内容。由 `Engine::start_tun` 在调用它的线程上写 |
| INFO | `fake-ip: rurge did not stop cleanly last time; 2048 fake addresses are held back for 120 seconds` | | 打开时文件第二行是 `clean 0`（上一次崩溃或被强杀）。只有个数，没有名字与地址。同样由 `Engine::start_tun` 写 |
| WARN | `fake-ip: the addresses could not be saved` | `error` | 一次保存失败（目录不可写、磁盘满）。由池的保存任务（`keep_saved`）写，不带名字；下一次有变化时再试 |

没有文件和干净的文件什么也不说。

### 没有覆盖的

- 绑在 `0.0.0.0` / `[::]` 上的监听器的会话，`remote` 是被接受的那条连接的本地地址、不是绑定的地址：这只由读代码和回环上的用例保证（用例只绑回环，M3b 延后第 6 行），M4 的手工验收看。
- 经虚拟网卡来的会话、池的文件与 `listener.tun` 只在一台 Windows 11 x86-64 的开发机上、内存设备里跑过，CI 没有跑过；真实的设备、真实的 `Pinger`（ICMP 套接字）、系统 DNS 与防环随 M4。池的文件在 Linux 与 macOS 上读写出错的情形只按两处假定写了用例，没有在那两个平台上跑过（M3b 延后第 27 行）。
- 被拒与失败的会话，引擎的 INFO 行照旧带 `src` 与 `dst`（阶段 1 起的写法）；经虚拟网卡来的会话的 `dst` 是 fake 地址背后的名字。设计第 14 节要名字只出现在 DEBUG，这一处没有照做（M3b 延后第 1 行，留给项目所有者）。
- 手工验收清单 `docs/acceptance/phase3-manual.md` 的 M3 一节（M3a、M3b、M3c 合在一起，M3c 收尾时加的）里有 M3b 的行为要在真实设备上看的几项：M3b 加的行为在 M4 之前验不了，那一节一项也没有做过。

## M3c　虚拟网卡的 DNS 应答器、预匹配与经监听器来的 fake 地址

阶段 3 / M3c 在虚拟网卡的内核（`crates/rurge-tun`）里做了 DNS 应答器（`Responder`）和预匹配在 DNS 与 SYN 两层的拒绝，在引擎里做了预匹配的钩子（`crates/rurge-engine/src/prematch.rs`）和经 HTTP / SOCKS5 监听器来的 fake 地址的换回（`crates/rurge-engine/src/fake_address.rs`）；M3（内存设备上的 TUN 内核）到此做完。**没有新增端点、诊断码、命令行选项或配置键**：`allow-dns-svcb`、`hijack-dns`、`always-real-ip` 自阶段 1 起被解析，到这一份才有读它们的人。

**`rurge run` 要到 M4 才起虚拟网卡，所以使用者今天看不到这一节的任何东西。**经虚拟网卡来的会话、应答器的应答、预匹配的记录和下面的日志行，都只在 `Engine::start_tun` 起着 TUN 的时候才有，今天只有测试调用它；连"经监听器来的 fake 地址"那一半也一样——引擎的池（`Engine.fake_pool`）由 `start_tun` 放进去、TUN 结束时清掉，没有池时那样的地址照普通地址处理，`rurge run` 今天从不放。使用者今天看得到的只有两处：`rurge dns lookup --trace` 一类的输出里，应答码 FORMERR 与 NOTIMP 现在写它们的名字（原来写 `RCODE1`、`RCODE4`）；和 UDP 上游给在途的同 ID 的第二个查询换一个空着的 ID（`Pending::claim`）——原来第二个查询顶掉第一个的等待者，第一个以 `waiter replaced or dropped` 失败，两个同时在途的查询碰巧带着同一个 ID 时才碰得到。M3 整体今天看得到的还有 M3b 的三处，见「M3b」一节：`listener.tun` 恒为 0，进程查找 DEBUG 行的 `remote`，`rurge run` 的 TCP 进程查找连远端地址一起比。

### 请求记录里的虚拟网卡会话

`GET /v1/requests/recent` 与 `active` 的形状不变（`docs/api/phase1.md`、本页「M2」一节）。经虚拟网卡来的会话（M3b 起）在里面是这个样子：

| 键 | 内容 |
| --- | --- |
| `listener` | `"tun"` |
| `listenPort` | `null`：虚拟网卡没有监听端口（`in_port` 为 0） |
| `transport` | `"tcp"` 或 `"udp"`；预匹配的 DNS 层记录是那条查询自己的传输 |
| `src` | 应用的源端点（M4 起是虚拟网卡上的地址，如 `198.18.0.1:53000`） |
| `pid`、`processPath` | 照 M2 与 M3b：TCP 按应用连的端点查，UDP 只按应用自己的端点查，查不到是 `null`（预匹配的记录该记时也查，DNS 层的按那条查询自己的传输） |
| `dst` | fake 地址背后的**名字**（连接开着时它拿着地址的租约），或目的 IP，加 `:端口`；**端口为 0 时只写名字，不写 `:0`**——只有预匹配在 DNS 层的记录是这样，同一个写法也用在引擎的会话日志行里 |

被预匹配拒绝的会话（DNS 层的一次查询、SYN 层的一个连接）进记录，是这样的一条（DNS 层，`DOMAIN-SUFFIX,ads.test,REJECT,pre-matching`）：

```json
{"id":31,"listener":"tun","transport":"udp","src":"198.18.0.1:53000","listenPort":null,"pid":null,"processPath":null,"dst":"x.ads.test","remoteAddr":null,"rule":"DOMAIN-SUFFIX,ads.test,REJECT,pre-matching","policy":["REJECT"],"sni":null,"protocol":null,"up":0,"down":0,"startedMs":1792800000000,"elapsedMs":0,"connectMs":null,"firstByteMs":null,"dnsMs":null,"status":"rejected","rejectKind":"REJECT","error":null,"notes":["pre-matching at DNS"]}
```

- `status` 是 `"rejected"`，`rejectKind` 是实际用的那一种：`REJECT`、`REJECT-TINYGIF`、`REJECT-NO-DROP`，或自动升级（见下）之后的 `REJECT-DROP`；`policy` 是规则点名的那一个内置策略（升级之后也是），`rule` 是规则的文字；`error` 是 `null`（`notes` 说明来由，不说明失败）。这样的会话当场结束，不会出现在 `active` 里。
- `notes` 的第一句是 **`pre-matching at DNS`** 或 **`pre-matching at SYN`**；命中的是集合（`RULE-SET` 或 `DOMAIN-SET`）里的一行时，第二句是 ``Sub-rule matched: <那一行> (in <集合>)``（同一句也在 DEBUG 日志里，带会话号）。
- **同一代里同一条规则 5 分钟内只记一次**（换代之后从头记：重载之后，和网络变化让生效的 DNS 覆盖变了、引擎用内存里的配置重建一代之后）。不该记的拒绝照样被拒、照样计入自动升级，只是不进记录，也不为它查进程。

**自动升级**：`REJECT` 与 `REJECT-TINYGIF` 的拒绝记入引擎已有的计数（`REJECT-DROP` 本来就是丢弃，`REJECT-NO-DROP` 从不升级），同一个主机（DNS 层是名字，SYN 层是名字或地址的文字）30 秒内的第 50 次起按 `REJECT-DROP` 处理——第 50 次本身就是：DNS 层不答，SYN 层丢 SYN。计数与监听器的会话共用一份；计入的是每个被拒的 **A 或 AAAA 查询**，所以一次查找问 A 与 AAAA 的应用，比按会话数更早静默。

**只在规则模式下**：钩子问当前的出站模式（取活的，不随代），不是规则模式就不看，也不看 `strict` 的暂停；出站模式是全局而全局策略不可用时，引擎在拨号处按规则走（含预匹配那一遍），这样的拒绝落在拨号，不在 DNS 或 SYN。

### `fake address not known`

有池时（TUN 起着），经 HTTP / SOCKS5 监听器来的、目的是 rurge 地址空间里的 IP 的会话，引擎在规则之前把它换回池里那个编号的名字：

- 池里有：会话的 `dst` 是名字，规则与日志看到的是名字，地址的租约挂在会话上直到它结束（SOCKS5 的 UDP 流：到引擎回收这条流为止，空闲 60 秒，已有应答的 53 端口的流更早）。
- 池里没有：会话失败，**`error` 是 `fake address not known`**，`status` 是 `"failed"`，`dst` 是客户端写的地址，什么也不拨。HTTP 监听器答 **502**（`show-error-page` 开着时是错误页，它缺省开着；关着时关连接），SOCKS5 答 **`0x05`**（连接被拒），SOCKS5 的 UDP 流照别的失败的流办（数据报丢弃，约 5 秒后再试）。
- "rurge 地址空间"是整个 `198.18.0.0/15` 与 `fd00:6152::/64`：运行 TUN 时，经代理端口去 rurge 自己的地址（`198.18.0.1`、`.2`、`.244`……）或这个空间里池不认识的地址，也是 `fake address not known`；这个空间里真实的主机，经代理端口到不了。
- 没有池（没有 TUN）时，这样的地址照普通地址处理，没有这条错误。
- SOCKS5 的 UDP 流换回名字之后，回包的来源在引擎里换回客户端写的那个 fake 地址（客户端认得的是它），**只有引擎知道回包应答的是哪一条流时**。流按它发往的地方记：经 DIRECT 或 `wireguard` 时（它们的载体自己把名字解析成地址）是名字解析出的地址，两条流发往同一个地址与端口（两个 fake 地址，或一个名字与它自己的真实地址）时，回包保留真实的来源，客户端对不上；经别的代理策略时名字由服务器解析（载体把名字原样交回），流按名字记：载体是全锥的（回包带着远端自己的地址，如 `socks5`、`ss`、`trojan` 的 UDP），同一个端口上发往名字的流又不止一条（两个都走 443 的 QUIC 站点）时，两条流都活着的时候回包保留真实的来源，只有一条时，别的主机从这个端口发来的包也被说成来自那个 fake 地址；每个目标一条连接的载体（`vmess`、`h2-connect`、`masque` 的 UDP）把回包的来源报成流写下的那个目标，回包总对得上它的流，没有这个限制。全锥的那一种与 TUN 一侧是同一个限制（兼容性清单 9.1 节"同一个端口上同时对两个名字收发、出站又是全锥的载体时"那一行），同一份 `Routes` 的代码。
- **明文 HTTP 请求**（`GET http://<fake 地址>/…`）换回名字之后，请求行、`Host` 与 `session.url` 仍是 fake 地址：经 `http` 策略（绝对形式）时上游代理被要求去连 fake 地址，直连时源站看到 `Host: <fake 地址>`，`URL-REGEX` 与扩展匹配看到的也是 fake 地址；`CONNECT` 与 SOCKS5 的目标里是名字，不受影响。不改，去向阶段 4（HTTP 引擎改写请求行与 `Host`）。

### 计数

`TunHandle::counters()` 给出一份快照：丢弃的包按原因各一个数，加 `stack_panics`。`GET /v1/features/enhanced_mode` 要到 M4 才生效，它的形状在设计第 14 节：`{"enabled": <bool>, "dropped": {"malformed": <n>, …}, "stackPanics": <n>}`。M3c 之后各原因里变了的：

| 原因 | M3c 起还数 |
| --- | --- |
| `queue_full` | 应答器的队列（`dns_queue`，缺省 1,024）放不下的查询；写任务的队列放不下的本地应答（应答器在 UDP 上的应答，和 M3b 起的 ICMP 消息、回显的本地应答：本地应答从不等空位；TCP 上的应答写进连接，由分片经同一个队列送出、等空位，从不在这里丢）；TUN 里排队的 UDP 超过字节预算（`udp_bytes`，缺省 64 MiB，所有 UDP 会话共用）的数据报 |
| `busy` | 到应答器 53 端口的 SYN 超过 `dns_tcp_max`（缺省 128，整个 TUN 共用）：回 RST（也经"3 秒 100 个"的限频），不占套接字的位置 |
| `rst_limited` | 预匹配的 RST 也经"3 秒 100 个"的限频，超出的丢包计这里 |

不计数的：预匹配丢掉的 SYN（只有一行 DEBUG），应答器正常答出去的查询。

**应答器的 TCP**：到它 53 端口的 TCP 连接在本地应答，不交给引擎；一条连接上的查询按次序一个接一个答，空闲 `dns_tcp_idle`（缺省 10 秒）关。同时服务的连接整个 TUN 至多 `dns_tcp_max` 条（缺省 128）：位置从分片收下 SYN 起算，到这条连接的服务结束归还（握手没完成的连接在半开超时之后归还），多出来的 SYN 回 RST 并计 `busy`。

### 日志

日志文本是英文；DEBUG 的行要 `--log-level debug`。**本节新增的行里，名字与上游的地址只出现在 DEBUG 的行与请求记录里**，下面表里的 INFO 行是例外；引擎照旧写的 INFO 行 `session rejected by …` 与 `session failed`（每个被拒、每个失败的会话一行，带 `src` 与 `dst`，阶段 1 起的写法，见上面 M3b 的「没有覆盖的」第三条）也带名字或地址：被预匹配拒绝的会话、`fake address not known` 的失败都会出这样的行。

| 级别 | 文本 | 字段 | 什么时候 |
| --- | --- | --- | --- |
| DEBUG | `tun: DNS query` | `name`、`qtype`、`decision`、`fitted` | 应答器读成查询的每个消息一行。`decision` 是下面这些之一：`a class other than IN: NOTIMP`、`the canary: NXDOMAIN`、`SVCB or HTTPS while they are not allowed: NOTIMP`、`refused by a rule: an empty answer`、`refused by a rule: no answer`、`refused by a rule: an address of rurge's own`、`the family of the transport is the other: an empty answer`、`always-real-ip: the addresses [Host] maps`、`a fake address`、`every fake address is in use: SERVFAIL`、`too many queries are being forwarded: SERVFAIL`、`forwarded`、`the forward failed: SERVFAIL`。`fitted` 只在应答被改过时有：`truncated: over the UDP limit`（UDP 上超过限度，置 TC）或 `SERVFAIL: too long for TCP` |
| DEBUG | `tun: a DNS message that is no query is not answered`；``tun: a DNS message of an opcode other than QUERY is answered NOTIMP``（字段 `opcode`）；`tun: a DNS message that does not read is answered FORMERR` | | 不是查询的消息（QR 置位、不足 12 字节）不答；opcode 不是 0；读不懂。不带名字 |
| WARN | `tun: every fake address is in use; new names get SERVFAIL` | | 池里的编号全有租约、又来了新名字：这个 TUN 的一生里第一次一行，不带名字与地址 |
| DEBUG | `tun: a SYN is dropped, as a pre-matching rule says` | `src`、`dst`、`name` | SYN 层被 `REJECT-DROP`（或升级之后）丢掉的 SYN；不回包、不计数；`name` 是 fake 地址背后的名字（有的话） |
| INFO | `session rejected by <REJECT 的种类>` | `session`、`listener`、`src`、`dst`、`rule`、`policy`、`elapsed_ms`、`error` | 引擎已有的会话日志行，预匹配被拒的记录也经它：**DNS 层被拒的名字在 `dst` 里**，每条规则每代 5 分钟至多一行（设计要名字只在 DEBUG，这一处不照做，留给项目所有者，M3c 计划延后第 6 行） |
| DEBUG | ``Sub-rule matched: <行> (in <集合>)`` | `session` | 命中的是集合里的一行；同文的一句也在 `notes` 里 |
| DEBUG | `forwarded` / `forward failed`（目标 `rurge_dns::resolver`） | `name`、`qtype`、`rcode` 或 `error`、`elapsed_ms` | 应答器转发一个查询的结果，一次转发一行；读不了的查询只有 `error` |
| DEBUG | `forward send`、`forward answer`、`forward failed`、`forward ignored a message that is no reply to the send`（目标 `rurge_dns::fanout`） | `upstream`、`round` 或 `rcode`、`error`、`elapsed_ms` | 转发扇出里每个上游的往返；这里有上游的名字 |
| WARN | `tun: the system's random source failed; the identifications of the fragments the TUN writes begin at a fixed number` | `error` | 系统的随机源失败，分片的标识从一个固定的数起（M3b 的分片，M3c 加的这行）；不带地址 |

### 没有覆盖的

- 这一节说的一切只在一台 Windows 11 x86-64 的开发机上、内存设备里跑过，CI 没有跑过；真实的设备、地址、路由、系统 DNS 与防环随 M4。手工验收清单 `docs/acceptance/phase3-manual.md` 的 M3 一节（19 项）一项也没有做过。
- 真实的应用（浏览器、`dig`、`nslookup`、带 0x20 大小写混排的解析器）经真实设备问应答器会怎样，没有观察过，同时开 128 条以上到应答器的 TCP 连接的应用会怎样（多出的 SYN 被拒）也没有试过；用例里的"应用"是测试里的另一个 smoltcp 栈。真实设备上一阵来上千个查询时，写任务的队列放不下的应答被丢（应用约 1 秒后重问）。
- 应答器的延迟（NFR-01：小于 0.2 毫秒）只量过内核本身——`Responder::answer` 加造应答包，不含设备、写任务和引擎的规则评估：中位数 0.9 微秒、p99 3.2 微秒，最大 5.8 毫秒；整条路在真实设备上的数字要在 M4 量。
- 上游全挂时，被转发的查询要等约 5 秒才得到 SERVFAIL；系统的 DNS 服务器名单要排除应答器与虚拟网卡的地址，随 M4。

## M4　增强模式（平台接入）

阶段 3 / M4（平台接入，设计 `docs/superpowers/specs/2026-10-26-phase3-m4-platform-design.md`）分四份计划：M4a「平台无关的骨架」、M4b「防环与接管期间的 DNS」、M4c「Linux 与 macOS」、M4d「Windows 与收尾」。**这一节随它们写成：M4a 与 M4b 做到的写在这里**（M4b 的在下面「M4b：防环与接管期间的 DNS」一节）；M4c ～ M4d 写进来时各补各的（三个平台的设备与权限）。

**这一版开不了增强模式。** 缺省的后端（平台后端）对每一个动作都答同一句：`the enhanced mode is not available on this platform in this version`。所以：

- `rurge run --enhanced-mode` 在**每个平台**上都以 1 退出，标准错误是 `error: cannot turn the enhanced mode on: the enhanced mode is not available on this platform in this version`；
- `POST /v1/features/enhanced_mode` 带 `{"enabled": true}` 回 500，体是 `{"error":"the enhanced mode is not available on this platform in this version"}`；
- 两处都不留下任何改动：没有设备、没有地址、没有路由、没有系统 DNS 的改动，`state.json` 的 `enhanced_backup` 不变。

真的能开是在 M4c（Linux 与 macOS）与 M4d（Windows）之后。下面说的其余行为今天**只有经只供测试的文件后端**（`RURGE_ENHANCED_BACKEND=file:<路径>`）才看得到，见本页最后一节与 `docs/testing.md`；那个后端不碰任何系统设置，`rurge run` 里输入它的只有用例。

这一节还订正上面「接管暂停时的系统代理端点」一节的最后一条：那一句写的"增强模式的那一半（撤掉虚拟网卡）随阶段 3 / M4，今天没有"，M4a 有了（今天只经文件后端）。

### 启动参数与环境变量

| 写法 | 取值 |
| --- | --- |
| `--enhanced-mode` | clap 的布尔写法，与 `--system-proxy` 同一个解析器（`true` / `false`、`yes` / `no`、`on` / `off`、`1` / `0`，不分大小写）；不带这个选项就是不开。写错是用法错误（clap 自己的标准错误，以 2 退出） |
| `RURGE_ENHANCED_MODE` | 同一个开关的环境变量背面，缺省的值不设等于不开。写错同样以 2 退出 |

不跨重启保留（M4-D15）：只有这个选项决定启动时开不开，`state.json` 的 `features.enhanced_mode` 不被读。它是 rurge 专有的运行时选项，不写进 Profile（FR-CFG-17）。

启动时的次序与输出（`docs/api/phase1.md` 的「启动输出顺序」一条在 M4a 多了中间一行）：

1. 监听器（`listening on …`）与 HTTP API（`api on http://<addr>`）起来；
2. 接管暂停判定（`Takeover::begin`，见「接管暂停：增强模式的那一半」）；
3. 不在暂停的网络上且带了 `--enhanced-mode`：打开增强模式。成功时标准输出印 `enhanced mode on`；失败时标准错误印 `error: cannot turn the enhanced mode on: <原因>`，以 1 退出（打开自己已经把做过的倒着撤销了），这一行之后不会有系统代理那一步；
4. 带 `--system-proxy` 时打开系统代理。**打不开时先关掉已经开着的增强模式**，再以 1 退出：关成功标准输出印 `enhanced mode off`，关失败标准错误印 `error: cannot turn the enhanced mode off: <原因>; what outlives rurge stays in state.json and is undone on the next start`（P25、M4-D16）；
5. 汇总行 `rurge <version> running: …`。

退出时（Ctrl-C、`POST /v1/stop`）增强模式先于系统代理关，在 `drop(following_the_network)` 之后、系统代理之前：成功标准输出印 `enhanced mode off`，失败标准错误印上面同一句 `error: cannot turn the enhanced mode off: …`，退出码不变。之后才是系统代理、API 令牌与监听器。

### `GET` / `POST /v1/features/enhanced_mode`

`GET` 给的是真实状态：开着时是那个 TUN 的计数，关着时每个数都是 0。

```json
{"enabled":true,"dropped":{"malformed":0,"fragment":0,"martian":0,"unsupported":0,"queueFull":0,"unknownFake":0,"busy":0,"beyondWindow":0,"rstLimited":0,"icmpLimited":0},"stackPanics":0}
```

`dropped` 的十个键是 camelCase（同 API 别处），`stackPanics` 在顶层；它们的含义是 `rurge-tun` 的 `Counters`（丢弃按原因各一个数，加 TCP 栈的 panic 次数），本页「M3c」一节「计数」一表说的是各原因还数什么。这个形状是 M3 设计定的（M3-D22，它的第 14 节），M4 设计第 3 节照用；阶段 1 到 M3 这个端点 `GET` 恒为 `{"enabled":false}`、`POST` 回 501，M4a 起才有真值。

`POST` 的体是 `{"enabled": <bool>}`（与其余功能端点同一个读法：不是布尔值、不是 JSON 对象都是 400）。

- 成功回 `{}`。打开成功时日志先有一行 INFO `enhanced mode on`（管理器写的），然后一行 INFO `enhanced mode switched via http-api`（端点写的，字段 `enabled`）；关闭成功时是 INFO `enhanced mode off` 再这一行。
- 失败回 500，体是 `{"error":"<原因>"}`，原因就是后端（或者它上面的 `state.json`）给的那句话；主循环另记一行 ERROR `cannot switch the enhanced mode`（字段 `error`、`enabled`）。打开失败已经自己撤销了做过的事，`GET` 照实说 `enabled: false`（想要的不因一次没做成的打开而改）。关闭失败时回 500，但模式已经关了——关从不半途停下，`GET` 为假，想要的也记成关（协调者的裁定 R35；M4a 计划「执行期修正记录」第 97 行）。
- **已经是要的状态时直接回成功**：开着的再 `POST true`、关着的再 `POST false` 都回 `{}`，一个后端动作都没有（日志里只有端点那一行）。
- 接管暂停期间（见下）回成功 `{}`，但什么也不开、什么也不关，只记下想要的。
- 打开与关闭都在主循环里一次一个，与重载、网络变化、`POST /v1/stop` 排在同一个队列里。

### 日志行

日志文本是英文；缺省的日志级别（`loglevel = notify`，对应 tracing 的 INFO）看得到 INFO 与 ERROR，DEBUG 的行要 `--log-level debug`。**网卡名、地址与前缀只在 DEBUG**（`CLAUDE.md`「日志的分级」）：下面 INFO 的行只有路由的条数与 IPv6 开没开。

| 级别 | 文本 | 字段 | 什么时候 |
| --- | --- | --- | --- |
| INFO | `enhanced mode on` | `routes`（这一次装进去的条数）、`ipv6` | 打开完成之后（`--enhanced-mode`、`POST`、暂停结束都经它） |
| DEBUG | `the enhanced mode's adapter` | `adapter`（网卡名） | 紧跟上一行 |
| INFO | `enhanced mode off` | | 关闭的每一步都走过之后，不论成败 |
| INFO | `enhanced mode switched via http-api` | `enabled` | 每一次得到成功应答的 `POST`（暂停期间也一样） |
| WARN | `IPv6 could not be configured; the enhanced mode goes on without it` | | 要 IPv6 而没有配上：打开时 `configure` 答"没有"，或重载 / 网络变化时 `configure_ipv6` 答"没有"。这一行由管理器写，后端不另写 |
| WARN | `the TUN did not end within 3 seconds; going on` | | 关闭等 TUN 结束超过 3 秒（停止不取消正在进行的拨号，M3b 延后第 8 行），照常往下 |
| ERROR | `cannot undo part of the enhanced mode; going on with the rest` | `step`、`error` | 关闭（含打开失败的回退）里一条撤销失败。`step` 是那条撤销动作的种类：`take_dns`、`adjust`、`rp_filter <值>`、`mac_dns`（macOS 的网络服务名不在这一行，在紧跟的 DEBUG 行里） |
| DEBUG | `the undo action that could not be undone` | `step`（完整的名字，含 macOS 的服务名） | 紧跟上一行 |
| ERROR | `state.json's enhanced backup cannot be read; it is left as it is and nothing is undone` | `error` | 启动的恢复读到读不懂的备份：什么也不撤销，备份原样留着 |
| ERROR | `state.json's enhanced backup cannot be read; what outlives the device is left as it is` | `error` | 关闭时读到读不懂的备份：其余各步照做，备份原样留着 |
| ERROR | `cannot follow the change; the enhanced mode goes on with what it has` | `step`、`error` | 重载或网络变化时某一步失败；`step` 是 `configure_ipv6`、`routes`、`dns_undo`、`dns_backup`、`take_dns`、`adjust_undo`、`adjust_backup`、`adjust`、`unconfigure_ipv6` 之一 |
| DEBUG | `the virtual adapter failed` | `error`（设备的原话） | 设备自己坏了。原因只在这里，下面那一行 ERROR 由主循环说 |
| ERROR | `the virtual adapter failed; the enhanced mode is off` | | 主循环收到"正开着的这一次的 TUN 坏了"（P24），关掉增强模式之前 |
| ERROR | `the enhanced mode is off after the adapter failed, but part of it could not be undone; what outlives rurge stays in state.json and is undone on the next start` | `error` | 上面那次关闭失败（不重试） |
| INFO | `restored what the enhanced mode left last time` | | 启动的恢复把备份里的撤销动作都做成了 |
| ERROR | `cannot undo what the enhanced mode left; it stays in state.json and is tried on the next start` | `error` | 启动的恢复里有撤销失败；失败的那几条留在备份里 |
| ERROR | `cannot switch the enhanced mode` | `error`、`enabled` | `POST` 的开关失败（API 这时回 500） |

接管暂停的那几行（`takeover paused on this network`、`takeover resumed`、`the takeover is paused on this network; the enhanced mode is left as it is, and set as asked when the pause ends`，以及两条 `cannot turn the enhanced mode …`）在下面「接管暂停：增强模式的那一半」，与系统代理那几行在同一张表里（本页「网络任务的日志行」）。

上面「启动参数与环境变量」与下面「`state.json`」里那些标准输出与标准错误不是日志行：它们照旧不受 `--log-level` 与 `--log-file` 影响，上表只讲 tracing 的行。

### `state.json` 的 `enhanced_backup` 与恢复

管理器自己在 `state.json` 里记一份备份（引擎不解释它，与 `system_proxy_backup` 一样）：只有"会在 rurge 死后还留在系统里"的改动才进备份——macOS 各网络服务的 DNS 原值、Linux 的 `net.ipv4.conf.all.rp_filter`；设备、地址、路由、按链路的 resolved 设置不进备份，它们随设备消失。

```json
{"enhanced_backup":{"undo":[{"kind":"mac_dns","service":"Wi-Fi","servers":["192.168.1.1"]},{"kind":"rp_filter","value":1}]}}
```

- 形状是 `{"undo":[…]}`，次序即做成的次序；每一项是 `{"kind": …}` 加自己的字段：`mac_dns`（`service`、`servers`，原来的服务器，没有手设过就是空表）、`rp_filter`（`value`，原来的值）、`test`（`step`，只供测试的文件后端用的那一种）。读不懂的一项让整份备份读不懂：`serde` 拒绝一个不认识的 `kind`。
- **什么时候写**：打开的第 6 步（接管系统 DNS）与第 8 步（系统自己的调整）之前各写一次，写的是"打开前备份里已有的 + 这一步新要的"，用 `try_update`（写进磁盘才换内存里的状态）；写不进时那一步不做，已经做过的倒着撤销，打开失败，错误是 `cannot write state.json: <系统的原因>`。重载与网络变化再做这两步时同样先写。
- **什么时候清**：关闭（含打开失败的回退）与启动的恢复里，做成一条就从备份里去掉一条，最后把剩下的写回；一条也不剩时 `enhanced_backup` 是 `null`，同时 `features.enhanced_mode` 写 `false`。这一步用 `update`：写不进只记一行 ERROR，下一次启动会把同样的几条再撤销一遍（撤销已经撤销过的东西是无害的）。
- `features.enhanced_mode`：打开的最后一步写 `true`，备份清空时写 `false`。启动时不读它——开关只看参数与 `POST`（M4-D15）——所以它只是给人和以后的会话看的记录。崩溃之后没有备份而它为真（Windows 的备份总是空的，Linux 不用改 `rp_filter` 时也是），启动的恢复也把它写回 `false`：那一次启动把上次留下的都算清了（协调者的裁定 R37；M4a 计划「执行期修正记录」第 99 行）。
- **启动时的恢复**：拿到数据目录的实例锁之后、任何新改动之前，与系统代理的恢复同一个位置（配置读不了、有错误、日志文件打不开的那三个出口也做，见 `docs/api/phase1.md` 的「配置加载不了时也恢复」一条）。备份里有撤销动作就**倒着**做（最后做的先撤），成功的从备份里去掉。有一句成功时是 INFO `restored what the enhanced mode left last time`，有失败时是 ERROR `cannot undo what the enhanced mode left; it stays in state.json and is tried on the next start`。
- 那三个没有日志的出口（配置读不了、配置有错误、`--log-file` 打不开）把结果写到标准错误，四句之一：`warning: a previous run left changes of the enhanced mode; they were undone`、`… ; they were undone, but state.json could not be updated: the next start undoes them again`、`warning: cannot undo what the enhanced mode left: <原因>; it stays in state.json and is tried on the next start`，以及取不到后端时的 `warning: <后端的报错>; the enhanced mode's changes were not undone`（`RURGE_ENHANCED_BACKEND` 的取值不对时）。都不改变退出码。
- **读不懂的备份照原样留着**：启动的恢复、关闭、打开三处都只记一行（前两处各一句 ERROR、打开是回一个以 `state.json's enhanced backup cannot be read` 开头的错误并且一个后端动作也不做），谁都不改写它。理由与系统代理一样：那可能是以后版本写下的形状，猜着撤销比不动更坏。

### 接管暂停：增强模式的那一半

生效的 `[SSID Setting]` 行写了 `suspend=true` 的网络上，接管暂停（本页「接管暂停时的系统代理端点」一节说的是系统代理那一半），增强模式照同一种语义（设计第 9.9 节、M4-D18）：

- **进入暂停**：开着的增强模式按关闭的次序关掉（系统 DNS 先交还），并记下"想要开着"。关从不半途停下：这一关之后它是关着的，失败只是某一条撤销没成——那一句 ERROR `the enhanced mode is off for the pause, but part of it could not be undone; what outlives rurge stays in state.json and is undone on the next start` 说的是没撤销成的那部分，它留在 `state.json` 里等下一次启动。离开暂停时想要开着就再开（不是在原来那一次上继续）。
- **离开暂停**：照记下的"想要"办：想要开着而关着的就打开（用的是这一刻这一代的配置与这个网络的 IPv6，失败记 ERROR `cannot turn the enhanced mode on when the pause ends; it stays off`，不重试）。"想要关着而仍开着就关掉"只对系统代理成立：它的恢复可以失败而仍指向 rurge；增强模式的关从不半途停下，所以没有这一支。
- **先增强模式、后系统代理**，两个方向都是（与退出的次序一致，M4-D16）：增强模式留下的是整台机器的 DNS，越早还越好。
- **启动时就在这样的网络上**：两样都不开，`--enhanced-mode` 与 `--system-proxy` 只记下想要的；`GET` 照实说 `false`，标准输出里没有 `enhanced mode on` 那一行。离开这个网络时才开。
- 暂停期间的 `POST /v1/features/enhanced_mode` 回成功 `{}`、只记下想要的，日志里一行 INFO `the takeover is paused on this network; the enhanced mode is left as it is, and set as asked when the pause ends`（字段 `enabled`）。
- 暂停只在内存里，不进 `state.json`。进入暂停时增强模式已经关掉、备份通常已经清空；关失败时模式仍是关的，只是没撤销成的那几条还在备份里，那一次崩溃之后的下一次启动照常撤销。所以暂停期间 rurge 崩溃，通常没有它的东西要恢复。
- "想要的状态"由三处决定，最后一次生效：`--enhanced-mode`、进入暂停时它是不是开着、暂停期间每一次得到成功应答的 `POST`。
- **设备自己坏了之后，"想要"不变**（P24）：管理器把增强模式整个关掉（交还 DNS、撤销会留下的），记 ERROR 一行，不重试；`Takeover` 记的"想要"仍是原来那个，所以离开 / 再进暂停时不会因为一次设备故障改变使用者的意图。`GET` 从此说 `enabled: false`。

> **2026-10-29 的订正（阶段 3 / M6）：上面“想要的状态由三处决定，最后一次生效”那一句比代码松。** 照 `crates/rurge/src/cli/run.rs` 的 `Takeover`（字段 `enhanced_wanted`，方法 `follow` 与 `switch_enhanced`）：“想要”起初是 `--enhanced-mode`；每一次得到成功应答的 `POST /v1/features/enhanced_mode` 都把它设成这一次要的值，暂停里外都一样（关闭回 500 也算成关：`disable` 之后先把 `enhanced_wanted` 设成 `false`，再把错误交回；打开失败不改它）；进入暂停时只有增强模式开着才把它记成开着，关着时不改。所以打开失败之后与设备自己坏了之后（P24 那一行），都是**模式关着而“想要”仍是开着**：下一次暂停结束时再试一次，进入暂停只关不开、不试；不在暂停里时，只有使用者再 `POST` 一次才会再开。原句松在两处：“进入暂停时它是不是开着”——代码只记“开着”，关着时不记成关；“暂停期间每一次得到成功应答的 `POST`”——代码里不论在不在暂停里，每一次成功的 `POST` 都算，关闭回 500 也算成关。P24 那一行说的对。兼容性清单 9.4 节 `suspend` 一行的写法与此一致（“进入暂停时只在模式开着时把它记成想要开着、关着时不改它”）。

### 重载与网络变化：只做差

一次成功的重载之后、一次网络变化之后（设计第 9.7、9.8 节），开着的增强模式按新的这一代与新的网络重算：要哪些路由（`tun-excluded-routes`、`tun-included-routes`、`external` 策略的 `addresses`、rurge 自己的地址空间）、`ipv6-vif` 说 IPv6 开不开、出口表的主网卡。**只做差**：`RouteSet::diff` 算出要加与要减的，成批加减，不加也不减时连后端都不叫。

- **IPv6 从关到开**：先配网卡的 IPv6 地址（`configure_ipv6`，答"没有"时一行 WARN、这一次按没有 IPv6 办），再按差加路由，再接管系统 DNS（IPv6 变了或网络变了，一次 `follow` 至多接管一次），最后在网络变化时再做一遍系统自己的调整（Linux 的 `rp_filter`）。
- **IPv6 从开到关**：**先交还系统 DNS**（它指着的应答器地址还在网卡上时交还最稳），再按差减路由，最后撤掉网卡的 IPv6 地址。这一条是协调者的裁定（M4a 计划「执行期修正记录」第 79 行），它订正了任务正文写的次序；理由：系统 DNS 指着一个网卡已经没有的地址时，每一次查询都要等到它超时才落到别处。
- **`ipv6-vif = auto` 的判据**（P18）：主网卡有 IPv6 网关（网络快照的 `routers` 里有 IPv6 地址，链路本地的也算——IPv6 的默认路由本来通常写的就是链路本地地址），且本机地址列表里有落在 `2000::/3` 的 IPv6 地址（平台给出的是主机位清零的 /64 网络地址，所以看的是前缀）。两个都成立才算这个网络有 IPv6，判一次、随网络变化再判。
- **网络变化时 DNS 与系统自己的调整再做一次**：新网络上可能多了（macOS）或换了一批网络服务、`rp_filter` 可能被网络管理程序改回去了。做法与打开时一样：先 `dns_undo` 读一次要撤销什么、写进备份（`try_update`），再 `take_dns`；先 `adjust_undo`，再 `adjust`。哪一步失败都只记一行 ERROR、后面的照做；备份写不进去时紧接着的那一步不做（崩溃之后系统是什么样就不知道了），与打开的规矩一致。
- **失败的步骤记 ERROR，增强模式照开**：已做的保留，不回滚到旧集合（旧集合本身也可能已经不对），`GET` 照实说 `enabled: true`。路由那一步失败时出口表留着"旧的与新的合起来"那一份（M4a 计划「执行期修正记录」第 75 行）：出口表始终是装进网卡的路由的**超集**，否则 rurge 自己的一条连接会回到虚拟网卡里（M4b 的读者按它判定，见设计第 7.1 节）。
- **路由的 IPv6 一半失败（`routes` 回 `Ok(false)`）**：后端只撤掉它新加的那一半、照常减掉要减的，所以网卡里剩下的 IPv6 路由是"旧的减掉要减的"（旧集与新的交集），管理器就记下这一份，出口表也换成它——仍是网卡里路由的超集。这一次调用之前 IPv6 没开着的（打开，或重载 / 网络变化把 IPv6 从关变到开），这一次按 IPv6 没开往下做（设计 M4-D17）：`on.ipv6` 为假，系统 DNS 以没有 IPv6 接管，打开时 INFO `enhanced mode on` 的 `ipv6` 字段也为假；IPv6 本来就开着的，照开着，不多接管一次 DNS。网卡上已有的 IPv6 地址留着（随设备走）。这一条是协调者的裁定 R33（M4a 计划「执行期修正记录」第 95 行），它覆盖了第 75 行里"`Ok(false)` 时只去掉 `next.v6`"那半句。
- 出口表在每次 `follow` 里都换成新的主网卡；路由不变时也一样（换网络可能只换了主网卡）。

### `rurge check` 的四个新码与 `external` 的 `addresses`

`rurge check`、`POST /v1/profiles/check`、启动与重载共用的诊断里，M4a 多了四个码。消息逐字照代码（`W0035` ～ `W0037` 在 `crates/rurge-config/src/general.rs`，`W0038` 在 `crates/rurge-config/src/config.rs`），建议逐字照 `crates/rurge-config/src/diagnostic.rs` 的 `codes::hint`。四个都只在一处报，都是缺省建议（`WITH_DEFAULT`）：`rurge check` 的文本、`--json` 的 `hint` 与 API 的 `hint` 都是那一句。

| 码 | 消息 | 建议 | 什么时候 |
| --- | --- | --- | --- |
| `W0035` | `` `ipv6-vif = always` may break apps on networks without IPv6 connectivity `` | `` use `ipv6-vif = auto` to turn IPv6 on only where the network has it `` | `[General]` 的 `ipv6-vif` 写成 `always`（手册的 Profile 检查也说这件事） |
| `W0036` | `` `tun-included-routes` takes in the whole private range {range} ({item}); this is normally unnecessary and can cause system issues `` | `` include only the addresses that have to go through rurge `` | `tun-included-routes` 的某一项**整个包含**了一段 RFC 1918 私网。`{range}` 是它包含的第一段，次序 `10.0.0.0/8`、`172.16.0.0/12`、`192.168.0.0/16`；一项一条，只判 IPv4 的项 |
| `W0037` | `` `tun-excluded-routes`: {item} overlaps rurge's own range (198.18.0.0/15 or fd00:6152::/64), which always goes through the virtual adapter `` | `` exclude only addresses outside 198.18.0.0/15 and fd00:6152::/64 `` | `tun-excluded-routes` 的某一项与 rurge 自己的地址空间相交（两个方向任一包含都算）。一项一条 |
| `W0038` | `` `server:force-syslib` asks the system's resolver, which answers with fake addresses while the enhanced mode is on `` | `` use `server:system` to keep real addresses while the enhanced mode is on `` | `[Host]` 的一行写成 `server:force-syslib`（那一行照样保留，照 Surge 的意思） |

消息里的 `{item}` 是**解析之后再写出来的那个值**：写法规范化（IPv6 写成压缩形式），**主机位照留**（`10.1.2.3/8` 就显示成 `10.1.2.3/8`），不是行上的原文，也不是把主机位清零之后的写法（M4a 计划「执行期修正记录」第 40 行的订正）。写不出来的项不参与这两个码（它有自己的 `W0012`）。

`external` 策略的 `addresses` 不再是被解析然后丢掉的（原来是 `W0029`）：它进了增强模式的路由计算——每一个地址是路由集里的一个 /32 或 /128 的**洞**（与 `tun-excluded-routes` 同样处理：那段地址不进虚拟网卡，`external` 程序自己的连接因此不回到网卡里）。只取会被起的 `external`（`Config::external_addresses`）；`udp-relay` 一类不影响这一条。

### 只供测试的后端

`RURGE_ENHANCED_BACKEND` 把这个管理器的"系统"换掉：不设时是平台后端（这一版对每个动作答"不可用"），`off` 是一个打开即失败的后端，`file:<路径>` 是把系统当一个 JSON 文件读写、设备什么也不运的后端。取值、文件的形状、`fail` 一类键、动作名与写进文件的东西见 `docs/testing.md` 的「只供测试的后端」一节。

### M4b：防环与接管期间的 DNS

阶段 3 / M4b（计划 `docs/superpowers/plans/2026-10-26-phase3-m4b-loop-dns-plan.md`，下面写“M4b 记录第 N 行”“M4b 延后第 N 行”指它末尾的两张表）做的是两件事：增强模式开着时，rurge 自己建的每一个出站套接字都不回到虚拟网卡；接管系统 DNS 的那段时间，`server:system` 一类的名字去问物理网卡的服务器。**平台后端仍然答“这一版还不能”，所以使用者仍然开不了增强模式**（本节开头那三句照旧）；下面这些行为只在用例里看得到：单元用例、`crates/rurge/tests/sockets.rs` 的源码扫描、`crates/rurge-engine/tests/egress.rs` 的运行时用例，以及经文件后端起的守护进程。

#### 出口的判定（防环）

出口是一个纯函数 `rurge_platform::socket::egress::decide`（`crates/rurge-platform/src/socket/egress.rs`），表是 `EgressTable`（`routes`、`primary`、`subnets`），由管理器在打开、关闭、重载与网络变化时重建，可执行文件的 `PlatformSockets::egress`（`crates/rurge/src/cli/runtime.rs`）读那一个槽。`rurge-net` 里出站套接字只有 `open_socket` 一处建法：它先照策略的 `interface` 绑，没有（或绑不上而 `allow-other-interface` 退回）时才问出口，按回答绑那块网卡。次序（设计第 7.1 节）：

| # | 目的地址 | 绑哪 |
| - | -------- | ---- |
| 1 | 增强模式没开（槽里没有表） | 不绑 |
| 2 | 回环、未指定、组播、链路本地、`255.255.255.255` | 不绑 |
| 3 | 落在某块非虚拟网卡的直连子网里（取最长前缀） | 那块网卡 |
| 4 | 落在装进网卡的路由集里（含 rurge 自己的 `198.18.0.0/15` 与 `fd00:6152::/64`） | 主网卡；没有主网卡时这次连接失败，错误是 `no network outside the enhanced mode to reach <目的地址>`（种类 `NetworkUnreachable`） |
| 5 | 其余（被排除的、`external` 的地址、IPv6 没开时的 IPv6） | 不绑，照系统的路由走 |

第 3 条在第 4 条之前：`tun-included-routes` 把一段局域网送进虚拟网卡时，rurge 自己到局域网的连接仍走局域网的网卡。绑上出口的网卡失败时这次连接失败、不退回不绑（退回就会进虚拟网卡），错误是 `cannot bind to the network outside the enhanced mode: <错误种类>`。

问出口的地方：`open_socket`；`DirectPacket` 的每个数据报（套接字按（地址族，出口）各一个，用到才建）；QUIC 直连的套接字（按服务端的地址）；`rurge-dns` 的纯 UDP 上游（经 `ResolverDeps::direct`，见下）；流水线回退的连接器；资源下载与 Profile 来源（`resource_client_with`）；ICMP 的回显套接字。建套接字的地方查得住的证据是 `crates/rurge/tests/sockets.rs` 的 `every_outbound_socket_is_made_in_one_place`：它读 `crates/*/src` 的生产代码，把建套接字、问系统解析库、建不带钩子的连接器的写法与一张 24 行的名单逐处比对，同一个文件里同一种写法的第二处也要一行，多一处少一处都变红（名单与改法见 `docs/testing.md`）。

目的地址先规范化成 `IpAddr::to_canonical`：`::ffff:a.b.c.d` 是 IPv4，按 IPv4 判第 3、4 条（双栈的 IPv6 套接字连它时发出去的是 IPv4 包）。只改判定，`DirectConnector` 拨号仍用按地址族选的那个套接字（“拨号前先规范化、改用 IPv4 套接字”这一种更彻底的做法没有采用，M4b 记录第 71 行）。

日志（网卡名与地址只在 DEBUG；两种错误文本不带网卡名）：

| 级别 | 文本 | 字段 | 什么时候 |
| ---- | ---- | ---- | -------- |
| DEBUG | `rurge's own connection leaves through the egress interface` | `dst`、`interface` | 绑上了出口判定的网卡（`rurge-net` 的 `bind_egress`） |
| DEBUG | `rurge's own connection cannot leave through the egress interface` | `dst`、`interface`、`error` | 绑不上；系统的原话与网卡名只在这一行 |
| WARN | `cannot read the subnets of the physical interfaces; rurge's own connections to them follow the primary interface` | `error` | 打开或跟随时读不到直连子网：表照写、子网是空的（第 3 条不起作用，第 4、5 条照旧） |
| WARN | `interface unavailable; using the default one (allow-other-interface)` | `interface`、`error` | 策略的 `interface` 绑不上而 `allow-other-interface` 退回时（M4b 之前就有，不是新的）：退回之后才问出口、由出口决定绑哪块网卡。每个 `DirectConnector` 只报一次 |
| INFO | `<协议名>: the egress of rurge's own sockets changed; a new connection is made` | `policy` | QUIC 策略（TUIC、Hysteria 2、MASQUE、TrustTunnel）发现自己的连接是按另一张出口表绑的：旧连接不再接新请求，下一个请求重拨（`rurge-proto-quic` 的 `on_route`） |
| INFO | `wireguard: the egress of rurge's own sockets changed; the peers are dialled anew` | `policy` | WireGuard 发现载体是按另一张出口表绑的：每个对端重拨，经它的连接照常（`rurge-proto-wireguard` 的 `Device::rerouted`，至多 250 毫秒一次） |

#### 出口表变了之后

直连的连接器（`DirectConnector`）的 `route()` 说出它的套接字是按哪一张出口表绑的（`SocketHook::egress_route` 答 `Some(Route::egress(&表))`，没有表时是 `None`）；`Route` 相等仍只比那个对象，所以同一张表是同一个 `Route`。活得比一次会话长的套接字靠它跟上表（M4b 记录第 68 行）：

- QUIC 的每条连接一个不连接的 UDP 套接字（TUIC、Hysteria 2、MASQUE、TrustTunnel）：连接被取用时比一次（一个 UDP 会话至多一秒一次），不同则不再接新请求、下一个请求重拨，那一行 INFO 换成了上面表里的那一句。
- WireGuard 的载体（连接了的 UDP，每个对端一个）：每个 `TICK`（250 毫秒）比一次，不同则每个对端重拨，那一行 INFO 同样换了。

管理器只在**表的内容变了**时才往槽里写另一张表（`set_egress`：内容相同时留着同一个 `Arc`，返回 `false`），否则每一次重载都会让所有 WireGuard 隧道与 QUIC 连接重拨一次（M4b 记录第 69 行）。跟随（重载或网络变化）写了另一张表而这一次不重新接管 DNS 时，管理器仍调一次 `system_dns_changed`：解析器的上游是按旧表绑的，下一个查询按新表重建它们（`Resolver::reread_system_servers` 也清 `[Host]` 的 `server:` 上游，M4b 记录第 67 行）。网络变化时引擎先按旧表重建、跟随后写新表的那一次序不变（设计第 9.8 节）：写下新表之后 WireGuard 与 QUIC 照 `route()` 重拨，DNS 的重新接管让解析器按新表重建（M4b 记录第 70 行）。

#### 非直连的 VPN 路由（登记的限制）

出口判定不读操作系统的整张路由表，只看直连子网（`if_addrs`）与装进网卡的路由集。所以**不是直连的 VPN 路由**上的目的地（VPN 推下来的 `10.0.0.0/8 via 10.8.0.1 dev tun0` 就是一例）：应用的流量照系统的路由进 VPN（那条路由比基础前缀更具体），rurge 自己到那里的连接却按第 4 条绑到主网卡、到不了。办法是把那一段写进 `tun-excluded-routes`——它落在第 5 条，不绑、照系统的路由走，rurge 自己的下载与 DNS 因此也进 VPN。

#### 接管期间的名字去哪里问

系统解析库在接管期间可能答假地址（系统的 DNS 指着 rurge，或 `hijack-dns` 把查询收走），所以 `SystemDns::enhanced_mode()` 为真的那段时间里，去处分四种（设计第 6.2 节）：

| 查找 | 不接管时（不变） | 接管期间 |
| ---- | ---------------- | -------- |
| `[Host]` 的 `server:system` / `server:syslib` | 系统解析库 | 物理网卡的服务器（纯 UDP、经直连连接器）；没有已知的服务器时这次查找失败，**不退回系统解析库** |
| `[Host]` 的 `server:force-syslib` | 系统解析库 | 系统解析库（照手册，会拿到假地址；加载时 `W0038` 说过） |
| 单标签名字（加上搜索域之后）、`primary` 为空时的普通查找 | 系统的服务器，没有时系统解析库 | 物理网卡的服务器，没有时失败 |
| `.local` | 系统解析库 | 系统解析库（mDNS 不经单播 DNS） |

没有服务器可问时的错误是 `all upstreams failed (system DNS servers: no DNS server outside the enhanced mode)`（`DnsError::AllFailed` 的一对，`crates/rurge-dns/src/resolver.rs` 的 `no_server_outside`）。物理网卡的服务器是 `PlatformSystemDns::servers()`：增强模式开着且 `physical` 有值就是它，否则是 `rurge_platform::dns::servers()`。**`physical` 今天没有人写**（M4c 的 macOS 写备份里的原值与 `ipconfig getpacket`，Linux 写 resolved 的上游名单；Windows 不需要它），所以两边给出的是同一份名单。

系统的 DNS 名单一律去掉 rurge 的地址空间：`rurge_platform::dns::without_rurge_space`（`servers()` 的每一项）与网络环境的快照（`netenv` 读到的 `dns`）都经过它，**不只在接管期间**。理由：没有真的 DNS 服务器在 `198.18.0.0/15` 或 `fd00:6152::/64` 里，而崩溃之后系统 DNS 还指着应答器的那段时间，这样它也不会把查询送回一个不在的应答器。

接管与交还之后解析器要重读系统的服务器：`Engine::system_dns_changed` 调 `Resolver::reread_system_servers`（重建系统那一组、引导与“主服务器就是系统的”那一组，**不清缓存**；经接口建的上游随之清掉，下一次按新服务器重建）。管理器在增强模式的标志变了、接管之后、交还之后各调一次。它与 `network_changed` 是两件事：后者是网络真的变了（清缓存、读 IPv6、重读服务器、清 `[Host]` 的上游），照旧只在网络真的变了时调。

#### 资源下载与 Profile 来源

守护进程下载外部资源与 Profile 来源用的是 `resource_client_with(钩子, OwnResolve)`：直连带平台钩子（所以按出口绑网卡），名字由 `OwnResolve` 查——平时是系统解析库，接管期间（`enhanced_mode()` 为真、且这一代的解析器已经发布）改问这一代的解析器，于是拿到真地址。一个例外：`rurge run` 启动时、引擎建起来之前等 Profile 来源的那一次下载（`run.rs` 的 `wait_for_the_sources`）用 `resource_client()`：那时引擎还没有、增强模式还开不了，出口表是空的，带了钩子也只会答“不绑”；用例也用它（不绑、走系统解析库），不起引擎的命令（`rurge check` 一类）不下载、也不调它。

#### ICMP 回显

TUN 的 `Pinger` 现在是可执行文件的 `PlatformPinger`（`crates/rurge/src/cli/pinger.rs`），套接字由 `rurge_platform::icmp` 建：每个（地址族，出口）一个、用到才建；标识符是这个 Pinger 自己的随机数，序号逐个加。套接字的种类按平台：Linux 先数据报（不要特权，受 `net.ipv4.ping_group_range` 管）、不行退原始；macOS 数据报；Windows 原始（**要管理员**，增强模式本来也要）。原始套接字与 macOS 的数据报套接字按标识符与序号对号，Linux / Android 的数据报套接字只按序号对（内核改写了标识符）；macOS 这一条没有在 macOS 上观察过（M4b 记录第 46 行）。建不出套接字时一行 WARN 说回显不可用（这次运行的第一条，之后每次 DEBUG），`ping` 经虚拟网卡因此不通，别的照旧。

日志：

| 级别 | 文本 | 字段 | 什么时候 |
| ---- | ---- | ---- | -------- |
| WARN | `icmp: echo requests are unavailable, so pings go unanswered` | `family`、`reason` | 建不出套接字；一个 `PlatformPinger` 只报一次（之后的失败是 DEBUG） |
| DEBUG | `icmp: no socket for an echo request` | `family`、`error` | 同上，第一条之后 |
| DEBUG | `icmp: the echo socket could not be bound` | `interface`、`error` | 绑出口的网卡失败；进连接的错误文本照出口那一句 |
| DEBUG | `icmp: echo requests leave through an interface` | `interface`、`family` | 绑上了出口的网卡 |
| DEBUG | `icmp: reading a reply failed` | `error` | 收包出错；接连第二次起，下一次读之前退避 50 毫秒（第一次不退避） |

#### 外部程序一律 DIRECT

`external` 策略拉起的程序（以及 Unix 上它起的子进程）的会话不看规则与出站模式，一律 `DIRECT`（设计第 11 节）：引擎的 `choose_policy` 在看出站模式之前先看“有没有外部程序在跑”（`ExternalPrograms::any_running`），有才等这个会话的进程（上限照旧 `PROCESS_WAIT`，200 毫秒），PID 在某个程序的进程树里（`ExternalOutbound::runs`，进程树是 `ProcessTree::contains`）就走 DIRECT；请求记录的 `notes` 写 `Requested by an external program: always DIRECT`，另有一行 DEBUG 带 `session` 与 `pid`。没有程序在跑时一个字也不多等；进程的槽已经等过时规则评估复用同一个，不等两次。

日志：程序的生命周期三行 INFO（`external: the program started`、`external: the program exited`（带 `code`）、`external: the program stopped`）只带策略名；程序的 PID 在启动之后那一行 DEBUG `external: the program's process id` 里（进程的名字、路径、PID 只进 DEBUG，M4b 记录第 74 行）。

登记的限制：

- **预匹配先于它**：预匹配规则在 DNS 与 SYN 两层先判，外部程序自己发往被预匹配拒绝的名字或地址的查询与连接照旧被拒绝，会话不成为一条请求。
- **`setsid` 的子进程不算**：Unix 上认的是“与程序同一个进程组”，自己 `setsid` 出去的子进程不在里面，它的会话照规则走。
- **进程查找超时**：等进程超过 `PROCESS_WAIT` 时不把会话当外部程序的。规则模式下照规则走，`notes` 里照旧是 `Process not known within 200 ms: PROCESS-NAME did not match`；直连与全局（`outbound-mode = direct` / 指定了 `global-policy`）模式下在等进程之前就定了去向，照出站模式走，不记这一句（`crates/rurge-engine/src/engine.rs` 的 `choose_policy`）。
- **Windows 上 M4d 之前只认程序本身**：`ProcessTree::contains` 在 Windows 上恒答 `false`（`in_job` 是 M4d 的），所以 `runs(pid)` 只认程序自己的 PID；程序拉起的子进程的会话要等 M4d 的 `in_job` 才认得出来。

#### 打开那一刻与之后出现的网卡（登记的限制）

- **表变之前建的会话的套接字不重建**：每一个已经建立的会话的 TCP 连接（含 DoH、HTTP/2、SSH 的连接池）与 UDP 中继，以及旧 QUIC 连接上已经在走的流，都是按建它们时的那张表绑的。表变的那一刻（增强模式打开、或者换了网络）它们经虚拟网卡绕一次或断一次，之后新建的照新表。WireGuard 的包最晚到下一个 `TICK`（至多 250 毫秒）才按新表发出，那之前进虚拟网卡（M4b 记录第 68 行、延后第 16 行）。
- **直连子网只在打开与跟随时读**（设计第 7.1 节）：打开之后才出现的次网卡（`docker compose up` 建的网桥、后接的第二块网卡、USB 共享）的子网要到下一次跟随（重载或网络变化）才进表，在那之前 rurge 自己到那里的连接绑在主网卡上、到不了（M4b 延后第 14 行）。

### M4c：Linux 与 macOS

阶段 3 / M4c（计划 `docs/superpowers/plans/2026-10-26-phase3-m4c-linux-macos-plan.md`，下面写“M4c 记录第 N 行”“M4c 延后第 N 行”指它末尾的两张表，写“P N”指它的「计划期决定」）做的是这两个平台的系统一侧：虚拟网卡、地址与路由、系统 DNS 的接管、Linux 的 `rp_filter`，以及接管期间问哪里的 DNS 服务器。**本节订正上面几句，那是 M4a 与 M4b 当时的事实**（规矩是历史的段落不逐处改、由后写的段落订正）：

- M4 一节开头粗体的“这一版开不了增强模式”与它下面三条，在 Linux 与 macOS 上不成立了：`rurge run --enhanced-mode` 与 `POST /v1/features/enhanced_mode` 带 `{"enabled": true}` 有了平台后端的代码（不再答“不可用”），照代码会建虚拟网卡、配地址、装路由、接管系统 DNS，Linux 上按需要改 `net.ipv4.conf.all.rp_filter`；**其中开设备的代码与挑平台的 `cfg` 一次也没有编译过（这台开发机编译不到，CI 还没有见过），动系统的那一半在这台开发机上编译、只用假的执行器测过，真机上都没有跑过**。**Windows 上照旧**：平台后端仍是 `Unsupported`，答 `the enhanced mode is not available on this platform in this version`，以 M4d 为准。
- 「只供测试的后端」里“（这一版对每个动作答‘不可用’）”与 M4b 一节里“平台后端仍然答‘这一版还不能’，所以使用者仍然开不了增强模式”同样以这一段为准：不设 `RURGE_ENHANCED_BACKEND` 时，Linux 与 macOS 用的是这一节说的平台后端的代码（照代码会做那些事；开设备与挑平台的代码一次也没有编译过，动系统的一半只用假的执行器测过，真机上都没有跑过），Windows 才是那个什么都答不行的 `Unsupported`。上面的出口判定、接管期间的 DNS、外部程序一律 DIRECT 那几节写的道理不变，只是 `physical` 从此有人写。
- 下面「没有覆盖的」一节里的“没有真实的设备”、“接管期间的去处只在单元用例里”那句里的“`physical` 今天没有人写（M4c）”、与“防环没有在真机上验过”里“随 M4c 接上平台后端”那半句，也都不成立了；今天还没有做到的事以本节末尾的「没有覆盖的」为准。

**这两个平台开设备与挑平台的代码在这台 Windows 开发机上编译不到，CI 还没有见过 M4c 的提交**（最后一次被 CI 见过的运行是 `ac0a98a` 上的 40283632986）；动系统的那一半不带 `cfg`，在这台开发机上编译、只用假的执行器测过；真机上都没有跑过。所以下面写的是代码的行为，不是观察到的行为。

#### 打开时改了什么，怎样回去

两个平台共用 M4a 的路由集（rurge 自己的 `198.18.0.0/15` 与 `fd00:6152::/64`、`tun-included-routes`、`tun-excluded-routes`、`external` 的 `addresses`）与 M4b 的出口表；下面只说每一步真的动了系统的什么。两个平台上**只加自己要加的，从不换掉别人已经装的**：前缀已经在时打开失败（见「另一个全隧道 VPN 开着时」）。

| 改什么 | Linux | macOS | 怎样回去 |
| --- | --- | --- | --- |
| 虚拟网卡 | `/dev/net/tun`，名字 `rurge`，`IFF_TUN \| IFF_NO_PI`（`crates/rurge-platform/src/tun/linux.rs` 的 `set_iff`）。不持久：描述符一关，网卡连同它的地址与路由一起没有 | utun：`com.apple.net.utun_control` 的控制套接字，名字由内核挑（`utunN`，`crates/rurge-platform/src/tun/macos.rs` 的 `utun_connect`） | 设备随关闭与进程结束消失；`PlatformBackend::close` 丢掉最后一个所有者 |
| 地址与 MTU | `ip -batch`：`link set dev rurge mtu 1500`、`addr add 198.18.0.1/15 dev rurge`、`link set dev rurge up`；IPv6 单独一批：`addr add fd00:6152::1/64 dev rurge` | `ifconfig utunN inet 198.18.0.1 198.18.0.1 netmask 255.254.0.0 mtu 1500 up`；IPv6 单独一条：`ifconfig utunN inet6 fd00:6152::1 prefixlen 128 alias`（`/128` 与 `alias` 的理由见 P5） | 随网卡走。IPv6 从开到关时显式撤一次：`addr del …` / `ifconfig … inet6 … delete` |
| 路由 | `ip -batch`，一行一条 `route add <前缀> dev rurge`（IPv4 一批，IPv6 一批）；减的时候 `route del` 一批，`ip -force -batch` 走过失败的 | 一条一个进程：`route -n add -inet <前缀> -interface utunN`（IPv6 用 `-inet6`） | 随网卡走；打开失败与重载 / 网络变化时显式减（`route del` / `route -n delete`，减不掉不算错） |
| 系统 DNS | 只在 `/etc/resolv.conf` 是 systemd-resolved 的存根（唯一的服务器是 `127.0.0.53`）且找得到 `resolvectl` 时接管，且只是这一条链路的设置，次序是：`resolvectl llmnr rurge no`、`resolvectl mdns rurge no`、`resolvectl dnssec rurge no`、`resolvectl dnsovertls rurge no`（四步都是尽力而为，失败不算错：全局的 DoT 会让发往 `198.18.0.2` 的查询走 853 上的 TLS、DNSSEC 验不过 fake 应答、LLMNR / mDNS 会把单标签名字与 `.local` 的组播发到这条链路上），然后 `resolvectl dns rurge 198.18.0.2 [fd00:6152::2]`、`resolvectl domain rurge ~.`（这两步失败就是这次不接管）、`resolvectl default-route rurge yes`（失败不算错，P4）、`resolvectl flush-caches`（失败不算错：只是缓存里已有的名字到期之前照旧是真地址） | 每一个启用的网络服务：先 `networksetup -getdnsservers <服务>` 读原值，再 `networksetup -setdnsservers <服务> 198.18.0.2 [fd00:6152::2]`；全部改完 `dscacheutil -flushcache` 与 `killall -HUP mDNSResponder` | Linux 的设置随链路走，设备没了就没了（交还时另做一次 `resolvectl revert rurge`）；**macOS 的改动会留在系统里**：原值进 `state.json` 的备份，撤销时按原值写回（原来一条也没有的写 `networksetup -setdnsservers <服务> empty`），服务已经被删掉的算做完（P9） |
| `rp_filter` | 只在主网卡的有效值（`all` 与主网卡自己的两个值里大的那个）是 1 时，把 `all` 写成 2（直接写 `/proc/sys/net/ipv4/conf/all/rp_filter`，不经 `sysctl`） | 没有这一步 | **会留在系统里**：`all` 的原值进备份，撤销时写回 |
| 脚本文件 | 数据目录的 `enhanced/` 下，`rurge-steps-<pid>-<n>.txt`，程序读完就删 | 没有这一步：macOS 每一步一个进程，`route/macos.rs` 里没有 `SCRIPT`，`ifconfig` / `route` / `networksetup` / `ipconfig` 的参数都在命令行上 | 跑完就删；崩溃留下的在下一次 `open` 时删（P12） |

macOS 上读不了原值的那个服务不接管（WARN 一行，它照旧按原来的服务器解析——P8）；一个服务也接管不了时 `take_dns` 回 `Ok(false)`，与 Linux 没有 resolved 的情形一样，增强模式照开。

#### 日志行

与上面 M4 一节那张表同一个读法：INFO 与 ERROR 是缺省级别看得到的（`loglevel = notify`），DEBUG 要 `--log-level debug`；**网卡名、网络服务的名字、DNS 服务器与脚本的内容只在 DEBUG**（`CLAUDE.md`「日志的分级」）。下表是 M4c 加的行。

| 级别 | 文本 | 字段 | 什么时候 |
| --- | --- | --- | --- |
| INFO | `loosened net.ipv4.conf.all.rp_filter to 2: replies to rurge's own connections come in on another interface than the routes to their senders` | | Linux 打开的第 8 步或一次网络变化放宽了 `all`（`LinuxSystem::adjust`）：主网卡的有效值是 1 时，或备份里已有 `all` 的原值、换网络之后再写一次时；不需要放宽的机器不写、也没有这一行（F1，M4c 记录第 40 行） |
| INFO | `put net.ipv4.conf.all.rp_filter back to {value}` | | 撤销那一条，写回原值（`LinuxSystem::undo`） |
| INFO | `a network service the enhanced mode changed is gone; there is nothing to put back for it` | | macOS 撤销时那个网络服务已经不在了，算做完（P9） |
| WARN | `the system's DNS is not systemd-resolved's stub; it is left as it is: write hijack-dns = *:53, or point the system's DNS at 198.18.0.2` | | Linux 不接管系统 DNS：`/etc/resolv.conf` 不是存根，或找不到 `resolvectl`。一次打开至多一行（重载不再写） |
| WARN | `the DNS servers of a network service could not be read; its DNS is left as it is` | | macOS 某个服务的 DNS 读不出来（命令失败，或输出不是地址表）：那个服务不接管（P8），一个服务一行 |
| WARN | `no network service's DNS could be pointed at the enhanced mode` | | macOS 一个服务也接管不了（`take_dns` 回 `Ok(false)`），一次打开至多一行 |
| WARN | `an undo of another platform is dropped` | | 撤销动作是另一个平台的（数据目录从别的机器带过来）：`LinuxSystem::undo` 见到 `mac_dns`，`MacSystem::undo` 见到 `rp_filter`（P9） |
| WARN | `an undo the platform backend does not know is dropped` | | 撤销动作平台后端不认识（文件后端写下的 `test` 一类）：`PlatformBackend::undo`（P9） |
| WARN | `the IPv6 routes could not be added to the virtual adapter; they are left out` | | 加路由的 IPv6 一半失败（`enhanced::run_routes`，协调者的裁定 R15）。**上面 M4 一节那张表里的 `IPv6 could not be configured; the enhanced mode goes on without it` 照旧只管地址那一半** |
| WARN | `the system had no room for a packet; the enhanced mode drops such packets and counts them` | `error` | 设备写不进一个包（`tun/mod.rs` 的 `write_all`，P14）：这一次运行的第一条，之后每条 DEBUG；包丢掉并计进 `dropped()` |
| DEBUG | `running a step of the enhanced mode` | `program`、`args`、`script`（脚本的全文，没有脚本时没有这个字段） | 每个要执行的程序之前（`route::exec::run_step`）。命令行里有网卡名、路由前缀与脚本的路径，脚本里有前缀与网卡名 |
| DEBUG | `running a command of the enhanced mode` | `program`、`args` | macOS 的每条 `networksetup` 与 `ipconfig` 之前（`MacSystem::command`）：`args` 里有网络服务的名字 |
| DEBUG | `a step of the IPv6 half failed` | `error` | 标 `Ipv6Off` 的一步失败（`route::exec::run`）：告警由调用方写（R15），这里只有 DEBUG |
| DEBUG | `a step that may fail failed; going on` | `error` | 标 `Ignore` 的一步失败（`route::exec::run`）：撤销、`default-route`、`flush` 一类 |
| DEBUG | `a packet with no room in the system is dropped` | `error` | 上面那条 WARN 之后的每一次（`tun/mod.rs` 的 `write_all`） |
| DEBUG | `a datagram from the virtual adapter that is no IP packet is skipped` | `read`（这一次读到的字节数） | macOS 从 utun 读到的数据报前面不是已知的地址族、或后面没有包（`tun/macos.rs` 的 `skipped`） |
| DEBUG | `cannot read /etc/resolv.conf` | `error` | Linux 判存根、macOS 记接管之前的服务器时读不出来：分别当成“不是存根”与一份空名单（macOS 的空名单当不知道，往下读服务原值与 DHCP，F8） |
| DEBUG | `no resolvectl in the system directories: the resolver's per-link settings cannot be made` | | Linux 找不到 `resolvectl`（systemd 239 之前）：不接管系统 DNS |
| DEBUG | `cannot read resolved's resolv.conf; no servers outside the enhanced mode are known` | `error` | Linux 接管着、但读不出 `/run/systemd/resolve/resolv.conf`：`server:system` 的名单是空的 |
| DEBUG | `cannot list the network services' devices` | `error` | macOS 读 `-listnetworkserviceorder` 失败：找不到主网卡所在的服务，只能去问 DHCP |
| DEBUG | `the DNS servers of a network service are not a list of addresses` | `service`、`output` | macOS 某个服务的 `-getdnsservers` 输出读不懂：服务的名字与工具的原话只在这一行 |
| DEBUG | `cannot read the DNS servers of a network service` | `service`、`error` | 同上，命令本身失败 |
| DEBUG | `cannot point a network service's DNS at the enhanced mode` | `service`、`error` | 改一个服务的 DNS 失败；回到管理器的错误是固定的一句（P7） |
| DEBUG | `cannot put back a network service's DNS servers` | `service`、`error` | 撤销一个服务的 DNS 失败，同上 |
| DEBUG | `the network service is gone` | `service` | 撤销时那个服务不在 `-listallnetworkservices` 里了，紧跟一行 INFO |
| DEBUG | `cannot read the DHCP packet of the main interface; no servers outside the enhanced mode are known` | `error` | macOS 换网络之后 `ipconfig getpacket` 读不出来：`server:system` 的名单是空的 |
| DEBUG | `cannot read the script directory`、`cannot remove a script left behind`、`cannot remove a script` | `path`、`error` | 脚本目录与脚本文件的清理失败（`route::exec`）：留下的就是下一次 `open` 删的残留 |

#### 接管期间 `server:system` 去哪里问

上面的「接管期间的名字去哪里问」一表说的是四种去处；物理网卡的服务器由平台一侧给（`System::physical_dns`，管理器的 `read_physical` 抄进 `PlatformSystemDns`）。两个平台各一段：

- **Linux**：问的是 resolved 的上游名单，`/run/systemd/resolve/resolv.conf`——resolved 写给整台机器的那一份（P10）；它只在这一次真的接管了系统 DNS 之后才有（没接管时回答“机器自己的列表是对的”，即 `None`，用 `rurge_platform::dns::servers()`）。文件读不出来时是空的列表：查找失败，**不退回机器自己的列表**。
- **macOS**：打开时是接管之前的 `/etc/resolv.conf`（`dns_saved` 在改任何服务之前读的那一份；崩溃之后它可能还指着应答器，去掉 rurge 的地址空间之后可能是空的）。**空的当不知道，不当成“没有服务器”**：往下读服务原值与 DHCP（F8，M4c 记录第 47 行）——`/etc/resolv.conf` 由 configd 异步重写，关闭之后马上再打开、或崩溃恢复刚写回就打开时，读到的可能只有应答器。换过网络之后那一份作废，改成主网卡所在的那个网络服务在备份里原来手动设的服务器；一条也没有的，是 `ipconfig getpacket <主网卡>` 给的 DHCP 服务器；两个都没有时是空的列表，查找失败、不退回系统解析库。没有主网卡时（没有路由可以出去）同样是空的列表。

三处都先去掉 rurge 的地址空间（协调者的裁定 R16）：使用者照那句 WARN 的提示把服务的 DNS 手动设成 `198.18.0.2` 时，原值就是应答器，问它必然失败。**读的那一步本身没有回来**（阻塞的任务没了）也是空的列表，不是“机器自己的列表”（协调者的裁定 R19）：`PlatformSystemDns` 里的槽留着 `Some(空)`，只有后端自己该答 `None` 的那几种（Linux 还没接管、文件后端没有那个键、`RURGE_ENHANCED_BACKEND=off`、以后的 Windows）才是 `None`。

**IPv6 路由器通告（RDNSS）给的服务器，这两种读法都读不到**（P10），登记为 rurge 的限度：接管期间 `server:system` 问不到只由 RA 分发的那一台。Windows 不需要这一段（M4d）。

#### 启动时的恢复多了一处

「`state.json` 的 `enhanced_backup` 与恢复」里写的那个位置（拿到实例锁之后、任何新改动之前）照旧成立；M4c 多了一处同样的恢复（协调者的裁定 R10，M4c 记录第 32 行）：要下载 Profile 的远程来源时，`run.rs` 的 `wait_for_the_sources` 在拿到实例锁之后、印 `waiting for the profile's remote sources (at most N s)` 与开始下载之前调一次 `lock.recover(&args.runtime)`。理由：那些下载问的是系统解析库，macOS 上崩溃留下的系统 DNS 还指着已经不在的应答器时不先恢复，就要等到时限。拿到的是 `Lock::Taken(None)` 时什么也不做；恢复两次无害（撤销已经撤销过的东西是安全的）。

#### 权限不够与网卡正在被使用

打开设备的失败按 `io::ErrorKind` 换成一句说明（`crates/rurge-platform/src/tun/frame.rs` 的 `explained`，P16），错误种类照系统的原样留给调用方；不是下面这几种的，原话是 `cannot open the virtual adapter: <系统的原话>`。

| 平台 | 情形 | 文本 |
| --- | --- | --- |
| Linux | `PermissionDenied` | `the enhanced mode needs root (in a container, root with CAP_NET_ADMIN)` |
| Linux | `NotFound`（`/dev/net/tun` 不在） | `there is no /dev/net/tun: the kernel's tun module is not loaded` |
| Linux | `ResourceBusy` | `the virtual adapter is in use: is another rurge running with the enhanced mode on?` |
| macOS | `PermissionDenied` | `the enhanced mode needs root` |
| 两个平台 | 其余 | `cannot open the virtual adapter: <系统的原话>` |

它出现在 `rurge run --enhanced-mode` 的标准错误（`error: cannot turn the enhanced mode on: <这一句>`）与 `POST /v1/features/enhanced_mode` 的 500 体（`{"error":"<这一句>"}`）里；打开失败已经把它做过的倒着撤销了。Linux 那一句只说 root：`ip` 与 `resolvectl` 是机器上的外部程序，只给 rurge 自己的文件能力（`setcap`）到不了它们（`ip` 配地址那一步才以 `Operation not permitted` 失败），而容器里的 root 又可能被拿掉 `CAP_NET_ADMIN`（F7，M4c 记录第 46 行）。

#### 脚本与程序

- 要程序读的一批命令写成脚本文件，放在数据目录的 `enhanced/` 下（以 root 运行时那个目录属于 root；不写 `/tmp`，别人可能在那里预先放好符号链接），名字 `rurge-steps-<pid>-<n>.txt`，用 `create_new` 建（P12）。程序跑完就删；删不掉不算错，崩溃留下的在下一次 `open` 时由 `exec::remove_leftovers` 删掉，只删这个名字形状的普通文件。**只有 Linux 走脚本**：macOS 每一步一个进程，命令行的参数就是全部（`route/macos.rs` 里没有 `SCRIPT`）；Windows 的 `netsh -f` 以后也写（M4d）。写了脚本的步骤失败时，错误里脚本的路径每一处都换成 `…`（命令行的与工具原文里的都换，`ip -batch` 自己会写 `Command failed <路径>:<行>`），DEBUG 那一行照旧带路径（F5，M4c 记录第 44 行）。
- 程序一律用绝对路径（`LinuxTools::find` 在 `/usr/sbin`、`/sbin`、`/usr/bin`、`/bin` 里找，不查 `PATH`），一律经 `TimedRunner` 执行，**每个程序 30 秒为限**（P11）：到时杀掉，那一步失败，挂住的工具（比如 D-Bus 卡住的 `resolvectl`）不会让主循环一直等。一批几十条路由的 `ip -batch` 只要毫秒；`networksetup` 慢的时候要几秒。
- 程序与参数、脚本的内容都只在 DEBUG（上面那张表的两行 `running a step …` / `running a command …`）：macOS 的命令行里有使用者自己起的网络服务名。带服务名的命令失败时，回到管理器的错误是固定的一句，工具的原文只在 DEBUG（P7）。

#### 另一个全隧道 VPN 开着时

另一个程序装着与 rurge 那八个前缀之一相同的路由（OpenVPN 的 `redirect-gateway def1` 装 `0.0.0.0/1` 与 `128.0.0.0/1`，后者与其中一个相同，就是一例）时：两个平台都**打开失败**再倒着撤销，错误里带工具的原文（`RTNETLINK answers: File exists` 一类，P3、P22）。rurge 不换掉也不绕开别人的路由——Linux 从前设想的 `route replace` 会悄悄换掉它，关闭时再把它删掉，那个 VPN 从此漏流量，所以改成了 `route add`。办法只有一个：**先关掉那个 VPN** 再开增强模式。

#### 手工恢复 macOS 的 DNS

rurge 崩溃之后的下一次启动会自己撤销（上面的「`state.json` 的 `enhanced_backup` 与恢复」与「启动时的恢复多了一处」），但机器上可能等不到那次启动。手工恢复的办法是给每个被改过的网络服务写回原值：

```bash
networksetup -listallnetworkservices                      # 带 * 的是停用的服务
sudo networksetup -setdnsservers "Wi-Fi" 192.168.1.1      # 写回原来的服务器
sudo networksetup -setdnsservers "Wi-Fi" empty            # 原来一条也没有设过
sudo dscacheutil -flushcache; sudo killall -HUP mDNSResponder
```

原值在数据目录的 `state.json` 里：`enhanced_backup.undo` 里每一项 `kind` 是 `mac_dns` 的，`service` 是服务名、`servers` 是原来的服务器（空表就是“原来一条也没有”）。rurge 自己撤销时做的就是这几条命令。

#### 没有覆盖的

- **两个平台的设备与系统改动没有在真机上跑过**：Linux 与 macOS 开设备与挑平台的代码在这台 Windows 开发机上编译不到，CI 还没有见过 M4c 的提交（最后一次被 CI 见过的运行是 `ac0a98a` 上的 40283632986）；设备、地址、路由、`networksetup` 那几条命令一条也没有在真机上执行过。系统一侧（`LinuxSystem` / `MacSystem`）只在开发机上用假的执行器与临时目录测过。
- **L2 特权用例写好了，没有运行过**（计划任务 9；门控与跑法见 `docs/testing.md` 的「阶段 3 / M4c 起」与 `tests/privileged/README.md`）：经虚拟网卡的一个往返、rurge 自己的 DNS 查询不进虚拟网卡、`rp_filter` 的放宽与写回，都还没有在虚拟机上跑过。
- **macOS 的 `-listnetworkserviceorder` 输出格式与公开的样本对得上**（两个出处：Apple Support Communities 的帖子贴的原样输出，每个服务两行 `(N) 名字` 与下一行 `(Hardware Port: …, Device: …)`，VPN 服务的 `Device:` 可以是空的、服务名里可以有括号；Jamf Nation 的帖子是另一种排版的同一份）：服务与网卡的配对、与 `ipconfig getpacket` 周围的行走仍要等手工清单核对（M4d；M4c 记录第 49 行、延后第 2 行仍然开着）。
- **交给管理器的物理网卡服务器没有端到端用例**：`run.rs` 里把 `physical` 交给管理器的那一行没有用例走到（`server:system` 在测试里会去问这台机器真的系统 DNS，失败时就离开了回环，`CLAUDE.md`「测试约定」）。
- **Windows 照旧不能开**：平台后端在 Windows 上仍是 `Unsupported`，`rurge run --enhanced-mode` 以 1 退出、`POST` 回 500，见 M4d。
- **手工验收没有做**：`docs/acceptance/phase3-manual.md` 还没有 M4 一节（只有 M1、M2、M3），M4d 写它。

### M4d：Windows 与收尾

阶段 3 / M4d（计划 `docs/superpowers/plans/2026-10-26-phase3-m4d-windows-wrapup-plan.md`，下面写“M4d 记录第 N 行”指它末尾的「执行期修正记录」）做的是 Windows 那一半：Wintun 的设备、`wintun.dll` 的查找与签名校验、`netsh` 的步骤与读回、`--wintun-dll`，以及 M4 的文档收尾。**本节订正上面几句**（规矩是历史的段落不逐处改、由后写的段落订正）：

- M4 一节开头的“这一版开不了增强模式”在**三个平台**上都不成立了：Windows 的平台后端不再答 `Unsupported`，它答 `WindowsSystem` 与 Wintun 的设备——照代码会建名叫 `rurge` 的适配器、配地址与 MTU、装路由、把系统 DNS 指向应答器，关闭或崩溃之后的下一次启动把它们还回去。**Windows 的代码与不带 `cfg` 的部分在这台 Windows 11 x86-64 的开发机上编译、单元用例通过（一台机器上的一次运行，不是 CI）；Wintun 的设备与它的十个放开的函数（任务 5）没有在任何地方运行过**（没有一条缺省的用例开设备，`RURGE_TEST_WINTUN_DLL` 也不设，M4d 记录第 70 行），真机上也没有跑过；任务 3 的 `verify_signer` 与 `load` 在本机的单元用例里跑过（校验测试二进制自己的签名、载入 System32 的 `kernel32.dll`、一个不是 DLL 的文件），真的 `wintun.dll` 上没有（手工清单的 W36）。
- 「外部程序一律 DIRECT」一节里“Windows 上 M4d 之前只认程序本身”与本节末尾「没有覆盖的」里“Windows 的 `ProcessTree::contains` 恒假”都不成立了：Windows 上它经 `in_job`（`IsProcessInJob`，M4d 任务 1）回答，程序拉起的子进程算在里面；端到端用例 `what_the_program_started_goes_direct_too` 三个平台都编译、在这台开发机上跑过（`crates/rurge-platform/src/process.rs` 的三条用例 `a_child_in_the_job_is_contained_and_this_process_is_not`、`an_exited_child_whose_handle_is_still_open_is_not_contained`、`a_process_that_is_gone_is_not_contained`）。
- 「没有覆盖的」里“本机没有编译过的 Unix 代码”中列着的 `what_the_program_started_goes_direct_too` 与 `ProcessTree::contains` 的 `#[cfg(unix)]` 一段照旧，只是那一条端到端用例不再是 Unix 专属。
- M4c 一节末尾「没有覆盖的」与本页末尾「没有覆盖的」里“`docs/acceptance/phase3-manual.md` 还没有 M4 一节…… M4d 写它”不成立了：M4 一节写好了（W19 ～ W41、M17 ～ M30、L18 ～ L32、P29 ～ P31，55 项），一项也没有做过。

#### 打开 Windows 的增强模式要什么

- 一个 **WireGuard LLC 签名的 `wintun.dll` 0.14 或更新**：0.13 没有 `WintunCloseAdapter`（那时是 `WintunFreeAdapter` / `WintunDeleteAdapter`），加载时因缺入口被拒，文字是 `wintun.dll has no WintunCloseAdapter: is it wintun.dll?`。往下取：<https://www.wintun.net/>。**要取与 `rurge.exe` 同一架构的那一份**：压缩包里每种架构一份（`bin/amd64`、`bin/x86`、`bin/arm64`、`bin/arm`），x86-64 的 Windows 用 `bin/amd64`；架构不对的那一份签名也对，但 `LoadLibraryExW` 不收（`cannot load wintun.dll: …`）。
- 它放在 `rurge.exe` 旁边，或用 `--wintun-dll <PATH>`（环境变量 `RURGE_WINTUN_DLL`）指出。**别的平台给出这个选项是用法错误**：`--wintun-dll is only for Windows`，退出码 2，在 `Overrides::parse` 与读配置之前（`crates/rurge/src/cli/run.rs` 的 `wintun_dll_applies`，`:220`；`--help` 里这个选项照抄 `RunArgs` 的说明“Windows: where wintun.dll is (default: next to rurge.exe)”，与别的路径选项一样连 `RURGE_WINTUN_DLL` 的值一起显示，M4d 记录第 74、75、76 行）。
- 它以**管理员**运行：不是管理员的错误是 `the enhanced mode needs to run as Administrator`（`crates/rurge-platform/src/tun/frame.rs` 的 `explained`，在 `rurge run` 的标准错误里是 `error: cannot turn the enhanced mode on: <这一句>`，在 API 里是 500 的 `{"error":"<这一句>"}`）。
- **`wintun.dll` 所在的目录应当只有管理员能写**：rurge 校验的是那个文件、加载的也是它，但一个能被普通用户换掉的目录里，换文件与换签名是一回事（M4d 计划 P7）。

#### `wintun.dll` 的查找与签名校验（各句错误）

查找在 `crates/rurge-platform/src/tun/dll.rs` 的 `wintun_dll`：`--wintun-dll` 给的相对路径按当前目录补成绝对路径（加载器只查给它的那一个绝对路径），没给时是可执行文件旁边的 `wintun.dll`。校验与加载在 `crates/rurge-platform/src/tun/windows/wintun.rs` 的 `loaded`：**先以只许读的共享方式开着那个文件**（`share_mode(FILE_SHARE_READ)`，别人不能写它、删它或给它改名），**校验的就是这个开着的文件**（`WINTRUST_FILE_INFO` 的 `hFile`），签名者对**恰好**是 `WireGuard LLC`，校验不联网（`WTD_REVOKE_NONE` 与 `WTD_CACHE_ONLY_URL_RETRIEVAL`，`WTD_UI_NONE`），校验完到 `LoadLibraryExW` 之间文件一直开着；加载只从那个绝对路径找，它的依赖只从 System32 找（`LOAD_LIBRARY_SEARCH_SYSTEM32`）。模块在进程的一生里只载一次、从不卸载。

每一句错误都是英文，**不带路径**（路径可能在 `rurge.exe` 旁边、带着账户名，错误会进 API 的 500；路径只在 DEBUG 那一行 `wintun.dll` 里），种类照系统给的那一个（没有这个文件是 `NotFound`，签名为空 / 签名者不对 / 不受信任是 `PermissionDenied`，缺入口是 `InvalidData`）：

| 情形 | 文本 |
| --- | --- |
| 没给 `--wintun-dll`，又不知道 `rurge.exe` 在哪里 | `cannot tell where rurge.exe is, to find wintun.dll next to it: give --wintun-dll` |
| `--wintun-dll` 是相对路径，又读不到当前目录 | `cannot tell where the current directory is, to make --wintun-dll absolute: give an absolute path` |
| 文件不在 | `cannot find wintun.dll (next to rurge.exe, or where --wintun-dll points): get it from https://www.wintun.net/` |
| 文件在，但打不开（`NotFound` 以外） | `cannot open wintun.dll: <系统的错误>` |
| 没有签名 | `wintun.dll is not signed` |
| 签名者不是 `WireGuard LLC` | `wintun.dll is not signed by WireGuard LLC (it is signed by <签名者的名字>)`（签名者的简单显示名可以带，它不是个人信息） |
| 签名不可信 | `wintun.dll's signature cannot be trusted (0x<状态码>)` 或 `… (no signer)` |
| 一个文件，签名对但加载不了（最常见的是架构不对的 `wintun.dll`） | `cannot load wintun.dll: <系统的错误>`（这台开发机上量到的是 `cannot load wintun.dll: …`，用例 `a_file_that_is_not_a_dll_cannot_be_loaded`；x86-64 的 Windows 上拿错架构的那一份通常是 `%1 is not a valid Win32 application` 一类） |
| 缺少一个入口（含 0.13 的 `WintunCloseAdapter`） | `wintun.dll has no <入口>: is it wintun.dll?` |
| 路径里有 NUL | `the path of wintun.dll has a NUL in it` |

这些错误不经 `frame::explained`（那里 `PermissionDenied` 一律被说成“要管理员”，签名不对会被说成缺管理员权限，M4d 记录第 59 行）。`--wintun-dll` 在 Windows 上给了而文件不在时，两条路不同：带 `--enhanced-mode` **启动**时 `enable` 失败那一支印 `error: cannot turn the enhanced mode on: cannot find wintun.dll (…)` 并**以 1 退出**（增强模式不开）；经 `POST /v1/features/enhanced_mode` 打开时是 500 `{"error":"cannot find wintun.dll …"}`，其余的照旧运行。

#### 网卡、序号与“正在被使用”

设备在 `crates/rurge-platform/src/tun/windows/mod.rs`：**先探后建，不复用**（M4d 计划 P8）——先 `WintunOpenAdapter("rurge")` 探名字，探到就答 `the virtual adapter is in use: is another rurge running with the enhanced mode on?`（种类 `ResourceBusy`）；探不到才 `WintunCreateAdapter` 建一个名字 `rurge`、类型 `rurge`、**固定 GUID** 的适配器，环 8 MiB。建好之后每 100 毫秒问一次接口序号，至多 5 秒（`INDEX_WITHIN`），仍没有是 `cannot open the virtual adapter: the virtual adapter has no interface index 5 seconds after it was made: <系统的话>`（`frame::explained` 的缺省一支加上了那个前缀）。适配器随建它的进程消失（Wintun 用 `SwDeviceCreate`，寿命跟着句柄，这是读源码与邮件列表得出的，**真机没有核对**，手工清单的一项），所以 `taskkill /F` 之后的下一次启动不会撞上“正在被使用”。网卡名可能不是 `rurge`：`rurge` 被一块它改不了名的网卡占着时 Wintun 会把新的叫 `rurge 1`、`rurge 2`……，而 rurge 认定名字是常数 `rurge`，于是网络任务的排除名单与出口表认不出它（罕见；本轮的延后事项第 10 行）。

设备的关闭在最后一个 `Arc` 放掉时（`Drop`），次序是：置停止标志 → 关读线程那个通道的接收端（卡在满通道上的 `blocking_send` 因此醒来）→ 等读线程 → `WintunEndSession` → `WintunCloseAdapter`（它把 `WintunCreateAdapter` 建的适配器从系统里移除）。正常的关闭经 `call`（`spawn_blocking`）交出管理器那一份，这几步不在 tokio 的工作线程上；TUN 3 秒内没结束的那一种，最后一份在 TUN 的任务里放掉，那时 `Drop` 会占住一个工作线程（M4d 记录第 29 行；这个次序与“两枚停止标志”的缺陷见 `docs/status.md` 的 M4d 一节）。

发包不等：`WintunAllocateSendPacket` 回空（环满）就是 `WouldBlock`，TUN 按“没有地方”丢掉并计数（`the system had no room for a packet; …` 那一行 WARN，与 Linux / macOS 同一条路）；包长超过 `wintun.h` 的 `WINTUN_MAX_IP_PACKET_SIZE`（`MAX_PACKET`，65535）的在放开的函数之前就被拒（`a packet for the virtual adapter is longer than 65535 bytes`），否则驱动会把整个发送环当成坏的。

#### `netsh` 的步骤与读回

系统一侧在 `crates/rurge-platform/src/enhanced/windows.rs`（不带 `cfg`，用例用假的执行器），翻译在 `crates/rurge-platform/src/route/windows.rs`。程序是 `<SystemRoot>\System32\netsh.exe` 与 `ipconfig.exe`（`SystemRoot` 没有时是 `C:\Windows`），经 `TimedRunner`，每个程序 30 秒为限。**每一步都当作“可能失败”执行**（`OnFailure::Ignore`），`netsh` 的退出码与输出只进 DEBUG；成没成只看之后的读回（M4d 计划 P10）。

| 改什么 | 一行一条的 `netsh` 写法 | 读回 |
| --- | --- | --- |
| 地址与 MTU | `interface ipv4 add address name=<序号> address=198.18.0.1 mask=255.254.0.0 store=active`、`interface ipv4 set interface interface=<序号> forwarding=disabled weakhostsend=disabled weakhostreceive=disabled store=active`、`interface ipv4 set interface interface=<序号> metric=1 store=active`、`interface ipv4 set subinterface interface=<序号> mtu=1500 store=active`（IPv6 同，地址是 `fd00:6152::1/64`） | `show addresses` 里有 `198.18.0.1`、`show subinterfaces` 的第一个数是 1500；缺了分别是 `the virtual adapter did not get its address` 与 `the virtual adapter's MTU is not 1500`（MTU 读回是必须的：Wintun 适配器的缺省 MTU 没有文档，比 1500 大会让系统往协议栈送更大的包） |
| 路由 | `interface ipv4 add route prefix=<前缀> interface=<序号> metric=1 store=active`（一行一条，不写下一跳；减的时候 `delete route`，减不掉不算错） | `show route` 里按**序号**筛出的行要有这一条前缀（`show route` 没有网卡的参数）；IPv4 缺了先撤掉这一批、再回 `<n> of the routes are missing after netsh added them`，IPv6 缺了回 `Ok(false)`，由 `run_routes` 写那一行 WARN 并撤掉新加的 |
| 系统 DNS | `interface ipv4 set dnsservers name=<序号> source=static address=198.18.0.2 register=none validate=no`（IPv6 是 `fd00:6152::2`；IPv6 没开时另跑一行 `address=none` 清掉 IPv6 的名单） | `show dnsservers` 里有应答器的地址；IPv4 没有是 `the system's DNS could not be pointed at the enhanced mode`，IPv6 没有只一行 WARN `the system's IPv6 DNS could not be pointed at the enhanced mode; it is left as it is`（M4d 记录第 62 行） |
| 清缓存 | `ipconfig /flushdns`（失败不算错） | —— |

**Windows 上不进 `240.0.0.0/4`**：Windows 所有版本缺省都挡这一段的流量（IETF 草案 draft-schoen-intarea-unicast-240 的 Windows 一段：“blocked by default in all versions of the Microsoft Windows operating system”；Windows SDK 的 `mstcpip.h` 里 `Ipv4AddressType` 把 `240.0.0.0/4`（除 `255.255.255.255`）判成 `NlatInvalid`），`netsh` 加不加得进这一段没有人核实过；加不进的话上面那张表的读回会让每一次打开都以「28 of the routes are missing」失败。所以 `RouteInputs.class_e` 为假（Windows）时，`RouteSet::compute` 在最后把 `240.0.0.0/4` 从 IPv4 的每一条里挖掉，包含项写了 `0.0.0.0/0` 或 `240.0.0.0/4` 也带不回来：缺省的 Profile 下 IPv4 是 27 条，Linux 与 macOS 是 55 条（多的 28 条就在这一段；M4d 记录第 87 行）。

**网卡的度量与转发设置**：接口度量 `1`（比系统给物理网卡的低，也不写含义没有文档的 `0`），单独一行——现行的 `netsh interface` 页里 `set interface` 没有 `metric`，一行里有一个系统不认的参数可能整行失败，拆开之后失败的代价只是度量（M4d 计划 P9）；虚拟网卡自己的**转发、weak host send 与 weak host receive 都关掉**，这是防环的 Windows 一半（另一部分是 `IP_UNICAST_IF` / `IPV6_UNICAST_IF` 绑网卡），不动别的网卡（设计第 7.4 节、M4-D11）。

读回只认前缀、序号、地址与数字，不认任何文字，所以系统的语言不影响结论；输出按 `from_utf8_lossy` 读，中文系统上汉字会坏、数字与地址不坏（夹具 `ROUTES_ZH`、`ADDRESSES_ZH`、`DNS_ZH`、`MTU_ZH` 是手写的，注明了不是从机器上取的，M4d 计划 P17）。每一步的 DEBUG 行是 `running a step of the enhanced mode`（带程序、参数与脚本全文）与 `reading back a setting of the enhanced mode`、`what the readback printed`；脚本写在数据目录的 `enhanced/` 下、`rurge-steps-<pid>-<n>.txt`、`create_new`、跑完删掉、崩溃留下的下一次 `open` 删，错误里脚本的路径换成 `…`。

**发送失败的 ICMP 载体退休，而不是立刻停掉**（M4d 记录第 88 行）：`PlatformPinger::forget` 按 `Arc` 指针把载体移出表（下一次回显新建一个），但它的接收任务再读一个回显时限（TUN 的 `echo_timeout`）才停——Linux 的 ICMP 数据报套接字只收得到自己那些请求的回复，同一个载体上已经发出、正在等回复的回显要靠它收；同时退休着的至多 4 个，第 5 个退休时最早的立刻停。原始套接字与 macOS 的数据报套接字收得到本机全部 ICMP，重复的那一份由等待表丢掉。

#### 测试后端与没有覆盖的

`RURGE_ENHANCED_BACKEND` 的三个取值不变（上面「只供测试的后端」一节与 `docs/testing.md`）；**不设时 Windows 上也是平台后端**——`WindowsSystem` 与 Wintun 的设备（`crates/rurge-platform/src/enhanced/mod.rs` 的 `system()` 第三支）。`docs/testing.md` 的 `RURGE_ENHANCED_BACKEND` 一段在任务 9 跟着改了（同一段就说三个平台都是平台后端的代码）。

- **Windows 的设备与系统改动没有在任何地方运行过**：`tun/windows/mod.rs`、`tun/windows/wintun.rs` 与 `enhanced/windows.rs` 在这台开发机上编译、`clippy -D warnings` 过，单元用例用的是假的执行器与临时目录；没有一条用例调 `TunDevice::open` 或设备的那十个放开的函数（`loaded`、`load`、`verify_signer` 有单元用例）（`RURGE_TEST_WINTUN_DLL` 不设），真机上也没有跑过（M4d 记录第 70 行）。
- **`a_real_wintun_dll_passes` 在本机没有真正跑过**（没有 `wintun.dll`，它打印原因就返回）：真的 `wintun.dll` 的签名者读成 `WireGuard LLC`、只读共享开着文件时 `LoadLibraryExW` 照样能载入，都由 `RURGE_TEST_WINTUN_DLL` 与手工清单核对（M4d 计划 P5、P7）。
- **`the_wintun_dll_option_is_a_usage_error_elsewhere` 在这台开发机上只编译、不运行**（Windows 上这个选项是寻常的那个，那一句用法错误只出现在别的平台），第一次运行是 Linux / macOS 的 CI（M4d 记录第 75、76 行）。
- **`TunDevice::dropped()` 今天没有读者**：设备写不进的丢包只记在设备自己的计数与那一行 WARN 里，不进 `GET /v1/features/enhanced_mode` 的 `dropped`（那是 `rurge-tun` 的原因表，设计第 3、13 节）。
- **网卡名固定当作 `rurge`**：Wintun 把新的网卡叫 `rurge 1`、`rurge 2`…… 时（`rurge` 被一块它改不了名的网卡占着），网络任务的排除名单与 `physical_subnets`、出口表都认不出它。罕见；修法是建好之后按固定 GUID 经已有的 `ipconfig` 包读回真的别名（不加放开的函数），去向是本轮延后事项第 10 行。
- 全部用例只在一台 Windows 11 x86-64 的开发机上跑过；M4 的提交 CI 一个也没有见过（最后一次被 CI 见过的运行是 `ac0a98a` 上的 40283632986）。

### 没有覆盖的

- **没有真实的设备**：这一版没有任何代码执行系统命令、建虚拟网卡、改路由或系统 DNS。三个平台的步骤只生成为数据（`rurge-platform::route::{linux,macos,windows}`，任何平台上都编译、都测），执行它们、建真的设备、拿权限都在 M4c / M4d。`RURGE_ENHANCED_BACKEND` 缺省时 `rurge run --enhanced-mode` 在每个平台上都以 1 退出，见本节开头。M4b 之后仍是。
- **防环没有在真机上验过**：出口判定与绑网卡是 M4b 做的，但“绑上真网卡”这一事只用记下不真绑的假钩子与数据用例测过；`rurge-platform::socket::egress::subnets` 读本机网卡（`if_addrs`）的那一步没有用例走到——它要真的网卡与出口表，随 M4c 接上平台后端。经虚拟网卡的一个往返、rurge 自己的 DNS 查询不进虚拟网卡这些事要在特权虚拟机里验（设计第 14 节 L2，今天还没有写）。
- **接管期间的去处只在单元用例里**：`server:system` 一类、单标签名字、`force-syslib`、`.local` 的四条路由 `crates/rurge-dns/src/resolver.rs` 的用例钉住；`physical` 今天没有人写（M4c），所以真机上接管期间拿到的是本机配置的那一份。
- **`PlatformPinger` 的两种情形没有用例**：出口是 `Some(网卡)` 的 ICMP 套接字（要真的网卡与出口表）与收包出错后的退避（`RECV_ERROR_BACKOFF`）都没有用例走到。
- **Windows 的 `ProcessTree::contains` 恒假**：外部程序拉起的子进程在 Windows 上要等 M4d 的 `in_job` 才认得出来。
- **本机没有编译过的 Unix 代码**：`rurge-platform::icmp::open` 的 Linux / Android 一份、macOS 一份与“其余平台恒失败”一份（那一份 CI 的三个平台也编不到，所以“第一次 CI 核对”对它不成立，M4b 延后第 17 行），`ProcessTree::contains` 的 Unix 一段与它的用例 `a_process_tree_contains_its_leader_and_what_it_started_and_nothing_else`，`crates/rurge-proto/src/external.rs` 的测试助手 `long_runner` 的 Unix 一支，与端到端用例 `what_the_program_started_goes_direct_too`。第一次 CI 的 Linux / macOS 核对。
- **`pings_the_loopback` 只在本机的那一条路上跑过**：这台 Windows 上原始套接字开得起来，`127.0.0.1` 与 `::1` 都真的来回了一次；Linux / Android 的数据报套接字与它退到原始套接字的那一路、macOS 的数据报套接字、“开不了套接字所以跳过”那条路没有任何平台跑过，macOS 的数据报套接字按标识符与序号对号（M4b 记录第 46 行）也没有在 macOS 上观察过。
- **Linux 与 macOS 的 `0600` 用例只在 CI 上编译运行**：池的文件 `fake-ip.txt` 在 Unix 上以 `0600` 建出（M4a 的任务 2），那条 `#[cfg(unix)]` 的用例在这台 Windows 开发机上没有编译过。
- **三个平台的步骤没有在机器上跑过**：`netsh` 与 `ip` 的写法是按文档与手册页核对出来的（M4a 计划 P2 ～ P5），没有一条在真机上执行过；`rurge check` 的四个码只对着 Profile 的解析测过。
- **手工验收没有做**：增强模式在真机上的开与关、路由表的样子、系统 DNS 的接管与交还、`kill -9` 之后的恢复、防环、睡眠唤醒与换网络、`ping`、接管暂停，都要等 M4c / M4d 之后；`docs/acceptance/phase3-manual.md` 里还没有 M4 一节（只有 M1、M2、M3），M4d 写它，要验的东西列在设计第 16 节，今天一项也没有做过。
- 全部用例只在一台 Windows 11 x86-64 的开发机上跑过，`rurge run --enhanced-mode` 的每一种行为都只经文件后端端到端测过；CI 没有见过 M4a 与 M4b 的提交。

### M4 推送之后的 CI（2026-10-28）

**本节订正上面几处“CI 没有见过”的说法**（规矩是历史的段落不逐处改、由后写的段落订正）：M4a ～ M4d 的提交推送了，运行 40611702392（提交 `3a76b80`）三个平台的 `check` 都绿（M4c 记录第 51 行、M4d 记录第 91 行）。

- M4c、M4d 两节与「没有覆盖的」里“CI 还没有见过 M4c 的提交”“M4 的提交 CI 一个也没有见过”“CI 没有见过 M4a 与 M4b 的提交”不再成立：Linux 与 macOS 开设备与挑平台的代码在这两个平台上过了 `cargo clippy --all-targets -- -D warnings`，缺省用例通过。
- 「没有覆盖的」里“本机没有编译过的 Unix 代码”列的东西，除 `icmp::open` 的“其余平台恒失败”一份（CI 的三个平台也编不到，M4b 延后第 17 行）之外，都在 CI 的 Linux 与 macOS 上编译了，那几条用例在那里通过；`0600` 那条 `#[cfg(unix)]` 的用例与 `the_wintun_dll_option_is_a_usage_error_elsewhere` 同样在 Linux 与 macOS 上通过。这些由测试一步的成功推出，逐条的结果在要登录才能读的任务日志里。
- 照旧成立的：缺省的用例不建网卡、不改系统设置，三个平台的设备与系统改动都没有真的运行过、没有在真机上验证；Wintun 设备的十个放开的函数没有在任何地方运行过（`RURGE_TEST_WINTUN_DLL` 不设）；L2 特权用例在 Linux 上编译了、没有运行过（`RURGE_TEST_PRIVILEGED` 不设）；`pings_the_loopback` 在 CI 的各平台上走的是哪一条路，没有读到。

## M5　识别、阻断与 `[IP Rewrite]`

阶段 3 / M5（设计 `docs/superpowers/specs/2026-10-28-phase3-m5-recognition-blocking-ip-rewrite-design.md`，实施计划 `docs/superpowers/plans/2026-10-28-phase3-m5-recognition-blocking-ip-rewrite-plan.md`，下面写“M5 记录第 N 行”指计划末尾的「执行期修正记录」）做的是：所有端口上的 QUIC 与 STUN 标签、`block-quic` 的 `adaptive` 与 6.11 的 `auto`、`[IP Rewrite]` 生效、写回虚拟网卡的两条队列（`udp-priority`）、内部 DNS 会话带主机名、手册 6.11 的 WireGuard 写法能加载。这一节只说 API、日志与请求记录上看得到的那一部分；配置与语义在兼容性清单的 1.2、2.1、3.1、4.3、4.6、6.1、6.2、7.2 与 9.10 节。

**这一节的提交都在本地，CI 没有见过**（最近一次被 CI 见过的运行是 `3a76b80` 上的 40611702392）；`[IP Rewrite]` 只在内存设备上测过，`udp-priority` 的效果没有观察过，手工验收一项也没有做（`docs/acceptance/phase3-manual.md` 的 M5 一节是这一轮写的）。

### `GET` / `POST /v1/features/enhanced_mode`：多两个数

M5 之后 `GET` 的形状（`dropped` 多第十一个键 `ipRewrite`，顶层多 `reflected`，它在 `stackPanics` 之后）：

```json
{"enabled":true,"dropped":{"malformed":0,"fragment":0,"martian":0,"unsupported":0,"queueFull":0,"unknownFake":0,"busy":0,"beyondWindow":0,"rstLimited":0,"icmpLimited":0,"ipRewrite":0},"stackPanics":0,"reflected":0}
```

本节的例子**替代上面「M4」一节那个**（`GET /v1/features/enhanced_mode` 的旧例子）；旧例子没有改，是这一段订正它。`POST` 的体、成功与失败的应答、接管暂停期间的行为都不变。

- `dropped.ipRewrite`：`[IP Rewrite]` 的 `reject` 与 `drop` 命中的包各计一次（`rurge-tun` 的 `Reason::IpRewrite`）。它是“profile 要的”，所以**从不进 WARN**（`Reason::is_wanted`），每次一行 DEBUG；`reject` 又被 RST 或 ICMP 的限频挡下的那一个包仍计一次 `ipRewrite`，只是没有回包，限频各自计 `rstLimited` / `icmpLimited`。
- `reflected`：`[IP Rewrite]` 的 `reflect` 送回、并且被写任务的队列收下的包。队列满的那一个计 `queueFull`（两个地址是那个要发出去的包的），不计 `reflected`。它不写日志。两个数在增强模式关着时都是 0；`Counters` 的字段名是 `ip_rewrite` 与 `reflected`，`GET /v1/traffic` 不涉及它们。
- 计数表的其余十个原因、`stackPanics` 与本页「M3c」一节「计数」一表相同；M5 只加 `queue_full` 的一个来源（`reflect` 的包遇上写任务队列满）与两个限频的一次共用（见下）。

### `protocol` 的 `quic` 与 `stun` 不再限端口

请求记录里的 `protocol` 由引擎的嗅探定（`crates/rurge-engine/src/udp.rs`）：一个 UDP 流的第一个数据报，先试 QUIC（`rurge_net::quic::initial::is_client_initial`：长首部（QUIC 位可以是 0，RFC 9287 §3.1）、v1 / v2 / 草案 29 ～ 32、该版本的 Initial 类型、至少 1200 字节，再用自己的 Destination Connection ID 的 Initial 密钥解密确认），再试 STUN（`crates/rurge-engine/src/sniff.rs::is_stun`：RFC 8489 的魔数、首两位、长度恰好等于数据报），都不是就是 `UDP`。**端口是多少都一样**（阶段 3 / M5 之前，阶段 2 / M5a 起只认 UDP 443）；`PROTOCOL,QUIC` / `PROTOCOL,STUN` / `block-quic` 都跟着它走。识别不写日志。仿一个 Initial 的普通 UDP 照样标 `UDP`（用例 `a_lookalike_of_a_quic_initial_is_plain_udp`）。

### 内部 DNS 会话的目标是名字

开了 `encrypted-dns-follow-outbound-mode`、上游写成主机名时：

- 流水线看到的会话 `dst` 是**名字与端口**（`dns-up.test:53` 一类），`DOMAIN` 一族与规则集里的域名规则因此能匹配 DNS 服务器的主机名；`IP-CIDR` 一类用跟着会话来的那个地址匹配（`FixedResolve`），不问引擎的解析器；协议标签照旧按 URL 的方案（`DOH` / `DOT` / `DNS` / `DOQ` / `DOH3`）。
- 请求记录的 `dst` 是名字与端口；`remoteAddr` 照旧（TCP 是拨到的地址，UDP 是 `null`），**没有查找**，所以 `dnsMs` 是 `null`；`listenPort` 仍是 `null`（rurge 自己发起的会话）。
- **经代理时，代理收到的是引导解析出的地址，不是名字**（M5-D16）：流水线把地址交给终端出站，规则与请求记录只在上游这一侧看到名字；计划「延后事项」第 7 行。`smart` 组给这类会话记的站点也从服务器的地址变成名字。
- 引擎还没接上时（`dns_pipeline` 的那一支）同样拨引导给的地址，名字只经 `Connector::connect_resolved` / `connect_udp_resolved` 交给回落连接器，绝不进系统解析库（M5 记录第 50 行，用例 `without_an_engine_a_named_server_is_dialled_at_its_address`）。

### 新增的日志行

日志文本是英文；下面都是 DEBUG，要 `--log-level debug`。名字与地址只在 DEBUG。`[IP Rewrite]` 的行里 `reason` 是 `ip_rewrite`（`tun: packet dropped`，字段 `src` / `dst` 是包的两端），第一次也不是 WARN；`reflected` 什么都不写；QUIC / STUN 的识别不写日志。

| 级别 | 文本 | 字段 | 什么时候 |
| --- | --- | --- | --- |
| DEBUG | `quic: <名字> falls back to TCP; QUIC stays blocked` | | `block-quic=adaptive`（或 6.11 的 `auto` 落到自适应）在 1 秒的窗口里等到同一个主机名的客户端 TCP 会话：这一次 QUIC 挡住、这个名字以后也一直挡 |
| DEBUG | `quic: <名字>: no TCP within 1s; QUIC allowed` | | 窗口过了没有 TCP（不论是下一条 QUIC 流还是来晚了的 TCP 让它落定）：这个名字从此放行，下一次 QUIC 直接过 |
| DEBUG | `tun: packet dropped` | `reason`=`ip_rewrite`、`src`、`dst` | `[IP Rewrite]` 的 `reject` / `drop` 命中的每个包一行；没有 WARN 行 |
| DEBUG | `tun: packet dropped` | `reason`=`queue_full`、`src`、`dst` | `reflect` 要送回的包遇上写任务队列满（两个地址是那个要发出去的包的，M5 记录第 27 行） |

写任务的两条队列本身不写日志：`udp-priority` 开着时 UDP 的回包与应答器的 UDP 应答进优先队列，其余进普通队列，每一轮先写优先的那一批；两条队列各自的容量都是 `TunOptions.writer_queue`。

### 与 `[IP Rewrite]` 共用的两个限频

`reject` 回的 RST 走虚拟网卡自己的“3 秒 100 个”限频（超出计 `rstLimited`），回的 ICMP“管理上禁止”走虚拟网卡自己的 ICMP 限频（另一份，同样每 3 秒 100 个，超出计 `icmpLimited`）：**RST 与 ICMP 是两份限频**，`reject` 的 RST 与虚拟网卡自己回的 RST（停止中、忙、发往 rurge 自己的地址、不认识的 fake IP、预匹配拒绝，都经 `tcp/shard.rs` 的 `turn_away`）共用一份，`reject` 的 ICMP 与被拒 UDP 流的 ICMP 共用另一份（M5 记录第 28 行）。这两种被限频挡下的包仍计一次 `ipRewrite`（`reject` 命中了），只是没有回包；`src` / `dst` 照被拒的那个包的两端记。

### 没有覆盖的

- **没有真机**：这一节的行为只在内存设备（引擎用例的 `Rig`）与单元用例里验证过；`ping`、真实的应用、真实的 Windows 防火墙、`reflect` 送回的包在真实系统上被怎样回应（操作系统的应答又被送回、靠 TTL 与“不对 RST 回 RST、不对差错回差错”终止）都没有观察过——手工验收的 W42 / M31 / L33 / P32 就是看这个。
- **`udp-priority` 的效果看不出来**（延迟的差别在真实应用上量不出来），不设手工验收项，兼容性清单 2.1 节照实写着“尽力而为”。
- **QUIC 少认的版本**（gQUIC、草案 28 以前）与 Surge 那边是否同样不认，没有在 Surge 上核对过；QUIC 里的 SNI、跨几个包的 ClientHello、`auto-quic-block` 都在阶段 4。
- 全部用例只在一台 Windows 11 x86-64 的开发机上跑过；M5 的提交 CI 一个也没有见过。
