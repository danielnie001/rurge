# 阶段 3 / M5「识别、阻断与 `[IP Rewrite]`」细化设计

| 项       | 内容 |
| -------- | ---- |
| 日期     | 2026-10-28 |
| 状态     | 草案。已决事项 M5-D1 ～ D24 是在项目所有者"自己决定、继续做"的常设授权下定的（代项目所有者作的裁定），所有者可以改。执行交给 Pi：一份计划，由协调的会话逐个任务调 Pi 做、逐个任务评审（同 M4d，第 2 节） |
| 上位设计 | [阶段 3 总设计](2026-10-16-phase3-enhanced-mode-design.md) 第 1.4、2.3（Q-22、Q-26）、8.1、8.4、8.7、10、12 ～ 14 节；[M3 细化设计](2026-10-18-phase3-m3-tun-core-design.md) 第 4.3 ～ 4.5、14 节。不一致处以本文为准，订正列在第 17 节 |
| 需求     | FR-RULE-08（QUIC、STUN 的识别）、FR-OUT-11（虚拟网卡上的 `block-quic`）、FR-IN-08（`udp-priority`）；`[IP Rewrite]` 归在 FR-IN-06 之下（总设计 P3-D24；要不要在需求文档第 4 节另立一行是总设计 Q8，归项目所有者） |
| 清单     | 2.1 节 `udp-priority`（L176）、`block-quic`（L177）；3.1 节 `PROTOCOL`（L249）；4.3 节策略参数 `block-quic`（L375）；6.1 节 `.local`（L502）；6.2 节 `encrypted-dns-follow-outbound-mode`（L520）；7.2 节 `auto-quic-block`（L584）；9.10 节与 1.x 节 L61 的 `[IP Rewrite]`；1.x 节 L66 与 4.2 节 L356 的 WireGuard；10.4 节 `GET /v1/policies`。行号是 2026-10-28（`cdd011d`）的 |
| 前置     | 阶段 3 / M1 ～ M4 已合并、已推送（`origin/main` = `3a76b80`）；CI 运行 40611702392（2026-10-28）三个平台的 `check` 与 `bench` 全绿。本地 `main` 领先两个文档提交（`b3b785a`、`cdd011d`） |
| 调研     | 手册 2026-10-28 复查：[`docs/reports/2026-10-28-manual-6.11-recheck.md`](../../reports/2026-10-28-manual-6.11-recheck.md)（87 页里 83 页不变、4 页改过，手册现在至少描述到 Mac 6.11.0 / iOS 5.24.0）。QUIC 与 STUN 的识别读了 sing-box（`testing` 分支 `common/sniff/quic.go`、`stun.go`）、Xray-core（`common/protocol/quic/sniff.go`）、mihomo（`component/sniffer/quic_sniffer.go`）的做法（只取事实，代码不抄）；RFC 9000、9001、9369、5389 / 8489；本机 cargo 注册表里 rustls 0.23.43 的 `quic` 模块 |

M5 做阶段 3 剩下的识别与阻断：UDP 流在所有端口上识别 QUIC（解密 Initial 来确认）与 STUN，`block-quic` 照手册 6.11 多一个 `adaptive`、`auto` 改成 6.11 的含义；`udp-priority` 让写回虚拟网卡的 UDP 包不排在 TCP 后面；`[IP Rewrite]` 第一次生效（`reflect` / `reject` / `drop`）；开着 `encrypted-dns-follow-outbound-mode` 时，DNS 服务器自己的连接带上它的主机名，按主机名写的规则第一次能匹配它。另有两件从手册复查带出的：手册 6.11 的四页登记进清单；手册自己的 WireGuard 服务端例子在 rurge 里今天加载不了（`E0023`），改成能加载、说清不生效。

**M5 没有平台代码**：改的都是 `rurge-net`、`rurge-config`、`rurge-engine`、`rurge-tun` 与 `rurge-api` 里不带 `cfg` 的部分，开发机上全部编译、全部跑；没有等 CI 的停点。

## 1. 范围

| 做 | 不做（去向） |
| -- | ------------ |
| QUIC 识别：所有端口，看流的第一个数据报；包头筛过之后用 Initial 密钥解密来确认（v1、v2、草案 29 ～ 32）（第 4 节） | QUIC 里的 SNI、跨几个包的 ClientHello、按 SNI 匹配规则（阶段 4）；gQUIC 与草案 28 以前（不再算 QUIC，登记，第 4.5 节） |
| STUN 识别：所有端口，看流的第一个数据报（第 5 节） | 经典 STUN（RFC 3489，没有魔数）、TURN 的 ChannelData |
| `block-quic`：识别扩到所有端口之后，经虚拟网卡与经 SOCKS5 的流照旧阻断；策略参数多一个 `adaptive`，`auto` 照手册 6.11 的含义（第 6 节） | `[MITM]` 的 `auto-quic-block`（阶段 4，第 11.1 节） |
| `udp-priority`：写任务两条队列，UDP 一类先写（第 7 节） | 收包一侧与引擎里的调度（不重排：tokio 没有任务优先级） |
| `[IP Rewrite]`：解析、每一代一张表、包进虚拟网卡时按目的地址做 `reflect` / `reject` / `drop`、计数与 API（第 8 节） | 网关模式里局域网设备的包（阶段 7）；经代理端口来的请求（手册：不受影响） |
| 内部 DNS 会话带上 DNS 服务器的主机名（清单 L520，第 9 节） | 经代理时把名字交给代理（照旧交引导解析出的地址，登记，第 9.5 节） |
| 手册 6.11 的四页登记进清单；`[WireGuard <名字>]` 的 `listen-port`、`accept-incoming` 与不带 `endpoint` 的 peer 能加载（第 10 节） | WireGuard 与 Tailscale 接受连入（阶段 7 / 远期）；`GET /v1/policies?include_external=true`（阶段 6） |
| 基准 B1、B2、b3 重跑，对 NFR-01 的 70% 记数（第 12 节） | 新的基准 |
| — | `.local` 答案里带作用域号的 IPv6（清单 L502；不做，第 11.2 节） |

## 2. 计划与执行

一份计划：`docs/superpowers/plans/2026-10-28-phase3-m5-recognition-blocking-ip-rewrite-plan.md`。任务的次序：

| 任务 | 内容 |
| ---- | ---- |
| 0 | 开工前核对：第 16 节的各项对着当时的代码核一遍，出入记进「执行期修正记录」 |
| 1 | `rurge-net`：QUIC Initial 的识别与测试用的构造器（第 4 节） |
| 2 | 引擎：所有端口上的 QUIC 与 STUN 识别；测试里的假 Initial 换成真的（第 4.4、5 节） |
| 3 | `block-quic` 的 `adaptive` 与 6.11 的 `auto`：配置、判定、引擎里的表与 TCP 的通知（第 6 节） |
| 4 | 配置：`[IP Rewrite]` 的解析（第 8.1 节） |
| 5 | `rurge-tun`：`[IP Rewrite]` 进包的去向、三种动作、计数；引擎把表交给虚拟网卡；API 的计数（第 8.2 ～ 8.8 节） |
| 6 | `rurge-tun`：`udp-priority`（第 7 节） |
| 7 | 内部 DNS 会话带上主机名（第 9 节） |
| 8 | 配置：手册 6.11 的 WireGuard 写法能加载（第 10.2 节） |
| 9 | 基准 B1、B2、b3 重跑并记数（第 12 节） |
| 10 | 文档收尾（第 15 节） |

