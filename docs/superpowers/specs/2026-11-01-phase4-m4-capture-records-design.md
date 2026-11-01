# 阶段 4 / M4「抓包与请求记录的完整」细化设计

| 项       | 内容 |
| -------- | ---- |
| 日期     | 2026-11-01 |
| 状态     | 构建者检查之后的定稿。已决事项 M4-D1 ～ D31（D26 ～ D31 是构建者检查之后定的）是在项目所有者"自己决定、继续做"的常设授权下定的（代项目所有者作的裁定），所有者可以改。执行交给 Pi：M4a、M4b 两份计划由协调的会话逐个任务调 Pi 做、逐个任务评审（第 2 节） |
| 上位设计 | [阶段 4 总设计](2026-10-29-phase4-http-engine-design.md) 第 1.4、1.5、9、10、13、14、17 节；[M1 细化设计](2026-10-29-phase4-m1-session-entry-http-engine-design.md) 第 8、10 节（先读第 20、21 节的订正）；[M2 细化设计](2026-10-30-phase4-m2-mitm-design.md) 第 11、12 节（先读第 23 节）；[M3 细化设计](2026-10-31-phase4-m3-rewrite-map-local-design.md) 第 8.5、11、12 节（先读第 22 节）。不一致处以本文为准，订正列在第 15 节 |
| 需求     | FR-HTTP-11（抓包，M4a）；FR-RULE-14（`always-capture`，M4a）；FR-HTTP-13 的 `capture`（M4a）；FR-HTTP-10 的完整（M4b：各阶段耗时的第三段、两个正文的字节数、备注里直通的原因、`remoteAddr` 在复用的连接、隧道与 UDP 流上、`dnsMs` 在解析途中；来源的设备在阶段 7）。FR-HTTP-11 的"通过 Dashboard 查看"在阶段 6 |
| 清单     | 3.2 节 L271（`always-capture`）；7.1 节 L569（抓包视图与备注）；10.3 节 L863（`rurge run`，两个新参数）；10.4 节 L871（功能开关）、L884（`/v1/requests/*`）。行号是 2026-11-01（`e963d58`）的 |
| 前置     | M3（M3a、M3b 与 M3b 的任务 10）做完，本地 `main` = `e963d58`；`origin/main` = `41c35d4`（M1、M2、M3 的提交等项目所有者推送，推送之后读那次 CI）。M4 在分支 `phase4-m4` 上，从 `e963d58` 起 |
| 调研     | 总设计的调研 `docs/reports/2026-10-29-phase4-http-mitm-survey.md` 第 5 节（HAR 1.2 的字段与 `-1` 的约定，mitmproxy 与 proxyfor 怎样存、怎样导出）；手册三页：`rules/overview.html`（`always-capture`："Any rule, including FINAL"，"Force-enable HTTP capture for connections matched by the rule, recorded under the named capture session. Intended for debugging."）、`http/overview.html`（"Requests handled by the HTTP engine appear as individual HTTP requests in the Dashboard's capture viewer, with full headers and bodies available."；"the request detail view shows the notes for applied rewrites and scripts."）、`tools/http-api.html`（功能开关；没有读抓包内容的端点，见 `docs/reports/2026-10-29-phase4-manual-recheck.md` 第 8 条），下文的引文出自它们；业界的上限（2026-11-01 读的源码与文档，只取事实）：Chromium 的 `inspector_network_agent.cc`（桌面的总量 200 000 000 字节、单条 20 000 000 字节，Android 10 MB / 5 MB）与 `network_resources_data.cc`（每块数据进来时记账，`EnsureFreeSpace` 先挤掉最早的，单条超限时整条的内容不要）、Node.js 的 inspector（提交 `93c5776b0e`：总量 100 MiB、单条 5 MiB，最早的先挤掉）、mitmproxy（不设上限，`stream_large_bodies` 缺省不设，流式的正文缺省不存）、Charles（到上限就停止录制）；RFC 7541 第 4.1 节（一个头记作名字 + 值 + 32 字节）；阶段 1 / M5a 计划延后事项第 3、20、21、22 行 |

M4 做两件事。**抓包**（M4a）：`capture` 开关开着、或这个请求求规则时命中的那一行带着 `always-capture=<名字>` 时，经 HTTP 引擎的每个请求——明文的、M2 解密之后的、被拒绝的与 rurge 自己答复的都算——在它的 HTTP 部分结束时存进一份只在内存里的抓包存储：请求离开 rurge 时（没有离开的是求规则时）的请求行、头与正文的开头，交给客户端的状态行、头与正文的开头，各段耗时、远端地址、结局、规则、策略与备注。存储有总量与单条正文两个上限（只走命令行参数与环境变量），正文按块记账、装不下时先挤掉最早的条目。抓到的经 rurge 专有的 `/v1/rurge/capture/…` 读出，一个会话也能导出成 HAR 1.2。**请求记录的完整**（M4b）：记录多出收正文的耗时与两个正文的字节数；可嗅探的会话没有进 HTTP 引擎、名单命中却没有解密时，备注写明原因；`remoteAddr` 改成"这条会话离开 rurge 时所经的连接或载体的远端"，在复用的连接、隧道与 UDP 流上补全；`dnsMs` 在解析途中也有数。

**不加新包、不下载**（第 3 节）。两份计划改的代码都在本机编译、运行，没有只在 CI 上编译的代码。

## 1. 范围

| 做 | 不做（去向） |
| -- | ------------ |
| 抓包存储：一个引擎一份、只在内存里、重载不清；总量与单条正文两个上限，`--capture-size` / `RURGE_CAPTURE_SIZE` 与 `--capture-body-size` / `RURGE_CAPTURE_BODY_SIZE`（第 4.1、4.2 节，M4a） | 落盘、跨进程保留（不做：总设计第 9.2 节"存储只在内存里"） |
| 正文按块记账、装不下先挤掉最早的条目、单条超限只留开头（第 4.3、4.4 节，M4a）；量一次内存（第 4.5 节） | — |
| 命名会话：开关抓到的进 `default`，`always-capture=<名字>` 的进那个名字（第 4.6、5 节，M4a） | — |
| `capture` 开关生效并持久化（第 5.1 节，M4a） | — |
| `always-capture`：命中的那一行（含 `FINAL`）带着它时开关关着也抓；没有名字的给 `W0012`（第 5.2 节，M4a） | 让命中的连接进 HTTP 引擎或被解密（不做：M4-D8，登记） |
| 抓到的：请求、请求正文、应答、应答正文、耗时、远端地址、结局、规则、策略、备注（第 6 节，M4a） | 原始的头字节与大小写、`headersSize`（hyper 读不到，总设计第 6.9 节）；trailer；WebSocket 的消息与隧道里的字节（总设计第 1.3 节） |
| `/v1/rurge/capture/…` 六个端点；`/v1/features/capture` 生效（第 7 节，M4a） | Dashboard 的抓包视图（阶段 6） |
| HAR 1.2 导出（第 8 节，M4a） | 导入 HAR；拆 `cookies`（写空表，第 8.2 节） |
| 读一条时按 `Content-Encoding` 解开 `gzip`、`x-gzip`、`deflate`（第 7.5 节，M4a） | `br`、`zstd`（等总设计 Q4，同 M3c） |
| 记录的新字段：`receiveMs`、`requestBodyBytes`、`responseBodyBytes`（第 10.1 节，M4b） | 来源的设备（阶段 7）；脚本的备注（阶段 5） |
| 直通的原因写进备注，九种（第 10.2 节，M4b） | — |
| `remoteAddr` 的新口径与在复用的连接、隧道、UDP 流上的补全（第 10.3 节，M4b） | 第 10.3 节末尾列出的几种照旧是 `null`（登记） |
| `dnsMs` 在解析途中也有数（第 10.4 节，M4b） | — |
| 记录的容量量一次（第 10.5 节，M4b） | 改 `--request-log-size` 的缺省（不改，M4-D25） |
| 手工清单加 M4 的几项（M4a、M4b 各自的收尾） | 手工验收本身（归所有者） |

## 2. 计划与执行

两份计划。后一份在前一份合并之后开工，它的计划也在那时对着合并之后的代码写（同 M1 ～ M3）：

- `docs/superpowers/plans/2026-11-01-phase4-m4a-capture-plan.md`
- M4b（请求记录的完整）的计划日后写，文件名同样的格式。

M4a 的任务：

| 任务 | 内容 |
| ---- | ---- |
| 0 | 开工前核对：第 14 节的各项对着当时的代码核一遍，出入记进「执行期修正记录」；量一次基线 |
| 1 | `rurge-inbound`：`SessionHandle::when_finished` 与抓包会话的槽（第 5.3、6.4 节）；`rurge-config`：没有名字的 `always-capture=` 给 `W0012`（第 5.2 节） |
| 2 | `rurge-http` 新 `capture` 模块：存储、上限、记账、挤掉与截断、会话、条目、读出时的解压，都是不碰网络的函数、各有单元用例（第 4、6、7.5 节） |
| 3 | `rurge-http` 的引擎：`Routed` 带 `capture`、每个请求一个 `CaptureSlot`、请求的两个取样点、`Idle` 里抄请求正文、包住应答正文的一层、存进去的时刻（第 6 节） |
| 4 | `rurge-engine`：`EngineShared.capture`、`capture` 开关与 `state.json`、启动时还原、`choose_policy_with` 填抓包会话的槽、`ConnUpstreams::route` 给 `capture`、再导出 `rurge_http::capture`（第 4.1、5 节） |
| 5 | 可执行文件：两个参数与两个环境变量、大小的写法、单条大于总量时降到总量（第 4.2 节） |
| 6 | `rurge-api`：`/v1/features/capture` 生效；状态、会话、一个会话的条目、一条、清除；遮住四个头（第 7 节） |
| 7 | `rurge-api`：HAR 导出（第 8 节） |
| 8 | 端到端用例（第 12 节第 3 条）与量内存的两条忽略的用例（第 4.5 节） |
| 9 | 文档收尾（第 13 节）；手工清单加 M4a 的几项 |

