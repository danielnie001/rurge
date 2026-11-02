# 阶段 4 / M5「`auto-quic-block`、基准与遗留」细化设计

| 项       | 内容 |
| -------- | ---- |
| 日期     | 2026-11-02 |
| 状态     | 定稿（2026-11-02，构建者检查之后：`auto-quic-block` 的第 3 条改成跟着 TCP 一侧会不会嗅探、会不会解密，第 6.1 节；拼 CRYPTO 只存到了的字节，第 4.5 节）。已决事项 M5-D1 ～ D18 是在项目所有者"自己决定、继续做"的常设授权下定的（代项目所有者作的裁定），所有者可以改。执行交给 Pi：M5a、M5b 两份计划由协调的会话逐个任务调 Pi 做、逐个任务评审（第 2 节） |
| 上位设计 | [阶段 4 总设计](2026-10-29-phase4-http-engine-design.md) 第 1.4、1.5、3.6、7.1、11、13、14、16、17 节；[M1 细化设计](2026-10-29-phase4-m1-session-entry-http-engine-design.md) 第 6.2 节（`parse_client_hello` 是留给 M5 复用的那一半；先读第 20、21 节的订正）；[M2 细化设计](2026-10-30-phase4-m2-mitm-design.md) 第 7 节（何时解密；先读第 23 节）；[M4 细化设计](2026-11-01-phase4-m4-capture-records-design.md) 第 10.2 节（直通的原因与判断的次序；先读第 18 节）；[阶段 3 / M5 设计](2026-10-28-phase3-m5-recognition-blocking-ip-rewrite-design.md) 第 4、6、11.1、12 节。不一致处以本文为准，订正列在第 13 节 |
| 需求     | FR-HTTP-04（`auto-quic-block`，M5a）；FR-RULE-07 / 08 的拨号之前部分里 QUIC 的那一块（求规则之前从 QUIC 的 ClientHello 取 SNI，M5a；TCP 的那一块 M1a 做了）；FR-HTTP-10 的备注（M5a）；NFR-01 的读数（M5b，写成数字，不作门禁） |
| 清单     | L584（`auto-quic-block`）、L249（`PROTOCOL`）、L265（`extended-matching`）、L177（全局的 `block-quic`）。行号是 2026-11-02（`a262d4c`）的 |
| 前置     | M4 做完（M4a、M4b 与 M4b 插入的任务 10、11），本地 `main` = `802a634`；`origin/main` = `a262d4c`（`f9994db` 起的五个提交——任务 11 与读回第 17 次 CI 的文档——等项目所有者推送，推送之后读那次 CI）。M5 在分支 `phase4-m5` 上，从 `802a634` 起，不等推送 |
| 调研     | 2026-11-02 一个只读的助手查的，标【原文】的是它取回原文读的，标【摘要】的是经网页摘要或搜索片段读到的（用到之前对着原文再核）：Surge 手册 `http/mitm.html` 的两句（下文第 6 节；项目 2026-10-28 的手册文本里就有，阶段 3 / M5 设计第 11.1 节引过）；App Store 上 Surge iOS 的版本历史【原文】；Telegram 频道 "Surge TestFlight Feed" 第 418、420 条【原文；频道是不是 Surge 官方的没有核】；RFC 9000 第 2.2、12.2、14.1、17.2.2、19.3、19.6 节与附录 A.3，RFC 9001 第 5 节与附录 A，RFC 9002 第 6.2.2 节，RFC 9369【原文；RFC 9000 的第 2.2、19.3、19.6 节是协调者另取原文核的】；QUICHE 的 `quic_chaos_protector.cc` 与 `quic_packet_creator.cc`（BSD）【原文】；sing-box（GPLv3，只取事实）、Xray-core（MPL-2.0）、mihomo（GPLv3，只取事实）、Hysteria（MIT）怎样读跨包的 ClientHello（第 4.6 节） |

M5 做三件事。**读 QUIC 里的 ClientHello**（M5a）：一条 UDP 流的第一个数据报认出是客户端的 QUIC Initial（阶段 3 / M5 起在所有端口上认）之后，从它与它后面的数据报里的 Initial 包解出 CRYPTO 帧、按偏移拼出完整的 ClientHello，交给 M1 写好的 `parse_client_hello` 取出 SNI。拼齐之前这条流不求规则、不拨号，持住已经来的数据报（至多 300 毫秒、16 个数据报），定下来之后照原来的次序发出。SNI 写进会话，规则（`extended-matching`）与请求记录都看得到它。**`auto-quic-block`**（M5a）：读到了 ClientHello、`[MITM] auto-quic-block` 为真（缺省）、而同一个 ClientHello 在 TCP 上会被解密时，这条 QUIC 流被阻断，客户端回落到 TCP 上的 HTTPS、被 MITM 解开（手册）。**基准与遗留**（M5b）：经引擎的明文与解密之后的 HTTP 的吞吐与每请求延迟、一千条规则下每请求求值的开销、叶证书签发的耗时、三项内存、读 ClientHello 的开销，照实写成数字；M1b 留下的两条没有能红的用例。

**不加新包、不下载**（第 3 节）。M5a 改的代码都在本机编译、运行；M5b 的基准在本机用 release 编一次引擎的用例（第 8.1 节）。

## 1. 范围

| 做 | 不做（去向） |
| -- | ------------ |
| 从客户端的 Initial 包里读 ClientHello：合并的包逐个看、只认 Initial 里允许的五种帧、CRYPTO 按偏移拼（乱序、重叠、跨包、跨数据报）、上限 64 KiB（第 4 节，M5a） | gQUIC 与草案 28 以前（不认，阶段 3 / M5-D3，所有者的事项）；从没有拼齐的 ClientHello 里取 SNI（第 4.7 节） |
| 第一个数据报里没有拼齐时持住这条流，至多 300 毫秒、16 个数据报（第 5 节，M5a） | 测试用的调时限的旋钮（不设，M5-D7） |
| SNI 在求规则之前写进会话：规则、`extended-matching`、请求记录与 `/v1/requests/*` 都看得到（第 5.3 节，M5a） | 按 SNI 换拨号的目标（不做：总设计 P4-D5，目标照旧是这条流的目标） |
| `auto-quic-block` 生效，跟着 TCP 一侧：回落之后的 TCP 会被嗅探、解密的才阻断（第 6 节，M5a） | HTTP/3 的解密（总设计第 1.3 节）；自适应的表按 SNI 记（不改，第 6.3 节） |
| `/v1/rurge/http` 多 `quic` 四个数；两种备注、一种说明（第 5.4、7 节，M5a） | 读到了 ClientHello 而没有阻断时写原因（不写，M5-D10） |
| 手工清单加 M5a 的几项（M5a 收尾） | 手工验收本身（归所有者） |
| 基准：B1 ～ B5、Q1（第 8 节，M5b） | 门禁（基准不作门禁）；重跑阶段 3 的 B1、B2、b3（第 8.8 节） |
| 遗留：M1b 计划延后第 4 行的两条用例（第 9 节，M5b） | 其余的遗留（第 9 节列出各自的去向） |
| — | 调研经摘要读到的 `policies/parameters.html` 的新说法（`adaptive` 在接管流量之后的头 10 秒一律阻断等，与项目 2026-10-28 的文本不同）：M6 重对手册时对着原文核，不在 M5 追 |

## 2. 计划与执行

两份计划。后一份在前一份合并之后开工，它的计划也在那时对着合并之后的代码写（同 M1 ～ M4）：

- `docs/superpowers/plans/2026-11-02-phase4-m5a-quic-hello-auto-quic-block-plan.md`
- M5b（基准与遗留）的计划日后写，文件名同样的格式。

M5a 的任务：

| 任务 | 内容 |
| ---- | ---- |
| 0 | 开工前核对：第 12 节的各项对着当时的代码核一遍，出入记进「执行期修正记录」；量一次基线（全量门禁的数） |
| 1 | `rurge-net` 的 `quic::testing`：造 Initial 包与数据报（任意的帧、包号与它的长度、合并、尾部填零）、用 rustls 的 QUIC 客户端写出真实的 ClientHello、Chrome 的两种打散的排法；RFC 9001 与 RFC 9369 的两份样例从 `initial.rs` 的用例里挪过来；构造器自己的用例（第 10 节第 1 条） |
| 2 | `rurge-net` 的 `quic::initial`：`read_first`、`HelloReader::read`（第 4 节）；`is_client_initial` 改成 `read_first(..).is_some()`，原有的用例原样通过；单元用例（第 10 节第 2 条） |
| 3 | `rurge-engine` 的 `udp.rs`：持住、SNI 写进会话、`forward` 与 `drain` 带着持住的数据报、计数与备注（第 5 节）；暂停时钟的单元用例、经 SOCKS5 的端到端（第 10 节第 3 条） |
| 4 | `rurge-engine`：`auto-quic-block`（`Engine::auto_quic_blocks`、在 `open` 里的位置、说明与计数，第 6 节）；经 SOCKS5 与内存设备上的虚拟网卡的端到端（第 10 节第 4 条） |
| 5 | `rurge-api`：`/v1/rurge/http` 多 `quic`（第 7 节）；两处逐字节的用例 |
| 6 | 文档收尾（第 11 节）；手工清单加 M5a 的几项 |