**执行的约定**（计划开头重申，同 M4d）：协调的会话逐个任务开 Pi 的会话、把任务的全文与要用的接口交给它；Pi 做完写报告、提交；协调者读改动、跑门禁、评审，有问题回到同一个 Pi 会话修正，再评审，过了才派下一个任务。每个任务先写失败的用例、看到红，再实现到绿；门禁是 `CLAUDE.md` 的全量门禁；提交说明的标题用中文。计划没有写到、要做决定的地方：能按本文与总设计的原则定的，执行者自己定，记进修正记录并写"执行者的裁定"；去向写着"项目所有者"的、或会改变本文已决事项的，停下来列出。仓库的规矩（`CLAUDE.md` 的「必须遵守的规矩」「测试约定」「这台开发机」）一律适用。

做完之后：项目所有者推送，读回那次 CI 的注解与任务摘要；再往后是 M6（阶段 3 的需求逐句对照代码、验收文档、手工验收）。

## 3. crate 与接口

依赖不变：不加包，不加工作区里的依赖边（`rurge-net` 与 `rurge-engine` 已直接依赖 `rustls`，工作区的 `rustls` 开着 `ring`；`rurge-tun` 已依赖 `rurge-config`）。

| crate | 改动 |
| ----- | ---- |
| `rurge-net` | `quic::initial`（新）：`is_client_initial(datagram: &[u8]) -> bool`（第 4 节）。`quic::testing`（只在 `test` / `testing` 特性下）加一个造客户端 Initial 的函数。`Connector` 加两个带缺省实现的方法 `connect_resolved`、`connect_udp_resolved`（第 9.2 节） |
| `rurge-dns` | `BootstrapConnector` 解析出地址之后调内层连接器的 `*_resolved`，把名字带上（第 9.2 节） |
| `rurge-config` | `ip_rewrite`（新）：一张表与它的解析，`Config.ip_rewrite`；`DEFERRED` 去掉 `IP Rewrite`（第 8.1 节）。`block-quic` 的类型从 `Tristate` 换成四个值的 `BlockQuic`（第 6.2 节）。`[WireGuard <名字>]` 认识 `listen-port`、`accept-incoming`（第 10.2 节） |
| `rurge-engine` | `sniff`：`is_quic_initial` 去掉，加 `is_stun`；`udp.rs` 的协议标签与阻断的判定；新的 `adaptive` 表（第 6.3 节）；客户端 TCP 会话的通知；`TunSettings` 带上 `ip_rewrite` 与 `udp_priority`；`dns_pipeline` 与内部会话的路由（第 9 节） |
| `rurge-tun` | `TunSettings` 多两项：`ip_rewrite`（这一代的表）与 `udp_priority`；`packet`：怪地址的检查挪到分片之前，首部之后可以停下来问 `[IP Rewrite]`；`dispatch`：每批读一次设置，命中的包按动作办；`writer`：两条队列；`Context` 的写回接口分类；`Reason::IpRewrite` 与计数 `reflected`（第 7、8 节） |
| `rurge-api` | `GET /v1/features/enhanced_mode` 多两个数（第 8.7 节） |

## 4. QUIC 识别

### 4.1 今天

引擎在 UDP 流的第一个数据报上判断（`crates/rurge-engine/src/udp.rs` 的 `run_flow`）：目的端口是 443、且 `sniff::is_quic_initial` 说是，会话的协议就是 `QUIC`，否则是 `UDP`。`is_quic_initial` 只看包头：不少于 1200 字节、长包头带固定位、版本不是 0、类型是 Initial（v2 是 1，其余是 0）。`block-quic` 与 `PROTOCOL,QUIC` 都看这个标签。

只看包头的判据挡不住长相像 Initial 的别的 UDP：任何不少于 1200 字节、首字节落在 `0xc0` ～ `0xcf` 的数据报都过得去（加密的随机字节的协议里约十六分之一过得去）。今天只在 443 上判断，误判的范围有限；扩到所有端口之后，误判会让 `block-quic`（缺省对代理阻断）挡掉别的协议。所以扩端口与收紧判据一起做。

### 4.2 判据

只看流的第一个数据报，只看其中的第一个 QUIC 包（Length 圈出的那一段，后面合并的 0-RTT 等不看）。先便宜地筛：

1. 不少于 1200 字节（RFC 9000 第 14.1 节：客户端带 Initial 的数据报必须填到 1200）。
2. 长包头（`0x80`）与固定位（`0x40`）都置位。
3. 版本是 v1（`0x00000001`）、v2（`0x6b3343cf`）或草案 29 ～ 32（`0xff00001d` ～ `0xff000020`）。
4. 类型是 Initial：v2 是 `0b01`，其余是 `0b00`（首字节第 4、5 位）。
5. 目的连接 ID 的长度 1 ～ 20，源连接 ID 不超过 20。
6. Token 长度（变长整数）与 Token、Length（变长整数）都在数据报之内；包号的偏移加 Length 不超过数据报的长度；Length 至少 20（头部保护的样本从包号偏移加 4 处取 16 个字节）。

再确认：

7. 用目的连接 ID 与这个版本的 salt 推出客户端的 Initial 密钥（rustls 0.23 的 `quic::Suite::keys(dcid, Side::Server, version)`，`remote` 一侧就是客户端的；套件是 `TLS13_AES_128_GCM_SHA256`，Initial 包固定用它，RFC 9001 第 5.2 节）。版本对应 `quic::Version::V1`、`V2`、`V1Draft`（rustls 的 `V1Draft` 正是草案 29 ～ 32 的 salt）。
8. 去掉头部保护：从包号偏移加 4 处取样本，解出首字节的低四位与包号（长度 1 ～ 4 字节由首字节的低两位给出）。
9. 以去掉保护之后的头部为附加数据，解密 Length 圈出的其余部分（含 16 字节的标签）。**解得开就是 QUIC Initial**；解不开（标签不对）就不是。

判断在数据报的一份副本上做（要改头部的字节），原数据报不动。第 1 ～ 6 步过不去的数据报不分配、不算密钥。解开之后不再看里面的帧：AEAD 的标签对上，伪造的概率是 2⁻¹²⁸ 量级，帧里有没有 ClientHello 不影响结论；SNI 留给阶段 4。

开销：通过第 1 ～ 6 步的数据报（真正的 QUIC 几乎全是）一次 HKDF 推导加一次约 1.2 KB 的 AES-128-GCM 解密，量级是十微秒；每条流只做一次。

### 4.3 版本的取舍

| 版本 | 今天（443 上） | M5 |
| ---- | -------------- | -- |
| v1、v2 | 认 | 认（解密确认） |
| 草案 29 ～ 32 | 认 | 认（解密确认） |
| 草案 28 及以前、gQUIC（`Q046`、`Q050`、`T051` 等） | 包头像就认 | **不认**：salt 不同或根本不是 IETF 的保护方式，解不开 |
| 将来的版本、版本协商用的保留值 | 包头像就认 | 不认，直到 rustls 认识它 |

sing-box、Xray、mihomo 都用解密确认，认的版本与这里相同。gQUIC 在 Chrome 里早已换成 IETF QUIC；草案 28 以前没有在用的实现。少认的这些登记进清单（L249、L177），不在 rurge 里另做。

### 4.4 位置与调用

识别放在 `rurge-net` 的 `quic::initial`（新文件 `crates/rurge-net/src/quic/initial.rs`），引擎调用它；测试用的构造器放在已有的 `quic::testing` 里，引擎与可执行文件的用例经 `rurge-net` 的 `testing` 特性用它（两处的 dev-dependencies 已经开着）。放在 `rurge-net` 而不是引擎的 `sniff.rs`，是因为阶段 4 取 QUIC 里的 SNI 要从同一处解出的明文接着读，构造器也要给几个 crate 的用例共用。

引擎的 `run_flow` 照旧在第一个数据报上定标签，判断不再看端口：

```
QUIC  ← rurge_net::quic::initial::is_client_initial(第一个数据报)
STUN  ← sniff::is_stun(第一个数据报)（第 5 节）
UDP   ← 其余
```