M4b 的任务（计划日后写，这里是大纲）：核对（第 14 节 V13、V14）；记录的三个新字段（`observe.rs`；正文的字节数在 `Idle`、`Tail`、`LocalFile` 与 rurge 自己的答复里、`finish` 之前数，不在 M4a 包住应答的那一层，M4-D30）；直通的原因（`serve.rs` 的 `sniffable_session`、`serve_sniffed`、`decrypting`，`Mitm::decides` 的返回）；`Trail` 记在途的查找、`rurge-dns` 的 `Noted`；复用的 TCP 载体记着对端（snell 连同失效重拨、anytls、`h2pool`、ssh）；QUIC 一族与 WireGuard 的对端；UDP 流的 `remoteAddr`；端到端与量一条记录的大小；文档收尾（`docs/status.md`、`docs/api/phase4.md` 里订正 `docs/api/phase1.md` 那一段 `remoteAddr` 的句子、清单、两份 README、`CLAUDE.md`）。

**执行的约定**（两份计划开头重申，同 M1 ～ M3）：协调的会话逐个任务开 Pi 的会话、把任务的全文与要用的接口交给它；Pi 做完写报告、提交；协调者读改动、跑门禁、评审，有问题回到同一个 Pi 会话修正，再评审，过了才派下一个任务。每个任务先写失败的用例、看到红，再实现到绿；门禁是 `CLAUDE.md` 的全量门禁；提交说明的标题用中文。计划没有写到、要做决定的，Pi 停下来报告，不自己定。

做完 M4b 之后：项目所有者推送（M1 ～ M4 那时还没有推送的一起），读回那次 CI；再往后是 M5（`auto-quic-block`、基准与遗留）。M4a 之后不单独停下来等推送。

## 3. crate 与接口

**不加新包、不下载**：M4a 用到的 `flate2`（M3b 起是 `rurge-http` 的正式依赖）、`base64` 0.22、`futures-util`、`time` 0.3.45、`url` 都在 `Cargo.lock` 里。`rurge-api` 新增三条正式依赖，都取自工作区的依赖表：`base64`（API 与 HAR 里的正文）、`futures-util`（HAR 逐条写出，第 8.4 节；用不上时计划可以改用已在锁文件里的 `http-body`，第 14 节 V8）、`time`（写 `startedDateTime`，开 `formatting` 特性——它只多要 `itoa`，`itoa` 已在锁文件里）。`Cargo.lock` 的包数应当不变（今天 520 个），用到新依赖的任务各核一次。依赖方向不加新的工作区边：`rurge-engine → rurge-http`、`rurge-api → rurge-engine` 都已有；`rurge-api` 经 `rurge-engine` 的再导出用 `rurge_http::capture` 的类型（同 `ApiTls`），`rurge-api` 只依赖 `rurge-engine` 与 `rurge-config` 的规矩不变。`unsafe` 不变（`LISTED` 23）。

| crate | 改动 |
| ----- | ---- |
| `rurge-config` | 没有名字的 `always-capture=`：`W0012`，规则照常、这一行不抓（M4a）；`RuleParams` 不改 |
| `rurge-inbound` | `SessionHandle::when_finished`；抓包会话的槽 `set_capture_session` / `capture_session`（M4a） |
| `rurge-http` | 新 `capture` 模块（M4a）；`engine.rs`：`Routed` 的 `capture`、`CaptureSlot`、两个取样点、`Idle` 与包住应答正文的一层（M4a），两个正文的字节数（M4b） |
| `rurge-engine` | `EngineShared.capture`；`capture` 开关（`capture_on` / `set_capture`，与 `rewrite_on` 并排）、`state.json`、`restore_switches`；`choose_policy_with` 填槽；`ConnUpstreams::route` 给 `capture`；`pub use rurge_http::capture`（M4a）；`observe.rs` 的新字段与 UDP 的 `remoteAddr`、`serve.rs` 的直通原因（M4b） |
| `rurge-api` | `/v1/features/capture`；新 `routes/capture.rs`（六个端点）与 HAR（M4a）；`/v1/requests/*` 的三个新字段（M4b） |
| `rurge`（可执行文件） | 两个参数与两个环境变量（M4a） |
| `rurge-net`、`rurge-dns` | M4b：`Trail` 记在途的查找、`Noted` 改成在途的票据（第 10.4 节）；在 `follow` 之外写对端用已有的 `Trail::set_peer`（第 10.3 节） |
| `rurge-proto`、`rurge-proto-quic`、`rurge-proto-ssh`、`rurge-proto-wireguard` | M4b：复用、共用的载体记着自己的对端，借出时写进会话的 `Trail`（第 10.3 节） |

接口的形状（细节以计划为准）：

```rust
// rurge-inbound（M4a）
impl SessionHandle {
    /// 记录还没有结束：同 `on_finish`；已经结束：当场以它的结局调 `f`。
    /// 与 `finish` 并发时不漏、不调两次（第 6.4 节）
    pub fn when_finished(&self, f: impl FnOnce(&SessionHandle, &SessionOutcome) + Send + 'static);
    /// 命中的那一行的 `always-capture`（第 5.3 节）：只有第一次设的算
    pub fn set_capture_session(&self, name: Arc<str>);
    pub fn capture_session(&self) -> Option<Arc<str>>;
}

// rurge-http（M4a）
pub mod capture {
    pub const DEFAULT_SESSION: &str = "default";
    pub const DEFAULT_TOTAL: u64 = 100 * 1024 * 1024;
    pub const DEFAULT_BODY: u64 = 5 * 1024 * 1024;
    /// `body` 不大于 `total`（`Limits::new` 降下来，第 4.2 节）
    #[derive(Clone, Copy)]
    pub struct Limits { pub total: u64, pub body: u64 }
    /// 一把锁：条目的队列（由旧到新）、记着的字节、进行中的预留、四个数（第 4.3、4.4 节）
    pub struct CaptureStore { /* … */ }
    impl CaptureStore {
        pub fn new(limits: Limits) -> Arc<CaptureStore>;
        pub fn limits(&self) -> Limits;
        pub fn status(&self) -> Status;                         // 第 7.1 节
        pub fn sessions(&self) -> Vec<SessionSummary>;          // 只有有条目的，按名字排
        pub fn session(&self, name: &str) -> Vec<Arc<Entry>>;   // 由旧到新；锁只握到拿完 `Arc`
        pub fn entry(&self, id: u64) -> Option<Arc<Entry>>;
        pub fn clear(&self, session: Option<&str>) -> usize;
    }
    /// `route` 交给引擎的：这个请求进哪个会话（第 5.3 节）
    #[derive(Clone)]
    pub struct Capture { pub store: Arc<CaptureStore>, pub session: Arc<str> }
    /// 不实现 `Debug`（第 6.6 节）
    pub struct Entry {
        pub id: u64,                    // = 记录的 ID
        pub session: Arc<str>,
        pub started: SystemTime,
        pub request: Head,
        pub request_body: Body,
        pub response: Option<ResponseHead>,   // None：没有应答
        pub response_body: Body,
        pub timings: Timings,
        pub remote: Option<SocketAddr>,
        pub outcome: Outcome,
        pub rule: Option<String>,
        pub policy: Vec<String>,
        pub notes: Vec<String>,
    }
    /// `url`：第 6.1 节的 URL；`CONNECT` 是请求行里的 `host:port`（M4-D26）
    pub struct Head { pub method: Method, pub url: String, pub version: Version, pub headers: HeaderMap }
    /// `version`：客户端看到的（第 6.3 节，M4-D28）
    pub struct ResponseHead { pub status: StatusCode, pub version: Version, pub headers: HeaderMap }
    /// `size`：见过的全部字节；`data`：存下的开头
    pub struct Body { pub data: Bytes, pub size: u64, pub truncated: Option<Truncated> }
    pub enum Truncated { Limit, Budget }
    /// 都从这个请求的句柄建好时算起；`total` 量到第 6.4 节的"结束的那一刻"
    pub struct Timings {
        pub connect: Option<Duration>, pub dns: Option<Duration>, pub wait: Option<Duration>,
        pub receive: Option<Duration>, pub total: Duration, pub reused: bool,
    }
    pub enum Outcome { Completed, Upgraded, Rejected(RejectKind), Failed(String) }
    impl Body {
        /// 按这些 `Content-Encoding` 由外到里解开（`gzip`、`x-gzip`、`deflate`），解开之后至多 `limit` 字节。
        /// 截断的、有别的编码的、解不开的、超过 `limit` 的：Err 说是哪一种（第 7.5 节）
        pub fn decoded(&self, encodings: &HeaderMap, limit: u64) -> Result<Bytes, NotDecoded>;
    }
}

pub enum Routed<R, K> {
    Go { route: R, key: K, pages: Pages, rewrite: Option<Arc<Rewrite>>, capture: Option<Capture> },
    Refused { refusal: Refusal, pages: Pages, capture: Option<Capture> },
}
```

`Head` 与 `ResponseHead` 用 `http` 的类型（`rurge-http` 本来就依赖它；`rurge-api` 经 axum 的再导出用它们），头的值照原样存字节。

## 4. 抓包存储（M4a）

### 4.1 一份存储

- 一个引擎一份：`EngineShared.capture: Arc<CaptureStore>`。`EngineShared::new` 用缺省的上限建一份，可执行文件照命令行给的上限换掉它（同 `empty_group`、`owners` 由可执行文件在 `EngineShared::new` 之后改的写法，`crates/rurge/src/cli/run.rs`）。它属于引擎、不属于一代配置：重载不清、不换。只在内存里，进程退出就没了。
- 存进去的条目在一条队列里由旧到新排着，每条一个 `Arc<Entry>`，各自带着会话的名字。条目的 ID 就是这个请求的记录的 ID（`SessionHandle::id`），同一个请求在 `/v1/requests/*` 与抓包里是同一个数。
- 进行中的条目（请求还没有走完）不在队列里，API 读不到（第 6.4 节）；它们的正文已经记在账上（第 4.3 节）。

