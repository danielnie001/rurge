# rurge HTTP API（阶段 3）

阶段 3 / M1（网络环境与子网，设计 `docs/superpowers/specs/2026-10-16-phase3-m1-network-subnet-design.md`，计划 `docs/superpowers/plans/2026-10-16-phase3-m1-network-subnet-plan.md`）**没有新增端点，也没有改任何响应的形状**。变的是三个已有端点的行为——`POST` / `GET /v1/policy_groups/select` 对 `subnet` 组、`GET` / `POST /v1/features/system_proxy` 在接管暂停时、`GET /v1/dns` 的 `upstreams` 随网络的 DNS 覆盖——，和一组日志行。没有任何端点给出本机当前所在的网络：SSID、BSSID 和网关说明使用者在哪里，只在 DEBUG 日志里出现；读网络的日志行（网络任务与平台层）不带本机网卡的地址，也不带系统 DNS 服务器的取值（设计 M1-D16）。这不等于这些地址在任何级别的日志里都没有——DNS 服务器的地址出现在解析器自己的行里，会话的日志行带来源地址——，见「网络任务的日志行」末尾（M1 收尾时写的"本机地址与 DNS 服务器的取值在任何级别的日志里都不出现"说得过了头，阶段 3 / M2 订正）。鉴权（`X-Key`）、错误体（`{"error":"<message>"}`）与无内容的成功响应（`{}`）沿用阶段 1 的约定，见 `docs/api/phase1.md`；本页只记录变了的部分。

阶段 3 / M2（进程识别，设计 `docs/superpowers/specs/2026-10-17-phase3-m2-process-design.md`，计划 `docs/superpowers/plans/2026-10-17-phase3-m2-process-plan.md`）**没有新增端点**，两个已有端点 `GET /v1/requests/recent` 与 `GET /v1/requests/active` 的每条记录多了两个键 `pid` 与 `processPath`；规则等不到进程时，会话的 `notes` 里多一句说明。下面到「没有覆盖的」为止说的是 M1，M2 在「M2　进程识别」一节。

阶段 3 / M3b（UDP、ICMP、fake-IP 池与引擎接入，设计 `docs/superpowers/specs/2026-10-18-phase3-m3-tun-core-design.md`，计划 `docs/superpowers/plans/2026-10-22-phase3-m3b-udp-icmp-pool-engine-plan.md`）**没有新增端点**：`GET /v1/traffic` 的 `listener` 多了一个键 `tun`，进程查找的 DEBUG 行的字段 `listen_port` 换成了 `remote`，数据目录里多了池的文件 `fake-ip.txt` 与它的三行日志。引擎一侧的 `Engine::start_tun` 只有测试调用，`rurge run` 要到 M4 才起虚拟网卡，所以前两处在今天的 `rurge run` 里是这样的：`tun` 恒为 0，查找行只有监听器的会话；池的文件与它的日志使用者要到 M4 才看得到。见最后一节「M3b　虚拟网卡的 UDP、ICMP 与引擎」。

阶段 3 / M3c（DNS 应答器、预匹配与经监听器来的 fake 地址，设计 `docs/superpowers/specs/2026-10-18-phase3-m3-tun-core-design.md` 第 11 ～ 13 节，计划 `docs/superpowers/plans/2026-10-23-phase3-m3c-dns-responder-prematch-plan.md`）**没有新增端点**，也没有改任何响应的形状：经虚拟网卡来的会话在请求记录里多了预匹配的两种 `notes` 和没有端口的 `dst`，经监听器来的 fake 地址有了一条失败原因 `fake address not known`，应答器与预匹配有了几行日志，计数的 `queue_full` 多了几处来源。**`rurge run` 要到 M4 才起虚拟网卡，使用者今天看不到这些**（连经监听器来的那一半也要有 TUN 的池）；见最后一节「M3c　虚拟网卡的 DNS 应答器、预匹配与经监听器来的 fake 地址」。

