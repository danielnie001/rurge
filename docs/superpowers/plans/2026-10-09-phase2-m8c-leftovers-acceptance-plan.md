# 阶段 2 / M8c（遗留事项、报告与验收）实施计划

> **执行方式**：子代理驱动（superpowers:subagent-driven-development）——每个任务一个新的实现者，做完由新的评审者评审，全部做完后整分支评审一次。按 M8 细化设计 M8-D2，本计划写到"改哪里、接口是什么、要哪些用例、判据是什么"为止：实现者先写失败的用例、看到红，再实现到绿；与计划的出入记进末尾的「执行期修正记录」。

**目标**：把阶段 2 各份计划标给 M8 的遗留事项做掉，写出阶段 2 的验收清单与遗留事项总表，阶段 2 由此收尾。

**做法**：每个任务一处独立的改动，互不依赖（任务 4 用任务 7 搬到 `rurge-config` 的 `to_ascii`，所以任务 7 排在它前面执行——见「执行顺序」）；最后一个任务只动文档。不新增 crate。

**依据**：M8 细化设计（`docs/superpowers/specs/2026-10-08-phase2-m8-wrapup-design.md`）第 5、6、7 节；各小节写着登记它的计划与「延后事项」的编号。

## 全局约束

- 不新增 crate。`rurge-config` 加一条依赖边 `idna`（工作区依赖已有，`rurge-proto` 在用）；`Cargo.lock` 里不出现新的包。
- 全工作区 `forbid(unsafe_code)`；MSRV 1.89，edition 2024。
- 凭据不进日志、错误文本、`Debug` 与 API 输出。
- 测试只用回环地址与端口 0，等待都有时限，不用固定的 sleep 当同步（断言"什么也不发生"时除外）；不联网；不改系统设置——系统代理的用例只经 `RURGE_SYSTEM_PROXY_BACKEND=file:<路径>` 的文件后端。
- 不下载、不安装任何东西。GPL 的实现只取协议事实，不抄代码。
- 注释、日志与错误文本用英文，照所在文件的写法；提交说明的标题用中文。
- 门禁：`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test -j 8 --workspace --no-fail-fast`（本机 `RUSTFMT` 指向 gnu 工具链的 rustfmt）。测试二进制无失败用例的异常退出（`STATUS_HEAP_CORRUPTION`）单独重跑那个二进制。

## 评审要点

计划的用例之外最可能咬到使用者的五种情形，各自归到拥有那段代码的任务里：

1. 带不动 1350 字节数据报的路径上，经 `underlying-proxy` 的 `masque`：连接要自己退回 1200，不能一直丢大包（任务 1；用例里用一个丢弃长数据报的载体）。
2. Windows 上已连接的 UDP socket 在对端端口还没开时收到 ICMP 端口不可达（`ConnectionReset`）：跟随出站模式的 DoQ / DoH3 载体不能因此结束内部会话、也不能给 `smart` 的成员记失败（任务 2）。
3. 同一个失效站点的两条会话同时经不同成员失败：恰好一个成员受罚——"查有没有别的成员失败过"与"记下这次失败"是一步（任务 4）。
4. 看门狗不能对会话的第一条流、对协议版本 1 的对端生效；带错误文本的 SYNACK（服务端拒绝这条流）同样解除它（任务 5）。
5. 坏配置时的恢复：另有实例持锁时不碰备份；数据目录或 `state.json` 不存在时什么也不创建（任务 8）。

## 计划期决定

| 编号 | 决定 | 依据 |
| ---- | ---- | ---- |
| P1 | 256 MiB 只给不设流数上限的会话：`Transport::STANDARD` 的连接窗口改为 32 × 每流窗口；`Transport::DNS` 与 `capped_transport` 的 20 MiB 写成各自的常数，不再跟着 `STANDARD` 变 | 设计 5.1"有流数上限的会话不变"；DNS 的连接上每条流只有一个应答，20 MiB 已是宽裕的 |
| P2 | 起步包长只对 `masque` 经载体的连接设：`Transport` 多一个只在载体上生效的字段，`hysteria2` / `tuic` 与 DNS 不设 | 设计 5.2；quinn-proto 0.11.19：`may_fragment()` 为真时 `MtuDiscovery::disabled(initial_mtu, min_mtu)`，当前 MTU 即 `initial_mtu`；黑洞检测（`BlackHoleDetector`）与探测无关，判定后把 MTU 设回 `min_mtu`；对端的 `max_udp_payload_size` 仍是上界 |
| P3 | "quinn 会忽略的载体错误"只有 `ConnectionReset` 一种 | quinn 0.11.12 的 `endpoint.rs`：接收循环对 `ConnectionReset` 直接 `continue`，其它错误让 endpoint 的驱动任务结束 |
| P4 | "没写路径"按写出来的文本判：`https://` 之后直到结尾没有 `/`、`?`、`#`。`https://dns.example/` 照旧发往 `/` | `url` crate 把两种写法都规范成路径 `/`，只能看原文；mihomo 也按"路径为空"与"路径是 `/`"分开处理 |
| P5 | `Age` 从应答里除 OPT 之外的每条记录的 TTL 里减去（不低于 0）；头缺失、不是十进制整数或有多个时不动 | RFC 8484 5.1；OPT 伪记录的 TTL 字段装的是扩展 RCODE 与标志，不能动 |
| P6 | (c) 不给 `OutboundError` 加变体：引擎在 `smart` 的拨号路径上按目标自己判——域名经 `to_ascii` 转不了，或转出来超过 255 字节——这样的目标不回报、不换成员 | 这样的名字任何成员都发不出去，也解析不了；判据落在目标上，比让十几种出站各自标记"本地拒绝"省事，UDP 也一并覆盖 |
| P7 | 看门狗按会话记一个时限：rurge 的 AnyTLS 会话同一时刻只承载一条流（`Session::open` 按值取会话），"只盯最近一次 SYN"与"按流计"是一回事。收到当前流号的 SYNACK（带不带文本）即解除；流在 SYNACK 之前就被本端放下时不解除——三秒内等不到 SYNACK 的空闲会话同样可疑 | anytls-go `session.go`：`sid >= 2 && peerVersion >= 2` 时 `NewDeadlineWatcher(3s, s.Close)`，SYNACK 时取消；服务端在拨通目标**之后**才回 SYNACK（`outbound_tcp.go`），所以复用的会话上目标三秒拨不通也会触发——参考客户端即如此，登记为差异 |
| P8 | 对端的协议版本取自 `cmdServerSettings` 里的 `v=<n>` 一行；没收到或没有这一行按 1 | anytls-go 的 `peerVersion`；rurge 现在把这条设置读过即丢 |
| P9 | 流式的"读端没有数据时才 flush"与缓冲大小做成 `relay` 内部的一个私有参数，忽略的基准在同一个二进制里交替各跑 8 轮取中位数 | M2b 的教训是"顺序测量把漂移看成差别"；参数是运行期的，同一个进程里交替与两份二进制交替等价，且不用编两遍 |
| P10 | 坏配置时的恢复写到标准错误，不初始化日志；数据目录按 `--data-dir` / 环境变量 / 平台缺省取，不创建 | 这条路径在读到配置之前；`Runtime::resolve` 会创建目录，这里不能调它 |
| P11 | `server` 的转换放在 `rurge-config`：`rurge_config::hostname::to_ascii`（从 `rurge-proto` 搬来，检查不变），`rurge-proto` 的同名函数改为转调 | 设计 5.9；`rurge-proto` 依赖 `rurge-config`，反过来不行 |

## 承接事项

| 出处 | 事项 | 任务 |
| ---- | ---- | ---- |
| M7c #14 | 不设流数上限的 QUIC 会话的连接窗口 | 1 |
| M7c #15 | MASQUE 经链式载体的 UDP 余量 | 1 |
| M7c #28 ～ #32 | 载体的错误 | 2 |
| M7c #9、#10 | DoH 的缺省路径与 `Age` | 3 |
| M3c #11 | `smart` 的失败计分 | 4 |
| M2b #1（计划期） | AnyTLS 的 SYNACK 看门狗 | 5 |
| M4b #14、#20、#23、#28，#31 的前半 | WireGuard 的收尾 | 6 |
| M3a #13 | 指向构建失败成员的中继 | 7 |
| M2b #2（P15）的出站一半、M4a #7、M4b #8 | 出站一侧的 IDN | 7 |
| M1b（同一缺陷在 M2c #6、M3a #9 另登记过） | 坏配置时的系统代理恢复 | 8 |
| M2b #16、#30，M2c #1、#15 | 转发循环的 flush 与缓冲；握手的 flush | 9 |
| M7b #8、#16，M7c #33 | IPv4 映射地址的 UDP；用例里的时限 | 10、11 |
| M3c #4 | `CORE_VERSION` | 10 |
| M8b 延后事项 #6、#7、#9、#12 与评审记下的几处 | 互操作夹具逐个取端口；两条偶发失败的用例；`udp-port` 的措辞；已报错的行多说一句的其余各处；转发循环的写约定；几处过时的文档句子 | 11、11、12、10、9、12 |
| M3b #9、#18、#19、#20，M3a #33，M6c #10 的后一半，M4a #5、#11，M8a #4 | （执行期新增）不该 panic 的地方与锁、`with_params`、旧资源管理器的任务、VMess 的重开循环、Keystore 的 Base64、`russh` 的日志、`General` 的 `Debug` | 13 |
| 任务 4 的实现者的发现 | （执行期新增）只因本机 9 号端口开着才通过的用例 | 14 |
| M3a #20 | （执行期新增）解析器可能空转 | 15 |

## 执行顺序

计划写的次序是 1、2、3、5、6、7、4、8、9、10、11、12（任务 4 用任务 7 的 `rurge_config::hostname::to_ascii`）。实际按各任务的第一个提交是 1、3、2、5、6、7、8、4、9、10、11、14、15、13，最后是任务 12（拆成 A、B、C 三部分）；14、15、13 是执行中新加的任务，见各自的小节。排成这个次序的原因、各任务的修正轮怎样插在别的任务之间，见「执行期修正记录」第 1 行。

---

## Task 1：QUIC 的接收窗口与 MASQUE 的起步包长

**文件**：`crates/rurge-net/src/quic/{dial.rs,mod.rs}`、`crates/rurge-proto-quic/src/{session.rs,masque/mod.rs}`，用例在这些文件与 `crates/rurge-proto-quic/src/masque/udp.rs`、`crates/rurge-engine/tests/outbounds_masque.rs`。

**现状**：`Transport::STANDARD` 每流 8 MiB、每连接 20 MiB，`Transport::DNS` 与 `capped_transport`（`session.rs`）都从它取 20 MiB。经载体的路径 `may_fragment()` 为真，quinn 不做路径 MTU 探测，连接停在 1200 字节。

**要做的**：

1. `Transport::STANDARD.connection_window` 取 32 × `stream_window`（268,435,456）。`Transport::DNS` 的连接窗口仍是 20 MiB，`capped_transport` 的下限仍是 20 MiB——各自写明，不再从 `STANDARD` 取。文档注释说明取值的理由（上限不是预占的内存；32 条以上的隧道同时停住且各攒满 8 MiB 时仍会互相拖住）。
2. `Transport` 多一个字段，说明经载体的连接从多大的包起步（`None` 即 quinn 的 1200）；`masque` 的拨号器设 1350，其余不设。经载体的连接用 `TransportConfig::initial_mtu`，`min_mtu` 不动；直连的连接不受影响（照旧 1200 起步加探测）。怎样把"经载体才生效"接进 `quic::connect` / `Dialer` 由实现者定，要求是每个出站仍只构造一次配置所需的 TLS 部分。

**先写的用例**：

- `dial::tests`：`STANDARD` 的 `receive_window` 是 268435456、`stream_receive_window` 是 8388608；`DNS` 的 `receive_window` 仍是 20971520；起步包长只出现在经载体的配置里（`initial_mtu: 1350`），直连的仍是 1200。
- `session::tests`：`capped_transport(1)` 与 `(2)` 仍是 20 MiB，`(3)` 24 MiB，`(40)` 320 MiB（现有的断言照旧通过）。
- `rurge-proto-quic` 里用不设上限的会话（`masque` 对 `FakeH3Proxy`，或 `session::tests` 的回显服务端）：开五条隧道，四条的读端停住、服务端各发 9 MiB，第五条在时限内来回 1 MiB。改动之前这条用例停在第五条上（先看到红）。
- `masque::udp::tests`：经载体的连接握手后 `max_datagram_size()` 不小于 1280；直连的连接不断言具体值，只断言它没有被这项改动设成 1350 起步（配置文本）。
- 同处：载体丢弃长于 1250 字节的数据报时，连接在时限（10 秒）内把 `max_datagram_size()` 降到 1200 以下，之后一个 1000 字节的 UDP 包往返成功。
- `crates/rurge-engine/tests/outbounds_masque.rs`：经 `underlying-proxy`（socks5）的 `masque`，1250 字节的 UDP 负载往返成功（原来被当作超长丢弃）。

**判据**：用例先红后绿；`trust-tunnel` 的 HTTP/3、`hysteria2`、`tuic` 与 DNS 现有的用例不改地通过；全量门禁通过。

**提交**：`feat(net,proto-quic): 不设流数上限的 QUIC 会话 256 MiB 的连接窗口；masque 经载体从 1350 字节的包起步`。

---

## Task 2：载体的错误

**文件**：`crates/rurge-net/src/quic/{carrier.rs,dial.rs}`、`crates/rurge-net/src/packet_datagram.rs`、`crates/rurge-policy/src/cell.rs`、`crates/rurge-engine/src/dns_pipeline.rs`，用例在这些文件与 `crates/rurge-engine/tests/{pipeline,smart}.rs`。

**要做的**（设计 5.3 的五处）：

1. `Carried::try_send`：`poll_send` 返回 `Pending` 而紧接着的 `poll_send_ready` 说可以发时，再试一次 `poll_send`；仍是 `Pending` 才丢。
2. 包装载体对 `poll_send_ready` 的转发补用例：非测试代码里每个包着另一个 `Datagram` 的类型（`dns_pipeline.rs` 的 `FinishOnDrop` 与 `CountingDatagram`、本任务第 3 条若新加一层；`packet_datagram.rs` 的载体把发送排进队列，总是可发，不在此列）各一条——里层说 `Pending` 时外层也是，里层放行并唤醒时外层被唤醒。用一个"开闸才收"的里层（`carrier.rs` 的测试里已有 `Gate`，各 crate 自己写一份小的即可），不看耗时。
3. `ChainConnector::connect_udp` 交出的载体：打开之后报出的错误（发送与接收）补上 `via <策略>: ` 前缀，错误的种类不变——与同一个函数里打开时的错误一致。
4. `packet_datagram` 的接收任务现在把 `recv_from` 的错误丢掉、只让通道关闭，接收方于是读到固定的 `the carrier through the chain has closed`。改为留下载体自己的结束原因：接收方先读完已到的数据报，再拿到载体的原话（如 `socks5: the proxy closed the UDP association`，种类不变），说一次；此后、或载体无话可说时，才是固定的那句。
5. `ConnectionReset`：
   - `Carried::poll_recv` 不把它记作载体的失败（照旧把错误交给 quinn，quinn 会跳过它）；
   - `CountingDatagram::poll_recv` 遇到它不结束会话、不标记上游失败，原样交出；
   - `quic::connect` 只在握手错误是"endpoint 没了"（`DRIVER_LOST`）时取载体的话，别的握手错误用自己的文本。

**先写的用例**：

- `carrier::tests`：一个"第一次 `poll_send` 说 `Pending`、随后 `poll_send_ready` 说可以、第二次 `poll_send` 收下"的载体——`try_send` 返回 `Ok` 且包真的发了出去（原来丢了）。
- 第 2 条的转发用例（每个包装类型一条）。
- `cell::tests`：链上的载体在发送时失败——经它拨号的错误文本以 `via <策略>: ` 开头、`kind()` 与载体给的相同；接收一侧同理。
- `packet_datagram::tests`：载体的 `recv_from` 以 `socks5: the proxy closed the UDP association` 结束 → 接收方读到的错误文本就是它；载体正常结束 → 仍是固定的那句。
- `carrier::tests`：`poll_recv` 交出 `ConnectionReset` 之后 `failure()` 是 `None`；交出别的错误之后 `failure()` 是它。
- `dial::tests`：握手因超时失败、而载体上早先留有一条失败——错误文本是 `quic: the handshake timed out`，不是载体那条。
- `dns_pipeline::tests`（或 `tests/pipeline.rs`）：内部 UDP 会话的载体交出一次 `ConnectionReset` 之后会话仍在进行、请求记录里没有失败、`smart` 的成员没有被记失败；之后载体交出别的错误，会话以失败结束（现有行为）。

**判据**：用例先红后绿；M7c 的"载体此刻发不了时等而不丢""载体失败时拨号以它的原话失败"几条用例不改地通过；全量门禁通过。

**提交**：`fix(net,policy,engine): 载体的错误——再试一次发送、via 前缀、接收一侧留下原话、ConnectionReset 不算失败`。

---

## Task 3：DNS——DoH 的缺省路径与 `Age`

**文件**：`crates/rurge-dns/src/upstream/{mod.rs,doh.rs,doh3.rs}`（TTL 的调整若放在报文一层则加 `message.rs`），用例在这些文件。

**要做的**：

1. `https://` 的上游，写出来的 URL 在主机（与端口）之后就结束时，请求发往 `/dns-query`；`https://dns.example/` 照旧发往 `/`。上游的名字（日志、`rurge dns lookup --trace`、诊断）照旧按写出来的样子显示，不带补上的路径。带用户名口令的写法同样处理。`h3://` 不动。
2. 应答带 `Age` 头时，把应答里除 OPT 之外每条记录的 TTL 减去它（不低于 0），再交给缓存。DoH 与 DoH3 共用同一段代码（`check_head` 之后）。头缺失、解析不了或出现多次时不动。

**先写的用例**：

- `upstream::tests`：`https://dns.example`、`https://dns.example:8443`、`https://u:p@dns.example` 的请求路径是 `/dns-query`；`https://dns.example/` 是 `/`；`https://dns.example/custom` 是 `/custom`；四者的名字都与写出来的一致（带口令的那条不含口令）。
- `doh::tests`（对回环的 `TestServer`）：不写路径的上游向 `/dns-query` 发 POST 并解出应答。
- `doh::tests`：应答里两条 A 记录 TTL 100 与 20、`Age: 30` → 交出的 TTL 是 70 与 0；OPT 记录原样；`Age: abc`、没有 `Age`、两个 `Age` 头 → TTL 不变。
- `doh3::tests`（对 `FakeDoh3`）：`Age: 30` 同样生效。

**判据**：用例先红后绿；全量门禁通过。

**提交**：`feat(dns): DoH 没写路径时发往 /dns-query；应答的 Age 计入 TTL`。

---

## Task 4：`smart` 的失败计分

**文件**：`crates/rurge-policy/src/smart.rs`、`crates/rurge-engine/src/{smart.rs,engine.rs,udp.rs}`（回报失败与换成员的地方），用例在 `smart.rs` 与 `crates/rurge-engine/tests/smart.rs`。

**接口**（任务 7 提供）：`rurge_config::hostname::to_ascii(name: &str) -> Option<String>`。

**要做的**（设计 5.5）：

1. (b) `SmartBook::report_failure`：带 `host` 时，先在站点记忆里看这个站点在记忆期内有没有**别的**成员的失败记录，再记下这次；有，则这次不动成员的记录（不加罚分、不计入连续失败）。"看"与"记"在同一次加锁里完成（评审要点 3）。没有 `host` 的回报照旧。测速（`SmartBook::failure`）不受影响。
2. (c) 引擎：经 `smart` 组的会话，目标是域名而 `to_ascii` 转不了、或转出来超过 255 字节时，失败不回报、也不换成员——第一个成员的错误原样作为会话的错误。TCP 与 UDP 都是。

**先写的用例**：