### 4.2 上限与命令行

- 两个上限：**总量**（队列里的条目加进行中的预留，按第 4.3 节的算法记的字节）与**单条正文**（一个正文至多存多少字节，请求与应答各自算）。它们是 rurge 专有的运行时选项，只走命令行参数与环境变量（FR-CFG-17），照 `--request-log-size` / `RURGE_REQUEST_LOG_SIZE` 的写法：`--capture-size <SIZE>` / `RURGE_CAPTURE_SIZE`、`--capture-body-size <SIZE>` / `RURGE_CAPTURE_BODY_SIZE`，只有 `rurge run` 有。
- 写法：十进制的字节数，后面可以跟一个不分大小写的 `k`、`m`、`g`（乘 1024、1024²、1024³）；别的写法与超出 `u64` 的，命令行报错退出（clap 的 `value_parser`，退出码同别的写错的参数）。`0` 合法：总量 0 什么也存不进（每条都丢掉，第 4.4 节）；单条 0 只存头。
- 缺省：总量 100 MiB、单条 5 MiB（照 Node.js 的 inspector；Chromium 桌面是 200 MB / 20 MB，Android 10 MB / 5 MB）。抓包缺省关着（第 5.1 节），关着时存储是一条空的队列。缺省值归所有者可以改（第 17 节）；量过的内存（第 4.5 节）照实写进 `docs/api/phase4.md`。
- 单条大于总量时降到总量。使用者写了单条（参数或环境变量）而它大于总量时，启动时一行 WARN 说降成了多少；单条是缺省值、被使用者写的总量压下去时照样降，不告警（使用者只想要一个小一点的总量）。

### 4.3 记账

- 一个条目的账：请求头与应答头的每一行按 RFC 7541 第 4.1 节记"名字 + 值 + 32"字节；URL、方法、规则、策略、备注的字节；两个正文存下的字节；再加固定的 512 字节（条目本身、队列的一格、几个 `String` 与 `Vec` 的头；第 4.5 节量过之后可以改这个数）。
- 正文**按块记账**（照 Chromium `NetworkResourcesData` 的做法）：一块数据进来时，先算这一块里还能存多少（单条正文还剩的额度），再向存储预留那么多字节；不够时先挤掉最早的条目（第 4.4 节），挤完还不够，这个正文从这一块起不再存（截断，原因是"总量"），之后的字节只计数。
- 头与别的文字在存进去时一次记账，同样先挤掉最早的；挤完还放不下，整个条目丢掉，它的正文预留还回去。
- 进行中的条目没有存进去就被放下时（第 6.4 节的哪一种都没有走到——不该发生，但不漏），它的预留还回去。
- 所以存储记着的字节在任何时刻不超过总量；没有记进去的只有进行中的条目的头（存进去时才记，个数随同时在抓的请求）与 HAR 导出握着的条目（第 8.4 节）。

### 4.4 挤掉、截断、丢掉

- **挤掉**：从队列最前面（最早存进去的）一条一条拿掉，不分会话（总设计第 9.2 节"总量超出时丢掉最早的条目"）。拿掉的条目正被读时（API 握着它的 `Arc`），内存到读完才放掉。
- **截断**：一个正文超过单条上限时只存开头的那么多字节（`Truncated::Limit`）；总量挤不出地方时从那一块起不再存（`Truncated::Budget`）。截断的正文照样记着见过的全部字节数（`size`）。
- **丢掉**：一个条目的头存不进去（总量太小，例如 `0`）时整条不要。
- 四个数，从引擎启动起，重载与清除都不清零：`stored`（存进去的条目）、`evicted`（被挤掉的）、`dropped`（丢掉的）、`truncated`（存进去时至少有一个正文被截断的条目）。

### 4.5 量内存

任务 8 照 `crates/rurge-rules/tests/memory.rs` 的做法写两条忽略的用例（`#[ignore]`，各自单独以 release 运行，进程的峰值工作集由操作系统读）：一条把存储按缺省的上限装满（条目的形状像真实的页面请求：十几个头、几 KiB 的正文，另有一些超过单条上限而截断的），一条什么也不装做基线。结果写进 `docs/api/phase4.md`：装满时比基线多出的内存与总量之比。多出的超过总量的 25% 时调第 4.3 节固定的 512 字节，使记的账接近实际；缺省的上限不因此改。

### 4.6 命名会话

- 会话就是条目上的名字：开关抓到的进 `default`，`always-capture=<名字>` 抓到的进那个名字（照写的原样，区分大小写）。一个会话在它还有条目时存在：列会话时只列有条目的，没有一条的名字对 API 来说不存在（第 7 节答 404）。
- 使用者自己写 `always-capture=default` 时，它与开关抓到的在同一个会话里（登记）。
- 挤掉不分会话；清除可以只清一个会话（第 7.6 节）。

## 5. 什么请求被抓（M4a）

### 5.1 开关

- `capture`：缺省关（总设计第 10.1 节：抓包要占内存、会存正文，要使用者自己打开）。`POST /v1/features/capture` 切换，写进 `state.json` 第 2 版的 `features.capture`（M2 已经留好三态），启动时还原（`restore_switches`）。`GET` 读真实状态。日志一行 INFO：`capture switched via http-api`（同 `mitm`、`rewrite`）。
- 每个请求在求规则时读一次开关（第 5.3 节）：切换对此后求规则的请求生效，已经在途的照它求规则时的状态。关掉之后已经存进去的保留，直到被挤掉、被清除或进程退出（总设计第 9.2 节）。

### 5.2 `always-capture`

- rurge 读的是 `[Rule]` 里一行顶层规则（含 `FINAL`、`RULE-SET`、`DOMAIN-SET` 与逻辑规则）的参数，`RuleParams.always_capture`（阶段 1 起就解析了，至今没人读）。一个经 HTTP 引擎的请求求规则时命中的那一行带着它，这个请求就进那个名字的会话——开关关着也一样（总设计第 1.4 节 M4 一行的验收："`always-capture` 在开关关着时照样抓"）。命中集合时（集里的哪一行命中都一样）用集合那一行的参数；规则集文件里的行上写的 `always-capture` 不读（登记）。
- 只在求规则时：出站模式是直连或全局代理、或会话属于 `external` 程序时，`choose_policy_with` 不求规则就定了策略，没有命中的那一行，`always-capture` 不起作用；开关照样起作用。
- `always-capture=` 的值去掉首尾空白之后是空的：`W0012`（`` `always-capture` needs a capture session name; the parameter is ignored ``），规则照常，这一行不抓。不带 `=` 的 `always-capture` 照旧是未知参数（`W0003`）。
- **不让连接进 HTTP 引擎、不让 TLS 被解密**（M4-D8）：手册说的是"HTTP capture"，HTTP 概览页说进引擎的请求才在抓包视图里；Surge 会不会因为 `always-capture` 把一条连接强行交给 HTTP 引擎，手册没有写。rurge 的读法：只有本来就经 HTTP 引擎的请求被抓（明文的 HTTP、`[MITM]` 解密的、`force-http-engine-hosts` 留在引擎里的）；要抓原样转发的连接，使用者另写 `force-http-engine-hosts` 或 `[MITM]`。登记在清单 L271 的备注里，所有者可以改。

### 5.3 在哪里定

- `choose_policy_with` 在填 `handle.set_rule(...)` 的同一处，命中的那一行带着非空的 `always-capture` 时填抓包会话的槽（`set_capture_session`）。槽在 `SessionHandle` 上，与 `method`、`status` 并排；不是 HTTP 请求的会话也可能被填，没有人读。
- `ConnUpstreams::route`（每个经引擎的请求都经过它，被拒的也是）在两个分支里都给 `capture`：槽里有名字就用它；没有、而开关开着是 `default`；都没有是 `None`。存储是 `EngineShared.capture`。
- 选定 REJECT 一族的请求（M3-D27：拨号时才被拒）照样带着 `capture`，抓到的是它被拒绝时的样子（第 6.1、6.3 节）。

### 5.4 抓不到的

- 没有走到 `route` 的：HTTP/2 上的 `CONNECT`（答 501；它有自己的记录，但不经 `route`，M2b 计划 P6）；hyper 自己答 400 的坏请求、请求头超时、只发了半个头（那时还没有这个请求的句柄，只有承载连接补的一条记录，M1 设计第 10.2 节）；经 HTTP 监听器来的、不是 `http://` 绝对形式的请求（`handle` 在建句柄之前就答 400）；`route` 还没有回来、hyper 就丢下了的请求。
- 承载请求的连接自己的记录（握手失败、握手之后不发请求，M1 设计第 10.2 节）：那不是请求。
- 不经 HTTP 引擎的：原样转发的 TCP（嗅探判不出、等到时限、`always-raw-tcp-*`、端口不可嗅探）、不解密的 TLS、UDP 与 QUIC、DNS。

## 6. 抓到什么（M4a）

### 6.1 请求

- **两个取样点**（M4-D11）：
  1. **离开 rurge 时**：要发往上游的请求，在第 5 步 `headers::strip` 之后、`absolute_form` / `origin_form`、`fit` 与 `write_host` 之前取一份。头是 Header Rewrite 之后、去掉逐跳头之后的；URL 是 URL Rewrite 之后的（`header` 模式换过目标的是新的 URL）；版本是客户端的版本（HTTP/2 的请求记作 HTTP/2，不记转成 HTTP/1.1 之后的样子）。
  2. **求规则时**：没有发往上游的请求（被拒绝、URL Rewrite 当场答复、Map Local、Header Rewrite 改了框架头而失败（M3 设计第 7.3 节）、请求正文超过 Body Rewrite 的上限而断开（M3 设计第 9.3 节）、拨号失败、终止……）记的是它求规则时的样子：`route` 回来之后马上取一份，URL 是记录里的 URL（客户端问的），头照 `headers::strip` 去掉逐跳的头（`Keep` 照这个请求的 `wants_upgrade` 与"是不是嗅探到的连接"）。走到第 1 个取样点的请求以第 1 份为准。