M5b 的任务（计划日后写，这里是大纲）：核对与磁盘；遗留的两条用例（第 9 节）；基准的夹具与 B1、B3；B2 与 B4；B5 与 Q1；release 下跑一遍、记数（第 8 节）；文档收尾。

**执行的约定**（两份计划开头重申，同 M1 ～ M4）：协调的会话逐个任务开 Pi 的会话、把任务的全文与要用的接口交给它；Pi 做完写报告、提交；协调者读改动、跑门禁、评审，有问题回到同一个 Pi 会话修正，再评审，过了才派下一个任务。每个任务先写失败的用例、看到红，再实现到绿；门禁是 `CLAUDE.md` 的全量门禁；提交说明的标题用中文。计划没有写到、要做决定的，Pi 停下来报告，不自己定。

做完 M5b 之后：项目所有者推送（那时还没有推送的一起），读回那次 CI；再往后是 M6（收尾与验收）。M5a 之后不单独停下来等推送。

## 3. crate 与接口

**不加新包、不下载**：读 Initial 用的 rustls 0.23.43 的 `quic` 模块（`Suite::keys`、`DirectionalKeys` 的 `header` 与 `packet`、`HeaderProtectionKey::decrypt_in_place`、`PacketKey::decrypt_in_place`）阶段 3 / M5 已经在用；测试里写真实 ClientHello 的 `rustls::quic::ClientConnection::new_with_alpn` 与 `write_hs` 在同一个包里。`Cargo.lock` 的包数应当不变（今天 520 个）。依赖方向不加新的工作区边：`rurge-engine` 已经依赖 `rurge-net` 与 `rurge-http`；`rurge-net` 不依赖 `rurge-http`（反过来才有），所以读出的握手消息交给 `parse_client_hello` 的那一步在引擎里接（M5-D2）。`unsafe` 不变（`LISTED` 23）。

| crate | 改动 |
| ----- | ---- |
| `rurge-net` | `quic::initial`：`read_first`、`HelloReader`、`Hello`、`MAX_HELLO`；`is_client_initial` 改由 `read_first` 定（M5a）。`quic::testing`：Initial 包与数据报的构造器、真实的 ClientHello、两种打散的排法、两份 RFC 样例（M5a） |
| `rurge-http` | 不改（`tls::parse_client_hello` 照用） |
| `rurge-engine` | `udp.rs`：持住、`forward` / `drain` 带着持住的数据报、`open` 多一个参数（M5a）；`serve.rs`：`Engine::auto_quic_blocks`、QUIC 的计数与 `HttpCounts::quic`（M5a）；新的忽略的用例文件（M5b） |
| `rurge-api` | `/v1/rurge/http` 的 `quic`（M5a） |
| `rurge-config` | 不改（`MitmConfig::auto_quic_block` 从阶段 4 / M2a 起读进来，缺省真，`crates/rurge-config/src/mitm.rs:45`） |

接口的形状（细节以计划为准）：

```rust
// rurge-net::quic::initial（M5a）
/// 拼 ClientHello 的 CRYPTO 字节至多这么多（第 4.5 节）
pub const MAX_HELLO: usize = 64 * 1024;

/// 读到了哪一步
pub enum Hello {
    /// 第一个握手消息完整了：类型（1）、24 位的长度与正文，共 4 + 长度个字节
    Whole(Vec<u8>),
    /// 还缺：拿着它读这条流的下一个数据报
    Partial(HelloReader),
    /// 读不成（第 4.4、4.5 节）；字符串说为什么，只进 DEBUG 的日志
    GaveUp(&'static str),
}

/// 一条流的第一个数据报。不以客户端的 Initial 开头时是 None——与今天的
/// is_client_initial 说"不是"的情形完全相同（第 4.3 节）
pub fn read_first(datagram: &[u8]) -> Option<Hello>;

/// 第一个数据报的版本、客户端一侧的 Initial 密钥、打开过的最大包号、
/// 到了的 CRYPTO 字节（只存到了的，几段，第 4.4、4.5 节）；Send（引擎持着它跨过 await）
pub struct HelloReader { /* … */ }
impl HelloReader {
    /// 读这条流后来的一个数据报
    pub fn read(self, datagram: &[u8]) -> Hello;
}

/// = read_first(datagram).is_some()
pub fn is_client_initial(datagram: &[u8]) -> bool;
```

```rust
// rurge-engine（M5a）
/// QUIC 的 ClientHello 读成什么样、auto-quic-block 阻断了几条（第 5.4 节）：
/// GET /v1/rurge/http 读的
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QuicSniffed { pub hello: u64, pub incomplete: u64, pub unreadable: u64, pub blocked: u64 }

pub struct HttpCounts { /* 已有的 sniffed、requests、mitm、rewrite */ pub quic: QuicSniffed }

impl Engine {
    /// auto-quic-block 阻断这条读到了 ClientHello 的 QUIC 流吗：同一个目标与端口上的
    /// TCP 会话会被嗅探、并被解密时才阻断（第 6.1 节）
    pub(crate) fn auto_quic_blocks(&self, session: &SessionInfo) -> bool;
}
```

## 4. 读 QUIC 里的 ClientHello（M5a）

### 4.1 今天

`run_flow`（`crates/rurge-engine/src/udp.rs:503`）在第一个数据报上定这条流的协议：`rurge_net::quic::initial::is_client_initial`（`udp.rs:521`）说是就是 `QUIC`。`is_client_initial`（`crates/rurge-net/src/quic/initial.rs`）只看第一个包：先便宜地筛包头（至少 1200 字节、长包头、认得的版本、这个版本的 Initial 类型、连接 ID 的长度、Token 与 Length 都在数据报之内），再用这个包自己的目的连接 ID 导出客户端的 Initial 密钥、去掉包头保护、解密，标签对上就是（阶段 3 / M5 设计第 4.2 节）。它不读帧，合并在后面的包也不看。会话信息在 `new_handle_with`（`udp.rs:529`）之后不再变，所以 SNI 要在那之前写进去。

### 4.2 客户端今天怎样发

- **一个 ClientHello 装不进一个包**。后量子的密钥交换让 ClientHello 变大：X25519MLKEM768 的密钥份额是 1216 字节，X25519 的是 32 字节【摘要，Cloudflare 的博客】；Chrome 130 的 ClientHello 是 1955 字节【摘要，Xray PR #7292 的抓包】。照 RFC 9001 附录 A.2 的样例算，一个 1200 字节、目的连接 ID 8 字节的数据报装得下约 1158 字节的 CRYPTO 数据，带 Token 或更长的连接 ID 时更少。Hysteria PR #1693 写 Chrome、Firefox、curl（OpenSSL）、quiche 与 quic-go 都把 1.5 ～ 2 KB 的 ClientHello 分在两个包里【摘要】；OpenSSL 3.5 起缺省的密钥份额同样要两个 Initial【摘要】。
- **Chrome 打散 CRYPTO 帧**（"chaos protection"，QUICHE 的 `quic_chaos_protector.cc`【原文】）：只在客户端、只在 Initial：把 CRYPTO 帧在随机的位置再切出 2 ～ 10 个，加 2 ～ 10 个 PING 帧，PADDING 随机地插在帧之间，除 ACK 之外的帧随机排序——一个包里的 CRYPTO 帧可以不按偏移的次序。**跨包的排法**（`quic_packet_creator.cc` 的 `MultiPacketChaosProtect`【原文】）：第一个包放偏移 0 起 55 ～ 86 字节的开头与 ClientHello 的结尾，中间放到后面的包里，代码的注释说目的就是逼接收方读几个包；Xray PR #7292 里 Chrome 130 的抓包【摘要】：第一个 Initial 有 1955 字节里的 0 ～ 59 与 1039 ～ 1955，SNI 在中间的空缺里。这一种排法从哪个版本起用没有查到。
- **合并的包**：RFC 9000 第 12.2 节"Receivers MUST be able to process coalesced packets."；第 14.1 节，客户端可以用 PADDING 帧、也可以把 Initial 与别的包合并来凑够 1200 字节【原文】。数据报的尾部也有填零的（Xray 为它改过，v26.10.21【摘要】）。
- **Initial 里只许五种帧**（RFC 9000 第 17.2.2 节【原文】）：CRYPTO、ACK，以及"PING, PADDING, and CONNECTION_CLOSE frames of type 0x1c"。
- **密钥**：Initial 的密钥由客户端第一个 Initial 的目的连接 ID 导出（RFC 9001 第 5.2 节【原文】），整个 Initial 阶段不变——Retry 之后才换，而 rurge 持住第一批数据报时服务端还什么都没收到，不会有 Retry（第 4.7 节）。
- **客户端什么时候重发**：RFC 9002 第 6.2.2 节，初始 RTT 333 毫秒，握手从 1 秒的 PTO 开始【原文】。第一批数据报被持住不到 1 秒，客户端不会因此重发。

### 4.3 分类不变（M5-D3）