- `smart::tests`（时间由调用方注入，照文件里已有的写法）：
  - A 在 `x.test` 失败 → A 受罚（分数与连续失败数都变）；随后 B 在 `x.test` 失败 → B 的记录不变，站点记忆里 A、B 都在 `failed`。
  - 之后 A 再在 `x.test` 失败（记忆里有 B 的失败）→ A 不再受罚。
  - 记忆期（一小时）过后 B 在 `x.test` 失败 → 受罚。
  - A 在 `x.test` 失败之后 B 在 `y.test` 失败 → B 受罚（不同站点）。
  - 只有一个成员反复在同一站点失败 → 每次都受罚（没有"别的成员"）。
  - B 在 `x.test` 成功过、之后 A 失败、B 再失败 → B 不受罚（A 的失败在记忆里）；A 的失败之前 B 的成功不算"失败过"。
  - 没有 `host` 的两次失败都受罚。
- `crates/rurge-engine/tests/smart.rs`：
  - 三个成员的 `smart` 组，目标在三个成员上都连不上（假上游回"拒绝"）：三条会话依次经三个成员失败之后，恰好一个成员带罚分。
  - 300 字节的域名经两个成员的 `smart` 组：会话失败一次，只有一个成员的假上游见到连接（或都没见到——出站在本地就拒绝），两个成员都没有失败记录。

**判据**：用例先红后绿；`tests/smart.rs` 现有的用例里凡是断言"每次失败都罚"的，按新规则改写并在报告里逐条说明；全量门禁通过。

**提交**：`feat(policy,engine): smart 的失败计分不被失效的站点拖累；本地就发不出去的目标不计入`。

---

## Task 5：AnyTLS 的 SYNACK 看门狗

**文件**：`crates/rurge-proto/src/anytls/{session.rs,frame.rs,mod.rs}`、`crates/rurge-proto/src/testing/anytls.rs`，用例在 `anytls` 模块与 `crates/rurge-engine/tests/outbounds_tls_family.rs`（或它现有的 anytls 用例所在处）。

**要做的**（设计 5.6，计划期决定 P7、P8）：

1. 读循环记下对端的协议版本：`cmdServerSettings` 的正文按行读，`v=<n>` 即版本；没有这条设置或这一行按 1。
2. `Session::open`：流号不小于 2 且对端版本不低于 2 时起一个 3 秒的时限（常数 `SYNACK_TIMEOUT`）；读循环收到这个流号的 `cmdSYNACK`（带不带文本）即解除；到时则关闭整条会话——流上的读写以 `anytls: the session is closed` 结束，池不再交出它。日志一条 `debug`：`anytls: no answer to a new stream on a reused session; the session is closed`。
3. 时限做成可以在测试里缩短的（照 `rurge-proto-quic` 的 `Session::with_idle` 的做法），用例不真的等 3 秒。
4. `FakeAnyTls` 的脚本加两个开关：对第 N 条流不回 SYNACK；服务端设置里的版本（缺省 2，可设 1 或不发设置）。

**先写的用例**：

- 复用的会话上第二条流收不到 SYNACK：流的读在（缩短后的）时限后以 `anytls: the session is closed` 结束；下一个请求另拨一条连接（服务端见到两条）。
- 会话的第一条流收不到 SYNACK：过了两倍时限数据仍照常往返（没有看门狗）。
- 对端版本 1（或不发设置）：第二条流收不到 SYNACK 也不关会话。
- 带文本的 SYNACK（服务端拒绝这条流）解除看门狗，流照旧以服务端给的文本失败；过了时限会话没有因看门狗而关闭（它的去留照现有行为）。
- SYNACK 在时限之内到：过了时限流仍在工作。
- 经引擎：复用到一条"半死"的会话时会话在几秒内失败（用缩短的时限），而不是挂到空闲超时。

**判据**：用例先红后绿；anytls 现有的用例不改地通过；全量门禁通过。

**提交**：`feat(proto): anytls 的 SYNACK 看门狗（复用的会话上三秒没有应答即关闭）`。

---

## Task 6：WireGuard 的收尾

**文件**：`crates/rurge-proto-wireguard/src/{device.rs,outbound.rs,stack.rs}`，用例在这个 crate 里。

**要做的**（设计 5.7）：

1. 设备任务已经结束（包括协议栈的锁中毒）时，出站把它当作"没有隧道"：下一个连接换一条新隧道，与被接替之后的重建走同一条路。持锁处不再 `expect`：锁中毒时这条隧道标记为已关闭，调用以 `wireguard: the tunnel has closed` 失败，不向上 panic。
2. 隧道被接替或关闭时唤醒全部等待者：`Device::connect`、`Device::query`、`Device::handshake`、`Device::established` 立即结束——被接替的是 `wireguard: a newer configuration of this tunnel is in use`，其余是 `wireguard: the tunnel has closed`——不等各自的时限。`handshake` 与 `established` 现在没有失败的出口，签名随之改，调用方（原生测速、拨号）照着处理。
3. 共用一条在跑的隧道时，把它的代次提到两者里较大的那个：重载 S → S′ → S 而第一条隧道还有连接占着时，S′ 残留的会话不再接替它。

**先写的用例**：

- 设备任务被中止（测试里直接 abort，或让它 panic）之后，出站的下一个连接成功，且用的是一条新隧道（假对端见到新的握手）。
- 协议栈的锁中毒之后（测试里在持锁时 panic 的线程），出站的调用返回错误而不 panic，再下一个连接经新隧道成功。
- 等在 `connect`（握手还没完成）上的调用，在隧道被更晚的配置接替时一秒内以"newer configuration"失败；`query`、`handshake`、`established` 各一条同样的用例。
- 关闭（最后一个持有者放手 / 出站被替换）时等待者以 `the tunnel has closed` 结束。
- 代次：以代次 1 建隧道并留一条连接占着；以代次 2、同样的节与载体共用它；再有代次 2 的另一个配置（冲突的节）来接替代次 1 的残留会话时不被切断——照 `device.rs` 里 `conflict` 与 `generation` 的现有用例的写法构造。

**判据**：用例先红后绿；这个 crate 现有的用例不改地通过（签名变化带来的调用处改动除外）；`grep -n 'expect("the tunnel")' crates/rurge-proto-wireguard/src` 没有结果；全量门禁通过。

**提交**：`fix(wireguard): 设备任务死掉后换新隧道；关闭与接替唤醒等待者；共用隧道取较大的代次`。

---

## Task 7：指向构建失败成员的中继；出站一侧的 IDN

**文件**：`crates/rurge-policy/src/registry.rs`（构建之后的清理）；`crates/rurge-config/src/hostname.rs`（新）、`crates/rurge-config/src/lib.rs`、`crates/rurge-config/Cargo.toml`（`idna.workspace = true`）、`crates/rurge-config/src/spec/mod.rs`（或 `policy.rs`，`server` 进 spec 的地方）；`crates/rurge-proto/src/hostname.rs`（转调）；`crates/rurge-proto-ssh/src/`（目标主机名）；`crates/rurge-proto-wireguard/src/{outbound.rs,dns.rs}`（隧道内 DNS 的问题）。

**接口**（任务 4 使用）：

```rust
// crates/rurge-config/src/hostname.rs
/// `name` as it goes into a request: the A-label form (UTS #46) of a
/// Unicode name; `None` when it has none or the result holds anything but
/// ASCII letters, digits, `-`, `.` and `_`.
pub fn to_ascii(name: &str) -> Option<String>;
```

**要做的**：

1. （设计 5.8）注册表构建之后再过一遍：导入的条目与组级中继派生的条目，中继链上任何一环已因构建失败被略去的，一并略去；每个组一条告警、不带取值（照装配期对悬空中继的那条告警的写法）。重复到不再有可略去的为止。
2. （设计 5.9）`to_ascii` 搬到 `rurge-config`（文档注释与检查原样带过去，`rurge-proto` 的 `hostname::to_ascii` 改为转调，调用处不动）。
3. 策略行的 `server` 是域名时，进 spec 之前经 `to_ascii`：ASCII 的名字原样；Unicode 的转成 A-label；转不了是 `E0018`——`` policy `<名字>`: the server's host name cannot be written in ASCII (IDNA) ``。订阅导入的行同样（按那里跳过一行的现有写法，不带取值）。核对"防环用的服务器名单"与 TLS 的名字取的是 spec 里转换后的名字。
4. `ssh`：把目标主机名写进 `direct-tcpip` 请求之前经 `to_ascii`；转不了以 `ssh: the host name cannot be sent to the server` 失败，不开通道。
5. `wireguard`：隧道内 DNS 发问之前经 `to_ascii`；转不了的名字查找失败，文本 `wireguard: the host name cannot be looked up`。

**先写的用例**：

- `registry::tests`：导入的 X 构建失败、导入的 Y 的 `underlying-proxy` 指向 X → 组的成员里两者都没有，告警一条且不含 X、Y 的参数取值；Z 的中继指向 Y（两环）→ Z 也被略去；派生条目 `M (via X)` 同理。
- `hostname::tests`（`rurge-config`）：原来 `rurge-proto` 里的用例搬过来；`rurge-proto` 留一条"转调"的用例。
- `spec::tests`：`trojan, bücher.example, 443, password=x` → spec 的服务器是 `xn--bcher-kva.example`；`例え.テスト` → `xn--r8jz45g.xn--zckzah`；ASCII 的名字原样（大小写不变）；`a＠b.example`（全角 `＠`）→ `E0018`，文本如上。
- `assemble::tests`：订阅里 Unicode 的 `server` 被转换；转不了的行被跳过，告警不含那一行的取值。
- `rurge-proto-ssh`：目标 `bücher.example` → 假服务端收到的 `direct-tcpip` 主机是 `xn--bcher-kva.example`；转不了的目标 → 上面的错误，假服务端没有见到通道。
- `rurge-proto-wireguard`：隧道内 DNS 问的是 `xn--bcher-kva.example`；转不了 → 上面的错误。

**判据**：用例先红后绿；`cargo tree -p rurge-config -e normal | grep idna` 有一条、`git diff Cargo.lock` 里没有新的 `[[package]]`；全量门禁通过。

**提交**：两次——`fix(policy): 中继链上有构建失败成员的导入条目一并略去`；`feat(config,proto): 策略行的 server 以 Unicode 写时转成 A-label；ssh 与 wireguard 的目标名同样处理`。

---

## Task 8：坏配置时的系统代理恢复

**文件**：`crates/rurge/src/cli/{run.rs,sysproxy.rs}`，用例在 `crates/rurge/tests/cli.rs`（只经文件后端）。

**要做的**（设计 5.10，计划期决定 P10）：`rurge run` 在配置读不了（现在经 `?` 以 1 退出）或有错误（以 2 退出）、即将返回之前，试一次恢复：

1. 数据目录照 `--data-dir` / 环境变量 / 平台缺省取，**不创建**；目录或其中的 `state.json` 不存在时什么也不做。
2. 拿实例锁（与正常启动同一个 `rurge.lock`、同样的"拿不到就不碰"）：拿不到说明另有实例在跑，备份是它的，不碰、不出声。
3. 读 `state.json`；里面有系统代理的备份时取后端、恢复、清掉备份。
4. 结果写到标准错误：
   - 成功：`warning: a previous run left the system proxy pointing at rurge; the saved settings were restored`
   - 失败：`warning: cannot restore the system proxy: <原因>; the saved settings stay in state.json and are restored on the next start`
   - 后端的环境变量取值不对：`warning: <后端的报错>; the system proxy was not restored`
5. 退出码与原来相同；恢复本身出什么错都不改变它。

**先写的用例**（`cli.rs` 的 `run` 模块，照现有的文件后端用例的写法）：

- `state.json` 里有备份、配置有错误 → 退出 2，文件后端里的设置回到备份的内容，`state.json` 里的备份被清掉，标准错误有"restored"那一句。
- 配置文件不存在 → 退出码与原来相同（1），同样恢复。
- 没有 `state.json` → 数据目录里没有多出文件（`rurge.lock` 也不创建——先看 `state.json` 再拿锁）。
- 数据目录不存在 → 不被创建。
- 另一个实例持锁（照现有"第二个实例退出 1"用例的写法先起一个）→ 备份原样、标准错误没有"restored"。
- `RURGE_SYSTEM_PROXY_BACKEND=bogus` → 退出 2，标准错误有"was not restored"，`state.json` 原样。
- `state.json` 里没有备份 → 什么也不输出。

**判据**：用例先红后绿；`cli.rs` 现有的系统代理用例不改地通过；全量门禁通过。

**提交**：`fix(cli): 配置加载不了时也恢复上一次崩溃留下的系统代理设置`。

---

## Task 9：转发循环的 flush 与缓冲的实测；握手的 flush

**文件**：`crates/rurge-engine/src/relay.rs`；`crates/rurge-proto/src/{http.rs,socks5.rs}`；用例与基准在这些文件；结果写进本计划的「执行期修正记录」与 M8 设计 5.11 末尾。

**要做的**（设计 5.11，计划期决定 P9）：

1. `relay` 的复制循环多一个私有的参数：缓冲的大小（8 / 16 / 32 / 64 KiB）与 flush 的时机（"每次写后"——现在的做法——或"读端此刻没有数据时"，即 tokio 的 `copy` 的做法：写完先再读，读端 `Pending` 时先 flush 再等）。两种时机下"最后一段数据写完即 flush""半关闭之前 flush"都要成立——`HoldsUntilFlushed` 的现有用例对两种时机各跑一遍。
   约束（M8b 终审记下的）：各协议的加密流（`AeadStream`、`CipherStream`、Snell 的两种流、VMess 的分块流）有一条共同的约定——返回 `Pending` 的写必须以同样的字节重试，中间不能插进 flush（停着的密文会被发出去，重试时再加密一遍就发了两遍）。新的时机只在一次 `write_all` 完成之后才 flush，不在它中途。
2. 忽略的吞吐基准扩成矩阵：上游分别是裸 TCP、TLS（trojan 对 `FakeTrojan`）、WebSocket（trojan + ws）、VMess（对 `FakeVmess`）；2 种时机 × 4 种缓冲；每种组合交替跑 8 轮（外层循环是轮次，内层是组合），每方向 256 MiB，打印每种组合的中位数与最小 / 最大。`--release` 下运行。
3. 判定：某种组合在 TLS、WebSocket、VMess 三种栈上的中位数都比现状（每次写后 flush、8 KiB）快至少 10%，且裸 TCP 上不慢于现状的 97%，就把缺省换成它；有多个时取最小的缓冲。否则保持现状。无论结果如何，把整张表（MiB/s）写进「执行期修正记录」。
4. `http` 与 `socks5` 出站的握手：每次"写完就等应答"之前显式 flush（CONNECT 请求、方法选择、认证、连接请求）。

**先写的用例**：

- `relay::tests`：两种时机下，写端是 `HoldsUntilFlushed` 时最后一段数据都能到达对端（现有用例参数化）；"读端没有数据时才 flush"的时机下，读端连续有数据时 flush 的次数少于写的次数（数一数）。
- `http` / `socks5` 的测试模块：下层是"只在 flush 时转发"的包装流时握手完成（改动之前挂住——用带时限的等待看到红）。

**判据**：用例先红后绿；基准的整张表进了「执行期修正记录」并写明结论；缺省若改了，全量门禁通过且 `tests/` 里与空闲超时、半关闭有关的用例不改地通过。

**提交**：`perf(engine): 转发循环的 flush 与缓冲——<结论一句>`；`fix(proto): http / socks5 出站的握手写完即 flush`。

---

## Task 10：小项（产品代码）

**文件**：`crates/rurge-inbound/src/{socks5.rs,udp.rs}`（SOCKS5 的目标）；`crates/rurge/src/capabilities.rs`、`crates/rurge/tests/cli.rs`；`crates/rurge-config/src/spec/`（第 3 项）；用例另在 `crates/rurge-engine/tests/udp.rs`。

**要做的**：

1. （M7b #8）SOCKS5 UDP ASSOCIATE 的客户端发往 IPv4 映射地址（`[::ffff:a.b.c.d]`）时，入站把目标归一成 IPv4 地址再交给引擎；回包的来源写回客户端时仍是 IPv4（SOCKS5 的地址类型 1）。TCP 的 CONNECT 目标同样归一。
2. （M3c #4，设计 6.3）`CORE_VERSION` 改为 22。
3. （M8b 延后事项 #12）一行已经有错误时不再多说一句，把 M8b 终审只在两处做了的规则推到其余各处：一个参数自己的取值——或决定它适用与否的那个参数的取值——已经报了 `E0018` 时，不再对它报"不适用""没有效果"一类的告警。已知的情形（M8b 终审用 `rurge check` 试到的，不是穷尽的）：`direct` 上取值写错的 `ecn` / `no-error-alert`；`hysteria2`、`tuic-v5`、`masque`、带 `h3=true` 的 `trust-tunnel` 与 `external` 上取值写错的 `tfo`；`vmess` 的 `tls` 取值写错时对 TLS 参数的 `W0028`；`trojan` 的 `ws` 取值写错时对 `ws-path` / `ws-headers` 的告警；空的 `interface` 与 `dns-follow-interface`；`underlying-proxy` 指向不存在的策略时对 `interface` 的告警。做法由实现者定（读参数的那一层记下"这个参数已经报过错"是一种），判据是下面的用例。

**先写的用例**：

- `crates/rurge-engine/tests/udp.rs`：客户端向 `[::ffff:127.0.0.1]:<回显端口>` 发 UDP，回包回到这条流（请求记录里这条流的下行字节数不为 0），SOCKS5 回包里的来源是 IPv4 的 `127.0.0.1`。
- `rurge-inbound` 的测试模块：CONNECT 的目标 `[::ffff:127.0.0.1]` 交给拨号器时是 `127.0.0.1`。
- `rurge-config` 的 `config::tests`（照 M8b 的 `a_parameter_that_is_an_error_already_gets_no_second_sentence` 扩充）：上面第 3 项列的每种情形各一行，每行恰好一条诊断（那条 `E0018`）；取值都合法时原有的告警一条不少（各配一行反例）。
- `crates/rurge/tests/cli.rs`：`rurge check` 下一行 `#!REQUIREMENT CORE_VERSION>=22 <策略组行>` 生效（原来被跳过），`CORE_VERSION>=23` 的仍被跳过；`rurge-config` 里 `requirement.rs` 的用例用的是注入的版本号，不受影响。

**判据**：用例先红后绿；全量门禁通过。

**提交**：`fix(inbound): SOCKS5 的 IPv4 映射目标归一成 IPv4`；`chore: CORE_VERSION 报告 22`；`fix(config): 已经报错的参数不再多一条告警（其余各处）`。

---

## Task 11：测试的稳固

**文件**：`crates/rurge-proto-wireguard/src/outbound.rs`、`crates/rurge-proto-quic/src/hysteria2/{mod.rs,udp.rs}`（用例的时限）；`tests/interop/src/*.rs`、`tests/interop/tests/common/mod.rs`（端口、`Failures`）；`crates/rurge-dns/src/bootstrap.rs`、`crates/rurge-engine/tests/pipeline.rs`（两条偶发失败的用例）。只动测试与测试支撑代码，不改产品代码。

**要做的**：

1. （M7c #33）`rurge-proto-wireguard` 的 `outbound.rs` 里两处 `< 2 s` 的墙钟断言放宽到 5 秒。
2. （M7b #16）`hysteria2/mod.rs` 与 `hysteria2/udp.rs` 的用例里没加时限的 `carrier.send_to(...).await`（约十处）包进 10 秒的 `timeout`——用文件里已有的辅助函数，没有就加一个。
3. （M8b 延后事项 #6）互操作夹具里逐个取端口的（`SingBox::spawn` 等，`tests/interop/src/`）改用 `rurge_interop::free_ports`：一次拉起的端口全部取到才放开，两个入站不会取到同一个号。同一处顺带：`tests/interop/tests/common/mod.rs` 的 `Failures` 加 `#[must_use]`（建了却不调 `finish` 的用例会静默通过）。
4. （M8b 延后事项 #7）两条在本机连着跑全量时偶发失败的用例：`rurge-dns` 的 `bootstrap::tests::set_upstreams_takes_effect_for_the_next_resolve`（假服务器把 100 毫秒后的重发也数了进去——按问题去重地数，或让这条用例不会触发重发）与 `rurge-engine` `tests/pipeline.rs` 的 `rebind_failure_leaves_no_listeners_and_a_later_rebind_recovers`（重绑刚放掉的端口，`TIME_WAIT` 堆积时 `os error 10048`——让被占的端口由用例自己一直占着，而不是放掉再抢）。改之前先说清楚每条为什么会偶发失败（读代码推出时序，能复现就复现），改法要针对那个原因，不是放宽断言。