- `CONNECT`（嗅探到的连接上客户端对它自己的代理开的隧道）没有 URL：`prepare` 给它 `url = None`，记录里的 `url` 是 `null`。条目的 `url` 写请求行里的 `host:port`（照 mitmproxy 对这种请求行的写法，M4-D26），API 与 HAR 同（登记）。
- 两份都不含 rurge 自己的凭据：rurge 对上游 HTTP 代理的 `Proxy-Authorization` 由 `absolute_form` 加在取样之后；客户端交给 rurge 监听器的 `Proxy-Authorization`（`password@` 写的口令）在取样之前被 `strip` 去掉（经 HTTP 监听器来的请求不保留它）。嗅探到的连接上客户端给它自己的代理的 `Proxy-Authorization` 照 M1 保留着，抓到了，输出时遮住（第 6.6 节）。
- 头的名字是小写的（hyper 读不到原来的大小写，总设计第 6.9 节）；次序是 `http::HeaderMap` 给的（同名的行挨着，不一定是线上的次序，登记）；值照原样存字节。

### 6.2 请求的正文

- 在 `Idle`（发往上游时包住请求正文的那一层）里抄：抄的是发往上游的字节，Body Rewrite 改过的是改过之后的。数据帧抄，trailer 不抄。
- 没有发往上游的请求没有请求正文：rurge 没有读它（Map Local 不读请求正文，M3-D23），或读进来之后没有发（Body Rewrite 缓冲着的请求正文超过上限、停住或读失败，登记）。请求一侧的 Header Rewrite 在缓冲之前（`engine.rs` 里它在读正文那一步的前面），改了框架头的请求一个正文字节也没有读。
- 一个请求重发（M1 设计第 8.6 节）不会把正文抄两遍：没有写出去的整个交回、hyper 没有读过正文；换连接再发的只有没有正文的幂等请求。

### 6.3 应答

- 抄的是交给客户端的应答：`Requests::handle` 返回的应答，在交给 hyper 之前包一层（M4-D12）。状态码、版本、头（应答一侧的 Header Rewrite 与 Body Rewrite 之后、去掉逐跳头之后），正文的数据帧；trailer 不抄。rurge 自己答复的——错误页、拒绝页、`302` / `307`、Map Local——与上游来的一样记。
- 包的这一层不改变应答：`size_hint` 与 `is_end_stream` 原样转给里面的正文（hyper 靠它们定 `content-length`、定还要不要读，见 `Tail::new` 的注释），每一帧原样交出。
- hyper 写出时自己加的头（`date`，HTTP/1.1 的 `content-length` / `transfer-encoding` 等）不在抓到的头里（登记）。
- 应答的版本记客户端看到的（M4-D28）：讲 HTTP/2 的客户端连接上一律是 HTTP/2（hyper 的 HTTP/2 服务端不看应答的 `version()`）；HTTP/1.x 的客户端连接上是交给 hyper 的应答的 `version()`——hyper 照它写状态行（`proto/h1/role.rs`），上游的 HTTP/2 应答在交给 HTTP/1.1 的客户端之前已经改成 HTTP/1.1（M2b 计划 P8）。rurge 自己的答复照同样的规则记。
- 没有应答的——rurge 关掉连接作答（`REJECT` 不给页时、超时、终止、客户端走了）——记"没有应答"。

### 6.4 存进去的时刻

- 一个请求的条目在它的 HTTP 部分结束时存进去（M4-D13）：
  1. **升级与隧道**（应答是 101，或嗅探到的连接上的 `CONNECT` 得到 2xx）：交出应答头的时候。之后的字节不抄，记录照旧到隧道结束；条目的结局是 `upgraded`。
  2. **其余有应答的**：应答正文交完或被 hyper 丢下、**而且**这个请求的记录已经结束，两者后到的那一刻。包住正文的那一层被丢下时，先丢下里面的正文（`Tail`、`LocalFile` 与守卫在那时结束记录），再以 `SessionHandle::when_finished` 等记录结束。
  3. **没有应答的**（`handle` 返回 `Err(Close)`，或 hyper 在应答之前丢下了这个请求——HTTP/2 上客户端重置了流）：知道没有应答、而且记录已经结束，两者后到的那一刻。`REJECT-DROP` 先结束记录、再停留 `drop_hold`（缺省 30 秒）才 `Err(Close)`：它在停留之前就告诉 `CaptureSlot` 这个请求没有应答，条目在记录结束时就存进去，不等停留完（M4-D29）。
- **结束的那一刻**（条目的 `total` 与 `receive` 量到这里，M4-D29）：有应答的是"记录结束"与"应答正文交完或被丢下"两者后到的那一刻；升级与隧道是交出应答头的那一刻；没有应答的是记录结束的那一刻（`when_finished` 的回调里记下）。所以 `REJECT-DROP` 的 `total` 与记录一样只有几毫秒，不含停留。
- 为什么等两件事都到：`Tail` 读到上游的最后一帧时先 `complete()`（结束记录）、再把那一帧交出（`engine.rs` 里 `PolledUpstream::Frame` 那一支），记录先于最后一帧结束；而 hyper 在 `is_end_stream` 为真时不再读、直接丢下正文。"记录结束"与"正文交完"谁先谁后都有，取后到的那一刻才既有全部正文、又有最终的结局与耗时。
- `SessionHandle::when_finished`（新）：今天的 `on_finish` 在记录已经结束之后加的钩子永远不会被调（`finish` 只取一次钩子），所以要一个"已经结束就当场调"的版本。它与 `finish` 并发时不漏、不调两次，单元用例钉住（任务 1）。
- 每个请求一个 `CaptureSlot`，在 `Requests::handle` 的最外面建、交给请求的流程：握着 `Capture`、请求的句柄与正在建的条目（请求正文的那一层与应答正文的那一层共用的一份）。存进去的那一刻把条目从里面拿走，之后再来的字节不再抄。`handle` 的 future 被丢下时，`CaptureSlot` 的 `Drop` 做第 3 种。

### 6.5 耗时、远端地址、结局与备注

存进去的那一刻从请求的句柄读（M4-D14）：

- `started`：这个请求的句柄建好时的墙上时间。
- `connect`：从句柄建好到上游连接可用（`mark_connected`；借来的连接是借到的那一刻）；`dns`：`Trail` 记的解析时间；`wait`：从连接可用到第一个上游字节（HTTP/2 的上游记的是应答头到达的那一刻，M2b 计划 P11）；`receive`：从第一个上游字节到结束的那一刻；`total`：从句柄建好到结束的那一刻（第 6.4 节）。都从句柄建好时算起（`connect_time()` 的起点），所以连上了、也收到了上游字节的 `connect + wait + receive` 恰好等于 `total`。拿不到的是 `None`。`reused`：这个请求借了池里的或共享的连接（第 8.3 节靠它分 `blocked` 与 `connect`）。
- `remote`：`Trail` 的对端，就是记录的 `remoteAddr`（借来的连接是拨它时记下的）。
- 结局：`Completed` / `Rejected(kind)` / `Failed(消息)` 取自记录的结局；升级与隧道是 `Upgraded`。
- 规则、策略（链）与备注：记录的 `rule`、`policy`、`notes`。M3 的重写备注在这里（清单 L569："引擎处理的请求在抓包视图中逐条展示，含重写与脚本备注"；脚本在阶段 5）。

### 6.6 凭据

- 抓到的是用户流量的内容，不是 rurge 持有的凭据；它与 `CLAUDE.md`「凭据」一条怎样相处是所有者的 Q1。在所有者回答之前照总设计第 9.2 节的临时做法（M4-D15）：`authorization`、`proxy-authorization`、`cookie`、`set-cookie` 四个头的值在 API 输出与 HAR 里写 `***`，存储里保留原值；正文与 URL（含查询串）照给。
- rurge 自己的凭据从不进抓包（第 6.1 节）；两种各一条用例：经监听器来、带着 `password@` 口令的请求，与经带凭据的上游 HTTP 代理转发的请求，抓到的头里都没有 `proxy-authorization`。
- `Entry` 与它的部件不实现 `Debug`（它们装着用户的头与正文）；日志里没有抓到的任何内容（总设计 P4-D25）。

## 7. API（M4a）

都在 rurge 专有的前缀 `/v1/rurge/…` 下（Surge 没有读抓包内容的端点）。鉴权、错误体（`{"error":"<message>"}`）与无内容的成功（`{}`）沿用阶段 1 的约定。时间是 Unix 毫秒，字节数是数字。

### 7.1 `GET /v1/rurge/capture`

```json
{"limits":{"total":104857600,"body":5242880},"used":0,"entries":0,"stored":0,"evicted":0,"dropped":0,"truncated":0}
```

`used`：此刻记着的字节（队列里的条目加进行中的预留，第 4.3 节）；`entries`：队列里的条目数；后四个数见第 4.4 节。开关的状态读 `/v1/features/capture`。

### 7.2 `GET /v1/rurge/capture/sessions`

```json
[{"name":"default","entries":12,"bytes":34567}]
```

只有有条目的会话，按名字排；`bytes` 是这个会话的条目记着的字节。

### 7.3 `GET /v1/rurge/capture/sessions/{name}`

这个会话的条目的摘要，由新到旧；`limit` 同 `/v1/requests/recent`（缺省 100，`0` 答 400）。没有这个会话：404 ``{"error":"unknown capture session `<name>`"}``。

```json
[{"id":42,"startedMs":1761984000000,"method":"GET","url":"https://example.com/a?b=1","status":200,"outcome":"completed","requestBodySize":0,"responseBodySize":1234,"truncated":false,"totalMs":35}]
```

`url` 见第 6.1 节（`CONNECT` 是 `host:port`）；`status` 是应答的状态码，没有应答时 `null`；`outcome` ∈ `completed` `upgraded` `rejected` `failed`；`truncated`：至少一个正文被截断。

### 7.4 `GET /v1/rurge/capture/sessions/{name}/har`