阶段 3 / M4a（增强模式的生命周期与平台接入的骨架，设计 `docs/superpowers/specs/2026-10-26-phase3-m4-platform-design.md`，计划 `docs/superpowers/plans/2026-10-26-phase3-m4a-skeleton-plan.md`）**没有新增端点**：`GET` / `POST /v1/features/enhanced_mode` 从恒假、不可切换变成活的（`GET` 的形状阶段 1 就写着，现在有真值），`rurge run` 多了 `--enhanced-mode` / `RURGE_ENHANCED_MODE`，`rurge check` 多了 `W0035` ～ `W0038`。**这一版的平台后端对每个动作都答 "not available"**，所以使用者仍然开不了增强模式；下面说的其余行为今天只有经只供测试的文件后端才看得到。见最后一节「M4　增强模式（平台接入）」。

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

阶段 3 / M4（平台接入，设计 `docs/superpowers/specs/2026-10-26-phase3-m4-platform-design.md`）分四份计划：M4a「平台无关的骨架」、M4b「防环与接管期间的 DNS」、M4c「Linux 与 macOS」、M4d「Windows 与收尾」。**这一节随它们写成，这里是 M4a 做到的**；M4b ～ M4d 写进来时各补各的（出口表的读者、接管期间的 `system` 一类、三个平台的设备与权限、真实的 `Pinger`）。

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

`dropped` 的十个键是 camelCase（同 API 别处），`stackPanics` 在顶层；它们的含义是 `rurge-tun` 的 `Counters`（丢弃按原因各一个数，加 TCP 栈的 panic 次数），本页「M3c」一节「计数」一表说的是各原因还数什么。这个形状从阶段 1 起就写着（那时恒为 `{"enabled":false}`），M4a 起才有真值。

`POST` 的体是 `{"enabled": <bool>}`（与其余功能端点同一个读法：不是布尔值、不是 JSON 对象都是 400）。

- 成功回 `{}`。打开成功时日志先有一行 INFO `enhanced mode on`（管理器写的），然后一行 INFO `enhanced mode switched via http-api`（端点写的，字段 `enabled`）；关闭成功时是 INFO `enhanced mode off` 再这一行。
- 失败回 500，体是 `{"error":"<原因>"}`，原因就是后端（或者它上面的 `state.json`）给的那句话；主循环另记一行 ERROR `cannot switch the enhanced mode`（字段 `error`、`enabled`）。打开失败已经自己撤销了做过的事，`GET` 照实说 `enabled: false`。
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
| ERROR | `cannot undo part of the enhanced mode; going on with the rest` | `step`、`error` | 关闭（含打开失败的回退）里一条撤销失败。`step` 是那条撤销动作的名字：`take_dns`、`adjust`、`rp_filter <值>`、`mac_dns <服务>` |
| ERROR | `state.json's enhanced backup cannot be read; it is left as it is and nothing is undone` | `error` | 启动的恢复读到读不懂的备份：什么也不撤销，备份原样留着 |
| ERROR | `state.json's enhanced backup cannot be read; what outlives the device is left as it is` | `error` | 关闭时读到读不懂的备份：其余各步照做，备份原样留着 |
| ERROR | `cannot follow the change; the enhanced mode goes on with what it has` | `step`、`error` | 重载或网络变化时某一步失败；`step` 是 `configure_ipv6`、`routes`、`dns_undo`、`dns_backup`、`take_dns`、`adjust_undo`、`adjust_backup`、`adjust`、`unconfigure_ipv6` 之一 |
| DEBUG | `the virtual adapter failed` | `error`（设备的原话） | 设备自己坏了。原因只在这里，下面那一行 ERROR 由主循环说 |
| ERROR | `the virtual adapter failed; the enhanced mode is off` | | 主循环收到"正开着的这一次的 TUN 坏了"（P24），关掉增强模式之前 |
| ERROR | `cannot turn the enhanced mode off after the adapter failed` | `error` | 上面那次关闭失败（不重试） |
| INFO | `restored what the enhanced mode left last time` | | 启动的恢复把备份里的撤销动作都做成了 |
| ERROR | `cannot undo what the enhanced mode left; it stays in state.json and is tried on the next start` | `error` | 启动的恢复里有撤销失败；失败的那几条留在备份里 |
| ERROR | `cannot switch the enhanced mode` | `error`、`enabled` | `POST` 的开关失败（API 这时回 500） |

