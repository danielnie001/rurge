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
| M2b 计划期 | AnyTLS 的 SYNACK 看门狗 | 5 |
| M4b #14、#20、#23、#28、#31 | WireGuard 的收尾 | 6 |
| M3a #13 | 指向构建失败成员的中继 | 7 |
| M2b P15、M4a #7、M4b #8 | 出站一侧的 IDN | 7 |
| M1b | 坏配置时的系统代理恢复 | 8 |
| M2b、M2c #1、#15 | 转发循环的 flush 与缓冲；握手的 flush | 9 |
| M7b #8、#16，M7c #33 | IPv4 映射地址的 UDP；用例里的时限 | 10、11 |
| M3c #4 | `CORE_VERSION` | 10 |
| M8b 延后事项 #6、#7、#9、#12 与评审记下的几处 | 互操作夹具逐个取端口；两条偶发失败的用例；`udp-port` 的措辞；已报错的行多说一句的其余各处；转发循环的写约定；几处过时的文档句子 | 11、11、12、10、9、12 |

## 执行顺序

1、2、3、5、6、7、4、8、9、10、11、12（任务 4 用任务 7 的 `rurge_config::hostname::to_ascii`）。

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

## 执行期修正记录

| 编号 | 任务 | 修正 | 原因 |
| ---- | ---- | ---- | ---- |

## 延后事项

| 编号 | 事项 | 去向 |
| ---- | ---- | ---- |

## 附录：阶段 2 遗留事项总表

（任务 12 填写。）