**判据**：改动的每个文件的用例通过；夹具渲染出的配置的单元用例不变地通过；这两条用例各自单独连跑 20 遍都通过（这是唯一允许的"连跑"，两条用例都不占临时端口以外的资源）；全量门禁通过。

**提交**：`test: 用例里补时限；互操作夹具一次取齐端口；两条偶发失败的用例不再依赖时序`。

---

## Task 12：文档与验收

**文件**：`docs/acceptance/phase2-acceptance.md`（新）、`docs/acceptance/phase2-manual.md`、`docs/surge-compatibility-matrix.md`、`docs/requirements.md`、`docs/api/phase2.md`、`README.md`、`README_en.md`、`CLAUDE.md`、M8 设计（末尾加「M8c 实施期的订正」）、本计划的「执行期修正记录」「延后事项」与附录。

**要做的**：

1. `docs/acceptance/phase2-acceptance.md`：总设计第 14 节的九条验收标准（按 M8 设计 6.2 订正后的文字）逐条给出证据——用例的文件与名字、互操作用例、CI 运行。还没有证据的照实写（三平台 CI 是否已绿、互操作是否真的跑过，以写文档时的事实为准），不写成已通过。
2. `docs/acceptance/phase2-manual.md`：补"经 tailscaled 的 SOCKS5 接入 tailnet""`masque` 经 `underlying-proxy` 时内层 QUIC 可用""AnyTLS 半死会话三秒内失败"三项，以及 M8b 文档任务没有覆盖到的项。
3. `docs/surge-compatibility-matrix.md`：开头"所有条目目前均处于计划状态"一句、`[Snell Server]` 一行的"待阶段 2 评估后确定"照现状改写（M8b 文档评审记下的）；M8 设计第 8 节的第 5 ～ 12 条逐条登记（DoH 的缺省路径与 `Age`、QUIC 的接收窗口、`masque` 经载体的起步包长、`smart` 的计分、`anytls` 的看门狗、`server` 的 IDN 与"规则与 `[Host]` 须写成 `xn--` 形式"、坏配置时的系统代理恢复、`CORE_VERSION` 22）；改掉与之矛盾的旧文字；附录的统计重数一遍。
4. `docs/requirements.md`：Tailscale 与 Snell v6 几处"待评估"照可行性报告的结论改写（M8b 文档评审记下的三处）；FR-OUT-15（报告已出，三项的结论）、FR-CFG-08（阶段 2 报告 22）、阶段 2 的验收一节指向验收清单；路线图里阶段 2 的状态。
5. `README.md` / `README_en.md`：阶段 2 完成（以验收清单的事实为准）、特性表与路线图；两份内容一致。
6. `CLAUDE.md`：状态一段补 M8c（M8b 一句已在；补上 M8b 终审改的 CI 行为——各安装步骤 `continue-on-error`、`RURGE_INTEROP_REQUIRED` 在工作流一级）；文档索引补本计划与验收清单；命令一节补新用例的过滤词；手册路径的示例改成现在的路径（`policies/<协议>.html`）。
7. 本计划的附录「阶段 2 遗留事项总表」：阶段 2 的 20 份计划「延后事项」的每一行，标上去向——M8 做掉（任务号）/ 关闭（理由）/ 移交（去向），依据是 M8 设计第 7 节；控制者提供已收集好的清单。
8. （M8b 延后事项 #9）`snell` 的 `udp-port`：对照手册原文（`https://manual.nssurge.com/policies/snell.html`）订正代码注释（`spec/snell.rs`、`snell/mod.rs`、`tests/outbounds_snell.rs`）与兼容性清单 4.2 / 4.5 里把它等同于"v5 的 QUIC Proxy Mode 的端口"的说法：手册怎么说就怎么写，rurge 的行为（只解析，`W0029`，UDP 一律经主端口）不变并照实登记。只动注释与文档。
9. M8 设计末尾加「M8c 实施期的订正」，把执行中与设计不同的决定逐条记下；5.11 末尾写上实测的结论。

**判据**：文档里的每个用例名、文件名、命令都对得上仓库（逐条 `grep` 核对）；README 两份的特性表行数相同；`docs/` 里不出现开发机上终端管控软件的产品名与 DLL 名。

**提交**：`docs: M8c 遗留事项、阶段 2 验收清单与遗留事项总表；兼容性清单、PRD、README 与 CLAUDE.md 同步`。

---

## Task 13（执行期新增）：遗留事项总表查出的无人认领的小项

**来由**：阶段 2 遗留事项总表（附录与 `docs/superpowers/plans/2026-10-09-phase2-leftovers.md`）逐行核对了二十份计划的「延后事项」。有九行当初写的去向是"单独小改动""单独排查""阶段收尾时""需要时另议"或"由项目所有者决定"，此后没有任何一份计划接手，代码里也还是原样。M8 是这些行最后的去处，这里一并做掉。另有一项来自本里程碑任务 8 的评审（第 10 项）。十项互不相干，各自很小，各带自己的用例；分三个提交。

**文件**：`crates/rurge-policy/src/{registry.rs,auto.rs}`、`crates/rurge-engine/src/engine.rs`、`crates/rurge-config/src/{policy.rs,general.rs}`、`crates/rurge-config/tests/secrets.rs`、`crates/rurge-net/src/resource/mod.rs`、`crates/rurge-proto/src/{vmess/udp.rs,h2connect/udp.rs,keystore.rs,testing/vmess.rs}`、`crates/rurge-proto-ssh/src/keys.rs`、`crates/rurge/src/cli/run.rs`、`crates/rurge-platform/src/command.rs`。

**要做的**：

第一个提交——不该 panic 的地方与锁：

1. （M3b #19）`PolicyRegistry::round_timeout` 用 `Duration * u32`，`[General] test-timeout` 写成荒谬的大数时溢出 panic：改成饱和运算，并顺着这个取值从配置一路查到每一处做加法或乘法的地方（含 `Instant + Duration`）。用例：成员的测试时限取 `Duration::MAX` 一级的值时 `round_timeout` 返回而不 panic；一条从配置文本进来的用例走到一次测试轮而不 panic。
2. （M3b #20）`Engine::generation_lock()` 在锁中毒时 `expect`，而这把锁只护着 `()`：改用 `PoisonError::into_inner`。用例：让一个线程持着它 panic，之后 `snapshot` 与一次拨号照常。
3. （M3b #18）`AutoGroups::override_of` 在持有状态锁时写告警：把要说的话带出锁再写，行为不变（现有用例若已钉住那条告警就够，否则补一条"告警仍只说一次"）。

第二个提交——三处行为：

4. （M3b #9）`rurge_config::policy::with_params` 按原文取键：订阅行自己带引号的整项 `"k=v"` 认不出键，修饰（`external-policy-modifier`）的同名参数被追加在后、读到的仍是订阅行自己的值；订阅行以未闭合的引号结尾时，追加的修饰整个被吞进那一项。判据是一条不变量：**套用修饰之后的行，读出来等于"原行单独读出来的各项，同名的换成修饰的值，其余修饰项追加在后"**——"读出来"指策略行解析器对它的读法。`client-cert` / `underlying-proxy` 的安全检查（按套用修饰后的整行）不动，补一条"未闭合的引号挡不住它"的用例。
5. （M3a #33）重载之后旧资源管理器的 `url_task` 拿着条目（连同正文）睡满剩余的刷新间隔才发现管理器没了：改成管理器一没，它的 URL 任务至迟 60 秒内结束，不管刷新间隔多长（能立刻结束更好）；不新增 crate。用例（暂停的时钟）：刷新间隔为一天的 URL 条目，丢掉管理器，推进 61 秒，任务已结束、条目已释放。
6. （M6c #10 的后一半）`vmess/udp.rs` 的 `send_to` 取到已结束的连接就 `continue` 重拨，没有次数限制：照 `h2connect/udp.rs` 在 M6 合并后的复查里改成的"只重开一次"改，第二次仍是已结束的连接时以 `vmess: the server ended the UDP connection`（`ConnectionAborted`）失败。把"取连接"的循环做成能确定地测到的形状，用例钉住"至多拨两次"与那句文本；`h2connect/udp.rs` 现有用例没有断言它那句文本，同样的办法顺手补上。

第三个提交——三处整理（第 10 项归入第二个提交）：

7. （M4a #5）Keystore 条目的 Base64 解码在 `rurge-proto-ssh/src/keys.rs` 与 `rurge-proto/src/keystore.rs` 里逐字重复：抽成 `rurge_proto::keystore` 里的一个函数，两处共用，报错文本不变。
8. （M4a #11）russh 自己的日志经 `LogTracer` 进 rurge 的日志，info / debug 级别里可能带服务器发来的文本：照 `boringtun`、`quinn` 的先例定为只留 WARN 及以上，`quiet_dependencies()` 加一条，用例照 `quinn_says_only_what_rurge_cannot` 的样子。
9. （M8a 延后事项 #4）`General` 的 `Debug` 原样打印 `proxy-test-url` / `internet-test-url`：改成与 `CommonOpts.test_url` 一致，并把这两个键写进标记用例 `crates/rurge-config/tests/secrets.rs` 的配置。
10. （任务 8 的评审）`rurge_platform::command::SystemRunner::run` 失败时的文本是 `` `<整条命令>` failed (<状态>): <工具的输出> ``；恢复系统代理的命令把原来的设置当参数写回去，KDE 的 `httpProxy` 一类取值若是带 `user:pass@` 的 URL，命令一失败，口令就进了日志与标准错误。改成报错里的命令行不带凭据——含 `@` 的参数，`@` 之前（scheme 之后）的部分换成 `***`，别的参数原样，工具自己的输出不动。用例：把文本的拼法抽成不用真的运行程序就能测的函数；没有 `@` 的命令文本与原来逐字相同。归入第二个提交。

**不在这里做**：（M3c #1）组环检测把 `smart` 组里写的嵌套组算作边——改它要先证明每一处沿成员关系往下走的遍历都不会从 `smart` 组走进嵌套组，风险与收益不相称；绕开的办法是把那个本来就不起作用的嵌套组从 `smart` 组里删掉。记为已知限制，登记进兼容性清单。

**判据**：每一项的用例先红后绿（第 3、7 项是不改行为的整理，以现有用例全绿为准并说明）；全量门禁通过；`Cargo.lock` 没有新的 `[[package]]`。

**提交**：`fix(policy,engine): 测试时限的溢出不再 panic；代际锁中毒后照常拨号；覆盖的告警带出锁再写`；`fix(config,net,proto): external-policy-modifier 不被引号挡住；旧资源管理器的任务随它结束；VMess 的 UDP 连接只重开一次；外部命令的报错不带凭据`；`refactor(proto,cli,config): Keystore 的 Base64 解码共用一处；russh 的日志只留告警；General 的 Debug 不显示测试 URL`。

---

## Task 14（执行期新增）：只因本机 9 号端口开着才通过的用例

**来由**：CI 一次也没有绿过：最近一次运行里三个平台的 `cargo test` 一步都失败，任务日志要登录才能读，没有人知道失败的是哪些用例。而本项目的每一条用例都只在一台机器上跑过。任务 4 的实现者发现这台机器做了 CI 的机器不做的一件事：**本机回环的 TCP 与 UDP 9 号端口是开着的**（`netstat -an` 显示 `0.0.0.0:9 LISTENING`，像是系统的 discard 服务），TCP 连接立刻被接受、从不应答；GitHub 托管的 runner 上它是关着的，连接被拒绝——Linux 与 macOS 上立刻，Windows 上约两秒后。测试代码在十五个文件里把 `127.0.0.1:9` 当"没有应答的地方"或"从不打算连上的测试 URL"（最大的杠杆是 `crates/rurge-engine/tests/common/mod.rs` 的 `NO_TEST`）。结果或耗时取决于遇到哪种行为的用例，在这里通过、在 CI 上失败。已知的一条是 `crates/rurge-engine/tests/smart.rs` 的 `udp_silence_counts_only_where_answers_always_come`。

**文件**：只动测试与测试支撑代码（`#[cfg(test)]` 模块、`testing` 模块、`tests/` 目录、两个只用于测试的工作区成员）。产品代码不在范围。

**要做的**：

1. **用实验找出每一个依赖者**：在全部测试代码里把 9 号端口换成这台机器上关着的端口（先确认：连接尝试必须被拒绝），跑整套用例，与正常的一遍比较——每条失败的、或者走了不同路径才通过的用例都是依赖者。替换与还原都用脚本机械地做，保持行尾，用 `git status` / `git diff --stat` 核对没有留下什么。通过的用例也要从代码问一遍：立刻拒绝（Linux、macOS）会不会改变它的结果，尤其是计时紧的地方。
2. **让每个依赖者不再依赖固定端口上答什么**：用例要在那个端口"接了不答""立刻拒绝""两秒后拒绝"三种情形下都通过。办法由各条自己选：用例自己拥有的端点（绑端口 0、接受后从不应答的监听，在各平台上相同；接受后立刻关的也是）、关掉用例不打算考的东西（连通性测试与断言无关的组）、断言用例要考的东西而不是顺带的计数。不许靠放宽等待掩盖依赖，不许改变用例要考的东西。
3. **常量本身**：对那些只因为配置需要一个测试 URL 才带着它的配置（`NO_TEST` 及其拷贝）：判断固定的回环端口能不能留——步骤 1 证明使用它的用例都与之无关的地方可以留——，并在常量处的注释里写明对它假设什么、不假设什么。不为整齐去改十五个文件。
4. **产品代码不在范围**，只有一个例外：实验若显示**产品**在两种世界之一里行为不对（挂死、panic、错误的状态，而不只是某条用例的期望），停下，连同显示它的用例写进报告，不在这里修。
5. **UDP**：同样的问题问少数几处用到 9 号端口的 UDP（`crates/rurge-dns/src/upstream/udp.rs` 的测试等）：Windows 把 ICMP"端口不可达"报成下一次接收的 reset，Linux 报在下一次发送上，凡是因此改变用例所见的地方，让用例不依赖它。
6. **同一批文件里的 `closed_port()`**（任务 11 的提交 `6e22fa2` 之后加的）：`crates/rurge-engine/tests/auto_groups.rs` 与 `smart.rs` 各有一个 `closed_port()`，绑端口 0、读号、放掉，再把号写进配置当"挂掉的代理"（五处）；放掉的号回到系统池，并行用例的 `bind(0)` 随后可能拿到它。任务 11 已经为 UDP 去掉了这个竞争（`tests/common` 的 `closed_udp_port()`：取自 20000–29999、现场试绑）：给 TCP 同样一个，与 UDP 共用一趟遍历而不是第二份拷贝，让两个 `closed_port()` 用它。**不要**用"绑了不听的 TCP socket"造关着的端口：Linux 与 Windows 拒绝连接，macOS 默默丢掉 SYN（XNU `tcp_input` 里 `TCPS_CLOSED` 的 `goto drop`），用例会在那里挂住。

**判据**：实验的结果在改任何用例之前写下；每条改动的用例在两种世界里都通过（正常的一遍，与换上关着的端口的一遍——后者同样是机械替换、机械还原）；全量门禁通过。

**提交**：`test: 不再依赖本机 9 号端口开着的用例`。

---

## Task 15（执行期新增）：曾让测试进程空转数小时的解析器用例

**来由**：遗留事项的核对找到一行，是两周前一份计划（M3a #20）送给"单独排查"、此后无人接手的：`rurge-dns` 的 `bootstrap::set_upstreams_takes_effect_for_the_next_resolve` 与 `resolver::tests::a_partial_result_completes_aaaa_in_the_background` 在全工作区运行中各见一两次计时类失败；一次全量运行里 `resolver::tests::aaaa_suppression_after_five_timeouts_and_flush_resumes` 卡住、测试进程以 100% CPU 空转了数小时（同一轮 `cache_fresh_stale_negative_and_coalescing` 失败），单独重跑 3 次都在 2 秒内通过。空转说明解析器或 `MockDns` 在某种时序下可能有忙等循环，值得当作缺陷查——若循环在解析器里，`rurge` 守护进程在生产里也会这样。此后解析器又变了（QUIC 上游、会话代码），循环可能已经不在，也可能还在。

**问题**：**解析器——或测试里的 mock DNS 服务器——有没有一种交错，使它在不等待的情况下一直转？** 值得找的候选：对着已过去的截止时间一遍遍 `sleep`；某一支永远就绪的 `select!`；停顿可以为零的重试 / 重发循环；把"已关闭"当成"再试一次"的循环；`recv` 出错（Windows 上 ICMP"端口不可达"在下一次接收时报错，代码若每次又发一个就一直报）后立刻再收；暂停的测试时钟自动前进，撞进一个把自己重新定在"现在"的循环；在自己的 poll 里唤醒自己的 waker。读的地方：`crates/rurge-dns/src/{resolver.rs,fanout.rs,cache.rs,upstream/udp.rs,upstream/mod.rs,testing.rs}` 与 `testing/`。同一轮出事的两条用例共用了什么（静态量、时钟、端口、缓存）也要看。

**要做的**：

- **在产品代码里找到这样的循环**：先写让它空转的用例，用 `timeout` 兜住，使 RED 是失败不是挂起；再做最小的修复，单独一个提交（`fix(dns): <空转的是什么，几个字>`）；报告里写使用者会看到什么、在什么条件下 `rurge` 进程会被钉在 100% CPU。
- **在 mock 或测试里找到**：就在那里修（`test(dns): …`）。
- **没有找到**：如实写读了什么、每个候选为什么不会空转、还有什么没查；不虚构修复。"没找到，这是排除掉的"是这个任务的完整答案。
- 那一行里没有被别的任务接走的两条计时类用例——`resolver::tests::a_partial_result_completes_aaaa_in_the_background` 与 `cache_fresh_stale_negative_and_coalescing`（以及 `aaaa_suppression_after_five_timeouts_and_flush_resumes`，若它是计时依赖而不是空转）：对每一条从代码里立出它在忙的机器上怎么失败，把用例改到这个原因不再能让它失败，不放宽等待、不改变用例要考的东西，或者说明为什么从代码里看不出来。（`bootstrap::tests::set_upstreams_takes_effect_for_the_next_resolve` 已由任务 11 改好，不动。）放进一个 `test(dns): …` 提交。

**已决**：先读、先推理；不为复现空转去连着跑测试二进制几个小时——每个假设一次有时限的尝试，不开放式地跑；产品代码只为一个用例能演示的循环而改，不做推测性的加固、不重构解析器；每条改动的用例可以单独连跑 20 遍证明它站得住。

**判据**：上面三种结果之一，附候选清单与结论；改动的用例通过；全量门禁通过。

**提交**：产品代码里找到循环时 `fix(dns): <空转的是什么，几个字>`；测试的改动 `test(dns): …`；什么也没找到又没有用例要改时没有提交，只交报告。

---

## 执行期修正记录

下表记执行中与计划的文字不同、或计划没有写到的地方，写到提交 `28340f3`（M8c 的代码在此冻结）。先是贯穿全程的几行，之后按任务分组，任务依执行次序排列；提交写在「修正」一栏里。「原因」一栏写"控制者代项目所有者裁定"的，是控制者替项目所有者作的决定（M8 设计前言：项目所有者 2026-10-06 授权实施者自己决定，不逐项询问），事后都可以改，改的代价写在同一栏。门禁的数字是本机上的，由控制者重新求和日志；互操作用例在本机没有参照服务端而全部跳过，CI 没有在这些提交上跑过，下表里凡是「靠读源码」的结论都没有在别的平台上验证。「简报」「派工单」指任务派给实现者时的说明（以本计划里该任务的文字为准，加控制者的补充）。