两种判据互斥（QUIC 的首字节最高两位是 `11`，STUN 的是 `00`），次序无所谓。`silence_tells`（smart 组按 53 / 443 与目标是 IP 判断"出声的流"）不改。

### 4.5 行为的变化（登记）

- 443 以外端口上的 QUIC 第一次被认出：`PROTOCOL,QUIC` 匹配它们，`block-quic` 管它们。一个只走 QUIC、不回落到 TCP 的应用（某些游戏、某些 P2P 程序）开在 443 以外，经代理时今天能通，现在照 `block-quic` 办：缺省的 `auto` 下，不载 UDP 的代理直接阻断，载 UDP 的代理按 `adaptive` 第一次阻断、1 秒内没有 TCP 就放行（第 6 节）。
- 443 上包头像 Initial 而解不开的数据报不再是 `QUIC`：今天会被 `block-quic` 挡掉的加密 UDP 协议（恰好用 443、恰好像）不再被挡；gQUIC 不再被挡。
- 用例里那个"长得像 Initial 的假包"（`crates/rurge-engine/tests/common/mod.rs` 的 `quic_initial()`：`0xc3 0 0 0 1` 补零到 1200 字节）从此是 `UDP`。用到它的用例（`tests/udp.rs` 四处、`tests/tun.rs` 的 `a_blocked_quic_flow_answers_with_icmp`）改用构造器造的真 Initial；另加一条用例钉住"假包现在是 UDP"。

### 4.6 用例

- `rurge-net`：RFC 9001 附录 A.2 的客户端 Initial（v1）与 RFC 9369 附录 A.2 的客户端 Initial（v2）——两份公开的测试向量，十六进制写进用例——都认得；构造器造的 v1、v2、草案 29 都认得；quinn 客户端在回环上真发出的第一个数据报认得（交给一个只收不答的 UDP 套接字截下）。认不得的：改了负载里任一个字节、改了 DCID 的一个字节（密钥变了）、1199 字节、短包头、固定位是 0、版本 0、未知版本、草案 28（`0xff00001c`）、v2 的类型写成 0、DCID 长 0 或 21、Length 超出数据报、旧的假包。
- 引擎：端口 8443 上的真 Initial 标成 `QUIC` 并被 `PROTOCOL,QUIC,REJECT` 拒、被 `block-quic` 挡；443 上的旧假包标成 `UDP`；经虚拟网卡的被挡的 QUIC 照旧回 ICMP（改用真 Initial）。

## 5. STUN 识别

判据（RFC 5389 / 8489 第 5、6 节；只看流的第一个数据报）：

1. 不少于 20 字节；
2. 首字节最高两位是 `00`；
3. 第 4 ～ 7 字节是魔数 `0x2112A442`；
4. 第 2 ～ 3 字节的消息长度是 4 的倍数，且 20 加它**恰好**等于数据报的长度（UDP 上一个数据报就是一条消息）。

所有端口（3478、19302、5349 之外的也算）。经典 STUN（RFC 3489）没有魔数，不认；TURN 的 ChannelData（首字节 `0x40` ～ `0x7f`）不认。认出的流标 `STUN`：`PROTOCOL,STUN` 匹配它，`PROTOCOL,UDP` 照旧也匹配（规则引擎已按传输层覆盖）；请求记录与 API 里是 `stun`（`rurge-api` 已有映射）。判据写在引擎的 `sniff.rs`（纯函数，几行），用例手写数据报：Binding 请求（长度 0）、带一个属性的请求认得；魔数错一位、长度不是 4 的倍数、长度与数据报对不上、首字节 `0x80`（RTP）、19 字节认不得。

清单 L249 那句"STUN 与 DNS 的嗅探未做"随之订正：STUN 做了；`PROTOCOL,DNS` 只匹配 rurge 自己发出的 DNS 请求，手册就是这样写的（"only match DNS requests sent by Surge itself"），客户端的 DNS 流不标 `DNS` 不算缺口。

## 6. `block-quic`

### 6.1 虚拟网卡上

经虚拟网卡来的 UDP 流与经 SOCKS5 来的一样走引擎的 UDP 流水线，M3 起 `block-quic` 已经管它们：被挡的流请求记录写 REJECT 与 `QUIC blocked`，每个被丢的数据报由 `TunUdpClient::refused` 回一个 ICMP"管理上禁止"（限频 3 秒 100 个），应用立刻回落而不是等超时（用例 `a_blocked_quic_flow_answers_with_icmp`）；经 SOCKS5 的只能丢。M5 不改阻断的方式，只把识别扩到所有端口（第 4 节）。

### 6.2 `adaptive` 与 6.11 的 `auto`（手册 `policies/parameters.html`）

手册 6.11 的原话见复查报告第 3.1 节。照它：

- 策略参数 `block-quic` 取 `auto`（缺省）/ `on` / `off` / `adaptive`，`true` / `false` 照旧读作 `on` / `off`（今天 `Tristate` 的读法）。配置里换成四个值的 `BlockQuic`（`ecn` 仍用 `Tristate`）。不认识的值照旧是 `E0018`。
- 判定（`quic_blocked` 换成 `quic_verdict`，结果是"放行 / 阻断 / 自适应"三种）：

| 全局 `[General] block-quic` | 判定 |
| --------------------------- | ---- |
| `always-allow` | 放行（照今天：直连类写了 `on` 的也放行，清单 L177 已登记） |
| `all` | 阻断 |
| `all-proxy` | 经代理的阻断；直连类照它自己的参数（下一行的读法） |
| `per-policy`（缺省） | 终端策略的参数：`on` 阻断、`off` 放行、`adaptive` 自适应；`auto`：DIRECT 类放行、**不载 UDP 的代理阻断、其余代理自适应** |

"不载 UDP"即终端出站的 `udp()` 是 `UdpSupport::Unsupported`；组照旧按解析出的终端策略判断。`auto` 的这一行是 6.11 的含义（6.10 是"代理一律阻断"），是缺省行为的变化，登记（清单 L177、L375）。

### 6.3 自适应的表

引擎里一张表（每个引擎一张，跨重载保留，网络变化不清），键是会话的目标：名字（小写、去掉末尾的点；经虚拟网卡来的是 fake 地址反查出的名字），没有名字的是地址的文字。表至多记 200 个，按"最近用到"淘汰最久没用的（手册："up to the 200 most recently used hostnames"）。每一项是三种状态之一：试探中（记着开始的时刻）、已回落、已放行。

| 事件 | 没有这一项 | 试探中（时刻 t） | 已回落 | 已放行 |
| ---- | ---------- | ---------------- | ------ | ------ |
| 一条自适应的 QUIC 流（时刻 now） | 记为试探中（now）；**阻断** | now − t ≤ 1 秒：阻断；否则改为已放行、**放行** | 阻断 | 放行 |
| 一个客户端 TCP 会话（时刻 now） | 不记 | now − t ≤ 1 秒：改为已回落；否则改为已放行 | 不变 | 不变 |

判定是懒的：没有定时器，"1 秒内没有 TCP"在下一次 QUIC 或 TCP 到来时才落定，结果与定时器相同。QUIC 流会刷新"最近用到"，TCP 会话不刷新（表记的是 QUIC 的历史）。"客户端 TCP 会话"是任何入口（HTTP、SOCKS5、虚拟网卡）来的、不是 `Internal` 的 TCP 会话，在它的目标定下来（fake 地址已反查成名字）之后、规则之前通知表；表空的时候这一步只是一次加锁的查找。被自适应挡掉的流在请求记录里与别的被挡的一样写 `QUIC blocked`；状态落定时 DEBUG 一行（`quic: … falls back to TCP; QUIC stays blocked` / `… no TCP within 1s; QUIC allowed`），带名字。