这个会话导出的 HAR 1.2（第 8 节），`Content-Type: application/json`，条目由旧到新。没有这个会话：同上的 404。

### 7.5 `GET /v1/rurge/capture/entries/{id}`

一条的全部。`id` 不是数字答 400，没有这一条答 404。

```json
{"id":42,"session":"default","startedMs":1761984000000,
 "request":{"method":"GET","url":"https://example.com/a?b=1","version":"HTTP/1.1",
            "headers":[{"name":"host","value":"example.com"},{"name":"cookie","value":"***"}],
            "body":{"size":0,"stored":0,"truncated":null,"base64":""}},
 "response":{"status":200,"version":"HTTP/1.1","headers":[{"name":"content-encoding","value":"gzip"}],
             "body":{"size":1234,"stored":1234,"truncated":null,"base64":"H4sI…"}},
 "timings":{"connectMs":3,"dnsMs":null,"waitMs":20,"receiveMs":12,"totalMs":35,"reused":false},
 "remoteAddr":"127.0.0.1:8080","outcome":"completed","rejectKind":null,"error":null,
 "rule":"DOMAIN,example.com,DIRECT","policy":["DIRECT"],"notes":[]}
```

- `truncated`：`null`、`"limit"` 或 `"budget"`；`base64` 是存下的字节。头的值不是 UTF-8 的按 U+FFFD 替换（存储里照原样）；四个头遮住（第 6.6 节）。没有应答时 `response` 是 `null`。
- `?decode=true`（`decode` 只认 `true` 与 `false`，缺省 `false`，别的值答 400）：两个正文各自照自己那一侧的 `content-encoding` 由外到里解开（`gzip`、`x-gzip`、`deflate`，`Body::decoded`；解开之后至多单条正文的上限）。每个 `body` 多两个键：`decoded`（是否解开了）与 `notDecoded`（没解开的原因：截断的、有别的编码的——`br`、`zstd` 等总设计 Q4——、解不开的、解开之后超过上限的；解开了是 `null`）；解开了的 `base64` 是解开之后的字节。没有编码的正文 `decoded` 为 `false`、`notDecoded` 为 `null`。

### 7.6 `POST /v1/rurge/capture/clear`

请求体 `{}` 清除全部，`{"session":"<名字>"}` 只清这个会话（没有这个会话时 `{"cleared":0}`，不答 404）；没有请求体同 `{}`。应答 `{"cleared":<条数>}`。进行中的条目不受影响（之后照常存进去）；四个数不清零。

### 7.7 `/v1/features/capture`

`GET` 读真实状态、`POST` 切换（第 5.1 节）；形状同 `mitm`、`rewrite`。

## 8. HAR（M4a）

### 8.1 `log`

`{"log":{"version":"1.2","creator":{"name":"rurge","version":"<CARGO_PKG_VERSION>"},"pages":[],"entries":[…]}}`；`entries` 由旧到新。

### 8.2 一条

| HAR | 取自 |
| --- | ---- |
| `startedDateTime` | `started`，RFC 3339、毫秒、UTC |
| `time` | `timings` 里不是 `-1` 的各项之和（第 8.3 节） |
| `request.method` / `url` / `httpVersion` | 第 6.1 节（`CONNECT` 的 `url` 是 `host:port`，M4-D26）；版本写 `http::Version` 的写法（`HTTP/1.1`、`HTTP/2.0`） |
| `request.cookies` / `response.cookies` | `[]`：不拆（`cookie` 与 `set-cookie` 遮住了，拆开会漏出它们） |
| `request.headers` / `response.headers` | 头，四个遮住；没有应答时 `[]` |
| `request.queryString` | URL 的查询串拆成名与值（`url` 的 `query_pairs`）；URL 读不了时 `[]` |
| `request.postData` | 请求正文见过字节时才有：`mimeType` 取 `content-type`（没有时空串）；正文没有截断、是 UTF-8、没有 `content-encoding` 时 `text` 是它，否则 `text` 是 Base64、另写 `"_encoding":"base64"`（HAR 1.2 的 `postData` 没有 `encoding`）；截断的写 `comment` |
| `request.headersSize` / `bodySize` | `-1` / 请求正文见过的字节数 |
| `response.status` / `statusText` / `httpVersion` | 状态码 / 标准的原因短语（`StatusCode::canonical_reason`，没有时空串）/ 版本；没有应答时 `0`、`""`、`""` |
| `response.content` | 能解开的（第 7.5 节）：`size` 是解开之后的字节数，`compression` 是 `size − bodySize`，`text` 是解开之后的；没有编码的：`size` 是存下的字节数，`text` 是存下的；不能解开的：`size` 是存下的字节数，`text` 是存下的（压缩着的）字节，`comment` 说原因。`text` 是 UTF-8 时照写，否则 Base64 加 `"encoding":"base64"`；截断的在 `comment` 里写"存下 N 字节，共 M 字节"；`mimeType` 取 `content-type` |
| `response.redirectURL` | `location` 头，没有时空串 |
| `response.headersSize` / `bodySize` | `-1` / 应答正文见过的字节数；没有应答时 `-1` |
| `cache` | `{}` |
| `timings` | 第 8.3 节 |
| `serverIPAddress` | `remoteAddr` 的 IP；没有时不写这个键 |
| `_rurgeId` / `_session` / `_outcome` / `_rejectKind` / `_error` / `_notes` | 记录的 ID、会话、结局、拒绝的种类与失败的说明（没有时不写）、备注 |

没有应答的条目照 mitmproxy 的写法：`response.status` 为 `0`，失败的说明在 `_error`。

### 8.3 `timings`

`Trail` 记的 `dns` 是求规则时（IP 规则要解析）与拨号时的查找的合计（缓存命中也记），两者都在连接可用之前；查找可能并行，合计可能超过墙上的时间。所以先把它压到它所在的那一段之内：连上了的 `d = min(dns, connect)`，没连上的 `d = min(dns, total)`。各项（M4-D27）：

- `blocked`：借了连接的（`reused`）是 `connect − d`（从句柄建好到借到，去掉求规则时的查找）；没有借的 `-1`。
- `dns`：`d`；没有时 `-1`。
- `connect`：没有借连接、连上了的是 `connect − d`（含求规则、重写的几步与拨号）；借了的 `-1`；没有连上的 `-1`。
- `ssl`：`-1`（握手的时间在 `connect` 里，不单列）。
- `send`：`0`（rurge 不单独量发请求的时间，它在 `wait` 里）。
- `wait`：`wait`；连上了而没有第一个字节的是 `total − connect`；没有连上的（拒绝、rurge 自己答复、拨号失败）是 `total − d`。
- `receive`：`receive`；没有时 `0`。
- `time`：上面不是 `-1` 的各项之和（HAR 1.2："This is the sum of all timings available in the timings object"）。HAR 1.2 要求 `send`、`wait`、`receive` 不为负。
- 都是整数毫秒，各项先向下取整再相加（同记录的 `connectMs` 等），所以 `time` 恰好等于各项之和。

这样每一种情形下各项之和在取整之前都恰好是 `total`：新拨了连接的 `d + (connect − d) + wait + receive`，借了连接的 `(connect − d) + d + wait + receive`，没连上的 `d + (total − d)`（连上了而没有第一个字节的，`wait` 是 `total − connect`、`receive` 是 0，同样）。取整之后 `totalMs − time` 在 0 到 3 之间（至多四项各舍去不到 1 毫秒）。

### 8.4 写出

- 逐条写出：取会话的条目（一份 `Arc` 的快照）之后一条一条序列化交给连接，不先在内存里拼出整个文件（一个会话可能有上百 MiB 的正文，Base64 之后更大）。
- 快照握着的条目在导出期间被挤掉或清除时，内存到导出写完才放掉（第 4.3 节的例外，登记）。

## 9. 日志与诊断（M4a）

- 抓包只多两行日志：开关切换的 INFO（第 5.1 节），单条上限被降到总量的启动 WARN（第 4.2 节）。条目的内容不进任何日志。
- 诊断：空的 `always-capture=` 给 `W0012`（第 5.2 节）；不设新的诊断码（`W0012` 在 `codes::hint` 的表里已经是"每处自带"）。

## 10. 请求记录的完整（M4b）

### 10.1 新字段

`/v1/requests/recent` 与 `active` 的每条记录多三个键（M4-D21）：

- `receiveMs`（紧跟 `dnsMs`）：从收到第一个上游字节到会话结束的毫秒数；会话还没有结束、或从没有收到过上游的字节时是 `null`。它与 `connectMs`、`firstByteMs` 合起来就是总设计第 9.1 节的"取连接、等应答头、收正文三段"。每种会话都有，不只是 HTTP 请求。
- `requestBodyBytes`、`responseBodyBytes`（紧跟 `statusCode`）：一个 HTTP 请求的请求正文发往上游的字节数、应答正文交给客户端的字节数；不是 HTTP 请求的会话是 `null`。`up` / `down` 照旧是上游一侧的全部字节（含头，M1-D23）。
- **在哪里数**（M4-D30）：记录在句柄 `finish` 的那一刻拍下（`observe.rs` 的 `record_finished` → `record_of`），之后句柄上的计数进不了记录；而 `Tail` 与 `LocalFile` 先 `complete()`、再交出最后一帧，rurge 自己的答复（错误页、拒绝页、`302` / `307`、tiny-gif）在返回应答之前就 `finish` 了。所以不能在 M4a 包住应答的那一层数（那里总漏掉最后一帧、自己的答复总是 0），而是：应答正文在 `Tail` 与 `LocalFile` 交出每一帧时、`complete()` 之前数（`Tail` 已有的每个请求的 `body: Meter` 就在那里计数，改成两种版本都计）；rurge 自己的答复在建应答时、`finish` 之前记下正文的长度；请求正文在 `Idle` 交出每一帧时数。记录拍下的时刻不变。请求正文在应答结束之后还在上传的（源站提前作答），记录里是到结束那一刻为止的字节数（登记）。

### 10.2 直通的原因