一条流是不是 QUIC，照旧只由第一个数据报的第一个包定：`read_first` 在 `is_client_initial` 今天说"不是"的每一种情形答 `None`，说"是"的每一种情形答 `Some(..)`——帧读不读得成（`GaveUp`）不改变"是 QUIC"。为了只有一处定分类，`is_client_initial` 改成 `read_first(datagram).is_some()`；它现有的用例（RFC 的两份样例、构造器造的各版本、清掉的 QUIC 位、合并的包、quinn 的第一个数据报、改了一个字节的、各种不像 Initial 的包头、每一个前缀、前 64 字节的每一位翻转）原样通过，就是分类没有变的证据。引擎只调 `read_first`，不再调 `is_client_initial`。

### 4.4 一个数据报怎样读（M5-D4、D5）

**第一个数据报**：第一个包照今天的判据（第 4.1 节）判，判过的才往下读；用它的目的连接 ID 与版本导出客户端一侧的 Initial 密钥，读完这个数据报里所有的包（下面的走法），留着密钥读后来的数据报。

**走一个数据报**：从头起，一个包一个包地看，直到数据报的末尾或下面的停止条件：

1. 首字节的长包头位（`0x80`）是 0：短包头或填充的零，到此为止（短包头没有长度，后面的都属于它）。
2. 版本不是第一个包的版本（含版本 0，即版本协商）：到此为止。
3. 类型（v2 的编号不同，RFC 9369 第 3.2 节）是 Retry：到此为止（Retry 没有 Length）。
4. 读两个连接 ID 的长度（各至多 20）、Initial 的 Token、Length；任一个超出数据报：到此为止。
5. 不是 Initial（0-RTT、Handshake）：按 Length 跳过。
6. 是 Initial：用留着的密钥去掉包头保护、解密。打不开（标签不对）：跳过这个包，**不算读不成**——它可能不是这个连接的；打得开：读它的帧（下一段）。

后来的数据报不要求 1200 字节，Initial 包的目的连接 ID 不与第一个比（密钥只认第一个 Initial 的目的连接 ID，第 4.2 节）：除了第一个数据报的第一个包，**打开**是唯一的检验。

**包号**：去掉包头保护之后得到 1 ～ 4 字节的截断包号，照 RFC 9000 附录 A.3 的 `DecodePacketNumber`、以这个读者打开过的最大包号还原（一个也没有打开过时当作 −1，即期望 0），解密用还原出的包号。

**帧**（一个打得开的 Initial 包的明文，逐帧读到末尾）：

- `0x00` PADDING：一个字节，跳过；
- `0x01` PING：跳过；
- `0x02` / `0x03` ACK：照 RFC 9000 第 19.3 节读完它的变长整数（`0x03` 另有三个 ECN 计数），跳过；
- `0x06` CRYPTO：偏移、长度、数据，交给拼接（第 4.5 节）；
- `0x1c` CONNECTION_CLOSE：客户端放弃了这个连接——**读不成**；
- 别的类型、在包的末尾之前截断的帧：**读不成**（打得开的包是客户端真发的；它违反了 RFC，服务端也会关掉这个连接）。

一个数据报读完、ClientHello 拼齐了就答 `Whole`；哪一步答了读不成就答 `GaveUp`，不再往下读；其余答 `Partial`。

### 4.5 CRYPTO 的拼接（M5-D6）

- **只存到了的字节**：按偏移排好的几段，互不重叠；新来的一段与已有的相接或重叠时合成一段。不预先分配 64 KiB 的缓冲，也不用位图（mihomo 用的是整块缓冲加位图【原文】；构建者检查指出那样每条持着的流先占 72 KiB，不记在任何预算里）。
- 一个 CRYPTO 帧的偏移加长度超过 `MAX_HELLO`（64 KiB）：读不成。
- 已经到了的字节再来（重发、重叠）：必须相同，不同就读不成（RFC 9000 第 2.2 节："The data at a given offset MUST NOT change if it is sent multiple times"【原文】；Xray PR #7292 也不收有空缺的 ClientHello【摘要】）。
- 合并之后互不相接的段至多 32 段，第 33 段读不成：挡住一个专发零碎字节的客户端让段的开销比字节还多。Chrome 的打散一个包里至多多切出 10 个 CRYPTO 帧，跨包的排法在第一个包之后留两段（第 4.2 节），远在 32 之下。
- 偏移 0 起的四个字节到了之后：第一个字节不是 `0x01`（ClientHello 的握手类型）读不成；4 加上 24 位的长度超过 `MAX_HELLO` 读不成；`[0, 4 + 长度)` 都到了（落在从偏移 0 起的那一段里）就答 `Whole`，交出这 4 + 长度个字节，之后的字节不看。
- 交出的字节由引擎交给 `rurge_http::tls::parse_client_hello`（它要恰好一个握手消息，`crates/rurge-http/src/tls.rs:46`），读不出的算读不成（第 5.4 节）。

上限的理由：64 KiB 是总设计第 11 节定的数，与 TCP 一侧嗅探的前缀上限 `MAX_PREFIX`（`crates/rurge-http/src/sniff.rs:17`）相同；mihomo 也是 64 KiB，它的注释说这高于 quic-go 与 BoringSSL 的 16 KiB、与 ngtcp2 和 quiche 一致【原文】。

内存：存下的字节至多是持着的数据报里 CRYPTO 帧的字节；持着的数据报都是 `Queued`，记在关联的 `QUEUED_BYTES`（1 MiB，`crates/rurge-engine/src/udp.rs:48`、`:390`）里，装不下的照旧丢。所以一个读者至多多占持着的字节那么多、加 32 段的开销，只在持住的那一段里有，放了就丢。

### 4.6 别人怎样做（只取事实）

| 实现 | 跨包 | 等多久、读多少 | 合并的包 | 来源 |
| ---- | ---- | -------------- | -------- | ---- |
| sing-box（GPLv3） | 拼，按偏移从 0 起首尾相接 | 每多读一个包等一次嗅探的时限，缺省 300 毫秒；没有看到包数的上限；超时之后把攒着的包发出去、不带域名 | 只解第一个包 | 【原文】`common/sniff/quic.go`、`route/route.go` |
| Xray-core（MPL-2.0） | 拼（v24.9.16 起多包，v25.4.30 起"Full support"）；2026-10-31 起不收有空缺的 | 整个嗅探 200 毫秒，读缓冲 32767 字节 | — | 【原文】`common/protocol/quic/sniff.go`；PR #7292【摘要】 |
| mihomo（GPLv3） | 拼，位图，乱序、重叠与重发都认 | 等 3 秒；CRYPTO 至多 64 KiB | 逐个看；为 Retry 留两把密钥；打不开的 Initial 跳过；只许 Initial 的五种帧 | 【原文】`component/sniffer/quic_sniffer.go` |
| Hysteria（MIT） | 拼，乱序与重发都认 | 8 个包之后放弃 | — | PR #1693【摘要】 |
| QUICHE 服务端（BSD） | `TlsChloExtractor` 分单包与多包两种结局 | 每个连接至多攒 10 个解不开的包，空闲 5 秒 | — | 【原文】 |

rurge 取：64 KiB（同 mihomo，也是总设计的数）、乱序、重叠与重发都认（同 mihomo、Hysteria），但只存到了的字节（不用 mihomo 的整块缓冲与位图，第 4.5 节）、合并的包逐个看（同 mihomo，合 RFC 9000 第 12.2 节）、300 毫秒（同 sing-box 的缺省，远小于客户端 1 秒的 PTO）、16 个数据报（第 5.2 节）。

### 4.7 不做的（M5-D15）

- **从没有拼齐的 ClientHello 里取 SNI**：Surge iOS 5.17.0 的版本说明"Optimize QUIC SNI extraction to support extracting SNI from incomplete initial packets"【原文】。rurge 只在 `[0, 4 + 长度)` 都到了时读：Chrome 跨包的排法让 SNI 落在中间的空缺里，只读到一半时多半读不到 SNI；本地的入口（虚拟网卡、回环上的 SOCKS5）几乎不丢包，凑不齐的情形少。登记为与 Surge 的差异，要不要做归所有者（第 15 节）。
- **gQUIC 的 SNI**：Surge iOS 5.14.6 的版本说明"Supports SNI extraction for gQUIC."【原文】。rurge 不认 gQUIC（阶段 3 / M5-D3，阶段 3 / M5 计划延后第 16 行，归所有者）：这一条新的事实列给所有者（第 15 节）。
- **Retry 之后的第二把密钥**：持住第一批数据报时它们还没有到服务端，不会有 Retry；放出去之后的数据报不再读。
- **ECH**：rurge 读到的是外层的 SNI（总设计第 1.3 节），QUIC 同样。

## 5. 引擎：持住第一批数据报（M5a）

### 5.1 在哪里持

`run_flow` 在 `turn_back`（fake 地址换回名字）之后、`new_handle_with` 之前调 `read_first(第一个数据报)`：答 `None` 的照旧去判 STUN 与 UDP（`udp.rs:520-527`）；答 `Some` 的协议是 `QUIC`，按它的答复：