| 编号 | 任务 | 修正 | 原因 |
| ---- | ---- | ---- | ---- |
| 1 | 全程 | 执行顺序。计划写的是 1、2、3、5、6、7、4、8、9、10、11、12；按各任务的第一个提交，实际是 1（`7222293`）、3（`105a6fd`）、2（`e6f65b7`）、5（`cf22f19`）、6（`a24b26d`）、7（`cde2ce2`、`010a9ca`）、8（`6a473ed`）、4（`1325bbc`）、9（`a96eabf`、`6752afa`）、10（`21ecb85`、`f58678b`、`57977af`）、11（`6e22fa2`）、14（`6c83d07`）、15（`f28d67c`、`f0412ca`）、13（`5cd3fc8`、`8812e3f`、`eafd6be`），任务 12 的文档拆成三部分排在最后（见第 6 行）。各任务的修正轮也按"同一时刻只有一个写者"插在别的任务的实现之间：下一个文件不相交的任务在评审（只读）时开始实现，修正轮等正在写的实现者提交之后才发出。任务 3 排到任务 2 之前，因为它只动 `crates/rurge-dns`；任务 4 仍排在任务 7 之后（它用任务 7 搬到 `rurge-config` 的 `to_ascii`），任务 8 却先于任务 4 派出（任务 8 在任务 7 的修正轮提交 `2e4cb63` 之后即派，任务 4 在任务 8 的实现提交 `6a473ed` 之后才派），账本没有另记这个先后的理由 | 控制者的做法：同时只让一个实现者写检出，评审者只读，所以评审与下一个任务的实现可以重叠；预检时发现的文件重叠（任务 1 与 2 都改 `dial.rs`，任务 6、7、11 都改 `rurge-proto-wireguard`）靠先后次序避开 |
| 2 | 13（执行期新增） | 任务 13 是执行中加的。遗留事项总表的核对（五个只读的审计，见附录）查出九行，当初的去向写的是"单独小改动""单独排查""阶段收尾时""需要时另议"或"由项目所有者决定"，此后没有任何一份计划接手、代码里也还是原样：M3b #9、#18、#19、#20，M3a #33，M6c #10 的后一半，M4a #5、#11，M8a #4；另有一项来自任务 8 的评审（外部命令失败时的报错带着整条命令行）。M3c #1（组环检测把 `smart` 的嵌套组当作边）起初也在内，随后裁定不改（见第 69 行）。`http` 出站握手的错误归一并入任务 9（M1a #25、M1b #8，它们改的正是那几行），M3a #20 的空转先并入任务 11、后来单列为任务 15 | 控制者的裁定：M8 是这些行最后的去处，"单独小改动"没有主人正是事情丢掉的办法；代价是多一个任务周期 |
| 3 | 14（执行期新增） | 任务 14 是执行中加的。任务 4 的实现者发现本机回环的 TCP 与 UDP 9 号端口是开着的，而测试在十五个文件里把 `127.0.0.1:9` 当"没有应答的地方"，CI 上它是关着的。每一条用例至今只在这一台机器上跑过，这是三个平台的 `cargo test` 都失败的嫌疑之一 | 控制者的裁定：这是不跑 CI 就能排除的最便宜的失败来源之一；排在任务 11 之后、派给 opus 实现 |
| 4 | 15（执行期新增） | 任务 15 是执行中加的。核对里的 M3a #20——一条 `rurge-dns` 用例曾让测试进程以 100% CPU 空转数小时——两周来没有人查；若循环在解析器里，`rurge` 守护进程在生产里也会这样。它先并入任务 11（派给 opus、允许改产品代码），随后因工作性质不同而单列：任务 11 回到它的四项（外加之后在门禁里见到的偶发用例）并改派 sonnet | 控制者的裁定：空转说明解析器或 `MockDns` 在某种时序下可能有忙等循环，值得当作缺陷查；"稳固测试"与"找循环"是两种活 |
| 5 | 11 | 任务 11 的范围先后变过。计划写四项；加过 M3a #20 的空转调查（第 5 项，允许改产品代码）与第三条偶发的用例，随后撤回空转调查（单列为任务 15）；实际派发六项：计划的四项（第 4 项由两条偶发用例扩成七条，见第 49 行）、任务 9 修正轮留下的两项（relay 里 flush 永不完成的用例；假 HTTPS 代理的警报时序）。修正轮又接了任务 14 顺带发现的两条（第 54 行） | 任务 8、9 的门禁与评审里陆续见到的偶发用例；账本 |
| 6 | 12 | 任务 12 拆成三部分、各一个提交、文件互不相交：A（本计划的两张表与附录、M8 设计第 13 节、阶段 2 遗留事项总表）、B（兼容性清单、PRD、API 参考、两份手工验收清单、三处代码注释）、C（验收清单、两份 README、`CLAUDE.md`）。计划写的是一个提交 | 控制者的裁定：一个代理要同时装下约一兆字节的来源；三部分只有单向依赖（C 引用 A、B）；兼容性清单是用户最容易被一句错话误导的地方，交给最强的模型；每部分由评审者逐条核对引用的名字与数字。代价：三个提交而不是一个，一个 sonnet 做不好的部分改派 opus 重做 |
| 7 | 全程 | 2026-10-09 07:30 ～ 11:00，所有子代理因账号的周用量上限停住（opus 模型 HTTP 429，11:00 +0800 复位）。任务 7 的实现者已提交第一部分（`cde2ce2`）、第二部分（IDN）写了一半没提交，11:01 用消息接着做；五个只读的审计 07:29 开始、什么也没写出就停了，11:02 重新开始。此后每个席位用能胜任的最弱模型：五个审计、任务 10 与 11 的实现者、只含测试或文档的 diff 的评审用 sonnet；任务 4、7、8、9 的实现与评审、整分支评审和（拆分后的）B 部分用 opus；之后加的任务 14、15、13 的首轮实现用 opus，任务 9、11、13、15 的修正轮用 sonnet（逐个席位的模型记在账本里） | 控制者的裁定：周额度是项目所有者的，技能的规矩是胜任的最弱模型；代价：一个 sonnet 没有一次做对的部分要多一轮修正 |
| 8 | 全程 | 磁盘写满。任务 8 期间 D: 盘只剩 2.0 GiB，`target/` 有 146 GiB（两天的聚焦构建攒下的）；实现者在 `-p rurge` 的构建里把盘写满，删掉了那次构建自己写出的 2566 个文件后改用 `--workspace` 的聚焦构建；控制者在没有代理构建时 `cargo clean`。约束文件加了一条：门禁前看 `df -h /d`，剩余不足约 15 GiB 就停下报告，不自己删构建产物 | 聚焦构建（`-p <crate>`）因特性合并方式不同会另编一份依赖，累积得很快 |
| 9 | 全程 | 任务 6、7、8、9 的实现者的运行环境不许写报告文件：任务 6、7、8 的报告由控制者据交回消息整理，任务 9 的由控制者存下实现者交回的全文（只把一张表换成指向证据文件的指针）。这几份报告是转述，细节以证据日志和代码为准 | 环境限制 |
| 10 | 全程 | 门禁里的本机偶发：测试二进制无失败用例的异常退出（`0xc0000374`，终端管控软件注入的 DLL，M8 设计 3.5）出现在任务 2（`rurge-engine` 的 `udp`，一个用例都没跑）、任务 6 的修正轮（`rurge-dns` 的 `resolver`）、任务 8 的修正轮（`outbounds_wireguard`）、任务 10 的修正轮（`pipeline`）、任务 11 修正轮的第二遍（`rurge-proto-ssh` 的库用例）、任务 13 的修正轮（`udp_tls_family`），每次只重跑那个二进制，通过，数字是日志与重跑相加；任务 9 的修正轮里 `clippy-driver` 以 `0xc0000005` 退出过一次（重跑通过），任务 14 的实验里一次被 `env` 包装盖住了真实退出码，形状都与那个已知的毛病一致，没有证据 | M8-D4：遇到重跑那个二进制 |
| 11 | 全程 | 各任务提交后的门禁（本机，结果行 / 通过 / 失败 / 忽略；每一行由控制者重新求和日志，标「推算」的没有跑全量）：基线 70 / 2301 / 0 / 3；任务 1 的 2309；任务 3 的 2318；任务 1 修正轮的 2319；任务 3 修正轮的 2320（推算）；任务 2 的 2332；任务 5 的 2342；任务 2 修正轮的 2348；任务 6 的 2362；任务 5 修正轮的 2364（推算）；任务 7 的 2378；任务 6 修正轮的 2384；任务 7 修正轮的 2386；任务 8 的 2395（该任务的门禁留出两条去真实注册表的 `rurge-platform` 用例，跑出来是 2393）；任务 4 的 2408；任务 8 修正轮的 2412；任务 4 修正轮的 2419；任务 9 的 2428；任务 10 的 2435；任务 9 修正轮的 2442；任务 10 修正轮的 2447；任务 11 的 2449；任务 14 的 2449；任务 11 修正轮的 2452；任务 15 的 2454；任务 13 的 2469；任务 15 修正轮的 2471；任务 13 修正轮的 2477。`28340f3` 上：格式检查与 clippy 通过，70 个结果行 / 2477 通过 / 0 失败 / 3 忽略（那一遍丢了 `udp_tls_family` 一个二进制，重跑 7 条通过，所以是 69 / 2470 加 7）；比基线多 176 条，全是各任务新加的用例 | |
| 12 | 全程 | 两个提交的说明排版有误：`21ecb85`（任务 10）主题行之后少一个空行，git 把整个第一段当主题；`d309f33`（任务 9 修正轮）的两行 trailer 缩进在最后一个列表项下，git 不读作 trailer。另外任务 4、8、13 的报告记下它们的提交署名行带 `(1M context)`（分支上较早的提交也是这样），与派工单写的不同：运行环境规定只有用户自己的指示能改它，控制者裁定不用管，之后的派工单改成"你的环境给你的署名行" | 提交不改写；推送前是否 reword 由项目所有者决定（见延后事项） |
| 13 | 1 | 起步包长的接线与计划预检不同：`Transport` 多一个字段 `carried_initial_mtu: Option<u16>`，`Transport::apply`（直连）旁加 `apply_carried`，`Path` 多 `is_carried()`；`rurge-proto-quic` 的 `Dialer` 留两份 `quinn::ClientConfig`（TLS 部分只构造一次、两份共用），拨号时按真实打开的路径选一份（`Dialer::path`；经载体的是 `masque` 的 `CARRIED_MTU`，其余不设），`quic::connect` 没有动（预检表写的是"`connect` 应用它"），`rurge-dns` 一行没动。计划里 `dial::tests` 的最后一项（起步包长只出现在经载体的配置里）是实现之后才写的 `only_a_connection_over_a_carrier_starts_from_the_carried_size`（字段不存在时编不过）；比计划多三条用例：`a_path_says_whether_it_goes_over_a_carrier`、`a_connection_takes_the_transport_as_its_path_does`（只看两份配置的文本，测不出"直连的连接也拿了经载体的那份"）、`a_tunnel_gets_through_a_path_that_cannot_carry_the_first_size`（回落若不起作用，受害最重的是 TCP 隧道，而回落用例只用 UDP 驱动） | `quinn::ClientConfig` 不透明，传输配置没有读取口：让 `connect` 自己按路径选，要么多一个与 `client_config` 里已烤进去的那份重复的参数，要么把配置换成一对，后者让 `rurge-dns` 的 5 个文件、11 处跟着改，而这项功能只有 `masque` 用 |
| 14 | 1 | 对 M8 设计 5.2 的订正与没有实测的：(1) quinn 的黑洞检测要**四**批"只丢大包"才触发（`BLACK_HOLE_THRESHOLD` 是 3，条件是超过 3 批），设计写的是三批；(2) 缺省 `mtu`（1280）的 WireGuard 带不动 1350 字节的数据报（隧道里的 UDP 负载至多 1252 / 1232 字节），设计的"缺省 MTU 的 WireGuard 也带得动"不成立；那时是分片送出、整包送出还是被丢弃而回落到 1200，只读了 smoltcp 0.12 的源码、没有实测；(3) 回落是每条连接各自的、此后不再升回：窄路径上每建一条连接都先丢一轮满包；回环上（debug 构建，往返 0.64 ～ 0.86 毫秒）UDP 驱动 187 ～ 226 毫秒、TCP 驱动 158 ～ 217 毫秒；(4) 经载体的数据报余量按 quinn 的算式是 1310（服务端连接 ID 8 字节）或 1298（20 字节）字节，回落到 1200 之后是 1160；`mtu=1280` 的 WireGuard 满包（1312 字节）经 `masque` 放不下，要设到 1264 以下，这也是按余量算的、没有实测 | 实现者与评审读 quinn-proto 0.11.19 的源码得出；任务 1 的判据只要求 1250 字节的 UDP 负载往返成功（`over_an_underlying_proxy_a_packet_of_1250_bytes_goes_through`），没有跑真正的内层 QUIC 握手 |
| 15 | 1 | 评审发现 quinn 一次公布连接窗口的八分之一（quinn-proto 0.11.19 `streams/state.rs` 的 `add_read_credits`：`local_max_data − sent_max_data ≥ receive_window / 8` 才发 `MAX_DATA`）：没人读的数据超过窗口的八分之七时，其余隧道才被拖住——256 MiB 的窗口上是 **29** 条攒满 8 MiB 的隧道（设计 5.1 与第一版提交写的是 32 条）。同一条规则让 M7c 的 `capped_transport`（max(`max-streams` × 8 MiB, 20 MiB)）在 `max-streams` ≥ 9 时保不住"其余隧道都停住时最后一条仍在读"（新用例 `the_last_tunnel_goes_on_beside_all_the_others_unread` 在旧窗口下停在 8,387,877 字节）。修正轮 `17dd21f`：带上限的会话的窗口改为 ⌈`max-streams` × 8 MiB × 8/7⌉、至少 20 MiB（缺省 `max-streams=3` 时 28,760,942 字节，原 24 MiB）；窗口是 u32，从 `max-streams` 448 起饱和在 `u32::MAX`，那条保证到 449 为止成立、从 450 起保不住；带上限的会话换窗口时保留 `Dialer` 的起步包长；一条连接最坏缓冲的未读数据仍是 `max-streams` × 8 MiB（各流自己的窗口封顶），多出来的七分之一只是额度上的余量；每流窗口 8 MiB 不变（流级额度同样按八分之一公布，对单条流无害）；HTTP/2 池的窗口不需要这个余量（读 `h2` 0.4.19 的流控：阈值随剩余窗口缩小）。M7c 那条用例的函数体因此挪进共用的 `unread_tunnels`，断言与参数不变 | 控制者代项目所有者裁定：在任务 1 的修正轮里改，尽管是 M7c 的代码——同一个函数、同一个主题，这条保证正是该函数存在的理由 |
| 16 | 3 | 实现与计划不同的几处：(1) 计划期决定 P4 的"没写路径"判据在 `/`、`?`、`#` 之外还数 `\`——`url` crate 把 `https` URL 里的 `\` 当 `/`，不数的话 `https://dns.example\custom` 会被判成没写路径、路径被覆盖成 `/dns-query`；(2) 没写路径的上游，名字就是写出来的样子（`https://dns.example`，没有末尾的 `/`；此前显示成 `https://dns.example/`），名字是规范形式——主机小写、缺省端口与用户名口令去掉——而不是逐字符的原文，`h3://dns.example` 的名字仍带 `/dns-query`，自己托管、在 `/` 上应答的服务端从此必须明写 `/`；(3) `Age` 在报文原处改写（`message::lower_ttls`：应答、权威、附加三个段，OPT 不动；走不到最后一条记录的报文原样交出），不经 hickory 重新编码，那样会连带改动名字的压缩与 OPT 的位置；减到 0 的应答这一次照用、不进缓存（`DnsCache::put` 不存 TTL 为 0 的条目）；(4) 接口变了：`UpstreamSpec::Https { url, pathless }`、`DohUpstream::new(url, pathless, http)`，常量 `DEFAULT_H3_PATH` 改名 `DEFAULT_PATH`，`Doh3Script` 多 `age` | (1) 判据取自写出来的文本；(2) 控制者代项目所有者裁定：两种写法现在去的路径不同，名字相同的话日志里分不出，名字还是在途查询合并的键，代价是日志里一个名字变了；(3)(4) 保住"报文的其余每个字节都是服务端发来的样子"，用例能逐字节断言 |
| 17 | 3 | 计划期决定 P5（"`Age` 有多个时不动"）被取代。修正轮 `aee16df`：`Age` 按 RFC 9111 5.1 读成列表（多个头，或一个头里逗号分隔的多个成员），取第一个成员——两侧去掉空格与制表符、跳过空成员——，该成员不是非负十进制整数时这个头才不算，装不进 `u32` 的按 `u32::MAX`（全部 TTL 归 0）。收下应答的一步收成 `doh::answer(headers, body)`（DoH 与 DoH3 各调一次，`check_head` 仍在读正文之前），`age` 与 `check_body` 退回 `doh.rs` 私有；补了"不写路径又带用户名口令"的用例和三种畸形报文的用例 | 控制者代项目所有者裁定：P5 比 RFC 严而没有理由，不算 `Age` 的方向正是 RFC 8484 5.1 要防的（TTL 偏长）；代价：带双 `Age` 头的应答按第一个算。因此不再有"与 RFC 的出入"要登记 |
| 18 | 2 | 第 5 条第三项照字面做不成：`quic::connect` 认的"endpoint 没了"是 quinn 的两种握手错误——`DRIVER_LOST` 的传输错误**与** `LocallyClosed`（连接的驱动退出时，隐式关闭把前者改写成后者；单线程运行时上拨号读到的总是后者）。照字面只认 `DRIVER_LOST`，M7c 那条必须不改地通过的 `a_carrier_that_cannot_send_fails_the_dial_in_its_own_words` 就红。计划期决定 P3 与设计 5.3 第 5 条只读了 endpoint 的接收循环；握手完成之前本仓库没有别处会关连接，所以握手期间的 `LocallyClosed` 就是 endpoint 没了 | 实现者读 quinn 0.11.12 的 `connection.rs` 得出；简报自己的判据（M7c 的用例不改地通过）已经在两种读法之间做了选择 |
| 19 | 2 | 其余与计划（设计 5.3）不同的实现：第 2 条做成不看耗时的转发用例，三层各一条（`FinishOnDrop`、`CountingDatagram`、新加的 `Relayed`），不是设计写的"看首包耗时"；第 1 条的规则是"试两次都收不下、且载体两次都说可发才丢；第二次之后载体说发不了的留给 quinn 等"，那半步的用例在修正轮补（`a_carrier_that_cannot_send_after_all_has_quinn_wait_for_it`）；第 4 条：载体自己有话（自己的文本，或系统错误）才留作结束原因，只有种类没有文本的错误（本仓库的载体在"收件箱关了"处正是这样写的）不留，接收方直接读到固定的那句；第 3 条的 `via <策略>: ` 前缀只加在 `ChainConnector::connect_udp` 交出的载体（`Relayed`）上，`open_udp` 交出的 `PacketSocket` 没有动，`Relayed` 也给 `poll_send_ready` 的错误补前缀 | 简报没有写到的取舍，实现者提出 |
| 20 | 2 | 评审的第一条意见与修正轮 `df814a6`：(1) 端到端用例 `a_doq_carrier_that_outlives_a_port_unreachable_is_no_failed_session` 重新绑定一个放掉的端口，在 Linux / macOS 上什么也没测（Linux 把 ICMP 错误交给下一次发送，接收一侧收不到；macOS 的接收拿到 `ConnectionRefused`；这两条是评审读出来的，没有跑），还给那里添了偶发；改成只在 Windows 上编译、不起服务端、不按号绑定（Windows 上本机实测的红是 `os error 10054`）；(2) 载体自己**结束**时的 `ConnectionReset`（跑在一条流上的 UDP 载体 `trojan`、`anytls` 在服务端重置连接时正是这样结束）以 `ConnectionAborted` 加同样的文本交出（`as_the_end`）——"错误的种类不变"唯一的例外，只对结束原因；否则 quinn 把它当作被弹回的一个数据报跳过，用户拿到固定的那句而不是原话（三条用例 RED：接收方、经链拨号、内部会话）；(3) 握手丢了 endpoint 时两种 quinn 错误读作同一句 `quic: the UDP path to the server failed`（`BrokenPipe`），此前视 quinn 报的是哪一种，也可能是 `quic: the connection was closed`（`a_handshake_that_lost_its_endpoint_says_the_path_failed`）；(4) 补 `Relayed` 的 `peer_addr` / `set_tos` 断言与"先报发送失败、后报结束原因"的次序用例 | 控制者代项目所有者裁定（对 (2)）：stream-based 的 UDP 载体的结束正是如此，不例外则第 4 条的目的落空；代价：别处若按 `ConnectionReset` 查找，看到的是 `ConnectionAborted`（没有这样的代码） |
| 21 | 2 | 载体报出 `ConnectionReset`（Windows 上改走直连的载体收到 ICMP 端口不可达）不再结束内部 UDP 会话、不进请求记录、不给 `smart` 的成员记失败；"不给成员记失败"只在 `dns_pipeline::tests` 里用真的 `SmartBook` 测，`tests/smart.rs` 没有动（回环上没有哪种载体会交出 reset）。M7c 延后事项 #28 ～ #32 全部做掉 | |
| 22 | 5 | 看门狗与参考客户端的出入（计划期决定 P7、P8 之外）：只认**当前这条流**的 `cmdSYNACK`（参考实现任何 `cmdSYNACK` 都解除），更早那条流迟到的应答流号对不上、不解除；流在应答之前被本端放下时时限照旧，回到池里的会话到时同样关闭；对端版本取自服务端设置里的 `v=<n>`，没收到设置时是 0，判据 `>= 2` 与"按 1"等价；时限先起、再写 SYN（反过来的话，多线程运行时上应答可能先被读循环处理、找不到要解除的东西，3 秒后把好会话误关）；时限做成 `Session::with_synack_timeout`（`pub(crate)`），生产代码里每条新会话也调它一次（为了不改 `Session::start` 的签名），出站上的同名构造才是 `cfg(test)`；请求**不重试**，3 秒的失败出现在转发阶段（请求记录的 `error`），不是 502（复用的会话上 `connect_tcp` 立刻返回） | 实现者读 anytls-go 的 `session.go` 与 `outbound_tcp.go`（只取事实）；"请求已写出、不能透明重试，但 3 秒失败远好于挂 10 分钟"（设计 5.6） |
| 23 | 5 | 经引擎的用例没有用缩短的时限，而是用 tokio 的 `pause()` / `advance(3 秒)` / `resume()` 把时钟拨过生产常数；"什么也不发生"的四条用例同样让生产的时限在 tokio 的时钟上过去两倍；"该关闭"的两条用 300 毫秒的缩短时限与真实时间。拨钟那一跳之后有一个 20 毫秒的真实 sleep：让被唤醒的会话任务先运行一次，没有它变异检查放过了一处错。看门狗的 `debug` 日志那一行没有用例 | 出站是引擎的工厂建的，测试够不到时限；加公开的钩子要么给 `rurge-proto` 加 `testing` 特性下的进程级覆盖值，要么动 `rurge-engine/src`，都不值；`rurge-proto` 没有日志捕获，加依赖边约束不允许 |
| 24 | 5 | UDP 的后果（设计与简报都没提）：参考服务端对 UDP over TCP 的流先读到 UoT 请求、再回 `cmdSYNACK`，而 rurge 的 UoT 请求随第一个数据报发出，所以复用的会话上打开 UDP 载体后 3 秒内没发出第一个包，这条会话就被关。引擎的 UDP 流水线开了就发，不受影响。实现者说的 WireGuard 那一条是错的，评审更正：`wireguard` 经 `underlying-proxy` 到 `anytls`、endpoint 写成域名时，隧道空闲的五分钟重拨拨出的新载体当场丢掉（地址没变），若它落在复用的 AnyTLS 会话上，这条池中的会话 3 秒后被关，隧道本身不受影响；"开了、什么也没发"的同样代价还有三处：隧道启动时遇到已有的共用隧道而丢掉自己刚拨好的载体、打开载体的过程中被取消的 UDP 流、第一个数据报在写出任何东西之前就被载体拒绝。与参考客户端共有的两个后果：一次大的传输被中断后，下一个请求的应答排在旧流的数据后面，慢的链路上可能在 3 秒时失败；服务端从未发过设置的会话从不受监视。修正轮 `16b54c5`（只动测试）让假服务端照参考的次序应答 UDP 流，并补 `a_udp_carrier_on_a_reused_session_is_answered_with_its_first_datagram` 与反面用例 `a_udp_carrier_that_sends_nothing_costs_its_reused_session` | 控制者代项目所有者裁定：看门狗对 UDP 流仍在 `open` 时起表、不挪到第一个数据报——这是参考客户端的行为，引擎里没有哪条正常路径开了载体而不立刻发，而且会话最脆弱的状态不该再多一种形状；代价：开了不用的载体会让那条池中的会话在 3 秒后关闭 |
| 25 | 6 | 计划第 2 条括号里的"关闭（最后一个持有者放手 / 出站被替换）时等待者以 `the tunnel has closed` 结束"做不成测试：每个等待者自己持有 `Arc<Device>`，这两种关闭下不可能有等待者；改用等待者眼前会发生的两种关闭来测——设备任务死掉、协议栈的锁中毒。设计 5.7 的订正：`connect` 以前并不总是等满拨号时限（排队中的 SYN 立刻结束，只是文本不对：`the destination refused the connection`，现在是 `wireguard: a newer configuration of this tunnel is in use`） | 实现者提出 |
| 26 | 6 | 实现的形状与随之的行为：隧道只有一种结束方式 `Ending::{Superseded, Closed}` 与 `Shared::end`，三个入口——被更晚的配置接替；设备任务以任何方式结束（panic、被中止都算）；协议栈的锁中毒；取锁只剩 `Shared::stack()` 与 `Shared::running()` 两个口子，持锁处不再 `expect`（原 17 处）。隧道结束后，它上面已有的 TCP 连接读到 reset、写得 broken pipe（只有锁中毒才是 `wireguard: the tunnel has closed`），等待者与 UDP 载体读到那两句 `wireguard: a newer configuration of this tunnel is in use` / `wireguard: the tunnel has closed`；隧道内 DNS 在隧道结束后不再往后问 `system`；`Device::handshake`、`established`、`query` 的返回值变成 `Result` | 计划只写了"签名随之改" |
| 27 | 6 | 修正轮 `996667e`（评审发现的相邻缺口，控制者一并收进）：(A) 还没绑过 socket 的 UDP 载体在隧道结束时被唤醒（`Shared.ended: Notify`）；(B) 载体的两把小锁中毒时不 panic——旧代码在 `Drop` 里二次 panic，进程中止（RED 是 `0xc0000409`）；(C) 隧道自己结束后，在下一个连接到来时记一条 warn（`wireguard: the tunnel ended unexpectedly; the next connection starts a new one`，带策略名，每条这样结束的隧道一次，被接替不记），出站发现处即清空槽；(D) 设备任务持锁 panic 的用例；(E) M4b #12：假对端沉默时不再空转（只加计数时，300 毫秒里走了 212,345 次）——并入本轮 | 控制者代项目所有者裁定：A、B 与简报只覆盖协议栈的锁的缺陷是同一个，这是收尾里程碑，不留给"以后"；代价：一个 waker 槽和两处取锁。留下的限制：隧道死掉到当前配置的下一个连接之间，旧配置里与之冲突（同一私钥与 peer、节或载体设置不同）的残留出站可能自己起一条隧道，下一个连接时被接替 |
| 28 | 7 | 第一部分（`cde2ce2`）：构建之后的清理是一个闭包（`dangling`），与行的次序、链的长度无关；派生条目 `M (via X)` 在真实配置里写不出来（组级 `underlying-proxy` 只能指向主配置的策略或组，主配置的策略构建失败是整代失败），用例手工造装配结果，规则照做；一个成员全都挂在被略去的中继上的组成了空组——`--empty-group-reject` 没开时回落 DIRECT 并告警，作中继时一律拒绝——，此前经它的每次拨号都失败；告警每个因此少了成员的组一条（字段 `group`、`members`），修正轮 `2e4cb63` 加 `count` 并至多列 20 个名字（一个订阅至多 10,000 条，全都经同一个失败的中继时是一行很长的日志，每次重建一次）；注册表用例里手写了约 50 行的 `tracing` 订阅者，没加依赖边 | 实现者提出，评审同意 |
| 29 | 7 | 第二部分（`010a9ca`）：ASCII 的 `server` 不经 `to_ascii`（设计写的是"以 Unicode 写时"，简报写"是域名时"）。于是字母表之外的名字有三种结局：没有 ASCII 形式的 Unicode 名是加载时的 `E0018`；ASCII 的字母表之外的名字（`a@b.example`、带空格）在需要它做 TLS 名字或 WebSocket `Host` 时是构建错误（`E0022`，`is not a valid TLS server name`），否则能加载、到第一次拨号才失败（解析器编不出它；经 `underlying-proxy` 时下一跳以 `<协议>: the host name cannot be sent to the server` 拒绝），拨号时的这两句是读代码得出的、没有跑；修正轮另让全是点的 Unicode 名（`。`、`．。`）是 `E0018`（ASCII 的 `.` 照旧） | 控制者的裁定（评审同意）：全转会让今天能加载的行变成加载错误，是一个没有人决定的兼容性变化；代价：一个 `if` |
| 30 | 7 | 实现者核对"spec 里服务器名的每一处用法"时多查出、简报文件清单之外的：`Config::proxy_hostnames()`（"`[Host]` 不适用于代理服务器名"的名单）原来取写下的名字，而出站问解析器的是 A-label，现在两种写法都在（订阅导入的策略的服务器名从来不在这张名单里，是既有的缺口，没动）；`rurge-proto` 不再直接用 `idna`，依赖边从它搬到 `rurge-config`（`Cargo.lock` 两行变动，`[[package]]` 前后都是 517 个）；简报的判据 `cargo tree -p rurge-config -e normal \| grep idna` 在改动之前就有一条（经 `url` 间接），有意义的检查是 `--depth 1`；没有转换的名字：`[WireGuard]` peer 的 `endpoint`、`sni`、`shadow-tls-sni`、`server-cert-verify-name`、`obfs-host`、`ws-headers` 里的 host，自定义 `sni` 以 Unicode 写时仍是干构建错误 | |
| 31 | 7 | `ssh` 的检查（`channel_host`）放在 `dial` 最前面——不建会话、不登录、不开通道；`wireguard` 的错误是 `OutboundError::Dns`，`Display` 在前面加 `dns: `，请求记录里是简报的那句；修正轮让隧道里的 DNS 服务器对问不了的名字跳过、之后的 `system` 条目仍被问，只有这样一次跳过之后搜索没有应答才报 `wireguard: the host name cannot be looked up`，只剩 `system` 时文本仍是 `dns lookup of … failed` | |
| 32 | 8 | 与计划和任务书的出入：(1) 配置读不了时 `rurge run` 的退出码是 2，不是正文写的 1（`main` 把 `Err` 映射成 2；设计 5.10 与 `docs/api/phase1.md` 写的也是 2），保持 2；(2) `RuntimeArgs::data_dir()` 在文件清单之外新加（`--data-dir` / 环境变量 / 平台缺省，不创建），`resolve` 改用它；(3) `SystemProxyManager::recover()` 的返回值从 `()` 改成 `Option<Result<(), String>>`；(4) 看 `state.json` 里有没有备份是只读的：用 `StateStore::open` 的话，解析不了的 `state.json` 会在这条没有日志的路径上被悄悄改名成 `.broken`；(5) 比计划多两条用例（第二句文本、解析不了的 `state.json`），共九条，全部经文件后端；(6) 门禁留出了两条会去真实注册表的既有用例（`real_registry_round_trips_under_a_scratch_key`、`real_internet_settings_can_be_snapshotted`），因为派工单强调不读不改注册表；每个 `rurge` 进程都是 `cli` 测试二进制的子进程，通往真实后端的路只有 `sysproxy::backend()` 在 `RURGE_SYSTEM_PROXY_BACKEND` 没设时的一处，实现者每次调 cargo 前都在父环境里设了一个 rurge 拒绝的取值 | 实现者提出 |
| 33 | 8 | 评审意见与修正轮 `9bab75e`（三条）：(1) 锁不了的数据目录（`lock_data_dir` 给 `Ok(None)`）不做提前恢复、不出声——第一版照正常启动那样"不带锁继续"，但这里进程本来就要退出，不存在"别为一把锁丢掉守护进程"，在活着的实例底下恢复是更糟的错；(2) 恢复成功而 `state.json` 里的备份没能清掉时，说第四句（`… the saved settings were restored, but state.json could not be updated: the next start restores them again`）而不是第一句，否则下一次启动会悄悄盖掉用户这期间的设置；(3) 配置能加载但 `--log-file` 打不开时同样先恢复（函数改名 `recover_without_a_log`）——同一个洞，更可能与崩溃同时出现（磁盘满、挂载点没了） | 控制者代项目所有者裁定（三条）：(1) 代价是文件系统没有锁时，坏配置会让死掉的代理设置留到配置改好为止；(2) 第二句文本多一句要写进文档；(3) 一次调用和两条用例 |
| 34 | 8 | 留给文档与以后的：提前恢复期间这个进程持着 `rurge.lock`，同一数据目录上的正常启动此刻以 `another rurge instance is already running` 退出 1；标准错误上的次序因出口而异（配置有错误时诊断在前、告警在后，配置读不了时告警在前、`error: cannot read …` 在后）；仍不恢复的出口（数据目录建不出来、锁被别的实例持着、后端取不到、运行时建不出来）各有理由，都不是漏洞；`state.json` 存在但读不了时按"没有备份"处理、不出声（正常启动会记一条 WARN）；真实的系统代理后端从没在这条路径上跑过，阶段 1 手工验收清单要加一项 | |
| 35 | 8 | 备份里的凭据：后端的报错文本可能带凭据（失败的命令整条引用；KDE 的 `httpProxy` 取值可以是 `user:pass@…`），第二句会把它带进标准错误——从源头 `SystemRunner::run` 遮掉，作为任务 13 的第 10 项 | 控制者的裁定：该缺陷自阶段 1 起就有，但项目的规矩是凭据不进日志与错误 |
| 36 | 4 | 规则 (b) 在评审后改为 (b′)（修正轮 `dd3aabc`）。设计 5.5 与计划写的 (b) 是"看有没有**另一个**成员的失败"；实现者量到：同时发起的 6 条会话打到同一个失效站点，都选中排在最前的成员，那一刻站点记忆里只有它自己，于是同一个成员被罚 6 次、判为失败（三个成员的组：旧规则按规则推算 18 次，(b) 下 6 次，(b′) 下端到端用例量到 1 次）——设计承诺的"每小时至多让一个成员受一次罚"在并发下不成立。(b′)：记下一次失败的那一刻，站点的记忆里没有任何未过期的失败记录——不论是别的成员的，还是回报者自己上一次在这里的——才另算成员自己的失败；"看"在回报者自己的条目被替换之前，与"记"是同一次调用、同一次加锁。结果：一个失效的地址只要还有人在一小时之内接着访问，总共只罚一次（每次失败，被豁免的也是，都刷新站点记忆，一小时从最后一次失败算）；别的成员其后在那里的成功不结束豁免；回报者自己上一次是成功，下一次失败照罚；N 个不同的失效地址仍罚 N 次，摊在成员身上（没有采用 (a) 的代价），连续 9 个不同的失效地址、中间没有成功的会话，三个成员仍全部判为失败；只有一个可用成员的组里，同一站点上反复失败的成员不再被会话判为失败（要三个站点或测速）；被豁免的失败不碰成员的记录，包括 `reported` 标记；站点记忆按策略名记，站点数超过 4096、一个站点超过 16 个策略或重载去掉了策略时，同一站点一小时内可能再罚一次 | 控制者代项目所有者裁定（两条）：(b′) 是让"一个失效的站点只罚一次"在并发下成立的最小改动，而且只会比 (b) 罚得更少；其后的成功不结束豁免——被豁免的失败仍会降低该站点上成员的排序，错罚会移动全局排序，时好时坏的站点也是站点的问题，按先后判断会把对并发的敏感带回来。代价：一个在单一站点上反复失败的成员只能由别的站点或测速发现 |
| 37 | 4 | (c) 的实现比简报多：目标发不出去（域名经 `to_ascii` 转不了，或转出来超过 255 字节，结尾的点也算）时，引擎把这条会话**整个当作不经这个组**——失败不回报、不换成员，首字节的样本与使用计数也不记，TCP 与 UDP 相同，请求记录的 `policy` 仍是 `[组, 成员]`、没有 `smart group … tried …` 的备注；内部 DNS 会话只有地址，不受影响；垃圾名字因此也不会挤掉站点记忆（4096 个主机的 LRU）里的真实站点。判断落在引擎的拨号路径上，不给 `OutboundError` 加变体（计划期决定 P6） | 判据落在目标上，换哪个成员都一样；若评审认为连上的会话应照常回报成功，改法是在失败分支里加条件 |
| 38 | 4 | 用例：第一轮没有既有用例需要改写；修正轮改写了第一轮自己的两条（`a_member_failing_alone_at_a_site_pays_every_time` 反过来成了 `a_member_failing_at_one_site_time_and_again_pays_once`，`failures_without_a_host_are_the_members_own` 多了一步）；实现者量到的并发写成端到端用例 `a_burst_of_sessions_to_a_site_that_is_down_costs_one_penalty`（旧规则下 `[Failed, Unknown{0}, Unknown{0}]`，(b′) 下 `[Unknown{1}, Unknown{0}, Unknown{0}]`），端到端用例用解析不了的 `test-url` 把测速挡在外面；"看"与"记"拆成两次加锁的变异让两条并发用例红 | |
| 39 | 9 | 基准与派工字面的出入：一个线程跑整条链（应用、转发、源站），先上行再下行在同一条会话上，两遍各 8 轮，`TCP_NODELAY`，`RELAY_BENCH_MIB` 可缩小；多线程运行时的四次尝试在这台机器上量不出加密栈上的任何差别，所以改成一个线程，数字因此是整条链路在一个核上的吞吐。第二遍是因为第一遍里"过线"的余量只有 0.1 个百分点（读端空闲时 flush、32 KiB 靠 ws 上的 110.1%），而同一组合各轮的极差是中位数的 12% ～ 57%；办法先定下再跑第二遍，按两遍 16 轮的中位数套同一条判据 | 实现者（M2b 的教训：顺序测量把漂移看成差别） |
| 40 | 9 | 结果（设计 5.11 的判据；整张表在 M8 设计 5.11 末尾的结果段）：(1) flush 的时机测不出差别——"读端没有数据时才 flush"÷"每次写后 flush"的 16 个比值在 96.5% ～ 102.0%，平均 100.3%，读代码也是这样（三个加密流在没有积压时 `poll_flush` 是空操作），每次写后 flush 保留；(2) 缓冲越大越快：两遍 16 轮合计，裸 TCP 相对 8 KiB 为 128.0% / 146.1% / 152.8%（16 / 32 / 64 KiB），TLS 110.2% / 112.3% / 114.8%，WebSocket 108.1% / 111.2% / 110.3%，VMess 104.0% / 108.9% / 116.1%；加密栈的增益全在上行方向，VMess 在 16 KiB 上几乎不动；(3) 判据按两遍 16 轮合计读，指向 64 KiB——只按合计这样读：第 1 遍单看指向"读端空闲时 flush、32 KiB"，第 2 遍单看只有 64 KiB 过线，每次过线都只高出 0.1 ～ 0.3 个百分点；**缺省没有改**，仍是每次写后 flush、8 KiB。数字是这台机器的（Windows 回环，基准进程只用了约 0.3 个核），说的更多是每字节的读写次数，不是计算量；纯 TCP 的 110 MiB/s（0.92 Gbps）是整条链在一个线程上的吞吐，不是 NFR-01 的测量 | 控制者代项目所有者裁定：PRD 的 NFR-01（1 万并发连接常驻内存 < 300 MB）在里程碑设计之上，每条会话从建立到结束一直拿着两块缓冲，8 KiB 是 156 MB，16 KiB 是 312 MB，64 KiB 是 1.22 GiB；余量薄、增益只在上行。自适应缓冲（从 8 KiB 起、读满时才换大）是新设计，要有自己的内存与吞吐实测，不在收尾里程碑里做，移交阶段 3 建立性能基准时一并定。代价：加密栈上 10% ～ 16%、裸 TCP 上约一半的吞吐留在桌上 |
| 41 | 9 | 修正轮（`d309f33`、`771b94f`，一个新的 sonnet 实现者）：(A) 评审认为 `Flush::ReaderIdle` 的问题已有答案、答案不依赖机器，却让最热的函数多一个标签块、三个 `select!` 和一个生产永不走的分支，裁定去掉——`Flush`、`Tuning`、`TUNING`、`pump_tuned` 全部去掉，回到 `BUF` 与私有的 `pump_sized`，循环体与 `dd3aabc` 逐字节相同（只多缓冲大小的参数），基准只留缓冲一维（四个栈 × 四种缓冲 × 8 轮），两种时机的对比可从 `6752afa` 重现；修正轮之后的矩阵只跑过冒烟（`RELAY_BENCH_MIB=4`，128 次运行与汇总表），没有跑满；(B) `http` / `socks5` 的 `handshake_io` 只把不带话的 reset / aborted / broken pipe 读作"代理关了连接"，下层自己带话的（`underlying-proxy` 链上下一跳的原话，如 `ss: the server reset the connection without answering (wrong password or method?)`）原样通过，EOF 仍一律算代理的（已知限制：下层的拒绝若以 EOF 表现，仍被说成上层代理关了连接）；(C) `http` 的 `response header larger than N bytes` 只在 `read_head` 自己判定超长时说（私有的 `TooLong` 标记、`transport::head::is_too_long`），流自己的 `InvalidData` 原样通过——`https` 代理要客户端证书而策略没有 `client-cert` 时是 `received fatal alert: CertificateRequired`（TLS 1.2 上是别的 alert 文本） | 控制者的裁定（三条）：(A) 去掉，代价是需要时从 `6752afa` 取回约 45 行；(B) 任务 9 原样会让 `http` 把下一跳的拒绝（`socks5` 自 M1 起就这样）误认为上层代理的，ss / snell 在 M8b 写的句子恰好在链上用得着，代价是少数带话的 reset 显示自己的话而不是固定的一句；(C) 评审在任务外查出的、自 M1a 起的错误诊断，小改动就在同一函数里，代价是一个标记类型 |
| 42 | 9 | 任务 9 原版里 `http` / `socks5` 出站的握手每一步写完先 flush 再等应答（下层是会缓冲的一跳时，握手的请求不该等到下一次写才发出），`http` 把代理在 CONNECT 期间关了或重置连接统一说成一句 `http proxy closed the connection during CONNECT`（此前是系统自己的文本，如 `os error 10054`）。评审发现一处缺口，移到任务 11 补：把 relay 里的 flush 挪出与 `stop` 竞速的那个 future 的变异活过了全部用例（从 `dd3aabc` 起就如此，不是任务 9 引入的），任务 11 加了 `cancelling_stop_interrupts_a_flush_that_never_completes` | |
| 43 | 10 | IPv4 映射地址：简报只管 SOCKS5 的目标，评审发现同类问题还在三处，修正轮（`0d7dac0`）全收：HTTP 入站的 `CONNECT [::ffff:a.b.c.d]:port` 与绝对 URI 的主机（`CONNECT [::ffff:192.168.1.1]:80` 命不中任何 IPv4 的 `IP-CIDR` 规则，而 DIRECT 的 TCP socket 在 Linux / macOS 上会连到那个主机——规则可被写法绕过）；出站报回的数据报来源（`receive()` 先归一再认流；tuic-server 0.8.5 在双栈 socket 上就这样报）；接受循环里 TCP 客户端自己的地址（双栈监听上 IPv4 客户端报来的是 `::ffff:a.b.c.d`，`SRC-IP` 的 IPv4 规则永远命不中，阶段 1 起如此）。唯一的实现是 `rurge-config` 的 `HostName::canonical`（原先在 `rurge-inbound` 里的 `canonical_host` 搬来，动了基础 crate，整个工作区重编一次）；记录里 `dst` 对映射目标显示 IPv4。接受循环那个调用点只靠读代码核对，监听层面没有用例（Windows 上产品不设那个 socket 选项）。改动之前，经 DIRECT 向 `[::ffff:127.0.0.1]` 发 UDP 在本机根本发不出去（os error 10049） | 控制者代项目所有者裁定（三条）：规则不该被写法绕过，两个入站不该互相不一致；回包认流错了，`smart` 的 3 秒规则可能因此错罚健康的成员；代价是几处调用点和一条用例 |
| 44 | 10 | "已经报错的参数不再多一条告警"（第 3 项）的做法与范围：机制是 `ParamReader` 记下哪些键的取值已经报过错（`error_on` / `rejected` / `has_unrejected`——修正轮把"记键"与"报错"合在 `error_on` 一个方法里，记过的键旁边一定有一条 `Error`，并去掉公开的 `reject`），不给各协议的读取函数加参数；简报列的 6 类之外，对 20 种策略写法 × 约 50 个参数的审计又找到 7 类（external 与 wireguard 整张"不适用"名单的取值写错、`underlying-proxy` 指向 `REJECT` 之后的 `interface` 等、`h3` 写错 + `alpn` / `tfo` / `headers` 禁用字段、两个 `addresses` 里一个写错……），修正轮再加：出错的链对 `port-hopping` 什么也不说（`hysteria2` + 合法的 `port-hopping` + `underlying-proxy=Missing` 或 `=REJECT` 此前各多一条"cannot be combined"）。审计脚本是临时的，没有保留；语料库快照没有变化。有意没动的：`port-hopping` × `underlying-proxy` 的 `E0018`（两个参数的真实冲突，修好其中一个后依然成立）、`port-hopping` 写错时 `port-hopping-interval` 过短的 `W0012`、几处"错误的一方解析成缺省值"的旧写法。`h3` 写错时 `headers` 里的禁用字段不再提（那句话得点名一个没有人知道的传输），按现状接受 | |
| 45 | 10 | 一条合法行说的话变了：`wireguard` 上合法的 `tfo=true` 此前得到 `W0028`（不适用）加 `W0029`（没有效果），现在只有 `W0028`，与 `external` 一致（`ecn` 仍留 `W0029`）。`CORE_VERSION` 改为 22（`f58678b`；设计 6.3 已执行），用例 `check_reports_core_version_22` | 控制者代项目所有者裁定："一个参数一句话"；代价：表里一行 |
| 46 | 10 | 披露：为核对 SOCKS5 RED 的原因，实现者用 `rustc` 编了一个只依赖 std 的 5 行程序并运行过一次（IPv6 UDP socket 向 `[::ffff:127.0.0.1]:9` 发一个包，得到 `os error 10049`），违反"不运行不是由本仓库构建的二进制"的字面；实质无害（源码全是它自己的，没有东西离开本机）。约束文件随后写明：探测写成仓库里的测试、用 `cargo test` 跑，不编独立的程序 | 控制者记下，留给项目所有者知道 |
| 47 | 11 | 与计划的出入（第 1 ～ 3 项）：`rurge-proto-wireguard/src/outbound.rs` 里"< 2 秒"的墙钟断言是五处，不是两处（文件在 M7c 之后又长了用例），都放宽到 5 秒——两处"没有等待"的用例里 `set_dns_wait(5 秒)`，断言与等待值之间没有余量，真有等待时耗时恰好 ≥ 5 秒仍分得出来；`hysteria2` 里没加时限的 `send_to` 是十二处；一次拉起多于一个端口的互操作夹具（`SingBox` 的四种形式、`Xray`、`Dnsproxy`、`CoreDns`）改用 `free_ports`，只取一个端口的（snell-server、sshd、TrustTunnel、tuic-server、masque-server）不用。夹具渲染出的配置没有变，夹具的单元用例 40 条不变地通过；这些夹具改动没有对真实的参照服务端跑过 | |
| 48 | 11 | 第 3 项的 `Failures`：计划与简报写"加 `#[must_use]`"，实现者核对后发现它拦不住真正的写法（绑了变量、记了失败、不调 `finish`：rustc 没有 lint 管），只拦"建了就丢"。修正轮（`284c40a`）加 `Drop` 守卫——记过失败、没 `finish` 就被丢掉、线程又不在 panic 时，当场让用例失败并列出那些失败；`finish` 在自己 panic 之前先清空失败（它报出去的不再归守卫管）。守卫立刻找出一条一直静默通过的用例（`tests/interop/tests/mihomo.rs` 的 `only_the_two_sentences_of_an_unanswered_request_are_taken` 记了两条失败，只调 `report`），连同 `shadowsocks.rs` 的那条改成断言 `finish` 的 panic 文本；新加三条不需要参照服务端的用例，其中一条重新拉起自己的测试二进制作子进程（环境变量 `RURGE_INTEROP_UNWINDING_CHILD`），所以 `tests/interop` 的 `tokio` 开了 `process` 特性。M8b 延后事项 #6 的 `Failures` 一半到此才做完 | 控制者代项目所有者裁定：`#[must_use]` 看不见绑定的变量，十五处用法全是这样写的；互操作用例是对真实服务端的唯一证据，而且从没跑过；代价：十来行和一条用例 |
| 49 | 11 | 第 4 项的偶发用例由计划的两条变成七条，每条先写时序再改，单条连跑 20 次都通过：(a) `bootstrap::tests::set_upstreams_takes_effect_for_the_next_resolve`（答案晚于 100 毫秒就多发一轮，被数进去；复现：假服务器各延迟 250 毫秒；改用不重发的 `once()`，保留延迟让退回 `fast()` 时稳定地红；同模块另三条只数问题的用例同样换）；(b) `rebind_failure_leaves_no_listeners_and_a_later_rebind_recovers`：**原因没能证明**——放掉再抢同一个号一万次（空闲与三十个满载线程各五千次）一次也没失败；改成占着的端口一直占到用例结束，恢复靠一个新的一代（`http-listen = 127.0.0.1:0`，由系统在绑的那一刻选），不再依赖系统把同一个号还回来；若 `os error 10048` 再出现，最可能是临时端口耗尽（Windows 上 `connect` 在那时也报 10048），是机器的状态，不是用例能修的；(c) `a_closed_port_does_not_break_the_carrier`（放掉的 UDP 端口被并行用例的 `bind(0)` 拿走；让端口一直被占着不行，Windows 上被占用的端口不产生 ICMP）；(d) `rurge-external-tests` 的 `a_program_that_exits_ends_its_whole_tree`、`a_program_that_exited_is_started_again`（放掉的端口被并行用例的回显服务器拿走，`socks5: the proxy selected authentication method 1, which was not offered`，即 `05 01 00` 被回显）以及这个 crate 里所有把放掉的端口交给辅助程序的用例；(e) `socks5` 的 `the_bound_address_may_be_a_domain_or_ipv6_and_unknown_types_are_refused` 给握手 200 毫秒的预算，改用缺省的 10 秒；(f) CLI 的 `watch_reloads_rules_on_change`：配置若写在监视器装好之前就永远不会被看到（测试读到第二行 `listening on` 后 0.59 ～ 1.09 毫秒 `running` 才到），改为先等 `running`，等重载生效的上限从 8 秒放到 30 秒 | 任务 8 的门禁、任务 9 与 10 的评审陆续见到的偶发；(b) 的原因没证明，所以改的是它不再依赖的那一步，不是"找到了原因"。控制者裁定：计划要求的单条用例连跑 20 遍（先是两条，后来七条，各 20 遍）远低于曾把临时端口耗尽的量级（数千次），允许；代价是偶发的端口错误要重跑 |
| 50 | 11 | 测试约定（新的，也写进 `CLAUDE.md`）：要先于主人绑定就写进配置的端口，或必须一直关着的端口，取自 20000–29999、现场试绑一次（`crates/rurge-engine/tests/common/mod.rs` 的 `closed_udp_port()`、`tests/external` 的 `free_port()`），不用 `bind(0)` 再放掉；数"服务器被问了几次"的用例用不重发的 `FanoutOpts`；对守护进程做事之前先等它说准备好了；"断言什么都没发生"的等待是仅有允许的固定等待，注释要写它等的是什么、余量多大；不要用"绑了不听的 TCP socket"当关着的端口——Linux 与 Windows 拒绝连接，macOS 默默丢掉 SYN，连接会挂住（XNU `tcp_input` 里 `TCPS_CLOSED` 的 `goto drop`） | 控制者代项目所有者裁定（20000–29999 的范围）：放掉的号回到系统池，并行用例的 `bind(0)` 可能立刻拿到（两种情形都复现过）；这个范围在 Linux（32768 起）、macOS 与 Windows（49152 起）以及本机（1024–15000）的动态端口之下；代价：动态区间覆盖它的机器上保证退回"试绑时空闲"。约束文件里"只用端口 0"因此多了唯一的一个例外 |
| 51 | 11 | 其余两项：relay 的 `cancelling_stop_interrupts_a_flush_that_never_completes`（RED 是把 `writer.flush()` 挪出与 `stop` 竞速的 future 的变异：只有这一条红，其余 16 条绿）；假 HTTPS 代理握手失败后留着连接等客户端先走（`TlsFixture::accept_patiently`，至多 5 秒），免得 TLS 警报被复位盖住——旧的假服务器在请求晚于服务端的失败到来时，Windows 上读到的是 `os error 10053` 而不是警报（`the_fake_https_proxy_lets_a_refused_client_read_the_alert`），这也说明任务 9 里"本机 200 / 200 都是警报"只是时序碰巧。修正轮（`284c40a`）另：过期缓存用例改成按事件等（`stale_entries_are_served_and_refreshed_once`：让上游晚一秒回另一个值，过期命中必须交出旧值；去掉 50 毫秒的墙钟断言和固定 200 毫秒的 sleep），评审的几处小点（三处等待加时限、三处注释） | 控制者裁定（对过期缓存用例）：它的 50 毫秒墙钟断言在回环上什么也证明不了（一次往返不到那么久），在忙的机器上还会误红，固定的 200 毫秒 sleep 与 `fast()` 的 100 毫秒重发是 bootstrap 另一条用例已经关掉的三个口子；代价：一条用例 |
| 52 | 14 | 机器事实与实验：开发机开着 Windows 的 Simple TCP/IP Services，回环的 TCP 与 UDP 7（echo）、9（discard）、13、17、19 都开着；关着的回环端口在 Windows 上约 2 秒后才拒绝连接（实测 2.03 ～ 2.05 秒），Linux / macOS 立刻；本机 TCP 的动态端口是 1024–15000（顺序发放），UDP 是 49152–65535。写到回环 9 号端口的 66 行（28 个文件；分发时数的是十五个文件）全部换成关着的 1 号端口后跑整套用例：**只有一条**用例变红——`udp_silence_counts_only_where_answers_always_come`（`crates/rurge-engine/tests/smart.rs`；成员的连通性测试在这里 5 秒后才失败，那里 2 秒后失败，断言处读到 2 次失败），没有别的用例走了不同的路径（没有引擎用例的耗时差超过 0.2 秒；评审用原始日志重算：244 条，最大 0.192 秒，对照文件用的阈值是 0.5 秒）。改后整套在两个世界里都通过：正常 70 / 2449 / 0 / 3，换成关着的端口 56 / 2449 / 0 / 3（56 是因为这一遍不含 14 个空的文档测试二进制）；"立刻拒绝"那种情形只能用"接了立刻关"顶替（只覆盖引擎的集成测试），真的要等 CI | |
| 53 | 14 | 改了什么：那条用例给成员 `Quiet` 加 `{UNTESTED}`（测试 URL 解析不了，不参加轮次，只通过会话被了解）；常量 `NO_TEST`（`http://127.0.0.1:9/`）留着，并在注释里写明对那个端口上有什么**不作任何假设**（立刻拒绝、两秒后拒绝、接了不答都成立；经自动组拨号会在后台触发一轮这样的测试，拨号之后断言的东西必须不管这一轮何时结束都成立，数成员失败次数的用例要让成员不参加轮次）；因此新写的用例只在本机"接了不答"的世界里验证过，约定加一条：新写的、经自动组拨号的用例，把常量指到关着的端口再跑一遍。`closed_port()`（`auto_groups.rs`、`smart.rs`）的 TCP 版并进 `tests/common` 的 `closed_tcp_port()`，与 `closed_udp_port()` 共用一趟遍历 `unassigned_port`（分发信数的 5 处，加 `outbounds.rs` 的 `a_broken_hop_is_named_in_the_error` 一处）；`api.rs` 里一条注释。没有发现产品缺陷 | 实验的结果在改任何用例之前写下（`NO_TEST` 的用例都与那个端口答什么无关，所以不为整齐改十五个文件） |
| 54 | 14 | 任务 14 顺带发现、由任务 11 的修正轮（`284c40a`）做掉的两件：`udp_silence_counts_only_where_answers_always_come` 的后一半自 `dd3aabc`（任务 4 的修正轮：成员在一个站点的第二次失败被豁免）起已经分辨不出它要防的错——两条流都发往 `127.0.0.1`，是同一个"站点"，把 `silence_tells` 改成恒为真照样通过——，改成时间上的两步（只发不该算的那条流，等那个套接字收到数据报，固定等 3.5 秒看站点记忆仍为空、成员 `Unknown { failures: 0 }`，再发 443 那条；两个变异都红在第一步），用例长了约 3.5 秒；`pipeline.rs` 的 `a_failed_dial_through_a_direct_substituted_empty_group_keeps_both_reasons` 里还有一处放掉的端口，改用 `closed_tcp_port()`。这是一条被同一里程碑里较早的任务悄悄解除了武装的用例，只因为较晚的任务为别的目的变异了产品才被发现；任务 14 的评审与任务 11 修正轮的复评合并成一次（两个都是很小的只含测试的 diff，修正轮修的正是任务 14 点出的那条用例），两者都通过 | 控制者代项目所有者裁定：用时间上的两步而不是第二个站点——回环地址有哪些随 CI 平台而异；代价：用例慢 3.5 秒，3.5 秒的固定等待只有 0.5 秒余量（计时器晚了半秒以上时，一个错误的产品可能蒙混过去，用例不会因此误红） |
| 55 | 15 | 在产品代码里找到了一个循环并修好（`f28d67c`）：`rurge_dns::fanout::resolve_name` 的循环，在"还有轮次没发、而时钟已过了这次查询的截止时间（产品里 resend 1 秒 × 5 次 = 5 秒）、又没有有效应答"时没有出口——不再发送、轮次不再增加，接着等的"下一次重发"时刻已经过去，`sleep_until` 立即返回，于是不停地转（tokio 的协作预算让每次 poll 至多转 128 圈再让出，所以运行时照常响应，只是一个核烧着，约每秒 48 万圈）；这次查询永不返回，之后对同一个名字、没命中缓存的查询挂在它的 in-flight 条目上；只有重启 rurge 才停。触发条件：跑这次查询的任务在某次重发之后直到截止时间一直没得到运行——进程被停住再继续，机器睡眠而平台的单调时钟把睡眠算进去（Windows 的 `QueryPerformanceCounter`；Linux 与 macOS 不计睡眠），运行时被饿住。自 `667af52`（2026-09-05，阶段 1 M2b，"发送门槛检查截止时间"）起就在代码里。修复是一个条件（醒来时已过截止时间就结束，收尾与到点结束相同）；用例 `a_fanout_kept_from_running_until_past_its_deadline_ends_as_a_timeout` 先暂停时钟、拨过截止时间再恢复，用 10 秒的真实时钟 `timeout`，所以 RED 是失败不是挂起（拨钟后 2 秒里被 poll 了 7480 次，其中 91% 的时间在 poll 里面）。**把握**：与 M3a #20 记下的空转相符（用卡住那条用例原样的部件、占住运行时线程 350 毫秒就稳定复现：查询 3 秒内不结束），但那次运行没有留下日志，不能证明就是它；写成"找到并修复了一个与观察相符的空转，没有证据表明还有第二个"，不写成"原因" | 任务书的问题是"解析器或 mock 有没有不等待的循环"，七类候选逐个对过，其余都不会自己转。顺带的机器事实：Windows 记在进程名下的 CPU 时间在这台机器上不可信（一个计算 8 秒的线程只被记约 0.1 秒），所以证据取自进程内部（圈数、poll 内的时间占比），不拿任务管理器判断"是不是在空转" |
| 56 | 15 | 三条计时类用例（`f0412ca`）：`resolver::tests` 的 `a_partial_result_completes_aaaa_in_the_background`、`aaaa_suppression_after_five_timeouts_and_flush_resumes`、`cache_fresh_stale_negative_and_coalescing` 原来在真实时钟上经回环 UDP 问 `MockDns`，应答晚到 100 毫秒就多发一轮（计数变），晚到 300 毫秒就超时；改在暂停的时钟上问内存里的上游（新的 `rurge_dns::testing::MockDns::upstream()`：在内存里问 mock 自己的表，用例体不变）。这三条与曾卡住的那条共用的只有进程和墙上的时间：曾同一轮失败的 `cache_fresh…` 会被同一种停顿弄红 | |
| 57 | 15 | 修正轮（`dd22e04`，只动测试与测试支撑）：评审指出同一个 crate 里还有 12 条同类用例，裁定一并做。`fanout::tests` 里调用 `resolve_name` 的 5 条搬到暂停的时钟和内存上游，断言精确的虚拟时刻（0、100 毫秒、400 毫秒）；多线程的那条（有意如此，`667af52` 为显示真实线程下错误的种类而加）只留错误的种类，数问题和到点交给暂停时钟上的孪生 `all_dropped_sends_every_round_and_times_out_at_the_deadline`；`resolver::tests` 里经 `[Host]` / 系统 / TCP / 引导 / 按网卡的服务器的 6 条（清单上是 5 条，漏了 `with_encrypted_dns_lookups_via_an_interface_go_as_usual`）留在真实 socket 上，换成不重发的选项。18 条用例改动、2 条新增，各单独连跑 20 次通过，六个对 fanout 的变异全部被拦下。限制写在测试支撑里：`MockDns::upstream()` 不带截断与 TCP 重试；`resolver_in_memory` 只换 `primary`，仍走 UDP 的路径在暂停的时钟上不再按解析器自己的时序（实测十二个名字里十一个按时应答、一个多问了一遍），不要在那里碰真实 socket；fanout 的重发 / 部分结果 / 空答规则在 100 毫秒这个量级上不再有真实 socket 上的用例（`tests/resolver.rs` 默认 1 秒的两条覆盖重发与部分结果），解析器的"过期命中 → 后台刷新"与"部分结果之后的补查"也只在内存里 | 控制者代项目所有者裁定：CI 从没绿过、每次运行都要项目所有者推送，100 毫秒的停顿在忙的机器上很平常，手段（暂停的时钟、内存上游、不重发的选项）现在有了；`resolver_in_memory` 的说明按实测写，不照简报写的"立刻超时" |
| 58 | 15 | AAAA 抑制的计数没有改，作为待定行为记下（修正轮把阈值钉住）：原来的 `aaaa_suppression_after_five_timeouts_and_flush_resumes` 一口气问五个名字再看，对 1 ～ 8 里任何阈值都过（实现者用基线版本的文件跑了 4 与 6，都过）；现在每个名字问完、等它的后台补查结束才问下一个，每一步都断言是否已抑制，对 1 ～ 8 里除 5 以外的阈值、以及"后台补查的超时不计数"都红。现行计数：一次 A 有应答而 AAAA 超时的查询计一次，它的后台补查再超时又计一次；一个接一个地问，计数是 1、2（第一个名字），3、4（第二个），5（第三个）——**第三个名字**的查询期间 AAAA 就停了；背靠背地问（任务 15 第一次观察）是 1、2、4、6、7，第四个名字期间停。M2 设计写的是"连续 5 次『A 有应答而 AAAA 超时』"，现行计数逐字满足每次 fanout；后台补查该不该算进去是设计问题。兼容性清单里"连续 5 次 AAAA 超时后停止发送 AAAA"一行保持状态并加备注 | 控制者代项目所有者裁定：本里程碑不改计数；代价：AAAA 被黑洞时，抑制在第三或第四个名字而不是第五个时生效。由项目所有者决定：定了之后改计数，同时改测试里的 `i >= 2` |
| 59 | 15 | 评审顺带扫了整个工作区里同一类循环（对着可能已过去的时刻睡、没有进展；机器睡过之后 `interval` 的补发）：`rurge-dns` 之外的 9 处 `sleep_until` 每一圈都有进展，5 处 `interval` 里 4 处已经用 `MissedTickBehavior::Delay`；唯一的发现是 `rurge-engine/src/engine.rs` 里每秒一次的流量采样（`interval` 取缺省的 `Burst`）：机器睡过 N 秒后（Windows）约有 N 个 tick 背靠背触发（一小时约 3600 个），每个取一次请求记录的活动锁并对活着的会话求和，没有日志、没有拨号、没有测试轮——严重程度低、把握高。裁定给它加 `MissedTickBehavior::Delay`，放在最终修正波里（一行；有可观察量时才加用例，不为此加产品代码）。没有扫的：用 `sleep(时长)` 写的循环（看了四处，没问题） | 控制者的裁定：这一类问题在测试里永远不显，笔记本每天遇到；代价：一行 |
| 60 | 13 | 第 1、2、3 项（`5cd3fc8`）：溢出点是**两处**——`round_timeout` 的乘法（`evaluate-before-use` 的组多于 8 个可测成员时第一次拨号 panic）之外，`wireguard` 策略的 `test_spec` 里 `+ 10 秒` 的加法在**加载配置时**就 panic；其余用到 `test-timeout` 的地方都交给 `tokio::time::timeout`（它用 `checked_add`），本来就不 panic。代际锁改用 `PoisonError::into_inner`（锁只护着 `()`）。`override_of` 把要说的话带出 `state` 锁再写告警——这一项不改行为，没有 RED，用例改动前后都过，咬得住由变异证明（去掉 `o.warned = true` 与去掉 `tracing::warn!` 各被杀） | |
| 61 | 13 | 第 4 项 `with_params` 偏离任务书的"追加在后"：行尾一项留着未闭合的引号或括号时，写在它后面的任何文字都会被解析器算进它，要让追加项仍在它之后被读到只能把用户那一项改写成闭合的等价写法，一般情形下做不到；所以追加的修饰写在它**前面**，那一项的原文一个字不动。读出来的位置参数与每个键的取值与不变量完全一致，只有"追加的参数"与"那一项"在 `ParamMap` 里的先后与"追加在后"不同，而它们的键不同。连带：此前靠安全检查跳过的两种订阅行（带引号的同名项；修饰设了 `underlying-proxy` / `client-cert` 时以未闭合引号结尾的行，旧告警 `` `external-policy-modifier` cannot set `<key>` on this line ``）现在照修饰的值导入。修正轮（`28340f3`）：评审指出 `with_params` 对解析器当作类型、服务器与端口的项也取键（`http, tfo=x, 80` 里的 `tfo=x` 是服务器），改成按**字段**数而不是按项数保护（解析器丢空项，`trojan, , , tfo=false, 443` 里解析器的服务器是 `tfo=false`），用例的行从一种开头变成四种（17472 行）；不变量是"套用修饰之后的行，读出来等于原行读出来的各项，同名的换成修饰的值，其余修饰项追加在后" | 控制者的裁定（两条）：修饰放在未闭合项前面，按键读这行就满足不变量，代价是参数表里不同键的先后（没有人读）；头三个字段不取键，因为 `with_params` 的契约就是"按解析器的读法"，评审指出的缺口早于本任务，但契约是任务给的 |
| 62 | 13 | 第 5 项旧资源管理器的 `url_task`：没有切分 sleep，用的是随管理器关闭的 `watch` 通道（`gone`），管理器一没任务就醒来结束（任务书只要求 60 秒内）；"自动刷新关闭时每 60 秒醒一次"原样留着当第二道保险，修正轮（`28340f3`）把它去掉（`gone` 让它多余，是本任务自己的遗留），自动刷新关闭的任务只等刷新请求与管理器消失，新用例同时覆盖"随管理器结束"（它在改动之前也通过，不是红绿用例）。管理器在一次抓取进行中消失没有用例（真 socket 加计时器，靠读代码） | |
| 63 | 13 | 第 6、7、8、9 项：VMess 的重开循环与 `h2connect/udp.rs` 的同一段各抽成以拨号 / 开流为参数的函数（`VmessUdp::connection`、`H2Udp::stream`）才能确定地测到，`testing/vmess.rs` 没有动；修正轮让两条用例在错误之后再调一次，钉住上界不跨调用累计（两个变异——"错误路径把死连接留在表里""界限比数据报活得长"——都被杀）。Keystore 的 Base64 解码共用 `rurge_proto::keystore::decode_base64`，`rurge-proto-ssh` 的 `base64` 因此改成随 `testing` 特性（任务书没有提），"不带填充也收"这一半此前没有用例。`russh` 的日志只留 WARN 及以上（实现者读 russh 0.63.3 的源码：日志全经 `log` 的宏、target 是模块路径；临时探针对 `FakeSsh` 做一次成功、一次口令错误的登录，记到 16 种（target，级别）的组合，全是 DEBUG / TRACE、target 全以 `russh` 开头，会带服务器发来的文本的 INFO 行（`client/encrypted.rs` 里的 `Unknown channel request`）是读源码找到的；从 `log` 到过滤器的一段读的是 tracing-subscriber 与 tracing-log 的源码，没有对着运行时的 `rurge` 验证）。`General` 手写 `Debug`（解构全部 58 个字段，新加字段不写进来就编不过） | |
| 64 | 13 | 第 9 项连带改了 API 看得见的输出：任务书要把 `proxy-test-url` / `internet-test-url` 写进标记用例，标记用例的一条检查读 `redact_profile` 的输出，而 `redact_profile` 原来有意不动这两个键（M3b 终审 `ac660fe` 钉下的断言："不是内联参数、保持原样"）——两个键带着标记进配置，那条检查必然失败。于是脱敏后的配置（`GET /v1/profiles/current`，缺省 `sensitive=0`）里这两行显示 `***`，那条断言改写并改名为 `a_policy_line_and_the_profile_lose_their_test_urls` | 控制者代项目所有者裁定：进了标记配置的就是可能带凭据的字段（服务商的 URL 可以带 token），三处输出都不该出现，策略自己的 `test-url` 早已如此；这是要告诉项目所有者的一件事；代价：两个 `SECRET_KEYS` 条目、一条断言和标记配置里的两行 |
| 65 | 13 | 第 10 项 `SystemRunner::run` 失败时的报错文本抽成 `failure(cmd, status, detail)`：命令行的每个参数里最后一个 `@` 之前的部分换成 `***`（口令里可以有 `@`），scheme 留着，没有 `@` 的参数原样，工具自己的输出不动，只认 ASCII 的 `@`；修正轮（`28340f3`）：`@` 之前什么也没有的参数（`@as []`）原样显示，路径参数里的 `@` 仍被遮（宁可多遮）。任务书说"现有用例为准"，全仓库没有任何用例断言过这句文本，所以新用例就是判据；`SystemRunner::run` 确实调用 `failure` 这一点没有用例（要真的运行一个会失败的程序） | |
| 66 | 13 | 控制者在裁定第 9 项时发现、修正轮（`199148c`）收掉的六处凭据仍会进输出：(1) `[General] geoip-maxmind-url`（MaxMind 自己的下载链接带账号的 `license_key`）被 `General` 的 `Debug` 与脱敏后的配置原样显示；(2) 链接解析不了时 `rurge` 的启动告警引用整个取值；(3) GeoIP 更新器的两个资源没有标签，`resource updated`（INFO，每次启动与每 7 天）与 `resource fetch failed`（WARN）打印整条 URL——现在以数据库的文件名（`GeoLite2-Country.mmdb` / `GeoLite2-ASN.mmdb`）称呼，用 `get_labelled`；(4) `--help` 打印 `RURGE_API_KEY`、`RURGE_GEOIP_URL`、`RURGE_GEOIP_ASN_URL` 的当前值（clap 的缺省行为），三个选项现在 `hide_env_values`；(5) `#!MANAGED-CONFIG <URL>`——订阅 token 最常出现的地方——被 `ManagedConfig` 的 `Debug` 与脱敏后的配置显示，两条诊断（`unknown MANAGED-CONFIG parameter`、`unknown directive ignored`，后者对节内的 `#!` 行在 `text/mod.rs` 里也有一份）引用用户写的东西：现在脱敏后的配置留指令与参数、把链接抹成 `***`，诊断只说参数名 / 指令名；(6) 组的旧参数 `url=` 与策略自己的 `test-url` 一样隐去 | 控制者代项目所有者裁定（几条）：(1)(3)(6) 凭据不进日志与输出是项目所有者的常设约束，查询串正是 URL 带 token 的地方；`Entry::log_name()` **不改**（先裁定改，随后更正）：规则集到处以 URL 命名（诊断、`set reloaded`、请求记录与 API 里规则的文字，与 Surge 一样），在一个日志字段里截掉查询串对规则集什么也不隐藏、只是看起来如此，而 GeoIP 的 URL 是没有东西需要称呼它的设置，所以给标签；(4) 一个属性一个选项；代价：`--help` 不再显示这三个变量的当前值，GeoIP 数据库的标签是文件名而不是更长的说明 |
| 67 | 13 | 脱敏后的配置里仍然看得见、有意留着、列给项目所有者的：规则集的 URL（含查询串里的 token；与 Surge 一样以 URL 命名）、`encrypted-dns-server` 的路径与查询串（上游名字的一部分）、`[General]` 里 rurge 不认识的键的取值（隐去它们等于隐去配置的大半）。`#!MANAGED-CONFIG` 行上的参数原样保留（托管配置只有 `interval`、`strict`）；写错或长得像的指令（`#!MANAGED-CONFIGX …`、`#!MANAGED_CONFIG https://…`）在脱敏输出里目前（`28340f3`）原样——`parse_managed` 用前缀匹配，照它的读法脱敏只会抹掉 `X`、真链接留着，毫无意义——，这一条已列给最终修正波（见延后事项） | 控制者的裁定：任务书写的是"其他 `#!` 行不动" |
| 68 | 13 | 修正轮 `28340f3` 的其余几条：`publish_generation` 里 `auto.retain`（读新的组规格与它自己的状态）挪到两次 store 之前，注释与 `generation_lock` 的文档写成"在它之下先做完所有会失败的、才做第一次 store"——评审指出"没有留下半个"对一个窗口是假的（`auto.retain` 在注册表与解析器已存入之后、运行时存入之前可能 panic）；没有新用例（要毒化 `rurge-policy` 里引擎够不着的锁），守卫是 `reload`、`auto_groups`、`smart`、`subscriptions` 的现有用例。`Cargo.lock` 没有变，快照没有变。实现者在任务书之外多做的两处小事：`text/mod.rs` 里同一句诊断、诊断里"只引用像名字的参数名"，以及 R1 的第四个开头 | 控制者的裁定：实现者的六个任务书之外的取舍暂时成立，由复评逐条判断 |
| 69 | 13 | M3c #1（组环检测把 `smart` 组里写的嵌套组当作边：加载时 `W0030`、运行期那个组 REJECT）起初在任务 13 内，随后裁定不改，登记为已知限制，绕开的办法是把那个本来就不起作用的嵌套组从 `smart` 组里删掉 | 控制者代项目所有者裁定：去掉这条边要先证明每一处沿成员关系往下走的遍历都不会从 `smart` 组走进嵌套组，风险与收益不相称；代价：一个少见的配置保留一个本可解除的 REJECT |
| 70 | 13 | 过程：任务 13 首轮的三个提交一起过的门禁（它们的文件不相交），不是一个一个过；修正轮的两个提交也是一起过的。首轮的提交说明署名行 `Claude Opus 5.5 (1M context)` 与派工单写的不同（见第 12 行） | |