不写的：手册只说"a TCP connection arrives within 1 second"，阻断之前已经在的 TCP 连接（Chrome 先起 TCP 再试 QUIC 的情形）不算；照字面读，登记为没有观察过 Surge 的读法。

### 6.4 用例

判定的表（四种全局 × 四种参数 × 直连与否 × 载不载 UDP）逐格；自适应的表用显式的时刻逐格测（每一格、1 秒的两边、200 个之后的淘汰、名字的大小写与末尾的点）；引擎的接线：经 SOCKS5 UDP、终端是载 UDP 的代理、缺省 `auto` 时，第一条 QUIC 流被挡、请求记录写 `QUIC blocked`，紧接着来一个到同名的 TCP 会话之后，第二条 QUIC 流仍被挡；`block-quic=adaptive` 写在 DIRECT 上同样走表；`E0018` 照旧给不认识的值。"1 秒没有 TCP 就放行"只在表的单元用例里测（用例不睡 1 秒：测试约定只许"什么都没发生"的固定等待）。

## 7. `udp-priority`

### 7.1 今天

写任务（`crates/rurge-tun/src/writer.rs`）只有一条有界队列（`writer_queue` 批，缺省 256），按到达的次序写：各片的 TCP 输出、UDP 会话的回包、应答器的应答、ICMP、回显应答都排在里面。设备写得慢时，一个 UDP 回包要等它前面排着的全部 TCP 批写完（256 批、每批几十个段，量级是十几 MB）。`udp-priority` 已解析（缺省 true），从没被用到。

### 7.2 两条队列

- 写任务有两条有界队列：**优先**与**普通**，容量都是 `writer_queue`。
- 每一轮：等到任一条队列有东西，然后两条都取（各至多 `limit` 批，不等），**优先队列的包排在这一次设备写的前面**。每一轮都取普通队列，普通队列不会饿死；UDP 回包至多等正在进行的那一次设备写。
- 停止与结束照旧：看到停止之后 `STOP_GRACE`（1 秒）为止；两条队列都关了（它们的发送端都没了）才算全部的发送者都结束。
- 优先的一类是**虚拟网卡写回的 UDP 数据报**：UDP 会话给应用的回包（含它的分片）、应答器经 UDP 的应答。其余（TCP 的段与 RST、ICMP 的消息、回显应答、`[IP Rewrite]` 的 `reflect` 与它回的 RST / ICMP）进普通队列。
- 由发送的一方按这一代的 `TunSettings.udp_priority` 选：开着时 UDP 一类进优先队列，关着时一律进普通队列，等于今天的先来先写。每次发送时读设置（`Hooks::settings()` 是一次 `Arc` 克隆）；重载切换的那一刻同一条 UDP 流的几个数据报可能换队列、先后颠倒，UDP 本来就不保证次序。
- 写回的接口：`Context` 里一个小的写回句柄，`send(类, 批)` 等空位、`try_send(类, 批)` 不等；各片仍只拿普通队列的发送端。哪些地方等、哪些不等不变（收包的任务从不等）。

### 7.3 不做的

收包一侧不重排：收包的任务对每个包都不等待地交出去（交给片、会话、应答器的队列），一批里先交 UDP 还是先交 TCP 只差微秒。会话与片在 tokio 上一样调度，没有优先级。这些照实写进清单 L176："尽力而为，只在写回虚拟网卡时生效"。两条队列让写任务最多多攒 `writer_queue` 批（UDP 一类自己的），不进 `queue_bytes` 的预算（那份预算管的是进片的队列）。

### 7.4 用例

写任务的单元用例：先把普通队列放进若干批、优先队列放进一批，再起写任务，记录设备收到的次序——开着时优先的包在第一次写的最前面，关着时（全部进普通队列）它在最后；普通队列满时优先队列照样收；两条队列都关了写任务才结束；停止之后的宽限照旧。发送一方的用例：设置开 / 关时 UDP 会话的回包与应答器的应答进了哪条队列，TCP 与 ICMP 总在普通队列。

## 8. `[IP Rewrite]`

### 8.1 解析

- 节从"解析了、这一版不生效"（`deferred.rs` 的 `DEFERRED`、`W0016`）里拿掉，成为生效的有序节；模块里的这一节照可覆盖节的规则把新行插到顶部（清单 1.5 节，文本层已经这样做）。
- 每行 `<IP 地址或 CIDR> = <动作>`。地址是 IPv4 或 IPv6，单个地址读作 `/32` 或 `/128`；CIDR 带主机位时照 `IP-CIDR` 规则的读法（写计划时核对，第 16 节 V6）。动作 `reflect`、`reject`、`drop`，不分大小写。
- 写坏的行（没有 `=`、地址读不出、前缀长度越界、动作不认识、多出别的东西）：`W0012`，这一行跳过，其余照常（照 `[SSID Setting]` 等列表项的宽容写法；不报错，Profile 照常加载）。
- 结果是 `Config.ip_rewrite`：一张按写的次序排的表，分 IPv4、IPv6 两份存（一个地址只可能命中自己那一族的行，"自上而下第一条"在一族之内不变）。查表 `action(dst) -> Option<动作>` 线性扫这一族的行；手册的例子都是一两行，空表时只是一次判空。
- 用例 `ip_rewrite_is_a_known_section_kept_for_later` 换成"生效的节"的用例：没有 `W0016`、行按次序读成表；另有大小写、IPv6、主机位、坏行各自的 `W0012` 与跳过、模块插在顶部的次序。

### 8.2 一代一张表

引擎每一代建 `TunSettings` 时带上这张表（`Arc`）。收包的任务每取到一批包读一次 `Hooks::settings()`，这一批用同一张表；重载之后的下一批用新表。

### 8.3 在包的去向里的位置

手册："a matching packet is handled as soon as it arrives, before it reaches Surge's TCP/UDP stacks, rules or policies, and it never shows up as a request"。`packet::classify` 的检查次序改成：

| 次序 | 检查 | 不过时 |
| ---- | ---- | ------ |
| 1 | `ipv6` 关着时的 IPv6（只看版本） | 丢弃，`unsupported` |
| 2 | IP 首部读得出、长度对、IPv4 首部校验和对 | 丢弃，`malformed` |
| 3 | 怪地址（未指定、回环、组播、受限广播、IPv4 映射） | 丢弃，`martian`（不写日志） |
| 4 | **`[IP Rewrite]`：按目的地址查这一代的表** | 命中：照动作办（第 8.4 ～ 8.6 节），不往下走 |
| 5 | 分片 | 丢弃，`fragment` |
| 6 | 传输层（协议、首部、端口、校验和） | 照今天 |

第 3 与第 5 步对调了（M3 设计 4.4 节是分片在先）：`[IP Rewrite]` 要排在分片之前（大的数据报的分片也要按目的地址处理），又必须排在怪地址之后（对调未指定或组播地址会造出不合法的包）。对调的代价：既是分片又是怪地址的包从计 `fragment` 改计 `martian`，订正 M3 设计（第 17 节）。

命中的判断只看目的地址，fake 地址也一样（总设计 Q-26，在反查名字之前）；rurge 自己的地址（虚拟网卡的地址、应答器）也不例外：把 `198.18.0.0/15` 写进去会挡掉 DNS，这是使用者写的。IPv6 关着时 IPv6 的包在第 1 步就丢了，IPv6 的行永不命中。

### 8.4 `reflect`

对调源地址与目的地址，其余字节一个不动，写回虚拟网卡（普通队列，不等：满了计 `queue_full`），计 `reflected`。