- `Whole`：交给 `parse_client_hello`（第 4.5 节）；
- `GaveUp`：读不成；
- `Partial`：持住——从这条流的队列（`waiting`）里接着收数据报，每一个交给 `HelloReader::read`；收到的数据报都放进一个持着的队列（`VecDeque<Queued>`，第一个数据报在最前），直到第 5.2 节的任一条。

持住时没有句柄、没有记录，同 TCP 的嗅探（`crates/rurge-engine/src/serve.rs` 的 `serve_sniffed`："nothing is held and no record made while it waits"）。关联在这期间结束（`association.ended` 被取消：它是引擎会话令牌的孩子，`serve` 不论怎样结束——客户端的控制连接关了、收包出错、引擎取消全部会话——都由一个 drop guard 取消它，`udp.rs:445-449`；队列的发送端随 `serve` 一起丢掉，`waiting` 收到 `None` 也算），这条流不留记录、不计数，`run_flow` 照旧向 `serve` 报自己结束了（`association.done`）。fake 地址背后没有名字（`Turned::Unknown`）的流同样读：它照旧失败，记录里多了 SNI（使用者看得出它想去哪里）。

### 5.2 什么时候放（M5-D7）

下面任一条成立就不再持：

1. 拼齐了（`Whole`），或读不成（`GaveUp`）；
2. **从第一个数据报起 300 毫秒**（常数 `HELLO_WAIT`）；
3. **持着的数据报到了 16 个**（常数 `HELLO_DATAGRAMS`，第一个算在内）；
4. 关联结束（第 5.1 节）。

理由：拼齐了就立刻放，所以 ClientHello 装得进第一个数据报的流一点也不多等；跨两个数据报的（Chrome 与 Firefox 的后量子 ClientHello）多等的是两个数据报到达的间隔——客户端一次发出整个第一批，本地的入口上是微秒到毫秒。300 毫秒与 16 个只管凑不齐的：300 毫秒同 sing-box 的缺省、在 Xray 的 200 毫秒与 mihomo 的 3 秒之间，远小于客户端 1 秒的 PTO（第 4.2 节）；16 个数据报装得下 16 KiB（quic-go 与 BoringSSL 收的上限，第 4.5 节）的 ClientHello——按每个约 1158 字节算要 15 个——，也挡住一个只发别的包的客户端让持着的队列一直长（每个关联 1 MiB 的预算 `QUEUED_BYTES` 照旧管着全部）。两个数都是 `udp.rs` 里的常数，**不设测试用的旋钮**：时限的边界由暂停时钟的单元用例测（第 10 节第 3 条），端到端的用例不等它（只在"只发第一个数据报"那一例里等它过去，有界）。

持着的不满 16 个时，队列（`QUEUE`，64 个）里后来的数据报照旧排着，放了之后照常转发。

### 5.3 放了之后（M5-D8）

- **SNI**：`parse_client_hello` 答出的 `sni`（小写；没有、为空或不全是 ASCII 时是 `None`，`crates/rurge-http/src/tls.rs:34`）写进 `session.sni`，协议是 `QUIC`，之后才 `new_handle_with`。规则看会话的 SNI（`crates/rurge-rules/src/matcher.rs` 的 `domain_targets`），所以 `DOMAIN` 一族带 `extended-matching` 的行、规则集文件里一行带它的整集（M1a）对 QUIC 流按 SNI 命中；请求记录与 `/v1/requests/*` 的 `sni` 照 `crates/rurge-engine/src/observe.rs:164` 取会话的。拨号的目标照旧是这条流的目标（总设计 P4-D5），选组时的主机（`SelectCtx`）照旧是目标。
- **持着的数据报**：`forward`（`udp.rs:770`）先按原来的次序发持着的、再收队列里的（今天它只拿一个 `first`）。`open` 失败或拒绝时：拒绝（`Rejected`）的对每个持着的数据报调一次 `association.client.refused`（今天对 `first` 调一次，`drain` 对后来的每个各调一次；虚拟网卡上每次回一个 ICMP"管理上禁止"，3 秒至多 100 个），失败（`Failed`）的照旧什么也不告诉客户端；然后照旧 `drain`（`udp.rs:838`）。`forward` 失败之后的 `RETRY_AFTER` 不变。
- `open`（`udp.rs:602`）多一个参数：这条流的 ClientHello 读到了没有（第 6 节用）。

### 5.4 计数与备注（M5-D9、D10）

每条认出 QUIC、走到 `new_handle_with` 的流在下面三个里恰好计一次：

| 计数 | 情形 | 记录的备注（`add_note`） |
| ---- | ---- | ------------------------ |
| `hello` | 拼齐了且 `parse_client_hello` 读得出（有没有 SNI 都算） | 不写 |
| `incomplete` | 300 毫秒到了、或持着的到了 16 个，还没有拼齐 | `No SNI: the QUIC ClientHello did not arrive whole` |
| `unreadable` | `GaveUp`，或拼齐了而 `parse_client_hello` 读不出 | `No SNI: the QUIC ClientHello could not be read` |

另有 `blocked`：被 `auto-quic-block` 阻断的流（第 6 节），都在 `hello` 里。关联在持住时结束的流不计。四个数在引擎里（与 `SniffCounts` 并排，`serve.rs:84`），从引擎启动起累计，重载不清。没有读到时 DEBUG 一行说为什么（读不成的原因是 `GaveUp` 的字符串）；INFO 与以上的行不多写什么（会话日志行照旧没有 SNI，`crates/rurge-engine/src/engine.rs` 的 `log_session`）。

### 5.5 行为的变化（登记）

- QUIC 流的规则看得到 SNI：带 `extended-matching` 的域名规则从此对 QUIC 流按 SNI 命中，以前落到后面的行（多半是 `FINAL`）的 QUIC 流可能改走别的策略（清单 L265、L249 改写）。
- ClientHello 跨数据报的 QUIC 流要等后面的数据报到了才求规则、拨号；凑不齐的等 300 毫秒或 16 个数据报。
- 请求记录与 `/v1/requests/*` 里的 QUIC 流有了 `sni`。
- `auto-quic-block` 生效（第 6 节）：Profile 里 `[MITM]` 有名单、有可用的 CA、`mitm` 开着时，发往名单里主机的 HTTP/3——回落之后的 TCP 会被解开的那些——被阻断、浏览器回落到 TCP。这是手册写的行为，对今天的使用者是新的（清单 L584）。

## 6. `auto-quic-block`（M5a）

手册（`http/mitm.html`，项目 2026-10-28 的文本）："When a QUIC connection (i.e. HTTP/3) hits the MITM hostname list, Surge automatically blocks that QUIC connection, causing the client to fall back to HTTP/2 or HTTP/1.1 so the traffic can be intercepted by MITM."；同一页的 `hostname`："For TLS or QUIC connections identified by the sniffed SNI, a bare entry matches regardless of the connection's actual port." 缺省是 true。

### 6.1 条件（M5-D11）

一条 QUIC 流在下面几条都成立时被阻断：

1. 它的 ClientHello 读到了（第 5.4 节的 `hello`）。凑不齐、读不成的不阻断（总设计第 11 节）。
2. 这一代配置的 `[MITM] auto-quic-block` 为真。
3. 回落之后的 TCP 会被解开：**同一个目标、同一个端口上的一条 TCP 会话，带着同一个 ClientHello 去掉 ALPN，会被嗅探、并被解密**。两步，都照 TCP 一侧今天的代码：
   - 会被嗅探：照 `sniffable_session`（`serve.rs:178`）——入口是客户端的入口（HTTP、SOCKS5、虚拟网卡；传输一项不看，今天送 UDP 流进 `run_flow` 的只有 SOCKS5 的 UDP 关联与虚拟网卡）；fake 地址背后有名字（`named_at_a_proxy_port`）；不在 `always-raw-tcp-hosts` 里、不含 `always-raw-tcp-keywords` 的词（`raw_listed`，`serve.rs:63`）；端口是 80 或 443、或 `force-http-engine-hosts` 加的、或 `[MITM]` 放宽到的（`widened_by_mitm`，M2 设计第 7.6 节）。暂停与预匹配的探测不问：暂停的引擎在 `open` 的开头就拒绝了这条流（`udp.rs:616`），走不到这里；预匹配管的是 TCP 先答后读之前的拒绝，这条流已经求过规则。
   - 会被解密：照 `Engine::decrypting`（`serve.rs:328`）的次序——名单（有 SNI 时按 SNI，不带端口的项不看端口；没有 SNI 时按目标，不带端口的项只对 443，第 6.4 节）、`mitm` 开关、这一代有可用的 CA、`client-source-address` 允许这个来源（`source_of`：虚拟网卡与回环的来源算 `127.0.0.1`）、名字签得出证书。

ALPN 不看：QUIC 的 ALPN 是 `h3`，回落到 TCP 之后客户端给的是 `h2` 与 `http/1.1`，那时由 TCP 一侧照常判断。实现上 `sniffable_session` 里入口、名字、两张名单与端口的那几步抽成一个私有的方法（名字计划定），`sniffable_session` 与 `auto_quic_blocks` 共用；`auto_quic_blocks(session)` 在第 2 条成立时先问它，再调 `decrypting(session, &ClientHello { sni: session.sni.clone(), alpn: Vec::new() })`，答 `Ok` 就阻断：同一段代码，两侧的条件就不会分叉。`decrypting` 的两行 DEBUG（没有可用的 CA、ALPN 不合）照旧可能写第一行，第二行在空的 ALPN 下不会出现。