可嗅探的会话没有进 HTTP 引擎、或判为 TLS 而 `[MITM]` 的名单命中却没有解密时，记录的备注写一句（M4-D22；英文，同已有的备注）：

| 情形 | 备注 |
| ---- | ---- |
| `always-raw-tcp-hosts` 命中 | `Passed through: always-raw-tcp-hosts` |
| `always-raw-tcp-keywords` 命中 | `Passed through: always-raw-tcp-keywords` |
| 嗅探判为既不是 HTTP 也不是 TLS（`force-http-engine-hosts` 没有把它留在引擎里） | `Passed through: neither HTTP nor TLS` |
| 嗅探等到了时限 | `Passed through: sniffing timed out` |
| 名单命中、`mitm` 开关关着 | `Not decrypted: MITM is off` |
| 名单命中、这一代没有可用的 CA | `Not decrypted: no usable CA` |
| 名单命中、客户端给的协议（ALPN）rurge 都不解密 | `Not decrypted: the client offers no protocol rurge decrypts` |
| 名单命中、来源不在 `client-source-address` 里 | `Not decrypted: the client is not in client-source-address` |
| 名单命中、名字是 rustls 不收的、签不了证书（`Mitm::decides` 的第三种 `Pass`） | `Not decrypted: no certificate can be made for this name` |

- **原因照代码的判断次序取最先成立的那一条**（M4-D31）：`sniffable_session` 先看 `always-raw-tcp-hosts`、`always-raw-tcp-keywords`，再看端口，所以不可嗅探的端口上命中 `always-raw-tcp-*` 的会话也写那一句；最先成立的是"端口不可嗅探"的不写。暂停、fake 地址背后没有名字、被预匹配拒绝的会话本来就被拒绝或失败、各有自己的说明，也不写。
- TLS 的五种只在名单命中时写：名单没有命中的 TLS 原样转发是 TLS 的常态，不写——来源不允许而名单也没有命中的同样不写。`Mitm::decides` 今天先看来源、再看名单，来源不允许时不看名单，要让它在那时也说出名单命中没有；名单命中之后照它的次序（开关、CA、来源、名字、ALPN 由计划对着 `decrypting` 与 `decides` 核定，V14）取最先成立的那一条。
- 嗅探之前决定的（`always-raw-tcp-*`）与之后决定的，都在拨号建好这条会话的句柄时写进去：原因随会话交给拨号（M4b 的计划定怎样带）。
- `Mitm::decides` 今天把"来源不允许"、"名单没有命中"与"名字签不了证书"三种都答成 `Decision::Pass`（`crates/rurge-http/src/mitm/decrypt.rs` 的 `decides`），要分开；`decrypting` 在开关关着时今天不看名单，要看一次。

### 10.3 `remoteAddr`

- **新口径**（M4-D23）：这条会话离开 rurge 时所经的那条连接或载体的远端——会话自己开的、从池里借来的、与别的会话共用的都算；链式策略照旧是第一跳。阶段 1 的口径是"会话**自己**开的那条 TCP 连接"，复用、隧道与 UDP 一律 `null`（`docs/api/phase1.md` 的 `remoteAddr` 一段，阶段 1 / M5a 延后第 3、20、21 行）；M4b 起照新口径，`docs/api/phase4.md` 写订正的那一句，历史段落不改。
- 补全的（都有本机的进程内服务端或假的对端可测：`rurge-engine/tests/` 下的 `outbounds_snell.rs`、`outbounds_http2.rs`、`outbounds_ssh.rs`、`outbounds_hysteria2.rs`、`outbounds_tuic.rs`、`outbounds_masque.rs`、`outbounds_wireguard.rs` 等）：

| 载体 | 今天 | M4b |
| ---- | ---- | --- |
| snell 池里取出的连接（`rurge-proto/src/snell/pool.rs`） | `null` | 池里的连接带着拨它时的对端，借出时写进会话的 `Trail`。取出的连接已失效、转发时换新连接重发的（`snell/tunnel.rs`）：重拨在会话的 `follow` 之外，要把会话的 `Trail` 交给隧道、重拨之后写新的对端（延后第 20 行） |
| anytls 池里的会话（`anytls/pool.rs`） | `null` | 同上，会话带着对端 |
| `h2-connect` 与 HTTP/2 的 `trust-tunnel` 共用的连接（`h2pool`） | 只有拨了新连接的那一条会话有 | 连接带着对端，每个借用它的流写进自己的会话 |
| ssh 的会话（`rurge-proto-ssh` 的 `outbound.rs`） | 只有建立（或重建）那条 SSH 会话的有 | 会话带着对端 |
| QUIC 一族（`hysteria2`、`tuic` / `tuic-v5`、`masque`、`trust-tunnel` 的 `h3=true`） | `null`（延后第 21 行：经底层代理时只有碰巧打开 QUIC 连接的那一条有） | 直连的是这条 QUIC 连接此刻的远端（quinn 的 `remote_address()`；端口跳跃时是此刻的那个端口）；经底层代理的是承载的远端（第一跳），**不是**替身地址 `192.0.2.1:443`（`rurge-net/src/quic/carrier.rs`） |
| `wireguard` | `null` | 覆盖这个目标的那个对端的端点：它的载体数据报的远端（经底层代理的是承载的远端） |
| UDP 流 | 一律 `null` | 载体只有一个远端的（`ss`、`socks5` 的中继、`trojan`、`snell`、`anytls`、QUIC 一族、`wireguard`）是那个远端；DIRECT 的是这条流第一个数据报发往的地址；`external` 是本机的中继 `127.0.0.1:<端口>` |

- **照旧是 `null` 的**（登记）：`vmess`、`h2-connect`、MASQUE 的 UDP——它们按目标在第一次发送时才在另一个任务里开连接（`rurge-net/src/packet_datagram.rs`），那时会话的 `Trail` 不在；被拒绝的会话；rurge 不实现的协议。
- 记录读 `Trail` 的对端时不再分 TCP 与 UDP（`observe.rs` 今天对 UDP 一律给 `None`）。在 `follow` 之外要写对端的（snell 的重拨、DIRECT 的 UDP 流），握一份会话的 `Trail`（`Trail` 的克隆共享同一份记录）、用已有的 `set_peer` 写；怎样交给隧道与 UDP 的流由 M4b 的计划定。

### 10.4 `dnsMs` 在解析途中

- 今天：一次查找结束时（`rurge-dns` 的 `Noted` 被丢下）才把它花的时间加进 `Trail`，卡在一次慢解析里的进行中会话显示 `null`（延后第 22 行）。
- M4b（M4-D24）：`Trail` 另记正在进行的查找各自的开始时刻；`dns()` 给已结束的合计加上进行中的到此刻为止的时间。`Noted` 改成"开始时登记、结束时把花的时间加上并注销"的票据。进行中的会话的 `dnsMs` 随时间增长，结束之后照旧是合计。

### 10.5 记录的容量

- 量一次一条 HTTP 请求的记录占多少内存（一条忽略的用例，同第 4.5 节的做法），写进 `docs/api/phase4.md`。缺省的 `--request-log-size` 1000 不改（M4-D25）：它只决定能回看多少条，要更长的使用者自己设；量到的数交给所有者决定要不要改缺省。

## 11. 行为的变化（登记）

1. 抓包开着时，经引擎的请求多一份内存里的副本与按块的记账；关着时引擎照旧（`capture` 是 `None`，不包应答正文；M4b 数正文字节数不靠这一层，第 10.1 节）。
2. `always-capture` 不让连接进 HTTP 引擎、不让 TLS 被解密（M4-D8）；只在求规则时起作用。
3. 抓到的请求头去掉了逐跳的头与 rurge 自己的转发头；抓到的应答头没有 hyper 写出时加的头；头的次序是 `HeaderMap` 的。
4. `default` 会话与使用者写的 `always-capture=default` 是同一个。
5. HAR 导出期间被挤掉的条目，内存到导出写完才放掉；进行中的条目的头不记账。
6. （M4b）`remoteAddr` 的口径改了（第 10.3 节）：复用、共用的连接与 UDP 流从 `null` 变成有地址。
7. （M4b）进行中的会话的 `dnsMs` 从 `null` 变成随时间增长的数。
8. （M4b）记录多三个键，备注多九种。
9. 抓到的 `CONNECT` 的 `url` 是 `host:port`，而记录里它的 `url` 是 `null`（M4-D26）。
10. 抓到的应答的版本是客户端看到的，可能与上游的不同（M4-D28）。
11. （M4b）请求正文在应答结束之后还在上传的，记录的 `requestBodyBytes` 只到记录结束的那一刻（M4-D30）。

## 12. 测试