接管暂停的那几行（`takeover paused on this network`、`takeover resumed`、`the takeover is paused on this network; the enhanced mode is left as it is, and set as asked when the pause ends`，以及三条 `cannot turn the enhanced mode …`）在下面「接管暂停：增强模式的那一半」，与系统代理那几行在同一张表里（本页「网络任务的日志行」）。

上面「启动参数与环境变量」与下面「`state.json`」里那些标准输出与标准错误不是日志行：它们照旧不受 `--log-level` 与 `--log-file` 影响，上表只讲 tracing 的行。

### `state.json` 的 `enhanced_backup` 与恢复

管理器自己在 `state.json` 里记一份备份（引擎不解释它，与 `system_proxy_backup` 一样）：只有"会在 rurge 死后还留在系统里"的改动才进备份——macOS 各网络服务的 DNS 原值、Linux 的 `net.ipv4.conf.all.rp_filter`；设备、地址、路由、按链路的 resolved 设置不进备份，它们随设备消失。

```json
{"enhanced_backup":{"undo":[{"kind":"mac_dns","service":"Wi-Fi","servers":["192.168.1.1"]},{"kind":"rp_filter","value":1}]}}
```

- 形状是 `{"undo":[…]}`，次序即做成的次序；每一项是 `{"kind": …}` 加自己的字段：`mac_dns`（`service`、`servers`，原来的服务器，没有手设过就是空表）、`rp_filter`（`value`，原来的值）、`test`（`step`，只供测试的文件后端用的那一种）。读不懂的一项让整份备份读不懂：`serde` 拒绝一个不认识的 `kind`。
- **什么时候写**：打开的第 6 步（接管系统 DNS）与第 8 步（系统自己的调整）之前各写一次，写的是"打开前备份里已有的 + 这一步新要的"，用 `try_update`（写进磁盘才换内存里的状态）；写不进时那一步不做，已经做过的倒着撤销，打开失败，错误是 `cannot write state.json: <系统的原因>`。重载与网络变化再做这两步时同样先写。
- **什么时候清**：关闭（含打开失败的回退）与启动的恢复里，做成一条就从备份里去掉一条，最后把剩下的写回；一条也不剩时 `enhanced_backup` 是 `null`，同时 `features.enhanced_mode` 写 `false`。这一步用 `update`：写不进只记一行 ERROR，下一次启动会把同样的几条再撤销一遍（撤销已经撤销过的东西是无害的）。
- `features.enhanced_mode`：打开的最后一步写 `true`，备份清空时写 `false`。启动时不读它——开关只看参数与 `POST`（M4-D15）——所以它只是给人和以后的会话看的记录。
- **启动时的恢复**：拿到数据目录的实例锁之后、任何新改动之前，与系统代理的恢复同一个位置（配置读不了、有错误、日志文件打不开的那三个出口也做，见 `docs/api/phase1.md` 的「配置加载不了时也恢复」一条）。备份里有撤销动作就**倒着**做（最后做的先撤），成功的从备份里去掉。有一句成功时是 INFO `restored what the enhanced mode left last time`，有失败时是 ERROR `cannot undo what the enhanced mode left; it stays in state.json and is tried on the next start`。
- 那三个没有日志的出口（配置读不了、配置有错误、`--log-file` 打不开）把结果写到标准错误，四句之一：`warning: a previous run left changes of the enhanced mode; they were undone`、`… ; they were undone, but state.json could not be updated: the next start undoes them again`、`warning: cannot undo what the enhanced mode left: <原因>; it stays in state.json and is tried on the next start`，以及取不到后端时的 `warning: <后端的报错>; the enhanced mode's changes were not undone`（`RURGE_ENHANCED_BACKEND` 的取值不对时）。都不改变退出码。
- **读不懂的备份照原样留着**：启动的恢复、关闭、打开三处都只记一行（前两处各一句 ERROR、打开是回一个以 `state.json's enhanced backup cannot be read` 开头的错误并且一个后端动作也不做），谁都不改写它。理由与系统代理一样：那可能是以后版本写下的形状，猜着撤销比不动更坏。