这样定的两个结果（构建者检查问的两例）：

- **名单与 `always-raw-tcp-hosts` 都有的主机**不阻断：回落之后的 TCP 原样转发、不解开，阻断只让客户端慢。
- **不带端口的项、HTTP/3 在 TCP 一侧不嗅探的端口上**（例如名单写 `example.com`、HTTP/3 在 8443，没有 `force-http-engine-hosts` 或写明端口的项把 8443 加进来）不阻断：TCP 一侧不带端口的项不让别的端口可嗅探（M2 设计第 7.6 节，M2 第 16 节登记的差异），回落之后的 8443 不会被解开。手册的 `hostname` 说不带端口的项对看得见 SNI 的 QUIC 不论端口都算（上面的引文）：rurge 在 TCP 一侧本来就与它不同，QUIC 跟着 TCP。登记为与 Surge 的差异（清单 L584）；要阻断，写明 `:8443` 或 `:0`，两侧一起生效。代所有者作的裁定（第 15 节）。

### 6.2 位置与阻断的方式（M5-D12）

在 `open` 里，选好策略、`REJECT` 一类的终端已经拒绝（`resolution.terminal == TerminalKind::Reject` 那一处）之后，`block-quic` 的判定（`quic_verdict`，`udp.rs` 里写 `QUIC blocked` 的那一段之前）之前：

- 记录的说明（`set_error`）：`QUIC blocked by auto-quic-block`（`block-quic` 的照旧是 `QUIC blocked`）；
- 结局 `Rejected(RejectKind::Reject)`：虚拟网卡上每个丢掉的数据报回一个 ICMP"管理上禁止"、客户端立刻回落，SOCKS5 上只能丢（同 `block-quic`，阶段 3 / M5 设计第 6.1 节）；
- 计数 `blocked` 加一；
- 记录照旧带规则与策略：放在规则之后，使用者看得到这条流命中了哪一行、本来要走哪条策略。

### 6.3 与 `block-quic`、自适应的表（M5-D13）

- 两者各自独立：任一个要阻断就阻断。`auto-quic-block` 先判，两者都要阻断时说明写它。全局的 `block-quic = always-allow` 不放过 `auto-quic-block` 要阻断的流：手册说 `always-allow` "Overrides the proxy policies' `block-quic` parameter, allowing everything."（`docs/reports/2026-10-14-manual-baseline-recheck.md` 的 R29），说的是各策略的 `block-quic` 参数，没有说 `[MITM]` 的这个键；不想阻断名单里主机的 QUIC，写 `auto-quic-block = false`。登记为 rurge 的读法（清单 L177、L584）。
- 被 `auto-quic-block` 阻断的流不碰自适应的表（不调 `QuicAdaptive::on_quic`）：它不是试探，每次都阻断。表照旧按目标记（阶段 3 / M5 设计第 6.3 节），不改成按 SNI：总设计没有要求，表的两边（QUIC 流与客户端的 TCP 会话）要一起改，不在 M5 的范围。

### 6.4 没有 SNI 的 ClientHello（M5-D14）

读到了 ClientHello 而它没有 SNI（客户端按地址连，或名字不全是 ASCII）时，名单按目标比，不带端口的项只对 443——同 TCP 上没有 SNI 的 TLS（总设计第 7.1 节第 4 条）。总设计第 11 节只写了"名单按 SNI 命中"；这样做让"QUIC 流被阻断"与"回落之后的 TCP 被解密"用同一个条件（第 6.1 节）：阻断了的，回落之后都会被解开。订正列在第 13 节。

## 7. API 与日志（M5a）

`GET /v1/rurge/http` 多一个键 `quic`（在 `rewrite` 之后）：

```json
"quic": { "hello": 12, "incomplete": 0, "unreadable": 1, "blocked": 3 }
```

四个数的意思见第 5.4 节。`docs/api/phase4.md` 写形状、四个数、两种备注、一种说明与 DEBUG 的行。`/v1/requests/*` 的形状不变（QUIC 流的 `sni` 有了值，`notes` 可能多一句）。

## 8. 基准（M5b）

### 8.1 怎么跑（M5-D16）

- 都是忽略的用例（`#[ignore = "a measurement, not a gate: …"]`），放在引擎的一个新的用例文件里（`crates/rurge-engine/tests/` 下，名字计划定），各自打印数字，不作门禁。CI 的 `cargo test` 编译它们、不跑。
- 用 release 跑：`cargo test -p rurge-engine --release --test <文件> -- --ignored --exact <名字> --nocapture`。这是引擎的用例第一次用 `-p` 跑，同阶段 3 的 `rurge-tun` 基准（`crates/rurge-tun/src/tests/bench/mod.rs` 开头写着的例外）：只编引擎与它的依赖的一份 release。跑之前 `df -h /d`，剩不到 20 GiB 就停下报告（2026-11-02：`target/debug` 124 GiB、`target/release` 1.6 GiB、D: 剩 34 GiB）。
- 单线程：客户端、rurge 与源站都在一个当前线程的运行时里（同 `crates/rurge-engine/src/relay.rs` 的 `relay_throughput`），NFR-01 说的是单核，各方的开销加在一起。每一项另量一个不经 rurge 的基线（客户端直接连源站），写出两者与它们的差。
- 每一项至少 5 轮、取中位数；延迟给中位数、p99 与最大。
- 只用回环与端口 0；名字经 `[Host]` 钉到 `127.0.0.1`，另有一条按地址的规则（`CLAUDE.md`「测试约定」）。

### 8.2 B1：明文 HTTP/1.1 经引擎

- 入口：HTTP 监听器的绝对形式请求（它们总进引擎，M1b）；源站是回环上一个 keep-alive 的 HTTP/1.1 服务器；规则只有 `FINAL,DIRECT`。
- 吞吐：一个 256 MiB 的应答正文（带 `Content-Length`）下行，一个 256 MiB 的请求正文上行，Gbit/s。对照基线、`relay_throughput` 在同一台机器上的数（`pump` 经 TCP）与 NFR-01 的"本地回环 TCP 直连转发单核 ≥ 1 Gbps"。
- 每请求延迟：一条 keep-alive 连接上依次 20,000 个 `GET`，应答正文 16 字节；引擎多出的是两者中位数之差。

### 8.3 B2：解密之后

- 入口：经 SOCKS5 连一个名单里的名字，客户端只信测试的 CA；源站是回环上的 TLS 服务器，证书由 `EngineShared.roots` 信任（同 M2 的端到端）。
- 两行：客户端与上游都讲 HTTP/1.1；都讲 `h2`（`[MITM] h2 = true`）。量 B1 的两项，基线是客户端直接与源站讲 TLS。

### 8.4 B3：一千条规则下每请求求值

B1 的延迟一项，Profile 换成 `FINAL` 之前有 1,000 条都不命中的规则：`DOMAIN-SUFFIX`、`DOMAIN-KEYWORD`、`URL-REGEX`、`USER-AGENT` 各 250 条（`URL-REGEX` 与 `USER-AGENT` 逐条求值，`DOMAIN-SUFFIX` 走索引）。与 B1 的中位数之差就是这一千条在每个请求上的开销。对照 NFR-01"10 万条规则集下单次规则匹配 p99 < 50 µs"：那一条量的是 `rurge-rules` 的单次匹配（criterion 基准 `evaluate_1000_rules_3x100k_sets_miss`），这里是引擎里一个请求实际付的。

### 8.5 B4：叶证书

`Authority::issue` 是 `pub(crate)`（`crates/rurge-http/src/mitm/authority.rs:446`），不为基准放开：经 SOCKS5 与 1,000 个不同的名字（`n0000.bench.test` 起，名单 `*.bench.test`；1,000 恰好是叶证书缓存的容量 `LEAF_SLOTS`，`authority.rs:59`）各做一次 TLS 握手——只握手、不发请求：MITM 先与客户端握手、再按请求连上游（总设计 P4-D7）——，再对同样的名字各握一次。前一轮每次签一张新的叶证书，后一轮从缓存拿；两轮中位数之差就是签发的开销。

### 8.6 B5：内存（总设计第 16 节风险 F）

各自一个进程跑，读进程的峰值（同 M4a、M4b 的 `memory.rs`：Windows 的 `PeakWorkingSet64`、Linux 的 `VmHWM`；macOS 计划定，没有就照实写没有）：

- **每条连接**：1,000 条客户端连接各走完一个请求之后空闲着，三种各一轮——判为 TLS 而没有解密、原样转发的；明文、在引擎里、池里留着一条上游连接的；解密的（两侧 TLS）。读数减去同一个用例里建连接之前的读数、除以 1,000。对照 NFR-01"1 万并发连接常驻内存 < 300 MB"（每条 30 KB）：那一条当初量的是虚拟网卡的协议栈（阶段 3 的 B2），这里是经引擎的连接。
- **正文的缓冲**：一个请求带 32 MiB 减 1 字节的正文、命中一条 `[Body Rewrite]`，峰值减去之前的读数、除以正文的大小：缓冲有几份。
- **叶证书的缓存**：缓存装满（1,000 张）时的峰值减去之前的读数、除以张数。