1. **单元**（M4a）：`capture` 模块——记账的算法、跨会话的挤掉、单条与总量两种截断、丢掉、清除、四个数、按 ID 找、`Limits` 的降低、读出时的解开（多层、截断的、别的编码、解开之后超过上限）；`when_finished`——之前、之后、与 `finish` 在别的线程上并发（循环一万次以上：不漏、不调两次）；大小的写法。
2. **引擎**（M4a，`rurge-http` 引擎用例里的测试 `Upstreams`）：两个取样点各自的样子；`absolute_form` 加的头不在；最后一帧在（`Tail` 先结束记录的那种次序）；包了一层之后 `content-length` 不变；客户端在正文中途走掉——结局失败、正文是走掉之前的；101 在应答头时存进去；嗅探到的连接上的 `CONNECT` 得到 2xx——在应答头时存进去、`url` 是 `host:port`；`Err(Close)` 的条目；`REJECT-DROP` 的条目在记录结束时就存进去（`drop_hold` 设成远大于用例的等待，条目在停留结束之前就能读到）、`total` 不含停留；HTTP/2 上客户端在应答之前重置流；讲 HTTP/2 的客户端、上游是 HTTP/1.1 时应答记 HTTP/2；改写过的请求正文；trailer 不抄。
3. **端到端**（M4a，`rurge-engine` 的 `tests/http.rs` 与 `tests/mitm.rs`，回环上的 `TestServer`）：开关关着什么也不存；开着进 `default`；`always-capture` 在开关关着时；`FINAL` 上的；被 `REJECT` 规则拒绝的；直连模式下不起作用；带 `password@` 口令的监听器请求与经带凭据的上游 HTTP 代理的请求，抓到的头里都没有 `proxy-authorization`；`[MITM]` 解密的 HTTPS（`TlsFixture`，两张 CA 不同名）；重载之后条目还在；开关进 `state.json`。另有一条命令行的（`crates/rurge/tests/cli.rs` 的 `rurge_process()`）：带 `--capture-size` 起守护进程、`POST` 打开开关、经 HTTP 监听器问回环的源站、取 HAR 并解析。
4. **API**（M4a，`rurge-api` 的用例）：六个端点的形状、404 与 400、遮住、清除、`decode`；HAR 用 `serde_json` 解析、必有的字段都在、`time` 等于各项之和、`totalMs − time` 在 0 到 3 之间（第 8.3 节的三种情形各一条：新拨的、借来的、没连上的，各带一次求规则时的解析）、Base64 解回原字节、由旧到新。
5. **量内存**（M4a）：第 4.5 节的两条忽略的用例。
6. **M4b**：照它的计划；九种备注各一条，另有"来源不允许而名单没有命中的不写"与"不可嗅探的端口上命中 `always-raw-tcp-hosts` 的照写"各一条；第 10.3 节的表每一行至少一条；`dnsMs` 用一个慢的解析器看进行中的会话；`responseBodyBytes` 在只有一帧的应答、错误页与 Map Local 上都是正文的长度。
7. 测试约定照 `CLAUDE.md` 与 `docs/testing.md`（回环、端口 0、`LogCapture`、等待的写法）；新写的计时用例提交前单独循环跑二十次以上。

## 13. 文档

| 文档 | 改动 | 时机 |
| ---- | ---- | ---- |
| `docs/api/phase4.md` | 「M4a　抓包」一节：开关、六个端点、HAR、上限与量到的内存、登记的几件；「M4b」一节：三个新字段、九种备注、`remoteAddr` 的新口径（订正 `docs/api/phase1.md` 那一段的句子）、`dnsMs`、量到的记录大小 | 各自的收尾 |
| `docs/surge-compatibility-matrix.md` | L271（`always-capture`，备注写 M4-D8）、L569（抓包视图：API 有了、Dashboard 在阶段 6）、L863（`rurge run` 的两个参数）、L871（`capture` 生效）、L884（记录的新字段与 `remoteAddr`，M4b）；开头"最近一次"的括号 | 各自的收尾 |
| `docs/status.md` | 阶段 4 / M4a、M4b 两节 | 各自的收尾 |
| `README.md` / `README_en.md` | 特性表与状态 | 各自的收尾 |
| `docs/acceptance/phase4-manual.md` | 加 M4 的几项：真实浏览器的流量开着抓包、导出的 HAR 能被 Chrome 与 Firefox 的开发者工具导入、`always-capture` 在开关关着时、截断与挤掉在真实的大下载上 | 各自的收尾 |
| `docs/testing.md` | 量内存的做法若与 `memory.rs` 的不同，写一条 | M4a |
| `docs/README.md` | 本文与两份计划各一项 | 随各自 |
| `CLAUDE.md` | 「现在在哪里」 | 本文、两份计划、各自的收尾 |

## 14. 写计划时要核对的事项

| 编号 | 核对什么 |
| ---- | -------- |
| V1 | `rurge-http` 的 `Requests::handle`：第 0 ～ 5 步与之后的发送、重发（`Sent::NotWritten`、`head_of`）、`promote`、`answer`、`failed`、`page`、URL Rewrite 当场答复的那一支（`UrlOutcome::Redirect`）、`map_local`、`timed_out`、`upstream_failed` 各在哪里，哪些在第 5 步的 `strip` 之前返回；`headers::strip` 与 `Keep`；`absolute_form` 加了哪些头；`prepare` 什么时候没有 `url`；`Idle` 的字段与 `poll_frame`，`Idle` 与 `Tail` 的 `request_meter`（M2b 计划 P11）；`rewrite` 模块的 `ReadAhead`；借连接的两处（`borrow_h2`、`pool.take`）与 `mark_connected` |
| V2 | `SessionHandle` 的 `on_finish` / `finish` 的锁与次序（`finish` 先置 `finished`、再记 `outcome`、最后取钩子：`when_finished` 握着钩子的锁看 `outcome`，有就当场调，没有就排进去，同 `on_first_byte` 的写法）；`Guard` 什么时候结束记录；`Tail` 与 `LocalFile` 的 `Drop`；`Tail::new` 对空正文当场 `complete()`；`answer` 里 `REJECT-DROP` 那一支的停留 |
| V3 | hyper 1.11.1：HTTP/1.1 与 HTTP/2 服务端什么时候丢下应答正文、`is_end_stream` 为真时不读（`proto/h1/dispatch.rs`）、客户端重置流时丢下服务的 future（M2 设计 V16）；`size_hint` 怎样定 `content-length` |
| V4 | `choose_policy_with` 与 `rule_raw`：从 `decision.matched` 找到 `CompiledRule.params`；命中 `RULE-SET` 与逻辑规则时 `matched` 指哪一行；规则集文件里的行上的参数今天怎样解析、给不给诊断；直连与全局代理模式、`external` 的早退 |
| V5 | `ConnUpstreams::route` 的两个分支；`EngineShared::new` 的四处调用与可执行文件怎样改它的字段 |
| V6 | `state.rs` 的 `features.capture`；`restore_switches`、`rewrite_on` / `set_rewrite` 的写法；`rurge-api` 的 `features.rs` |
| V7 | 可执行文件里 `--request-log-size` 到 `RuntimeOptions` 的路；clap 的 `value_parser` 与 `env` 的写法；写错的参数的退出码 |
| V8 | `rurge-api`：`requests.rs` 的 `limit`；`error.rs` 的 `json_body` 与没有请求体的 `POST`；axum 0.8 的 `Path` 解码与 `Body::from_stream` 要的类型；工作区给 `futures-util`、`time` 开的特性；`env!("CARGO_PKG_VERSION")` 的值 |
| V9 | flate2 1.1.10 的三种解码器与以字节数为上限地读（M3b 的 `rewrite` 模块怎样做，能不能共用）；`url` 的 `query_pairs` |
| V10 | 规则参数的诊断在哪里报（`W0003` 的那一处）；`W0012` 的写法与 `codes::hint`；`every_code_is_decided_once` |
| V11 | 测试夹具：`rurge-http` 引擎用例的测试 `Upstreams` 与 `Step`；`rurge-engine` 的 `tests/http.rs`、`tests/mitm.rs`；`TlsFixture::with_ca_name`；`LogCapture`；`crates/rurge/tests/cli.rs` 的 `rurge_process()`；HTTP 监听器的 `password@`；带凭据的上游 HTTP 代理的夹具 |
| V12 | `crates/rurge-rules/tests/memory.rs` 的做法与这台开发机上读峰值工作集的办法 |
| V13 | （M4b）`snell/pool.rs`、`snell/tunnel.rs` 的重拨、`anytls/pool.rs`、`h2pool` 的 `lease` 与 `Conn`、ssh 的 `session()`、`rurge-proto-quic` 的 `Session::take` 与 `Conn`、`rurge-net/src/quic` 的 `Path.remote` 与替身、wireguard 的 `device.rs` 的载体与端点、`rurge-engine/src/udp.rs` 的流与载体、`packet_datagram.rs` 的懒开、`Datagram::peer_addr`、`trail.rs` 的 `Left` / `follow` / `connected` / `looked_up` / `set_peer`、`resolver.rs` 的 `Noted` 与合并的等待、`observe.rs` 对 UDP 的 `None` |
| V14 | （M4b）`serve.rs` 的 `sniffable_session`、`serve_sniffed`、`decrypting`（开关、CA、来源、名字、ALPN 各在哪一步判、谁先谁后）；`Mitm::decides` 的三种 `Pass` 与来源不允许时怎样再看名单；`dial` 建句柄的地方 |

## 15. 对总设计的订正

1. 第 9.2 节"正文存线上的字节" → 请求正文存发往上游的字节，应答正文存交给客户端的字节，都在重写之后（第 6.2、6.3 节）；压缩着的照原样存，读出时可以解开（第 7.5 节）。
2. 第 9.2 节"单条正文超过上限时只存开头一段并标'截断'；总量超出时丢掉最早的条目" → 另有：正文按块记账，挤不出地方时也截断（原因是"总量"）；头存不进去的整条丢掉（第 4.3、4.4 节）。
3. 第 9.2 节"名字在 M4 定"、"缺省值在 M4 量过内存之后定" → `--capture-size` / `RURGE_CAPTURE_SIZE`、`--capture-body-size` / `RURGE_CAPTURE_BODY_SIZE`，缺省 100 MiB / 5 MiB；量内存校的是记账的算法，缺省值不因它改（第 4.2、4.5 节）。
4. 第 9.2 节"开关抓到的归进缺省会话" → 缺省会话叫 `default`（第 4.6 节）。
5. 第 9.2 节"存什么" → 另存远端地址、结局、规则、策略与备注（清单 L569 要备注）；请求在两处取样（第 6.1、6.5 节）。
6. 第 9.2 节 HAR"拿不到的耗时写 `-1`" → `send`、`wait`、`receive` 不为负（HAR 1.2），`send` 写 0（第 8.3 节）。
7. 第 9.2 节"API 形状在 M4 定" → 第 7 节。
8. 第 9.1 节"耗时分出取连接、等应答头、收正文三段"、"请求与应答的正文字节数" → 新字段 `receiveMs`、`requestBodyBytes`、`responseBodyBytes`（第 10.1 节，M4b）。
9. 第 9.1 节"备注里写……放过的原因" → 九种（第 10.2 节）。
10. 第 1.5 节阶段 1 / M5a 延后第 3、20、21、22 行"其余……M4" → M4b：`remoteAddr` 改口径并补全，`dnsMs` 在途有数；仍是 `null` 的几种登记（第 10.3、10.4 节）。
11. 第 1.4 节 M4 一行 → 两份计划（第 2 节）。