- 校验和不用重算：IPv4 首部校验和与 TCP / UDP / ICMPv6 伪首部的校验和都是 16 位的反码和，加法交换，对调两个地址和不变。
- TTL / 跳数不减：rurge 不是转发它的路由器。两个都写成 `reflect` 的地址之间若由操作系统来回转发（要开着 IP 转发），每一趟由操作系统减 TTL，不会无限来回。
- 什么协议都对调（TCP、UDP、ICMP、别的协议、分片）。发给 `reflect` 地址的回显请求被送回成一个"对方发来的回显请求"，操作系统回它的回显应答又进虚拟网卡、再被送回，`ping` 因此看到应答（Windows 的防火墙缺省挡入站的回显请求时看不到，手工验收记一句）。
- IPv6 只对调固定首部里的地址；带路由首部（扩展首部）的包照样对调，不做别的。

### 8.5 `reject`

手册："A TCP connection attempt is answered with a TCP RST, and other packets with an ICMP 'administratively prohibited' message"。

自上而下，第一行成立的定：

| 命中的包 | 回什么 |
| -------- | ------ |
| 非首片的分片（偏移不为 0） | 不回（读不到传输层；RFC 1122 / 1812 只对首片回差错） |
| ICMPv4 的差错（类型 3、4、5、11、12）、ICMPv6 的差错（类型 0 ～ 127；下一个首部直接是 ICMPv6 时才读得到类型）；首片也一样看类型 | 不回（RFC 1122 3.2.2、RFC 4443 2.4：不对差错回差错） |
| 不分片的 TCP：首部读不出 | 不回（`rst_for` 要一个读得出的 SYN） |
| 不分片的 TCP：带 RST | 不回（不对 RST 回 RST） |
| 不分片的 TCP：带 SYN、不带 ACK | RST（`packet::rst_for`），受虚拟网卡的 RST 限频（与被拒的 SYN 共用那一个，3 秒 100 个；超出计 `rst_limited`） |
| 其余：别的 TCP 段、TCP 的首片、UDP、回显请求、别的协议 | ICMP"管理上禁止"（ICMPv4 3/13、ICMPv6 1/1，`icmp::prohibited`），来源是原包的目的地址，引用原包的 IP 首部与其后至多 8 个字节；受虚拟网卡的 ICMP 限频（与 UDP 被拒共用那一个，3 秒 100 个；超出计 `icmp_limited`） |

命中的包本身计 `ip_rewrite`。回的 RST 与 ICMP 进普通队列、不等（满了计 `queue_full`）。

### 8.6 `drop`

丢弃，计 `ip_rewrite`。

### 8.7 计数、日志与 API

- 新的丢弃原因 `ip_rewrite`（`Reason::IpRewrite`，`Counters.ip_rewrite`）：`reject` 与 `drop` 命中的包。它是使用者要的结果，**从不 WARN**；每个包一行 DEBUG（`tun: packet dropped`，带原因与两个地址），同别的原因的 DEBUG 行。
- 新的顶层计数 `reflected`（不是丢弃）：送回去的包。不写日志（`reflect` 的流量可能很大）。
- `GET /v1/features/enhanced_mode`：`dropped` 里多 `ipRewrite`，顶层多 `reflected`（与 `stackPanics` 并列）；虚拟网卡没在跑时照旧没有这些数。形状写进 `docs/api/phase3.md` 的 M5 一节。
- 不进请求记录（手册："never shows up as a request"）。

### 8.8 重载与已有的连接

表按代换。重载加上的行对已经建立的连接同样生效：它后来的段被 `reject`（非 SYN 的 TCP 段回 ICMP，应用的 TCP 把它当软错误、多半等到超时）、被 `drop`、被 `reflect`。手册没说，登记为 rurge 的读法（它按包判断，本来就不认连接）。

### 8.9 用例

`rurge-tun` 的内存设备上（L1）：`reflect` 的 TCP、UDP、ICMP、IPv6 与分片——写回的包地址对调、其余字节不变、两种校验和都对、计 `reflected`；`reject` 表里的每一行；限频用尽之后计 `rst_limited` / `icmp_limited`；`drop`；自上而下第一条（`10.0.0.1 = reflect` 在 `10.0.0.0/8 = drop` 之前与之后）；IPv6 关着时 IPv6 的行不命中；fake 地址段里的地址按地址命中、不反查；怪地址先于 `[IP Rewrite]`；分片在 `[IP Rewrite]` 之后（命中的分片按动作、不命中的照旧计 `fragment`）；换一代设置之后下一批用新表；空表时与今天一样。引擎一条：经内存设备的虚拟网卡，Profile 里的 `[IP Rewrite]` 生效，重载换掉它之后新表生效。API 一条：两个新的数。

## 9. 内部 DNS 会话带上主机名（清单 L520）

### 9.1 今天

开着 `encrypted-dns-follow-outbound-mode` 时，DNS 上游的 TCP / TLS / HTTPS 连接与 DoQ / DoH3 的 QUIC 载体经引擎的流水线（`Internal` 会话，`PROTOCOL` 按方案标）。上游写成名字时，`rurge-dns` 的 `BootstrapConnector` 先用引导解析出地址，对每个地址调内层连接器（流水线的 `Face`）的 `connect` / `connect_udp`；流水线只见到地址，会话的目标是地址。手册说"You can then configure a rule for the DNS server's hostname to use a proxy"，rurge 按 DNS 服务器主机名写的域名规则匹配不上它（清单 L520 登记了这一处差异，去向是阶段 3 的会话模型）。

### 9.2 改法

- `rurge_net::connector::Connector` 加两个带缺省实现的方法：`connect_resolved(name, target, opts)` 与 `connect_udp_resolved(name, target, opts)`——"`target` 是 `name` 解析出的一个地址"。缺省实现忽略名字、调 `connect` / `connect_udp`，所以别的连接器一个不用改；`ConnectOpts` 不加字段（它有 44 处字面量建法）。
- `BootstrapConnector::each_address` 解析出地址之后调内层的 `*_resolved`，把名字带上；目标本来就是地址时照旧调 `connect` / `connect_udp`。
- 流水线的 `Face` 实现这两个方法：会话的目标是名字（`HostName::Domain(name)`），地址跟着会话走（`dial_internal` / `dial_internal_udp` 多带这个地址，具体形状写计划时定，第 16 节 V4）。目标是名字而不带地址的会话照旧不进流水线（防自己解析自己的那个回落不变）。

### 9.3 规则

内部会话求值规则时，IP 类规则用跟着会话来的那个地址（`rurge-rules` 已有的 `FixedResolve`），**不经引擎的解析器**：DNS 服务器的名字若再交给解析器，就是自己解析自己。于是 `DOMAIN`、`DOMAIN-SUFFIX`、`DOMAIN-KEYWORD`、`DOMAIN-SET`、规则集里的域名规则第一次能匹配 DNS 会话；`IP-CIDR` 一类照旧按地址匹配；`PROTOCOL,DOH` 一类不变。组的选择上下文（`SelectCtx.host`）随之是名字：`smart` 组给这类会话记的站点从服务器的地址变成服务器的名字（登记）。

### 9.4 拨号

拨号一律拨那个地址：DIRECT 拨它，代理收到的目标也是它（与今天相同），防环的两条回落（终端是以名字配置的代理、链底是 REJECT）照旧拨它。这样不论规则选了谁，这条会话都不需要解析 DNS 服务器的名字。

### 9.5 请求记录与不变的

- 请求记录与会话日志里这类会话的目标是名字；地址照今天的做法记在远端地址里（写计划时核对，第 16 节 V4）。
- 经代理时代理收到的是引导解析出的地址，不是名字：本机的引导解析若被污染，经代理也救不回来。手册没说 Surge 交给代理的是哪一个，登记为没有观察过；改成交名字要先改"引导先解析"的次序，不在 M5。
- 纯 UDP 的上游不经连接器（不变）；上游写成地址的照旧是地址。

### 9.6 用例