### 8.7 Q1：读 ClientHello 的开销

`read_first` 加 `HelloReader::read` 读一个跨两个数据报的 ClientHello（第 10 节第 1 条的构造器造的、Chrome 跨包的排法），100,000 次，每条流的中位数与 p99。对照阶段 3 / M5 设计第 4.2 节估的"十微秒"量级。

### 8.8 写下来

读数写进 M5b 计划的「执行期修正记录」与 `docs/status.md`，照实写是一台 Windows 开发机上的一次运行、不是 CI；对着 NFR-01 的线写成数字，不写"通过"：阶段 4 的验收标准（总设计第 15 节）没有性能的一条，NFR-01 的线怎样用在经引擎的路径上归所有者（第 15 节）。阶段 3 的 B1、B2、b3 不重跑：它们量的是 `rurge-tun` 的协议栈，阶段 4 没有改 `rurge-tun` 的生产代码（M4b 的任务 10 只改了一条用例）。

## 9. 遗留（M5-D18）

总设计第 1.5 节登记到阶段 4 的事项，归 M1 ～ M4 的各由它们的设计收了（M1：R27、L314、拨号之前的 SNI 与协议；M3：Map Local 的路径先问 `Reach`；M4：`remoteAddr` 与 `dnsMs`，M4 设计第 18 节第 10 条）。归 M5 的与阶段 4 自己的延后事项：

- **M5a 收**：阶段 3 / M5 计划延后第 5 行（`auto-quic-block`）与第 6 行（"QUIC 里的 SNI、跨几个包的 ClientHello、按 SNI 匹配规则"）——第 4 ～ 6 节；手册基线重对计划延后第 8 行（"发行说明里 2026-10-13 之后的测试版条目（自适应的 `block-quic`、`GET /v1/policies?include_external=true`、跨多个包的 QUIC 主机名识别、QUIC v2 / draft-29）：手册还没有写到"）——总设计第 1.5 节："M5 照这一条做跨包重组；M6 重对一次发行说明"，M5a 做跨包重组（第 4 节），发行说明照旧由 M6 重对。
- **M5b 收**：M1b 计划延后第 4 行——`Idle` 第一次 `poll_frame` 才起表、`Progress::restart`（`crates/rurge-http/src/engine.rs`）没有能红的用例。用暂停时钟的单元用例补两条：一条在第一次 `poll_frame` 之前先把时钟拨过时限，断言那一次答 `Pending` 而不是 `Stalled`（把起表挪到 `Idle::new` 就变红）；一条在 `Progress` 记过进展、时钟走到快满时调 `restart`，断言 `remaining` 回到整个时限（把 `restart` 的正文去掉就变红）。`restart` 的调用处（重发之前那一处，今天 `engine.rs:2019`）要一条去掉调用就红的用例，得排出"第一条连接用掉大半个时限才失败、在新连接上重发"的那一刻：M5b 的计划核对排不排得出（V9），排不出就照实写进它的延后事项。
- **不收**（各自的去向不变）：M1b 延后第 1 行（`FakeDoq` 的那一次，再见到时先分清）、第 2 行（升级之后清池，"可以考虑"）、第 3 行（两路的次序，归所有者）；M2a 延后第 1、2 行（归所有者）、第 3、4 行（等得到能排出那一刻的写法或 `AppTcp` 的改动）、第 5 行（"需要时"再给 `MitmShared` 一个能拨的时钟）；M2c 延后第 1 行（归所有者）。M4a、M4b 的延后事项是空的。归所有者、与本阶段相交的（总设计第 1.5 节末行）照旧列在总设计第 17 节，其中阶段 3 / M5 计划延后第 4、16 行与 M5 有关的新事实在第 15 节。

## 10. 测试

沿用 `CLAUDE.md`「测试约定」：只用回环与端口 0；有界等待；"什么都没发生"的固定等待写注释；新写的计时用例提交前单独循环跑二十次以上。QUIC 的用例只把数据报喂给函数、SOCKS5 的 UDP 关联与内存设备上的虚拟网卡，不完成 QUIC 握手（总设计第 14 节第 5 条）。

1. **构造器**（`rurge-net` 的 `quic::testing`，任务 1）：造出的每个 Initial 包都能被它的密钥打开、读回的帧与写进去的相同；真实的 ClientHello 由 rustls 的 QUIC 客户端写（`ClientConnection::new_with_alpn` 与 `write_hs`）——ring 的提供者没有 ML-KEM，用一长串 ALPN 把它撑到一个包装不下（1.5 ～ 2 KB）；Chrome 的两种排法：单包里切成多个 CRYPTO 帧、乱序、夹 PING 与 PADDING；跨包——第一个包放开头 55 ～ 86 字节与结尾，中间放到第二个数据报。随机的部分用固定的种子，失败时打印种子。
2. **读 ClientHello**（`rurge-net` 的 `quic::initial`，任务 2）：
   - RFC 9001 附录 A.2 与 RFC 9369 附录 A.2 的客户端 Initial：一个数据报就 `Whole`，交出的 241 字节以 `01 00 00 ed` 开头、含 `example.com`（RFC 9001 附录 A.2 的 CRYPTO 帧：偏移 0、长度 241，握手类型 1、长度 237，包号 2【原文】）；
   - 跨两个数据报：按偏移的次序；与数据报的次序相反（第一个数据报带后半段、第二个带前半段）；Chrome 跨包的排法；单包里打散的；两个 Initial 合并在一个数据报；Initial 之后合并一个 0-RTT 样的长包头包（按 Length 跳过）；尾部填零；后来的数据报的目的连接 ID 不同而用同一把密钥封的（打得开）；
   - 重发：同一段 CRYPTO 再来一次（相同的字节）照样 `Whole`；部分重叠、重叠处相同的两段合成一段；
   - 段数：互不相接的 32 段照样往下读，第 33 段读不成；
   - 读不成：重叠的字节不同、CONNECTION_CLOSE、Initial 不许有的帧（如 `0x08` STREAM）、截断的帧、第一个字节不是 1、长度加 4 超过 64 KiB、偏移加长度超过 64 KiB；
   - 不算读不成（答 `Partial`）：后来的数据报里打不开的 Initial、别的版本的包、以短包头开头的数据报；
   - 包号：RFC 9000 附录 A.3 的例子（最大 `0xa82f30ea`、截断的 `0x9b32`、16 位 → `0xa82f9b32`）；第二个包用 1 字节的包号 1 打得开；
   - 分类：`is_client_initial` 现有的用例原样通过（第 4.3 节）；
   - quinn 的真客户端：一串长的 ALPN 让它的 ClientHello 跨两个数据报，从只收不答的回环套接字截下前几个数据报，读得出它连的名字（同现有的 `a_real_quinn_client_initial_is_one`，不完成握手）。
3. **持住**（`rurge-engine` 的 `udp.rs`，任务 3）：
   - 暂停时钟的单元用例：持住在第二个数据报来时放、在 300 毫秒到时放（299 毫秒时还持着）、在第 16 个数据报时放、关联结束时不放而结束；
   - 经 SOCKS5 的 UDP 关联：两个数据报连着发（中间不等），`DOMAIN,<名字>,REJECT,extended-matching` 按 SNI 拒绝、记录的 `sni` 是那个名字；不带 `extended-matching` 时不命中；放行时回环上收数据报的服务器按原来的次序收到全部；只发第一个数据报时它在 300 毫秒之后照样到达、记录没有 SNI、备注是 `incomplete` 那一句、计数 `incomplete` 加一；读不成的一例（备注与计数）；
   - 名字经 `[Host]` 钉到 `127.0.0.1`，另有按地址的规则作第二道防线；RFC 样例里的 `example.com` 同样。
   - 现有的用例：引擎用例的 `quic_initial()`（`crates/rurge-engine/tests/common/mod.rs:604`，`client_initial(1, DCID)`）今天只有 PADDING，M5a 之后它会被持住 300 毫秒、记录多一句 `incomplete` 的备注。它改成一个数据报装得下、不带 SNI 的完整 ClientHello（rustls 写的），用它的现有用例（`tests/udp.rs` 与 `tests/tun.rs` 的 `a_blocked_quic_flow_answers_with_icmp`）照旧不等、不多备注；`rurge-net` 的 `client_initial` 照旧只有 PADDING，给分类的用例用。要看的现有用例，计划列一张表。