## 16. 已决事项

| 编号 | 事项 | 决定 |
| ---- | ---- | ---- |
| M4-D1 | 计划与执行 | 两份计划（M4a 抓包 → M4b 请求记录的完整），后一份在前一份合并之后对着代码写；逐个任务调 Pi、逐个任务评审；没有只在 CI 上编译的代码（第 2 节） |
| M4-D2 | 依赖 | 不加新包、不下载；`rurge-api` 正式依赖工作区的 `base64`、`futures-util`、`time`（开 `formatting`）；`Cargo.lock` 的包数不变（第 3 节） |
| M4-D3 | 存储 | 一个引擎一份，在 `EngineShared` 里，只在内存，重载不清；条目是 `Arc`、一条队列由旧到新、带会话名；条目的 ID 是记录的 ID（第 4.1 节） |
| M4-D4 | 上限 | 两个参数与两个环境变量；字节数可带 `k` / `m` / `g`（1024 进）；`0` 合法；缺省 100 MiB / 5 MiB；单条大于总量时降到总量，使用者写了单条时启动时 WARN（第 4.2 节） |
| M4-D5 | 记账 | 头按 RFC 7541 记"名字 + 值 + 32"，加 URL、方法、规则、策略、备注、存下的正文与固定的 512；正文按块预留，头在存进去时记（第 4.3 节） |
| M4-D6 | 挤掉、截断、丢掉 | 不分会话挤掉最早的；单条超限只留开头，挤不出地方从那一块起不存；头放不下整条丢掉；四个数从引擎启动起（第 4.4 节） |
| M4-D7 | 量内存 | 两条忽略的用例；多出的超过总量的 25% 时调固定的字节数，缺省上限不因此改（第 4.5 节） |
| M4-D8 | `always-capture` 的范围 | 只作用于本来就经 HTTP 引擎的请求，不让连接进引擎、不让 TLS 被解密；只在求规则时、看命中的那一行顶层规则（第 5.2 节） |
| M4-D9 | 会话的归属 | 规则给的名字先于开关；开关的是 `default`；在 `route` 时定；会话只在有条目时存在（第 4.6、5.3 节） |
| M4-D10 | 抓哪些 | 走到 `route` 的经引擎的请求，被拒的与 rurge 自己答复的都算；HTTP/2 的 `CONNECT`、没有句柄的坏请求、承载连接的记录、不经引擎的流量不算（第 5.4 节） |
| M4-D11 | 请求的样子 | 两个取样点：离开 rurge 时（`strip` 之后、`absolute_form` 之前）；没有发出的取求规则时的（去掉逐跳头）；URL 随之；版本是客户端的；头的名字小写、`HeaderMap` 的次序（第 6.1 节） |
| M4-D12 | 正文 | 请求正文在 `Idle` 里抄发往上游的，没有发出的没有；应答正文在包住应答的一层里抄交给客户端的；trailer 不抄；那一层不改 `size_hint` 与 `is_end_stream`（第 6.2、6.3 节） |
| M4-D13 | 存进去的时刻 | 升级与隧道在交出应答头时；其余在正文交完或被丢下、与记录结束两者后到的那一刻；没有应答的在知道没有应答、记录也已结束时（`REJECT-DROP` 见 M4-D29）；新的 `SessionHandle::when_finished`（第 6.4 节） |
| M4-D14 | 耗时等 | 存进去时从句柄读：六个时间与 `reused`、远端地址、结局、规则、策略、备注（第 6.5 节） |
| M4-D15 | 凭据 | 四个头在 API 与 HAR 里写 `***`、存储里原样；rurge 自己的两种凭据从不进抓包，各一条用例；`Entry` 不实现 `Debug`（第 6.6 节） |
| M4-D16 | `capture` 开关 | 缺省关；`state.json` 第 2 版；启动时还原；在求规则时读；关掉之后已存的保留（第 5.1 节） |
| M4-D17 | API | 第 7 节的六个端点与形状；摘要由新到旧、HAR 由旧到新；没有条目的会话 404；清除不动进行中的与四个数 |
| M4-D18 | HAR | 第 8 节的映射；`cookies` 写空表；能解开的 `content` 写解开之后的；`timings` 的算法；逐条写出 |
| M4-D19 | 诊断与日志 | 空的 `always-capture=` 给 `W0012`，不设新码；日志只多开关的 INFO 与降上限的 WARN（第 9 节） |
| M4-D20 | 读出时解开 | `gzip`、`x-gzip`、`deflate`，由外到里，解开之后至多单条正文的上限；截断的与别的编码不解（第 7.5 节） |
| M4-D21 | 记录的新字段（M4b） | `receiveMs`（会话结束才有）；`requestBodyBytes`、`responseBodyBytes`（HTTP 请求才有）（第 10.1 节） |
| M4-D22 | 直通的原因（M4b） | 九种备注（第九种见 M4-D31）；名单没命中的 TLS 与不可嗅探的端口不写；`Mitm::decides` 分开"来源不允许"、"没命中"与"名字签不了证书"（第 10.2 节） |
| M4-D23 | `remoteAddr` 的口径（M4b） | 离开 rurge 时所经的连接或载体的远端，自己开的、借的、共用的都算；UDP 流照载体；按目标懒开的三种仍 `null`（第 10.3 节） |
| M4-D24 | `dnsMs`（M4b） | 在途的查找也计到此刻（第 10.4 节） |
| M4-D25 | 记录的容量（M4b） | 量一次；`--request-log-size` 的缺省 1000 不改（第 10.5 节） |
| M4-D26 | `CONNECT` 的 URL（构建者检查之后定的，下同） | 嗅探到的连接上的 `CONNECT` 没有 URL（记录里是 `null`）：条目、API 与 HAR 的 `url` 写请求行里的 `host:port`，照 mitmproxy 对这种请求行的写法（第 6.1 节） |
| M4-D27 | HAR 的耗时不把 DNS 算两遍 | `dns` 先压到它所在的那一段之内；借了连接的 `blocked = connect − d`；没连上的 `wait = total − d`；各项之和在取整之前恰好是 `total`，`totalMs − time` 在 0 到 3 之间（第 8.3 节） |
| M4-D28 | 应答的版本 | 客户端看到的：讲 HTTP/2 的客户端连接上是 HTTP/2，HTTP/1.x 的是交给 hyper 的应答的 `version()`（第 6.3 节） |
| M4-D29 | 结束的那一刻 | `total` 与 `receive` 量到"记录结束"与"应答正文交完或被丢下"两者后到的那一刻；升级与隧道到交出应答头；没有应答的到记录结束；`REJECT-DROP` 在停留之前告诉 `CaptureSlot` 没有应答，条目在记录结束时存进去（第 6.4、6.5 节） |
| M4-D30 | 正文字节数在哪里数（M4b） | 应答正文在 `Tail` / `LocalFile` 交出每一帧、`complete()` 之前；rurge 自己的答复在建应答时、`finish` 之前；请求正文在 `Idle`；记录照旧在 `finish` 时拍下，不靠 M4a 包住应答的那一层（第 10.1 节） |
| M4-D31 | 直通原因的次序（M4b） | 照 `sniffable_session` 与 `Mitm::decides` 的判断次序取最先成立的那一条；不可嗅探的端口上命中 `always-raw-tcp-*` 的照写；TLS 的五种只在名单命中时写；第九种 `Not decrypted: no certificate can be made for this name`（第 10.2 节） |

## 17. 留给项目所有者的

| 问题 | 与 M4 的关系 |
| ---- | ------------ |
| 总设计 Q1：用户流量的内容算不算 API 输出 | M4 照临时做法：抓包与 HAR 里遮住 `authorization`、`proxy-authorization`、`cookie`、`set-cookie` 四个头的值，正文与 URL（含查询串）照给，存储里原样（第 6.6 节）。Header Rewrite 加的头（例如一个 `x-api-key`）随请求进抓包——那是 Profile 里使用者写的值，不是四个头之一就不遮（与 Q2 照写的显示相同）。要改，改成什么？ |
| 抓包的两个缺省上限（M4-D4） | 100 MiB / 5 MiB，照 Node.js；第 4.5 节量到的内存写进 `docs/api/phase4.md` |
| `always-capture` 的范围（M4-D8） | rurge 只抓本来就经 HTTP 引擎的请求；要不要让它像 `force-http-engine-hosts` 那样把命中的连接留在引擎里？ |
| `remoteAddr` 的新口径（M4-D23，M4b） | 从"会话自己开的那条 TCP 连接"改成"所经的连接或载体的远端"，`docs/api/phase1.md` 的说法随之订正；照旧 `null` 的几种登记 |
| `--request-log-size` 的缺省（M4-D25，M4b） | 1000 不改；量到的一条记录的大小供所有者决定 |
| 总设计 Q4 | 读一条时 `br`、`zstd` 解不开（第 7.5 节），随 M3c |
| 总设计 Q2、Q3、Q5、Q6 中别的 | 与 M4 无直接关系，照旧开着 |

另有两件事要项目所有者做：**推送**（M1 ～ M3 的提交还没有推送；M4 做完之后若仍没有，就一起，推送之后读回那次 CI）；**手工验收**（`docs/acceptance/phase4-manual.md` 随 M4a、M4b 各加几项；阶段 1、2、3 的清单与阶段 4 的照旧一项也没有做）。

所有者可以改的裁定：本文 M4-D1 ～ D31，尤其 M4-D4（缺省上限）、M4-D8（`always-capture` 不让连接进引擎）、M4-D11（请求在两处取样、没有发出的取求规则时的）、M4-D13（存进去的时刻）、M4-D15（遮住四个头）、M4-D23（`remoteAddr` 的新口径）与 M4-D25（`--request-log-size` 不改）。

## 18. 计划与执行期的订正

（写计划与执行时在这里接着写。）