## 延后事项

写到提交 `28340f3`。来源是账本里每一条"minor (deferred)"、各任务报告与评审点名而没有任何后续任务接手的事项；"最终修正波"指整分支评审之后的那一轮修正，本表写下时这些事项已经列给它、都还没有做。阶段 2 的 20 份计划留下的 403 行的去向见附录与 `docs/superpowers/plans/2026-10-09-phase2-leftovers.md`，不在这里重复。

| 编号 | 事项 | 去向 |
| ---- | ---- | ---- |
| 1 | 任务 1：窄路径上 MASQUE 的回落是每条连接各自的、此后不再升回，会话不记得"这条路走不了 1350 字节"：这样的路径上每建一条连接（会话空闲 60 秒即关）都先丢一轮满包 | 移交：有用户报告再说 |
| 2 | 任务 1：缺省 `mtu`（1280）的 WireGuard 经载体的行为没有实测——1350 字节的数据报放不进一个隧道包，是分片送出、整包送出还是被丢弃后回落到 1200，只读了 smoltcp 0.12 的源码；`mtu=1280` 的 WireGuard 满包（1312 字节）经 `masque` 放不下（按余量算要设到 1264 以下），同样没有实测；内层 QUIC 经 `masque` 的真实握手也没跑过（用例只测了 1250 字节的 UDP 包往返） | 移交：手工验收（手工验收清单"`masque` 经 `underlying-proxy` 时内层 QUIC 可用"一项，任务 12 的 B 部分加入） |
| 3 | 任务 1：`trust-tunnel` 的 HTTP/3 的连接窗口在 `max-streams` ≥ 450 时保不住"仍在读的隧道照旧收到"（窗口是 u32，448 起饱和在 `u32::MAX`）；用例表里只钉了 `usize::MAX → u32::MAX`，没有另作处理；`carried_initial_mtu` 对只用 `client_config` + `connect` 的调用方（`rurge-dns`）没有效果，设计如此，注释里写了 | 关闭：这样的取值没有实际意义；DNS 的连接不经 `masque` |
| 4 | 任务 1：`session.rs` 测试里的局部常量 `STANDARD: u32` 与 `Transport::STANDARD` 同名容易看错；`capped_transport` 的注释没有提 u32 的上限 | 移交：下次动到时 |
| 5 | 任务 3：最高位为 1 的 TTL（RFC 2181 第 8 节要求读作 0）在 `Age` 的减法里与 `parse_response`、缓存一样按普通无符号数处理；要改就在读 TTL 的地方统一改 | 移交：有用户报告再说 |
| 6 | 任务 3：`DohUpstream::new(url, pathless: bool, http)` 的裸 bool（十一处调用，`rurge-dns` 之外没有使用者）；测试辅助 `canned` 与邻近用例里的局部变量同名；`age` 的注释没有说"只有空成员的头也算没有" | 移交：下次动到时 |
| 7 | 任务 2：两层中继时，里层载体打开之后报出的错误只带外层的名字（`via U: <V 的原话>`），打开时的错误两层都带 | 移交：有用户报告再说 |
| 8 | 任务 2：三个小的 `Gate` 测试载体各 crate 一份；修正轮的端到端用例的一个出口（"查找结束、有载体会话、没有失败记录"）会放过一种窄的退化——载体被重置而连接死掉、记录却没被标成失败，那条核心由 `dial.rs` 的单元用例钉住，更严的出口还要求 `down > 0` | 移交：下次动到时 |
| 9 | 任务 2：各平台对已连接 UDP socket 的 ICMP 端口不可达的处理（Windows 在下一次接收报 `ConnectionReset`，Linux 在下一次发送，macOS 在接收报 `ConnectionRefused`）后两条是评审读出来的，没有跑；端到端用例只在 Windows 上编译与运行，别的平台上是否编干净是逐个核对出来的 | 移交：CI 首跑 |
| 10 | 任务 5：引擎层的 AnyTLS 用例靠拨 tokio 的时钟，若拨钟哪天碰不到会话，它会悄悄变成真的等 3 秒（现在整个二进制 0.24 秒）；看门狗的 `debug` 日志那一行没有用例 | 关闭：接受 |
| 11 | 任务 5：sing-box 与 mihomo 的 AnyTLS 服务端什么时候应答 UDP 流，只有真实服务端验得出来（自动化现在按 anytls-go 的次序应答）；任务 5 的报告建议加一项手工验收"复用的会话上的 UDP 过了 3 秒仍然可用"，任务 12 的 B 部分的派工单没有列它 | 移交：手工验收（是否加这一项由控制者决定） |
| 12 | 任务 6：隧道死掉到当前配置的下一个连接之间，旧配置里与之冲突（同一私钥与 peer、节或载体设置不同）的残留出站可能自己起一条隧道，下一个连接时被接替 | 移交：有用户报告再说 |
| 13 | 任务 6：`send_all` 的文档只说"更新的隧道"；名字目标的 `send_to` 在查找期间碰上隧道结束时错误种类是 `Other` 而不是 `ConnectionAborted`；`recv_from` 里手写的结束检查与 `running()` 等价；`closed()` 的注释没说"对端的 FIN 先到的连接读到的是干净的 EOF 而不是错误"；`Shared::ended()` 的文档没说"`Notified` 须被 poll 过一次才会被之后的结束唤醒"；一条假对端用例依赖当前线程运行时的队列次序而没有注释；毒锁修复一旦退化，整个测试二进制会中止（双重 panic）而不是一条用例失败 | 移交：下次动到时 |
| 14 | 任务 6：`rurge-proto-wireguard` 里 `TUNNELS`、隧道内 DNS 的缓存与限频告警表的锁仍是 `expect`（载体的两把小锁已在修正轮改掉）；M4b #31 的后一半、#22、#10 留着 | 移交：有用户报告再说 |
| 15 | 任务 7：注册表用例里手写的 `tracing` 订阅者（约 50 行）与 `rurge-proto-quic` 的 `test_log` 有同样的理论上的注册窗口 | 移交：下次动到时 |
| 16 | 任务 7：字母表之外的 ASCII `server` 名在拨号时才失败（拨号时的两句是读代码得出的，没有跑）；自定义 `sni` 以 Unicode 写时仍是干构建错误（`tls_name` 不转换）；订阅导入的策略的服务器名不在 `Config::proxy_hostnames()` 里（既有缺口）；`rurge-proto-ssh` 测试模块里一处重复的 `use` | 移交：有用户报告再说 |
| 17 | 任务 8：`state.json` 存在但读不了（权限、被独占）时按"没有备份"处理、不出声（正常启动会记一条 WARN）；日志文件打不开的出口上 tracing-appender 自己先往标准错误打一行 | 移交：有用户报告再说 |
| 18 | 任务 8：真实的系统代理后端（Windows 注册表、macOS `networksetup`、Linux 的 `gsettings` / `kwriteconfig`）从没在"坏配置时的恢复"这条路径上跑过 | 移交：手工验收（阶段 1 手工验收清单要加一项：带 `--system-proxy` 启动后强杀进程，把配置改坏，再 `rurge run`，及"日志文件打不开"的变体；任务 12 的 B 部分） |
| 19 | 任务 4：被豁免的失败不设成员的 `reported` 标记，所以只被豁免的会话认识的成员之后因测速而在"健康 / 失败"之间切换时，不出那两行 INFO，直到有一次真正算数的会话回报 | 移交：有用户报告再说 |
| 20 | 任务 4：站点记忆提前丢掉条目的三种情形（站点数超过 4096、一个站点超过 16 个策略、重载去掉了策略）会让同一站点在一小时内多罚一次；N 个不同的失效地址仍罚 N 次（没有采用 (a) 的代价）；站点记忆按策略名记，定义变了的成员（记录清零）在旧站点上仍被它自己不到一小时的旧条目豁免 | 关闭：接受（量级上不成问题；任务 12 的 B 部分登记进兼容性清单的 `smart` 一行） |
| 21 | 任务 9：自适应的转发缓冲（从 8 KiB 起、某个方向接连把它读满时才换大，v2ray 的读取分配策略）：缓冲越大越快，但每条会话两块缓冲、忙闲都在，1 万条会话 8 KiB 是 156 MB、16 KiB 是 312 MB、64 KiB 是 1.22 GiB，与 NFR-01（< 300 MB）相抵，要有自己的内存与吞吐实测；基准 `relay_throughput` 留在仓库里 | 移交：阶段 3 建立性能基准（NFR-01 写明那时校准）时一并定；在回环正常的机器（CI 的 Linux）上重跑基准 |
| 22 | 任务 9：基准的数字是这台机器的（Windows 回环、整条链在一个线程上、基准进程只用约 0.3 个核），10% 那条线附近的输赢没有把握；下行方向的增益没有测到（假服务端每次只转 / 读 8 KiB，真实服务端发 16 KiB 的 TLS 记录时大缓冲在下载方向上也会省一半读写）；`vmess::chunk::MAX_PAYLOAD` 是 16384 − 16，二的幂大小的写总带一个极小的尾块，`VmessStream` 读每个分块要向下层读两次 | 移交：阶段 3 建立性能基准时一并看 |
| 23 | 任务 9：`transport::shadow_tls` 的测试模块有自己的 `FlushGated`，与 `testing::held::HoldsUntilFlushed` 是同一个意思 | 移交：下次动到时 |
| 24 | 任务 9：下层的拒绝若以 EOF 表现（如 `ss` 的"服务端一个字节没回就关闭"），在 `http` / `socks5` 之上仍被说成上层代理关了连接——TLS 层的同类 EOF 也带话，二者在这里分不开；要分得改成按错误来源而不是按类型判断 | 移交：有用户报告再说（与 M7c"链上的错误没有主人"的 TCP 一半同题） |
| 25 | 任务 9：端到端用例 `a_proxy_that_wants_a_client_certificate_says_so` 断言警报的原话，只在警报没有被复位抢先时成立：本机 200 / 200，Linux / macOS 的接收队列在 RST 之后是否仍先交出已到的数据，是凭记忆、没有跑过；退路是只断言不含 `larger than` | 移交：CI 首跑 |
| 26 | 任务 10：提交 `21ecb85` 的说明少了主题行后的空行，`d309f33`（任务 9 修正轮）的两行 trailer 缩进在最后一个列表项下（见"执行期修正记录"） | 由项目所有者决定：推送前是否 reword（提交不改写） |
| 27 | 任务 10：接受循环里 `canonical_peer` 的调用点没有监听层面的用例（Windows 上产品不设那个 socket 选项，造不出收 IPv4 客户端的 IPv6 监听）；几处没有改成用 `rejected()` 的旧写法（`ecn_read`、`relay_unknown`、`spec/mod.rs` 里 `h3` 的 `open`，它们错误的一方解析成缺省值或 `None`，本来就不再说话） | 移交：有用户报告再说；旧写法下次动到时 |
| 28 | 任务 11：`rebind_failure_leaves_no_listeners_and_a_later_rebind_recovers` 的 `os error 10048` 的原因没能证明（放掉再抢一万次一次也没失败），用例不再依赖系统把同一个号还回来；再出现时最可能是临时端口耗尽，是机器的状态 | 移交：有用户报告再说（再出现时先查机器状态，不改用例） |
| 29 | 任务 11：没有改的同类写法——`pipeline.rs` 的两处 `free_port()`（各自已重试一次）、`rurge-proto` 的 `direct.rs`、`snell/pool.rs` 与 `rurge-proto-quic` 的 `session.rs` 里各一处 `drop(listener)`（单次使用，从没见过失败）；两个端口辅助函数（`closed_udp_port()` 与 `tests/external` 的 `free_port()`）是同一段代码分在两个互不共享测试支撑的 crate 里（合并要改 manifest，`rurge_net::testing` 是候选的家） | 移交：下次动到时 |
| 30 | 任务 11：`udp_silence_counts_only_where_answers_always_come` 的 3.5 秒是 `NO_RESPONSE`（3 秒，`pub(crate)`）的字面拷贝，只有 0.5 秒余量，常量若抬到 3.5 秒以上，第一步什么也证明不了而仍绿；重绑用例从不断言重绑失败之后确实没有东西在听；wireguard 的三处"马上"断言从 < 2 秒放宽到 < 5 秒，拨号自己的预算也是 5 秒，真有等待时仍分得出来，余量全在慢机器一侧 | 关闭：接受（已知的耦合，注释里写了） |
| 31 | 任务 11：互操作夹具（`SingBox`、`Xray`、`Dnsproxy`、`CoreDns`）改用 `free_ports` 后的取端口行为没有对真实的参照服务端跑过；`free_ports` 要求每个端口对 TCP 空闲，UDP 端口因此多占一个同号的 TCP 空闲，不影响正确性 | 移交：CI 首跑 |
| 32 | 任务 14：`udp_silence_counts_only_where_answers_always_come` 里发往 `127.0.0.1:443` 的 UDP 流是真的由假服务端发出去的（端口号正是用例要考的东西）；UDP 443 上有服务的机器会把它弄红 | 移交：有用户报告再说 |
| 33 | 任务 14：关着的回环端口"立刻拒绝"（Linux / macOS）只能在引擎的集成测试里用"接了立刻关"顶替，真的要等 CI；`NO_TEST` 仍指向 9 号端口，新写的用例只在本机"接了不答"的世界里验证过 | 移交：CI 首跑；约定：新写的、经自动组拨号的用例，把常量指到关着的端口再跑一遍 |
| 34 | 任务 11、14：`tests/interop/README.md` 有三处陈旧的句子——讲 `free_ports` 的段落只说 mihomo 用它（现在是所有一次拉起多个端口的夹具：sing-box、xray、dnsproxy、CoreDns、mihomo、ssserver；只取一个端口的不用）、`common::Failures` 一节没有写 `Drop` 守卫与它的三条用例（以及子进程用例需要 `tokio` 的 `process` 特性）、dnsproxy 的上游 `127.0.0.1:9`"到那里就没有下文"只对本机成立（CI 上那个端口关着，问题会被拒绝；没有用例问到它，措辞应改为"9 号端口（用例不会问到它）"） | 没有去向：任务 12 的三部分都没有列这个文件，控制者另行指派；这是测试说明文档里的陈旧句子，不影响任何产品行为，所以可以等；尚未做 |
| 35 | 任务 15：迟到但还没过截止时间的 fanout 把错过的轮次背靠背发出去（至多 `attempts − 1` 轮）；醒来即结束的 fanout 可能丢掉一条已完成、还没被取走的应答（与普通到点时的比赛相同，没有用 `biased`） | 移交：有用户报告再说 |
| 36 | 任务 15：AAAA 抑制的计数——后台补查的超时是否也算进"连续 5 次"。现行：一个接一个地问，抑制在第三个名字的查询期间生效，背靠背地问在第四个名字期间；测试把阈值钉住并如实写出现行计数 | 由项目所有者决定：定了之后改计数，同时改 `aaaa_suppression_after_five_timeouts_and_flush_resumes` 里的 `i >= 2`；兼容性清单"连续 5 次 AAAA 超时后停止发送 AAAA"一行带备注（任务 12 的 B 部分） |
| 37 | 任务 15：`UdpUpstream::query` 在查询被放弃（fanout 的 `abort_all`）且应答永远不来时留下 `pending` 条目，以 16 位 id 空间为界（每个上游 65,536 条）；WireGuard 协议栈的 `advance` 会不会一轮又一轮地给出零等待（smoltcp 的 `poll_at`；控制者读了 `seq_to_transmit`，它顾及了拥塞窗口，没有再查，一条数设备循环空转次数的用例可以定论）；用 `sleep(时长)` 写的循环没有扫全（看了四处） | 移交：有用户报告再说 |
| 38 | 任务 15：解析器的"过期命中 → 后台刷新 → 再进缓存"与"部分结果之后的后台补查进缓存"在真实 socket 上没有用例了（只在内存里；`bootstrap` 的孪生用例走 UDP）；fanout 的重发 / 部分结果 / 空答规则在 100 毫秒这个量级上也只在内存里（`tests/resolver.rs` 默认 1 秒的两条走真 socket）；测试支撑的限制：`MockDns::upstream()` 不带截断与 TCP 重试，`resolver_in_memory` 只换 `primary` | 关闭：接受（写在测试支撑的注释里） |
| 39 | 任务 15：Windows 上机器睡眠再唤醒后 rurge 的 CPU 回落、解析恢复——真实的睡眠只能手工验（单元用例用拨钟代替） | 移交：手工验收（阶段 1 手工验收清单，任务 12 的 B 部分） |
| 40 | 任务 15（最终修正波，测试，各一行）：多线程用例 `all_dropped_is_a_timeout_after_every_attempt` 的名字比它现在断言的多（只剩错误的种类），改名（建议 `all_dropped_is_a_timeout_on_a_multi_thread_runtime`）；`fanout.rs` 测试里三处讲一点历史的注释改成描述代码；对孪生 `a_fanout_kept_from_running_until_past_its_deadline_hands_over_what_it_has` 做评审给的变异并记结果（至今没有变异让它红过）；三条在 `fast()` 下做真实查询而不数问题的解析器用例（`host_chain_ips_aliases_loops_and_proxy_exemption`、`no_servers_uses_system_upstreams_then_lookup_host`、`implements_lazy_resolver_and_resolve`）改用 `resolver_once`；`upstream/tcp.rs` 的 `lock_wait_respects_the_callers_deadline`（真 TCP、真时钟，100 毫秒期限与 400 毫秒持有之间只有 200 毫秒余量）让服务端在短查询返回之后才放行、整体加时限、去掉 `< 300 ms` 的窗口；`testing.rs` 的 `misbehaviours`：`udp_ask` 等 500 毫秒收一个迟 200 毫秒的应答，改成等得更久（等到应答来了就结束） | 最终修正波（整分支评审之后的一轮）；尚未做 |
| 41 | 任务 15：每秒一次的流量采样（`crates/rurge-engine/src/engine.rs`）的 `interval` 取缺省的 `Burst`，机器睡过之后的 tick 背靠背触发；改 `MissedTickBehavior::Delay`（一行；有可观察量时才加用例） | 最终修正波（整分支评审之后的一轮）；尚未做 |
| 42 | 任务 9、14、11：三处文档注释——`BUF`（`relay.rs`）把测得的增益说松了（"64 KiB 在每个加密栈上约多一成"，一成是两个方向合起来的，增益全在上行方向）；`closed_tcp_port`（`crates/rurge-engine/tests/common/mod.rs`）的第二句难读，拆成两句（试过端口的 socket 已不在；为什么不留一个绑了不听的 socket；Windows 上拒绝要约两秒）；`udp_silence_counts_only_where_answers_always_come` 的文档注释里"the other follows once it has had its three seconds"指的是哪条流 | 最终修正波（整分支评审之后的一轮）；尚未做 |
| 43 | 任务 13：脱敏后的配置里仍然看得见、有意留着的：规则集的 URL（含查询串里的 token；规则集到处以 URL 命名，与 Surge 一样）、`encrypted-dns-server` 的路径与查询串（上游名字的一部分）、`[General]` 里不认识的键的取值（隐去它们等于隐去配置的大半） | 由项目所有者决定：是否隐藏、怎样隐藏；现状登记进兼容性清单（任务 12 的 B 部分） |
| 44 | 任务 13：`redact_profile` 对写错或长得像的托管指令（`#!MANAGED-CONFIGX …`、`#!MANAGED_CONFIG https://…`）原样不动，并保留托管行上多出的词（任务书说"其他 `#!` 行不动"）；候选规则：任何 `#!` 行里含 `://` 的词都隐去，托管行上除 `interval=…`、`strict=…` 外的词都隐去 | 最终修正波（整分支评审之后的一轮），取决于对任务 13 修正轮的复评；尚未做 |
| 45 | 任务 13：`ConfigSummary.managed`（语料库快照用的概览，`crates/rurge-config/src/config.rs`）仍带托管配置的 URL，现在没有产品代码用它；`GeoUrls`、bin 里的 `Runtime`、`ControlArgs`、`RuntimeArgs` 派生 `Debug`，会显示 GeoIP 的 URL 或 `--key` 的明文，`TestMode` 的 `Url` 变体同理（没有打印处，只按常见写法搜过） | 移交：下次动到时：任何把 `ConfigSummary` 放上 API、或把这些类型按 `Debug` 打印的改动之前，先去掉 URL 与密钥（改 `managed` 要重写语料库快照） |
| 46 | 任务 13：`managed.rs` 的 `invalid interval` / `invalid strict` 仍引用取值（打错的数字、开关，不是凭据）；失败命令文本里路径参数中的 `@` 仍显示为 `***@…`（宁可多遮）；句子 `` `external-policy-modifier` cannot set `<key>` on this line `` 经 `with_params` 已走不到，留作兜底，由 `a_line_that_is_not_as_the_modifier_sets_it_is_refused` 直接测 | 关闭：接受 |
| 47 | 任务 13：没有用例的几处——管理器在一次抓取进行时消失（真 socket 加计时器，靠读代码）；`SystemRunner::run` 确实调用 `failure`（要真的运行一个会失败的程序）；`override_of` 写日志时锁已放开（靠读代码）；`publish_generation` 的顺序调整（要毒化 `rurge-policy` 里引擎够不着的锁）；`russh` 的过滤在运行时的 target 只验证了过滤器的一半 | 关闭：接受 |
| 48 | 任务 13：`settle`（`crates/rurge-engine/src/stack.rs`）靠 URL 以 `GeoLite2-Country.mmdb` / `GeoLite2-ASN.mmdb` 结尾认出 GeoIP 资源，所以配了自定义的 `geoip-maxmind-url` 时，第一次下载后紧接着的一次性命令不会等到数据库装好 | 移交：有用户报告再说 |
| 49 | 全程：M8a 任务 6（CI 首跑的修正）和整个 M8 的收尾都要等一次推送：CI 从没在这些提交上跑过，互操作用例（含 M8b 与 M8c 新加和改动的夹具）在本机没有参照服务端而全部跳过，手工验收清单一项也没有做过 | 移交：CI 首跑（项目所有者推送之后；失败清单靠 M8a 任务 1 加的失败摘要读到） |
| 50 | 全程：偶发的 `STATUS_HEAP_CORRUPTION`（`0xc0000374`）来自开发机上终端管控软件注入的 DLL，不是 rurge 的缺陷；`clippy-driver` 的一次 `0xc0000005` 形状相同、没有证据 | 关闭（M8-D4）：遇到重跑那个二进制 |