4. **`auto-quic-block`**（任务 4）：Profile 有可用的 CA（照 M2 的端到端的写法）。
   - 经 SOCKS5：名单命中时被阻断（说明、`Rejected`、`blocked` 加一、收数据报的服务器什么也没收到）；`auto-quic-block = false`、`mitm` 关、没有 CA、来源不允许、名单排除（`-` 开头）各不阻断；带端口的项（`<名字>:8443`）只对那个端口；不带端口的项、HTTP/3 在 8443 上不阻断；名单与 `always-raw-tcp-hosts` 都有的名字不阻断；`block-quic = always-allow` 时照样阻断；`REJECT` 规则先于它；没有 SNI 的 ClientHello 按目标（443 与别的端口各一例）；凑不齐的不阻断；
   - 经内存设备上的虚拟网卡：跨两个数据报的 ClientHello 被阻断，每个数据报回一个 ICMP"管理上禁止"（同 `crates/rurge-engine/tests/tun.rs` 的 `a_blocked_quic_flow_answers_with_icmp`）。
5. **API**（任务 5）：`/v1/rurge/http` 的两处逐字节的用例（`crates/rurge-api/tests/api.rs` 从第 1607 行起的那一条、`crates/rurge-engine/tests/sniff.rs` 里的那一处）加 `quic`。
6. **基准与遗留**（M5b）：第 8、9 节。

## 11. 文档

| 文档 | M5a | M5b |
| ---- | --- | --- |
| `docs/api/phase4.md` | `/v1/rurge/http` 的 `quic`；两种备注、一种说明、DEBUG 的行 | — |
| `docs/surge-compatibility-matrix.md` | L584 的状态与备注（生效；第 4.7、6.3、6.4 节的读法与差异）；L249、L265 加 QUIC 的 SNI；L177 写与 `auto-quic-block` 的关系 | — |
| `docs/testing.md` | QUIC 的用例只喂数据报、不完成握手；持住的时限在暂停时钟的单元用例里测 | 基准的命令（引擎的 `-p` 例外与磁盘的检查） |
| `docs/acceptance/phase4-manual.md` | 三个平台的真浏览器（Chrome、Firefox）经虚拟网卡：HTTP/3 发往名单里的主机被阻断、回落之后被解密；`auto-quic-block = false` 时 HTTP/3 照通；按 SNI 分流的 QUIC；记录里的 SNI | — |
| `docs/status.md` | 阶段 4 / M5a 一节 | 阶段 4 / M5b 一节（含读数） |
| `README.md`、`README_en.md` | 状态、特性表（`auto-quic-block`、QUIC 的 SNI），两份意思一致 | 状态 |
| `docs/requirements.md` | 第 7 节阶段 4 的状态 | 同 |
| `CLAUDE.md` | 「现在在哪里」 | 同 |
| `docs/README.md` | 本设计与 M5a 计划各一项（随各自的提交） | M5b 计划一项 |

## 12. 写计划时要核对的事项

| 编号 | 事项 |
| ---- | ---- |
| V1 | `udp.rs` 的 `run_flow`、`open`、`forward`、`drain` 与 `refused` 的调用处（第 5 节引的行号）；`Queued` 与 `QUEUED_BYTES` 怎样记持着的数据报 |
| V2 | rustls 0.23.43 的 `quic` 接口：`Suite::keys`（`quic.rs:892`）、`DirectionalKeys`（`:591`）、`HeaderProtectionKey::decrypt_in_place`（`:707`）、`PacketKey::decrypt_in_place`（`:760`）、`ClientConnection::new_with_alpn`（`:163` 起的 `new` 之下）与 `write_hs`（`:430`）；打开一个包与 `initial.rs` 的 `opens` 共用哪些；`HelloReader` 是 `Send` |
| V3 | `decrypting`（`serve.rs:328`）与从 `sniffable_session`（`serve.rs:178`）抽出的入口、名字、两张名单与端口那几步，在 UDP 流的会话上是不是同样成立：`named_at_a_proxy_port` 与 `named_target` 对 SOCKS5 的 UDP 关联与虚拟网卡（`run_flow` 已经 `turn_back` 过）、`widened_by_mitm`、`source_of` 对两种来源、两行 DEBUG |
| V4 | `HttpCounts`（`serve.rs`）、`Engine::http_counts`（`engine.rs:1576`）、`crates/rurge-api/src/routes/rurge_http.rs` 与两处逐字节的用例 |
| V5 | 测试夹具：`crates/rurge-engine/tests/common/mod.rs` 的 `udp_associate`、内存设备上的 UDP 与 ICMP（`tests/tun.rs` 的 `a_blocked_quic_flow_answers_with_icmp`）、Profile 里可用 CA 的写法（`tests/mitm.rs`）、`[Host]` 能不能写通配 |
| V6 | `quic::testing` 能不能被引擎与 API 的用例用（`rurge-net` 的 `testing` 特性在它们的 dev-dependencies 里开着没有）；RFC 两份样例挪过去之后 `initial.rs` 的用例怎样引用 |
| V7 | M5b：HTTP 监听器的绝对形式进引擎的那一路、`EngineShared.roots`、叶证书缓存的容量（第 8.5 节）、`relay_throughput` 的读法、峰值内存的读法在 macOS 上有没有、磁盘 |
| V8 | `Cargo.lock` 的包数（今天 520） |
| V9 | M5b：`Idle` 与 `Progress` 在 `rurge-http` 的单元用例里怎样建（`Idle::new` 要 `Meter`、`SessionHandle`）；重发之前那一处 `progress.restart()`（今天 `engine.rs:2019`）排不排得出去掉调用就红的用例（第 9 节） |

## 13. 对总设计与前面设计的订正

1. 总设计第 11 节"名单按 SNI 命中"：有 SNI 时按 SNI；读到的 ClientHello 没有 SNI 时按目标，不带端口的项只对 443（M5-D14）。
2. 总设计第 11 节"至多若干个包、若干毫秒与 64 KiB，M5 定"：16 个数据报、从第一个数据报起 300 毫秒、64 KiB（M5-D6、D7）；"等待的上限与凑不齐时按没有 SNI 求值，M5 定"：凑不齐时按没有 SNI 求规则、不阻断、记录写备注（第 5.4 节）。
3. 总设计第 14 节第 5 条"只把包喂给判断函数与内存设备"：另有经 SOCKS5 的 UDP 关联，与 quinn 客户端在回环上真发出的数据报（不完成握手）。
4. 总设计第 14 节第 7 条与风险 F：基准是 B1 ～ B5 与 Q1（第 8 节）；叶证书签发的耗时由握手量出（第 8.5 节）。
5. 总设计第 1.4 节 M5 的"遗留事项里落到本阶段而前四个里程碑没有收的"：阶段 3 / M5 计划延后第 5、6 行与手册基线重对计划延后第 8 行（M5a）、M1b 延后第 4 行（M5b），见第 9 节。
6. 阶段 3 / M5 设计第 4.2 节"解开之后不再看里面的帧……SNI 留给阶段 4"、第 4.4 节"阶段 4 取 QUIC 里的 SNI 要从同一处解出的明文接着读"：照做；分类的判据不变（M5-D3）。
7. 总设计第 11 节第三条"第 7.1 节的条件（`mitm` 开、有可用的 CA、来源允许、名单按 SNI 命中——不带端口的项不看端口）都成立……时阻断"：另加一条，回落之后的 TCP 会被嗅探（入口、名字、`always-raw-tcp-hosts` 与 `always-raw-tcp-keywords`、端口，第 6.1 节）。所以"不带端口的项不看端口"在 QUIC 一侧同 TCP 一侧，只在 TCP 一侧嗅探的端口上成立（M2 设计第 7.6 节）（M5-D11）。

## 14. 已决事项

| 编号 | 事项 | 决定 |
| ---- | ---- | ---- |
| M5-D1 | 计划 | 两份：M5a（QUIC 的 ClientHello 与 `auto-quic-block`）→ M5b（基准与遗留），M5b 的计划在 M5a 合并之后写（第 2 节） |
| M5-D2 | 分工 | `rurge-net::quic::initial` 读包、读帧、拼 CRYPTO、看握手消息的头；`rurge-http::tls::parse_client_hello` 读 ClientHello；引擎接起来；不加新包、不加工作区的边（第 3 节） |
| M5-D3 | 分类 | 不变；`is_client_initial` 由 `read_first` 定，现有用例原样通过（第 4.3 节） |
| M5-D4 | 一个数据报 | 合并的包逐个看；后来的数据报不要求 1200 字节、不比目的连接 ID；打不开的包跳过（第 4.4 节） |
| M5-D5 | 帧与包号 | 只认五种帧，CONNECTION_CLOSE 与别的帧读不成；包号照 RFC 9000 附录 A.3 还原（第 4.4 节） |
| M5-D6 | 拼接 | 只存到了的字节，按偏移的几段，相接的合成一段、至多 32 段；重叠的字节必须相同；`[0, 4 + 长度)` 都到了就交出；64 KiB（第 4.5 节） |
| M5-D7 | 持住 | 拼齐或读不成就放，否则至多 300 毫秒、16 个数据报；不设测试的旋钮（第 5.2 节） |
| M5-D8 | 放了之后 | SNI 写进会话；持着的数据报按次序发；拒绝时每个各告诉客户端一次（第 5.3 节） |
| M5-D9 | 计数 | `quic` 的四个数（第 5.4、7 节） |
| M5-D10 | 备注 | 没有读到的写两种之一；读到了而没有阻断的不写原因（第 5.4 节） |
| M5-D11 | 条件 | 读到了、`auto-quic-block` 为真、同一个目标与端口上的 TCP 会话带着去掉 ALPN 的同一个 ClientHello 会被嗅探并解密——`sniffable_session` 的入口、名字、名单与端口几步与 `decrypting` 共用（第 6.1 节） |
| M5-D12 | 位置 | 规则与 `REJECT` 之后、`block-quic` 之前；`Rejected(Reject)`；说明 `QUIC blocked by auto-quic-block`（第 6.2 节） |
| M5-D13 | 与 `block-quic` | 各自独立，`always-allow` 不放过它；不碰自适应的表（第 6.3 节） |
| M5-D14 | 没有 SNI | 按目标，同 TCP（第 6.4 节） |
| M5-D15 | 不做 | 不完整的 ClientHello 里的 SNI、gQUIC、Retry 之后的密钥（第 4.7 节） |
| M5-D16 | 基准的跑法 | 引擎的一个忽略的用例文件、`-p rurge-engine --release`、单线程、每项有基线、先查磁盘（第 8.1 节） |
| M5-D17 | 基准的内容 | B1 ～ B5、Q1；写成数字、不作门禁、不重跑阶段 3 的三项（第 8.2 ～ 8.8 节） |
| M5-D18 | 遗留 | M5a 收阶段 3 / M5 计划延后第 5、6 行与手册基线重对计划延后第 8 行的跨包重组；M5b 收 M1b 延后第 4 行（两条单元用例，调用处看 V9）；其余的去向不变（第 9 节） |