### 接管暂停：增强模式的那一半

生效的 `[SSID Setting]` 行写了 `suspend=true` 的网络上，接管暂停（本页「接管暂停时的系统代理端点」一节说的是系统代理那一半），增强模式照同一种语义（设计第 9.9 节、M4-D18）：

- **进入暂停**：开着的增强模式按关闭的次序关掉（系统 DNS 先交还），并记下"想要开着"——即便这一关失败了，也照样记下（它仍开着，离开暂停时不会再开一次）。失败是 ERROR `cannot turn the enhanced mode off for the pause; it stays on`。
- **离开暂停**：照记下的"想要"办，两个方向都对：想要开着而关着的就打开（用的是这一刻这一代的配置与这个网络的 IPv6，失败记 ERROR `cannot turn the enhanced mode on when the pause ends; it stays off`，不重试）；想要关着而开着的（进入暂停时没关掉）就关掉（失败记 ERROR `cannot turn the enhanced mode off when the pause ends; it stays on`）。
- **先增强模式、后系统代理**，两个方向都是（与退出的次序一致，M4-D16）：增强模式留下的是整台机器的 DNS，越早还越好。
- **启动时就在这样的网络上**：两样都不开，`--enhanced-mode` 与 `--system-proxy` 只记下想要的；`GET` 照实说 `false`，标准输出里没有 `enhanced mode on` 那一行。离开这个网络时才开。
- 暂停期间的 `POST /v1/features/enhanced_mode` 回成功 `{}`、只记下想要的，日志里一行 INFO `the takeover is paused on this network; the enhanced mode is left as it is, and set as asked when the pause ends`（字段 `enabled`）。
- 暂停只在内存里，不进 `state.json`。进入暂停时增强模式已经关掉、备份已经清空（关失败时它仍开着、备份也还在，那一次崩溃之后的下一次启动照常撤销），所以暂停期间 rurge 崩溃，通常没有它的东西要恢复。
- "想要的状态"由三处决定，最后一次生效：`--enhanced-mode`、进入暂停时它是不是开着、暂停期间每一次得到成功应答的 `POST`。
- **设备自己坏了之后，"想要"不变**（P24）：管理器把增强模式整个关掉（交还 DNS、撤销会留下的），记 ERROR 一行，不重试；`Takeover` 记的"想要"仍是原来那个，所以离开 / 再进暂停时不会因为一次设备故障改变使用者的意图。`GET` 从此说 `enabled: false`。

### 重载与网络变化：只做差

一次成功的重载之后、一次网络变化之后（设计第 9.7、9.8 节），开着的增强模式按新的这一代与新的网络重算：要哪些路由（`tun-excluded-routes`、`tun-included-routes`、`external` 策略的 `addresses`、rurge 自己的地址空间）、`ipv6-vif` 说 IPv6 开不开、出口表的主网卡。**只做差**：`RouteSet::diff` 算出要加与要减的，成批加减，不加也不减时连后端都不叫。