`rurge-dns`：Bootstrap 解析出地址之后，内层连接器收到的是带名字的那个调用。引擎（开着 `encrypted-dns-follow-outbound-mode`，回环上的 DoT / DoH 测试服务器，名字经引导解析到 `127.0.0.1`）：`DOMAIN,<上游名>,<策略>` 匹配 DNS 会话；`IP-CIDR,127.0.0.1/32` 照旧匹配；DIRECT 拨的是那个地址；请求记录的目标是名字；整个过程中引擎的解析器没有被问过上游的名字（用一个记着被问过什么的解析器，或一个只要被问就失败的解析器）。

## 10. 手册 6.11 的四页

### 10.1 登记

复查报告第 3 节的四处登记进清单：`block-quic` 的 `adaptive` 与 6.11 的 `auto`（L375，M5 实现，第 6 节）；WireGuard 的 `listen-port`、`accept-incoming`（L66、L356，阶段 7；M5 只让它们能加载，第 10.2 节）；Tailscale 的三个键（`tailscale` 一行，远期；`[Tailscale <名字>]` 照旧整节 `W0016`）；`GET /v1/policies?include_external=true`（10.4 节，阶段 6）。清单文首与 `CLAUDE.md`「工作流约定」里"本项目对照的是 2026-10-14 取的手册文本，对应 Surge Mac 6.10.0 / iOS 5.23.0"一句改成 2026-10-28 的复查（83 页不变、4 页改过，对应 Mac 6.11.0 / iOS 5.24.0），指向复查报告（M5-D18）。

### 10.2 WireGuard 的服务端写法能加载

手册自己的例子（`listen-port = 51820`、两个不带 `endpoint` 的 peer）今天在 rurge 里是 `W0001` 加 `E0023`，Profile 加载不了，违反"不认识或平台不适用的配置项要解析并忽略、记录日志、不能报错"。M5 只改配置层：

- `listen-port`：认识，`W0029`（"rurge 这一版不接受连入的 WireGuard peer，忽略"）。
- `accept-incoming`：值不是 `false` 时 `W0029`（同上）；`false` 什么也不说。
- 不带 `endpoint` 的 peer：节里有 `listen-port` 时不是错误——`W0029`，这个 peer 不进节（rurge 不拨它，也不等它连进来）；没有 `listen-port` 时照旧 `E0023`。
- 节里一个 peer 也不剩（纯服务端的节）：不是错误，节照常加载；用这个节的 `wireguard` 策略是"这一版不实现"（`W0007`，运行时 `Entry::Unsupported`：经它的连接立刻失败，失败的文字说明原因），与别的不实现的写法一样，不让整份 Profile 加载不了。手册对还没握手的 peer 也是"connections … fail immediately"。

用例：手册的例子加载无错、诊断是上面几条；有 `endpoint` 的 peer 与没有的混在一节里时，有的照常进节；没有 `listen-port` 而 peer 不带 `endpoint` 照旧 `E0023`。

## 11. 不做的

### 11.1 `auto-quic-block` 推到阶段 4

手册（`http/mitm.html`）："When a QUIC connection (i.e. HTTP/3) hits the MITM hostname list, Surge automatically blocks that QUIC connection, causing the client to fall back to HTTP/2 or HTTP/1.1 so the traffic can be intercepted by MITM." 它为 MITM 服务：只有回落之后的 TCP 被 MITM 解开，阻断才有用。阶段 3 没有 MITM（`[MITM]` 整节照旧 `W0016`），阻断了也没有人去解开回落的 TCP，反而改变使用者看到的协议；名单的匹配（通配、`-` 排除、端口、`hostname-disabled`）也属于 MITM 的实现。所以推到阶段 4 与 MITM 一起做，订正总设计第 1.4、10 节，清单 L584 的阶段从"3 / 4"改成 4（M5-D7）。

### 11.2 `.local` 答案里带作用域号的 IPv6

今天系统解析库给的带作用域号的 IPv6（`fe80::1%en0`）被丢掉（`IpAddr` 带不了作用域），一个 `.local` 名字只有这样的地址时查找得到空应答（清单 L502）。做它要让 `DnsResult`、`ResolvedAddrs`、缓存、规则的地址与连接器的目标都能带作用域号，牵动整条解析与拨号的类型；局域网设备多半也有 IPv4 地址，增强模式下 `.local` 照旧交系统解析库。不做，清单 L502 的去向写成远期（M5-D19）。

## 12. 基准

M5 改了每个包都走的路（收包时多一次查表的判断、写回时两条队列），收尾前重跑三个基准（`docs/testing.md` 里的命令，release，忽略的用例，不作门禁；只有它们用 `-p`），记下读数，对着 NFR-01 的 70% 的线：

| 基准 | 量什么 | 线 | 上一次（一台 Windows 开发机） |
| ---- | ------ | -- | ----------------------------- |
| B1 | 内存设备上 TCP 单流的吞吐 | 每个方向 0.7 Gbit/s | 中位数上行约 7.2、下行约 5.5 Gbit/s |
| B2 | 一万条空闲连接加一百条活跃连接的内存 | 429 MB | 门槛读数 402.5 MB；保守读数超线，归项目所有者（M3a 计划延后第 15 行） |
| b3 | fake 应答的延迟 | 0.2 ms | 中位数 0.9 µs、p99 3.2 µs、最大 5.8 ms（HashMap 翻倍处） |

`[IP Rewrite]` 是空表（缺省）。读数写进计划的「执行期修正记录」与 `docs/status.md`，照实写是一台机器上的一次运行。不为 `udp-priority` 与 `[IP Rewrite]` 另加基准。

## 13. 日志与个人信息

- `ip_rewrite` 的丢弃只在 DEBUG，带地址（同别的原因的 DEBUG 行）；`reflected` 不写日志。
- 自适应的状态落定在 DEBUG，带名字。
- QUIC / STUN 的识别不写日志：标签在请求记录与会话日志里（今天已有）。
- 内部 DNS 会话的会话日志写名字而不是地址：名字来自 Profile，不是凭据。
- WireGuard 的新诊断不引用密钥（`wireguard.rs` 的 `Report` 照旧只引可以引的键名）。
- 规矩不变：凭据不进日志、错误与 API；进程与网络的东西只在 DEBUG。

## 14. 测试

各节的"用例"汇总：`rurge-net` 的 QUIC 识别（测试向量、构造器、quinn 的真包、反例）；引擎的 STUN、端口与标签、`block-quic` 的判定与自适应的表、内部 DNS 会话；`rurge-config` 的 `[IP Rewrite]`、`BlockQuic`、WireGuard 的新写法；`rurge-tun` 的 `[IP Rewrite]`（内存设备，L1）与写任务的两条队列；`rurge-api` 的两个新数。

沿用的规矩（`CLAUDE.md`「测试约定」与 `docs/testing.md`）：只用回环与端口 0；缺省的 `cargo test` 不改系统设置、不建网卡；有界等待；固定等待只许"什么都没发生"；计时的新用例提交前单独循环跑二十次以上；数"被问了几次"的用例用不重发的 `FanoutOpts`。quinn 截包的用例只在回环上、给每一步时限。M5 没有 L2 用例。

## 15. 文档