## 15. 留给项目所有者的

1. **gQUIC**（阶段 3 / M5 计划延后第 16 行，归所有者）多了一条事实：Surge iOS 5.14.6 的版本说明"Supports SNI extraction for gQUIC."（App Store 的版本历史，调研的助手读了原文）。rurge 照旧不认 gQUIC；要不要认归所有者。
2. **从不完整的 ClientHello 里取 SNI**（Surge iOS 5.17.0 的说法）：rurge 不做（M5-D15）；要不要做。
3. **`auto-quic-block` 与 `block-quic = always-allow`**：M5-D13 让前者不受后者管，是代所有者作的裁定。
4. **没有 SNI 的 ClientHello 按目标**（M5-D14）：代所有者作的裁定。
5. **`auto-quic-block` 跟着 TCP 一侧**（M5-D11）：`always-raw-tcp-hosts` 里的主机不阻断；不带端口的项只在 TCP 一侧本来就嗅探的端口上阻断（80、443 与 `force-http-engine-hosts` 加的；TCP 一侧不带端口的项不让别的端口可嗅探，M2 设计第 7.6 节）。手册说不带端口的项对看得见 SNI 的 QUIC 不论端口都算；照手册阻断 8443 上的 HTTP/3，回落之后的 TCP 也不会被解开。代所有者作的裁定；要不要让 TCP 与 QUIC 两侧都照手册（不带端口的项在任何端口上解密），归所有者。
6. **NFR-01 的线怎样用在经引擎的路径上**：第 8 节的读数写成数字；"本地回环 TCP 直连转发单核 ≥ 1 Gbps"管不管经 HTTP 引擎的明文与解密的路径、"1 万并发连接常驻内存 < 300 MB"管不管经引擎的连接，归所有者（阶段 3 / M5 计划延后第 4 行 B2 的保守读数照旧归所有者）。
7. 总设计的 Q1 ～ Q6 照旧，M5 不改变它们。

## 16. 计划与执行期的订正

写 M5a 计划时（2026-11-02）改的三处：

1. 第 2 节的任务 6 分成 6a 与 6b，各一次评审：6a 是写行为的文档（手工清单、`docs/api/phase4.md`、`docs/testing.md`、兼容性清单的四行），6b 是写进度的文档（`docs/status.md`、两份 README、需求文档、`CLAUDE.md` 与本节的执行期订正）。两半的读者与核对的办法不同。
2. 第 10 节第 4 条"收数据报的服务器什么也没收到"改成看记录与客户端。不带端口的项只在 TCP 一侧嗅探的端口上成立（第 6.1 节），用例的目标因此多是钉到 `127.0.0.1` 的名字的 443 号；那个号不归用例所有，不能在那里放一个收数据报的服务器。被阻断的流断言记录的说明是 `QUIC blocked by auto-quic-block`、`quic.blocked` 加一、经用例自己的关联时每个持着的数据报各被拒绝一次；不阻断的流断言记录没有这句说明、`blocked` 不变（放行的数据报发往 `127.0.0.1:443`，在回环上，没有人收也无妨）。
3. 第 10 节第 2 条最后一项"读得出它连的名字"：`rurge-net` 不依赖 `rurge-http`，在那里读不了 ClientHello 的扩展；那条用例断言拼出的消息以握手类型 1 开头、含名字的字节。从 ClientHello 取出名字由 `rurge-http` 的 `parse_client_hello` 已有的用例与任务 3 的端到端证明。

**执行 M5a 时**：改了正文意思的只有下面一处，其余各行的订正只改计划、写法或代码，意思不变。

4. 第 4.5 节末段"所以一个读者至多多占持着的字节那么多、加 32 段的开销"：改成"至多多占持着的字节的两倍、加 32 段的开销"。CRYPTO 的段缓冲是 `VecDeque<u8>`，按倍数增长，一段的容量至多是它的字节数的两倍（修正记录第 13、14 行）。以第 13、14 行为准。

**写 M5b 计划时（2026-11-03）**改的六处：

5. 第 8.1 节第 2 条"这是引擎的用例第一次用 `-p` 跑"：不是第一次。`relay_throughput`（`crates/rurge-engine/src/relay.rs` 的库内用例）一直用 `cargo test -p rurge-engine --release --lib` 跑（`docs/testing.md` 的命令一节）；M5b 的 `--test bench` 是引擎的第二处，同一个例外（M5b 计划的全局约束）。
6. 第 8.1 节第 3 条"单线程：客户端、rurge 与源站都在一个当前线程的运行时里"：签叶证书不在那条线程上。`Authority::leaf` 把签发交给 tokio 的阻塞线程池（`crates/rurge-http/src/mitm/authority.rs` 的 `spawn_blocking`），所以 B2、B4 与 B5 的叶证书一项里签发是另一条线程做的，B4 两遍之差是那条线程上的签发加两次交接；其余照旧在一条线程上（M5b 计划 P9 ⑥）。
7. 第 8.5 节 B4：客户端不恢复会话（rustls 的 `Resumption::disabled()`），每次握手断言是完整的握手。MITM 存着 `SESSION_SLOTS`（1,024）个会话，客户端若恢复，第二遍做的是会话恢复、不发证书，量不到缓存（M5b 计划 P9）。
8. 第 8.6 节 B5：
   - 每一项各是一条用例、各在自己的进程里，经 rurge 的那几项的直连基线也是：峰值只涨不落，同一个进程里量两项，后一项的读数含前一项的峰值。
   - 每条连接的三种之外有两条直连的基线（明文、TLS）；正文的缓冲之外有两条基线（经 rurge 而不改写、直连）。
   - 每个读数同时打出峰值与当前的工作集（Windows 的 `WorkingSet64`、Linux 的 `VmRSS`）。
   - 叶证书一项"之前的读数"在用同一个名字握过 1,000 次之后读：请求记录那时已经装满，后面 1,000 次握手留下的记录各挤掉一条旧的，差里不含记录（M5b 计划 P10）。
9. 第 8.4 节 B3"与 B1 的中位数之差"：B3 在同一个进程里另建一个只有 `FINAL` 的引擎，两者各轮轮流量，差取这两者之间的，不拿 B1 那一次运行的数（M5b 计划 P7）。
10. 第 8.1 节最后一条"名字经 `[Host]` 钉到 `127.0.0.1`，另有一条按地址的规则"：第二道防线改成共用夹具的 `dns-server`（回环上的 `MockDns`，它只答 `target.test` 与 `alt.test`）。Profile 的最后一行总是 `FINAL,DIRECT`；按地址的规则要不带 `no-resolve` 才看得到名字背后的地址，那样每个请求多一次查找，B3 的一千条之外也多出别的规则（M5b 计划的全局约束）。

**执行 M5b 时**：没有改正文意思的地方。上面第 5 ～ 10 条执行时都照做，没有出入。第 16 行订正的是计划 P9 ⑤ 的变异写法（B4 的 `handshake_kind` 断言红不了的原因在客户端一侧的两层：票据在握手之后才到、客户端缺省的存储至多留 31 个服务器；不是服务端不发票据），不是正文。第 7 条关掉恢复的理由照旧对：B4 量的是完整握手，客户端关着恢复才量得到缓存。任务 5 直接起编好的测试二进制、外面套 GNU 的 `timeout`，是第 8.1 节那条命令的等价写法（`cargo` 的 `--` 之后那些参数不变，工作目录 `crates\rurge-engine`；修正记录第 19 行）。派工单给的理由（套在 `cargo` 外面的 `timeout` 只结束 `cargo`）没有试过，照 cargo 的源码多半不成立（修正记录第 29 行）。