## 附录：阶段 2 遗留事项总表

阶段 2 的 20 份计划（M1a ～ M7c）各有一张「延后事项」表，共 403 行。每一行最后的去向——已做掉 / M8 做掉 / 关闭 / 移交——逐行写在 `docs/superpowers/plans/2026-10-09-phase2-leftovers.md` 里，连同依据（提交、文件与函数、接手它的计划），并附一份「阶段 2 结束时仍开着的事项」按标签分组的清单。表放在单独的文件里是控制者的裁定：403 行放进本计划会埋掉本计划自己的两张表。下面是小计（M8a 与 M8b 两份计划自己的 18 行、本计划的 50 行不在其中）：

| 计划 | 已做掉 | M8 做掉 | 关闭 | 移交 | 合计 |
| ---- | ------ | ------- | ---- | ---- | ---- |
| M1a | 17 | 1 | 7 | 6 | 31 |
| M1b | 11 | 1 | 12 | 9 | 33 |
| M2a | 4 | 1 | 8 | 1 | 14 |
| M2b | 12 | 6 | 9 | 5 | 32 |
| M2c | 1 | 4 | 0 | 10 | 15 |
| M3a | 10 | 9 | 3 | 12 | 34 |
| M3b | 3 | 6 | 0 | 14 | 23 |
| M3c | 1 | 2 | 1 | 8 | 12 |
| M4a | 2 | 3 | 0 | 10 | 15 |
| M4b | 3 | 6 | 4 | 18 | 31 |
| M4c | 0 | 0 | 0 | 9 | 9 |
| M5a | 2 | 1 | 0 | 18 | 21 |
| M5b | 2 | 0 | 2 | 10 | 14 |
| M5c | 1 | 1 | 2 | 15 | 19 |
| M6a | 2 | 2 | 2 | 9 | 15 |
| M6b | 0 | 0 | 3 | 6 | 9 |
| M6c | 1 | 1 | 2 | 7 | 11 |
| M7a | 5 | 2 | 2 | 4 | 13 |
| M7b | 2 | 2 | 6 | 6 | 16 |
| M7c | 1 | 15 | 10 | 10 | 36 |
| 合计 | 80 | 63 | 73 | 187 | 403 |

做法：五个只读的审计各按里程碑分一组（M1a ～ M1b、M2a ～ M2c、M3a ～ M4a、M4b ～ M5c、M6a ～ M7c），逐行打开登记它的计划、接手它的计划与代码，写下结局和依据；一份草稿把五份合并，并应用控制者的裁定（行的归属、同一缺陷在几张表里的重复登记、哪些行算「关闭」）；任务 12 的 A 部分再把草稿里依赖当时尚未执行的 M8c 任务的行逐一核对到提交 `28340f3`、把行里引用的名字与提交核对到仓库、重数小计（核对时改了结局的三行写在那个文件的开头）。小计由脚本按每行「结局」一列的第一个词数出，不是手工加减，每份计划表头里的行数与表一致。阶段 2 结束时仍开着的 187 行按标签分：项目所有者 2、CI 首跑 20、手工验收 8、阶段 3 8、依赖升级时 3、有用户报告再说 98、下次动到时 37、其它 11。