| 文档 | 改动 |
| ---- | ---- |
| `docs/surge-compatibility-matrix.md` | 文首的手册基线；L61 与 9.10 节（`[IP Rewrite]` 生效，逐项写 rurge 的做法：`W0012`、位置、三种动作、计数、重载）；L176（`udp-priority`：只在写回时）；L177、L375（所有端口、解密确认、少认的版本、`adaptive` 与 6.11 的 `auto`）；L249（QUIC 所有端口、STUN 做了、`DNS` 的那半句照手册订正）；L502（M5-D19）；L520（DNS 会话带名字，代理收到地址）；L584（阶段 4）；L66、L356 与 `tailscale` 一行（6.11 的键）；10.4 节 `GET /v1/policies`；附录的统计随改动的行 |
| `docs/api/phase3.md` | M5 一节：`ipRewrite` 与 `reflected`；`protocol` 的 `quic` / `stun` 不再限端口；内部 DNS 会话的目标 |
| `docs/testing.md` | M5 的用例过滤词；测试向量的出处；基准的读数 |
| `docs/acceptance/phase3-manual.md` | M5 一节：`[IP Rewrite]` 三种动作（`reflect` 用 `ping`，注意 Windows 防火墙；`reject` 的 TCP 立刻被拒、UDP 收到 ICMP；`drop` 超时）、443 以外端口的 QUIC 被挡、`PROTOCOL,STUN` 挡掉 WebRTC 的 STUN、`curl --http3-only` 经载 UDP 的代理第一次失败而 1 秒后成功（自适应）、按 DNS 服务器名写的规则在请求记录里生效；`udp-priority` 不设手工项（看不出来，照实写） |
| `README.md`、`README_en.md` | 状态、特性表、路线图（两份一致） |
| `docs/requirements.md` | 第 7 节阶段 3 的状态一句；FR 的字面不改（总设计 Q8 归项目所有者） |
| `docs/status.md` | 阶段 3 下新的 M5 一节（做了什么、没做的、必须知道的事、去向写着项目所有者的） |
| `CLAUDE.md` | 「现在在哪里」；「工作流约定」里手册基线那一句（M5-D18） |
| `docs/README.md` | 本文、计划与复查报告各一项 |

## 16. 写计划时要核对的事项

| 编号 | 核对什么 |
| ---- | -------- |
| V1 | rustls 0.23.43 的确切路径与签名：`rustls::quic::{Suite, Keys, DirectionalKeys, Version, HeaderProtectionKey, PacketKey}`、`rustls::Side`、`rustls::crypto::ring::cipher_suite::TLS13_AES_128_GCM_SHA256` 与 `tls13()` / `quic_suite()`；`HeaderProtectionKey::decrypt_in_place` 怎样从首字节定包号的长度 |
| V2 | RFC 9001 附录 A.2 与 RFC 9369 附录 A.2 的客户端 Initial（十六进制），写进计划 |
| V3 | 引擎里客户端 TCP 会话的目标定下来（`turn_back` 之后）、规则之前的那一处，自适应的通知放在那里；`Internal` 的会话不通知 |
| V4 | `choose_policy` 的签名与解析器的来路；内部会话怎样带地址（`dial_internal` 的参数还是会话的字段）；请求记录的远端地址今天怎样记 |
| V5 | `block-quic` 的读法（`r.choice`、`E0018`）与 `CommonParams` 里 `block_quic` 的全部用处（含用例里的字面量） |
| V6 | `IP-CIDR` 规则读 CIDR 的写法（主机位、前缀长度越界），`[IP Rewrite]` 照它 |
| V7 | 写任务队列的全部发送者（`writer.send` / `try_send`）与建 `Context` 的测试辅助 |
| V8 | `Counters` 的全部用处（API、用例、`stats.rs`）；`Warnings` 的 16 位图放得下新原因 |
| V9 | `[WireGuard]` 的解析：`endpoint is required` 的 `E0023` 在哪里报；`NotImplemented` 与 `W0007` 怎样挂到策略上；`W0029` 的提示 |
| V10 | `Hooks::settings()` 的开销（引擎的 `runtime()`）；内存设备的测试辅助怎样给 `TunSettings` |
| V11 | 三个基准的命令与上一次的读数（`docs/testing.md`、M3a 计划的修正记录） |
| V12 | `TunUdpClient::refused` 引用原包的长度、`icmp::prohibited` 与 `rst_for` 的前提（`rst_for` 要一个过了 `classify` 的 SYN） |

## 17. 对总设计与 M3 设计的订正

1. 总设计第 1.4 节 M5 一行与第 10 节"`auto-quic-block`（`[MITM]`，清单 L582 标 3 / 4）：……本阶段按 `[MITM] hostname` 的名单判断目标域名" → 推到阶段 4（第 11.1 节，M5-D7）。
2. 总设计第 10 节"M5 定识别是否扩到所有端口（要权衡误判）与阻断的方式" → 所有端口、以解密确认（第 4 节，M5-D2、D3）；阻断的方式不变（经虚拟网卡回 ICMP"管理上禁止"、经 SOCKS5 丢弃，M3 起已是）。总设计第 2.3 节 Q-22 里 QUIC 识别的范围同此。
3. 总设计第 10 节"发行说明里手册还没写的三件（`block-quic=adaptive`、跨几个包的 QUIC ClientHello、v2 与 draft-29）不在本阶段" → `adaptive` 手册 6.11 已经写了，M5 实现（第 6.2 节，M5-D5）；跨几个包的仍不在；v2 与草案 29 由解密确认覆盖。
4. 总设计第 10 节"`udp-priority`：……包的去向里每批先处理 UDP，写回设备时 UDP 的队列优先" → 只做写回的一半（第 7 节，M5-D8）。
5. 总设计第 8.4 节"`[IP Rewrite]` 的 `reject` 回'管理上禁止'" 与第 8.7 节"`reject` 对 TCP 回 RST、对其余回'管理上禁止'" → 只有不带 ACK 的 SYN 回 RST；RST、ICMP 差错、非首片不回；别的 TCP 段回 ICMP（第 8.5 节，M5-D12）。第 8.7 节"`reflect` 对本机应用的含义在 M5 对着手册定" → 第 8.4 节（M5-D11）。
6. 总设计第 8.1 节包的去向里 `[IP Rewrite]` 排在"坏包、组播、广播、分片……丢弃"之后 → 排在怪地址之后、分片之前（第 8.3 节，M5-D10）。M3 设计第 4.4 节的表：怪地址的检查挪到分片之前，计数表加 `ip_rewrite`。
7. M3 设计第 4.5 节"写任务……一个全局有界队列" → 两条（第 7.2 节，M5-D8）。
8. 总设计第 1.4 节 M5 一行"`.local` 答案里带作用域号的 IPv6（清单 L501，做不到就登记延后）" → 不做（第 11.2 节，M5-D19）。

## 18. 已决事项