- **IPv6 从关到开**：先配网卡的 IPv6 地址（`configure_ipv6`，答"没有"时一行 WARN、这一次按没有 IPv6 办），再按差加路由，再接管系统 DNS（IPv6 变了或网络变了，一次 `follow` 至多接管一次），最后在网络变化时再做一遍系统自己的调整（Linux 的 `rp_filter`）。
- **IPv6 从开到关**：**先交还系统 DNS**（它指着的应答器地址还在网卡上时交还最稳），再按差减路由，最后撤掉网卡的 IPv6 地址。这一条是协调者的裁定（M4a 计划「执行期修正记录」第 79 行），它订正了任务正文写的次序；理由：系统 DNS 指着一个网卡已经没有的地址时，每一次查询都要等到它超时才落到别处。
- **`ipv6-vif = auto` 的判据**（P18）：主网卡有 IPv6 网关（网络快照的 `routers` 里有 IPv6 地址，链路本地的也算——IPv6 的默认路由本来通常写的就是链路本地地址），且本机地址列表里有落在 `2000::/3` 的 IPv6 地址（平台给出的是主机位清零的 /64 网络地址，所以看的是前缀）。两个都成立才算这个网络有 IPv6，判一次、随网络变化再判。
- **网络变化时 DNS 与系统自己的调整再做一次**：新网络上可能多了（macOS）或换了一批网络服务、`rp_filter` 可能被网络管理程序改回去了。做法与打开时一样：先 `dns_undo` 读一次要撤销什么、写进备份（`try_update`），再 `take_dns`；先 `adjust_undo`，再 `adjust`。哪一步失败都只记一行 ERROR、后面的照做；备份写不进去时紧接着的那一步不做（崩溃之后系统是什么样就不知道了），与打开的规矩一致。
- **失败的步骤记 ERROR，增强模式照开**：已做的保留，不回滚到旧集合（旧集合本身也可能已经不对），`GET` 照实说 `enabled: true`。路由那一步失败时出口表留着"旧的与新的合起来"那一份（M4a 计划「执行期修正记录」第 75 行）：出口表始终是装进网卡的路由的**超集**，否则 rurge 自己的一条连接会回到虚拟网卡里（M4b 的读者按它判定，见设计第 7.1 节）。
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

### 没有覆盖的

- **没有真实的设备**：这一版没有任何代码执行系统命令、建虚拟网卡、改路由或系统 DNS。三个平台的步骤只生成为数据（`rurge-platform::route::{linux,macos,windows}`，任何平台上都编译、都测），执行它们、建真的设备、拿权限都在 M4c / M4d。`RURGE_ENHANCED_BACKEND` 缺省时 `rurge run --enhanced-mode` 在每个平台上都以 1 退出，见本节开头。
- **出口表没有读的人**：M4a 写出口表（`EgressSlot`），判定的那一半（`PlatformSockets` 的 `egress`）是 M4b。所以这一版没有防环：rurge 自己的出站连接若真进了虚拟网卡，没有谁拦它。文件后端不起真设备，碰不到；M4c / M4d 之前也不会碰到，因为那时平台后端仍答"不可用"。
- **ICMP 回显不转发**：这一版 TUN 的 `Pinger` 是 `NoPinger`，每个回显请求都立刻失败（`ICMP forwarding is not available in this version`），`ping` 经虚拟网卡不通。M4b 换成真的套接字。
- **Linux 与 macOS 的 `0600` 用例只在 CI 上编译运行**：池的文件 `fake-ip.txt` 在 Unix 上以 `0600` 建出（M4a 的任务 2），那条 `#[cfg(unix)]` 的用例在这台 Windows 开发机上没有编译过。
- **三个平台的步骤没有在机器上跑过**：`netsh` 与 `ip` 的写法是按文档与手册页核对出来的（M4a 计划 P2 ～ P5），没有一条在真机上执行过；`rurge check` 的四个码只对着 Profile 的解析测过。
- **手工验收没有做**：增强模式在真机上的开与关、路由表的样子、系统 DNS 的接管与交还、`kill -9` 之后的恢复、防环、睡眠唤醒与换网络、`ping`、接管暂停，都要等 M4c / M4d 之后；`docs/acceptance/phase3-manual.md` 的 M4 一节在设计第 16 节里列了要验的东西，今天一项也没有做过。
- 全部用例只在一台 Windows 11 x86-64 的开发机上跑过，`rurge run --enhanced-mode` 的每一种行为都只经文件后端端到端测过；CI 没有见过 M4a 的提交。