| 编号 | 事项 | 决定 |
| ---- | ---- | ---- |
| M5-D1 | 计划与执行 | 一份计划，任务 0 ～ 10；协调的会话逐个任务调 Pi、逐个任务评审；没有平台代码、没有等 CI 的停点；做完请所有者推送、读 CI（第 2 节） |
| M5-D2 | QUIC 识别的范围 | 所有端口，流的第一个数据报的第一个包（第 4.2、4.4 节） |
| M5-D3 | QUIC 的判据 | 包头筛 + Initial 密钥解密确认；v1、v2、草案 29 ～ 32；gQUIC 与草案 28 以前不再算 QUIC，登记（第 4.2、4.3、4.5 节） |
| M5-D4 | 识别放在哪里 | `rurge_net::quic::initial`；测试的构造器在 `rurge_net::quic::testing`；引擎的 `is_quic_initial` 去掉（第 4.4 节） |
| M5-D5 | `adaptive` 与 `auto` | 照手册 6.11：四个值的 `BlockQuic`；`auto` = DIRECT 类放行、不载 UDP 的代理阻断、其余自适应；全局的四种覆盖不变（第 6.2 节） |
| M5-D6 | 自适应的表 | 每个引擎一张、200 个名字、按最近用到淘汰；1 秒的窗口懒判定；客户端 TCP 会话（不含 `Internal`）在目标定下来之后通知；阻断之前已在的 TCP 不算（第 6.3 节） |
| M5-D7 | `auto-quic-block` | 推到阶段 4；清单 L584 的阶段改成 4（第 11.1 节） |
| M5-D8 | `udp-priority` | 写任务两条队列，每轮两条都取、优先的在前；优先的一类是写回的 UDP 数据报；发送的一方按这一代的设置选；收包一侧不动（第 7 节） |
| M5-D9 | `[IP Rewrite]` 的解析 | 生效的有序节；`W0012` 跳过写坏的行；CIDR 照 `IP-CIDR` 的读法；动作不分大小写；按族分存、线性查（第 8.1 节） |
| M5-D10 | 位置 | 首部 → 怪地址 → `[IP Rewrite]` → 分片 → 传输层；怪地址与分片两步对调（第 8.3 节） |
| M5-D11 | `reflect` | 只对调两个地址，校验和与 TTL 不动；普通队列；计 `reflected`（第 8.4 节） |
| M5-D12 | `reject` | 不带 ACK 的 SYN 回 RST；RST、ICMP 差错、非首片、读不出的 TCP 不回；其余回 ICMP"管理上禁止"；共用虚拟网卡的 RST 与 ICMP 两个限频（第 8.5 节） |
| M5-D13 | 计数与日志 | `ip_rewrite`（`reject`、`drop` 命中的包，从不 WARN、逐包 DEBUG）；`reflected`（不写日志）；API 的 `ipRewrite` 与 `reflected`（第 8.7 节） |
| M5-D14 | 重载 | 表按代，每批读一次；已建立的连接的包同样受新表管，登记（第 8.2、8.8 节） |
| M5-D15 | 内部 DNS 会话的名字 | `Connector` 两个带缺省实现的 `*_resolved`；Bootstrap 调它们；流水线的会话目标是名字、地址跟着会话；规则用那个地址、不经解析器；拨号一律拨地址（第 9.2 ～ 9.4 节） |
| M5-D16 | 经代理的 DNS 会话 | 代理收到的是引导解析出的地址（不变），登记（第 9.5 节） |
| M5-D17 | WireGuard 的 6.11 写法 | 能加载：`listen-port`、`accept-incoming` 是 `W0029`；有 `listen-port` 时不带 `endpoint` 的 peer 是 `W0029` 并留在节外；节里没有 peer 时用它的策略是 `W0007`、运行时立刻失败；没有 `listen-port` 而缺 `endpoint` 照旧 `E0023`（第 10.2 节） |
| M5-D18 | 手册基线 | 前移到 2026-10-28 的复查（Mac 6.11.0 / iOS 5.24.0）；清单文首与 `CLAUDE.md` 的那一句随之改；四页的变化登记（第 10.1 节） |
| M5-D19 | `.local` 带作用域号的 IPv6 | 不做，清单 L502 去向远期（第 11.2 节） |
| M5-D20 | 基准 | 收尾前重跑 B1、B2、b3，记数，对 70% 的线；不新加（第 12 节） |
| M5-D21 | STUN | RFC 5389 的魔数、首两位、长度是 4 的倍数且恰好等于数据报；所有端口；第一个数据报（第 5 节） |
| M5-D22 | 诊断 | 不加新码：`W0012`、`W0029`、`W0007`、`E0018`、`E0023` 沿用（第 6.2、8.1、10.2 节） |
| M5-D23 | 依赖 | 不加包、不加依赖边（第 3 节） |
| M5-D24 | 需求文档的字面 | FR-OUT-11 的"UDP 443"与 `[IP Rewrite]` 要不要另立一行不改（总设计 Q8 归项目所有者）；只在第 7 节写状态（第 15 节） |

## 19. 留给项目所有者的

| 问题 | 与 M5 的关系 |
| ---- | ------------ |
| 总设计 Q8：需求文档第 4 节要不要为 `[IP Rewrite]` 另立一行；FR-OUT-11 的"UDP 443"按本设计（所有端口）改字面吗 | M5 照本设计做，不改字面（M5-D24） |
| B2 的保守读数超线（M3a 计划延后第 15 行） | M5 重跑、记数，不改判 |
| 总设计 Q1 ～ Q7 | 与 M5 无直接关系，照旧开着 |

另有两件事要项目所有者做：**推送**（M5 做完之后，读回那次 CI）；**手工验收**（手工清单 M5 一节，连同阶段 1、2、3 一项也没有做过的各节）。

所有者可以改的裁定：本文 M5-D1 ～ D24，尤其 M5-D3（gQUIC 与草案 28 以前不再算 QUIC）、M5-D5（缺省的 `auto` 从"代理一律阻断"变成 6.11 的自适应，缺省行为变了）、M5-D10（`[IP Rewrite]` 排在分片之前带来的计数变化）、M5-D16（经代理时交地址而不是名字）与 M5-D17（WireGuard 的服务端写法读成"不实现"而不是错误）。

## 20. 写计划时的订正

写实施计划（`docs/superpowers/plans/2026-10-28-phase3-m5-recognition-blocking-ip-rewrite-plan.md`）时对着 `9ee270f` 的代码核对，改了正文的五处（编号是计划「计划期决定」的行）：

1. 第 6.4 节"第一条 QUIC 流被挡……紧接着来一个到同名的 TCP 会话之后，第二条 QUIC 流仍被挡"的集成用例不写：两步之间超过 1 秒（机器忙时）第二条就会被放行，用例会时红时绿。它要证的两件分别由表的单元用例（显式的时刻）与 crate 里的一条单元用例（预置一项一小时之后才过期的"试探中"，经 `Dialer::dial` 拨一个 TCP 会话之后变成"已回落"）证（P8）。
2. 第 8.1 节"模块里的这一节照可覆盖节的规则把新行插到顶部（清单 1.5 节，文本层已经这样做）"不成立：代码里没有模块的叠加（模块在阶段 5）。"模块插在顶部的次序"的用例不写，`[IP Rewrite]` 在模块里的叠加随模块一起做（P9）。
3. 第 9.2 节只写了 `BootstrapConnector`；DoQ 与 DoH3 不经它，而是自己经引导解析、在地址上开 QUIC 的路径。为让第 9.1 节说的 QUIC 载体同样带名字，`rurge-net` 的 `quic::Path` 加 `open_resolved`，`rurge-dns` 在服务器写成名字时用它（P15）。
4. 第 9.6 节引擎的用例写"回环上的 DoT / DoH 测试服务器"：测试服务器的证书只给 `localhost` 与 `127.0.0.1`，名字过不了校验。引擎的用例改用 `tcp://` 上游（同一个 `BootstrapConnector`、同一个流水线的连接器）；四种加密上游带名字由 `rurge-dns` 的用例证（P16）。
5. 第 3 节把 `TunSettings` 的 `ip_rewrite` 与 `udp_priority` 写在一处；计划分在两个任务里加（P18）。

## 21. 执行期的订正

1. 第 4.2 节第 2 步写的是“长首部 + 固定位”；RFC 9287 §3.1 允许客户端在首个 Initial 里把 QUIC 位（RFC 9000 §17.2 的 fixed bit）清零，所以包头筛只要求长包头位，固定位可以是 0。第 4.6 节里“固定位是 0 的包头不是 Initial”那个反例随之不再成立（改由“封好之后再翻这一位”的解密失败钉住）。M5 记录第 73 行（R14）。
2. 第 8.5 节的表补两条：ICMPv6 的重定向（类型 137）不回；一个分片首部之外的 IPv6 扩展首部链不被遍历，其后的差错读不到，也不回。引用的长度按地址族分开：IPv4 是首部 + 8 字节（RFC 1122 3.2.2），IPv6 是整个包、至多 1232 字节（1280 的最小 MTU 减 IPv6 与 ICMPv6 的首部，RFC 4443 2.4(c)）——分片首部让传输层首部不在第 48 字节之前。M5 记录第 70、71 行（R11、R12）。
3. 第 10.2 节把 `accept-incoming` 与 `listen-port` 说成同一个理由；手册的意思是前者针对对端经隧道向本机发起的连接，后者才是接受对端连入。两句 `W0029` 的理由分开写。M5 记录第 72 行（R13）。
